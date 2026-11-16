//! Data carried by the messages. Field names are the JSON keys (snake_case).

use crate::{ErrorCode, ROOM_CODE_ALPHABET, ROOM_CODE_LEN};
use serde::{Deserialize, Serialize};
use std::ops::RangeInclusive;

/// Server-assigned player identity: unique on the server, stable across reconnections.
pub type PlayerId = u32;

/// Game mode, chosen by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Everyone races on the same city, each with a private round.
    Course,
    /// Players take turns guessing in one shared round.
    Cooperation,
}

/// Difficulty level, chosen by the host; `facile` shows hint distances, `difficile` does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Facile,
    Difficile,
}

impl From<boussole_core::game::Difficulty> for Difficulty {
    fn from(d: boussole_core::game::Difficulty) -> Self {
        match d {
            boussole_core::game::Difficulty::Facile => Self::Facile,
            boussole_core::game::Difficulty::Difficile => Self::Difficile,
        }
    }
}

impl From<Difficulty> for boussole_core::game::Difficulty {
    fn from(d: Difficulty) -> Self {
        match d {
            Difficulty::Facile => Self::Facile,
            Difficulty::Difficile => Self::Difficile,
        }
    }
}

/// The game played in a room (the room's `mode` — Course / Coopération — is orthogonal).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameKind {
    /// Guess the hidden city (the original game).
    #[default]
    Devinette,
    /// Pick the arrow that points from the named origin to the named target.
    Fleches,
}

/// `GameKind::Devinette`? (the serde skip rule that keeps old configs byte-identical).
fn is_default_game(game: &GameKind) -> bool {
    *game == GameKind::Devinette
}

/// Allowed `RoomConfig::rounds`.
pub const ROUNDS_RANGE: RangeInclusive<u32> = 1..=10;
/// Allowed `RoomConfig::max_players`.
pub const MAX_PLAYERS_RANGE: RangeInclusive<u32> = 2..=8;
/// Allowed `RoomConfig::round_time_s` (Course).
pub const ROUND_TIME_S_RANGE: RangeInclusive<u32> = 60..=300;
/// Allowed `RoomConfig::turn_time_s` (Coopération).
pub const TURN_TIME_S_RANGE: RangeInclusive<u32> = 15..=120;
/// Allowed `RoomConfig::question_time_s` (Flèches).
pub const QUESTION_TIME_S_RANGE: RangeInclusive<u32> = 5..=60;

/// Room settings. Numbers are `u32` on the wire so that out-of-range values reach
/// [`RoomConfig::validate`] (→ `invalid_config`); omitted numeric fields take their default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomConfig {
    pub mode: Mode,
    pub difficulty: Difficulty,
    /// Which game the room plays (default, and therefore **omitted on the wire**:
    /// a config without `game` is a Devinette room).
    #[serde(default, skip_serializing_if = "is_default_game")]
    pub game: GameKind,
    /// Rounds per match, 1–10 (default 5).
    #[serde(default = "default_rounds")]
    pub rounds: u32,
    /// Seats, 2–8 (default 8).
    #[serde(default = "default_max_players")]
    pub max_players: u32,
    /// Course: seconds per round, 60–300 (default 120).
    #[serde(default = "default_round_time_s")]
    pub round_time_s: u32,
    /// Coopération: seconds per turn, 15–120 (default 45).
    #[serde(default = "default_turn_time_s")]
    pub turn_time_s: u32,
    /// Flèches: seconds per question, 5–60 (default 15). Omitted on the wire when 15
    /// (the default), so a Devinette config stays byte-identical to protocol 1.
    #[serde(
        default = "default_question_time_s",
        skip_serializing_if = "is_default_question_time_s"
    )]
    pub question_time_s: u32,
}

fn default_rounds() -> u32 {
    5
}
fn default_max_players() -> u32 {
    8
}
fn default_round_time_s() -> u32 {
    120
}
fn default_turn_time_s() -> u32 {
    45
}
fn default_question_time_s() -> u32 {
    15
}
fn is_default_question_time_s(s: &u32) -> bool {
    *s == default_question_time_s()
}

impl RoomConfig {
    /// A valid configuration with the default numbers.
    pub fn new(mode: Mode, difficulty: Difficulty) -> Self {
        Self {
            mode,
            difficulty,
            game: GameKind::Devinette,
            rounds: default_rounds(),
            max_players: default_max_players(),
            round_time_s: default_round_time_s(),
            turn_time_s: default_turn_time_s(),
            question_time_s: default_question_time_s(),
        }
    }

    /// `Err(InvalidConfig)` when any number is outside its range (all time limits are
    /// checked whatever the mode or game).
    pub fn validate(&self) -> Result<(), ErrorCode> {
        let ok = ROUNDS_RANGE.contains(&self.rounds)
            && MAX_PLAYERS_RANGE.contains(&self.max_players)
            && ROUND_TIME_S_RANGE.contains(&self.round_time_s)
            && TURN_TIME_S_RANGE.contains(&self.turn_time_s)
            && QUESTION_TIME_S_RANGE.contains(&self.question_time_s);
        if ok {
            Ok(())
        } else {
            Err(ErrorCode::InvalidConfig)
        }
    }
}

/// Trim a nickname and check it: 1–16 Unicode scalar values, no control character.
/// Returns the trimmed nickname, or `None` (→ `invalid_nickname`).
pub fn normalize_nickname(raw: &str) -> Option<String> {
    let nick = raw.trim();
    let len = nick.chars().count();
    if len == 0 || len > crate::NICKNAME_MAX_CHARS || nick.chars().any(char::is_control) {
        return None;
    }
    Some(nick.to_owned())
}

/// Key under which nicknames are compared: unique case-insensitively inside a room.
pub fn nickname_key(nickname: &str) -> String {
    nickname.to_lowercase()
}

/// Canonical (upper-case) form of a typed room code, or `None` if it cannot be a room code.
/// Joining is case-insensitive.
pub fn normalize_room_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    let valid = code.chars().count() == ROOM_CODE_LEN
        && code.chars().all(|c| ROOM_CODE_ALPHABET.contains(c));
    valid.then_some(code)
}

/// An arrow target seen from the secret city.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HintDto {
    pub city: String,
    /// Compass bearing from the secret to `city`, degrees clockwise from north, 0.1° precision.
    pub bearing_deg: f64,
    /// Rounded kilometres. Present **only** in `facile`; in `difficile` the key is omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_km: Option<u32>,
}

/// One Flèches question, as sent in `round_started` (and in the `round` of a snapshot).
/// It carries **no** correct index and **no** distance: both are revealed by `round_ended`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlechesQuestion {
    /// Start city, at the centre of the dial.
    pub origin: String,
    /// City to aim at, named in the question ("De Lyon vers Brest ?").
    pub target: String,
    /// The four option bearings in display order, degrees clockwise from north, 0.1°
    /// precision, each in `[0, 360)`.
    pub options_deg: [f64; 4],
}

/// A player as listed in a room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: PlayerId,
    pub nickname: String,
    pub host: bool,
    /// `false` while the player's socket is lost and their slot is held (up to 60 s).
    pub connected: bool,
    /// Course: personal total. Coopération: the team total (same for everyone).
    pub score: u32,
}

/// Where a room is in its life cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomPhase {
    Lobby,
    RoundActive,
    Intermission,
}

/// How many tries a player has used in the current round, and whether they are done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub player_id: PlayerId,
    pub tries_used: u8,
    pub solved: bool,
    pub out_of_tries: bool,
}

/// One accepted guess. Distance and bearing go **from the guess to the secret**.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuessInfo {
    /// Who guessed (in Course always the receiver).
    pub by: PlayerId,
    pub city: String,
    /// Rounded kilometres, in both difficulties.
    pub distance_km: u32,
    /// Degrees clockwise from north, 0.1° precision.
    pub bearing_deg: f64,
    pub correct: bool,
    /// Tries used after this guess (Course: the guesser's; Coopération: the team's).
    pub tries_used: u8,
}

/// The countdown that applies to a round, as time **remaining** when the message was built
/// (clients must not assume synchronised clocks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Timer {
    /// Course: time left in the round.
    Round { remaining_ms: u64 },
    /// Coopération: whose turn it is and the time left for that turn.
    Turn {
        player_id: PlayerId,
        remaining_ms: u64,
    },
}

/// Opening of a round. A Devinette round carries `max_tries` and `hints`; a Flèches round
/// carries `question` instead. The other game's fields are **absent** (never `null`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundStarted {
    /// 1-based.
    pub round_index: u8,
    pub total_rounds: u8,
    /// Devinette only: the try limit (always 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tries: Option<u8>,
    /// Devinette only: five arrow targets sorted by bearing; never contains the secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hints: Option<Vec<HintDto>>,
    /// Flèches only: the question (origin, target, the four option bearings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<FlechesQuestion>,
    pub timer: Timer,
}

/// The round in progress, as seen by one player (part of [`RoomSnapshot`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundView {
    /// The `round_started` data, with `timer` updated to the current remaining time.
    pub start: RoundStarted,
    /// Devinette: the receiver's own guesses (Course) or the team's (Coopération).
    /// Flèches: always empty (nothing may be revealed before `round_ended`).
    pub guesses: Vec<GuessInfo>,
    /// Devinette Course: one entry per player (including the receiver). Otherwise empty.
    pub progress: Vec<Progress>,
    /// Flèches Course: the players who already answered this question, in join order.
    /// Absent in Devinette and in Flèches Coopération.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered: Option<Vec<PlayerId>>,
}

/// One guess as revealed at the end of the round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuessEntry {
    pub city: String,
    pub distance_km: u32,
}

/// A player's round, revealed at the end of the round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerRoundResult {
    pub player_id: PlayerId,
    /// Course: the player's guesses. Coopération: the guesses this player made.
    pub guesses: Vec<GuessEntry>,
    /// Course: found the city. Coopération: made the winning guess.
    pub solved: bool,
    pub tries_used: u8,
    /// Course: the player's points. Coopération: the team's points (same for everyone).
    pub points: u32,
    /// Match total after this round (Coopération: team total).
    pub total: u32,
}

/// One Flèches answer as revealed at the end of the round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlechesAnswer {
    pub player_id: PlayerId,
    /// The chosen option index (0–3); **absent** when the player did not answer in time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<u8>,
    /// Whether `option` is the correct arrow (`false` when there was no answer).
    pub correct: bool,
    /// Course: the player's points for the round. Coopération: the team's points.
    pub points: u32,
    /// Match total after this round (Coopération: the team total).
    pub total: u32,
}

/// The Flèches reveal carried by `round_ended`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlechesReveal {
    /// Index (0–3) in `options_deg` of the correct arrow.
    pub correct: u8,
    /// Distance origin → target, whole kilometres.
    pub distance_km: u32,
    /// Compass direction of the correct arrow (`Compass8::label_fr`: "N", "NE", … "NO").
    pub compass: String,
    /// One entry per player present at the end of the round, in join order.
    pub answers: Vec<FlechesAnswer>,
}

/// Close of a round: the reveal and everybody's answers or guesses. A Devinette round
/// carries `secret` and `results`; a Flèches round carries `fleches`. The other game's
/// fields are **absent** (never `null`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundEnded {
    pub round_index: u8,
    pub total_rounds: u8,
    /// Devinette only: name of the secret city.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// Coopération (both games): the player who found it / made the winning answer.
    /// Always absent in Course.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found_by: Option<PlayerId>,
    /// Devinette only: one entry per player present at the end of the round, in join order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results: Option<Vec<PlayerRoundResult>>,
    /// Flèches only: the correct arrow, the distance, the compass and each player's answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleches: Option<FlechesReveal>,
    /// Milliseconds until the next round starts; `None` after the last round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_round_in_ms: Option<u64>,
}

/// The pause between two rounds (part of [`RoomSnapshot`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intermission {
    pub next_round_index: u8,
    pub remaining_ms: u64,
    /// The `round_ended` that opened this pause (`next_round_in_ms` = `remaining_ms`).
    pub last_round: RoundEnded,
}

/// Full state of a room for one receiver; replaces whatever the client knew.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomSnapshot {
    pub code: String,
    pub config: RoomConfig,
    pub phase: RoomPhase,
    /// In join order.
    pub players: Vec<PlayerInfo>,
    /// Present iff `phase == round_active`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<RoundView>,
    /// Present iff `phase == intermission`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intermission: Option<Intermission>,
}

/// Why the turn changed (Coopération).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnReason {
    /// The previous player ran out of time (no try consumed).
    Timeout,
    /// The previous player made an accepted guess.
    Guess,
    /// The previous player disconnected or left.
    Disconnect,
}

/// One line of the final ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankEntry {
    pub player_id: PlayerId,
    pub nickname: String,
    /// Course: personal total. Coopération: team total.
    pub total: u32,
    /// Competition ranking (1, 1, 3): 1 + number of players with a strictly higher total.
    /// Coopération: everyone is 1.
    pub rank: u32,
    /// Rounds this player solved (Coopération: rounds in which they made the winning move —
    /// in Flèches, the correct answer).
    pub solved_rounds: u32,
}

/// End of the match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchEnded {
    pub mode: Mode,
    /// Course: by total, highest first. Coopération: by `solved_rounds`, highest first.
    /// Ties keep join order.
    pub ranking: Vec<RankEntry>,
}
