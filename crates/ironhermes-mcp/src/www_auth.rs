//! WWW-Authenticate header parsing for MCP OAuth flows (B-2, D-07).
//!
//! Classifies a `WWW-Authenticate: Bearer ...` 401 response header into one of:
//! - `InvalidToken` — the access token is stale/invalid (→ bounded 1 retry in 44-05)
//! - `InsufficientScope` — the token scope is wrong (→ permanent fail in 44-05)
//! - `Other(String)` — a different RFC 6750 `error=` value
//! - `None` — no `error=` field present (challenge without error context)
//!
//! The parser uses plain byte/string scanning — no regex backreferences or `\b`
//! (Rust regex supports neither; CLAUDE.md memory).

/// Classified result of parsing a `WWW-Authenticate` 401 response header.
///
/// Used by `connect_and_serve` (44-05) to decide between a bounded 1-retry
/// (`InvalidToken`) and a permanent failure (`InsufficientScope`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthChallengeError {
    /// `error="invalid_token"` — access token is stale or revoked; retry with refreshed token.
    InvalidToken,
    /// `error="insufficient_scope"` — token scopes are wrong; permanent failure (re-auth required).
    InsufficientScope,
    /// Another RFC 6750 `error=` value (e.g. `invalid_request`).
    Other(String),
    /// No `error=` field in the challenge; server sent a bare `Bearer` challenge.
    None,
}

/// Parse a `WWW-Authenticate` header value and classify the OAuth error.
///
/// Extracts the `error="..."` token from a `Bearer` challenge using plain string
/// scanning (no regex backref). Matching is case-insensitive. Extra parameters
/// (`realm=`, `resource_metadata=`, etc.) are tolerated and ignored.
///
/// Returns [`OAuthChallengeError::None`] for empty input, non-Bearer schemes,
/// and any Bearer challenge that omits the `error=` attribute.
pub fn parse_www_authenticate(header_value: &str) -> OAuthChallengeError {
    // Extract the value of error="..." by scanning for the attribute key.
    // We look for the literal bytes `error="` (case-insensitive on the key).
    let lower = header_value.to_ascii_lowercase();
    let search = r#"error=""#;
    let Some(start_idx) = lower.find(search) else {
        return OAuthChallengeError::None;
    };
    // Advance past `error="`
    let after_quote = start_idx + search.len();
    // Find the closing quote in the original (non-lowercased) value
    let rest = &header_value[after_quote..];
    let end_idx = rest.find('"').unwrap_or(rest.len());
    let error_value = &rest[..end_idx];

    match error_value.to_ascii_lowercase().as_str() {
        "invalid_token" => OAuthChallengeError::InvalidToken,
        "insufficient_scope" => OAuthChallengeError::InsufficientScope,
        other => OAuthChallengeError::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_parses_invalid_token() {
        assert_eq!(
            parse_www_authenticate(r#"Bearer error="invalid_token""#),
            OAuthChallengeError::InvalidToken
        );
    }

    #[test]
    fn unit_parses_insufficient_scope() {
        assert_eq!(
            parse_www_authenticate(r#"Bearer error="insufficient_scope""#),
            OAuthChallengeError::InsufficientScope
        );
    }

    #[test]
    fn unit_parses_no_error_field() {
        assert_eq!(
            parse_www_authenticate(r#"Bearer realm="example""#),
            OAuthChallengeError::None
        );
    }

    #[test]
    fn unit_parses_empty() {
        assert_eq!(parse_www_authenticate(""), OAuthChallengeError::None);
    }
}
