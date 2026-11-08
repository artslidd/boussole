//! One WebSocket connection: transport limits around the engine driver.
//!
//! Enforced here (the engine knows nothing about sockets): text frames only, 4096-byte
//! frames, per-connection rate limit, handshake timeout, WebSocket ping / idle timeout, and
//! a write timeout. Everything else is forwarded to the driver as raw text.

use crate::config::ServerConfig;
use crate::driver::{Cmd, OUT_QUEUE, Out};
use crate::engine::ConnId;
use crate::limit::{RateLimiter, Verdict};
use crate::server::{Shared, stopped};
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use boussole_protocol::{ErrorCode, MAX_FRAME_BYTES, ServerMsg, encode};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc};
use tokio::time::{interval_at, sleep, timeout};

/// WebSocket close codes (RFC 6455 §7.4.1).
const CLOSE_NORMAL: u16 = 1000;
const CLOSE_GOING_AWAY: u16 = 1001;
const CLOSE_POLICY: u16 = 1008;

/// A peer that does not drain a send within this long is gone.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// After our close frame, how long to wait for the peer's before dropping the socket.
const CLOSE_LINGER: Duration = Duration::from_secs(1);

/// Largest message the WebSocket layer will buffer. Above [`MAX_FRAME_BYTES`] we still want to
/// read the frame to answer `bad_message` and close with a policy violation; above this the
/// WebSocket layer itself aborts the connection.
pub(crate) const MAX_WS_MESSAGE: usize = 16 * 1024;

/// Counts the connection against `max_connections` until dropped.
pub(crate) struct ConnGuard(Arc<Shared>);

impl ConnGuard {
    pub fn acquire(shared: &Arc<Shared>) -> Option<Self> {
        use std::sync::atomic::Ordering::SeqCst;
        if shared.open_conns.fetch_add(1, SeqCst) >= shared.config.max_connections {
            shared.open_conns.fetch_sub(1, SeqCst);
            return None;
        }
        Some(Self(Arc::clone(shared)))
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0
            .open_conns
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

pub(crate) async fn serve(
    socket: WebSocket,
    shared: Arc<Shared>,
    peer: SocketAddr,
    guard: ConnGuard,
) {
    let conn = shared.next_conn_id();
    let (tx, rx) = mpsc::channel(OUT_QUEUE);
    let kick = Arc::new(Notify::new());
    let opened = Cmd::Open {
        conn,
        out: tx,
        kick: Arc::clone(&kick),
    };
    if shared.cmds.send(opened).await.is_err() {
        return; // the server is shutting down
    }
    tracing::info!(conn, %peer, "client connected");
    let started = Instant::now();
    let reason = Connection {
        socket,
        shared: &shared,
        conn,
        rx,
        kick,
    }
    .run()
    .await;
    // Tell the engine (idempotent if it already forgot this connection).
    let _ = shared.cmds.send(Cmd::Closed { conn }).await;
    drop(guard);
    tracing::info!(conn, %peer, reason, secs = started.elapsed().as_secs(), "client disconnected");
}

struct Connection<'a> {
    socket: WebSocket,
    shared: &'a Shared,
    conn: ConnId,
    rx: mpsc::Receiver<Out>,
    kick: Arc<Notify>,
}

impl Connection<'_> {
    fn config(&self) -> &ServerConfig {
        &self.shared.config
    }

    /// Run until the connection ends; returns why (for the log).
    async fn run(mut self) -> &'static str {
        let cfg = self.config().clone();
        let origin = Instant::now();
        let mut limiter = RateLimiter::new(cfg.msg_rate_per_s, cfg.msg_burst, Duration::ZERO);
        let mut last_seen = Instant::now();
        let mut ping = interval_at(
            tokio::time::Instant::now() + cfg.ping_interval,
            cfg.ping_interval,
        );
        let handshake = sleep(cfg.handshake_timeout);
        tokio::pin!(handshake);
        let mut identified = false;
        let mut shutdown = self.shared.shutdown.clone();

        loop {
            tokio::select! {
                () = stopped(&mut shutdown) => {
                    self.close(CLOSE_GOING_AWAY, "server stopping").await;
                    return "server shutdown";
                }
                () = self.kick.notified() => return "slow consumer",
                out = self.rx.recv() => {
                    let Some(Out { msg, close }) = out else {
                        // The driver dropped our queue: it is stopping (say goodbye), or it
                        // kicked us as a slow consumer (the kick wins the race to say so).
                        if *shutdown.borrow() {
                            self.close(CLOSE_GOING_AWAY, "server stopping").await;
                            return "server shutdown";
                        }
                        return "dropped by the server";
                    };
                    if matches!(msg, ServerMsg::Welcome { .. }) {
                        identified = true;
                    }
                    if !self.send(Message::text(encode(&msg))).await {
                        return "write failed";
                    }
                    if close {
                        self.close(CLOSE_NORMAL, "").await;
                        return "closed by server";
                    }
                }
                frame = self.socket.recv() => {
                    let msg = match frame {
                        None => return "socket closed",
                        Some(Err(e)) => {
                            tracing::debug!(conn = self.conn, error = %e, "websocket read error");
                            return "read error";
                        }
                        Some(Ok(Message::Close(_))) => return "client closed",
                        Some(Ok(msg)) => msg,
                    };
                    last_seen = Instant::now();
                    if matches!(msg, Message::Pong(_)) {
                        continue;
                    }
                    match limiter.check(origin.elapsed()) {
                        Verdict::Allow => {}
                        Verdict::Limited => {
                            if !self.send_error(ErrorCode::RateLimited).await {
                                return "write failed";
                            }
                            continue;
                        }
                        Verdict::Abusive => {
                            self.send_error(ErrorCode::RateLimited).await;
                            self.close(CLOSE_POLICY, "rate limit exceeded").await;
                            return "rate limit abuse";
                        }
                    }
                    match msg {
                        Message::Text(text) => {
                            if text.len() > MAX_FRAME_BYTES {
                                self.send_error(ErrorCode::BadMessage).await;
                                self.close(CLOSE_POLICY, "frame too large").await;
                                return "oversize frame";
                            }
                            let frame = Cmd::Frame { conn: self.conn, text: text.to_string() };
                            if self.shared.cmds.send(frame).await.is_err() {
                                return "server stopped";
                            }
                        }
                        Message::Binary(_) => {
                            // Text frames only (protocol §1); the connection is kept.
                            if !self.send_error(ErrorCode::BadMessage).await {
                                return "write failed";
                            }
                        }
                        // Ping: the WebSocket layer answers it. Pong / Close handled above.
                        Message::Ping(_) | Message::Pong(_) | Message::Close(_) => {}
                    }
                }
                _ = ping.tick() => {
                    if last_seen.elapsed() >= cfg.idle_timeout {
                        return "idle timeout";
                    }
                    if !self.send(Message::Ping(Vec::new().into())).await {
                        return "write failed";
                    }
                }
                () = &mut handshake, if !identified => {
                    self.close(CLOSE_POLICY, "no hello received").await;
                    return "handshake timeout";
                }
            }
        }
    }

    async fn send(&mut self, msg: Message) -> bool {
        matches!(
            timeout(WRITE_TIMEOUT, self.socket.send(msg)).await,
            Ok(Ok(()))
        )
    }

    async fn send_error(&mut self, code: ErrorCode) -> bool {
        self.send(Message::text(encode(&ServerMsg::error(code))))
            .await
    }

    /// Send a close frame, then give the peer a moment to answer so that what we sent
    /// before is not lost to a reset.
    async fn close(&mut self, code: u16, reason: &'static str) {
        let frame = Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        }));
        if !self.send(frame).await {
            return;
        }
        let _ = timeout(CLOSE_LINGER, async {
            while let Some(Ok(msg)) = self.socket.recv().await {
                if matches!(msg, Message::Close(_)) {
                    break;
                }
            }
        })
        .await;
    }
}
