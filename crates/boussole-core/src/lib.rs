//! Boussole core: the GUI-free, dependency-free game kernel shared by the desktop client and
//! the server.
//!
//! [`cities`] (embedded dataset), [`geo`] (distance, bearing, compass), [`search`]
//! (autocomplete), [`game`] (difficulty, hints, rounds) and [`arrows`] (the Flèches
//! arrow-picking game).
//!
//! The dataset is part of the multiplayer contract: the server and every client must agree on
//! the list of cities (names are the wire identifiers). [`dataset_fingerprint`] lets them check
//! that on connect.

pub mod arrows;
pub mod cities;
pub mod game;
pub mod geo;
mod rng;
pub mod search;

/// Human-readable dataset revision. **Bump it whenever the content of
/// [`cities::CITIES`](cities::CITIES) changes in a way that matters on the wire**: a city added,
/// removed, renamed, moved or reordered. The `dataset_fingerprint_is_pinned` test fails in that
/// case; update the pinned value and bump this constant in the same commit. Edits that do not
/// touch names or coordinates (population, `reference`) do not change the fingerprint and need
/// no bump.
pub const DATASET_VERSION: u32 = 1;

/// Stable 64-bit FNV-1a fingerprint of the dataset: for each city, in order, its name bytes,
/// a `0xFF` separator (never valid in UTF-8), then `lat` and `lon` as little-endian IEEE-754
/// bits. Identical on every platform and Rust version.
pub fn dataset_fingerprint() -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    for city in cities::CITIES {
        feed(city.name.as_bytes());
        feed(&[0xFF]);
        feed(&city.lat.to_bits().to_le_bytes());
        feed(&city.lon.to_bits().to_le_bytes());
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// If this fails the dataset changed: update the value AND bump [`DATASET_VERSION`].
    const PINNED_FINGERPRINT: u64 = 11_027_042_293_905_816_568;

    #[test]
    fn dataset_fingerprint_is_pinned() {
        assert_eq!(
            dataset_fingerprint(),
            PINNED_FINGERPRINT,
            "dataset changed: pin the new fingerprint and bump DATASET_VERSION (now {DATASET_VERSION})"
        );
    }
}
