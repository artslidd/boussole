//! Geometry checks for the round canvas over the real data: arrows keep their bearing and a
//! fixed length, and the labels never overlap each other, an arrow, the dial or the compass
//! rose, and never leave the canvas, at the default window size and at the 800 × 600 minimum.

use boussole::board::{HintLabel, format_hint_label, layout_board, screen_vector};
use boussole::cities::CITIES;
use boussole::game::{Difficulty, Round};
use boussole::search::find_exact;

// Canvas sizes (logical px) for the window sizes the app supports: the window minus the
// header (52), status strip (46) and dock (168) of `ui/app.slint`.
const CHROME_HEIGHT: f64 = 52.0 + 46.0 + 168.0;
const SIZES: [(f64, f64); 4] = [
    (1000.0, 760.0 - CHROME_HEIGHT), // default window
    (800.0, 600.0 - CHROME_HEIGHT),  // minimum window
    (900.0, 700.0 - CHROME_HEIGHT),
    (1440.0, 900.0 - CHROME_HEIGHT),
];

fn hint_labels(round: &Round) -> Vec<HintLabel> {
    round
        .hints()
        .iter()
        .map(|hint| HintLabel {
            bearing_deg: hint.bearing_deg,
            text: format_hint_label(hint, round.difficulty()),
        })
        .collect()
}

/// Furthest a label may sit from its arrow tip on real data (px).
const MAX_TIP_DISTANCE: f64 = 70.0;

fn assert_valid(context: &str, width: f64, height: f64, hints: &[HintLabel]) {
    assert_valid_within(context, width, height, hints, MAX_TIP_DISTANCE);
}

fn assert_valid_within(
    context: &str,
    width: f64,
    height: f64,
    hints: &[HintLabel],
    max_tip_distance: f64,
) {
    let layout = layout_board(width, height, hints);
    let problems = layout.problems();
    assert!(
        problems.is_empty(),
        "{context} at {width}×{height}: {problems:?}\nlabels: {:#?}",
        layout.labels
    );
    // Arrow directions are exact and lengths identical.
    for (arrow, hint) in layout.arrows.iter().zip(hints) {
        let dir = screen_vector(hint.bearing_deg);
        let dx = arrow.tip.x - arrow.start.x;
        let dy = arrow.tip.y - arrow.start.y;
        let length = dx.hypot(dy);
        assert!((length - layout.arrow_length).abs() < 1e-6, "{context}");
        assert!((dx / length - dir.x).abs() < 1e-9 && (dy / length - dir.y).abs() < 1e-9);
    }
    // A label stays close to its arrow: its anchor is within the allowed drift and the tip is
    // never far from the label box.
    for label in &layout.labels {
        let gap = label.rect.distance_to_point(label.tip);
        assert!(
            gap < max_tip_distance,
            "{context}: {:?} is {gap:.0}px from its tip",
            label.text
        );
    }
}

#[test]
fn every_secret_gives_a_valid_board_in_both_difficulties() {
    for secret in CITIES {
        for seed in 0..4 {
            for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
                let round = Round::with_secret(secret, difficulty, seed);
                let hints = hint_labels(&round);
                for (w, h) in SIZES {
                    assert_valid(
                        &format!("secret {} seed {seed} {difficulty:?}", secret.name),
                        w,
                        h,
                        &hints,
                    );
                }
            }
        }
    }
}

#[test]
fn random_rounds_give_valid_boards() {
    for seed in 0..300 {
        for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
            let round = Round::new_random(difficulty, seed);
            for (w, h) in SIZES {
                assert_valid(
                    &format!("seed {seed} {difficulty:?}"),
                    w,
                    h,
                    &hint_labels(&round),
                );
            }
        }
    }
}

/// The tightest relaxed secret: Porto-Vecchio has hints only 15° apart and up to 1000 km away.
#[test]
fn porto_vecchio_with_its_15_degree_gap_stays_readable() {
    let secret = find_exact("Porto-Vecchio").unwrap();
    let mut tightest = f64::MAX;
    for seed in 0..60 {
        for difficulty in [Difficulty::Facile, Difficulty::Difficile] {
            let round = Round::with_secret(secret, difficulty, seed);
            let bearings: Vec<f64> = round.hints().iter().map(|h| h.bearing_deg).collect();
            for (i, a) in bearings.iter().enumerate() {
                for b in &bearings[i + 1..] {
                    let diff = (a - b).abs();
                    tightest = tightest.min(diff.min(360.0 - diff));
                }
            }
            let hints = hint_labels(&round);
            for (w, h) in SIZES {
                assert_valid(
                    &format!("Porto-Vecchio seed {seed} {difficulty:?}"),
                    w,
                    h,
                    &hints,
                );
            }
        }
    }
    assert!(
        tightest < 20.0,
        "the sweep should include a near-15° pair (tightest {tightest:.1}°)"
    );
}

/// Longest realistic Facile labels (`Boulogne-sur-Mer · 1 000 km`): one close pair (15°) and
/// otherwise the minimum 35° spacing, from every starting bearing.
#[test]
fn long_facile_labels_with_a_close_pair_stay_readable() {
    let names = [
        "Boulogne-sur-Mer · 1 000 km",
        "Châlons-en-Champagne · 1 000 km",
        "Saint-Jean-de-Luz · 1 000 km",
        "Évry-Courcouronnes · 1 000 km",
        "Aix-en-Provence · 1 000 km",
    ];
    let gaps = [15.0, 35.0, 35.0, 35.0];
    for start in (0..360).step_by(5) {
        let mut bearing = f64::from(start);
        let mut hints = Vec::new();
        for (i, name) in names.iter().enumerate() {
            hints.push(HintLabel {
                bearing_deg: bearing.rem_euclid(360.0),
                text: (*name).to_owned(),
            });
            if i < gaps.len() {
                bearing += gaps[i];
            }
        }
        for (w, h) in SIZES {
            assert_valid(&format!("close pair from {start}°"), w, h, &hints);
        }
    }
}

/// Far beyond what the game produces — five long labels all 15° apart — must still never
/// overlap or leave the canvas (labels may then sit further from their tips).
#[test]
fn five_long_labels_at_15_degrees_still_never_overlap() {
    for start in (0..360).step_by(10) {
        let hints: Vec<HintLabel> = (0..5)
            .map(|i| HintLabel {
                bearing_deg: (f64::from(start) + 15.0 * f64::from(i)).rem_euclid(360.0),
                text: "Boulogne-sur-Mer · 1 000 km".to_owned(),
            })
            .collect();
        for (w, h) in SIZES {
            assert_valid_within(&format!("stress from {start}°"), w, h, &hints, 140.0);
        }
    }
}
