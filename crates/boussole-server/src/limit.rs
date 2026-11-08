//! Per-connection message rate limiting (token bucket with an abuse tolerance).

use std::time::Duration;

/// What to do with the message that was just received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Process it.
    Allow,
    /// Drop it and answer `rate_limited`.
    Limited,
    /// The client keeps flooding: close the connection.
    Abusive,
}

/// Token bucket (`rate` messages per second, bursts up to `burst`) plus a second, smaller
/// bucket that pays for every refused message: a client that merely brushes the limit gets
/// `rate_limited` answers, one that keeps flooding empties the tolerance and is dropped.
///
/// Time is passed in (monotonic, as a [`Duration`] since any fixed origin) so the logic is
/// testable without sleeping.
#[derive(Debug)]
pub struct RateLimiter {
    rate: f64,
    burst: f64,
    tokens: f64,
    tolerance: f64,
    tolerance_max: f64,
    last: Duration,
}

/// Refused messages tolerated in a row before the connection is dropped.
const TOLERANCE: f64 = 30.0;
/// Tolerance regained per second while the client behaves.
const TOLERANCE_REFILL_PER_S: f64 = 5.0;

impl RateLimiter {
    pub fn new(rate_per_s: u32, burst: u32, now: Duration) -> Self {
        let burst = f64::from(burst.max(1));
        Self {
            rate: f64::from(rate_per_s.max(1)),
            burst,
            tokens: burst,
            tolerance: TOLERANCE,
            tolerance_max: TOLERANCE,
            last: now,
        }
    }

    pub fn check(&mut self, now: Duration) -> Verdict {
        let dt = now.saturating_sub(self.last).as_secs_f64();
        self.last = self.last.max(now);
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
        self.tolerance = (self.tolerance + dt * TOLERANCE_REFILL_PER_S).min(self.tolerance_max);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return Verdict::Allow;
        }
        self.tolerance -= 1.0;
        if self.tolerance < 0.0 {
            Verdict::Abusive
        } else {
            Verdict::Limited
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn burst_then_limited_then_refills() {
        let mut l = RateLimiter::new(10, 20, ms(0));
        for i in 0..20 {
            assert_eq!(l.check(ms(0)), Verdict::Allow, "burst message {i}");
        }
        assert_eq!(l.check(ms(0)), Verdict::Limited);
        // 100 ms regains exactly one token at 10 msg/s.
        assert_eq!(l.check(ms(100)), Verdict::Allow);
        assert_eq!(l.check(ms(100)), Verdict::Limited);
    }

    #[test]
    fn sustained_rate_is_allowed_forever() {
        let mut l = RateLimiter::new(10, 20, ms(0));
        for i in 0..1_000u64 {
            // 10 msg/s exactly.
            assert_eq!(l.check(ms(i * 100)), Verdict::Allow, "message {i}");
        }
    }

    #[test]
    fn flooding_ends_in_abuse_but_a_slight_overrun_does_not() {
        let mut l = RateLimiter::new(10, 20, ms(0));
        let mut verdicts = (0..200).map(|_| l.check(ms(0)));
        assert_eq!(verdicts.nth(19), Some(Verdict::Allow));
        assert!(
            verdicts.any(|v| v == Verdict::Abusive),
            "a flood must be dropped"
        );

        // 12 msg/s for 10 s: refused every few messages, but never abusive.
        let mut l = RateLimiter::new(10, 20, ms(0));
        let mut limited = 0;
        for i in 0..120u64 {
            match l.check(ms(i * 1000 / 12)) {
                Verdict::Allow => {}
                Verdict::Limited => limited += 1,
                Verdict::Abusive => panic!("a slight overrun must not be abusive (message {i})"),
            }
        }
        assert!(limited > 0);
    }
}
