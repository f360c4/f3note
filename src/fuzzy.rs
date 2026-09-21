//! Subsequence matching for the Ctrl+P tab switcher.
//!
//! Deliberately small and dependency-free. The job is narrow: rank at most a
//! few hundred tab names against a query the user is typing one character at a
//! time. What makes a switcher feel right is not a clever algorithm but a
//! scoring rule that matches intuition — a match at the start of a word should
//! beat one in the middle, consecutive characters should beat scattered ones,
//! and the file name should beat the directory it sits in.

/// Score a candidate against a query. Higher is better; `None` means no match.
///
/// Matching is case-insensitive and requires the query to appear as a
/// subsequence, which is what lets `mn` find `main.rs`.
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    let c: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = candidate.chars().flat_map(|c| c.to_lowercase()).collect();
    // Lowercasing can change length for some scripts; fall back to a plain
    // containment test rather than indexing past the end.
    if lower.len() != c.len() {
        return candidate
            .to_lowercase()
            .contains(&query.to_lowercase())
            .then_some(1);
    }

    let mut total = 0i32;
    let mut qi = 0usize;
    let mut previous_index: Option<usize> = None;

    for (i, ch) in lower.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if *ch != q[qi] {
            continue;
        }

        let mut points = 10;

        // Consecutive characters are the strongest signal that the user is
        // typing a real prefix rather than hitting scattered letters.
        if previous_index == Some(i.saturating_sub(1)) && i > 0 {
            points += 15;
        }

        // Start of the string, or start of a word inside it.
        let at_boundary = i == 0
            || matches!(c[i - 1], '/' | '_' | '-' | '.' | ' ')
            || (c[i].is_uppercase() && c[i - 1].is_lowercase());
        if at_boundary {
            points += 12;
        }

        // Later characters are worth slightly less, so an early match wins a
        // tie. Capped so a long path does not score negatively overall.
        points -= (i as i32 / 4).min(8);

        total += points;
        previous_index = Some(i);
        qi += 1;
    }

    (qi == q.len()).then_some(total)
}

/// Rank candidates, best first, dropping those that do not match.
///
/// `name_of` extracts the part that should dominate the ranking — for a tab,
/// the file name rather than the whole path. A match there is worth more than
/// the same match somewhere in the directory.
pub fn rank<'a, T, F>(query: &str, items: &'a [T], name_of: F) -> Vec<(&'a T, i32)>
where
    F: Fn(&T) -> (String, String),
{
    let mut scored: Vec<(&T, i32)> = items
        .iter()
        .filter_map(|item| {
            let (name, full) = name_of(item);
            let by_name = score(query, &name).map(|s| s + 40);
            let by_full = score(query, &full);
            match (by_name, by_full) {
                (Some(a), Some(b)) => Some((item, a.max(b))),
                (Some(a), None) => Some((item, a)),
                (None, Some(b)) => Some((item, b)),
                (None, None) => None,
            }
        })
        .collect();
    // Stable sort, so equally scored items keep the order they came in — which
    // for the switcher is most-recently-used.
    scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_query_matches_everything() {
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn matches_a_subsequence_not_just_a_substring() {
        assert!(score("mn", "main.rs").is_some());
        assert!(score("mrs", "main.rs").is_some());
    }

    #[test]
    fn rejects_characters_that_are_not_there() {
        assert_eq!(score("xyz", "main.rs"), None);
        // Order matters: the characters must appear in sequence.
        assert_eq!(score("sr", "rs"), None);
    }

    #[test]
    fn is_case_insensitive() {
        assert!(score("MAIN", "main.rs").is_some());
        assert!(score("main", "MAIN.RS").is_some());
    }

    #[test]
    fn a_prefix_beats_a_scattered_match() {
        let prefix = score("mai", "main.rs").unwrap();
        let scattered = score("mai", "my-application-index").unwrap();
        assert!(prefix > scattered, "{prefix} should beat {scattered}");
    }

    #[test]
    fn a_word_boundary_beats_the_middle_of_a_word() {
        let boundary = score("t", "my_test").unwrap();
        let middle = score("t", "artist").unwrap();
        assert!(boundary > middle, "{boundary} should beat {middle}");
    }

    #[test]
    fn the_file_name_outranks_the_directory() {
        let items = vec![
            (
                "notes.txt".to_string(),
                "/home/u/config/notes.txt".to_string(),
            ),
            (
                "readme.md".to_string(),
                "/home/u/notes/readme.md".to_string(),
            ),
        ];
        let ranked = rank("notes", &items, |i| (i.0.clone(), i.1.clone()));
        assert_eq!(ranked[0].0 .0, "notes.txt");
    }

    #[test]
    fn ranking_drops_non_matches() {
        let items = vec![
            ("main.rs".to_string(), "src/main.rs".to_string()),
            ("banner.rs".to_string(), "src/ui/banner.rs".to_string()),
        ];
        let ranked = rank("zzz", &items, |i| (i.0.clone(), i.1.clone()));
        assert!(ranked.is_empty());
    }

    #[test]
    fn equal_scores_keep_their_incoming_order() {
        let items = vec![
            ("a.txt".to_string(), "a.txt".to_string()),
            ("a.txt".to_string(), "a.txt".to_string()),
        ];
        let ranked = rank("a", &items, |i| (i.0.clone(), i.1.clone()));
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].1, ranked[1].1);
    }
}
