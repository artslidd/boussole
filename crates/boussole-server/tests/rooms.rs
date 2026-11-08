//! Rooms: codes, joining, host rights, host transfer, lobby configuration.

mod common;
use boussole_protocol::*;
use common::*;

#[test]
fn creator_becomes_host_and_gets_a_lobby_snapshot() {
    let mut sim = Sim::new(1);
    let a = sim.hello(1, "Alice");
    sim.send(1, ClientMsg::CreateRoom { config: course(3) });
    let snap = snapshot(&sim.take(1)).unwrap();
    assert_eq!(snap.phase, RoomPhase::Lobby);
    assert_eq!(snap.config, course(3));
    assert_eq!(snap.players.len(), 1);
    assert_eq!(snap.players[0].id, a.id);
    assert!(snap.players[0].host && snap.players[0].connected);
    assert_eq!(snap.players[0].score, 0);
    assert!(snap.round.is_none() && snap.intermission.is_none());
}

#[test]
fn room_codes_use_the_alphabet_and_are_unique() {
    let mut sim = Sim::new(7);
    let mut codes = std::collections::BTreeSet::new();
    for i in 1..=400u64 {
        let c = sim.hello(i, &format!("P{i}"));
        let code = sim.create(&c, course(1));
        assert_eq!(code.chars().count(), ROOM_CODE_LEN);
        assert!(
            code.chars().all(|ch| ROOM_CODE_ALPHABET.contains(ch)),
            "{code}"
        );
        assert!(codes.insert(code), "duplicate room code");
    }
    assert_eq!(sim.engine.room_count(), 400);
}

#[test]
fn a_colliding_room_code_is_redrawn() {
    // hello = 2 draws (token); create = 5 draws (one character each).
    let same = [0, 1, 2, 3, 4];
    let different = [5, 6, 7, 8, 9];
    let mut script = vec![100, 101];
    script.extend(same);
    script.extend([102, 103]);
    script.extend(same); // second room: collides with the first…
    script.extend(different); // …so it draws again
    let mut sim = Sim::with_entropy(Box::new(Scripted::new(script)));
    let a = sim.hello(1, "A");
    let first = sim.create(&a, course(1));
    let b = sim.hello(2, "B");
    let second = sim.create(&b, course(1));
    let alphabet: Vec<char> = ROOM_CODE_ALPHABET.chars().collect();
    let expect = |idx: [usize; 5]| idx.iter().map(|i| alphabet[*i]).collect::<String>();
    assert_eq!(first, expect([0, 1, 2, 3, 4]));
    assert_eq!(second, expect([5, 6, 7, 8, 9]));
}

#[test]
fn joining_is_case_insensitive_and_players_are_kept_in_join_order() {
    let mut sim = Sim::new(2);
    let a = sim.hello(1, "Alice");
    let code = sim.create(&a, course(1));
    let b = sim.hello(2, "Bob");
    let c = sim.hello(3, "Cleo");
    sim.take(1);
    let reply = sim.join(&b, &code.to_lowercase());
    assert!(errors(&reply).is_empty());
    let reply = sim.join(&c, &format!(" {} ", code));
    assert!(errors(&reply).is_empty());
    let snap = snapshot(&sim.take(1)).unwrap();
    let names: Vec<_> = snap.players.iter().map(|p| p.nickname.as_str()).collect();
    assert_eq!(names, ["Alice", "Bob", "Cleo"]);
}

#[test]
fn every_member_gets_a_snapshot_when_someone_joins() {
    let mut sim = Sim::new(2);
    let a = sim.hello(1, "Alice");
    let code = sim.create(&a, course(1));
    let b = sim.hello(2, "Bob");
    sim.take(1);
    let outs = sim.send(b.conn, ClientMsg::JoinRoom { code });
    for conn in [1, 2] {
        let snap = snapshot(&for_conn(&outs, conn)).expect("snapshot");
        assert_eq!(snap.players.len(), 2);
    }
}

#[test]
fn nicknames_are_unique_case_insensitively_inside_a_room() {
    let mut sim = Sim::new(3);
    let a = sim.hello(1, "Élodie");
    let code = sim.create(&a, course(1));
    let zed = sim.hello(4, "Zed");
    assert!(errors(&sim.join(&zed, &code)).is_empty());
    let b = sim.hello(2, "éLODIE");
    assert_eq!(errors(&sim.join(&b, &code)), [ErrorCode::NicknameTaken]);
    // The same nickname is fine in another room.
    let c = sim.hello(3, "Élodie");
    let other = sim.create(&c, course(1));
    assert_ne!(other, code);
    // After the first Élodie leaves, the name is free again.
    sim.send(1, ClientMsg::LeaveRoom);
    assert!(errors(&sim.join(&b, &code)).is_empty());
}

#[test]
fn join_errors() {
    let mut sim = Sim::new(4);
    let a = sim.hello(1, "Alice");
    let b = sim.hello(2, "Bob");
    assert_eq!(errors(&sim.join(&b, "ZZZZZ")), [ErrorCode::RoomNotFound]);
    assert_eq!(errors(&sim.join(&b, "abc")), [ErrorCode::RoomNotFound]);
    assert_eq!(errors(&sim.join(&b, "")), [ErrorCode::RoomNotFound]);
    assert_eq!(errors(&sim.join(&b, "AB0DE")), [ErrorCode::RoomNotFound]);
    // Already in a room: can neither create nor join another.
    let code = sim.create(&a, course(1));
    assert_eq!(errors(&sim.join(&a, &code)), [ErrorCode::InvalidState]);
    sim.send(1, ClientMsg::CreateRoom { config: course(1) });
    assert_eq!(errors(&sim.take(1)), [ErrorCode::InvalidState]);
    // Leaving when in no room.
    sim.send(2, ClientMsg::LeaveRoom);
    assert_eq!(errors(&sim.take(2)), [ErrorCode::InvalidState]);
}

#[test]
fn a_full_room_refuses_more_players() {
    let mut sim = Sim::new(5);
    let config = RoomConfig {
        max_players: 2,
        ..course(1)
    };
    let (_players, code) = sim.lobby(2, config);
    let c = sim.hello(3, "Cleo");
    assert_eq!(errors(&sim.join(&c, &code)), [ErrorCode::RoomFull]);
    // A disconnected player still holds a seat during the grace period.
    sim.disconnect(2);
    assert_eq!(errors(&sim.join(&c, &code)), [ErrorCode::RoomFull]);
    // Once the seat is released it can be taken.
    sim.advance(RESUME_GRACE_MS);
    assert!(errors(&sim.join(&c, &code)).is_empty());
}

#[test]
fn an_eight_player_room_fills_up() {
    let mut sim = Sim::new(5);
    let (players, code) = sim.lobby(8, course(1));
    let extra = sim.hello(99, "Extra");
    assert_eq!(errors(&sim.join(&extra, &code)), [ErrorCode::RoomFull]);
    sim.send(players[0].conn, ClientMsg::StartMatch);
    sim.check();
}

#[test]
fn joining_a_room_with_a_match_in_progress_is_refused() {
    let mut sim = Sim::new(6);
    let (_p, code) = sim.started(2, course(2));
    let c = sim.hello(3, "Cleo");
    assert_eq!(errors(&sim.join(&c, &code)), [ErrorCode::MatchInProgress]);
    // Also during the intermission.
    let secret = sim.secret(&code);
    sim.guess(
        &Client {
            conn: 1,
            id: 1,
            token: String::new(),
        },
        secret,
    );
    sim.guess(
        &Client {
            conn: 2,
            id: 2,
            token: String::new(),
        },
        secret,
    );
    assert_eq!(errors(&sim.join(&c, &code)), [ErrorCode::MatchInProgress]);
}

#[test]
fn only_the_host_can_update_the_config_or_start() {
    let mut sim = Sim::new(8);
    let (p, _code) = sim.lobby(3, course(3));
    for msg in [
        ClientMsg::StartMatch,
        ClientMsg::UpdateConfig { config: course(2) },
    ] {
        sim.send(p[1].conn, msg);
        assert_eq!(errors(&sim.take(p[1].conn)), [ErrorCode::NotHost]);
    }
    sim.send(p[0].conn, ClientMsg::UpdateConfig { config: coop(2) });
    for c in &p {
        let snap = snapshot(&sim.take(c.conn)).expect("everyone sees the new config");
        assert_eq!(snap.config, coop(2));
    }
}

#[test]
fn update_config_is_validated_and_lobby_only() {
    let mut sim = Sim::new(8);
    let (p, _code) = sim.lobby(3, course(3));
    let host = p[0].conn;
    let bad = [
        RoomConfig {
            rounds: 0,
            ..course(3)
        },
        RoomConfig {
            rounds: 11,
            ..course(3)
        },
        RoomConfig {
            max_players: 1,
            ..course(3)
        },
        RoomConfig {
            max_players: 9,
            ..course(3)
        },
        RoomConfig {
            round_time_s: 59,
            ..course(3)
        },
        RoomConfig {
            round_time_s: 301,
            ..course(3)
        },
        RoomConfig {
            turn_time_s: 14,
            ..course(3)
        },
        RoomConfig {
            turn_time_s: 121,
            ..course(3)
        },
        // Cannot shrink below the players already seated.
        RoomConfig {
            max_players: 2,
            ..course(3)
        },
    ];
    for config in bad {
        sim.send(host, ClientMsg::UpdateConfig { config });
        assert_eq!(
            errors(&sim.take(host)),
            [ErrorCode::InvalidConfig],
            "{config:?}"
        );
    }
    let edge = RoomConfig {
        rounds: 10,
        max_players: 3,
        round_time_s: 300,
        turn_time_s: 120,
        ..course(3)
    };
    sim.send(host, ClientMsg::UpdateConfig { config: edge });
    assert!(errors(&sim.take(host)).is_empty());
    sim.send(host, ClientMsg::StartMatch);
    sim.drain_all();
    sim.send(host, ClientMsg::UpdateConfig { config: course(2) });
    assert_eq!(errors(&sim.take(host)), [ErrorCode::InvalidState]);
}

#[test]
fn create_room_validates_the_config() {
    let mut sim = Sim::new(9);
    sim.hello(1, "Alice");
    sim.send(
        1,
        ClientMsg::CreateRoom {
            config: RoomConfig {
                rounds: 99,
                ..course(1)
            },
        },
    );
    assert_eq!(errors(&sim.take(1)), [ErrorCode::InvalidConfig]);
    assert_eq!(sim.engine.room_count(), 0);
}

#[test]
fn starting_needs_two_connected_players() {
    // Alone.
    let mut sim = Sim::new(10);
    let a = sim.hello(1, "Alice");
    sim.create(&a, course(1));
    sim.send(1, ClientMsg::StartMatch);
    assert_eq!(errors(&sim.take(1)), [ErrorCode::NotEnoughPlayers]);

    // Two seats, but one player's socket is down.
    let mut sim = Sim::new(10);
    let (p, _code) = sim.lobby(2, course(1));
    sim.disconnect(p[1].conn);
    sim.send(p[0].conn, ClientMsg::StartMatch);
    assert_eq!(errors(&sim.take(p[0].conn)), [ErrorCode::NotEnoughPlayers]);
}

#[test]
fn guess_and_start_outside_their_phase_are_invalid_state() {
    let mut sim = Sim::new(11);
    let (p, _code) = sim.lobby(2, course(1));
    sim.guess(&p[0], "Paris");
    assert_eq!(errors(&sim.take(p[0].conn)), [ErrorCode::InvalidState]);
    sim.send(p[0].conn, ClientMsg::StartMatch);
    sim.drain_all();
    sim.send(p[0].conn, ClientMsg::StartMatch);
    assert_eq!(errors(&sim.take(p[0].conn)), [ErrorCode::InvalidState]);
}

#[test]
fn leaving_the_room_hands_the_host_to_the_next_player_in_join_order() {
    let mut sim = Sim::new(12);
    let (p, _code) = sim.lobby(3, course(1));
    let outs = sim.send(p[0].conn, ClientMsg::LeaveRoom);
    for c in &p[1..] {
        let snap = snapshot(&for_conn(&outs, c.conn)).expect("broadcast");
        assert_eq!(snap.players.len(), 2);
        let host: Vec<_> = snap
            .players
            .iter()
            .filter(|x| x.host)
            .map(|x| x.id)
            .collect();
        assert_eq!(host, [p[1].id], "second player is the new host");
    }
    // The leaver is out of the room but still connected: they can create a new one.
    assert!(snapshot(&for_conn(&outs, p[0].conn)).is_none());
    sim.send(p[0].conn, ClientMsg::CreateRoom { config: course(1) });
    assert!(errors(&sim.take(p[0].conn)).is_empty());
    // The new host can act.
    sim.send(p[1].conn, ClientMsg::StartMatch);
    assert!(errors(&sim.take(p[1].conn)).is_empty());
    sim.check();
}

#[test]
fn a_dropped_host_hands_over_to_the_next_connected_player() {
    let mut sim = Sim::new(13);
    let (p, _code) = sim.lobby(3, course(1));
    sim.disconnect(p[1].conn); // second player is away too
    let outs = sim.disconnect(p[0].conn);
    let snap = snapshot(&for_conn(&outs, p[2].conn)).unwrap();
    let host: Vec<_> = snap
        .players
        .iter()
        .filter(|x| x.host)
        .map(|x| x.id)
        .collect();
    assert_eq!(host, [p[2].id], "skips the disconnected second player");
    assert!(snap.players.iter().filter(|x| !x.connected).count() == 2);
    // The old host resuming does not take it back.
    sim.send(10, ClientMsg::hello("x", Some(p[0].token.clone())));
    let snap = snapshot(&sim.take(10)).unwrap();
    assert!(!snap.players[0].host && snap.players[2].host);
}

#[test]
fn a_host_removed_after_the_grace_period_hands_over_too() {
    let mut sim = Sim::new(14);
    let (p, _code) = sim.lobby(2, course(1));
    // Only the host's slot is held; the other player stays connected.
    sim.disconnect(p[0].conn);
    sim.take(p[1].conn);
    let outs = sim.advance(RESUME_GRACE_MS);
    let snap = snapshot(&for_conn(&outs, p[1].conn)).unwrap();
    assert_eq!(snap.players.len(), 1);
    assert!(snap.players[0].host);
    sim.check();
}

#[test]
fn a_room_with_no_players_is_deleted_at_once() {
    let mut sim = Sim::new(15);
    let (p, code) = sim.lobby(2, course(1));
    sim.send(p[0].conn, ClientMsg::LeaveRoom);
    assert_eq!(sim.engine.room_count(), 1);
    sim.send(p[1].conn, ClientMsg::LeaveRoom);
    assert_eq!(sim.engine.room_count(), 0);
    let c = sim.hello(3, "Cleo");
    assert_eq!(errors(&sim.join(&c, &code)), [ErrorCode::RoomNotFound]);
    sim.check();
}

#[test]
fn a_player_outside_any_room_is_forgotten_on_disconnect() {
    let mut sim = Sim::new(16);
    let a = sim.hello(1, "Alice");
    sim.disconnect(1);
    assert_eq!(sim.engine.player_count(), 0);
    // Their token is worthless now.
    sim.send(2, ClientMsg::hello("Alice", Some(a.token)));
    let ServerMsg::Welcome { resumed, .. } = sim.take(2)[0].clone() else {
        panic!()
    };
    assert!(!resumed);
}
