//! Golden JSON fixtures: one file per message shape in `tests/fixtures/{client,server}/`.
//! The files are the wire format; `docs/protocol.md` quotes them verbatim (checked below).
//! Regenerate after an intentional change with `BLESS=1 cargo test -p boussole-protocol`.

mod samples;

use boussole_protocol::*;
use samples::{client_samples, server_samples};
use std::fs;
use std::path::PathBuf;

fn fixture_path(side: &str, name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(side)
        .join(format!("{name}.json"))
}

fn bless() -> bool {
    std::env::var_os("BLESS").is_some()
}

fn read_fixture(side: &str, name: &str, current: &str) -> String {
    if bless() {
        fs::write(fixture_path(side, name), format!("{current}\n")).unwrap();
    }
    read_golden(side, name)
}

fn read_golden(side: &str, name: &str) -> String {
    let path = fixture_path(side, name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing fixture {} ({e}); run with BLESS=1", path.display()))
        .trim_end()
        .to_owned()
}

#[test]
fn every_client_message_matches_its_golden_fixture() {
    for (name, msg) in client_samples() {
        let golden = read_fixture("client", name, &encode_client(&msg));
        assert_eq!(
            encode_client(&msg),
            golden,
            "client/{name}: encoding drifted"
        );
        assert_eq!(
            decode_client(&golden).unwrap(),
            msg,
            "client/{name}: decoding"
        );
        assert!(golden.len() <= MAX_FRAME_BYTES);
    }
}

#[test]
fn every_server_message_matches_its_golden_fixture() {
    for (name, msg) in server_samples() {
        let golden = read_fixture("server", name, &encode(&msg));
        assert_eq!(encode(&msg), golden, "server/{name}: encoding drifted");
        assert_eq!(
            decode_server(&golden).unwrap(),
            msg,
            "server/{name}: decoding"
        );
    }
}

/// Exhaustive on purpose: adding a variant breaks the build here until it gets a fixture.
fn client_type(msg: &ClientMsg) -> &'static str {
    match msg {
        ClientMsg::Hello { .. } => "hello",
        ClientMsg::CreateRoom { .. } => "create_room",
        ClientMsg::JoinRoom { .. } => "join_room",
        ClientMsg::LeaveRoom => "leave_room",
        ClientMsg::UpdateConfig { .. } => "update_config",
        ClientMsg::StartMatch => "start_match",
        ClientMsg::Guess { .. } => "guess",
        ClientMsg::Ping => "ping",
    }
}

fn server_type(msg: &ServerMsg) -> &'static str {
    match msg {
        ServerMsg::Welcome { .. } => "welcome",
        ServerMsg::RoomSnapshot(_) => "room_snapshot",
        ServerMsg::RoundStarted(_) => "round_started",
        ServerMsg::GuessResult(_) => "guess_result",
        ServerMsg::PlayerProgress(_) => "player_progress",
        ServerMsg::TurnChanged { .. } => "turn_changed",
        ServerMsg::RoundEnded(_) => "round_ended",
        ServerMsg::MatchEnded(_) => "match_ended",
        ServerMsg::Error { .. } => "error",
        ServerMsg::Pong => "pong",
    }
}

#[test]
fn every_variant_has_a_fixture_and_the_tag_is_the_snake_case_name() {
    let mut seen: Vec<&str> = client_samples()
        .iter()
        .map(|(_, m)| client_type(m))
        .collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        seen,
        [
            "create_room",
            "guess",
            "hello",
            "join_room",
            "leave_room",
            "ping",
            "start_match",
            "update_config"
        ]
    );
    let mut seen: Vec<&str> = server_samples()
        .iter()
        .map(|(_, m)| server_type(m))
        .collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        seen,
        [
            "error",
            "guess_result",
            "match_ended",
            "player_progress",
            "pong",
            "room_snapshot",
            "round_ended",
            "round_started",
            "turn_changed",
            "welcome"
        ]
    );
    for (name, msg) in client_samples() {
        let json: serde_json::Value = serde_json::from_str(&encode_client(&msg)).unwrap();
        assert_eq!(json["type"], client_type(&msg), "client/{name}");
    }
    for (name, msg) in server_samples() {
        let json: serde_json::Value = serde_json::from_str(&encode(&msg)).unwrap();
        assert_eq!(json["type"], server_type(&msg), "server/{name}");
    }
}

#[test]
fn no_stale_fixture_files() {
    for (side, names) in [
        (
            "client",
            client_samples().iter().map(|s| s.0).collect::<Vec<_>>(),
        ),
        (
            "server",
            server_samples().iter().map(|s| s.0).collect::<Vec<_>>(),
        ),
    ] {
        let dir = fixture_path(side, "x").parent().unwrap().to_owned();
        for entry in fs::read_dir(dir).unwrap() {
            let file = entry.unwrap().file_name().into_string().unwrap();
            let stem = file.trim_end_matches(".json");
            assert!(names.contains(&stem), "{side}/{file} has no sample");
        }
    }
}

#[test]
fn protocol_doc_quotes_every_fixture_verbatim() {
    let doc = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/protocol.md"),
    )
    .expect("docs/protocol.md");
    for (side, names) in [
        (
            "client",
            client_samples().iter().map(|s| s.0).collect::<Vec<_>>(),
        ),
        (
            "server",
            server_samples().iter().map(|s| s.0).collect::<Vec<_>>(),
        ),
    ] {
        for name in names {
            let golden = read_golden(side, name);
            assert!(
                doc.contains(&golden),
                "docs/protocol.md does not quote {side}/{name}.json:\n{golden}"
            );
        }
    }
}

#[test]
fn difficile_hints_carry_no_distance_key_and_facile_hints_do() {
    let msgs: std::collections::BTreeMap<_, _> = server_samples().into_iter().collect();
    let facile = encode(&msgs["round_started_course_facile"]);
    let difficile = encode(&msgs["round_started_coop_difficile"]);
    assert!(facile.contains("\"distance_km\":403"));
    assert!(!difficile.contains("distance_km"), "{difficile}");
    // …including inside a snapshot, and on decoding a frame that omits the key.
    assert!(!encode(&msgs["room_snapshot_course_round"]).contains("\"distance_km\":403"));
    let hint: HintDto = serde_json::from_str(r#"{"city":"Lyon","bearing_deg":12.0}"#).unwrap();
    assert_eq!(hint.distance_km, None);
}
