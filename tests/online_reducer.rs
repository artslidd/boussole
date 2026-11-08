//! The online reducer: `apply(state, ServerMsg, now) -> state`, tested without any socket by
//! feeding it the protocol's golden fixtures (the exact JSON the server sends) and hand-built
//! sequences for whole Course and Coopération matches.

use boussole::online::{Clock, Link, NoticeKind, OnlineState, apply, set_link};
use boussole_protocol::{
    ErrorCode, GuessInfo, Mode, PlayerInfo, Progress, RoomPhase, RoomSnapshot, ServerMsg,
    TurnReason, decode_server,
};
use std::time::{Duration, Instant};

macro_rules! fixture {
    ($name:literal) => {
        decode_server(include_str!(concat!(
            "../crates/boussole-protocol/tests/fixtures/server/",
            $name,
            ".json"
        )))
        .expect(concat!("fixture ", $name, " decodes"))
    };
}

/// A scripted clock: `t(0)` is the test's origin, `t(500)` 500 ms later.
struct Clockwork(Instant);

impl Clockwork {
    fn new() -> Self {
        Self(Instant::now())
    }
    fn t(&self, ms: u64) -> Instant {
        self.0 + Duration::from_millis(ms)
    }
}

fn player(id: u32, name: &str, host: bool) -> PlayerInfo {
    PlayerInfo {
        id,
        nickname: name.to_owned(),
        host,
        connected: true,
        score: 0,
    }
}

fn snapshot(msg: ServerMsg) -> RoomSnapshot {
    match msg {
        ServerMsg::RoomSnapshot(s) => s,
        other => panic!("not a snapshot: {other:?}"),
    }
}

fn welcomed(c: &Clockwork) -> OnlineState {
    apply(OnlineState::new(), fixture!("welcome"), c.t(0))
}

// --- welcome and links ----------------------------------------------------------------------

#[test]
fn welcome_identifies_me_and_marks_the_link_online() {
    let c = Clockwork::new();
    let state = welcomed(&c);
    assert_eq!(state.me, Some(1));
    assert_eq!(state.link, Link::Online);
    assert!(state.room.is_none());
}

#[test]
fn an_unresumed_welcome_while_in_a_room_means_the_room_is_gone() {
    let c = Clockwork::new();
    let state = apply(welcomed(&c), fixture!("room_snapshot_lobby"), c.t(1));
    assert!(state.room.is_some());
    let state = set_link(
        state,
        Link::Reconnecting {
            give_up_at: c.t(60_010),
            attempt: 2,
        },
    );
    // `welcome.resumed == false`: the server forgot us (grace expired, server restarted)
    let state = apply(state, fixture!("welcome"), c.t(20));
    assert!(state.room.is_none(), "back to the menu");
    assert_eq!(state.link, Link::Online);
    let notice = state.notice.expect("the player is told why");
    assert_eq!(notice.kind, NoticeKind::Error);
    assert!(notice.text.contains("partie"), "{}", notice.text);
}

#[test]
fn a_resumed_welcome_keeps_the_room_until_the_snapshot_replaces_it() {
    let c = Clockwork::new();
    let state = apply(welcomed(&c), fixture!("room_snapshot_lobby"), c.t(1));
    let state = set_link(
        state,
        Link::Reconnecting {
            give_up_at: c.t(60_010),
            attempt: 1,
        },
    );
    let resumed = ServerMsg::Welcome {
        player_id: 1,
        resume_token: "3f9a1c5e7b2d4086a1b3c5d7e9f01234".into(),
        resumed: true,
    };
    let state = apply(state, resumed, c.t(20));
    assert_eq!(state.link, Link::Online);
    assert!(state.room.is_some());
    assert!(state.notice.is_none());
}

// --- snapshots --------------------------------------------------------------------------------

#[test]
fn a_snapshot_creates_the_room_and_the_next_one_replaces_it() {
    let c = Clockwork::new();
    let state = apply(welcomed(&c), fixture!("room_snapshot_lobby"), c.t(1));
    let room = state.room.as_ref().expect("room");
    assert_eq!(room.code, "K7QX2");
    assert_eq!(room.phase, RoomPhase::Lobby);
    assert_eq!(room.players.len(), 2);
    assert!(room.players[0].host && room.players[0].nickname == "Alice");
    assert!(!room.players[1].connected, "Bob's slot is held");

    // a third player joins: the snapshot replaces the list wholesale
    let mut next = snapshot(fixture!("room_snapshot_lobby"));
    next.players.push(player(3, "Camille", false));
    next.players[1].connected = true;
    let state = apply(state, ServerMsg::RoomSnapshot(next), c.t(2));
    let room = state.room.as_ref().unwrap();
    assert_eq!(room.players.len(), 3);
    assert!(room.players[1].connected);
}

#[test]
fn a_snapshot_for_another_room_replaces_everything_including_the_round() {
    let c = Clockwork::new();
    let state = apply(welcomed(&c), fixture!("room_snapshot_course_round"), c.t(1));
    assert!(state.room.as_ref().unwrap().round.is_some());
    let mut other = snapshot(fixture!("room_snapshot_lobby"));
    other.code = "ABCDE".into();
    let state = apply(state, ServerMsg::RoomSnapshot(other), c.t(2));
    let room = state.room.unwrap();
    assert_eq!(room.code, "ABCDE");
    assert!(room.round.is_none());
}

#[test]
fn a_course_snapshot_restores_my_guesses_everybodys_progress_and_the_countdown() {
    let c = Clockwork::new();
    let state = apply(
        welcomed(&c),
        fixture!("room_snapshot_course_round"),
        c.t(1_000),
    );
    let room = state.room.as_ref().unwrap();
    assert_eq!(room.phase, RoomPhase::RoundActive);
    let round = room.round.as_ref().unwrap();
    assert_eq!(round.index, 1);
    assert_eq!(round.total, 3);
    assert_eq!(round.guesses.len(), 2);
    assert_eq!(round.guesses[0].city, "Lyon");
    assert_eq!(round.progress.len(), 2);
    assert_eq!(round.progress[1].tries_used, 1);
    // timer: 71 250 ms remaining when the snapshot arrived at t = 1 s
    assert_eq!(
        round.clock.remaining(c.t(1_000)),
        Duration::from_millis(71_250)
    );
    assert_eq!(
        round.clock.remaining(c.t(31_000)),
        Duration::from_millis(41_250)
    );
    assert_eq!(
        round.clock.remaining(c.t(500_000)),
        Duration::ZERO,
        "never negative"
    );
    assert!(matches!(round.clock, Clock::Round { .. }));
}

#[test]
fn a_coop_snapshot_restores_the_team_guesses_and_whose_turn_it_is() {
    let c = Clockwork::new();
    let state = apply(welcomed(&c), fixture!("room_snapshot_coop_round"), c.t(0));
    let round = state.room.as_ref().unwrap().round.as_ref().unwrap();
    assert_eq!(round.guesses.len(), 1);
    assert_eq!(round.guesses[0].by, 1);
    match round.clock {
        Clock::Turn { player, .. } => assert_eq!(player, 2),
        other => panic!("expected a turn clock, got {other:?}"),
    }
    assert_eq!(round.clock.remaining(c.t(0)), Duration::from_millis(31_000));
}

#[test]
fn an_intermission_snapshot_shows_the_last_result_and_the_countdown_to_the_next_round() {
    let c = Clockwork::new();
    let state = apply(welcomed(&c), fixture!("room_snapshot_intermission"), c.t(0));
    let room = state.room.as_ref().unwrap();
    assert_eq!(room.phase, RoomPhase::Intermission);
    assert!(room.round.is_none());
    let result = room.result.as_ref().expect("last round result");
    assert_eq!(result.ended.secret, "Besançon");
    assert_eq!(
        result.next_at.map(|at| at.duration_since(c.t(0))),
        Some(Duration::from_millis(5_500))
    );
}

// --- Course -------------------------------------------------------------------------------------

#[test]
fn round_started_in_facile_keeps_the_hint_distances_and_in_difficile_has_none() {
    let c = Clockwork::new();
    let lobby = apply(welcomed(&c), fixture!("room_snapshot_lobby"), c.t(0));
    let facile = apply(
        lobby.clone(),
        fixture!("round_started_course_facile"),
        c.t(1),
    );
    let round = facile.room.as_ref().unwrap().round.as_ref().unwrap();
    assert_eq!(round.hints.len(), 5);
    assert!(round.hints.iter().all(|h| h.distance_km.is_some()));
    assert_eq!(round.max_tries, 6);
    assert_eq!(round.clock.remaining(c.t(1)), Duration::from_millis(90_000));

    let difficile = apply(lobby, fixture!("round_started_coop_difficile"), c.t(1));
    let round = difficile.room.as_ref().unwrap().round.as_ref().unwrap();
    assert!(round.hints.iter().all(|h| h.distance_km.is_none()));
}

#[test]
fn a_whole_course_round_from_the_first_guess_to_the_intermission() {
    let c = Clockwork::new();
    let mut s = apply(welcomed(&c), fixture!("room_snapshot_lobby"), c.t(0));
    s = apply(s, fixture!("round_started_course_facile"), c.t(10));
    // The room is still `lobby` until the snapshot of the same step arrives
    s = apply(
        s,
        ServerMsg::RoomSnapshot(course_round_snapshot_empty()),
        c.t(10),
    );

    // My first guess: private feedback to me, and my own progress follows from it
    s = apply(s, fixture!("guess_result"), c.t(2_000));
    let round = s.room.as_ref().unwrap().round.as_ref().unwrap();
    assert_eq!(round.guesses.len(), 1);
    let mine = round.progress.iter().find(|p| p.player_id == 1).unwrap();
    assert_eq!((mine.tries_used, mine.solved), (1, false));

    // Bob's progress: counts only
    s = apply(
        s,
        ServerMsg::PlayerProgress(Progress {
            player_id: 2,
            tries_used: 2,
            solved: false,
            out_of_tries: false,
        }),
        c.t(3_000),
    );
    let round = s.room.as_ref().unwrap().round.as_ref().unwrap();
    assert_eq!(
        round
            .progress
            .iter()
            .find(|p| p.player_id == 2)
            .unwrap()
            .tries_used,
        2
    );
    assert_eq!(
        round.guesses.len(),
        1,
        "Bob's guesses are never known before the end"
    );

    // I find it: my progress shows solved
    s = apply(
        s,
        ServerMsg::GuessResult(GuessInfo {
            by: 1,
            city: "Besançon".into(),
            distance_km: 0,
            bearing_deg: 0.0,
            correct: true,
            tries_used: 2,
        }),
        c.t(5_000),
    );
    let round = s.room.as_ref().unwrap().round.as_ref().unwrap();
    let mine = round.progress.iter().find(|p| p.player_id == 1).unwrap();
    assert!(mine.solved);

    // Round end reveals everything and starts the 8 s intermission
    s = apply(s, fixture!("round_ended_course"), c.t(6_000));
    let room = s.room.as_ref().unwrap();
    let result = room.result.as_ref().unwrap();
    assert_eq!(result.ended.secret, "Besançon");
    assert_eq!(
        result.hints.len(),
        5,
        "the board stays up behind the result"
    );
    assert_eq!(result.guesses.len(), 2, "…with my guesses");
    assert_eq!(
        result.next_at.map(|at| at.duration_since(c.t(6_000))),
        Some(Duration::from_millis(8_000))
    );
    s = apply(s, fixture!("room_snapshot_intermission"), c.t(6_000));
    let room = s.room.as_ref().unwrap();
    assert_eq!(room.phase, RoomPhase::Intermission);
    assert!(
        room.result.as_ref().unwrap().hints.len() == 5,
        "hints survive the snapshot"
    );

    // Next round clears the result
    s = apply(s, fixture!("round_started_course_facile"), c.t(14_000));
    let room = s.room.as_ref().unwrap();
    assert!(room.result.is_none());
    assert!(room.round.as_ref().unwrap().guesses.is_empty());
}

fn course_round_snapshot_empty() -> RoomSnapshot {
    let mut snap = snapshot(fixture!("room_snapshot_course_round"));
    let round = snap.round.as_mut().unwrap();
    round.guesses.clear();
    round.progress.iter_mut().for_each(|p| {
        p.tries_used = 0;
    });
    round.start = match fixture!("round_started_course_facile") {
        ServerMsg::RoundStarted(r) => r,
        _ => unreachable!(),
    };
    snap.config.difficulty = boussole_protocol::Difficulty::Facile;
    snap
}

#[test]
fn the_last_round_keeps_its_result_until_the_final_ranking_is_shown() {
    let c = Clockwork::new();
    let mut s = apply(welcomed(&c), fixture!("room_snapshot_course_round"), c.t(0));
    let mut last = match fixture!("round_ended_course") {
        ServerMsg::RoundEnded(r) => r,
        _ => unreachable!(),
    };
    last.round_index = 3;
    last.next_round_in_ms = None;
    s = apply(s, ServerMsg::RoundEnded(last), c.t(100));
    s = apply(s, fixture!("match_ended"), c.t(100));
    s = apply(s, fixture!("room_snapshot_lobby"), c.t(100));

    let room = s.room.as_ref().unwrap();
    assert_eq!(room.phase, RoomPhase::Lobby);
    assert!(
        room.result.is_some(),
        "the last round's result is not lost to the snapshot"
    );
    let ended = s.ended.as_ref().expect("final ranking");
    assert_eq!(ended.ranking.ranking[0].nickname, "Alice");
    assert!(
        ended.show_at > c.t(100),
        "a few seconds to read the last result first"
    );

    s.dismiss_final();
    assert!(s.ended.is_none());
    assert!(s.room.as_ref().unwrap().result.is_none());
}

// --- Coopération --------------------------------------------------------------------------------

#[test]
fn a_whole_coop_round_turns_guesses_and_the_end() {
    let c = Clockwork::new();
    let mut lobby = snapshot(fixture!("room_snapshot_lobby"));
    lobby.config.mode = Mode::Cooperation;
    let mut s = apply(welcomed(&c), ServerMsg::RoomSnapshot(lobby), c.t(0));
    s = apply(s, fixture!("round_started_coop_difficile"), c.t(10));
    let round = s.room.as_ref().unwrap().round.as_ref().unwrap();
    assert!(matches!(round.clock, Clock::Turn { player: 2, .. }));
    assert!(
        round.progress.is_empty(),
        "Coopération has no per-player progress"
    );

    // Bob guesses: broadcast to everybody, `by` says who
    s = apply(
        s,
        ServerMsg::GuessResult(GuessInfo {
            by: 2,
            city: "Lyon".into(),
            distance_km: 215,
            bearing_deg: 201.5,
            correct: false,
            tries_used: 1,
        }),
        c.t(5_000),
    );
    s = apply(
        s,
        ServerMsg::TurnChanged {
            player_id: 1,
            remaining_ms: 45_000,
            reason: TurnReason::Guess,
        },
        c.t(5_000),
    );
    let round = s.room.as_ref().unwrap().round.as_ref().unwrap();
    assert_eq!(round.guesses.len(), 1);
    assert_eq!(round.guesses[0].by, 2);
    assert!(matches!(round.clock, Clock::Turn { player: 1, .. }));
    assert_eq!(
        round.clock.remaining(c.t(15_000)),
        Duration::from_millis(35_000)
    );

    // Alice times out: the turn passes without a new guess
    s = apply(
        s,
        ServerMsg::TurnChanged {
            player_id: 2,
            remaining_ms: 45_000,
            reason: TurnReason::Timeout,
        },
        c.t(50_000),
    );
    let round = s.room.as_ref().unwrap().round.as_ref().unwrap();
    assert_eq!(round.guesses.len(), 1, "a timeout uses no try");
    assert!(matches!(round.clock, Clock::Turn { player: 2, .. }));

    s = apply(s, fixture!("round_ended_coop"), c.t(60_000));
    let result = s.room.as_ref().unwrap().result.as_ref().unwrap();
    assert_eq!(result.ended.found_by, Some(2));
    assert_eq!(
        result.guesses.len(),
        1,
        "the team's guesses stay for the result view"
    );
}

#[test]
fn a_turn_change_with_no_round_is_ignored() {
    let c = Clockwork::new();
    let s = apply(welcomed(&c), fixture!("room_snapshot_lobby"), c.t(0));
    let s = apply(s, fixture!("turn_changed"), c.t(1));
    assert!(s.room.as_ref().unwrap().round.is_none());
}

// --- errors and housekeeping --------------------------------------------------------------------

#[test]
fn room_not_found_while_in_a_room_means_the_room_was_deleted() {
    let c = Clockwork::new();
    let s = apply(welcomed(&c), fixture!("room_snapshot_lobby"), c.t(0));
    let s = apply(s, ServerMsg::error(ErrorCode::RoomNotFound), c.t(1));
    assert!(s.room.is_none());
    assert_eq!(s.notice.as_ref().unwrap().kind, NoticeKind::Error);
}

#[test]
fn other_errors_become_a_french_notice_and_change_nothing_else() {
    let c = Clockwork::new();
    let s = apply(welcomed(&c), fixture!("room_snapshot_lobby"), c.t(0));
    let s = apply(s, fixture!("error"), c.t(1));
    assert!(s.room.is_some());
    assert_eq!(s.notice.as_ref().unwrap().text, "Cette salle est complète.");
    assert_eq!(s.last_error, Some(ErrorCode::RoomFull));
}

#[test]
fn room_not_found_without_a_room_is_a_plain_notice_for_the_join_form() {
    let c = Clockwork::new();
    let s = apply(
        welcomed(&c),
        ServerMsg::error(ErrorCode::RoomNotFound),
        c.t(1),
    );
    assert!(s.room.is_none());
    assert_eq!(s.last_error, Some(ErrorCode::RoomNotFound));
}

#[test]
fn pong_changes_nothing() {
    let c = Clockwork::new();
    let before = apply(welcomed(&c), fixture!("room_snapshot_lobby"), c.t(0));
    let after = apply(before.clone(), ServerMsg::Pong, c.t(1));
    assert_eq!(before, after);
}

#[test]
fn progress_for_a_player_not_yet_listed_is_added() {
    let c = Clockwork::new();
    let mut s = apply(welcomed(&c), fixture!("room_snapshot_lobby"), c.t(0));
    s = apply(s, fixture!("round_started_course_facile"), c.t(1));
    s = apply(s, fixture!("player_progress"), c.t(2));
    let round = s.room.as_ref().unwrap().round.as_ref().unwrap();
    assert!(round.progress.iter().any(|p| p.player_id == 1 && p.solved));
}

#[test]
fn every_server_message_variant_is_accepted_without_panicking_in_any_state() {
    // One fixture per variant (an exhaustive match fails the build when a variant is added).
    fn kind(msg: &ServerMsg) -> &'static str {
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
    let all = [
        fixture!("welcome"),
        fixture!("room_snapshot_lobby"),
        fixture!("room_snapshot_course_round"),
        fixture!("room_snapshot_coop_round"),
        fixture!("room_snapshot_intermission"),
        fixture!("round_started_course_facile"),
        fixture!("round_started_coop_difficile"),
        fixture!("guess_result"),
        fixture!("player_progress"),
        fixture!("turn_changed"),
        fixture!("round_ended_course"),
        fixture!("round_ended_coop"),
        fixture!("match_ended"),
        fixture!("error"),
        fixture!("pong"),
    ];
    let mut kinds: Vec<_> = all.iter().map(kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds.len(), 10, "every variant has a fixture: {kinds:?}");

    // Every message after every other one, from an empty state: never a panic.
    let c = Clockwork::new();
    for first in &all {
        for second in &all {
            let state = apply(OnlineState::new(), first.clone(), c.t(0));
            let _ = apply(state, second.clone(), c.t(1));
        }
    }
}

#[test]
fn a_timer_kind_always_matches_the_mode_of_the_round() {
    // Course rounds carry a round clock, Coopération rounds a turn clock (protocol §6.4).
    let c = Clockwork::new();
    let course = apply(
        welcomed(&c),
        fixture!("round_started_course_facile"),
        c.t(0),
    );
    // no room yet: the event is dropped, not applied to a phantom room
    assert!(course.room.is_none());
    let lobby = apply(welcomed(&c), fixture!("room_snapshot_lobby"), c.t(0));
    let course = apply(lobby, fixture!("round_started_course_facile"), c.t(0));
    let round = course.room.unwrap().round.unwrap();
    assert!(matches!(round.clock, Clock::Round { .. }));
}
