//! Online mode, GUI-free: the pure reducer ([`state`]), the French texts ([`text`]) and the
//! view-model each screen shows ([`view`]).

pub mod state;
pub mod text;
pub mod view;

pub use state::{
    Clock, Link, MatchEnd, Notice, NoticeKind, OnlineState, RESULT_BEFORE_RANKING, Room,
    RoundResult, RoundState, apply, set_link,
};
