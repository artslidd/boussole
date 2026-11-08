//! Smoke test of the online mode in the *real* backend (winit window + the default renderer):
//! hosts a game on the loopback interface with the real embedded server, lets a second client
//! join by its room code, starts a Course round, saves what the renderer drew at each step, then
//! presses *Quit* the way the application menu does (⌘Q sends `terminate:`, which the app routes
//! through the window-close path): the host is warned, confirms, the guest is told, the server
//! stops, the window closes and the process ends by itself. Nothing leaves this machine and no
//! tunnel is started.
//!
//! ```sh
//! cargo run --release --example real_window_online -- /tmp/real-online   # PNGs in that folder
//! ```
//!
//! The exit status is non-zero if a step did not happen.

use boussole::app::{App, Online, OnlineScreen};
use boussole::embedded_host::EmbeddedHost;
use boussole::net::{NetConfig, NetEvent, NetHandle};
use boussole_protocol::{ClientMsg, ServerMsg};
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, Timer, TimerMode};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::Duration;

fn save(path: &PathBuf, image: &slint::SharedPixelBuffer<slint::Rgba8Pixel>) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).expect("create png"));
    let mut encoder = png::Encoder::new(file, image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(image.as_bytes()))
        .expect("write png");
    println!("{} ({}×{})", path.display(), image.width(), image.height());
}

fn press(window: &slint::Window, text: impl Into<slint::SharedString>) {
    let text = text.into();
    window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window.dispatch_event(WindowEvent::KeyReleased { text });
}

/// A second player, outside the window: a plain protocol client.
struct Guest {
    /// Kept so the connection stays open for as long as the guest is.
    _handle: NetHandle,
    events: mpsc::Receiver<NetEvent>,
    seen: Vec<String>,
}

impl Guest {
    fn drain(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.seen.push(match event {
                NetEvent::Msg(ServerMsg::Welcome { .. }) => "welcome".into(),
                NetEvent::Msg(ServerMsg::RoomSnapshot(s)) => {
                    format!("snapshot:{:?}:{}", s.phase, s.players.len())
                }
                NetEvent::Msg(ServerMsg::RoundStarted(_)) => "round_started".into(),
                NetEvent::Msg(_) => "message".into(),
                NetEvent::Failed(error) => format!("failed:{error}"),
                other => format!("{other:?}"),
            });
        }
    }

    fn saw(&self, what: &str) -> bool {
        self.seen.iter().any(|s| s.starts_with(what))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "/tmp/real-online".into()),
    );
    std::fs::create_dir_all(&out)?;

    let app = App::with_seed_sequence(5)?;
    // Loopback only: no firewall prompt, nothing reachable from outside.
    app.online().set_host(Arc::new(EmbeddedHost::with_lan_bind(
        "127.0.0.1:0".parse()?,
    )));
    let window = app.window().as_weak();
    let online_ui = app.online().clone();
    app.online().install_quit_hook()?;
    let hosted_port = Rc::new(Cell::new(0u16));
    let hosted_port_in = hosted_port.clone();
    app.window().show()?;

    let failures = Rc::new(RefCell::new(Vec::<String>::new()));
    let failures_in = failures.clone();
    let failures_after = failures.clone();
    let guest: Rc<RefCell<Option<Guest>>> = Rc::new(RefCell::new(None));
    let guest_after = guest.clone();
    let step = Rc::new(Cell::new(0));
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(700), move || {
        let Some(app_window) = window.upgrade() else {
            return;
        };
        let w = app_window.window();
        let online = app_window.global::<Online>();
        let check = |ok: bool, what: &str| {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
            if !ok {
                failures_in.borrow_mut().push(what.to_owned());
            }
        };
        if let Some(guest) = guest.borrow_mut().as_mut() {
            guest.drain();
        }
        match step.get() {
            0 => {
                online.invoke_open();
                for ch in "Alice".chars() {
                    press(w, ch.to_string());
                }
            }
            1 => {
                check(
                    online.get_screen() == OnlineScreen::Menu,
                    "the online menu is up",
                );
                save(&out.join("online-1-menu.png"), &w.take_snapshot().unwrap());
                // Tab to the host-mode choice, Tab to "Héberger une partie", Enter
                press(w, Key::Tab);
                press(w, Key::Tab);
                press(w, Key::Return);
            }
            2 | 3 => {}
            4 => {
                check(
                    online.get_screen() == OnlineScreen::Lobby,
                    "hosting reached the lobby",
                );
                check(
                    online.get_room_code().len() == 5,
                    "the lobby shows a room code",
                );
                save(&out.join("online-2-lobby.png"), &w.take_snapshot().unwrap());
                // a second player joins by code, on the host's loopback address
                let code = online.get_room_code().to_string();
                if let Some(info) = online_ui.hosted().map(|h| h.port) {
                    hosted_port_in.set(info);
                    let (tx, events) = mpsc::channel();
                    let handle = NetHandle::connect(
                        NetConfig::new(format!("ws://127.0.0.1:{info}/ws"), "Bob"),
                        move |event| {
                            let _ = tx.send(event);
                        },
                    );
                    handle.send(ClientMsg::JoinRoom { code });
                    *guest.borrow_mut() = Some(Guest {
                        _handle: handle,
                        events,
                        seen: Vec::new(),
                    });
                } else {
                    check(false, "the hosted port is known");
                }
            }
            5 => {
                check(
                    online.get_can_start(),
                    "with two players the host can start",
                );
                save(
                    &out.join("online-3-two-players.png"),
                    &w.take_snapshot().unwrap(),
                );
                press(w, Key::Return); // the start button has the focus
            }
            6 | 7 => {}
            8 => {
                check(
                    online.get_screen() == OnlineScreen::Round,
                    "the round started",
                );
                save(&out.join("online-4-round.png"), &w.take_snapshot().unwrap());
                if let Some(guest) = guest.borrow().as_ref() {
                    check(
                        guest.saw("round_started"),
                        "the second client received the round",
                    );
                }
                // the host presses Quit in the application menu (⌘Q)
                boussole::macos_quit::send_terminate();
                #[cfg(not(target_os = "macos"))]
                online.invoke_quit_requested();
            }
            9 => {
                check(
                    online.get_confirm_leave_open(),
                    "Quit warns the host before closing the game",
                );
                check(w.is_visible(), "and the window is still open at that point");
                save(
                    &out.join("online-5-confirm.png"),
                    &w.take_snapshot().unwrap(),
                );
                online.invoke_confirm_leave(); // the window hides: the event loop ends by itself
            }
            10..=14 => {} // the process must be on its way out: nothing to do
            15 => {
                check(
                    false,
                    "the window closed and the event loop ended by itself",
                );
                let _ = slint::quit_event_loop();
            }
            _ => {}
        }
        step.set(step.get() + 1);
    });
    app.run()?; // returns once the window is hidden and the host's stop has been seen through
    if let Some(guest) = guest_after.borrow_mut().as_mut() {
        guest.drain();
        let told = guest.saw("failed:L’hôte a fermé la partie");
        println!(
            "{} the second client was told the host closed the game",
            if told { "ok  " } else { "FAIL" }
        );
        if !told {
            failures_after
                .borrow_mut()
                .push("the second client was told".to_owned());
        }
    }
    let freed = hosted_port.get() != 0
        && std::net::TcpListener::bind(("127.0.0.1", hosted_port.get())).is_ok();
    println!(
        "{} the server's port was released when the app exited",
        if freed { "ok  " } else { "FAIL" }
    );
    if !freed {
        failures_after
            .borrow_mut()
            .push("the port was released".to_owned());
    }
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("all steps passed");
        Ok(())
    } else {
        Err(format!("{} step(s) failed: {failures:?}", failures.len()).into())
    }
}
