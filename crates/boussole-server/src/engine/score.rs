//! Scoring and ranking formulas (pure).

use boussole_protocol::MAX_TRIES;

/// Course points for a round solved with `tries` tries (1–6) after `elapsed_ms` of a
/// `limit_ms` round: `100 × (7 − tries) + ⌊100 × (limit − elapsed) / limit⌋`.
///
/// The theoretical maximum is 700 (one try at 0 ms); any real answer takes at least 1 ms, so
/// the practical maximum is 699. `elapsed_ms` is clamped to the limit.
pub fn course_points(tries: u32, elapsed_ms: u64, limit_ms: u64) -> u32 {
    let tries = tries.clamp(1, u32::from(MAX_TRIES));
    let limit_ms = limit_ms.max(1);
    let left = limit_ms - elapsed_ms.min(limit_ms);
    100 * (7 - tries) + (100 * left / limit_ms) as u32
}

/// Coopération team points for a round found in `tries` tries: `100 × (7 − tries)`.
pub fn coop_points(tries: u32) -> u32 {
    100 * (7 - tries.clamp(1, u32::from(MAX_TRIES)))
}

/// Competition ranking ("1224"): `1 + number of entries with a strictly higher score`.
pub fn competition_ranks(scores: &[u32]) -> Vec<u32> {
    scores
        .iter()
        .map(|s| 1 + scores.iter().filter(|o| *o > s).count() as u32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn course_points_follow_the_formula() {
        let l = 120_000;
        assert_eq!(course_points(1, 0, l), 700); // theoretical max
        assert_eq!(course_points(1, 1, l), 699); // practical max: floor(99.99…)
        assert_eq!(course_points(1, 12_000, l), 600 + 90);
        assert_eq!(course_points(3, 60_000, l), 400 + 50);
        assert_eq!(course_points(6, 119_999, l), 100);
        assert_eq!(course_points(6, l, l), 100); // solved at the very limit: speed bonus 0
        assert_eq!(course_points(2, 40_000, 90_000), 500 + 55); // floor(55.55…)
        assert_eq!(course_points(1, 999_999, l), 600); // clamped
    }

    #[test]
    fn course_points_are_bounded_and_decrease_with_tries_and_time() {
        for tries in 1..=6 {
            for e in (0..=120_000).step_by(997) {
                let p = course_points(tries, e, 120_000);
                assert!((100..=700).contains(&p), "{tries} {e} {p}");
                assert!(course_points(tries, e + 1000, 120_000) <= p);
                if tries < 6 {
                    assert!(course_points(tries + 1, e, 120_000) < p);
                }
            }
        }
    }

    #[test]
    fn coop_points_follow_the_formula() {
        let all: Vec<u32> = (1..=6).map(coop_points).collect();
        assert_eq!(all, [600, 500, 400, 300, 200, 100]);
    }

    #[test]
    fn ties_share_a_rank_and_the_next_rank_skips() {
        assert_eq!(competition_ranks(&[500, 900, 500, 100]), [2, 1, 2, 4]);
        assert_eq!(competition_ranks(&[0, 0, 0]), [1, 1, 1]);
        assert_eq!(competition_ranks(&[7]), [1]);
        assert!(competition_ranks(&[]).is_empty());
    }
}
