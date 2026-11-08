//! `start`, [`ServerHandle`] and the HTTP surface (`/ws`, `/healthz`, `/version`).

use crate::config::ServerConfig;
use crate::conn::{ConnGuard, MAX_WS_MESSAGE};
use crate::driver::{self, Clock, Cmd};
use crate::engine::{ConnId, Engine};
use crate::entropy::OsEntropy;
use axum::Json;
use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use boussole_protocol::PROTOCOL_VERSION;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// Commands queued between all connections and the driver.
const CMD_QUEUE: usize = 1024;
/// How long [`ServerHandle::shutdown`] waits for connections to wind down before aborting.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Resolve once `stop` is `true` (or its sender is gone). Returns `()` so that the lock guard
/// of `watch::Ref` is not kept across awaits (it is not `Send`).
pub(crate) async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// State shared by every connection task.
pub(crate) struct Shared {
    pub config: ServerConfig,
    pub cmds: mpsc::Sender<Cmd>,
    pub shutdown: watch::Receiver<bool>,
    pub open_conns: AtomicUsize,
    next_conn: AtomicU64,
}

impl Shared {
    /// A fresh connection id (unique, never reused).
    pub fn next_conn_id(&self) -> ConnId {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
    }
}

/// A running server. Dropping it stops the server in the background; call
/// [`ServerHandle::shutdown`] to wait until it has stopped and released its port.
#[derive(Debug)]
pub struct ServerHandle {
    addr: SocketAddr,
    stop: watch::Sender<bool>,
    http: Option<JoinHandle<io::Result<()>>>,
    driver: Option<JoinHandle<()>>,
}

impl ServerHandle {
    /// The address actually bound (useful with port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stop the server: connected clients get a WebSocket close (`1001 going away`), the
    /// listener is closed and the port released. Returns once everything has stopped
    /// (connections that do not answer are cut after a few seconds).
    pub async fn shutdown(mut self) {
        self.stop.send_replace(true);
        if let Some(mut http) = self.http.take() {
            match tokio::time::timeout(SHUTDOWN_GRACE, &mut http).await {
                Ok(Ok(Err(e))) => tracing::warn!(error = %e, "http server ended with an error"),
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!("connections did not close in time, aborting the server");
                    http.abort();
                    let _ = http.await;
                }
            }
        }
        if let Some(driver) = self.driver.take() {
            let _ = driver.await;
        }
        tracing::info!(addr = %self.addr, "server stopped");
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

/// Bind `config.bind` (port 0 allowed) and run the server on the **current** tokio runtime.
/// Returns once the socket is listening.
///
/// ```no_run
/// # async fn demo() -> std::io::Result<()> {
/// use boussole_server::{ServerConfig, lan_ws_urls, start};
/// let server = start(ServerConfig::for_host_mode("0.0.0.0:0".parse().unwrap())).await?;
/// let port = server.local_addr().port();
/// println!("friends on the LAN join: {:?}", lan_ws_urls(port));
/// server.shutdown().await;
/// # Ok(()) }
/// ```
pub async fn start(config: ServerConfig) -> io::Result<ServerHandle> {
    let entropy = OsEntropy::new()?;
    let listener = TcpListener::bind(config.bind).await?;
    let addr = listener.local_addr()?;

    let (stop, stop_rx) = watch::channel(false);
    let (cmds, cmd_rx) = mpsc::channel(CMD_QUEUE);
    let shared = Arc::new(Shared {
        config: config.clone(),
        cmds,
        shutdown: stop_rx.clone(),
        open_conns: AtomicUsize::new(0),
        next_conn: AtomicU64::new(1),
    });

    let driver = tokio::spawn(driver::run(
        Engine::new(Box::new(entropy)),
        Clock::new(config.time_scale),
        config.max_rooms,
        cmd_rx,
        stop_rx.clone(),
    ));

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/healthz", get(|| async { "ok" }))
        .route("/version", get(version))
        .with_state(shared);
    let mut graceful = stop_rx;
    let http = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { stopped(&mut graceful).await })
        .await
    });

    tracing::info!(%addr, max_connections = config.max_connections, max_rooms = config.max_rooms, "boussole server listening");
    Ok(ServerHandle {
        addr,
        stop,
        http: Some(http),
        driver: Some(driver),
    })
}

async fn ws_handler(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    if *shared.shutdown.borrow() {
        return (StatusCode::SERVICE_UNAVAILABLE, "server is stopping").into_response();
    }
    let Some(guard) = ConnGuard::acquire(&shared) else {
        tracing::warn!(%peer, max = shared.config.max_connections, "connection limit reached, upgrade refused");
        return (StatusCode::SERVICE_UNAVAILABLE, "server is full").into_response();
    };
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| crate::conn::serve(socket, shared, peer, guard))
}

async fn version() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "server_version": env!("CARGO_PKG_VERSION"),
        "protocol_version": PROTOCOL_VERSION,
        "dataset_fingerprint": format!("{:016x}", boussole_core::dataset_fingerprint()),
    }))
}
