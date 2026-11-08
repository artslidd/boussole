//! End-to-end tests of the real `.slint` UI, rendered headlessly: whole rounds played with the
//! keyboard only and with the mouse only, difficulty locking, win / lose / duplicate / unknown
//! guesses, accessible names, and the layout of the real window at the default and minimum
//! sizes. Events are injected as key presses and pointer clicks; mouse targets are found by
//! accessible label, which also proves the controls have a name.

use boussole::app::{App, BoardView, Level, Phase};
use boussole::board::{HintLabel, format_hint_label, format_km, layout_board};
use boussole::cities::City;
use boussole::game::{Difficulty, Round, RoundStatus};
use boussole::headless::Headless;
use boussole::search::{normalize, search};
use boussole::session::Phase as SessionPhase;
use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementRoot};
use slint::ComponentHandle;
use slint::platform::Key;
use std::time::Duration;

/// Header + status strip + dock of `ui/app.slint`: the board gets the rest of the window.
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

    fn secret(&self) -> &'static City {
        self.app.session().round().expect("a round").secret()
    }

    fn tries(&self) -> usize {
        self.app.session().tries_used()
    }

    fn key(&self, key: Key) {
        self.h.press_key(key);
    }

    /// Let the guard that protects the result screen from a stray Enter run out.
    fn after_result(&self) {
        self.h.advance(Duration::from_millis(800));
    }

    /// Type a name and press Enter.
    fn guess(&self, name: &str) {
        self.h.type_text(name);
        self.key(Key::Return);
    }

    fn element(&self, label: &str, role: Option<AccessibleRole>) -> ElementHandle {
        ElementHandle::find_by_accessible_label(self.app.window(), label)
            .find(|e| role.is_none() || e.accessible_role() == role)
            .unwrap_or_else(|| panic!("no element labelled {label:?} ({role:?})"))
    }

    fn center(element: &ElementHandle) -> (f32, f32) {
        let p = element.absolute_position();
        let s = element.size();
        (p.x + s.width / 2.0, p.y + s.height / 2.0)
    }

    fn click(&self, label: &str, role: Option<AccessibleRole>) {
        let (x, y) = Self::center(&self.element(label, role));
        self.h.click(x, y);
    }

    /// Click a suggestion row of the open list, scrolling the list down with the mouse wheel
    /// until the row is on screen (rows scrolled out of the list are not part of the visible
    /// element tree).
    fn click_suggestion(&self, name: &str) {
        let list = self.element("Suggestions de villes", Some(AccessibleRole::List));
        let (lx, ly) = Self::center(&list);
        let top = list.absolute_position().y;
        let bottom = top + list.size().height;
        for _ in 0..200 {
            let row = ElementHandle::find_by_accessible_label(self.app.window(), name)
                .find(|e| e.accessible_role() == Some(AccessibleRole::ListItem));
            if let Some(row) = row {
                let (x, y) = Self::center(&row);
                if y > top + 8.0 && y < bottom - 8.0 {
                    self.h.click(x, y);
                    return;
                }
            }
            self.h.scroll(lx, ly, -60.0);
        }
        panic!("could not scroll {name:?} into view");
    }

    /// The labels the board currently draws, as computed for the real canvas size.
    fn board(&self) -> BoardView {
        let (w, h) = self.app.board_size();
        self.app.window().invoke_compute_board(w, h, 0)
    }

    fn board_label_texts(&self) -> Vec<String> {
        use slint::Model;
        let board = self.board();
        board.labels.iter().map(|l| l.text.to_string()).collect()
    }

    /// Two cities that are not the secret.
    fn wrong_cities(&self, count: usize) -> Vec<&'static City> {
        let secret = self.secret().name;
        let mut all: Vec<&'static City> = boussole::cities::CITIES.iter().collect();
        all.sort_by_cached_key(|c| normalize(c.name));
        all.into_iter()
            .filter(|c| c.name != secret)
            .take(count)
            .collect()
    }
}

// --- keyboard only ----------------------------------------------------------------------

#[test]
fn a_whole_round_can_be_won_with_the_keyboard_only() {
    let ui = ui(1000, 760);
    assert_eq!(ui.phase(), SessionPhase::Setup);

    ui.key(Key::Return); // the level picker has the focus: Enter starts the round
    assert_eq!(ui.phase(), SessionPhase::Playing);
    assert_eq!(ui.app.window().get_phase(), Phase::Playing);

    let secret = ui.secret();
    let wrong = ui.wrong_cities(1)[0];
    ui.guess(&normalize(wrong.name));
    assert_eq!(ui.tries(), 1, "the typed name was accepted");
    assert_eq!(
        ui.app.session().round().unwrap().guesses()[0].city.name,
        wrong.name
    );

    ui.guess(&normalize(secret.name));
    assert_eq!(ui.phase(), SessionPhase::Ended);
    assert_eq!(ui.app.session().round().unwrap().status(), RoundStatus::Won);

    let window = ui.app.window();
    assert_eq!(window.get_phase(), Phase::Ended);
    assert!(window.get_won());
    assert_eq!(window.get_result_city(), secret.name);
    assert!(window.get_result_before().starts_with("Bravo ! C’était"));
    assert!(window.get_result_after().contains("en 2 essais."));

    // Enter again (the focus moved to the level picker, once the result guard has run out): a
    // new round starts
    ui.after_result();
    ui.key(Key::Return);
    assert_eq!(ui.phase(), SessionPhase::Playing);
    assert_eq!(ui.tries(), 0);
}

#[test]
fn six_wrong_guesses_with_the_keyboard_lose_and_reveal_the_city() {
    let ui = ui(1000, 760);
    ui.key(Key::Return);
    let secret = ui.secret();
    for city in ui.wrong_cities(Round::MAX_GUESSES) {
        assert_eq!(ui.phase(), SessionPhase::Playing);
        ui.guess(&normalize(city.name));
    }
    assert_eq!(ui.phase(), SessionPhase::Ended);
    assert_eq!(
        ui.app.session().round().unwrap().status(),
        RoundStatus::Lost
    );
    let window = ui.app.window();
    assert!(!window.get_won());
    assert_eq!(window.get_result_before(), "Perdu ! C’était\u{a0}");
    assert_eq!(window.get_result_city(), secret.name);
    // the dial reveals the city
    let board = ui.board();
    assert!(board.revealed);
    assert_eq!(board.text, secret.name);
}

#[test]
fn arrow_keys_pick_a_suggestion_and_escape_closes_the_list() {
    let ui = ui(1000, 760);
    ui.key(Key::Return);
    let window = ui.app.window();

    ui.h.type_text("sa");
    assert!(window.get_suggestions_open());
    assert_eq!(window.get_highlighted(), 0);
    ui.key(Key::DownArrow);
    ui.key(Key::DownArrow);
    ui.key(Key::UpArrow);
    assert_eq!(window.get_highlighted(), 1);

    ui.key(Key::Escape);
    assert!(!window.get_suggestions_open(), "Esc closes the list");
    assert_eq!(window.get_query(), "sa", "and keeps the text");
    ui.key(Key::Escape);
    assert_eq!(window.get_query(), "", "a second Esc empties the field");

    // Down then Enter submits the second suggestion
    let expected = search("sa", 8)[1];
    ui.h.type_text("sa");
    ui.key(Key::DownArrow);
    ui.key(Key::Return);
    assert_eq!(
        ui.app.session().round().unwrap().guesses()[0].city.name,
        expected.name
    );
}

#[test]
fn a_duplicate_guess_is_rejected_without_using_a_try() {
    let ui = ui(1000, 760);
    ui.key(Key::Return);
    let city = ui.wrong_cities(1)[0];
    ui.guess(&normalize(city.name));
    assert_eq!(ui.tries(), 1);

    ui.guess(&normalize(city.name));
    assert_eq!(ui.tries(), 1, "no try consumed");
    let notice = ui.app.window().get_notice().to_string();
    assert!(notice.contains("déjà été essayée"), "{notice}");
    assert!(
        !ui.app.window().get_suggestions_open(),
        "the list closes so the history shows how that guess went"
    );
    assert_eq!(ui.phase(), SessionPhase::Playing);
}

#[test]
fn an_unknown_name_cannot_be_submitted() {
    let ui = ui(1000, 760);
    ui.key(Key::Return);
    ui.guess("Atlantide");
    assert_eq!(ui.tries(), 0);
    let notice = ui.app.window().get_notice().to_string();
    assert!(notice.starts_with("Ville inconnue"), "{notice}");
    assert_eq!(ui.phase(), SessionPhase::Playing);
}

// --- difficulty ---------------------------------------------------------------------------

#[test]
fn the_difficulty_is_chosen_before_a_round_and_locked_during_it() {
    let ui = ui(1000, 760);
    let window = ui.app.window();
    assert_eq!(window.get_level(), Level::Facile, "Facile is preselected");

    ui.key(Key::RightArrow); // the picker has the focus
    assert_eq!(window.get_level(), Level::Difficile);
    ui.key(Key::Return);
    assert_eq!(ui.phase(), SessionPhase::Playing);
    assert_eq!(window.get_level_badge(), "Difficile · flèches seules");

    // locked: neither the keyboard (the guess field has the focus) nor a stray event changes it
    ui.key(Key::LeftArrow);
    window.invoke_choose_level(Level::Facile);
    ui.h.settle();
    assert_eq!(window.get_level(), Level::Difficile);
    assert_eq!(
        ui.app.session().round().unwrap().difficulty(),
        Difficulty::Difficile
    );

    // after the round the picker is live again and the last choice is remembered
    ui.guess(&normalize(ui.secret().name));
    assert_eq!(ui.phase(), SessionPhase::Ended);
    assert_eq!(window.get_level(), Level::Difficile);
    ui.key(Key::LeftArrow);
    assert_eq!(window.get_level(), Level::Facile);
    ui.after_result();
    ui.key(Key::Return);
    assert_eq!(ui.phase(), SessionPhase::Playing);
    assert_eq!(
        ui.app.session().round().unwrap().difficulty(),
        Difficulty::Facile
    );
    assert_eq!(window.get_level_badge(), "Facile · distances affichées");
}

#[test]
fn facile_labels_show_distances_and_difficile_labels_do_not() {
    let ui = ui(1000, 760);
    for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
        // the picker has the focus before each round: Left = Facile, Right = Difficile
        ui.key(if difficulty.shows_distances() {
            Key::LeftArrow
        } else {
            Key::RightArrow
        });
        ui.key(Key::Return);
        let round = ui.app.session().round().unwrap().clone();
        assert_eq!(round.difficulty(), difficulty);
        let texts = ui.board_label_texts();
        assert_eq!(texts.len(), Round::HINT_COUNT);
        for hint in round.hints() {
            let expected = if difficulty.shows_distances() {
                format!("{} · {} km", hint.city.name, format_km(hint.distance_km))
            } else {
                hint.city.name.to_owned()
            };
            assert!(texts.contains(&expected), "{expected:?} not in {texts:?}");
        }
        if !difficulty.shows_distances() {
            assert!(
                texts.iter().all(|t| !t.contains("km") && !t.contains('·')),
                "no distance anywhere on Difficile labels: {texts:?}"
            );
        }
        // finish the round so the next one can start
        ui.guess(&normalize(round.secret().name));
        assert_eq!(ui.phase(), SessionPhase::Ended);
        ui.after_result();
    }
}

// --- mouse only ---------------------------------------------------------------------------

#[test]
fn a_whole_round_can_be_played_with_the_mouse_only() {
    let ui = ui(1000, 760);
    let window = ui.app.window();

    // Choose Difficile and start, by clicking
    ui.click("Difficile : Flèches seules, sans distances", None);
    assert_eq!(window.get_level(), Level::Difficile);
    ui.click("Nouvelle partie", None);
    assert_eq!(ui.phase(), SessionPhase::Playing);
    assert_eq!(
        ui.app.session().round().unwrap().difficulty(),
        Difficulty::Difficile
    );

    // Pick cities from the full list with the chevron: two wrong ones, then the secret
    let secret = ui.secret();
    for city in ui.wrong_cities(2) {
        ui.click("Afficher la liste des villes", None);
        ui.click_suggestion(city.name);
    }
    assert_eq!(ui.tries(), 2);
    ui.click("Afficher la liste des villes", None);
    ui.click_suggestion(secret.name);
    assert_eq!(ui.phase(), SessionPhase::Ended);
    assert_eq!(ui.app.session().round().unwrap().status(), RoundStatus::Won);
    assert_eq!(window.get_result_city(), secret.name);

    // Back to Facile and a new round, by clicking
    ui.click("Facile : Distances affichées", None);
    assert_eq!(window.get_level(), Level::Facile);
    ui.click("Nouvelle partie", None);
    assert_eq!(ui.phase(), SessionPhase::Playing);
    assert_eq!(
        ui.app.session().round().unwrap().difficulty(),
        Difficulty::Facile
    );
}

#[test]
fn tab_reaches_the_new_round_button_and_space_presses_it() {
    let ui = ui(1000, 760);
    ui.key(Key::Return);
    ui.guess(&normalize(ui.secret().name));
    assert_eq!(ui.phase(), SessionPhase::Ended);
    // the level picker has the focus; Tab moves on to "Nouvelle partie"; Space presses it
    ui.key(Key::Tab);
    ui.h.press_key(" ");
    assert_eq!(ui.phase(), SessionPhase::Playing);
    assert_eq!(ui.tries(), 0);
}

#[test]
fn the_guess_field_has_the_keyboard_focus_after_a_mouse_start() {
    let ui = ui(1000, 760);
    ui.click("Nouvelle partie", None);
    assert_eq!(ui.phase(), SessionPhase::Playing);
    ui.h.type_text("lyo");
    assert_eq!(ui.app.window().get_query(), "lyo");
}

#[test]
fn clicking_a_typed_suggestion_submits_it() {
    let ui = ui(1000, 760);
    ui.key(Key::Return);
    ui.h.type_text("sai");
    let expected = search("sai", 8)[2];
    ui.click_suggestion(expected.name);
    assert_eq!(
        ui.app.session().round().unwrap().guesses()[0].city.name,
        expected.name
    );
}

// --- accessibility ------------------------------------------------------------------------

#[test]
fn controls_have_accessible_names() {
    let ui = ui(1000, 760);
    // Start screen
    for label in [
        "Nouvelle partie",
        "Facile : Distances affichées sur les flèches",
        "Difficile : Flèches seules, sans distances",
        "À propos",
    ] {
        ui.element(label, None);
    }
    let facile = ui.element("Facile : Distances affichées sur les flèches", None);
    assert_eq!(facile.accessible_role(), Some(AccessibleRole::RadioButton));
    assert_eq!(facile.accessible_checked(), Some(true));

    ui.key(Key::Return);
    // Round
    for label in [
        "Nom de la ville à deviner",
        "Afficher la liste des villes",
        "Essai 1 sur 6",
    ] {
        ui.element(label, None);
    }
    let board_summary = ui
        .app
        .window()
        .invoke_compute_board(ui.app.board_size().0, ui.app.board_size().1, 0)
        .summary
        .to_string();
    assert!(board_summary.starts_with("Cinq flèches partent de la ville cachée vers"));
    // each arrow label is exposed with its text
    for text in ui.board_label_texts() {
        ui.element(&text, Some(AccessibleRole::Text));
    }
}

// --- regressions from the review ------------------------------------------------------------

/// Position and size of an element, in logical pixels.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Bounds {
    fn of(element: &ElementHandle) -> Self {
        let (p, s) = (element.absolute_position(), element.size());
        Self {
            x: p.x,
            y: p.y,
            w: s.width,
            h: s.height,
        }
    }

    fn contains(&self, inner: &Bounds) -> bool {
        inner.x >= self.x - 0.5
            && inner.y >= self.y - 0.5
            && inner.x + inner.w <= self.x + self.w + 0.5
            && inner.y + inner.h <= self.y + self.h + 0.5
    }
}

/// The suggestion row the keyboard highlight is on, if it is on screen at all (rows scrolled out
/// of the list are not part of the visible element tree).
fn highlighted_row(ui: &Ui) -> Option<Bounds> {
    ui.app
        .window()
        .root_element()
        .query_descendants()
        .match_accessible_role(AccessibleRole::ListItem)
        .find_all()
        .into_iter()
        .find(|e| e.accessible_item_selected() == Some(true))
        .map(|e| Bounds::of(&e))
}

fn assert_highlight_is_visible(ui: &Ui) {
    let list = Bounds::of(&ui.element("Suggestions de villes", Some(AccessibleRole::List)));
    let row = highlighted_row(ui).expect("the highlighted row is on screen (not scrolled out)");
    assert!(
        list.contains(&row),
        "highlighted row {row:?} is outside the list {list:?}"
    );
}

#[test]
fn a_reopened_full_list_is_scrolled_to_its_highlighted_row() {
    let ui = ui(1000, 760);
    ui.key(Key::Return); // start
    let window = ui.app.window();
    ui.key(Key::DownArrow); // empty field: opens the list of every city
    assert!(window.get_suggestions_open());
    for _ in 0..30 {
        ui.key(Key::DownArrow);
    }
    assert_eq!(window.get_highlighted(), 30);
    assert_highlight_is_visible(&ui);

    ui.key(Key::Escape); // closes the list
    assert!(!window.get_suggestions_open());
    ui.key(Key::DownArrow); // reopens it
    assert!(window.get_suggestions_open());
    // Enter would submit row 30: it must be the row the player sees highlighted
    assert_eq!(window.get_highlighted(), 30);
    assert_highlight_is_visible(&ui);
}

/// Enter held down (auto-repeat) or tapped twice right after the last guess must not skip the
/// "Bravo !" / "Perdu !" screen by starting the next round.
#[test]
fn enter_repeat_and_a_quick_double_tap_cannot_skip_the_result() {
    let ui = ui(1000, 760);
    ui.key(Key::Return); // Setup has no guard: Enter starts at once
    assert_eq!(ui.phase(), SessionPhase::Playing);
    ui.guess(&normalize(ui.secret().name));
    assert_eq!(ui.phase(), SessionPhase::Ended);

    for _ in 0..5 {
        ui.h.press_key_repeat(Key::Return); // key auto-repeat
    }
    assert_eq!(ui.phase(), SessionPhase::Ended, "auto-repeat is ignored");
    ui.key(Key::Return); // an immediate second tap
    assert_eq!(ui.phase(), SessionPhase::Ended, "the result stays up");

    ui.after_result();
    ui.h.press_key_repeat(Key::Return);
    assert_eq!(
        ui.phase(),
        SessionPhase::Ended,
        "a repeat never starts a round"
    );
    ui.key(Key::Return);
    assert_eq!(ui.phase(), SessionPhase::Playing, "a deliberate Enter does");
}

/// The suggestion list opens in the dock, so the hint arrows and labels stay readable while
/// the player chooses.
fn assert_list_clear_of_the_board(width: u32, height: u32) {
    let ui = ui(width, height);
    ui.key(Key::Return);
    let board_bottom = 52.0 + (height as f32 - CHROME_HEIGHT);
    let check = |what: &str| {
        let list = ui.element("Suggestions de villes", Some(AccessibleRole::List));
        let (p, s) = (list.absolute_position(), list.size());
        assert!(
            p.y >= board_bottom - 0.5,
            "{what} at {width}×{height}: list top {} is inside the board (bottom {board_bottom})",
            p.y
        );
        assert!(p.x >= 0.0 && p.x + s.width <= width as f32 + 0.5);
        assert!(p.y + s.height <= height as f32 + 0.5);
        let field = ui.element("Nom de la ville à deviner", None);
        assert!(
            p.x >= field.absolute_position().x + field.size().width,
            "{what}: the list does not cover the field"
        );
    };
    ui.h.type_text("sa"); // a typed query: eight results, five rows visible
    check("typed query");
    ui.key(Key::Escape);
    ui.key(Key::Escape);
    ui.key(Key::DownArrow); // every city
    check("full list");
}

#[test]
fn the_suggestion_list_does_not_cover_the_board_at_the_default_size() {
    assert_list_clear_of_the_board(1000, 760);
}

#[test]
fn the_suggestion_list_does_not_cover_the_board_at_the_minimum_size() {
    assert_list_clear_of_the_board(800, 600);
}

#[test]
fn the_about_box_has_a_close_button_and_closes_with_escape() {
    let ui = ui(1000, 760);
    let is_open = |ui: &Ui| {
        ElementHandle::find_by_accessible_label(ui.app.window(), "Fermer")
            .next()
            .is_some()
    };
    assert!(!is_open(&ui));

    ui.click("À propos", None);
    assert!(is_open(&ui), "the box opens");
    ui.element("À propos de Boussole", Some(AccessibleRole::Groupbox));
    // Elements of a popup report their position relative to the popup, which `app.slint` centres
    // in the window (440 × 456).
    let (fermer_x, fermer_y) = Ui::center(&ui.element("Fermer", Some(AccessibleRole::Button)));
    ui.h.click(
        (1000.0 - 440.0) / 2.0 + fermer_x,
        (760.0 - 456.0) / 2.0 + fermer_y,
    );
    assert!(!is_open(&ui), "the Fermer button closes it");

    ui.app.window().invoke_show_about();
    ui.h.settle();
    assert!(is_open(&ui));
    ui.key(Key::Escape);
    assert!(!is_open(&ui), "Esc closes it too");
    assert_eq!(ui.phase(), SessionPhase::Setup, "and does nothing else");
}

/// `--seed N` must give round k the seed N + k: a refused start must not burn one.
#[test]
fn a_refused_start_does_not_use_up_a_seed() {
    let ui = ui(1000, 760);
    ui.key(Key::Return); // round 1: seed 100
    assert_eq!(
        *ui.app.session().round().unwrap(),
        Round::new_random(Difficulty::Facile, 100)
    );
    ui.app.window().invoke_start_round(); // refused: a round is in progress
    ui.guess(&normalize(ui.secret().name));
    ui.after_result();
    ui.key(Key::Return); // round 2: seed 101, not 102
    assert_eq!(
        *ui.app.session().round().unwrap(),
        Round::new_random(Difficulty::Facile, 101)
    );
}

// --- layout of the real window ------------------------------------------------------------

#[test]
fn the_board_fits_the_real_window_at_default_and_minimum_size() {
    let ui = ui(1000, 760);
    for (width, height) in [(1000u32, 760u32), (800, 600), (1280, 800), (900, 640)] {
        ui.h.resize(width, height, 1.0);
        ui.h.settle();
        ui.key(Key::Return); // start a round (the picker has the focus)
        for _ in 0..3 {
            // a few different rounds, alternating the difficulty
            let round = ui.app.session().round().unwrap().clone();
            let (bw, bh) = ui.app.board_size();
            assert!(
                (bw - width as f32).abs() < 1.0
                    && (bh - (height as f32 - CHROME_HEIGHT)).abs() < 1.0,
                "board {bw}×{bh} in a {width}×{height} window"
            );
            let hints: Vec<HintLabel> = round
                .hints()
                .iter()
                .map(|h| HintLabel {
                    bearing_deg: h.bearing_deg,
                    text: format_hint_label(h, round.difficulty()),
                })
                .collect();
            let layout = layout_board(f64::from(bw), f64::from(bh), &hints);
            assert!(layout.problems().is_empty(), "{:?}", layout.problems());
            // finish the round, flip the level and start the next one
            ui.guess(&normalize(round.secret().name));
            ui.after_result();
            ui.key(Key::RightArrow);
            ui.key(Key::Return);
        }
        ui.guess(&normalize(ui.secret().name));
        ui.after_result();
        ui.key(Key::LeftArrow);
    }
}

#[test]
fn the_window_renders_every_phase_without_a_blank_screen() {
    let ui = ui(1000, 760);
    let distinct_colors = |image: &slint::SharedPixelBuffer<slint::Rgba8Pixel>| {
        let mut seen = std::collections::HashSet::new();
        for pixel in image.as_slice() {
            seen.insert((pixel.r, pixel.g, pixel.b));
        }
        seen.len()
    };
    let setup = ui.h.snapshot(ui.app.window());
    assert_eq!((setup.width(), setup.height()), (1000, 760));
    assert!(distinct_colors(&setup) > 20);

    ui.key(Key::Return);
    let playing = ui.h.snapshot(ui.app.window());
    assert!(distinct_colors(&playing) > 20);
    assert_ne!(setup.as_bytes(), playing.as_bytes());

    ui.guess(&normalize(ui.secret().name));
    let ended = ui.h.snapshot(ui.app.window());
    assert_ne!(playing.as_bytes(), ended.as_bytes());
}
