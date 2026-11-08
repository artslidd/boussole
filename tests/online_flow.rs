//! The online screens driven like a player would, against a scripted WebSocket server: the
//! "En ligne" menu and its errors, Rejoindre, Héberger (with a fake host), the lobby and its
//! host-only controls, Course and Coopération rounds, results, final ranking, the reconnect
//! banner and leaving. Keys and clicks go through the real `.slint` UI; controls are found by
//! accessible label (which also proves they have a name).

mod common;

use boussole::app::{App, BannerKind, HostChoice, MenuHelp, Online, OnlineScreen, RoomMode};
use boussole::clipboard::MemoryClipboard;
use boussole::headless::Headless;
use boussole::host::{FakeHost, HostError, HostHealth, HostInfo, HostMode};
use boussole::online::view::Screen;
use boussole::online_ui::Stage;
use boussole_protocol::{
    ClientMsg, Difficulty, ErrorCode, GuessInfo, Mode, PlayerInfo, Progress, RoomConfig, RoomPhase,
    RoomSnapshot, RoundStarted, RoundView, ServerMsg, Timer, TurnReason, decode_server,
};
use common::{Handshake, MockServer};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::ComponentHandle;
use slint::platform::Key;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

macro_rules! fixture {
    ($name:literal) => {
        decode_server(include_str!(concat!(
            "../crates/boussole-protocol/tests/fixtures/server/",
            $name,
            ".json"
        )))
        .expect(concat!("fixture ", $name))
    };
}

struct Ui {
    h: Headless,
    app: App,
    clipboard: Rc<MemoryClipboard>,
}

fn ui(width: u32, height: u32) -> Ui {
    let h = Headless::new(width, height, 1.0).expect("headless platform");
    let app = App::with_seed_sequence(100).expect("window");
    app.window().show().expect("show");
    let clipboard = Rc::new(MemoryClipboard::new());
    app.online().set_clipboard(clipboard.clone());
    app.online().set_net_tweak(|config| {
        config.connect_timeout = Duration::from_millis(1500);
        config.reconnect_window = Duration::from_secs(3);
        config.backoff_initial = Duration::from_millis(30);
        config.backoff_max = Duration::from_millis(100);
    });
    h.settle();
    Ui { h, app, clipboard }
}

impl Ui {
    fn online(&self) -> Online<'_> {
        self.app.window().global::<Online>()
    }

    fn key(&self, key: Key) {
        self.h.press_key(key);
    }

    fn element(&self, label: &str, role: Option<AccessibleRole>) -> ElementHandle {
        self.try_element(label, role)
            .unwrap_or_else(|| panic!("no element labelled {label:?} ({role:?})"))
    }

    fn try_element(&self, label: &str, role: Option<AccessibleRole>) -> Option<ElementHandle> {
        ElementHandle::find_by_accessible_label(self.app.window(), label)
            .find(|e| role.is_none() || e.accessible_role() == role)
    }

    fn click(&self, label: &str, role: Option<AccessibleRole>) {
        let element = self.element(label, role);
        let p = element.absolute_position();
        let s = element.size();
        self.h.click(p.x + s.width / 2.0, p.y + s.height / 2.0);
    }

    fn button(&self, label: &str) {
        self.click(label, Some(AccessibleRole::Button));
    }

    fn has_button(&self, label: &str) -> bool {
        self.try_element(label, Some(AccessibleRole::Button))
            .is_some()
    }

    /// Click into a text field (found by its label) and type.
    fn fill(&self, label: &str, text: &str) {
        self.click(label, Some(AccessibleRole::TextInput));
        self.h.type_text(text);
    }

    /// Let the network and the timers run until `condition` holds (10 s of real time at most).
    fn wait(&self, what: &str, condition: impl Fn(&Ui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.h.tick(Duration::from_millis(50));
            std::thread::sleep(Duration::from_millis(2));
        }
        self.h.settle();
    }

    /// The next message the server receives, while the UI keeps running (its poll timer drains
    /// the network inbox: a blocking wait on the server alone would never let it send anything).
    fn next_msg(&self, server: &MockServer) -> ClientMsg {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.h.tick(Duration::from_millis(50));
            if let Some(msg) = server.try_next_client_msg(Duration::from_millis(3)) {
                return msg;
            }
            assert!(
                Instant::now() < deadline,
                "the client sent no message within 10 s"
            );
        }
    }

    /// Nothing reaches the server for a while, although the UI keeps running.
    fn no_msg(&self, server: &MockServer) -> bool {
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            self.h.tick(Duration::from_millis(50));
            if server
                .try_next_client_msg(Duration::from_millis(5))
                .is_some()
            {
                return false;
            }
        }
        true
    }

    fn screen(&self) -> OnlineScreen {
        self.online().get_screen()
    }

    fn open_menu(&self) {
        self.button("Jouer en ligne");
        assert!(self.online().get_active());
        assert_eq!(self.screen(), OnlineScreen::Menu);
    }

    /// The menu filled in for joining `server`'s room `code`.
    fn fill_join(&self, server: &MockServer, code: &str) {
        self.fill("Pseudo", "Alice");
        self.fill("Adresse", &format!("127.0.0.1:{}", server.port));
        self.fill("Code de la salle", code);
    }
}

fn player(id: u32, name: &str, host: bool, connected: bool) -> PlayerInfo {
    PlayerInfo {
        id,
        nickname: name.to_owned(),
        host,
        connected,
        score: 0,
    }
}

fn lobby_msg(config: RoomConfig, players: Vec<PlayerInfo>) -> ServerMsg {
    ServerMsg::RoomSnapshot(RoomSnapshot {
        code: "K7QX2".into(),
        config,
        phase: RoomPhase::Lobby,
        players,
        round: None,
        intermission: None,
    })
}

fn course_config() -> RoomConfig {
    RoomConfig::new(Mode::Course, Difficulty::Facile)
}

fn round_started(msg: ServerMsg) -> RoundStarted {
    match msg {
        ServerMsg::RoundStarted(r) => r,
        other => panic!("not round_started: {other:?}"),
    }
}

/// `round_started` + the snapshot of the same step, as the server sends them.
fn start_round(
    server: &MockServer,
    config: RoomConfig,
    players: Vec<PlayerInfo>,
    start: RoundStarted,
) {
    server.send(ServerMsg::RoundStarted(start.clone()));
    let progress = if config.mode == Mode::Course {
        players
            .iter()
            .map(|p| Progress {
                player_id: p.id,
                tries_used: 0,
                solved: false,
                out_of_tries: false,
            })
            .collect()
    } else {
        Vec::new()
    };
    server.send(ServerMsg::RoomSnapshot(RoomSnapshot {
        code: "K7QX2".into(),
        config,
        phase: RoomPhase::RoundActive,
        players,
        round: Some(RoundView {
            start,
            guesses: Vec::new(),
            progress,
        }),
        intermission: None,
    }));
}

/// A joined room: the client has said hello and joined, the server answered with a lobby.
fn joined(config: RoomConfig, players: Vec<PlayerInfo>) -> (Ui, MockServer) {
    joined_at(1000, 760, config, players)
}

fn joined_at(
    width: u32,
    height: u32,
    config: RoomConfig,
    players: Vec<PlayerInfo>,
) -> (Ui, MockServer) {
    let ui = ui(width, height);
    let server = MockServer::start();
    ui.open_menu();
    ui.fill_join(&server, "k7qx2");
    ui.button("Rejoindre");
    ui.wait("the join request", |_| server.hellos().len() == 1);
    assert_eq!(
        ui.next_msg(&server),
        ClientMsg::JoinRoom {
            code: "K7QX2".into()
        }
    );
    server.send(lobby_msg(config, players));
    ui.wait("the lobby", |ui| ui.screen() == OnlineScreen::Lobby);
    (ui, server)
}

fn alice_and_bob() -> Vec<PlayerInfo> {
    vec![
        player(2, "Bob", true, true),
        player(1, "Alice", false, true),
    ]
}

// --- entering and leaving the online mode -----------------------------------------------------

#[test]
fn the_start_screen_offers_online_and_the_menu_has_both_paths() {
    let ui = ui(1000, 760);
    assert!(ui.has_button("Nouvelle partie"), "solo is still there");
    ui.open_menu();
    assert!(
        !ui.has_button("Nouvelle partie"),
        "solo screens are hidden while online"
    );
    for label in [
        "Héberger une partie",
        "Rejoindre",
        "Créer une salle",
        "Retour",
    ] {
        assert!(ui.has_button(label), "{label}");
    }
    ui.element("Pseudo", Some(AccessibleRole::TextInput));
    ui.element("Adresse", Some(AccessibleRole::TextInput));
    ui.element("Code de la salle", Some(AccessibleRole::TextInput));
    ui.button("Retour");
    assert!(!ui.online().get_active());
    assert!(
        ui.has_button("Nouvelle partie"),
        "back to the solo start screen"
    );
}

#[test]
fn the_form_is_checked_before_anything_is_sent() {
    let ui = ui(1000, 760);
    let server = MockServer::start();
    ui.open_menu();
    ui.button("Rejoindre");
    assert!(ui.online().get_nickname_error().contains("1 à 16"));
    assert!(ui.online().get_address_error().contains("adresse"));
    assert!(ui.online().get_code_error().contains("5 caractères"));

    ui.fill("Pseudo", "Alice");
    ui.fill("Adresse", "ftp://nope");
    ui.fill("Code de la salle", "ABC");
    ui.button("Rejoindre");
    assert!(ui.online().get_nickname_error().is_empty());
    assert!(
        ui.online().get_address_error().contains("ws://"),
        "{}",
        ui.online().get_address_error()
    );
    assert!(ui.online().get_code_error().contains("5 caractères"));
    assert_eq!(ui.app.online().stage(), Stage::Menu);
    assert_eq!(server.connections(), 0, "nothing was sent anywhere");
}

#[test]
fn a_nickname_is_cut_at_sixteen_characters() {
    let ui = ui(1000, 760);
    ui.open_menu();
    ui.fill("Pseudo", "Un pseudo vraiment beaucoup trop long");
    assert_eq!(ui.online().get_nickname().chars().count(), 16);
}

// --- Rejoindre --------------------------------------------------------------------------------

#[test]
fn joining_says_hello_then_joins_with_the_normalised_code_and_reaches_the_lobby() {
    let (ui, server) = joined(course_config(), alice_and_bob());
    // hello carried the nickname; the address typed as host:port worked
    match &server.hellos()[0] {
        ClientMsg::Hello { nickname, .. } => assert_eq!(nickname, "Alice"),
        other => panic!("{other:?}"),
    }
    assert_eq!(ui.app.online().stage(), Stage::InRoom);
    assert_eq!(ui.online().get_room_code(), "K7QX2");
    assert_eq!(ui.app.online().screen(), Screen::Lobby);
    // guests do not get an address to share
    assert!(ui.online().get_share_hint().contains("Attendez"));
    assert_eq!(
        ui.element("Code de la salle : K7QX2", None)
            .accessible_role(),
        Some(AccessibleRole::Text)
    );
    ui.element("Bob (hôte)", Some(AccessibleRole::ListItem));
    ui.element("Alice (vous)", Some(AccessibleRole::ListItem));
}

#[test]
fn a_room_that_does_not_exist_sends_the_player_back_with_a_clear_message() {
    let ui = ui(1000, 760);
    let server = MockServer::start();
    ui.open_menu();
    ui.fill_join(&server, "ZZZZZ");
    ui.button("Rejoindre");
    assert!(matches!(ui.next_msg(&server), ClientMsg::JoinRoom { .. }));
    server.send(ServerMsg::error(ErrorCode::RoomNotFound));
    ui.wait("the error", |ui| !ui.online().get_error().is_empty());
    assert_eq!(ui.screen(), OnlineScreen::Menu);
    assert!(
        ui.online().get_error().contains("Salle introuvable"),
        "{}",
        ui.online().get_error()
    );
    assert!(
        !ui.online().get_code_error().is_empty(),
        "the code field points at the problem"
    );
    assert_eq!(ui.app.online().stage(), Stage::Menu);
    server.wait_until("the connection to be closed", |s| s.closed_by_client() == 1);
}

/// Joining is refused with `code`: back on the menu, with `expected` in French.
fn join_refused(code: ErrorCode, expected: &str) {
    let ui = ui(1000, 760);
    let server = MockServer::start();
    ui.open_menu();
    ui.fill_join(&server, "K7QX2");
    ui.button("Rejoindre");
    let _ = ui.next_msg(&server);
    server.send(ServerMsg::error(code));
    ui.wait("the error", |ui| !ui.online().get_error().is_empty());
    assert!(
        ui.online().get_error().contains(expected),
        "{code:?}: {}",
        ui.online().get_error()
    );
    assert_eq!(ui.screen(), OnlineScreen::Menu);
}

#[test]
fn a_full_room_is_explained() {
    join_refused(ErrorCode::RoomFull, "complète");
}

#[test]
fn a_match_already_under_way_is_explained() {
    join_refused(ErrorCode::MatchInProgress, "déjà en cours");
}

#[test]
fn a_nickname_already_taken_is_explained_and_points_at_the_field() {
    join_refused(ErrorCode::NicknameTaken, "déjà pris");
}

#[test]
fn an_outdated_game_is_told_to_update() {
    let ui = ui(1000, 760);
    let server = MockServer::start();
    server.set_handshake(Handshake::Reject {
        code: ErrorCode::BadVersion,
        close: true,
    });
    ui.open_menu();
    ui.fill_join(&server, "K7QX2");
    ui.button("Rejoindre");
    ui.wait("the error", |ui| !ui.online().get_error().is_empty());
    assert!(
        ui.online().get_error().contains("mettez le jeu à jour"),
        "{}",
        ui.online().get_error()
    );
}

#[test]
fn a_server_that_is_not_there_is_reported_as_unreachable() {
    let ui = ui(1000, 760);
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    ui.open_menu();
    ui.fill("Pseudo", "Alice");
    ui.fill("Adresse", &format!("127.0.0.1:{port}"));
    ui.fill("Code de la salle", "K7QX2");
    ui.button("Rejoindre");
    assert!(ui.online().get_busy(), "a connection is in progress");
    ui.wait("the failure", |ui| !ui.online().get_error().is_empty());
    assert!(
        ui.online().get_error().starts_with("Serveur injoignable"),
        "{}",
        ui.online().get_error()
    );
    assert!(!ui.online().get_busy());
}

#[test]
fn creating_a_room_on_a_remote_server_sends_create_room_and_shares_the_server_address() {
    let ui = ui(1000, 760);
    let server = MockServer::start();
    ui.open_menu();
    ui.fill("Pseudo", "Alice");
    ui.fill("Adresse", &format!("127.0.0.1:{}", server.port));
    ui.button("Créer une salle");
    match ui.next_msg(&server) {
        ClientMsg::CreateRoom { config } => assert_eq!(config, course_config()),
        other => panic!("{other:?}"),
    }
    server.send(lobby_msg(
        course_config(),
        vec![player(1, "Alice", true, true)],
    ));
    ui.wait("the lobby", |ui| ui.screen() == OnlineScreen::Lobby);
    assert!(ui.online().get_is_host());
    assert_eq!(
        ui.online().get_share_addresses().row_count(),
        1,
        "the server address"
    );
}

use slint::Model;

// --- Héberger ---------------------------------------------------------------------------------

fn host_info(server: &MockServer) -> HostInfo {
    HostInfo {
        port: server.port,
        local_url: server.url.clone(),
        lan_urls: vec!["ws://192.168.1.20:7878/ws".into()],
        tunnel_url: Some("wss://abc-def-ghi.trycloudflare.com/ws".into()),
    }
}

fn hosted(mode: HostChoice) -> (Ui, MockServer, Arc<FakeHost>) {
    let ui = ui(1000, 760);
    let server = MockServer::start();
    let host = Arc::new(FakeHost::ok(host_info(&server)));
    ui.app.online().set_host(host.clone());
    ui.open_menu();
    ui.fill("Pseudo", "Alice");
    if mode == HostChoice::Tunnel {
        ui.click(
            "Internet : tunnel gratuit, sans compte",
            Some(AccessibleRole::RadioButton),
        );
    }
    ui.button("Héberger une partie");
    ui.wait("the room request", |_| server.hellos().len() == 1);
    match ui.next_msg(&server) {
        ClientMsg::CreateRoom { config } => assert_eq!(config, course_config()),
        other => panic!("{other:?}"),
    }
    server.send(lobby_msg(
        course_config(),
        vec![player(1, "Alice", true, true)],
    ));
    ui.wait("the lobby", |ui| ui.screen() == OnlineScreen::Lobby);
    (ui, server, host)
}

#[test]
fn hosting_on_the_local_network_creates_the_room_and_shows_what_to_share() {
    let (ui, _server, host) = hosted(HostChoice::Lan);
    assert_eq!(host.starts(), [HostMode::Lan]);
    assert!(ui.online().get_is_host());
    // address (LAN first in this fake: the tunnel URL also exists, the fake returns both) + code
    let shared = ui.online().get_share_addresses();
    assert!(shared.row_count() >= 1);
    let urls: Vec<String> = (0..shared.row_count())
        .map(|i| shared.row_data(i).unwrap().url.to_string())
        .collect();
    assert!(
        urls.contains(&"ws://192.168.1.20:7878/ws".to_owned()),
        "{urls:?}"
    );
    assert_eq!(
        ui.online().get_share_hint(),
        "Partagez l’adresse et le code avec vos amis."
    );
    ui.element(
        "Adresse à partager (Réseau local)",
        Some(AccessibleRole::TextInput),
    );
}

#[test]
fn the_tunnel_choice_starts_the_tunnel_and_lists_its_address_first() {
    let (ui, _server, host) = hosted(HostChoice::Tunnel);
    assert_eq!(host.starts(), [HostMode::Tunnel]);
    let shared = ui.online().get_share_addresses();
    let first = shared.row_data(0).unwrap();
    assert_eq!(first.label, "Internet");
    assert_eq!(first.url, "wss://abc-def-ghi.trycloudflare.com/ws");
}

#[test]
fn the_code_and_every_address_can_be_copied() {
    let (ui, _server, _host) = hosted(HostChoice::Tunnel);
    let t0 = Instant::now();
    let offset = Rc::new(std::cell::Cell::new(Duration::ZERO));
    let o = offset.clone();
    ui.app.online().set_clock(Rc::new(move || t0 + o.get()));
    // three "Copier" buttons exist (the code and two addresses): tell them apart by description
    let buttons: Vec<_> = ElementHandle::find_by_accessible_label(ui.app.window(), "Copier")
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .collect();
    assert!(buttons.len() >= 2);
    // by accessible description
    let by_description = |wanted: &str| {
        buttons
            .iter()
            .find(|e| {
                e.accessible_description()
                    .is_some_and(|d| d.contains(wanted))
            })
            .unwrap_or_else(|| panic!("no copy button described {wanted:?}"))
    };
    let center = |e: &ElementHandle| {
        let p = e.absolute_position();
        let s = e.size();
        (p.x + s.width / 2.0, p.y + s.height / 2.0)
    };
    let (x, y) = center(by_description("code de la salle"));
    ui.h.click(x, y);
    assert_eq!(ui.clipboard.content().as_deref(), Some("K7QX2"));
    assert!(
        ui.online().get_code_copied(),
        "the button says Copié for a moment"
    );
    offset.set(Duration::from_millis(2_500));
    ui.app.online().pump();
    assert!(!ui.online().get_code_copied(), "…and goes back to Copier");

    let (x, y) = center(by_description("trycloudflare"));
    ui.h.click(x, y);
    assert_eq!(
        ui.clipboard.content().as_deref(),
        Some("wss://abc-def-ghi.trycloudflare.com/ws")
    );
}

#[test]
fn a_clipboard_that_fails_says_so_and_the_text_stays_selectable() {
    let (ui, _server, _host) = hosted(HostChoice::Lan);
    ui.clipboard.set_failing(true);
    let buttons: Vec<_> = ElementHandle::find_by_accessible_label(ui.app.window(), "Copier")
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .collect();
    let code = buttons
        .iter()
        .find(|e| {
            e.accessible_description()
                .is_some_and(|d| d.contains("code"))
        })
        .unwrap();
    let p = code.absolute_position();
    ui.h.click(p.x + 10.0, p.y + 10.0);
    assert!(
        ui.online().get_share_hint().contains("Copie impossible"),
        "{}",
        ui.online().get_share_hint()
    );
    // read-only text inputs can be selected by hand
    let field = ui.element(
        "Adresse à partager (Réseau local)",
        Some(AccessibleRole::TextInput),
    );
    assert_eq!(field.accessible_read_only(), Some(true));
}

#[test]
fn a_tunnel_without_cloudflared_shows_how_to_install_it_and_offers_the_local_network() {
    let ui = ui(1000, 760);
    let server = MockServer::start();
    let host =
        Arc::new(FakeHost::ok(host_info(&server)).with_tunnel(Err(HostError::TunnelNotInstalled)));
    ui.app.online().set_host(host.clone());
    ui.open_menu();
    ui.fill("Pseudo", "Alice");
    ui.click(
        "Internet : tunnel gratuit, sans compte",
        Some(AccessibleRole::RadioButton),
    );
    ui.button("Héberger une partie");
    ui.wait("the host answer", |ui| !ui.online().get_error().is_empty());
    assert_eq!(ui.online().get_help(), MenuHelp::TunnelMissing);
    assert!(
        ui.online().get_error().contains("cloudflared"),
        "{}",
        ui.online().get_error()
    );
    assert!(ui.has_button("Utiliser le réseau local"));
    assert_eq!(ui.screen(), OnlineScreen::Menu);
    assert_eq!(
        server.connections(),
        0,
        "no connection was made to a server that never started"
    );

    // the offered alternative: host on the local network instead
    ui.button("Utiliser le réseau local");
    ui.wait("the room request", |_| server.hellos().len() == 1);
    assert_eq!(host.starts(), [HostMode::Tunnel, HostMode::Lan]);
    assert_eq!(ui.online().get_host_choice(), HostChoice::Lan);
    assert!(ui.online().get_help() == MenuHelp::None);
}

#[test]
fn the_menu_fits_the_minimum_window_in_its_tallest_state() {
    // Internet chosen (the longer explanation) and the cloudflared help showing.
    let ui = ui(800, 600);
    ui.app
        .online()
        .set_host(Arc::new(FakeHost::failing(HostError::TunnelNotInstalled)));
    ui.open_menu();
    ui.fill("Pseudo", "Alice");
    ui.click(
        "Internet : tunnel gratuit, sans compte",
        Some(AccessibleRole::RadioButton),
    );
    ui.button("Héberger une partie");
    ui.wait("the help", |ui| {
        ui.online().get_help() == MenuHelp::TunnelMissing
    });
    for label in [
        "Héberger une partie",
        "Rejoindre",
        "Utiliser le réseau local",
    ] {
        let e = ui.element(label, Some(AccessibleRole::Button));
        let p = e.absolute_position();
        assert!(
            p.y + e.size().height <= 600.0 && p.x + e.size().width <= 800.0,
            "{label:?} is outside the 800×600 window: {p:?}"
        );
    }
}

#[test]
fn a_port_already_in_use_is_explained() {
    let ui = ui(1000, 760);
    ui.app
        .online()
        .set_host(Arc::new(FakeHost::failing(HostError::PortInUse)));
    ui.open_menu();
    ui.fill("Pseudo", "Alice");
    ui.button("Héberger une partie");
    ui.wait("the error", |ui| !ui.online().get_error().is_empty());
    assert!(
        ui.online()
            .get_error()
            .starts_with("Le port est déjà utilisé"),
        "{}",
        ui.online().get_error()
    );
    assert_eq!(ui.online().get_help(), MenuHelp::None);
}

#[test]
fn the_binary_default_host_refuses_honestly() {
    let ui = ui(1000, 760);
    ui.open_menu();
    ui.fill("Pseudo", "Alice");
    ui.button("Héberger une partie");
    ui.wait("the error", |ui| !ui.online().get_error().is_empty());
    assert!(
        ui.online().get_error().contains("pas disponible"),
        "{}",
        ui.online().get_error()
    );
}

#[test]
fn hosting_that_stops_unexpectedly_sends_the_host_back_to_the_menu_and_cleans_up() {
    // `ServerHandle::is_running()` turned false (a crash, not a close we saw on the socket).
    let (ui, _server, host) = hosted(HostChoice::Lan);
    host.set_health(HostHealth::ServerStopped);
    ui.wait("the menu", |ui| ui.screen() == OnlineScreen::Menu);
    assert_eq!(
        ui.online().get_error(),
        "Le serveur de la partie s’est arrêté."
    );
    ui.wait("the host to be stopped", |_| host.stops() == 1);
}

#[test]
fn a_lost_tunnel_is_shown_to_the_host_without_ending_the_game() {
    let (ui, server, host) = hosted(HostChoice::Tunnel);
    assert_eq!(ui.online().get_banner_kind(), BannerKind::None);
    host.set_health(HostHealth::TunnelLost); // `Tunnel::is_running()` turned false
    ui.wait("the banner", |ui| {
        ui.online().get_banner_kind() == BannerKind::TunnelLost
    });
    assert_eq!(ui.online().get_banner_text(), "Tunnel perdu");
    assert_eq!(
        ui.screen(),
        OnlineScreen::Lobby,
        "the game goes on for the players already in"
    );
    assert!(
        ui.online().get_share_hint().contains("Tunnel perdu"),
        "{}",
        ui.online().get_share_hint()
    );
    // the Internet address is marked out of service, the local one is not
    let shared = ui.online().get_share_addresses();
    let internet = shared.row_data(0).unwrap();
    assert_eq!(internet.label, "Internet");
    assert!(internet.lost);
    assert!(!shared.row_data(1).unwrap().lost);
    ui.element(
        "Adresse à partager (Internet), tunnel perdu",
        Some(AccessibleRole::TextInput),
    );
    // and the host can still run the game
    server.send(lobby_msg(
        course_config(),
        vec![
            player(1, "Alice", true, true),
            player(2, "Bob", false, true),
        ],
    ));
    ui.wait("two players", |ui| ui.online().get_can_start());
    assert_eq!(
        host.stops(),
        0,
        "nothing is shut down: LAN players are fine"
    );
}

#[test]
fn a_lost_tunnel_still_fits_the_minimum_window_with_eight_players() {
    let ui = ui(800, 600);
    let server = MockServer::start();
    let host = Arc::new(FakeHost::ok(host_info(&server)));
    ui.app.online().set_host(host.clone());
    ui.open_menu();
    ui.fill("Pseudo", "Alice");
    ui.click(
        "Internet : tunnel gratuit, sans compte",
        Some(AccessibleRole::RadioButton),
    );
    ui.button("Héberger une partie");
    let _ = ui.next_msg(&server);
    server.send(lobby_msg(course_config(), eight_players()));
    ui.wait("the lobby", |ui| ui.screen() == OnlineScreen::Lobby);
    host.set_health(HostHealth::TunnelLost);
    ui.wait("the banner", |ui| {
        ui.online().get_banner_kind() == BannerKind::TunnelLost
    });
    let banner = ui.element("Tunnel perdu", Some(AccessibleRole::Text));
    let quit = ui.element("Quitter", Some(AccessibleRole::Button));
    let (b, q) = (banner.absolute_position(), quit.absolute_position());
    assert!(
        b.x + banner.size().width <= q.x + 0.5,
        "banner {b:?} overlaps Quitter {q:?}"
    );
    let start = ui.element("Lancer la partie", Some(AccessibleRole::Button));
    assert!(start.absolute_position().y + start.size().height <= 600.0);
}

#[test]
fn a_fresh_tunnel_address_comes_with_a_few_seconds_of_patience() {
    // The URL is printed by cloudflared before it answers: say so, then go back to the plain hint.
    let ui = ui(1000, 760);
    let server = MockServer::start();
    ui.app
        .online()
        .set_host(Arc::new(FakeHost::ok(host_info(&server))));
    let t0 = Instant::now();
    let offset = Rc::new(std::cell::Cell::new(Duration::ZERO));
    let o = offset.clone();
    ui.app.online().set_clock(Rc::new(move || t0 + o.get()));
    ui.open_menu();
    ui.fill("Pseudo", "Alice");
    ui.click(
        "Internet : tunnel gratuit, sans compte",
        Some(AccessibleRole::RadioButton),
    );
    ui.button("Héberger une partie");
    let _ = ui.next_msg(&server);
    server.send(lobby_msg(
        course_config(),
        vec![player(1, "Alice", true, true)],
    ));
    ui.wait("the lobby", |ui| ui.screen() == OnlineScreen::Lobby);
    assert_eq!(
        ui.online().get_share_hint(),
        "Partagez l’adresse dans quelques secondes."
    );
    offset.set(Duration::from_secs(20));
    ui.app.online().pump();
    assert_eq!(
        ui.online().get_share_hint(),
        "Partagez l’adresse et le code avec vos amis."
    );
}

#[test]
fn a_local_network_game_needs_no_patience() {
    let (ui, _server, _host) = hosted(HostChoice::Lan);
    // the fake's LAN answer carries a tunnel URL too; only a tunnel *choice* warms up
    assert_eq!(
        ui.online().get_share_hint(),
        "Partagez l’adresse et le code avec vos amis."
    );
}

#[test]
fn a_host_who_quits_is_warned_that_it_closes_the_game_for_everybody() {
    let (ui, server, host) = hosted(HostChoice::Lan);
    ui.button("Quitter");
    assert!(ui.online().get_confirm_leave_open());
    assert_eq!(
        ui.online().get_leave_warning(),
        "Quitter fermera la partie pour tout le monde."
    );
    ui.element(
        "Confirmation : Quitter fermera la partie pour tout le monde.",
        Some(AccessibleRole::Groupbox),
    );
    // "Rester" (focused by default) keeps the game
    ui.button("Rester");
    assert!(!ui.online().get_confirm_leave_open());
    assert_eq!(ui.screen(), OnlineScreen::Lobby);
    assert_eq!(host.stops(), 0);

    // Esc cancels too
    ui.button("Quitter");
    ui.key(Key::Escape);
    assert!(!ui.online().get_confirm_leave_open());

    // really leaving: leave_room goes out, the server and the tunnel are shut down
    ui.button("Quitter");
    ui.button("Fermer la partie");
    assert_eq!(ui.next_msg(&server), ClientMsg::LeaveRoom);
    ui.wait("the server to stop", |_| host.stops() == 1);
    assert_eq!(ui.screen(), OnlineScreen::Menu);
    assert_eq!(ui.app.online().stage(), Stage::Menu);
    assert!(ui.app.online().hosted().is_none());
}

#[test]
fn closing_the_window_as_host_asks_first_and_then_shuts_everything_down() {
    let (ui, server, host) = hosted(HostChoice::Lan);
    assert!(
        !ui.app.online().request_close(),
        "the window stays open for the question"
    );
    assert!(ui.online().get_confirm_leave_open());
    ui.button("Fermer la partie");
    assert_eq!(ui.next_msg(&server), ClientMsg::LeaveRoom);
    assert_eq!(host.stops(), 1, "stopped before the window goes (blocking)");
}

#[test]
fn closing_the_window_as_a_guest_just_leaves_the_room() {
    let (ui, server) = joined(course_config(), alice_and_bob());
    assert!(ui.app.online().request_close(), "no question for a guest");
    assert_eq!(ui.next_msg(&server), ClientMsg::LeaveRoom);
    server.wait_until("the socket to close", |s| s.closed_by_client() == 1);
}

#[test]
fn closing_the_window_in_solo_mode_changes_nothing() {
    let ui = ui(1000, 760);
    assert!(ui.app.online().request_close());
    assert_eq!(ui.app.online().stage(), Stage::Menu);
}

// --- lobby: host-only controls ------------------------------------------------------------------

#[test]
fn the_host_edits_the_settings_and_each_change_is_sent_to_the_server() {
    let (ui, server, _host) = hosted(HostChoice::Lan);
    // two players, so "Lancer la partie" is possible
    server.send(lobby_msg(
        course_config(),
        vec![
            player(1, "Alice", true, true),
            player(2, "Bob", false, true),
        ],
    ));
    ui.wait("two players", |ui| ui.online().get_can_start());

    ui.click("Coopération", Some(AccessibleRole::RadioButton));
    match ui.next_msg(&server) {
        ClientMsg::UpdateConfig { config } => assert_eq!(config.mode, Mode::Cooperation),
        other => panic!("{other:?}"),
    }
    ui.click("Difficile", Some(AccessibleRole::RadioButton));
    match ui.next_msg(&server) {
        ClientMsg::UpdateConfig { config } => {
            assert_eq!(config.difficulty, Difficulty::Difficile);
            assert_eq!(
                config.mode,
                Mode::Cooperation,
                "builds on the previous change"
            );
        }
        other => panic!("{other:?}"),
    }
    ui.button("Manches : augmenter");
    match ui.next_msg(&server) {
        ClientMsg::UpdateConfig { config } => assert_eq!(config.rounds, 6),
        other => panic!("{other:?}"),
    }
    ui.button("Joueurs max : diminuer");
    match ui.next_msg(&server) {
        ClientMsg::UpdateConfig { config } => assert_eq!(config.max_players, 7),
        other => panic!("{other:?}"),
    }
    // the server's snapshot is what the screen then shows
    let mut config = course_config();
    config.mode = Mode::Cooperation;
    config.difficulty = Difficulty::Difficile;
    config.rounds = 6;
    config.max_players = 7;
    server.send(lobby_msg(
        config,
        vec![
            player(1, "Alice", true, true),
            player(2, "Bob", false, true),
        ],
    ));
    ui.wait("the new settings", |ui| {
        ui.online().get_setting_mode() == RoomMode::Coop
    });
    assert_eq!(ui.online().get_setting_time_label(), "Temps par tour");
    assert_eq!(ui.online().get_setting_rounds(), "6");
    ui.button("Temps par tour : augmenter");
    match ui.next_msg(&server) {
        ClientMsg::UpdateConfig { config } => assert_eq!(config.turn_time_s, 50),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_guest_sees_the_settings_but_every_control_is_disabled() {
    let (ui, server) = joined(course_config(), alice_and_bob());
    assert!(!ui.online().get_settings_editable());
    assert!(ui.online().get_settings_note().contains("Seul l’hôte"));
    for label in ["Course", "Coopération", "Facile", "Difficile"] {
        let e = ui.element(label, Some(AccessibleRole::RadioButton));
        assert_eq!(
            e.accessible_enabled(),
            Some(false),
            "{label} is read-only for a guest"
        );
    }
    for label in ["Manches", "Joueurs max", "Temps par manche"] {
        let e = ui.element(label, Some(AccessibleRole::Spinbox));
        assert_eq!(e.accessible_enabled(), Some(false), "{label}");
    }
    let start = ui.element("Lancer la partie", Some(AccessibleRole::Button));
    assert_eq!(start.accessible_enabled(), Some(false));

    // clicking them does nothing
    ui.click("Coopération", Some(AccessibleRole::RadioButton));
    ui.click("Lancer la partie", Some(AccessibleRole::Button));
    assert!(ui.no_msg(&server));
}

#[test]
fn the_host_cannot_start_alone_then_can_with_a_second_player() {
    let (ui, server, _host) = hosted(HostChoice::Lan);
    let start = ui.element("Lancer la partie", Some(AccessibleRole::Button));
    assert_eq!(start.accessible_enabled(), Some(false));
    assert!(ui.online().get_start_hint().contains("2 joueurs"));
    ui.click("Lancer la partie", Some(AccessibleRole::Button));
    assert!(ui.no_msg(&server), "refused locally");

    server.send(lobby_msg(
        course_config(),
        vec![
            player(1, "Alice", true, true),
            player(2, "Bob", false, true),
        ],
    ));
    ui.wait("two players", |ui| ui.online().get_can_start());
    ui.button("Lancer la partie");
    assert_eq!(ui.next_msg(&server), ClientMsg::StartMatch);
}

fn eight_players() -> Vec<PlayerInfo> {
    let names = [
        "Alice",
        "Bob",
        "Camille",
        "Dominique",
        "Émilie",
        "François-Xavier",
        "Gaëlle",
        "Hugo",
    ];
    names
        .iter()
        .enumerate()
        .map(|(i, n)| player(i as u32 + 1, n, i == 0, i != 5))
        .collect()
}

/// Every element labelled `label` (any role) lies inside the window.
fn assert_inside(ui: &Ui, label: &str, width: f32, height: f32) {
    let e = ui.element(label, None);
    let p = e.absolute_position();
    let s = e.size();
    assert!(
        p.x >= -0.5
            && p.y >= -0.5
            && p.x + s.width <= width + 0.5
            && p.y + s.height <= height + 0.5,
        "{label:?} at {p:?} size {s:?} is outside the {width}×{height} window"
    );
}

#[test]
fn eight_players_fit_in_the_lobby_at_the_minimum_window_size() {
    let ui = ui(800, 600);
    let server = MockServer::start();
    ui.app
        .online()
        .set_host(Arc::new(FakeHost::ok(host_info(&server))));
    ui.open_menu();
    ui.fill("Pseudo", "Alice");
    ui.button("Héberger une partie");
    ui.wait("the room request", |_| server.hellos().len() == 1);
    let _ = ui.next_msg(&server);
    server.send(lobby_msg(course_config(), eight_players()));
    ui.wait("the lobby", |ui| ui.screen() == OnlineScreen::Lobby);
    let lines: Vec<_> =
        ElementHandle::find_by_element_type_name(ui.app.window(), "PlayerLine").collect();
    assert_eq!(lines.len(), 8);
    for line in &lines {
        let p = line.absolute_position();
        assert!(
            p.y + line.size().height <= 600.0,
            "a player row is outside the window: {p:?}"
        );
    }
    let start = ui.element("Lancer la partie", Some(AccessibleRole::Button));
    let p = start.absolute_position();
    assert!(
        p.y + start.size().height <= 600.0,
        "the start button is inside the window"
    );
    // the tunnel address is not cut: the host can read what they share
    let field = ui.element(
        "Adresse à partager (Internet)",
        Some(AccessibleRole::TextInput),
    );
    assert!(field.size().width >= 200.0);
    ui.element(
        "Adresse à partager (Réseau local)",
        Some(AccessibleRole::TextInput),
    );
}

#[test]
fn eight_players_fit_in_a_course_round_at_the_minimum_window_size() {
    let (ui, server) = joined_at(800, 600, course_config(), eight_players());
    start_round(
        &server,
        course_config(),
        eight_players(),
        round_started(fixture!("round_started_course_facile")),
    );
    ui.wait("the round", |ui| ui.screen() == OnlineScreen::Round);
    assert_eq!(ui.online().get_scoreboard().row_count(), 8);
    assert_inside(&ui, "Nom de la ville à deviner", 800.0, 600.0);
    assert_inside(&ui, "Essai 6 : libre", 800.0, 600.0);
    // the 8th scoreboard row is above the status strip (46 px) and the dock (168 px)
    let last = ui.online().get_scoreboard().row_data(7).unwrap();
    let e = ui.element(&last.description, Some(AccessibleRole::ListItem));
    assert!(e.absolute_position().y + e.size().height <= 600.0 - 46.0 - 168.0);
}

#[test]
fn eight_players_fit_in_a_coop_round_at_the_minimum_window_size() {
    let mut config = course_config();
    config.mode = Mode::Cooperation;
    let (ui, server) = joined_at(800, 600, config, eight_players());
    let mut start = round_started(fixture!("round_started_coop_difficile"));
    start.timer = Timer::Turn {
        player_id: 4,
        remaining_ms: 22_000,
    };
    start_round(&server, config, eight_players(), start);
    ui.wait("the round", |ui| ui.screen() == OnlineScreen::Round);
    assert_eq!(ui.online().get_scoreboard().row_count(), 8);
    assert_inside(&ui, "Nom de la ville à deviner", 800.0, 600.0);
    assert_inside(&ui, "Essai 6 : libre", 800.0, 600.0);
    let last = ui.online().get_scoreboard().row_data(7).unwrap();
    let e = ui.element(&last.description, Some(AccessibleRole::ListItem));
    assert!(e.absolute_position().y + e.size().height <= 600.0 - 46.0 - 168.0);
}

#[test]
fn eight_players_fit_in_the_results_at_the_minimum_window_size() {
    let (ui, server) = joined_at(800, 600, course_config(), eight_players());
    start_round(
        &server,
        course_config(),
        eight_players(),
        round_started(fixture!("round_started_course_facile")),
    );
    ui.wait("the round", |ui| ui.screen() == OnlineScreen::Round);
    let results = eight_players()
        .iter()
        .map(|p| boussole_protocol::PlayerRoundResult {
            player_id: p.id,
            guesses: ["Lyon", "Dijon", "Paris", "Lille", "Nice", "Brest"]
                .iter()
                .map(|c| boussole_protocol::GuessEntry {
                    city: (*c).to_owned(),
                    distance_km: 100,
                })
                .collect(),
            solved: p.id % 2 == 0,
            tries_used: 6,
            points: if p.id % 2 == 0 { 400 } else { 0 },
            total: 400,
        })
        .collect();
    server.send(ServerMsg::RoundEnded(boussole_protocol::RoundEnded {
        round_index: 1,
        total_rounds: 3,
        secret: "Besançon".into(),
        found_by: None,
        results,
        next_round_in_ms: Some(8_000),
    }));
    ui.wait("the result", |ui| ui.screen() == OnlineScreen::Result);
    assert_eq!(ui.online().get_results().row_count(), 8);
    let last = ui.online().get_results().row_data(7).unwrap();
    let e = ui.element(&last.description, Some(AccessibleRole::ListItem));
    assert!(
        e.absolute_position().y + e.size().height <= 600.0 - 46.0 - 168.0,
        "the 8th result row is above the status strip"
    );
    assert_inside(&ui, "Essai 6 : libre", 800.0, 600.0);
}

// --- rounds ---------------------------------------------------------------------------------

fn course_round() -> (Ui, MockServer) {
    let players = alice_and_bob();
    let (ui, server) = joined(course_config(), players.clone());
    start_round(
        &server,
        course_config(),
        players,
        round_started(fixture!("round_started_course_facile")),
    );
    ui.wait("the round", |ui| ui.screen() == OnlineScreen::Round);
    (ui, server)
}

fn coop_round(first_turn: u32) -> (Ui, MockServer) {
    let mut config = course_config();
    config.mode = Mode::Cooperation;
    let players = alice_and_bob();
    let (ui, server) = joined(config, players.clone());
    let mut start = round_started(fixture!("round_started_coop_difficile"));
    start.timer = Timer::Turn {
        player_id: first_turn,
        remaining_ms: 45_000,
    };
    start_round(&server, config, players, start);
    ui.wait("the round", |ui| ui.screen() == OnlineScreen::Round);
    (ui, server)
}

fn board_texts(ui: &Ui) -> Vec<String> {
    let (w, h) = ui.app.board_size();
    let _ = (w, h);
    let view = ui
        .app
        .window()
        .global::<Online>()
        .invoke_compute_board(700.0, 330.0, 0);
    view.labels.iter().map(|l| l.text.to_string()).collect()
}

#[test]
fn a_course_round_shows_km_in_facile_and_the_live_scoreboard() {
    let (ui, server) = course_round();
    let texts = board_texts(&ui);
    assert_eq!(texts.len(), 5);
    assert!(
        texts.iter().all(|t| t.contains(" km")),
        "Facile shows distances: {texts:?}"
    );
    assert_eq!(ui.online().get_scoreboard().row_count(), 2);
    assert!(ui.online().get_input_enabled());
    assert_eq!(ui.online().get_clock_text(), "1:30");

    // Bob's progress arrives: dots and no city name anywhere on screen
    server.send(ServerMsg::PlayerProgress(Progress {
        player_id: 2,
        tries_used: 3,
        solved: false,
        out_of_tries: false,
    }));
    ui.wait("progress", |ui| {
        ui.online()
            .get_scoreboard()
            .row_data(0)
            .is_some_and(|r| r.tries == 3)
    });
    let row = ui.online().get_scoreboard().row_data(0).unwrap();
    assert_eq!(row.name, "Bob");
    assert!(row.description.contains("3 sur 6"), "{}", row.description);
}

#[test]
fn difficile_arrows_never_show_a_distance() {
    let mut config = course_config();
    config.difficulty = Difficulty::Difficile;
    let players = alice_and_bob();
    let (ui, server) = joined(config, players.clone());
    start_round(
        &server,
        config,
        players,
        round_started(fixture!("round_started_coop_difficile")),
    );
    // (that fixture is a Difficile start; its timer kind is a turn, which the Course UI tolerates)
    ui.wait("the round", |ui| ui.screen() == OnlineScreen::Round);
    let texts = board_texts(&ui);
    assert_eq!(texts.len(), 5);
    assert!(
        texts.iter().all(|t| !t.contains("km") && !t.contains('·')),
        "{texts:?}"
    );
}

#[test]
fn a_course_guess_goes_out_with_the_keyboard_and_comes_back_in_the_history() {
    let (ui, server) = course_round();
    // the guess field has the focus: type a name, accents and case do not matter
    ui.h.type_text("lyon");
    ui.key(Key::Return);
    assert_eq!(
        ui.next_msg(&server),
        ClientMsg::Guess {
            city: "Lyon".into()
        }
    );
    assert!(
        !ui.online().get_input_enabled(),
        "the field waits for the answer"
    );
    assert_eq!(ui.online().get_input_message(), "Envoi…");

    server.send(ServerMsg::GuessResult(GuessInfo {
        by: 1,
        city: "Lyon".into(),
        distance_km: 163,
        bearing_deg: 24.5,
        correct: false,
        tries_used: 1,
    }));
    ui.wait("the result", |ui| {
        ui.online().get_history().row_count() == 1
    });
    assert!(ui.online().get_input_enabled());
    assert!(
        ui.online().get_query().is_empty(),
        "the field is cleared after an accepted guess"
    );
    let row = ui.online().get_history().row_data(0).unwrap();
    assert_eq!(
        (row.name.as_str(), row.distance.as_str()),
        ("Lyon", "163 km")
    );
    assert_eq!(ui.online().get_tries_used(), 1);

    // the same city again is refused locally, in French, without a message to the server
    ui.h.type_text("Lyon");
    ui.key(Key::Return);
    assert!(
        ui.online().get_notice().contains("déjà été essayée"),
        "{}",
        ui.online().get_notice()
    );
    assert!(ui.no_msg(&server));
    // and a name that is not a city too
    ui.key(Key::Escape);
    ui.key(Key::Escape);
    ui.h.type_text("Atlantide");
    ui.key(Key::Return);
    assert!(
        ui.online().get_notice().contains("Ville inconnue"),
        "{}",
        ui.online().get_notice()
    );
}

/// Typing opens the suggestion list in the dock: like in solo it must never cover the board
/// (its top is at or below the board's bottom edge) and must stay inside the window.
fn suggestions_stay_off_the_board(width: u32, height: u32) {
    let (ui, server) = joined_at(width, height, course_config(), alice_and_bob());
    start_round(
        &server,
        course_config(),
        alice_and_bob(),
        round_started(fixture!("round_started_course_facile")),
    );
    ui.wait("the round", |ui| ui.screen() == OnlineScreen::Round);
    ui.h.type_text("sa");
    let list = ui.element("Suggestions de villes", Some(AccessibleRole::List));
    let top = list.absolute_position().y;
    let bottom = top + list.size().height;
    let board_bottom = height as f32 - 46.0 - 168.0;
    assert!(
        top >= board_bottom - 0.5,
        "the list (top {top}) covers the board (bottom {board_bottom}) at {width}×{height}"
    );
    assert!(
        bottom <= height as f32 + 0.5,
        "the list is inside the window"
    );
    // typing a full list too
    ui.key(Key::Escape);
    ui.key(Key::Escape);
    ui.key(Key::DownArrow);
    let list = ui.element("Suggestions de villes", Some(AccessibleRole::List));
    assert!(list.absolute_position().y >= board_bottom - 0.5);
}

#[test]
fn the_suggestion_list_does_not_cover_the_board_at_the_default_size() {
    suggestions_stay_off_the_board(1000, 760);
}

#[test]
fn the_suggestion_list_does_not_cover_the_board_at_the_minimum_size() {
    suggestions_stay_off_the_board(800, 600);
}

#[test]
fn a_server_refusal_reopens_the_field_and_explains() {
    let (ui, server) = course_round();
    ui.h.type_text("Paris");
    ui.key(Key::Return);
    assert!(matches!(ui.next_msg(&server), ClientMsg::Guess { .. }));
    server.send(ServerMsg::error(ErrorCode::RoundOver));
    ui.wait("the notice", |ui| !ui.online().get_notice().is_empty());
    assert!(
        ui.online().get_notice().contains("terminée"),
        "{}",
        ui.online().get_notice()
    );
}

#[test]
fn course_scoreboard_marks_solved_and_out_of_tries() {
    let (ui, server) = course_round();
    server.send(ServerMsg::PlayerProgress(Progress {
        player_id: 2,
        tries_used: 4,
        solved: true,
        out_of_tries: false,
    }));
    ui.wait("progress", |ui| {
        ui.online()
            .get_scoreboard()
            .row_data(0)
            .is_some_and(|r| r.status == boussole::app::DotStatus::Solved)
    });
    let row = ui.online().get_scoreboard().row_data(0).unwrap();
    assert!(
        row.description.contains("a trouvé en 4 essais"),
        "{}",
        row.description
    );
    ui.element(&row.description, Some(AccessibleRole::ListItem));
}

#[test]
fn coop_input_is_closed_when_it_is_not_my_turn_and_opens_on_my_turn() {
    let (ui, server) = coop_round(2); // Bob plays; I am Alice (id 1)
    assert!(ui.online().get_coop());
    assert!(!ui.online().get_my_turn());
    assert_eq!(ui.online().get_turn_banner(), "Au tour de Bob…");
    assert!(!ui.online().get_input_enabled());
    assert!(ui.online().get_input_message().contains("Bob"));
    let field = ui.element("Nom de la ville à deviner", Some(AccessibleRole::TextInput));
    assert_eq!(field.accessible_read_only(), Some(true));

    // typing and Enter do nothing
    ui.h.type_text("Lyon");
    ui.key(Key::Return);
    assert!(ui.no_msg(&server));
    assert!(
        ui.app
            .online()
            .sent()
            .iter()
            .all(|m| !matches!(m, ClientMsg::Guess { .. }))
    );

    // Bob guesses, the turn comes to me
    server.send(ServerMsg::GuessResult(GuessInfo {
        by: 2,
        city: "Lyon".into(),
        distance_km: 215,
        bearing_deg: 201.5,
        correct: false,
        tries_used: 1,
    }));
    server.send(ServerMsg::TurnChanged {
        player_id: 1,
        remaining_ms: 45_000,
        reason: TurnReason::Guess,
    });
    ui.wait("my turn", |ui| ui.online().get_my_turn());
    assert_eq!(ui.online().get_turn_banner(), "À toi de jouer !");
    assert!(ui.online().get_input_enabled());
    // the shared history shows Bob's guess with his name
    let row = ui.online().get_history().row_data(0).unwrap();
    assert_eq!(
        (row.name.as_str(), row.by.as_str(), row.initial.as_str()),
        ("Lyon", "Bob", "B")
    );
    // and now I can guess
    ui.h.type_text("Dijon");
    ui.key(Key::Return);
    assert_eq!(
        ui.next_msg(&server),
        ClientMsg::Guess {
            city: "Dijon".into()
        }
    );
}

#[test]
fn the_round_clock_counts_down_on_screen() {
    let (ui, _server) = course_round();
    let t0 = Instant::now();
    let offset = Rc::new(std::cell::Cell::new(Duration::ZERO));
    let o = offset.clone();
    ui.app.online().set_clock(Rc::new(move || t0 + o.get()));
    // re-deliver the round so its deadline is relative to the controlled clock
    ui.app
        .online()
        .deliver(ServerMsg::RoundStarted(round_started(fixture!(
            "round_started_course_facile"
        ))));
    ui.app.online().sync();
    assert_eq!(ui.online().get_clock_text(), "1:30");
    assert!(!ui.online().get_clock_urgent());
    offset.set(Duration::from_secs(45));
    ui.app.online().sync();
    assert_eq!(ui.online().get_clock_text(), "0:45");
    offset.set(Duration::from_secs(83));
    ui.app.online().sync();
    assert_eq!(ui.online().get_clock_text(), "0:07");
    assert!(ui.online().get_clock_urgent());
}

// --- end of round and of the match ------------------------------------------------------------

#[test]
fn the_end_of_a_round_reveals_the_city_and_everybodys_results_then_the_next_round_starts() {
    let (ui, server) = course_round();
    server.send(fixture!("round_ended_course"));
    server.send(fixture!("room_snapshot_intermission"));
    ui.wait("the result", |ui| ui.screen() == OnlineScreen::Result);
    assert!(ui.online().get_headline().contains("Besançon"));
    assert!(
        ui.online()
            .get_countdown()
            .starts_with("Manche suivante dans")
    );
    assert_eq!(ui.online().get_results().row_count(), 2);
    let best = ui.online().get_results().row_data(0).unwrap();
    assert_eq!(best.name, "Alice");
    assert_eq!(best.points, "+496");
    assert_eq!(best.guesses, "Lyon · Dijon · Besançon");
    // the dial shows the revealed city
    let view = ui.online().invoke_compute_board(700.0, 330.0, 0);
    assert!(view.revealed);
    assert_eq!(view.text, "Besançon");
    assert!(!ui.online().get_input_enabled());

    // the next round replaces the result
    start_round(
        &server,
        course_config(),
        alice_and_bob(),
        round_started(fixture!("round_started_course_facile")),
    );
    ui.wait("the next round", |ui| ui.screen() == OnlineScreen::Round);
    assert!(ui.online().get_input_enabled());
}

#[test]
fn resuming_during_an_intermission_still_shows_the_revealed_city() {
    // A reconnecting player only gets the result in the snapshot (no hints): the dial still
    // shows the city, just without arrows.
    let (ui, server) = joined(course_config(), alice_and_bob());
    server.send(fixture!("room_snapshot_intermission"));
    ui.wait("the result", |ui| ui.screen() == OnlineScreen::Result);
    let view = ui.online().invoke_compute_board(700.0, 330.0, 0);
    assert!(view.revealed);
    assert_eq!(view.text, "Besançon");
    assert_eq!(view.arrows.row_count(), 0);
}

#[test]
fn the_final_ranking_comes_after_the_last_result_and_leads_back_to_the_lobby() {
    let (ui, server) = course_round();
    let mut last = match fixture!("round_ended_course") {
        ServerMsg::RoundEnded(r) => r,
        _ => unreachable!(),
    };
    last.round_index = 3;
    last.next_round_in_ms = None;
    server.send(ServerMsg::RoundEnded(last));
    server.send(fixture!("match_ended"));
    server.send(lobby_msg(course_config(), alice_and_bob()));
    ui.wait("the last result", |ui| ui.screen() == OnlineScreen::Result);
    assert!(ui.online().get_last_round());
    assert!(
        ui.online()
            .get_countdown()
            .starts_with("Classement final dans")
    );

    // "Voir le classement" is focused: Enter shows it at once
    ui.key(Key::Return);
    assert_eq!(ui.screen(), OnlineScreen::Podium);
    assert_eq!(ui.online().get_podium_title(), "Classement final");
    assert_eq!(ui.online().get_ranking().row_count(), 2);
    let first = ui.online().get_ranking().row_data(0).unwrap();
    assert_eq!(
        (
            first.rank.as_str(),
            first.name.as_str(),
            first.total.as_str()
        ),
        ("1", "Alice", "1 190")
    );
    assert!(first.first);

    ui.button("Retour au salon");
    assert_eq!(ui.screen(), OnlineScreen::Lobby);
}

#[test]
fn the_coop_final_shows_the_team_score() {
    let (ui, server) = coop_round(1);
    let mut last = match fixture!("round_ended_coop") {
        ServerMsg::RoundEnded(r) => r,
        _ => unreachable!(),
    };
    last.next_round_in_ms = None;
    server.send(ServerMsg::RoundEnded(last));
    server.send(ServerMsg::MatchEnded(boussole_protocol::MatchEnded {
        mode: Mode::Cooperation,
        ranking: vec![
            boussole_protocol::RankEntry {
                player_id: 2,
                nickname: "Bob".into(),
                total: 1400,
                rank: 1,
                solved_rounds: 2,
            },
            boussole_protocol::RankEntry {
                player_id: 1,
                nickname: "Alice".into(),
                total: 1400,
                rank: 1,
                solved_rounds: 1,
            },
        ],
    }));
    ui.wait("the last result", |ui| ui.screen() == OnlineScreen::Result);
    ui.key(Key::Return);
    assert_eq!(ui.screen(), OnlineScreen::Podium);
    assert_eq!(ui.online().get_podium_title(), "Score d’équipe");
    assert_eq!(ui.online().get_team_total(), "1 400 points");
}

// --- connection loss ----------------------------------------------------------------------------

#[test]
fn a_dropped_connection_shows_the_banner_then_resumes_with_the_token_and_the_same_room() {
    let (ui, server) = course_round();
    ui.h.type_text("Lyon");
    ui.key(Key::Return);
    let _ = ui.next_msg(&server);
    server.drop_connection();
    ui.wait("the banner", |ui| {
        ui.online().get_banner_kind() == BannerKind::Reconnecting
    });
    assert!(
        ui.online().get_banner_text().starts_with("Reconnexion"),
        "{}",
        ui.online().get_banner_text()
    );
    assert!(
        !ui.online().get_input_enabled(),
        "no guessing while disconnected"
    );
    assert_eq!(
        ui.screen(),
        OnlineScreen::Round,
        "the screen stays up behind the banner"
    );

    // the server recognises the token and restores the player
    ui.wait("the new hello", |_| server.hellos().len() == 2);
    match &server.hellos()[1] {
        ClientMsg::Hello { resume_token, .. } => assert!(resume_token.is_some()),
        other => panic!("{other:?}"),
    }
    // what the real server sends right after a resumed welcome
    server.send(fixture!("room_snapshot_course_round"));
    ui.wait("the snapshot", |ui| {
        ui.online().get_history().row_count() == 2
    });
    assert_eq!(ui.online().get_banner_kind(), BannerKind::None);
    assert_eq!(ui.screen(), OnlineScreen::Round);
    assert!(ui.online().get_input_enabled());
}

#[test]
fn giving_up_shows_the_lost_banner_and_a_way_back() {
    let (ui, server) = course_round();
    server.refuse_connections(true);
    server.drop_connection();
    ui.wait("the lost banner", |ui| {
        ui.online().get_banner_kind() == BannerKind::Lost
    });
    assert!(ui.online().get_banner_text().contains("Connexion perdue"));
    assert!(ui.has_button("Retour au menu"));
    ui.button("Retour au menu");
    assert_eq!(ui.screen(), OnlineScreen::Menu);
    assert_eq!(ui.app.online().stage(), Stage::Menu);
}

#[test]
fn the_header_never_overlaps_at_the_minimum_width_even_with_every_pill_showing() {
    let (ui, server) = joined_at(800, 600, course_config(), eight_players());
    start_round(
        &server,
        course_config(),
        eight_players(),
        round_started(fixture!("round_started_course_facile")),
    );
    ui.wait("the round", |ui| ui.screen() == OnlineScreen::Round);
    server.refuse_connections(true);
    server.drop_connection();
    ui.wait("the lost banner", |ui| {
        ui.online().get_banner_kind() == BannerKind::Lost
    });
    // left to right: title, clock, banner, "Retour au menu", "Quitter"
    let span = |label: &str| {
        let e = ui.element(label, None);
        let x = e.absolute_position().x;
        (x, x + e.size().width)
    };
    let clock = span(&format!("Temps restant : {}", ui.online().get_clock_text()));
    let banner = span("Connexion perdue");
    let back = span("Retour au menu");
    let quit = span("Quitter");
    for (name, left, right) in [
        ("clock/banner", clock, banner),
        ("banner/retour", banner, back),
        ("retour/quitter", back, quit),
    ] {
        assert!(
            left.1 <= right.0 + 0.5,
            "{name} overlap: {left:?} vs {right:?}"
        );
    }
    assert!(
        quit.1 <= 800.0 + 0.5,
        "Quitter is inside the window: {quit:?}"
    );
}

#[test]
fn a_server_that_forgot_us_sends_the_player_back_to_the_menu_with_an_explanation() {
    let (ui, server) = course_round();
    // a restarted server: unknown token, `resumed: false`
    server.forget_tokens();
    server.drop_connection();
    ui.wait("the new hello", |_| server.hellos().len() == 2);
    ui.wait("the menu", |ui| ui.screen() == OnlineScreen::Menu);
    assert!(!ui.online().get_error().is_empty());
}

#[test]
fn the_host_closing_the_game_sends_everybody_back_to_the_menu_without_reconnecting() {
    // A shutting-down server says goodbye with WebSocket close code 1001 and no protocol message.
    let (ui, server) = joined(course_config(), alice_and_bob());
    server.close_connection_with_code(1001);
    ui.wait("the menu", |ui| ui.screen() == OnlineScreen::Menu);
    assert_eq!(ui.online().get_error(), "L’hôte a fermé la partie.");
    assert_eq!(
        ui.online().get_banner_kind(),
        BannerKind::None,
        "not a connection blip"
    );
    assert_eq!(ui.app.online().stage(), Stage::Menu);
    assert!(ui.no_msg(&server));
    assert_eq!(
        server.connections(),
        1,
        "the client did not try to reconnect"
    );
}

#[test]
fn a_host_app_that_vanishes_sends_guests_to_the_menu_within_seconds_not_a_minute() {
    let (ui, server) = joined(course_config(), alice_and_bob());
    drop(server); // the host's process is gone: bare EOF, then a refused port
    ui.wait("the menu", |ui| ui.screen() == OnlineScreen::Menu);
    assert_eq!(ui.online().get_error(), "L’hôte a fermé la partie.");
    assert_eq!(ui.online().get_banner_kind(), BannerKind::None);
}

#[test]
fn if_the_hosts_own_server_goes_away_the_host_is_told_and_everything_is_cleaned_up() {
    let (ui, server, host) = hosted(HostChoice::Lan);
    server.close_connection_with_code(1001); // the embedded server stopped under us
    ui.wait("the menu", |ui| ui.screen() == OnlineScreen::Menu);
    assert_eq!(
        ui.online().get_error(),
        "Le serveur de la partie s’est arrêté."
    );
    ui.wait("the host to be stopped", |_| host.stops() == 1);
    assert!(ui.app.online().hosted().is_none());
}

#[test]
fn a_server_over_its_connection_cap_is_reported_on_the_menu() {
    let ui = ui(1000, 760);
    let server = MockServer::start();
    server.answer_with_http_status(Some(503));
    ui.open_menu();
    ui.fill_join(&server, "K7QX2");
    ui.button("Rejoindre");
    ui.wait("the error", |ui| !ui.online().get_error().is_empty());
    assert!(
        ui.online().get_error().contains("plein"),
        "{}",
        ui.online().get_error()
    );
    assert_eq!(ui.screen(), OnlineScreen::Menu);
}

#[test]
fn a_create_room_refused_at_the_room_cap_is_explained() {
    // The server answers `invalid_state` when it cannot hold another room.
    let ui = ui(1000, 760);
    let server = MockServer::start();
    ui.open_menu();
    ui.fill("Pseudo", "Alice");
    ui.fill("Adresse", &format!("127.0.0.1:{}", server.port));
    ui.button("Créer une salle");
    assert!(matches!(ui.next_msg(&server), ClientMsg::CreateRoom { .. }));
    server.send(ServerMsg::error(ErrorCode::InvalidState));
    ui.wait("the error", |ui| !ui.online().get_error().is_empty());
    assert!(
        ui.online().get_error().contains("créer de salle"),
        "{}",
        ui.online().get_error()
    );
    assert_eq!(ui.screen(), OnlineScreen::Menu);
}

#[test]
fn a_deleted_room_sends_everybody_back_to_the_menu() {
    let (ui, server) = joined(course_config(), alice_and_bob());
    server.send(ServerMsg::error(ErrorCode::RoomNotFound));
    ui.wait("the menu", |ui| ui.screen() == OnlineScreen::Menu);
    assert_eq!(ui.online().get_error(), "La salle a été fermée.");
}

// --- keyboard only ---------------------------------------------------------------------------

#[test]
fn the_whole_path_create_lobby_guess_works_with_the_keyboard_only() {
    let ui = ui(1000, 760);
    let server = MockServer::start();
    let host = Arc::new(FakeHost::ok(host_info(&server)));
    ui.app.online().set_host(host.clone());
    // Start screen: the level picker has the focus; Tab reaches "Nouvelle partie", then
    // "Jouer en ligne"; Enter opens the online menu.
    ui.key(Key::Tab);
    ui.key(Key::Tab);
    ui.key(Key::Return);
    assert!(ui.online().get_active(), "reached the online menu");
    // The pseudo field has the focus: type, Tab to the choice, Tab to "Héberger", Enter.
    ui.h.type_text("Alice");
    ui.key(Key::Tab); // host-mode choice
    ui.key(Key::Tab); // Héberger une partie
    ui.key(Key::Return);
    ui.wait("the room request", |_| server.hellos().len() == 1);
    assert!(matches!(ui.next_msg(&server), ClientMsg::CreateRoom { .. }));
    server.send(lobby_msg(
        course_config(),
        vec![player(1, "Alice", true, true)],
    ));
    ui.wait("the lobby", |ui| ui.screen() == OnlineScreen::Lobby);
    // A second player arrives: the start button takes the focus, Enter starts.
    server.send(lobby_msg(
        course_config(),
        vec![
            player(1, "Alice", true, true),
            player(2, "Bob", false, true),
        ],
    ));
    ui.wait("can start", |ui| ui.online().get_can_start());
    ui.key(Key::Return);
    assert_eq!(ui.next_msg(&server), ClientMsg::StartMatch);
    start_round(
        &server,
        course_config(),
        vec![
            player(1, "Alice", true, true),
            player(2, "Bob", false, true),
        ],
        round_started(fixture!("round_started_course_facile")),
    );
    ui.wait("the round", |ui| ui.screen() == OnlineScreen::Round);
    // The guess field has the focus.
    ui.h.type_text("lyo");
    ui.key(Key::DownArrow);
    ui.key(Key::Return);
    match ui.next_msg(&server) {
        ClientMsg::Guess { city } => assert!(city.starts_with("Lyo"), "{city}"),
        other => panic!("{other:?}"),
    }
}
