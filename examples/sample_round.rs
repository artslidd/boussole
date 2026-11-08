//! Prints a sample round to stdout (the original ticket-01 smoke test; the game itself is the
//! Slint app in `src/main.rs`).
//!
//! Usage: `cargo run --example sample_round -- [SEED] [--reveal]` — the secret stays hidden unless `--reveal`.

use boussole::game::{Difficulty, Round};
use boussole::geo::Compass8;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let reveal = args.iter().any(|a| a == "--reveal");
    let seed = args.iter().find_map(|a| a.parse().ok()).unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
    });

    for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
        let round = Round::new_random(difficulty, seed);
        println!("Boussole — {} (graine {seed})", difficulty.label_fr());
        println!(
            "           ?   (ville secrète, {} essais)",
            Round::MAX_GUESSES
        );
        for hint in round.hints() {
            let compass = Compass8::from_bearing(hint.bearing_deg);
            let distance = if difficulty.shows_distances() {
                format!(" · {:.0} km", hint.distance_km)
            } else {
                String::new()
            };
            println!(
                "  {} {:<2} {:>5.1}°  {}{distance}",
                compass.arrow(),
                compass.label_fr(),
                hint.bearing_deg,
                hint.city.name
            );
        }
        if reveal {
            println!("  secret : {}", round.secret().name);
        }
        println!();
    }
}
