//! The real [`Host`]: the Boussole server embedded in the app, and its optional Cloudflare
//! quick tunnel.
//!
//! `boussole_server::start` spawns on the *current* tokio runtime, so the host owns a small
//! multi-thread runtime of its own: it keeps serving between [`Host::start`] and [`Host::stop`],
//! which are plain blocking calls (the UI makes them from worker threads). Created on the first
//! start, so a player who only plays solo never pays for it.

use crate::host::{Host, HostError, HostHealth, HostInfo, HostMode};
use boussole_server::tunnel::{self, Tunnel, TunnelError};
use boussole_server::{ServerConfig, ServerHandle, lan_ws_urls, start};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::watch;

/// The port a local-network game tries first, so friends can type a short address.
pub const PREFERRED_LAN_PORT: u16 = 7878;

/// Connections per visitor address: the host's own client and a few friends (or windows) on the
/// same machine count against it. The whole server takes 16.
const MAX_CONNECTIONS_PER_IP: usize = 8;

/// Longest the server is given to say goodbye before the tunnel is killed regardless (the server
/// itself gives up after 5 s). The app waits longer than this at exit, see `online_ui::EXIT_WAIT`.
const SERVER_SHUTDOWN_BOUND: Duration = Duration::from_secs(6);

/// Longest a loopback connection is given to tell whether something listens (it answers or is
/// refused at once).
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

type TunnelFuture = Pin<Box<dyn Future<Output = Result<Tunnel, TunnelError>> + Send>>;
type TunnelStarter = Arc<dyn Fn(u16) -> TunnelFuture + Send + Sync>;

/// What runs while hosting.
struct Running {
    server: ServerHandle,
    tunnel: Option<Tunnel>,
}

/// The embedded server (and tunnel) behind "Héberger une partie".
pub struct EmbeddedHost {
    /// Where a local-network game tries to listen first.
    lan_bind: SocketAddr,
    tunnel_starter: TunnelStarter,
    runtime: OnceLock<tokio::runtime::Runtime>,
    running: Mutex<Option<Running>>,
    /// Held for the whole of a `start` or `stop`: they never interleave, so a slow start can
    /// never overwrite (and thereby kill) a server started after it, nor be stopped by it.
    lifecycle: Mutex<()>,
    /// Bumped by [`Host::cancel_start`]; a start in flight watches it.
    cancel: watch::Sender<u64>,
}

impl EmbeddedHost {
    /// The host the app uses: local-network games try `0.0.0.0:7878`, then any free port;
    /// tunnels run the real `cloudflared`.
    pub fn new() -> Self {
        Self::with_lan_bind(SocketAddr::from(([0, 0, 0, 0], PREFERRED_LAN_PORT)))
    }

    /// Like [`EmbeddedHost::new`] with another address to try first for local-network games
    /// (tests use `127.0.0.1:0`). When it is taken, a free port on the same interface is used.
    pub fn with_lan_bind(lan_bind: SocketAddr) -> Self {
        Self {
            lan_bind,
            tunnel_starter: Arc::new(|port| Box::pin(tunnel::start_cloudflared(port))),
            runtime: OnceLock::new(),
            running: Mutex::new(None),
            lifecycle: Mutex::new(()),
            cancel: watch::channel(0).0,
        }
    }

    /// A host whose tunnels are started by `starter` instead of the real `cloudflared`: tests
    /// use it to exercise the tunnel path without ever opening a public address.
    pub fn with_tunnel_starter(
        starter: impl Fn(u16) -> TunnelFuture + Send + Sync + 'static,
    ) -> Self {
        let mut host = Self::with_lan_bind(SocketAddr::from(([127, 0, 0, 1], 0)));
        host.tunnel_starter = Arc::new(starter);
        host
    }

    /// Is a server (and maybe a tunnel) started?
    pub fn is_running(&self) -> bool {
        self.running.lock().is_ok_and(|running| running.is_some())
    }

    fn runtime(&self) -> Result<&tokio::runtime::Runtime, HostError> {
        if let Some(runtime) = self.runtime.get() {
            return Ok(runtime);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("boussole-host")
            .enable_all()
            .build()
            .map_err(|error| HostError::Other(error.to_string()))?;
        Ok(self.runtime.get_or_init(|| runtime))
    }

    async fn launch(
        &self,
        mode: HostMode,
        cancel: &mut watch::Receiver<u64>,
    ) -> Result<(Running, HostInfo), HostError> {
        match mode {
            HostMode::Lan => {
                let server = self.start_lan().await?;
                let port = server.local_addr().port();
                let info = HostInfo {
                    port,
                    local_url: format!("ws://127.0.0.1:{port}/ws"),
                    lan_urls: lan_ws_urls(port),
                    tunnel_url: None,
                };
                Ok((
                    Running {
                        server,
                        tunnel: None,
                    },
                    info,
                ))
            }
            HostMode::Tunnel => {
                // Loopback is enough behind a tunnel (and avoids the macOS firewall prompt).
                let server = start(host_config(SocketAddr::from(([127, 0, 0, 1], 0))))
                    .await
                    .map_err(map_io)?;
                let port = server.local_addr().port();
                // Opening the tunnel can take 20 s: it is the step that can be abandoned. Dropping
                // the starter's future kills the `cloudflared` child it spawned.
                let tunnel = tokio::select! {
                    biased;
                    _ = cancel.changed() => None,
                    result = (self.tunnel_starter)(port) => Some(result),
                };
                let Some(tunnel) = tunnel else {
                    server.shutdown().await;
                    return Err(HostError::Cancelled);
                };
                match tunnel {
                    Ok(tunnel) => {
                        let info = HostInfo {
                            port,
                            local_url: format!("ws://127.0.0.1:{port}/ws"),
                            lan_urls: Vec::new(),
                            tunnel_url: Some(tunnel.public_ws_url().to_owned()),
                        };
                        Ok((
                            Running {
                                server,
                                tunnel: Some(tunnel),
                            },
                            info,
                        ))
                    }
                    Err(error) => {
                        // No tunnel, no game: do not leave a server nobody can reach running.
                        server.shutdown().await;
                        Err(map_tunnel_error(error))
                    }
                }
            }
        }
    }

    async fn start_lan(&self) -> Result<ServerHandle, HostError> {
        // On macOS a wildcard bind succeeds next to a program that listens on the loopback
        // address of the same port, and the host's own client (and anyone on this machine)
        // would then reach that program instead of the game: so anything that answers on the
        // loopback address means the preferred port is not ours to use.
        if self.lan_bind.port() != 0 && something_answers_on_loopback(self.lan_bind.port()).await {
            return start(host_config(SocketAddr::new(self.lan_bind.ip(), 0)))
                .await
                .map_err(map_io);
        }
        match start(host_config(self.lan_bind)).await {
            Ok(server) => Ok(server),
            // The preferred port is taken (another game, another program): any free one will do,
            // the lobby shows the address to share anyway.
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                start(host_config(SocketAddr::new(self.lan_bind.ip(), 0)))
                    .await
                    .map_err(map_io)
            }
            Err(error) => Err(map_io(error)),
        }
    }
}

impl Default for EmbeddedHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EmbeddedHost {
    fn drop(&mut self) {
        self.stop();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl EmbeddedHost {
    /// Stop what runs. The caller holds the lifecycle lock.
    fn stop_locked(&self) {
        let Some(Running { server, tunnel }) = self.running.lock().unwrap().take() else {
            return;
        };
        let Some(runtime) = self.runtime.get() else {
            return;
        };
        runtime.block_on(async move {
            // The server first: its goodbye (close 1001) to every player still goes through the
            // tunnel. Bounded, so that the tunnel is killed even if the server is slow.
            let _ = tokio::time::timeout(SERVER_SHUTDOWN_BOUND, server.shutdown()).await;
            if let Some(tunnel) = tunnel {
                tunnel.stop().await;
            }
        });
    }
}

impl Host for EmbeddedHost {
    fn start(&self, mode: HostMode) -> Result<HostInfo, HostError> {
        let _lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        // Watch for a cancellation from now on (one that came earlier belongs to a start that is
        // over).
        let mut cancel = self.cancel.subscribe();
        self.stop_locked(); // a second start replaces the first
        let runtime = self.runtime()?;
        let (running, info) = runtime.block_on(self.launch(mode, &mut cancel))?;
        *self.running.lock().unwrap() = Some(running);
        Ok(info)
    }

    fn stop(&self) {
        let _lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        self.stop_locked();
    }

    fn cancel_start(&self) {
        self.cancel.send_modify(|generation| *generation += 1);
    }

    fn health(&self) -> HostHealth {
        // Never block the UI thread: while a start or stop holds the lock, nothing is wrong.
        let Ok(mut guard) = self.running.try_lock() else {
            return HostHealth::Running;
        };
        match guard.as_mut() {
            Some(running) => health_of(
                running.server.is_running(),
                running.tunnel.as_mut().map(Tunnel::is_running),
            ),
            None => HostHealth::Running,
        }
    }
}

/// Does a connection to `127.0.0.1:port` succeed? Nothing is sent: the connection is dropped at
/// once. A refusal, a timeout or any other error counts as "nothing answers" (a bind will still
/// report a port that is really taken).
async fn something_answers_on_loopback(port: u16) -> bool {
    let connect = tokio::net::TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port)));
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, connect).await,
        Ok(Ok(_))
    )
}

/// What is wrong, given whether the server runs and (if there is a tunnel) whether it does.
/// A dead server ends the game whatever the tunnel does; a dead tunnel only kills the address.
pub fn health_of(server_running: bool, tunnel_running: Option<bool>) -> HostHealth {
    if !server_running {
        HostHealth::ServerStopped
    } else if tunnel_running == Some(false) {
        HostHealth::TunnelLost
    } else {
        HostHealth::Running
    }
}

fn host_config(bind: SocketAddr) -> ServerConfig {
    ServerConfig {
        max_connections_per_ip: MAX_CONNECTIONS_PER_IP,
        ..ServerConfig::for_host_mode(bind)
    }
}

fn map_io(error: io::Error) -> HostError {
    if error.kind() == io::ErrorKind::AddrInUse {
        HostError::PortInUse
    } else {
        HostError::Other(error.to_string())
    }
}

fn map_tunnel_error(error: TunnelError) -> HostError {
    match error {
        TunnelError::NotInstalled => HostError::TunnelNotInstalled,
        TunnelError::StartFailed(why) => HostError::TunnelFailed(why),
        TunnelError::Timeout => HostError::TunnelFailed("délai dépassé".to_owned()),
    }
}
