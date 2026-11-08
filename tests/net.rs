//! The network layer against a scripted WebSocket server: handshake, handshake errors, room
//! messages both ways, keep-alive, reconnecting with the resume token, giving up, closing, and
//! `wss://` without a panic.

mod common;

use boussole::net::{NetConfig, NetError, NetEvent, NetHandle};
use boussole_protocol::{
    ClientMsg, ErrorCode, Mode, PROTOCOL_VERSION, RoomConfig, ServerMsg, decode_server,
};
use common::{Handshake, MockServer};
use std::sync::mpsc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

struct Client {
    handle: NetHandle,
    events: mpsc::Receiver<NetEvent>,
}

fn fast(url: &str) -> NetConfig {
    let mut config = NetConfig::new(url, "Alice");
    config.connect_timeout = Duration::from_millis(1500);
    config.reconnect_window = Duration::from_secs(5);
    config.backoff_initial = Duration::from_millis(20);
    config.backoff_max = Duration::from_millis(80);
    config
}

fn connect(config: NetConfig) -> Client {
    let (tx, events) = mpsc::channel();
    let handle = NetHandle::connect(config, move |event| {
        let _ = tx.send(event);
    });
    Client { handle, events }
}

impl Client {
    fn next(&self) -> NetEvent {
        self.events
            .recv_timeout(WAIT)
            .expect("a network event within 10 s")
    }

    /// The next event that is not a `Reconnecting` notification.
    fn next_skipping_reconnecting(&self) -> NetEvent {
        loop {
            match self.next() {
                NetEvent::Reconnecting { .. } => continue,
                other => return other,
            }
        }
    }

    fn nothing_for(&self, wait: Duration) -> bool {
        self.events.recv_timeout(wait).is_err()
    }
}

fn welcome_of(event: NetEvent) -> (u32, String, bool) {
    match event {
        NetEvent::Msg(ServerMsg::Welcome {
            player_id,
            resume_token,
            resumed,
        }) => (player_id, resume_token, resumed),
        other => panic!("expected welcome, got {other:?}"),
    }
}

// --- handshake ----------------------------------------------------------------------------------

#[test]
fn connecting_says_hello_with_the_nickname_version_and_fingerprint() {
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    let (player_id, token, resumed) = welcome_of(client.next());
    assert_eq!(player_id, 1);
    assert_eq!(token.len(), 32);
    assert!(!resumed);

    let hellos = server.hellos();
    assert_eq!(hellos.len(), 1);
    match &hellos[0] {
        ClientMsg::Hello {
            protocol_version,
            dataset_fingerprint,
            nickname,
            resume_token,
        } => {
            assert_eq!(*protocol_version, PROTOCOL_VERSION);
            assert_eq!(*dataset_fingerprint, boussole_core::dataset_fingerprint());
            assert_eq!(nickname, "Alice");
            assert!(resume_token.is_none(), "a first connection has no token");
        }
        other => panic!("not a hello: {other:?}"),
    }
}

#[test]
fn a_wrong_protocol_version_is_reported_and_not_retried() {
    let server = MockServer::start();
    server.set_handshake(Handshake::Reject {
        code: ErrorCode::BadVersion,
        close: true,
    });
    let client = connect(fast(&server.url));
    assert_eq!(
        client.next(),
        NetEvent::Failed(NetError::Rejected(ErrorCode::BadVersion))
    );
    assert!(
        client.nothing_for(Duration::from_millis(300)),
        "no retry after a refusal"
    );
    assert_eq!(server.connections(), 1);
    let message = NetError::Rejected(ErrorCode::BadVersion).to_string();
    assert!(message.contains("mettez le jeu à jour"), "{message}");
}

#[test]
fn a_different_city_list_is_reported_as_a_dataset_mismatch() {
    let server = MockServer::start();
    server.set_handshake(Handshake::Reject {
        code: ErrorCode::DatasetMismatch,
        close: true,
    });
    let client = connect(fast(&server.url));
    assert_eq!(
        client.next(),
        NetEvent::Failed(NetError::Rejected(ErrorCode::DatasetMismatch))
    );
}

#[test]
fn an_invalid_nickname_is_reported_to_the_player() {
    let server = MockServer::start();
    server.set_handshake(Handshake::Reject {
        code: ErrorCode::InvalidNickname,
        close: false,
    });
    let client = connect(fast(&server.url));
    assert_eq!(
        client.next(),
        NetEvent::Failed(NetError::Rejected(ErrorCode::InvalidNickname))
    );
}

#[test]
fn a_server_that_never_answers_hello_times_out() {
    let server = MockServer::start();
    server.set_handshake(Handshake::Silent);
    let mut config = fast(&server.url);
    config.connect_timeout = Duration::from_millis(300);
    let client = connect(config);
    assert!(matches!(
        client.next(),
        NetEvent::Failed(NetError::Unreachable(_))
    ));
}

#[test]
fn nothing_listening_means_server_unreachable() {
    // bind then drop to get a port nobody listens on
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let client = connect(fast(&format!("ws://127.0.0.1:{port}/ws")));
    match client.next() {
        NetEvent::Failed(error @ NetError::Unreachable(_)) => {
            assert!(
                error.to_string().starts_with("Serveur injoignable"),
                "{error}"
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_wss_url_to_a_plain_server_fails_cleanly_instead_of_panicking() {
    // Proves the TLS stack is wired (a crypto provider is installed): the handshake is refused
    // by the plain-text server and surfaces as an ordinary connection failure.
    let server = MockServer::start();
    let client = connect(fast(&format!("wss://127.0.0.1:{}/ws", server.port)));
    assert!(matches!(
        client.next(),
        NetEvent::Failed(NetError::Unreachable(_))
    ));
}

// --- messages both ways -------------------------------------------------------------------------

#[test]
fn messages_flow_in_both_directions() {
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    welcome_of(client.next());

    let config = RoomConfig::new(Mode::Course, boussole_protocol::Difficulty::Facile);
    client.handle.send(ClientMsg::CreateRoom { config });
    assert_eq!(server.next_client_msg(), ClientMsg::CreateRoom { config });

    server.send(ServerMsg::error(ErrorCode::RoomFull));
    assert_eq!(
        client.next(),
        NetEvent::Msg(ServerMsg::error(ErrorCode::RoomFull))
    );

    // a frame the client cannot decode is skipped, the next one still arrives
    server.send_raw("{not json".into());
    server.send_raw(r#"{"type":"from_the_future"}"#.into());
    server.send(ServerMsg::Pong);
    assert_eq!(client.next(), NetEvent::Msg(ServerMsg::Pong));
}

#[test]
fn the_client_pings_to_keep_the_connection_alive() {
    let server = MockServer::start();
    let mut config = fast(&server.url);
    config.ping_interval = Duration::from_millis(60);
    let client = connect(config);
    welcome_of(client.next());
    // The mock answers every ping with a pong: the client sees them as ordinary messages.
    let mut pongs = 0;
    while pongs < 2 {
        if client.next() == NetEvent::Msg(ServerMsg::Pong) {
            pongs += 1;
        }
    }
}

// --- reconnecting ------------------------------------------------------------------------------

#[test]
fn a_dropped_socket_is_reopened_with_the_resume_token() {
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    let (player_id, token, _) = welcome_of(client.next());

    server.drop_connection();
    assert!(matches!(
        client.next(),
        NetEvent::Reconnecting { attempt: 1 }
    ));

    let (again_id, again_token, resumed) = welcome_of(client.next_skipping_reconnecting());
    assert!(resumed, "the server recognised the token");
    assert_eq!((again_id, again_token.clone()), (player_id, token.clone()));

    let hellos = server.hellos();
    assert_eq!(hellos.len(), 2);
    match &hellos[1] {
        ClientMsg::Hello { resume_token, .. } => {
            assert_eq!(resume_token.as_deref(), Some(token.as_str()))
        }
        other => panic!("not a hello: {other:?}"),
    }

    // and the new socket works
    client.handle.send(ClientMsg::LeaveRoom);
    assert_eq!(server.next_client_msg(), ClientMsg::LeaveRoom);
}

#[test]
fn the_view_survives_a_reconnect_because_the_snapshot_arrives_again() {
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    welcome_of(client.next());
    server.drop_connection();
    welcome_of(client.next_skipping_reconnecting());
    // what the real server sends right after a resumed welcome
    let snapshot = decode_server(include_str!(
        "../crates/boussole-protocol/tests/fixtures/server/room_snapshot_lobby.json"
    ))
    .unwrap();
    server.send(snapshot.clone());
    assert_eq!(client.next(), NetEvent::Msg(snapshot));
}

#[test]
fn messages_sent_while_the_socket_is_down_are_dropped_not_replayed() {
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    welcome_of(client.next());
    server.refuse_connections(true);
    server.drop_connection();
    assert!(matches!(client.next(), NetEvent::Reconnecting { .. }));
    client.handle.send(ClientMsg::StartMatch); // lost: there is no socket
    std::thread::sleep(Duration::from_millis(150));
    server.refuse_connections(false);
    welcome_of(client.next_skipping_reconnecting());
    assert!(
        server
            .try_next_client_msg(Duration::from_millis(300))
            .is_none(),
        "a stale start_match must not be replayed after reconnecting"
    );
}

#[test]
fn it_keeps_retrying_with_a_backoff_until_the_server_is_back() {
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    welcome_of(client.next());
    server.refuse_connections(true);
    server.drop_connection();

    let mut attempts = 0;
    while attempts < 3 {
        if let NetEvent::Reconnecting { attempt } = client.next() {
            attempts = attempt;
        }
    }
    server.refuse_connections(false);
    let (_, _, resumed) = welcome_of(client.next_skipping_reconnecting());
    assert!(resumed);
}

#[test]
fn it_gives_up_when_the_reconnect_window_is_over() {
    let server = MockServer::start();
    let mut config = fast(&server.url);
    config.reconnect_window = Duration::from_millis(400);
    let client = connect(config);
    welcome_of(client.next());
    server.refuse_connections(true);
    server.drop_connection();

    let started = std::time::Instant::now();
    loop {
        match client.next() {
            NetEvent::Reconnecting { .. } => continue,
            NetEvent::Failed(NetError::ConnectionLost) => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(
        started.elapsed() >= Duration::from_millis(350),
        "kept trying for the window"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "and stopped soon after"
    );
    assert!(
        NetError::ConnectionLost
            .to_string()
            .starts_with("Connexion perdue"),
        "French message"
    );
}

#[test]
fn a_server_restart_on_the_same_port_gives_an_unresumed_welcome() {
    let first = MockServer::start();
    let port = first.port;
    let client = connect(fast(&first.url));
    welcome_of(client.next());
    drop(first);
    // restart (fresh token table) on the same port; give the OS a moment to release it
    let second = loop {
        match std::panic::catch_unwind(|| MockServer::start_on(&format!("127.0.0.1:{port}"))) {
            Ok(server) => break server,
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let (_, _, resumed) = welcome_of(client.next_skipping_reconnecting());
    assert!(!resumed, "the new server does not know our token");
    assert!(second.hellos().len() == 1);
}

#[test]
fn a_silent_socket_is_treated_as_dropped() {
    // A half-open connection: nothing comes in, not even pongs.
    let server = MockServer::start();
    let mut config = fast(&server.url);
    config.silence_limit = Duration::from_millis(300);
    config.ping_interval = Duration::from_secs(60); // no ping, hence no pong either
    let client = connect(config);
    welcome_of(client.next());
    assert!(matches!(
        client.next(),
        NetEvent::Reconnecting { attempt: 1 }
    ));
    let (_, _, resumed) = welcome_of(client.next_skipping_reconnecting());
    assert!(resumed);
}

// --- the server going away, the server being full ------------------------------------------------

#[test]
fn a_server_shutdown_close_1001_means_the_host_closed_the_game_and_is_not_retried() {
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    welcome_of(client.next());
    server.close_connection_with_code(1001);
    assert_eq!(client.next(), NetEvent::Failed(NetError::ServerClosed));
    assert!(
        client.nothing_for(Duration::from_millis(400)),
        "no reconnecting after the host closed the game"
    );
    assert_eq!(server.connections(), 1, "no new socket was opened");
    assert_eq!(
        NetError::ServerClosed.to_string(),
        "L’hôte a fermé la partie."
    );
}

#[test]
fn other_close_codes_are_still_treated_as_a_network_blip_and_retried() {
    // 1011: the server hit an internal error; the room may well still exist.
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    welcome_of(client.next());
    server.close_connection_with_code(1011);
    assert!(matches!(
        client.next(),
        NetEvent::Reconnecting { attempt: 1 }
    ));
    let (_, _, resumed) = welcome_of(client.next_skipping_reconnecting());
    assert!(resumed);
}

#[test]
fn a_server_over_its_connection_cap_is_reported_as_full() {
    let server = MockServer::start();
    server.answer_with_http_status(Some(503));
    let client = connect(fast(&server.url));
    assert_eq!(client.next(), NetEvent::Failed(NetError::ServerFull));
    let message = NetError::ServerFull.to_string();
    assert!(message.contains("plein"), "{message}");
    assert_eq!(server.hellos().len(), 0, "the upgrade never happened");
}

#[test]
fn a_full_server_while_reconnecting_is_just_retried_until_there_is_room() {
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    welcome_of(client.next());
    server.answer_with_http_status(Some(503));
    server.drop_connection();
    assert!(matches!(
        client.next(),
        NetEvent::Reconnecting { attempt: 1 }
    ));
    std::thread::sleep(Duration::from_millis(150));
    server.answer_with_http_status(None);
    let (_, _, resumed) = welcome_of(client.next_skipping_reconnecting());
    assert!(
        resumed,
        "the slot was kept: resumed once the server had room"
    );
}

// --- closing ------------------------------------------------------------------------------------

#[test]
fn close_sends_what_was_queued_then_closes_the_socket() {
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    welcome_of(client.next());
    client.handle.send(ClientMsg::LeaveRoom);
    client.handle.close();
    assert_eq!(client.next(), NetEvent::Closed);
    assert_eq!(
        server.next_client_msg(),
        ClientMsg::LeaveRoom,
        "leave_room went out first"
    );
    server.wait_until("the socket to close", |s| s.closed_by_client() == 1);
}

#[test]
fn closing_while_reconnecting_stops_the_retries() {
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    welcome_of(client.next());
    server.refuse_connections(true);
    server.drop_connection();
    assert!(matches!(client.next(), NetEvent::Reconnecting { .. }));
    client.handle.close();
    loop {
        match client.next() {
            NetEvent::Reconnecting { .. } => continue,
            NetEvent::Closed => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    client.handle.close(); // harmless twice
}

#[test]
fn dropping_the_handle_closes_the_connection() {
    let server = MockServer::start();
    let client = connect(fast(&server.url));
    welcome_of(client.next());
    drop(client.handle);
    server.wait_until("the socket to close", |s| s.closed_by_client() == 1);
}
