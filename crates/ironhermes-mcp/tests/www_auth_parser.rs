//! Unit and property tests for WWW-Authenticate header parsing (B-2, D-07).
//!
//! Validates that `parse_www_authenticate` correctly classifies the two OAuth 401
//! error classes: `invalid_token` (→ bounded 1 retry in 44-05) and
//! `insufficient_scope` (→ permanent fail in 44-05).
//!
//! Run with: cargo test -p ironhermes-mcp --test www_auth_parser

use ironhermes_mcp::{OAuthChallengeError, parse_www_authenticate};

// -------------------------------------------------------------------------
// Core behavior: error classification
// -------------------------------------------------------------------------

#[test]
fn parses_invalid_token_error() {
    let header = r#"Bearer error="invalid_token""#;
    let result = parse_www_authenticate(header);
    assert!(
        matches!(result, OAuthChallengeError::InvalidToken),
        "Bearer error=invalid_token must parse to InvalidToken, got {result:?}"
    );
}

#[test]
fn parses_insufficient_scope_error() {
    let header = r#"Bearer error="insufficient_scope""#;
    let result = parse_www_authenticate(header);
    assert!(
        matches!(result, OAuthChallengeError::InsufficientScope),
        "Bearer error=insufficient_scope must parse to InsufficientScope, got {result:?}"
    );
}

#[test]
fn parses_missing_error_field_as_none() {
    // A header with no error= field must not false-positive as InvalidToken (B-2)
    let header = r#"Bearer realm="example""#;
    let result = parse_www_authenticate(header);
    assert!(
        matches!(result, OAuthChallengeError::None),
        "Missing error field must parse to None, got {result:?}"
    );
}

#[test]
fn parses_extra_params_are_tolerated() {
    // Extra params (realm, resource_metadata) must be ignored — B-2
    let header = r#"Bearer realm="https://api.example.com/", error="invalid_token", resource_metadata="https://api.example.com/.well-known/oauth-protected-resource""#;
    let result = parse_www_authenticate(header);
    assert!(
        matches!(result, OAuthChallengeError::InvalidToken),
        "Extra params should be ignored; error=invalid_token must still be InvalidToken, got {result:?}"
    );
}

// -------------------------------------------------------------------------
// Property-style tests: edge cases and other error values
// -------------------------------------------------------------------------

#[test]
fn parses_invalid_request_maps_to_other() {
    let header = r#"Bearer error="invalid_request""#;
    let result = parse_www_authenticate(header);
    match &result {
        OAuthChallengeError::Other(val) => {
            assert_eq!(val, "invalid_request");
        }
        _ => panic!("invalid_request must map to Other(...), got {result:?}"),
    }
}

#[test]
fn parses_case_insensitive_invalid_token() {
    // RFC 6750 error values are case-sensitive in practice but we normalise
    let header = r#"Bearer error="INVALID_TOKEN""#;
    let result = parse_www_authenticate(header);
    assert!(
        matches!(result, OAuthChallengeError::InvalidToken),
        "Case-insensitive match: INVALID_TOKEN must parse to InvalidToken, got {result:?}"
    );
}

#[test]
fn parses_case_insensitive_insufficient_scope() {
    let header = r#"Bearer error="INSUFFICIENT_SCOPE""#;
    let result = parse_www_authenticate(header);
    assert!(
        matches!(result, OAuthChallengeError::InsufficientScope),
        "Case-insensitive match: INSUFFICIENT_SCOPE must parse to InsufficientScope, got {result:?}"
    );
}

#[test]
fn empty_header_parses_to_none() {
    let result = parse_www_authenticate("");
    assert!(
        matches!(result, OAuthChallengeError::None),
        "Empty header must parse to None, got {result:?}"
    );
}

#[test]
fn non_bearer_scheme_parses_to_none() {
    // Digest challenge — not a Bearer challenge, no error field
    let header = r#"Digest realm="example.com", nonce="abc123""#;
    let result = parse_www_authenticate(header);
    assert!(
        matches!(result, OAuthChallengeError::None),
        "Non-Bearer scheme without error= must parse to None, got {result:?}"
    );
}
