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

/// Plan 25.2-16 (D-19): transform sibling of [`contains_secret`] — preserves URL structure
/// while replacing each secret-valued query parameter's value with `***`. Single source of
/// truth: same `SECRET_URL_PATTERNS` const + same `extras` list as the predicate.
///
/// Behavior contract (locked by 8 unit tests in this module):
/// - Clean URLs are returned bytewise-equal (fast-path via [`contains_secret`] gate).
/// - Secret-keyed values are replaced with `***`; the parameter NAME and surrounding URL
///   structure (host, path, other params) are preserved.
/// - Match is case-insensitive; the original case of the parameter name is preserved
///   in the output.
/// - The decoded URL form is what's emitted on dirty URLs — percent-encoding fidelity
///   of the redacted span is traded for redaction completeness. Cleartext secrets must
///   never leak; preserving `%20` in the output is cosmetic by comparison.
pub fn redact_secrets_in_url(url: &str, extras: &[String]) -> String {
    // Fast-path: clean URLs are returned untouched (bytewise-equal).
    if !contains_secret(url, extras) {
        return url.to_string();
    }

    // Decode once. We emit the decoded form because percent-encoded keys (e.g.
    // `?token%3Dabc`) can only be detected after decode — and we must not let
    // an undetected percent-encoded form silently round-trip the cleartext value
    // back to the caller.
    let decoded = percent_decode_lossy(url);
    let decoded_lower = decoded.to_lowercase();
    let bytes = decoded.as_bytes();

    // Build the combined pattern list once (lowercase), so we can scan in a single pass.
    // SECRET_URL_PATTERNS is already lowercase by construction.
    let mut all_patterns: Vec<String> = SECRET_URL_PATTERNS.iter().map(|p| (*p).to_string()).collect();
    for extra in extras {
        all_patterns.push(extra.to_lowercase());
    }

    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        // Try to match any pattern beginning at byte position `i`.
        let remaining_lower = &decoded_lower[i..];
        let matched = all_patterns
            .iter()
            .find(|p| remaining_lower.starts_with(p.as_str()));

        if let Some(p) = matched {
            // Copy through the matched pattern (preserving original case of the key).
            let plen = p.len();
            out.extend_from_slice(&bytes[i..i + plen]);
            i += plen;
            // Now we're at the start of the value. Scan to next delimiter.
            // Delimiters that end a query value: `&`, `#`. End-of-string also stops.
            let value_start = i;
            while i < bytes.len() && bytes[i] != b'&' && bytes[i] != b'#' {
                i += 1;
            }
            // Only emit `***` if there was actually a value to redact (avoid empty `auth=`
            // becoming `auth=***` which would be a false positive for `?auth=` with no value).
            if i > value_start {
                out.extend_from_slice(b"***");
            }
            // Loop continues; delimiter byte (or EOS) handled by outer copy below.
            continue;
        }

        // No pattern at this position; copy one byte and advance.
        out.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
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

    // ── Plan 25.2-16: redact_secrets_in_url tests (UAT Issue 9 R2 fix) ─────────

    #[test]
    fn redact_simple_token_query_param() {
        let out = redact_secrets_in_url("https://example.com/?token=sk-fake-abc123", &[]);
        assert_eq!(
            out, "https://example.com/?token=***",
            "simple token redaction must preserve URL structure: {}",
            out
        );
    }

    #[test]
    fn redact_handles_multiple_secrets_in_one_url() {
        let out = redact_secrets_in_url(
            "https://example.com/?token=abc&api_key=def&safe=ok",
            &[],
        );
        assert_eq!(
            out, "https://example.com/?token=***&api_key=***&safe=ok",
            "multi-secret redaction must only touch matched values: {}",
            out
        );
    }

    #[test]
    fn redact_case_insensitive_key_match() {
        let out = redact_secrets_in_url("https://example.com/?TOKEN=ABC", &[]);
        assert_eq!(
            out, "https://example.com/?TOKEN=***",
            "case-insensitive key match must preserve key case in output: {}",
            out
        );
    }

    #[test]
    fn redact_handles_percent_encoded_param_name() {
        // ?token%3Dabc — the `=` is percent-encoded. Must still redact the `abc` value.
        let out = redact_secrets_in_url("https://example.com/?token%3Dabc", &[]);
        assert!(
            !out.contains("abc"),
            "redacted URL must NOT contain literal secret value 'abc': {}",
            out
        );
        assert!(
            out.to_lowercase().contains("token"),
            "redacted URL must still contain the parameter name 'token': {}",
            out
        );
    }

    #[test]
    fn redact_respects_extra_patterns() {
        let extras = vec!["x_custom_secret=".to_string()];
        let out = redact_secrets_in_url("https://example.com/?x_custom_secret=foo", &extras);
        assert!(
            out.contains("x_custom_secret=***"),
            "operator extras must trigger redaction: {}",
            out
        );
        assert!(
            !out.contains("foo"),
            "operator-matched secret value must be removed: {}",
            out
        );
    }

    #[test]
    fn redact_returns_input_unchanged_on_clean_url() {
        let clean1 = "https://example.com/article?id=1234";
        assert_eq!(
            redact_secrets_in_url(clean1, &[]),
            clean1,
            "clean URL with id= must be bytewise-equal"
        );

        let clean2 = "https://arxiv.org/abs/2401.12345.pdf";
        assert_eq!(
            redact_secrets_in_url(clean2, &[]),
            clean2,
            "clean arxiv URL must be bytewise-equal"
        );
    }

    #[test]
    fn redact_bearer_value_in_query_string() {
        // `Bearer ` (with space) is a value-bearing pattern after percent-decode.
        // Acceptable to emit either `Bearer%20***` or `Bearer ***` — gate is just that
        // the literal `sk-abc` value MUST NOT remain.
        let out = redact_secrets_in_url("https://example.com/?h=Bearer%20sk-abc", &[]);
        assert!(
            !out.contains("sk-abc"),
            "redacted URL must NOT contain bearer value 'sk-abc': {}",
            out
        );
    }

    #[test]
    fn secret_url_patterns_const_count_unchanged() {
        // Plan 16 must NOT modify the const list — only add a new function.
        assert_eq!(
            SECRET_URL_PATTERNS.len(),
            9,
            "Plan 16 forbids modifying SECRET_URL_PATTERNS — count must stay at 9"
        );
    }
}
