//! Name normalization and autocomplete over the embedded dataset.

use crate::cities::{CITIES, City};

/// Lowercase, strip French diacritics (`œ`→`oe`, `æ`→`ae`), turn every non-alphanumeric
/// run (`-`, `'`, spaces, …) into a single space, and expand the words `st`/`ste` to
/// `saint`/`sainte`.
pub fn normalize(s: &str) -> String {
    let mut folded = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => folded.push('a'),
            'ç' => folded.push('c'),
            'è' | 'é' | 'ê' | 'ë' => folded.push('e'),
            'ì' | 'í' | 'î' | 'ï' => folded.push('i'),
            'ñ' => folded.push('n'),
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => folded.push('o'),
            'ù' | 'ú' | 'û' | 'ü' => folded.push('u'),
            'ý' | 'ÿ' => folded.push('y'),
            'œ' => folded.push_str("oe"),
            'æ' => folded.push_str("ae"),
            c if c.is_alphanumeric() => folded.push(c),
            _ => folded.push(' '),
        }
    }
    folded
        .split_whitespace()
        .map(|w| match w {
            "st" => "saint",
            "ste" => "sainte",
            w => w,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Autocomplete: cities whose normalized name starts with the query first, then those
/// with a word starting with it, then substring matches; ties by population (desc),
/// then name. An empty (or all-punctuation) query returns nothing.
pub fn search(query: &str, limit: usize) -> Vec<&'static City> {
    let q = normalize(query);
    if q.is_empty() {
        return Vec::new();
    }
    let word_q = format!(" {q}");
    let mut hits: Vec<(u8, &'static City)> = CITIES
        .iter()
        .filter_map(|c| {
            let name = normalize(c.name);
            let tier = if name.starts_with(&q) {
                0
            } else if name.contains(&word_q) {
                1
            } else if name.contains(&q) {
                2
            } else {
                return None;
            };
            Some((tier, c))
        })
        .collect();
    hits.sort_by(|(ta, a), (tb, b)| {
        ta.cmp(tb)
            .then(b.population.cmp(&a.population))
            .then(a.name.cmp(b.name))
    });
    hits.into_iter().take(limit).map(|(_, c)| c).collect()
}

/// City whose normalized name equals the normalized `name`.
pub fn find_exact(name: &str) -> Option<&'static City> {
    let n = normalize(name);
    CITIES.iter().find(|c| normalize(c.name) == n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(query: &str) -> &'static str {
        search(query, 5)
            .first()
            .unwrap_or_else(|| panic!("no result for {query:?}"))
            .name
    }

    #[test]
    fn normalize_folds_case_accents_and_separators() {
        assert_eq!(normalize("Besançon"), "besancon");
        assert_eq!(normalize("Saint-Étienne"), "saint etienne");
        assert_eq!(normalize("NÎMES"), "nimes");
        assert_eq!(normalize("Châlons-en-Champagne"), "chalons en champagne");
        assert_eq!(normalize("L'Haÿ  les-Roses"), "l hay les roses");
        assert_eq!(normalize("Cœur d’Alène Æsir"), "coeur d alene aesir");
        assert_eq!(normalize("  Le   Puy-en-Velay "), "le puy en velay");
        assert_eq!(normalize("St Etienne"), "saint etienne");
        assert_eq!(normalize("Ste. Maxime"), "sainte maxime");
        assert_eq!(normalize("Stras"), "stras", "only whole words are expanded");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize(" - ' "), "");
    }

    #[test]
    fn search_accent_and_case_insensitive() {
        assert_eq!(first("besancon"), "Besançon");
        assert_eq!(first("st etienne"), "Saint-Étienne");
        assert_eq!(first("NIMES"), "Nîmes");
        assert_eq!(first("Orléans"), "Orléans");
    }

    #[test]
    fn search_ties_broken_by_population() {
        assert_eq!(first("pa"), "Paris");
        let saints: Vec<_> = search("saint", 3).iter().map(|c| c.name).collect();
        assert_eq!(saints, ["Saint-Étienne", "Saint-Nazaire", "Saint-Quentin"]);
    }

    #[test]
    fn search_empty_query_or_zero_limit_is_empty() {
        assert!(search("", 5).is_empty());
        assert!(search("   ", 5).is_empty());
        assert!(search("---", 5).is_empty());
        assert!(search("paris", 0).is_empty());
        assert!(search("zzzz", 5).is_empty());
    }

    #[test]
    fn search_ranks_prefix_then_word_prefix_then_substring() {
        // "le": name prefix (Le Havre, Le Mans, Le Puy…) < word prefix (Bar-le-Duc) <
        // mid-word substring (Arles).
        let names: Vec<_> = search("le", 200).iter().map(|c| c.name).collect();
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert_eq!(names[0], "Le Havre");
        assert!(
            pos("Le Puy-en-Velay") < pos("Bar-le-Duc"),
            "name prefix before word prefix"
        );
        assert!(
            pos("Bar-le-Duc") < pos("Arles"),
            "word prefix before substring"
        );
        // A word prefix after a separator is found even though the name starts differently.
        assert_eq!(first("etienne"), "Saint-Étienne");
        assert_eq!(first("mezieres"), "Charleville-Mézières");
    }

    #[test]
    fn search_respects_limit() {
        assert_eq!(search("a", 3).len(), 3);
        assert!(search("saint", 100).len() >= 5);
    }

    #[test]
    fn find_exact_is_normalized_equality() {
        assert_eq!(
            find_exact("saint-etienne").map(|c| c.name),
            Some("Saint-Étienne")
        );
        assert_eq!(
            find_exact("  ST  ETIENNE ").map(|c| c.name),
            Some("Saint-Étienne")
        );
        assert_eq!(find_exact("PARIS").map(|c| c.name), Some("Paris"));
        assert_eq!(find_exact("Pari"), None);
        assert_eq!(find_exact(""), None);
    }

    #[test]
    fn every_city_is_found_by_its_own_name() {
        for c in CITIES {
            assert_eq!(find_exact(c.name).map(|x| x.name), Some(c.name));
            assert_eq!(first(c.name), c.name, "searching its full name");
        }
    }
}
