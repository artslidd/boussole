//! Boussole: guess the hidden French city from compass arrows to other cities.
//!
//! * Core, GUI-free: [`cities`], [`geo`], [`search`], [`game`].
//! * Presentation logic, still GUI-free and unit-tested: [`board`] (arrow and label geometry,
//!   French texts), [`session`] (Setup → Playing → Ended state machine, autocomplete) and
//!   [`text`] (label measurement).
//! * The Slint front end: [`app`] (glue between a [`session::Session`] and the `ui/*.slint`
//!   files) and [`headless`] (a window without a screen, for screenshots and UI tests).

pub mod app;
pub mod board;
pub mod cities;
pub mod game;
pub mod geo;
pub mod headless;
pub mod search;
pub mod session;
pub mod text;
