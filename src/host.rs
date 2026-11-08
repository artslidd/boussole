//! Hosting a game from the app: the embedded server and its optional tunnel, behind a small
//! trait so the UI and its tests do not depend on the server crate.
//!
//! [`FakeHost`] stands in for the real implementation (tests, screenshots); [`UnavailableHost`]
//! is what the binary uses until the real embedded server is wired in. Nothing here ever starts
//! a public tunnel by itself.

use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Where the hosted game can be reached from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostMode {
    /// On the local network: bind every interface (`0.0.0.0:7878`, or a free port).
    Lan,
    /// Over the Internet through a free Cloudflare quick tunnel (loopback only + `cloudflared`).
    Tunnel,
}

/// An address the host shares with friends, with what it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareAddress {
    /// "Internet" / "Réseau local".
    pub label: String,
    pub url: String,
}

/// A running hosted server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    pub port: u16,
    /// The address the host's own client connects to (always loopback).
    pub local_url: String,
    /// `ws://<ip>:<port>/ws` for every network interface (LAN mode).
    pub lan_urls: Vec<String>,
    /// `wss://….trycloudflare.com/ws` (Tunnel mode).
    pub tunnel_url: Option<String>,
}

impl HostInfo {
    /// What to show under "Adresse à partager", most useful first: the tunnel, then every LAN
    /// address.
    pub fn share_addresses(&self) -> Vec<ShareAddress> {
        let mut out = Vec::new();
        if let Some(url) = &self.tunnel_url {
            out.push(ShareAddress {
                label: "Internet".to_owned(),
                url: url.clone(),
            });
        }
        for url in &self.lan_urls {
            out.push(ShareAddress {
                label: "Réseau local".to_owned(),
                url: url.clone(),
            });
        }
        out
    }
}

/// Why hosting could not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// The port is taken (another server, another game).
    PortInUse,
    /// `cloudflared` is not installed (or not found).
    TunnelNotInstalled,
    /// `cloudflared` started but gave no usable address (carries the technical reason).
    TunnelFailed(String),
    /// Anything else (carries the technical reason).
    Other(String),
}

impl fmt::Display for HostError {
    /// The French message for the player.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PortInUse => f.write_str(
                "Le port est déjà utilisé : fermez l’autre partie ou le programme qui l’occupe, puis réessayez.",
            ),
            Self::TunnelNotInstalled => f.write_str("Le tunnel gratuit a besoin de cloudflared, qui n’est pas installé."),
            Self::TunnelFailed(_) => f.write_str(
                "Tunnel indisponible : cloudflared n’a pas donné d’adresse. Réessayez, ou jouez en réseau local.",
            ),
            Self::Other(why) => write!(f, "Impossible de démarrer la partie : {why}"),
        }
    }
}

impl std::error::Error for HostError {}

/// Starts and stops the embedded server (and its tunnel). Both calls may block for a few
/// seconds (binding, waiting for the tunnel address, shutting down): the UI calls them from a
/// worker thread, never from the UI thread.
pub trait Host: Send + Sync {
    /// Start the server (and the tunnel in [`HostMode::Tunnel`]). Starting while already running
    /// replaces the previous server.
    fn start(&self, mode: HostMode) -> Result<HostInfo, HostError>;

    /// Shut the tunnel and the server down cleanly and release the port. Harmless when nothing
    /// runs.
    fn stop(&self);
}

/// The host of binaries that cannot host (yet): always refuses, honestly.
#[derive(Debug, Default)]
pub struct UnavailableHost;

impl Host for UnavailableHost {
    fn start(&self, _mode: HostMode) -> Result<HostInfo, HostError> {
        Err(HostError::Other(
            "l’hébergement n’est pas disponible dans cette version.".to_owned(),
        ))
    }

    fn stop(&self) {}
}

/// A scripted host for tests and screenshots: returns what it was told to, and counts calls.
/// It starts no server and no process; its `local_url` points wherever the test wants (a mock
/// WebSocket server).
pub struct FakeHost {
    lan: Mutex<Result<HostInfo, HostError>>,
    tunnel: Mutex<Result<HostInfo, HostError>>,
    started: Mutex<Vec<HostMode>>,
    stops: AtomicUsize,
}

impl FakeHost {
    /// Both modes answer with `info` (the tunnel one with its tunnel URL, if it has one).
    pub fn ok(info: HostInfo) -> Self {
        Self {
            lan: Mutex::new(Ok(info.clone())),
            tunnel: Mutex::new(Ok(info)),
            started: Mutex::new(Vec::new()),
            stops: AtomicUsize::new(0),
        }
    }

    /// Both modes fail with `error`.
    pub fn failing(error: HostError) -> Self {
        Self {
            lan: Mutex::new(Err(error.clone())),
            tunnel: Mutex::new(Err(error)),
            started: Mutex::new(Vec::new()),
            stops: AtomicUsize::new(0),
        }
    }

    /// Change what the Tunnel mode answers.
    pub fn with_tunnel(self, result: Result<HostInfo, HostError>) -> Self {
        *self.tunnel.lock().unwrap() = result;
        self
    }

    /// Change what the LAN mode answers.
    pub fn with_lan(self, result: Result<HostInfo, HostError>) -> Self {
        *self.lan.lock().unwrap() = result;
        self
    }

    /// Every `start` so far, oldest first.
    pub fn starts(&self) -> Vec<HostMode> {
        self.started.lock().unwrap().clone()
    }

    /// How many times `stop` was called.
    pub fn stops(&self) -> usize {
        self.stops.load(Ordering::SeqCst)
    }
}

impl Host for FakeHost {
    fn start(&self, mode: HostMode) -> Result<HostInfo, HostError> {
        self.started.lock().unwrap().push(mode);
        match mode {
            HostMode::Lan => self.lan.lock().unwrap().clone(),
            HostMode::Tunnel => self.tunnel.lock().unwrap().clone(),
        }
    }

    fn stop(&self) {
        self.stops.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> HostInfo {
        HostInfo {
            port: 7878,
            local_url: "ws://127.0.0.1:7878/ws".into(),
            lan_urls: vec![
                "ws://192.168.1.20:7878/ws".into(),
                "ws://10.0.0.4:7878/ws".into(),
            ],
            tunnel_url: Some("wss://abc-def.trycloudflare.com/ws".into()),
        }
    }

    #[test]
    fn the_tunnel_address_comes_first_then_every_lan_address() {
        let shared = info().share_addresses();
        assert_eq!(shared.len(), 3);
        assert_eq!(shared[0].label, "Internet");
        assert_eq!(shared[0].url, "wss://abc-def.trycloudflare.com/ws");
        assert_eq!(shared[1].label, "Réseau local");
        assert_eq!(shared[2].url, "ws://10.0.0.4:7878/ws");
        let lan_only = HostInfo {
            tunnel_url: None,
            ..info()
        };
        assert_eq!(lan_only.share_addresses().len(), 2);
    }

    #[test]
    fn every_host_error_has_a_french_message_without_technical_noise() {
        let port = HostError::PortInUse.to_string();
        assert!(port.starts_with("Le port est déjà utilisé"), "{port}");
        assert!(
            HostError::TunnelNotInstalled
                .to_string()
                .contains("cloudflared")
        );
        let failed = HostError::TunnelFailed("exit status: 1".into()).to_string();
        assert!(failed.starts_with("Tunnel indisponible"), "{failed}");
        assert!(
            !failed.contains("exit status"),
            "technical detail stays out of the UI"
        );
    }

    #[test]
    fn the_fake_host_counts_starts_and_stops_and_answers_per_mode() {
        let host = FakeHost::ok(info()).with_tunnel(Err(HostError::TunnelNotInstalled));
        assert!(host.start(HostMode::Lan).is_ok());
        assert_eq!(
            host.start(HostMode::Tunnel),
            Err(HostError::TunnelNotInstalled)
        );
        assert_eq!(host.starts(), [HostMode::Lan, HostMode::Tunnel]);
        host.stop();
        host.stop();
        assert_eq!(host.stops(), 2);
    }

    #[test]
    fn the_unavailable_host_refuses_with_a_reason() {
        let error = UnavailableHost.start(HostMode::Lan).unwrap_err();
        assert!(error.to_string().contains("pas disponible"), "{error}");
    }
}
