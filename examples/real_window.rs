//! Smoke test of the *real* backend (winit window + the default renderer, not the headless
//! software one): opens the window for about three seconds, plays a few keystrokes through the
//! window system's event path, opens the À propos box and saves what the renderer drew.
//!
//! ```sh
//! cargo run --release --example real_window -- /tmp/real      # PNGs in /tmp/real/
//! ```

use boussole::app::App;
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, Timer, TimerMode};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "/tmp/real".into()),
    );
    std::fs::create_dir_all(&out)?;

    let app = App::with_seed_sequence(5)?;
    let window = app.window().as_weak();
    app.window().show()?;

    let step = Rc::new(Cell::new(0));
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
        let Some(app_window) = window.upgrade() else {
            return;
        };
        let w = app_window.window();
        match step.get() {
            0 => save(&out.join("real-1-start.png"), &w.take_snapshot().unwrap()),
            1 => press(w, Key::Return), // starts a round
            2 => {
                for ch in "lyon".chars() {
                    press(w, ch.to_string());
                }
                press(w, Key::Return);
            }
            3 => save(&out.join("real-2-round.png"), &w.take_snapshot().unwrap()),
            4 => app_window.invoke_show_about(),
            5 => save(&out.join("real-3-about.png"), &w.take_snapshot().unwrap()),
            _ => {
                let _ = slint::quit_event_loop();
            }
        }
        step.set(step.get() + 1);
    });
    app.run()?;
    Ok(())
}
