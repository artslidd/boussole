//! Slint front end: glue between a [`Session`] and the `ui/*.slint` files.
//!
//! The `.slint` files only draw. Every event (choose a level, start a round, type, move the
//! highlight, submit) is forwarded to the [`Session`]; after each one the whole view state is
//! pushed back with [`sync`]. The board canvas is the exception: Slint calls the pure callback
//! `compute-board` with the canvas size, so arrows and labels are recomputed on every resize.

use crate::board::{self, HintLabel};
use crate::cities::City;
use crate::game::{Difficulty, Round, RoundStatus};
use crate::geo::Compass8;
use crate::online_ui::OnlineUi;
use crate::session::{Phase as SessionPhase, Session, Submission};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

slint::include_modules!();

/// A seed from the system clock, for unpredictable rounds.
pub fn time_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

fn level_to_ui(difficulty: Difficulty) -> Level {
    match difficulty {
        Difficulty::Facile => Level::Facile,
        Difficulty::Difficile => Level::Difficile,
    }
}

fn level_from_ui(level: Level) -> Difficulty {
    match level {
        Level::Facile => Difficulty::Facile,
        Level::Difficile => Difficulty::Difficile,
    }
}

/// Text of the badge in the header while a round is on (the level is locked).
pub fn level_badge(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Facile => "Facile · distances affichées",
        Difficulty::Difficile => "Difficile · flèches seules",
    }
}

/// Spoken direction, for accessibility labels.
pub fn direction_phrase(compass: Compass8) -> &'static str {
    match compass {
        Compass8::N => "au nord",
        Compass8::NE => "au nord-est",
        Compass8::E => "à l’est",
        Compass8::SE => "au sud-est",
        Compass8::S => "au sud",
        Compass8::SW => "au sud-ouest",
        Compass8::W => "à l’ouest",
        Compass8::NW => "au nord-ouest",
    }
}

/// "Bravo ! C’était" / "Besançon" / "en 3 essais." — the end-of-round message in three parts
/// so the city can be set in colour.
pub fn result_message(round: &Round) -> (String, String, String) {
    let city = round.secret().name.to_owned();
    match round.status() {
        RoundStatus::Won => {
            let tries = round.guesses().len();
            let plural = if tries > 1 { "essais" } else { "essai" };
            (
                "Bravo ! C’était\u{a0}".to_owned(),
                city,
                format!("\u{a0}en {tries} {plural}."),
            )
        }
        RoundStatus::Lost => ("Perdu ! C’était\u{a0}".to_owned(), city, ".".to_owned()),
        RoundStatus::InProgress => (String::new(), String::new(), String::new()),
    }
}

/// What the board shows: the five hints, and the secret once the round is over.
#[derive(Default)]
pub(crate) struct BoardInput {
    pub(crate) hints: Vec<HintLabel>,
    pub(crate) revealed: Option<&'static City>,
}

impl BoardInput {
    fn for_round(round: &Round) -> Self {
        Self {
            hints: round
                .hints()
                .iter()
                .map(|hint| HintLabel {
                    bearing_deg: hint.bearing_deg,
                    text: board::format_hint_label(hint, round.difficulty()),
                })
                .collect(),
            revealed: (round.status() != RoundStatus::InProgress).then(|| round.secret()),
        }
    }
}

/// Compute everything the board draws for a canvas of `width × height` logical pixels.
pub(crate) fn compute_board(width: f32, height: f32, input: &BoardInput) -> BoardView {
    if width < 1.0 || height < 1.0 || input.hints.is_empty() {
        return BoardView::default();
    }
    let layout = board::layout_board(f64::from(width), f64::from(height), &input.hints);

    let arrows: Vec<ArrowView> = layout
        .arrows
        .iter()
        .map(|arrow| ArrowView {
            shaft: arrow.shaft_commands().into(),
            head: arrow.head_commands().into(),
        })
        .collect();
    let labels: Vec<LabelView> = layout
        .labels
        .iter()
        .map(|label| LabelView {
            text: label.text.as_str().into(),
            x: label.rect.x as f32,
            y: label.rect.y as f32,
            width: label.rect.w as f32,
            height: label.rect.h as f32,
        })
        .collect();

    let (text, text_size, revealed) = match input.revealed {
        Some(city) => (
            city.name,
            board::dial_text_size(city.name, layout.dial_body_radius),
            true,
        ),
        None => ("?", layout.dial_body_radius * 1.15, false),
    };

    let mut summary = String::from("Cinq flèches partent de la ville cachée vers : ");
    for (i, label) in layout.labels.iter().enumerate() {
        if i > 0 {
            summary.push_str(", ");
        }
        let bearing = layout.arrows[i].bearing_deg;
        summary.push_str(&format!(
            "{} {}",
            label.text,
            direction_phrase(Compass8::from_bearing(bearing))
        ));
    }
    summary.push('.');

    BoardView {
        arrows: ModelRc::new(VecModel::from(arrows)),
        labels: ModelRc::new(VecModel::from(labels)),
        cx: layout.center.x as f32,
        cy: layout.center.y as f32,
        outer: layout.dial_outer_radius as f32,
        body: layout.dial_body_radius as f32,
        text: text.into(),
        text_size: text_size as f32,
        revealed,
        summary: summary.into(),
    }
}

/// Push the whole view state of `session` into the window.
fn sync(window: &AppWindow, session: &Session) {
    window.set_phase(match session.phase() {
        SessionPhase::Setup => Phase::Setup,
        SessionPhase::Playing => Phase::Playing,
        SessionPhase::Ended => Phase::Ended,
    });
    window.set_level(level_to_ui(session.difficulty()));
    window.set_level_badge(level_badge(session.difficulty()).into());

    let autocomplete = session.autocomplete();
    // Only write the text when it differs, so the cursor never jumps while typing.
    if window.get_query().as_str() != autocomplete.query() {
        window.set_query(autocomplete.query().into());
    }
    let suggestions: Vec<Suggestion> = autocomplete
        .suggestions()
        .iter()
        .map(|city| Suggestion {
            name: city.name.into(),
            tried: session.already_guessed(city),
        })
        .collect();
    window.set_suggestions(ModelRc::new(VecModel::from(suggestions)));
    window.set_highlighted(autocomplete.highlighted() as i32);
    window.set_suggestions_open(autocomplete.is_open());
    window.set_notice(
        session
            .notice()
            .map(|n| n.to_string())
            .unwrap_or_default()
            .into(),
    );

    let rows: Vec<GuessRow> = session
        .round()
        .map(|round| {
            round
                .guesses()
                .iter()
                .map(|guess| GuessRow {
                    name: guess.city.name.into(),
                    distance: if guess.correct {
                        "Trouvé".into()
                    } else {
                        board::format_distance(guess.distance_km).into()
                    },
                    arrow: if guess.correct {
                        "M 4 10.5 L 8.5 15 L 16.5 5".into()
                    } else {
                        board::direction_arrow_commands(guess.bearing_deg, 20.0).into()
                    },
                    direction: direction_phrase(Compass8::from_bearing(guess.bearing_deg)).into(),
                    correct: guess.correct,
                    by: Default::default(),
                    initial: Default::default(),
                })
                .collect()
        })
        .unwrap_or_default();
    window.set_history(ModelRc::new(VecModel::from(rows)));
    window.set_tries_used(session.tries_used() as i32);
    window.set_max_tries(Round::MAX_GUESSES as i32);

    let (before, city, after) = session.round().map(result_message).unwrap_or_default();
    window.set_won(
        session
            .round()
            .is_some_and(|r| r.status() == RoundStatus::Won),
    );
    window.set_result_before(before.into());
    window.set_result_city(city.into());
    window.set_result_after(after.into());
}

struct Shared {
    session: RefCell<Session>,
    seeds: RefCell<Box<dyn FnMut() -> u64>>,
    board: RefCell<BoardInput>,
    board_version: Cell<i32>,
    board_size: Cell<(f32, f32)>,
}

impl Shared {
    /// Re-read the round into the board input and make Slint recompute the board.
    fn refresh_board(&self, window: &AppWindow) {
        *self.board.borrow_mut() = self
            .session
            .borrow()
            .round()
            .map(BoardInput::for_round)
            .unwrap_or_default();
        self.board_version.set(self.board_version.get() + 1);
        window.set_board_version(self.board_version.get());
    }

    fn sync(&self, window: &AppWindow) {
        sync(window, &self.session.borrow());
    }
}

/// The running application: a window plus its session.
pub struct App {
    window: AppWindow,
    shared: Rc<Shared>,
    online: OnlineUi,
}

impl App {
    /// A new window in the Setup phase. `seeds` provides one seed per round.
    pub fn new(seeds: impl FnMut() -> u64 + 'static) -> Result<Self, slint::PlatformError> {
        let window = AppWindow::new()?;
        let shared = Rc::new(Shared {
            session: RefCell::new(Session::new()),
            seeds: RefCell::new(Box::new(seeds)),
            board: RefCell::new(BoardInput::default()),
            board_version: Cell::new(0),
            board_size: Cell::new((0.0, 0.0)),
        });

        window.set_ticks(board::dial_ticks_commands().into());
        window.set_north_tick(board::north_tick_commands().into());
        shared.sync(&window);

        window.on_compute_board({
            let shared = shared.clone();
            move |width, height, _version| {
                // Test-only bookkeeping (what size did Slint lay the board out at?); the board
                // itself is a pure function of the size and the round.
                shared.board_size.set((width, height));
                compute_board(width, height, &shared.board.borrow())
            }
        });

        window.on_choose_level({
            let shared = shared.clone();
            let weak = window.as_weak();
            move |level| {
                let Some(window) = weak.upgrade() else { return };
                shared
                    .session
                    .borrow_mut()
                    .select_difficulty(level_from_ui(level));
                shared.sync(&window);
            }
        });

        window.on_start_round({
            let shared = shared.clone();
            let weak = window.as_weak();
            move || {
                let Some(window) = weak.upgrade() else { return };
                // A refused start (a round is in progress) must not use up a seed.
                if shared.session.borrow().phase() != SessionPhase::Playing {
                    let seed = (shared.seeds.borrow_mut())();
                    if shared.session.borrow_mut().start_round(seed) {
                        shared.refresh_board(&window);
                    }
                }
                shared.sync(&window);
            }
        });

        window.on_query_edited({
            let shared = shared.clone();
            let weak = window.as_weak();
            move |text| {
                let Some(window) = weak.upgrade() else { return };
                shared.session.borrow_mut().edit_query(&text);
                shared.sync(&window);
            }
        });

        window.on_submit_guess({
            let shared = shared.clone();
            let weak = window.as_weak();
            move |index| {
                let Some(window) = weak.upgrade() else { return };
                let picked = usize::try_from(index).ok();
                let outcome = shared.session.borrow_mut().submit(picked);
                if matches!(outcome, Submission::Accepted(_))
                    && shared.session.borrow().phase() == SessionPhase::Ended
                {
                    shared.refresh_board(&window);
                }
                shared.sync(&window);
            }
        });

        window.on_move_highlight({
            let shared = shared.clone();
            let weak = window.as_weak();
            move |delta| {
                let Some(window) = weak.upgrade() else { return };
                shared.session.borrow_mut().move_selection(delta);
                shared.sync(&window);
            }
        });

        window.on_toggle_browse({
            let shared = shared.clone();
            let weak = window.as_weak();
            move || {
                let Some(window) = weak.upgrade() else { return };
                shared.session.borrow_mut().toggle_browse();
                shared.sync(&window);
            }
        });

        window.on_dismiss_suggestions({
            let shared = shared.clone();
            let weak = window.as_weak();
            move || {
                let Some(window) = weak.upgrade() else { return };
                shared.session.borrow_mut().dismiss_suggestions();
                shared.sync(&window);
            }
        });

        let online = OnlineUi::new(&window);
        Ok(Self {
            window,
            shared,
            online,
        })
    }

    /// A new window whose rounds are seeded from the system clock.
    pub fn with_time_seeds() -> Result<Self, slint::PlatformError> {
        Self::new(time_seed)
    }

    /// A new window whose rounds use the seeds `first`, `first + 1`, … (reproducible games).
    pub fn with_seed_sequence(first: u64) -> Result<Self, slint::PlatformError> {
        let mut next = first;
        Self::new(move || {
            let seed = next;
            next = next.wrapping_add(1);
            seed
        })
    }

    pub fn window(&self) -> &AppWindow {
        &self.window
    }

    /// The online mode of this window (hosting, joining, rooms).
    pub fn online(&self) -> &OnlineUi {
        &self.online
    }

    /// The session behind the window (read-only, for tests).
    pub fn session(&self) -> Ref<'_, Session> {
        self.shared.session.borrow()
    }

    /// Size in logical pixels of the board canvas the last time Slint laid it out.
    pub fn board_size(&self) -> (f32, f32) {
        self.shared.board_size.get()
    }

    /// Start a round with a chosen secret instead of a random one (tests, screenshots).
    /// Does nothing while a round is in progress.
    pub fn start_round_with_secret(&self, secret: &'static City, seed: u64) -> bool {
        let started = self
            .shared
            .session
            .borrow_mut()
            .start_round_with_secret(secret, seed);
        if started {
            self.shared.refresh_board(&self.window);
        }
        self.shared.sync(&self.window);
        started
    }

    /// Show the window and run the event loop until it is closed.
    pub fn run(self) -> Result<(), slint::PlatformError> {
        self.window.run()
    }
}
