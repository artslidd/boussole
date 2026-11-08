//! How the bot guesses: keep every city that is consistent with the feedback so far (the
//! distance and bearing from each guess to the secret pin the secret down almost at once)
//! and pick one of them at random. Shared by the bot example and the integration tests.

use boussole_core::cities::{CITIES, City};
use boussole_core::geo::{bearing_deg, distance_km};
use boussole_core::search::find_exact;
use boussole_protocol::GuessInfo;

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
}

/// Smallest angle between two bearings, in degrees.
fn angle_diff(a: f64, b: f64) -> f64 {
    ((a - b + 540.0).rem_euclid(360.0) - 180.0).abs()
}
