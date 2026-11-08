//! Boussole core: UI-independent data, geometry and round rules.
//!
//! No GUI types live here; the egui front end (ticket 02) is built on this API.

pub mod cities;
pub mod game;
pub mod geo;
pub mod search;
