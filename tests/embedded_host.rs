//! The real [`Host`]: the Boussole server embedded in the app, driven the way the UI drives it
//! (`start` / `stop` / `health` from plain threads). Real sockets on `127.0.0.1`, real server,
//! two protocol clients playing a round. A tunnel is never started here: `cloudflared` is
//! replaced by an injected starter that fails, to test the path where it cannot start.

use boussole::embedded_host::{EmbeddedHost, health_of};
use boussole::host::{Host, HostError, HostHealth, HostMode};
use boussole::net::{NetConfig, NetError, NetEvent, NetHandle};
use boussole_protocol::{ClientMsg, Difficulty, Mode, RoomConfig, RoomPhase, ServerMsg};
use boussole_server::tunnel::TunnelError;
use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(15);

/// A host that binds on loopback (the real one tries `0.0.0.0:7878` first).
fn host() -> EmbeddedHost {
    EmbeddedHost::with_lan_bind("127.0.0.1:0".parse().unwrap())
}

fn port_is_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

struct Player {
    handle: NetHandle,
    events: mpsc::Receiver<NetEvent>,
    log: Vec<ServerMsg>,
}

fn join(url: &str, nickname: &str) -> Player {
    let (tx, events) = mpsc::channel();
    let handle = NetHandle::connect(NetConfig::new(url, nickname), move |event| {
        let _ = tx.send(event);
    });
    Player {
        handle,
        events,
        log: Vec::new(),
    }
}

impl Player {
    /// Receive until `pick` returns something for a message (or an event ends the connection).
    fn until<T>(&mut self, what: &str, mut pick: impl FnMut(&ServerMsg) -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(NetEvent::Msg(msg)) => {
                    if let Some(found) = pick(&msg) {
                        self.log.push(msg);
                        return found;
                    }
                    self.log.push(msg);
                }
                Ok(other) => panic!("while waiting for {what}: {other:?}"),
                Err(_) => panic!("timed out waiting for {what}"),
            }
        }
    }

    /// Skip what is still queued and return how the connection ended.
    fn ended(&self) -> NetEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left).expect("the connection ends") {
                NetEvent::Msg(_) => continue,
                other => return other,
            }
        }
    }
}

// --- hosting a game --------------------------------------------------------------------------

#[test]
fn lan_hosting_runs_a_real_match_for_two_clients_and_stopping_releases_the_port() {
    let host = host();
    let info = host.start(HostMode::Lan).expect("the server starts");
    assert_eq!(info.local_url, format!("ws://127.0.0.1:{}/ws", info.port));
    assert!(info.tunnel_url.is_none(), "a LAN game opens no tunnel");
    assert!(!port_is_free(info.port), "the server listens");
    assert_eq!(host.health(), HostHealth::Running);

    // The host's own client creates the room…
    let mut alice = join(&info.local_url, "Alice");
    alice.until("alice's welcome", |m| {
        matches!(m, ServerMsg::Welcome { .. }).then_some(())
    });
    let mut config = RoomConfig::new(Mode::Course, Difficulty::Facile);
    config.rounds = 1;
    alice.handle.send(ClientMsg::CreateRoom { config });
    let code = alice.until("the room", |m| match m {
        ServerMsg::RoomSnapshot(s) => Some(s.code.clone()),
        _ => None,
    });

    // …a second client joins it by code…
    let mut bob = join(&info.local_url, "Bob");
    bob.until("bob's welcome", |m| {
        matches!(m, ServerMsg::Welcome { .. }).then_some(())
    });
    bob.handle.send(ClientMsg::JoinRoom { code: code.clone() });
    bob.until("bob in the room", |m| match m {
        ServerMsg::RoomSnapshot(s) => (s.players.len() == 2).then_some(()),
        _ => None,
    });
    alice.until("two players", |m| match m {
        ServerMsg::RoomSnapshot(s) => (s.players.len() == 2).then_some(()),
        _ => None,
    });

    // …the host starts the match and both play their six tries.
    alice.handle.send(ClientMsg::StartMatch);
    alice.until("round 1 (alice)", |m| {
        matches!(m, ServerMsg::RoundStarted(_)).then_some(())
    });
    bob.until("round 1 (bob)", |m| {
        matches!(m, ServerMsg::RoundStarted(_)).then_some(())
    });
    let cities: Vec<&str> = boussole::cities::CITIES
        .iter()
        .take(6)
        .map(|c| c.name)
        .collect();
    for city in &cities {
        alice.handle.send(ClientMsg::Guess {
            city: (*city).to_owned(),
        });
        bob.handle.send(ClientMsg::Guess {
            city: (*city).to_owned(),
        });
    }
    // (a lucky guess ends a player's round early; the round ends when both are done)
    let secret_alice = alice.until("round end (alice)", |m| match m {
        ServerMsg::RoundEnded(r) => Some((r.secret.clone(), r.results.len())),
        _ => None,
    });
    let secret_bob = bob.until("round end (bob)", |m| match m {
        ServerMsg::RoundEnded(r) => Some((r.secret.clone(), r.results.len())),
        _ => None,
    });
    assert_eq!(
        secret_alice, secret_bob,
        "both see the same secret and both results"
    );
    assert_eq!(secret_alice.1, 2);
    let ranking = alice.until("match end", |m| match m {
        ServerMsg::MatchEnded(e) => Some(
            e.ranking
                .iter()
                .map(|r| r.nickname.clone())
                .collect::<Vec<_>>(),
        ),
        _ => None,
    });
    assert_eq!(ranking.len(), 2);
    let phase_back = alice.until("back to the lobby", |m| match m {
        ServerMsg::RoomSnapshot(s) if s.phase == RoomPhase::Lobby => Some(s.phase),
        _ => None,
    });
    assert_eq!(phase_back, RoomPhase::Lobby);

    // Stopping shuts the server down cleanly: both clients are told the game is over (close
    // 1001), and the port is free again.
    let port = info.port;
    host.stop();
    assert_eq!(alice.ended(), NetEvent::Failed(NetError::ServerClosed));
    assert_eq!(bob.ended(), NetEvent::Failed(NetError::ServerClosed));
    assert!(port_is_free(port), "stop released the port");
    assert!(!host.is_running());
    // harmless twice
    host.stop();
}

#[test]
fn a_second_start_replaces_the_first_server() {
    let host = host();
    let first = host.start(HostMode::Lan).unwrap();
    let second = host.start(HostMode::Lan).unwrap();
    if first.port != second.port {
        assert!(port_is_free(first.port), "the first server is gone");
    }
    assert!(!port_is_free(second.port), "the second one listens");
    host.stop();
    assert!(port_is_free(second.port));
}

#[test]
fn when_the_preferred_port_is_taken_it_falls_back_to_a_free_one() {
    // The real host prefers 0.0.0.0:7878; here the "preferred" port is held by someone else.
    let squatter = TcpListener::bind("127.0.0.1:0").unwrap();
    let taken: SocketAddr = squatter.local_addr().unwrap();
    let host = EmbeddedHost::with_lan_bind(taken);
    let info = host
        .start(HostMode::Lan)
        .expect("falls back instead of failing");
    assert_ne!(info.port, taken.port());
    host.stop();
}

#[test]
fn lan_addresses_are_listed_without_loopback() {
    let host = host();
    let info = host.start(HostMode::Lan).unwrap();
    assert!(
        info.lan_urls.iter().all(|u| !u.contains("127.0.0.1")),
        "{:?}",
        info.lan_urls
    );
    assert!(
        info.lan_urls
            .iter()
            .all(|u| u.ends_with(&format!(":{}/ws", info.port))),
        "{:?}",
        info.lan_urls
    );
    host.stop();
}

// --- the tunnel path (cloudflared is never run here) -----------------------------------------------

#[test]
fn a_missing_cloudflared_is_reported_and_leaves_nothing_running() {
    let seen_port = Arc::new(Mutex::new(None));
    let spy = seen_port.clone();
    let host = EmbeddedHost::with_tunnel_starter(move |port| {
        *spy.lock().unwrap() = Some(port);
        Box::pin(async { Err(TunnelError::NotInstalled) })
    });
    assert_eq!(
        host.start(HostMode::Tunnel),
        Err(HostError::TunnelNotInstalled)
    );
    assert!(
        !host.is_running(),
        "the server that was started for the tunnel is shut down again"
    );
    let port = seen_port
        .lock()
        .unwrap()
        .expect("the starter was given the server's port");
    assert!(port_is_free(port), "and its port is released");
}

#[test]
fn a_failing_or_slow_tunnel_is_reported_as_unavailable() {
    for error in [
        TunnelError::StartFailed("exit status: 1".into()),
        TunnelError::Timeout,
    ] {
        let host = EmbeddedHost::with_tunnel_starter(move |_| {
            let error = error.clone();
            Box::pin(async move { Err(error) })
        });
        match host.start(HostMode::Tunnel) {
            Err(HostError::TunnelFailed(_)) => {}
            other => panic!("{other:?}"),
        }
        assert!(!host.is_running());
    }
}

// --- health -------------------------------------------------------------------------------------

#[test]
fn health_follows_the_server_and_the_tunnel() {
    assert_eq!(health_of(true, None), HostHealth::Running);
    assert_eq!(health_of(true, Some(true)), HostHealth::Running);
    assert_eq!(health_of(true, Some(false)), HostHealth::TunnelLost);
    assert_eq!(
        health_of(false, Some(true)),
        HostHealth::ServerStopped,
        "the server wins"
    );
    assert_eq!(health_of(false, None), HostHealth::ServerStopped);
}

#[test]
fn a_host_that_never_started_reports_nothing_wrong() {
    let host = host();
    assert_eq!(host.health(), HostHealth::Running);
    assert!(!host.is_running());
    host.stop(); // harmless: nothing runs
}
