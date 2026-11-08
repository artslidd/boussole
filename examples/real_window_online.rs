//! Smoke test of the online mode in the *real* backend (winit window + the default renderer):
//! hosts a game on the loopback interface with the real embedded server, lets a second client
//! join by its room code, starts a Course round, saves what the renderer drew at each step and
//! closes the game. Nothing leaves this machine and no tunnel is started.
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
    handle: NetHandle,
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
    app.window().show()?;

    let failures = Rc::new(RefCell::new(Vec::<String>::new()));
    let failures_in = failures.clone();
    let guest: Rc<RefCell<Option<Guest>>> = Rc::new(RefCell::new(None));
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
                    let (tx, events) = mpsc::channel();
                    let handle = NetHandle::connect(
                        NetConfig::new(format!("ws://127.0.0.1:{info}/ws"), "Bob"),
                        move |event| {
                            let _ = tx.send(event);
                        },
                    );
                    handle.send(ClientMsg::JoinRoom { code });
                    *guest.borrow_mut() = Some(Guest {
                        handle,
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
                // the host quits: confirmation, then the game is closed for everybody
                online.invoke_leave();
            }
            9 => {
                check(
                    online.get_confirm_leave_open(),
                    "the host is warned before closing the game",
                );
                save(
                    &out.join("online-5-confirm.png"),
                    &w.take_snapshot().unwrap(),
                );
                online.invoke_confirm_leave();
            }
            10 => {
                check(
                    online.get_screen() == OnlineScreen::Menu,
                    "back on the menu",
                );
                if let Some(guest) = guest.borrow().as_ref() {
                    check(
                        guest.saw("failed:L’hôte a fermé la partie"),
                        "the second client was told the host closed the game",
                    );
                    let _ = &guest.handle;
                }
                let _ = slint::quit_event_loop();
            }
            _ => {}
        }
        step.set(step.get() + 1);
    });
    app.run()?;
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("all steps passed");
        Ok(())
    } else {
        Err(format!("{} step(s) failed: {failures:?}", failures.len()).into())
    }
}
