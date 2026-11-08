//! Renders the real UI headlessly (software renderer, no window) and writes PNG screenshots
//! to `docs/screenshots/`. The games are scripted with fixed secrets and seeds, so the output
//! is reproducible.
//!
//! ```sh
//! cargo run --release --example screenshots            # all scenes into docs/screenshots/
//! cargo run --release --example screenshots -- out/    # elsewhere
//! ```

use boussole::app::{App, Level};
use boussole::headless::Headless;
use boussole::search::find_exact;
use boussole::session::Phase;
use slint::platform::Key;
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// Screenshots are rendered at twice the logical size so text stays crisp.
const SCALE: f32 = 2.0;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn save_png(path: &Path, image: &SharedPixelBuffer<Rgba8Pixel>) -> Result<()> {
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(path)?),
        image.width(),
        image.height(),
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(image.as_bytes())?;
    Ok(())
}

struct Studio {
    headless: Headless,
    app: App,
    out: PathBuf,
}

impl Studio {
    fn shoot(&self, name: &str) -> Result<()> {
        let image = self.headless.snapshot(self.app.window());
        let path = self.out.join(name);
        save_png(&path, &image)?;
        println!("{} ({}×{})", path.display(), image.width(), image.height());
        Ok(())
    }

    fn key(&self, key: Key) {
        self.headless.press_key(key);
    }

    /// Start a round with a chosen secret at the given level.
    fn start(&self, secret: &str, level: Level) {
        self.app.window().invoke_choose_level(level);
        assert!(
            self.app
                .start_round_with_secret(find_exact(secret).expect("known city"), 7),
            "a round is already in progress"
        );
        self.headless.settle();
    }

    /// Type a name and press Enter, like a player.
    fn guess(&self, name: &str) {
        self.headless.type_text(name);
        self.key(Key::Return);
    }

    /// Close the suggestion list, then empty the field (Esc, Esc).
    fn clear_field(&self) {
        self.key(Key::Escape);
        self.key(Key::Escape);
    }

    /// Win the round if it is still running, so the next scene can start a new one.
    fn finish(&self, secret: &str) {
        if self.app.session().phase() == Phase::Playing {
            self.guess(secret);
        }
    }

    fn resize(&self, width: u32, height: u32) {
        self.headless.resize(width, height, SCALE);
        self.headless.settle();
    }
}

fn main() -> Result<()> {
    let out = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("docs/screenshots"), PathBuf::from);
    std::fs::create_dir_all(&out)?;

    let headless = Headless::new(1000, 760, SCALE)?;
    let app = App::with_seed_sequence(1)?;
    app.window().show()?;
    headless.settle();
    let studio = Studio { headless, app, out };

    // Start screen, at the default size and at the minimum size
    studio.shoot("01-start.png")?;
    studio.resize(800, 600);
    studio.shoot("02-start-800x600.png")?;
    studio.resize(1000, 760);

    // Facile: distances on the arrow labels, two guesses in the history
    studio.start("Besançon", Level::Facile);
    studio.guess("Lyon");
    studio.guess("Paris");
    studio.shoot("03-facile-round.png")?;

    // Autocomplete while typing, then the list of every city (chevron, or Down on an empty field)
    studio.headless.type_text("sa");
    studio.shoot("04-autocomplete.png")?;
    studio.clear_field();
    studio.key(Key::DownArrow);
    studio.shoot("05-all-cities.png")?;
    studio.key(Key::Escape);

    // A rejected duplicate guess: the notice replaces the keyboard hint, no try is used
    studio.guess("Lyon");
    studio.shoot("06-duplicate-notice.png")?;
    studio.clear_field();
    studio.finish("Besançon");
    studio.shoot("07-won.png")?;

    // Difficile: names only, same secret and seed so the boards are comparable
    studio.start("Besançon", Level::Difficile);
    studio.guess("Lyon");
    studio.guess("Paris");
    studio.shoot("08-difficile-round.png")?;
    for wrong in ["Lille", "Nantes", "Brest", "Nice"] {
        studio.guess(wrong);
    }
    studio.shoot("09-lost.png")?;

    // Facile at the minimum window size
    studio.start("Besançon", Level::Facile);
    studio.resize(800, 600);
    studio.shoot("10-facile-800x600.png")?;
    studio.resize(1000, 760);
    studio.finish("Besançon");

    // Tightest board: Porto-Vecchio (hints 15° apart, up to 1000 km), default and minimum size
    studio.start("Porto-Vecchio", Level::Facile);
    studio.shoot("11-porto-vecchio-facile.png")?;
    studio.resize(800, 600);
    studio.shoot("12-porto-vecchio-800x600.png")?;
    studio.resize(1000, 760);
    studio.finish("Porto-Vecchio");

    // À propos (Slint attribution)
    studio.app.window().invoke_show_about();
    studio.headless.settle();
    studio.shoot("13-about.png")?;
    Ok(())
}
