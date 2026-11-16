//! The Flèches solo game: presentation logic between the engine
//! (`boussole_core::arrows`) and the Slint screen (`ui/fleches.slint`).
//!
//! A [`FlechesSession`] chains the ten questions of one [`ArrowGame`]: which question is up,
//! which arrow the keyboard aims at, the verdict after an answer and the French texts of the
//! reveal. Pure Rust, no GUI types, unit-tested like a [`Session`](crate::session): the
//! Slint glue in [`crate::app`] only forwards events and renders what this module reports.
//!
//! The board itself reuses the dial geometry of [`crate::board`]: the four options are laid
//! out like the five hints of a Devinette round (arrow at the exact bearing, label pill at
//! the tip), only with numbers instead of city names until the answer reveals one of them.

use crate::arrows::{AnswerFeedback, ArrowGame, ArrowQuestion, OPTION_COUNT, QUESTIONS_PER_GAME};
use crate::board::{HintLabel, direction_phrase, format_km};
use crate::cities::City;
use crate::game::Difficulty;
use crate::geo::Compass8;

/// Angular half-width of every arrow's click zone: the board is one click zone and a click
/// is mapped to the nearest arrow when it is within this angle of it (45°, i.e. a cosine of
/// √2⁄2, pushed to the UI as `aim-min-cos`). Arrows are never less than 25° apart, so the
/// nearest arrow is always the one the player aimed at.
pub const AIM_TOLERANCE_DEG: f64 = 45.0;

/// Whether a city name starts with a vowel — accented ones included (Évreux, Orléans,
/// Angoulême) — or a mute *h*, which all elide "de" in French. The dataset has no
/// aspirated-*h* name; the only *h* cities would be mute ones like Hyères.
fn starts_with_vowel_or_mute_h(name: &str) -> bool {
    name.chars()
        .next()
        .and_then(|first| first.to_lowercase().next())
        .is_some_and(|first| "haeiouyàâäéèêëîïôöùûüœ".contains(first))
}

/// "de" before a city name, with the French elision: "De Lyon" / "D’Aurillac"
/// (capitalised, at the start of a sentence) or "de Lyon" / "d’Évreux". Every
/// vowel-initial or mute-*h* name takes the elided form.
///
/// Kept reusable: the online Flèches screens (ticket 10) need the same sentences.
pub fn de_city(name: &str, capitalized: bool) -> String {
    if starts_with_vowel_or_mute_h(name) {
        format!("{}’{name}", if capitalized { "D" } else { "d" })
    } else {
        format!("{} {name}", if capitalized { "De" } else { "de" })
    }
}

/// "De Lyon vers Brest ?" / "D’Aurillac vers Laval ?" — the question heading.
pub fn heading_text(origin: &str, target: &str) -> String {
    format!("{} vers {target} ?", de_city(origin, true))
}

/// "Bonne réponse !" / "Raté !" — the verdict.
pub fn verdict_text(is_correct: bool) -> &'static str {
    if is_correct {
        "Bonne réponse !"
    } else {
        "Raté !"
    }
}

/// "Brest est à 690 km au nord-ouest de Lyon." / "… au nord-ouest d’Aurillac." — the
/// sentence under the verdict.
pub fn detail_text(origin: &str, target: &str, distance_km: f64, compass: Compass8) -> String {
    format!(
        "{target} est à {} km {} {}.",
        format_km(distance_km),
        direction_phrase(compass),
        de_city(origin, false)
    )
}

/// "Brest · 690 km · NO" — the label the correct arrow takes once the question is answered.
pub fn reveal_label(target: &str, distance_km: f64, compass: Compass8) -> String {
    format!(
        "{target} · {} km · {}",
        format_km(distance_km),
        compass.label_fr()
    )
}

/// What the reveal of the current answer shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Reveal {
    /// The label of the correct arrow ("Brest · 690 km · NO").
    pub label: String,
    /// The sentence under the verdict ("Brest est à 690 km au nord-ouest de Lyon.").
    pub sentence: String,
}

/// One line of the end-of-game recap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecapRow {
    /// 1-based question number.
    pub number: usize,
    pub from: &'static str,
    pub to: &'static str,
    pub correct: bool,
}

/// The four arrow labels: their number while the question is up, or, once it is answered,
/// the reveal (name, rounded km, compass direction) on the correct arrow.
pub fn option_hint_labels(question: &ArrowQuestion, reveal: Option<&str>) -> Vec<HintLabel> {
    question
        .options_deg
        .iter()
        .enumerate()
        .map(|(i, &bearing)| HintLabel {
            bearing_deg: bearing,
            text: if question.correct == i
                && let Some(reveal) = reveal
            {
                reveal.to_owned()
            } else {
                (i + 1).to_string()
            },
        })
        .collect()
}

/// The Flèches game as the UI drives it: a question, one answer, a verdict, then "Suivant".
#[derive(Debug, Clone, PartialEq)]
pub struct FlechesSession {
    game: ArrowGame,
    /// Arrow the keyboard aims at, in `0..OPTION_COUNT`.
    focus: usize,
    /// The answer just given, on screen until [`FlechesSession::advance`].
    feedback: Option<AnswerFeedback>,
    /// `(origin, target)` of every answered question, in order: the engine's history does
    /// not carry city names, the recap needs them.
    pairs: Vec<(&'static City, &'static City)>,
}

impl FlechesSession {
    /// A game fully determined by `(difficulty, seed)`.
    pub fn new(difficulty: Difficulty, seed: u64) -> Self {
        Self {
            game: ArrowGame::new(difficulty, seed),
            focus: 0,
            feedback: None,
            pairs: Vec::with_capacity(QUESTIONS_PER_GAME),
        }
    }

    /// The question on screen (still returned while its feedback shows, until the last
    /// `advance` of the game).
    pub fn question(&self) -> Option<&ArrowQuestion> {
        self.game.current()
    }

    /// 0-based index of the question on screen.
    pub fn index(&self) -> usize {
        self.game.index()
    }

    /// The difficulty this game was built with.
    pub fn difficulty(&self) -> Difficulty {
        self.game.difficulty()
    }

    /// Answers given so far, in question order.
    pub fn history(&self) -> &[AnswerFeedback] {
        self.game.history()
    }

    /// Correctly answered questions so far.
    pub fn score(&self) -> usize {
        self.game.score()
    }

    /// The answer on screen, until "Suivant".
    pub fn feedback(&self) -> Option<&AnswerFeedback> {
        self.feedback.as_ref()
    }

    /// Whether the current question has been answered (the verdict is showing).
    pub fn is_answered(&self) -> bool {
        self.feedback.is_some()
    }

    /// True once the last question's feedback has been dismissed: score-screen time. (The
    /// engine reports `is_over` as soon as the tenth answer is recorded; the feedback of
    /// that tenth question still has to be seen and dismissed first.)
    pub fn is_over(&self) -> bool {
        self.game.is_over() && self.feedback.is_none()
    }

    /// The arrow the keyboard aims at.
    pub fn focus(&self) -> usize {
        self.focus
    }

    /// ← / →: aim the arrow `delta` steps away, wrapping around.
    pub fn move_focus(&mut self, delta: i32) {
        self.focus = (self.focus as i32 + delta).rem_euclid(OPTION_COUNT as i32) as usize;
    }

    /// Answer the current question with arrow `option` (`0..OPTION_COUNT`). Refused (`None`),
    /// recording nothing, when the question is already answered, when the game is over, or
    /// for an out-of-range arrow.
    pub fn answer(&mut self, option: usize) -> Option<AnswerFeedback> {
        if self.feedback.is_some() || option >= OPTION_COUNT {
            return None;
        }
        let question = self.game.current()?;
        let pair = (question.origin, question.target);
        let feedback = self.game.answer(option).ok()?;
        self.pairs.push(pair);
        // The verdict moves the keyboard on to the arrow that was just played.
        self.focus = option;
        self.feedback = Some(feedback);
        Some(feedback)
    }

    /// "Suivant": dismiss the verdict and move on — to the next question, or (after the
    /// tenth) to the score screen. Returns `false` when no verdict is showing.
    pub fn advance(&mut self) -> bool {
        if self.feedback.take().is_none() {
            return false;
        }
        self.game.advance();
        true
    }

    /// The reveal of the answer on screen, once the question is answered.
    pub fn reveal(&self) -> Option<Reveal> {
        let feedback = self.feedback.as_ref()?;
        let question = self.game.current()?;
        Some(Reveal {
            label: reveal_label(question.target.name, feedback.distance_km, feedback.compass),
            sentence: detail_text(
                question.origin.name,
                question.target.name,
                feedback.distance_km,
                feedback.compass,
            ),
        })
    }

    /// The questions answered so far, in order, for the recap.
    pub fn recap(&self) -> Vec<RecapRow> {
        self.game
            .history()
            .iter()
            .zip(&self.pairs)
            .enumerate()
            .map(|(i, (feedback, &(from, to)))| RecapRow {
                number: i + 1,
                from: from.name,
                to: to.name,
                correct: feedback.is_correct,
            })
            .collect()
    }

    /// The engine game, for tests and screenshots.
    pub fn engine(&self) -> &ArrowGame {
        &self.game
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cities::CITIES;
    use crate::geo::bearing_deg;
    use std::collections::HashSet;

    fn question_at(difficulty: Difficulty, seed: u64) -> (ArrowQuestion, ArrowGame) {
        let game = ArrowGame::new(difficulty, seed);
        let question = game.current().expect("first question").clone();
        (question, game)
    }

    #[test]
    fn texts_are_french() {
        assert_eq!(heading_text("Lyon", "Brest"), "De Lyon vers Brest ?");
        assert_eq!(heading_text("Aurillac", "Laval"), "D’Aurillac vers Laval ?");
        assert_eq!(verdict_text(true), "Bonne réponse !");
        assert_eq!(verdict_text(false), "Raté !");
        assert_eq!(
            detail_text("Lyon", "Brest", 690.4, Compass8::NW),
            "Brest est à 690 km au nord-ouest de Lyon."
        );
        assert_eq!(
            detail_text("Aurillac", "Laval", 427.3, Compass8::N),
            "Laval est à 427 km au nord d’Aurillac."
        );
        assert_eq!(
            reveal_label("Brest", 690.4, Compass8::NW),
            "Brest · 690 km · NO"
        );
        assert_eq!(
            reveal_label("Brest", 999.6, Compass8::SW),
            "Brest · 1 000 km · SO"
        );
    }

    #[test]
    fn de_elides_before_every_vowel_initial_city_of_the_dataset() {
        // spot checks, accents and case included
        assert_eq!(de_city("Lyon", true), "De Lyon");
        assert_eq!(de_city("Lyon", false), "de Lyon");
        assert_eq!(de_city("Aurillac", true), "D’Aurillac");
        assert_eq!(de_city("Évreux", false), "d’Évreux");
        assert_eq!(de_city("Orléans", true), "D’Orléans");
        assert_eq!(de_city("Ajaccio", false), "d’Ajaccio");
        assert_eq!(de_city("Angoulême", false), "d’Angoulême");
        // a mute-h name elides too (none in the dataset yet, the rule must still hold)
        assert_eq!(de_city("Hyères", true), "D’Hyères");

        // the whole dataset: every name gets exactly one of the two forms, and the
        // vowel-initial ones (there are many) all elide.
        let mut vowel_initial = 0;
        for city in CITIES {
            let elided = starts_with_vowel_or_mute_h(city.name);
            assert_eq!(
                de_city(city.name, true),
                if elided {
                    format!("D’{}", city.name)
                } else {
                    format!("De {}", city.name)
                },
                "{}",
                city.name
            );
            assert_eq!(
                de_city(city.name, false),
                if elided {
                    format!("d’{}", city.name)
                } else {
                    format!("de {}", city.name)
                },
                "{}",
                city.name
            );
            vowel_initial += usize::from(elided);
        }
        assert!(
            vowel_initial >= 15,
            "the dataset has vowel-initial cities ({vowel_initial}), the elision matters"
        );
    }

    #[test]
    fn labels_are_numbers_then_the_reveal_on_the_correct_arrow() {
        let (question, _) = question_at(Difficulty::Facile, 3);
        let labels = option_hint_labels(&question, None);
        assert_eq!(
            labels.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["1", "2", "3", "4"]
        );
        for (i, label) in labels.iter().enumerate() {
            assert_eq!(label.bearing_deg, question.options_deg[i]);
        }
        let reveal = "Brest · 690 km · NO";
        let labels = option_hint_labels(&question, Some(reveal));
        for (i, label) in labels.iter().enumerate() {
            let expected = if i == question.correct {
                reveal
            } else {
                &(i + 1).to_string()
            };
            assert_eq!(label.text, expected);
        }
    }

    #[test]
    fn a_full_game_flows_through_ten_questions() {
        let difficulty = Difficulty::Difficile;
        let mut session = FlechesSession::new(difficulty, 42);
        let mut expected_score = 0;
        let mut seen_pairs = HashSet::new();
        for i in 0..QUESTIONS_PER_GAME {
            let question = session
                .question()
                .expect("question while the game runs")
                .clone();
            assert_eq!(session.index(), i);
            assert!(!session.is_over());
            assert!(!session.is_answered());
            assert!(seen_pairs.insert((question.origin.name, question.target.name)));
            // Right on even questions, wrong on odd ones.
            let chosen = if i.is_multiple_of(2) {
                question.correct
            } else {
                (question.correct + 2) % OPTION_COUNT
            };
            let feedback = session.answer(chosen).expect("first answer accepted");
            assert_eq!(feedback.chosen, chosen);
            assert_eq!(feedback.correct, question.correct);
            assert_eq!(feedback.is_correct, chosen == question.correct);
            assert_eq!(
                feedback.distance_km, question.distance_km,
                "the feedback reveals the true distance"
            );
            assert_eq!(feedback.compass, question.compass());
            expected_score += usize::from(chosen == question.correct);
            assert_eq!(session.score(), expected_score);
            assert!(session.is_answered());
            // One answer per question.
            assert_eq!(session.answer(chosen), None);
            assert_eq!(session.answer(0), None);
            // The reveal is available while the verdict shows.
            let reveal = session.reveal().expect("a reveal after the answer");
            assert_eq!(
                reveal.label,
                reveal_label(question.target.name, feedback.distance_km, feedback.compass)
            );
            // Suivant moves on (except the last time, when it ends the game).
            assert!(session.advance());
        }
        assert_eq!(session.question(), None);
        assert!(session.is_over());
        assert!(!session.is_answered());
        assert_eq!(session.score(), QUESTIONS_PER_GAME / 2);
        assert!(!session.advance(), "nothing left to dismiss");
        assert_eq!(session.answer(0), None, "the game is over");
        // The recap tells every question with its verdict.
        let recap = session.recap();
        assert_eq!(recap.len(), QUESTIONS_PER_GAME);
        for (i, row) in recap.iter().enumerate() {
            assert_eq!(row.number, i + 1);
            assert_eq!(row.correct, i.is_multiple_of(2));
            assert!(!row.from.is_empty() && !row.to.is_empty());
        }
        // The pairs match the engine's history length.
        assert_eq!(session.history().len(), QUESTIONS_PER_GAME);
    }

    #[test]
    fn focus_wraps_around_and_follows_the_answer() {
        let mut session = FlechesSession::new(Difficulty::Facile, 1);
        assert_eq!(session.focus(), 0);
        session.move_focus(1);
        assert_eq!(session.focus(), 1);
        session.move_focus(-1);
        session.move_focus(-1);
        assert_eq!(session.focus(), 3, "left from the first wraps to the last");
        session.move_focus(1);
        assert_eq!(session.focus(), 0);
        session.answer(2).expect("answer");
        assert_eq!(session.focus(), 2, "the aim moved to the arrow just played");
    }

    #[test]
    fn invalid_answers_are_refused_and_change_nothing() {
        let mut session = FlechesSession::new(Difficulty::Facile, 5);
        for bad in [OPTION_COUNT, usize::MAX] {
            assert_eq!(session.answer(bad), None);
        }
        assert_eq!(session.history().len(), 0);
        assert!(!session.is_answered());
        assert!(!session.advance(), "no verdict to dismiss");
        assert_eq!(session.index(), 0, "no question was skipped");
    }

    #[test]
    fn the_game_matches_the_engine_for_the_same_seed() {
        for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
            for seed in 0..30u64 {
                let mut session = FlechesSession::new(difficulty, seed);
                let mut engine = ArrowGame::new(difficulty, seed);
                while !session.is_over() {
                    let session_q = session.question().expect("question");
                    let engine_q = engine.current().expect("question");
                    assert_eq!(session_q, engine_q);
                    let chosen = engine_q.correct;
                    let from_session = session.answer(chosen).expect("answer");
                    let from_engine = engine.answer(chosen).expect("answer");
                    assert_eq!(from_session, from_engine);
                    assert!(session.advance(), "the verdict was showing");
                    // the engine's advance returns false after the tenth question: the game
                    // was over already, the verdict still had to be dismissed
                    engine.advance();
                }
                assert!(engine.is_over());
                assert_eq!(session.engine().history(), engine.history());
                assert_eq!(session.score(), engine.score());
            }
        }
    }

    #[test]
    fn the_board_shows_the_true_bearing_of_every_option() {
        // The options are laid out by src::board at their exact engine bearing; a quick
        // sanity check here that the labels carry exactly those bearings.
        for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
            for seed in 0..20u64 {
                let (question, _) = question_at(difficulty, seed);
                let labels = option_hint_labels(&question, None);
                for (i, label) in labels.iter().enumerate() {
                    assert_eq!(label.bearing_deg, question.options_deg[i]);
                    assert!((0.0..360.0).contains(&label.bearing_deg));
                }
                // and the correct option is the true bearing origin → target
                assert_eq!(
                    question.options_deg[question.correct],
                    bearing_deg(question.origin, question.target)
                );
            }
        }
    }
}
