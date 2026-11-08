//! The online mode's glue with the Slint window.
//!
//! [`OnlineUi`] owns everything that is not drawing: the reducer state ([`OnlineState`]), the
//! background connection ([`NetHandle`]), the embedded server behind the [`Host`] trait, the
//! clipboard and the form state of the "En ligne" menu. Events arrive from other threads
//! (network, hosting) through an inbox that the UI thread drains: a wake-up through
//! `slint::Weak::upgrade_in_event_loop` for immediacy, and a 40 ms poll that also drives the
//! countdowns, so nothing depends on the platform's event-loop proxy.
//!
//! After every change [`OnlineUi::sync`] copies the view-model ([`crate::online::view`]) into
//! the `Online` global of `ui/online-state.slint`; the `.slint` files only draw.

use crate::app::{
    AppWindow, BannerKind, DotStatus, GuessRow, HostChoice, Level, MenuHelp, Online, OnlineScreen,
    PlayerRowView, RankRowView, ResultRowView, RoomMode, ScoreRowView, ShareAddressView,
    Suggestion, compute_board,
};
use crate::board::{self, HintLabel};
use crate::clipboard::{Clipboard, SystemClipboard};
use crate::host::{Host, HostError, HostInfo, HostMode, ShareAddress, UnavailableHost};
use crate::net::{AddressError, NetConfig, NetEvent, NetHandle, normalize_address};
use crate::online::state::{Link, OnlineState, apply, set_link};
use crate::online::view::{
    self, Banner, ProgressStatus, Screen, Setting, format_clock, step_setting,
};
use crate::search::find_exact;
use crate::session::{Autocomplete, Rejection};
use boussole_protocol::{
    ClientMsg, Difficulty, ErrorCode, Mode, RoomConfig, ServerMsg, normalize_nickname,
    normalize_room_code,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the UI thread looks at its inbox (and refreshes countdowns).
const POLL: Duration = Duration::from_millis(40);
/// How often the clocks on screen are recomputed when nothing else happened.
const TICK: Duration = Duration::from_millis(200);
/// How long "Copié" stays on a copy button.
const COPIED_FOR: Duration = Duration::from_secs(2);
/// A guess the server has not answered after this long is given up on (the field reopens).
const GUESS_TIMEOUT: Duration = Duration::from_secs(8);

/// Where the player is in the flow, from the menu to a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The "En ligne" menu (or nothing: solo mode).
    Menu,
    /// The embedded server (and tunnel) is starting.
    Starting,
    /// Connected or connecting to a server, waiting for the room.
    Connecting,
    /// In a room.
    InRoom,
}

/// What the player asked for on the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Intent {
    Host,
    Join(String),
    Create,
}

/// What other threads hand to the UI thread.
enum Inbound {
    Net {
        session: u64,
        event: NetEvent,
    },
    Host {
        session: u64,
        result: Result<HostInfo, HostError>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CopyTarget {
    Code,
    Address(usize),
}

/// What was last pushed into the window's models, to rebuild a model only when it changed (a
/// rebuilt model resets scrolling and makes assistive technology re-read the list).
#[derive(Default)]
struct Pushed {
    players: Vec<view::LobbyPlayer>,
    share: Vec<(ShareAddress, bool)>,
    scoreboard: Vec<view::ScoreRow>,
    history: Vec<view::HistoryRow>,
    results: Vec<view::ResultRow>,
    ranking: Vec<view::RankRow>,
    suggestions: Vec<(String, bool)>,
    board_key: Option<(Vec<HintLabel>, Option<String>)>,
}

struct Core {
    state: OnlineState,
    stage: Stage,
    intent: Option<Intent>,
    net: Option<NetHandle>,
    /// Bumped for every connection attempt and every teardown: stale inbox entries are dropped.
    session: u64,
    /// The embedded server, while it runs.
    hosted: Option<HostInfo>,
    host_started: bool,
    /// The server address typed for a remote room (shown as the address to share).
    remote_url: Option<String>,
    // menu
    nickname_error: String,
    address_error: String,
    code_error: String,
    error: String,
    help: MenuHelp,
    busy_text: String,
    // room
    autocomplete: Autocomplete,
    pending: Option<(String, Instant)>,
    notice: Option<String>,
    pending_config: Option<RoomConfig>,
    copied: Option<(CopyTarget, Instant)>,
    confirm_leave: bool,
    close_after_leave: bool,
    // bookkeeping
    last_sync: Instant,
    pushed: Pushed,
    sent: Vec<ClientMsg>,
}

/// Adjusts the [`NetConfig`] of every connection (tests shorten the time-outs).
type NetTweak = Box<dyn Fn(&mut NetConfig)>;

struct Inner {
    window: slint::Weak<AppWindow>,
    host: RefCell<Arc<dyn Host>>,
    clipboard: RefCell<Rc<dyn Clipboard>>,
    clock: RefCell<Rc<dyn Fn() -> Instant>>,
    net_tweak: RefCell<NetTweak>,
    inbox: Arc<Mutex<VecDeque<Inbound>>>,
    core: RefCell<Core>,
    board: RefCell<crate::app::BoardInput>,
    board_version: Cell<i32>,
    poll: slint::Timer,
}

/// The online mode of one window.
pub struct OnlineUi {
    inner: Rc<Inner>,
}

impl OnlineUi {
    /// Wire the online mode into `window`: callbacks, the poll timer and the close handler.
    pub fn new(window: &AppWindow) -> Self {
        let clock: Rc<dyn Fn() -> Instant> = Rc::new(Instant::now);
        let now = clock();
        let inner = Rc::new(Inner {
            window: window.as_weak(),
            host: RefCell::new(Arc::new(UnavailableHost)),
            clipboard: RefCell::new(Rc::new(SystemClipboard)),
            clock: RefCell::new(clock),
            net_tweak: RefCell::new(Box::new(|_| {})),
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            core: RefCell::new(Core {
                state: OnlineState::new(),
                stage: Stage::Menu,
                intent: None,
                net: None,
                session: 0,
                hosted: None,
                host_started: false,
                remote_url: None,
                nickname_error: String::new(),
                address_error: String::new(),
                code_error: String::new(),
                error: String::new(),
                help: MenuHelp::None,
                busy_text: String::new(),
                autocomplete: Autocomplete::default(),
                pending: None,
                notice: None,
                pending_config: None,
                copied: None,
                confirm_leave: false,
                close_after_leave: false,
                last_sync: now,
                pushed: Pushed::default(),
                sent: Vec::new(),
            }),
            board: RefCell::new(crate::app::BoardInput::default()),
            board_version: Cell::new(0),
            poll: slint::Timer::default(),
        });
        let ui = Self { inner };
        ui.wire(window);
        ui
    }

    // --- test and embedding hooks ------------------------------------------------------------

    /// Use another [`Host`] (the real embedded server, or a fake in tests).
    pub fn set_host(&self, host: Arc<dyn Host>) {
        *self.inner.host.borrow_mut() = host;
    }

    pub fn set_clipboard(&self, clipboard: Rc<dyn Clipboard>) {
        *self.inner.clipboard.borrow_mut() = clipboard;
    }

    /// Replace the clock used for countdowns (tests step it by hand).
    pub fn set_clock(&self, clock: Rc<dyn Fn() -> Instant>) {
        *self.inner.clock.borrow_mut() = clock;
    }

    /// Adjust every [`NetConfig`] before connecting (tests shorten the time-outs).
    pub fn set_net_tweak(&self, tweak: impl Fn(&mut NetConfig) + 'static) {
        *self.inner.net_tweak.borrow_mut() = Box::new(tweak);
    }

    /// A copy of the reducer state.
    pub fn state(&self) -> OnlineState {
        self.inner.core.borrow().state.clone()
    }

    pub fn stage(&self) -> Stage {
        self.inner.core.borrow().stage
    }

    /// The screen currently shown.
    pub fn screen(&self) -> Screen {
        let core = self.inner.core.borrow();
        self.screen_of(&core, self.now())
    }

    /// The last messages the client sent (up to 64), oldest first.
    pub fn sent(&self) -> Vec<ClientMsg> {
        self.inner.core.borrow().sent.clone()
    }

    /// The embedded server's address info while it runs.
    pub fn hosted(&self) -> Option<HostInfo> {
        self.inner.core.borrow().hosted.clone()
    }

    /// Put the online mode on screen in a room, with no socket: for screenshots and view tests.
    /// The room, round and results then come from [`OnlineUi::deliver`].
    pub fn attach_offline(&self, hosted: Option<HostInfo>) {
        let Some(window) = self.inner.window.upgrade() else {
            return;
        };
        {
            let mut core = self.inner.core.borrow_mut();
            core.session += 1;
            core.state = OnlineState::new();
            core.stage = Stage::Connecting;
            core.intent = Some(Intent::Host);
            core.host_started = hosted.is_some();
            core.hosted = hosted;
        }
        window.global::<Online>().set_active(true);
        self.sync();
    }

    /// Feed a server message in as if it had just arrived.
    pub fn deliver(&self, msg: ServerMsg) {
        let session = self.inner.core.borrow().session;
        self.inner.inbox.lock().unwrap().push_back(Inbound::Net {
            session,
            event: NetEvent::Msg(msg),
        });
        self.pump();
    }

    /// Feed a connection event in (reconnecting, lost…).
    pub fn deliver_event(&self, event: NetEvent) {
        let session = self.inner.core.borrow().session;
        self.inner
            .inbox
            .lock()
            .unwrap()
            .push_back(Inbound::Net { session, event });
        self.pump();
    }

    /// Drain the inbox and refresh what changed.
    pub fn pump(&self) {
        let events: Vec<Inbound> = self.inner.inbox.lock().unwrap().drain(..).collect();
        let had_events = !events.is_empty();
        for event in events {
            self.handle_inbound(event);
        }
        let due = {
            let core = self.inner.core.borrow();
            self.now().saturating_duration_since(core.last_sync) >= TICK
        };
        let expired = self.expire_transients();
        if had_events || due || expired {
            self.sync();
        }
    }

    /// The window is being closed. Returns `true` if it may close now; `false` while the
    /// "Quitter fermera la partie pour tout le monde" confirmation is up.
    pub fn request_close(&self) -> bool {
        let hosting = {
            let core = self.inner.core.borrow();
            core.hosted.is_some() && core.stage != Stage::Menu
        };
        if hosting {
            {
                let mut core = self.inner.core.borrow_mut();
                core.confirm_leave = true;
                core.close_after_leave = true;
            }
            self.sync();
            return false;
        }
        self.teardown(true);
        true
    }

    fn now(&self) -> Instant {
        (self.inner.clock.borrow())()
    }

    fn screen_of(&self, core: &Core, now: Instant) -> Screen {
        if core.stage == Stage::InRoom {
            view::screen(&core.state, now)
        } else {
            Screen::Menu
        }
    }

    // --- wiring -------------------------------------------------------------------------------

    fn wire(&self, window: &AppWindow) {
        let online = window.global::<Online>();
        online.set_ticks(board::dial_ticks_commands().into());
        online.set_north_tick(board::north_tick_commands().into());

        macro_rules! on {
            ($setter:ident, |$ui:ident $(, $arg:ident)*| $body:expr) => {{
                let inner = self.inner.clone();
                online.$setter(move |$($arg),*| {
                    let $ui = OnlineUi { inner: inner.clone() };
                    $body
                });
            }};
        }

        on!(on_open, |ui| ui.open());
        on!(on_back, |ui| ui.back());
        on!(on_host, |ui| ui.host_clicked());
        on!(on_join, |ui| ui.join_clicked(Intent::Join(String::new())));
        on!(on_create_room, |ui| ui.join_clicked(Intent::Create));
        on!(on_use_local_network, |ui| ui.use_local_network());
        on!(on_nickname_edited, |ui, text| ui
            .nickname_edited(text.as_str()));
        on!(on_copy_code, |ui| ui.copy(CopyTarget::Code));
        on!(on_copy_address, |ui, index| ui.copy(CopyTarget::Address(
            usize::try_from(index).unwrap_or(0)
        )));
        on!(on_choose_mode, |ui, mode| ui.choose_mode(mode));
        on!(on_choose_level, |ui, level| ui.choose_level(level));
        on!(on_step_rounds, |ui, delta| ui.step(Setting::Rounds, delta));
        on!(on_step_players, |ui, delta| ui
            .step(Setting::MaxPlayers, delta));
        on!(on_step_time, |ui, delta| ui.step(Setting::Time, delta));
        on!(on_start_match, |ui| ui.start_match());
        on!(on_leave, |ui| ui.leave_clicked());
        on!(on_confirm_leave, |ui| ui.confirm_leave());
        on!(on_cancel_leave, |ui| ui.cancel_leave());
        on!(on_query_edited, |ui, text| ui.query_edited(text.as_str()));
        on!(on_submit_guess, |ui, index| ui.submit(index));
        on!(on_move_highlight, |ui, delta| ui.move_highlight(delta));
        on!(on_dismiss_suggestions, |ui| ui.dismiss_suggestions());
        on!(on_toggle_browse, |ui| ui.toggle_browse());
        on!(on_show_ranking, |ui| ui.show_ranking());
        on!(on_back_to_lobby, |ui| ui.back_to_lobby());
        on!(on_back_to_menu, |ui| {
            ui.teardown(false);
            ui.sync();
        });
        on!(on_pump, |ui| ui.pump());

        {
            let inner = self.inner.clone();
            online.on_compute_board(move |width, height, _version| {
                compute_board(width, height, &inner.board.borrow())
            });
        }

        // Window close: tell the room (and ask first if closing ends a hosted game).
        {
            let inner = self.inner.clone();
            window.window().on_close_requested(move || {
                let ui = OnlineUi {
                    inner: inner.clone(),
                };
                if ui.request_close() {
                    slint::CloseRequestResponse::HideWindow
                } else {
                    slint::CloseRequestResponse::KeepWindowShown
                }
            });
        }

        // The poll: inbox and countdowns.
        {
            let inner = self.inner.clone();
            self.inner
                .poll
                .start(slint::TimerMode::Repeated, POLL, move || {
                    OnlineUi {
                        inner: inner.clone(),
                    }
                    .pump();
                });
        }
    }

    // --- menu ---------------------------------------------------------------------------------

    fn open(&self) {
        let Some(window) = self.inner.window.upgrade() else {
            return;
        };
        {
            let mut core = self.inner.core.borrow_mut();
            if core.stage == Stage::Menu {
                core.state = OnlineState::new();
                clear_menu_messages(&mut core);
            }
        }
        window.global::<Online>().set_active(true);
        self.sync();
    }

    /// "Retour" on the menu: back to the start screen.
    fn back(&self) {
        let Some(window) = self.inner.window.upgrade() else {
            return;
        };
        self.teardown(false);
        window.global::<Online>().set_active(false);
        self.sync();
    }

    fn nickname_edited(&self, text: &str) {
        if text.chars().count() > boussole_protocol::NICKNAME_MAX_CHARS
            && let Some(window) = self.inner.window.upgrade()
        {
            let clamped: String = text
                .chars()
                .take(boussole_protocol::NICKNAME_MAX_CHARS)
                .collect();
            window.global::<Online>().set_nickname(clamped.into());
        }
        let mut core = self.inner.core.borrow_mut();
        core.nickname_error.clear();
    }

    fn read_nickname(&self, core: &mut Core) -> Option<String> {
        let window = self.inner.window.upgrade()?;
        let raw = window.global::<Online>().get_nickname();
        match normalize_nickname(raw.as_str()) {
            Some(nick) => Some(nick),
            None => {
                core.nickname_error = "1 à 16 caractères, sans caractère de contrôle".to_owned();
                None
            }
        }
    }

    fn host_clicked(&self) {
        let Some(window) = self.inner.window.upgrade() else {
            return;
        };
        let mode = match window.global::<Online>().get_host_choice() {
            HostChoice::Lan => HostMode::Lan,
            HostChoice::Tunnel => HostMode::Tunnel,
        };
        let session;
        {
            let mut core = self.inner.core.borrow_mut();
            if core.stage != Stage::Menu {
                return;
            }
            clear_menu_messages(&mut core);
            if self.read_nickname(&mut core).is_none() {
                drop(core);
                self.sync();
                return;
            }
            core.stage = Stage::Starting;
            core.intent = Some(Intent::Host);
            core.busy_text = match mode {
                HostMode::Lan => "Démarrage de la partie…".to_owned(),
                HostMode::Tunnel => "Ouverture du tunnel gratuit… (quelques secondes)".to_owned(),
            };
            core.session += 1;
            session = core.session;
        }
        // Binding a port and waiting for the tunnel's address can take seconds: not on this thread.
        let host = self.inner.host.borrow().clone();
        let inbox = self.inner.inbox.clone();
        let weak = self.inner.window.clone();
        std::thread::spawn(move || {
            let result = host.start(mode);
            inbox
                .lock()
                .unwrap()
                .push_back(Inbound::Host { session, result });
            wake(&weak);
        });
        self.sync();
    }

    /// "Rejoindre" (`Intent::Join`, the code is read from the form) or "Créer une salle".
    fn join_clicked(&self, intent: Intent) {
        let Some(window) = self.inner.window.upgrade() else {
            return;
        };
        let online = window.global::<Online>();
        let mut start: Option<(String, String)> = None;
        {
            let mut core = self.inner.core.borrow_mut();
            if core.stage != Stage::Menu {
                return;
            }
            clear_menu_messages(&mut core);
            let nick = self.read_nickname(&mut core);
            let url = match normalize_address(online.get_address().as_str()) {
                Ok(url) => Some(url),
                Err(error) => {
                    core.address_error = address_error_text(&error);
                    None
                }
            };
            let intent = match intent {
                Intent::Join(_) => match normalize_room_code(online.get_code_input().as_str()) {
                    Some(code) => Some(Intent::Join(code)),
                    None => {
                        core.code_error = "5 caractères : lettres et chiffres".to_owned();
                        None
                    }
                },
                other => Some(other),
            };
            if let (Some(nick), Some(url), Some(intent)) = (nick, url, intent) {
                core.intent = Some(intent);
                core.stage = Stage::Connecting;
                core.busy_text = "Connexion au serveur…".to_owned();
                core.remote_url = Some(url.clone());
                start = Some((url, nick));
            }
        }
        if let Some((url, nick)) = start {
            self.start_connection(&url, &nick);
        }
        self.sync();
    }

    /// After "cloudflared n'est pas installé": switch to the local network and host again.
    fn use_local_network(&self) {
        if let Some(window) = self.inner.window.upgrade() {
            window.global::<Online>().set_host_choice(HostChoice::Lan);
        }
        self.host_clicked();
    }

    fn start_connection(&self, url: &str, nickname: &str) {
        let mut config = NetConfig::new(url, nickname);
        (self.inner.net_tweak.borrow())(&mut config);
        let session = {
            let mut core = self.inner.core.borrow_mut();
            core.session += 1;
            core.state = OnlineState::new();
            core.session
        };
        let inbox = self.inner.inbox.clone();
        let weak = self.inner.window.clone();
        let handle = NetHandle::connect(config, move |event| {
            inbox
                .lock()
                .unwrap()
                .push_back(Inbound::Net { session, event });
            wake(&weak);
        });
        self.inner.core.borrow_mut().net = Some(handle);
    }

    // --- events from other threads --------------------------------------------------------------

    fn handle_inbound(&self, inbound: Inbound) {
        match inbound {
            Inbound::Host { session, result } => self.on_host_result(session, result),
            Inbound::Net { session, event } => {
                if session == self.inner.core.borrow().session {
                    self.on_net_event(event);
                }
            }
        }
    }

    fn on_host_result(&self, session: u64, result: Result<HostInfo, HostError>) {
        let current = self.inner.core.borrow().session;
        if session != current {
            // The player gave up while it was starting: shut what just came up back down.
            if result.is_ok() {
                self.stop_host_in_background();
            }
            return;
        }
        match result {
            Ok(info) => {
                let url = info.local_url.clone();
                let nick = {
                    let mut core = self.inner.core.borrow_mut();
                    core.hosted = Some(info);
                    core.host_started = true;
                    core.stage = Stage::Connecting;
                    core.busy_text = "Création de la salle…".to_owned();
                    self.inner
                        .window
                        .upgrade()
                        .map(|w| w.global::<Online>().get_nickname().to_string())
                        .and_then(|raw| normalize_nickname(&raw))
                };
                match nick {
                    Some(nick) => self.start_connection(&url, &nick),
                    None => self.fail_to_menu("Pseudo invalide.".to_owned(), None),
                }
            }
            Err(error) => {
                let help = if error == HostError::TunnelNotInstalled {
                    MenuHelp::TunnelMissing
                } else {
                    MenuHelp::None
                };
                self.fail_to_menu(error.to_string(), Some(help));
            }
        }
    }

    fn on_net_event(&self, event: NetEvent) {
        match event {
            NetEvent::Msg(msg) => self.on_server_msg(msg),
            NetEvent::Reconnecting { attempt } => {
                let now = self.now();
                let mut core = self.inner.core.borrow_mut();
                let since = match core.state.link {
                    Link::Reconnecting { since, .. } => since,
                    _ => now,
                };
                core.state = set_link(
                    std::mem::take(&mut core.state),
                    Link::Reconnecting { since, attempt },
                );
                core.pending = None;
            }
            NetEvent::Failed(error) => {
                let stage = self.inner.core.borrow().stage;
                if matches!(stage, Stage::Connecting | Stage::Starting) {
                    self.fail_to_menu(error.to_string(), None);
                } else {
                    let mut core = self.inner.core.borrow_mut();
                    core.state = set_link(std::mem::take(&mut core.state), Link::Lost);
                    core.pending = None;
                }
            }
            NetEvent::Closed => {}
        }
    }

    fn on_server_msg(&self, msg: ServerMsg) {
        let now = self.now();
        let mut fail: Option<(String, Option<ErrorCode>)> = None;
        {
            let mut core = self.inner.core.borrow_mut();
            let fresh_welcome = matches!(msg, ServerMsg::Welcome { resumed: false, .. });
            let new_round = matches!(msg, ServerMsg::RoundStarted(_));
            let my_guess = match &msg {
                ServerMsg::GuessResult(info) => Some(info.by) == core.state.me,
                _ => false,
            };
            let is_snapshot = matches!(msg, ServerMsg::RoomSnapshot(_));
            let is_error = matches!(msg, ServerMsg::Error { .. });

            let before = std::mem::take(&mut core.state);
            core.state = apply(before, msg, now);

            if is_snapshot {
                core.pending_config = None;
            }
            if new_round {
                core.autocomplete.clear();
                core.notice = None;
                core.pending = None;
            }
            if my_guess {
                core.pending = None;
                core.autocomplete.clear();
                core.notice = None;
            }
            if fresh_welcome && core.stage == Stage::Connecting {
                core.state.begin_room_request();
                let request = match core.intent.clone() {
                    Some(Intent::Join(code)) => ClientMsg::JoinRoom { code },
                    _ => ClientMsg::CreateRoom {
                        config: RoomConfig::new(Mode::Course, Difficulty::Facile),
                    },
                };
                send(&mut core, request);
            }
            if core.stage == Stage::Connecting && core.state.room.is_some() {
                core.stage = Stage::InRoom;
                clear_menu_messages(&mut core);
            } else if is_error {
                let code = core.state.last_error;
                let text = core.state.notice.as_ref().map(|n| n.text.clone());
                match (core.stage, text) {
                    (Stage::Connecting, Some(text)) => fail = Some((text, code)),
                    (Stage::InRoom, Some(text)) => {
                        core.pending = None;
                        core.notice = Some(text);
                    }
                    _ => {}
                }
            }
            if core.stage == Stage::InRoom && core.state.room.is_none() {
                // The room is gone (closed, or the server forgot us): back to the menu.
                let text = core.state.notice.as_ref().map(|n| n.text.clone());
                fail = Some((text.unwrap_or_default(), None));
            }
        }
        if let Some((text, code)) = fail {
            self.fail_to_menu(text, None);
            if let Some(code) = code {
                self.mark_field_error(code);
            }
        }
    }

    /// Show `text` on the menu and tear everything down (connection, embedded server).
    fn fail_to_menu(&self, text: String, help: Option<MenuHelp>) {
        self.teardown(false);
        let mut core = self.inner.core.borrow_mut();
        core.error = text;
        core.help = help.unwrap_or(MenuHelp::None);
    }

    /// Point an error at the form field that caused it.
    fn mark_field_error(&self, code: ErrorCode) {
        let mut core = self.inner.core.borrow_mut();
        match code {
            ErrorCode::RoomNotFound => {
                core.code_error = "salle introuvable".to_owned();
            }
            ErrorCode::NicknameTaken | ErrorCode::InvalidNickname => {
                core.nickname_error = "pseudo refusé".to_owned();
            }
            _ => {}
        }
    }

    // --- leaving ----------------------------------------------------------------------------------

    fn leave_clicked(&self) {
        let hosting = self.inner.core.borrow().hosted.is_some();
        if hosting {
            self.inner.core.borrow_mut().confirm_leave = true;
            self.sync();
        } else {
            self.teardown(false);
            self.sync();
        }
    }

    fn confirm_leave(&self) {
        let close_window = {
            let mut core = self.inner.core.borrow_mut();
            core.confirm_leave = false;
            std::mem::take(&mut core.close_after_leave)
        };
        self.teardown(close_window);
        self.sync();
        if close_window && let Some(window) = self.inner.window.upgrade() {
            let _ = window.hide();
        }
    }

    fn cancel_leave(&self) {
        {
            let mut core = self.inner.core.borrow_mut();
            core.confirm_leave = false;
            core.close_after_leave = false;
        }
        self.sync();
    }

    /// Leave the room, close the connection, stop the embedded server, back to the menu.
    /// `blocking`: wait for the server to stop (the window is closing); otherwise it stops in
    /// the background.
    fn teardown(&self, blocking: bool) {
        let (net, host_started) = {
            let mut core = self.inner.core.borrow_mut();
            let in_room = core.state.room.is_some();
            let net = core.net.take();
            if in_room {
                send_via(&mut core.sent, net.as_ref(), ClientMsg::LeaveRoom);
            }
            core.session += 1; // late events of the old connection are dropped
            core.state.leave_room_locally();
            core.state = OnlineState::new();
            core.stage = Stage::Menu;
            core.intent = None;
            core.hosted = None;
            core.remote_url = None;
            core.autocomplete.clear();
            core.pending = None;
            core.notice = None;
            core.pending_config = None;
            core.copied = None;
            core.confirm_leave = false;
            let started = std::mem::take(&mut core.host_started);
            (net, started)
        };
        if let Some(net) = net {
            net.close(); // sends what is queued (leave_room) first, then closes
        }
        if host_started {
            if blocking {
                self.inner.host.borrow().stop();
            } else {
                self.stop_host_in_background();
            }
        }
    }

    fn stop_host_in_background(&self) {
        let host = self.inner.host.borrow().clone();
        std::thread::spawn(move || host.stop());
    }

    // --- lobby ------------------------------------------------------------------------------------

    fn copy(&self, target: CopyTarget) {
        let now = self.now();
        let text = {
            let core = self.inner.core.borrow();
            match target {
                CopyTarget::Code => core.state.room.as_ref().map(|r| r.code.clone()),
                CopyTarget::Address(index) => {
                    share_addresses(&core).get(index).map(|a| a.url.clone())
                }
            }
        };
        let Some(text) = text else { return };
        let result = self.inner.clipboard.borrow().set_text(&text);
        {
            let mut core = self.inner.core.borrow_mut();
            match result {
                Ok(()) => {
                    core.copied = Some((target, now));
                    core.notice = None;
                }
                Err(_) => {
                    core.notice = Some(
                        "Copie impossible : sélectionnez le texte et copiez-le à la main."
                            .to_owned(),
                    );
                }
            }
        }
        self.sync();
    }

    fn current_config(&self, core: &Core) -> Option<RoomConfig> {
        core.pending_config
            .or(core.state.room.as_ref().map(|r| r.config))
    }

    fn send_config(&self, change: impl FnOnce(RoomConfig, usize) -> Option<RoomConfig>) {
        {
            let mut core = self.inner.core.borrow_mut();
            if !core.state.am_host() || core.state.link != Link::Online {
                return;
            }
            let Some(config) = self.current_config(&core) else {
                return;
            };
            let members = core.state.room.as_ref().map_or(0, |r| r.players.len());
            let Some(next) = change(config, members) else {
                return;
            };
            if next.validate().is_err() {
                return;
            }
            core.pending_config = Some(next);
            core.notice = None;
            send(&mut core, ClientMsg::UpdateConfig { config: next });
        }
        self.sync();
    }

    fn choose_mode(&self, mode: RoomMode) {
        let mode = match mode {
            RoomMode::Course => Mode::Course,
            RoomMode::Coop => Mode::Cooperation,
        };
        self.send_config(|config, _| {
            (config.mode != mode).then_some(RoomConfig { mode, ..config })
        });
    }

    fn choose_level(&self, level: Level) {
        let difficulty = match level {
            Level::Facile => Difficulty::Facile,
            Level::Difficile => Difficulty::Difficile,
        };
        self.send_config(|config, _| {
            (config.difficulty != difficulty).then_some(RoomConfig {
                difficulty,
                ..config
            })
        });
    }

    fn step(&self, setting: Setting, delta: i32) {
        self.send_config(|config, members| step_setting(&config, setting, delta, members));
    }

    fn start_match(&self) {
        {
            let mut core = self.inner.core.borrow_mut();
            let allowed = view::lobby(&core.state).is_some_and(|lobby| lobby.can_start);
            if !allowed {
                return;
            }
            core.notice = None;
            send(&mut core, ClientMsg::StartMatch);
        }
        self.sync();
    }

    // --- round --------------------------------------------------------------------------------------

    fn query_edited(&self, text: &str) {
        {
            let mut core = self.inner.core.borrow_mut();
            core.autocomplete.set_query(text);
            core.notice = None;
        }
        self.sync();
    }

    fn move_highlight(&self, delta: i32) {
        self.inner
            .core
            .borrow_mut()
            .autocomplete
            .move_highlight(delta);
        self.sync();
    }

    fn dismiss_suggestions(&self) {
        self.inner.core.borrow_mut().autocomplete.dismiss();
        self.sync();
    }

    fn toggle_browse(&self) {
        {
            let mut core = self.inner.core.borrow_mut();
            core.autocomplete.toggle_browse();
            core.notice = None;
        }
        self.sync();
    }

    fn submit(&self, index: i32) {
        let now = self.now();
        {
            let mut core = self.inner.core.borrow_mut();
            let Some(round) = view::round(&core.state, now) else {
                return;
            };
            if !round.input.enabled || core.pending.is_some() {
                return;
            }
            let city = match usize::try_from(index) {
                Ok(i) => core.autocomplete.pick(i).ok_or(Rejection::UnknownCity),
                Err(_) => core.autocomplete.resolve(),
            };
            match city {
                Err(rejection) => core.notice = Some(rejection.to_string()),
                Ok(city) if round.history.iter().any(|g| g.city == city.name) => {
                    core.notice = Some(Rejection::AlreadyGuessed(city.name).to_string());
                    core.autocomplete.close_list();
                }
                Ok(city) => {
                    core.notice = None;
                    core.autocomplete.close_list();
                    core.pending = Some((city.name.to_owned(), now));
                    send(
                        &mut core,
                        ClientMsg::Guess {
                            city: city.name.to_owned(),
                        },
                    );
                }
            }
        }
        self.sync();
    }

    fn show_ranking(&self) {
        let now = self.now();
        self.inner.core.borrow_mut().state.skip_result(now);
        self.sync();
    }

    fn back_to_lobby(&self) {
        self.inner.core.borrow_mut().state.dismiss_final();
        self.sync();
    }

    /// Time-based clean-ups; `true` if something changed on screen.
    fn expire_transients(&self) -> bool {
        let now = self.now();
        let mut core = self.inner.core.borrow_mut();
        let mut changed = false;
        if let Some((_, at)) = core.copied
            && now.saturating_duration_since(at) >= COPIED_FOR
        {
            core.copied = None;
            changed = true;
        }
        if let Some((_, at)) = &core.pending
            && now.saturating_duration_since(*at) >= GUESS_TIMEOUT
        {
            core.pending = None;
            core.notice = Some("Le serveur ne répond pas : réessayez.".to_owned());
            changed = true;
        }
        changed
    }
}

// --- helpers on Core ------------------------------------------------------------------------------

fn clear_menu_messages(core: &mut Core) {
    core.nickname_error.clear();
    core.address_error.clear();
    core.code_error.clear();
    core.error.clear();
    core.help = MenuHelp::None;
}

fn send(core: &mut Core, msg: ClientMsg) {
    send_via(&mut core.sent, core.net.as_ref(), msg);
}

fn send_via(log: &mut Vec<ClientMsg>, net: Option<&NetHandle>, msg: ClientMsg) {
    log.push(msg.clone());
    if log.len() > 64 {
        log.remove(0);
    }
    if let Some(net) = net {
        net.send(msg);
    }
}

/// Wake the UI thread so it drains its inbox now (the poll would get there within 40 ms; this is
/// for immediacy). Where the platform has no event-loop proxy (the headless test platform) the
/// call fails and the poll does the work.
fn wake(window: &slint::Weak<AppWindow>) {
    let _ = window.upgrade_in_event_loop(|window| window.global::<Online>().invoke_pump());
}

fn address_error_text(error: &AddressError) -> String {
    match error {
        AddressError::Empty => "saisissez l’adresse".to_owned(),
        AddressError::UnsupportedScheme(_) => "ws://, wss:// ou https://".to_owned(),
        AddressError::InvalidHost => "nom d’hôte invalide".to_owned(),
        AddressError::InvalidPort => "port invalide (1 à 65535)".to_owned(),
    }
}

/// The addresses the host shares: the embedded server's, or the remote server's.
fn share_addresses(core: &Core) -> Vec<ShareAddress> {
    if let Some(info) = &core.hosted {
        return info.share_addresses();
    }
    if core.state.am_host()
        && let Some(url) = &core.remote_url
    {
        return vec![ShareAddress {
            label: "Serveur".to_owned(),
            url: url.clone(),
        }];
    }
    Vec::new()
}

// --- view-model → window ----------------------------------------------------------------------------

fn level_of(difficulty: Difficulty) -> Level {
    match difficulty {
        Difficulty::Facile => Level::Facile,
        Difficulty::Difficile => Level::Difficile,
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Course => "Course",
        Mode::Cooperation => "Coopération",
    }
}

fn level_name(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Facile => "Facile",
        Difficulty::Difficile => "Difficile",
    }
}

fn points_text(points: u32) -> String {
    board::format_km(f64::from(points))
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

fn initial_of(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

fn history_rows(rows: &[view::HistoryRow]) -> Vec<GuessRow> {
    rows.iter()
        .map(|g| GuessRow {
            name: g.city.as_str().into(),
            distance: if g.correct {
                "Trouvé".into()
            } else {
                board::format_distance(f64::from(g.distance_km)).into()
            },
            arrow: if g.correct {
                "M 4 10.5 L 8.5 15 L 16.5 5".into()
            } else {
                board::direction_arrow_commands(g.bearing_deg, 20.0).into()
            },
            direction: crate::app::direction_phrase(crate::geo::Compass8::from_bearing(
                g.bearing_deg,
            ))
            .into(),
            correct: g.correct,
            by: g.by.clone().unwrap_or_default().into(),
            initial: g.by.as_deref().map(initial_of).unwrap_or_default().into(),
        })
        .collect()
}

impl OnlineUi {
    /// Copy the whole view state into the window.
    pub fn sync(&self) {
        let Some(window) = self.inner.window.upgrade() else {
            return;
        };
        let now = self.now();
        let online = window.global::<Online>();
        let mut guard = self.inner.core.borrow_mut();
        let core = &mut *guard;
        core.last_sync = now;

        let screen = self.screen_of(core, now);
        online.set_screen(match screen {
            Screen::Menu => OnlineScreen::Menu,
            Screen::Lobby => OnlineScreen::Lobby,
            Screen::Round => OnlineScreen::Round,
            Screen::Result => OnlineScreen::Result,
            Screen::Final => OnlineScreen::Podium,
        });

        // Connection strip
        match view::banner(&core.state, now) {
            Some(Banner::Reconnecting { seconds_left, .. }) => {
                online.set_banner_kind(BannerKind::Reconnecting);
                online.set_banner_text(format!("Reconnexion… {seconds_left} s").into());
            }
            Some(Banner::Lost { text }) => {
                online.set_banner_kind(BannerKind::Lost);
                online.set_banner_text(text.into());
            }
            None => {
                online.set_banner_kind(BannerKind::None);
                online.set_banner_text("".into());
            }
        }

        online.set_confirm_leave_open(core.confirm_leave);
        online.set_leave_warning("Quitter fermera la partie pour tout le monde.".into());
        online.set_is_host(core.state.am_host());

        match screen {
            Screen::Menu => self.sync_menu(&online, core),
            Screen::Lobby => self.sync_lobby(&online, core, now),
            Screen::Round => self.sync_round(&online, core, now),
            Screen::Result => self.sync_result(&online, core, now),
            Screen::Final => self.sync_final(&online, core),
        }
    }

    fn sync_menu(&self, online: &Online<'_>, core: &mut Core) {
        let busy = matches!(core.stage, Stage::Starting | Stage::Connecting);
        online.set_busy(busy);
        online.set_busy_text(if busy {
            core.busy_text.as_str().into()
        } else {
            "".into()
        });
        online.set_nickname_error(core.nickname_error.as_str().into());
        online.set_address_error(core.address_error.as_str().into());
        online.set_code_error(core.code_error.as_str().into());
        online.set_error(core.error.as_str().into());
        online.set_help(core.help);
    }

    fn sync_lobby(&self, online: &Online<'_>, core: &mut Core, _now: Instant) {
        let Some(lobby) = view::lobby(&core.state) else {
            return;
        };
        online.set_room_code(lobby.code.as_str().into());
        online.set_code_copied(matches!(core.copied, Some((CopyTarget::Code, _))));

        let share = share_addresses(core);
        let share_pairs: Vec<(ShareAddress, bool)> = share
            .iter()
            .enumerate()
            .map(|(i, a)| {
                (
                    a.clone(),
                    matches!(core.copied, Some((CopyTarget::Address(j), _)) if j == i),
                )
            })
            .collect();
        if core.pushed.share != share_pairs {
            online.set_share_addresses(model(
                share_pairs
                    .iter()
                    .map(|(a, copied)| ShareAddressView {
                        label: a.label.as_str().into(),
                        url: a.url.as_str().into(),
                        copied: *copied,
                    })
                    .collect(),
            ));
            core.pushed.share = share_pairs;
        }
        let hint = if let Some(notice) = &core.notice {
            notice.clone()
        } else if !lobby.i_am_host {
            "Attendez que l’hôte lance la partie.".to_owned()
        } else if core.hosted.is_some() && share.is_empty() {
            "Aucune adresse réseau détectée : vérifiez votre Wi-Fi ou utilisez le tunnel."
                .to_owned()
        } else if core.hosted.is_some() {
            "Partagez l’adresse et le code avec vos amis.".to_owned()
        } else {
            "Donnez le code (et l’adresse du serveur) à vos amis.".to_owned()
        };
        online.set_share_hint(hint.into());

        let config = core.pending_config.unwrap_or_else(|| {
            core.state
                .room
                .as_ref()
                .map(|r| r.config)
                .unwrap_or(RoomConfig::new(Mode::Course, Difficulty::Facile))
        });
        let members = core.state.room.as_ref().map_or(0, |r| r.players.len());
        let settings = &lobby.settings;
        online.set_settings_editable(settings.editable);
        online.set_settings_note(
            if settings.editable {
                "Vous êtes l’hôte : modifiez les réglages."
            } else {
                "Seul l’hôte peut modifier les réglages."
            }
            .into(),
        );
        online.set_setting_mode(match config.mode {
            Mode::Course => RoomMode::Course,
            Mode::Cooperation => RoomMode::Coop,
        });
        online.set_setting_level(level_of(config.difficulty));
        online.set_setting_rounds(config.rounds.to_string().into());
        online.set_setting_max_players(config.max_players.to_string().into());
        online.set_setting_time_label(
            match config.mode {
                Mode::Course => "Temps par manche",
                Mode::Cooperation => "Temps par tour",
            }
            .into(),
        );
        let time_s = match config.mode {
            Mode::Course => config.round_time_s,
            Mode::Cooperation => config.turn_time_s,
        };
        online.set_setting_time(format_clock(Duration::from_secs(u64::from(time_s))).into());
        let can = |setting, delta| step_setting(&config, setting, delta, members).is_some();
        online.set_rounds_can_dec(can(Setting::Rounds, -1));
        online.set_rounds_can_inc(can(Setting::Rounds, 1));
        online.set_players_can_dec(can(Setting::MaxPlayers, -1));
        online.set_players_can_inc(can(Setting::MaxPlayers, 1));
        online.set_time_can_dec(can(Setting::Time, -1));
        online.set_time_can_inc(can(Setting::Time, 1));

        online.set_players_title(
            format!("Joueurs ({}/{})", lobby.players.len(), config.max_players).into(),
        );
        if core.pushed.players != lobby.players {
            online.set_players(model(
                lobby
                    .players
                    .iter()
                    .map(|p| PlayerRowView {
                        name: p.name.as_str().into(),
                        host: p.host,
                        me: p.me,
                        connected: p.connected,
                        description: p.description.as_str().into(),
                    })
                    .collect(),
            ));
            core.pushed.players = lobby.players.clone();
        }
        online.set_can_start(lobby.can_start);
        online.set_start_hint(lobby.start_hint.as_str().into());
    }

    /// Board input for the current round or result; bumps the board version when it changed.
    fn update_board(&self, core: &mut Core, hints: Vec<HintLabel>, revealed: Option<String>) {
        let key = (hints.clone(), revealed.clone());
        if core.pushed.board_key.as_ref() == Some(&key) {
            return;
        }
        *self.inner.board.borrow_mut() = crate::app::BoardInput {
            hints,
            revealed: revealed.as_deref().and_then(find_exact),
        };
        core.pushed.board_key = Some(key);
        self.inner
            .board_version
            .set(self.inner.board_version.get() + 1);
        if let Some(window) = self.inner.window.upgrade() {
            window
                .global::<Online>()
                .set_board_version(self.inner.board_version.get());
        }
    }

    fn sync_suggestions(
        &self,
        online: &Online<'_>,
        core: &mut Core,
        tried: &[String],
        enabled: bool,
    ) {
        let autocomplete = &core.autocomplete;
        if online.get_query().as_str() != autocomplete.query() {
            online.set_query(autocomplete.query().into());
        }
        let suggestions: Vec<(String, bool)> = autocomplete
            .suggestions()
            .iter()
            .map(|city| (city.name.to_owned(), tried.iter().any(|t| t == city.name)))
            .collect();
        if core.pushed.suggestions != suggestions {
            online.set_suggestions(model(
                suggestions
                    .iter()
                    .map(|(name, tried)| Suggestion {
                        name: name.as_str().into(),
                        tried: *tried,
                    })
                    .collect(),
            ));
            core.pushed.suggestions = suggestions;
        }
        online.set_highlighted(i32::try_from(autocomplete.highlighted()).unwrap_or(0));
        online.set_suggestions_open(autocomplete.is_open() && enabled);
    }

    fn sync_round(&self, online: &Online<'_>, core: &mut Core, now: Instant) {
        let Some(round) = view::round(&core.state, now) else {
            return;
        };
        let coop = round.mode == Mode::Cooperation;
        online.set_result_view(false);
        online.set_coop(coop);
        online.set_mode_badge(
            format!(
                "{} · {}",
                mode_name(round.mode),
                level_name(round.difficulty)
            )
            .into(),
        );
        online.set_round_label(
            format!(
                "Manche {} sur {} · {} · {}",
                round.index,
                round.total,
                mode_name(round.mode),
                level_name(round.difficulty)
            )
            .into(),
        );
        online.set_clock_text(round.clock.text.as_str().into());
        online.set_clock_urgent(round.clock.urgent);
        online.set_clock_caption(
            if coop {
                if round.my_turn {
                    "Ton tour".to_owned()
                } else {
                    let name = round
                        .banner
                        .trim_start_matches("Au tour de ")
                        .trim_end_matches('…');
                    format!("Tour de {name}")
                }
            } else {
                "Temps restant".to_owned()
            }
            .into(),
        );
        online.set_turn_banner(round.banner.as_str().into());
        online.set_my_turn(round.my_turn);
        online.set_tries_used(i32::from(round.tries_used));
        online.set_max_tries(i32::from(round.max_tries));
        online.set_scoreboard_title(
            if coop {
                "Ordre de jeu"
            } else {
                "Classement en direct"
            }
            .into(),
        );
        online.set_team_note(
            if coop {
                core.state
                    .room
                    .as_ref()
                    .and_then(|r| r.players.first())
                    .map_or(String::new(), |p| {
                        format!("Score d’équipe : {}", points_text(p.score))
                    })
            } else {
                String::new()
            }
            .into(),
        );
        online.set_history_title(
            if coop {
                "Essais de l’équipe : distance et direction vers la ville cachée"
            } else {
                "Distance et direction de chaque essai vers la ville cachée"
            }
            .into(),
        );

        if core.pushed.scoreboard != round.scoreboard {
            online.set_scoreboard(model(
                round
                    .scoreboard
                    .iter()
                    .map(|r| ScoreRowView {
                        name: r.name.as_str().into(),
                        me: r.me,
                        host: r.host,
                        connected: r.connected,
                        tries: i32::from(r.tries),
                        status: match r.status {
                            ProgressStatus::Playing => DotStatus::Playing,
                            ProgressStatus::Solved => DotStatus::Solved,
                            ProgressStatus::OutOfTries => DotStatus::Out,
                        },
                        active: r.active,
                        description: r.description.as_str().into(),
                    })
                    .collect(),
            ));
            core.pushed.scoreboard = round.scoreboard.clone();
        }
        if core.pushed.history != round.history {
            online.set_history(model(history_rows(&round.history)));
            core.pushed.history = round.history.clone();
        }

        // The field: closed when it is not my turn, when I am done, or while a guess is in flight
        let pending = core.pending.is_some();
        let enabled = round.input.enabled && !pending;
        online.set_input_enabled(enabled);
        online.set_input_message(
            if pending {
                "Envoi…".to_owned()
            } else {
                round.input.message.clone()
            }
            .into(),
        );
        online.set_notice(core.notice.clone().unwrap_or_default().into());
        let tried: Vec<String> = round.history.iter().map(|g| g.city.clone()).collect();
        self.sync_suggestions(online, core, &tried, enabled);

        self.update_board(core, round.hints.clone(), None);
    }

    fn sync_result(&self, online: &Online<'_>, core: &mut Core, now: Instant) {
        let Some(result) = view::result(&core.state, now) else {
            return;
        };
        let coop = result.mode == Mode::Cooperation;
        online.set_result_view(true);
        online.set_coop(coop);
        online.set_round_label(
            format!(
                "{} · {} · {}",
                result.round_label,
                mode_name(result.mode),
                level_name(result.difficulty)
            )
            .into(),
        );
        online.set_headline(result.headline.as_str().into());
        online.set_headline_good(
            result
                .rows
                .iter()
                .any(|r| (r.me && r.solved) || (coop && r.found)),
        );
        online.set_countdown(result.countdown.clone().unwrap_or_default().into());
        online.set_last_round(result.last_round);
        online.set_results_title(
            if coop {
                "Résultat de l’équipe"
            } else {
                "Résultats de la manche"
            }
            .into(),
        );
        online.set_team_note(
            result
                .team_points
                .map(|p| format!("+{} points pour l’équipe", points_text(p)))
                .unwrap_or_default()
                .into(),
        );
        online.set_history_title("Vos essais, distance et direction vers la ville".into());
        online.set_input_enabled(false);
        online.set_suggestions_open(false);
        online.set_notice("".into());

        if core.pushed.results != result.rows {
            online.set_results(model(
                result
                    .rows
                    .iter()
                    .map(|r| ResultRowView {
                        name: r.name.as_str().into(),
                        me: r.me,
                        solved: r.solved,
                        found: r.found,
                        points: if coop {
                            String::new()
                        } else {
                            format!("+{}", points_text(r.points))
                        }
                        .into(),
                        total: points_text(r.total).into(),
                        guesses: r.guesses.as_str().into(),
                        description: r.description.as_str().into(),
                    })
                    .collect(),
            ));
            core.pushed.results = result.rows.clone();
        }
        if core.pushed.history != result.history {
            online.set_history(model(history_rows(&result.history)));
            core.pushed.history = result.history.clone();
        }
        self.update_board(core, result.hints.clone(), Some(result.secret.clone()));
    }

    fn sync_final(&self, online: &Online<'_>, core: &mut Core) {
        let Some(podium) = view::final_ranking(&core.state) else {
            return;
        };
        online.set_podium_title(podium.title.as_str().into());
        online.set_team_total(
            podium
                .team_total
                .map(|t| format!("{} points", points_text(t)))
                .unwrap_or_default()
                .into(),
        );
        if core.pushed.ranking != podium.rows {
            online.set_ranking(model(
                podium
                    .rows
                    .iter()
                    .map(|r| RankRowView {
                        rank: r.rank.as_str().into(),
                        name: r.name.as_str().into(),
                        me: r.me,
                        total: points_text(r.total).into(),
                        detail: r.description.as_str().into(),
                        first: podium.mode == Mode::Course && r.rank == "1",
                    })
                    .collect(),
            ));
            core.pushed.ranking = podium.rows.clone();
        }
    }
}
