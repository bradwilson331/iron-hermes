//! Phase 24 — Profile name validation (D-03, D-17).
//!
//! Validates `--profile <NAME>` slugs before any filesystem path is constructed.
//! Reserved tokens `default`, `current`, `none` are rejected to prevent operator
//! confusion with the bare-`hermes` root sentinel (D-15). Names beginning with
//! `_` are reserved for future internal use.
//!
//! Cross-crate convention (D-17): returns plain `String`, not a newtype enum.

use std::fmt;

const RESERVED_NAMES: &[&str] = &["default", "current", "none"];
const PROFILE_NAME_MAX_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileNameError {
    Empty,
    LeadingUnderscore,
    Reserved(String),
    InvalidChars,
    TooLong,
}

impl fmt::Display for ProfileNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileNameError::Empty => {
                write!(f, "profile name is empty")
            }
            ProfileNameError::LeadingUnderscore => {
                write!(f, "profile name must not begin with '_'")
            }
            ProfileNameError::Reserved(n) => write!(
                f,
                "profile name '{}' is reserved (default, current, none are not allowed)",
                n
            ),
            ProfileNameError::InvalidChars => write!(
                f,
                "profile name must match [a-z0-9][a-z0-9-]* (lowercase alphanumeric and dashes; must start with letter or digit)"
            ),
            ProfileNameError::TooLong => write!(
                f,
                "profile name exceeds {} characters",
                PROFILE_NAME_MAX_LEN
            ),
        }
    }
}

impl std::error::Error for ProfileNameError {}

/// Validate a profile name per D-03 rules. Returns the validated name as
/// owned `String` (D-17 cross-crate plain-String convention).
///
/// Rules:
/// - non-empty
/// - length <= 64
/// - must not begin with `_`
/// - must not be a reserved word (`default`, `current`, `none`)
/// - must match `[a-z0-9][a-z0-9-]*` (first char letter/digit; rest lowercase
///   alphanumeric or dash)
pub fn validate_profile_name(name: &str) -> Result<String, ProfileNameError> {
    if name.is_empty() {
        return Err(ProfileNameError::Empty);
    }
    if name.len() > PROFILE_NAME_MAX_LEN {
        return Err(ProfileNameError::TooLong);
    }
    if name.starts_with('_') {
        return Err(ProfileNameError::LeadingUnderscore);
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(ProfileNameError::Reserved(name.to_string()));
    }
    let valid_chars = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let starts_ok = name
        .chars()
        .next()
        .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .unwrap_or(false);
    if !valid_chars || !starts_ok {
        return Err(ProfileNameError::InvalidChars);
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_slug() {
        assert_eq!(validate_profile_name("work").unwrap(), "work");
        assert_eq!(validate_profile_name("client-acme").unwrap(), "client-acme");
        assert_eq!(validate_profile_name("a1b2").unwrap(), "a1b2");
    }

    #[test]
    fn rejects_default_token() {
        assert!(matches!(
            validate_profile_name("default"),
            Err(ProfileNameError::Reserved(ref n)) if n == "default"
        ));
    }

    #[test]
    fn rejects_current_token() {
        assert!(matches!(
            validate_profile_name("current"),
            Err(ProfileNameError::Reserved(_))
        ));
    }

    #[test]
    fn rejects_none_token() {
        assert!(matches!(
            validate_profile_name("none"),
            Err(ProfileNameError::Reserved(_))
        ));
    }

    #[test]
    fn rejects_leading_underscore() {
        assert!(matches!(
            validate_profile_name("_priv"),
            Err(ProfileNameError::LeadingUnderscore)
        ));
    }

    #[test]
    fn rejects_empty() {
        assert!(matches!(
            validate_profile_name(""),
            Err(ProfileNameError::Empty)
        ));
    }

    #[test]
    fn rejects_path_traversal_slash() {
        assert!(matches!(
            validate_profile_name("foo/bar"),
            Err(ProfileNameError::InvalidChars)
        ));
    }

    #[test]
    fn rejects_path_traversal_dotdot() {
        assert!(matches!(
            validate_profile_name("../etc"),
            Err(ProfileNameError::InvalidChars)
        ));
    }

    #[test]
    fn rejects_uppercase() {
        assert!(matches!(
            validate_profile_name("Work"),
            Err(ProfileNameError::InvalidChars)
        ));
    }

    #[test]
    fn rejects_space() {
        assert!(matches!(
            validate_profile_name("foo bar"),
            Err(ProfileNameError::InvalidChars)
        ));
    }

    #[test]
    fn rejects_leading_dash() {
        assert!(matches!(
            validate_profile_name("-leading"),
            Err(ProfileNameError::InvalidChars)
        ));
    }

    #[test]
    fn rejects_too_long() {
        let long = "a".repeat(65);
        assert!(matches!(
            validate_profile_name(&long),
            Err(ProfileNameError::TooLong)
        ));
    }

    #[test]
    fn accepts_64_char_boundary() {
        let boundary = "a".repeat(64);
        assert_eq!(validate_profile_name(&boundary).unwrap().len(), 64);
    }

    #[test]
    fn returns_owned_string_for_d17() {
        // D-17: cross-crate convention — return owned String, not &str.
        let result: Result<String, _> = validate_profile_name("work");
        assert_eq!(result.unwrap(), String::from("work"));
    }
}
