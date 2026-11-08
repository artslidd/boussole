//! Handshake: versions, dataset, nickname, framing.

mod common;
use boussole_protocol::*;
use common::*;

fn hello_with(version: u16, fingerprint: u64, nick: &str) -> ClientMsg {
    ClientMsg::Hello {
        protocol_version: version,
        dataset_fingerprint: fingerprint,
        nickname: nick.into(),
        resume_token: None,
    }
}

#[test]
fn a_matching_hello_gets_a_welcome_with_a_128_bit_hex_token() {
    let mut sim = Sim::new(1);
    let a = sim.hello(1, "Alice");
    let b = sim.hello(2, "Bob");
    assert_ne!(a.id, b.id);
    for c in [&a, &b] {
        assert_eq!(c.token.len(), 32, "128 bits as hex");
        assert!(c.token.bytes().all(|x| x.is_ascii_hexdigit()));
    }
    assert_ne!(a.token, b.token);
    assert_eq!(sim.engine.player_count(), 2);
}

#[test]
fn wrong_protocol_version_is_refused_and_the_connection_closed() {
    let mut sim = Sim::new(1);
    let fp = boussole_core::dataset_fingerprint();
    let outs = sim.send(1, hello_with(PROTOCOL_VERSION + 1, fp, "Alice"));
    assert_eq!(outs.len(), 1);
    assert!(outs[0].close);
    assert_eq!(errors(&[outs[0].msg.clone()]), [ErrorCode::BadVersion]);
    assert_eq!(sim.engine.player_count(), 0);
}

#[test]
fn dataset_mismatch_is_refused_and_the_connection_closed() {
    let mut sim = Sim::new(1);
    let fp = boussole_core::dataset_fingerprint();
    let outs = sim.send(1, hello_with(PROTOCOL_VERSION, fp ^ 1, "Alice"));
    assert!(outs[0].close);
    assert_eq!(errors(&[outs[0].msg.clone()]), [ErrorCode::DatasetMismatch]);
    assert_eq!(sim.engine.player_count(), 0);
}

#[test]
fn version_is_checked_before_the_dataset() {
    let mut sim = Sim::new(1);
    let outs = sim.send(1, hello_with(0, 0, "Alice"));
    assert_eq!(errors(&[outs[0].msg.clone()]), [ErrorCode::BadVersion]);
}

#[test]
fn anything_but_hello_first_ends_the_connection() {
    let mut sim = Sim::new(1);
    for msg in [
        ClientMsg::StartMatch,
        ClientMsg::LeaveRoom,
        ClientMsg::JoinRoom {
            code: "K7QX2".into(),
        },
        ClientMsg::Guess {
            city: "Paris".into(),
        },
        ClientMsg::CreateRoom { config: course(1) },
    ] {
        let outs = sim.send(7, msg);
        assert_eq!(outs.len(), 1);
        assert!(outs[0].close);
        assert_eq!(errors(&[outs[0].msg.clone()]), [ErrorCode::InvalidState]);
        sim.closed.clear();
    }
}

#[test]
fn ping_is_answered_even_before_hello() {
    let mut sim = Sim::new(1);
    let outs = sim.send(1, ClientMsg::Ping);
    assert_eq!(outs.len(), 1);
    assert_eq!(outs[0].msg, ServerMsg::Pong);
    assert!(!outs[0].close);
}

#[test]
fn invalid_nickname_keeps_the_connection_so_the_client_can_retry() {
    let mut sim = Sim::new(1);
    let fp = boussole_core::dataset_fingerprint();
    for bad in ["", "   ", "tab\there", &"x".repeat(17), "ctl\u{7}"] {
        let outs = sim.send(1, hello_with(PROTOCOL_VERSION, fp, bad));
        assert_eq!(
            errors(&[outs[0].msg.clone()]),
            [ErrorCode::InvalidNickname],
            "{bad:?}"
        );
        assert!(!outs[0].close);
    }
    assert_eq!(sim.engine.player_count(), 0);
    let c = sim.hello(1, "  Alice  ");
    sim.send(c.conn, ClientMsg::CreateRoom { config: course(1) });
    let snap = snapshot(&sim.take(1)).unwrap();
    assert_eq!(snap.players[0].nickname, "Alice", "trimmed");
}

#[test]
fn a_second_hello_is_an_invalid_state_and_changes_nothing() {
    let mut sim = Sim::new(1);
    let a = sim.hello(1, "Alice");
    let outs = sim.send(1, ClientMsg::hello("Mallory", None));
    assert_eq!(errors(&[outs[0].msg.clone()]), [ErrorCode::InvalidState]);
    assert!(!outs[0].close);
    assert_eq!(sim.engine.player_count(), 1);
    let _ = a;
}

#[test]
fn an_unknown_resume_token_starts_a_fresh_session() {
    let mut sim = Sim::new(1);
    let outs = sim.send(1, ClientMsg::hello("Alice", Some("0".repeat(32))));
    let ServerMsg::Welcome { resumed, .. } = &outs[0].msg else {
        panic!("{outs:?}")
    };
    assert!(!resumed);
}

#[test]
fn malformed_frames_get_bad_message_and_keep_the_connection() {
    let mut sim = Sim::new(1);
    for frame in ["", "nope", "{}", r#"{"type":"fly"}"#, r#"{"type":"guess"}"#] {
        let outs = sim.frame(1, frame);
        assert_eq!(outs.len(), 1, "{frame}");
        assert_eq!(errors(&[outs[0].msg.clone()]), [ErrorCode::BadMessage]);
        assert!(!outs[0].close);
    }
    // …and the connection still works.
    let outs = sim.frame(1, &encode_client(&ClientMsg::hello("Alice", None)));
    assert!(matches!(outs[0].msg, ServerMsg::Welcome { .. }));
}

#[test]
fn oversize_frames_get_bad_message_and_are_closed() {
    let mut sim = Sim::new(1);
    let frame = format!(
        r#"{{"type":"guess","city":"{}"}}"#,
        "x".repeat(MAX_FRAME_BYTES)
    );
    let outs = sim.frame(1, &frame);
    assert_eq!(outs.len(), 1);
    assert!(outs[0].close);
    assert_eq!(errors(&[outs[0].msg.clone()]), [ErrorCode::BadMessage]);
}
