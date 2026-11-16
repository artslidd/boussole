//! Great-circle distance, bearing and 8-point compass.

use crate::cities::City;

/// Mean Earth radius (km) used for the haversine formula.
const EARTH_RADIUS_KM: f64 = 6371.0088;

/// Haversine distance between two cities, in km.
pub fn distance_km(a: &City, b: &City) -> f64 {
    let (lat1, lat2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b.lon - a.lon).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * h.sqrt().min(1.0).asin()
}

/// Initial great-circle bearing from `from` to `to`: 0 = north, clockwise, in `[0, 360)`.
pub fn bearing_deg(from: &City, to: &City) -> f64 {
    let (lat1, lat2) = (from.lat.to_radians(), to.lat.to_radians());
    let dlon = (to.lon - from.lon).to_radians();
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    let deg = y.atan2(x).to_degrees().rem_euclid(360.0);
    // `rem_euclid` can round a tiny negative angle up to exactly 360.0.
    if deg >= 360.0 { 0.0 } else { deg }
}

/// Smallest angle between two bearings, in `[0, 180]`.
pub(crate) fn angle_gap(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 360.0;
    d.min(360.0 - d)
}

/// The eight principal compass directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compass8 {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

impl Compass8 {
    /// Nearest direction for a bearing; each direction owns a 45° sector centred on it
    /// (so N covers `[337.5, 360) ∪ [0, 22.5)`). Any real angle is accepted.
    pub fn from_bearing(deg: f64) -> Self {
        const ALL: [Compass8; 8] = [
            Compass8::N,
            Compass8::NE,
            Compass8::E,
            Compass8::SE,
            Compass8::S,
            Compass8::SW,
            Compass8::W,
            Compass8::NW,
        ];
        ALL[((deg.rem_euclid(360.0) + 22.5) / 45.0) as usize % 8]
    }

    pub fn arrow(self) -> char {
        match self {
            Self::N => '↑',
            Self::NE => '↗',
            Self::E => '→',
            Self::SE => '↘',
            Self::S => '↓',
            Self::SW => '↙',
            Self::W => '←',
            Self::NW => '↖',
        }
    }

    /// French abbreviation (west is "O" for *Ouest*).
    pub fn label_fr(self) -> &'static str {
        match self {
            Self::N => "N",
            Self::NE => "NE",
            Self::E => "E",
            Self::SE => "SE",
            Self::S => "S",
            Self::SW => "SO",
            Self::W => "O",
            Self::NW => "NO",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cities::by_name;

    fn assert_within_2_percent(a: &str, b: &str, expected_km: f64) {
        let d = distance_km(by_name(a), by_name(b));
        assert!(
            (d - expected_km).abs() <= expected_km * 0.02,
            "{a}–{b}: {d:.1} km, expected ≈ {expected_km} km"
        );
    }

    #[test]
    fn reference_distances_within_two_percent() {
        assert_within_2_percent("Paris", "Lyon", 392.0);
        assert_within_2_percent("Paris", "Marseille", 661.0);
        assert_within_2_percent("Paris", "Toulouse", 589.0);
        assert_within_2_percent("Paris", "Bordeaux", 500.0);
        assert_within_2_percent("Paris", "Lille", 204.0);
        assert_within_2_percent("Paris", "Strasbourg", 397.0);
    }

    #[test]
    fn distance_is_zero_to_self_and_symmetric() {
        let (p, l) = (by_name("Paris"), by_name("Lyon"));
        assert_eq!(distance_km(p, p), 0.0);
        assert!((distance_km(p, l) - distance_km(l, p)).abs() < 1e-9);
    }

    #[test]
    fn paris_bearings_match_geography() {
        let p = by_name("Paris");
        let compass = |to: &str| Compass8::from_bearing(bearing_deg(p, by_name(to)));
        assert_eq!(compass("Lille"), Compass8::N);
        assert_eq!(compass("Strasbourg"), Compass8::E);
        assert_eq!(compass("Lyon"), Compass8::SE);
        assert_eq!(compass("Bordeaux"), Compass8::SW);
    }

    #[test]
    fn bearing_is_always_in_range_and_reverse_differs_by_about_180() {
        for a in crate::cities::CITIES {
            for b in crate::cities::CITIES {
                let deg = bearing_deg(a, b);
                assert!(
                    (0.0..360.0).contains(&deg),
                    "{} → {}: {deg}",
                    a.name,
                    b.name
                );
            }
        }
        let (p, l) = (by_name("Paris"), by_name("Lyon"));
        let diff = (bearing_deg(p, l) - bearing_deg(l, p)).rem_euclid(360.0);
        assert!((diff - 180.0).abs() < 5.0, "diff {diff}");
    }

    #[test]
    fn compass_sector_boundaries() {
        use Compass8::*;
        let cases = [
            (0.0, N),
            (22.49, N),
            (22.5, NE),
            (45.0, NE),
            (67.49, NE),
            (67.5, E),
            (90.0, E),
            (112.5, SE),
            (157.5, S),
            (180.0, S),
            (202.5, SW),
            (247.5, W),
            (270.0, W),
            (292.5, NW),
            (337.49, NW),
            (337.5, N),
            (359.9, N),
            (360.0, N),
            (-10.0, N),
            (-90.0, W),
        ];
        for (deg, expected) in cases {
            assert_eq!(Compass8::from_bearing(deg), expected, "{deg}°");
        }
    }

    #[test]
    fn arrows_and_french_labels() {
        use Compass8::*;
        let all = [N, NE, E, SE, S, SW, W, NW];
        let arrows: String = all.iter().map(|c| c.arrow()).collect();
        assert_eq!(arrows, "↑↗→↘↓↙←↖");
        let labels: Vec<_> = all.iter().map(|c| c.label_fr()).collect();
        assert_eq!(labels, ["N", "NE", "E", "SE", "S", "SO", "O", "NO"]);
    }
}
