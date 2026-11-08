//! Boussole multiplayer server.
//!
//! For now this crate holds only the **pure match engine** ([`engine`]): synchronous,
//! deterministic and free of any IO. Networking (tokio/axum, `main.rs`) is added by a later
//! ticket and drives the engine through [`engine::Engine::handle_frame`],
//! [`engine::Engine::disconnect`] and [`engine::Engine::tick`].

pub mod engine;
