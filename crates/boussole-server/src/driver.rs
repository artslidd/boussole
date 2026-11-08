//! The engine driver: one task owns the [`Engine`]; connections talk to it over channels.
//!
//! Connections send [`Cmd`]s (open / text frame / closed); the driver feeds them to the
//! engine with a monotonic clock, fires the engine's timers when they are due, and fans the
//! resulting [`Outbound`] messages out to bounded per-connection queues. A connection whose
//! queue is full is a slow consumer: it is kicked and reported to the engine as disconnected
//! (its player keeps the 60 s resume window).

use crate::engine::{ConnId, Engine, Millis, Outbound};
use crate::server::stopped;
use boussole_protocol::{ClientMsg, ErrorCode, ServerMsg, decode_client};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::sleep_until;

/// Messages queued per connection before it is considered too slow.
pub(crate) const OUT_QUEUE: usize = 64;

/// Longest the driver sleeps without looking at the engine again.
const MAX_SLEEP: Duration = Duration::from_secs(1);

/// What the driver sends to a connection task.
#[derive(Debug)]
pub(crate) struct Out {
    pub msg: ServerMsg,
    /// Close the socket after sending `msg`.
    pub close: bool,
}

/// What connection tasks send to the driver.
pub(crate) enum Cmd {
    Open {
        conn: ConnId,
        out: mpsc::Sender<Out>,
        /// Wake the connection task to drop the socket at once (slow consumer).
        kick: Arc<Notify>,
    },
    Frame {
        conn: ConnId,
        text: String,
    },
    Closed {
        conn: ConnId,
    },
}

/// Game clock: real time since start, optionally sped up (`time_scale`, a test hook).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Clock {
    start: Instant,
    scale: u128,
}

impl Clock {
    pub fn new(time_scale: u32) -> Self {
        Self {
            start: Instant::now(),
            scale: u128::from(time_scale.max(1)),
        }
    }

    pub fn now_ms(&self) -> Millis {
        (self.start.elapsed().as_micros() * self.scale / 1000) as Millis
    }

    /// The wall-clock instant at which the game clock reads `ms` (rounded up, so waking at it
    /// always finds the deadline due).
    pub fn instant_at(&self, ms: Millis) -> Instant {
        let micros = (u128::from(ms) * 1000).div_ceil(self.scale);
        self.start + Duration::from_micros(micros as u64)
    }
}

struct Sink {
    tx: mpsc::Sender<Out>,
    kick: Arc<Notify>,
}

struct Driver {
    engine: Engine,
    clock: Clock,
    max_rooms: usize,
    sinks: HashMap<ConnId, Sink>,
}

/// Run the driver until `shutdown` flips to `true` or every connection handle is gone.
pub(crate) async fn run(
    engine: Engine,
    clock: Clock,
    max_rooms: usize,
    mut cmds: mpsc::Receiver<Cmd>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut d = Driver {
        engine,
        clock,
        max_rooms,
        sinks: HashMap::new(),
    };
    let mut counts = (0, 0);
    loop {
        let wake = d
            .engine
            .next_deadline()
            .map_or(Instant::now() + MAX_SLEEP, |t| d.clock.instant_at(t))
            .min(Instant::now() + MAX_SLEEP);
        tokio::select! {
            () = stopped(&mut shutdown) => break,
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { break };
                d.on_cmd(cmd);
            }
            () = sleep_until(wake.into()) => {
                let out = d.engine.tick(d.clock.now_ms());
                d.deliver(out);
            }
        }
        let now_counts = (d.engine.room_count(), d.engine.player_count());
        if now_counts != counts {
            if now_counts.0 != counts.0 {
                tracing::info!(
                    rooms = now_counts.0,
                    players = now_counts.1,
                    "rooms changed"
                );
            }
            counts = now_counts;
        }
    }
    tracing::debug!("engine driver stopped");
}

impl Driver {
    fn on_cmd(&mut self, cmd: Cmd) {
        let now = self.clock.now_ms();
        match cmd {
            Cmd::Open { conn, out, kick } => {
                self.sinks.insert(conn, Sink { tx: out, kick });
            }
            Cmd::Closed { conn } => {
                // Already forgotten if it was kicked; the engine tolerates that.
                self.sinks.remove(&conn);
                let out = self.engine.disconnect(now, conn);
                self.deliver(out);
            }
            Cmd::Frame { conn, text } => {
                if !self.sinks.contains_key(&conn) {
                    return; // kicked or closed: its frames no longer count
                }
                let out = if self.engine.room_count() >= self.max_rooms && is_create_room(&text) {
                    let mut out = self.engine.tick(now);
                    out.push(Outbound {
                        to: conn,
                        msg: ServerMsg::error(ErrorCode::InvalidState),
                        close: false,
                    });
                    tracing::warn!(
                        max_rooms = self.max_rooms,
                        "room limit reached, create_room refused"
                    );
                    out
                } else {
                    self.engine.handle_frame(now, conn, &text)
                };
                self.deliver(out);
            }
        }
    }

    /// Send every outbound message to its connection's queue. A full queue kicks the
    /// connection, which makes the engine produce more output; keep going until it is quiet.
    fn deliver(&mut self, out: Vec<Outbound>) {
        let mut queue: VecDeque<Outbound> = out.into();
        while let Some(Outbound { to, msg, close }) = queue.pop_front() {
            let Some(sink) = self.sinks.get(&to) else {
                continue;
            };
            match sink.tx.try_send(Out { msg, close }) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(conn = to, "outbound queue full, dropping slow connection");
                    sink.kick.notify_one();
                    self.sinks.remove(&to);
                    queue.extend(self.engine.disconnect(self.clock.now_ms(), to));
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    // The task is gone; its `Closed` command is on its way.
                    self.sinks.remove(&to);
                }
            }
        }
    }
}

/// Cheap guard used only when the room cap is reached: is this frame a `create_room`?
fn is_create_room(frame: &str) -> bool {
    matches!(decode_client(frame), Ok(ClientMsg::CreateRoom { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_scales_and_rounds_deadlines_up() {
        let c = Clock::new(100);
        // 8001 game ms = 80.01 real ms: the wake-up must not be earlier than that.
        let at = c.instant_at(8001) - c.start;
        assert!(at >= Duration::from_micros(80_010), "{at:?}");
        assert!(at < Duration::from_micros(80_020), "{at:?}");
        assert_eq!(Clock::new(0).scale, 1, "a zero scale is clamped");
    }

    #[test]
    fn create_room_detection_is_exact() {
        assert!(is_create_room(
            r#"{"type":"create_room","config":{"mode":"course","difficulty":"facile"}}"#
        ));
        assert!(!is_create_room(r#"{"type":"ping"}"#));
        assert!(!is_create_room("not json"));
    }

    #[tokio::test]
    async fn a_full_queue_kicks_the_connection_and_tells_the_engine() {
        use crate::engine::SeededEntropy;
        use boussole_protocol::encode_client;

        let mut d = Driver {
            engine: Engine::new(Box::new(SeededEntropy::new(1))),
            clock: Clock::new(1),
            max_rooms: 10,
            sinks: HashMap::new(),
        };
        // A connection that never drains its queue (capacity 1).
        let (tx, _never_read) = mpsc::channel(1);
        let kick = Arc::new(Notify::new());
        d.on_cmd(Cmd::Open {
            conn: 7,
            out: tx,
            kick: Arc::clone(&kick),
        });
        let hello = encode_client(&ClientMsg::hello("Slow", None));
        d.on_cmd(Cmd::Frame {
            conn: 7,
            text: hello.clone(),
        }); // the welcome fills the queue
        assert_eq!(d.engine.player_count(), 1);
        assert!(d.sinks.contains_key(&7));

        d.on_cmd(Cmd::Frame {
            conn: 7,
            text: r#"{"type":"ping"}"#.into(),
        }); // the pong does not fit: slow consumer
        assert!(
            !d.sinks.contains_key(&7),
            "kicked connections are forgotten"
        );
        tokio::time::timeout(Duration::from_secs(1), kick.notified())
            .await
            .expect("the connection task is woken to drop the socket");
        assert_eq!(
            d.engine.player_count(),
            0,
            "the engine was told, and a player in no room is forgotten at once"
        );

        // Frames still in flight from the kicked connection are ignored.
        d.on_cmd(Cmd::Frame {
            conn: 7,
            text: hello,
        });
        assert_eq!(d.engine.player_count(), 0);
    }
}
