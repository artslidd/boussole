//! Course mode (race): private rounds, early end, timeout, scoring, ranking, match flow.

mod common;
use boussole_protocol::*;
use boussole_server::engine::Outbound;
use common::*;

const ROUND_MS: u64 = 60_000;

fn start(sim: &mut Sim, host: &Client) -> Vec<Outbound> {
    sim.send(host.conn, ClientMsg::StartMatch)
}

#[test]
fn round_started_reaches_everyone_with_five_sorted_hints_and_the_round_timer() {
    let mut sim = Sim::new(1);
    let (p, code) = sim.lobby(3, course(2));
    let outs = start(&mut sim, &p[0]);
    let secret = sim.secret(&code);
    for c in &p {
        let started = round_started(&for_conn(&outs, c.conn)).expect("round_started");
        assert_eq!(started.round_index, 1);
        assert_eq!(started.total_rounds, 2);
        assert_eq!(started.max_tries, Some(6));
        assert_eq!(
            started.timer,
            Timer::Round {
                remaining_ms: ROUND_MS
            }
        );
        assert_eq!(started.hints.as_ref().unwrap().len(), 5);
        assert!(
            started
                .hints
                .as_ref()
                .unwrap()
                .iter()
                .all(|h| h.city != secret)
        );
        assert!(
            started
                .hints
                .as_ref()
                .unwrap()
                .iter()
                .all(|h| h.distance_km.is_some()),
            "facile"
        );
        let bearings: Vec<f64> = started
            .hints
            .as_ref()
            .unwrap()
            .iter()
            .map(|h| h.bearing_deg)
            .collect();
        assert!(
            bearings.windows(2).all(|w| w[0] <= w[1]),
            "sorted by bearing"
        );
        let snap = snapshot(&for_conn(&outs, c.conn)).unwrap();
        assert_eq!(snap.phase, RoomPhase::RoundActive);
    }
    // Same hints for everybody.
    let first = round_started(&for_conn(&outs, p[0].conn)).unwrap();
    let last = round_started(&for_conn(&outs, p[2].conn)).unwrap();
    assert_eq!(first, last);
}

#[test]
fn difficile_hints_carry_no_distance_at_all() {
    let mut sim = Sim::new(2);
    let config = RoomConfig {
        difficulty: Difficulty::Difficile,
        ..course(1)
    };
    let (p, _code) = sim.lobby(2, config);
    let outs = start(&mut sim, &p[0]);
    for o in &outs {
        if let ServerMsg::RoundStarted(r) = &o.msg {
            assert!(
                r.hints
                    .as_ref()
                    .unwrap()
                    .iter()
                    .all(|h| h.distance_km.is_none())
            );
            assert!(!encode(&o.msg).contains("distance_km"));
        }
        if let ServerMsg::RoomSnapshot(s) = &o.msg {
            let hints = s
                .round
                .as_ref()
                .expect("round")
                .start
                .hints
                .as_ref()
                .unwrap();
            assert!(
                !serde_json::to_string(hints)
                    .unwrap()
                    .contains("distance_km")
            );
        }
    }
}

#[test]
fn feedback_is_private_and_everyone_else_only_sees_progress() {
    let mut sim = Sim::new(3);
    let (p, code) = sim.started(3, course(1));
    let wrong = wrong_cities(sim.secret(&code), 1)[0];
    let outs = sim.guess(&p[1], wrong);
    // The guesser: a full result, nothing else.
    let mine = for_conn(&outs, p[1].conn);
    let results = guess_results(&mine);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].by, p[1].id);
    assert_eq!(results[0].city, wrong);
    assert_eq!((results[0].tries_used, results[0].correct), (1, false));
    assert!(progress(&mine).is_empty());
    // The others: progress without any city name.
    for c in [&p[0], &p[2]] {
        let theirs = for_conn(&outs, c.conn);
        assert!(guess_results(&theirs).is_empty());
        assert_eq!(
            progress(&theirs),
            [Progress {
                player_id: p[1].id,
                tries_used: 1,
                solved: false,
                out_of_tries: false
            }]
        );
        assert!(!encode(&theirs[0]).contains(wrong));
    }
    assert_eq!(outs.len(), 3);
}

#[test]
fn feedback_distance_and_bearing_are_from_the_guess_to_the_secret() {
    use boussole_core::{cities::CITIES, geo};
    let mut sim = Sim::new(4);
    let (p, code) = sim.started(2, course(1));
    let secret = sim.secret(&code);
    let secret_city = CITIES.iter().find(|c| c.name == secret).unwrap();
    let guess_city = CITIES.iter().find(|c| c.name != secret).unwrap();
    let outs = sim.guess(&p[0], guess_city.name);
    let r = &guess_results(&for_conn(&outs, p[0].conn))[0];
    assert_eq!(
        r.distance_km,
        geo::distance_km(guess_city, secret_city).round() as u32
    );
    assert!((r.bearing_deg - geo::bearing_deg(guess_city, secret_city)).abs() <= 0.05 + 1e-9);
}

#[test]
fn a_correct_guess_is_flagged_and_others_see_solved() {
    let mut sim = Sim::new(5);
    let (p, code) = sim.started(2, course(1));
    let secret = sim.secret(&code);
    let outs = sim.guess(&p[0], secret);
    let r = &guess_results(&for_conn(&outs, p[0].conn))[0];
    assert!(r.correct);
    assert_eq!(r.distance_km, 0);
    let seen = progress(&for_conn(&outs, p[1].conn));
    assert_eq!(seen.len(), 1);
    assert!(seen[0].solved && !seen[0].out_of_tries);
}

#[test]
fn a_duplicate_guess_is_refused_and_costs_nothing() {
    let mut sim = Sim::new(6);
    let (p, code) = sim.started(2, course(1));
    let wrong = wrong_cities(sim.secret(&code), 2);
    sim.guess(&p[0], wrong[0]);
    sim.drain_all();
    let outs = sim.guess(&p[0], wrong[0]);
    assert_eq!(outs.len(), 1, "error to the sender only");
    assert_eq!(
        errors(&for_conn(&outs, p[0].conn)),
        [ErrorCode::AlreadyGuessed]
    );
    let outs = sim.guess(&p[0], wrong[1]);
    assert_eq!(guess_results(&for_conn(&outs, p[0].conn))[0].tries_used, 2);
}

#[test]
fn guess_names_are_validated_server_side() {
    let mut sim = Sim::new(7);
    let (p, code) = sim.started(2, course(1));
    for bad in ["Atlantis", "", "paris, france"] {
        let outs = sim.guess(&p[0], bad);
        assert_eq!(
            errors(&for_conn(&outs, p[0].conn)),
            [ErrorCode::UnknownCity],
            "{bad:?}"
        );
        assert_eq!(outs.len(), 1);
    }
    // find_exact is accent- and case-insensitive: a sloppy but valid name is accepted
    // and reported under its canonical spelling.
    let target = if sim.secret(&code) == "Saint-Étienne" {
        "Lyon"
    } else {
        "Saint-Étienne"
    };
    let sloppy = if target == "Lyon" {
        "LYON"
    } else {
        "saint-etienne"
    };
    let outs = sim.guess(&p[0], sloppy);
    assert_eq!(guess_results(&for_conn(&outs, p[0].conn))[0].city, target);
}

#[test]
fn six_tries_then_the_round_is_over_for_that_player() {
    let mut sim = Sim::new(8);
    let (p, code) = sim.started(2, course(1));
    let wrong = wrong_cities(sim.secret(&code), 7);
    let mut last = vec![];
    for city in &wrong[..6] {
        last = sim.guess(&p[0], city);
    }
    assert_eq!(
        progress(&for_conn(&last, p[1].conn)),
        [Progress {
            player_id: p[0].id,
            tries_used: 6,
            solved: false,
            out_of_tries: true
        }]
    );
    let outs = sim.guess(&p[0], wrong[6]);
    assert_eq!(errors(&for_conn(&outs, p[0].conn)), [ErrorCode::RoundOver]);
    // The other player is unaffected and the round is still running.
    let outs = sim.guess(&p[1], wrong[0]);
    assert!(errors(&for_conn(&outs, p[1].conn)).is_empty());
}

#[test]
fn a_solved_player_cannot_guess_again() {
    let mut sim = Sim::new(9);
    let (p, code) = sim.started(2, course(1));
    sim.guess(&p[0], sim.secret(&code));
    let wrong = wrong_cities(sim.secret(&code), 1)[0];
    let outs = sim.guess(&p[0], wrong);
    assert_eq!(errors(&for_conn(&outs, p[0].conn)), [ErrorCode::RoundOver]);
}

#[test]
fn the_round_ends_as_soon_as_every_connected_player_is_done() {
    let mut sim = Sim::new(10);
    let (p, code) = sim.started(3, course(2));
    let secret = sim.secret(&code);
    let wrong = wrong_cities(secret, 6);
    sim.guess(&p[0], secret);
    for city in &wrong {
        sim.guess(&p[1], city);
    }
    sim.drain_all();
    let outs = sim.guess(&p[2], secret);
    for c in &p {
        let ended = round_ended(&for_conn(&outs, c.conn)).expect("round_ended for everyone");
        assert_eq!(ended.secret.as_deref(), Some(secret));
        assert_eq!(ended.round_index, 1);
        assert_eq!(ended.next_round_in_ms, Some(INTERMISSION_MS));
        assert_eq!(ended.found_by, None);
        let solved: Vec<_> = ended
            .results
            .as_ref()
            .unwrap()
            .iter()
            .map(|r| r.solved)
            .collect();
        assert_eq!(solved, [true, false, true]);
        // All guesses are revealed now.
        let p2 = &ended.results.as_ref().unwrap()[1];
        assert_eq!(p2.tries_used, 6);
        let cities: Vec<_> = p2.guesses.iter().map(|g| g.city.as_str()).collect();
        assert_eq!(cities, wrong);
        assert_eq!(p2.points, 0);
        let snap = snapshot(&for_conn(&outs, c.conn)).unwrap();
        assert_eq!(snap.phase, RoomPhase::Intermission);
    }
}

#[test]
fn points_follow_the_formula_exactly() {
    let mut sim = Sim::new(11);
    let (p, code) = sim.started(3, course(1));
    let secret = sim.secret(&code);
    let wrong = wrong_cities(secret, 2);
    sim.advance(6_000);
    sim.guess(&p[0], secret); // 1 try at 6 s of 60 s → 600 + 90
    sim.advance(24_000);
    sim.guess(&p[1], wrong[0]);
    sim.guess(&p[1], wrong[1]);
    sim.guess(&p[1], secret); // 3 tries at 30 s → 400 + 50
    sim.drain_all();
    let outs = sim.advance(ROUND_MS - 30_000); // P3 never answers: timeout
    let ended = round_ended(&for_conn(&outs, p[0].conn)).expect("ended by the timer");
    let points: Vec<_> = ended
        .results
        .as_ref()
        .unwrap()
        .iter()
        .map(|r| (r.points, r.total))
        .collect();
    assert_eq!(points, [(690, 690), (450, 450), (0, 0)]);
    assert_eq!(course_points_via_engine(1, 6_000, ROUND_MS), 690);
    // One-round match: the ranking comes right behind.
    let m = match_ended(&for_conn(&outs, p[2].conn)).unwrap();
    let ranking: Vec<_> = m
        .ranking
        .iter()
        .map(|r| (r.player_id, r.total, r.rank))
        .collect();
    assert_eq!(
        ranking,
        [(p[0].id, 690, 1), (p[1].id, 450, 2), (p[2].id, 0, 3)]
    );
}

fn course_points_via_engine(tries: u32, e: u64, l: u64) -> u32 {
    boussole_server::engine::course_points(tries, e, l)
}

#[test]
fn the_timer_ends_the_round_at_the_limit_not_before() {
    let mut sim = Sim::new(12);
    let (p, _code) = sim.started(2, course(2));
    assert!(sim.advance(ROUND_MS - 1).is_empty());
    let outs = sim.advance(1);
    let ended = round_ended(&for_conn(&outs, p[0].conn)).expect("timeout");
    assert!(
        ended
            .results
            .as_ref()
            .unwrap()
            .iter()
            .all(|r| r.points == 0 && !r.solved)
    );
}

#[test]
fn a_guess_after_the_deadline_is_too_late_even_before_any_tick() {
    let mut sim = Sim::new(13);
    let (p, code) = sim.started(2, course(2));
    let secret = sim.secret(&code);
    sim.now += ROUND_MS; // no tick: the guess message itself carries the time
    let outs = sim.guess(&p[0], secret);
    let mine = for_conn(&outs, p[0].conn);
    assert!(round_ended(&mine).is_some(), "the timeout fired first");
    assert_eq!(errors(&mine), [ErrorCode::RoundOver]);
    assert!(guess_results(&mine).is_empty());
}

#[test]
fn the_intermission_lasts_8_seconds_then_the_next_round_has_a_different_secret() {
    let mut sim = Sim::new(14);
    let (p, code) = sim.started(2, course(3));
    let first = sim.secret(&code);
    sim.guess(&p[0], first);
    let outs = sim.guess(&p[1], first);
    let ended = round_ended(&for_conn(&outs, p[0].conn)).unwrap();
    assert_eq!(
        sim.engine.debug_secret(&code),
        None,
        "no secret between rounds"
    );
    // Guessing during the pause is refused.
    let outs = sim.guess(&p[0], "Paris");
    assert_eq!(errors(&for_conn(&outs, p[0].conn)), [ErrorCode::RoundOver]);
    sim.drain_all();
    assert!(sim.advance(INTERMISSION_MS - 1).is_empty());
    let outs = sim.advance(1);
    for c in &p {
        let started = round_started(&for_conn(&outs, c.conn)).expect("round 2");
        assert_eq!(started.round_index, 2);
        assert_eq!(
            started.timer,
            Timer::Round {
                remaining_ms: ROUND_MS
            }
        );
    }
    assert_eq!(ended.round_index, 1);
    assert_ne!(
        sim.secret(&code),
        first,
        "no two consecutive rounds share a secret"
    );
}

#[test]
fn a_late_tick_still_starts_the_next_round_at_the_right_time() {
    let mut sim = Sim::new(15);
    let (p, code) = sim.started(2, course(3));
    let secret = sim.secret(&code);
    sim.guess(&p[0], secret);
    sim.guess(&p[1], secret);
    sim.drain_all();
    // The transport stalls for 20 s: round 2 must still have started 8 s after round 1 ended,
    // i.e. 12 s ago, so 12 s of its 60 s are gone.
    let outs = sim.advance(20_000);
    let started = round_started(&for_conn(&outs, p[0].conn)).expect("round 2");
    assert_eq!(
        started.timer,
        Timer::Round {
            remaining_ms: ROUND_MS
        }
    );
    // Its limit is counted from when it really started: a snapshot taken now shows 48 s left.
    sim.disconnect(p[1].conn);
    sim.drain_all();
    let outs = sim.send(50, ClientMsg::hello("x", Some(p[1].token.clone())));
    let snap = snapshot(&for_conn(&outs, 50)).unwrap();
    assert_eq!(
        snap.round.unwrap().start.timer,
        Timer::Round {
            remaining_ms: ROUND_MS - 12_000
        }
    );
    assert_eq!(
        sim.engine.next_deadline(),
        Some(sim.now - 12_000 + ROUND_MS)
    );
}

#[test]
fn the_match_ends_with_a_ranking_then_the_room_returns_to_the_lobby_with_scores_kept() {
    let mut sim = Sim::new(16);
    let (p, code) = sim.started(2, course(2));
    // Round 1: P1 solves at once, P2 never does.
    let secret = sim.secret(&code);
    sim.guess(&p[0], secret);
    sim.advance(ROUND_MS);
    sim.advance(INTERMISSION_MS);
    // Round 2: both solve (P2 one second later).
    let secret = sim.secret(&code);
    sim.guess(&p[0], secret);
    sim.advance(1_000);
    sim.drain_all();
    let outs = sim.guess(&p[1], secret);
    for c in &p {
        let msgs = for_conn(&outs, c.conn);
        let order: Vec<&str> = msgs
            .iter()
            .filter_map(|m| match m {
                ServerMsg::RoundEnded(_) => Some("round_ended"),
                ServerMsg::MatchEnded(_) => Some("match_ended"),
                ServerMsg::RoomSnapshot(s) if s.phase == RoomPhase::Lobby => Some("lobby"),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["round_ended", "match_ended", "lobby"]);
        let ended = round_ended(&msgs).unwrap();
        assert_eq!(ended.next_round_in_ms, None, "last round");
        let m = match_ended(&msgs).unwrap();
        assert_eq!(m.mode, Mode::Course);
        assert_eq!(m.ranking.len(), 2);
        assert_eq!(m.ranking[0].player_id, p[0].id);
        assert!(m.ranking[0].total > m.ranking[1].total);
        assert_eq!((m.ranking[0].rank, m.ranking[1].rank), (1, 2));
        assert_eq!(
            (m.ranking[0].solved_rounds, m.ranking[1].solved_rounds),
            (2, 1)
        );
        // Scores stay visible in the lobby.
        let snap = snapshot(&msgs).unwrap();
        assert_eq!(snap.phase, RoomPhase::Lobby);
        assert_eq!(snap.players[0].score, m.ranking[0].total);
        assert_eq!(snap.players[1].score, m.ranking[1].total);
    }
    // The host can restart: scores reset.
    let outs = start(&mut sim, &p[0]);
    let snap = snapshot(&for_conn(&outs, p[1].conn)).unwrap();
    assert_eq!(snap.phase, RoomPhase::RoundActive);
    assert!(snap.players.iter().all(|x| x.score == 0));
    assert_eq!(snap.round.unwrap().start.round_index, 1);
    sim.check();
}

#[test]
fn equal_totals_share_a_rank() {
    let mut sim = Sim::new(17);
    let (p, code) = sim.started(3, course(1));
    let secret = sim.secret(&code);
    sim.advance(5_000);
    sim.guess(&p[0], secret);
    sim.guess(&p[1], secret); // same try, same millisecond → same points
    sim.drain_all();
    let outs = sim.advance(ROUND_MS);
    let m = match_ended(&for_conn(&outs, p[0].conn)).unwrap();
    let ranks: Vec<_> = m.ranking.iter().map(|r| (r.player_id, r.rank)).collect();
    assert_eq!(ranks, [(p[0].id, 1), (p[1].id, 1), (p[2].id, 3)]);
    assert_eq!(m.ranking[0].total, m.ranking[1].total);
}

#[test]
fn no_two_consecutive_rounds_share_a_secret_in_long_matches() {
    for seed in 0..20 {
        let mut sim = Sim::new(seed);
        let (p, code) = sim.started(2, course(10));
        let mut secrets = Vec::new();
        for _ in 0..10 {
            let secret = sim.secret(&code);
            secrets.push(secret);
            sim.guess(&p[0], secret);
            sim.guess(&p[1], secret);
            sim.advance(INTERMISSION_MS);
        }
        assert!(
            secrets.windows(2).all(|w| w[0] != w[1]),
            "seed {seed}: {secrets:?}"
        );
        assert_eq!(sim.engine.debug_secret(&code), None, "match over");
    }
}

#[test]
fn when_all_but_one_player_drops_the_round_continues_solo() {
    let mut sim = Sim::new(18);
    let (p, code) = sim.started(2, course(1));
    let secret = sim.secret(&code);
    sim.disconnect(p[1].conn);
    assert_eq!(
        sim.engine.debug_secret(&code),
        Some(secret),
        "round goes on"
    );
    sim.drain_all();
    let outs = sim.guess(&p[0], secret);
    let ended = round_ended(&for_conn(&outs, p[0].conn)).expect("solo finish ends the round");
    assert!(
        ended.results.as_ref().unwrap()[0].solved && !ended.results.as_ref().unwrap()[1].solved
    );
    sim.check();
}

#[test]
fn the_round_ends_when_a_disconnect_leaves_only_finished_players() {
    let mut sim = Sim::new(19);
    let (p, code) = sim.started(3, course(2));
    let secret = sim.secret(&code);
    sim.guess(&p[0], secret);
    sim.guess(&p[1], secret);
    sim.drain_all();
    let outs = sim.disconnect(p[2].conn);
    assert!(round_ended(&for_conn(&outs, p[0].conn)).is_some());
    sim.check();
}

#[test]
fn a_match_nobody_is_connected_to_ends_and_returns_to_the_lobby() {
    let mut sim = Sim::new(20);
    let (p, code) = sim.started(2, course(3));
    sim.disconnect(p[0].conn);
    sim.disconnect(p[1].conn);
    assert_eq!(sim.engine.debug_secret(&code), None);
    sim.send(50, ClientMsg::hello("x", Some(p[0].token.clone())));
    let snap = snapshot(&sim.take(50)).unwrap();
    assert_eq!(snap.phase, RoomPhase::Lobby);
    assert!(snap.round.is_none());
    sim.check();
    // Nothing is left running: only the resume grace and idle timers remain.
    assert!(sim.engine.next_deadline().unwrap() > sim.now);
}

#[test]
fn a_player_who_leaves_mid_round_drops_out_of_the_results() {
    let mut sim = Sim::new(21);
    let (p, code) = sim.started(3, course(1));
    let secret = sim.secret(&code);
    sim.guess(&p[0], secret);
    sim.send(p[2].conn, ClientMsg::LeaveRoom);
    sim.drain_all();
    let outs = sim.guess(&p[1], secret);
    let ended = round_ended(&for_conn(&outs, p[0].conn)).unwrap();
    let ids: Vec<_> = ended
        .results
        .as_ref()
        .unwrap()
        .iter()
        .map(|r| r.player_id)
        .collect();
    assert_eq!(ids, [p[0].id, p[1].id]);
    assert!(
        round_ended(&for_conn(&outs, p[2].conn)).is_none(),
        "the leaver hears no more"
    );
    sim.check();
}

#[test]
fn the_secret_never_appears_in_other_players_progress_or_snapshots_before_the_end() {
    let mut sim = Sim::new(22);
    let (p, code) = sim.started(3, course(1));
    let secret = sim.secret(&code);
    let wrong = wrong_cities(secret, 3);
    sim.guess(&p[0], wrong[0]);
    sim.guess(&p[1], wrong[1]);
    // A reconnecting player's snapshot lists only their own guesses.
    sim.disconnect(p[2].conn);
    sim.drain_all();
    let outs = sim.send(50, ClientMsg::hello("x", Some(p[2].token.clone())));
    let snap = snapshot(&for_conn(&outs, 50)).unwrap();
    let json = serde_json::to_string(&snap).unwrap();
    assert!(!json.contains(&format!("\"{secret}\"")));
    assert!(!json.contains(wrong[0]) && !json.contains(wrong[1]));
}
