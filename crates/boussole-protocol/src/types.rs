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

/// Allowed `RoomConfig::rounds`.
pub const ROUNDS_RANGE: RangeInclusive<u32> = 1..=10;
/// Allowed `RoomConfig::max_players`.
pub const MAX_PLAYERS_RANGE: RangeInclusive<u32> = 2..=8;
/// Allowed `RoomConfig::round_time_s` (Course).
pub const ROUND_TIME_S_RANGE: RangeInclusive<u32> = 60..=300;
/// Allowed `RoomConfig::turn_time_s` (Coopération).
pub const TURN_TIME_S_RANGE: RangeInclusive<u32> = 15..=120;

/// Room settings. Numbers are `u32` on the wire so that out-of-range values reach
/// [`RoomConfig::validate`] (→ `invalid_config`); omitted numeric fields take their default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomConfig {
    pub mode: Mode,
    pub difficulty: Difficulty,
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

impl RoomConfig {
    /// A valid configuration with the default numbers.
    pub fn new(mode: Mode, difficulty: Difficulty) -> Self {
        Self {
            mode,
            difficulty,
            rounds: default_rounds(),
            max_players: default_max_players(),
            round_time_s: default_round_time_s(),
            turn_time_s: default_turn_time_s(),
        }
    }

    /// `Err(InvalidConfig)` when any number is outside its range (both time limits are
    /// checked whatever the mode).
    pub fn validate(&self) -> Result<(), ErrorCode> {
        let ok = ROUNDS_RANGE.contains(&self.rounds)
            && MAX_PLAYERS_RANGE.contains(&self.max_players)
            && ROUND_TIME_S_RANGE.contains(&self.round_time_s)
            && TURN_TIME_S_RANGE.contains(&self.turn_time_s);
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

/// Opening of a round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundStarted {
    /// 1-based.
    pub round_index: u8,
    pub total_rounds: u8,
    /// Always 6.
    pub max_tries: u8,
    /// Five arrow targets sorted by bearing. Never contains the secret.
    pub hints: Vec<HintDto>,
    pub timer: Timer,
}

/// The round in progress, as seen by one player (part of [`RoomSnapshot`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundView {
    /// The `round_started` data, with `timer` updated to the current remaining time.
    pub start: RoundStarted,
    /// Course: the receiver's own guesses. Coopération: the team's guesses, in order.
    pub guesses: Vec<GuessInfo>,
    /// Course: one entry per player (including the receiver). Coopération: empty.
    pub progress: Vec<Progress>,
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

/// Close of a round: the secret and everybody's guesses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundEnded {
    pub round_index: u8,
    pub total_rounds: u8,
    /// Name of the secret city.
    pub secret: String,
    /// Coopération: the player who found it (`None` if lost). Always `None` in Course.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found_by: Option<PlayerId>,
    /// One entry per player present at the end of the round, in join order.
    pub results: Vec<PlayerRoundResult>,
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
    /// Rounds this player solved (Coopération: rounds in which they made the winning guess).
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
