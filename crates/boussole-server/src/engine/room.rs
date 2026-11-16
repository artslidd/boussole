//! Engine state: players, rooms and the rounds in progress.

use super::{ConnId, Millis};
use boussole_core::arrows::ArrowQuestion;
use boussole_core::cities::City;
use boussole_core::game::Round;
use boussole_protocol::{PlayerId, RoomConfig, RoundEnded};
use std::collections::BTreeMap;

pub(super) struct Player {
    pub nickname: String,
    pub token: String,
    /// `None` while the socket is lost (the slot is held for the grace period).
    pub conn: Option<ConnId>,
    pub room: Option<String>,
    /// `Some` exactly while `conn` is `None` (players outside a room are dropped at once).
    pub disconnected_at: Option<Millis>,
}

#[derive(Default, Clone, Copy)]
pub(super) struct Score {
    pub total: u32,
    pub solved_rounds: u32,
}

pub(super) struct CourseState {
    pub round: Round,
    /// Points earned by solving, fixed at the moment of the winning guess.
    pub points: u32,
}

pub(super) struct CourseRound {
    pub deadline: Millis,
    /// One private round per member (same secret and hints).
    pub players: BTreeMap<PlayerId, CourseState>,
}

pub(super) struct CoopRound {
    pub round: Round,
    /// Who made each guess of `round`, in order.
    pub guessers: Vec<PlayerId>,
    pub turn: PlayerId,
    pub turn_deadline: Millis,
    /// Turns that timed out in a row (reset by any accepted guess).
    pub idle_turns: u32,
}

/// One player's answer state in a Flèches Course round.
#[derive(Default, Clone, Copy)]
pub(super) struct FlechesPlayerAnswer {
    /// The chosen option (0–3); `None` until the player answered.
    pub option: Option<u8>,
    /// Points earned (fixed at answer time); 0 when wrong or unanswered.
    pub points: u32,
}

pub(super) struct FlechesCourseRound {
    /// End of the question (`question_time_s` after the round started).
    pub deadline: Millis,
    /// One answer slot per member (same question for everyone).
    pub answers: BTreeMap<PlayerId, FlechesPlayerAnswer>,
}

pub(super) struct FlechesCoopRound {
    /// The single answerer of this question (rotates with the round index).
    pub turn: PlayerId,
    /// End of the answerer's question (`question_time_s` after the round started).
    pub turn_deadline: Millis,
    /// The team's answer, `None` until the answerer played (the round ends at once then).
    pub answer: Option<u8>,
}

pub(super) enum Kind {
    Course(CourseRound),
    Coop(CoopRound),
    FlechesCourse(FlechesCourseRound),
    FlechesCoop(FlechesCoopRound),
}

/// What the round is about: the Devinette secret, or the Flèches question. Both are
/// revealed by `round_ended` and must never leave the engine before.
pub(super) enum RoundContent {
    Devinette { secret: &'static City },
    Fleches(ArrowQuestion),
}

pub(super) struct Active {
    pub started_at: Millis,
    pub content: RoundContent,
    pub kind: Kind,
}

pub(super) struct Pause {
    pub until: Millis,
    pub ended: RoundEnded,
}

pub(super) enum Phase {
    Lobby,
    Active(Active),
    Intermission(Pause),
}

pub(super) struct Room {
    pub code: String,
    pub config: RoomConfig,
    pub host: PlayerId,
    /// Join order.
    pub members: Vec<PlayerId>,
    pub phase: Phase,
    /// Course totals, kept after the match until the next `start_match`.
    pub scores: BTreeMap<PlayerId, Score>,
    /// Coopération team total.
    pub team_total: u32,
    /// 1-based index of the current (or last) round; 0 before the first.
    pub round_index: u8,
    pub last_secret: Option<&'static City>,
    /// The origin/target of the last Flèches round (no two consecutive rounds share one).
    pub last_fleches: Option<(&'static City, &'static City)>,
    /// Match seed drawn at `start_match`; Flèches round *r* uses `seed + r` (documented).
    pub seed: u64,
    pub last_activity: Millis,
}
