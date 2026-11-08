//! The background WebSocket client.
//!
//! [`NetHandle::connect`] starts one thread with a small tokio runtime. Messages go out through
//! [`NetHandle::send`]; everything that happens comes back through the `sink` closure as a
//! [`NetEvent`] (the Slint glue forwards them to the UI thread with
//! `slint::invoke_from_event_loop`, so the UI thread never touches a socket).
//!
//! Life of a connection:
//!
//! 1. open the socket (one attempt, `connect_timeout`) and send `hello`; the first answer must be
//!    `welcome` (forwarded, and its `resume_token` remembered) or an `error` (`bad_version`,
//!    `dataset_mismatch`, `invalid_nickname`: [`NetError::Rejected`], no retry);
//! 2. forward every server message; send an app-level `ping` every `ping_interval`, and treat
//!    `silence_limit` without any frame as a dead (half-open) socket;
//! 3. when the socket drops, retry with a back-off for up to `reconnect_window` (the server keeps
//!    the player's slot for 60 s), saying `hello` with the stored resume token each time;
//! 4. give up with [`NetError::ConnectionLost`] when the window is over.

use super::NetError;
use boussole_protocol::{
    ClientMsg, MAX_SERVER_FRAME_BYTES, ServerMsg, decode_server, encode_client,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};

/// How to connect and how patient to be.
#[derive(Debug, Clone)]
pub struct NetConfig {
    /// A normalised `ws://` or `wss://` URL ([`super::normalize_address`]).
    pub url: String,
    pub nickname: String,
    /// Opening the socket and receiving `welcome` must take less than this.
    pub connect_timeout: Duration,
    /// How long to keep retrying after a drop (the server holds the slot for 60 s).
    pub reconnect_window: Duration,
    /// First pause between retries; doubled each time up to `backoff_max`.
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    /// An app-level `ping` is sent this often (also keeps a room from idling out).
    pub ping_interval: Duration,
    /// No frame at all for this long: the connection is considered dead.
    pub silence_limit: Duration,
}

impl NetConfig {
    pub fn new(url: impl Into<String>, nickname: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            nickname: nickname.into(),
            connect_timeout: Duration::from_secs(8),
            reconnect_window: Duration::from_secs(60),
            backoff_initial: Duration::from_millis(500),
            backoff_max: Duration::from_secs(5),
            ping_interval: Duration::from_secs(15),
            silence_limit: Duration::from_secs(45),
        }
    }
}

/// What the background thread reports.
// An event is moved once through a channel: boxing every server message would only add noise
// to each match arm, and size is irrelevant at a few messages per second.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum NetEvent {
    /// A message from the server (`welcome` included: `resumed` tells a new session from a
    /// restored one).
    Msg(ServerMsg),
    /// The socket dropped; attempt `attempt` (1-based) to get it back is about to start.
    Reconnecting { attempt: u32 },
    /// Nothing more will happen: the connection could not be opened, was refused, or was lost
    /// for good.
    Failed(NetError),
    /// [`NetHandle::close`] was honoured.
    Closed,
}

enum Command {
    Send(ClientMsg),
    Close,
}

/// A running connection. Dropping it closes the connection (without waiting).
pub struct NetHandle {
    commands: mpsc::UnboundedSender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl NetHandle {
    /// Start connecting in the background. `sink` is called from the network thread for every
    /// event; keep it cheap and non-blocking.
    pub fn connect(config: NetConfig, sink: impl Fn(NetEvent) + Send + Sync + 'static) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let sink: Sink = Arc::new(sink);
        let thread = std::thread::Builder::new()
            .name("boussole-net".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(run(config, receiver, sink)),
                    Err(error) => sink(NetEvent::Failed(NetError::Unreachable(error.to_string()))),
                }
            })
            .expect("spawning the network thread");
        Self {
            commands,
            thread: Some(thread),
        }
    }

    /// Queue a message. Messages sent while the socket is down are dropped.
    pub fn send(&self, msg: ClientMsg) {
        let _ = self.commands.send(Command::Send(msg));
    }

    /// Close the socket politely and stop. Messages queued before this call are still sent
    /// (put `leave_room` first). A [`NetEvent::Closed`] follows.
    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }

    /// Has the network thread ended (after [`NetHandle::close`], or a terminal failure)?
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// [`NetHandle::close`], then wait for the thread to end (tests, clean shutdown).
    pub fn close_and_wait(mut self) {
        self.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for NetHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Close);
        // The thread ends by itself; never block the caller (the UI thread) on it.
    }
}

type Sink = Arc<dyn Fn(NetEvent) + Send + Sync>;
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Why [`open`] failed. `gone` tells "nobody is there any more" from every other failure: the TCP
/// connection was refused (no listener), or a Cloudflare tunnel answered 530 / 521 (its origin,
/// the host's `cloudflared`, is gone). While reconnecting that means the server process is gone
/// and the game with it, whereas a network blip never says so.
struct OpenError {
    error: NetError,
    gone: bool,
}

impl From<NetError> for OpenError {
    fn from(error: NetError) -> Self {
        Self { error, gone: false }
    }
}

/// Why a connected session ended.
enum Ended {
    /// The player closed it (or dropped the handle).
    Closed,
    /// The socket was lost or went silent.
    Dropped,
    /// The server said goodbye with close code 1001: the game is over, do not retry.
    ServerClosed,
}

/// Close code 1001 ("going away"): a server that is shutting down.
fn is_going_away(frame: Option<&CloseFrame>) -> bool {
    frame.is_some_and(|f| f.code == CloseCode::Away)
}

async fn run(config: NetConfig, mut commands: mpsc::UnboundedReceiver<Command>, sink: Sink) {
    let (mut socket, mut token) = match open(&config, None).await {
        Ok((socket, welcome, token)) => {
            sink(NetEvent::Msg(welcome));
            (socket, token)
        }
        Err(failure) => {
            sink(NetEvent::Failed(failure.error));
            return;
        }
    };

    loop {
        match session(&mut socket, &config, &mut commands, &sink).await {
            Ended::Closed => {
                let _ = socket.close(None).await;
                sink(NetEvent::Closed);
                return;
            }
            Ended::ServerClosed => {
                sink(NetEvent::Failed(NetError::ServerClosed));
                return;
            }
            Ended::Dropped => {}
        }
        drop(socket);

        // Retry until the window is over.
        let since = Instant::now();
        let mut attempt = 0u32;
        let mut pause = config.backoff_initial;
        socket = loop {
            attempt += 1;
            sink(NetEvent::Reconnecting { attempt });
            if wait_or_close(&mut commands, pause).await {
                sink(NetEvent::Closed);
                return;
            }
            pause = (pause * 2).min(config.backoff_max);
            if since.elapsed() >= config.reconnect_window {
                sink(NetEvent::Failed(NetError::ConnectionLost));
                return;
            }
            match open(&config, Some(token.as_str())).await {
                Ok((socket, welcome, new_token)) => {
                    token = new_token;
                    sink(NetEvent::Msg(welcome));
                    break socket;
                }
                // Nobody listens any more: the server process is gone (a host app that quit
                // without its 1001 reaching us), and the game cannot come back.
                Err(OpenError { gone: true, .. }) => {
                    sink(NetEvent::Failed(NetError::ServerClosed));
                    return;
                }
                // The server was upgraded under us, refuses us, or said it is going away:
                // retrying cannot help. (A full server, 503, may have room later: keep trying.)
                Err(OpenError {
                    error: error @ (NetError::Rejected(_) | NetError::ServerClosed),
                    ..
                }) => {
                    sink(NetEvent::Failed(error));
                    return;
                }
                Err(_) => {}
            }
        };
    }
}

/// Sleep for `pause`, dropping queued sends; `true` if the player closed the connection meanwhile.
async fn wait_or_close(commands: &mut mpsc::UnboundedReceiver<Command>, pause: Duration) -> bool {
    let deadline = Instant::now() + pause;
    loop {
        tokio::select! {
            () = sleep_until(deadline) => return false,
            command = commands.recv() => match command {
                Some(Command::Send(_)) => {} // the socket is down: nothing to send it on
                Some(Command::Close) | None => return true,
            },
        }
    }
}

/// `wss://` needs a process-wide rustls crypto provider; install `ring` once. An error means
/// another provider was installed first, which is just as good.
fn install_tls_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Open the socket, say `hello`, wait for `welcome`. Returns the socket, the `welcome` message
/// and its resume token.
async fn open(
    config: &NetConfig,
    resume_token: Option<&str>,
) -> Result<(Socket, ServerMsg, String), OpenError> {
    install_tls_provider();
    let attempt = async {
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(MAX_SERVER_FRAME_BYTES))
            .max_frame_size(Some(MAX_SERVER_FRAME_BYTES));
        let (mut socket, _response) = tokio_tungstenite::connect_async_with_config(
            config.url.as_str(),
            Some(ws_config),
            false,
        )
        .await
        .map_err(|error| match error {
            // Over the connection cap: the server answers before the WebSocket upgrade.
            tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == 503 => {
                OpenError::from(NetError::ServerFull)
            }
            // Too many connections from this address: on a first connection that is the answer;
            // while reconnecting it is expected for a moment (the server may still count our
            // dead socket) and is retried.
            tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == 429 => {
                OpenError::from(NetError::TooManyConnections)
            }
            tokio_tungstenite::tungstenite::Error::Io(io)
                if io.kind() == std::io::ErrorKind::ConnectionRefused =>
            {
                OpenError {
                    error: NetError::Unreachable(io.to_string()),
                    gone: true,
                }
            }
            // Cloudflare in front of a host's tunnel: 530 (error 1033, tunnel not connected) and
            // 521 (origin down) mean the host's side is gone. Other gateway errors (502, 504)
            // can be a hiccup and are retried.
            tokio_tungstenite::tungstenite::Error::Http(response)
                if matches!(response.status().as_u16(), 521 | 530) =>
            {
                OpenError {
                    error: NetError::Unreachable(format!("HTTP {}", response.status())),
                    gone: true,
                }
            }
            other => OpenError::from(NetError::Unreachable(other.to_string())),
        })?;

        let hello = ClientMsg::hello(config.nickname.clone(), resume_token.map(str::to_owned));
        socket
            .send(Message::text(encode_client(&hello)))
            .await
            .map_err(|error| OpenError::from(NetError::Unreachable(error.to_string())))?;

        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => match decode_server(text.as_str()) {
                    Ok(welcome @ ServerMsg::Welcome { .. }) => {
                        let ServerMsg::Welcome { resume_token, .. } = &welcome else {
                            unreachable!()
                        };
                        let token = resume_token.clone();
                        return Ok((socket, welcome, token));
                    }
                    Ok(ServerMsg::Error { code, .. }) => {
                        return Err(NetError::Rejected(code).into());
                    }
                    Ok(ServerMsg::Pong) => {}
                    Ok(_) | Err(_) => return Err(NetError::Protocol.into()),
                },
                Some(Ok(Message::Close(frame))) if is_going_away(frame.as_ref()) => {
                    return Err(NetError::ServerClosed.into());
                }
                Some(Ok(Message::Close(_))) | None => {
                    return Err(NetError::Unreachable("connexion fermée".into()).into());
                }
                Some(Ok(_)) => {} // ping / pong / binary: not a handshake answer
                Some(Err(error)) => {
                    return Err(NetError::Unreachable(error.to_string()).into());
                }
            }
        }
    };
    timeout(config.connect_timeout, attempt)
        .await
        .map_err(|_| OpenError::from(NetError::Unreachable("délai dépassé".into())))?
}

/// Drive one connected socket until the player closes it or it drops.
async fn session(
    socket: &mut Socket,
    config: &NetConfig,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    sink: &Sink,
) -> Ended {
    let mut next_ping = Instant::now() + config.ping_interval;
    let mut silent_after = Instant::now() + config.silence_limit;
    loop {
        tokio::select! {
            frame = socket.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    silent_after = Instant::now() + config.silence_limit;
                    // An undecodable frame is skipped: the protocol ignores unknown fields, and
                    // a broken one must not end the game.
                    if let Ok(msg) = decode_server(text.as_str()) {
                        sink(NetEvent::Msg(msg));
                    }
                }
                Some(Ok(Message::Close(frame))) if is_going_away(frame.as_ref()) => {
                    return Ended::ServerClosed;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return Ended::Dropped,
                Some(Ok(_)) => silent_after = Instant::now() + config.silence_limit,
            },
            command = commands.recv() => match command {
                Some(Command::Send(msg)) => {
                    if socket.send(Message::text(encode_client(&msg))).await.is_err() {
                        return Ended::Dropped;
                    }
                }
                Some(Command::Close) | None => return Ended::Closed,
            },
            () = sleep_until(next_ping) => {
                next_ping = Instant::now() + config.ping_interval;
                if socket.send(Message::text(encode_client(&ClientMsg::Ping))).await.is_err() {
                    return Ended::Dropped;
                }
            },
            () = sleep_until(silent_after) => return Ended::Dropped,
        }
    }
}
