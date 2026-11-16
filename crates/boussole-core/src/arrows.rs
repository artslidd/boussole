//! The Flèches game: from a named start city, pick the arrow that points to a named target.
//!
//! An [`ArrowQuestion`] offers [`OPTION_COUNT`] bearings leaving the start city
//! ([`ArrowQuestion::origin`]); exactly one is the true bearing towards
//! [`ArrowQuestion::target`] (as computed by [`crate::geo::bearing_deg`]), the
//! other three are synthetic distractor bearings. Distractors keep a difficulty-dependent
//! minimum angular separation from each other and from the answer ([`min_gap_deg`]: 60° in
//! Facile, 25° in Difficile); in Difficile, at least one distractor lies within 25–45° of
//! the answer so the choice stays genuinely hard. The position of the answer among the
//! options is uniform over `0..OPTION_COUNT`.
//!
//! An [`ArrowGame`] chains [`QUESTIONS_PER_GAME`] such questions, one answer each, and is
//! fully deterministic for a given `(difficulty, seed)` pair: the same seed always replays
//! the same game. Generation always terminates — city pairs and layouts are retried a
//! bounded number of times, then produced by deterministic fallbacks.

use crate::cities::{CITIES, City};
use crate::game::Difficulty;
use crate::geo::{Compass8, angle_gap, bearing_deg, distance_km};
use crate::rng::SplitMix64;

/// Arrows (option bearings) offered per question.
pub const OPTION_COUNT: usize = 4;

/// Questions per [`ArrowGame`]; the score is out of this many.
pub const QUESTIONS_PER_GAME: usize = 10;

/// Minimum great-circle distance between the origin and the target of a question, in km.
pub const MIN_DISTANCE_KM: f64 = 100.0;

/// Minimum pairwise angular gap between option bearings, in degrees: 60° in Facile, 25° in
/// Difficile. Four bearings that far apart can never be confused with each other, while
/// Difficile stays tight enough to require real geographic thinking.
pub fn min_gap_deg(difficulty: Difficulty) -> f64 {
    match difficulty {
        Difficulty::Facile => 60.0,
        Difficulty::Difficile => 25.0,
    }
}

/// Lower bound (inclusive), in degrees from the correct bearing, of the window inside which
/// Difficile must place at least one distractor.
const NEAR_MIN_DEG: f64 = 25.0;
/// Upper bound (inclusive) of that window.
const NEAR_MAX_DEG: f64 = 45.0;

/// Random `(origin, target)` draws tried before falling back to a deterministic scan.
const MAX_PAIR_DRAWS: usize = 64;

/// Random option layouts tried before falling back to the fixed one.
const MAX_LAYOUT_DRAWS: usize = 1000;

/// One Flèches question: 4 arrows around `origin`, one of which points at `target`.
///
/// Deterministic for a given `(difficulty, seed)`: the same inputs build the same question.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrowQuestion {
    /// Start city, at the centre of the dial.
    pub origin: &'static City,
    /// City to aim at, named in the question header.
    pub target: &'static City,
    /// The [`OPTION_COUNT`] option bearings in display order, each in `[0, 360)`.
    pub options_deg: [f64; OPTION_COUNT],
    /// Index in `options_deg` of the true bearing `origin → target`.
    pub correct: usize,
    /// Great-circle distance `origin → target` in km (≥ [`MIN_DISTANCE_KM`]); round it for
    /// display.
    pub distance_km: f64,
}

impl ArrowQuestion {
    /// One question drawn with a random `(origin, target)` pair, fully determined by
    /// `seed`.
    ///
    /// Both cities are drawn from all of [`crate::cities::CITIES`], not only the
    /// `reference` ones: in Flèches both cities are named on screen, and `reference` is a
    /// Devinette criterion for hint arrows, not for targets.
    pub fn new_random(difficulty: Difficulty, seed: u64) -> Self {
        let mut rng = SplitMix64::new(seed);
        Self::generate(difficulty, &mut rng, &mut Vec::new())
    }

    /// Compass direction of the correct arrow — the direction `origin → target` points.
    pub fn compass(&self) -> Compass8 {
        Compass8::from_bearing(self.options_deg[self.correct])
    }

    /// Draw one question with `rng`, skipping `(origin, target)` pairs already `used` in
    /// the same game.
    fn generate(
        difficulty: Difficulty,
        rng: &mut SplitMix64,
        used: &mut Vec<(&'static City, &'static City)>,
    ) -> Self {
        let (origin, target) = pick_pair(rng, used);
        used.push((origin, target));
        let correct_deg = bearing_deg(origin, target);
        let (options_deg, correct) = generate_options(difficulty, correct_deg, rng);
        ArrowQuestion {
            origin,
            target,
            options_deg,
            correct,
            distance_km: distance_km(origin, target),
        }
    }
}

/// Outcome of one answer, revealed immediately (also for wrong picks).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnswerFeedback {
    /// Option index the player picked.
    pub chosen: usize,
    /// Option index of the correct arrow.
    pub correct: usize,
    /// Whether `chosen == correct`.
    pub is_correct: bool,
    /// Distance `origin → target` in km, for the reveal; round it for display.
    pub distance_km: f64,
    /// Compass direction of the correct arrow.
    pub compass: Compass8,
}

/// Why [`ArrowGame::answer`] refused to record an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerError {
    /// The current question was already answered (advance with
    /// [`ArrowGame::advance`]).
    AlreadyAnswered,
    /// All [`QUESTIONS_PER_GAME`] questions have been answered.
    GameOver,
    /// `option` is not an index in `0..OPTION_COUNT`.
    InvalidOption,
}

/// A full Flèches game: [`QUESTIONS_PER_GAME`] questions, one answer each, deterministic
/// for a given `(difficulty, seed)` pair.
///
/// Flow: [`answer`](ArrowGame::answer) the [`current`](ArrowGame::current) question, read
/// the returned feedback, then [`advance`](ArrowGame::advance) to move on. A question is
/// never skipped: `advance` only moves past an answered question.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrowGame {
    questions: Vec<ArrowQuestion>,
    cursor: usize,
    history: Vec<AnswerFeedback>,
    difficulty: Difficulty,
}

impl ArrowGame {
    /// A game fully determined by `(difficulty, seed)`: all questions are generated up
    /// front, so replaying a seed replays the game.
    pub fn new(difficulty: Difficulty, seed: u64) -> Self {
        let mut rng = SplitMix64::new(seed);
        let mut used = Vec::with_capacity(QUESTIONS_PER_GAME);
        let questions = (0..QUESTIONS_PER_GAME)
            .map(|_| ArrowQuestion::generate(difficulty, &mut rng, &mut used))
            .collect();
        ArrowGame {
            questions,
            cursor: 0,
            history: Vec::with_capacity(QUESTIONS_PER_GAME),
            difficulty,
        }
    }

    /// The current question. Still returned while it is answered but not yet advanced
    /// past (the UI keeps it on screen to reveal the feedback), and `None` once the game
    /// is over and `advance` has moved past the last question.
    pub fn current(&self) -> Option<&ArrowQuestion> {
        self.questions.get(self.cursor)
    }

    /// 0-based index of the current question — [`QUESTIONS_PER_GAME`] once the game is
    /// over and the last question has been advanced past.
    pub fn index(&self) -> usize {
        self.cursor
    }

    /// Answer the current question with the option index `option` (in
    /// `0..OPTION_COUNT`), recording the outcome. Refuses with
    /// [`AnswerError::GameOver`] once the game is over, [`AnswerError::AlreadyAnswered`]
    /// if the current question was already answered (see [`ArrowGame::advance`]), and
    /// [`AnswerError::InvalidOption`]
    /// for any other index (nothing is recorded in any of those cases).
    pub fn answer(&mut self, option: usize) -> Result<AnswerFeedback, AnswerError> {
        if self.is_over() {
            return Err(AnswerError::GameOver);
        }
        if self.current_answered() {
            return Err(AnswerError::AlreadyAnswered);
        }
        if option >= OPTION_COUNT {
            return Err(AnswerError::InvalidOption);
        }
        let question = &self.questions[self.cursor];
        let feedback = AnswerFeedback {
            chosen: option,
            correct: question.correct,
            is_correct: option == question.correct,
            distance_km: question.distance_km,
            compass: question.compass(),
        };
        self.history.push(feedback);
        Ok(feedback)
    }

    /// Advance past the current question. Returns `true` when a new unanswered question
    /// became current; `false` when the game is over — in particular when the current
    /// question has not been answered yet, in which case nothing happens (a question is
    /// never skipped).
    ///
    /// Named `advance`, not `next`, to avoid confusion with [`Iterator::next`].
    pub fn advance(&mut self) -> bool {
        if !self.current_answered() {
            return false;
        }
        self.cursor += 1;
        self.cursor < QUESTIONS_PER_GAME
    }

    /// Number of correctly answered questions so far.
    pub fn score(&self) -> usize {
        self.history.iter().filter(|f| f.is_correct).count()
    }

    /// `true` once all [`QUESTIONS_PER_GAME`] questions have been answered (the score is
    /// final then, even before the final `advance`).
    pub fn is_over(&self) -> bool {
        self.history.len() >= QUESTIONS_PER_GAME
    }

    /// The answers given so far, in question order.
    pub fn history(&self) -> &[AnswerFeedback] {
        &self.history
    }

    /// The difficulty this game was built with.
    pub fn difficulty(&self) -> Difficulty {
        self.difficulty
    }

    /// Whether the current question has already been answered (at most one answer ahead
    /// of the cursor, so this is a simple length comparison).
    fn current_answered(&self) -> bool {
        self.history.len() > self.cursor
    }
}

/// Pick a `(origin, target)` pair: different cities, at least [`MIN_DISTANCE_KM`] apart,
/// and not already `used` in this game. Random draws are tried [`MAX_PAIR_DRAWS`] times,
/// then every pair is scanned in index order from a random start — with 120 cities spread
/// over France, thousands of pairs qualify, so the scan always finds one and generation
/// always terminates.
fn pick_pair(
    rng: &mut SplitMix64,
    used: &[(&'static City, &'static City)],
) -> (&'static City, &'static City) {
    for _ in 0..MAX_PAIR_DRAWS {
        let origin = &CITIES[rng.below(CITIES.len())];
        let target = &CITIES[rng.below(CITIES.len())];
        if pair_ok(origin, target, used) {
            return (origin, target);
        }
    }
    let n = CITIES.len();
    let start = rng.below(n);
    for i in 0..n {
        let origin = &CITIES[(start + i) % n];
        for j in 0..n {
            let target = &CITIES[(start + j) % n];
            if pair_ok(origin, target, used) {
                return (origin, target);
            }
        }
    }
    unreachable!("10 questions cannot exhaust the pairs ≥ {MIN_DISTANCE_KM} km apart");
}

fn pair_ok(
    origin: &'static City,
    target: &'static City,
    used: &[(&'static City, &'static City)],
) -> bool {
    origin != target
        && distance_km(origin, target) >= MIN_DISTANCE_KM
        && !used.contains(&(origin, target))
}

/// Build the option layout of one question around `correct_deg`, the true bearing.
///
/// Returns the [`OPTION_COUNT`] bearings in display order and the index of the correct
/// one (uniform over `0..OPTION_COUNT`). Facile draws three free bearings; Difficile pins
/// one distractor within 25–45° of the answer (on a random side) and draws the other two
/// freely. A candidate layout is accepted once all its pairwise gaps reach
/// [`min_gap_deg`]; after [`MAX_LAYOUT_DRAWS`] misses, the fixed fallback layout is used —
/// Facile: 90° apart around the answer; Difficile: the near distractor at 30° (a random
/// side), then 180° and 270°. Both satisfy every rule, so generation always terminates.
fn generate_options(
    difficulty: Difficulty,
    correct_deg: f64,
    rng: &mut SplitMix64,
) -> ([f64; OPTION_COUNT], usize) {
    let min_gap = min_gap_deg(difficulty);
    let mut distractors = None;
    for _ in 0..MAX_LAYOUT_DRAWS {
        let candidate = match difficulty {
            Difficulty::Facile => [360.0 * rng.unit(), 360.0 * rng.unit(), 360.0 * rng.unit()],
            Difficulty::Difficile => [
                wrap_deg(correct_deg + near_offset(rng)),
                360.0 * rng.unit(),
                360.0 * rng.unit(),
            ],
        };
        if all_gaps_at_least(
            &[correct_deg, candidate[0], candidate[1], candidate[2]],
            min_gap,
        ) {
            distractors = Some(candidate);
            break;
        }
    }
    let distractors =
        distractors.unwrap_or_else(|| fallback_distractors(difficulty, correct_deg, rng));
    let correct = rng.below(OPTION_COUNT);
    let mut options_deg = [0.0; OPTION_COUNT];
    let mut next_distractor = distractors.iter();
    for (i, slot) in options_deg.iter_mut().enumerate() {
        *slot = if i == correct {
            correct_deg
        } else {
            *next_distractor.next().expect("3 distractors for 4 options")
        };
    }
    (options_deg, correct)
}

/// Signed offset of the Difficile near distractor: 25–45° on a random side of the answer.
fn near_offset(rng: &mut SplitMix64) -> f64 {
    let magnitude = NEAR_MIN_DEG + (NEAR_MAX_DEG - NEAR_MIN_DEG) * rng.unit();
    if rng.below(2) == 0 {
        magnitude
    } else {
        -magnitude
    }
}

/// Fixed fallback layout (see [`generate_options`]). Facile: evenly spaced 90° apart.
/// Difficile: one distractor 30° off the answer (a random side), the others at 180° and
/// 270° — pairwise gaps of 30°, 60°+ everywhere, and the near distractor inside the
/// 25–45° window.
fn fallback_distractors(
    difficulty: Difficulty,
    correct_deg: f64,
    rng: &mut SplitMix64,
) -> [f64; 3] {
    match difficulty {
        Difficulty::Facile => [
            wrap_deg(correct_deg + 90.0),
            wrap_deg(correct_deg + 180.0),
            wrap_deg(correct_deg + 270.0),
        ],
        Difficulty::Difficile => {
            let side = if rng.below(2) == 0 { 1.0 } else { -1.0 };
            [
                wrap_deg(correct_deg + 30.0 * side),
                wrap_deg(correct_deg + 180.0),
                wrap_deg(correct_deg + 270.0),
            ]
        }
    }
}

/// Whether all pairwise circular gaps between `bearings` reach at least `min_gap`.
fn all_gaps_at_least(bearings: &[f64], min_gap: f64) -> bool {
    for (i, &a) in bearings.iter().enumerate() {
        for &b in &bearings[i + 1..] {
            if angle_gap(a, b) < min_gap {
                return false;
            }
        }
    }
    true
}

/// Wrap an angle into `[0, 360)`; the guard mirrors [`bearing_deg`](crate::geo::bearing_deg)
/// because `rem_euclid` can round a tiny negative angle up to exactly 360.0.
fn wrap_deg(deg: f64) -> f64 {
    let d = deg.rem_euclid(360.0);
    if d >= 360.0 { 0.0 } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Tolerance for floating-point comparisons of angles against rule bounds.
    const EPS: f64 = 1e-9;

    fn distractors_of(q: &ArrowQuestion) -> [f64; 3] {
        let mut d = [0.0; 3];
        let mut i = 0;
        for (j, &o) in q.options_deg.iter().enumerate() {
            if j != q.correct {
                d[i] = o;
                i += 1;
            }
        }
        d
    }

    /// Every rule a single question must satisfy (all but in-game pair uniqueness).
    fn check_question(q: &ArrowQuestion, difficulty: Difficulty) {
        let answer = q.options_deg[q.correct];
        assert_ne!(q.origin.name, q.target.name, "origin == target");
        assert!(
            q.distance_km >= MIN_DISTANCE_KM,
            "{} → {}: {:.1} km",
            q.origin.name,
            q.target.name,
            q.distance_km
        );
        assert_eq!(q.distance_km, distance_km(q.origin, q.target));
        assert!(q.correct < OPTION_COUNT);
        for &o in &q.options_deg {
            assert!((0.0..360.0).contains(&o), "option {o}° out of range");
        }
        assert_eq!(answer, bearing_deg(q.origin, q.target));
        let min_gap = min_gap_deg(difficulty);
        for (i, &a) in q.options_deg.iter().enumerate() {
            for &b in &q.options_deg[i + 1..] {
                assert!(
                    angle_gap(a, b) >= min_gap - EPS,
                    "{} → {}: gap between {a}° and {b}° below {min_gap}°",
                    q.origin.name,
                    q.target.name
                );
            }
        }
        if difficulty == Difficulty::Difficile {
            assert!(
                distractors_of(q).iter().any(|&d| {
                    let gap = angle_gap(d, answer);
                    (NEAR_MIN_DEG - EPS..=NEAR_MAX_DEG + EPS).contains(&gap)
                }),
                "{} → {}: no distractor within 25–45° of the answer",
                q.origin.name,
                q.target.name
            );
        }
        assert_eq!(q.compass(), Compass8::from_bearing(answer));
    }

    #[test]
    fn min_gap_per_difficulty() {
        assert_eq!(min_gap_deg(Difficulty::Facile), 60.0);
        assert_eq!(min_gap_deg(Difficulty::Difficile), 25.0);
    }

    /// Acceptance: 10 000 seeds per level, every question of every game follows every rule,
    /// and the correct index is uniform (22–28 % per position).
    #[test]
    fn ten_thousand_games_per_level_follow_every_rule() {
        for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
            let mut counts = [0usize; OPTION_COUNT];
            for seed in 0..10_000u64 {
                let mut game = ArrowGame::new(difficulty, seed);
                assert_eq!(game.difficulty(), difficulty);
                let mut pairs = HashSet::new();
                for i in 0..QUESTIONS_PER_GAME {
                    assert_eq!(game.index(), i);
                    assert!(!game.is_over());
                    let q = game.current().expect("question while the game runs");
                    check_question(q, difficulty);
                    assert!(
                        pairs.insert((q.origin.name, q.target.name)),
                        "repeated pair {} → {}",
                        q.origin.name,
                        q.target.name
                    );
                    counts[q.correct] += 1;
                    let (correct, distance_km, compass) = (q.correct, q.distance_km, q.compass());
                    let fb = game.answer(correct).unwrap();
                    assert!(fb.is_correct);
                    assert_eq!(fb.chosen, correct);
                    assert_eq!(fb.correct, correct);
                    assert_eq!(fb.distance_km, distance_km);
                    assert_eq!(fb.compass, compass);
                    game.advance();
                }
                assert!(game.is_over());
                assert_eq!(game.score(), QUESTIONS_PER_GAME);
                assert!(!game.advance());
                assert_eq!(game.current(), None);
                assert_eq!(game.index(), QUESTIONS_PER_GAME);
                assert_eq!(game.history().len(), QUESTIONS_PER_GAME);
            }
            let total = 10_000 * QUESTIONS_PER_GAME;
            for (i, &c) in counts.iter().enumerate() {
                let freq = c as f64 / total as f64;
                assert!(
                    (0.22..=0.28).contains(&freq),
                    "correct index {i}: {freq} ({c}/{total})"
                );
            }
        }
    }

    #[test]
    fn same_seed_gives_identical_games() {
        for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
            for seed in 0..200u64 {
                assert_eq!(
                    ArrowGame::new(difficulty, seed),
                    ArrowGame::new(difficulty, seed)
                );
            }
        }
        assert_ne!(
            ArrowGame::new(Difficulty::Facile, 0),
            ArrowGame::new(Difficulty::Facile, 1)
        );
        for seed in 0..100u64 {
            assert_eq!(
                ArrowQuestion::new_random(Difficulty::Difficile, seed),
                ArrowQuestion::new_random(Difficulty::Difficile, seed)
            );
        }
        // Origins vary across seeds, not just the first city of the dataset.
        let origins: HashSet<_> = (0..1000u64)
            .map(|s| {
                ArrowGame::new(Difficulty::Facile, s)
                    .current()
                    .unwrap()
                    .origin
                    .name
            })
            .collect();
        assert!(
            origins.len() > 60,
            "origins should vary, got {}",
            origins.len()
        );
    }

    #[test]
    fn standalone_questions_are_valid() {
        for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
            for seed in 0..1000u64 {
                check_question(&ArrowQuestion::new_random(difficulty, seed), difficulty);
            }
        }
    }

    #[test]
    fn fallback_layouts_satisfy_every_rule() {
        for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
            for correct_deg in [0.0, 1.5, 90.0, 180.0, 269.9, 300.0, 359.99] {
                // Several streams so both Difficile near-distractor sides are covered.
                for seed in 0..4u64 {
                    let mut rng = SplitMix64::new(seed);
                    let d = fallback_distractors(difficulty, correct_deg, &mut rng);
                    let all = [correct_deg, d[0], d[1], d[2]];
                    for &b in &all {
                        assert!((0.0..360.0).contains(&b), "{b}° out of range");
                    }
                    assert!(all_gaps_at_least(&all, min_gap_deg(difficulty)));
                    if difficulty == Difficulty::Difficile {
                        assert!(
                            d.iter().any(|&x| {
                                let gap = angle_gap(x, correct_deg);
                                (NEAR_MIN_DEG - EPS..=NEAR_MAX_DEG + EPS).contains(&gap)
                            }),
                            "fallback near distractor outside 25–45°"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn answer_errors_behave() {
        let mut game = ArrowGame::new(Difficulty::Facile, 42);

        // advance() before answering does not skip.
        assert!(!game.advance());
        assert_eq!(game.index(), 0);
        assert_eq!(game.history().len(), 0);

        // Invalid options are refused and recorded nothing.
        for bad in [OPTION_COUNT, OPTION_COUNT + 1, usize::MAX] {
            assert_eq!(game.answer(bad).unwrap_err(), AnswerError::InvalidOption);
        }
        assert_eq!(game.history().len(), 0);

        // A second answer for the same question is AlreadyAnswered until advance().
        let correct = game.current().unwrap().correct;
        assert!(game.answer(correct).unwrap().is_correct);
        assert_eq!(game.answer(0).unwrap_err(), AnswerError::AlreadyAnswered);
        assert_eq!(game.history().len(), 1);
        assert_eq!(game.score(), 1);

        // advance() moves past exactly one answered question, never an unanswered one.
        assert!(game.advance());
        assert_eq!(game.index(), 1);
        assert!(!game.advance());
        assert_eq!(game.index(), 1);
        let q = game.current().unwrap();
        let (correct, distance_km, compass) = (q.correct, q.distance_km, q.compass());
        let wrong = (correct + 1) % OPTION_COUNT;
        let fb = game.answer(wrong).unwrap();
        assert!(!fb.is_correct);
        assert_eq!(fb.chosen, wrong);
        assert_eq!(fb.correct, correct);
        assert_eq!(fb.distance_km, distance_km);
        assert_eq!(fb.compass, compass);
        assert_eq!(game.score(), 1);
    }

    #[test]
    fn ten_answers_end_the_game_with_the_right_score() {
        for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
            let mut game = ArrowGame::new(difficulty, 7);
            let mut expected = 0;
            while !game.is_over() {
                let correct = game.current().unwrap().correct;
                // Correct on even questions, wrong on odd ones.
                let chosen = if game.index().is_multiple_of(2) {
                    correct
                } else {
                    (correct + 2) % OPTION_COUNT
                };
                let fb = game.answer(chosen).unwrap();
                assert_eq!(fb.is_correct, chosen == correct);
                expected += usize::from(chosen == correct);
                assert_eq!(game.score(), expected);
                game.advance();
            }
            assert_eq!(game.score(), QUESTIONS_PER_GAME / 2);
            assert_eq!(game.history().len(), QUESTIONS_PER_GAME);
            for (i, fb) in game.history().iter().enumerate() {
                assert_eq!(fb.is_correct, i.is_multiple_of(2));
            }
            // After the 10th answer the game is over: no next question, no more answers.
            assert_eq!(game.answer(0).unwrap_err(), AnswerError::GameOver);
            assert!(!game.advance());
            assert_eq!(game.current(), None);
            assert_eq!(game.index(), QUESTIONS_PER_GAME);
            assert_eq!(game.answer(0).unwrap_err(), AnswerError::GameOver);
        }
    }
}
