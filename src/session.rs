//! Game session: the Setup → Playing → Ended state machine and the autocomplete state.
//!
//! Pure Rust, no GUI types: the Slint glue in [`crate::app`] only forwards events to a
//! [`Session`] and renders what it reports, so every rule below is unit-tested without a window.

use crate::arrows::AnswerFeedback;
use crate::cities::{CITIES, City};
use crate::fleches::FlechesSession;
use crate::game::{Difficulty, GuessError, GuessFeedback, Round, RoundStatus};
use crate::search::{find_exact, normalize, search};

/// Number of suggestions shown under the guess field.
pub const SUGGESTION_LIMIT: usize = 8;

/// The game played on the start screen: guess the hidden city, or aim at the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameType {
    #[default]
    Devinette,
    Fleches,
}

/// The round (Devinette) or the ten-question game (Flèches) a session is currently playing.
#[derive(Debug, Clone, PartialEq)]
pub enum Game {
    Devinette(Round),
    Fleches(FlechesSession),
}

/// What the player is doing: before the first round, in a round, or looking at its result.
/// (For Flèches: Setup → the ten questions → the score screen; the verdict between an
/// answer and "Suivant" is still part of the question, so `Playing`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Setup,
    Playing,
    Ended,
}

/// Why a submitted guess was not counted. The `Display` text is what the player reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// Nothing typed.
    Empty,
    /// The text matches no city of the list.
    UnknownCity,
    /// The city was already tried in this round (costs no try).
    AlreadyGuessed(&'static str),
    /// No round in progress.
    NotPlaying,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rejection::Empty => f.write_str("Tapez le nom d’une ville."),
            Rejection::UnknownCity => {
                f.write_str("Ville inconnue : choisissez-en une dans la liste.")
            }
            Rejection::AlreadyGuessed(name) => {
                write!(f, "{name} a déjà été essayée, aucun essai n’est perdu.")
            }
            Rejection::NotPlaying => f.write_str("Lancez une nouvelle partie pour jouer."),
        }
    }
}

/// What happened to a submitted guess.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Submission {
    Accepted(GuessFeedback),
    Rejected(Rejection),
}

/// Autocomplete state of the guess field: the query, its suggestions and the highlighted one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Autocomplete {
    query: String,
    suggestions: Vec<&'static City>,
    highlighted: usize,
    open: bool,
}

impl Autocomplete {
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn suggestions(&self) -> &[&'static City] {
        &self.suggestions
    }

    pub fn highlighted(&self) -> usize {
        self.highlighted
    }

    /// Whether the dropdown is showing (a non-empty query that was not dismissed with Esc).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The player edited the text: refresh the suggestions, highlight the best one, reopen.
    pub fn set_query(&mut self, text: &str) {
        self.query = text.to_owned();
        self.suggestions = search(text, SUGGESTION_LIMIT);
        self.highlighted = 0;
        self.open = !text.trim().is_empty();
    }

    /// Empty the field and close the dropdown.
    pub fn clear(&mut self) {
        self.set_query("");
    }

    /// Open the list of every city, alphabetically, for players who would rather pick than
    /// type (or use a mouse only); with a typed query it reopens that query's suggestions;
    /// closes the list if it is already open.
    pub fn toggle_browse(&mut self) {
        if self.open {
            self.open = false;
        } else if self.query.trim().is_empty() {
            let mut all: Vec<&'static City> = CITIES.iter().collect();
            all.sort_by_cached_key(|city| normalize(city.name));
            self.suggestions = all;
            self.highlighted = 0;
            self.open = true;
        } else {
            let query = self.query.clone();
            self.set_query(&query);
        }
    }

    /// ↑ / ↓: move the highlight by `delta`, wrapping around. On a closed list it reopens it
    /// (on an empty field: the list of every city) without moving the highlight.
    pub fn move_highlight(&mut self, delta: i32) {
        if !self.open {
            if self.suggestions.is_empty() {
                self.toggle_browse();
            } else {
                self.open = true;
            }
            return;
        }
        if self.suggestions.is_empty() {
            return;
        }
        let len = self.suggestions.len() as i32;
        self.highlighted = (self.highlighted as i32 + delta).rem_euclid(len) as usize;
    }

    /// Close the list and keep the text (unlike [`Autocomplete::dismiss`], never empties the field).
    pub fn close_list(&mut self) {
        self.open = false;
    }

    /// Esc: close the list but keep the text; Esc again (list already closed) empties the field.
    pub fn dismiss(&mut self) {
        if self.open {
            self.open = false;
        } else {
            self.clear();
        }
    }

    /// The city a plain "Enter" would submit: the highlighted suggestion while the list is open,
    /// otherwise the exact match of the typed name. While the highlight is still on the default
    /// first row, a name typed in full wins over the first suggestion, so a longer name that
    /// starts with it (a "Saint-Quentin" next to a "Saint-Quentin-en-Yvelines") never takes its
    /// place.
    pub fn resolve(&self) -> Result<&'static City, Rejection> {
        if self.open
            && let Some(city) = self.suggestions.get(self.highlighted)
        {
            if self.highlighted == 0
                && let Some(exact) = find_exact(&self.query)
            {
                return Ok(exact);
            }
            return Ok(city);
        }
        if let Some(city) = find_exact(&self.query) {
            return Ok(city);
        }
        if self.query.trim().is_empty() {
            Err(Rejection::Empty)
        } else {
            Err(Rejection::UnknownCity)
        }
    }

    /// The city at suggestion `index` (a mouse click), if any.
    pub fn pick(&self, index: usize) -> Option<&'static City> {
        self.suggestions.get(index).copied()
    }
}

/// The whole game state seen by the UI.
#[derive(Debug, Clone)]
pub struct Session {
    difficulty: Difficulty,
    game_type: GameType,
    game: Option<Game>,
    autocomplete: Autocomplete,
    notice: Option<Rejection>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// A fresh session in the Setup phase; Facile and Devinette are preselected.
    pub fn new() -> Self {
        Self {
            difficulty: Difficulty::Facile,
            game_type: GameType::Devinette,
            game: None,
            autocomplete: Autocomplete::default(),
            notice: None,
        }
    }

    pub fn phase(&self) -> Phase {
        match &self.game {
            None => Phase::Setup,
            Some(Game::Devinette(round)) => {
                if round.status() == RoundStatus::InProgress {
                    Phase::Playing
                } else {
                    Phase::Ended
                }
            }
            // The verdict of the tenth question is still on screen: the score screen comes
            // with its "Suivant".
            Some(Game::Fleches(fleches)) => {
                if fleches.is_over() {
                    Phase::Ended
                } else {
                    Phase::Playing
                }
            }
        }
    }

    /// The difficulty selected for the next round, or the one locked in for the current round.
    pub fn difficulty(&self) -> Difficulty {
        self.difficulty
    }

    /// The game type chosen for the next round, or the one locked in for the current game.
    pub fn game_type(&self) -> GameType {
        self.game_type
    }

    /// Choose the game type of the next round. Returns `false` (and changes nothing) while a
    /// round or a Flèches game is in progress: the choice is locked for the whole game.
    pub fn select_game_type(&mut self, game_type: GameType) -> bool {
        if self.phase() == Phase::Playing {
            return false;
        }
        self.game_type = game_type;
        true
    }

    /// The Devinette round, when one is playing.
    pub fn round(&self) -> Option<&Round> {
        match &self.game {
            Some(Game::Devinette(round)) => Some(round),
            _ => None,
        }
    }

    /// The Devinette round, mutable, for [`Session::submit`].
    fn round_mut(&mut self) -> Option<&mut Round> {
        match &mut self.game {
            Some(Game::Devinette(round)) => Some(round),
            _ => None,
        }
    }

    /// The Flèches game, when one is playing.
    pub fn fleches(&self) -> Option<&FlechesSession> {
        match &self.game {
            Some(Game::Fleches(fleches)) => Some(fleches),
            _ => None,
        }
    }

    pub fn autocomplete(&self) -> &Autocomplete {
        &self.autocomplete
    }

    /// The last rejection, shown until the next edit or guess.
    pub fn notice(&self) -> Option<Rejection> {
        self.notice
    }

    /// Choose the difficulty of the next round. Returns `false` (and changes nothing) while a
    /// round is in progress: the difficulty is locked for the whole round.
    pub fn select_difficulty(&mut self, difficulty: Difficulty) -> bool {
        if self.phase() == Phase::Playing {
            return false;
        }
        self.difficulty = difficulty;
        true
    }

    /// Start a new round of the selected game type with the selected difficulty. Returns
    /// `false` while a round is in progress.
    pub fn start_round(&mut self, seed: u64) -> bool {
        if self.phase() == Phase::Playing {
            return false;
        }
        self.game = Some(match self.game_type {
            GameType::Devinette => Game::Devinette(Round::new_random(self.difficulty, seed)),
            GameType::Fleches => Game::Fleches(FlechesSession::new(self.difficulty, seed)),
        });
        self.autocomplete.clear();
        self.notice = None;
        true
    }

    /// Start a Devinette round with a chosen secret (tests, screenshots). A test hook for
    /// Devinette: it plays that game whatever the selected type (and makes it the selection).
    pub fn start_round_with_secret(&mut self, secret: &'static City, seed: u64) -> bool {
        if self.phase() == Phase::Playing {
            return false;
        }
        self.game_type = GameType::Devinette;
        self.game = Some(Game::Devinette(Round::with_secret(
            secret,
            self.difficulty,
            seed,
        )));
        self.autocomplete.clear();
        self.notice = None;
        true
    }

    /// The player edited the guess field.
    pub fn edit_query(&mut self, text: &str) {
        self.autocomplete.set_query(text);
        self.notice = None;
    }

    pub fn move_selection(&mut self, delta: i32) {
        self.autocomplete.move_highlight(delta);
    }

    pub fn dismiss_suggestions(&mut self) {
        self.autocomplete.dismiss();
    }

    /// The chevron of the guess field: show or hide the list of every city.
    pub fn toggle_browse(&mut self) {
        self.autocomplete.toggle_browse();
        self.notice = None;
    }

    /// Submit a guess: the suggestion at `picked` (a click), or, with `None`, what Enter means
    /// (the highlighted suggestion, else the exact match). A rejected guess keeps the typed text
    /// and consumes no try.
    pub fn submit(&mut self, picked: Option<usize>) -> Submission {
        let city = match picked {
            Some(index) => self.autocomplete.pick(index).ok_or(Rejection::UnknownCity),
            None => self.autocomplete.resolve(),
        };
        let outcome = match (city, self.round_mut()) {
            (Err(rejection), _) => Submission::Rejected(rejection),
            (Ok(_), None) => Submission::Rejected(Rejection::NotPlaying),
            (Ok(city), Some(round)) => match round.guess(city) {
                Ok(feedback) => Submission::Accepted(*feedback),
                Err(GuessError::AlreadyGuessed) => {
                    Submission::Rejected(Rejection::AlreadyGuessed(city.name))
                }
                Err(GuessError::RoundOver) => Submission::Rejected(Rejection::NotPlaying),
            },
        };
        match outcome {
            Submission::Accepted(_) => {
                self.autocomplete.clear();
                self.notice = None;
            }
            Submission::Rejected(rejection) => {
                self.notice = Some(rejection);
                // The list takes the history's place while it is open: for a city already
                // tried, close it so the history (which says how that guess went) is back.
                if matches!(rejection, Rejection::AlreadyGuessed(_)) {
                    self.autocomplete.close_list();
                }
            }
        }
        outcome
    }

    /// Whether a city was already tried in the current round (for dimming suggestions).
    pub fn already_guessed(&self, city: &City) -> bool {
        self.round()
            .is_some_and(|round| round.guesses().iter().any(|g| g.city.name == city.name))
    }

    /// Tries used in the current round.
    pub fn tries_used(&self) -> usize {
        self.round().map_or(0, |round| round.guesses().len())
    }

    // --- Flèches ---------------------------------------------------------------------------

    /// Answer the current Flèches question with arrow `option` (0–3). Refused (`None`)
    /// outside a Flèches game, and by [`FlechesSession::answer`] when the question is
    /// already answered or the option out of range.
    pub fn fleches_answer(&mut self, option: usize) -> Option<AnswerFeedback> {
        match &mut self.game {
            Some(Game::Fleches(fleches)) => fleches.answer(option),
            _ => None,
        }
    }

    /// ← / →: move the aimed arrow of the current Flèches question.
    pub fn fleches_move_focus(&mut self, delta: i32) {
        if let Some(Game::Fleches(fleches)) = &mut self.game {
            fleches.move_focus(delta);
        }
    }

    /// "Suivant": dismiss the verdict and move to the next question (or the score screen).
    /// Returns `false` outside a Flèches game or with no verdict showing.
    pub fn fleches_advance(&mut self) -> bool {
        match &mut self.game {
            Some(Game::Fleches(fleches)) => fleches.advance(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn city(name: &str) -> &'static City {
        find_exact(name).unwrap_or_else(|| panic!("unknown city {name}"))
    }

    // --- Autocomplete -------------------------------------------------------------------

    #[test]
    fn typing_fills_suggestions_and_highlights_the_best() {
        let mut ac = Autocomplete::default();
        ac.set_query("pa");
        assert!(ac.is_open());
        assert_eq!(ac.suggestions()[0].name, "Paris");
        assert_eq!(ac.highlighted(), 0);
        assert!(ac.suggestions().len() <= SUGGESTION_LIMIT);
    }

    #[test]
    fn empty_or_blank_query_closes_the_list() {
        let mut ac = Autocomplete::default();
        ac.set_query("lyon");
        assert!(ac.is_open());
        ac.set_query("");
        assert!(!ac.is_open());
        assert!(ac.suggestions().is_empty());
        ac.set_query("   ");
        assert!(!ac.is_open());
    }

    #[test]
    fn arrows_move_the_highlight_and_wrap_around() {
        let mut ac = Autocomplete::default();
        ac.set_query("sa");
        let len = ac.suggestions().len();
        assert!(len >= 3, "several cities start with 'sa'");
        ac.move_highlight(1);
        assert_eq!(ac.highlighted(), 1);
        ac.move_highlight(-1);
        ac.move_highlight(-1);
        assert_eq!(
            ac.highlighted(),
            len - 1,
            "up from the top wraps to the bottom"
        );
        ac.move_highlight(1);
        assert_eq!(ac.highlighted(), 0, "down from the bottom wraps to the top");
    }

    #[test]
    fn editing_resets_the_highlight() {
        let mut ac = Autocomplete::default();
        ac.set_query("sa");
        ac.move_highlight(2);
        ac.set_query("sai");
        assert_eq!(ac.highlighted(), 0);
    }

    #[test]
    fn escape_closes_the_list_then_empties_the_field() {
        let mut ac = Autocomplete::default();
        ac.set_query("lyo");
        ac.dismiss();
        assert!(!ac.is_open());
        assert_eq!(ac.query(), "lyo", "the first Esc keeps the text");
        ac.dismiss();
        assert_eq!(ac.query(), "", "the second Esc empties the field");
        assert!(ac.suggestions().is_empty());
    }

    #[test]
    fn down_reopens_a_dismissed_list() {
        let mut ac = Autocomplete::default();
        ac.set_query("lyo");
        ac.dismiss();
        ac.move_highlight(1);
        assert!(ac.is_open(), "↓ reopens the list");
        assert_eq!(ac.highlighted(), 0, "without moving the highlight");
        ac.move_highlight(1);
        assert_eq!(ac.highlighted(), 1.min(ac.suggestions().len() - 1));
    }

    #[test]
    fn browsing_lists_every_city_alphabetically() {
        let mut ac = Autocomplete::default();
        ac.toggle_browse();
        assert!(ac.is_open());
        assert_eq!(ac.suggestions().len(), CITIES.len());
        let names: Vec<String> = ac.suggestions().iter().map(|c| normalize(c.name)).collect();
        assert!(names.windows(2).all(|w| w[0] <= w[1]), "alphabetical order");
        assert_eq!(ac.highlighted(), 0);
        // clicking any row picks that city
        let last = ac.suggestions().len() - 1;
        assert_eq!(
            ac.pick(last).map(|c| c.name),
            Some(ac.suggestions()[last].name)
        );
        // toggling again closes the list
        ac.toggle_browse();
        assert!(!ac.is_open());
    }

    #[test]
    fn down_on_an_empty_field_opens_the_full_list() {
        let mut ac = Autocomplete::default();
        ac.move_highlight(1);
        assert!(ac.is_open());
        assert_eq!(ac.suggestions().len(), CITIES.len());
        ac.move_highlight(1);
        assert_eq!(ac.highlighted(), 1);
    }

    #[test]
    fn the_chevron_with_a_typed_query_reopens_its_suggestions() {
        let mut ac = Autocomplete::default();
        ac.set_query("sa");
        ac.dismiss();
        ac.toggle_browse();
        assert!(ac.is_open());
        assert!(ac.suggestions().len() <= SUGGESTION_LIMIT);
        assert_eq!(ac.query(), "sa");
    }

    #[test]
    fn enter_resolves_the_highlighted_suggestion() {
        let mut ac = Autocomplete::default();
        ac.set_query("besan");
        assert_eq!(ac.resolve().unwrap().name, "Besançon");
        ac.set_query("sa");
        ac.move_highlight(1);
        let expected = ac.suggestions()[1].name;
        assert_eq!(ac.resolve().unwrap().name, expected);
    }

    #[test]
    fn a_name_typed_in_full_beats_the_first_suggestion_until_the_highlight_moves() {
        // The dataset has no city whose name starts another one, so build the list by hand:
        // typing "lyon" while "Paris" happens to be the first suggestion.
        let (paris, lyon) = (city("Paris"), city("Lyon"));
        let mut ac = Autocomplete {
            query: "lyon".to_owned(),
            suggestions: vec![paris, lyon],
            highlighted: 0,
            open: true,
        };
        assert_eq!(ac.resolve().unwrap().name, "Lyon");
        // once the player moved the highlight, their choice wins
        ac.highlighted = 1;
        ac.query = "paris".to_owned();
        assert_eq!(ac.resolve().unwrap().name, "Lyon");
        ac.highlighted = 0;
        assert_eq!(ac.resolve().unwrap().name, "Paris");
    }

    #[test]
    fn enter_with_a_closed_list_needs_an_exact_match() {
        let mut ac = Autocomplete::default();
        ac.set_query("Nîmes");
        ac.dismiss();
        assert_eq!(ac.resolve().unwrap().name, "Nîmes");
        ac.set_query("nime");
        ac.dismiss();
        assert_eq!(ac.query(), "nime");
        assert_eq!(ac.resolve(), Err(Rejection::UnknownCity));
    }

    #[test]
    fn unknown_and_empty_queries_cannot_be_resolved() {
        let mut ac = Autocomplete::default();
        assert_eq!(ac.resolve(), Err(Rejection::Empty));
        ac.set_query("zzzzz");
        assert!(ac.is_open(), "the list stays open to show 'no match'");
        assert!(ac.suggestions().is_empty());
        assert_eq!(ac.resolve(), Err(Rejection::UnknownCity));
        assert_eq!(ac.pick(0), None);
    }

    // --- Session ------------------------------------------------------------------------

    fn playing(secret: &str, difficulty: Difficulty) -> Session {
        let mut s = Session::new();
        assert!(s.select_difficulty(difficulty));
        assert!(s.start_round_with_secret(city(secret), 7));
        s
    }

    fn type_and_enter(s: &mut Session, text: &str) -> Submission {
        s.edit_query(text);
        s.submit(None)
    }

    #[test]
    fn a_session_starts_in_setup_with_facile_preselected() {
        let s = Session::new();
        assert_eq!(s.phase(), Phase::Setup);
        assert_eq!(s.difficulty(), Difficulty::Facile);
        assert!(s.round().is_none());
    }

    #[test]
    fn the_difficulty_is_locked_during_a_round_and_free_between_rounds() {
        let mut s = playing("Besançon", Difficulty::Difficile);
        assert_eq!(s.phase(), Phase::Playing);
        assert!(
            !s.select_difficulty(Difficulty::Facile),
            "locked while playing"
        );
        assert_eq!(s.difficulty(), Difficulty::Difficile);
        assert_eq!(s.round().unwrap().difficulty(), Difficulty::Difficile);
        assert!(!s.start_round(1), "cannot restart a round in progress");

        type_and_enter(&mut s, "Besançon");
        assert_eq!(s.phase(), Phase::Ended);
        assert!(
            s.select_difficulty(Difficulty::Facile),
            "free again after the round"
        );
        assert!(s.start_round(2));
        assert_eq!(s.round().unwrap().difficulty(), Difficulty::Facile);
    }

    #[test]
    fn the_last_choice_is_remembered_for_the_next_round() {
        let mut s = playing("Lille", Difficulty::Difficile);
        type_and_enter(&mut s, "Lille");
        assert_eq!(s.difficulty(), Difficulty::Difficile);
        assert!(s.start_round(3));
        assert_eq!(s.round().unwrap().difficulty(), Difficulty::Difficile);
    }

    #[test]
    fn a_correct_guess_wins() {
        let mut s = playing("Besançon", Difficulty::Facile);
        let outcome = type_and_enter(&mut s, "besancon");
        match outcome {
            Submission::Accepted(f) => assert!(f.correct),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(s.phase(), Phase::Ended);
        assert_eq!(s.round().unwrap().status(), RoundStatus::Won);
        assert_eq!(s.autocomplete().query(), "", "the field is emptied");
    }

    #[test]
    fn six_wrong_guesses_lose() {
        let mut s = playing("Besançon", Difficulty::Facile);
        for name in ["Paris", "Lyon", "Lille", "Nantes", "Brest", "Nice"] {
            assert_eq!(s.phase(), Phase::Playing);
            assert!(matches!(
                type_and_enter(&mut s, name),
                Submission::Accepted(f) if !f.correct
            ));
        }
        assert_eq!(s.phase(), Phase::Ended);
        assert_eq!(s.round().unwrap().status(), RoundStatus::Lost);
        assert_eq!(s.tries_used(), Round::MAX_GUESSES);
    }

    #[test]
    fn a_duplicate_guess_is_rejected_and_costs_no_try() {
        let mut s = playing("Besançon", Difficulty::Facile);
        type_and_enter(&mut s, "Lyon");
        assert_eq!(s.tries_used(), 1);
        let outcome = type_and_enter(&mut s, "lyon");
        assert_eq!(
            outcome,
            Submission::Rejected(Rejection::AlreadyGuessed("Lyon"))
        );
        assert_eq!(s.tries_used(), 1, "no try consumed");
        assert_eq!(s.notice(), Some(Rejection::AlreadyGuessed("Lyon")));
        assert_eq!(
            s.autocomplete().query(),
            "lyon",
            "the text stays so it can be fixed"
        );
        assert!(s.already_guessed(city("Lyon")));
        assert!(!s.already_guessed(city("Paris")));
    }

    #[test]
    fn unknown_names_are_rejected_without_a_try() {
        let mut s = playing("Besançon", Difficulty::Facile);
        assert_eq!(
            type_and_enter(&mut s, "Atlantide"),
            Submission::Rejected(Rejection::UnknownCity)
        );
        assert_eq!(
            type_and_enter(&mut s, ""),
            Submission::Rejected(Rejection::Empty)
        );
        assert_eq!(s.tries_used(), 0);
        s.edit_query("Lyon");
        assert_eq!(s.notice(), None, "editing clears the notice");
    }

    #[test]
    fn clicking_a_suggestion_submits_it() {
        let mut s = playing("Besançon", Difficulty::Facile);
        s.edit_query("sa");
        let second = s.autocomplete().suggestions()[1].name;
        match s.submit(Some(1)) {
            Submission::Accepted(f) => assert_eq!(f.city.name, second),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            s.submit(Some(99)),
            Submission::Rejected(Rejection::UnknownCity),
            "a stale index is harmless"
        );
    }

    #[test]
    fn guesses_are_refused_outside_a_round() {
        let mut s = Session::new();
        assert_eq!(
            type_and_enter(&mut s, "Lyon"),
            Submission::Rejected(Rejection::NotPlaying)
        );
        let mut s = playing("Lyon", Difficulty::Facile);
        type_and_enter(&mut s, "Lyon");
        assert_eq!(
            type_and_enter(&mut s, "Paris"),
            Submission::Rejected(Rejection::NotPlaying)
        );
    }

    #[test]
    fn a_new_round_clears_the_field_and_the_notice() {
        let mut s = playing("Lyon", Difficulty::Facile);
        type_and_enter(&mut s, "Atlantide");
        assert!(s.notice().is_some());
        type_and_enter(&mut s, "Lyon");
        assert!(s.start_round(9));
        assert_eq!(s.notice(), None);
        assert_eq!(s.autocomplete().query(), "");
        assert_eq!(s.tries_used(), 0);
    }

    #[test]
    fn rejection_messages_are_french() {
        assert_eq!(
            Rejection::AlreadyGuessed("Lyon").to_string(),
            "Lyon a déjà été essayée, aucun essai n’est perdu."
        );
        assert!(
            Rejection::UnknownCity
                .to_string()
                .starts_with("Ville inconnue")
        );
        assert!(Rejection::Empty.to_string().contains("ville"));
    }

    // --- Game type and Flèches ---------------------------------------------------------

    /// Plays a whole Flèches game through the session, right on even questions.
    fn play_fleches(s: &mut Session) {
        while !s.fleches().unwrap().is_over() {
            let correct = s.fleches().unwrap().question().unwrap().correct;
            let index = s.fleches().unwrap().index();
            let chosen = if index.is_multiple_of(2) {
                correct
            } else {
                (correct + 1) % 4
            };
            assert!(s.fleches_answer(chosen).is_some());
            assert!(s.fleches_advance());
        }
    }

    #[test]
    fn the_game_type_is_chosen_on_the_start_screen_and_locked_during_a_game() {
        let mut s = Session::new();
        assert_eq!(
            s.game_type(),
            GameType::Devinette,
            "Devinette is the default"
        );
        assert_eq!(s.phase(), Phase::Setup);

        assert!(s.select_game_type(GameType::Fleches));
        assert_eq!(s.game_type(), GameType::Fleches);
        assert!(s.select_difficulty(Difficulty::Difficile));
        assert!(s.start_round(7));
        assert!(s.fleches().is_some());
        assert!(s.round().is_none());
        assert_eq!(s.phase(), Phase::Playing);

        // locked while the game runs — the game type like the difficulty
        assert!(!s.select_game_type(GameType::Devinette));
        assert!(!s.select_difficulty(Difficulty::Facile));
        assert_eq!(s.game_type(), GameType::Fleches);
        assert!(!s.start_round(8), "a game is in progress");

        play_fleches(&mut s);
        assert_eq!(s.phase(), Phase::Ended);
        assert_eq!(s.fleches().unwrap().score(), 5);

        // free again between games, and the choice is remembered
        assert!(s.select_game_type(GameType::Devinette));
        assert!(s.select_difficulty(Difficulty::Facile));
        assert!(s.start_round(9));
        assert!(s.round().is_some());
        assert!(s.fleches().is_none());
    }

    #[test]
    fn a_fleches_game_is_driven_through_the_session() {
        let mut s = Session::new();
        s.select_game_type(GameType::Fleches);
        assert!(s.start_round(1));

        for i in 0..10 {
            let question = s.fleches().unwrap().question().unwrap();
            assert_eq!(s.fleches().unwrap().index(), i);
            let chosen = question.correct;
            let feedback = s.fleches_answer(chosen).expect("answer accepted");
            assert!(feedback.is_correct);
            // one answer per question, and nothing before Suivant
            assert_eq!(s.fleches_answer(chosen), None);
            assert_eq!(s.phase(), Phase::Playing);
            assert!(s.fleches_advance());
        }
        assert_eq!(s.phase(), Phase::Ended);
        assert_eq!(s.fleches().unwrap().score(), 10);
        let recap = s.fleches().unwrap().recap();
        assert_eq!(recap.len(), 10);
        assert!(recap.iter().all(|row| row.correct));
    }

    #[test]
    fn fleches_helpers_do_nothing_outside_a_fleches_game() {
        let mut s = Session::new();
        assert_eq!(s.fleches_answer(0), None);
        assert!(!s.fleches_advance());
        s.fleches_move_focus(1); // no panic, nothing to aim at
        assert_eq!(s.fleches(), None);
    }

    #[test]
    fn the_secret_hook_plays_devinette_whatever_the_choice() {
        let mut s = Session::new();
        s.select_game_type(GameType::Fleches);
        assert!(s.start_round_with_secret(city("Lyon"), 3));
        assert!(s.round().is_some(), "a Devinette round is playing");
        assert_eq!(s.fleches(), None);
        assert_eq!(s.game_type(), GameType::Devinette, "the choice follows");
    }

    #[test]
    fn the_last_verdict_keeps_the_game_in_the_playing_phase() {
        let mut s = Session::new();
        s.select_game_type(GameType::Fleches);
        assert!(s.start_round(2));
        for _ in 0..9 {
            s.fleches_answer(0);
            assert!(s.fleches_advance());
        }
        // tenth question: answered, verdict on screen, score screen not yet
        assert!(s.fleches_answer(0).is_some());
        assert_eq!(s.phase(), Phase::Playing);
        assert!(s.fleches_advance());
        assert_eq!(s.phase(), Phase::Ended);
    }
}
