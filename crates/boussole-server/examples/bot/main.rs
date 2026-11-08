//! A scripted player for manual LAN checks and load tests.
//!
//! ```text
//! cargo run --release -p boussole-server --example bot -- \
//!     --url ws://192.168.1.20:7878/ws --create --mode course --players 3 --nickname Ada
//! cargo run --release -p boussole-server --example bot -- \
//!     --url ws://192.168.1.20:7878/ws --room K7QX2 --nickname Bob
//! ```
//!
//! The creating bot prints `ROOM <code>` and starts the match once `--players` are in the
//! room; every bot plays until `match_ended`, then exits 0 (exit 1 on any failure).

mod strategy;

use boussole_protocol::{
    ClientMsg, Difficulty, ErrorCode, MAX_TRIES, Mode, RoomConfig, ServerMsg, Timer, decode_server,
    encode_client,
};
use futures_util::{SinkExt, StreamExt};
use std::process::ExitCode;
use std::time::Duration;
use strategy::Strategy;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const USAGE: &str = "\
usage: bot --url <ws://host:port/ws> (--create | --room <CODE>) [options]

  --url <url>          server WebSocket URL (default ws://127.0.0.1:7878/ws)
  --create             create the room (and start the match once enough players joined)
  --room <code>        join this room
  --nickname <name>    default Bot<pid>
  --mode <m>           course | coop (create only, default course)
  --difficulty <d>     facile | difficile (create only, default facile)
  --rounds <n>         1-10 (create only, default 2)
  --players <n>        start when n players are in the room (create only, default 2)
  --think-ms <n>       pause before each guess (default 300)
  --seed <n>           random seed (default: from the nickname)";

struct Args {
    url: String,
    create: bool,
    room: Option<String>,
    nickname: String,
    mode: Mode,
    difficulty: Difficulty,
    rounds: u32,
    players: usize,
    think: Duration,
    seed: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        url: "ws://127.0.0.1:7878/ws".into(),
        create: false,
        room: None,
        nickname: format!("Bot{}", std::process::id() % 1000),
        mode: Mode::Course,
        difficulty: Difficulty::Facile,
        rounds: 2,
        players: 2,
        think: Duration::from_millis(300),
        seed: 0,
    };
    let mut seed = None;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--url" => a.url = value("--url")?,
            "--create" => a.create = true,
            "--room" => a.room = Some(value("--room")?),
            "--nickname" => a.nickname = value("--nickname")?,
            "--mode" => {
                a.mode = match value("--mode")?.as_str() {
                    "course" => Mode::Course,
                    "coop" | "cooperation" => Mode::Cooperation,
                    other => return Err(format!("unknown mode {other:?}")),
                }
            }
            "--difficulty" => {
                a.difficulty = match value("--difficulty")?.as_str() {
                    "facile" => Difficulty::Facile,
                    "difficile" => Difficulty::Difficile,
                    other => return Err(format!("unknown difficulty {other:?}")),
                }
            }
            "--rounds" => {
                a.rounds = value("--rounds")?
                    .parse()
                    .map_err(|e| format!("--rounds: {e}"))?
            }
            "--players" => {
                a.players = value("--players")?
                    .parse()
                    .map_err(|e| format!("--players: {e}"))?
            }
            "--think-ms" => {
                a.think = Duration::from_millis(
                    value("--think-ms")?
                        .parse()
                        .map_err(|e| format!("--think-ms: {e}"))?,
                )
            }
            "--seed" => {
                seed = Some(
                    value("--seed")?
                        .parse()
                        .map_err(|e| format!("--seed: {e}"))?,
                )
            }
            "--help" | "-h" => return Err(String::new()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if a.create == a.room.is_some() {
        return Err("give exactly one of --create and --room <code>".into());
    }
    a.seed = seed.unwrap_or_else(|| {
        a.nickname.bytes().fold(0x0B05_501Eu64, |h, b| {
            h.wrapping_mul(31).wrapping_add(u64::from(b))
        })
    });
    Ok(a)
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("{msg}\n");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match play(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bot failed: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn play(args: Args) -> Result<(), String> {
    let tag = format!("[{}]", args.nickname);
    let (mut ws, _) = connect_async(&args.url)
        .await
        .map_err(|e| format!("cannot connect to {}: {e}", args.url))?;
    let send = async |ws: &mut tokio_tungstenite::WebSocketStream<_>, msg: ClientMsg| {
        ws.send(Message::text(encode_client(&msg)))
            .await
            .map_err(|e| format!("send: {e}"))
    };
    send(&mut ws, ClientMsg::hello(args.nickname.clone(), None)).await?;

    let mut me = 0;
    let mut strat = Strategy::new(args.seed);
    let mut joined = false;
    let mut started = false;
    // Course or Coopération is the room's setting: learned from the first round_started.
    let mut course = args.mode == Mode::Course;
    let think = args.think;

    while let Some(frame) = ws.next().await {
        let text = match frame.map_err(|e| format!("read: {e}"))? {
            Message::Text(t) => t,
            Message::Close(f) => return Err(format!("server closed the connection: {f:?}")),
            _ => continue,
        };
        let msg = decode_server(&text).map_err(|e| format!("undecodable server frame: {e}"))?;
        match msg {
            ServerMsg::Welcome { player_id, .. } => {
                me = player_id;
                println!("{tag} connected as player {me}");
                let request = if args.create {
                    ClientMsg::CreateRoom {
                        config: RoomConfig {
                            rounds: args.rounds,
                            ..RoomConfig::new(args.mode, args.difficulty)
                        },
                    }
                } else {
                    ClientMsg::JoinRoom {
                        code: args.room.clone().unwrap_or_default(),
                    }
                };
                send(&mut ws, request).await?;
            }
            ServerMsg::RoomSnapshot(s) => {
                if !joined {
                    joined = true;
                    println!("{tag} in room {}", s.code);
                    if args.create {
                        println!("ROOM {}", s.code);
                    }
                }
                let ready = s.players.iter().filter(|p| p.connected).count() >= args.players;
                if args.create
                    && ready
                    && !started
                    && s.phase == boussole_protocol::RoomPhase::Lobby
                {
                    started = true;
                    println!(
                        "{tag} {} players in the room, starting the match",
                        args.players
                    );
                    send(&mut ws, ClientMsg::StartMatch).await?;
                }
            }
            ServerMsg::RoundStarted(r) => {
                println!(
                    "{tag} round {}/{} started ({} hints)",
                    r.round_index,
                    r.total_rounds,
                    r.hints.len()
                );
                strat.reset();
                course = matches!(r.timer, Timer::Round { .. });
                let my_turn = match r.timer {
                    Timer::Round { .. } => true,
                    Timer::Turn { player_id, .. } => player_id == me,
                };
                if my_turn {
                    tokio::time::sleep(think).await;
                    send(
                        &mut ws,
                        ClientMsg::Guess {
                            city: strat.next_guess().into(),
                        },
                    )
                    .await?;
                }
            }
            ServerMsg::GuessResult(g) => {
                println!(
                    "{tag} player {} guessed {} -> {} km, {:.1}°{}",
                    g.by,
                    g.city,
                    g.distance_km,
                    g.bearing_deg,
                    if g.correct { " (correct)" } else { "" }
                );
                strat.observe(&g);
                if g.by == me && course {
                    let done = g.correct || g.tries_used >= MAX_TRIES;
                    if !done {
                        tokio::time::sleep(think).await;
                        send(
                            &mut ws,
                            ClientMsg::Guess {
                                city: strat.next_guess().into(),
                            },
                        )
                        .await?;
                    }
                }
            }
            ServerMsg::TurnChanged { player_id, .. } if player_id == me => {
                tokio::time::sleep(think).await;
                send(
                    &mut ws,
                    ClientMsg::Guess {
                        city: strat.next_guess().into(),
                    },
                )
                .await?;
            }
            ServerMsg::RoundEnded(e) => {
                let mine = e.results.iter().find(|r| r.player_id == me);
                println!(
                    "{tag} round {}/{} ended: secret {}, my points {}, my total {}",
                    e.round_index,
                    e.total_rounds,
                    e.secret,
                    mine.map_or(0, |r| r.points),
                    mine.map_or(0, |r| r.total)
                );
            }
            ServerMsg::MatchEnded(m) => {
                let ranking: Vec<String> = m
                    .ranking
                    .iter()
                    .map(|r| format!("{}. {} {}", r.rank, r.nickname, r.total))
                    .collect();
                println!("{tag} MATCH ENDED: {}", ranking.join(" | "));
                // Leave politely so the room is freed at once instead of held for 60 s.
                send(&mut ws, ClientMsg::LeaveRoom).await?;
                let _ = ws.close(None).await;
                return Ok(());
            }
            ServerMsg::Error { code, message } => match code {
                // Races with timers or teammates are part of the game.
                ErrorCode::NotYourTurn
                | ErrorCode::AlreadyGuessed
                | ErrorCode::RoundOver
                | ErrorCode::UnknownCity => {
                    eprintln!("{tag} refused ({code:?}): {message}");
                }
                _ => return Err(format!("server error {code:?}: {message}")),
            },
            ServerMsg::PlayerProgress(_) | ServerMsg::TurnChanged { .. } | ServerMsg::Pong => {}
        }
    }
    Err("connection closed before the match ended".into())
}
