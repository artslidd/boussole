//! Decoding robustness, validation helpers and error texts.

use boussole_protocol::*;

fn hello(nickname: &str) -> String {
    format!(
        r#"{{"type":"hello","protocol_version":1,"dataset_fingerprint":"{:016x}","nickname":"{nickname}"}}"#,
        boussole_core::dataset_fingerprint()
    )
}

#[test]
fn round_trip_through_the_public_codec() {
    let msg = ClientMsg::hello("Alice", Some("ab".repeat(16)));
    assert_eq!(decode_client(&encode_client(&msg)).unwrap(), msg);
    let msg = ServerMsg::error(ErrorCode::NotHost);
    assert_eq!(decode_server(&encode(&msg)).unwrap(), msg);
}

#[test]
fn hello_helper_carries_this_builds_version_and_dataset() {
    let ClientMsg::Hello {
        protocol_version,
        dataset_fingerprint,
        ..
    } = ClientMsg::hello("A", None)
    else {
        panic!("not a hello")
    };
    assert_eq!(protocol_version, PROTOCOL_VERSION);
    assert_eq!(dataset_fingerprint, boussole_core::dataset_fingerprint());
}

#[test]
fn frames_over_the_limit_are_rejected_before_parsing() {
    // A syntactically valid message padded past 4096 bytes.
    let padded = hello(&"x".repeat(MAX_FRAME_BYTES));
    assert!(padded.len() > MAX_FRAME_BYTES);
    let err = decode_client(&padded).unwrap_err();
    assert!(matches!(err, ProtocolError::FrameTooLarge { .. }), "{err}");
    assert_eq!(err.code(), ErrorCode::BadMessage);
    // Exactly at the limit is fine.
    let base = hello("");
    let at_limit = hello(&"x".repeat(MAX_FRAME_BYTES - base.len()));
    assert_eq!(at_limit.len(), MAX_FRAME_BYTES);
    assert!(decode_client(&at_limit).is_ok());
}

#[test]
fn unknown_type_and_malformed_json_are_bad_message_without_panicking() {
    let bad = [
        "",
        " ",
        "null",
        "42",
        "[]",
        "{}",
        r#"{"type":"nope"}"#,
        r#"{"type":"HELLO"}"#,
        r#"{"type":"ping"#,
        r#"{"type":"guess"}"#,
        r#"{"type":"guess","city":7}"#,
        r#"{"type":"join_room","code":null}"#,
        r#"{"type":"create_room","config":{"mode":"battle","difficulty":"facile"}}"#,
        r#"{"type":"create_room","config":{"mode":"course","difficulty":"facile","rounds":-1}}"#,
        r#"{"type":"hello","protocol_version":1,"dataset_fingerprint":"zz","nickname":"A"}"#,
        r#"{"type":"hello","protocol_version":1,"dataset_fingerprint":42,"nickname":"A"}"#,
        r#"{"type":"hello","protocol_version":99999,"dataset_fingerprint":"0000000000000000","nickname":"A"}"#,
        "\u{0}",
        "{\"type\":\"ping\"}garbage",
    ];
    for frame in bad {
        let err = decode_client(frame).expect_err(frame);
        assert_eq!(err.code(), ErrorCode::BadMessage, "{frame}");
    }
}

#[test]
fn decoding_arbitrary_text_never_panics() {
    // Deterministic pseudo-random strings and truncations of valid frames.
    let mut state = 0x1234_5678_9abc_def0_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet: Vec<char> = "{}[]\":,0123456789 \\/tfnulerypsghoéè\u{0}\u{7f}😀"
        .chars()
        .collect();
    for _ in 0..5_000 {
        let len = (next() % 200) as usize;
        let s: String = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        let _ = decode_client(&s);
        let _ = decode_server(&s);
    }
    let valid = hello("Alice");
    for cut in 0..valid.len() {
        if valid.is_char_boundary(cut) {
            let _ = decode_client(&valid[..cut]);
        }
    }
}

#[test]
fn unknown_fields_are_ignored_and_unit_variants_need_only_a_type() {
    assert_eq!(
        decode_client(r#"{"type":"ping","extra":[1,2,3]}"#).unwrap(),
        ClientMsg::Ping
    );
    assert_eq!(
        decode_client(r#"{"type":"leave_room"}"#).unwrap(),
        ClientMsg::LeaveRoom
    );
    assert_eq!(
        decode_client(r#"{"type":"start_match"}"#).unwrap(),
        ClientMsg::StartMatch
    );
}

#[test]
fn omitted_config_numbers_take_their_defaults() {
    let ClientMsg::CreateRoom { config } = decode_client(
        r#"{"type":"create_room","config":{"mode":"cooperation","difficulty":"difficile"}}"#,
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(
        config,
        RoomConfig::new(Mode::Cooperation, Difficulty::Difficile)
    );
    assert_eq!(
        (
            config.game,
            config.rounds,
            config.max_players,
            config.round_time_s,
            config.turn_time_s,
            config.question_time_s
        ),
        (GameKind::Devinette, 5, 8, 120, 45, 15)
    );
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn a_config_without_game_is_a_devinette_room_and_devinette_bytes_are_unchanged() {
    // A protocol-1 room config (no `game`, no `question_time_s`) still parses, as Devinette.
    let old = r#"{"mode":"course","difficulty":"facile","rounds":3,"max_players":4,"round_time_s":90,"turn_time_s":45}"#;
    let config: RoomConfig = serde_json::from_str(old).unwrap();
    assert_eq!(config.game, GameKind::Devinette);
    assert_eq!(config.question_time_s, 15);
    assert_eq!(
        config,
        RoomConfig {
            rounds: 3,
            max_players: 4,
            round_time_s: 90,
            ..RoomConfig::new(Mode::Course, Difficulty::Facile)
        }
    );
    // A default Devinette config serialises exactly as protocol 1 did (no new keys).
    let json = serde_json::to_string(&RoomConfig::new(Mode::Course, Difficulty::Facile)).unwrap();
    assert_eq!(
        json,
        r#"{"mode":"course","difficulty":"facile","rounds":5,"max_players":8,"round_time_s":120,"turn_time_s":45}"#
    );
    // A Flèches config carries both new keys.
    let json = serde_json::to_string(&RoomConfig {
        game: GameKind::Fleches,
        question_time_s: 30,
        ..RoomConfig::new(Mode::Course, Difficulty::Difficile)
    })
    .unwrap();
    assert!(json.contains("\"game\":\"fleches\""));
    assert!(json.contains("\"question_time_s\":30"));
    assert!(
        serde_json::from_str::<RoomConfig>(&json)
            .unwrap()
            .validate()
            .is_ok()
    );
}

#[test]
fn config_ranges_are_inclusive() {
    let ok = RoomConfig::new(Mode::Course, Difficulty::Facile);
    let with = |f: fn(&mut RoomConfig)| {
        let mut c = ok;
        f(&mut c);
        c.validate()
    };
    assert_eq!(with(|c| c.rounds = 1), Ok(()));
    assert_eq!(with(|c| c.rounds = 10), Ok(()));
    assert_eq!(with(|c| c.rounds = 0), Err(ErrorCode::InvalidConfig));
    assert_eq!(with(|c| c.rounds = 11), Err(ErrorCode::InvalidConfig));
    assert_eq!(with(|c| c.max_players = 2), Ok(()));
    assert_eq!(with(|c| c.max_players = 8), Ok(()));
    assert_eq!(with(|c| c.max_players = 1), Err(ErrorCode::InvalidConfig));
    assert_eq!(with(|c| c.max_players = 9), Err(ErrorCode::InvalidConfig));
    assert_eq!(with(|c| c.round_time_s = 60), Ok(()));
    assert_eq!(with(|c| c.round_time_s = 300), Ok(()));
    assert_eq!(with(|c| c.round_time_s = 59), Err(ErrorCode::InvalidConfig));
    assert_eq!(
        with(|c| c.round_time_s = 301),
        Err(ErrorCode::InvalidConfig)
    );
    assert_eq!(with(|c| c.turn_time_s = 15), Ok(()));
    assert_eq!(with(|c| c.turn_time_s = 120), Ok(()));
    assert_eq!(with(|c| c.turn_time_s = 14), Err(ErrorCode::InvalidConfig));
    assert_eq!(with(|c| c.turn_time_s = 121), Err(ErrorCode::InvalidConfig));
    assert_eq!(with(|c| c.question_time_s = 5), Ok(()));
    assert_eq!(with(|c| c.question_time_s = 60), Ok(()));
    assert_eq!(
        with(|c| c.question_time_s = 4),
        Err(ErrorCode::InvalidConfig)
    );
    assert_eq!(
        with(|c| c.question_time_s = 61),
        Err(ErrorCode::InvalidConfig)
    );
    // The game does not loosen the checks: every number is validated whatever the game.
    let mut fleches = RoomConfig::new(Mode::Cooperation, Difficulty::Difficile);
    fleches.game = GameKind::Fleches;
    assert_eq!(fleches.validate(), Ok(()));
    fleches.turn_time_s = 14;
    assert_eq!(fleches.validate(), Err(ErrorCode::InvalidConfig));
}

#[test]
fn nickname_rules() {
    assert_eq!(normalize_nickname("  Alice \t"), Some("Alice".into()));
    assert_eq!(normalize_nickname("Zoé 🚀"), Some("Zoé 🚀".into()));
    assert_eq!(normalize_nickname(""), None);
    assert_eq!(normalize_nickname(" \n "), None);
    assert_eq!(normalize_nickname("a\nb"), None);
    assert_eq!(normalize_nickname("a\u{7}b"), None);
    assert_eq!(normalize_nickname("a\u{0}"), None);
    // 16 scalar values pass (even when they take far more than 16 bytes), 17 do not.
    assert!(normalize_nickname(&"é".repeat(16)).is_some());
    assert!(normalize_nickname(&"é".repeat(17)).is_none());
    assert!(normalize_nickname(&"😀".repeat(16)).is_some());
    assert_eq!(nickname_key("ÉLODIE"), nickname_key("élodie"));
}

#[test]
fn room_codes_are_canonicalised() {
    assert_eq!(ROOM_CODE_ALPHABET.len(), 32);
    assert!(!ROOM_CODE_ALPHABET.contains(['I', 'O', '0', '1']));
    assert_eq!(normalize_room_code(" k7qx2 "), Some("K7QX2".into()));
    for bad in ["", "K7QX", "K7QX22", "K7QXI", "K7QX0", "K7Q-2", "K7QXé"] {
        assert_eq!(normalize_room_code(bad), None, "{bad}");
    }
}

#[test]
fn every_error_code_has_a_distinct_french_message_and_a_snake_case_name() {
    let names: Vec<String> = ErrorCode::ALL
        .iter()
        .map(|c| {
            serde_json::to_string(c)
                .unwrap()
                .trim_matches('"')
                .to_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "bad_version",
            "dataset_mismatch",
            "bad_message",
            "invalid_state",
            "invalid_nickname",
            "nickname_taken",
            "room_not_found",
            "room_full",
            "match_in_progress",
            "not_host",
            "not_enough_players",
            "invalid_config",
            "not_your_turn",
            "unknown_city",
            "already_guessed",
            "invalid_option",
            "round_over",
            "rate_limited",
        ]
    );
    let mut texts: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.message_fr()).collect();
    assert!(texts.iter().all(|t| t.ends_with('.')));
    texts.sort_unstable();
    texts.dedup();
    assert_eq!(
        texts.len(),
        ErrorCode::ALL.len(),
        "messages must be distinct"
    );
    assert_eq!(
        ErrorCode::RoomFull.message_fr(),
        "Cette salle est complète."
    );
    // `ALL` is exhaustive: this match has no wildcard.
    for code in ErrorCode::ALL {
        match code {
            ErrorCode::BadVersion
            | ErrorCode::DatasetMismatch
            | ErrorCode::BadMessage
            | ErrorCode::InvalidState
            | ErrorCode::InvalidNickname
            | ErrorCode::NicknameTaken
            | ErrorCode::RoomNotFound
            | ErrorCode::RoomFull
            | ErrorCode::MatchInProgress
            | ErrorCode::NotHost
            | ErrorCode::NotEnoughPlayers
            | ErrorCode::InvalidConfig
            | ErrorCode::NotYourTurn
            | ErrorCode::UnknownCity
            | ErrorCode::AlreadyGuessed
            | ErrorCode::InvalidOption
            | ErrorCode::RoundOver
            | ErrorCode::RateLimited => {}
        }
    }
}

#[test]
fn error_frames_carry_the_french_message() {
    let json = encode(&ServerMsg::error(ErrorCode::RoomFull));
    assert_eq!(
        json,
        r#"{"type":"error","code":"room_full","message":"Cette salle est complète."}"#
    );
}

#[test]
fn fingerprint_is_a_strict_16_digit_lowercase_hex_string() {
    let ok = r#"{"type":"hello","protocol_version":1,"dataset_fingerprint":"0123456789abcdef","nickname":"A"}"#;
    assert!(decode_client(ok).is_ok());
    for bad in [
        "0123456789ABCDEF",
        "123",
        "0123456789abcdef0",
        "0x23456789abcdef",
    ] {
        let frame = ok.replace("0123456789abcdef", bad);
        assert!(decode_client(&frame).is_err(), "{bad}");
    }
}

#[test]
fn realistic_worst_case_server_frames_fit_the_client_cap() {
    // 8 players × 6 long city names, in a round_ended.
    let entry = || GuessEntry {
        city: "Châlons-en-Champagne".into(),
        distance_km: 1000,
    };
    let ended = RoundEnded {
        round_index: 10,
        total_rounds: 10,
        secret: Some("Châlons-en-Champagne".into()),
        found_by: Some(8),
        results: Some(
            (1..=8)
                .map(|id| PlayerRoundResult {
                    player_id: id,
                    guesses: (0..6).map(|_| entry()).collect(),
                    solved: false,
                    tries_used: 6,
                    points: 0,
                    total: 6990,
                })
                .collect(),
        ),
        fleches: None,
        next_round_in_ms: Some(8000),
    };
    let frame = encode(&ServerMsg::RoundEnded(ended));
    assert!(
        frame.len() < MAX_SERVER_FRAME_BYTES / 4,
        "{} bytes",
        frame.len()
    );

    // The Flèches worst case: a reveal for 8 players (long names live in players, not here,
    // but keep the check alongside).
    let reveal = FlechesReveal {
        correct: 3,
        distance_km: 1000,
        compass: "NO".into(),
        answers: (1..=8)
            .map(|id| FlechesAnswer {
                player_id: id,
                option: Some(3),
                correct: true,
                points: 150,
                total: 1500,
            })
            .collect(),
    };
    let frame = encode(&ServerMsg::RoundEnded(RoundEnded {
        round_index: 10,
        total_rounds: 10,
        secret: None,
        found_by: None,
        results: None,
        fleches: Some(reveal),
        next_round_in_ms: Some(8000),
    }));
    assert!(
        frame.len() < MAX_SERVER_FRAME_BYTES / 4,
        "{} bytes",
        frame.len()
    );
}
