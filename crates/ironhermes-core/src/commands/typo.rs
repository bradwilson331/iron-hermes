//! Phase 22.3 D-10 / UI-SPEC TYPO-1..6:
//! Levenshtein-distance-2 typo suggester for unknown slash commands and
//! unknown `/agents` subcommands. Pure function, no IO, no new crate
//! dependency (Phase 21 D-18 forbids `strsim`).
//!
//! Consumers (see Plan 22.3-05):
//!   - `cmd_agents` `Some(other)` arm in `commands::handlers`
//!   - `dispatch_command` `ResolveResult::NotFound` arm in `tui::commands`

/// Returns a suggestion string if any candidate is within Levenshtein
/// distance 2 of `input`. Case-insensitive comparison.
///
/// Returns `None` when:
///   - 0 candidates match within threshold
///   - more than 3 candidates match within threshold (avoids noisy output)
///
/// The returned suffix is intended to be appended to an error message.
/// Format follows UI-SPEC TYPO-2 verbatim:
///   - 1 match:    `"— did you mean /{X}?"`
///   - 2-3 match:  `"— did you mean one of: /{A}, /{B}?"` (sorted by ascending distance)
///
/// Note the em-dash (U+2014) prefix and the leading slash on each candidate.
pub fn suggest_typo(input: &str, candidates: &[&str]) -> Option<String> {
    let input_lc = input.to_lowercase();
    let mut matches: Vec<(&str, usize)> = candidates
        .iter()
        .filter_map(|&c| {
            let d = levenshtein(&input_lc, &c.to_lowercase());
            if d <= 2 { Some((c, d)) } else { None }
        })
        .collect();
    matches.sort_by_key(|(_, d)| *d);
    match matches.len() {
        0 => None,
        1 => Some(format!("— did you mean /{}?", matches[0].0)),
        2 | 3 => {
            let names: Vec<String> = matches.iter().map(|(c, _)| format!("/{}", c)).collect();
            Some(format!("— did you mean one of: {}?", names.join(", ")))
        }
        _ => None, // > 3 matches: too noisy per UI-SPEC TYPO-2
    }
}

/// Standard O(n*m) DP-matrix Levenshtein distance. Operates on Unicode
/// scalar values (chars), not bytes — multi-byte UTF-8 input compares
/// at the character level.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 0..=m {
        dp[i][0] = i;
    }
    for j in 0..=n {
        dp[0][j] = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1]
            } else {
                1 + dp[i - 1][j].min(dp[i][j - 1]).min(dp[i - 1][j - 1])
            };
        }
    }
    dp[m][n]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typo_single_candidate_logs() {
        assert_eq!(
            suggest_typo("log", &["list", "kill", "logs"]),
            Some("— did you mean /logs?".to_string())
        );
    }

    #[test]
    fn typo_no_candidate() {
        assert_eq!(suggest_typo("zzz", &["list", "kill", "logs"]), None);
    }

    #[test]
    fn typo_two_candidates_sorted_by_distance() {
        // "agen" → "agent" (distance 1), "agents" (distance 2). "queue" excluded.
        // Closer match listed first.
        assert_eq!(
            suggest_typo("agen", &["agents", "agent", "queue"]),
            Some("— did you mean one of: /agent, /agents?".to_string())
        );
    }

    #[test]
    fn typo_three_candidates_returns_some() {
        // All three within distance 2 of "ax".
        let result = suggest_typo("ax", &["a", "ab", "axe"]);
        assert!(
            result.is_some(),
            "3 matches within threshold should return Some"
        );
        let s = result.unwrap();
        assert!(s.contains("/a"), "should include /a");
        assert!(s.contains("/ab"), "should include /ab");
        assert!(s.contains("/axe"), "should include /axe");
        assert!(
            s.starts_with("— did you mean one of:"),
            "multi-candidate prefix"
        );
    }

    #[test]
    fn typo_more_than_three_returns_none() {
        // 4 matches all within distance ≤ 2 of "a" — should suppress per UI-SPEC TYPO-2.
        assert_eq!(
            suggest_typo("a", &["a", "ab", "ac", "ad"]),
            None,
            "more than 3 matches should return None to avoid noisy output"
        );
    }

    #[test]
    fn typo_case_insensitive() {
        assert_eq!(
            suggest_typo("LOG", &["logs"]),
            Some("— did you mean /logs?".to_string())
        );
    }

    #[test]
    fn typo_distance_three_excluded() {
        // "xyz" vs "abc" — distance 3, above threshold.
        assert_eq!(suggest_typo("xyz", &["abc"]), None);
    }

    #[test]
    fn typo_exact_match_returns_some_one() {
        // Distance 0 is ≤ 2, so an exact match still suggests itself.
        // (Caller is expected to short-circuit before invoking suggest_typo for known commands.)
        assert_eq!(
            suggest_typo("logs", &["logs", "list"]),
            Some("— did you mean /logs?".to_string())
        );
    }

    #[test]
    fn levenshtein_basic() {
        // Classic kitten→sitting example.
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn levenshtein_empty_strings() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", "abc"), 3);
    }
}
