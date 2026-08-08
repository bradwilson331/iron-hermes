//! Board slug validation (T-1 mitigation — path-traversal defence).
//!
//! `validate_board_slug` is the single entry point for all untrusted slug
//! input: CLI argv (`--board <slug>`, `boards create <slug>`), the
//! `IRONHERMES_KANBAN_BOARD` env var (via the resolver), and the
//! `~/.ironhermes/kanban/current` file.
//!
//! # Security ordering
//!
//! Path-traversal checks MUST run BEFORE downcasing.  A crafted input such as
//! `"../Etc/Passwd"` passes the alphanumeric body check after lowercasing
//! (`"../etc/passwd"` still contains `..`) — but the ordering guarantee means
//! the `..` and `/` checks fire first regardless of case.

/// Error variants returned by [`validate_board_slug`].
#[derive(Debug, thiserror::Error)]
pub enum BoardSlugError {
    #[error("board slug must not be empty")]
    Empty,
    #[error("board slug exceeds 64 characters")]
    TooLong,
    #[error("board slug must start with alphanumeric character")]
    BadStart,
    #[error("board slug contains invalid characters (allowed: a-z 0-9 - _)")]
    InvalidChars,
    #[error("board slug '{0}' is reserved")]
    Reserved(String),
    #[error("board slug must not contain path separators or '..'")]
    PathTraversal,
}

/// Reserved slug names that must not be used as board identifiers.
const RESERVED_BOARD_SLUGS: &[&str] = &["_archived"];

/// Validate and normalise a board slug.
///
/// Returns the downcased, trimmed slug on success.
///
/// # Security ordering (T-1 mitigation)
///
/// 1. Detect leading/trailing whitespace (trim mismatch) → `PathTraversal`
/// 2. Detect `/`, `\\`, `..` → `PathTraversal`
/// 3. Downcase
/// 4. Empty → `Empty`
/// 5. Length > 64 → `TooLong`
/// 6. Reserved slug → `Reserved` (before BadStart so `_archived` returns Reserved, not BadStart)
/// 7. First char not ASCII alphanumeric → `BadStart`
/// 8. Body char outside `[a-z0-9_-]` → `InvalidChars`
///
/// Note: `"default"` is NOT rejected here — it maps to the legacy DB path
/// (D-01 back-compat). The `boards create default` CLI command rejects it
/// separately with a user-friendly message (Plan 03).
pub fn validate_board_slug(input: &str) -> Result<String, BoardSlugError> {
    // Step 1: leading/trailing whitespace check (before downcasing — Pitfall from RESEARCH §12).
    if input.trim() != input {
        return Err(BoardSlugError::PathTraversal);
    }

    // Step 2: path-traversal characters (before downcasing).
    if input.contains('/') || input.contains('\\') || input.contains("..") {
        return Err(BoardSlugError::PathTraversal);
    }

    // Step 3: downcase.
    let s = input.to_lowercase();

    // Step 4: empty check.
    if s.is_empty() {
        return Err(BoardSlugError::Empty);
    }

    // Step 5: length check.
    if s.len() > 64 {
        return Err(BoardSlugError::TooLong);
    }

    // Step 6: reserved slugs — checked before character validation so that
    // reserved names like "_archived" (which would otherwise trigger BadStart)
    // produce the Reserved error, not BadStart.
    if RESERVED_BOARD_SLUGS.contains(&s.as_str()) {
        return Err(BoardSlugError::Reserved(s));
    }

    // Step 7: first char must be ASCII alphanumeric.
    let first = s.chars().next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return Err(BoardSlugError::BadStart);
    }

    // Step 8: body chars must all be [a-z0-9_-].
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(BoardSlugError::InvalidChars);
    }

    Ok(s)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Positive (acceptance) cases
    // -----------------------------------------------------------------------

    #[test]
    fn accepts_simple_lowercase() {
        assert_eq!(validate_board_slug("alpha").unwrap(), "alpha");
    }

    #[test]
    fn accepts_slug_with_dash() {
        assert_eq!(validate_board_slug("atm10-server").unwrap(), "atm10-server");
    }

    #[test]
    fn accepts_slug_with_underscore() {
        // Underscore IS allowed in board slugs (unlike profile names).
        assert_eq!(validate_board_slug("my_board_2").unwrap(), "my_board_2");
    }

    #[test]
    fn accepts_single_char_and_downcases() {
        assert_eq!(validate_board_slug("A").unwrap(), "a");
    }

    #[test]
    fn accepts_64_char_boundary() {
        let input = "X".repeat(64);
        let expected = "x".repeat(64);
        assert_eq!(validate_board_slug(&input).unwrap(), expected);
    }

    #[test]
    fn downcases_mixed_case() {
        assert_eq!(validate_board_slug("Alpha-Beta").unwrap(), "alpha-beta");
    }

    #[test]
    fn accepts_leading_digit() {
        assert_eq!(validate_board_slug("1board").unwrap(), "1board");
    }

    #[test]
    fn accepts_default_slug() {
        // "default" must NOT be rejected by the validator (Pitfall 3).
        // The boards create CLI rejects it separately.
        assert_eq!(validate_board_slug("default").unwrap(), "default");
    }

    // -----------------------------------------------------------------------
    // Negative (rejection) cases — one test per error variant
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_empty() {
        assert!(matches!(
            validate_board_slug(""),
            Err(BoardSlugError::Empty)
        ));
    }

    #[test]
    fn rejects_too_long() {
        let input = "X".repeat(65);
        assert!(matches!(
            validate_board_slug(&input),
            Err(BoardSlugError::TooLong)
        ));
    }

    #[test]
    fn rejects_underscore_start() {
        assert!(matches!(
            validate_board_slug("_underscore_start"),
            Err(BoardSlugError::BadStart)
        ));
    }

    #[test]
    fn rejects_dash_start() {
        assert!(matches!(
            validate_board_slug("-dash-start"),
            Err(BoardSlugError::BadStart)
        ));
    }

    #[test]
    fn rejects_space_in_body() {
        assert!(matches!(
            validate_board_slug("alpha bravo"),
            Err(BoardSlugError::InvalidChars)
        ));
    }

    #[test]
    fn rejects_dot_in_body() {
        assert!(matches!(
            validate_board_slug("alpha.board"),
            Err(BoardSlugError::InvalidChars)
        ));
    }

    #[test]
    fn rejects_control_char() {
        assert!(matches!(
            validate_board_slug("alpha\x01"),
            Err(BoardSlugError::InvalidChars)
        ));
    }

    #[test]
    fn rejects_forward_slash() {
        assert!(matches!(
            validate_board_slug("a/b"),
            Err(BoardSlugError::PathTraversal)
        ));
    }

    #[test]
    fn rejects_backslash_and_dotdot() {
        assert!(matches!(
            validate_board_slug("..\\evil"),
            Err(BoardSlugError::PathTraversal)
        ));
    }

    #[test]
    fn rejects_dotdot_path() {
        assert!(matches!(
            validate_board_slug("../etc/passwd"),
            Err(BoardSlugError::PathTraversal)
        ));
    }

    #[test]
    fn rejects_leading_space_as_path_traversal() {
        // Leading whitespace triggers trim-mismatch detection BEFORE downcasing.
        assert!(matches!(
            validate_board_slug(" leading-space"),
            Err(BoardSlugError::PathTraversal)
        ));
    }

    #[test]
    fn rejects_reserved_archived() {
        assert!(matches!(
            validate_board_slug("_archived"),
            Err(BoardSlugError::Reserved(_))
        ));
    }

    #[test]
    fn rejects_dotdot_alone() {
        assert!(matches!(
            validate_board_slug(".."),
            Err(BoardSlugError::PathTraversal)
        ));
    }

    #[test]
    fn rejects_trailing_space_as_path_traversal() {
        assert!(matches!(
            validate_board_slug("board "),
            Err(BoardSlugError::PathTraversal)
        ));
    }
}
