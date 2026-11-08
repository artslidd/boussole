//! Coopération mode: shared round, turns, shared tries, team scoring.

mod common;
use boussole_protocol::*;
use boussole_server::engine::Outbound;
use common::*;

const TURN_MS: u64 = 30_000;

fn turn_of(started: &RoundStarted) -> PlayerId {
    match started.timer {
        Timer::Turn { player_id, .. } => player_id,
        other => panic!("not a turn timer: {other:?}"),
    }
}

/// Everyone's view of one engine step must agree on the broadcast part.
fn for_all(outs: &[Outbound], players: &[Client]) -> Vec<Vec<ServerMsg>> {
    players.iter().map(|c| for_conn(outs, c.conn)).collect()
}

#[test]
fn round_started_names_the_first_player_and_the_turn_timer() {
    let mut sim = Sim::new(1);
    let (p, _code) = sim.lobby(3, coop(2));
    let outs = sim.send(p[0].conn, ClientMsg::StartMatch);
    for msgs in for_all(&outs, &p) {
        let started = round_started(&msgs).unwrap();
        assert_eq!(
            started.timer,
            Timer::Turn {
                player_id: p[0].id,
                remaining_ms: TURN_MS
            }
        );
        assert_eq!(started.hints.len(), 5);
    }
}

#[test]
fn round_r_starts_with_player_r_minus_1_mod_n_and_the_order_is_join_order() {
    let mut sim = Sim::new(2);
    let (p, code) = sim.lobby(3, coop(4));
    let outs = sim.send(p[0].conn, ClientMsg::StartMatch);
    let mut starters = vec![turn_of(&round_started(&for_conn(&outs, 1)).unwrap())];
    for _ in 0..3 {
        // The active player solves it at once.
        let secret = sim.secret(&code);
        let active = *starters.last().unwrap();
        let who = p.iter().find(|c| c.id == active).unwrap();
        sim.guess(who, secret);
        let outs = sim.advance(INTERMISSION_MS);
        starters.push(turn_of(&round_started(&for_conn(&outs, 1)).unwrap()));
    }
    let expected: Vec<_> = [0, 1, 2, 0].iter().map(|i| p[*i].id).collect();
    assert_eq!(starters, expected);
}

#[test]
fn disconnected_players_are_skipped_in_the_rotation() {
    let mut sim = Sim::new(3);
    let (p, code) = sim.lobby(3, coop(3));
    sim.disconnect(p[1].conn);
    sim.send(p[0].conn, ClientMsg::StartMatch);
    let secret = sim.secret(&code);
    sim.guess(&p[0], secret); // round 1: connected = [P1, P3] → P1
    let outs = sim.advance(INTERMISSION_MS);
    // round 2: index (2 − 1) mod 2 = 1 → P3, not the absent P2.
    assert_eq!(
        turn_of(&round_started(&for_conn(&outs, p[0].conn)).unwrap()),
        p[2].id
    );
}

#[test]
fn the_turn_passes_to_the_next_player_after_every_accepted_guess() {
    let mut sim = Sim::new(4);
    let (p, code) = sim.started(3, coop(1));
    let wrong = wrong_cities(sim.secret(&code), 4);
    let order = [0, 1, 2, 0];
    for (i, idx) in order.iter().enumerate() {
        let outs = sim.guess(&p[*idx], wrong[i]);
        let next = p[order.get(i + 1).copied().unwrap_or(1)].id;
        for msgs in for_all(&outs, &p) {
            // Every guess is broadcast, with its author and the shared try count…
            let results = guess_results(&msgs);
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].by, p[*idx].id);
            assert_eq!(results[0].city, wrong[i]);
            assert_eq!(results[0].tries_used as usize, i + 1);
            // …followed by the turn change.
            let turns = turn_changes(&msgs);
            assert_eq!(turns.len(), 1);
            if i + 1 < order.len() {
                assert_eq!(turns[0], (next, TURN_MS, TurnReason::Guess));
            }
            assert!(progress(&msgs).is_empty(), "no per-player progress in coop");
        }
    }
}

#[test]
fn only_the_active_player_may_guess() {
    let mut sim = Sim::new(5);
    let (p, code) = sim.started(3, coop(1));
    let wrong = wrong_cities(sim.secret(&code), 2);
    let outs = sim.guess(&p[1], wrong[0]);
    assert_eq!(outs.len(), 1, "the refusal goes to the sender only");
    assert_eq!(
        errors(&for_conn(&outs, p[1].conn)),
        [ErrorCode::NotYourTurn]
    );
    // Even an unknown city is a turn error first, and nothing changed: P1 still has the turn.
    let outs = sim.guess(&p[2], "Atlantis");
    assert_eq!(
        errors(&for_conn(&outs, p[2].conn)),
        [ErrorCode::NotYourTurn]
    );
    let outs = sim.guess(&p[0], wrong[0]);
    assert_eq!(guess_results(&for_conn(&outs, p[0].conn))[0].tries_used, 1);
}

#[test]
fn an_unknown_city_costs_neither_a_try_nor_the_turn() {
    let mut sim = Sim::new(6);
    let (p, code) = sim.started(2, coop(1));
    let wrong = wrong_cities(sim.secret(&code), 1);
    let outs = sim.guess(&p[0], "Atlantis");
    assert_eq!(
        errors(&for_conn(&outs, p[0].conn)),
        [ErrorCode::UnknownCity]
    );
    assert_eq!(outs.len(), 1);
    let outs = sim.guess(&p[0], wrong[0]);
    assert_eq!(guess_results(&for_conn(&outs, p[0].conn))[0].tries_used, 1);
}

#[test]
fn a_duplicate_city_is_refused_to_the_sender_and_the_turn_stays() {
    let mut sim = Sim::new(7);
    let (p, code) = sim.started(2, coop(1));
    let wrong = wrong_cities(sim.secret(&code), 2);
    sim.guess(&p[0], wrong[0]); // team guess #1, turn → P2
    sim.drain_all();
    let outs = sim.guess(&p[1], wrong[0]); // the team already tried it
    assert_eq!(outs.len(), 1);
    assert_eq!(
        errors(&for_conn(&outs, p[1].conn)),
        [ErrorCode::AlreadyGuessed]
    );
    // P2 still holds the turn and the try count did not move.
    let outs = sim.guess(&p[1], wrong[1]);
    assert_eq!(guess_results(&for_conn(&outs, p[1].conn))[0].tries_used, 2);
}

#[test]
fn a_turn_timeout_passes_the_turn_without_using_a_try() {
    let mut sim = Sim::new(8);
    let (p, code) = sim.started(3, coop(1));
    let wrong = wrong_cities(sim.secret(&code), 1);
    assert!(sim.advance(TURN_MS - 1).is_empty());
    let outs = sim.advance(1);
    for msgs in for_all(&outs, &p) {
        assert_eq!(
            turn_changes(&msgs),
            [(p[1].id, TURN_MS, TurnReason::Timeout)]
        );
        assert!(guess_results(&msgs).is_empty());
    }
    // P1 lost the turn: too late now.
    let outs = sim.guess(&p[0], wrong[0]);
    assert_eq!(
        errors(&for_conn(&outs, p[0].conn)),
        [ErrorCode::NotYourTurn]
    );
    // P2's guess is the team's first try: the timeout consumed nothing.
    let outs = sim.guess(&p[1], wrong[0]);
    assert_eq!(guess_results(&for_conn(&outs, p[1].conn))[0].tries_used, 1);
}

#[test]
fn a_guess_just_before_the_deadline_is_accepted_and_resets_the_clock() {
    let mut sim = Sim::new(9);
    let (p, code) = sim.started(2, coop(1));
    let wrong = wrong_cities(sim.secret(&code), 1);
    sim.advance(TURN_MS - 1);
    let outs = sim.guess(&p[0], wrong[0]);
    assert_eq!(
        turn_changes(&for_conn(&outs, p[1].conn)),
        [(p[1].id, TURN_MS, TurnReason::Guess)]
    );
    assert!(sim.advance(TURN_MS - 1).is_empty(), "P2 has a full turn");
}

#[test]
fn a_late_message_sees_the_timeout_first() {
    let mut sim = Sim::new(10);
    let (p, code) = sim.started(2, coop(1));
    let wrong = wrong_cities(sim.secret(&code), 1);
    sim.now += TURN_MS; // no tick before the message
    let outs = sim.guess(&p[0], wrong[0]);
    let mine = for_conn(&outs, p[0].conn);
    assert_eq!(
        turn_changes(&mine),
        [(p[1].id, TURN_MS, TurnReason::Timeout)]
    );
    assert_eq!(errors(&mine), [ErrorCode::NotYourTurn]);
}

#[test]
fn when_the_active_player_disconnects_the_turn_passes_at_once() {
    let mut sim = Sim::new(11);
    let (p, code) = sim.started(3, coop(1));
    let wrong = wrong_cities(sim.secret(&code), 3);
    sim.advance(10_000);
    let outs = sim.disconnect(p[0].conn);
    for c in &p[1..] {
        assert_eq!(
            turn_changes(&for_conn(&outs, c.conn)),
            [(p[1].id, TURN_MS, TurnReason::Disconnect)]
        );
    }
    // P1 comes back: not their turn, but they are back in the rotation after P3.
    sim.send(50, ClientMsg::hello("x", Some(p[0].token.clone())));
    sim.drain_all();
    sim.guess(&p[1], wrong[0]);
    sim.guess(&p[2], wrong[1]);
    let outs = sim.send(
        50,
        ClientMsg::Guess {
            city: wrong[2].into(),
        },
    );
    assert_eq!(guess_results(&for_conn(&outs, 50))[0].by, p[0].id);
}

#[test]
fn a_disconnect_of_a_waiting_player_does_not_move_the_turn() {
    let mut sim = Sim::new(12);
    let (p, _code) = sim.started(3, coop(1));
    let outs = sim.disconnect(p[2].conn);
    assert!(turn_changes(&for_conn(&outs, p[0].conn)).is_empty());
}

#[test]
fn leaving_while_active_passes_the_turn_too() {
    let mut sim = Sim::new(13);
    let (p, _code) = sim.started(3, coop(1));
    let outs = sim.send(p[0].conn, ClientMsg::LeaveRoom);
    assert_eq!(
        turn_changes(&for_conn(&outs, p[1].conn)),
        [(p[1].id, TURN_MS, TurnReason::Disconnect)]
    );
    assert!(
        for_conn(&outs, p[0].conn).is_empty(),
        "the leaver hears nothing about the room they left"
    );
    sim.check();
}

#[test]
fn the_team_shares_six_tries() {
    let mut sim = Sim::new(14);
    let (p, code) = sim.started(3, coop(1));
    let secret = sim.secret(&code);
    let wrong = wrong_cities(secret, 6);
    let mut last = vec![];
    for (i, city) in wrong.iter().enumerate() {
        last = sim.guess(&p[i % 3], city);
    }
    for msgs in for_all(&last, &p) {
        assert_eq!(guess_results(&msgs).last().unwrap().tries_used, 6);
        let ended = round_ended(&msgs).expect("sixth miss loses the round");
        assert_eq!(ended.secret, secret);
        assert_eq!(ended.found_by, None);
        assert!(
            ended
                .results
                .iter()
                .all(|r| r.points == 0 && !r.solved && r.total == 0)
        );
        // Each player's own guesses are listed: two each, in turn order.
        let mine: Vec<_> = ended.results[0]
            .guesses
            .iter()
            .map(|g| g.city.as_str())
            .collect();
        assert_eq!(mine, [wrong[0], wrong[3]]);
        assert!(
            turn_changes(&msgs).is_empty(),
            "no turn after the round is over"
        );
    }
}

#[test]
fn team_points_are_100_times_7_minus_tries_and_the_finder_is_named() {
    let mut sim = Sim::new(15);
    let (p, code) = sim.started(3, coop(3));
    let secret = sim.secret(&code);
    let wrong = wrong_cities(secret, 2);
    sim.guess(&p[0], wrong[0]);
    sim.guess(&p[1], wrong[1]);
    let outs = sim.guess(&p[2], secret); // third try: 100 × (7 − 3)
    for msgs in for_all(&outs, &p) {
        let ended = round_ended(&msgs).unwrap();
        assert_eq!(ended.found_by, Some(p[2].id));
        assert!(
            ended
                .results
                .iter()
                .all(|r| r.points == 400 && r.total == 400)
        );
        let solved: Vec<_> = ended.results.iter().map(|r| r.solved).collect();
        assert_eq!(solved, [false, false, true]);
        assert_eq!(ended.next_round_in_ms, Some(INTERMISSION_MS));
        let snap = snapshot(&msgs).unwrap();
        assert!(
            snap.players.iter().all(|x| x.score == 400),
            "team total in every row"
        );
    }
}

#[test]
fn a_coop_match_ranks_the_team_and_counts_each_players_finds() {
    let mut sim = Sim::new(16);
    let (p, code) = sim.started(2, coop(3));
    // Round 1 (P1 starts): P1 finds it first try → 600.
    let secret = sim.secret(&code);
    sim.guess(&p[0], secret);
    sim.advance(INTERMISSION_MS);
    // Round 2 (P2 starts): P2 misses, P1 finds → 500.
    let secret = sim.secret(&code);
    let wrong = wrong_cities(secret, 1);
    sim.guess(&p[1], wrong[0]);
    sim.guess(&p[0], secret);
    sim.advance(INTERMISSION_MS);
    // Round 3 (P1 starts): lost by silence → 0.
    sim.drain_all();
    sim.advance(TURN_MS);
    let outs = sim.advance(TURN_MS);
    let msgs = for_conn(&outs, p[1].conn);
    let m = match_ended(&msgs).expect("match over");
    assert_eq!(m.mode, Mode::Cooperation);
    let rows: Vec<_> = m
        .ranking
        .iter()
        .map(|r| (r.player_id, r.total, r.rank, r.solved_rounds))
        .collect();
    assert_eq!(rows, [(p[0].id, 1_100, 1, 2), (p[1].id, 1_100, 1, 0)]);
    assert_eq!(round_ended(&msgs).unwrap().found_by, None);
    assert_eq!(snapshot(&msgs).unwrap().phase, RoomPhase::Lobby);
}

#[test]
fn a_silent_table_loses_the_round_after_a_full_lap_of_timeouts() {
    let mut sim = Sim::new(17);
    let (p, _code) = sim.started(2, coop(2));
    let outs = sim.advance(TURN_MS);
    assert_eq!(
        turn_changes(&for_conn(&outs, 1)),
        [(p[1].id, TURN_MS, TurnReason::Timeout)]
    );
    let outs = sim.advance(TURN_MS);
    let ended = round_ended(&for_conn(&outs, 1)).expect("everyone timed out in a row");
    assert_eq!(ended.found_by, None);
    assert!(
        ended
            .results
            .iter()
            .all(|r| r.points == 0 && r.tries_used == 0)
    );
    // An accepted guess resets the lap: the next round survives one timeout.
    sim.advance(INTERMISSION_MS);
    sim.check();
}

#[test]
fn any_accepted_guess_resets_the_lap_counter() {
    let mut sim = Sim::new(18);
    let (p, code) = sim.started(2, coop(1));
    let wrong = wrong_cities(sim.secret(&code), 2);
    sim.advance(TURN_MS); // P1 times out → P2
    sim.guess(&p[1], wrong[0]); // → P1
    sim.advance(TURN_MS); // P1 times out again: one in a row, not two
    assert!(sim.engine.debug_secret(&code).is_some(), "round still on");
    sim.guess(&p[1], wrong[1]);
}

#[test]
fn a_lone_connected_player_keeps_taking_turns() {
    let mut sim = Sim::new(19);
    let (p, code) = sim.started(2, coop(1));
    let secret = sim.secret(&code);
    let wrong = wrong_cities(secret, 2);
    sim.disconnect(p[1].conn);
    let outs = sim.guess(&p[0], wrong[0]);
    assert_eq!(
        turn_changes(&for_conn(&outs, p[0].conn)),
        [(p[0].id, TURN_MS, TurnReason::Guess)],
        "the turn comes straight back"
    );
    let outs = sim.guess(&p[0], secret);
    let ended = round_ended(&for_conn(&outs, p[0].conn)).unwrap();
    assert_eq!(ended.found_by, Some(p[0].id));
    assert_eq!(ended.results[0].points, 500);
}

#[test]
fn guessing_between_rounds_is_round_over() {
    let mut sim = Sim::new(20);
    let (p, code) = sim.started(2, coop(2));
    let secret = sim.secret(&code);
    sim.guess(&p[0], secret);
    let outs = sim.guess(&p[1], "Paris");
    assert_eq!(errors(&for_conn(&outs, p[1].conn)), [ErrorCode::RoundOver]);
}
