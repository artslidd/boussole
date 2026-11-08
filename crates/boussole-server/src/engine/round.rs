//! Round life cycle for both modes: start, guesses, turns, end, intermission, ranking.

use super::room::{Active, CoopRound, CourseRound, CourseState, Kind, Pause, Phase, Room};
use super::score::{competition_ranks, coop_points, course_points};
use super::{Engine, Millis, Outbound, find_city, guess_info, reply};
use boussole_core::game::{GuessError, Round, RoundStatus};
use boussole_protocol::{
    ErrorCode, GuessEntry, HintDto, INTERMISSION_MS, MAX_TRIES, MatchEnded, Mode, PlayerId,
    PlayerRoundResult, Progress, RankEntry, RoundEnded, RoundStarted, RoundView, ServerMsg, Timer,
    TurnReason,
};

fn entries(round: &Round) -> Vec<GuessEntry> {
    round
        .guesses()
        .iter()
        .map(|g| GuessEntry {
            city: g.city.name.to_owned(),
            distance_km: g.distance_km.round() as u32,
        })
        .collect()
}

/// Arrow targets for the wire: distances only in Facile; bearings to 0.1°.
fn hints_dto(round: &Round) -> Vec<HintDto> {
    let facile = round.difficulty().shows_distances();
    round
        .hints()
        .iter()
        .map(|h| HintDto {
            city: h.city.name.to_owned(),
            bearing_deg: (h.bearing_deg * 10.0).round() / 10.0,
            distance_km: facile.then(|| h.distance_km.round() as u32),
        })
        .collect()
}

fn progress_of(player_id: PlayerId, round: &Round) -> Progress {
    Progress {
        player_id,
        tries_used: round.guesses().len() as u8,
        solved: round.status() == RoundStatus::Won,
        out_of_tries: round.status() == RoundStatus::Lost,
    }
}

fn turn_ms(room: &Room) -> u64 {
    u64::from(room.config.turn_time_s) * 1000
}

impl Engine {
    // ---- delivery ---------------------------------------------------------------------------

    fn send_player(&self, out: &mut Vec<Outbound>, pid: PlayerId, msg: ServerMsg) {
        if let Some(conn) = self.players[&pid].conn {
            out.push(reply(conn, msg));
        }
    }

    /// To every connected member of `code`, optionally skipping one.
    fn broadcast(
        &self,
        out: &mut Vec<Outbound>,
        code: &str,
        skip: Option<PlayerId>,
        msg: ServerMsg,
    ) {
        for m in self.connected_members(code) {
            if Some(m) != skip {
                self.send_player(out, m, msg.clone());
            }
        }
    }

    // ---- round start ------------------------------------------------------------------------

    pub(super) fn start_round(&mut self, code: &str, now: Millis, out: &mut Vec<Outbound>) {
        let connected = self.connected_members(code);
        let room = self.rooms.get_mut(code).expect("room exists");
        let difficulty = room.config.difficulty.into();
        // No two consecutive rounds share a secret; the bound only guards a degenerate stream.
        let mut round = Round::new_random(difficulty, self.entropy.next_u64());
        for _ in 0..64 {
            if Some(round.secret()) != room.last_secret {
                break;
            }
            round = Round::new_random(difficulty, self.entropy.next_u64());
        }
        room.round_index += 1;
        room.last_secret = Some(round.secret());
        let secret = round.secret();
        let (kind, timer) = match room.config.mode {
            Mode::Course => {
                let limit = u64::from(room.config.round_time_s) * 1000;
                let players = room
                    .members
                    .iter()
                    .map(|m| {
                        (
                            *m,
                            CourseState {
                                round: round.clone(),
                                points: 0,
                            },
                        )
                    })
                    .collect();
                (
                    Kind::Course(CourseRound {
                        deadline: now + limit,
                        players,
                    }),
                    Timer::Round {
                        remaining_ms: limit,
                    },
                )
            }
            Mode::Cooperation => {
                // Round r starts with the (r − 1) mod n-th connected player.
                let turn = connected[(usize::from(room.round_index) - 1) % connected.len()];
                let ms = turn_ms(room);
                (
                    Kind::Coop(CoopRound {
                        round: round.clone(),
                        guessers: Vec::new(),
                        turn,
                        turn_deadline: now + ms,
                        idle_turns: 0,
                    }),
                    Timer::Turn {
                        player_id: turn,
                        remaining_ms: ms,
                    },
                )
            }
        };
        let started = RoundStarted {
            round_index: room.round_index,
            total_rounds: room.config.rounds as u8,
            max_tries: MAX_TRIES,
            hints: hints_dto(&round),
            timer,
        };
        room.phase = Phase::Active(Active {
            started_at: now,
            secret,
            kind,
        });
        self.dirty.insert(code.to_owned());
        self.broadcast(out, code, None, ServerMsg::RoundStarted(started));
    }

    pub(super) fn round_view(
        &self,
        room: &Room,
        active: &Active,
        viewer: PlayerId,
        now: Millis,
    ) -> RoundView {
        let (round, guesses, progress, timer) = match &active.kind {
            Kind::Course(c) => {
                let mine = &c.players[&viewer].round;
                let guesses = mine
                    .guesses()
                    .iter()
                    .enumerate()
                    .map(|(i, g)| guess_info(viewer, g, i + 1))
                    .collect();
                let progress = room
                    .members
                    .iter()
                    .map(|m| progress_of(*m, &c.players[m].round))
                    .collect();
                let timer = Timer::Round {
                    remaining_ms: c.deadline.saturating_sub(now),
                };
                (mine, guesses, progress, timer)
            }
            Kind::Coop(c) => {
                let guesses = c
                    .round
                    .guesses()
                    .iter()
                    .zip(&c.guessers)
                    .enumerate()
                    .map(|(i, (g, by))| guess_info(*by, g, i + 1))
                    .collect();
                let timer = Timer::Turn {
                    player_id: c.turn,
                    remaining_ms: c.turn_deadline.saturating_sub(now),
                };
                (&c.round, guesses, Vec::new(), timer)
            }
        };
        RoundView {
            start: RoundStarted {
                round_index: room.round_index,
                total_rounds: room.config.rounds as u8,
                max_tries: MAX_TRIES,
                hints: hints_dto(round),
                timer,
            },
            guesses,
            progress,
        }
    }

    // ---- guesses ----------------------------------------------------------------------------

    /// A guess from `pid` in a room whose phase is `Active`.
    pub(super) fn guess(
        &mut self,
        code: &str,
        pid: PlayerId,
        name: &str,
        now: Millis,
        out: &mut Vec<Outbound>,
    ) -> Result<(), ErrorCode> {
        let room = self.rooms.get_mut(code).expect("room exists");
        let Phase::Active(active) = &mut room.phase else {
            return Err(ErrorCode::InvalidState);
        };
        let started_at = active.started_at;
        let elapsed = now - started_at;
        match &mut active.kind {
            Kind::Course(c) => {
                let state = c.players.get_mut(&pid).ok_or(ErrorCode::InvalidState)?;
                if state.round.status() != RoundStatus::InProgress {
                    return Err(ErrorCode::RoundOver);
                }
                let city = find_city(name).ok_or(ErrorCode::UnknownCity)?;
                let fb = *state.round.guess(city).map_err(|e| match e {
                    GuessError::AlreadyGuessed => ErrorCode::AlreadyGuessed,
                    GuessError::RoundOver => ErrorCode::RoundOver,
                })?;
                let tries = state.round.guesses().len();
                if fb.correct {
                    let limit = c.deadline - started_at;
                    state.points = course_points(tries as u32, elapsed, limit);
                }
                let info = guess_info(pid, &fb, tries);
                let progress = progress_of(pid, &state.round);
                // Private feedback to the guesser, bare progress to everyone else.
                self.send_player(out, pid, ServerMsg::GuessResult(info));
                self.broadcast(out, code, Some(pid), ServerMsg::PlayerProgress(progress));
                self.finish_course_round_if_done(code, now, out);
            }
            Kind::Coop(c) => {
                if c.turn != pid {
                    return Err(ErrorCode::NotYourTurn);
                }
                let city = find_city(name).ok_or(ErrorCode::UnknownCity)?;
                let fb = *c.round.guess(city).map_err(|e| match e {
                    GuessError::AlreadyGuessed => ErrorCode::AlreadyGuessed,
                    GuessError::RoundOver => ErrorCode::RoundOver,
                })?;
                c.guessers.push(pid);
                c.idle_turns = 0;
                let tries = c.round.guesses().len();
                let over = c.round.status() != RoundStatus::InProgress;
                let info = guess_info(pid, &fb, tries);
                self.broadcast(out, code, None, ServerMsg::GuessResult(info));
                if over {
                    self.end_round(code, now, out);
                } else if let Some(next) = self.next_connected_after(code, pid, None) {
                    self.set_turn(code, next, now, TurnReason::Guess, out);
                }
            }
        }
        Ok(())
    }

    // ---- turns (Coopération) ----------------------------------------------------------------

    /// First connected member after `after` in join order (cyclic, `after` itself last),
    /// ignoring `exclude`.
    fn next_connected_after(
        &self,
        code: &str,
        after: PlayerId,
        exclude: Option<PlayerId>,
    ) -> Option<PlayerId> {
        let members = &self.rooms[code].members;
        let start = members.iter().position(|m| *m == after)?;
        (1..=members.len())
            .map(|offset| members[(start + offset) % members.len()])
            .find(|m| Some(*m) != exclude && self.players[m].conn.is_some())
    }

    fn set_turn(
        &mut self,
        code: &str,
        next: PlayerId,
        now: Millis,
        reason: TurnReason,
        out: &mut Vec<Outbound>,
    ) {
        let room = self.rooms.get_mut(code).expect("room exists");
        let ms = turn_ms(room);
        if let Phase::Active(Active {
            kind: Kind::Coop(c),
            ..
        }) = &mut room.phase
        {
            c.turn = next;
            c.turn_deadline = now + ms;
        }
        self.broadcast(
            out,
            code,
            None,
            ServerMsg::TurnChanged {
                player_id: next,
                remaining_ms: ms,
                reason,
            },
        );
    }

    /// Who should get the turn if `leaver` holds it in an active Coopération round: the next
    /// connected player, not `leaver`. `None` if they do not hold it or nobody can take it.
    pub(super) fn turn_successor(&self, code: &str, leaver: PlayerId) -> Option<PlayerId> {
        let holds = matches!(
            &self.rooms[code].phase,
            Phase::Active(Active { kind: Kind::Coop(c), .. }) if c.turn == leaver
        );
        holds
            .then(|| self.next_connected_after(code, leaver, Some(leaver)))
            .flatten()
    }

    /// Hand the turn over because its holder disconnected or left.
    pub(super) fn pass_turn_to(
        &mut self,
        code: &str,
        next: PlayerId,
        now: Millis,
        out: &mut Vec<Outbound>,
    ) {
        self.set_turn(code, next, now, TurnReason::Disconnect, out);
    }

    // ---- timers -----------------------------------------------------------------------------

    pub(super) fn on_room_deadline(&mut self, code: &str, t: Millis, out: &mut Vec<Outbound>) {
        enum Step {
            EndRound,
            TurnTimeout(PlayerId, u32),
            OpenNextRound,
            Nothing,
        }
        let step = match &self.rooms[code].phase {
            Phase::Lobby => Step::Nothing,
            Phase::Intermission(_) => Step::OpenNextRound,
            Phase::Active(a) => match &a.kind {
                Kind::Course(_) => Step::EndRound,
                Kind::Coop(c) => Step::TurnTimeout(c.turn, c.idle_turns + 1),
            },
        };
        match step {
            Step::Nothing => {}
            Step::EndRound => self.end_round(code, t, out),
            Step::OpenNextRound => {
                if self.connected_members(code).is_empty() {
                    self.rooms.get_mut(code).expect("exists").phase = Phase::Lobby;
                    self.dirty.insert(code.to_owned());
                } else {
                    self.start_round(code, t, out);
                }
            }
            Step::TurnTimeout(holder, idle) => {
                // A full lap of silent turns (at least two) means the table has gone away:
                // the round is lost rather than cycling forever.
                let connected = self.connected_members(code).len() as u32;
                if idle >= connected.max(2) {
                    self.end_round(code, t, out);
                    return;
                }
                if let Phase::Active(Active {
                    kind: Kind::Coop(c),
                    ..
                }) = &mut self.rooms.get_mut(code).expect("exists").phase
                {
                    c.idle_turns = idle;
                }
                match self.next_connected_after(code, holder, None) {
                    Some(next) => self.set_turn(code, next, t, TurnReason::Timeout, out),
                    None => {
                        self.rooms.get_mut(code).expect("exists").phase = Phase::Lobby;
                        self.dirty.insert(code.to_owned());
                    }
                }
            }
        }
    }

    // ---- round end --------------------------------------------------------------------------

    /// Course: end the round once every connected player has solved or run out of tries.
    pub(super) fn finish_course_round_if_done(
        &mut self,
        code: &str,
        now: Millis,
        out: &mut Vec<Outbound>,
    ) {
        let connected = self.connected_members(code);
        let done = match &self.rooms[code].phase {
            Phase::Active(Active {
                kind: Kind::Course(c),
                ..
            }) => {
                !connected.is_empty()
                    && connected
                        .iter()
                        .all(|m| c.players[m].round.status() != RoundStatus::InProgress)
            }
            _ => false,
        };
        if done {
            self.end_round(code, now, out);
        }
    }

    /// Score the round, reveal it, and either open the intermission or end the match.
    pub(super) fn end_round(&mut self, code: &str, now: Millis, out: &mut Vec<Outbound>) {
        let room = self.rooms.get_mut(code).expect("room exists");
        let Phase::Active(active) = std::mem::replace(&mut room.phase, Phase::Lobby) else {
            return;
        };
        let members = room.members.clone();
        let total_rounds = room.config.rounds as u8;
        let last = room.round_index >= total_rounds;
        let mut found_by = None;
        let results: Vec<PlayerRoundResult> = match active.kind {
            Kind::Course(c) => members
                .iter()
                .map(|m| {
                    let state = &c.players[m];
                    let solved = state.round.status() == RoundStatus::Won;
                    let points = if solved { state.points } else { 0 };
                    let score = room.scores.entry(*m).or_default();
                    score.total += points;
                    score.solved_rounds += u32::from(solved);
                    PlayerRoundResult {
                        player_id: *m,
                        guesses: entries(&state.round),
                        solved,
                        tries_used: state.round.guesses().len() as u8,
                        points,
                        total: score.total,
                    }
                })
                .collect(),
            Kind::Coop(c) => {
                let won = c.round.status() == RoundStatus::Won;
                let points = if won {
                    coop_points(c.round.guesses().len() as u32)
                } else {
                    0
                };
                room.team_total += points;
                if won {
                    found_by = c.guessers.last().copied();
                    if let Some(score) = found_by.and_then(|f| room.scores.get_mut(&f)) {
                        score.solved_rounds += 1;
                    }
                }
                members
                    .iter()
                    .map(|m| {
                        let mine: Vec<GuessEntry> = c
                            .round
                            .guesses()
                            .iter()
                            .zip(&c.guessers)
                            .filter(|(_, by)| *by == m)
                            .map(|(g, _)| GuessEntry {
                                city: g.city.name.to_owned(),
                                distance_km: g.distance_km.round() as u32,
                            })
                            .collect();
                        PlayerRoundResult {
                            player_id: *m,
                            tries_used: mine.len() as u8,
                            guesses: mine,
                            solved: found_by == Some(*m),
                            points,
                            total: room.team_total,
                        }
                    })
                    .collect()
            }
        };
        let ended = RoundEnded {
            round_index: room.round_index,
            total_rounds,
            secret: active.secret.name.to_owned(),
            found_by,
            results,
            next_round_in_ms: (!last).then_some(INTERMISSION_MS),
        };
        if !last {
            room.phase = Phase::Intermission(Pause {
                until: now + INTERMISSION_MS,
                ended: ended.clone(),
            });
        }
        self.dirty.insert(code.to_owned());
        self.broadcast(out, code, None, ServerMsg::RoundEnded(ended));
        if last {
            let ranking = self.ranking(&self.rooms[code]);
            self.broadcast(out, code, None, ServerMsg::MatchEnded(ranking));
        }
    }

    fn ranking(&self, room: &Room) -> MatchEnded {
        let mut rows: Vec<(PlayerId, u32, u32)> = room
            .members
            .iter()
            .map(|m| {
                let score = room.scores.get(m).copied().unwrap_or_default();
                match room.config.mode {
                    Mode::Course => (*m, score.total, score.solved_rounds),
                    Mode::Cooperation => (*m, room.team_total, score.solved_rounds),
                }
            })
            .collect();
        // Stable sorts: ties keep join order.
        match room.config.mode {
            Mode::Course => rows.sort_by_key(|r| std::cmp::Reverse(r.1)),
            Mode::Cooperation => rows.sort_by_key(|r| std::cmp::Reverse(r.2)),
        }
        let ranks = match room.config.mode {
            Mode::Course => competition_ranks(&rows.iter().map(|r| r.1).collect::<Vec<_>>()),
            Mode::Cooperation => vec![1; rows.len()],
        };
        MatchEnded {
            mode: room.config.mode,
            ranking: rows
                .iter()
                .zip(ranks)
                .map(|((id, total, solved_rounds), rank)| RankEntry {
                    player_id: *id,
                    nickname: self.players[id].nickname.clone(),
                    total: *total,
                    rank,
                    solved_rounds: *solved_rounds,
                })
                .collect(),
        }
    }
}
