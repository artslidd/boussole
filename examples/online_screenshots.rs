//! Renders the online screens headlessly (software renderer, no window, no socket) into
//! `docs/screenshots/online/`. Every scene is scripted from protocol messages fed to the real
//! controller, with a hand-driven clock, so the output is reproducible.
//!
//! ```sh
//! cargo run --release --example online_screenshots          # into docs/screenshots/online/
//! cargo run --release --example online_screenshots -- out/  # elsewhere
//! ```

use boussole::app::{App, HostChoice, Online};
use boussole::clipboard::MemoryClipboard;
use boussole::headless::Headless;
use boussole::host::{FakeHost, HostError, HostInfo};
use boussole::net::NetEvent;
use boussole_protocol::{
    Difficulty, GuessEntry, GuessInfo, HintDto, MatchEnded, Mode, PlayerInfo, PlayerRoundResult,
    Progress, RankEntry, RoomConfig, RoomPhase, RoomSnapshot, RoundEnded, RoundStarted, RoundView,
    ServerMsg, Timer, TurnReason,
};
use slint::platform::Key;
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer};
use std::cell::Cell;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SCALE: f32 = 2.0;
const ME: u32 = 1;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn save_png(path: &Path, image: &SharedPixelBuffer<Rgba8Pixel>) -> Result<()> {
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(path)?),
        image.width(),
        image.height(),
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(image.as_bytes())?;
    Ok(())
}

struct Studio {
    headless: Headless,
    app: App,
    out: PathBuf,
    offset: Rc<Cell<Duration>>,
}

impl Studio {
    fn new(out: PathBuf, width: u32, height: u32) -> Result<Self> {
        let headless = Headless::new(width, height, SCALE)?;
        let app = App::with_seed_sequence(1)?;
        app.window().show()?;
        let t0 = Instant::now();
        let offset = Rc::new(Cell::new(Duration::ZERO));
        let o = offset.clone();
        app.online().set_clock(Rc::new(move || t0 + o.get()));
        app.online().set_clipboard(Rc::new(MemoryClipboard::new()));
        headless.settle();
        Ok(Self {
            headless,
            app,
            out,
            offset,
        })
    }

    fn online(&self) -> Online<'_> {
        self.app.window().global::<Online>()
    }

    fn shoot(&self, name: &str) -> Result<()> {
        self.app.online().sync();
        let image = self.headless.snapshot(self.app.window());
        let path = self.out.join(name);
        save_png(&path, &image)?;
        println!("{} ({}×{})", path.display(), image.width(), image.height());
        Ok(())
    }

    fn send(&self, msg: ServerMsg) {
        self.app.online().deliver(msg);
        self.headless.settle();
    }

    /// Let time pass on the scripted clock.
    fn elapse(&self, by: Duration) {
        self.offset.set(self.offset.get() + by);
        self.app.online().sync();
        self.headless.settle();
    }

    /// A fresh online session (as the host, with the embedded server's addresses) in a room.
    fn enter(&self, hosted: bool, config: RoomConfig, players: Vec<PlayerInfo>) {
        let info = hosted.then(|| HostInfo {
            port: 7878,
            local_url: "ws://127.0.0.1:7878/ws".into(),
            lan_urls: vec!["ws://192.168.1.24:7878/ws".into()],
            tunnel_url: Some("wss://quiet-maple-harbor-7f3.trycloudflare.com/ws".into()),
        });
        self.app.online().attach_offline(info);
        self.send(ServerMsg::Welcome {
            player_id: ME,
            resume_token: "0".repeat(32),
            resumed: false,
        });
        self.send(lobby(config, players));
    }

    fn resize(&self, width: u32, height: u32) {
        self.headless.resize(width, height, SCALE);
        self.headless.settle();
    }
}

fn player(id: u32, name: &str, host: bool, connected: bool, score: u32) -> PlayerInfo {
    PlayerInfo {
        id,
        nickname: name.to_owned(),
        host,
        connected,
        score,
    }
}

fn config(mode: Mode, difficulty: Difficulty) -> RoomConfig {
    RoomConfig {
        rounds: 5,
        ..RoomConfig::new(mode, difficulty)
    }
}

fn lobby(config: RoomConfig, players: Vec<PlayerInfo>) -> ServerMsg {
    ServerMsg::RoomSnapshot(RoomSnapshot {
        code: "K7QX2".into(),
        config,
        phase: RoomPhase::Lobby,
        players,
        round: None,
        intermission: None,
    })
}

/// The five hints of a round whose secret is Besançon, with or without distances.
fn hints(with_distances: bool) -> Vec<HintDto> {
    let raw = [
        ("Amiens", 330.5, 403),
        ("Belfort", 48.0, 77),
        ("Bastia", 128.5, 572),
        ("Perpignan", 201.0, 562),
        ("Angers", 275.5, 497),
    ];
    raw.iter()
        .map(|(city, bearing, km)| HintDto {
            city: (*city).to_owned(),
            bearing_deg: *bearing,
            distance_km: with_distances.then_some(*km),
        })
        .collect()
}

fn start(round_index: u8, difficulty: Difficulty, timer: Timer) -> RoundStarted {
    RoundStarted {
        round_index,
        total_rounds: 5,
        max_tries: 6,
        hints: hints(difficulty == Difficulty::Facile),
        timer,
    }
}

fn guess(by: u32, city: &str, km: u32, bearing: f64, correct: bool, tries: u8) -> GuessInfo {
    GuessInfo {
        by,
        city: city.to_owned(),
        distance_km: km,
        bearing_deg: bearing,
        correct,
        tries_used: tries,
    }
}

fn progress(player_id: u32, tries_used: u8, solved: bool, out: bool) -> Progress {
    Progress {
        player_id,
        tries_used,
        solved,
        out_of_tries: out,
    }
}

/// `round_started` and the snapshot of the same step (with the receiver's state).
fn round(
    studio: &Studio,
    config: RoomConfig,
    players: Vec<PlayerInfo>,
    start: RoundStarted,
    guesses: Vec<GuessInfo>,
    progress: Vec<Progress>,
) {
    studio.send(ServerMsg::RoundStarted(start.clone()));
    studio.send(ServerMsg::RoomSnapshot(RoomSnapshot {
        code: "K7QX2".into(),
        config,
        phase: RoomPhase::RoundActive,
        players,
        round: Some(RoundView {
            start,
            guesses,
            progress,
        }),
        intermission: None,
    }));
}

fn five_players(scores: [u32; 5]) -> Vec<PlayerInfo> {
    let names = ["Alice", "Bob", "Camille", "Dominique", "Émilie"];
    names
        .iter()
        .enumerate()
        .map(|(i, n)| player(i as u32 + 1, n, i == 0, true, scores[i]))
        .collect()
}

fn eight_players() -> Vec<PlayerInfo> {
    let names = [
        "Alice",
        "Bob",
        "Camille",
        "Dominique",
        "Émilie",
        "François-Xavier",
        "Gaëlle",
        "Hugo",
    ];
    names
        .iter()
        .enumerate()
        .map(|(i, n)| player(i as u32 + 1, n, i == 0, i != 5, 0))
        .collect()
}

fn entries(list: &[(&str, u32)]) -> Vec<GuessEntry> {
    list.iter()
        .map(|(c, d)| GuessEntry {
            city: (*c).to_owned(),
            distance_km: *d,
        })
        .collect()
}

fn main() -> Result<()> {
    let out = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("docs/screenshots/online"), PathBuf::from);
    std::fs::create_dir_all(&out)?;
    let s = Studio::new(out, 1000, 760)?;

    // 01 · the En ligne screen
    s.online().invoke_open();
    s.headless.settle();
    s.headless.type_text("Alice");
    s.shoot("01-menu.png")?;

    // 02 · the same, joining filled in, and an error from the server
    s.online().set_address("192.168.1.24:7878".into());
    s.online().set_code_input("k7qx2".into());
    s.shoot("02-menu-join-filled.png")?;

    // 03 · tunnel requested but cloudflared is not installed
    s.app
        .online()
        .set_host(Arc::new(FakeHost::failing(HostError::TunnelNotInstalled)));
    s.online().set_host_choice(HostChoice::Tunnel);
    s.online().invoke_host();
    for _ in 0..50 {
        s.headless.tick(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(5));
        if !s.online().get_error().is_empty() {
            break;
        }
    }
    s.headless.settle();
    s.shoot("03-tunnel-not-installed.png")?;
    s.online().set_host_choice(HostChoice::Lan);
    s.online().invoke_back();

    // 04 · the host's lobby: address to share (tunnel + LAN) and the code
    let lobby_config = config(Mode::Course, Difficulty::Facile);
    s.enter(
        true,
        lobby_config,
        vec![
            player(1, "Alice", true, true, 0),
            player(2, "Bob", false, true, 0),
        ],
    );
    s.shoot("04-lobby-host-share.png")?;

    // 05 · the lobby with five players, host view
    s.send(lobby(lobby_config, five_players([0; 5])));
    s.shoot("05-lobby-host-5-players.png")?;

    // 06 · a guest's lobby: settings visible, read-only
    s.enter(
        false,
        config(Mode::Cooperation, Difficulty::Difficile),
        vec![
            player(2, "Bob", true, true, 0),
            player(1, "Alice", false, true, 0),
            player(3, "Camille", false, true, 0),
        ],
    );
    s.shoot("06-lobby-guest.png")?;

    // 07 · the host asked to quit
    s.enter(true, lobby_config, five_players([0; 5]));
    s.online().invoke_leave();
    s.headless.settle();
    s.shoot("07-host-quit-warning.png")?;
    s.online().invoke_cancel_leave();

    // 08 · Course, Facile: live scoreboard
    let course = config(Mode::Course, Difficulty::Facile);
    let players = five_players([620, 450, 540, 300, 0]);
    s.enter(false, course, players.clone());
    round(
        &s,
        course,
        players.clone(),
        start(
            2,
            Difficulty::Facile,
            Timer::Round {
                remaining_ms: 72_000,
            },
        ),
        vec![
            guess(ME, "Lyon", 163, 24.5, false, 1),
            guess(ME, "Paris", 327, 117.0, false, 2),
        ],
        vec![
            progress(1, 2, false, false),
            progress(2, 3, false, false),
            progress(3, 4, true, false),
            progress(4, 1, false, false),
            progress(5, 6, false, true),
        ],
    );
    s.shoot("08-course-facile.png")?;

    // 09 · typing a guess: the suggestions open in the dock, the board stays readable
    s.headless.type_text("sa");
    s.shoot("09-course-autocomplete.png")?;
    s.headless.press_key(Key::Escape);
    s.headless.press_key(Key::Escape);

    // 10 · Course, Difficile
    let hard = config(Mode::Course, Difficulty::Difficile);
    let four: Vec<_> = five_players([0; 5]).into_iter().take(4).collect();
    s.enter(false, hard, four.clone());
    round(
        &s,
        hard,
        four,
        start(
            1,
            Difficulty::Difficile,
            Timer::Round {
                remaining_ms: 103_000,
            },
        ),
        vec![guess(ME, "Dijon", 76, 112.0, false, 1)],
        vec![
            progress(1, 1, false, false),
            progress(2, 0, false, false),
            progress(3, 2, false, false),
            progress(4, 0, false, false),
        ],
    );
    s.shoot("10-course-difficile.png")?;

    // 11 · Coopération, my turn
    let coop = config(Mode::Cooperation, Difficulty::Facile);
    let team = five_players([300, 300, 300, 300, 300]);
    s.enter(false, coop, team.clone());
    round(
        &s,
        coop,
        team.clone(),
        start(
            2,
            Difficulty::Facile,
            Timer::Turn {
                player_id: ME,
                remaining_ms: 31_000,
            },
        ),
        vec![
            guess(2, "Lyon", 215, 201.5, false, 1),
            guess(3, "Dijon", 76, 112.0, false, 2),
        ],
        Vec::new(),
    );
    s.shoot("11-coop-my-turn.png")?;

    // 12 · Coopération, waiting for someone else
    s.send(ServerMsg::TurnChanged {
        player_id: 4,
        remaining_ms: 38_000,
        reason: TurnReason::Guess,
    });
    s.headless.type_text("Lille");
    s.shoot("12-coop-waiting.png")?;

    // 14 · end of a Course round
    s.enter(false, course, players.clone());
    round(
        &s,
        course,
        players.clone(),
        start(
            2,
            Difficulty::Facile,
            Timer::Round {
                remaining_ms: 40_000,
            },
        ),
        vec![
            guess(ME, "Lyon", 163, 24.5, false, 1),
            guess(ME, "Dijon", 76, 112.0, false, 2),
            guess(ME, "Besançon", 0, 0.0, true, 3),
        ],
        vec![
            progress(1, 3, true, false),
            progress(2, 3, false, false),
            progress(3, 4, true, false),
            progress(4, 1, false, false),
            progress(5, 6, false, true),
        ],
    );
    let results = vec![
        PlayerRoundResult {
            player_id: 1,
            guesses: entries(&[("Lyon", 163), ("Dijon", 76), ("Besançon", 0)]),
            solved: true,
            tries_used: 3,
            points: 496,
            total: 1116,
        },
        PlayerRoundResult {
            player_id: 2,
            guesses: entries(&[("Paris", 327), ("Nancy", 154), ("Lille", 480)]),
            solved: false,
            tries_used: 3,
            points: 0,
            total: 450,
        },
        PlayerRoundResult {
            player_id: 3,
            guesses: entries(&[
                ("Lyon", 163),
                ("Dijon", 76),
                ("Belfort", 77),
                ("Besançon", 0),
            ]),
            solved: true,
            tries_used: 4,
            points: 381,
            total: 921,
        },
        PlayerRoundResult {
            player_id: 4,
            guesses: entries(&[("Grenoble", 190)]),
            solved: false,
            tries_used: 1,
            points: 0,
            total: 300,
        },
        PlayerRoundResult {
            player_id: 5,
            guesses: entries(&[
                ("Nice", 480),
                ("Toulouse", 560),
                ("Rennes", 560),
                ("Lille", 480),
                ("Brest", 750),
                ("Pau", 700),
            ]),
            solved: false,
            tries_used: 6,
            points: 0,
            total: 0,
        },
    ];
    s.send(ServerMsg::RoundEnded(RoundEnded {
        round_index: 2,
        total_rounds: 5,
        secret: "Besançon".into(),
        found_by: None,
        results,
        next_round_in_ms: Some(8_000),
    }));
    s.elapse(Duration::from_millis(2_000));
    s.shoot("13-round-result-course.png")?;

    // 15 · the final ranking (Course)
    let mut last = match s.app.online().state().room.and_then(|r| r.result) {
        Some(result) => result.ended,
        None => return Err("no result".into()),
    };
    last.round_index = 5;
    last.next_round_in_ms = None;
    s.send(ServerMsg::RoundEnded(last));
    s.send(ServerMsg::MatchEnded(MatchEnded {
        mode: Mode::Course,
        ranking: vec![
            rank(1, "Alice", 2310, 1, 4),
            rank(3, "Camille", 2310, 1, 4),
            rank(2, "Bob", 1450, 3, 2),
            rank(4, "Dominique", 900, 4, 1),
            rank(5, "Émilie", 120, 5, 0),
        ],
    }));
    s.send(lobby(course, players.clone()));
    s.elapse(Duration::from_millis(9_000));
    s.shoot("14-final-ranking.png")?;

    // 16 · reconnecting
    s.enter(false, course, players.clone());
    round(
        &s,
        course,
        players.clone(),
        start(
            3,
            Difficulty::Facile,
            Timer::Round {
                remaining_ms: 64_000,
            },
        ),
        vec![guess(ME, "Lyon", 163, 24.5, false, 1)],
        vec![
            progress(1, 1, false, false),
            progress(2, 2, false, false),
            progress(3, 0, false, false),
            progress(4, 0, false, false),
            progress(5, 4, false, false),
        ],
    );
    s.app
        .online()
        .deliver_event(NetEvent::Reconnecting { attempt: 3 });
    s.headless.settle();
    s.elapse(Duration::from_secs(9));
    s.shoot("15-reconnecting.png")?;

    // 17 · connection lost for good
    s.app
        .online()
        .deliver_event(NetEvent::Failed(boussole::net::NetError::ConnectionLost));
    s.shoot("16-connection-lost.png")?;

    // 18 · 8 players at the minimum window size: lobby, Course round, Coopération round
    s.resize(800, 600);
    let small = &s;
    small.enter(true, lobby_config, eight_players());
    small.shoot("17-lobby-8-players-800x600.png")?;

    let eight = eight_players();
    small.enter(false, course, eight.clone());
    round(
        small,
        course,
        eight.clone(),
        start(
            2,
            Difficulty::Facile,
            Timer::Round {
                remaining_ms: 58_000,
            },
        ),
        vec![
            guess(ME, "Lyon", 163, 24.5, false, 1),
            guess(ME, "Paris", 327, 117.0, false, 2),
            guess(ME, "Nancy", 154, 190.0, false, 3),
        ],
        eight
            .iter()
            .map(|p| progress(p.id, (p.id % 4) as u8, p.id == 3, p.id == 8))
            .collect(),
    );
    small.shoot("18-course-8-players-800x600.png")?;

    let coop8 = config(Mode::Cooperation, Difficulty::Facile);
    small.enter(false, coop8, eight.clone());
    round(
        small,
        coop8,
        eight.clone(),
        start(
            3,
            Difficulty::Facile,
            Timer::Turn {
                player_id: 4,
                remaining_ms: 22_000,
            },
        ),
        vec![
            guess(2, "Lyon", 215, 201.5, false, 1),
            guess(3, "Dijon", 76, 112.0, false, 2),
            guess(1, "Nancy", 154, 190.0, false, 3),
        ],
        Vec::new(),
    );
    small.shoot("19-coop-8-players-800x600.png")?;

    // 21 · the menu at the minimum size
    small.online().invoke_back_to_menu();
    small.headless.settle();
    small.shoot("20-menu-800x600.png")?;
    Ok(())
}

fn rank(id: u32, name: &str, total: u32, rank: u32, solved: u32) -> RankEntry {
    RankEntry {
        player_id: id,
        nickname: name.to_owned(),
        total,
        rank,
        solved_rounds: solved,
    }
}
