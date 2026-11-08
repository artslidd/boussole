use serde::{Deserialize, Serialize};
use std::fmt;

/// Machine-readable error identifiers (`ServerMsg::Error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadVersion,
    DatasetMismatch,
    BadMessage,
    InvalidState,
    InvalidNickname,
    NicknameTaken,
    RoomNotFound,
    RoomFull,
    MatchInProgress,
    NotHost,
    NotEnoughPlayers,
    InvalidConfig,
    NotYourTurn,
    UnknownCity,
    AlreadyGuessed,
    RoundOver,
    RateLimited,
}

impl ErrorCode {
    /// Every code, in wire order.
    pub const ALL: [ErrorCode; 17] = [
        Self::BadVersion,
        Self::DatasetMismatch,
        Self::BadMessage,
        Self::InvalidState,
        Self::InvalidNickname,
        Self::NicknameTaken,
        Self::RoomNotFound,
        Self::RoomFull,
        Self::MatchInProgress,
        Self::NotHost,
        Self::NotEnoughPlayers,
        Self::InvalidConfig,
        Self::NotYourTurn,
        Self::UnknownCity,
        Self::AlreadyGuessed,
        Self::RoundOver,
        Self::RateLimited,
    ];

    /// Message shown to the player, in French.
    pub fn message_fr(self) -> &'static str {
        match self {
            Self::BadVersion => "Version du protocole incompatible : mettez le jeu à jour.",
            Self::DatasetMismatch => {
                "La liste des villes du client ne correspond pas à celle du serveur."
            }
            Self::BadMessage => "Message illisible.",
            Self::InvalidState => "Action impossible à ce moment.",
            Self::InvalidNickname => {
                "Pseudo invalide : 1 à 16 caractères, sans caractère de contrôle."
            }
            Self::NicknameTaken => "Ce pseudo est déjà pris dans cette salle.",
            Self::RoomNotFound => "Salle introuvable.",
            Self::RoomFull => "Cette salle est complète.",
            Self::MatchInProgress => "Une partie est déjà en cours dans cette salle.",
            Self::NotHost => "Seul l’hôte de la salle peut faire cela.",
            Self::NotEnoughPlayers => "Il faut au moins 2 joueurs connectés pour commencer.",
            Self::InvalidConfig => "Réglages de la salle invalides.",
            Self::NotYourTurn => "Ce n’est pas votre tour.",
            Self::UnknownCity => "Ville inconnue.",
            Self::AlreadyGuessed => "Cette ville a déjà été proposée.",
            Self::RoundOver => "La manche est terminée.",
            Self::RateLimited => "Trop de messages : ralentissez.",
        }
    }
}

/// Why a frame could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// The frame is longer than the allowed maximum.
    FrameTooLarge { len: usize, max: usize },
    /// Not valid JSON, unknown `type`, or fields of the wrong shape.
    Malformed(String),
}

impl ProtocolError {
    /// The error code to answer with: always [`ErrorCode::BadMessage`].
    pub fn code(&self) -> ErrorCode {
        ErrorCode::BadMessage
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooLarge { len, max } => write!(f, "frame of {len} bytes exceeds {max}"),
            Self::Malformed(why) => write!(f, "malformed message: {why}"),
        }
    }
}

impl std::error::Error for ProtocolError {}
