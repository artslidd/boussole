//! Difficulty levels, hint generation and the guess/round state machine.

use crate::cities::{CITIES, City};
use crate::geo::{angle_gap, bearing_deg, distance_km};
use crate::rng::SplitMix64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    /// Arrow labels also show the distance (`Lyon · 392 km`).
    Facile,
    /// Arrow labels show city names only.
    Difficile,
}

impl Difficulty {
    pub fn shows_distances(self) -> bool {
        matches!(self, Self::Facile)
    }

    pub fn label_fr(self) -> &'static str {
        match self {
            Self::Facile => "Facile",
            Self::Difficile => "Difficile",
        }
    }
}

/// An arrow target, seen FROM the secret city.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hint {
    pub city: &'static City,
    pub bearing_deg: f64,
    pub distance_km: f64,
}

/// Result of one guess; direction and distance go FROM the guess TO the secret.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuessFeedback {
    pub city: &'static City,
    pub distance_km: f64,
    pub bearing_deg: f64,
    pub correct: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundStatus {
    InProgress,
    Won,
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuessError {
    /// Same city already guessed in this round (does not use up a try).
    AlreadyGuessed,
    /// The round is already won or lost.
    RoundOver,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    secret: &'static City,
    hints: Vec<Hint>,
    guesses: Vec<GuessFeedback>,
    difficulty: Difficulty,
}

/// Hint window: distance from the secret, in km.
const MIN_HINT_KM: f64 = 50.0;

/// Relaxation ladder `(min pairwise bearing gap in degrees, max hint distance in km)`,
/// tried in order; the first feasible step wins. Widening the distance only happens at the
/// 35° floor, then the gap shrinks until 5 hints fit (edge-of-France secrets such as
/// Ajaccio or Brest have all their neighbours within a ~120° sector).
const RELAXATION: &[(f64, f64)] = &[
    (40.0, 700.0),
    (35.0, 700.0),
    (35.0, 1000.0),
    (30.0, 1000.0),
    (25.0, 1000.0),
    (20.0, 1000.0),
    (15.0, 1000.0),
    (10.0, 1000.0),
    (5.0, 1000.0),
    (0.0, f64::INFINITY),
];

impl Round {
    pub const MAX_GUESSES: usize = 6;
    pub const HINT_COUNT: usize = 5;

    /// Uniformly random secret; fully determined by `seed`.
    pub fn new_random(difficulty: Difficulty, seed: u64) -> Round {
        let mut rng = SplitMix64::new(seed);
        let secret = &CITIES[rng.below(CITIES.len())];
        Self::build(secret, difficulty, &mut rng)
    }

    /// Fixed secret; hints are determined by `seed`.
    pub fn with_secret(secret: &'static City, difficulty: Difficulty, seed: u64) -> Round {
        Self::build(secret, difficulty, &mut SplitMix64::new(seed))
    }

    fn build(secret: &'static City, difficulty: Difficulty, rng: &mut SplitMix64) -> Round {
        Round {
            secret,
            hints: pick_hints(secret, rng),
            guesses: Vec::new(),
            difficulty,
        }
    }

    pub fn secret(&self) -> &'static City {
        self.secret
    }

    /// Hints sorted by bearing ascending (clockwise from north).
    pub fn hints(&self) -> &[Hint] {
        &self.hints
    }

    pub fn difficulty(&self) -> Difficulty {
        self.difficulty
    }

    pub fn guesses(&self) -> &[GuessFeedback] {
        &self.guesses
    }

    pub fn status(&self) -> RoundStatus {
        if self.guesses.iter().any(|g| g.correct) {
            RoundStatus::Won
        } else if self.guesses.len() >= Self::MAX_GUESSES {
            RoundStatus::Lost
        } else {
            RoundStatus::InProgress
        }
    }

    /// Tries left.
    pub fn remaining(&self) -> usize {
        Self::MAX_GUESSES.saturating_sub(self.guesses.len())
    }

    pub fn guess(&mut self, city: &'static City) -> Result<&GuessFeedback, GuessError> {
        if self.status() != RoundStatus::InProgress {
            return Err(GuessError::RoundOver);
        }
        if self.guesses.iter().any(|g| g.city == city) {
            return Err(GuessError::AlreadyGuessed);
        }
        self.guesses.push(GuessFeedback {
            city,
            distance_km: distance_km(city, self.secret),
            bearing_deg: bearing_deg(city, self.secret),
            correct: city == self.secret,
        });
        Ok(&self.guesses[self.guesses.len() - 1])
    }
}

/// Pick `HINT_COUNT` reference cities (never the secret, at least `MIN_HINT_KM` away),
/// following the `RELAXATION` ladder, sorted by bearing.
fn pick_hints(secret: &City, rng: &mut SplitMix64) -> Vec<Hint> {
    let mut pool: Vec<Hint> = CITIES
        .iter()
        .filter(|c| c.reference && *c != secret)
        .map(|city| Hint {
            city,
            bearing_deg: bearing_deg(secret, city),
            distance_km: distance_km(secret, city),
        })
        .filter(|h| h.distance_km >= MIN_HINT_KM)
        .collect();
    rng.shuffle(&mut pool);

    for &(min_gap, max_km) in RELAXATION {
        let window: Vec<Hint> = pool
            .iter()
            .copied()
            .filter(|h| h.distance_km <= max_km)
            .collect();
        let mut chosen = Vec::with_capacity(Round::HINT_COUNT);
        if choose(&window, min_gap, &mut chosen) {
            chosen.sort_by(|a, b| a.bearing_deg.total_cmp(&b.bearing_deg));
            return chosen;
        }
    }
    unreachable!(
        "the last relaxation step accepts any {} hints",
        Round::HINT_COUNT
    )
}

/// Depth-first search, in pool order, for `HINT_COUNT` hints whose bearings are pairwise
/// at least `min_gap` degrees apart. Deterministic for a given pool order.
fn choose(pool: &[Hint], min_gap: f64, chosen: &mut Vec<Hint>) -> bool {
    if chosen.len() == Round::HINT_COUNT {
        return true;
    }
    if pool.len() < Round::HINT_COUNT - chosen.len() {
        return false;
    }
    for (i, h) in pool.iter().enumerate() {
        if chosen
            .iter()
            .all(|c| angle_gap(c.bearing_deg, h.bearing_deg) >= min_gap)
        {
            chosen.push(*h);
            if choose(&pool[i + 1..], min_gap, chosen) {
                return true;
            }
            chosen.pop();
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cities::by_name;

    #[test]
    fn difficulty_flags_and_labels() {
        assert!(Difficulty::Facile.shows_distances());
        assert!(!Difficulty::Difficile.shows_distances());
        assert_eq!(Difficulty::Facile.label_fr(), "Facile");
        assert_eq!(Difficulty::Difficile.label_fr(), "Difficile");
    }

    #[test]
    fn same_seed_gives_identical_round() {
        for seed in 0..50 {
            let a = Round::new_random(Difficulty::Facile, seed);
            let b = Round::new_random(Difficulty::Facile, seed);
            assert_eq!(a, b);
            let s = by_name("Dijon");
            assert_eq!(
                Round::with_secret(s, Difficulty::Difficile, seed),
                Round::with_secret(s, Difficulty::Difficile, seed)
            );
        }
        let secrets: std::collections::HashSet<_> = (0..200)
            .map(|s| Round::new_random(Difficulty::Facile, s).secret().name)
            .collect();
        assert!(
            secrets.len() > 60,
            "secrets should vary across seeds, got {}",
            secrets.len()
        );
    }

    #[test]
    fn with_secret_uses_given_secret_and_difficulty() {
        let r = Round::with_secret(by_name("Lyon"), Difficulty::Difficile, 7);
        assert_eq!(r.secret().name, "Lyon");
        assert_eq!(r.difficulty(), Difficulty::Difficile);
        assert_eq!(r.status(), RoundStatus::InProgress);
        assert_eq!(r.remaining(), Round::MAX_GUESSES);
        assert!(r.guesses().is_empty());
    }

    /// Documented relaxations `(secret, min pairwise gap °, max hint km)`. Everyone else must
    /// reach 35° with hints within 700 km.
    ///
    /// These secrets sit on the edge of France: all their reference neighbours lie in a
    /// narrow sector (e.g. Ajaccio/Bastia/Porto-Vecchio: only a ~100° arc towards the
    /// mainland; Brest: ~90° between NE and SE; Strasbourg: ~135°), and 5 hints 35° apart
    /// need a 140° span. Quimper reaches 35° only by widening the window to 1000 km.
    /// The ladder is exhaustive, so each value is the best step on the ladder for that secret.
    const RELAXED: &[(&str, f64, f64)] = &[
        ("Strasbourg", 30.0, 1000.0),
        ("Brest", 20.0, 1000.0),
        ("Dunkerque", 25.0, 1000.0),
        ("Ajaccio", 20.0, 1000.0),
        ("Calais", 25.0, 1000.0),
        ("Bayonne", 30.0, 1000.0),
        ("Bastia", 25.0, 1000.0),
        ("Porto-Vecchio", 15.0, 1000.0),
        ("Quimper", 35.0, 1000.0),
    ];

    fn min_gap(r: &Round) -> f64 {
        let h = r.hints();
        let mut min = f64::MAX;
        for (i, a) in h.iter().enumerate() {
            for b in &h[i + 1..] {
                min = min.min(angle_gap(a.bearing_deg, b.bearing_deg));
            }
        }
        min
    }

    fn check_hints(r: &Round) {
        let name = r.secret().name;
        let hints = r.hints();
        assert_eq!(hints.len(), Round::HINT_COUNT, "{name}");
        let (min_gap_deg, max_km) = RELAXED
            .iter()
            .find(|(n, ..)| *n == name)
            .map_or((35.0, 700.0), |&(_, gap, km)| (gap, km));
        for h in hints {
            assert!(
                h.city.reference,
                "{name}: hint {} is not a reference city",
                h.city.name
            );
            assert_ne!(h.city, r.secret(), "{name}: secret used as hint");
            assert_eq!(h.distance_km, distance_km(r.secret(), h.city));
            assert_eq!(h.bearing_deg, bearing_deg(r.secret(), h.city));
            assert!(
                (50.0..=max_km).contains(&h.distance_km),
                "{name}: hint {} at {:.0} km",
                h.city.name,
                h.distance_km
            );
        }
        let names: std::collections::HashSet<_> = hints.iter().map(|h| h.city.name).collect();
        assert_eq!(names.len(), Round::HINT_COUNT, "{name}: duplicate hints");
        assert!(
            hints
                .windows(2)
                .all(|w| w[0].bearing_deg <= w[1].bearing_deg),
            "{name}: hints not sorted by bearing"
        );
        let gap = min_gap(r);
        assert!(
            gap >= min_gap_deg,
            "{name}: min gap {gap:.1}° < {min_gap_deg}°"
        );
    }

    #[test]
    fn thousand_random_rounds_have_valid_hints() {
        for seed in 0..1000 {
            let d = if seed % 2 == 0 {
                Difficulty::Facile
            } else {
                Difficulty::Difficile
            };
            check_hints(&Round::new_random(d, seed));
        }
    }

    #[test]
    fn every_city_as_secret_has_valid_hints() {
        for city in CITIES {
            for seed in 0..10 {
                check_hints(&Round::with_secret(city, Difficulty::Facile, seed));
            }
        }
    }

    fn wrong_cities(secret: &City, n: usize) -> Vec<&'static City> {
        CITIES.iter().filter(|c| *c != secret).take(n).collect()
    }

    #[test]
    fn correct_guess_wins() {
        let mut r = Round::with_secret(by_name("Lyon"), Difficulty::Facile, 1);
        let secret = r.secret();
        let fb = *r.guess(secret).unwrap();
        assert!(fb.correct);
        assert_eq!(fb.distance_km, 0.0);
        assert_eq!(r.status(), RoundStatus::Won);
        assert_eq!(r.remaining(), Round::MAX_GUESSES - 1);
    }

    #[test]
    fn win_on_last_try_is_not_a_loss() {
        let mut r = Round::with_secret(by_name("Lyon"), Difficulty::Facile, 1);
        for c in wrong_cities(r.secret(), Round::MAX_GUESSES - 1) {
            r.guess(c).unwrap();
        }
        assert_eq!(r.status(), RoundStatus::InProgress);
        r.guess(by_name("Lyon")).unwrap();
        assert_eq!(r.status(), RoundStatus::Won);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn six_wrong_guesses_lose() {
        let mut r = Round::with_secret(by_name("Lyon"), Difficulty::Facile, 1);
        for (i, c) in wrong_cities(r.secret(), Round::MAX_GUESSES)
            .into_iter()
            .enumerate()
        {
            assert_eq!(r.status(), RoundStatus::InProgress, "before guess {i}");
            assert!(!r.guess(c).unwrap().correct);
        }
        assert_eq!(r.status(), RoundStatus::Lost);
        assert_eq!(r.remaining(), 0);
        assert_eq!(r.guesses().len(), Round::MAX_GUESSES);
    }

    #[test]
    fn duplicate_guess_is_rejected_and_free() {
        let mut r = Round::with_secret(by_name("Lyon"), Difficulty::Facile, 1);
        let paris = by_name("Paris");
        r.guess(paris).unwrap();
        assert_eq!(r.guess(paris).unwrap_err(), GuessError::AlreadyGuessed);
        assert_eq!(r.guesses().len(), 1);
        assert_eq!(r.remaining(), Round::MAX_GUESSES - 1);
    }

    #[test]
    fn guess_after_end_is_round_over() {
        let mut won = Round::with_secret(by_name("Lyon"), Difficulty::Facile, 1);
        won.guess(by_name("Lyon")).unwrap();
        assert_eq!(
            won.guess(by_name("Paris")).unwrap_err(),
            GuessError::RoundOver
        );
        assert_eq!(
            won.guess(by_name("Lyon")).unwrap_err(),
            GuessError::RoundOver
        );

        let mut lost = Round::with_secret(by_name("Lyon"), Difficulty::Facile, 1);
        for c in wrong_cities(lost.secret(), Round::MAX_GUESSES) {
            lost.guess(c).unwrap();
        }
        assert_eq!(
            lost.guess(by_name("Lyon")).unwrap_err(),
            GuessError::RoundOver
        );
        assert_eq!(lost.guesses().len(), Round::MAX_GUESSES);
    }

    #[test]
    fn feedback_is_from_guess_to_secret() {
        use crate::geo::Compass8;
        // Secret Lille, guess Paris: Lille is ~204 km to the north of Paris...
        let mut r = Round::with_secret(by_name("Lille"), Difficulty::Facile, 1);
        let fb = *r.guess(by_name("Paris")).unwrap();
        assert_eq!(fb.city.name, "Paris");
        assert!(!fb.correct);
        assert!((fb.distance_km - 204.0).abs() < 5.0, "{}", fb.distance_km);
        assert_eq!(Compass8::from_bearing(fb.bearing_deg), Compass8::N);
        assert_eq!(
            fb.bearing_deg,
            bearing_deg(by_name("Paris"), by_name("Lille"))
        );

        // ...and with the roles swapped (secret Paris, guess Lille) the arrow points south.
        let mut r = Round::with_secret(by_name("Paris"), Difficulty::Facile, 1);
        let fb = *r.guess(by_name("Lille")).unwrap();
        assert_eq!(Compass8::from_bearing(fb.bearing_deg), Compass8::S);
        assert_eq!(r.guesses().len(), 1);
    }
}
