//! The view-model: what each online screen shows, derived from the reducer state. Covers the
//! acceptance rules: Facile shows km on hint labels and Difficile never does, Coopération input
//! is disabled when it is not my turn, a Course scoreboard shows no city names, host-only
//! controls are disabled for the others, plus screen selection, banners, results and ranking.

use boussole::online::view::{
    self, Banner, ProgressStatus, Screen, Setting, format_clock, step_setting,
};
use boussole::online::{OnlineState, apply, set_link};
use boussole_protocol::{
    Difficulty, GuessInfo, Mode, PlayerInfo, Progress, RoomConfig, RoomPhase, RoomSnapshot,
    RoundEnded, ServerMsg, Timer, TurnReason, decode_server,
};
use std::time::{Duration, Instant};

macro_rules! fixture {
    ($name:literal) => {
        decode_server(include_str!(concat!(
            "../crates/boussole-protocol/tests/fixtures/server/",
            $name,
            ".json"
        )))
        .expect(concat!("fixture ", $name))
    };
}

struct T(Instant);
impl T {
    fn new() -> Self {
        Self(Instant::now())
    }
    fn at(&self, ms: u64) -> Instant {
        self.0 + Duration::from_millis(ms)
    }
}

fn snapshot(msg: ServerMsg) -> RoomSnapshot {
    match msg {
        ServerMsg::RoomSnapshot(s) => s,
        other => panic!("not a snapshot: {other:?}"),
    }
}

fn player(id: u32, name: &str, host: bool, connected: bool) -> PlayerInfo {
    PlayerInfo {
        id,
        nickname: name.to_owned(),
        host,
        connected,
        score: 0,
    }
}

/// A lobby of `players`, me being player `me`.
fn lobby(
    me: u32,
    mode: Mode,
    difficulty: Difficulty,
    players: Vec<PlayerInfo>,
    t: &T,
) -> OnlineState {
    let mut snap = snapshot(fixture!("room_snapshot_lobby"));
    snap.config = RoomConfig::new(mode, difficulty);
    snap.players = players;
    let state = apply(
        OnlineState::new(),
        ServerMsg::Welcome {
            player_id: me,
            resume_token: "0".repeat(32),
            resumed: false,
        },
        t.at(0),
    );
    apply(state, ServerMsg::RoomSnapshot(snap), t.at(0))
}

fn three_players() -> Vec<PlayerInfo> {
    vec![
        player(1, "Alice", true, true),
        player(2, "Bob", false, true),
        player(3, "Camille", false, true),
    ]
}

fn started(mut state: OnlineState, start: ServerMsg, t: &T) -> OnlineState {
    state = apply(state, start, t.at(10));
    // the snapshot of the same step
    let room = state.room.clone().unwrap();
    let round = room.round.clone().unwrap();
    let snap = RoomSnapshot {
        code: room.code,
        config: room.config,
        phase: RoomPhase::RoundActive,
        players: room.players,
        round: Some(boussole_protocol::RoundView {
            answered: None,
            start: boussole_protocol::RoundStarted {
                round_index: round.index,
                total_rounds: round.total,
                max_tries: Some(round.max_tries),
                hints: Some(round.hints),
                question: None,
                timer: match round.clock {
                    boussole::online::Clock::Round { .. } => Timer::Round {
                        remaining_ms: 90_000,
                    },
                    boussole::online::Clock::Turn { player, .. } => Timer::Turn {
                        player_id: player,
                        remaining_ms: 45_000,
                    },
                },
            },
            guesses: round.guesses,
            progress: round.progress,
        }),
        intermission: None,
    };
    apply(state, ServerMsg::RoomSnapshot(snap), t.at(10))
}

// --- hint labels ---------------------------------------------------------------------------------

#[test]
fn facile_labels_show_the_distance_and_difficile_labels_never_do() {
    let t = T::new();
    let facile = started(
        lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t),
        fixture!("round_started_course_facile"),
        &t,
    );
    let round = view::round(&facile, t.at(20)).unwrap();
    let texts: Vec<_> = round.hints.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Amiens · 403 km",
            "Belfort · 77 km",
            "Bastia · 572 km",
            "Perpignan · 562 km",
            "Angers · 497 km"
        ]
    );

    let difficile = started(
        lobby(
            1,
            Mode::Cooperation,
            Difficulty::Difficile,
            three_players(),
            &t,
        ),
        fixture!("round_started_coop_difficile"),
        &t,
    );
    let round = view::round(&difficile, t.at(20)).unwrap();
    let texts: Vec<_> = round.hints.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(
        texts,
        ["Amiens", "Belfort", "Bastia", "Perpignan", "Angers"]
    );
    assert!(
        round
            .hints
            .iter()
            .all(|h| !h.text.contains("km") && !h.text.contains('·')),
        "no distance anywhere on a Difficile arrow"
    );
}

#[test]
fn bearings_are_kept_exactly_for_the_board() {
    let t = T::new();
    let state = started(
        lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t),
        fixture!("round_started_course_facile"),
        &t,
    );
    let round = view::round(&state, t.at(20)).unwrap();
    assert_eq!(round.hints[0].bearing_deg, 330.5);
    assert_eq!(round.hints[1].bearing_deg, 48.0);
}

// --- Coopération: whose turn, input, banner, countdown ----------------------------------------

fn coop_round(me: u32, t: &T) -> OnlineState {
    // fixture: Bob (id 2) holds the first turn with 45 s
    started(
        lobby(
            me,
            Mode::Cooperation,
            Difficulty::Difficile,
            three_players(),
            t,
        ),
        fixture!("round_started_coop_difficile"),
        t,
    )
}

#[test]
fn coop_input_is_disabled_when_it_is_not_my_turn_and_says_why() {
    let t = T::new();
    let state = coop_round(1, &t); // I am Alice, Bob plays
    let round = view::round(&state, t.at(10)).unwrap();
    assert!(!round.input.enabled);
    assert!(!round.my_turn);
    assert_eq!(round.banner, "Au tour de Bob…");
    assert!(
        round.input.message.contains("Bob"),
        "the explanation names who is playing: {}",
        round.input.message
    );
}

#[test]
fn coop_input_is_enabled_on_my_turn_and_the_banner_calls_me() {
    let t = T::new();
    let state = coop_round(2, &t); // I am Bob
    let round = view::round(&state, t.at(10)).unwrap();
    assert!(round.input.enabled);
    assert!(round.my_turn);
    assert_eq!(round.banner, "À toi de jouer !");
}

#[test]
fn the_turn_passes_with_turn_changed_and_the_banner_follows() {
    let t = T::new();
    let mut state = coop_round(1, &t);
    state = apply(
        state,
        ServerMsg::GuessResult(GuessInfo {
            by: 2,
            city: "Lyon".into(),
            distance_km: 215,
            bearing_deg: 201.5,
            correct: false,
            tries_used: 1,
        }),
        t.at(5_000),
    );
    state = apply(
        state,
        ServerMsg::TurnChanged {
            player_id: 1,
            remaining_ms: 45_000,
            reason: TurnReason::Guess,
        },
        t.at(5_000),
    );
    let round = view::round(&state, t.at(5_000)).unwrap();
    assert!(round.input.enabled && round.my_turn);
    assert_eq!(round.banner, "À toi de jouer !");
    assert_eq!(round.tries_used, 1, "the team's tries");
    // the shared history says who guessed what
    assert_eq!(round.history.len(), 1);
    assert_eq!(round.history[0].by.as_deref(), Some("Bob"));
    assert_eq!(round.history[0].city, "Lyon");
}

#[test]
fn the_countdown_is_counted_down_locally_from_remaining_ms() {
    let t = T::new();
    let state = coop_round(1, &t); // 45 s turn announced at t = 10 ms
    assert_eq!(view::round(&state, t.at(10)).unwrap().clock.text, "0:45");
    assert_eq!(
        view::round(&state, t.at(14_010)).unwrap().clock.text,
        "0:31"
    );
    assert_eq!(
        view::round(&state, t.at(44_500)).unwrap().clock.text,
        "0:01"
    );
    assert_eq!(
        view::round(&state, t.at(99_000)).unwrap().clock.text,
        "0:00"
    );
    let almost = view::round(&state, t.at(40_000)).unwrap();
    assert!(almost.clock.urgent, "the last 10 s are flagged");
    assert!(!view::round(&state, t.at(10)).unwrap().clock.urgent);
}

#[test]
fn clock_text_is_minutes_and_seconds_rounded_up() {
    assert_eq!(format_clock(Duration::from_millis(0)), "0:00");
    assert_eq!(format_clock(Duration::from_millis(1)), "0:01");
    assert_eq!(format_clock(Duration::from_millis(9_000)), "0:09");
    assert_eq!(format_clock(Duration::from_millis(71_250)), "1:12");
    assert_eq!(format_clock(Duration::from_millis(120_000)), "2:00");
}

#[test]
fn coop_input_is_disabled_while_reconnecting_even_on_my_turn() {
    let t = T::new();
    let state = coop_round(2, &t);
    let state = set_link(
        state,
        boussole::online::Link::Reconnecting {
            give_up_at: t.at(60_100),
            attempt: 1,
        },
    );
    let round = view::round(&state, t.at(200)).unwrap();
    assert!(!round.input.enabled);
    assert!(
        round.input.message.contains("Reconnexion"),
        "{}",
        round.input.message
    );
}

// --- Course: private progress, no city names ----------------------------------------------------

#[test]
fn the_course_scoreboard_shows_tries_and_status_but_never_a_city_name() {
    let t = T::new();
    let mut state = started(
        lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t),
        fixture!("round_started_course_facile"),
        &t,
    );
    state = apply(
        state,
        ServerMsg::PlayerProgress(Progress {
            player_id: 2,
            tries_used: 3,
            solved: false,
            out_of_tries: false,
        }),
        t.at(1_000),
    );
    state = apply(
        state,
        ServerMsg::PlayerProgress(Progress {
            player_id: 3,
            tries_used: 4,
            solved: true,
            out_of_tries: false,
        }),
        t.at(2_000),
    );
    state = apply(
        state,
        ServerMsg::GuessResult(GuessInfo {
            by: 1,
            city: "Lyon".into(),
            distance_km: 163,
            bearing_deg: 24.5,
            correct: false,
            tries_used: 1,
        }),
        t.at(3_000),
    );
    let round = view::round(&state, t.at(3_000)).unwrap();
    let rows = &round.scoreboard;
    assert_eq!(rows.len(), 3);
    assert_eq!(
        (rows[0].name.as_str(), rows[0].tries, rows[0].status),
        ("Alice", 1, ProgressStatus::Playing)
    );
    assert!(rows[0].me && rows[0].host);
    assert_eq!((rows[1].name.as_str(), rows[1].tries), ("Bob", 3));
    assert_eq!((rows[2].tries, rows[2].status), (4, ProgressStatus::Solved));

    // Everything a scoreboard row can say, as text: no city of the dataset appears in it.
    let dump = format!("{rows:?}");
    for city in boussole::cities::CITIES {
        // my own guess "Lyon" is private to me and not part of any row either
        assert!(
            !dump.contains(city.name),
            "the scoreboard leaks a city name: {} in {dump}",
            city.name
        );
    }
    for row in rows {
        assert!(row.description.contains(&row.name));
    }
    // my own history, on the other hand, is mine to see
    assert_eq!(round.history.len(), 1);
    assert_eq!(round.history[0].city, "Lyon");
    assert!(
        round.history[0].by.is_none(),
        "in Course the history is mine alone"
    );
}

#[test]
fn course_input_closes_once_i_solved_or_ran_out_of_tries() {
    let t = T::new();
    let mut state = started(
        lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t),
        fixture!("round_started_course_facile"),
        &t,
    );
    assert!(view::round(&state, t.at(20)).unwrap().input.enabled);
    state = apply(
        state,
        ServerMsg::GuessResult(GuessInfo {
            by: 1,
            city: "Besançon".into(),
            distance_km: 0,
            bearing_deg: 0.0,
            correct: true,
            tries_used: 2,
        }),
        t.at(2_000),
    );
    let round = view::round(&state, t.at(2_000)).unwrap();
    assert!(!round.input.enabled);
    assert!(
        round.input.message.contains("Trouvé"),
        "{}",
        round.input.message
    );

    // out of tries
    let mut state = started(
        lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t),
        fixture!("round_started_course_facile"),
        &t,
    );
    for n in 1..=6u8 {
        state = apply(
            state,
            ServerMsg::GuessResult(GuessInfo {
                by: 1,
                city: format!("Ville{n}"),
                distance_km: 100,
                bearing_deg: 10.0,
                correct: false,
                tries_used: n,
            }),
            t.at(1_000 * u64::from(n)),
        );
    }
    let round = view::round(&state, t.at(7_000)).unwrap();
    assert!(!round.input.enabled);
    assert_eq!(round.scoreboard[0].status, ProgressStatus::OutOfTries);
}

#[test]
fn the_course_countdown_is_the_round_clock() {
    let t = T::new();
    let state = started(
        lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t),
        fixture!("round_started_course_facile"),
        &t,
    );
    let round = view::round(&state, t.at(30_010)).unwrap();
    assert_eq!(round.clock.text, "1:00");
    assert!(round.banner.is_empty(), "Course has no turn banner");
    assert!(round.input.enabled);
}

// --- lobby: host-only controls -----------------------------------------------------------------

#[test]
fn only_the_host_can_edit_the_settings_or_start() {
    let t = T::new();
    let host = lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t);
    let lobby_view = view::lobby(&host).unwrap();
    assert!(lobby_view.i_am_host);
    assert!(lobby_view.settings.editable);
    assert!(lobby_view.can_start);

    let guest = lobby(2, Mode::Course, Difficulty::Facile, three_players(), &t);
    let lobby_view = view::lobby(&guest).unwrap();
    assert!(!lobby_view.i_am_host);
    assert!(
        !lobby_view.settings.editable,
        "settings are read-only for guests"
    );
    assert!(!lobby_view.can_start);
    assert!(
        lobby_view.start_hint.contains("Alice"),
        "{}",
        lobby_view.start_hint
    );
}

#[test]
fn start_needs_two_connected_players() {
    let t = T::new();
    let alone = lobby(
        1,
        Mode::Course,
        Difficulty::Facile,
        vec![player(1, "Alice", true, true)],
        &t,
    );
    let view_alone = view::lobby(&alone).unwrap();
    assert!(!view_alone.can_start);
    assert!(
        view_alone.start_hint.contains("2 joueurs"),
        "{}",
        view_alone.start_hint
    );

    let one_gone = lobby(
        1,
        Mode::Course,
        Difficulty::Facile,
        vec![
            player(1, "Alice", true, true),
            player(2, "Bob", false, false),
        ],
        &t,
    );
    assert!(
        !view::lobby(&one_gone).unwrap().can_start,
        "a held slot is not a player"
    );

    let ready = lobby(
        1,
        Mode::Course,
        Difficulty::Facile,
        vec![
            player(1, "Alice", true, true),
            player(2, "Bob", false, true),
        ],
        &t,
    );
    assert!(view::lobby(&ready).unwrap().can_start);
}

#[test]
fn the_lobby_lists_players_with_crown_me_and_connection_state() {
    let t = T::new();
    let mut players = three_players();
    players[2].connected = false;
    let state = lobby(2, Mode::Course, Difficulty::Facile, players, &t);
    let lobby_view = view::lobby(&state).unwrap();
    assert_eq!(lobby_view.code, "K7QX2");
    let rows = &lobby_view.players;
    assert_eq!(rows.len(), 3);
    assert!(rows[0].host && !rows[0].me);
    assert!(rows[1].me);
    assert!(!rows[2].connected);
    assert!(
        rows[2].description.contains("déconnecté"),
        "{}",
        rows[2].description
    );
    assert!(
        rows[0].description.contains("hôte"),
        "{}",
        rows[0].description
    );
    assert!(
        rows[1].description.contains("vous"),
        "{}",
        rows[1].description
    );
}

#[test]
fn settings_describe_the_time_limit_of_the_mode() {
    let t = T::new();
    let course = view::lobby(&lobby(
        1,
        Mode::Course,
        Difficulty::Facile,
        three_players(),
        &t,
    ))
    .unwrap();
    assert_eq!(course.settings.time_label, "Temps par manche");
    assert_eq!(course.settings.time_s, 120);
    let coop = view::lobby(&lobby(
        1,
        Mode::Cooperation,
        Difficulty::Difficile,
        three_players(),
        &t,
    ))
    .unwrap();
    assert_eq!(coop.settings.time_label, "Temps par tour");
    assert_eq!(coop.settings.time_s, 45);
}

#[test]
fn steppers_stay_inside_the_protocol_ranges_and_never_cut_below_the_players_present() {
    let base = RoomConfig::new(Mode::Course, Difficulty::Facile);
    // rounds 1..=10
    let mut cfg = base;
    cfg.rounds = 10;
    assert_eq!(step_setting(&cfg, Setting::Rounds, 1, 3), None);
    cfg.rounds = 1;
    assert_eq!(step_setting(&cfg, Setting::Rounds, -1, 3), None);
    assert_eq!(
        step_setting(&base, Setting::Rounds, 1, 3).unwrap().rounds,
        6
    );
    // players 2..=8, never below the members present
    let mut cfg = base;
    cfg.max_players = 4;
    assert_eq!(
        step_setting(&cfg, Setting::MaxPlayers, -1, 4),
        None,
        "4 members hold 4 seats"
    );
    assert_eq!(
        step_setting(&cfg, Setting::MaxPlayers, -1, 3)
            .unwrap()
            .max_players,
        3
    );
    cfg.max_players = 2;
    assert_eq!(step_setting(&cfg, Setting::MaxPlayers, -1, 2), None);
    // time: Course 60..=300 by 15 s, Coopération 15..=120 by 5 s
    let mut cfg = base;
    cfg.round_time_s = 300;
    assert_eq!(step_setting(&cfg, Setting::Time, 1, 2), None);
    assert_eq!(
        step_setting(&base, Setting::Time, 1, 2)
            .unwrap()
            .round_time_s,
        135
    );
    let coop = RoomConfig::new(Mode::Cooperation, Difficulty::Facile);
    assert_eq!(
        step_setting(&coop, Setting::Time, 1, 2)
            .unwrap()
            .turn_time_s,
        50
    );
    let mut cfg = coop;
    cfg.turn_time_s = 15;
    assert_eq!(step_setting(&cfg, Setting::Time, -1, 2), None);
    // every result validates
    for setting in [Setting::Rounds, Setting::MaxPlayers, Setting::Time] {
        for delta in [-1, 1] {
            if let Some(next) = step_setting(&base, setting, delta, 2) {
                assert!(next.validate().is_ok(), "{setting:?} {delta}");
            }
        }
    }
}

// --- screens ------------------------------------------------------------------------------------

#[test]
fn the_screen_follows_the_match_from_menu_to_final_ranking_and_back() {
    let t = T::new();
    let mut state = OnlineState::new();
    assert_eq!(view::screen(&state, t.at(0)), Screen::Menu);

    state = lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t);
    assert_eq!(view::screen(&state, t.at(0)), Screen::Lobby);

    state = started(state, fixture!("round_started_course_facile"), &t);
    assert_eq!(view::screen(&state, t.at(20)), Screen::Round);

    state = apply(state, fixture!("round_ended_course"), t.at(6_000));
    state = apply(state, fixture!("room_snapshot_intermission"), t.at(6_000));
    assert_eq!(view::screen(&state, t.at(6_000)), Screen::Result);

    // last round: result first, ranking after the pause (or at once on request)
    let mut last = match fixture!("round_ended_course") {
        ServerMsg::RoundEnded(r) => r,
        _ => unreachable!(),
    };
    last.round_index = 3;
    last.next_round_in_ms = None;
    state = apply(state, ServerMsg::RoundEnded(last), t.at(20_000));
    state = apply(state, fixture!("match_ended"), t.at(20_000));
    state = apply(state, fixture!("room_snapshot_lobby"), t.at(20_000));
    assert_eq!(view::screen(&state, t.at(20_100)), Screen::Result);
    assert_eq!(view::screen(&state, t.at(28_100)), Screen::Final);
    let mut skipped = state.clone();
    skipped.skip_result(t.at(20_200));
    assert_eq!(view::screen(&skipped, t.at(20_300)), Screen::Final);

    state.dismiss_final();
    assert_eq!(view::screen(&state, t.at(30_000)), Screen::Lobby);
}

#[test]
fn without_a_room_the_menu_is_shown() {
    let t = T::new();
    let mut state = lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t);
    state.room = None;
    assert_eq!(view::screen(&state, t.at(1)), Screen::Menu);
}

// --- banner ------------------------------------------------------------------------------------

#[test]
fn the_banner_counts_down_to_the_deadline_the_net_layer_gave_and_then_gives_up() {
    let t = T::new();
    let state = lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t);
    assert_eq!(view::banner(&state, t.at(0)), None);

    let state = set_link(
        state,
        boussole::online::Link::Reconnecting {
            give_up_at: t.at(61_000),
            attempt: 3,
        },
    );
    match view::banner(&state, t.at(11_000)) {
        Some(Banner::Reconnecting { text, seconds_left }) => {
            assert_eq!(seconds_left, 50);
            assert!(text.starts_with("Reconnexion"), "{text}");
            assert!(text.contains("50"), "{text}");
        }
        other => panic!("expected a reconnecting banner, got {other:?}"),
    }
    // part of a second left still reads 1 s, never 0 s
    match view::banner(&state, t.at(60_500)) {
        Some(Banner::Reconnecting { seconds_left, .. }) => assert_eq!(seconds_left, 1),
        other => panic!("{other:?}"),
    }
    // Past the deadline the last attempt may still be running (a connection attempt takes up to
    // 8 s): say so instead of "encore 0 s", for as long as it lasts.
    for at in [61_000, 64_000, 69_000] {
        match view::banner(&state, t.at(at)) {
            Some(Banner::Reconnecting { text, seconds_left }) => {
                assert_eq!(seconds_left, 0);
                assert!(text.contains("dernière tentative"), "{text}");
                assert!(!text.contains(" 0 s"), "{text}");
            }
            other => panic!("expected a reconnecting banner at {at} ms, got {other:?}"),
        }
    }
    let state = set_link(state, boussole::online::Link::Lost);
    match view::banner(&state, t.at(70_000)) {
        Some(Banner::Lost { text }) => assert!(text.contains("Connexion perdue"), "{text}"),
        other => panic!("expected the lost banner, got {other:?}"),
    }
}

// --- end of round and final ranking -------------------------------------------------------------

#[test]
fn the_course_result_reveals_the_city_guesses_and_points_best_first() {
    let t = T::new();
    let mut state = started(
        lobby(
            2,
            Mode::Course,
            Difficulty::Facile,
            vec![
                player(1, "Alice", true, true),
                player(2, "Bob", false, true),
            ],
            &t,
        ),
        fixture!("round_started_course_facile"),
        &t,
    );
    state = apply(state, fixture!("round_ended_course"), t.at(6_000));
    state = apply(state, fixture!("room_snapshot_intermission"), t.at(6_000));
    let result = view::result(&state, t.at(6_000)).unwrap();
    assert_eq!(result.secret, "Besançon");
    assert_eq!(result.round_label, "Manche 1 sur 3");
    assert!(!result.last_round);
    assert_eq!(
        result.countdown.as_deref(),
        Some("Manche suivante dans 6 s")
    );
    // best first: Alice 496, Bob 0
    assert_eq!(result.rows[0].name, "Alice");
    assert_eq!(result.rows[0].points, 496);
    assert!(result.rows[0].solved);
    assert_eq!(result.rows[0].guesses, "Lyon · Dijon · Besançon");
    assert_eq!(result.rows[1].name, "Bob");
    assert!(result.rows[1].me);
    assert_eq!(result.rows[1].points, 0);
    assert_eq!(result.rows[1].guesses, "Paris");
    assert!(result.team_points.is_none());
    assert_eq!(
        result.hints.len(),
        5,
        "the arrows stay up around the revealed city"
    );
}

#[test]
fn the_coop_result_says_who_found_the_city_and_the_team_points() {
    let t = T::new();
    let mut lobby_snap = snapshot(fixture!("room_snapshot_lobby"));
    lobby_snap.config = RoomConfig::new(Mode::Cooperation, Difficulty::Facile);
    let mut state = apply(
        OnlineState::new(),
        ServerMsg::Welcome {
            player_id: 1,
            resume_token: "0".repeat(32),
            resumed: false,
        },
        t.at(0),
    );
    state = apply(state, ServerMsg::RoomSnapshot(lobby_snap), t.at(0));
    state = apply(state, fixture!("round_ended_coop"), t.at(1_000));
    let result = view::result(&state, t.at(1_000)).unwrap();
    assert_eq!(result.secret, "Nîmes");
    assert_eq!(result.team_points, Some(500));
    let finder = result.rows.iter().find(|r| r.found).expect("a finder");
    assert_eq!(finder.name, "Bob");
    assert!(result.headline.contains("Bob"), "{}", result.headline);
}

#[test]
fn a_lost_coop_round_has_no_finder() {
    let t = T::new();
    let mut lobby_snap = snapshot(fixture!("room_snapshot_lobby"));
    lobby_snap.config = RoomConfig::new(Mode::Cooperation, Difficulty::Facile);
    let mut state = apply(
        OnlineState::new(),
        ServerMsg::Welcome {
            player_id: 1,
            resume_token: "0".repeat(32),
            resumed: false,
        },
        t.at(0),
    );
    state = apply(state, ServerMsg::RoomSnapshot(lobby_snap), t.at(0));
    let mut ended = match fixture!("round_ended_coop") {
        ServerMsg::RoundEnded(r) => r,
        _ => unreachable!(),
    };
    ended.found_by = None;
    if let Some(results) = &mut ended.results {
        for r in results {
            r.solved = false;
            r.points = 0;
        }
    }
    state = apply(
        state,
        ServerMsg::RoundEnded(RoundEnded { ..ended }),
        t.at(1_000),
    );
    let result = view::result(&state, t.at(1_000)).unwrap();
    assert_eq!(result.team_points, Some(0));
    assert!(result.rows.iter().all(|r| !r.found));
    assert!(result.headline.contains("Perdu"), "{}", result.headline);
}

#[test]
fn the_last_round_counts_down_to_the_final_ranking() {
    let t = T::new();
    let mut state = lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t);
    let mut last = match fixture!("round_ended_course") {
        ServerMsg::RoundEnded(r) => r,
        _ => unreachable!(),
    };
    last.round_index = 3;
    last.next_round_in_ms = None;
    state = apply(state, ServerMsg::RoundEnded(last), t.at(0));
    state = apply(state, fixture!("match_ended"), t.at(0));
    let result = view::result(&state, t.at(2_000)).unwrap();
    assert!(result.last_round);
    assert_eq!(
        result.countdown.as_deref(),
        Some("Classement final dans 6 s")
    );
}

#[test]
fn the_final_ranking_lists_ranks_totals_and_ties_for_course() {
    let t = T::new();
    let mut state = lobby(2, Mode::Course, Difficulty::Facile, three_players(), &t);
    state = apply(state, fixture!("match_ended"), t.at(0));
    let fin = view::final_ranking(&state).unwrap();
    assert_eq!(fin.title, "Classement final");
    assert_eq!(fin.rows[0].rank, "1");
    assert_eq!(fin.rows[0].name, "Alice");
    assert_eq!(fin.rows[0].total, 1190);
    assert_eq!(fin.rows[1].rank, "2");
    assert!(fin.rows[1].me);
    assert!(
        fin.rows[0].description.contains("3 manches"),
        "{}",
        fin.rows[0].description
    );
    assert!(fin.team_total.is_none());
}

#[test]
fn the_coop_final_shows_the_team_score_and_each_players_finds() {
    let t = T::new();
    let state = lobby(
        1,
        Mode::Cooperation,
        Difficulty::Facile,
        three_players(),
        &t,
    );
    let ended = ServerMsg::MatchEnded(boussole_protocol::MatchEnded {
        mode: Mode::Cooperation,
        ranking: vec![
            boussole_protocol::RankEntry {
                player_id: 2,
                nickname: "Bob".into(),
                total: 1400,
                rank: 1,
                solved_rounds: 2,
            },
            boussole_protocol::RankEntry {
                player_id: 1,
                nickname: "Alice".into(),
                total: 1400,
                rank: 1,
                solved_rounds: 1,
            },
        ],
    });
    let state = apply(state, ended, t.at(0));
    let fin = view::final_ranking(&state).unwrap();
    assert_eq!(fin.title, "Score d’équipe");
    assert_eq!(fin.team_total, Some(1400));
    assert_eq!(fin.rows[0].name, "Bob");
    assert!(
        fin.rows[0].description.contains("2 manches trouvées"),
        "{}",
        fin.rows[0].description
    );
    assert!(
        fin.rows[1].description.contains("1 manche trouvée"),
        "{}",
        fin.rows[1].description
    );
}

#[test]
fn views_are_absent_when_their_screen_is_not_the_current_one() {
    let t = T::new();
    let state = lobby(1, Mode::Course, Difficulty::Facile, three_players(), &t);
    assert!(view::lobby(&state).is_some());
    assert!(view::round(&state, t.at(0)).is_none());
    assert!(view::result(&state, t.at(0)).is_none());
    assert!(view::final_ranking(&state).is_none());
    assert!(view::lobby(&OnlineState::new()).is_none());
}
