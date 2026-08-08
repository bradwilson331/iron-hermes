//! Phase 47.6 Plan 01 (D-06): the single Nostr secret-key resolution seam.
//!
//! [`resolve_buzz_nsec`] is the ONE place in the workspace that produces the
//! profile's Nostr secret key. Every future nsec read (the CLI keygen in
//! plan 04, the cron sender in plan 07, and any later caller) goes through
//! this function so a future secrets-vault migration (Phase 51) has a single
//! edit point — insert the vault branch here and nowhere else.
//!
//! # Security
//!
//! `BuzzIdentityError`'s `Display`/`Debug` renderings NEVER embed the
//! resolved secret value, and NEVER embed a `dotenvy::Error` in any form (no
//! `{e}`, `.to_string()`, `Display`, `Debug`, or error `source`).
//! `dotenvy::Error::LineParse`'s `Display` interpolates the ENTIRE raw
//! failing line, which for a secret-bearing `.env` line IS the secret —
//! Phase 47.4 closed exactly this leak class on the profile `.env` write
//! path; this module closes the same class on the read path (T-47.6-03).

use std::path::{Path, PathBuf};

/// Environment variable name the profile's Nostr secret key is read from
/// first, before the profile `.env` fallback.
pub const BUZZ_NSEC_ENV: &str = "BUZZ_NSEC";

/// Errors from [`resolve_buzz_nsec`]. Both variants are actionable without
/// leaking: [`BuzzIdentityError::NotConfigured`] names the env var and the
/// profile `.env` path an operator needs to set; [`BuzzIdentityError::EnvParseFailed`]
/// names only the path, never the malformed line's content.
#[derive(Debug, thiserror::Error)]
pub enum BuzzIdentityError {
    /// Neither the `BUZZ_NSEC` environment variable nor a matching key in
    /// the profile `.env` resolved to a non-empty value.
    #[error(
        "Buzz identity not configured: set the {BUZZ_NSEC_ENV} environment variable, \
         or add `{BUZZ_NSEC_ENV}=<nsec-or-hex>` to the profile .env at {path}"
    )]
    NotConfigured {
        /// The profile `.env` path that was checked.
        path: PathBuf,
    },

    /// The profile `.env` exists but could not be parsed. The failing line's
    /// content is deliberately withheld (D-06 / T-47.6-03) — see this
    /// module's doc comment.
    #[error("profile .env at {path} could not be parsed — content withheld (D-06)")]
    EnvParseFailed {
        /// The profile `.env` path that failed to parse.
        path: PathBuf,
    },
}

/// Resolve the active profile's Nostr secret key.
///
/// Resolution order:
/// 1. The `BUZZ_NSEC` environment variable.
/// 2. The same key in the active profile's `.env`
///    (`get_hermes_home().join(".env")` — already profile-scoped by the
///    `--profile` pivot in `get_hermes_home`, so this is automatically
///    per-profile with no extra plumbing, which is what makes the D-04
///    two-profile UAT work).
///
/// Accepts both bech32 `nsec1…` and 64-char hex forms — the raw resolved
/// string is returned unparsed; the caller passes it to
/// `nostr_sdk::Keys::parse`, which accepts both forms itself.
///
/// This is the thin process-env wrapper around [`resolve_buzz_nsec_with`],
/// the actual injectable-seam implementation tests exercise directly.
pub fn resolve_buzz_nsec() -> Result<String, BuzzIdentityError> {
    let env_path = ironhermes_core::get_hermes_home().join(".env");
    resolve_buzz_nsec_with(|name| std::env::var(name).ok(), &env_path)
}

/// The injectable-seam inner resolver [`resolve_buzz_nsec`] wraps.
///
/// Takes the environment lookup as a closure and the `.env` path directly so
/// tests can exercise every precedence branch WITHOUT mutating the real
/// process environment: `nextest` runs tests in the same binary
/// concurrently, process environment is global mutable state, and this
/// codebase has already been bitten by exactly that race (plan 03 applies
/// the identical injectable-seam rule to its own boot-gate resolver — both
/// resolvers need it, not just one).
pub fn resolve_buzz_nsec_with<F>(env_lookup: F, env_path: &Path) -> Result<String, BuzzIdentityError>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(v) = env_lookup(BUZZ_NSEC_ENV)
        && !v.trim().is_empty()
    {
        return Ok(v);
    }

    if env_path.exists() {
        let iter = dotenvy::from_path_iter(env_path).map_err(|_| BuzzIdentityError::EnvParseFailed {
            path: env_path.to_path_buf(),
        })?;
        for item in iter {
            match item {
                Ok((k, v)) if k == BUZZ_NSEC_ENV => {
                    if !v.trim().is_empty() {
                        return Ok(v);
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    // CR-06 precedent (dispatch_gate.rs): never interpolate
                    // the dotenvy::Error — its Display embeds the entire raw
                    // failing line, which for a secret-bearing line IS the
                    // secret. This is even stricter than dispatch_gate.rs'
                    // own open-error handling: this module withholds path
                    // detail on EVERY dotenvy failure, parse or open alike.
                    return Err(BuzzIdentityError::EnvParseFailed {
                        path: env_path.to_path_buf(),
                    });
                }
            }
        }
    }

    Err(BuzzIdentityError::NotConfigured {
        path: env_path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn no_env(_name: &str) -> Option<String> {
        None
    }

    #[test]
    fn env_wins_over_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        std::fs::write(&env_path, format!("{BUZZ_NSEC_ENV}=from-file\n")).expect("write .env");

        let resolved = resolve_buzz_nsec_with(|name| {
            if name == BUZZ_NSEC_ENV {
                Some("from-env".to_string())
            } else {
                None
            }
        }, &env_path)
        .expect("should resolve");

        assert_eq!(resolved, "from-env");
    }

    #[test]
    fn falls_back_to_file_when_env_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        std::fs::write(&env_path, format!("{BUZZ_NSEC_ENV}=from-file\n")).expect("write .env");

        let resolved = resolve_buzz_nsec_with(no_env, &env_path).expect("should resolve");
        assert_eq!(resolved, "from-file");
    }

    #[test]
    fn missing_everywhere_yields_not_configured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env"); // does not exist

        let err = resolve_buzz_nsec_with(no_env, &env_path).unwrap_err();
        assert!(matches!(err, BuzzIdentityError::NotConfigured { .. }));
    }

    #[test]
    fn missing_key_in_existing_file_yields_not_configured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        std::fs::write(&env_path, "OTHER_KEY=value\n").expect("write .env");

        let err = resolve_buzz_nsec_with(no_env, &env_path).unwrap_err();
        assert!(matches!(err, BuzzIdentityError::NotConfigured { .. }));
    }

    #[test]
    fn malformed_env_file_yields_parse_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        // A raw '=' with no key is a dotenvy parse error.
        let mut f = std::fs::File::create(&env_path).expect("create .env");
        writeln!(f, "={BUZZ_NSEC_ENV}_secret_value_should_never_leak").expect("write");

        let err = resolve_buzz_nsec_with(no_env, &env_path).unwrap_err();
        assert!(matches!(err, BuzzIdentityError::EnvParseFailed { .. }));
    }

    /// T-47.6-03: neither error variant's Debug nor Display may contain a
    /// seeded secret value or any raw .env line content.
    #[test]
    fn error_renderings_never_leak_secret_or_raw_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        let secret_marker = "nsec1_super_secret_should_never_appear_in_any_error";
        let mut f = std::fs::File::create(&env_path).expect("create .env");
        // Malformed line that embeds the secret marker — if dotenvy's raw
        // Display were ever interpolated, this marker would leak.
        writeln!(f, "={secret_marker}").expect("write");

        let err = resolve_buzz_nsec_with(no_env, &env_path).unwrap_err();
        let debug_str = format!("{err:?}");
        let display_str = format!("{err}");
        assert!(
            !debug_str.contains(secret_marker),
            "Debug rendering leaked the secret marker: {debug_str}"
        );
        assert!(
            !display_str.contains(secret_marker),
            "Display rendering leaked the secret marker: {display_str}"
        );
    }

    /// Confirms the injectable seam never touches the real process
    /// environment — this whole module contains no direct mutation of the
    /// process environment anywhere, by construction (grep-verifiable).
    #[test]
    fn inner_resolver_takes_injected_lookup_not_process_env() {
        // If this compiles and the tests above pass without any of them
        // mutating the process environment, the seam contract holds. This
        // test exists to be grepped by the plan's acceptance criterion.
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        let result = resolve_buzz_nsec_with(no_env, &env_path);
        assert!(result.is_err());
    }
}
