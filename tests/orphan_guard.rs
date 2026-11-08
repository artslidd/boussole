//! A hard exit of the app (SIGKILL, a crash, SIGTERM) cannot stop the `cloudflared` it started.
//! The app writes down which tunnel it started; the next launch closes it, and only it: a
//! process that is not our own `cloudflared tunnel … --url http://127.0.0.1:<port>` is never
//! touched (a system `cloudflared` service may be running on the same machine).

use boussole::embedded_host::EmbeddedHost;
use boussole::host::{Host, HostMode};
use boussole::orphan_guard::{
    Processes, SystemProcesses, TunnelRecord, is_our_cloudflared, reap_orphans, record_file,
    remove_record, write_record,
};
use boussole_server::tunnel;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Writing a script and then running it while another thread forks can fail with ETXTBSY, so
/// the tests of this file run one at a time (they are quick).
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A scratch directory, deleted at the end.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("boussole-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Processes that exist only in the test.
#[derive(Default)]
struct FakeProcesses {
    commands: HashMap<u32, String>,
    alive: HashSet<u32>,
    terminated: RefCell<Vec<u32>>,
}

impl FakeProcesses {
    fn with(mut self, pid: u32, command: &str) -> Self {
        self.commands.insert(pid, command.to_owned());
        self.alive.insert(pid);
        self
    }
}

impl Processes for FakeProcesses {
    fn command_line(&self, pid: u32) -> Option<String> {
        self.commands.get(&pid).cloned()
    }
    fn is_alive(&self, pid: u32) -> bool {
        self.alive.contains(&pid)
    }
    fn terminate(&self, pid: u32) -> bool {
        self.terminated.borrow_mut().push(pid);
        true
    }
}

const OURS: &str = "cloudflared tunnel --no-autoupdate --config /tmp/boussole-cloudflared-1-0.yml --url http://127.0.0.1:51234";

fn record(app_pid: u32, tunnel_pid: u32, port: u16) -> TunnelRecord {
    TunnelRecord {
        app_pid,
        tunnel_pid,
        port,
    }
}

// --- recognising our own tunnel ------------------------------------------------------------------

#[test]
fn our_own_cloudflared_is_recognised_by_program_and_port() {
    let _serial = serial();
    assert!(is_our_cloudflared(OURS, 51234));
    assert!(is_our_cloudflared(
        "/opt/homebrew/bin/cloudflared tunnel --no-autoupdate --config /tmp/x.yml --url http://127.0.0.1:51234",
        51234
    ));
    // a script standing in for it (the real-process test below), and a path with a space
    assert!(is_our_cloudflared(
        "/bin/sh /tmp/fake/cloudflared tunnel --no-autoupdate --url http://127.0.0.1:51234",
        51234
    ));
    assert!(is_our_cloudflared(
        "/Users/a b/bin/cloudflared tunnel --url http://127.0.0.1:51234",
        51234
    ));
}

#[test]
fn nothing_else_is_taken_for_our_tunnel() {
    let _serial = serial();
    // another port, including one that merely starts with the same digits
    assert!(!is_our_cloudflared(OURS, 51235));
    assert!(!is_our_cloudflared(OURS, 5123));
    assert!(!is_our_cloudflared(OURS, 1234));
    // a service of the machine (named tunnel, configuration file), whatever it points at
    assert!(!is_our_cloudflared(
        "/usr/local/bin/cloudflared --config /etc/cloudflared/config.yml tunnel run home",
        51234
    ));
    assert!(!is_our_cloudflared(
        "cloudflared tunnel run my-tunnel",
        51234
    ));
    // somebody else's quick tunnel: not to our loopback address
    assert!(!is_our_cloudflared(
        "cloudflared tunnel --url http://192.168.1.5:51234",
        51234
    ));
    assert!(!is_our_cloudflared(
        "cloudflared tunnel --url https://127.0.0.1:51234",
        51234
    ));
    // other programs, even one that mentions it
    assert!(!is_our_cloudflared(
        "vim cloudflared-notes tunnel --url http://127.0.0.1:51234",
        51234
    ));
    assert!(!is_our_cloudflared("sleep 600", 51234));
    assert!(!is_our_cloudflared("", 51234));
}

// --- the record ----------------------------------------------------------------------------------

#[test]
fn a_record_is_written_per_app_and_removed() {
    let _serial = serial();
    let dir = Scratch::new("record");
    let first = record(100, 200, 4000);
    let second = record(101, 201, 4001);
    write_record(dir.path(), &first).unwrap();
    write_record(dir.path(), &second).unwrap();
    assert!(record_file(dir.path(), 100).exists());
    assert!(record_file(dir.path(), 101).exists());
    assert_ne!(record_file(dir.path(), 100), record_file(dir.path(), 101));
    remove_record(dir.path(), 100);
    assert!(!record_file(dir.path(), 100).exists());
    assert!(
        record_file(dir.path(), 101).exists(),
        "another run's record stays"
    );
    remove_record(dir.path(), 100); // already gone: nothing to say
}

// --- cleaning up after a hard exit ---------------------------------------------------------------

#[test]
fn a_tunnel_left_by_a_dead_app_is_closed_and_its_record_deleted() {
    let _serial = serial();
    let dir = Scratch::new("reap-dead");
    write_record(dir.path(), &record(100, 200, 51234)).unwrap();
    // pid 100 (the app) is gone; pid 200 is our cloudflared, still running
    let processes = FakeProcesses::default().with(200, OURS);

    let closed = reap_orphans(dir.path(), 999, &processes);
    assert_eq!(closed, vec![record(100, 200, 51234)]);
    assert_eq!(*processes.terminated.borrow(), vec![200]);
    assert!(!record_file(dir.path(), 100).exists());
}

#[test]
fn a_tunnel_of_an_app_that_is_still_running_is_left_alone() {
    let _serial = serial();
    let dir = Scratch::new("reap-alive");
    write_record(dir.path(), &record(100, 200, 51234)).unwrap();
    // a second window of the game, or another instance, owns it
    let processes = FakeProcesses::default()
        .with(100, "/Applications/Boussole.app/Contents/MacOS/boussole")
        .with(200, OURS);

    assert!(reap_orphans(dir.path(), 999, &processes).is_empty());
    assert!(processes.terminated.borrow().is_empty());
    assert!(record_file(dir.path(), 100).exists(), "still needed");
}

#[test]
fn the_record_of_this_very_run_is_never_acted_on() {
    let _serial = serial();
    let dir = Scratch::new("reap-own");
    write_record(dir.path(), &record(999, 200, 51234)).unwrap();
    let processes = FakeProcesses::default().with(200, OURS);
    assert!(reap_orphans(dir.path(), 999, &processes).is_empty());
    assert!(processes.terminated.borrow().is_empty());
    assert!(record_file(dir.path(), 999).exists());
}

#[test]
fn a_process_that_is_not_our_tunnel_is_never_killed_even_if_the_pid_matches() {
    let _serial = serial();
    let dir = Scratch::new("reap-other");
    for (app_pid, tunnel_pid, command) in [
        // the pid was reused by something else
        (100, 200, "/usr/bin/python3 server.py"),
        // the system's own cloudflared service
        (
            101,
            201,
            "/usr/local/bin/cloudflared --config /etc/cloudflared/config.yml tunnel run",
        ),
        // somebody's quick tunnel to another local service
        (102, 202, "cloudflared tunnel --url http://127.0.0.1:8080"),
    ] {
        write_record(dir.path(), &record(app_pid, tunnel_pid, 51234)).unwrap();
        let processes = FakeProcesses::default().with(tunnel_pid, command);
        assert!(
            reap_orphans(dir.path(), 999, &processes).is_empty(),
            "{command}"
        );
        assert!(processes.terminated.borrow().is_empty(), "{command}");
        assert!(
            !record_file(dir.path(), app_pid).exists(),
            "the stale record is dropped all the same"
        );
    }
}

#[test]
fn a_tunnel_that_already_ended_just_clears_its_record() {
    let _serial = serial();
    let dir = Scratch::new("reap-gone");
    write_record(dir.path(), &record(100, 200, 51234)).unwrap();
    let processes = FakeProcesses::default(); // nothing runs any more
    assert!(reap_orphans(dir.path(), 999, &processes).is_empty());
    assert!(!record_file(dir.path(), 100).exists());
}

#[test]
fn unreadable_or_foreign_files_are_ignored_and_never_make_it_kill_anything() {
    let _serial = serial();
    let dir = Scratch::new("reap-junk");
    std::fs::write(record_file(dir.path(), 100), "not a record").unwrap();
    std::fs::write(record_file(dir.path(), 101), "101 0 51234").unwrap(); // pid 0
    std::fs::write(record_file(dir.path(), 102), "102 1 51234").unwrap(); // pid 1
    std::fs::write(dir.path().join("notes.txt"), "100 200 51234").unwrap();
    let processes = FakeProcesses::default().with(0, OURS).with(1, OURS);
    assert!(reap_orphans(dir.path(), 999, &processes).is_empty());
    assert!(processes.terminated.borrow().is_empty());
    assert!(
        dir.path().join("notes.txt").exists(),
        "other files are not ours"
    );
    // a directory that does not exist is no problem either
    assert!(reap_orphans(&dir.path().join("nowhere"), 999, &processes).is_empty());
}

// --- with real processes -------------------------------------------------------------------------

/// A `cloudflared` stand-in that prints a quick-tunnel address and then just runs.
fn fake_cloudflared(dir: &Path) -> PathBuf {
    let path = dir.join("cloudflared");
    std::fs::write(
        &path,
        "#!/bin/sh\necho 'INF | https://fake-words-here.trycloudflare.com |' >&2\nwhile true; do sleep 1; done\n",
    )
    .unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path
}

fn spawn_fake(program: &Path, port: u16) -> Child {
    Command::new(program)
        .args(["tunnel", "--no-autoupdate", "--url"])
        .arg(format!("http://127.0.0.1:{port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawning the fake cloudflared")
}

/// The pid of a process that has just ended.
fn dead_pid() -> u32 {
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn ends_within(child: &mut Child, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[cfg(unix)]
#[test]
fn a_really_orphaned_tunnel_is_closed_and_a_decoy_on_another_port_survives() {
    let _serial = serial();
    let scratch = Scratch::new("real");
    let program = fake_cloudflared(scratch.path());
    let mut orphan = spawn_fake(&program, 51234);
    let mut decoy = spawn_fake(&program, 51999); // same program, another port: not recorded
    std::thread::sleep(Duration::from_millis(300)); // let `sh` start
    write_record(scratch.path(), &record(dead_pid(), orphan.id(), 51234)).unwrap();

    let closed = reap_orphans(scratch.path(), std::process::id(), &SystemProcesses);
    assert_eq!(closed.len(), 1);
    assert!(
        ends_within(&mut orphan, Duration::from_secs(5)),
        "the orphan was closed"
    );
    assert!(
        !ends_within(&mut decoy, Duration::from_millis(500)),
        "the decoy was not touched"
    );
    let _ = decoy.kill();
    let _ = decoy.wait();
}

#[cfg(unix)]
#[test]
fn a_real_process_that_is_not_cloudflared_is_left_running() {
    let _serial = serial();
    let scratch = Scratch::new("real-other");
    let mut other = Command::new("sleep").arg("30").spawn().unwrap();
    write_record(scratch.path(), &record(dead_pid(), other.id(), 51234)).unwrap();
    assert!(reap_orphans(scratch.path(), std::process::id(), &SystemProcesses).is_empty());
    assert!(!ends_within(&mut other, Duration::from_millis(500)));
    let _ = other.kill();
    let _ = other.wait();
}

// --- the host keeps the record while a tunnel runs -----------------------------------------------

#[cfg(unix)]
#[test]
fn the_host_records_its_tunnel_while_it_runs_and_forgets_it_when_stopped() {
    let _serial = serial();
    let scratch = Scratch::new("host");
    let bin = scratch.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    fake_cloudflared(&bin);
    let records = scratch.path().join("records");
    std::fs::create_dir_all(&records).unwrap();
    // Our fake first on PATH. This test file's other tests never read PATH for anything but
    // `ps`/`kill`, which stay reachable behind it.
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin.clone()];
    dirs.extend(std::env::split_paths(&path));
    // SAFETY: nothing else in this test binary reads or writes the environment concurrently
    // except through `Command`, which only reads PATH.
    unsafe { std::env::set_var("PATH", std::env::join_paths(dirs).unwrap()) };

    let host = EmbeddedHost::with_tunnel_starter(|port| Box::pin(tunnel::start_cloudflared(port)))
        .with_record_dir(records.clone());
    let info = host
        .start(HostMode::Tunnel)
        .expect("the fake tunnel starts");
    assert_eq!(
        info.tunnel_url.as_deref(),
        Some("wss://fake-words-here.trycloudflare.com/ws")
    );
    let file = record_file(&records, std::process::id());
    let text = std::fs::read_to_string(&file).expect("a record while the tunnel runs");
    let parts: Vec<u32> = text
        .split_whitespace()
        .map(|p| p.parse().unwrap())
        .collect();
    assert_eq!(parts[0], std::process::id());
    assert_eq!(parts[2], u32::from(info.port));
    let tunnel_pid = parts[1];
    assert!(SystemProcesses.is_alive(tunnel_pid));
    assert!(is_our_cloudflared(
        &SystemProcesses.command_line(tunnel_pid).unwrap(),
        info.port
    ));

    host.stop();
    assert!(!file.exists(), "nothing to clean up after a clean stop");
    let deadline = Instant::now() + Duration::from_secs(5);
    while SystemProcesses.is_alive(tunnel_pid) {
        assert!(
            Instant::now() < deadline,
            "the tunnel process is still there"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
