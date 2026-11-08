//! One WebSocket connection: transport limits around the engine driver.
//!
//! Enforced here (the engine knows nothing about sockets): text frames only, 4096-byte
//! frames, per-connection rate limit (every frame counts, pongs included), handshake timeout,
//! idle timeout, WebSocket pings, a deadline to get into a room, a limit on room-code
//! guessing, and a write timeout. Everything else is forwarded to the driver as raw text.

use crate::driver::{Cmd, OUT_QUEUE, Out};
use crate::engine::ConnId;
use crate::limit::{RateLimiter, Verdict};
use crate::server::{Shared, stopped};
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use boussole_protocol::{ErrorCode, MAX_FRAME_BYTES, ServerMsg, encode};
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::PoisonError;
use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use tokio::time::{Instant, interval_at, sleep, timeout};

/// WebSocket close codes (RFC 6455 §7.4.1).
const CLOSE_NORMAL: u16 = 1000;
const CLOSE_GOING_AWAY: u16 = 1001;
const CLOSE_POLICY: u16 = 1008;

/// A peer that does not drain a send within this long is gone.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// After our close frame, how long to wait for the peer's before dropping the socket.
const CLOSE_LINGER: Duration = Duration::from_millis(500);

/// More than this many `room_not_found` answers within [`PROBE_WINDOW`] is room-code guessing
/// (a real client types a code a handful of times at most): the connection is closed.
const PROBE_LIMIT: usize = 5;
const PROBE_WINDOW: Duration = Duration::from_secs(60);

/// Largest message the WebSocket layer will buffer. Above [`MAX_FRAME_BYTES`] we still want to
/// read the frame to answer `bad_message` and close with a policy violation; above this the
/// WebSocket layer itself aborts the connection.
pub(crate) const MAX_WS_MESSAGE: usize = 16 * 1024;

/// Why a WebSocket upgrade was refused.
pub(crate) enum Refusal {
    /// `max_connections` reached (HTTP 503).
    ServerFull,
    /// `max_connections_per_ip` reached for this client address (HTTP 429).
    TooManyFromAddress,
}

/// Counts the connection against `max_connections` and its address's quota until dropped.
pub(crate) struct ConnGuard {
    shared: Arc<Shared>,
    client: IpAddr,
}

impl ConnGuard {
    pub fn acquire(shared: &Arc<Shared>, client: IpAddr) -> Result<Self, Refusal> {
        if shared.open_conns.fetch_add(1, SeqCst) >= shared.config.max_connections {
            shared.open_conns.fetch_sub(1, SeqCst);
            return Err(Refusal::ServerFull);
        }
        {
            let mut per_ip = shared.per_ip.lock().unwrap_or_else(PoisonError::into_inner);
            let used = per_ip.get(&client).copied().unwrap_or(0);
            if used >= shared.config.max_connections_per_ip {
                drop(per_ip);
                shared.open_conns.fetch_sub(1, SeqCst);
                return Err(Refusal::TooManyFromAddress);
            }
            per_ip.insert(client, used + 1);
        }
        Ok(Self {
            shared: Arc::clone(shared),
            client,
        })
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        {
            let mut per_ip = self
                .shared
                .per_ip
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(used) = per_ip.get_mut(&self.client) {
                *used -= 1;
                if *used == 0 {
                    per_ip.remove(&self.client);
                }
            }
        }
        // Last: `ServerHandle::shutdown` waits for this counter to reach zero.
        self.shared.open_conns.fetch_sub(1, SeqCst);
    }
}

pub(crate) async fn serve(
    socket: WebSocket,
    shared: Arc<Shared>,
    peer: SocketAddr,
    guard: ConnGuard,
) {
    let conn = shared.next_conn_id();
    let client = guard.client;
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
    tracing::info!(conn, %peer, %client, "client connected");
    let started = std::time::Instant::now();
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
    tracing::info!(conn, %peer, %client, reason, secs = started.elapsed().as_secs(), "client disconnected");
}

struct Connection<'a> {
    socket: WebSocket,
    shared: &'a Shared,
    conn: ConnId,
    rx: mpsc::Receiver<Out>,
    kick: Arc<Notify>,
}

impl Connection<'_> {
    /// Run until the connection ends; returns why (for the log).
    async fn run(mut self) -> &'static str {
        let cfg = self.shared.config.clone();
        let origin = Instant::now();
        let mut limiter = RateLimiter::new(cfg.msg_rate_per_s, cfg.msg_burst, Duration::ZERO);
        let mut ping = interval_at(origin + cfg.ping_interval, cfg.ping_interval);
        // Dropped when nothing at all (not even a pong) arrives for `idle_timeout`.
        let idle = sleep(cfg.idle_timeout);
        tokio::pin!(idle);
        let handshake = sleep(cfg.handshake_timeout);
        tokio::pin!(handshake);
        // Armed by `welcome`, disarmed by the first `room_snapshot`.
        let room_wait = sleep(cfg.room_join_timeout);
        tokio::pin!(room_wait);
        let mut identified = false;
        let mut waiting_for_room = false;
        let mut room_probes: VecDeque<Instant> = VecDeque::new();
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
                    match &msg {
                        ServerMsg::Welcome { .. } => {
                            identified = true;
                            waiting_for_room = true;
                            room_wait.as_mut().reset(Instant::now() + cfg.room_join_timeout);
                        }
                        ServerMsg::RoomSnapshot(_) => waiting_for_room = false,
                        _ => {}
                    }
                    let not_found = matches!(
                        &msg,
                        ServerMsg::Error { code: ErrorCode::RoomNotFound, .. }
                    );
                    if !self.send(Message::text(encode(&msg))).await {
                        return "write failed";
                    }
                    if close {
                        self.close(CLOSE_NORMAL, "").await;
                        return "closed by server";
                    }
                    if not_found && probing(&mut room_probes, Instant::now()) {
                        self.close(CLOSE_POLICY, "too many unknown room codes").await;
                        return "room code guessing";
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
                    idle.as_mut().reset(Instant::now() + cfg.idle_timeout);
                    // Every frame counts, pongs and pings included: they are free to send.
                    match limiter.check(origin.elapsed()) {
                        Verdict::Allow => {}
                        Verdict::Limited => {
                            if !matches!(msg, Message::Pong(_))
                                && !self.send_error(ErrorCode::RateLimited).await
                            {
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
                        // Ping: the WebSocket layer answers it. Pong: only counted above.
                        Message::Ping(_) | Message::Pong(_) | Message::Close(_) => {}
                    }
                }
                _ = ping.tick() => {
                    if !self.send(Message::Ping(Vec::new().into())).await {
                        return "write failed";
                    }
                }
                () = &mut idle => return "idle timeout",
                () = &mut handshake, if !identified => {
                    self.close(CLOSE_POLICY, "no hello received").await;
                    return "handshake timeout";
                }
                () = &mut room_wait, if waiting_for_room => {
                    self.close(CLOSE_POLICY, "no room joined").await;
                    return "no room joined";
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

    /// Transport-level errors are written straight to the socket, **not** through the queue
    /// of engine replies: one may overtake an engine reply that is still queued (see
    /// `docs/protocol.md` §14).
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

/// Record one `room_not_found` at `now` and say whether the connection has now guessed too
/// many codes.
fn probing(times: &mut VecDeque<Instant>, now: Instant) -> bool {
    while times
        .front()
        .is_some_and(|t| now.duration_since(*t) > PROBE_WINDOW)
    {
        times.pop_front();
    }
    times.push_back(now);
    times.len() > PROBE_LIMIT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_code_guessing_is_a_handful_per_minute() {
        let t0 = Instant::now();
        let mut times = VecDeque::new();
        for _ in 0..PROBE_LIMIT {
            assert!(!probing(&mut times, t0));
        }
        assert!(
            probing(&mut times, t0),
            "the sixth guess within a minute is too many"
        );

        // Spread out, the same number of misses is fine.
        let mut times = VecDeque::new();
        for i in 0..20 {
            assert!(!probing(&mut times, t0 + Duration::from_secs(20 * i)));
        }
    }
}
