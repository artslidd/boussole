use crate::hex;
use crate::types::*;
use crate::{ErrorCode, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};

/// Client → server messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Must be the first message of a connection.
    Hello {
        protocol_version: u16,
        /// `boussole_core::dataset_fingerprint()` as 16 lowercase hex digits.
        #[serde(with = "hex")]
        dataset_fingerprint: u64,
        nickname: String,
        /// Token from a previous `welcome`, to resume a dropped session.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume_token: Option<String>,
    },
    CreateRoom {
        config: RoomConfig,
    },
    JoinRoom {
        code: String,
    },
    LeaveRoom,
    /// Host only, lobby only.
    UpdateConfig {
        config: RoomConfig,
    },
    /// Host only, lobby only.
    StartMatch,
    /// Guess a city by its exact name (as in `boussole_core::cities::CITIES`).
    /// Devinette only.
    Guess {
        city: String,
    },
    /// Answer the current Flèches question by picking an option index (0–3).
    /// Flèches only.
    Answer {
        option: u8,
    },
    Ping,
}

impl ClientMsg {
    /// A `hello` for this build of the protocol and dataset.
    pub fn hello(nickname: impl Into<String>, resume_token: Option<String>) -> Self {
        Self::Hello {
            protocol_version: PROTOCOL_VERSION,
            dataset_fingerprint: boussole_core::dataset_fingerprint(),
            nickname: nickname.into(),
            resume_token,
        }
    }
}

/// Server → client messages.
// A `room_snapshot` embeds the whole room; messages are encoded and sent as they are
// produced and never stored in bulk, so the size difference between variants is fine.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Answer to an accepted `hello`.
    Welcome {
        player_id: PlayerId,
        /// 128-bit random hex (32 characters). Keep it secret: it is the session credential.
        resume_token: String,
        /// `true` if the session was restored (a `room_snapshot` follows if in a room).
        resumed: bool,
    },
    RoomSnapshot(RoomSnapshot),
    RoundStarted(RoundStarted),
    /// Course: to the guesser only. Coopération: to the whole room.
    GuessResult(GuessInfo),
    /// Course: to everyone except the guesser.
    PlayerProgress(Progress),
    /// Flèches Course: broadcast (guesser included) after each accepted answer, so everyone
    /// can see who has answered. Carries **no** option and no correctness.
    PlayerAnswered {
        player_id: PlayerId,
    },
    /// Coopération only.
    TurnChanged {
        player_id: PlayerId,
        remaining_ms: u64,
        reason: TurnReason,
    },
    RoundEnded(RoundEnded),
    MatchEnded(MatchEnded),
    Error {
        code: ErrorCode,
        /// `code.message_fr()`.
        message: String,
    },
    Pong,
}

impl ServerMsg {
    /// An `error` message with the standard French text.
    pub fn error(code: ErrorCode) -> Self {
        Self::Error {
            code,
            message: code.message_fr().to_owned(),
        }
    }
}
