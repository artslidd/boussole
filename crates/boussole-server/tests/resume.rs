//! Disconnection, resumption within the grace period, expiry and idle rooms.

mod common;
use boussole_protocol::*;
use common::*;

fn welcome(msgs: &[ServerMsg]) -> (PlayerId, String, bool) {
    msgs.iter()
        .find_map(|m| match m {
            ServerMsg::Welcome {
                player_id,
                resume_token,
                resumed,
            } => Some((*player_id, resume_token.clone(), *resumed)),
            _ => None,
        })
        .expect("welcome")
}

#[test]
fn a_lost_socket_keeps_the_slot_and_is_shown_as_disconnected() {
    let mut sim = Sim::new(1);
    let (p, _code) = sim.lobby(2, course(1));
    let outs = sim.disconnect(p[1].conn);
    let snap = snapshot(&for_conn(&outs, p[0].conn)).unwrap();
    assert_eq!(snap.players.len(), 2);
    assert!(snap.players[0].connected && !snap.players[1].connected);
    assert_eq!(sim.engine.next_deadline(), Some(sim.now + RESUME_GRACE_MS));
}

#[test]
fn resuming_within_60_seconds_restores_identity_and_lobby_state() {
    let mut sim = Sim::new(2);
    let (p, code) = sim.lobby(2, course(1));
    sim.disconnect(p[1].conn);
    sim.drain_all();
    sim.advance(RESUME_GRACE_MS - 1);
    let outs = sim.send(50, ClientMsg::hello("ignored", Some(p[1].token.clone())));
    let mine = for_conn(&outs, 50);
    let (id, token, resumed) = welcome(&mine);
    assert_eq!(
        (id, token.as_str(), resumed),
        (p[1].id, p[1].token.as_str(), true)
    );
    let snap = snapshot(&mine).expect("full snapshot after resume");
    assert_eq!(snap.code, code);
    assert!(snap.players.iter().all(|x| x.connected));
    assert_eq!(
        snap.players[1].nickname, "P2",
        "nickname of the old identity"
    );
    // The other player sees them come back.
    let theirs = snapshot(&for_conn(&outs, p[0].conn)).unwrap();
    assert!(theirs.players.iter().all(|x| x.connected));
    // The new socket now carries the player (and the old id is not reused for it).
    sim.send(50, ClientMsg::LeaveRoom);
    assert!(errors(&sim.take(50)).is_empty());
    sim.check();
}

#[test]
fn resuming_mid_round_restores_the_round_state_for_that_player() {
    let mut sim = Sim::new(3);
    let (p, code) = sim.started(3, course(2));
    let secret = sim.secret(&code);
    let wrong = wrong_cities(secret, 2);
    sim.guess(&p[1], wrong[0]);
    sim.guess(&p[1], wrong[1]);
    sim.guess(&p[0], wrong[0]);
    sim.disconnect(p[1].conn);
    sim.drain_all();
    sim.advance(10_000);
    let outs = sim.send(50, ClientMsg::hello("x", Some(p[1].token.clone())));
    let snap = snapshot(&for_conn(&outs, 50)).unwrap();
    assert_eq!(snap.phase, RoomPhase::RoundActive);
    let round = snap.round.expect("round state");
    assert_eq!(round.start.round_index, 1);
    assert_eq!(round.start.hints.len(), 5);
    assert_eq!(
        round.start.timer,
        Timer::Round {
            remaining_ms: 60_000 - 10_000
        }
    );
    // Their own two guesses, with feedback, in order…
    let cities: Vec<_> = round.guesses.iter().map(|g| g.city.as_str()).collect();
    assert_eq!(cities, [wrong[0], wrong[1]]);
    assert!(round.guesses.iter().all(|g| g.by == p[1].id));
    assert_eq!(round.guesses[1].tries_used, 2);
    // …and the progress of everybody, without anybody's cities.
    let tries: Vec<_> = round
        .progress
        .iter()
        .map(|x| (x.player_id, x.tries_used))
        .collect();
    assert_eq!(tries, [(p[0].id, 1), (p[1].id, 2), (p[2].id, 0)]);
    // They can keep playing from where they were: the same city is still a duplicate.
    sim.send(
        50,
        ClientMsg::Guess {
            city: wrong[0].into(),
        },
    );
    assert_eq!(errors(&sim.take(50)), [ErrorCode::AlreadyGuessed]);
    sim.check();
}

#[test]
fn resuming_in_a_coop_round_shows_the_team_guesses_and_the_current_turn() {
    let mut sim = Sim::new(4);
    let (p, code) = sim.started(2, coop(1));
    let secret = sim.secret(&code);
    let wrong = wrong_cities(secret, 1);
    sim.guess(&p[0], wrong[0]); // turn → P2
    sim.disconnect(p[0].conn);
    sim.drain_all();
    let outs = sim.send(50, ClientMsg::hello("x", Some(p[0].token.clone())));
    let round = snapshot(&for_conn(&outs, 50)).unwrap().round.unwrap();
    assert_eq!(round.guesses.len(), 1);
    assert_eq!(round.guesses[0].by, p[0].id);
    assert_eq!(
        round.start.timer,
        Timer::Turn {
            player_id: p[1].id,
            remaining_ms: 30_000
        }
    );
    assert!(round.progress.is_empty());
}

#[test]
fn resuming_during_an_intermission_includes_the_round_that_just_ended() {
    let mut sim = Sim::new(5);
    let (p, code) = sim.started(2, course(2));
    let secret = sim.secret(&code);
    sim.guess(&p[0], secret);
    sim.guess(&p[1], secret);
    sim.disconnect(p[1].conn);
    sim.drain_all();
    sim.advance(3_000);
    let outs = sim.send(50, ClientMsg::hello("x", Some(p[1].token.clone())));
    let snap = snapshot(&for_conn(&outs, 50)).unwrap();
    assert_eq!(snap.phase, RoomPhase::Intermission);
    let pause = snap.intermission.unwrap();
    assert_eq!(pause.next_round_index, 2);
    assert_eq!(pause.remaining_ms, INTERMISSION_MS - 3_000);
    assert_eq!(pause.last_round.secret, secret);
    assert_eq!(pause.last_round.next_round_in_ms, Some(pause.remaining_ms));
}

#[test]
fn after_60_seconds_the_player_is_removed_and_the_token_is_dead() {
    let mut sim = Sim::new(6);
    let (p, _code) = sim.lobby(3, course(1));
    sim.disconnect(p[2].conn);
    sim.drain_all();
    assert!(sim.advance(RESUME_GRACE_MS - 1).is_empty(), "nothing yet");
    let outs = sim.advance(1);
    for c in &p[..2] {
        let snap = snapshot(&for_conn(&outs, c.conn)).expect("removal is broadcast");
        let ids: Vec<_> = snap.players.iter().map(|x| x.id).collect();
        assert_eq!(ids, [p[0].id, p[1].id]);
    }
    assert_eq!(sim.engine.player_count(), 2);
    sim.send(50, ClientMsg::hello("P3", Some(p[2].token.clone())));
    let (id, _, resumed) = welcome(&sim.take(50));
    assert!(!resumed);
    assert_ne!(id, p[2].id);
    sim.check();
}

#[test]
fn removal_is_decided_by_time_even_if_tick_is_late() {
    // The next message arrives long after the deadline: the token is already dead.
    let mut sim = Sim::new(7);
    let (p, _code) = sim.lobby(2, course(1));
    sim.disconnect(p[1].conn);
    sim.now += 5 * 60_000;
    sim.send(50, ClientMsg::hello("P2", Some(p[1].token.clone())));
    let (_, _, resumed) = welcome(&sim.take(50));
    assert!(!resumed);
    sim.check();
}

#[test]
fn resuming_on_a_new_socket_while_the_old_one_is_still_registered_takes_over() {
    let mut sim = Sim::new(8);
    let (p, _code) = sim.lobby(2, course(1));
    let outs = sim.send(50, ClientMsg::hello("x", Some(p[1].token.clone())));
    let old = for_conn(&outs, p[1].conn);
    assert_eq!(errors(&old), [ErrorCode::InvalidState]);
    assert!(
        outs.iter().any(|o| o.to == p[1].conn && o.close),
        "old socket closed"
    );
    let (id, _, resumed) = welcome(&for_conn(&outs, 50));
    assert_eq!((id, resumed), (p[1].id, true));
    // The closed old socket disconnecting later must not knock the new one out.
    assert!(sim.closed.contains(&p[1].conn));
    let snap = snapshot(&sim.take(p[0].conn)).unwrap();
    assert!(
        snap.players.iter().all(|x| x.connected),
        "still connected: {snap:?}"
    );
    sim.check();
}

#[test]
fn a_room_idle_for_30_minutes_is_deleted_and_its_players_told() {
    let mut sim = Sim::new(9);
    let (p, code) = sim.lobby(2, course(1));
    sim.advance(ROOM_IDLE_MS - 1);
    assert_eq!(sim.engine.room_count(), 1);
    let outs = sim.advance(1);
    assert_eq!(sim.engine.room_count(), 0);
    for c in &p {
        assert_eq!(errors(&for_conn(&outs, c.conn)), [ErrorCode::RoomNotFound]);
    }
    let c = sim.hello(9, "Late");
    assert_eq!(errors(&sim.join(&c, &code)), [ErrorCode::RoomNotFound]);
    // Players are still logged in and can start over.
    sim.drain_all();
    sim.send(p[0].conn, ClientMsg::CreateRoom { config: course(1) });
    assert!(errors(&sim.take(p[0].conn)).is_empty());
    sim.check();
}

#[test]
fn any_client_message_keeps_a_room_alive() {
    let mut sim = Sim::new(10);
    let (p, _code) = sim.lobby(2, course(1));
    sim.advance(ROOM_IDLE_MS - 1_000);
    sim.send(p[1].conn, ClientMsg::Ping);
    sim.advance(ROOM_IDLE_MS - 1);
    assert_eq!(sim.engine.room_count(), 1);
    sim.advance(1);
    assert_eq!(sim.engine.room_count(), 0);
}

#[test]
fn next_deadline_tracks_the_earliest_timer() {
    let mut sim = Sim::new(11);
    assert_eq!(sim.engine.next_deadline(), None);
    let (p, _code) = sim.started(2, course(1));
    // Round limit (60 s) comes before the idle limit (30 min).
    assert_eq!(sim.engine.next_deadline(), Some(sim.now + 60_000));
    sim.disconnect(p[1].conn);
    assert_eq!(sim.engine.next_deadline(), Some(sim.now + 60_000));
    sim.advance(10_000);
    sim.send(50, ClientMsg::hello("x", Some(p[1].token.clone())));
    assert_eq!(sim.engine.next_deadline(), Some(sim.now - 10_000 + 60_000));
}
