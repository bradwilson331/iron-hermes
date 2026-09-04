//! PII/secret redaction for blackbox event metadata.
//!
//! `redact()` runs unconditionally on every blackbox event before it is
//! written to disk — see `recorder.rs`'s `writer_task`, where
//! `event.metadata = redact(event.metadata);` is the only call site. This
//! module therefore protects FUTURE records only. Blackbox files already on
//! operator disks — including `blackbox-2026-08-27.jsonl` in this phase's
//! own directory — keep whatever cleartext credentials they already
//! contain; this module cannot reach data already on disk. That file is
//! deleted (with key rotation, D-12) by 48.3-06 Task 3; the general
//! retention sweep for historical blackbox logs is tracked separately in
//! CONTEXT.md `<deferred>`.

use std::sync::OnceLock;

use sha2::Digest;

static PII_PATTERNS: OnceLock<Vec<regex::Regex>> = OnceLock::new();

fn pii_patterns() -> &'static Vec<regex::Regex> {
    PII_PATTERNS.get_or_init(|| {
        vec![
            // Card numbers: 12–19 consecutive digits
            regex::Regex::new(r"\b\d{12,19}\b").unwrap(),
            // Email addresses
            regex::Regex::new(r"[\w.\-+]+@[\w.\-]+\.\w{2,}").unwrap(),
            // API key / token / secret / password patterns
            regex::Regex::new(r"(?i)(api[_-]?key|token|secret|password)\s*[:=]\s*\S+").unwrap(),
            // D-10: bare `Bearer <token>` credentials — the shape that
            // survived this redactor during the 2026-08-27 Atomic Mail
            // incident (a live bearer token, including a uuid-shaped one,
            // had no adjacent `token`/`key`/`secret`/`password` label for
            // the pattern above to key on). Mirrors, without importing,
            // ironhermes_mcp::security::CREDENTIAL_PATTERN's `Bearer\s+\S+`
            // alternative: this crate is a dependency-free leaf (zero
            // `path = ` entries in its Cargo.toml) and must stay that way,
            // so a generic recording-infrastructure crate does not gain a
            // dependency on one specific subsystem crate. Keep the two
            // patterns aligned by hand if either changes.
            regex::Regex::new(r"(?i)bearer\s+\S+").unwrap(),
            // CR-01 (48.3 code review): `auth: <token>`. Phase 48.3's D-02
            // change made `McpServerConfig.auth` a LIVE Bearer shorthand, so a
            // raw credential can now sit under a bare `auth:` key — a shape the
            // labelled pattern above does not cover (`auth` is not one of its
            // alternatives) and the `Bearer` pattern does not cover either (the
            // shorthand value carries no `Bearer` prefix). Kept as its own
            // pattern rather than extending the labelled alternation, so the
            // existing `api_key|token|secret|password` behaviour is untouched.
            // `\b` prevents matching the tail of `oauth`/`oauth_provider`.
            regex::Regex::new(r"(?i)\bauth(?:oriz(?:ation|ed))?\s*[:=]\s*\S+").unwrap(),
        ]
    })
}

/// Recursively walk a `serde_json::Value`, replacing any string that matches
/// a PII pattern with `"[REDACTED]"`. Objects and arrays are walked in-place.
pub fn redact(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => serde_json::Value::String(redact_str(&s)),
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(redact).collect())
        }
        serde_json::Value::Object(map) => {
            let new_map = map.into_iter().map(|(k, v)| (k, redact(v))).collect();
            serde_json::Value::Object(new_map)
        }
        other => other,
    }
}

/// Apply all PII patterns to a plain string, replacing matches with `"[REDACTED]"`.
pub fn redact_str(s: &str) -> String {
    let mut result = s.to_string();
    for pattern in pii_patterns() {
        result = pattern.replace_all(&result, "[REDACTED]").into_owned();
    }
    result
}

/// Compute a stable SHA-256 hex hash of a `serde_json::Value`.
///
/// Keys in JSON objects are sorted recursively before serialization so that
/// `{"b":2,"a":1}` and `{"a":1,"b":2}` produce the same hash. This is the
/// canonical argument hash used in `tool_dispatched` events for dedup/lookup.
pub fn argument_hash(args: &serde_json::Value) -> String {
    let sorted = sort_keys(args);
    let canonical = serde_json::to_string(&sorted).unwrap_or_default();
    let hash = sha2::Sha256::digest(canonical.as_bytes());
    format!("{:x}", hash)
}

/// Recursively sort object keys in a JSON value (other types pass through).
fn sort_keys(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut sorted: Vec<(String, serde_json::Value)> =
                map.iter().map(|(k, v)| (k.clone(), sort_keys(v))).collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sort_keys).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every credential-shaped fixture below is ASSEMBLED AT RUNTIME from
    // parts (never written as a source literal). This file is tracked and
    // CI Gate 8's secret scanner scans every tracked file with the same
    // bearer-credential shape this module's pattern extension targets — a
    // literal fixture here would make this crate's own test file fail the
    // repository's secret scan it is meant to help pass.

    /// A bare opaque credential of `len` characters, built at runtime.
    fn assembled_opaque_token(len: usize) -> String {
        "a".repeat(len)
    }

    /// A uuid-shaped (8-4-4-4-12) credential with sequential hex digits,
    /// built at runtime — the exact shape that survived the redactor
    /// during the 2026-08-27 incident.
    fn assembled_uuid_shaped() -> String {
        let hex: Vec<char> = "0123456789abcdef".chars().collect();
        let seq: String = (0..32).map(|i| hex[i % hex.len()]).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &seq[0..8],
            &seq[8..12],
            &seq[12..16],
            &seq[16..20],
            &seq[20..32]
        )
    }

    #[test]
    fn redacts_bare_bearer_token() {
        let token = assembled_opaque_token(40);
        let input = format!("Authorization: Bearer {token}");
        let out = redact_str(&input);
        assert!(!out.contains(&token), "token leaked into output: {out}");
        assert!(out.contains("[REDACTED]"), "no redaction marker in: {out}");
    }

    #[test]
    fn redacts_uuid_shaped_bearer_token() {
        let token = assembled_uuid_shaped();
        let input = format!("Bearer {token}");
        let out = redact_str(&input);
        assert!(
            !out.contains(&token),
            "uuid-shaped token leaked into output: {out}"
        );
        assert!(out.contains("[REDACTED]"), "no redaction marker in: {out}");
    }

    /// CR-01 (48.3 code review): phase 48.3's D-02 change made
    /// `McpServerConfig.auth` a LIVE Bearer shorthand, so a raw credential can
    /// now sit under a bare `auth:` key with no `Bearer` prefix and no
    /// `token`/`key`/`secret`/`password` label. Neither pre-CR-01 pattern
    /// covered that shape.
    #[test]
    fn redacts_bare_auth_shorthand_credential() {
        let token = assembled_uuid_shaped();
        let input = format!("auth: {token}");
        let out = redact_str(&input);
        assert!(
            !out.contains(&token),
            "auth-shorthand credential leaked into output: {out}"
        );
        assert!(out.contains("[REDACTED]"), "no redaction marker in: {out}");
    }

    #[test]
    fn redacts_opaque_auth_shorthand_credential() {
        let token = assembled_opaque_token(40);
        let input = format!("auth = {token}");
        let out = redact_str(&input);
        assert!(
            !out.contains(&token),
            "auth-shorthand credential leaked into output: {out}"
        );
    }

    /// The `\b` in the CR-01 pattern must keep `oauth`-prefixed keys from
    /// matching — `oauth_provider` names a provider, never a credential, and
    /// redacting it would destroy useful diagnostic signal.
    #[test]
    fn does_not_redact_oauth_provider_key() {
        let out = redact_str("oauth_provider: cloudflare_api");
        assert_eq!(
            out, "oauth_provider: cloudflare_api",
            "oauth_provider must not be treated as a credential: {out}"
        );
    }

    #[test]
    fn redacts_bearer_inside_nested_json_metadata() {
        let token = assembled_opaque_token(40);
        let value = serde_json::json!({
            "cmd": {
                "args": ["curl", "-H", format!("Authorization: Bearer {token}")]
            }
        });
        let out = redact(value);
        let out_str = out.to_string();
        assert!(
            !out_str.contains(&token),
            "nested token leaked into output: {out_str}"
        );
        assert!(
            out_str.contains("[REDACTED]"),
            "no redaction marker in nested output: {out_str}"
        );
    }

    #[test]
    fn still_redacts_labelled_credential_assignments() {
        let secret = assembled_opaque_token(24);

        let out = redact_str(&format!("api_key={secret}"));
        assert!(!out.contains(&secret));
        assert!(out.contains("[REDACTED]"));

        let out2 = redact_str(&format!("token: {secret}"));
        assert!(!out2.contains(&secret));
        assert!(out2.contains("[REDACTED]"));

        let out3 = redact_str(&format!("secret={secret}"));
        assert!(!out3.contains(&secret));
        assert!(out3.contains("[REDACTED]"));

        let out4 = redact_str(&format!("password={secret}"));
        assert!(!out4.contains(&secret));
        assert!(out4.contains("[REDACTED]"));
    }

    #[test]
    fn still_redacts_email_and_long_digit_runs() {
        let email_out = redact_str("contact someone@example.com for access");
        assert!(!email_out.contains("someone@example.com"));
        assert!(email_out.contains("[REDACTED]"));

        let digits = "1".repeat(16);
        let digit_out = redact_str(&format!("card number {digits} on file"));
        assert!(!digit_out.contains(&digits));
        assert!(digit_out.contains("[REDACTED]"));
    }
}
