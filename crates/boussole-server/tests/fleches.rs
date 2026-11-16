//! The Flèches game end to end in the engine: questions on the wire, Course and Coopération
//! flows, timers, scoring, refusals, resume, and the frame-level secret rule (no frame
//! before `round_ended` carries the correct index or the distance).

mod common;
use boussole_core::geo::bearing_deg;
use boussole_core::search::find_exact;
use boussole_protocol::*;
use boussole_server::engine::Outbound;
use common::*;

/// `question_time_s` in ms for the configs built by [`fleches_course`]/[`fleches_coop`].
const QUESTION_MS: u64 = 15_000;

fn answered(msgs: &[ServerMsg]) -> Vec<PlayerId> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::PlayerAnswered { player_id } => Some(*player_id),
            _ => None,
        })
        .collect()
}

/// The last `round_started` (or snapshot round `start`) of one step, if any.
fn round_started_of(msgs: &[ServerMsg]) -> Option<RoundStarted> {
    msgs.iter().rev().find_map(|m| match m {
        ServerMsg::RoundStarted(r) => Some(r.clone()),
        _ => None,
    })
}

/// The reveal of a `round_ended`, asserting it is a Flèches round.
fn reveal(ended: &RoundEnded) -> &FlechesReveal {
    ended.fleches.as_ref().expect("a fleches reveal")
}

/// Every rule a Flèches `round_started` (or the `start` of a snapshot round view) follows.
fn check_question(start: &RoundStarted) -> &FlechesQuestion {
    assert!(start.max_tries.is_none(), "no try limit in Flèches");
    assert!(start.hints.is_none(), "no hints in Flèches");
    let question = start.question.as_ref().expect("a question");
    assert_eq!(question.options_deg.len(), 4);
    for &deg in &question.options_deg {
        assert!((0.0..360.0).contains(&deg), "{deg}° out of range");
    }
    let origin = find_exact(&question.origin).expect("known origin");
    let target = find_exact(&question.target).expect("known target");
    assert_ne!(question.origin, question.target);
    // The four options are pairwise at least 25° apart (facile is 60°, both are > 25°).
    for (i, &a) in question.options_deg.iter().enumerate() {
        for &b in &question.options_deg[i + 1..] {
            let gap = ((a - b + 540.0).rem_euclid(360.0) - 180.0).abs();
            assert!(gap >= 25.0 - 1e-9, "options {a}° and {b}° too close");
        }
    }
    // Fairness: exactly one option matches the true bearing origin → target (0.1° wire
    // rounding), and the server's hidden correct index is that one.
    let true_deg = bearing_deg(origin, target);
    let matching: Vec<usize> = question
        .options_deg
        .iter()
        .enumerate()
        .filter(|(_, d)| ((*d - true_deg + 540.0).rem_euclid(360.0) - 180.0).abs() <= 0.11)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(matching.len(), 1, "exactly one arrow points at the target");
    question
}

#[test]
fn a_three_round_course_match_from_start_to_ranking() {
    let mut sim = Sim::new(7);
    let (p, code) = sim.lobby(3, fleches_course(3));

    // Round 1: everyone answers, mixed correctness, at controlled times.
    let outs = sim.send(p[0].conn, ClientMsg::StartMatch);
    let started = round_started_of(&for_conn(&outs, p[0].conn)).expect("round 1 starts");
    let q = check_question(&started);
    assert_eq!(started.round_index, 1);
    assert_eq!(
        started.timer,
        Timer::Round {
            remaining_ms: QUESTION_MS
        }
    );
    let correct = sim.fleches(&code).correct as u8;
    let question = sim.fleches(&code);
    // The correct arrow on the wire is the true bearing — the question is fair.
    let true_deg = bearing_deg(
        find_exact(&q.origin).unwrap(),
        find_exact(&q.target).unwrap(),
    );
    assert!((q.options_deg[usize::from(correct)] - true_deg).abs() <= 0.11);
    assert_ne!(q.origin, q.target);

    // Wrong answer first: no reveal, only progress.
    sim.now += 3_000;
    let outs = sim.answer(&p[1], (correct + 1) % 4);
    for o in &outs {
        assert!(
            !encode(&o.msg).contains("\"option\""),
            "the answer leaked: {:?}",
            o.msg
        );
    }
    assert_eq!(answered(&for_conn(&outs, p[0].conn)), [p[1].id]);

    // Correct answers at known elapsed times (speed bonus maths, §9). No snapshot follows a
    // plain `player_answered` (§6.1: the event carries everything); a resume forces one,
    // which `resuming_mid_question_restores_the_question_and_who_answered` checks.
    sim.now += 2_000; // 5 s into the question
    sim.answer(&p[0], correct);
    sim.now += 9_000; // 14 s into the question
    let outs = sim.answer(&p[2], correct);
    let ended = round_ended(&for_conn(&outs, p[0].conn)).expect("the round ends");
    assert_eq!(ended.secret, None, "no secret city in Flèches");
    assert_eq!(ended.results, None, "no devinette results in Flèches");
    let rv = reveal(&ended);
    assert_eq!(rv.correct, correct);
    assert_eq!(rv.distance_km, question.distance_km.round() as u32);
    assert_eq!(
        rv.compass,
        question.compass().label_fr(),
        "the compass label of the correct arrow"
    );
    let by_id = |id: PlayerId| rv.answers.iter().find(|a| a.player_id == id).unwrap();
    // p0 answered at 5 s of 15: 100 + round(50 × 10/15) = 100 + 33 = 133.
    assert_eq!(
        (by_id(p[0].id).option, by_id(p[0].id).points),
        (Some(correct), 133)
    );
    // p1 was wrong at 3 s.
    assert_eq!(
        (
            by_id(p[1].id).option,
            by_id(p[1].id).correct,
            by_id(p[1].id).points
        ),
        (Some((correct + 1) % 4), false, 0)
    );
    // p2 answered at 14 s: 100 + round(50 × 1/15) = 100 + 3 = 103.
    assert_eq!(by_id(p[2].id).points, 103);
    assert!(by_id(p[0].id).correct && by_id(p[2].id).correct);

    // Rounds 2 and 3: everyone answers correctly at once, from the first answer.
    sim.drain_all();
    let mut last = Vec::new();
    for round in 2..=3u8 {
        sim.advance(INTERMISSION_MS);
        let started = round_started_of(&sim.take(p[0].conn)).expect("next round starts");
        assert_eq!(started.round_index, round);
        check_question(&started);
        let correct = sim.fleches(&code).correct as u8;
        sim.answer(&p[0], correct);
        sim.answer(&p[1], correct);
        let outs = sim.answer(&p[2], correct);
        last = for_conn(&outs, p[0].conn);
        let ended = round_ended(&last).expect("the round ends when all connected answered");
        assert!(reveal(&ended).answers.iter().all(|a| a.correct));
        assert_eq!(reveal(&ended).answers.len(), 3);
    }
    // The match ended in the same step as the last round.
    let ended = round_ended(&last).expect("last round ended");
    assert_eq!(ended.next_round_in_ms, None);
    let m = match_ended(&last).expect("the match ends with the last round");
    assert_eq!(m.mode, Mode::Course);
    let ranking: Vec<(PlayerId, u32)> = m.ranking.iter().map(|r| (r.player_id, r.total)).collect();
    // p0: 133 + 150 + 150, p1: 0 + 150 + 150, p2: 103 + 150 + 150.
    assert_eq!(
        ranking,
        vec![(p[0].id, 433), (p[2].id, 403), (p[1].id, 300)]
    );
    assert_eq!(m.ranking[0].rank, 1);
    assert_eq!(m.ranking[0].solved_rounds, 3);
    sim.check()
}

#[test]
fn course_question_time_out_scores_the_unanswered_at_zero() {
    let mut sim = Sim::new(11);
    let (p, code) = sim.started(2, fleches_course(1));
    let correct = sim.fleches(&code).correct as u8;
    sim.drain_all();
    // One player answers late (14 s), the other never does.
    sim.now += 14_000;
    let outs = sim.answer(&p[1], correct);
    assert!(
        round_ended(&for_conn(&outs, p[0].conn)).is_none(),
        "still waiting for p0"
    );
    sim.drain_all();
    // A late answer after the deadline is refused: the timer fired first.
    let outs = sim.advance(1_000);
    let ended = round_ended(&for_conn(&outs, p[0].conn)).expect("the question time ran out");
    let rv = reveal(&ended);
    let by_id = |id: PlayerId| rv.answers.iter().find(|a| a.player_id == id).unwrap();
    assert_eq!(by_id(p[1].id).option, Some(correct));
    assert_eq!(by_id(p[1].id).points, 103, "correct at 14 s of 15: bonus 3");
    assert_eq!(
        (
            by_id(p[0].id).option,
            by_id(p[0].id).correct,
            by_id(p[0].id).points
        ),
        (None, false, 0),
        "no answer: no option, no points"
    );
    assert!(match_ended(&for_conn(&outs, p[0].conn)).is_some());
    sim.check();
}

#[test]
fn duplicate_invalid_and_foreign_answers_are_refused() {
    let mut sim = Sim::new(13);
    let (p, code) = sim.lobby(2, fleches_course(2));
    sim.send(p[0].conn, ClientMsg::StartMatch);
    let correct = sim.fleches(&code).correct as u8;
    sim.drain_all();

    // A `guess` in a Flèches room is invalid (the game has no cities to guess).
    let outs = sim.guess(&p[0], "Lyon");
    assert_eq!(
        errors(&for_conn(&outs, p[0].conn)),
        [ErrorCode::InvalidState]
    );
    // Out of range: 4 is not an option.
    let outs = sim.answer(&p[0], 4);
    assert_eq!(
        errors(&for_conn(&outs, p[0].conn)),
        [ErrorCode::InvalidOption]
    );
    // A second answer for the same question is refused…
    sim.answer(&p[0], correct);
    sim.drain_all();
    let outs = sim.answer(&p[0], correct);
    assert_eq!(errors(&for_conn(&outs, p[0].conn)), [ErrorCode::RoundOver]);
    // …and the round is not over: the other player can still answer.
    let outs = sim.answer(&p[1], correct);
    assert!(
        round_ended(&for_conn(&outs, p[0].conn)).is_some(),
        "both answered: round over"
    );

    // During the intermission both games refuse: the round is over.
    let outs = sim.answer(&p[0], 0);
    assert_eq!(errors(&for_conn(&outs, p[0].conn)), [ErrorCode::RoundOver]);
    let outs = sim.guess(&p[0], "Lyon");
    assert_eq!(errors(&for_conn(&outs, p[0].conn)), [ErrorCode::RoundOver]);
    sim.check();
}

#[test]
fn an_answer_in_a_devinette_room_is_invalid_state() {
    let mut sim = Sim::new(17);
    let (p, code) = sim.started(2, course(1));
    let secret = sim.secret(&code);
    let outs = sim.answer(&p[0], 0);
    assert_eq!(
        errors(&for_conn(&outs, p[0].conn)),
        [ErrorCode::InvalidState]
    );
    // The devinette round is undisturbed.
    let outs = sim.guess(&p[0], secret);
    assert!(
        outs.iter()
            .any(|o| matches!(o.msg, ServerMsg::GuessResult(_)))
    );
}

#[test]
fn a_coop_match_rotates_one_answerer_per_question() {
    let mut sim = Sim::new(19);
    let (p, code) = sim.lobby(3, fleches_coop(3));
    let order: Vec<PlayerId> = p.iter().map(|c| c.id).collect();

    // Question 1: p0 answers, and only p0 may.
    let outs = sim.send(p[0].conn, ClientMsg::StartMatch);
    let correct = sim.fleches(&code).correct as u8;
    let started = round_started_of(&for_conn(&outs, p[0].conn)).expect("question 1 started");
    check_question(&started);
    assert_eq!(started.round_index, 1);
    assert_eq!(
        started.timer,
        Timer::Turn {
            player_id: order[0],
            remaining_ms: QUESTION_MS
        }
    );
    let outs = sim.answer(&p[1], correct); // not the answerer
    assert_eq!(
        errors(&for_conn(&outs, p[1].conn)),
        [ErrorCode::NotYourTurn]
    );
    sim.drain_all();
    let outs = sim.answer(&p[0], (correct + 1) % 4); // wrong for the team
    let ended = round_ended(&for_conn(&outs, p[0].conn)).expect("one answer ends the question");
    let rv = reveal(&ended);
    assert_eq!(rv.correct, correct);
    assert_eq!(ended.found_by, None);
    assert!(rv.answers.iter().all(|a| !a.correct && a.points == 0));
    // The answerer's pick is listed, the others' absence is.
    let by_id = |id: PlayerId| rv.answers.iter().find(|a| a.player_id == id).unwrap();
    assert_eq!(by_id(order[0]).option, Some((correct + 1) % 4));
    assert_eq!(by_id(order[1]).option, None);
    assert_eq!(by_id(order[2]).option, None);
    // Everyone's snapshot shows the team total (0 so far).
    let snap = snapshot(&sim.take(p[1].conn)).unwrap();
    assert!(snap.players.iter().all(|x| x.score == 0));

    // Question 2: p1; a timeout is a wrong answer for the team.
    sim.advance(INTERMISSION_MS);
    let started = round_started_of(&sim.take(p[1].conn)).expect("question 2 started");
    assert_eq!(
        started.timer,
        Timer::Turn {
            player_id: order[1],
            remaining_ms: QUESTION_MS
        }
    );
    let outs = sim.advance(QUESTION_MS);
    let ended = round_ended(&for_conn(&outs, p[0].conn)).expect("the answerer ran out of time");
    assert_eq!(ended.found_by, None);
    let rv = reveal(&ended);
    assert!(rv.answers.iter().all(|a| a.option.is_none() && !a.correct));
    // No try was consumed by a timeout — nothing to consume in Flèches; the question is over.

    // Question 3: p2, correct: the team earns its only point.
    sim.advance(INTERMISSION_MS);
    let correct = sim.fleches(&code).correct as u8;
    let outs = sim.answer(&p[2], correct);
    let ended = round_ended(&for_conn(&outs, p[0].conn)).expect("the question is answered");
    assert_eq!(ended.found_by, Some(order[2]));
    let rv = reveal(&ended);
    let by_id = |id: PlayerId| rv.answers.iter().find(|a| a.player_id == id).unwrap();
    assert_eq!(by_id(order[2]).option, Some(correct));
    assert!(rv.answers.iter().all(|a| a.points == 1 && a.total == 1));

    let m = match_ended(&sim.take(p[0].conn)).expect("the match ends");
    assert_eq!(m.mode, Mode::Cooperation);
    assert!(m.ranking.iter().all(|r| r.rank == 1), "one team, one rank");
    assert!(m.ranking.iter().all(|r| r.total == 1), "one correct answer");
    let solved: Vec<u32> = m.ranking.iter().map(|r| r.solved_rounds).collect();
    assert_eq!(solved, [1, 0, 0], "the answerer of the winning question");
    sim.check()
}

#[test]
fn a_disconnecting_coop_answerer_passes_the_question_on() {
    let mut sim = Sim::new(23);
    let (p, code) = sim.lobby(2, fleches_coop(2));
    sim.send(p[0].conn, ClientMsg::StartMatch);
    let correct = sim.fleches(&code).correct as u8;
    sim.drain_all();
    // The answerer vanishes: the turn passes (full question time again).
    let outs = sim.disconnect(p[0].conn);
    let (next, _, _) = turn_changes(&for_conn(&outs, p[1].conn))
        .first()
        .copied()
        .expect("the turn passes");
    assert_eq!(next, p[1].id);
    let outs = sim.answer(&p[1], correct);
    let ended = round_ended(&for_conn(&outs, p[1].conn)).expect("the new answerer plays");
    assert_eq!(ended.found_by, Some(p[1].id));
    sim.check();
}

#[test]
fn resuming_mid_question_restores_the_question_and_who_answered() {
    let mut sim = Sim::new(29);
    let (p, code) = sim.lobby(3, fleches_course(2));
    sim.send(p[0].conn, ClientMsg::StartMatch);
    let correct = sim.fleches(&code).correct as u8;
    sim.drain_all();
    sim.answer(&p[0], correct);
    sim.drain_all();
    // p2 drops and comes back while the question is still running.
    sim.disconnect(p[2].conn);
    sim.now += 2_000;
    let next = 100;
    let outs = sim.send(next, ClientMsg::hello("P3", Some(p[2].token.clone())));
    assert!(
        outs.iter()
            .any(|o| matches!(o.msg, ServerMsg::Welcome { resumed: true, .. }))
    );
    let snap = snapshot(&for_conn(&outs, next)).expect("a snapshot on resume");
    let view = snap.round.as_ref().expect("the question is still running");
    check_question(&view.start);
    assert_eq!(view.answered.as_deref(), Some(&[p[0].id][..]));
    // The resumed player can answer, and the round ends for the connected three.
    sim.answer(&p[1], correct);
    let outs = sim.answer(
        &Client {
            conn: next,
            id: p[2].id,
            token: p[2].token.clone(),
        },
        correct,
    );
    let ended = round_ended(&for_conn(&outs, p[0].conn)).expect("everyone connected answered");
    assert!(reveal(&ended).answers.iter().all(|a| a.correct));
    sim.check();
}

#[test]
fn a_leaver_does_not_hold_the_question_up() {
    let mut sim = Sim::new(31);
    let (p, code) = sim.lobby(3, fleches_course(2));
    sim.send(p[0].conn, ClientMsg::StartMatch);
    let correct = sim.fleches(&code).correct as u8;
    sim.drain_all();
    // p2 leaves mid-question; the round ends as soon as the remaining two answered.
    let outs = sim.send(p[2].conn, ClientMsg::LeaveRoom);
    assert!(outs.iter().all(|o| o.to != p[2].conn));
    sim.answer(&p[0], correct);
    let outs = sim.answer(&p[1], correct);
    let ended =
        round_ended(&for_conn(&outs, p[0].conn)).expect("the round ends without the leaver");
    let rv = reveal(&ended);
    assert_eq!(rv.answers.len(), 2, "the leaver has no entry");
    assert!(rv.answers.iter().all(|a| a.correct));
    sim.check()
}

// ---- the frame-level secret rule ------------------------------------------------------------

/// A frame that may legitimately carry the reveal: `round_ended` itself, and an
/// intermission snapshot (it embeds the `round_ended` that opened the pause).
fn carries_reveal_licitly(msg: &ServerMsg) -> bool {
    match msg {
        ServerMsg::RoundEnded(_) => true,
        ServerMsg::RoomSnapshot(s) => s.phase == RoomPhase::Intermission,
        _ => false,
    }
}

/// Does the JSON of `msg` contain a `correct`, `option` or `distance_km` key anywhere?
/// Before `round_ended` no Flèches frame may (they are the reveal, §13).
fn has_reveal_key(msg: &ServerMsg) -> bool {
    fn walk(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) => map.iter().any(|(k, v)| {
                matches!(k.as_str(), "correct" | "option" | "distance_km") || walk(v)
            }),
            serde_json::Value::Array(items) => items.iter().any(walk),
            _ => false,
        }
    }
    walk(&serde_json::from_str::<serde_json::Value>(&encode(msg)).expect("encodable"))
}

/// xorshift64*: tiny deterministic generator for the simulation (as in safety.rs).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

#[derive(Default)]
struct Stats {
    matches_finished: usize,
    questions_started: usize,
    reveals: usize,
    frames_checked: usize,
}

fn simulate_fleches_match(seed: u64, stats: &mut Stats) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let config = if seed.is_multiple_of(2) {
        fleches_course(1 + rng.below(3) as u32)
    } else {
        let mut c = fleches_coop(1 + rng.below(3) as u32);
        c.difficulty = if (seed / 4).is_multiple_of(2) {
            Difficulty::Facile
        } else {
            Difficulty::Difficile
        };
        c
    };
    let mut sim = Sim::new(seed);
    let n = 2 + rng.below(4);
    let (mut players, code) = sim.lobby(n, config);
    let mut next_conn = 1_000;
    let mut finished = false;

    // One engine step: no frame before the `round_ended` position may carry a reveal key,
    // and a reveal must match the question the engine had hidden. Advances stay below the
    // 8 s intermission, so a round can never start *and* end inside one step.
    let step = |sim: &mut Sim,
                stats: &mut Stats,
                act: &mut dyn FnMut(&mut Sim) -> Vec<Outbound>|
     -> bool {
        let before = sim.engine.debug_fleches(&code);
        let outs = act(sim);
        let reveal_at = outs
            .iter()
            .position(|o| matches!(o.msg, ServerMsg::RoundEnded(_)))
            .unwrap_or(outs.len());
        for (i, o) in outs.iter().enumerate() {
            stats.frames_checked += 1;
            if !carries_reveal_licitly(&o.msg) {
                assert!(
                    !has_reveal_key(&o.msg),
                    "reveal key in a pre-reveal frame: {:?}",
                    o.msg
                );
            } else if i == reveal_at
                && let ServerMsg::RoundEnded(e) = &o.msg
            {
                let question = before.as_ref().expect("a round was active to be ended");
                stats.reveals += 1;
                let rv = e.fleches.as_ref().expect("the reveal");
                assert_eq!(rv.correct, question.correct as u8, "the revealed index");
                assert_eq!(rv.distance_km, question.distance_km.round() as u32);
            }
        }
        stats.questions_started += outs
            .iter()
            .filter(|o| matches!(o.msg, ServerMsg::RoundStarted(_)))
            .count();
        sim.check();
        outs.iter()
            .any(|o| matches!(o.msg, ServerMsg::MatchEnded(_)))
    };

    let host = players[0].conn;
    if step(&mut sim, stats, &mut |s| {
        s.send(host, ClientMsg::StartMatch)
    }) {
        stats.matches_finished += 1;
        return;
    }
    for _ in 0..4_000 {
        let i = rng.below(players.len());
        let action = rng.below(100);
        let (conn, token) = (players[i].conn, players[i].token.clone());
        let player = players[i].clone();
        let new_conn = next_conn + 1;
        let ended = step(&mut sim, stats, &mut |s| {
            match action {
                // Answer: usually the hidden correct index, sometimes another arrow,
                // sometimes an out-of-range one (refused with invalid_option).
                0..=49 => {
                    let option = match s.engine.debug_fleches(&code) {
                        Some(q) if rng.chance(70) => q.correct as u8,
                        Some(q) => (q.correct as u8 + 1 + rng.below(3) as u8) % 4,
                        None => 0,
                    };
                    s.answer(&player, option)
                }
                50..=59 => s.answer(&player, rng.below(4) as u8),
                60..=64 => s.answer(&player, 4 + rng.below(3) as u8),
                // Drop and resume on a fresh socket (the old id is never reused).
                65..=84 => {
                    let mut outs = s.disconnect(conn);
                    s.now += rng.below(2_000) as u64;
                    outs.extend(s.send(new_conn, ClientMsg::hello("x", Some(token.clone()))));
                    outs
                }
                85..=89 => s.send(conn, ClientMsg::LeaveRoom),
                _ => Vec::new(),
            }
        });
        if (65..=84).contains(&action) {
            next_conn = new_conn;
            players[i].conn = new_conn;
        }
        if ended {
            finished = true;
            break;
        }
        let ms = rng.below(4_000) as u64;
        if step(&mut sim, stats, &mut |s| s.advance(ms)) {
            finished = true;
            break;
        }
    }
    if finished {
        stats.matches_finished += 1;
    }
}

#[test]
fn the_correct_arrow_and_distance_never_leave_before_round_ended() {
    let mut stats = Stats::default();
    for seed in 0..80 {
        simulate_fleches_match(seed, &mut stats);
    }
    assert!(
        stats.matches_finished >= 70,
        "only {} matches finished",
        stats.matches_finished
    );
    assert!(
        stats.questions_started >= 160,
        "{} questions started",
        stats.questions_started
    );
    assert!(stats.reveals >= 100, "{} reveals checked", stats.reveals);
    assert!(
        stats.frames_checked > 10_000,
        "{} frames checked",
        stats.frames_checked
    );
}
