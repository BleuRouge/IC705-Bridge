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
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::sse::{Event, KeepAlive, Sse},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

use crate::commands::CivResult;
use crate::error::BridgeError;
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

fn bridge_err(e: BridgeError) -> (StatusCode, Json<ApiError>) {
    let code = match &e {
        BridgeError::InvalidFrame(_) | BridgeError::Protocol(_) => StatusCode::BAD_REQUEST,
        BridgeError::NotConnected => StatusCode::SERVICE_UNAVAILABLE,
        BridgeError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
        BridgeError::Io(_) | BridgeError::InvalidCredentials | BridgeError::AuthFailed(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    err(code, e.to_string())
}

/// Hôtes acceptés dans l'en-tête `Host` (doivent correspondre à [`API_ADDR`]).
const ALLOWED_HOSTS: [&str; 2] = ["127.0.0.1:8765", "localhost:8765"];

/// En-tête réclamé sur les endpoints sensibles. Une page web ne peut pas
/// l'ajouter en cross-origin sans déclencher un préflight (que l'absence de
/// CORS fait échouer) : défense en profondeur par-dessus la garde `Host`.
const AUTH_HEADER: &str = "x-ic705-bridge";

/// Garde locale : (1) anti-DNS-rebinding — l'API n'écoute que sur la loopback,
/// mais une page web malveillante peut la viser via un nom qui résout vers
/// 127.0.0.1, d'où le rejet de tout `Host` non loopback ; (2) en-tête
/// `X-IC705-Bridge` exigé sur `/civ` et `/stream` pour bloquer un envoi de
/// trame (PTT inclus) depuis un navigateur. La librairie Python l'envoie.
async fn guard_local_host(req: Request, next: Next) -> Response {
    let allowed = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| ALLOWED_HOSTS.contains(&h));
    if !allowed {
        return err(StatusCode::FORBIDDEN, "hôte non autorisé").into_response();
    }

    let path = req.uri().path();
    let sensitive = path == "/civ" || path == "/stream";
    if sensitive && req.headers().get(AUTH_HEADER).is_none() {
        return err(
            StatusCode::FORBIDDEN,
            "en-tête X-IC705-Bridge requis (utilisez la librairie Python)",
        )
        .into_response();
    }

    next.run(req).await
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

    // Clone de l'Arc puis relâchement du verrou global : `Session` prend ensuite
    // en charge la sérialisation des transactions CI-V concurrentes.
    let session = st
        .session
        .lock()
        .await
        .as_ref()
        .cloned()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "non connecté à l'IC-705"))?;

    let response = session.send_civ(&bytes).await.map_err(bridge_err)?;

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
            None => {
                return err(StatusCode::SERVICE_UNAVAILABLE, "non connecté à l'IC-705")
                    .into_response()
            }
        }
    };

    let stream = BroadcastStream::new(rx).filter_map(|res| match res {
        Ok(frame) => Some(Ok::<Event, Infallible>(
            Event::default().data(to_hex(&frame)),
        )),
        // `Lagged` : le client n'a pas suivi le débit, on saute simplement.
        Err(_) => None,
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Construit le routeur de l'API locale (garde locale incluse).
fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/status", get(status_handler))
        .route("/civ", post(civ_handler))
        .route("/stream", get(stream_handler))
        .layer(middleware::from_fn(guard_local_host))
        .with_state(state)
}

/// Démarre le serveur HTTP local. Boucle jusqu'à l'arrêt de l'application.
pub async fn serve(state: Arc<AppState>) {
    let app = router(state.clone());

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // pour `oneshot`

    fn app() -> Router {
        router(Arc::new(AppState::new()))
    }

    async fn status_of(req: Request<Body>) -> StatusCode {
        app().oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn rejects_bad_host() {
        let req = Request::builder()
            .uri("/status")
            .header(header::HOST, "evil.example.com")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn allows_status_on_localhost_without_header() {
        let req = Request::builder()
            .uri("/status")
            .header(header::HOST, API_ADDR)
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_civ_without_auth_header() {
        let req = Request::builder()
            .method("POST")
            .uri("/civ")
            .header(header::HOST, API_ADDR)
            .header("content-type", "application/json")
            .body(Body::from(r#"{"frame":"FE FE A4 E0 03 FD"}"#))
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn civ_with_header_passes_guard_then_503_not_connected() {
        // En-tête présent : la garde laisse passer ; le handler répond 503
        // car aucune session n'est connectée (≠ 403 de la garde).
        let req = Request::builder()
            .method("POST")
            .uri("/civ")
            .header(header::HOST, "localhost:8765")
            .header("content-type", "application/json")
            .header(AUTH_HEADER, "1")
            .body(Body::from(r#"{"frame":"FE FE A4 E0 03 FD"}"#))
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn rejects_stream_without_auth_header() {
        let req = Request::builder()
            .uri("/stream")
            .header(header::HOST, API_ADDR)
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::FORBIDDEN);
    }

    #[test]
    fn civ_timeout_maps_to_gateway_timeout() {
        let (status, _) = bridge_err(BridgeError::Timeout("réponse CI-V".into()));
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    }
}
