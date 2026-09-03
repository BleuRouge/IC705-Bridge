//! API HTTP locale (127.0.0.1, port configurable) — passerelle Python.
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
use tokio::sync::{oneshot, Mutex};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

use crate::commands::CivResult;
use crate::error::BridgeError;
use crate::state::{AppState, StatusSnapshot, API_HOST, DEFAULT_API_PORT};
use crate::util::{parse_hex, to_hex};

/// Port minimal retenu pour éviter les ports privilégiés sur macOS/Linux.
pub const MIN_API_PORT: u16 = 1024;

struct RunningApi {
    generation: u64,
    port: u16,
    shutdown: oneshot::Sender<()>,
}

#[derive(Default)]
struct ApiManagerState {
    generation: u64,
    current: Option<RunningApi>,
}

/// Pilote le serveur HTTP et permet un changement de port sans redémarrer l'app.
#[derive(Default)]
pub struct ApiServerManager {
    inner: Mutex<ApiManagerState>,
}

impl ApiServerManager {
    /// Démarre le port standard, sauf si l'UI a déjà appliqué un port mémorisé.
    pub async fn ensure_started(self: &Arc<Self>, state: Arc<AppState>) {
        if let Err(error) = self.start(state, DEFAULT_API_PORT, true).await {
            tracing::error!("impossible de démarrer l'API locale : {error}");
        }
    }

    /// Bascule l'API vers `port`. L'ancien serveur n'est arrêté qu'une fois le
    /// nouveau port réservé avec succès.
    pub async fn set_port(
        self: &Arc<Self>,
        state: Arc<AppState>,
        port: u16,
    ) -> std::result::Result<(), BridgeError> {
        validate_api_port(port)?;
        self.start(state, port, false).await
    }

    async fn start(
        self: &Arc<Self>,
        state: Arc<AppState>,
        port: u16,
        only_if_absent: bool,
    ) -> std::result::Result<(), BridgeError> {
        let mut manager = self.inner.lock().await;
        if only_if_absent && manager.current.is_some() {
            return Ok(());
        }
        if manager.current.as_ref().is_some_and(|api| api.port == port) {
            return Ok(());
        }

        let listener = tokio::net::TcpListener::bind((API_HOST, port))
            .await
            .map_err(|error| {
                BridgeError::Protocol(format!(
                    "port API local {port} indisponible sur {API_HOST} : {error}"
                ))
            })?;
        let app = router(state.clone());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        manager.generation = manager.generation.wrapping_add(1);
        let generation = manager.generation;
        let previous = manager.current.replace(RunningApi {
            generation,
            port,
            shutdown: shutdown_tx,
        });
        state.set_api_endpoint(port, true);

        let api_manager = Arc::downgrade(self);
        tokio::spawn(async move {
            tracing::info!("API locale démarrée sur http://{API_HOST}:{port}");
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
            if let Err(error) = result {
                tracing::error!("API locale arrêtée sur le port {port} : {error}");
            }

            if let Some(api_manager) = api_manager.upgrade() {
                let mut manager = api_manager.inner.lock().await;
                if manager
                    .current
                    .as_ref()
                    .is_some_and(|api| api.generation == generation)
                {
                    manager.current = None;
                    state.set_api_stopped_if_port(port);
                }
            }
        });

        drop(manager);
        if let Some(previous) = previous {
            let _ = previous.shutdown.send(());
        }
        Ok(())
    }
}

fn validate_api_port(port: u16) -> std::result::Result<(), BridgeError> {
    if port < MIN_API_PORT {
        return Err(BridgeError::Protocol(format!(
            "le port API doit être compris entre {MIN_API_PORT} et 65535"
        )));
    }
    Ok(())
}

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

fn is_allowed_local_host(host: &str) -> bool {
    for prefix in ["127.0.0.1:", "localhost:"] {
        if let Some(port) = host.strip_prefix(prefix) {
            return port.parse::<u16>().is_ok_and(|port| port != 0);
        }
    }
    matches!(host, "127.0.0.1" | "localhost")
}

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
        .is_some_and(is_allowed_local_host);
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
            .header(header::HOST, format!("{API_HOST}:{DEFAULT_API_PORT}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_civ_without_auth_header() {
        let req = Request::builder()
            .method("POST")
            .uri("/civ")
            .header(header::HOST, format!("{API_HOST}:{DEFAULT_API_PORT}"))
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
            .header(header::HOST, format!("{API_HOST}:{DEFAULT_API_PORT}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::FORBIDDEN);
    }

    #[test]
    fn civ_timeout_maps_to_gateway_timeout() {
        let (status, _) = bridge_err(BridgeError::Timeout("réponse CI-V".into()));
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    }

    #[test]
    fn accepts_configurable_loopback_ports_only() {
        assert!(is_allowed_local_host("127.0.0.1:19001"));
        assert!(is_allowed_local_host("localhost:65535"));
        assert!(!is_allowed_local_host("localhost.example:8765"));
        assert!(!is_allowed_local_host("127.0.0.1:not-a-port"));
    }

    #[test]
    fn rejects_privileged_api_ports() {
        assert!(validate_api_port(MIN_API_PORT).is_ok());
        assert!(validate_api_port(MIN_API_PORT - 1).is_err());
    }

    fn available_port() -> u16 {
        std::net::TcpListener::bind((API_HOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[tokio::test]
    async fn switches_api_port_without_restarting_the_app() {
        let state = Arc::new(AppState::new());
        let manager = Arc::new(ApiServerManager::default());
        let first = available_port();
        let mut second = available_port();
        while second == first {
            second = available_port();
        }

        manager.set_port(state.clone(), first).await.unwrap();
        assert_eq!(state.snapshot().api_port, first);
        assert!(tokio::net::TcpStream::connect((API_HOST, first))
            .await
            .is_ok());

        manager.set_port(state.clone(), second).await.unwrap();
        assert_eq!(state.snapshot().api_port, second);
        assert!(state.snapshot().api_running);
        assert!(tokio::net::TcpStream::connect((API_HOST, second))
            .await
            .is_ok());

        let old_port_closed = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if tokio::net::TcpStream::connect((API_HOST, first))
                    .await
                    .is_err()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(old_port_closed.is_ok());
    }
}
