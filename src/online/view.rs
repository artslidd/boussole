//! What each online screen shows, derived from the reducer state ([`OnlineState`]) and the
//! current time. Pure Rust: the Slint glue only copies these values into the window.

use super::state::{Clock, Link, OnlineState, Room, RoundResult};
use crate::board::{HintLabel, format_dto_label};
use boussole_protocol::{
    Difficulty, GuessInfo, HintDto, MAX_PLAYERS_RANGE, MatchEnded, Mode, PlayerId,
    PlayerRoundResult, ROUND_TIME_S_RANGE, ROUNDS_RANGE, RoomConfig, RoomPhase, TURN_TIME_S_RANGE,
};
use std::time::{Duration, Instant};

/// Below this much time left the clock is flagged as urgent.
const URGENT_BELOW: Duration = Duration::from_secs(10);

/// Which screen to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Not in a room: the "En ligne" screen (Héberger / Rejoindre).
    Menu,
    Lobby,
    Round,
    /// End of a round: the city revealed, everybody's results, the countdown.
    Result,
    /// End of the match.
    Final,
}

/// The screen for `state` at `now`.
pub fn screen(state: &OnlineState, now: Instant) -> Screen {
    let Some(room) = &state.room else {
        return Screen::Menu;
    };
    if let Some(ended) = &state.ended {
        // The last round's result stays up until its countdown is over (or is skipped).
        return if now >= ended.show_at || room.result.is_none() {
            Screen::Final
        } else {
            Screen::Result
        };
    }
    match room.phase {
        RoomPhase::Lobby => Screen::Lobby,
        // Between `round_ended` and its snapshot the phase may still say "round" for the last
        // round: a result without a round on the table is the end-of-round screen.
        RoomPhase::RoundActive | RoomPhase::Intermission => {
            if room.round.is_some() {
                Screen::Round
            } else if room.result.is_some() {
                Screen::Result
            } else {
                Screen::Lobby
            }
        }
    }
}

// --- banner ------------------------------------------------------------------------------------

/// The strip across the top when the connection is not healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Banner {
    Reconnecting { text: String, seconds_left: u64 },
    Lost { text: String },
}

pub fn banner(state: &OnlineState, now: Instant) -> Option<Banner> {
    match state.link {
        Link::Reconnecting { give_up_at, .. } => {
            // Whole seconds, rounded up: part of a second left still reads 1 s.
            let left = give_up_at.saturating_duration_since(now);
            let seconds_left = left.as_secs() + u64::from(left.subsec_nanos() > 0);
            // Past the deadline the last attempt can still be running (it may take several
            // seconds to time out): say so instead of counting "encore 0 s".
            let text = if seconds_left == 0 {
                "Reconnexion… dernière tentative en cours".to_owned()
            } else {
                format!("Reconnexion… encore {seconds_left} s avant d’abandonner")
            };
            Some(Banner::Reconnecting { text, seconds_left })
        }
        Link::Lost => Some(Banner::Lost {
            text: "Connexion perdue".to_owned(),
        }),
        Link::Connecting | Link::Online => None,
    }
}

// --- clock -------------------------------------------------------------------------------------

/// `m:ss`, seconds rounded up so the clock reads 0:00 only when time is really up.
pub fn format_clock(remaining: Duration) -> String {
    let secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockView {
    pub text: String,
    /// The last seconds: the screen makes the clock stand out.
    pub urgent: bool,
    pub remaining: Duration,
}

fn clock_view(clock: &Clock, now: Instant) -> ClockView {
    let remaining = clock.remaining(now);
    ClockView {
        text: format_clock(remaining),
        urgent: remaining <= URGENT_BELOW,
        remaining,
    }
}

// --- lobby -------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyPlayer {
    pub id: PlayerId,
    pub name: String,
    pub host: bool,
    pub me: bool,
    pub connected: bool,
    /// For screen readers: "Alice (hôte, vous)".
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsView {
    /// Only the host can change the settings.
    pub editable: bool,
    pub mode: Mode,
    pub difficulty: Difficulty,
    pub rounds: u32,
    pub max_players: u32,
    /// "Temps par manche" (Course) or "Temps par tour" (Coopération).
    pub time_label: &'static str,
    pub time_s: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyView {
    pub code: String,
    pub players: Vec<LobbyPlayer>,
    pub settings: SettingsView,
    pub i_am_host: bool,
    pub can_start: bool,
    /// Why "Lancer la partie" is disabled, or what happens next.
    pub start_hint: String,
}

fn me_in(state: &OnlineState) -> Option<PlayerId> {
    state.me
}

pub fn lobby(state: &OnlineState) -> Option<LobbyView> {
    let room = state.room.as_ref()?;
    let me = me_in(state);
    let i_am_host = state.am_host();
    let connected = room.players.iter().filter(|p| p.connected).count();
    let players = room
        .players
        .iter()
        .map(|p| {
            let mut tags = Vec::new();
            if p.host {
                tags.push("hôte");
            }
            if Some(p.id) == me {
                tags.push("vous");
            }
            if !p.connected {
                tags.push("déconnecté");
            }
            LobbyPlayer {
                id: p.id,
                name: p.nickname.clone(),
                host: p.host,
                me: Some(p.id) == me,
                connected: p.connected,
                description: describe(&p.nickname, &tags),
            }
        })
        .collect();
    let can_start = i_am_host && connected >= 2 && state.link == Link::Online;
    let start_hint = if !i_am_host {
        match room.host() {
            Some(host) => format!("En attente du lancement par {}…", host.nickname),
            None => "En attente du lancement par l’hôte…".to_owned(),
        }
    } else if connected < 2 {
        "Il faut au moins 2 joueurs connectés.".to_owned()
    } else {
        "Lancez quand tout le monde est là.".to_owned()
    };
    Some(LobbyView {
        code: room.code.clone(),
        players,
        settings: settings_view(&room.config, i_am_host),
        i_am_host,
        can_start,
        start_hint,
    })
}

fn describe(name: &str, tags: &[&str]) -> String {
    if tags.is_empty() {
        name.to_owned()
    } else {
        format!("{name} ({})", tags.join(", "))
    }
}

fn settings_view(config: &RoomConfig, editable: bool) -> SettingsView {
    let (time_label, time_s) = match config.mode {
        Mode::Course => ("Temps par manche", config.round_time_s),
        Mode::Cooperation => ("Temps par tour", config.turn_time_s),
    };
    SettingsView {
        editable,
        mode: config.mode,
        difficulty: config.difficulty,
        rounds: config.rounds,
        max_players: config.max_players,
        time_label,
        time_s,
    }
}

/// A numeric setting of the lobby.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Rounds,
    MaxPlayers,
    /// The time limit of the current mode: per round (Course) or per turn (Coopération).
    Time,
}

/// The configuration after pressing "−" (`delta = -1`) or "+" (`delta = 1`) on `setting`, or
/// `None` when it is already at its limit. Seats never drop below the players present.
pub fn step_setting(
    config: &RoomConfig,
    setting: Setting,
    delta: i32,
    members: usize,
) -> Option<RoomConfig> {
    let mut next = *config;
    match setting {
        Setting::Rounds => {
            next.rounds = stepped(config.rounds, delta, 1, ROUNDS_RANGE);
        }
        Setting::MaxPlayers => {
            let floor =
                (members as u32).clamp(*MAX_PLAYERS_RANGE.start(), *MAX_PLAYERS_RANGE.end());
            next.max_players = stepped(
                config.max_players,
                delta,
                1,
                floor..=*MAX_PLAYERS_RANGE.end(),
            );
        }
        Setting::Time => match config.mode {
            Mode::Course => {
                next.round_time_s = stepped(config.round_time_s, delta, 15, ROUND_TIME_S_RANGE)
            }
            Mode::Cooperation => {
                next.turn_time_s = stepped(config.turn_time_s, delta, 5, TURN_TIME_S_RANGE)
            }
        },
    }
    (next != *config).then_some(next)
}

fn stepped(value: u32, delta: i32, step: u32, range: std::ops::RangeInclusive<u32>) -> u32 {
    let moved = if delta >= 0 {
        value.saturating_add(step)
    } else {
        value.saturating_sub(step)
    };
    moved.clamp(*range.start(), *range.end())
}

// --- round -------------------------------------------------------------------------------------

/// How a player stands in a Course round. Never carries a city name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStatus {
    Playing,
    Solved,
    OutOfTries,
}

/// One line of the live scoreboard (Course) or of the turn order (Coopération).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreRow {
    pub id: PlayerId,
    pub name: String,
    pub me: bool,
    pub host: bool,
    pub connected: bool,
    pub tries: u8,
    pub status: ProgressStatus,
    /// Coopération: this player holds the turn.
    pub active: bool,
    /// Match total so far.
    pub total: u32,
    pub description: String,
}

/// One guess of the history. `by` is set in Coopération only (the history is shared).
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRow {
    pub city: String,
    pub distance_km: u32,
    /// From the guess towards the hidden city.
    pub bearing_deg: f64,
    pub correct: bool,
    pub by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputView {
    pub enabled: bool,
    /// Why the input is closed (empty when it is open).
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoundView {
    pub mode: Mode,
    pub difficulty: Difficulty,
    pub index: u8,
    pub total: u8,
    pub hints: Vec<HintLabel>,
    pub clock: ClockView,
    /// Coopération: "À toi de jouer !" / "Au tour de Camille…". Empty in Course.
    pub banner: String,
    pub my_turn: bool,
    /// Course: my tries. Coopération: the team's.
    pub tries_used: u8,
    pub max_tries: u8,
    pub history: Vec<HistoryRow>,
    pub scoreboard: Vec<ScoreRow>,
    pub input: InputView,
}

/// Arrow labels for the board: the name, and the distance iff the server sent one.
pub fn board_hints(hints: &[HintDto]) -> Vec<HintLabel> {
    hints
        .iter()
        .map(|hint| HintLabel {
            bearing_deg: hint.bearing_deg,
            text: format_dto_label(&hint.city, hint.distance_km),
        })
        .collect()
}

fn name_of(room: &Room, id: PlayerId) -> String {
    room.nickname(id)
        .map_or_else(|| format!("Joueur {id}"), str::to_owned)
}

fn history_rows(room: &Room, guesses: &[GuessInfo]) -> Vec<HistoryRow> {
    guesses
        .iter()
        .map(|g| HistoryRow {
            city: g.city.clone(),
            distance_km: g.distance_km,
            bearing_deg: g.bearing_deg,
            correct: g.correct,
            by: (room.config.mode == Mode::Cooperation).then(|| name_of(room, g.by)),
        })
        .collect()
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n > 1 { many } else { one })
}

pub fn round(state: &OnlineState, now: Instant) -> Option<RoundView> {
    let room = state.room.as_ref()?;
    if room.phase != RoomPhase::RoundActive {
        return None;
    }
    let round = room.round.as_ref()?;
    let me = state.me;
    let coop = room.config.mode == Mode::Cooperation;

    let turn_player = match round.clock {
        Clock::Turn { player, .. } => Some(player),
        Clock::Round { .. } => None,
    };
    let my_turn = coop && turn_player.is_some() && turn_player == me;
    let banner = match (coop, turn_player) {
        (true, Some(_)) if my_turn => "À toi de jouer !".to_owned(),
        (true, Some(player)) => format!("Au tour de {}…", name_of(room, player)),
        _ => String::new(),
    };

    let scoreboard: Vec<ScoreRow> = room
        .players
        .iter()
        .map(|p| {
            let progress = round.progress.iter().find(|q| q.player_id == p.id);
            let tries = progress.map_or(0, |q| q.tries_used);
            let status = match progress {
                Some(q) if q.solved => ProgressStatus::Solved,
                Some(q) if q.out_of_tries => ProgressStatus::OutOfTries,
                _ => ProgressStatus::Playing,
            };
            let active = coop && turn_player == Some(p.id);
            let mut tags = Vec::new();
            if p.host {
                tags.push("hôte".to_owned());
            }
            if Some(p.id) == me {
                tags.push("vous".to_owned());
            }
            if !p.connected {
                tags.push("déconnecté".to_owned());
            }
            let detail = if coop {
                if active {
                    "joue en ce moment".to_owned()
                } else {
                    String::new()
                }
            } else {
                match status {
                    ProgressStatus::Solved => {
                        format!(
                            "a trouvé en {}",
                            plural(u32::from(tries), "essai", "essais")
                        )
                    }
                    ProgressStatus::OutOfTries => "n’a plus d’essais".to_owned(),
                    ProgressStatus::Playing => {
                        format!("{} sur {} utilisés", tries, round.max_tries)
                    }
                }
            };
            let mut description = describe(
                &p.nickname,
                &tags.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            if !detail.is_empty() {
                description = format!("{description} : {detail}");
            }
            ScoreRow {
                id: p.id,
                name: p.nickname.clone(),
                me: Some(p.id) == me,
                host: p.host,
                connected: p.connected,
                tries,
                status,
                active,
                total: p.score,
                description,
            }
        })
        .collect();

    let tries_used = if coop {
        u8::try_from(round.guesses.len()).unwrap_or(u8::MAX)
    } else {
        round
            .progress
            .iter()
            .find(|q| Some(q.player_id) == me)
            .map_or(u8::try_from(round.guesses.len()).unwrap_or(u8::MAX), |q| {
                q.tries_used
            })
    };

    let input = input_view(state, room, my_turn, turn_player);
    Some(RoundView {
        mode: room.config.mode,
        difficulty: room.config.difficulty,
        index: round.index,
        total: round.total,
        hints: board_hints(&round.hints),
        clock: clock_view(&round.clock, now),
        banner,
        my_turn,
        tries_used,
        max_tries: round.max_tries,
        history: history_rows(room, &round.guesses),
        scoreboard,
        input,
    })
}

fn input_view(
    state: &OnlineState,
    room: &Room,
    my_turn: bool,
    turn_player: Option<PlayerId>,
) -> InputView {
    let closed = |message: String| InputView {
        enabled: false,
        message,
    };
    match state.link {
        Link::Reconnecting { .. } | Link::Connecting => {
            return closed("Reconnexion en cours : la saisie reprendra ensuite.".to_owned());
        }
        Link::Lost => return closed("Connexion perdue.".to_owned()),
        Link::Online => {}
    }
    let round = room.round.as_ref();
    match room.config.mode {
        Mode::Cooperation => {
            if my_turn {
                InputView {
                    enabled: true,
                    message: String::new(),
                }
            } else {
                let who =
                    turn_player.map_or_else(|| "un autre joueur".to_owned(), |p| name_of(room, p));
                closed(format!("Ce n’est pas ton tour : {who} joue."))
            }
        }
        Mode::Course => {
            let mine =
                round.and_then(|r| r.progress.iter().find(|q| Some(q.player_id) == state.me));
            match mine {
                Some(q) if q.solved => {
                    closed("Trouvé ! Les autres joueurs finissent leur manche…".to_owned())
                }
                Some(q) if q.out_of_tries => {
                    closed("Plus d’essais : les autres joueurs finissent leur manche…".to_owned())
                }
                _ => InputView {
                    enabled: true,
                    message: String::new(),
                },
            }
        }
    }
}

// --- end of round ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ResultRow {
    pub id: PlayerId,
    pub name: String,
    pub me: bool,
    pub solved: bool,
    /// Coopération: made the winning guess.
    pub found: bool,
    pub tries: u8,
    /// Course: this round's points. Coopération: the team's.
    pub points: u32,
    pub total: u32,
    /// "Lyon · Dijon · Besançon"
    pub guesses: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResultView {
    pub mode: Mode,
    pub difficulty: Difficulty,
    pub secret: String,
    /// "Manche 1 sur 3"
    pub round_label: String,
    pub last_round: bool,
    pub headline: String,
    /// "Manche suivante dans 6 s" / "Classement final dans 6 s".
    pub countdown: Option<String>,
    pub seconds_left: u64,
    pub rows: Vec<ResultRow>,
    /// Coopération: the team's points for the round.
    pub team_points: Option<u32>,
    /// The round's arrows, so the board can stay up around the revealed city.
    pub hints: Vec<HintLabel>,
    pub history: Vec<HistoryRow>,
}

fn ceil_secs(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

pub fn result(state: &OnlineState, now: Instant) -> Option<ResultView> {
    let room = state.room.as_ref()?;
    let result: &RoundResult = room.result.as_ref()?;
    if screen(state, now) != Screen::Result {
        return None;
    }
    let ended = &result.ended;
    let me = state.me;
    let coop = room.config.mode == Mode::Cooperation;
    // Flèche rooms carry their reveal in `ended.fleches` instead; the client learns to show
    // it in ticket 10. Until then a Flèche round just renders empty here (no panic).
    let results: &[PlayerRoundResult] = ended.results.as_deref().unwrap_or(&[]);
    let secret = ended.secret.as_deref().unwrap_or("");

    let mut rows: Vec<ResultRow> = results
        .iter()
        .map(|r| {
            let name = name_of(room, r.player_id);
            let guesses = if r.guesses.is_empty() {
                "aucun essai".to_owned()
            } else {
                r.guesses
                    .iter()
                    .map(|g| g.city.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            let found = coop && ended.found_by == Some(r.player_id);
            let verdict = if coop {
                if found { "a trouvé la ville" } else { "" }
            } else if r.solved {
                "a trouvé la ville"
            } else {
                "n’a pas trouvé"
            };
            let mut description = if Some(r.player_id) == me {
                format!("{name} (vous)")
            } else {
                name.clone()
            };
            if !verdict.is_empty() {
                description = format!("{description} {verdict}");
            }
            description = format!(
                "{description}, {}, {guesses}, {} points",
                plural(u32::from(r.tries_used), "essai", "essais"),
                r.points
            );
            ResultRow {
                id: r.player_id,
                name,
                me: Some(r.player_id) == me,
                solved: r.solved,
                found,
                tries: r.tries_used,
                points: r.points,
                total: r.total,
                guesses,
                description,
            }
        })
        .collect();
    if !coop {
        rows.sort_by_key(|r| std::cmp::Reverse(r.points)); // stable: ties keep join order
    }

    let team_points = coop.then(|| results.first().map_or(0, |r| r.points));
    let headline = if coop {
        match ended.found_by {
            Some(id) => format!("{} a trouvé {} !", name_of(room, id), secret),
            None => format!("Perdu ! C’était {}.", secret),
        }
    } else {
        match results.iter().find(|r| Some(r.player_id) == me) {
            Some(r) if r.solved => format!(
                "Bravo ! C’était {} en {}.",
                secret,
                plural(u32::from(r.tries_used), "essai", "essais")
            ),
            _ => format!("C’était {}.", secret),
        }
    };

    let last_round = ended.next_round_in_ms.is_none();
    let (countdown, seconds_left) = if last_round {
        match &state.ended {
            Some(end) => {
                let secs = ceil_secs(end.show_at.saturating_duration_since(now));
                (Some(format!("Classement final dans {secs} s")), secs)
            }
            None => (None, 0),
        }
    } else {
        match result.next_at {
            Some(at) => {
                let secs = ceil_secs(at.saturating_duration_since(now));
                (Some(format!("Manche suivante dans {secs} s")), secs)
            }
            None => (None, 0),
        }
    };

    Some(ResultView {
        mode: room.config.mode,
        difficulty: room.config.difficulty,
        secret: secret.to_owned(),
        round_label: format!("Manche {} sur {}", ended.round_index, ended.total_rounds),
        last_round,
        headline,
        countdown,
        seconds_left,
        rows,
        team_points,
        hints: board_hints(&result.hints),
        history: history_rows(room, &result.guesses),
    })
}

// --- final ranking -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankRow {
    /// "1", "2"… in Course; empty in Coopération (one team).
    pub rank: String,
    pub name: String,
    pub me: bool,
    pub total: u32,
    /// "3 manches trouvées"
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalView {
    pub mode: Mode,
    /// "Classement final" (Course) or "Score d’équipe" (Coopération).
    pub title: String,
    pub rows: Vec<RankRow>,
    /// Coopération: the team's total.
    pub team_total: Option<u32>,
}

pub fn final_ranking(state: &OnlineState) -> Option<FinalView> {
    let ended = &state.ended.as_ref()?.ranking;
    Some(final_view(ended, state.me))
}

fn final_view(ended: &MatchEnded, me: Option<PlayerId>) -> FinalView {
    let coop = ended.mode == Mode::Cooperation;
    let rows = ended
        .ranking
        .iter()
        .map(|entry| RankRow {
            rank: if coop {
                String::new()
            } else {
                entry.rank.to_string()
            },
            name: entry.nickname.clone(),
            me: Some(entry.player_id) == me,
            total: entry.total,
            description: format!(
                "{} {}",
                plural(entry.solved_rounds, "manche", "manches"),
                if entry.solved_rounds > 1 {
                    "trouvées"
                } else {
                    "trouvée"
                }
            ),
        })
        .collect();
    FinalView {
        mode: ended.mode,
        title: if coop {
            "Score d’équipe"
        } else {
            "Classement final"
        }
        .to_owned(),
        rows,
        team_total: coop.then(|| ended.ranking.first().map_or(0, |e| e.total)),
    }
}
