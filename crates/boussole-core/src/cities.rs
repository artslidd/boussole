//! Embedded dataset of metropolitan French cities (Corsica included).
//!
//! Coordinates are city-centre latitude/longitude in decimal degrees (error ≲ 2 km).
//! Populations are approximate commune populations (INSEE ≈ 2021), used for ranking only.
//!
//! Selection: every metropolitan préfecture except the three inner-ring Paris suburbs
//! (Nanterre, Bobigny, Créteil: suburbs, and too close to Paris), plus other well-known
//! cities. No suburbs and no two cities closer than 10 km (so no Biarritz next to Bayonne,
//! no Antibes next to Cannes). `reference` cities are the large or regionally iconic ones
//! used as hint targets; the secret can be any city.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct City {
    pub name: &'static str,
    pub lat: f64,
    pub lon: f64,
    pub population: u32,
    /// Well-known city, eligible as a hint target.
    pub reference: bool,
}

const fn city(name: &'static str, lat: f64, lon: f64, population: u32, reference: bool) -> City {
    City {
        name,
        lat,
        lon,
        population,
        reference,
    }
}

pub static CITIES: &[City] = &[
    city("Paris", 48.8566, 2.3522, 2_133_000, true),
    city("Marseille", 43.2965, 5.3698, 873_000, true),
    city("Lyon", 45.7640, 4.8357, 522_000, true),
    city("Toulouse", 43.6047, 1.4442, 504_000, true),
    city("Nice", 43.7102, 7.2620, 342_000, true),
    city("Nantes", 47.2184, -1.5536, 320_000, true),
    city("Montpellier", 43.6108, 3.8767, 299_000, true),
    city("Strasbourg", 48.5734, 7.7521, 291_000, true),
    city("Bordeaux", 44.8378, -0.5792, 262_000, true),
    city("Lille", 50.6292, 3.0573, 236_000, true),
    city("Rennes", 48.1173, -1.6778, 222_000, true),
    city("Reims", 49.2583, 4.0317, 183_000, true),
    city("Toulon", 43.1242, 5.9280, 177_000, true),
    city("Saint-Étienne", 45.4397, 4.3872, 173_000, true),
    city("Le Havre", 49.4944, 0.1079, 169_000, true),
    city("Dijon", 47.3220, 5.0415, 159_000, true),
    city("Grenoble", 45.1885, 5.7245, 158_000, true),
    city("Angers", 47.4784, -0.5632, 155_000, true),
    city("Nîmes", 43.8367, 4.3601, 151_000, true),
    city("Clermont-Ferrand", 45.7772, 3.0870, 147_000, true),
    city("Aix-en-Provence", 43.5297, 5.4474, 145_000, true),
    city("Le Mans", 48.0061, 0.1996, 144_000, true),
    city("Brest", 48.3904, -4.4861, 140_000, true),
    city("Tours", 47.3941, 0.6848, 136_000, true),
    city("Amiens", 49.8941, 2.2958, 134_000, true),
    city("Limoges", 45.8336, 1.2611, 131_000, true),
    city("Annecy", 45.8992, 6.1294, 130_000, true),
    city("Perpignan", 42.6887, 2.8948, 121_000, true),
    city("Metz", 49.1193, 6.1757, 118_000, true),
    city("Besançon", 47.2378, 6.0241, 118_000, true),
    city("Orléans", 47.9029, 1.9093, 117_000, true),
    city("Rouen", 49.4432, 1.0993, 114_000, true),
    city("Mulhouse", 47.7508, 7.3359, 109_000, true),
    city("Caen", 49.1829, -0.3707, 106_000, true),
    city("Nancy", 48.6921, 6.1844, 105_000, true),
    city("Avignon", 43.9493, 4.8055, 92_000, true),
    city("Poitiers", 46.5802, 0.3404, 90_000, true),
    city("Dunkerque", 51.0343, 2.3768, 87_000, true),
    city("Versailles", 48.8014, 2.1301, 85_000, false),
    city("Cherbourg", 49.6337, -1.6221, 79_000, true),
    city("Béziers", 43.3441, 3.2150, 78_000, false),
    city("Pau", 43.2951, -0.3708, 77_000, true),
    city("La Rochelle", 46.1603, -1.1511, 77_000, true),
    city("Cannes", 43.5528, 7.0174, 74_000, true),
    city("Saint-Nazaire", 47.2733, -2.2138, 71_000, false),
    city("Ajaccio", 41.9192, 8.7386, 71_000, true),
    city("Colmar", 48.0794, 7.3585, 69_000, true),
    city("Évry-Courcouronnes", 48.6290, 2.4417, 69_000, false),
    city("Calais", 50.9513, 1.8587, 67_000, true),
    city("Cergy", 49.0364, 2.0760, 66_000, false),
    city("Bourges", 47.0810, 2.3988, 64_000, true),
    city("Valence", 44.9334, 4.8924, 64_000, false),
    city("Quimper", 47.9960, -4.0970, 63_000, true),
    city("Troyes", 48.2973, 4.0744, 62_000, true),
    city("Montauban", 44.0175, 1.3550, 61_000, false),
    city("Chambéry", 45.5646, 5.9178, 59_000, true),
    city("Niort", 46.3240, -0.4588, 59_000, false),
    city("Lorient", 47.7482, -3.3702, 57_000, true),
    city("Narbonne", 43.1837, 3.0042, 56_000, false),
    city("Beauvais", 49.4295, 2.0807, 56_000, false),
    city("La Roche-sur-Yon", 46.6705, -1.4260, 55_000, false),
    city("Saint-Quentin", 49.8472, 3.2876, 54_000, false),
    city("Vannes", 47.6587, -2.7603, 53_000, false),
    city("Bayonne", 43.4933, -1.4748, 52_000, true),
    city("Arles", 43.6768, 4.6278, 52_000, false),
    city("Laval", 48.0698, -0.7710, 50_000, false),
    city("Albi", 43.9298, 2.1480, 49_000, true),
    city("Bastia", 42.6977, 9.4509, 48_000, true),
    city("Évreux", 49.0241, 1.1508, 47_000, false),
    city("Carcassonne", 43.2130, 2.3491, 47_000, true),
    city("Belfort", 47.6397, 6.8639, 46_000, true),
    city("Blois", 47.5861, 1.3359, 46_000, false),
    city("Charleville-Mézières", 49.7733, 4.7208, 46_000, true),
    city("Saint-Malo", 48.6493, -2.0257, 46_000, true),
    city("Brive-la-Gaillarde", 45.1585, 1.5329, 46_000, false),
    city("Saint-Brieuc", 48.5136, -2.7602, 45_000, false),
    city("Châlons-en-Champagne", 48.9566, 4.3631, 44_000, false),
    city("Sète", 43.4028, 3.6970, 44_000, false),
    city("Châteauroux", 46.8103, 1.6910, 43_000, false),
    city("Angoulême", 45.6484, 0.1560, 42_000, true),
    city("Bourg-en-Bresse", 46.2056, 5.2256, 41_000, false),
    city("Gap", 44.5594, 6.0786, 41_000, false),
    city("Arras", 50.2910, 2.7775, 41_000, false),
    city("Boulogne-sur-Mer", 50.7264, 1.6147, 40_000, false),
    city("Compiègne", 49.4178, 2.8261, 40_000, false),
    city("Tarbes", 43.2328, 0.0781, 40_000, false),
    city("Melun", 48.5396, 2.6607, 40_000, false),
    city("Chartres", 48.4469, 1.4892, 38_000, false),
    city("Auxerre", 47.7982, 3.5733, 34_000, false),
    city("Nevers", 46.9896, 3.1590, 33_000, false),
    city("Mâcon", 46.3069, 4.8287, 33_000, false),
    city("Agen", 44.2033, 0.6171, 33_000, false),
    city("Épinal", 48.1724, 6.4495, 32_000, false),
    city("Mont-de-Marsan", 43.8903, -0.5007, 31_000, false),
    city("Lens", 50.4292, 2.8328, 31_000, false),
    city("Dieppe", 49.9229, 1.0770, 29_000, false),
    city("Périgueux", 45.1846, 0.7214, 29_000, false),
    city("Aurillac", 44.9264, 2.4400, 26_000, false),
    city("Alençon", 48.4311, 0.0945, 26_000, false),
    city("Laon", 49.5640, 3.6200, 25_000, false),
    city("Vichy", 46.1278, 3.4260, 25_000, false),
    city("Rodez", 44.3506, 2.5748, 24_000, false),
    city("Chaumont", 48.1113, 5.1390, 22_000, false),
    city("Auch", 43.6460, 0.5855, 21_000, false),
    city("Cahors", 44.4475, 1.4414, 20_000, false),
    city("Saint-Lô", 49.1156, -1.0910, 20_000, false),
    city("Moulins", 46.5667, 3.3333, 19_000, false),
    city("Le Puy-en-Velay", 45.0431, 3.8856, 18_000, false),
    city("Lons-le-Saunier", 46.6754, 5.5539, 17_000, false),
    city("Digne-les-Bains", 44.0921, 6.2354, 16_000, false),
    city("Bar-le-Duc", 48.7728, 5.1608, 15_000, false),
    city("Vesoul", 47.6197, 6.1550, 15_000, false),
    city("Tulle", 45.2655, 1.7707, 15_000, false),
    city("Guéret", 46.1706, 1.8706, 13_000, false),
    city("Lourdes", 43.0942, -0.0455, 13_000, true),
    city("Mende", 44.5180, 3.5003, 12_000, false),
    city("Porto-Vecchio", 41.5910, 9.2797, 12_000, false),
    city("Foix", 42.9650, 1.6050, 10_000, false),
    city("Privas", 44.7353, 4.5991, 8_000, false),
    city("Calvi", 42.5667, 8.7570, 5_000, false),
];

/// Exact-name lookup for tests.
#[cfg(test)]
pub(crate) fn by_name(name: &str) -> &'static City {
    CITIES
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("unknown city {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::distance_km;
    use std::collections::HashSet;

    #[test]
    fn dataset_size() {
        assert!(CITIES.len() >= 100, "only {} cities", CITIES.len());
        assert!(CITIES.len() <= 130, "{} cities", CITIES.len());
        let refs = CITIES.iter().filter(|c| c.reference).count();
        assert!(refs >= 50, "only {refs} reference cities");
    }

    #[test]
    fn all_cities_inside_metropolitan_bbox() {
        for c in CITIES {
            assert!((41.3..=51.1).contains(&c.lat), "{}: lat {}", c.name, c.lat);
            assert!((-5.2..=9.6).contains(&c.lon), "{}: lon {}", c.name, c.lon);
        }
    }

    #[test]
    fn names_are_unique_and_non_empty() {
        let mut seen = HashSet::new();
        for c in CITIES {
            assert!(!c.name.trim().is_empty());
            assert!(seen.insert(c.name), "duplicate name {}", c.name);
        }
    }

    #[test]
    fn no_two_cities_closer_than_10_km() {
        for (i, a) in CITIES.iter().enumerate() {
            for b in &CITIES[i + 1..] {
                let d = distance_km(a, b);
                assert!(d >= 10.0, "{} and {} only {d:.1} km apart", a.name, b.name);
            }
        }
    }

    #[test]
    fn populations_are_plausible() {
        for c in CITIES {
            assert!((1_000..3_000_000).contains(&c.population), "{}", c.name);
        }
    }

    #[test]
    fn famous_cities_are_present_and_spelled_with_accents() {
        for name in [
            "Paris",
            "Besançon",
            "Saint-Étienne",
            "Nîmes",
            "Orléans",
            "Châteauroux",
            "Ajaccio",
            "Bastia",
            "Cherbourg",
            "Perpignan",
        ] {
            by_name(name);
        }
        for suburb in [
            "Villeurbanne",
            "Boulogne-Billancourt",
            "Montreuil",
            "Saint-Denis",
        ] {
            assert!(
                CITIES.iter().all(|c| c.name != suburb),
                "{suburb} is a suburb"
            );
        }
    }
}
