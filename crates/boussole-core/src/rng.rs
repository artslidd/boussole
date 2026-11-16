//! The private SplitMix64 generator shared by the game modules.

/// SplitMix64: tiny, fast, well-mixed even for consecutive seeds (0, 1, 2, …).
pub(crate) struct SplitMix64(u64);

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-enough index in `0..n` (modulo bias is ~n/2^64, irrelevant here).
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// Uniform float in `[0, 1)` (53 random mantissa bits).
    pub(crate) fn unit(&mut self) -> f64 {
        const SCALE: f64 = 1.0 / (1u64 << 53) as f64;
        (self.next_u64() >> 11) as f64 * SCALE
    }

    /// Fisher–Yates.
    pub(crate) fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_matches_reference_vector() {
        // First outputs of SplitMix64 seeded with 0 (reference implementation).
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }

    #[test]
    fn unit_stays_in_the_half_open_interval() {
        let mut rng = SplitMix64::new(0);
        let mut sum = 0.0;
        for _ in 0..10_000 {
            let u = rng.unit();
            assert!((0.0..1.0).contains(&u), "{u}");
            sum += u;
        }
        // Rough sanity: the mean of 10 000 draws should be near 0.5.
        let mean = sum / 10_000.0;
        assert!((0.48..=0.52).contains(&mean), "mean {mean}");
    }
}
