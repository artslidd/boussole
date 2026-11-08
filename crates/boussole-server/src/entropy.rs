use crate::engine::Entropy;
use std::io;

/// [`Entropy`] backed by the operating system's CSPRNG (`getrandom`).
///
/// Resume tokens are bearer credentials and round seeds must be unpredictable, so the server
/// never uses [`crate::engine::SeededEntropy`].
#[derive(Debug)]
pub struct OsEntropy(());

impl OsEntropy {
    /// Probe the OS generator once, so that a machine without one fails at startup with an
    /// `io::Error` instead of panicking later in the middle of a match.
    pub fn new() -> io::Result<Self> {
        getrandom::u64()
            .map_err(|e| io::Error::other(format!("OS random source unavailable: {e}")))?;
        Ok(Self(()))
    }
}

impl Entropy for OsEntropy {
    fn next_u64(&mut self) -> u64 {
        // The generator worked at startup; a later failure is unrecoverable and must not be
        // papered over with weak randomness (that would silently weaken resume tokens).
        getrandom::u64().expect("OS random source failed after startup")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_distinct_values() {
        let mut e = OsEntropy::new().expect("os rng");
        let values: std::collections::BTreeSet<u64> = (0..64).map(|_| e.next_u64()).collect();
        assert_eq!(values.len(), 64, "64 draws from a CSPRNG must not collide");
    }
}
