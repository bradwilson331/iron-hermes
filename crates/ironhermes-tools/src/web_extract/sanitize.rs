//! Phase 25.2 D-08 + D-19: content + URL sanitizers.
//!
//! - [`strip_base64_images`] removes `data:image/...;base64,...` payloads from extracted
//!   Markdown to prevent token bloat and log-flooding (D-08).
//! - [`contains_secret`] checks a URL for known secret-bearing query params before any
//!   backend dispatch (D-19). Operator can extend the const pattern list via
//!   `config.extract.redact_url_patterns`.
//!
//! The `regex` crate uses an RE2 NFA engine (no backtracking) so the base64 pattern is
//! safe against adversarial input — see RESEARCH.md Pitfall 4 for analysis.

use std::sync::OnceLock;
use regex::Regex;

/// D-08 base64 image data-URL pattern. Matches `data:image/<mime>;base64,<payload>`.
/// Bounded character classes only — no nested quantifiers, no catastrophic backtracking risk
/// even with the regex crate's NFA backend.
const BASE64_PATTERN: &str = r"data:image/[a-zA-Z0-9+.\-]+;base64,[A-Za-z0-9+/=\n]+";

/// D-08 replacement string written into Markdown in place of stripped images.
const STRIPPED_IMAGE_PLACEHOLDER: &str = "[image stripped]";

/// D-19 const list of secret-URL substrings checked case-insensitively before backend dispatch.
/// Operator can append more via `config.extract.redact_url_patterns`.
pub const SECRET_URL_PATTERNS: &[&str] = &[
    "token=",
    "api_key=",
    "api-key=",
    "password=",
    "secret=",
    "bearer ",         // matches "Bearer ABC" after lowercasing
    "auth=",
    "access_token=",
    "access-token=",
];

fn base64_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(BASE64_PATTERN).expect("D-08 base64 pattern compiles")
    })
}

/// D-08: strip `data:image/...;base64,...` URLs from a Markdown body, replacing each with
/// the literal `[image stripped]`. Single regex pass; UTF-8 safe via `regex::Regex::replace_all`.
pub fn strip_base64_images(content: &str) -> String {
    base64_regex().replace_all(content, STRIPPED_IMAGE_PLACEHOLDER).into_owned()
}

/// D-19: returns `true` if `url` contains any pattern from `SECRET_URL_PATTERNS` OR the
/// caller-supplied `extra_patterns` (operator's `config.extract.redact_url_patterns`).
/// Match is case-insensitive and operates on the percent-decoded URL when possible —
/// `?token%3Dfoo` (encoded) is rejected as well as `?token=foo`.
///
/// Uses a hand-rolled minimal percent-decoder (no `urlencoding` crate) — keeps
/// `ironhermes-tools` workspace deps frozen per the no-new-deps mandate (D-25).
/// Pattern lifted from Phase 21.8 Plan 02's percent-decoder helper.
pub fn contains_secret(url: &str, extra_patterns: &[String]) -> bool {
    // Lowercase the original URL for case-insensitive matching against the
    // raw (possibly encoded) form.
    let lower_orig = url.to_lowercase();
    // Also produce a percent-decoded variant so encoded patterns like `?token%3Dfoo`
    // (which percent-decodes to `?token=foo`) are caught.
    let lower_decoded = percent_decode_lossy(&lower_orig);
    // Check both raw and decoded forms against every pattern.
    let haystack = format!("{lower_orig} {lower_decoded}");

    SECRET_URL_PATTERNS
        .iter()
        .copied()
        .any(|p| haystack.contains(p))
        || extra_patterns
            .iter()
            .any(|p| haystack.contains(&p.to_lowercase()))
}

/// Minimal `%xx` percent-decoder. Ignores invalid escapes (passes them through verbatim).
/// This is intentionally simpler than the `percent-encoding` / `urlencoding` crate APIs —
/// we only need it for substring matching, not roundtrip-correct URL parsing.
/// Lifted from Phase 21.8 Plan 02's hand-rolled decoder helper (~15 lines).
fn percent_decode_lossy(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = (bytes[i + 1] as char).to_digit(16);
            let l = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_base64_replaces_simple_data_url() {
        let md = "Before ![](data:image/png;base64,iVBORw0KGgo=) After";
        let stripped = strip_base64_images(md);
        assert!(stripped.contains("[image stripped]"), "{}", stripped);
        assert!(!stripped.contains("base64,iVBOR"), "payload removed");
    }

    #[test]
    fn strip_base64_handles_multiple_and_newlines() {
        let md = "A ![](data:image/jpeg;base64,abc\ndef) B ![](data:image/svg+xml;base64,xyz=) C";
        let stripped = strip_base64_images(md);
        // Should have stripped both
        assert_eq!(stripped.matches("[image stripped]").count(), 2, "{}", stripped);
    }

    #[test]
    fn strip_base64_leaves_non_image_data_urls_untouched() {
        let md = "data:application/json;base64,eyJhIjoxfQ==";
        let stripped = strip_base64_images(md);
        assert_eq!(stripped, md, "non-image data URLs untouched");
    }

    #[test]
    fn contains_secret_matches_token_query_param() {
        assert!(contains_secret("https://example.com/x?token=abc123", &[]));
    }

    #[test]
    fn contains_secret_matches_each_pattern() {
        for p in &["token=", "api_key=", "api-key=", "password=", "secret=", "auth=", "access_token=", "access-token="] {
            let url = format!("https://example.com/?{p}foo");
            assert!(contains_secret(&url, &[]), "should detect {p} in {url}");
        }
    }

    #[test]
    fn contains_secret_matches_bearer_with_space() {
        assert!(contains_secret("https://example.com/?h=Bearer%20abc", &[]),
            "URL-decoded `Bearer ` (with space) must match");
    }

    #[test]
    fn contains_secret_case_insensitive() {
        assert!(contains_secret("https://example.com/?TOKEN=ABC", &[]));
        assert!(contains_secret("https://example.com/?Api_Key=Abc", &[]));
    }

    #[test]
    fn contains_secret_decodes_percent_encoded_param_name() {
        // ?token%3Dabc decodes to ?token=abc
        assert!(contains_secret("https://example.com/?token%3Dabc", &[]),
            "percent-encoded `token=` must be detected after decode");
    }

    #[test]
    fn contains_secret_respects_extra_patterns() {
        // Operator-supplied extension list
        let extras = vec!["x_custom_secret=".to_string()];
        assert!(contains_secret("https://example.com/?x_custom_secret=foo", &extras));
        assert!(!contains_secret("https://example.com/?safe=foo", &extras));
    }

    #[test]
    fn contains_secret_returns_false_for_clean_url() {
        assert!(!contains_secret("https://example.com/article?id=1234", &[]));
        assert!(!contains_secret("https://arxiv.org/abs/2401.12345.pdf", &[]));
    }

    #[test]
    fn secret_url_patterns_const_contains_required_entries() {
        for required in &["token=", "api_key=", "api-key=", "password=", "secret=", "bearer ", "auth=", "access_token=", "access-token="] {
            assert!(SECRET_URL_PATTERNS.contains(required),
                "SECRET_URL_PATTERNS missing required entry: {}", required);
        }
        assert_eq!(SECRET_URL_PATTERNS.len(), 9, "exactly 9 patterns expected");
    }
}
