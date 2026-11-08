//! Acceptance 6b: the server embedded in a host application's tokio runtime.

mod net;

use boussole_protocol::*;
use boussole_server::{ServerConfig, start};
use net::{TestClient, play_course, room_config};
use std::time::Duration;

#[tokio::test]
async fn host_mode_server_plays_a_match_in_the_callers_runtime_and_releases_its_port() {
    // Exactly what the desktop client does: an existing runtime, host-mode caps, port 0.
    let mut config = ServerConfig::for_host_mode("127.0.0.1:0".parse().unwrap());
    config.time_scale = 20;
    let server = start(config).await.expect("start");
    let addr = server.local_addr();
    assert!(addr.ip().is_loopback());
    assert_ne!(addr.port(), 0, "port 0 must resolve to the real port");

    let mut host = TestClient::connect(addr, "Hote").await;
    let mut guest = TestClient::connect(addr, "Invite").await;
    let code = host
        .create(room_config(Mode::Course, Difficulty::Facile, 1))
        .await
        .code;
    guest.join(&code).await;
    host.send(&ClientMsg::StartMatch).await;

    let (a, b) = tokio::join!(
        play_course(&mut host, Difficulty::Facile, 1, 1),
        play_course(&mut guest, Difficulty::Facile, 1, 2),
    );
    assert_eq!(
        a.match_ended, b.match_ended,
        "both players see the same scoreboard"
    );
    assert_eq!(a.match_ended.ranking.len(), 2);

    // Shutdown completes, tells clients goodbye (1001) and frees the port.
    let (closed_host, closed_guest, ()) =
        tokio::join!(host.closed(), guest.closed(), server.shutdown());
    assert_eq!(closed_host, Some(1001));
    assert_eq!(closed_guest, Some(1001));
    tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpListener::bind(addr))
        .await
        .expect("bind timeout")
        .expect("the port is released after shutdown");
}

#[tokio::test]
async fn dropping_the_handle_stops_the_server_too() {
    let server = start(ServerConfig::for_host_mode("127.0.0.1:0".parse().unwrap()))
        .await
        .unwrap();
    let addr = server.local_addr();
    let mut c = TestClient::connect(addr, "Solo").await;
    drop(server);
    assert_eq!(c.closed().await, Some(1001));
}
