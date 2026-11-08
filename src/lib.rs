//! Boussole: guess the hidden French city from compass arrows to other cities.
//!
//! * Core, GUI-free (the `boussole-core` crate, re-exported here so paths such as
//!   `boussole::game::Round` keep working): [`cities`], [`geo`], [`search`], [`game`].
//! * Presentation logic, still GUI-free and unit-tested: [`board`] (arrow and label geometry,
//!   French texts), [`session`] (Setup → Playing → Ended state machine, autocomplete) and
//!   [`text`] (label measurement).
//! * The Slint front end: [`app`] (glue between a [`session::Session`] and the `ui/*.slint`
//!   files) and [`headless`] (a window without a screen, for screenshots and UI tests).

pub use boussole_core::{cities, game, geo, search};

pub mod app;
pub mod board;
pub mod headless;
pub mod session;
pub mod text;
