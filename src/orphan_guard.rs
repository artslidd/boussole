//! Closing a tunnel that a hard exit left behind.
//!
//! The app stops its `cloudflared` when it quits, but a crash, `kill -9` or SIGTERM cannot run
//! that code, and the child outlives its parent: the public address then points at a local port
//! that any other program may later listen on. So the app writes down which tunnel it started
//! (one small file per running app, in the temporary directory), deletes the file when it stops
//! it, and at the next launch closes the tunnels recorded by apps that are no longer running.
//!
//! Only our own tunnel is ever touched: the process must still be a `cloudflared tunnel …` whose
//! `--url` is exactly `http://127.0.0.1:<the recorded port>`. A pid that was reused, or the
//! `cloudflared` service of the machine, never matches.

use std::io;
use std::path::{Path, PathBuf};

/// What the app writes down about the tunnel it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TunnelRecord {
    /// The app that started the tunnel.
    pub app_pid: u32,
    /// The `cloudflared` process.
    pub tunnel_pid: u32,
    /// The local port it exposes.
    pub port: u16,
}

impl TunnelRecord {
    fn encode(&self) -> String {
        format!("{} {} {}\n", self.app_pid, self.tunnel_pid, self.port)
    }

    fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split_whitespace();
        let record = Self {
            app_pid: parts.next()?.parse().ok()?,
            tunnel_pid: parts.next()?.parse().ok()?,
            port: parts.next()?.parse().ok()?,
        };
        parts.next().is_none().then_some(record)
    }
}

/// What the cleanup needs to know about processes (a fake in the tests).
pub trait Processes {
    /// The command line of a process that exists, program first.
    fn command_line(&self, pid: u32) -> Option<String>;
    /// Does the process exist and still run (a zombie does not)?
    fn is_alive(&self, pid: u32) -> bool;
    /// Ask the process to end (SIGTERM). `true` if the request was delivered.
    fn terminate(&self, pid: u32) -> bool;
}

/// The operating system's processes, through `ps` and `kill` (Unix; elsewhere nothing is found
/// and nothing is ever killed).
pub struct SystemProcesses;

#[cfg(unix)]
impl Processes for SystemProcesses {
    fn command_line(&self, pid: u32) -> Option<String> {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (output.status.success() && !text.is_empty()).then_some(text)
    }

    fn is_alive(&self, pid: u32) -> bool {
        std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "stat="])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .is_some_and(|state| !state.is_empty() && !state.starts_with('Z'))
    }

    fn terminate(&self, pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success())
    }
}

#[cfg(not(unix))]
impl Processes for SystemProcesses {
    fn command_line(&self, _pid: u32) -> Option<String> {
        None
    }
    fn is_alive(&self, _pid: u32) -> bool {
        false
    }
    fn terminate(&self, _pid: u32) -> bool {
        false
    }
}

const PREFIX: &str = "boussole-tunnel-";
const SUFFIX: &str = ".rec";

/// Where the records live: the runtime directory when there is one (private to the user), else
/// the temporary directory (private to the user on macOS).
pub fn default_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

/// The record of the app with this pid.
pub fn record_file(dir: &Path, app_pid: u32) -> PathBuf {
    dir.join(format!("{PREFIX}{app_pid}{SUFFIX}"))
}

/// Write down a tunnel (replacing the record of the same app, if any).
pub fn write_record(dir: &Path, record: &TunnelRecord) -> io::Result<()> {
    let path = record_file(dir, record.app_pid);
    // Never through a symlink someone left at that name.
    let _ = std::fs::remove_file(&path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    io::Write::write_all(&mut options.open(&path)?, record.encode().as_bytes())
}

/// Forget the tunnel of this app (it was stopped). Nothing to forget is fine.
pub fn remove_record(dir: &Path, app_pid: u32) {
    let _ = std::fs::remove_file(record_file(dir, app_pid));
}

/// Is `command_line` a `cloudflared tunnel …` exposing `http://127.0.0.1:<port>`? The program is
/// the argument that is named `cloudflared` and followed by `tunnel` (it is not the first one
/// when a shell script stands in for it), and the address must be exactly ours, so another port,
/// another host or a named tunnel of the machine does not match.
pub fn is_our_cloudflared(command_line: &str, port: u16) -> bool {
    let tokens: Vec<&str> = command_line.split_whitespace().collect();
    let program = tokens.windows(2).any(|pair| {
        pair[1] == "tunnel" && pair[0].rsplit(['/', '\\']).next() == Some("cloudflared")
    });
    let address = format!("http://127.0.0.1:{port}");
    let ours = tokens
        .windows(2)
        .any(|pair| pair[0] == "--url" && pair[1] == address);
    program && ours
}

/// Close the tunnels recorded by apps that are gone. Returns the ones it closed. A record whose
/// app still runs (another instance, or this one: `me`) is kept; any other record is deleted
/// once looked at, whatever came of it.
pub fn reap_orphans(dir: &Path, me: u32, processes: &dyn Processes) -> Vec<TunnelRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut closed = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(PREFIX) && name.ends_with(SUFFIX)) {
            continue;
        }
        let path = entry.path();
        let Some(record) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| TunnelRecord::parse(&text))
        else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        if record.app_pid == me || processes.is_alive(record.app_pid) {
            continue;
        }
        let _ = std::fs::remove_file(&path);
        // pids 0 and 1 and our own are never a tunnel to close
        if record.tunnel_pid <= 1 || record.tunnel_pid == me {
            continue;
        }
        let ours = processes
            .command_line(record.tunnel_pid)
            .is_some_and(|command| is_our_cloudflared(&command, record.port));
        if ours && processes.terminate(record.tunnel_pid) {
            closed.push(record);
        }
    }
    closed
}

/// At startup: close the tunnels earlier runs of the app left behind. Returns how many.
pub fn reap_previous_runs() -> usize {
    reap_orphans(&default_dir(), std::process::id(), &SystemProcesses).len()
}
