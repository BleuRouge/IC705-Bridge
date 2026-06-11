//! API HTTP locale (127.0.0.1:8765) — passerelle pour les scripts Python.
//!
//! Endpoints :
//! - `GET  /status` → état de la connexion ;
//! - `POST /civ`     → `{ "frame": "FE FE A4 E0 03 FD" }` → `{ tx, response }` ;
//! - `GET  /stream`  → flux SSE : une trame CI-V reçue (hex) par événement ;
//! - `GET  /`        → aide rapide.

use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tower_http::cors::CorsLayer;

use crate::commands::CivResult;
use crate::state::{AppState, StatusSnapshot, API_ADDR};
use crate::util::{parse_hex, to_hex};

#[derive(Deserialize)]
struct CivRequest {
    frame: String,
}

#[derive(Serialize)]
struct ApiError {
    error: String,
}

fn err(code: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (code, Json(ApiError { error: msg.into() }))
}

async fn index() -> impl IntoResponse {
    "IC705 Bridge — API locale\n\n\
     GET  /status        -> état de la connexion\n\
     POST /civ {frame}   -> envoi d'une trame CI-V hex, renvoie {tx, response}\n\
     GET  /stream        -> flux SSE des trames CI-V reçues (une trame hex par événement)\n"
}

async fn status_handler(State(st): State<Arc<AppState>>) -> Json<StatusSnapshot> {
    Json(st.snapshot())
}

async fn civ_handler(
    State(st): State<Arc<AppState>>,
    Json(req): Json<CivRequest>,
) -> Result<Json<CivResult>, (StatusCode, Json<ApiError>)> {
    let bytes = parse_hex(&req.frame).map_err(|e| err(StatusCode::BAD_REQUEST, e.to_string()))?;

    let guard = st.session.lock().await;
    let session = guard
        .as_ref()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "non connecté à l'IC-705"))?;

    let response = session
        .send_civ(&bytes)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(CivResult {
        tx: to_hex(&bytes),
        response: to_hex(&response),
    }))
}

/// Flux SSE de toutes les trames CI-V reçues de la radio (une trame hex par
/// événement). Conçu pour les clients « streaming » (waterfall, monitoring) qui
/// ne peuvent pas passer par le `/civ` requête/réponse sous transceive continu.
async fn stream_handler(State(st): State<Arc<AppState>>) -> Response {
    // On capture un abonné au flux CI-V puis on relâche le verrou de session :
    // le `broadcast::Receiver` vit indépendamment de la durée de la requête.
    let rx = {
        let guard = st.session.lock().await;
        match guard.as_ref() {
            Some(session) => session.subscribe(),
            None => return err(StatusCode::SERVICE_UNAVAILABLE, "non connecté à l'IC-705").into_response(),
        }
    };

    let stream = BroadcastStream::new(rx).filter_map(|res| match res {
        Ok(frame) => Some(Ok::<Event, Infallible>(Event::default().data(to_hex(&frame)))),
        // `Lagged` : le client n'a pas suivi le débit, on saute simplement.
        Err(_) => None,
    });

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// Démarre le serveur HTTP local. Boucle jusqu'à l'arrêt de l'application.
pub async fn serve(state: Arc<AppState>) {
    let app = Router::new()
        .route("/", get(index))
        .route("/status", get(status_handler))
        .route("/civ", post(civ_handler))
        .route("/stream", get(stream_handler))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    match tokio::net::TcpListener::bind(API_ADDR).await {
        Ok(listener) => {
            state.set_api_running(true);
            tracing::info!("API locale démarrée sur http://{API_ADDR}");
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("API locale arrêtée : {e}");
                state.set_api_running(false);
            }
        }
        Err(e) => {
            tracing::error!("impossible de démarrer l'API locale sur {API_ADDR} : {e}");
        }
    }
}
