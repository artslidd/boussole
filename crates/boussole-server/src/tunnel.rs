//! Free public tunnel for a host without a server: a Cloudflare *quick tunnel* started with
//! the `cloudflared` binary (no account needed).
//!
//! `cloudflared tunnel --url http://127.0.0.1:<port>` prints a random
//! `https://<words>.trycloudflare.com` URL on stderr and forwards it to the local port;
//! WebSockets work through it, so friends join with `wss://<words>.trycloudflare.com/ws`.
//! Quick tunnels come with no uptime guarantee and a new URL on every start.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::Instant;

/// How long `cloudflared` gets to print its URL.
const START_TIMEOUT: Duration = Duration::from_secs(20);
/// After `cloudflared` closes its output, how long it may take to actually exit.
const EXIT_GRACE: Duration = Duration::from_secs(2);
/// Lines of cloudflared output kept to explain a failed start.
const TAIL_LINES: usize = 6;

/// A running `cloudflared` quick tunnel. The child process is killed by [`Tunnel::stop`] and,
/// as a safety net, when the `Tunnel` is dropped.
#[derive(Debug)]
pub struct Tunnel {
    child: Child,
    ws_url: String,
    _config: EmptyConfig,
}

/// An empty cloudflared configuration file, deleted when dropped.
///
/// Quick tunnels refuse to run next to a user's own cloudflared configuration (`proxy-dns`,
/// named tunnels, ... in `/etc/cloudflared` or `~/.cloudflared`: "running a quick tunnel with
/// `proxy-dns` is not supported"), so `--config` points at this empty file instead.
#[derive(Debug)]
struct EmptyConfig(PathBuf);

impl EmptyConfig {
    fn create() -> std::io::Result<Self> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        Self::create_with(|| {
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            std::env::temp_dir().join(format!(
                "boussole-cloudflared-{}-{n}.yml",
                std::process::id()
            ))
        })
    }

    /// Create the file under the first name `next_path` offers that is free.
    fn create_with(mut next_path: impl FnMut() -> PathBuf) -> std::io::Result<Self> {
        use std::io::Write;
        // `create_new` (O_EXCL) never follows a symlink or overwrites a file someone else put
        // at a predictable name in a shared temp directory; mode 0600 on Unix.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut attempts = 0;
        loop {
            let path = next_path();
            match options.open(&path) {
                Ok(mut file) => {
                    // A guard from here on: the file must not outlive a failed write.
                    let config = Self(path);
                    file.write_all(
                        b"# intentionally empty: a quick tunnel needs no configuration\n",
                    )?;
                    return Ok(config);
                }
                // A leftover of a crashed run, or someone squatting the name: try the next.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempts < 8 => {
                    attempts += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for EmptyConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl Tunnel {
    /// `wss://<words>.trycloudflare.com/ws`, the address to share with friends.
    ///
    /// It is returned as soon as `cloudflared` prints it, which can be a few seconds before
    /// the address is reachable (cloudflared says "it may take some time"; the live runs
    /// answered on the first probe). A UI should say "share it in a few seconds", not "ready".
    pub fn public_ws_url(&self) -> &str {
        &self.ws_url
    }

    /// `false` once `cloudflared` has exited (crash, lost network, killed): the address from
    /// [`Tunnel::public_ws_url`] is dead from then on and nothing restarts it.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Resolves when `cloudflared` has exited (returns at once if it already has).
    pub async fn exited(&mut self) {
        let _ = self.child.wait().await;
    }

    /// Stop `cloudflared` and wait for it to exit.
    pub async fn stop(mut self) {
        // An error here means the process already exited, which is what we want.
        let _ = self.child.kill().await;
    }
}

/// Why a tunnel could not be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelError {
    /// No `cloudflared` executable on `PATH` (`brew install cloudflared`, or see
    /// <https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/>).
    NotInstalled,
    /// It started but failed or could not be run; carries the reason and its last output.
    StartFailed(String),
    /// It ran for about 20 s without printing a public URL.
    Timeout,
}

impl fmt::Display for TunnelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => f.write_str(
                "cloudflared is not installed (macOS: `brew install cloudflared`; other systems: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/)",
            ),
            Self::StartFailed(why) => write!(f, "cloudflared failed to start: {why}"),
            Self::Timeout => f.write_str("cloudflared did not print a public URL within 20 seconds"),
        }
    }
}

impl std::error::Error for TunnelError {}

/// Run `cloudflared tunnel --no-autoupdate --config <empty file> --url
/// http://127.0.0.1:<local_port>` and wait for its public URL (about 20 s at most). The
/// server must listen on `127.0.0.1:<local_port>` (or `0.0.0.0`).
pub async fn start_cloudflared(local_port: u16) -> Result<Tunnel, TunnelError> {
    launch(OsStr::new("cloudflared"), None, local_port, START_TIMEOUT).await
}

/// [`start_cloudflared`] with the program, its `PATH` and the timeout injectable for tests.
pub(crate) async fn launch(
    program: &OsStr,
    path: Option<OsString>,
    local_port: u16,
    start_timeout: Duration,
) -> Result<Tunnel, TunnelError> {
    let config = EmptyConfig::create().map_err(|e| {
        TunnelError::StartFailed(format!("cannot write a temporary cloudflared config: {e}"))
    })?;
    let mut cmd = Command::new(program);
    cmd.args(["tunnel", "--no-autoupdate", "--config"])
        .arg(&config.0)
        .arg("--url")
        .arg(format!("http://127.0.0.1:{local_port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => TunnelError::NotInstalled,
        _ => TunnelError::StartFailed(e.to_string()),
    })?;

    // cloudflared logs to stderr; read both, and keep draining after the URL is found so the
    // child never blocks on a full pipe.
    let (lines_tx, mut lines) = mpsc::unbounded_channel::<String>();
    if let Some(out) = child.stdout.take() {
        tokio::spawn(forward_lines(out, lines_tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(forward_lines(err, lines_tx.clone()));
    }
    drop(lines_tx);

    let deadline = Instant::now() + start_timeout;
    let mut tail: Vec<String> = Vec::new();
    loop {
        tokio::select! {
            line = lines.recv() => match line {
                Some(line) => {
                    if let Some(https) = parse_quick_tunnel_url(&line) {
                        let host = https.trim_start_matches("https://");
                        return Ok(Tunnel { child, ws_url: format!("wss://{host}/ws"), _config: config });
                    }
                    if tail.len() == TAIL_LINES {
                        tail.remove(0);
                    }
                    tail.push(line);
                }
                // Both pipes closed: the process is exiting, or has closed its output and
                // carries on; do not wait for it forever.
                None => {
                    let output = tail.join(" | ");
                    return Err(TunnelError::StartFailed(
                        match tokio::time::timeout(EXIT_GRACE, child.wait()).await {
                            Ok(status) => format!(
                                "cloudflared exited ({}) before printing a URL; last output: {output}",
                                status.map_or_else(|e| e.to_string(), |s| s.to_string())
                            ),
                            Err(_) => {
                                let _ = child.kill().await;
                                format!("cloudflared closed its output without printing a URL; last output: {output}")
                            }
                        },
                    ));
                }
            },
            () = tokio::time::sleep_until(deadline) => {
                let _ = child.kill().await;
                return Err(TunnelError::Timeout);
            }
        }
    }
}

async fn forward_lines(stream: impl AsyncRead + Unpin, tx: mpsc::UnboundedSender<String>) {
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "cloudflared", "{line}");
        let _ = tx.send(line); // no listener after startup: just keep draining
    }
}

/// Find `https://<words>.trycloudflare.com` in a line of `cloudflared` output.
///
/// `api.trycloudflare.com` is excluded: it is the service cloudflared *talks to* and shows up
/// in its error messages ("failed to request quick Tunnel: Post https://api.trycloudflare.com…").
pub(crate) fn parse_quick_tunnel_url(line: &str) -> Option<String> {
    const SCHEME: &str = "https://";
    const DOMAIN: &str = ".trycloudflare.com";
    let mut rest = line;
    while let Some(at) = rest.find(SCHEME) {
        let after = &rest[at + SCHEME.len()..];
        let host_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
            .unwrap_or(after.len());
        let host = &after[..host_len];
        if let Some(label) = host.strip_suffix(DOMAIN)
            && !label.is_empty()
            && label != "api"
            && !label.contains('.')
        {
            return Some(format!("{SCHEME}{host}"));
        }
        rest = after;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quick-tunnel output as `cloudflared` prints it (modelled on 2026.1; the words in the URL
    /// are made up). The live run in the ticket log parsed the real output the same way.
    const SAMPLE: &str = "\
2026-11-08T17:00:00Z INF Thank you for trying Cloudflare Tunnel. Doing so, without a Cloudflare account, is a quick way to experiment and try it out. However, be aware that these account-less Tunnels have no uptime guarantee, are subject to the Cloudflare Online Services Terms of Use (https://www.cloudflare.com/website-terms/), and Cloudflare reserves the right to investigate your use of Tunnels for violations of such terms.
2026-11-08T17:00:00Z INF Requesting new quick Tunnel on trycloudflare.com...
2026-11-08T17:00:02Z INF +--------------------------------------------------------------------------------------------+
2026-11-08T17:00:02Z INF |  Your quick Tunnel has been created! Visit it at (it may take some time to be reachable):  |
2026-11-08T17:00:02Z INF |  https://foo-bar-baz-qux.trycloudflare.com                                                 |
2026-11-08T17:00:02Z INF +--------------------------------------------------------------------------------------------+";

    #[test]
    fn finds_the_url_in_cloudflared_output_and_only_there() {
        let urls: Vec<_> = SAMPLE.lines().filter_map(parse_quick_tunnel_url).collect();
        assert_eq!(urls, ["https://foo-bar-baz-qux.trycloudflare.com"]);
    }

    #[test]
    fn ignores_other_urls_and_the_api_host() {
        for line in [
            "",
            "INF Requesting new quick Tunnel on trycloudflare.com...",
            "ERR failed to request quick Tunnel: Post \"https://api.trycloudflare.com/tunnel\": EOF",
            "INF see https://www.cloudflare.com/website-terms/ for details",
            "https://trycloudflare.com",
            "https://.trycloudflare.com",
            "http://foo.trycloudflare.com",
            "https://a.b.trycloudflare.com",
        ] {
            assert_eq!(parse_quick_tunnel_url(line), None, "{line:?}");
        }
        assert_eq!(
            parse_quick_tunnel_url(
                "x https://www.cloudflare.com y https://ab-cd.trycloudflare.com|"
            ),
            Some("https://ab-cd.trycloudflare.com".to_owned()),
        );
    }

    // ---- process handling, with a fake `cloudflared` shell script ---------------------------
    #[cfg(unix)]
    mod process {
        use super::super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;

        /// Writing then exec'ing a script while another thread forks can fail with ETXTBSY,
        /// so every test that spawns holds this lock.
        static SPAWN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

        struct Fake {
            dir: PathBuf,
        }

        impl Fake {
            /// `body` runs with `$PIDFILE` / `$ARGSFILE` set; the script records its pid and
            /// arguments first.
            fn new(name: &str, body: &str) -> Self {
                let dir = std::env::temp_dir()
                    .join(format!("boussole-tunnel-{}-{name}", std::process::id()));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir).unwrap();
                let script = format!(
                    "#!/bin/sh\necho $$ > '{pid}'\necho \"$@\" > '{args}'\n{body}\n",
                    pid = dir.join("pid").display(),
                    args = dir.join("args").display(),
                );
                let path = dir.join("cloudflared");
                std::fs::write(&path, script).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
                Self { dir }
            }
            fn program(&self) -> PathBuf {
                self.dir.join("cloudflared")
            }
            fn pid(&self) -> u32 {
                std::fs::read_to_string(self.dir.join("pid"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap()
            }
            fn args(&self) -> String {
                std::fs::read_to_string(self.dir.join("args"))
                    .unwrap()
                    .trim()
                    .to_owned()
            }
        }

        impl Drop for Fake {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        fn alive(pid: u32) -> bool {
            std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        }

        async fn assert_dies(pid: u32) {
            for _ in 0..50 {
                if !alive(pid) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("process {pid} is still running");
        }

        #[tokio::test]
        async fn not_installed_when_the_binary_is_not_on_path() {
            let _lock = SPAWN.lock().await;
            let empty =
                std::env::temp_dir().join(format!("boussole-empty-path-{}", std::process::id()));
            std::fs::create_dir_all(&empty).unwrap();
            let err = launch(
                OsStr::new("cloudflared"),
                Some(empty.clone().into()),
                1,
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
            std::fs::remove_dir_all(&empty).unwrap();
            assert_eq!(err, TunnelError::NotInstalled);
            assert!(err.to_string().contains("brew install cloudflared"));
        }

        #[tokio::test]
        async fn parses_the_url_passes_the_right_arguments_and_stop_kills_the_child() {
            let _lock = SPAWN.lock().await;
            let fake = Fake::new(
                "ok",
                "echo 'INF Requesting new quick Tunnel on trycloudflare.com...' >&2\n\
                 echo 'INF |  https://fake-words-here.trycloudflare.com  |' >&2\n\
                 exec sleep 300",
            );
            let tunnel = launch(
                fake.program().as_os_str(),
                None,
                4321,
                Duration::from_secs(10),
            )
            .await
            .expect("tunnel starts");
            assert_eq!(
                tunnel.public_ws_url(),
                "wss://fake-words-here.trycloudflare.com/ws"
            );
            let args = fake.args();
            let config = args
                .strip_prefix("tunnel --no-autoupdate --config ")
                .and_then(|rest| rest.strip_suffix(" --url http://127.0.0.1:4321"))
                .unwrap_or_else(|| panic!("unexpected arguments: {args}"))
                .to_owned();
            assert!(
                std::path::Path::new(&config).is_file(),
                "the empty config exists while the tunnel runs"
            );
            assert_eq!(
                std::fs::metadata(&config).unwrap().permissions().mode() & 0o777,
                0o600,
                "the temporary config is private"
            );
            let pid = fake.pid();
            assert!(alive(pid));
            tunnel.stop().await;
            assert_dies(pid).await;
            assert!(
                !std::path::Path::new(&config).exists(),
                "the temporary config is removed with the tunnel"
            );
        }

        #[tokio::test]
        async fn dropping_the_tunnel_kills_the_child() {
            let _lock = SPAWN.lock().await;
            let fake = Fake::new(
                "drop",
                "echo https://drop-me.trycloudflare.com\nexec sleep 300",
            );
            let tunnel = launch(fake.program().as_os_str(), None, 1, Duration::from_secs(10))
                .await
                .expect("tunnel starts");
            let pid = fake.pid();
            assert!(alive(pid));
            drop(tunnel);
            assert_dies(pid).await;
        }

        #[tokio::test]
        async fn is_running_and_exited_follow_the_child() {
            let _lock = SPAWN.lock().await;
            // Prints its URL, lives for a moment, then exits on its own (a crash, say).
            let fake = Fake::new(
                "short-lived",
                "echo https://short-lived.trycloudflare.com\nsleep 1",
            );
            let mut tunnel = launch(fake.program().as_os_str(), None, 1, Duration::from_secs(10))
                .await
                .expect("tunnel starts");
            assert!(tunnel.is_running());
            tokio::time::timeout(Duration::from_secs(10), tunnel.exited())
                .await
                .expect("exited() resolves when the child exits");
            assert!(!tunnel.is_running());
            // exited() on a child that is already gone returns at once.
            tunnel.exited().await;
        }

        #[tokio::test]
        async fn closing_the_output_without_exiting_is_not_waited_for_forever() {
            let _lock = SPAWN.lock().await;
            let fake = Fake::new("closed-pipes", "exec >&- 2>&-\nexec sleep 300");
            let started = std::time::Instant::now();
            let err = launch(fake.program().as_os_str(), None, 1, Duration::from_secs(30))
                .await
                .unwrap_err();
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "{:?}",
                started.elapsed()
            );
            assert!(
                matches!(&err, TunnelError::StartFailed(why) if why.contains("closed its output")),
                "{err:?}"
            );
            assert_dies(fake.pid()).await;
        }

        #[test]
        fn the_temporary_config_never_overwrites_an_existing_file() {
            let dir = std::env::temp_dir().join(format!("boussole-excl-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let squatted = dir.join("squatted.yml");
            let fresh = dir.join("fresh.yml");
            std::fs::write(&squatted, "precious").unwrap();

            let first = std::sync::atomic::AtomicBool::new(true);
            let config = EmptyConfig::create_with(|| {
                if first.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    squatted.clone()
                } else {
                    fresh.clone()
                }
            })
            .expect("the next free name is used");

            assert_eq!(config.0, fresh);
            assert_eq!(std::fs::read_to_string(&squatted).unwrap(), "precious");
            assert_eq!(
                std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
                0o600
            );
            drop(config);
            assert!(!fresh.exists(), "dropped with the Tunnel");
            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[tokio::test]
        async fn early_exit_is_a_start_failure_with_the_output() {
            let _lock = SPAWN.lock().await;
            let fake = Fake::new("exit", "echo 'ERR no network' >&2\nexit 3");
            let err = launch(fake.program().as_os_str(), None, 1, Duration::from_secs(10))
                .await
                .unwrap_err();
            match err {
                TunnelError::StartFailed(why) => {
                    assert!(why.contains("no network") && why.contains('3'), "{why}");
                }
                other => panic!("expected StartFailed, got {other:?}"),
            }
        }

        #[tokio::test]
        async fn silence_times_out_and_the_child_is_killed() {
            let _lock = SPAWN.lock().await;
            let fake = Fake::new("silent", "exec sleep 300");
            let err = launch(
                fake.program().as_os_str(),
                None,
                1,
                Duration::from_millis(400),
            )
            .await
            .unwrap_err();
            assert_eq!(err, TunnelError::Timeout);
            assert_dies(fake.pid()).await;
        }
    }
}
