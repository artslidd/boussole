//! Networking for the online mode: address handling and the background WebSocket client.

pub mod address;
mod client;

use crate::online::text;
use boussole_protocol::ErrorCode;
use std::fmt;

pub use address::{AddressError, DEFAULT_ADDRESS, normalize_address};
pub use client::{NetConfig, NetEvent, NetHandle};

/// Why a connection failed for good.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    /// The socket could not be opened (nothing listening, DNS, TLS, timeout). Carries the
    /// technical reason, which is not shown to the player.
    Unreachable(String),
    /// The server answered `hello` with an error: wrong version, other city list, bad nickname.
    Rejected(ErrorCode),
    /// The server's first answer was not `welcome` or `error`.
    Protocol,
    /// The connection dropped and could not be restored within the reconnect window.
    ConnectionLost,
}

impl fmt::Display for NetError {
    /// The French message for the player.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(_) => f.write_str(text::SERVER_UNREACHABLE),
            Self::Rejected(code) => f.write_str(&text::error_fr(*code)),
            Self::Protocol => f.write_str("Réponse inattendue : ce n’est pas un serveur Boussole."),
            Self::ConnectionLost => f.write_str(text::CONNECTION_LOST),
        }
    }
}

impl std::error::Error for NetError {}
