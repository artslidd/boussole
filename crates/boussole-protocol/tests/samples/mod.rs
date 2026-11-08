//! One sample per message shape. The JSON of each sample is pinned in `tests/fixtures/` and
//! quoted verbatim in `docs/protocol.md`.

use boussole_protocol::*;

pub fn client_samples() -> Vec<(&'static str, ClientMsg)> {
    let fingerprint = 0x9907_ec8a_e376_fbf8;
    let cfg = RoomConfig {
        mode: Mode::Course,
        difficulty: Difficulty::Facile,
        rounds: 3,
        max_players: 4,
        round_time_s: 90,
        turn_time_s: 45,
    };
    vec![
        (
            "hello",
            ClientMsg::Hello {
                protocol_version: PROTOCOL_VERSION,
                dataset_fingerprint: fingerprint,
                nickname: "Alice".into(),
                resume_token: None,
            },
        ),
        (
            "hello_resume",
            ClientMsg::Hello {
                protocol_version: PROTOCOL_VERSION,
                dataset_fingerprint: fingerprint,
                nickname: "Alice".into(),
                resume_token: Some("3f9a1c5e7b2d4086a1b3c5d7e9f01234".into()),
            },
        ),
        ("create_room", ClientMsg::CreateRoom { config: cfg }),
        (
            "join_room",
            ClientMsg::JoinRoom {
                code: "K7QX2".into(),
            },
        ),
        ("leave_room", ClientMsg::LeaveRoom),
        (
            "update_config",
            ClientMsg::UpdateConfig {
                config: RoomConfig {
                    mode: Mode::Cooperation,
                    difficulty: Difficulty::Difficile,
                    ..cfg
                },
            },
        ),
        ("start_match", ClientMsg::StartMatch),
        (
            "guess",
            ClientMsg::Guess {
                city: "Saint-Étienne".into(),
            },
        ),
        ("ping", ClientMsg::Ping),
    ]
}

fn hints(facile: bool) -> Vec<HintDto> {
    let rows = [
        ("Amiens", 330.5, 403),
        ("Belfort", 48.0, 77),
        ("Bastia", 128.5, 572),
        ("Perpignan", 201.0, 562),
        ("Angers", 275.5, 497),
    ];
    rows.iter()
        .map(|&(city, bearing_deg, km)| HintDto {
            city: city.into(),
            bearing_deg,
            distance_km: facile.then_some(km),
        })
        .collect()
}

fn players() -> Vec<PlayerInfo> {
    let mut players = players_all_connected();
    players[1].connected = false;
    players
}

fn players_all_connected() -> Vec<PlayerInfo> {
    vec![
        PlayerInfo {
            id: 1,
            nickname: "Alice".into(),
            host: true,
            connected: true,
            score: 0,
        },
        PlayerInfo {
            id: 2,
            nickname: "Bob".into(),
            host: false,
            connected: true,
            score: 0,
        },
    ]
}

fn config(mode: Mode, difficulty: Difficulty) -> RoomConfig {
    RoomConfig {
        mode,
        difficulty,
        rounds: 3,
        max_players: 4,
        round_time_s: 90,
        turn_time_s: 45,
    }
}

fn guess(by: PlayerId, city: &str, km: u32, bearing: f64, tries: u8, correct: bool) -> GuessInfo {
    GuessInfo {
        by,
        city: city.into(),
        distance_km: km,
        bearing_deg: bearing,
        correct,
        tries_used: tries,
    }
}

fn course_ended() -> RoundEnded {
    let entry = |city: &str, distance_km| GuessEntry {
        city: city.into(),
        distance_km,
    };
    RoundEnded {
        round_index: 1,
        total_rounds: 3,
        secret: "Besançon".into(),
        found_by: None,
        results: vec![
            PlayerRoundResult {
                player_id: 1,
                guesses: vec![entry("Lyon", 163), entry("Dijon", 76), entry("Besançon", 0)],
                solved: true,
                tries_used: 3,
                points: 496,
                total: 496,
            },
            PlayerRoundResult {
                player_id: 2,
                guesses: vec![entry("Paris", 327)],
                solved: false,
                tries_used: 1,
                points: 0,
                total: 0,
            },
        ],
        next_round_in_ms: Some(8000),
    }
}

fn coop_ended() -> RoundEnded {
    let entry = |city: &str, distance_km| GuessEntry {
        city: city.into(),
        distance_km,
    };
    RoundEnded {
        round_index: 2,
        total_rounds: 3,
        secret: "Nîmes".into(),
        found_by: Some(2),
        results: vec![
            PlayerRoundResult {
                player_id: 1,
                guesses: vec![entry("Lyon", 215)],
                solved: false,
                tries_used: 1,
                points: 500,
                total: 900,
            },
            PlayerRoundResult {
                player_id: 2,
                guesses: vec![entry("Montpellier", 49), entry("Nîmes", 0)],
                solved: true,
                tries_used: 2,
                points: 500,
                total: 900,
            },
        ],
        next_round_in_ms: Some(8000),
    }
}

pub fn server_samples() -> Vec<(&'static str, ServerMsg)> {
    vec![
        (
            "welcome",
            ServerMsg::Welcome {
                player_id: 1,
                resume_token: "3f9a1c5e7b2d4086a1b3c5d7e9f01234".into(),
                resumed: false,
            },
        ),
        (
            "room_snapshot_lobby",
            ServerMsg::RoomSnapshot(RoomSnapshot {
                code: "K7QX2".into(),
                config: config(Mode::Course, Difficulty::Facile),
                phase: RoomPhase::Lobby,
                players: players(),
                round: None,
                intermission: None,
            }),
        ),
        (
            "room_snapshot_course_round",
            ServerMsg::RoomSnapshot(RoomSnapshot {
                code: "K7QX2".into(),
                config: config(Mode::Course, Difficulty::Difficile),
                phase: RoomPhase::RoundActive,
                players: players(),
                round: Some(RoundView {
                    start: RoundStarted {
                        round_index: 1,
                        total_rounds: 3,
                        max_tries: 6,
                        hints: hints(false),
                        timer: Timer::Round {
                            remaining_ms: 71_250,
                        },
                    },
                    guesses: vec![
                        guess(1, "Lyon", 163, 24.5, 1, false),
                        guess(1, "Dijon", 76, 112.0, 2, false),
                    ],
                    progress: vec![
                        Progress {
                            player_id: 1,
                            tries_used: 2,
                            solved: false,
                            out_of_tries: false,
                        },
                        Progress {
                            player_id: 2,
                            tries_used: 1,
                            solved: false,
                            out_of_tries: false,
                        },
                    ],
                }),
                intermission: None,
            }),
        ),
        (
            "room_snapshot_coop_round",
            ServerMsg::RoomSnapshot(RoomSnapshot {
                code: "K7QX2".into(),
                config: config(Mode::Cooperation, Difficulty::Facile),
                phase: RoomPhase::RoundActive,
                players: players_all_connected()
                    .into_iter()
                    .map(|p| PlayerInfo { score: 600, ..p })
                    .collect(),
                round: Some(RoundView {
                    start: RoundStarted {
                        round_index: 2,
                        total_rounds: 3,
                        max_tries: 6,
                        hints: hints(true),
                        timer: Timer::Turn {
                            player_id: 2,
                            remaining_ms: 31_000,
                        },
                    },
                    guesses: vec![guess(1, "Lyon", 215, 201.5, 1, false)],
                    progress: vec![],
                }),
                intermission: None,
            }),
        ),
        (
            "room_snapshot_intermission",
            ServerMsg::RoomSnapshot(RoomSnapshot {
                code: "K7QX2".into(),
                config: config(Mode::Course, Difficulty::Facile),
                phase: RoomPhase::Intermission,
                players: players(),
                round: None,
                intermission: Some(Intermission {
                    next_round_index: 2,
                    remaining_ms: 5_500,
                    last_round: RoundEnded {
                        next_round_in_ms: Some(5_500),
                        ..course_ended()
                    },
                }),
            }),
        ),
        (
            "round_started_course_facile",
            ServerMsg::RoundStarted(RoundStarted {
                round_index: 1,
                total_rounds: 3,
                max_tries: 6,
                hints: hints(true),
                timer: Timer::Round {
                    remaining_ms: 90_000,
                },
            }),
        ),
        (
            "round_started_coop_difficile",
            ServerMsg::RoundStarted(RoundStarted {
                round_index: 2,
                total_rounds: 3,
                max_tries: 6,
                hints: hints(false),
                timer: Timer::Turn {
                    player_id: 2,
                    remaining_ms: 45_000,
                },
            }),
        ),
        (
            "guess_result",
            ServerMsg::GuessResult(guess(1, "Lyon", 163, 24.5, 1, false)),
        ),
        (
            "player_progress",
            ServerMsg::PlayerProgress(Progress {
                player_id: 1,
                tries_used: 3,
                solved: true,
                out_of_tries: false,
            }),
        ),
        (
            "turn_changed",
            ServerMsg::TurnChanged {
                player_id: 2,
                remaining_ms: 45_000,
                reason: TurnReason::Guess,
            },
        ),
        ("round_ended_course", ServerMsg::RoundEnded(course_ended())),
        ("round_ended_coop", ServerMsg::RoundEnded(coop_ended())),
        (
            "match_ended",
            ServerMsg::MatchEnded(MatchEnded {
                mode: Mode::Course,
                ranking: vec![
                    RankEntry {
                        player_id: 1,
                        nickname: "Alice".into(),
                        total: 1_190,
                        rank: 1,
                        solved_rounds: 3,
                    },
                    RankEntry {
                        player_id: 2,
                        nickname: "Bob".into(),
                        total: 640,
                        rank: 2,
                        solved_rounds: 2,
                    },
                ],
            }),
        ),
        ("error", ServerMsg::error(ErrorCode::RoomFull)),
        ("pong", ServerMsg::Pong),
    ]
}
