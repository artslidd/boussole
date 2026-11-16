//! How the bot guesses: keep every city that is consistent with the feedback so far (the
//! distance and bearing from each guess to the secret pin the secret down almost at once)
//! and pick one of them at random. For Flèches, the correct arrow is recomputed from the
//! dataset and picked with a probability given by the `--skill` knob. Shared by the bot
//! example and the integration tests.

use boussole_core::cities::{CITIES, City};
use boussole_core::geo::{bearing_deg, distance_km};
use boussole_core::search::find_exact;
use boussole_protocol::{FLECHES_OPTIONS, FlechesQuestion, GuessInfo};

pub struct Strategy {
    candidates: Vec<&'static City>,
    rng: u64,
}

impl Strategy {
    pub fn new(seed: u64) -> Self {
        Self {
            candidates: CITIES.iter().collect(),
            rng: seed,
        }
    }

    /// A new round: every city is possible again.
    pub fn reset(&mut self) {
        self.candidates = CITIES.iter().collect();
    }

    /// The next city to try (never the same twice in a round).
    pub fn next_guess(&mut self) -> &'static str {
        if self.candidates.is_empty() {
            self.reset();
        }
        let i = (self.next_u64() % self.candidates.len() as u64) as usize;
        self.candidates.swap_remove(i).name
    }

    /// Learn from a guess result (own, or a teammate's in Coopération).
    pub fn observe(&mut self, result: &GuessInfo) {
        let Some(guess) = find_exact(&result.city) else {
            return;
        };
        self.candidates.retain(|c| {
            c.name != guess.name
                && distance_km(guess, c).round() as u32 == result.distance_km
                && angle_diff(bearing_deg(guess, c), result.bearing_deg) <= 0.11
        });
    }

    /// SplitMix64: plenty for choosing among cities.
    fn next_u64(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Answer a Flèches question: find the option that matches the true bearing
    /// origin → target and pick it with probability `skill` % (`--skill` knob);
    /// otherwise pick one of the other arrows at random.
    pub fn answer_fleches(&mut self, question: &FlechesQuestion, skill: u32) -> u8 {
        let correct = fleches_correct_option(question).unwrap_or(0);
        if self.next_u64() % 100 < u64::from(skill.min(100)) {
            return correct;
        }
        let wrong: Vec<u8> = (0..FLECHES_OPTIONS).filter(|&i| i != correct).collect();
        wrong[(self.next_u64() % wrong.len() as u64) as usize]
    }
}

/// The option index whose bearing points from `question.origin` to `question.target`
/// (the server rounds the true bearing to 0.1° on the wire; so does this lookup), or `None`
/// if no option matches (cannot happen with the built-in dataset).
pub fn fleches_correct_option(question: &FlechesQuestion) -> Option<u8> {
    let origin = find_exact(&question.origin)?;
    let target = find_exact(&question.target)?;
    let true_deg = bearing_deg(origin, target);
    (0..FLECHES_OPTIONS)
        .find(|&i| angle_diff(question.options_deg[usize::from(i)], true_deg) <= 0.11)
}

/// Smallest angle between two bearings, in degrees.
fn angle_diff(a: f64, b: f64) -> f64 {
    ((a - b + 540.0).rem_euclid(360.0) - 180.0).abs()
}
