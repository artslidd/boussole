//! Renders the real Flèches UI headlessly (software renderer, no window) and writes PNG
//! screenshots to `docs/screenshots/fleches/`. The games are scripted with fixed seeds, so
//! the output is reproducible.
//!
//! ```sh
//! cargo run --release --example fleches_screenshots            # into docs/screenshots/fleches/
//! cargo run --release --example fleches_screenshots -- out/    # elsewhere
//! ```
//!
//! Seed 26 was picked for a photogenic first game (Nancy → Perpignan, 715 km, then Aurillac
//! → Laval); the later games simply follow the seed sequence (27, 28…).

use boussole::app::App;
use boussole::arrows::{OPTION_COUNT, QUESTIONS_PER_GAME};
use boussole::headless::Headless;
use slint::platform::Key;
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Screenshots are rendered at twice the logical size so text stays crisp.
const SCALE: f32 = 2.0;

/// The first Flèches game uses this seed (the example starts it first).
const SEED: u64 = 26;

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

    /// Let the accidental-Enter guards run out, then press Enter ("Suivant" / start).
    fn suivant(&self) {
        self.headless.advance(Duration::from_millis(800));
        self.key(Key::Return);
    }

    /// The current question, straight from the session.
    fn question(&self) -> boussole::arrows::ArrowQuestion {
        self.app
            .session()
            .fleches()
            .and_then(|f| f.question())
            .cloned()
            .expect("a question is up")
    }

    /// Answer with the digit of arrow `option` (0-based), then take "Suivant".
    fn answer(&self, option: usize) {
        self.headless.press_key((option + 1).to_string());
        self.suivant();
    }

    fn resize(&self, width: u32, height: u32) {
        self.headless.resize(width, height, SCALE);
        self.headless.settle();
    }
}

fn main() -> Result<()> {
    let out = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("docs/screenshots/fleches"), PathBuf::from);
    std::fs::create_dir_all(&out)?;

    let headless = Headless::new(1000, 760, SCALE)?;
    let app = App::with_seed_sequence(SEED)?;
    app.window().show()?;
    headless.settle();
    let studio = Studio { headless, app, out };

    // Start screen with Flèches selected (keyboard: Tab reaches the game choice).
    studio.key(Key::Tab);
    studio.key(Key::RightArrow);
    studio.shoot("01-start-fleches.png")?;
    studio.resize(800, 600);
    studio.shoot("02-start-fleches-800x600.png")?;
    studio.resize(1000, 760);

    // Facile, question 1: the correct arrow, then a wrong one, then the score.
    studio.key(Key::Return);
    studio.shoot("03-facile-question.png")?;

    // A right answer: the green arrow, the reveal label, "Bonne réponse !".
    let question = studio.question();
    studio
        .headless
        .press_key((question.correct + 1).to_string());
    studio.shoot("04-bonne-reponse.png")?;
    studio.suivant();

    // A wrong answer on question 2: the red arrow, the correct one in green.
    let question = studio.question();
    let wrong = (question.correct + 2) % OPTION_COUNT;
    studio.headless.press_key((wrong + 1).to_string());
    studio.shoot("05-rate.png")?;
    studio.suivant();

    // Play the rest (right on even questions, wrong on odd ones) to the score screen.
    for index in 2..QUESTIONS_PER_GAME {
        let question = studio.question();
        let chosen = if index.is_multiple_of(2) {
            question.correct
        } else {
            (question.correct + 1) % OPTION_COUNT
        };
        studio.answer(chosen);
    }
    studio.shoot("06-score.png")?;

    // Difficile at the default size: from the score screen (the level choice has the
    // focus, like on the start screen), Right picks Difficile, Enter starts the next
    // game of the seed sequence.
    studio.key(Key::RightArrow);
    studio.suivant();
    studio.shoot("07-difficile-question.png")?;

    // Finish that game to reach its score screen.
    for _ in 0..QUESTIONS_PER_GAME {
        let question = studio.question();
        studio.answer(question.correct);
    }

    // Back to Facile, at the minimum window: the choices are remembered except the level,
    // changed back with the keyboard on the score screen.
    studio.key(Key::LeftArrow);
    studio.suivant();
    studio.resize(800, 600);
    studio.shoot("08-question-800x600.png")?;
    Ok(())
}
