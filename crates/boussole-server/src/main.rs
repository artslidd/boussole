//! Standalone Boussole server.
//!
//! Configuration (environment): `BOUSSOLE_BIND` (default `0.0.0.0:7878`),
//! `BOUSSOLE_MAX_CONNECTIONS` (2000), `BOUSSOLE_MAX_ROOMS` (500),
//! `BOUSSOLE_MAX_CONNECTIONS_PER_IP` (20), `BOUSSOLE_TRUST_PROXY_HEADERS` (false), `RUST_LOG`
//! (default `info`), `BOUSSOLE_LOG_FORMAT=json` for JSON logs. Flags: `--tunnel`, `--healthcheck`, `--help`.

use boussole_server::{ServerConfig, lan_ws_urls, start, tunnel};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HELP: &str = "\
boussole-server: multiplayer server for Boussole

USAGE: boussole-server [--tunnel] [--healthcheck]

  --tunnel       also open a free Cloudflare quick tunnel (needs `cloudflared`) and print the
                 public wss:// URL friends can join
  --healthcheck  probe GET /healthz on the configured port and exit 0 if the server is up
                 (used by the container HEALTHCHECK)
  --help         show this text

ENVIRONMENT
  BOUSSOLE_BIND              listen address              (default 0.0.0.0:7878)
  BOUSSOLE_MAX_CONNECTIONS   simultaneous connections    (default 2000)
  BOUSSOLE_MAX_ROOMS         simultaneous rooms           (default 500)
  BOUSSOLE_MAX_CONNECTIONS_PER_IP
                             connections per client address (default 20)
  BOUSSOLE_TRUST_PROXY_HEADERS
                             believe Cf-Connecting-Ip / X-Forwarded-For from any peer, only
                             if the server is reachable through your proxy alone
                             (default false: trusted from loopback peers only)
  RUST_LOG                   log filter                   (default info)
  BOUSSOLE_LOG_FORMAT        `json` for JSON logs         (default human readable)
";

#[tokio::main]
async fn main() -> ExitCode {
    let mut tunnel_flag = false;
    let mut healthcheck = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--tunnel" => tunnel_flag = true,
            "--healthcheck" => healthcheck = true,
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other:?}\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
    }

    let config = match ServerConfig::from_env() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::from(2);
        }
    };
    if healthcheck {
        return probe_healthz(config.bind).await;
    }
    init_logging();

    let server = match start(config).await {
        Ok(server) => server,
        Err(e) => {
            eprintln!("cannot start the server: {e}");
            return ExitCode::FAILURE;
        }
    };
    let port = server.local_addr().port();
    println!("Boussole server listening on {}", server.local_addr());
    println!("  local:  ws://127.0.0.1:{port}/ws");
    for url in lan_ws_urls(port) {
        println!("  LAN:    {url}");
    }

    let mut tunnel = if tunnel_flag {
        match tunnel::start_cloudflared(port).await {
            Ok(t) => {
                println!(
                    "  public: {}  (Cloudflare quick tunnel; the URL changes at every start)",
                    t.public_ws_url()
                );
                Some(t)
            }
            Err(e) => {
                eprintln!("cannot open the tunnel: {e}");
                server.shutdown().await;
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    // Run until told to stop. If the engine or the listener dies on its own the process must
    // not keep looking alive (a supervisor restarts a process that exits), so that is a
    // failure exit. A tunnel that dies only loses the public address: say so, keep serving.
    let mut tunnel_up = tunnel.is_some();
    let exit = loop {
        tokio::select! {
            () = wait_for_stop_signal() => {
                tracing::info!("stop signal received, shutting down");
                break ExitCode::SUCCESS;
            }
            () = server.closed() => {
                tracing::error!("the server stopped on its own (a bug): exiting");
                eprintln!("the server stopped unexpectedly; exiting so that a supervisor can restart it");
                break ExitCode::FAILURE;
            }
            () = async { if let Some(t) = tunnel.as_mut() { t.exited().await } }, if tunnel_up => {
                tunnel_up = false;
                tracing::error!("cloudflared exited: the public address no longer works");
                eprintln!("the tunnel stopped: the public address above no longer works (LAN and local play continue; restart to get a new address)");
            }
        }
    };
    if let Some(t) = tunnel {
        t.stop().await;
    }
    server.shutdown().await;
    exit
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    if std::env::var("BOUSSOLE_LOG_FORMAT").is_ok_and(|v| v.eq_ignore_ascii_case("json")) {
        builder.json().init();
    } else {
        builder.init();
    }
}

/// Ctrl-C everywhere, plus SIGTERM (what `docker stop` sends) on Unix.
async fn wait_for_stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
                return;
            }
            Err(e) => eprintln!("cannot listen for SIGTERM: {e}"),
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// `--healthcheck`: a bare HTTP/1.0 GET so the distroless image needs no curl.
async fn probe_healthz(bind: SocketAddr) -> ExitCode {
    let ip = if bind.ip().is_unspecified() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        bind.ip()
    };
    let target = SocketAddr::new(ip, bind.port());
    let probe = async {
        let mut stream = tokio::net::TcpStream::connect(target).await?;
        stream
            .write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await?;
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).await?;
        Ok::<_, std::io::Error>(reply)
    };
    match tokio::time::timeout(Duration::from_secs(3), probe).await {
        Ok(Ok(reply))
            if reply.starts_with(b"HTTP/1.1 200") || reply.starts_with(b"HTTP/1.0 200") =>
        {
            ExitCode::SUCCESS
        }
        Ok(Ok(_)) => {
            eprintln!("healthcheck: {target} answered, but not 200");
            ExitCode::FAILURE
        }
        Ok(Err(e)) => {
            eprintln!("healthcheck: {target}: {e}");
            ExitCode::FAILURE
        }
        Err(_) => {
            eprintln!("healthcheck: {target}: timeout");
            ExitCode::FAILURE
        }
    }
}
