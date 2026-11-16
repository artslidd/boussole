//! Test harness: a fake transport around the engine with injected time and randomness.
#![allow(dead_code)]

use boussole_core::cities::{CITIES, City};
use boussole_protocol::*;
use boussole_server::engine::{ConnId, Engine, Entropy, Millis, Outbound, SeededEntropy};
use std::collections::{BTreeMap, BTreeSet};

pub struct Sim {
    pub engine: Engine,
    pub now: Millis,
    /// Everything each connection has received, in order, not yet taken.
    pub inbox: BTreeMap<ConnId, Vec<ServerMsg>>,
    /// Everything ever sent, in order, with the time of the step.
    pub log: Vec<(Millis, Outbound)>,
    pub closed: BTreeSet<ConnId>,
}

#[derive(Debug, Clone)]
pub struct Client {
    pub conn: ConnId,
    pub id: PlayerId,
    pub token: String,
}

impl Sim {
    pub fn new(seed: u64) -> Self {
        Self::with_entropy(Box::new(SeededEntropy::new(seed)))
    }

    pub fn with_entropy(entropy: Box<dyn Entropy>) -> Self {
        Self {
            engine: Engine::new(entropy),
            now: 1_000,
            inbox: BTreeMap::new(),
            log: Vec::new(),
            closed: BTreeSet::new(),
        }
    }

    /// Record outputs; closing a connection also tells the engine, like a real transport.
    pub fn deliver(&mut self, outs: Vec<Outbound>) -> Vec<Outbound> {
        let mut all = outs.clone();
        let mut pending = outs;
        while !pending.is_empty() {
            let mut closes = Vec::new();
            for o in pending {
                self.inbox.entry(o.to).or_default().push(o.msg.clone());
                self.log.push((self.now, o.clone()));
                if o.close && self.closed.insert(o.to) {
                    closes.push(o.to);
                }
            }
            pending = Vec::new();
            for c in closes {
                let more = self.engine.disconnect(self.now, c);
                all.extend(more.clone());
                pending.extend(more);
            }
        }
        all
    }

    pub fn send(&mut self, conn: ConnId, msg: ClientMsg) -> Vec<Outbound> {
        let outs = self.engine.handle(self.now, conn, msg);
        self.deliver(outs)
    }

    pub fn frame(&mut self, conn: ConnId, frame: &str) -> Vec<Outbound> {
        let outs = self.engine.handle_frame(self.now, conn, frame);
        self.deliver(outs)
    }

    pub fn advance(&mut self, ms: Millis) -> Vec<Outbound> {
        self.now += ms;
        let outs = self.engine.tick(self.now);
        self.deliver(outs)
    }

    pub fn disconnect(&mut self, conn: ConnId) -> Vec<Outbound> {
        let outs = self.engine.disconnect(self.now, conn);
        self.deliver(outs)
    }

    /// Messages received by `conn` since the last call.
    pub fn take(&mut self, conn: ConnId) -> Vec<ServerMsg> {
        self.inbox.remove(&conn).unwrap_or_default()
    }

    pub fn drain_all(&mut self) {
        self.inbox.clear();
    }

    pub fn check(&self) {
        if let Err(e) = self.engine.check_invariants() {
            panic!("engine invariant violated: {e}");
        }
    }

    // ---- scenario helpers -------------------------------------------------------------------

    pub fn hello(&mut self, conn: ConnId, nickname: &str) -> Client {
        self.send(conn, ClientMsg::hello(nickname, None));
        let welcome = self
            .take(conn)
            .into_iter()
            .find_map(|m| match m {
                ServerMsg::Welcome {
                    player_id,
                    resume_token,
                    resumed: false,
                } => Some((player_id, resume_token)),
                _ => None,
            })
            .expect("welcome");
        Client {
            conn,
            id: welcome.0,
            token: welcome.1,
        }
    }

    pub fn create(&mut self, c: &Client, config: RoomConfig) -> String {
        self.send(c.conn, ClientMsg::CreateRoom { config });
        snapshot(&self.take(c.conn))
            .expect("snapshot after create")
            .code
    }

    pub fn join(&mut self, c: &Client, code: &str) -> Vec<ServerMsg> {
        self.send(c.conn, ClientMsg::JoinRoom { code: code.into() });
        self.take(c.conn)
    }

    /// `n` players (conns 1..=n, nicknames P1..Pn); the first creates the room, the rest join.
    pub fn lobby(&mut self, n: usize, config: RoomConfig) -> (Vec<Client>, String) {
        let clients: Vec<Client> = (1..=n)
            .map(|i| self.hello(i as ConnId, &format!("P{i}")))
            .collect();
        let code = self.create(&clients[0], config);
        for c in &clients[1..] {
            let reply = self.join(c, &code);
            assert!(errors(&reply).is_empty(), "join failed: {reply:?}");
        }
        self.drain_all();
        (clients, code)
    }

    /// Like [`Sim::lobby`] and then the host starts the match; inboxes are cleared.
    pub fn started(&mut self, n: usize, config: RoomConfig) -> (Vec<Client>, String) {
        let (clients, code) = self.lobby(n, config);
        self.send(clients[0].conn, ClientMsg::StartMatch);
        self.drain_all();
        (clients, code)
    }

    pub fn guess(&mut self, c: &Client, city: &str) -> Vec<Outbound> {
        self.send(c.conn, ClientMsg::Guess { city: city.into() })
    }

    /// A Flèches `answer { option }`.
    pub fn answer(&mut self, c: &Client, option: u8) -> Vec<Outbound> {
        self.send(c.conn, ClientMsg::Answer { option })
    }

    pub fn secret(&self, code: &str) -> &'static str {
        self.engine.debug_secret(code).expect("a round is active")
    }

    /// The Flèches question in progress (tests only: the correct index and distance must
    /// never appear on the wire before `round_ended`).
    pub fn fleches(&self, code: &str) -> boussole_core::arrows::ArrowQuestion {
        self.engine
            .debug_fleches(code)
            .expect("a fleches round is active")
    }
}

pub fn cfg(mode: Mode, difficulty: Difficulty) -> RoomConfig {
    RoomConfig::new(mode, difficulty)
}

pub fn course(rounds: u32) -> RoomConfig {
    RoomConfig {
        rounds,
        round_time_s: 60,
        ..cfg(Mode::Course, Difficulty::Facile)
    }
}

pub fn coop(rounds: u32) -> RoomConfig {
    RoomConfig {
        rounds,
        turn_time_s: 30,
        ..cfg(Mode::Cooperation, Difficulty::Facile)
    }
}

/// A Flèches Course room: `question_time_s` defaults to 15 s.
pub fn fleches_course(rounds: u32) -> RoomConfig {
    RoomConfig {
        game: GameKind::Fleches,
        rounds,
        ..cfg(Mode::Course, Difficulty::Facile)
    }
}

/// A Flèches Coopération room.
pub fn fleches_coop(rounds: u32) -> RoomConfig {
    RoomConfig {
        game: GameKind::Fleches,
        rounds,
        turn_time_s: 30,
        ..cfg(Mode::Cooperation, Difficulty::Facile)
    }
}

/// `n` city names that are not `secret`.
pub fn wrong_cities(secret: &str, n: usize) -> Vec<&'static str> {
    CITIES
        .iter()
        .map(|c: &City| c.name)
        .filter(|name| *name != secret)
        .take(n)
        .collect()
}

pub fn errors(msgs: &[ServerMsg]) -> Vec<ErrorCode> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::Error { code, .. } => Some(*code),
            _ => None,
        })
        .collect()
}

pub fn snapshot(msgs: &[ServerMsg]) -> Option<RoomSnapshot> {
    msgs.iter().rev().find_map(|m| match m {
        ServerMsg::RoomSnapshot(s) => Some(s.clone()),
        _ => None,
    })
}

/// Messages of one kind, as `(index, message)`.
pub fn find<'a, T>(msgs: &'a [ServerMsg], pick: impl Fn(&'a ServerMsg) -> Option<T>) -> Vec<T> {
    msgs.iter().filter_map(pick).collect()
}

pub fn round_ended(msgs: &[ServerMsg]) -> Option<RoundEnded> {
    msgs.iter().find_map(|m| match m {
        ServerMsg::RoundEnded(r) => Some(r.clone()),
        _ => None,
    })
}

pub fn match_ended(msgs: &[ServerMsg]) -> Option<MatchEnded> {
    msgs.iter().find_map(|m| match m {
        ServerMsg::MatchEnded(r) => Some(r.clone()),
        _ => None,
    })
}

pub fn round_started(msgs: &[ServerMsg]) -> Option<RoundStarted> {
    msgs.iter().find_map(|m| match m {
        ServerMsg::RoundStarted(r) => Some(r.clone()),
        _ => None,
    })
}

pub fn turn_changes(msgs: &[ServerMsg]) -> Vec<(PlayerId, u64, TurnReason)> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::TurnChanged {
                player_id,
                remaining_ms,
                reason,
            } => Some((*player_id, *remaining_ms, *reason)),
            _ => None,
        })
        .collect()
}

pub fn guess_results(msgs: &[ServerMsg]) -> Vec<GuessInfo> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::GuessResult(g) => Some(g.clone()),
            _ => None,
        })
        .collect()
}

pub fn progress(msgs: &[ServerMsg]) -> Vec<Progress> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::PlayerProgress(p) => Some(*p),
            _ => None,
        })
        .collect()
}

/// Messages `conn` received in `outs`, in order.
pub fn for_conn(outs: &[Outbound], conn: ConnId) -> Vec<ServerMsg> {
    outs.iter()
        .filter(|o| o.to == conn)
        .map(|o| o.msg.clone())
        .collect()
}

/// Entropy that plays back a script, then continues pseudo-randomly.
pub struct Scripted {
    script: std::collections::VecDeque<u64>,
    rest: SeededEntropy,
}

impl Scripted {
    pub fn new(script: Vec<u64>) -> Self {
        Self {
            script: script.into(),
            rest: SeededEntropy::new(99),
        }
    }
}

impl Entropy for Scripted {
    fn next_u64(&mut self) -> u64 {
        self.script
            .pop_front()
            .unwrap_or_else(|| self.rest.next_u64())
    }
}
