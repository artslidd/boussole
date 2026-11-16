//! Secret safety over simulated matches, and engine robustness against random input.

mod common;
use boussole_core::cities::CITIES;
use boussole_protocol::*;
use boussole_server::engine::{ConnId, Outbound};
use common::*;
use std::collections::BTreeMap;

/// xorshift64*: tiny deterministic generator for the simulations.
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
    fn city(&mut self) -> &'static str {
        CITIES[self.below(CITIES.len())].name
    }
}

/// Does `msg` reveal `secret`? A player's own *correct* guess is allowed: that is how a
/// player finds out they won (Course: private; Coopération: broadcast, and the round ends
/// in the same step).
fn leaks(msg: &ServerMsg, secret: &str) -> bool {
    let quoted = format!("\"{secret}\"");
    match msg {
        ServerMsg::GuessResult(g) => g.city == secret && !g.correct,
        ServerMsg::RoomSnapshot(s) => {
            let mut s = s.clone();
            if let Some(round) = &mut s.round {
                round.guesses.retain(|g| !(g.correct && g.city == secret));
            }
            encode(&ServerMsg::RoomSnapshot(s)).contains(&quoted)
        }
        other => encode(other).contains(&quoted),
    }
}

fn hints_in(msg: &ServerMsg) -> Option<&Vec<HintDto>> {
    match msg {
        ServerMsg::RoundStarted(r) => r.hints.as_ref(),
        ServerMsg::RoomSnapshot(s) => s.round.as_ref().and_then(|r| r.start.hints.as_ref()),
        _ => None,
    }
}

#[derive(Default)]
struct Stats {
    matches_finished: usize,
    rounds_started: usize,
    correct_guesses: usize,
    messages: usize,
}

fn check_step(
    outs: &[Outbound],
    before: Option<&str>,
    after: Option<&str>,
    difficulty: Difficulty,
    mode: Mode,
    who: &BTreeMap<ConnId, PlayerId>,
    stats: &mut Stats,
) {
    let first_ended = outs
        .iter()
        .position(|o| matches!(o.msg, ServerMsg::RoundEnded(_)))
        .unwrap_or(outs.len());
    let first_started = outs
        .iter()
        .position(|o| matches!(o.msg, ServerMsg::RoundStarted(_)));
    for (i, o) in outs.iter().enumerate() {
        stats.messages += 1;
        if let Some(s) = before
            && i < first_ended
        {
            assert!(
                !leaks(&o.msg, s),
                "secret {s} leaked before round_ended: {:?}",
                o.msg
            );
        }
        if let (Some(s), Some(start)) = (after, first_started)
            && before != after
            && i >= start
        {
            assert!(
                !leaks(&o.msg, s),
                "secret {s} leaked in the new round: {:?}",
                o.msg
            );
        }
        if difficulty == Difficulty::Difficile {
            if let ServerMsg::RoundStarted(_) = &o.msg {
                assert!(!encode(&o.msg).contains("distance_km"), "{:?}", o.msg);
            }
            if let Some(hints) = hints_in(&o.msg) {
                assert!(hints.iter().all(|h| h.distance_km.is_none()));
                assert!(
                    !serde_json::to_string(hints)
                        .unwrap()
                        .contains("distance_km")
                );
            }
        } else if let Some(hints) = hints_in(&o.msg) {
            assert!(hints.iter().all(|h| h.distance_km.is_some()));
        }
        if let ServerMsg::GuessResult(g) = &o.msg {
            if g.correct {
                stats.correct_guesses += 1;
            }
            if mode == Mode::Course {
                assert_eq!(
                    Some(&g.by),
                    who.get(&o.to),
                    "course feedback must stay private"
                );
            }
        }
    }
}

fn simulate_match(seed: u64, stats: &mut Stats) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mode = if seed.is_multiple_of(2) {
        Mode::Course
    } else {
        Mode::Cooperation
    };
    let difficulty = if (seed / 2).is_multiple_of(2) {
        Difficulty::Facile
    } else {
        Difficulty::Difficile
    };
    let config = RoomConfig {
        rounds: 1 + rng.below(3) as u32,
        round_time_s: 60,
        turn_time_s: 15 + 5 * rng.below(4) as u32,
        ..RoomConfig::new(mode, difficulty)
    };
    let mut sim = Sim::new(seed);
    let n = 2 + rng.below(5);
    let (mut players, code) = sim.lobby(n, config);
    let mut who: BTreeMap<ConnId, PlayerId> = players.iter().map(|c| (c.conn, c.id)).collect();
    let mut next_conn = 1_000;
    let mut finished = false;
    // Every engine step is checked against the secret in force just before and just after it.
    let step = |sim: &mut Sim,
                stats: &mut Stats,
                who: &BTreeMap<ConnId, PlayerId>,
                act: &mut dyn FnMut(&mut Sim) -> Vec<Outbound>|
     -> bool {
        let before = sim.engine.debug_secret(&code);
        let outs = act(sim);
        let after = sim.engine.debug_secret(&code);
        check_step(&outs, before, after, difficulty, mode, who, stats);
        stats.rounds_started += outs
            .iter()
            .filter(|o| o.to == 1 && matches!(o.msg, ServerMsg::RoundStarted(_)))
            .count();
        sim.check();
        outs.iter()
            .any(|o| matches!(o.msg, ServerMsg::MatchEnded(_)))
    };
    let host = players[0].conn;
    step(&mut sim, stats, &who, &mut |s| {
        s.send(host, ClientMsg::StartMatch)
    });
    for _ in 0..4_000 {
        // One random bot action, then a little time.
        let i = rng.below(players.len());
        let action = rng.below(100);
        let city = rng.city();
        let pause = rng.below(3_000) as u64;
        let (conn, token) = (players[i].conn, players[i].token.clone());
        let player = players[i].clone();
        let new_conn = next_conn + 1;
        let ended = step(&mut sim, stats, &who, &mut |s| {
            if action < 30 {
                match s.engine.debug_secret(&code) {
                    Some(secret) => s.guess(&player, secret),
                    None => s.send(conn, ClientMsg::Ping),
                }
            } else if action < 85 {
                s.guess(&player, city)
            } else if action < 90 {
                // Drop and resume on a fresh socket (the old id is never reused).
                let mut outs = s.disconnect(conn);
                s.now += pause;
                outs.extend(s.send(new_conn, ClientMsg::hello("x", Some(token.clone()))));
                outs
            } else {
                Vec::new()
            }
        });
        if (85..90).contains(&action) {
            next_conn = new_conn;
            who.remove(&conn);
            players[i].conn = new_conn;
            who.insert(new_conn, players[i].id);
        }
        if ended {
            finished = true;
            break;
        }
        let ms = rng.below(5_000) as u64;
        if step(&mut sim, stats, &who, &mut |s| s.advance(ms)) {
            finished = true;
            break;
        }
    }
    sim.check();
    if finished {
        stats.matches_finished += 1;
    }
}

#[test]
fn the_secret_never_leaves_the_server_before_round_ended_in_200_matches() {
    let mut stats = Stats::default();
    for seed in 0..200 {
        simulate_match(seed, &mut stats);
    }
    // The simulation must actually exercise what it claims to check.
    assert!(
        stats.matches_finished >= 190,
        "only {} matches finished",
        stats.matches_finished
    );
    assert!(
        stats.rounds_started >= 200,
        "{} rounds",
        stats.rounds_started
    );
    assert!(
        stats.correct_guesses >= 100,
        "{} correct guesses",
        stats.correct_guesses
    );
    assert!(
        stats.messages > 10_000,
        "{} messages checked",
        stats.messages
    );
}

// ---- random input ---------------------------------------------------------------------------

fn random_config(rng: &mut Rng) -> RoomConfig {
    let pick = |rng: &mut Rng, valid: std::ops::RangeInclusive<u32>| match rng.below(6) {
        0 => 0,
        1 => u32::MAX,
        2 => valid.end() + 1,
        3 => valid.start().saturating_sub(1),
        _ => valid.start() + rng.next() as u32 % (valid.end() - valid.start() + 1),
    };
    RoomConfig {
        mode: if rng.chance(50) {
            Mode::Course
        } else {
            Mode::Cooperation
        },
        game: if rng.chance(50) {
            GameKind::Devinette
        } else {
            GameKind::Fleches
        },
        difficulty: if rng.chance(50) {
            Difficulty::Facile
        } else {
            Difficulty::Difficile
        },
        rounds: pick(rng, ROUNDS_RANGE),
        max_players: pick(rng, MAX_PLAYERS_RANGE),
        round_time_s: pick(rng, ROUND_TIME_S_RANGE),
        turn_time_s: pick(rng, TURN_TIME_S_RANGE),
        question_time_s: pick(rng, QUESTION_TIME_S_RANGE),
    }
}

fn garbage_frame(rng: &mut Rng) -> String {
    const PIECES: [&str; 14] = [
        "{",
        "}",
        "\"type\"",
        ":",
        "\"guess\"",
        "\"hello\"",
        ",",
        "null",
        "[",
        "1e999",
        "\u{0}",
        "\"city\"",
        "😀",
        "\\u00",
    ];
    match rng.below(5) {
        0 => String::new(),
        1 => "x".repeat(MAX_FRAME_BYTES + 1 + rng.below(100)),
        2 => (0..rng.below(12))
            .map(|_| PIECES[rng.below(PIECES.len())])
            .collect(),
        3 => {
            let valid = encode_client(&ClientMsg::hello("Alice", None));
            valid[..rng.below(valid.len())].to_owned()
        }
        _ => format!(
            "{{\"type\":\"guess\",\"city\":\"{}\"}}",
            "é".repeat(rng.below(3_000))
        ),
    }
}

fn fuzz(seed: u64, steps: usize) -> usize {
    let mut rng = Rng(seed.wrapping_mul(0xD6E8_FEB8_6659_FD93) | 1);
    let mut sim = Sim::new(seed);
    let mut conns: Vec<ConnId> = (1..=12).collect();
    let mut next_conn = 100;
    let mut tokens: Vec<String> = Vec::new();
    let mut codes: Vec<String> = Vec::new();
    let mut messages = 0;
    for _ in 0..steps {
        // Closed sockets are replaced, as a real transport would.
        for c in conns.iter_mut() {
            if sim.closed.contains(c) {
                next_conn += 1;
                *c = next_conn;
            }
        }
        let conn = conns[rng.below(conns.len())];
        let outs = match rng.below(100) {
            0..=9 => {
                messages += 1;
                let frame = garbage_frame(&mut rng);
                sim.frame(conn, &frame)
            }
            10..=14 => sim.disconnect(conn),
            15..=29 => {
                let ms = match rng.below(6) {
                    0 => 0,
                    1 => 1_000,
                    2 => 20_000,
                    3 => RESUME_GRACE_MS + 1,
                    4 => ROOM_IDLE_MS / 10,
                    _ => rng.below(5_000) as u64,
                };
                sim.advance(ms)
            }
            _ => {
                messages += 1;
                let msg = match rng.below(14) {
                    0 | 1 => {
                        let version = if rng.chance(90) {
                            PROTOCOL_VERSION
                        } else {
                            rng.next() as u16
                        };
                        let fingerprint = if rng.chance(95) {
                            boussole_core::dataset_fingerprint()
                        } else {
                            rng.next()
                        };
                        let nickname = match rng.below(8) {
                            0 => String::new(),
                            1 => "x".repeat(40),
                            2 => "bad\nname".to_owned(),
                            3 => " ".to_owned(),
                            _ => format!("Player{}", rng.below(6)),
                        };
                        let resume_token = if !tokens.is_empty() && rng.chance(30) {
                            Some(tokens[rng.below(tokens.len())].clone())
                        } else if rng.chance(10) {
                            Some("nope".to_owned())
                        } else {
                            None
                        };
                        ClientMsg::Hello {
                            protocol_version: version,
                            dataset_fingerprint: fingerprint,
                            nickname,
                            resume_token,
                        }
                    }
                    2 | 3 => ClientMsg::CreateRoom {
                        config: random_config(&mut rng),
                    },
                    4 | 5 => {
                        let code = if !codes.is_empty() && rng.chance(80) {
                            codes[rng.below(codes.len())].clone()
                        } else {
                            ["", "ZZZZZ", "k7qx2", "  ", "AB0DE", "é"][rng.below(6)].to_owned()
                        };
                        ClientMsg::JoinRoom { code }
                    }
                    6 => ClientMsg::LeaveRoom,
                    7 => ClientMsg::UpdateConfig {
                        config: random_config(&mut rng),
                    },
                    8 => ClientMsg::StartMatch,
                    9..=11 => {
                        let secret = codes
                            .iter()
                            .filter_map(|c| sim.engine.debug_secret(c))
                            .next();
                        let city = match (secret, rng.below(10)) {
                            (Some(s), 0..=3) => s.to_owned(),
                            (_, 4) => "Atlantis".to_owned(),
                            _ => rng.city().to_owned(),
                        };
                        ClientMsg::Guess { city }
                    }
                    12 => ClientMsg::Answer {
                        // 0–3 are valid options; 4–5 probe the invalid_option path.
                        option: rng.below(6) as u8,
                    },
                    _ => ClientMsg::hello("Player0", None),
                };
                sim.send(conn, msg)
            }
        };
        for o in &outs {
            match &o.msg {
                ServerMsg::Welcome { resume_token, .. } => tokens.push(resume_token.clone()),
                ServerMsg::RoomSnapshot(s) if !codes.contains(&s.code) => {
                    codes.push(s.code.clone())
                }
                _ => {}
            }
        }
        if let Err(e) = sim.engine.check_invariants() {
            panic!("seed {seed}: invariant violated: {e}");
        }
        // Keep the log from growing without bound.
        sim.log.clear();
        sim.inbox.clear();
    }
    messages
}

#[test]
fn the_engine_survives_more_than_10000_random_messages() {
    let total: usize = (0..6).map(|seed| fuzz(seed, 4_000)).sum();
    assert!(
        total >= 10_000,
        "only {total} messages were thrown at the engine"
    );
}

#[test]
fn the_engine_survives_heavy_churn_in_a_single_room() {
    // Everybody keeps joining, dropping, resuming and guessing in one busy room.
    let mut rng = Rng(0xABCDEF);
    let mut sim = Sim::new(5);
    let (mut players, code) = sim.lobby(6, coop(3));
    sim.send(players[0].conn, ClientMsg::StartMatch);
    let mut next = 500;
    for _ in 0..3_000 {
        let i = rng.below(players.len());
        match rng.below(6) {
            0 => {
                sim.disconnect(players[i].conn);
            }
            1 => {
                next += 1;
                sim.send(next, ClientMsg::hello("x", Some(players[i].token.clone())));
                players[i].conn = next;
            }
            2 => {
                sim.advance(rng.below(40_000) as u64);
            }
            _ => {
                let city = match sim.engine.debug_secret(&code) {
                    Some(s) if rng.chance(30) => s,
                    _ => rng.city(),
                };
                sim.guess(&players[i], city);
            }
        }
        sim.check();
        sim.log.clear();
        sim.inbox.clear();
    }
}
