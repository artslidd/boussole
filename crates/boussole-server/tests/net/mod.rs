//! Harness for the network tests: a real server on an ephemeral port and real WebSocket
//! clients speaking the `boussole-protocol` types.
#![allow(dead_code)]

use boussole_protocol::*;
use boussole_server::{ServerConfig, ServerHandle, start};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

#[path = "../../examples/bot/strategy.rs"]
pub mod strategy;
use strategy::Strategy;

/// Longest a test waits for one message.
pub const WAIT: Duration = Duration::from_secs(10);

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Start a server on `127.0.0.1:0`; `tweak` adjusts the configuration first.
pub async fn server(tweak: impl FnOnce(&mut ServerConfig)) -> ServerHandle {
    let mut config = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        ..ServerConfig::default()
    };
    tweak(&mut config);
    start(config).await.expect("server starts")
}

/// Game time `n` times faster than the wall clock (60 s rounds in `60 / n` s).
pub fn fast(scale: u32) -> impl FnOnce(&mut ServerConfig) {
    move |c| c.time_scale = scale
}

/// What came off the socket.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // test helper, never stored in bulk
pub enum Frame {
    Msg(ServerMsg),
    /// The server closed the connection (with this close code, if it sent one).
    Closed(Option<u16>),
}

pub struct TestClient {
    ws: Ws,
    pub id: PlayerId,
    pub token: String,
    /// Raw text of every server frame received so far, in order.
    pub frames: Vec<String>,
}

impl TestClient {
    /// Open the socket without any handshake message.
    pub async fn raw(addr: SocketAddr) -> Self {
        let (ws, _) = timeout(WAIT, connect_async(format!("ws://{addr}/ws")))
            .await
            .expect("connect timeout")
            .expect("websocket handshake");
        Self {
            ws,
            id: 0,
            token: String::new(),
            frames: Vec::new(),
        }
    }

    /// Connect and complete `hello`.
    pub async fn connect(addr: SocketAddr, nickname: &str) -> Self {
        let mut c = Self::raw(addr).await;
        c.hello(nickname, None).await;
        c
    }

    /// Send `hello` and expect a `welcome`; returns whether the session was resumed.
    pub async fn hello(&mut self, nickname: &str, token: Option<String>) -> bool {
        self.send(&ClientMsg::hello(nickname, token)).await;
        match self.recv().await {
            ServerMsg::Welcome {
                player_id,
                resume_token,
                resumed,
            } => {
                self.id = player_id;
                self.token = resume_token;
                resumed
            }
            other => panic!("expected welcome, got {other:?}"),
        }
    }

    pub async fn send(&mut self, msg: &ClientMsg) {
        self.send_text(encode_client(msg)).await;
    }

    pub async fn send_text(&mut self, text: impl Into<String>) {
        self.send_ws(Message::text(text.into())).await;
    }

    pub async fn send_ws(&mut self, msg: Message) {
        use futures_util::SinkExt;
        timeout(WAIT, self.ws.send(msg))
            .await
            .expect("send timeout")
            .expect("send");
    }

    /// Like `send_ws` but reports failure instead of panicking (the server may have hung up).
    pub async fn try_send_ws(&mut self, msg: Message) -> bool {
        use futures_util::SinkExt;
        matches!(
            timeout(Duration::from_millis(500), self.ws.send(msg)).await,
            Ok(Ok(()))
        )
    }

    /// Next frame (ping/pong frames are skipped).
    pub async fn next(&mut self) -> Frame {
        use futures_util::StreamExt;
        loop {
            let item = timeout(WAIT, self.ws.next())
                .await
                .expect("timed out waiting for a server frame");
            match item {
                Some(Ok(Message::Text(text))) => {
                    self.frames.push(text.to_string());
                    return Frame::Msg(
                        decode_server(&text)
                            .unwrap_or_else(|e| panic!("bad server frame {text}: {e}")),
                    );
                }
                Some(Ok(Message::Close(frame))) => {
                    return Frame::Closed(frame.map(|f| u16::from(f.code)));
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(other)) => panic!("unexpected frame from the server: {other:?}"),
                Some(Err(_)) | None => return Frame::Closed(None),
            }
        }
    }

    /// Next message; panics if the connection closed.
    pub async fn recv(&mut self) -> ServerMsg {
        match self.next().await {
            Frame::Msg(m) => m,
            Frame::Closed(code) => panic!("connection closed (code {code:?})"),
        }
    }

    /// Skip messages until `pick` returns `Some`. A server `error` that `pick` does not claim
    /// fails the test, so unexpected refusals are never swallowed.
    pub async fn recv_where<R>(&mut self, mut pick: impl FnMut(&ServerMsg) -> Option<R>) -> R {
        loop {
            let msg = self.recv().await;
            if let Some(r) = pick(&msg) {
                return r;
            }
            if let ServerMsg::Error { code, .. } = msg {
                panic!("unexpected error {code:?} while waiting for a message");
            }
        }
    }

    pub async fn expect_error(&mut self, expected: ErrorCode) {
        match self.recv().await {
            ServerMsg::Error { code, message } => {
                assert_eq!(code, expected);
                assert_eq!(message, expected.message_fr());
            }
            other => panic!("expected error {expected:?}, got {other:?}"),
        }
    }

    /// Read until the connection is closed; returns the close code the server sent.
    pub async fn closed(&mut self) -> Option<u16> {
        loop {
            if let Frame::Closed(code) = self.next().await {
                return code;
            }
        }
    }

    pub async fn snapshot(&mut self) -> RoomSnapshot {
        self.recv_where(|m| match m {
            ServerMsg::RoomSnapshot(s) => Some(s.clone()),
            _ => None,
        })
        .await
    }

    /// Wait for a snapshot satisfying `ok`.
    pub async fn snapshot_where(&mut self, ok: impl Fn(&RoomSnapshot) -> bool) -> RoomSnapshot {
        loop {
            let s = self.snapshot().await;
            if ok(&s) {
                return s;
            }
        }
    }

    /// Create a room and return its first snapshot.
    pub async fn create(&mut self, config: RoomConfig) -> RoomSnapshot {
        self.send(&ClientMsg::CreateRoom { config }).await;
        self.snapshot().await
    }

    pub async fn join(&mut self, code: &str) -> RoomSnapshot {
        self.send(&ClientMsg::JoinRoom { code: code.into() }).await;
        self.snapshot().await
    }

    /// Drop the TCP connection without any close frame (a crashed or unplugged client).
    pub fn vanish(self) {
        drop(self);
    }
}

/// Minimal HTTP/1.0 GET: `(status, body)`.
pub async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).await.expect("connect");
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: test\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut raw = String::new();
    timeout(WAIT, s.read_to_string(&mut raw))
        .await
        .expect("http timeout")
        .unwrap();
    let status = raw
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let body = raw.split_once("\r\n\r\n").map_or("", |(_, b)| b).to_owned();
    (status, body)
}

pub fn room_config(mode: Mode, difficulty: Difficulty, rounds: u32) -> RoomConfig {
    RoomConfig {
        rounds,
        ..RoomConfig::new(mode, difficulty)
    }
}

// ---- playing -------------------------------------------------------------------------------

/// What one client saw of a whole match.
#[derive(Debug)]
pub struct Played {
    pub rounds: Vec<RoundEnded>,
    pub match_ended: MatchEnded,
    pub started: Vec<RoundStarted>,
    pub guesses: Vec<Vec<GuessInfo>>,
}

/// Play every round of a Course match with the bot's strategy until solved or out of tries.
/// Checks secret safety on every frame of every round.
pub async fn play_course(
    c: &mut TestClient,
    difficulty: Difficulty,
    rounds: usize,
    seed: u64,
) -> Played {
    let mut out = Played {
        rounds: vec![],
        match_ended: MatchEnded {
            mode: Mode::Course,
            ranking: vec![],
        },
        started: vec![],
        guesses: vec![],
    };
    let me = c.id;
    let mut strat = Strategy::new(seed);
    for round in 0..rounds {
        let started = c
            .recv_where(|m| match m {
                ServerMsg::RoundStarted(r) => Some(r.clone()),
                _ => None,
            })
            .await;
        let first_frame = c.frames.len() - 1;
        assert_eq!(usize::from(started.round_index), round + 1);
        let mut mine = Vec::new();
        strat.reset();
        loop {
            c.send(&ClientMsg::Guess {
                city: strat.next_guess().into(),
            })
            .await;
            let g = c
                .recv_where(|m| match m {
                    ServerMsg::GuessResult(g) if g.by == me => Some(g.clone()),
                    _ => None,
                })
                .await;
            let done = g.correct || g.tries_used == MAX_TRIES;
            strat.observe(&g);
            mine.push(g);
            if done {
                break;
            }
        }
        let ended = c
            .recv_where(|m| match m {
                ServerMsg::RoundEnded(e) => Some(e.clone()),
                _ => None,
            })
            .await;
        assert_round_safe(
            &c.frames[first_frame..c.frames.len() - 1],
            &ended.secret,
            difficulty,
        );
        out.started.push(started);
        out.guesses.push(mine);
        out.rounds.push(ended);
    }
    out.match_ended = c
        .recv_where(|m| match m {
            ServerMsg::MatchEnded(m) => Some(m.clone()),
            _ => None,
        })
        .await;
    out
}

/// Play every round of a Coopération match, guessing whenever it is this client's turn and
/// learning from every guess of the team.
pub async fn play_coop(
    c: &mut TestClient,
    difficulty: Difficulty,
    rounds: usize,
    seed: u64,
) -> Played {
    let mut out = Played {
        rounds: vec![],
        match_ended: MatchEnded {
            mode: Mode::Cooperation,
            ranking: vec![],
        },
        started: vec![],
        guesses: vec![],
    };
    let me = c.id;
    let mut strat = Strategy::new(seed);
    for _ in 0..rounds {
        let started = c
            .recv_where(|m| match m {
                ServerMsg::RoundStarted(r) => Some(r.clone()),
                _ => None,
            })
            .await;
        let first_frame = c.frames.len() - 1;
        let mut my_turn = matches!(started.timer, Timer::Turn { player_id, .. } if player_id == me);
        strat.reset();
        let mut seen = Vec::new();
        let ended = loop {
            if my_turn {
                my_turn = false;
                c.send(&ClientMsg::Guess {
                    city: strat.next_guess().into(),
                })
                .await;
            }
            enum Ev {
                Turn,
                Guess(GuessInfo),
                Ended(RoundEnded),
                Refused,
            }
            let ev = c
                .recv_where(|m| match m {
                    ServerMsg::TurnChanged { player_id, .. } if *player_id == me => Some(Ev::Turn),
                    ServerMsg::GuessResult(g) => Some(Ev::Guess(g.clone())),
                    ServerMsg::RoundEnded(e) => Some(Ev::Ended(e.clone())),
                    ServerMsg::Error {
                        code: ErrorCode::AlreadyGuessed | ErrorCode::NotYourTurn,
                        ..
                    } => Some(Ev::Refused),
                    _ => None,
                })
                .await;
            match ev {
                Ev::Turn => my_turn = true,
                Ev::Guess(g) => {
                    strat.observe(&g);
                    seen.push(g);
                }
                Ev::Ended(e) => break e,
                // A refused guess keeps the turn (duplicate) or was late (timeout): retry next
                // time we are told it is our turn, or right away for a duplicate.
                Ev::Refused => my_turn = true,
            }
        };
        assert_round_safe(
            &c.frames[first_frame..c.frames.len() - 1],
            &ended.secret,
            difficulty,
        );
        out.started.push(started);
        out.guesses.push(seen);
        out.rounds.push(ended);
    }
    out.match_ended = c
        .recv_where(|m| match m {
            ServerMsg::MatchEnded(m) => Some(m.clone()),
            _ => None,
        })
        .await;
    out
}

/// The frames of one round (from `round_started` up to, excluding, `round_ended`) must not
/// name the secret, except in the `guess_result` that found it, and in Difficile no hint may
/// carry a distance.
pub fn assert_round_safe(frames: &[String], secret: &str, difficulty: Difficulty) {
    let quoted = format!("\"{secret}\"");
    for frame in frames {
        let msg = decode_server(frame).expect("decodable frame");
        let finder = matches!(&msg, ServerMsg::GuessResult(g) if g.correct);
        assert!(
            finder || !frame.contains(&quoted),
            "secret {secret} leaked before round_ended: {frame}"
        );
        if difficulty == Difficulty::Difficile {
            let has_hints = matches!(
                &msg,
                ServerMsg::RoundStarted(_) | ServerMsg::RoomSnapshot(_)
            );
            if has_hints {
                let json: serde_json::Value = serde_json::from_str(frame).unwrap();
                assert!(
                    !contains_key(&json, "distance_km") || only_in_guesses(&json),
                    "distance_km in a Difficile frame: {frame}"
                );
            }
        }
    }
}

fn contains_key(v: &serde_json::Value, key: &str) -> bool {
    match v {
        serde_json::Value::Object(m) => {
            m.contains_key(key) || m.values().any(|v| contains_key(v, key))
        }
        serde_json::Value::Array(a) => a.iter().any(|v| contains_key(v, key)),
        _ => false,
    }
}

/// Guess feedback legitimately carries `distance_km` in both difficulties; only `hints` may not.
fn only_in_guesses(v: &serde_json::Value) -> bool {
    fn hints_have_distance(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::Object(m) => {
                m.get("hints")
                    .is_some_and(|h| contains_key(h, "distance_km"))
                    || m.values().any(hints_have_distance)
            }
            serde_json::Value::Array(a) => a.iter().any(hints_have_distance),
            _ => false,
        }
    }
    !hints_have_distance(v)
}
