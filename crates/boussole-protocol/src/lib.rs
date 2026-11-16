//! Boussole wire protocol: the messages exchanged between the desktop client and the server,
//! their JSON codec and the validation rules both sides share.
//!
//! `docs/protocol.md` is the prose specification; this crate and the golden fixtures in
//! `tests/fixtures/` are its executable form. Transport is one JSON object per WebSocket text
//! frame, tagged by a snake_case `"type"` field.

mod codec;
mod error;
mod hex;
mod msg;
mod types;

pub use codec::{decode_client, decode_server, encode, encode_client};
pub use error::{ErrorCode, ProtocolError};
pub use msg::{ClientMsg, ServerMsg};
pub use types::*;

/// Protocol revision. Bumped on any change a version-1 peer could not understand.
/// Version 2 added the Flèches game (rooms, questions, answers, reveals).
pub const PROTOCOL_VERSION: u16 = 2;

/// Largest client → server frame, in bytes. Longer frames are rejected with `bad_message`
/// (the transport also closes the connection).
pub const MAX_FRAME_BYTES: usize = 4096;

/// Largest server → client frame a client must accept, in bytes (a safety cap; real frames
/// stay far below, see `docs/protocol.md`).
pub const MAX_SERVER_FRAME_BYTES: usize = 65_536;

/// Tries per round (`boussole_core::game::Round::MAX_GUESSES`).
pub const MAX_TRIES: u8 = 6;

/// Number of options (arrows) in a Flèches question
/// (`boussole_core::arrows::OPTION_COUNT`).
pub const FLECHES_OPTIONS: u8 = 4;

/// Room codes: 5 characters from this alphabet (no `I`, `O`, `0`, `1`).
pub const ROOM_CODE_ALPHABET: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
/// Length of a room code.
pub const ROOM_CODE_LEN: usize = 5;

/// Nicknames are 1 to this many Unicode scalar values after trimming.
pub const NICKNAME_MAX_CHARS: usize = 16;

/// How long a disconnected player keeps their slot and can resume, in milliseconds.
pub const RESUME_GRACE_MS: u64 = 60_000;
/// Pause between two rounds of a match, in milliseconds.
pub const INTERMISSION_MS: u64 = 8_000;
/// A room that receives no client message for this long is deleted, in milliseconds.
pub const ROOM_IDLE_MS: u64 = 30 * 60 * 1000;
