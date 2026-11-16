//! The online state and its pure reducer.
//!
//! [`apply`] folds one [`ServerMsg`] into an [`OnlineState`]: no socket, no clock of its own (the
//! caller passes `now`), no GUI types. The rules it follows are `docs/protocol.md` §6.1 and §15:
//! a `room_snapshot` replaces the room, events (`round_started`, `guess_result`, …) are applied on
//! top, and every time is a `remaining_ms` turned into a local [`Instant`] deadline.

use super::text;
use boussole_protocol::{
    ErrorCode, GuessInfo, HintDto, INTERMISSION_MS, MatchEnded, Mode, PlayerId, PlayerInfo,
    Progress, RoomConfig, RoomPhase, RoomSnapshot, RoundEnded, RoundStarted, RoundView, ServerMsg,
    Timer,
};
use std::time::{Duration, Instant};

/// How long the last round's result stays up before the final ranking replaces it.
pub const RESULT_BEFORE_RANKING: Duration = Duration::from_millis(INTERMISSION_MS);

/// State of the connection, as reported by the net layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// The first connection is being opened.
    Connecting,
    /// Connected and identified (`welcome` received).
    Online,
    /// The socket dropped; the client is retrying with its resume token.
    /// `give_up_at` comes from the net layer (see `NetEvent::Reconnecting`).
    Reconnecting { give_up_at: Instant, attempt: u32 },
    /// The client gave up (or was closed): nothing more will arrive.
    Lost,
}

/// Whether a notice is an error or plain information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Error,
}

/// A short French message for the player (an error, or "the room was closed").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

impl Notice {
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            kind: NoticeKind::Error,
            text: text.into(),
        }
    }
}

/// What counts down on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    /// Course: the end of the round.
    Round { deadline: Instant },
    /// Coopération: `player` has until `deadline` to guess.
    Turn { player: PlayerId, deadline: Instant },
}

impl Clock {
    fn from_timer(timer: Timer, now: Instant) -> Self {
        match timer {
            Timer::Round { remaining_ms } => Clock::Round {
                deadline: now + Duration::from_millis(remaining_ms),
            },
            Timer::Turn {
                player_id,
                remaining_ms,
            } => Clock::Turn {
                player: player_id,
                deadline: now + Duration::from_millis(remaining_ms),
            },
        }
    }

    pub fn deadline(&self) -> Instant {
        match self {
            Clock::Round { deadline } | Clock::Turn { deadline, .. } => *deadline,
        }
    }

    /// Time left at `now` (never negative).
    pub fn remaining(&self, now: Instant) -> Duration {
        self.deadline().saturating_duration_since(now)
    }
}

/// The round in progress, as this player sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundState {
    /// 1-based.
    pub index: u8,
    pub total: u8,
    pub max_tries: u8,
    pub hints: Vec<HintDto>,
    /// Course: my own guesses. Coopération: the team's, with their authors.
    pub guesses: Vec<GuessInfo>,
    /// Course: one entry per player (tries, solved, out of tries). Coopération: empty.
    pub progress: Vec<Progress>,
    pub clock: Clock,
}

impl RoundState {
    fn new(start: &RoundStarted, now: Instant) -> Self {
        // A Flèches round carries no try limit and no hints; the client renders it in
        // ticket 10, so until then an empty round is shown (never a panic).
        Self {
            index: start.round_index,
            total: start.total_rounds,
            max_tries: start.max_tries.unwrap_or(0),
            hints: start.hints.clone().unwrap_or_default(),
            guesses: Vec::new(),
            progress: Vec::new(),
            clock: Clock::from_timer(start.timer, now),
        }
    }

    fn from_view(view: &RoundView, now: Instant) -> Self {
        let mut round = Self::new(&view.start, now);
        round.guesses = view.guesses.clone();
        round.progress = view.progress.clone();
        round
    }
}

/// A finished round: what the server revealed, plus what the board needs to stay on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundResult {
    pub ended: RoundEnded,
    /// The five hints of the round (empty when the player resumed during the intermission:
    /// the snapshot only carries the result).
    pub hints: Vec<HintDto>,
    /// My guesses (Course) or the team's (Coopération) — what the history showed.
    pub guesses: Vec<GuessInfo>,
    /// When the next round starts; `None` after the last round.
    pub next_at: Option<Instant>,
}

/// The final ranking and when to show it (after the last round's result has been read).
#[derive(Debug, Clone, PartialEq)]
pub struct MatchEnd {
    pub ranking: MatchEnded,
    pub show_at: Instant,
}

/// Everything the client knows about the room it is in.
#[derive(Debug, Clone, PartialEq)]
pub struct Room {
    pub code: String,
    pub config: RoomConfig,
    pub phase: RoomPhase,
    /// In join order.
    pub players: Vec<PlayerInfo>,
    pub round: Option<RoundState>,
    pub result: Option<RoundResult>,
}

impl Room {
    pub fn player(&self, id: PlayerId) -> Option<&PlayerInfo> {
        self.players.iter().find(|p| p.id == id)
    }

    pub fn nickname(&self, id: PlayerId) -> Option<&str> {
        self.player(id).map(|p| p.nickname.as_str())
    }

    pub fn host(&self) -> Option<&PlayerInfo> {
        self.players.iter().find(|p| p.host)
    }

    pub fn mode(&self) -> Mode {
        self.config.mode
    }
}

/// The whole client-side picture of an online session.
#[derive(Debug, Clone, PartialEq)]
pub struct OnlineState {
    /// My player id (from `welcome`).
    pub me: Option<PlayerId>,
    pub link: Link,
    pub room: Option<Room>,
    /// Shown until the player acts again.
    pub notice: Option<Notice>,
    /// The error code behind the latest error `notice`, for the screen that asked.
    pub last_error: Option<ErrorCode>,
    /// The final ranking, once the match is over and until the player returns to the lobby.
    pub ended: Option<MatchEnd>,
}

impl Default for OnlineState {
    fn default() -> Self {
        Self::new()
    }
}

impl OnlineState {
    pub fn new() -> Self {
        Self {
            me: None,
            link: Link::Connecting,
            room: None,
            notice: None,
            last_error: None,
            ended: None,
        }
    }

    /// Am I the host of the room?
    pub fn am_host(&self) -> bool {
        match (&self.room, self.me) {
            (Some(room), Some(me)) => room.player(me).is_some_and(|p| p.host),
            _ => false,
        }
    }

    /// The player is about to create or join a room: forget the stale error.
    pub fn begin_room_request(&mut self) {
        self.notice = None;
        self.last_error = None;
    }

    /// "Retour au salon" on the final ranking.
    pub fn dismiss_final(&mut self) {
        self.ended = None;
        if let Some(room) = &mut self.room
            && room.phase == RoomPhase::Lobby
        {
            room.result = None;
        }
    }

    /// Skip the wait on the last round's result and show the final ranking right away.
    pub fn skip_result(&mut self, now: Instant) {
        if let Some(ended) = &mut self.ended {
            ended.show_at = now;
        }
    }
}

/// A link change from the net layer.
pub fn set_link(mut state: OnlineState, link: Link) -> OnlineState {
    state.link = link;
    state
}

/// Fold one server message into the state.
pub fn apply(mut state: OnlineState, msg: ServerMsg, now: Instant) -> OnlineState {
    match msg {
        ServerMsg::Welcome {
            player_id, resumed, ..
        } => {
            state.me = Some(player_id);
            state.link = Link::Online;
            if !resumed && state.room.is_some() {
                // The server does not know us any more: the room (and our slot) is gone.
                state.room = None;
                state.ended = None;
                state.notice = Some(Notice::error(text::SESSION_EXPIRED));
            }
        }
        ServerMsg::RoomSnapshot(snapshot) => apply_snapshot(&mut state, snapshot, now),
        ServerMsg::RoundStarted(start) => {
            if let Some(room) = &mut state.room {
                room.phase = RoomPhase::RoundActive;
                let mut round = RoundState::new(&start, now);
                if room.config.mode == Mode::Course {
                    round.progress = room
                        .players
                        .iter()
                        .map(|p| Progress {
                            player_id: p.id,
                            tries_used: 0,
                            solved: false,
                            out_of_tries: false,
                        })
                        .collect();
                }
                room.round = Some(round);
                room.result = None;
                state.ended = None;
            }
        }
        ServerMsg::GuessResult(info) => apply_guess(&mut state, info),
        // Flèches Course progress (who answered, never what): the client UI is ticket 10.
        ServerMsg::PlayerAnswered { .. } => {}
        ServerMsg::PlayerProgress(progress) => {
            if let Some(round) = state.room.as_mut().and_then(|r| r.round.as_mut()) {
                upsert_progress(round, progress);
            }
        }
        ServerMsg::TurnChanged {
            player_id,
            remaining_ms,
            ..
        } => {
            if let Some(round) = state.room.as_mut().and_then(|r| r.round.as_mut()) {
                round.clock = Clock::Turn {
                    player: player_id,
                    deadline: now + Duration::from_millis(remaining_ms),
                };
            }
        }
        ServerMsg::RoundEnded(ended) => {
            if let Some(room) = &mut state.room {
                let (hints, guesses) = match room.round.take() {
                    Some(round) => (round.hints, round.guesses),
                    None => (Vec::new(), Vec::new()),
                };
                if ended.next_round_in_ms.is_some() {
                    // The snapshot of this step confirms it; until then the pause has begun.
                    room.phase = RoomPhase::Intermission;
                }
                room.result = Some(RoundResult {
                    next_at: ended
                        .next_round_in_ms
                        .map(|ms| now + Duration::from_millis(ms)),
                    ended,
                    hints,
                    guesses,
                });
            }
        }
        ServerMsg::MatchEnded(ranking) => {
            if state.room.is_some() {
                state.ended = Some(MatchEnd {
                    ranking,
                    show_at: now + RESULT_BEFORE_RANKING,
                });
            }
        }
        ServerMsg::Error { code, .. } => apply_error(&mut state, code),
        ServerMsg::Pong => {}
    }
    state
}

fn apply_error(state: &mut OnlineState, code: ErrorCode) {
    if code == ErrorCode::RoomNotFound && state.room.is_some() {
        // Only sent to a member when the room was deleted (idle for 30 minutes).
        state.room = None;
        state.ended = None;
        state.notice = Some(Notice::error(text::ROOM_CLOSED));
    } else {
        state.notice = Some(Notice::error(text::error_fr(code)));
    }
    state.last_error = Some(code);
}

fn apply_snapshot(state: &mut OnlineState, snapshot: RoomSnapshot, now: Instant) {
    let previous = match state.room.take() {
        Some(room) if room.code == snapshot.code => Some(room),
        _ => {
            // Another room: nothing of the old one survives.
            state.ended = None;
            None
        }
    };
    let previous_result = previous.and_then(|room| room.result);

    let (round, result) = match snapshot.phase {
        RoomPhase::Lobby => {
            // Right after the last round the lobby snapshot follows `round_ended` and
            // `match_ended` in the same step: keep that result until the ranking has been seen.
            let keep = state.ended.is_some();
            (None, previous_result.filter(|_| keep))
        }
        RoomPhase::RoundActive => (
            snapshot
                .round
                .as_ref()
                .map(|v| RoundState::from_view(v, now)),
            None,
        ),
        RoomPhase::Intermission => (
            None,
            snapshot.intermission.as_ref().map(|inter| {
                // Keep the hints and guesses captured when this very round ended; a player who
                // resumed mid-pause only has the result.
                let (hints, guesses) = previous_result
                    .filter(|r| r.ended.round_index == inter.last_round.round_index)
                    .map_or((Vec::new(), Vec::new()), |r| (r.hints, r.guesses));
                RoundResult {
                    ended: inter.last_round.clone(),
                    hints,
                    guesses,
                    next_at: Some(now + Duration::from_millis(inter.remaining_ms)),
                }
            }),
        ),
    };

    state.room = Some(Room {
        code: snapshot.code,
        config: snapshot.config,
        phase: snapshot.phase,
        players: snapshot.players,
        round,
        result,
    });
}

fn apply_guess(state: &mut OnlineState, info: GuessInfo) {
    let me = state.me;
    let Some(room) = &mut state.room else { return };
    let Some(round) = &mut room.round else { return };
    match room.config.mode {
        Mode::Course => {
            // Private feedback: always mine.
            if Some(info.by) != me {
                return;
            }
            upsert_progress(
                round,
                Progress {
                    player_id: info.by,
                    tries_used: info.tries_used,
                    solved: info.correct,
                    out_of_tries: !info.correct && info.tries_used >= round.max_tries,
                },
            );
            round.guesses.push(info);
        }
        Mode::Cooperation => round.guesses.push(info),
    }
}

fn upsert_progress(round: &mut RoundState, progress: Progress) {
    match round
        .progress
        .iter_mut()
        .find(|p| p.player_id == progress.player_id)
    {
        Some(existing) => *existing = progress,
        None => round.progress.push(progress),
    }
}
