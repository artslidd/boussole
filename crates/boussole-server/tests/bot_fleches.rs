//! The Flèches bot against a real server, both modes: the same strategy the `bot`
//! example uses (`--game fleches --skill …`) drives three WebSocket clients through whole
//! matches, with the frame-level secret check on every round.

mod net;

use boussole_protocol::*;
use net::strategy::Strategy;
use net::{TestClient, assert_fleches_round_safe, fast, server};

/// A Flèches room (`question_time_s` 5 s so a slow test round would time out visibly).
fn fleches_config(mode: Mode, rounds: u32) -> RoomConfig {
    RoomConfig {
        game: GameKind::Fleches,
        question_time_s: 5,
        rounds,
        ..RoomConfig::new(mode, Difficulty::Facile)
    }
}

/// Play one Flèches Course match like the bot does: on each question, answer once with the
/// bot's skill; collect every `round_ended` and the final ranking. Every round's frames are
/// checked against the reveal rule.
async fn play_fleches_course(
    c: &mut TestClient,
    skill: u32,
    seed: u64,
    rounds: usize,
) -> Vec<RoundEnded> {
    let mut strat = Strategy::new(seed);
    let mut rounds_seen = Vec::new();
    for _ in 0..rounds {
        let started = c
            .recv_where(|m| match m {
                ServerMsg::RoundStarted(r) => Some(r.clone()),
                _ => None,
            })
            .await;
        let question = started.question.clone().expect("a flèches question");
        let first_frame = c.frames.len() - 1;
        let option = strat.answer_fleches(&question, skill);
        c.send(&ClientMsg::Answer { option }).await;
        let ended = c
            .recv_where(|m| match m {
                ServerMsg::RoundEnded(e) => Some(e.clone()),
                _ => None,
            })
            .await;
        // The frames of this round, up to (excluding) the round_ended frame.
        assert_fleches_round_safe(&c.frames[first_frame..c.frames.len() - 1]);
        // The reveal says what this client answered.
        let mine = ended
            .fleches
            .as_ref()
            .expect("a reveal")
            .answers
            .iter()
            .find(|a| a.player_id == c.id)
            .expect("a result per player");
        assert_eq!(mine.option, Some(option), "the reveal shows my answer");
        assert_eq!(
            mine.correct,
            mine.option == Some(ended.fleches.as_ref().unwrap().correct)
        );
        rounds_seen.push(ended);
    }
    rounds_seen
}

#[tokio::test]
async fn three_bots_finish_a_fleches_course_match_with_identical_scoreboards() {
    let server = server(fast(10)).await;
    let addr = server.local_addr();
    let mut clients = [
        TestClient::connect(addr, "Ada").await,
        TestClient::connect(addr, "Bob").await,
        TestClient::connect(addr, "Cleo").await,
    ];
    let code = clients[0]
        .create(fleches_config(Mode::Course, 3))
        .await
        .code;
    clients[1].join(&code).await;
    clients[2].join(&code).await;
    clients[0].send(&ClientMsg::StartMatch).await;

    // Ada answers perfectly, Bob always wrongly, Cleo with the default skill.
    let [a, b, c] = &mut clients;
    let (ra, rb, rc) = tokio::join!(
        play_fleches_course(a, 100, 11, 3),
        play_fleches_course(b, 0, 22, 3),
        play_fleches_course(c, 50, 33, 3),
    );
    assert_eq!((ra.len(), rb.len(), rc.len()), (3, 3, 3));

    // Every client saw the same rounds and the same final ranking.
    let a = &mut clients[0];
    let end_a: MatchEnded = a
        .recv_where(|m| match m {
            ServerMsg::MatchEnded(m) => Some(m.clone()),
            _ => None,
        })
        .await;
    let b = &mut clients[1];
    let end_b: MatchEnded = b
        .recv_where(|m| match m {
            ServerMsg::MatchEnded(m) => Some(m.clone()),
            _ => None,
        })
        .await;
    let c = &mut clients[2];
    let end_c: MatchEnded = c
        .recv_where(|m| match m {
            ServerMsg::MatchEnded(m) => Some(m.clone()),
            _ => None,
        })
        .await;
    assert_eq!(end_a.ranking, end_b.ranking);
    assert_eq!(end_a.ranking, end_c.ranking, "identical scoreboards");

    let total_of =
        |m: &MatchEnded, id: PlayerId| m.ranking.iter().find(|r| r.player_id == id).unwrap().total;
    let solved_of = |m: &MatchEnded, id: PlayerId| {
        m.ranking
            .iter()
            .find(|r| r.player_id == id)
            .unwrap()
            .solved_rounds
    };
    // Ada: always right (100–150 per round); Bob: always wrong (0); Cleo: in between.
    assert!(
        (300..=450).contains(&total_of(&end_a, clients[0].id)),
        "{:?}",
        end_a.ranking
    );
    assert_eq!(total_of(&end_a, clients[1].id), 0);
    assert_eq!(solved_of(&end_a, clients[1].id), 0);
    assert!(solved_of(&end_a, clients[0].id) == 3);
    // The scoreboard agrees with the sum of the rounds' points (Ada's rounds carry
    // everyone's answers).
    let sum_for = |id: PlayerId| -> u32 {
        ra.iter()
            .filter_map(|e| {
                e.fleches.as_ref().and_then(|f| {
                    f.answers
                        .iter()
                        .find(|a| a.player_id == id)
                        .map(|a| a.points)
                })
            })
            .sum()
    };
    assert_eq!(sum_for(clients[0].id), total_of(&end_a, clients[0].id));
    assert_eq!(sum_for(clients[1].id), total_of(&end_a, clients[1].id));
    assert_eq!(sum_for(clients[2].id), total_of(&end_a, clients[2].id));
    server.shutdown().await;
}

/// Play one Flèches Coopération match like the bot does: answer only when the question is
/// yours (the turn rotates with the round index); every round is checked against the reveal
/// rule. Returns the rounds and the final ranking.
async fn play_coop(c: &mut TestClient) -> (Vec<RoundEnded>, MatchEnded) {
    let mut strat = Strategy::new(7);
    let mut rounds = Vec::new();
    for _ in 0..3 {
        let started = c
            .recv_where(|m| match m {
                ServerMsg::RoundStarted(r) => Some(r.clone()),
                _ => None,
            })
            .await;
        let question = started.question.clone().expect("a flèches question");
        let first_frame = c.frames.len() - 1;
        if let Timer::Turn { player_id, .. } = started.timer
            && player_id == c.id
        {
            let option = strat.answer_fleches(&question, 100);
            c.send(&ClientMsg::Answer { option }).await;
        }
        let ended = c
            .recv_where(|m| match m {
                ServerMsg::RoundEnded(e) => Some(e.clone()),
                _ => None,
            })
            .await;
        assert_fleches_round_safe(&c.frames[first_frame..c.frames.len() - 1]);
        rounds.push(ended);
    }
    let m = c
        .recv_where(|m| match m {
            ServerMsg::MatchEnded(m) => Some(m.clone()),
            _ => None,
        })
        .await;
    (rounds, m)
}

#[tokio::test]
async fn three_bots_finish_a_fleches_cooperation_match_as_one_team() {
    let server = server(fast(10)).await;
    let addr = server.local_addr();
    let mut clients = [
        TestClient::connect(addr, "Ada").await,
        TestClient::connect(addr, "Bob").await,
        TestClient::connect(addr, "Cleo").await,
    ];
    let code = clients[0]
        .create(fleches_config(Mode::Cooperation, 3))
        .await
        .code;
    clients[1].join(&code).await;
    clients[2].join(&code).await;
    clients[0].send(&ClientMsg::StartMatch).await;

    // One answerer per question, rotating in join order; the bots all play perfectly.
    let [a, b, c] = &mut clients;
    let ((ra, ma), (rb, mb), (rc, mc)) = tokio::join!(play_coop(a), play_coop(b), play_coop(c));

    // Everyone saw the same rounds and ranking.
    assert_eq!(ra, rb);
    assert_eq!(ra, rc);
    assert_eq!(ma.ranking, mb.ranking);
    assert_eq!(ma.ranking, mc.ranking, "identical scoreboards");

    // The team answered every question right: 1 point each, three perfect answerers.
    assert!(ma.ranking.iter().all(|r| r.total == 3));
    assert!(ma.ranking.iter().all(|r| r.rank == 1));
    let solved: Vec<u32> = ma.ranking.iter().map(|r| r.solved_rounds).collect();
    assert_eq!(solved, [1, 1, 1], "each bot answered one question");
    // Every reveal credits exactly the rotating answerer.
    for (round, e) in ra.iter().enumerate() {
        let reveal = e.fleches.as_ref().expect("a reveal");
        let answerer = clients[round % 3].id;
        assert_eq!(e.found_by, Some(answerer));
        assert!(
            reveal
                .answers
                .iter()
                .find(|a| a.player_id == answerer)
                .unwrap()
                .correct
        );
    }
    server.shutdown().await;
}
