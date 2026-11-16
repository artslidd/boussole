//! End-to-end tests of the Flèches mode, driven through the real `.slint` UI rendered
//! headlessly: full ten-question games with the keyboard only and with the mouse only, in
//! both levels, the accidental-Enter guard around the verdict, the polar click zone of the
//! board, accessible names, and the layout of the real window at the default and minimum
//! sizes. The games are compared against a fresh `ArrowGame` built with the same seed.

use boussole::app::{App, GameType, Phase};
use boussole::arrows::{ArrowGame, OPTION_COUNT, QUESTIONS_PER_GAME};
use boussole::fleches::{self, heading_text, verdict_text};
use boussole::game::Difficulty;
use boussole::headless::Headless;
use boussole::session::Phase as SessionPhase;
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::Key;
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// Header + question strip + dock of `ui/fleches.slint`: the board gets the rest of the
/// window, the same chrome heights as the Devinette round.
const CHROME_HEIGHT: f32 = 52.0 + 46.0 + 168.0;

struct Ui {
    h: Headless,
    app: App,
}

fn ui(width: u32, height: u32) -> Ui {
    let h = Headless::new(width, height, 1.0).expect("headless platform");
    let app = App::with_seed_sequence(100).expect("window");
    app.window().show().expect("show");
    h.settle();
    Ui { h, app }
}

impl Ui {
    fn phase(&self) -> SessionPhase {
        self.app.session().phase()
    }

    fn key(&self, key: Key) {
        self.h.press_key(key);
    }

    /// Let the accidental-Enter guards (the verdict's and the score screen's) run out.
    fn after_answer(&self) {
        self.h.advance(Duration::from_millis(800));
    }

    /// Answer, then press Suivant (the guard has run out).
    fn answer_and_next(&self, option: usize) {
        self.h.press_key((option + 1).to_string());
        self.after_answer();
        self.key(Key::Return);
    }

    fn element(&self, label: &str, role: Option<AccessibleRole>) -> ElementHandle {
        ElementHandle::find_by_accessible_label(self.app.window(), label)
            .find(|e| role.is_none_or(|r| e.accessible_role() == Some(r)))
            .unwrap_or_else(|| panic!("no element labelled {label:?} ({role:?})"))
    }

    fn click(&self, label: &str, role: Option<AccessibleRole>) {
        let p = self.element(label, role).absolute_position();
        let s = self.element(label, role).size();
        self.h.click(p.x + s.width / 2.0, p.y + s.height / 2.0);
    }

    /// The Flèches board as computed for the real canvas size.
    fn board(&self) -> boussole::app::FlechesBoardView {
        let (w, h) = self.app.board_size();
        self.app.window().invoke_compute_fleches_board(w, h, 0)
    }

    /// Select Flèches on the start screen with the keyboard and start the game.
    fn start_fleches_keyboard(&self) {
        self.key(Key::Tab); // the level picker has the focus: Tab reaches the game picker
        self.key(Key::RightArrow);
        assert_eq!(self.app.window().get_game_type(), GameType::Fleches);
        self.key(Key::Return);
        assert_eq!(self.phase(), SessionPhase::Playing);
    }
}

/// Plays a whole Flèches game through the real UI, right on even questions and wrong on odd
/// ones, and checks every screen against `expected`, a fresh engine game on the same seed.
/// Returns the number of correct answers (always 5).
fn play_full_game(ui: &Ui, expected: &mut ArrowGame) {
    let window = ui.app.window();
    // The aim starts on arrow 1 and follows the arrow just played (the session moves it
    // there with every answer).
    let mut focus = 0usize;
    for index in 0..QUESTIONS_PER_GAME {
        let question = expected.current().expect("engine question").clone();
        assert_eq!(ui.phase(), SessionPhase::Playing);
        assert!(!window.get_fleches_answered());
        assert_eq!(window.get_fleches_focused(), focus as i32);

        // The strip asks the engine's question, with the right progress and score.
        assert_eq!(
            window.get_fleches_heading(),
            heading_text(question.origin.name, question.target.name)
        );
        assert_eq!(window.get_fleches_question_index(), index as i32);
        assert_eq!(
            window.get_fleches_question_total(),
            QUESTIONS_PER_GAME as i32
        );
        assert_eq!(window.get_fleches_score() as usize, expected.score());

        // The board: the origin in the dial, four numbered pills at the engine's bearings.
        let board = ui.board();
        assert_eq!(board.text, question.origin.name);
        assert_eq!(board.options.iter().count(), OPTION_COUNT);
        for (i, option) in board.options.iter().enumerate() {
            assert_eq!(option.label.text, (i + 1).to_string());
        }

        // ← → aim an arrow, then answer it with its number.
        ui.key(Key::RightArrow);
        ui.key(Key::RightArrow);
        ui.key(Key::LeftArrow);
        focus = (focus + 1) % OPTION_COUNT;
        assert_eq!(window.get_fleches_focused(), focus as i32);

        let chosen = if index.is_multiple_of(2) {
            question.correct
        } else {
            (question.correct + 1) % OPTION_COUNT
        };
        ui.h.press_key((chosen + 1).to_string());
        focus = chosen;

        // The verdict, the chosen arrow and the reveal on the correct arrow.
        let feedback = expected.answer(chosen).unwrap();
        assert!(window.get_fleches_answered());
        assert_eq!(window.get_fleches_chosen(), chosen as i32);
        assert_eq!(
            window.get_fleches_verdict(),
            verdict_text(feedback.is_correct)
        );
        assert_eq!(window.get_fleches_verdict_good(), feedback.is_correct);
        assert_eq!(window.get_fleches_correct(), question.correct as i32);
        assert_eq!(
            window.get_fleches_detail(),
            fleches::detail_text(
                question.origin.name,
                question.target.name,
                feedback.distance_km,
                feedback.compass,
            )
        );
        let board = ui.board();
        assert_eq!(
            board
                .options
                .iter()
                .nth(question.correct)
                .unwrap()
                .label
                .text,
            fleches::reveal_label(question.target.name, feedback.distance_km, feedback.compass)
        );
        for (i, option) in board.options.iter().enumerate() {
            let expected_label = if i == question.correct {
                fleches::reveal_label(question.target.name, feedback.distance_km, feedback.compass)
            } else {
                (i + 1).to_string()
            };
            assert_eq!(option.label.text, expected_label);
        }

        // Suivant, with the accidental-Enter guard.
        assert_eq!(
            window.get_fleches_next_label(),
            if index + 1 == QUESTIONS_PER_GAME {
                "Voir le score"
            } else {
                "Suivant"
            }
        );
        ui.key(Key::Return); // still guarded: the verdict stays
        assert!(window.get_fleches_answered());
        ui.after_answer();
        ui.key(Key::Return);
        assert!(!window.get_fleches_answered(), "Suivant moved on");

        expected.advance();
    }
    assert!(expected.is_over());
}

// --- keyboard only ------------------------------------------------------------------------

#[test]
fn a_full_facile_game_with_the_keyboard_only() {
    let ui = ui(1000, 760);
    let window = ui.app.window();
    assert_eq!(window.get_game_type(), GameType::Devinette, "the default");

    ui.start_fleches_keyboard();
    assert!(ui.app.session().fleches().is_some());
    assert!(ui.app.session().round().is_none());
    assert_eq!(window.get_level_badge(), "Flèches · Facile");

    let mut expected = ArrowGame::new(Difficulty::Facile, 100);
    play_full_game(&ui, &mut expected);

    // The score screen: the recap of the ten questions and the running score.
    assert_eq!(ui.phase(), SessionPhase::Ended);
    assert_eq!(window.get_phase(), Phase::Ended);
    assert_eq!(window.get_fleches_score(), 5);
    let recap = window.get_fleches_recap();
    assert_eq!(recap.iter().count(), QUESTIONS_PER_GAME);
    let correct_rows = recap.iter().filter(|row| row.correct).count();
    assert_eq!(correct_rows, 5);
    for (index, (row, feedback)) in recap.iter().zip(expected.history()).enumerate() {
        assert_eq!(row.number, (index + 1) as i32);
        assert_eq!(row.correct, feedback.is_correct);
    }
    ui.element("Nouvelle partie", Some(AccessibleRole::Button));
    ui.element(
        "Devinette : Ville cachée au centre",
        Some(AccessibleRole::RadioButton),
    );
    ui.element(
        "Flèches : Visez la ville cible",
        Some(AccessibleRole::RadioButton),
    );

    // A stray Enter (auto-repeat or a quick double-tap) does not start the next game…
    ui.h.press_key_repeat(Key::Return);
    ui.key(Key::Return);
    assert_eq!(ui.phase(), SessionPhase::Ended, "the score stays up");
    // …a deliberate one does, with the choices remembered.
    ui.after_answer();
    ui.key(Key::Return);
    assert_eq!(ui.phase(), SessionPhase::Playing);
    assert!(ui.app.session().fleches().is_some());
    assert_eq!(window.get_level_badge(), "Flèches · Facile");
    // the second game plays the engine's seed 101
    assert_eq!(
        ui.app.session().fleches().unwrap().engine(),
        &ArrowGame::new(Difficulty::Facile, 101)
    );
}

#[test]
fn a_full_difficile_game_with_the_keyboard_only() {
    let ui = ui(1000, 760);
    let window = ui.app.window();

    // Difficile first, then Flèches, then play.
    ui.key(Key::RightArrow); // the level picker has the focus
    assert_eq!(window.get_level(), boussole::app::Level::Difficile);
    ui.start_fleches_keyboard();
    assert_eq!(window.get_level_badge(), "Flèches · Difficile");

    let mut expected = ArrowGame::new(Difficulty::Difficile, 100);
    // Answer everything right this time.
    for index in 0..QUESTIONS_PER_GAME {
        let question = expected.current().expect("question").clone();
        ui.h.press_key((question.correct + 1).to_string());
        assert_eq!(window.get_fleches_verdict(), "Bonne réponse !");
        assert_eq!(window.get_fleches_score() as usize, index + 1);
        expected.answer(question.correct).unwrap();
        expected.advance();
        ui.after_answer();
        ui.key(Key::Return);
    }
    assert_eq!(ui.phase(), SessionPhase::Ended);
    assert_eq!(window.get_fleches_score(), 10);
    let recap = window.get_fleches_recap();
    assert!(recap.iter().all(|row| row.correct));
}

// --- mouse only ---------------------------------------------------------------------------

#[test]
fn a_full_game_with_the_mouse_only_then_back_to_devinette() {
    let ui = ui(1000, 760);
    let window = ui.app.window();

    // Choose Flèches and start, by clicking.
    ui.click(
        "Flèches : Visez la ville cible depuis la ville de départ",
        None,
    );
    assert_eq!(window.get_game_type(), GameType::Fleches);
    ui.click("Nouvelle partie", None);
    assert_eq!(ui.phase(), SessionPhase::Playing);

    // One question answered through the polar click zone (a click along the arrow, not on
    // its pill): the nearest arrow wins.
    let board = ui.board();
    let question = ui
        .app
        .session()
        .fleches()
        .unwrap()
        .question()
        .unwrap()
        .clone();
    let radius = (board.hit_inner + board.hit_outer) / 2.0;
    let option = board.options.iter().nth(question.correct).unwrap();
    ui.h.click(
        board.cx + option.dir_x * radius,
        board.cy + option.dir_y * radius,
    );
    assert!(window.get_fleches_answered(), "the click aimed at an arrow");
    assert_eq!(window.get_fleches_chosen(), question.correct as i32);
    ui.after_answer();
    ui.click("Suivant", Some(AccessibleRole::Button));

    // The nine others by clicking the pill buttons.
    for index in 1..QUESTIONS_PER_GAME {
        let question = ui
            .app
            .session()
            .fleches()
            .unwrap()
            .question()
            .unwrap()
            .clone();
        assert_eq!(ui.phase(), SessionPhase::Playing);
        ui.click(
            &format!("Flèche {}", question.correct + 1),
            Some(AccessibleRole::Button),
        );
        assert!(window.get_fleches_answered());
        ui.after_answer();
        let label = if index + 1 == QUESTIONS_PER_GAME {
            "Voir le score"
        } else {
            "Suivant"
        };
        ui.click(label, Some(AccessibleRole::Button));
    }
    assert_eq!(ui.phase(), SessionPhase::Ended);
    assert_eq!(window.get_fleches_score(), 10);

    // The recap lines are named after the questions.
    let first = ArrowGame::new(Difficulty::Facile, 100)
        .current()
        .unwrap()
        .clone();
    let recap1 = ui.element(
        &format!(
            "Question 1 : {} vers {}, bonne réponse",
            first.origin.name, first.target.name
        ),
        Some(AccessibleRole::Text),
    );
    // The recap grid is centred (two 320px columns and a 24px gap in a 1000px window) and
    // the level sits to the left of the game choice, like on the start screen.
    let expected_left = (1000.0 - (2.0 * 320.0 + 24.0)) / 2.0;
    assert!(
        (recap1.absolute_position().x - expected_left).abs() < 2.0,
        "the recap is centred: {}",
        recap1.absolute_position().x
    );
    let level = ui
        .element(
            "Facile : Directions espacées",
            Some(AccessibleRole::RadioButton),
        )
        .absolute_position()
        .x;
    let game = ui
        .element(
            "Devinette : Ville cachée au centre",
            Some(AccessibleRole::RadioButton),
        )
        .absolute_position()
        .x;
    assert!(
        level < game,
        "the level comes first, like on the start screen"
    );
    ui.element("Nouvelle partie", Some(AccessibleRole::Button));

    // Back to Devinette and a new round, by clicking.
    ui.click("Devinette : Ville cachée au centre", None);
    assert_eq!(window.get_game_type(), GameType::Devinette);
    ui.click("Nouvelle partie", None);
    assert_eq!(ui.phase(), SessionPhase::Playing);
    assert!(ui.app.session().round().is_some());
    assert!(ui.app.session().fleches().is_none());
    assert_eq!(window.get_level_badge(), "Facile · distances affichées");
}

// --- the Enter guard ----------------------------------------------------------------------

#[test]
fn enter_repeat_and_a_quick_double_tap_cannot_skip_the_verdict() {
    let ui = ui(1000, 760);
    let window = ui.app.window();
    ui.start_fleches_keyboard();

    // Answer with a digit; the Enter that follows must not skip the verdict.
    ui.h.press_key("2");
    assert!(window.get_fleches_answered());
    for _ in 0..5 {
        ui.h.press_key_repeat(Key::Return); // auto-repeat
    }
    assert!(window.get_fleches_answered(), "auto-repeat is ignored");
    ui.key(Key::Return); // an immediate second tap
    assert!(window.get_fleches_answered(), "the verdict stays up");
    ui.after_answer();
    ui.h.press_key_repeat(Key::Return);
    assert!(window.get_fleches_answered(), "a repeat never advances");
    ui.key(Key::Return);
    assert!(!window.get_fleches_answered(), "a deliberate Enter does");

    // Enter answers the aimed arrow (the aim followed the arrow just played), and only
    // once per question.
    assert_eq!(window.get_fleches_focused(), 1);
    ui.key(Key::RightArrow);
    assert_eq!(window.get_fleches_focused(), 2);
    ui.key(Key::Return);
    assert!(window.get_fleches_answered());
    assert_eq!(window.get_fleches_chosen(), 2);
    ui.key(Key::Return); // still guarded
    assert!(window.get_fleches_answered());
    ui.after_answer();
    ui.key(Key::Return);
    assert!(!window.get_fleches_answered());

    // A held Enter on the last question must not start the next game either.
    // (questions 2 to 8 answered correctly, then the last one)
    for _ in 2..QUESTIONS_PER_GAME - 1 {
        let question = ui
            .app
            .session()
            .fleches()
            .unwrap()
            .question()
            .unwrap()
            .clone();
        ui.answer_and_next(question.correct);
    }
    let question = ui
        .app
        .session()
        .fleches()
        .unwrap()
        .question()
        .unwrap()
        .clone();
    ui.h.press_key((question.correct + 1).to_string());
    assert!(window.get_fleches_answered());
    ui.after_answer();
    ui.key(Key::Return); // Voir le score
    assert_eq!(ui.phase(), SessionPhase::Ended);
    for _ in 0..3 {
        ui.h.press_key_repeat(Key::Return);
    }
    assert_eq!(ui.phase(), SessionPhase::Ended, "the score stays up");
    ui.key(Key::Return);
    assert_eq!(ui.phase(), SessionPhase::Ended, "still guarded");
    ui.after_answer();
    ui.key(Key::Return);
    assert_eq!(
        ui.phase(),
        SessionPhase::Playing,
        "a deliberate Enter starts"
    );
}

// --- layout of the real window ------------------------------------------------------------

/// Every label of the board stays inside the canvas at the given window size.
fn assert_labels_inside(ui: &Ui, what: &str, width: u32, height: u32) {
    let board = ui.board();
    let (w, h) = (width as f32, height as f32 - CHROME_HEIGHT);
    for (i, option) in board.options.iter().enumerate() {
        let l = &option.label;
        assert!(
            l.x >= 0.0 && l.y >= 0.0 && l.x + l.width <= w && l.y + l.height <= h,
            "{what}: label {} of {:?} is outside the canvas {w}×{h}",
            i + 1,
            l.text
        );
    }
    assert!(
        board.summary.starts_with("De ") || board.summary.starts_with("D’"),
        "{what}: {summary}",
        summary = board.summary
    );
}

#[test]
fn the_fleches_board_fits_the_real_window_at_default_and_minimum_size() {
    let ui = ui(1000, 760);
    let window = ui.app.window();

    ui.start_fleches_keyboard();
    let (bw, bh) = ui.app.board_size();
    assert!(
        (bw - 1000.0).abs() < 1.0 && (bh - (760.0 - CHROME_HEIGHT)).abs() < 1.0,
        "board {bw}×{bh}"
    );
    assert_labels_inside(&ui, "Facile question at 1000×760", 1000, 760);

    // Answered: the correct arrow's pill becomes the long reveal label.
    let question = ui
        .app
        .session()
        .fleches()
        .unwrap()
        .question()
        .unwrap()
        .clone();
    ui.h.press_key((question.correct + 1).to_string());
    assert_labels_inside(&ui, "Facile verdict at 1000×760", 1000, 760);

    // The minimum window, both difficulties.
    ui.h.resize(800, 600, 1.0);
    ui.h.settle();
    let (bw, bh) = ui.app.board_size();
    assert!(
        (bw - 800.0).abs() < 1.0 && (bh - (600.0 - CHROME_HEIGHT)).abs() < 1.0,
        "board {bw}×{bh}"
    );
    assert_labels_inside(&ui, "Facile verdict at 800×600", 800, 600);
    ui.after_answer();
    ui.key(Key::Return);
    assert_labels_inside(&ui, "Facile question at 800×600", 800, 600);

    // Difficile at the minimum size, question and verdict.
    window.invoke_choose_level(boussole::app::Level::Difficile); // refused: a game is on
    assert_eq!(
        ui.app.session().fleches().unwrap().difficulty(),
        Difficulty::Facile,
        "the level is locked during a game"
    );
}

#[test]
fn the_difficile_board_fits_at_the_minimum_size() {
    let ui = ui(800, 600);
    ui.key(Key::RightArrow); // Difficile
    ui.start_fleches_keyboard();
    assert_eq!(
        ui.app.session().fleches().unwrap().difficulty(),
        Difficulty::Difficile
    );
    assert_labels_inside(&ui, "Difficile question at 800×600", 800, 600);
    // The tightest realistic case: answer, so the long reveal label appears next to the
    // 25°-close distractor of Difficile.
    for _ in 0..4 {
        let question = ui
            .app
            .session()
            .fleches()
            .unwrap()
            .question()
            .unwrap()
            .clone();
        ui.h.press_key((question.correct + 1).to_string());
        assert_labels_inside(&ui, "Difficile verdict at 800×600", 800, 600);
        ui.after_answer();
        ui.key(Key::Return);
    }
}

// --- seeds ----------------------------------------------------------------------------------

#[test]
fn a_refused_start_does_not_use_up_a_seed_and_games_follow_the_seed_sequence() {
    let ui = ui(1000, 760);
    let window = ui.app.window();

    ui.start_fleches_keyboard();
    assert_eq!(
        ui.app.session().fleches().unwrap().engine(),
        &ArrowGame::new(Difficulty::Facile, 100)
    );
    // a refused start (a game is in progress) burns no seed
    window.invoke_start_round();
    assert_eq!(
        ui.app.session().fleches().unwrap().engine(),
        &ArrowGame::new(Difficulty::Facile, 100)
    );

    // play to the end, then the next game is seed 101
    for _ in 0..QUESTIONS_PER_GAME {
        let question = ui
            .app
            .session()
            .fleches()
            .unwrap()
            .question()
            .unwrap()
            .clone();
        ui.answer_and_next(question.correct);
    }
    assert_eq!(ui.phase(), SessionPhase::Ended);
    ui.after_answer();
    window.invoke_start_round();
    assert_eq!(
        ui.app.session().fleches().unwrap().engine(),
        &ArrowGame::new(Difficulty::Facile, 101)
    );
}

#[test]
fn a_click_far_from_every_arrow_is_ignored() {
    let ui = ui(1000, 760);
    ui.start_fleches_keyboard();
    let window = ui.app.window();

    // The centre of the dial (the origin city) is inside the click band but clicking it
    // aims at nothing; a click beyond the label ring neither.
    let board = ui.board();
    ui.h.click(board.cx, board.cy);
    assert!(
        !window.get_fleches_answered(),
        "the dial itself is not a target"
    );
    let far = board.hit_outer + 60.0;
    ui.h.click(board.cx, board.cy - far);
    assert!(!window.get_fleches_answered(), "beyond the arrows, nothing");
    // A click at 90° from an arrow pair that is 180° apart is still within 45° of one of
    // them, so only the two extremes above need to be dead zones.
    let question = ui
        .app
        .session()
        .fleches()
        .unwrap()
        .question()
        .unwrap()
        .clone();
    ui.h.press_key((question.correct + 1).to_string());
    assert!(window.get_fleches_answered());
}

// --- accessibility -------------------------------------------------------------------------

#[test]
fn the_fleches_controls_have_accessible_names() {
    let ui = ui(1000, 760);
    let window = ui.app.window();

    // Start screen: the game cards.
    for label in [
        "Devinette : Trouvez la ville cachée au centre du cadran",
        "Flèches : Visez la ville cible depuis la ville de départ",
    ] {
        ui.element(label, Some(AccessibleRole::RadioButton));
    }

    ui.start_fleches_keyboard();
    // The four arrows are buttons, named by number.
    for number in 1..=4 {
        ui.element(&format!("Flèche {number}"), Some(AccessibleRole::Button));
    }
    // The board carries a spoken summary of the question.
    let board = ui.board();
    let question = ui
        .app
        .session()
        .fleches()
        .unwrap()
        .question()
        .unwrap()
        .clone();
    assert!(
        board.summary.contains(&format!(
            "{} vers {}",
            fleches::de_city(question.origin.name, true),
            question.target.name
        )),
        "{}",
        board.summary
    );
    for number in 1..=4 {
        assert!(
            board
                .summary
                .contains(&format!("la flèche {number} pointe")),
            "{}",
            board.summary
        );
    }

    // Answer correctly (the last check needs the right arrow).
    ui.h.press_key((question.correct + 1).to_string());
    ui.element("Suivant", Some(AccessibleRole::Button));
    assert_eq!(window.get_fleches_score(), 1);
}
