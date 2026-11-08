//! End-to-end tests over real WebSockets against a server on an ephemeral port.
//! Timers are shortened with the `time_scale` test hook, never by sleeping minutes.

mod net;

use boussole_protocol::*;
use net::*;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::Message;

const CLOSE_POLICY: u16 = 1008;

fn hex32(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// ---- HTTP ----------------------------------------------------------------------------------

#[tokio::test]
async fn healthz_and_version() {
    let server = server(|_| {}).await;
    let addr = server.local_addr();

    assert_eq!(http_get(addr, "/healthz").await, (200, "ok".to_owned()));

    let (status, body) = http_get(addr, "/version").await;
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["server_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["protocol_version"], PROTOCOL_VERSION);
    assert_eq!(
        v["dataset_fingerprint"],
        format!("{:016x}", boussole_core::dataset_fingerprint())
    );

    assert_eq!(http_get(addr, "/nope").await.0, 404);
    server.shutdown().await;
}

// ---- handshake -----------------------------------------------------------------------------

#[tokio::test]
async fn handshake_welcomes_and_refuses_wrong_versions() {
    let server = server(|_| {}).await;
    let addr = server.local_addr();

    // Accepted: welcome with a 128-bit hex token and a fresh player id.
    let mut a = TestClient::raw(addr).await;
    let resumed = a.hello("Alice", None).await;
    assert!(!resumed);
    assert!(hex32(&a.token), "token {}", a.token);
    let b = TestClient::connect(addr, "Bob").await;
    assert_ne!(a.id, b.id);
    assert_ne!(a.token, b.token);

    // Wrong protocol version: error, then the server closes.
    let mut c = TestClient::raw(addr).await;
    c.send_text(r#"{"type":"hello","protocol_version":99,"dataset_fingerprint":"0000000000000000","nickname":"C"}"#)
        .await;
    c.expect_error(ErrorCode::BadVersion).await;
    c.closed().await;

    // Wrong dataset.
    let mut d = TestClient::raw(addr).await;
    d.send_text(format!(
        r#"{{"type":"hello","protocol_version":{PROTOCOL_VERSION},"dataset_fingerprint":"0000000000000000","nickname":"D"}}"#
    ))
    .await;
    d.expect_error(ErrorCode::DatasetMismatch).await;
    d.closed().await;

    // Anything but hello first: invalid_state, then closed. Ping alone is allowed.
    let mut e = TestClient::raw(addr).await;
    e.send(&ClientMsg::Ping).await;
    assert_eq!(e.recv().await, ServerMsg::Pong);
    e.send(&ClientMsg::StartMatch).await;
    e.expect_error(ErrorCode::InvalidState).await;
    e.closed().await;

    // A bad nickname keeps the socket; a corrected hello then works.
    let mut f = TestClient::raw(addr).await;
    f.send(&ClientMsg::hello("   ", None)).await;
    f.expect_error(ErrorCode::InvalidNickname).await;
    assert!(!f.hello("Fanny", None).await);

    server.shutdown().await;
}

#[tokio::test]
async fn create_join_and_start_with_the_usual_refusals() {
    let server = server(|_| {}).await;
    let addr = server.local_addr();
    let mut host = TestClient::connect(addr, "Hote").await;
    let mut guest = TestClient::connect(addr, "Invite").await;
    let mut twin = TestClient::connect(addr, "invite").await;

    let lobby = host
        .create(room_config(Mode::Course, Difficulty::Facile, 2))
        .await;
    assert_eq!(lobby.phase, RoomPhase::Lobby);
    assert_eq!(lobby.code.len(), 5);
    assert_eq!(lobby.players.len(), 1);
    assert!(lobby.players[0].host);

    // Alone: cannot start.
    host.send(&ClientMsg::StartMatch).await;
    host.expect_error(ErrorCode::NotEnoughPlayers).await;

    // Unknown room, then join (case-insensitive code), then the same nickname is refused.
    guest
        .send(&ClientMsg::JoinRoom {
            code: "ZZZZZ".into(),
        })
        .await;
    guest.expect_error(ErrorCode::RoomNotFound).await;
    let joined = guest.join(&lobby.code.to_lowercase()).await;
    assert_eq!(joined.players.len(), 2);
    twin.send(&ClientMsg::JoinRoom {
        code: lobby.code.clone(),
    })
    .await;
    twin.expect_error(ErrorCode::NicknameTaken).await;

    // The host also hears about the new player; a non-host cannot start.
    let seen = host.snapshot_where(|s| s.players.len() == 2).await;
    assert!(
        seen.players
            .iter()
            .any(|p| p.nickname == "Invite" && p.connected)
    );
    guest.send(&ClientMsg::StartMatch).await;
    guest.expect_error(ErrorCode::NotHost).await;

    host.send(&ClientMsg::StartMatch).await;
    for c in [&mut host, &mut guest] {
        let started = c
            .recv_where(|m| match m {
                ServerMsg::RoundStarted(r) => Some(r.clone()),
                _ => None,
            })
            .await;
        assert_eq!(
            (
                started.round_index,
                started.total_rounds,
                started.hints.len()
            ),
            (1, 2, 5)
        );
        assert!(
            started.hints.iter().all(|h| h.distance_km.is_some()),
            "facile shows km"
        );
    }
    server.shutdown().await;
}

// ---- full matches --------------------------------------------------------------------------

/// Everything the three clients saw must agree.
fn assert_same_view(played: &[Played; 3], rounds: usize) {
    for p in played {
        assert_eq!(p.rounds.len(), rounds);
    }
    for r in 0..rounds {
        assert_eq!(
            played[0].rounds[r], played[1].rounds[r],
            "round {r} differs"
        );
        assert_eq!(
            played[0].rounds[r], played[2].rounds[r],
            "round {r} differs"
        );
        assert_eq!(played[0].started[r].hints, played[1].started[r].hints);
    }
    assert_eq!(played[0].match_ended, played[1].match_ended);
    assert_eq!(played[0].match_ended, played[2].match_ended);
}

#[tokio::test]
async fn course_match_with_three_clients_and_two_rounds() {
    let server = server(fast(20)).await;
    let addr = server.local_addr();
    let mut clients = [
        TestClient::connect(addr, "Ada").await,
        TestClient::connect(addr, "Bob").await,
        TestClient::connect(addr, "Cleo").await,
    ];
    let code = clients[0]
        .create(room_config(Mode::Course, Difficulty::Difficile, 2))
        .await
        .code;
    clients[1].join(&code).await;
    clients[2].join(&code).await;
    clients[0].send(&ClientMsg::StartMatch).await;

    let [a, b, c] = &mut clients;
    let (pa, pb, pc) = tokio::join!(
        play_course(a, Difficulty::Difficile, 2, 11),
        play_course(b, Difficulty::Difficile, 2, 22),
        play_course(c, Difficulty::Difficile, 2, 33),
    );
    let played = [pa, pb, pc];
    assert_same_view(&played, 2);

    // Difficile: no hint carried a distance.
    assert!(
        played[0]
            .started
            .iter()
            .all(|r| r.hints.iter().all(|h| h.distance_km.is_none()))
    );
    // Different secret in the two rounds, and every player's result is consistent.
    assert_ne!(played[0].rounds[0].secret, played[0].rounds[1].secret);
    for (idx, p) in played.iter().enumerate() {
        for (r, ended) in p.rounds.iter().enumerate() {
            let me = clients[idx].id;
            let mine = ended
                .results
                .iter()
                .find(|x| x.player_id == me)
                .expect("own result");
            assert_eq!(usize::from(mine.tries_used), p.guesses[r].len());
            if mine.solved {
                let base = 100 * (7 - u32::from(mine.tries_used));
                assert!((base..=base + 100).contains(&mine.points), "{mine:?}");
            } else {
                assert_eq!(mine.points, 0);
            }
        }
    }
    // The bot's strategy pins the secret down, so someone scored and the ranking is sound.
    let ranking = &played[0].match_ended.ranking;
    assert_eq!(ranking.len(), 3);
    assert!(ranking[0].total > 0, "{ranking:?}");
    assert!(ranking.windows(2).all(|w| w[0].total >= w[1].total));
    assert_eq!(ranking[0].rank, 1);
    let last = played[0].rounds.last().unwrap();
    for entry in ranking {
        let total = last
            .results
            .iter()
            .find(|r| r.player_id == entry.player_id)
            .unwrap()
            .total;
        assert_eq!(entry.total, total, "ranking total = last round total");
    }

    // Back in the lobby with scores kept, ready for another match.
    let lobby = clients[0]
        .snapshot_where(|s| s.phase == RoomPhase::Lobby)
        .await;
    assert_eq!(lobby.players.len(), 3);
    server.shutdown().await;
}

#[tokio::test]
async fn coop_match_with_three_clients_and_two_rounds() {
    let server = server(fast(20)).await;
    let addr = server.local_addr();
    let mut clients = [
        TestClient::connect(addr, "Ada").await,
        TestClient::connect(addr, "Bob").await,
        TestClient::connect(addr, "Cleo").await,
    ];
    let code = clients[0]
        .create(room_config(Mode::Cooperation, Difficulty::Facile, 2))
        .await
        .code;
    clients[1].join(&code).await;
    clients[2].join(&code).await;
    clients[0].send(&ClientMsg::StartMatch).await;

    let [a, b, c] = &mut clients;
    let (pa, pb, pc) = tokio::join!(
        play_coop(a, Difficulty::Facile, 2, 1),
        play_coop(b, Difficulty::Facile, 2, 2),
        play_coop(c, Difficulty::Facile, 2, 3),
    );
    let played = [pa, pb, pc];
    assert_same_view(&played, 2);

    // Turn rotation: round r starts with player (r - 1) mod n, in join order.
    let ids: Vec<_> = clients.iter().map(|c| c.id).collect();
    for r in 0..2 {
        let Timer::Turn { player_id, .. } = played[0].started[r].timer else {
            panic!("coop rounds carry a turn timer");
        };
        assert_eq!(
            player_id,
            ids[r % 3],
            "round {} starts with the next player",
            r + 1
        );
    }
    // The team shares tries and results: every client saw the same broadcast guesses.
    assert_eq!(played[0].guesses, played[1].guesses);
    assert_eq!(played[0].guesses, played[2].guesses);
    for (r, ended) in played[0].rounds.iter().enumerate() {
        let tries = played[0].guesses[r].len();
        assert!(tries <= 6);
        let solved = played[0].guesses[r].last().is_some_and(|g| g.correct);
        let team_points = if solved { 100 * (7 - tries as u32) } else { 0 };
        assert!(
            ended.results.iter().all(|x| x.points == team_points),
            "{ended:?}"
        );
        assert_eq!(ended.found_by.is_some(), solved);
    }
    let ranking = &played[0].match_ended.ranking;
    assert_eq!(played[0].match_ended.mode, Mode::Cooperation);
    assert!(ranking.iter().all(|r| r.rank == 1), "one team, one rank");
    let team_total: u32 = played[0].rounds.iter().map(|e| e.results[0].points).sum();
    assert!(ranking.iter().all(|r| r.total == team_total));
    assert!(team_total > 0, "a team with feedback finds the city");
    server.shutdown().await;
}

#[tokio::test]
async fn coop_turn_timeout_passes_the_turn_without_using_a_try() {
    // 15 s turn = 150 ms real time.
    let server = server(fast(100)).await;
    let addr = server.local_addr();
    let mut host = TestClient::connect(addr, "Hote").await;
    let mut guest = TestClient::connect(addr, "Invite").await;
    let code = host
        .create(RoomConfig {
            turn_time_s: 15,
            ..room_config(Mode::Cooperation, Difficulty::Facile, 1)
        })
        .await
        .code;
    guest.join(&code).await;
    host.send(&ClientMsg::StartMatch).await;

    // The host (first turn) stays silent.
    let started = guest
        .recv_where(|m| match m {
            ServerMsg::RoundStarted(r) => Some(r.clone()),
            _ => None,
        })
        .await;
    assert!(matches!(started.timer, Timer::Turn { player_id, .. } if player_id == host.id));
    let (to, reason) = guest
        .recv_where(|m| match m {
            ServerMsg::TurnChanged {
                player_id, reason, ..
            } => Some((*player_id, *reason)),
            _ => None,
        })
        .await;
    assert_eq!((to, reason), (guest.id, TurnReason::Timeout));

    // Now it is the guest's turn and the timeout cost nothing: this is try 1.
    guest
        .send(&ClientMsg::Guess {
            city: "Paris".into(),
        })
        .await;
    let g = guest
        .recv_where(|m| match m {
            ServerMsg::GuessResult(g) => Some(g.clone()),
            _ => None,
        })
        .await;
    assert_eq!((g.by, g.tries_used), (guest.id, 1));
    server.shutdown().await;
}

// ---- reconnection --------------------------------------------------------------------------

#[tokio::test]
async fn reconnecting_with_the_resume_token_restores_the_player() {
    let server = server(|_| {}).await;
    let addr = server.local_addr();
    let mut host = TestClient::connect(addr, "Hote").await;
    let mut guest = TestClient::connect(addr, "Invite").await;
    let code = host
        .create(room_config(Mode::Course, Difficulty::Facile, 1))
        .await
        .code;
    guest.join(&code).await;
    host.send(&ClientMsg::StartMatch).await;
    host.recv_where(|m| matches!(m, ServerMsg::RoundStarted(_)).then_some(()))
        .await;

    // The guest makes one guess, then loses the connection.
    guest
        .recv_where(|m| matches!(m, ServerMsg::RoundStarted(_)).then_some(()))
        .await;
    guest
        .send(&ClientMsg::Guess {
            city: "Lyon".into(),
        })
        .await;
    let first = guest
        .recv_where(|m| match m {
            ServerMsg::GuessResult(g) => Some(g.clone()),
            _ => None,
        })
        .await;
    let (id, token) = (guest.id, guest.token.clone());
    guest.vanish();

    let seen = host
        .snapshot_where(|s| {
            s.players
                .iter()
                .any(|p| p.nickname == "Invite" && !p.connected)
        })
        .await;
    assert_eq!(seen.phase, RoomPhase::RoundActive);

    // A new socket + hello with the token: same identity, state restored.
    let mut back = TestClient::raw(addr).await;
    let resumed = back.hello("ignored", Some(token.clone())).await;
    assert!(resumed);
    assert_eq!((back.id, back.token.as_str()), (id, token.as_str()));
    let snap = back.snapshot().await;
    assert_eq!(snap.code, code);
    let round = snap
        .round
        .expect("round state is part of the resume snapshot");
    assert_eq!(round.guesses, vec![first], "own guesses restored");
    assert!(snap.players.iter().all(|p| p.connected));
    host.snapshot_where(|s| s.players.iter().all(|p| p.connected))
        .await;

    // An unknown token is a fresh session, not an error.
    let mut stranger = TestClient::raw(addr).await;
    assert!(!stranger.hello("Stranger", Some("0".repeat(32))).await);
    server.shutdown().await;
}

#[tokio::test]
async fn resume_window_is_60_game_seconds_then_the_slot_is_released() {
    // 60 s of game time = 300 ms at scale 200.
    let server = server(fast(200)).await;
    let addr = server.local_addr();
    let mut host = TestClient::connect(addr, "Hote").await;
    let mut guest = TestClient::connect(addr, "Invite").await;
    let code = host
        .create(room_config(Mode::Course, Difficulty::Facile, 1))
        .await
        .code;
    guest.join(&code).await;
    let token = guest.token.clone();
    guest.vanish();

    host.snapshot_where(|s| s.players.iter().any(|p| !p.connected))
        .await;
    // After the window the player is removed as if they had left.
    let after = host.snapshot_where(|s| s.players.len() == 1).await;
    assert_eq!(after.players[0].nickname, "Hote");

    let mut late = TestClient::raw(addr).await;
    assert!(
        !late.hello("Invite", Some(token)).await,
        "expired token = new player"
    );
    server.shutdown().await;
}

// ---- hostile and sloppy input --------------------------------------------------------------

#[tokio::test]
async fn oversize_frames_get_bad_message_and_a_policy_close() {
    let server = server(|_| {}).await;
    let addr = server.local_addr();

    // Just over the limit: answered, then closed with 1008.
    let mut c = TestClient::connect(addr, "Big").await;
    c.send_text(" ".repeat(MAX_FRAME_BYTES + 1)).await;
    c.expect_error(ErrorCode::BadMessage).await;
    assert_eq!(c.closed().await, Some(CLOSE_POLICY));

    // Exactly the limit is still fine (and is just a malformed message here).
    let mut ok = TestClient::connect(addr, "Edge").await;
    ok.send_text(" ".repeat(MAX_FRAME_BYTES)).await;
    ok.expect_error(ErrorCode::BadMessage).await;
    ok.send(&ClientMsg::Ping).await;
    assert_eq!(ok.recv().await, ServerMsg::Pong);

    // Absurdly large: the connection ends (the transport refuses to buffer it).
    let mut huge = TestClient::connect(addr, "Huge").await;
    // The server may hang up before we finish writing, so a failed send is fine here.
    let _ = huge.try_send_ws(Message::text("x".repeat(1 << 20))).await;
    let _ = huge.closed().await;

    assert_eq!(http_get(addr, "/healthz").await.0, 200);
    server.shutdown().await;
}

#[tokio::test]
async fn malformed_json_and_binary_frames_do_not_kill_the_connection() {
    let server = server(|_| {}).await;
    let addr = server.local_addr();
    let mut c = TestClient::connect(addr, "Sloppy").await;

    for junk in [
        "not json at all",
        "{",
        r#"{"type":"nope"}"#,
        r#"{"type":"guess"}"#,
        r#"{"type":"guess","city":42}"#,
        "[]",
        "null",
        "",
    ] {
        c.send_text(junk).await;
        c.expect_error(ErrorCode::BadMessage).await;
    }
    c.send_ws(Message::Binary(vec![0, 1, 2, 255].into())).await;
    c.expect_error(ErrorCode::BadMessage).await;

    // Still alive and able to play.
    c.send(&ClientMsg::Ping).await;
    assert_eq!(c.recv().await, ServerMsg::Pong);
    let snap = c
        .create(room_config(Mode::Course, Difficulty::Facile, 1))
        .await;
    assert_eq!(snap.players.len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn flooding_gets_rate_limited_then_dropped() {
    let server = server(|_| {}).await;
    let addr = server.local_addr();

    // A burst within the allowance is served in full.
    let mut bursty = TestClient::connect(addr, "Bursty").await;
    for _ in 0..15 {
        bursty.send(&ClientMsg::Ping).await;
    }
    for _ in 0..15 {
        assert_eq!(bursty.recv().await, ServerMsg::Pong);
    }

    // A flood is refused with `rate_limited`, and sustained abuse ends in a policy close.
    let mut flooder = TestClient::connect(addr, "Flood").await;
    for _ in 0..300 {
        flooder.send(&ClientMsg::Ping).await;
    }
    let mut limited = 0;
    let code = loop {
        match flooder.next().await {
            Frame::Msg(ServerMsg::Pong) => {}
            Frame::Msg(ServerMsg::Error {
                code: ErrorCode::RateLimited,
                message,
            }) => {
                assert_eq!(message, ErrorCode::RateLimited.message_fr());
                limited += 1;
            }
            Frame::Msg(other) => panic!("unexpected {other:?}"),
            Frame::Closed(code) => break code,
        }
    };
    assert_eq!(
        code,
        Some(CLOSE_POLICY),
        "sustained abuse ends in a policy close"
    );
    assert!(limited > 0, "refused messages were announced");

    // A polite client at the sustained rate is never limited, and others are unaffected.
    let mut polite = TestClient::connect(addr, "Polite").await;
    for _ in 0..25 {
        polite.send(&ClientMsg::Ping).await;
        assert_eq!(polite.recv().await, ServerMsg::Pong);
        sleep(Duration::from_millis(105)).await;
    }
    server.shutdown().await;
}

// ---- caps and timeouts ---------------------------------------------------------------------

#[tokio::test]
async fn connection_cap_refuses_extra_websockets_with_503_and_frees_slots() {
    let server = server(|c| c.max_connections = 2).await;
    let addr = server.local_addr();
    let a = TestClient::raw(addr).await;
    let _b = TestClient::raw(addr).await;

    let err = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect_err("third connection must be refused");
    assert!(err.to_string().contains("503"), "{err}");
    assert_eq!(
        http_get(addr, "/healthz").await.0,
        200,
        "plain HTTP is not capped"
    );

    a.vanish();
    let mut admitted = false;
    for _ in 0..50 {
        if tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
            .await
            .is_ok()
        {
            admitted = true;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert!(admitted, "a freed slot admits a new connection");
    server.shutdown().await;
}

#[tokio::test]
async fn room_cap_refuses_create_room_but_not_joining() {
    let server = server(|c| c.max_rooms = 1).await;
    let addr = server.local_addr();
    let mut a = TestClient::connect(addr, "A").await;
    let mut b = TestClient::connect(addr, "B").await;
    let code = a
        .create(room_config(Mode::Course, Difficulty::Facile, 1))
        .await
        .code;

    b.send(&ClientMsg::CreateRoom {
        config: room_config(Mode::Course, Difficulty::Facile, 1),
    })
    .await;
    b.expect_error(ErrorCode::InvalidState).await;
    assert_eq!(
        b.join(&code).await.players.len(),
        2,
        "the existing room is still joinable"
    );

    // Freeing the room makes room for another one.
    a.send(&ClientMsg::LeaveRoom).await;
    b.send(&ClientMsg::LeaveRoom).await;
    sleep(Duration::from_millis(100)).await;
    a.send(&ClientMsg::CreateRoom {
        config: room_config(Mode::Course, Difficulty::Facile, 1),
    })
    .await;
    // Skip snapshots of the old room that are still in flight.
    let snap = a.snapshot_where(|s| s.code != code).await;
    assert_eq!(snap.players.len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn sockets_that_never_say_hello_are_closed() {
    let server = server(|c| c.handshake_timeout = Duration::from_millis(300)).await;
    let addr = server.local_addr();
    let mut silent = TestClient::raw(addr).await;
    assert_eq!(silent.closed().await, Some(CLOSE_POLICY));

    // Saying hello in time disarms the timeout.
    let mut chatty = TestClient::connect(addr, "Quick").await;
    sleep(Duration::from_millis(600)).await;
    chatty.send(&ClientMsg::Ping).await;
    assert_eq!(chatty.recv().await, ServerMsg::Pong);
    server.shutdown().await;
}

#[tokio::test]
async fn websocket_pings_keep_live_clients_and_idle_ones_are_dropped_but_resumable() {
    let server = server(|c| {
        c.ping_interval = Duration::from_millis(100);
        c.idle_timeout = Duration::from_millis(500);
    })
    .await;
    let addr = server.local_addr();
    let mut live = TestClient::connect(addr, "Live").await;
    let mut idle = TestClient::connect(addr, "Idle").await;
    let code = live
        .create(room_config(Mode::Course, Difficulty::Facile, 1))
        .await
        .code;
    idle.join(&code).await;
    let token = idle.token.clone();

    // `live` keeps reading (so its WebSocket layer answers the server's pings) but sends
    // nothing; `idle` stops touching its socket altogether.
    let dropped = live
        .snapshot_where(|s| {
            s.players
                .iter()
                .any(|p| p.nickname == "Idle" && !p.connected)
        })
        .await;
    assert!(
        dropped
            .players
            .iter()
            .any(|p| p.nickname == "Live" && p.connected)
    );
    // Silence of 2 s is 4 idle timeouts: `live` is still there because it answers pings.
    assert!(timeout(Duration::from_secs(2), live.next()).await.is_err());
    live.send(&ClientMsg::Ping).await;
    assert_eq!(live.recv().await, ServerMsg::Pong);

    // The dropped player's slot is held: they come back with their token.
    let mut back = TestClient::raw(addr).await;
    assert!(back.hello("x", Some(token)).await);
    assert_eq!(back.snapshot().await.players.len(), 2);
    drop(idle);
    server.shutdown().await;
}

// ---- hostile input, fuzz-ish ---------------------------------------------------------------

/// Tiny xorshift: reproducible junk without a dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn junk(rng: &mut Rng) -> Message {
    const VALID: &[&str] = &[
        r#"{"type":"create_room","config":{"mode":"course","difficulty":"facile","rounds":1}}"#,
        r#"{"type":"create_room","config":{"mode":"cooperation","difficulty":"difficile","rounds":2,"turn_time_s":15}}"#,
        r#"{"type":"join_room","code":"ABCDE"}"#,
        r#"{"type":"leave_room"}"#,
        r#"{"type":"update_config","config":{"mode":"course","difficulty":"facile"}}"#,
        r#"{"type":"start_match"}"#,
        r#"{"type":"guess","city":"Paris"}"#,
        r#"{"type":"guess","city":"Lyon"}"#,
        r#"{"type":"ping"}"#,
        r#"{"type":"hello","protocol_version":1,"dataset_fingerprint":"zz","nickname":"x"}"#,
    ];
    match rng.below(6) {
        // Random bytes, binary.
        0 => Message::Binary((0..rng.below(300)).map(|_| rng.next() as u8).collect::<Vec<_>>().into()),
        // Random printable text.
        1 => Message::text((0..rng.below(200)).map(|_| char::from(32 + rng.below(95) as u8)).collect::<String>()),
        // A valid message with a byte flipped or the tail cut off.
        2 => {
            let mut s = VALID[rng.below(VALID.len())].to_owned().into_bytes();
            if rng.below(2) == 0 && !s.is_empty() {
                let i = rng.below(s.len());
                s[i] = 32 + rng.below(95) as u8;
            } else {
                s.truncate(rng.below(s.len() + 1));
            }
            Message::text(String::from_utf8_lossy(&s).into_owned())
        }
        // Valid messages, in any order.
        3 | 4 => Message::text(VALID[rng.below(VALID.len())]),
        // Deep nesting, huge numbers, odd unicode.
        _ => Message::text(match rng.below(4) {
            0 => format!("{}{}", "[".repeat(5000), "]".repeat(5000)),
            1 => r#"{"type":"create_room","config":{"mode":"course","difficulty":"facile","rounds":99999999999999999999}}"#.to_owned(),
            2 => r#"{"type":"guess","city":"\u0000\ud800\u202e☃"}"#.to_owned(),
            _ => r#"{"type":"join_room","code":"😀😀😀"}"#.to_owned(),
        }),
    }
}

#[tokio::test]
async fn hostile_input_for_a_few_seconds_never_takes_the_server_down() {
    // Rate limits off for this test: we want the engine and the transport to see everything.
    // A panic inside server code would only kill one connection task; make it visible.
    static SERVER_PANICS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if info
            .location()
            .is_some_and(|l| l.file().replace('\\', "/").contains("boussole-server/src/"))
        {
            SERVER_PANICS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        previous(info);
    }));

    let server = server(|c| {
        c.msg_rate_per_s = 1_000_000;
        c.msg_burst = 1_000_000;
        c.time_scale = 50;
    })
    .await;
    let addr = server.local_addr();
    let run_for = Duration::from_secs(4);
    let started = std::time::Instant::now();

    let mut workers = Vec::new();
    for worker in 0..6u64 {
        workers.push(tokio::spawn(async move {
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ ((worker + 1) * 0x1234_5679));
            let mut sent = 0u64;
            while started.elapsed() < run_for {
                let mut c = TestClient::raw(addr).await;
                // Half the connections shake hands properly first so the engine's room logic
                // is exercised too.
                if rng.below(2) == 0 {
                    c.send(&ClientMsg::hello(
                        format!("fuzz{worker}-{}", rng.below(4)),
                        None,
                    ))
                    .await;
                }
                for _ in 0..rng.below(80) + 1 {
                    let msg = junk(&mut rng);
                    if !c.try_send_ws(msg).await {
                        break;
                    }
                    sent += 1;
                    // Read whatever came back so server writes never back up.
                    while let Ok(f) = timeout(Duration::from_millis(1), c.next()).await {
                        if matches!(f, Frame::Closed(_)) {
                            break;
                        }
                    }
                }
            }
            sent
        }));
    }
    let mut total = 0;
    for w in workers {
        total += w.await.expect("fuzz worker did not panic");
    }
    assert!(
        total > 500,
        "the fuzzers really sent traffic ({total} frames)"
    );

    // The server is still up and still plays.
    assert_eq!(http_get(addr, "/healthz").await, (200, "ok".to_owned()));
    let mut a = TestClient::connect(addr, "After1").await;
    let mut b = TestClient::connect(addr, "After2").await;
    let code = a
        .create(room_config(Mode::Course, Difficulty::Facile, 1))
        .await
        .code;
    assert_eq!(b.join(&code).await.players.len(), 2);
    a.send(&ClientMsg::StartMatch).await;
    a.recv_where(|m| matches!(m, ServerMsg::RoundStarted(_)).then_some(()))
        .await;
    server.shutdown().await;
    assert_eq!(
        SERVER_PANICS.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no panic inside the server under hostile input"
    );
}
