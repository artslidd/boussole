//! A scripted WebSocket server for the client tests.
//!
//! It is a real `tokio-tungstenite` accept loop on `127.0.0.1:<free port>`, running on its own
//! thread, but with no game logic: the test is the game. It answers `hello` itself (a `welcome`,
//! or an error, or nothing, see [`Handshake`]; a known resume token is honoured like the real
//! server does) and then hands every client message to the test ([`MockServer::next_client_msg`])
//! and sends whatever the test says ([`MockServer::send`]).
#![allow(dead_code)]

use boussole_protocol::{ClientMsg, ErrorCode, ServerMsg, decode_client, encode};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::Message;

/// How the server answers `hello`.
#[derive(Debug, Clone)]
pub enum Handshake {
    /// `welcome` (a valid, known `resume_token` restores the player: `resumed: true`).
    Welcome,
    /// An error, then close (`bad_version`, `dataset_mismatch`) or keep (`invalid_nickname`).
    Reject { code: ErrorCode, close: bool },
    /// Never answer.
    Silent,
}

enum Outgoing {
    Text(String),
    Close,
    /// Drop the TCP connection without a closing handshake.
    Kill,
}

#[derive(Default)]
struct Shared {
    handshake: Option<Handshake>,
    /// Sender of the newest connection.
    current: Option<tokio_mpsc::UnboundedSender<Outgoing>>,
    connections: usize,
    hellos: Vec<ClientMsg>,
    /// resume token → player id
    tokens: HashMap<String, u32>,
    next_player: u32,
    refuse: bool,
    closed_by_client: usize,
}

pub struct MockServer {
    /// `ws://127.0.0.1:<port>/ws`
    pub url: String,
    pub port: u16,
    shared: Arc<Mutex<Shared>>,
    incoming: std_mpsc::Receiver<ClientMsg>,
    stop: Option<tokio_mpsc::UnboundedSender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MockServer {
    pub fn start() -> Self {
        Self::start_on("127.0.0.1:0")
    }

    /// Start on a given address (to restart a server on a port that was just freed).
    pub fn start_on(address: &str) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            handshake: Some(Handshake::Welcome),
            next_player: 1,
            ..Shared::default()
        }));
        let (incoming_tx, incoming) = std_mpsc::channel();
        let (stop_tx, mut stop_rx) = tokio_mpsc::unbounded_channel::<()>();
        let (ready_tx, ready_rx) = std_mpsc::channel();
        let address = address.to_owned();
        let state = shared.clone();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = TcpListener::bind(&address).await.expect("bind mock server");
                ready_tx
                    .send(listener.local_addr().unwrap().port())
                    .unwrap();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { continue };
                            if state.lock().unwrap().refuse {
                                drop(stream);
                                continue;
                            }
                            tokio::spawn(serve(stream, state.clone(), incoming_tx.clone()));
                        }
                        _ = stop_rx.recv() => break,
                    }
                }
            });
        });
        let port = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("mock server up");
        Self {
            url: format!("ws://127.0.0.1:{port}/ws"),
            port,
            shared,
            incoming,
            stop: Some(stop_tx),
            thread: Some(thread),
        }
    }

    pub fn set_handshake(&self, handshake: Handshake) {
        self.shared.lock().unwrap().handshake = Some(handshake);
    }

    /// Stop accepting connections: new ones are closed at once (the server "is down").
    pub fn refuse_connections(&self, refuse: bool) {
        self.shared.lock().unwrap().refuse = refuse;
    }

    /// Send a message on the newest connection.
    pub fn send(&self, msg: ServerMsg) {
        self.send_raw(encode(&msg));
    }

    pub fn send_raw(&self, text: String) {
        let guard = self.shared.lock().unwrap();
        let tx = guard.current.as_ref().expect("a client is connected");
        let _ = tx.send(Outgoing::Text(text));
    }

    /// Close the newest connection with a closing handshake (the server went away politely).
    pub fn close_connection(&self) {
        if let Some(tx) = &self.shared.lock().unwrap().current {
            let _ = tx.send(Outgoing::Close);
        }
    }

    /// Cut the newest connection without a closing handshake (a network drop).
    pub fn drop_connection(&self) {
        if let Some(tx) = &self.shared.lock().unwrap().current {
            let _ = tx.send(Outgoing::Kill);
        }
    }

    /// The next client message after the handshake (pings are skipped), or `None` after `wait`.
    pub fn try_next_client_msg(&self, wait: Duration) -> Option<ClientMsg> {
        let deadline = std::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.incoming.recv_timeout(left) {
                Ok(ClientMsg::Ping) => continue,
                Ok(msg) => return Some(msg),
                Err(_) => return None,
            }
        }
    }

    pub fn next_client_msg(&self) -> ClientMsg {
        self.try_next_client_msg(Duration::from_secs(5))
            .expect("the client sent a message within 5 s")
    }

    /// Every `hello` received so far, oldest first.
    pub fn hellos(&self) -> Vec<ClientMsg> {
        self.shared.lock().unwrap().hellos.clone()
    }

    /// How many sockets have been accepted.
    pub fn connections(&self) -> usize {
        self.shared.lock().unwrap().connections
    }

    /// How many sockets the client closed itself (closing handshake or EOF).
    pub fn closed_by_client(&self) -> usize {
        self.shared.lock().unwrap().closed_by_client
    }

    /// Wait until `condition` holds (polling), panicking after 5 s.
    pub fn wait_until(&self, what: &str, condition: impl Fn(&MockServer) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !condition(self) {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn serve(
    stream: tokio::net::TcpStream,
    state: Arc<Mutex<Shared>>,
    incoming: std_mpsc::Sender<ClientMsg>,
) {
    let Ok(socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut write, mut read) = socket.split();
    let (out_tx, mut out_rx) = tokio_mpsc::unbounded_channel::<Outgoing>();
    {
        let mut shared = state.lock().unwrap();
        shared.connections += 1;
        shared.current = Some(out_tx.clone());
    }

    let mut identified = false;
    loop {
        tokio::select! {
            out = out_rx.recv() => match out {
                Some(Outgoing::Text(text)) => {
                    if write.send(Message::text(text)).await.is_err() {
                        break;
                    }
                }
                Some(Outgoing::Close) => {
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
                // Leaving the loop drops both halves: the TCP connection goes away.
                Some(Outgoing::Kill) | None => break,
            },
            frame = read.next() => {
                let Some(frame) = frame else { break };
                let text = match frame {
                    Ok(Message::Text(text)) => text,
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => continue,
                };
                let Ok(msg) = decode_client(text.as_str()) else { continue };
                if !identified && let ClientMsg::Hello { resume_token, .. } = &msg {
                    let handshake = {
                        let mut shared = state.lock().unwrap();
                        shared.hellos.push(msg.clone());
                        shared.handshake.clone().unwrap_or(Handshake::Welcome)
                    };
                    match handshake {
                        Handshake::Welcome => {
                            let reply = welcome_for(&state, resume_token.as_deref());
                            identified = true;
                            let _ = out_tx.send(Outgoing::Text(encode(&reply)));
                        }
                        Handshake::Reject { code, close } => {
                            let _ = out_tx.send(Outgoing::Text(encode(&ServerMsg::error(code))));
                            if close {
                                let _ = out_tx.send(Outgoing::Close);
                            }
                        }
                        Handshake::Silent => {}
                    }
                    continue;
                }
                if matches!(msg, ClientMsg::Ping) {
                    let _ = out_tx.send(Outgoing::Text(encode(&ServerMsg::Pong)));
                    let _ = incoming.send(ClientMsg::Ping);
                    continue;
                }
                let _ = incoming.send(msg);
            }
        }
    }
    state.lock().unwrap().closed_by_client += 1;
}

/// The `welcome` for a `hello`: a known token restores its player, anything else is a new one.
fn welcome_for(state: &Mutex<Shared>, resume_token: Option<&str>) -> ServerMsg {
    let mut shared = state.lock().unwrap();
    let known = resume_token.and_then(|t| shared.tokens.get(t).map(|id| (t.to_owned(), *id)));
    match known {
        Some((token, id)) => ServerMsg::Welcome {
            player_id: id,
            resume_token: token,
            resumed: true,
        },
        None => {
            let id = shared.next_player;
            shared.next_player += 1;
            let token = format!("{id:08x}{:024x}", 0xB055_u64 + u64::from(id));
            shared.tokens.insert(token.clone(), id);
            ServerMsg::Welcome {
                player_id: id,
                resume_token: token,
                resumed: false,
            }
        }
    }
}
