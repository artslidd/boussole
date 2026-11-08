//! Boussole multiplayer server.
//!
//! * [`engine`]: the pure, IO-free match engine (rooms, Course and Coopération rules, timers,
//!   scoring, session resumption), specified in `docs/protocol.md`.
//! * [`start`] / [`ServerHandle`]: the network server around it (axum, WebSocket at `/ws`,
//!   `GET /healthz`, `GET /version`). It runs on the *current* tokio runtime, so the same code
//!   serves the standalone binary and a game embedded in the desktop client ("Héberger une
//!   partie").
//! * [`lan_ws_urls`] and [`tunnel`]: how a host tells friends where to connect: LAN
//!   addresses, or a free Cloudflare quick tunnel.
//!
//! ```no_run
//! # async fn host() -> Result<(), Box<dyn std::error::Error>> {
//! use boussole_server::{ServerConfig, lan_ws_urls, start, tunnel};
//!
//! let server = start(ServerConfig::for_host_mode("127.0.0.1:0".parse()?)).await?;
//! let port = server.local_addr().port();
//! // The host's own client connects to ws://127.0.0.1:{port}/ws ...
//! let public = tunnel::start_cloudflared(port).await?; // friends outside the LAN
//! println!("share: {}", public.public_ws_url());
//! public.stop().await;
//! server.shutdown().await;
//! # Ok(()) }
//! ```

mod config;
mod conn;
mod driver;
pub mod engine;
mod entropy;
mod lan;
mod limit;
mod server;
pub mod tunnel;

pub use config::{ConfigError, DEFAULT_BIND, ServerConfig};
pub use entropy::OsEntropy;
pub use lan::lan_ws_urls;
pub use server::{ServerHandle, start};
