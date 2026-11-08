//! `start`, [`ServerHandle`] and the HTTP surface (`/ws`, `/healthz`, `/version`).
//!
//! The listener is served by a small accept loop around hyper (not `axum::serve`) because that
//! is the only way to get what a server exposed to strangers needs: a timeout for the request
//! head (slowloris), a cap on raw sockets, and `TCP_NODELAY`.

use crate::config::ServerConfig;
use crate::conn::{ConnGuard, MAX_WS_MESSAGE, Refusal};
use crate::driver::{self, Clock, Cmd};
use crate::engine::{ConnId, Engine};
use crate::entropy::OsEntropy;
use axum::Json;
use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use boussole_protocol::PROTOCOL_VERSION;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tower::ServiceExt;

/// Commands queued between all connections and the driver.
const CMD_QUEUE: usize = 1024;
/// How long [`ServerHandle::shutdown`] waits for connections to say goodbye before aborting.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// After the listener is closed, how long plain HTTP requests in flight may take to finish.
const HTTP_DRAIN: Duration = Duration::from_secs(1);
/// Largest buffer hyper may use for one request head (an upgrade request is a few hundred
/// bytes); smaller than hyper's default of about 400 KB so that slow clients cost little.
const MAX_HTTP_BUF: usize = 16 * 1024;
/// Refused connections are summed up in one warning at most this often (the volume of such
/// lines is attacker-controlled).
const REFUSAL_LOG_EVERY: Duration = Duration::from_secs(10);

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
    /// Open WebSocket connections (upgraded), against `max_connections`.
    pub open_conns: AtomicUsize,
    /// Open WebSocket connections per client address, against `max_connections_per_ip`.
    pub per_ip: Mutex<HashMap<IpAddr, usize>>,
    /// Sockets still in their HTTP phase (not yet upgraded), against twice `max_connections`.
    raw_open: AtomicUsize,
    next_conn: AtomicU64,
    refused: AtomicU64,
    last_refusal_log: Mutex<Instant>,
}

impl Shared {
    /// A fresh connection id (unique, never reused).
    pub fn next_conn_id(&self) -> ConnId {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
    }

    /// Count a refused connection; sum them up in one warning every few seconds.
    pub fn note_refusal(&self, why: &'static str, who: IpAddr) {
        self.refused.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(%who, why, "connection refused");
        let mut last = self
            .last_refusal_log
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if last.elapsed() >= REFUSAL_LOG_EVERY {
            *last = Instant::now();
            let refused = self.refused.swap(0, Ordering::Relaxed);
            tracing::warn!(refused, latest = why, "connections refused by the limits");
        }
    }
}

/// Counts a socket against the raw-socket cap while it is in its HTTP phase.
struct RawGuard(Arc<Shared>);

impl RawGuard {
    fn acquire(shared: &Arc<Shared>, cap: usize) -> Option<Self> {
        if shared.raw_open.fetch_add(1, Ordering::SeqCst) >= cap {
            shared.raw_open.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self(Arc::clone(shared)))
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        self.0.raw_open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Marks the server as ended when a task of it finishes, however it finishes (a panic
/// included: destructors run while the task unwinds).
struct EndGuard(Arc<watch::Sender<bool>>);

impl Drop for EndGuard {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

/// A running server. Dropping it stops the server in the background; call
/// [`ServerHandle::shutdown`] to wait until it has stopped and released its port.
pub struct ServerHandle {
    addr: SocketAddr,
    stop: watch::Sender<bool>,
    ended: watch::Receiver<bool>,
    shared: Arc<Shared>,
    http: Option<JoinHandle<()>>,
    driver: Option<JoinHandle<()>>,
}

impl fmt::Debug for ServerHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerHandle")
            .field("addr", &self.addr)
            .field("running", &self.is_running())
            .finish_non_exhaustive()
    }
}

impl ServerHandle {
    /// The address actually bound (useful with port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// `true` while the server accepts connections: `false` once a stop was requested, and
    /// also if the listener or the engine task died on its own (a bug), which an application
    /// embedding the server should treat as "the server is gone".
    pub fn is_running(&self) -> bool {
        !*self.stop.borrow() && !*self.ended.borrow()
    }

    /// Resolves when the server has stopped, for any reason (a requested stop, or a task that
    /// died). Lets a supervisor react: the standalone binary exits non-zero on it.
    pub async fn closed(&self) {
        let mut ended = self.ended.clone();
        stopped(&mut ended).await;
    }

    /// Stop the server: the listener is closed and the port released, every connected client
    /// is sent a WebSocket close (`1001 going away`) and gets a moment to answer. Returns once
    /// the listener is gone and no connection is left open, so a host that exits right after
    /// `shutdown().await` has delivered its goodbyes. Connections that do not cooperate are
    /// cut after a few seconds.
    pub async fn shutdown(mut self) {
        self.stop.send_replace(true);
        let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
        if let Some(mut http) = self.http.take()
            && tokio::time::timeout_at(deadline, &mut http).await.is_err()
        {
            tracing::warn!("the listener did not stop in time, aborting it");
            http.abort();
            let _ = http.await;
        }
        let shared = Arc::clone(&self.shared);
        let drained = tokio::time::timeout_at(deadline, async move {
            while shared.open_conns.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if drained.is_err() {
            tracing::warn!(
                open = self.shared.open_conns.load(Ordering::SeqCst),
                "connections still open after the grace period"
            );
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
/// assert!(server.is_running());
/// server.shutdown().await;
/// # Ok(()) }
/// ```
pub async fn start(config: ServerConfig) -> io::Result<ServerHandle> {
    let entropy = OsEntropy::new()?;
    let listener = TcpListener::bind(config.bind).await?;
    let addr = listener.local_addr()?;

    let (stop, stop_rx) = watch::channel(false);
    let (ended_tx, ended) = watch::channel(false);
    let ended_tx = Arc::new(ended_tx);
    let (cmds, cmd_rx) = mpsc::channel(CMD_QUEUE);
    let shared = Arc::new(Shared {
        config: config.clone(),
        cmds,
        shutdown: stop_rx.clone(),
        open_conns: AtomicUsize::new(0),
        per_ip: Mutex::new(HashMap::new()),
        raw_open: AtomicUsize::new(0),
        next_conn: AtomicU64::new(1),
        refused: AtomicU64::new(0),
        last_refusal_log: Mutex::new(
            Instant::now()
                .checked_sub(REFUSAL_LOG_EVERY)
                .unwrap_or_else(Instant::now),
        ),
    });

    let engine = Engine::new(Box::new(entropy));
    let clock = Clock::new(config.time_scale);
    let driver_end = EndGuard(Arc::clone(&ended_tx));
    let driver = tokio::spawn({
        let stop_rx = stop_rx.clone();
        async move {
            let _end = driver_end;
            driver::run(engine, clock, config.max_rooms, cmd_rx, stop_rx).await;
        }
    });

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .with_state(Arc::clone(&shared));
    let http_end = EndGuard(ended_tx);
    let http = tokio::spawn({
        let shared = Arc::clone(&shared);
        async move {
            let _end = http_end;
            accept_loop(listener, app, shared, stop_rx).await;
        }
    });

    tracing::info!(%addr, max_connections = shared.config.max_connections, max_rooms = shared.config.max_rooms, "boussole server listening");
    Ok(ServerHandle {
        addr,
        stop,
        ended,
        shared,
        http: Some(http),
        driver: Some(driver),
    })
}

/// Accept sockets and serve HTTP/1.1 on them until `stop`.
async fn accept_loop(
    listener: TcpListener,
    app: Router,
    shared: Arc<Shared>,
    mut stop: watch::Receiver<bool>,
) {
    let raw_cap = shared.config.max_connections.saturating_mul(2);
    let header_timeout = shared.config.header_read_timeout;
    let mut conns = JoinSet::new();
    loop {
        tokio::select! {
            () = stopped(&mut stop) => break,
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(accepted) => accepted,
                    Err(e) => {
                        // Out of file descriptors or a connection that died in the queue: do
                        // not spin.
                        tracing::warn!(error = %e, "accept failed");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let Some(raw) = RawGuard::acquire(&shared, raw_cap) else {
                    shared.note_refusal("socket limit", peer.ip());
                    continue; // dropping `stream` closes it
                };
                // Small messages (`round_started` then `room_snapshot`) must not wait for each
                // other's ACKs.
                let _ = stream.set_nodelay(true);
                conns.spawn(serve_http(stream, peer, app.clone(), header_timeout, raw));
            }
        }
    }
    drop(listener); // the port is free from here on
    // Plain HTTP requests in flight get a moment; upgraded sockets are not ours any more (their
    // tasks say goodbye on their own), and whatever is left after the grace is cut when
    // `conns` is dropped.
    let _ = tokio::time::timeout(HTTP_DRAIN, async {
        while conns.join_next().await.is_some() {}
    })
    .await;
}

/// One accepted socket: HTTP/1.1 with a request-head timeout, upgrades allowed.
async fn serve_http(
    stream: TcpStream,
    peer: SocketAddr,
    app: Router,
    header_read_timeout: Duration,
    _raw: RawGuard,
) {
    let service = service_fn(move |mut request: axum::http::Request<Incoming>| {
        request.extensions_mut().insert(ConnectInfo(peer));
        app.clone().oneshot(request)
    });
    let connection = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(header_read_timeout)
        .max_buf_size(MAX_HTTP_BUF)
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    if let Err(e) = connection.await {
        tracing::debug!(%peer, error = %e, "http connection ended with an error");
    }
}

/// The address a client counts as: the TCP peer, or, behind a trusted proxy, the address the
/// proxy reports in `Cf-Connecting-Ip` (cloudflared) or `X-Forwarded-For` (Caddy and others;
/// the **last** entry is the one our own proxy appended, earlier ones are client-controlled).
/// Headers are believed only from a loopback peer (a local tunnel or proxy), or from any peer
/// when `trust_all` is set because the server is reachable through the proxy only.
pub(crate) fn client_ip(peer: IpAddr, headers: &HeaderMap, trust_all: bool) -> IpAddr {
    let peer = peer.to_canonical();
    if !(trust_all || peer.is_loopback()) {
        return peer;
    }
    let parse = |value: &str| value.trim().parse::<IpAddr>().ok();
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(parse)
        .or_else(|| {
            headers
                .get_all("x-forwarded-for")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|v| v.split(','))
                .next_back()
                .and_then(parse)
        })
        .map_or(peer, |ip| ip.to_canonical())
}

async fn ws_handler(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if *shared.shutdown.borrow() {
        return (StatusCode::SERVICE_UNAVAILABLE, "server is stopping").into_response();
    }
    let client = client_ip(peer.ip(), &headers, shared.config.trust_proxy_headers);
    let guard = match ConnGuard::acquire(&shared, client) {
        Ok(guard) => guard,
        Err(Refusal::ServerFull) => {
            shared.note_refusal("connection limit", client);
            return (StatusCode::SERVICE_UNAVAILABLE, "server is full").into_response();
        }
        Err(Refusal::TooManyFromAddress) => {
            shared.note_refusal("per-address limit", client);
            return (
                StatusCode::TOO_MANY_REQUESTS,
                "too many connections from your address",
            )
                .into_response();
        }
    };
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| crate::conn::serve(socket, shared, peer, guard))
}

/// `200 ok`, or `503` once the engine task is gone: a server that cannot run matches must not
/// look healthy to a supervisor.
async fn healthz(State(shared): State<Arc<Shared>>) -> Response {
    if shared.cmds.is_closed() {
        (StatusCode::SERVICE_UNAVAILABLE, "engine stopped").into_response()
    } else {
        "ok".into_response()
    }
}

async fn version() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "server_version": env!("CARGO_PKG_VERSION"),
        "protocol_version": PROTOCOL_VERSION,
        "dataset_fingerprint": format!("{:016x}", boussole_core::dataset_fingerprint()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.append(*k, HeaderValue::from_static(v));
        }
        map
    }

    #[test]
    fn client_address_comes_from_proxy_headers_only_when_the_proxy_is_trusted() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let remote: IpAddr = "203.0.113.9".parse().unwrap();
        let cf = headers(&[("cf-connecting-ip", "198.51.100.7")]);
        let xff = headers(&[("x-forwarded-for", "6.6.6.6, 198.51.100.8")]);
        let both = headers(&[
            ("x-forwarded-for", "6.6.6.6"),
            ("cf-connecting-ip", "198.51.100.7"),
        ]);

        // A loopback peer (cloudflared, a local proxy) is believed; Cf-Connecting-Ip first.
        assert_eq!(
            client_ip(loopback, &cf, false),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            client_ip(loopback, &both, false),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
        // X-Forwarded-For: the last entry (appended by our proxy), not the spoofable first.
        assert_eq!(
            client_ip(loopback, &xff, false),
            "198.51.100.8".parse::<IpAddr>().unwrap()
        );
        // A remote peer cannot choose its own address...
        assert_eq!(client_ip(remote, &cf, false), remote);
        // ...unless the operator says every peer is the proxy.
        assert_eq!(
            client_ip(remote, &cf, true),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
        // Garbage or absent headers fall back to the peer; IPv4-mapped IPv6 is normalised.
        assert_eq!(
            client_ip(loopback, &headers(&[("cf-connecting-ip", "nope")]), false),
            loopback
        );
        assert_eq!(client_ip(loopback, &HeaderMap::new(), false), loopback);
        assert_eq!(
            client_ip(
                "::ffff:203.0.113.9".parse().unwrap(),
                &HeaderMap::new(),
                false
            ),
            remote
        );
    }

    async fn get(addr: SocketAddr, path: &str) -> String {
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(format!("GET {path} HTTP/1.0\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut raw = String::new();
        s.read_to_string(&mut raw).await.unwrap();
        raw
    }

    #[tokio::test]
    async fn shutdown_waits_for_open_connections_to_finish() {
        let server = start(ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..ServerConfig::default()
        })
        .await
        .unwrap();
        // A connection that is still saying goodbye: its guard is alive until it is done.
        let guard = ConnGuard::acquire(&server.shared, "192.0.2.1".parse().unwrap())
            .unwrap_or_else(|_| panic!("a slot is free"));
        let mut shutdown = tokio::spawn(server.shutdown());

        let early = tokio::time::timeout(Duration::from_millis(300), &mut shutdown).await;
        assert!(
            early.is_err(),
            "shutdown() must wait while a connection is open"
        );

        drop(guard);
        tokio::time::timeout(Duration::from_secs(2), shutdown)
            .await
            .expect("shutdown() returns once the connection is gone")
            .unwrap();
    }

    #[tokio::test]
    async fn healthz_turns_503_and_is_running_false_when_the_engine_task_dies() {
        let mut server = start(ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..ServerConfig::default()
        })
        .await
        .unwrap();
        let addr = server.local_addr();
        assert!(server.is_running());
        assert!(get(addr, "/healthz").await.contains(" 200 "));

        // Simulate the engine task dying on its own (a panic would end it the same way).
        server.driver.take().unwrap().abort();
        tokio::time::timeout(Duration::from_secs(5), server.closed())
            .await
            .expect("closed() resolves when a task dies");
        assert!(!server.is_running());
        let reply = get(addr, "/healthz").await;
        assert!(reply.contains(" 503 "), "{reply}");
        server.shutdown().await;
    }
}
