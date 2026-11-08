//! The pure match engine: rooms, handshake, Course and Coopération rules, timers, scoring and
//! session resumption. No IO, no clocks, no threads: time and randomness are injected.
//!
//! # Driving the engine
//!
//! The transport owns sockets and gives each one a unique [`ConnId`] (never reused). It calls
//!
//! * [`Engine::handle_frame`] (or [`Engine::handle`]) for every received text frame,
//! * [`Engine::disconnect`] when a socket closes for any reason,
//! * [`Engine::tick`] whenever [`Engine::next_deadline`] is due (or just regularly),
//!
//! always with a monotonic `now` in milliseconds, and sends every returned [`Outbound`] to
//! its connection (closing it afterwards when `close` is set). Every entry point first fires
//! the timers that are due at `now`, in chronological order, so the outcome never depends on
//! how often `tick` is called.
//!
//! Each [`Outbound`] is already resolved to one connection; the engine knows which connection
//! currently carries which player, so a resumed player simply receives on their new socket.

mod room;
mod round;
mod score;

pub use score::{competition_ranks, coop_points, course_points};

use boussole_core::search;
use boussole_protocol::{
    ClientMsg, ErrorCode, GuessInfo, Intermission, MAX_FRAME_BYTES, Mode, PlayerId, PlayerInfo,
    RESUME_GRACE_MS, ROOM_CODE_ALPHABET, ROOM_CODE_LEN, ROOM_IDLE_MS, RoomConfig, RoomPhase,
    RoomSnapshot, ServerMsg, decode_client, nickname_key, normalize_nickname, normalize_room_code,
};
use room::{Kind, Phase, Player, Room, Score};
use std::collections::{BTreeMap, BTreeSet};

/// Monotonic milliseconds, supplied by the caller.
pub type Millis = u64;
/// Transport-level connection handle, allocated by the caller (unique, never reused).
pub type ConnId = u64;

/// Source of randomness: secrets' seeds, room codes and resume tokens.
///
/// In production this **must be a cryptographically secure generator** (resume tokens are
/// bearer credentials and round seeds must not be predictable). Tests use [`SeededEntropy`].
pub trait Entropy: Send {
    fn next_u64(&mut self) -> u64;
}

/// Deterministic SplitMix64 stream for tests and simulations. **Not secure.**
#[derive(Debug, Clone)]
pub struct SeededEntropy(u64);

impl SeededEntropy {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
}

impl Entropy for SeededEntropy {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// A message for one connection.
#[derive(Debug, Clone, PartialEq)]
pub struct Outbound {
    pub to: ConnId,
    pub msg: ServerMsg,
    /// The transport must close the connection after sending `msg`.
    pub close: bool,
}

/// What can become due, in the order they are handled when simultaneous.
enum Due {
    Room(String),
    Grace(PlayerId),
    Idle(String),
}

impl Due {
    fn rank(&self) -> u8 {
        match self {
            Due::Room(_) => 0,
            Due::Grace(_) => 1,
            Due::Idle(_) => 2,
        }
    }
}

/// The match engine. See the [module documentation](self).
pub struct Engine {
    entropy: Box<dyn Entropy>,
    fingerprint: u64,
    clock: Millis,
    next_player_id: PlayerId,
    players: BTreeMap<PlayerId, Player>,
    /// Every known connection; `None` until it says `hello`.
    conns: BTreeMap<ConnId, Option<PlayerId>>,
    rooms: BTreeMap<String, Room>,
    /// Rooms whose snapshot must be re-sent at the end of the current step.
    dirty: BTreeSet<String>,
}

impl Engine {
    pub fn new(entropy: Box<dyn Entropy>) -> Self {
        Self {
            entropy,
            fingerprint: boussole_core::dataset_fingerprint(),
            clock: 0,
            next_player_id: 1,
            players: BTreeMap::new(),
            conns: BTreeMap::new(),
            rooms: BTreeMap::new(),
            dirty: BTreeSet::new(),
        }
    }

    /// Handle one received text frame: size check, decoding (`bad_message` on failure,
    /// and `close` when oversized) and [`Engine::handle`].
    pub fn handle_frame(&mut self, now: Millis, conn: ConnId, frame: &str) -> Vec<Outbound> {
        if frame.len() > MAX_FRAME_BYTES {
            let mut out = self.tick(now);
            out.push(Outbound {
                to: conn,
                msg: ServerMsg::error(ErrorCode::BadMessage),
                close: true,
            });
            return out;
        }
        match decode_client(frame) {
            Ok(msg) => self.handle(now, conn, msg),
            Err(err) => {
                let mut out = self.tick(now);
                out.push(reply(conn, ServerMsg::error(err.code())));
                out
            }
        }
    }

    /// Handle one decoded message from `conn`.
    pub fn handle(&mut self, now: Millis, conn: ConnId, msg: ClientMsg) -> Vec<Outbound> {
        let mut out = Vec::new();
        let now = self.advance(now, &mut out);
        self.conns.entry(conn).or_insert(None);
        self.dispatch(now, conn, msg, &mut out);
        self.flush(now, &mut out);
        out
    }

    /// The socket `conn` is gone. The player keeps their slot for [`RESUME_GRACE_MS`].
    pub fn disconnect(&mut self, now: Millis, conn: ConnId) -> Vec<Outbound> {
        let mut out = Vec::new();
        let now = self.advance(now, &mut out);
        if let Some(Some(pid)) = self.conns.remove(&conn) {
            self.on_player_disconnected(now, pid, &mut out);
        }
        self.flush(now, &mut out);
        out
    }

    /// Fire every timer due at `now` (round and turn limits, intermissions, resume grace,
    /// idle rooms).
    pub fn tick(&mut self, now: Millis) -> Vec<Outbound> {
        let mut out = Vec::new();
        let now = self.advance(now, &mut out);
        self.flush(now, &mut out);
        out
    }

    /// When [`Engine::tick`] next has something to do, if ever.
    pub fn next_deadline(&self) -> Option<Millis> {
        self.next_due().map(|(t, _)| t)
    }

    /// Live rooms.
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// Identified players (connected, or holding a slot during the grace period).
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    // ---- time -----------------------------------------------------------------------------

    fn phase_deadline(room: &Room) -> Option<Millis> {
        match &room.phase {
            Phase::Lobby => None,
            Phase::Intermission(p) => Some(p.until),
            Phase::Active(a) => Some(match &a.kind {
                Kind::Course(c) => c.deadline,
                Kind::Coop(c) => c.turn_deadline,
            }),
        }
    }

    fn next_due(&self) -> Option<(Millis, Due)> {
        let mut best: Option<(Millis, Due)> = None;
        let mut consider = |t: Millis, due: Due| {
            if best
                .as_ref()
                .is_none_or(|(bt, bd)| (t, due.rank()) < (*bt, bd.rank()))
            {
                best = Some((t, due));
            }
        };
        // shortcut: linear scan of every room and player per timer, fine up to a few
        // thousand rooms; upgrade to a deadline heap if the server ever hosts more.
        for (code, room) in &self.rooms {
            if let Some(t) = Self::phase_deadline(room) {
                consider(t, Due::Room(code.clone()));
            }
            consider(room.last_activity + ROOM_IDLE_MS, Due::Idle(code.clone()));
        }
        for (id, p) in &self.players {
            if let Some(at) = p.disconnected_at {
                consider(at + RESUME_GRACE_MS, Due::Grace(*id));
            }
        }
        best
    }

    /// Fire everything due at or before `now`, oldest first, flushing snapshots after each so
    /// the output is chronological. Returns the (monotonic) time to use for the rest of the
    /// step.
    fn advance(&mut self, now: Millis, out: &mut Vec<Outbound>) -> Millis {
        let now = now.max(self.clock);
        // The bound only protects against a bug that would stop a deadline from moving.
        for _ in 0..1_000_000 {
            let Some((t, due)) = self.next_due() else {
                break;
            };
            if t > now {
                break;
            }
            self.clock = self.clock.max(t);
            match due {
                Due::Room(code) => self.on_room_deadline(&code, t, out),
                Due::Grace(pid) => self.on_grace_expired(pid, t, out),
                Due::Idle(code) => self.close_idle_room(&code, out),
            }
            self.flush(t, out);
        }
        self.clock = now;
        now
    }

    // ---- dispatch ---------------------------------------------------------------------------

    fn dispatch(&mut self, now: Millis, conn: ConnId, msg: ClientMsg, out: &mut Vec<Outbound>) {
        let pid = self.conns.get(&conn).copied().flatten();
        self.dispatch_inner(now, conn, pid, msg, out);
        // Any message from a member (even a ping or a refused one) keeps their room alive.
        if let Some(pid) = pid.filter(|p| self.players.contains_key(p))
            && let Some(room) = self.player_room_mut(pid)
        {
            room.last_activity = now;
        }
    }

    fn dispatch_inner(
        &mut self,
        now: Millis,
        conn: ConnId,
        pid: Option<PlayerId>,
        msg: ClientMsg,
        out: &mut Vec<Outbound>,
    ) {
        match msg {
            ClientMsg::Ping => out.push(reply(conn, ServerMsg::Pong)),
            ClientMsg::Hello {
                protocol_version,
                dataset_fingerprint,
                nickname,
                resume_token,
            } => {
                if pid.is_some() {
                    out.push(error(conn, ErrorCode::InvalidState));
                } else {
                    self.on_hello(
                        conn,
                        protocol_version,
                        dataset_fingerprint,
                        &nickname,
                        resume_token.as_deref(),
                        out,
                    );
                }
            }
            other => {
                let Some(pid) = pid else {
                    // Handshake first: anything else on a fresh connection ends it.
                    out.push(Outbound {
                        to: conn,
                        msg: ServerMsg::error(ErrorCode::InvalidState),
                        close: true,
                    });
                    return;
                };
                let result = match other {
                    ClientMsg::CreateRoom { config } => self.on_create_room(now, pid, config),
                    ClientMsg::JoinRoom { code } => self.on_join_room(pid, &code),
                    ClientMsg::LeaveRoom => self.on_leave_room(now, pid, out),
                    ClientMsg::UpdateConfig { config } => self.on_update_config(pid, config),
                    ClientMsg::StartMatch => self.on_start_match(now, pid, out),
                    ClientMsg::Guess { city } => self.on_guess(now, pid, &city, out),
                    ClientMsg::Hello { .. } | ClientMsg::Ping => Ok(()),
                };
                if let Err(code) = result {
                    out.push(error(conn, code));
                }
            }
        }
    }

    // ---- handshake --------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn on_hello(
        &mut self,
        conn: ConnId,
        protocol_version: u16,
        fingerprint: u64,
        nickname: &str,
        resume_token: Option<&str>,
        out: &mut Vec<Outbound>,
    ) {
        let refuse = |code| Outbound {
            to: conn,
            msg: ServerMsg::error(code),
            close: true,
        };
        if protocol_version != boussole_protocol::PROTOCOL_VERSION {
            out.push(refuse(ErrorCode::BadVersion));
            return;
        }
        if fingerprint != self.fingerprint {
            out.push(refuse(ErrorCode::DatasetMismatch));
            return;
        }
        // A valid token resumes the old identity (the nickname in this hello is ignored).
        let resumed = resume_token.and_then(|t| {
            self.players
                .iter()
                .find(|(_, p)| p.token == t)
                .map(|(id, _)| *id)
        });
        if let Some(pid) = resumed {
            let player = self.players.get_mut(&pid).expect("found above");
            if let Some(old) = player.conn.replace(conn) {
                // Half-open socket still registered: the new connection takes over.
                self.conns.remove(&old);
                out.push(Outbound {
                    to: old,
                    msg: ServerMsg::error(ErrorCode::InvalidState),
                    close: true,
                });
            }
            player.disconnected_at = None;
            let token = player.token.clone();
            let room = player.room.clone();
            self.conns.insert(conn, Some(pid));
            out.push(reply(
                conn,
                ServerMsg::Welcome {
                    player_id: pid,
                    resume_token: token,
                    resumed: true,
                },
            ));
            if let Some(code) = room {
                self.dirty.insert(code);
            }
            return;
        }
        let Some(nickname) = normalize_nickname(nickname) else {
            // The connection stays open and may send another hello.
            out.push(error(conn, ErrorCode::InvalidNickname));
            return;
        };
        let pid = self.next_player_id;
        self.next_player_id += 1;
        let token = format!(
            "{:016x}{:016x}",
            self.entropy.next_u64(),
            self.entropy.next_u64()
        );
        self.players.insert(
            pid,
            Player {
                nickname,
                token: token.clone(),
                conn: Some(conn),
                room: None,
                disconnected_at: None,
            },
        );
        self.conns.insert(conn, Some(pid));
        out.push(reply(
            conn,
            ServerMsg::Welcome {
                player_id: pid,
                resume_token: token,
                resumed: false,
            },
        ));
    }

    // ---- lobby ------------------------------------------------------------------------------

    fn player_room_mut(&mut self, pid: PlayerId) -> Option<&mut Room> {
        let code = self.players.get(&pid)?.room.clone()?;
        self.rooms.get_mut(&code)
    }

    fn new_room_code(&mut self) -> String {
        let alphabet = ROOM_CODE_ALPHABET.as_bytes();
        loop {
            let code: String = (0..ROOM_CODE_LEN)
                .map(|_| {
                    alphabet[(self.entropy.next_u64() % alphabet.len() as u64) as usize] as char
                })
                .collect();
            if !self.rooms.contains_key(&code) {
                return code;
            }
        }
    }

    fn on_create_room(
        &mut self,
        now: Millis,
        pid: PlayerId,
        config: RoomConfig,
    ) -> Result<(), ErrorCode> {
        if self.players[&pid].room.is_some() {
            return Err(ErrorCode::InvalidState);
        }
        config.validate()?;
        let code = self.new_room_code();
        self.rooms.insert(
            code.clone(),
            Room {
                code: code.clone(),
                config,
                host: pid,
                members: vec![pid],
                phase: Phase::Lobby,
                scores: BTreeMap::from([(pid, Score::default())]),
                team_total: 0,
                round_index: 0,
                last_secret: None,
                last_activity: now,
            },
        );
        self.players.get_mut(&pid).expect("known player").room = Some(code.clone());
        self.dirty.insert(code);
        Ok(())
    }

    fn on_join_room(&mut self, pid: PlayerId, code: &str) -> Result<(), ErrorCode> {
        if self.players[&pid].room.is_some() {
            return Err(ErrorCode::InvalidState);
        }
        let code = normalize_room_code(code).ok_or(ErrorCode::RoomNotFound)?;
        let room = self.rooms.get(&code).ok_or(ErrorCode::RoomNotFound)?;
        if !matches!(room.phase, Phase::Lobby) {
            return Err(ErrorCode::MatchInProgress);
        }
        if room.members.len() >= room.config.max_players as usize {
            return Err(ErrorCode::RoomFull);
        }
        let key = nickname_key(&self.players[&pid].nickname);
        if room
            .members
            .iter()
            .any(|m| nickname_key(&self.players[m].nickname) == key)
        {
            return Err(ErrorCode::NicknameTaken);
        }
        let room = self.rooms.get_mut(&code).expect("checked above");
        room.members.push(pid);
        room.scores.insert(pid, Score::default());
        self.players.get_mut(&pid).expect("known player").room = Some(code.clone());
        self.dirty.insert(code);
        Ok(())
    }

    fn on_leave_room(
        &mut self,
        now: Millis,
        pid: PlayerId,
        out: &mut Vec<Outbound>,
    ) -> Result<(), ErrorCode> {
        let code = self.players[&pid]
            .room
            .clone()
            .ok_or(ErrorCode::InvalidState)?;
        self.players.get_mut(&pid).expect("known player").room = None;
        self.remove_member(&code, pid, now, out);
        Ok(())
    }

    fn on_update_config(&mut self, pid: PlayerId, config: RoomConfig) -> Result<(), ErrorCode> {
        let room = self.player_room_mut(pid).ok_or(ErrorCode::InvalidState)?;
        if room.host != pid {
            return Err(ErrorCode::NotHost);
        }
        if !matches!(room.phase, Phase::Lobby) {
            return Err(ErrorCode::InvalidState);
        }
        config.validate()?;
        if (config.max_players as usize) < room.members.len() {
            return Err(ErrorCode::InvalidConfig);
        }
        room.config = config;
        let code = room.code.clone();
        self.dirty.insert(code);
        Ok(())
    }

    fn on_start_match(
        &mut self,
        now: Millis,
        pid: PlayerId,
        out: &mut Vec<Outbound>,
    ) -> Result<(), ErrorCode> {
        let room = self.player_room_mut(pid).ok_or(ErrorCode::InvalidState)?;
        if room.host != pid {
            return Err(ErrorCode::NotHost);
        }
        if !matches!(room.phase, Phase::Lobby) {
            return Err(ErrorCode::InvalidState);
        }
        let code = room.code.clone();
        if self.connected_members(&code).len() < 2 {
            return Err(ErrorCode::NotEnoughPlayers);
        }
        let room = self.rooms.get_mut(&code).expect("exists");
        room.scores = room
            .members
            .iter()
            .map(|m| (*m, Score::default()))
            .collect();
        room.team_total = 0;
        room.round_index = 0;
        room.last_secret = None;
        self.start_round(&code, now, out);
        Ok(())
    }

    fn on_guess(
        &mut self,
        now: Millis,
        pid: PlayerId,
        city: &str,
        out: &mut Vec<Outbound>,
    ) -> Result<(), ErrorCode> {
        let code = self.players[&pid]
            .room
            .clone()
            .ok_or(ErrorCode::InvalidState)?;
        match self.rooms[&code].phase {
            Phase::Lobby => return Err(ErrorCode::InvalidState),
            Phase::Intermission(_) => return Err(ErrorCode::RoundOver),
            Phase::Active(_) => {}
        }
        self.guess(&code, pid, city, now, out)
    }

    // ---- membership -------------------------------------------------------------------------

    /// Members of `code` whose socket is up, in join order.
    fn connected_members(&self, code: &str) -> Vec<PlayerId> {
        self.rooms[code]
            .members
            .iter()
            .copied()
            .filter(|m| self.players[m].conn.is_some())
            .collect()
    }

    fn on_player_disconnected(&mut self, now: Millis, pid: PlayerId, out: &mut Vec<Outbound>) {
        let player = self.players.get_mut(&pid).expect("known player");
        player.conn = None;
        let Some(code) = player.room.clone() else {
            // Not in a room: nothing to resume into.
            self.players.remove(&pid);
            return;
        };
        player.disconnected_at = Some(now);
        // A host who drops hands over to the next connected player (they do not get it back).
        let room = &self.rooms[&code];
        if room.host == pid
            && let Some(next) = self
                .connected_members(&code)
                .into_iter()
                .find(|m| *m != pid)
        {
            self.rooms.get_mut(&code).expect("exists").host = next;
        }
        // The socket is already gone, so the broadcast cannot reach this player.
        if let Some(next) = self.turn_successor(&code, pid) {
            self.pass_turn_to(&code, next, now, out);
        }
        self.after_membership_change(&code, now, out);
    }

    fn on_grace_expired(&mut self, pid: PlayerId, now: Millis, out: &mut Vec<Outbound>) {
        let Some(code) = self.players.get(&pid).map(|p| p.room.clone()) else {
            return;
        };
        if let Some(code) = code {
            self.remove_member(&code, pid, now, out);
        }
        self.players.remove(&pid);
    }

    /// Take `pid` out of `code` (they left or timed out). The caller has already cleared or
    /// dropped the player's own `room`.
    fn remove_member(&mut self, code: &str, pid: PlayerId, now: Millis, out: &mut Vec<Outbound>) {
        // Pick the successor first (the leaver is still seated), but announce it only once
        // the leaver is out, so that they do not hear about a room they have left.
        let successor = self.turn_successor(code, pid);
        let room = self.rooms.get_mut(code).expect("room of a member exists");
        room.members.retain(|m| *m != pid);
        room.scores.remove(&pid);
        if let Phase::Active(a) = &mut room.phase
            && let Kind::Course(c) = &mut a.kind
        {
            c.players.remove(&pid);
        }
        if room.members.is_empty() {
            self.rooms.remove(code);
            self.dirty.remove(code);
            return;
        }
        if room.host == pid {
            let next = self
                .connected_members(code)
                .first()
                .copied()
                .unwrap_or(self.rooms[code].members[0]);
            self.rooms.get_mut(code).expect("exists").host = next;
        }
        if let Some(next) = successor {
            self.pass_turn_to(code, next, now, out);
        }
        self.after_membership_change(code, now, out);
    }

    /// Common tail of every membership change: end a match nobody is connected to, finish a
    /// Course round whose connected players are all done, and refresh snapshots.
    fn after_membership_change(&mut self, code: &str, now: Millis, out: &mut Vec<Outbound>) {
        self.dirty.insert(code.to_owned());
        if matches!(self.rooms[code].phase, Phase::Lobby) {
            return;
        }
        if self.connected_members(code).is_empty() {
            self.rooms.get_mut(code).expect("exists").phase = Phase::Lobby;
            return;
        }
        self.finish_course_round_if_done(code, now, out);
    }

    fn close_idle_room(&mut self, code: &str, out: &mut Vec<Outbound>) {
        let Some(room) = self.rooms.remove(code) else {
            return;
        };
        self.dirty.remove(code);
        for m in room.members {
            let connected = self.players[&m].conn;
            match connected {
                Some(conn) => {
                    self.players.get_mut(&m).expect("known").room = None;
                    out.push(error(conn, ErrorCode::RoomNotFound));
                }
                None => {
                    self.players.remove(&m);
                }
            }
        }
    }

    // ---- snapshots --------------------------------------------------------------------------

    fn flush(&mut self, now: Millis, out: &mut Vec<Outbound>) {
        for code in std::mem::take(&mut self.dirty) {
            let Some(room) = self.rooms.get(&code) else {
                continue;
            };
            for &m in &room.members {
                if let Some(conn) = self.players[&m].conn {
                    let snap = self.snapshot_for(room, m, now);
                    out.push(reply(conn, ServerMsg::RoomSnapshot(snap)));
                }
            }
        }
    }

    fn snapshot_for(&self, room: &Room, viewer: PlayerId, now: Millis) -> RoomSnapshot {
        let score = |m: &PlayerId| match room.config.mode {
            Mode::Course => room.scores.get(m).map_or(0, |s| s.total),
            Mode::Cooperation => room.team_total,
        };
        let players = room
            .members
            .iter()
            .map(|m| PlayerInfo {
                id: *m,
                nickname: self.players[m].nickname.clone(),
                host: room.host == *m,
                connected: self.players[m].conn.is_some(),
                score: score(m),
            })
            .collect();
        let (phase, round, intermission) = match &room.phase {
            Phase::Lobby => (RoomPhase::Lobby, None, None),
            Phase::Active(active) => (
                RoomPhase::RoundActive,
                Some(self.round_view(room, active, viewer, now)),
                None,
            ),
            Phase::Intermission(pause) => {
                let remaining_ms = pause.until.saturating_sub(now);
                let mut last_round = pause.ended.clone();
                last_round.next_round_in_ms = Some(remaining_ms);
                (
                    RoomPhase::Intermission,
                    None,
                    Some(Intermission {
                        next_round_index: room.round_index + 1,
                        remaining_ms,
                        last_round,
                    }),
                )
            }
        };
        RoomSnapshot {
            code: room.code.clone(),
            config: room.config,
            phase,
            players,
            round,
            intermission,
        }
    }

    // ---- diagnostics (tests and operators; never expose on the network) --------------------

    /// Name of the secret city of the round in progress in `code`.
    ///
    /// For tests only: it is exactly what the protocol must never reveal early.
    #[doc(hidden)]
    pub fn debug_secret(&self, code: &str) -> Option<&'static str> {
        match &self.rooms.get(code)?.phase {
            Phase::Active(a) => Some(a.secret.name),
            _ => None,
        }
    }

    /// Verify the engine's internal consistency; `Err` describes the first violation.
    #[doc(hidden)]
    pub fn check_invariants(&self) -> Result<(), String> {
        for (code, room) in &self.rooms {
            if room.members.is_empty() {
                return Err(format!("room {code} is empty"));
            }
            if !room.members.contains(&room.host) {
                return Err(format!("room {code}: host is not a member"));
            }
            let unique: BTreeSet<_> = room.members.iter().collect();
            if unique.len() != room.members.len() {
                return Err(format!("room {code}: duplicate member"));
            }
            if room.members.len() > 8 {
                return Err(format!("room {code}: too many members"));
            }
            for m in &room.members {
                let p = self
                    .players
                    .get(m)
                    .ok_or(format!("room {code}: ghost member {m}"))?;
                if p.room.as_deref() != Some(code) {
                    return Err(format!("player {m} does not point back to room {code}"));
                }
                if p.conn.is_none() != p.disconnected_at.is_some() {
                    return Err(format!("player {m}: conn / disconnected_at disagree"));
                }
            }
            if room.scores.keys().any(|k| !room.members.contains(k)) {
                return Err(format!("room {code}: score of a non-member"));
            }
            match &room.phase {
                Phase::Lobby => {}
                Phase::Intermission(_) | Phase::Active(_) if room.round_index == 0 => {
                    return Err(format!("room {code}: round 0 in a match"));
                }
                Phase::Intermission(_) => {}
                Phase::Active(a) => match &a.kind {
                    Kind::Course(c) => {
                        let keys: Vec<_> = c.players.keys().copied().collect();
                        let mut members = room.members.clone();
                        members.sort_unstable();
                        if keys != members {
                            return Err(format!("room {code}: course states != members"));
                        }
                    }
                    Kind::Coop(c) => {
                        if !room.members.contains(&c.turn) {
                            return Err(format!("room {code}: turn held by a non-member"));
                        }
                        if c.guessers.len() != c.round.guesses().len() {
                            return Err(format!("room {code}: guessers out of sync"));
                        }
                    }
                },
            }
        }
        for (id, p) in &self.players {
            if let Some(code) = &p.room {
                let room = self
                    .rooms
                    .get(code)
                    .ok_or(format!("player {id}: room {code} gone"))?;
                if !room.members.contains(id) {
                    return Err(format!("player {id} not in room {code}"));
                }
            } else if p.conn.is_none() {
                return Err(format!("player {id}: no room and no connection"));
            }
            if let Some(c) = p.conn
                && self.conns.get(&c) != Some(&Some(*id))
            {
                return Err(format!("player {id}: conn {c} not mapped back"));
            }
        }
        for (c, p) in &self.conns {
            if let Some(pid) = p
                && self.players.get(pid).and_then(|pl| pl.conn) != Some(*c)
            {
                return Err(format!("conn {c} points at player {pid} who is elsewhere"));
            }
        }
        Ok(())
    }
}

fn reply(to: ConnId, msg: ServerMsg) -> Outbound {
    Outbound {
        to,
        msg,
        close: false,
    }
}

fn error(to: ConnId, code: ErrorCode) -> Outbound {
    reply(to, ServerMsg::error(code))
}

/// A guess as the wire sees it (used by the round module).
pub(crate) fn guess_info(
    by: PlayerId,
    fb: &boussole_core::game::GuessFeedback,
    tries_used: usize,
) -> GuessInfo {
    GuessInfo {
        by,
        city: fb.city.name.to_owned(),
        distance_km: fb.distance_km.round() as u32,
        bearing_deg: (fb.bearing_deg * 10.0).round() / 10.0,
        correct: fb.correct,
        tries_used: tries_used as u8,
    }
}

/// `search::find_exact` re-exported for the round module (keeps the dependency in one place).
pub(crate) fn find_city(name: &str) -> Option<&'static boussole_core::cities::City> {
    search::find_exact(name)
}
