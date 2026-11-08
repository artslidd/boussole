//! "Héberger une partie" end to end through the real UI and the real embedded server: the host
//! hosts on the loopback interface, a second protocol client joins by room code, the host starts
//! a Course round and guesses, and quitting ends the game for everybody and releases the port.
//! Nothing here leaves the machine; no tunnel is started.

use boussole::app::{App, Online, OnlineScreen};
use boussole::embedded_host::EmbeddedHost;
use boussole::headless::Headless;
use boussole::net::{NetConfig, NetError, NetEvent, NetHandle};
use boussole_protocol::{ClientMsg, ServerMsg};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::{ComponentHandle, Model};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Ui {
    h: Headless,
    app: App,
}

impl Ui {
    fn online(&self) -> Online<'_> {
        self.app.window().global::<Online>()
    }

    fn element(&self, label: &str, role: AccessibleRole) -> ElementHandle {
        ElementHandle::find_by_accessible_label(self.app.window(), label)
            .find(|e| e.accessible_role() == Some(role))
            .unwrap_or_else(|| panic!("no {role:?} labelled {label:?}"))
    }

    fn click(&self, label: &str, role: AccessibleRole) {
        let e = self.element(label, role);
        let (p, s) = (e.absolute_position(), e.size());
        self.h.click(p.x + s.width / 2.0, p.y + s.height / 2.0);
    }

    fn button(&self, label: &str) {
        self.click(label, AccessibleRole::Button);
    }

    fn wait(&self, what: &str, condition: impl Fn(&Ui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !condition(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.h.tick(Duration::from_millis(50));
            std::thread::sleep(Duration::from_millis(2));
        }
        self.h.settle();
    }
}

#[test]
fn hosting_from_the_app_runs_a_real_game_and_quitting_ends_it_for_everybody() {
    let h = Headless::new(1000, 760, 1.0).expect("headless platform");
    let app = App::with_seed_sequence(7).expect("window");
    // Loopback: no firewall prompt, nothing reachable from outside.
    app.online().set_host(Arc::new(EmbeddedHost::with_lan_bind(
        "127.0.0.1:0".parse().unwrap(),
    )));
    app.window().show().expect("show");
    h.settle();
    let ui = Ui { h, app };

    // --- host: pseudo, then "Héberger une partie" with the keyboard path's button --------------
    ui.button("Jouer en ligne");
    ui.click("Pseudo", AccessibleRole::TextInput);
    ui.h.type_text("Alice");
    ui.button("Héberger une partie");
    ui.wait("the lobby", |ui| {
        ui.online().get_screen() == OnlineScreen::Lobby
    });
    let hosted = ui.app.online().hosted().expect("the server is running");
    assert_eq!(ui.online().get_room_code().len(), 5);
    assert!(ui.online().get_is_host());
    assert_eq!(ui.online().get_players().row_count(), 1);

    // --- a second client joins by room code, over the host's own address ---------------------------
    let (tx, events) = mpsc::channel();
    let guest = NetHandle::connect(NetConfig::new(hosted.local_url.clone(), "Bob"), move |e| {
        let _ = tx.send(e);
    });
    guest.send(ClientMsg::JoinRoom {
        code: ui.online().get_room_code().to_string(),
    });
    ui.wait("two players", |ui| {
        ui.online().get_players().row_count() == 2
    });
    assert!(ui.online().get_can_start());

    // --- the host starts a round and guesses; the answer comes from the real server -----------------
    ui.button("Lancer la partie");
    ui.wait("the round", |ui| {
        ui.online().get_screen() == OnlineScreen::Round
    });
    let hints = ui
        .online()
        .invoke_compute_board(700.0, 330.0, 0)
        .labels
        .row_count();
    assert_eq!(hints, 5, "five arrows");
    ui.h.type_text("Lyon");
    ui.h.press_key(slint::platform::Key::Return);
    ui.wait("my first guess to be answered", |ui| {
        ui.online().get_history().row_count() == 1
    });
    let first = ui.online().get_history().row_data(0).unwrap();
    assert_eq!(first.name, "Lyon");
    assert!(first.distance.ends_with("km") || first.distance == "Trouvé");
    // the other player saw the round too, and Alice's progress (a count, never a city)
    let mut guest_saw_round = false;
    while let Ok(event) = events.recv_timeout(Duration::from_millis(500)) {
        if matches!(event, NetEvent::Msg(ServerMsg::RoundStarted(_))) {
            guest_saw_round = true;
            break;
        }
    }
    assert!(guest_saw_round);

    // --- the host quits: warned first, then the game ends for everybody and the port is freed ------
    ui.button("Quitter");
    assert!(ui.online().get_confirm_leave_open());
    ui.button("Fermer la partie");
    assert_eq!(ui.online().get_screen(), OnlineScreen::Menu);
    assert!(
        ui.app
            .online()
            .wait_for_pending_stops(Duration::from_secs(10)),
        "the server finished stopping"
    );
    let ended = loop {
        match events
            .recv_timeout(Duration::from_secs(10))
            .expect("the guest's connection ends")
        {
            NetEvent::Msg(_) => continue,
            other => break other,
        }
    };
    assert_eq!(
        ended,
        NetEvent::Failed(NetError::ServerClosed),
        "the guest is told the host closed the game"
    );
    assert!(
        TcpListener::bind(("127.0.0.1", hosted.port)).is_ok(),
        "the port is free again"
    );
    assert!(ui.app.online().hosted().is_none());
}
