//! Per-profile vault path and policy-name builders (Phase 51 D-04/D-05/D-06).
//!
//! Deliberately dependency-free — no `rusty_vault` types, no feature gate — so it compiles and
//! is tested on the crate's default feature set. This is layer 1 of D-08's three independent
//! enforcement layers: every function here REFUSES an unsafe slug or leaf rather than
//! sanitizing it. Phase 47.4's CR-05 lineage is the reason: a sanitizer that "almost works" is
//! how a traversal check becomes decorative, so rejection is the only acceptable outcome for a
//! disallowed input — never a repaired string. No rejection path in this module ever echoes the
//! offending input back in its error message (CR-05 again: an error `Display` that embeds raw
//! input is itself the leak) — every error names the rule that fired instead.
//!
//! # KV v1 shape, no `data/` segment (D-04)
//!
//! `secret/profiles/{slug}/{leaf}` — the mount segment below is a private module constant, so
//! a future per-profile-mount migration (D-06) is a change to this one file, never to
//! agent-visible config.
//!
//! # Two deliberately different validators, not one (Task 2)
//!
//! [`validate_profile_slug`] and [`validate_profile_leaf`] are separate on purpose:
//!
//! - **The slug** (`validate_profile_slug`) is a positive allowlist — lowercase alphanumerics
//!   plus `-`, first character alphanumeric, non-empty, bounded length. A denylist is how
//!   percent-encoded and double-encoded variants get through; an allowlist rejects them without
//!   having to enumerate them. Tightening here is free: every profile slug upstream of this
//!   module already goes through an equivalent validator
//!   (`ironhermes_core::profile::validate_profile_name`, which this crate cannot import —
//!   `ironhermes-vault` must not depend on `ironhermes-core`, since core depends on vault, not
//!   the reverse — so this is a local, independent re-implementation of an equivalent rule,
//!   defense in depth per D-08, not duplication for its own sake). Do NOT "clean this up" by
//!   reaching for the core validator; the point of layer 1 is that it holds even if the
//!   upstream validator is bypassed or changed.
//!
//! - **The leaf** (`validate_profile_leaf`) is a PROVIDER name, and provider names are
//!   arbitrary operator-authored config keys: `CustomProviderConfig.name`
//!   (`ironhermes-core/src/config.rs`) carries no validation of its own, `ProviderResolver`
//!   inserts it verbatim as the endpoint key, and no `validate_provider_name` exists anywhere
//!   in the workspace. Applying the slug allowlist to the leaf would refuse a real,
//!   already-working install's `My_Provider` or `vLLM` with a path-traversal-shaped error for a
//!   name it has used successfully for months. So the leaf rule is a refusal list matched to
//!   the actual threat instead: reject path separators (`/` and `\`), the traversal token
//!   (`..`), the percent sign in its entirety (this is what makes `%2e%2e`, `%2E%2E`, `%2f` and
//!   every double-encoded variant fall out for free, with no decoded-sequence hunting needed —
//!   no legitimate provider name contains a percent sign), NUL, every control character,
//!   leading/trailing whitespace, the empty string, and anything over 128 bytes. Accept
//!   everything else, including uppercase and `_`. That is exactly `rusty_vault_store.rs`'s
//!   existing `validate_key` rule set for the root keyspace, plus four tightenings — consistency
//!   with what already ships, not a new concession.
//!
//! Accepting uppercase/`_` in the leaf costs no enforcement: D-05's profile policy rule is a
//! trailing-`*` prefix rule over the profile's own subtree, the pinned rev's ACL path matching
//! is case-sensitive, and only POLICY *names* are lowercased by `PolicyStore::sanitize_name` —
//! never paths. A mixed-case or underscore-bearing leaf therefore still lands inside exactly the
//! profile's own granted subtree and nowhere else.
//!
//! [`slug_rule_is_stricter_than_the_leaf_rule`](tests::slug_rule_is_stricter_than_the_leaf_rule)
//! is the test that makes a future unification of these two rules fail loudly instead of
//! quietly breaking an operator's install.

use crate::error::VaultError;

/// Mount segment for per-profile secrets — private so a future per-profile-mount migration
/// (D-06) touches only this constant, never agent-visible config, and appears in no other
/// file.
const SECRET_PROFILES_MOUNT: &str = "secret/profiles/";

/// Maximum byte length of a leaf (provider name) segment — generous for any real config key,
/// tight enough to bound HCL policy text and audit-log line length.
const MAX_LEAF_LEN: usize = 128;

/// Positive allowlist for the profile SLUG: `[a-z0-9][a-z0-9-]*`. See the module doc's "Two
/// deliberately different validators" section for why this is stricter than
/// [`validate_profile_leaf`] and why that asymmetry is deliberate.
fn validate_profile_slug(slug: &str) -> Result<(), VaultError> {
    if slug.is_empty() {
        return Err(VaultError::InvalidKey(
            "profile slug: must not be empty".to_string(),
        ));
    }
    let starts_ok = slug
        .chars()
        .next()
        .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .unwrap_or(false);
    if !starts_ok {
        return Err(VaultError::InvalidKey(
            "profile slug: must start with a lowercase letter or digit".to_string(),
        ));
    }
    let all_allowed = slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !all_allowed {
        return Err(VaultError::InvalidKey(
            "profile slug: must match [a-z0-9][a-z0-9-]* (lowercase alphanumeric and dashes \
             only)"
                .to_string(),
        ));
    }
    Ok(())
}

/// Refusal-list validator for the LEAF (provider name) segment — deliberately different from
/// [`validate_profile_slug`]. See the module doc's "Two deliberately different validators"
/// section for the full rationale: this matches `rusty_vault_store.rs`'s existing
/// `validate_key` rule set for the root keyspace, tightened by four rules (percent sign,
/// whitespace edges, and a length bound), and accepts uppercase and `_` because provider names
/// are arbitrary operator-authored config keys with no validation anywhere upstream.
fn validate_profile_leaf(leaf: &str) -> Result<(), VaultError> {
    if leaf.is_empty() {
        return Err(VaultError::InvalidKey(
            "profile leaf: must not be empty".to_string(),
        ));
    }
    if leaf.len() > MAX_LEAF_LEN {
        return Err(VaultError::InvalidKey(format!(
            "profile leaf: exceeds {MAX_LEAF_LEN} bytes"
        )));
    }
    if leaf.contains('/') {
        return Err(VaultError::InvalidKey(
            "profile leaf: must not contain '/' (path separator)".to_string(),
        ));
    }
    if leaf.contains('\\') {
        return Err(VaultError::InvalidKey(
            "profile leaf: must not contain '\\' (path separator)".to_string(),
        ));
    }
    if leaf.contains("..") {
        return Err(VaultError::InvalidKey(
            "profile leaf: must not contain '..' (traversal token)".to_string(),
        ));
    }
    if leaf.contains('%') {
        return Err(VaultError::InvalidKey(
            "profile leaf: must not contain '%' (percent-encoding is rejected outright, not \
             decoded)"
                .to_string(),
        ));
    }
    if leaf.chars().any(char::is_control) {
        return Err(VaultError::InvalidKey(
            "profile leaf: must not contain control characters".to_string(),
        ));
    }
    let starts_ws = leaf
        .chars()
        .next()
        .map(char::is_whitespace)
        .unwrap_or(false);
    let ends_ws = leaf
        .chars()
        .next_back()
        .map(char::is_whitespace)
        .unwrap_or(false);
    if starts_ws || ends_ws {
        return Err(VaultError::InvalidKey(
            "profile leaf: must not have leading or trailing whitespace".to_string(),
        ));
    }
    Ok(())
}

/// The `secret/profiles/{slug}/` prefix a profile's own policy (D-05) and its own secrets
/// (D-04) both derive from — the single source every other path in this module is built on.
pub fn profile_secret_prefix(slug: &str) -> Result<String, VaultError> {
    validate_profile_slug(slug)?;
    Ok(format!("{SECRET_PROFILES_MOUNT}{slug}/"))
}

/// `secret/profiles/{slug}/{leaf}` — KV v1, no `data/` segment anywhere (D-04). `leaf` is the
/// provider name (Open Question 1, resolved in `51-02-PLAN.md`'s `source_facts`). `slug` and
/// `leaf` are validated independently by two deliberately different rules — see the module doc.
pub fn profile_secret_path(slug: &str, leaf: &str) -> Result<String, VaultError> {
    let prefix = profile_secret_prefix(slug)?;
    validate_profile_leaf(leaf)?;
    Ok(format!("{prefix}{leaf}"))
}

/// `profile-{slug}` — the RustyVault policy-name convention Plan 04's `pre_route` guard
/// derives its allowed prefix from.
pub fn profile_policy_name(slug: &str) -> Result<String, VaultError> {
    validate_profile_slug(slug)?;
    Ok(format!("profile-{slug}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_secret_prefix_shape() {
        assert_eq!(
            profile_secret_prefix("alpha").expect("valid slug must build a prefix"),
            "secret/profiles/alpha/"
        );
    }

    #[test]
    fn profile_secret_path_shape() {
        assert_eq!(
            profile_secret_path("alpha", "openrouter").expect("valid slug+leaf must build a path"),
            "secret/profiles/alpha/openrouter"
        );
    }

    #[test]
    fn profile_policy_name_shape() {
        assert_eq!(
            profile_policy_name("alpha").expect("valid slug must build a policy name"),
            "profile-alpha"
        );
    }

    /// `profile_secret_path_is_kv_v1_shape` — asserts the full literal path for both a plain
    /// and a mixed-case leaf (Task 2 acceptance criteria).
    #[test]
    fn profile_secret_path_is_kv_v1_shape() {
        assert_eq!(
            profile_secret_path("alpha", "openrouter").expect("plain leaf"),
            "secret/profiles/alpha/openrouter"
        );
        assert_eq!(
            profile_secret_path("alpha", "My_Provider").expect("mixed-case leaf accepted"),
            "secret/profiles/alpha/My_Provider"
        );
    }

    /// Acceptance on the slug side: a plain lowercase slug, a hyphenated slug, a
    /// digit-containing slug all succeed with the exact resulting string.
    #[test]
    fn slug_acceptance_cases() {
        assert_eq!(
            profile_secret_prefix("alpha").expect("plain lowercase"),
            "secret/profiles/alpha/"
        );
        assert_eq!(
            profile_secret_prefix("client-acme").expect("hyphenated"),
            "secret/profiles/client-acme/"
        );
        assert_eq!(
            profile_secret_prefix("a1b2").expect("digit-containing"),
            "secret/profiles/a1b2/"
        );
    }

    /// Table-driven: every case here must be refused as a SLUG.
    #[test]
    fn path_builder_refuses_traversal_slugs() {
        let cases: &[&str] = &[
            "..",
            "../beta",
            "alpha/../beta",
            "/alpha",
            "/etc/passwd",
            "alpha\\beta",
            "%2e%2e",
            "%2E%2E",
            "%2f",
            "alpha\0beta",
            "alpha\nbeta",
            "alpha beta",
            "-alpha",
            "Alpha",
            "alpha_beta",
            "",
        ];
        for case in cases {
            assert!(
                profile_secret_prefix(case).is_err(),
                "expected slug {case:?} to be refused"
            );
        }
    }

    /// Table-driven: every case here must be refused as a LEAF.
    #[test]
    fn path_builder_refuses_traversal_leaves() {
        let cases: &[&str] = &[
            "a/b",
            "..",
            "a/../b",
            "../beta",
            "/abs",
            "a\\b",
            "%2e%2e",
            "%2E%2E",
            "%2f",
            "a\0b",
            "a\nb",
            "a\tb",
            " leading",
            "trailing ",
            "",
        ];
        for case in cases {
            assert!(
                profile_secret_path("alpha", case).is_err(),
                "expected leaf {case:?} to be refused"
            );
        }
        // A 129-byte name exceeds the 128-byte bound.
        let too_long = "a".repeat(129);
        assert!(profile_secret_path("alpha", &too_long).is_err());
    }

    /// Real operator-authored provider names must be accepted as leaves — including uppercase
    /// and underscore-bearing ones (T-51-57b compatibility).
    #[test]
    fn leaf_validator_accepts_real_custom_provider_names() {
        let cases: &[&str] = &[
            "openrouter",
            "openai",
            "anthropic",
            "venice",
            "local-llama",
            "My_Provider",
            "vLLM",
            "ollama2",
            "gpt-4o-proxy",
        ];
        for case in cases {
            assert!(
                profile_secret_path("alpha", case).is_ok(),
                "expected real provider name {case:?} to be accepted as a leaf"
            );
        }
        let max_len = "p".repeat(MAX_LEAF_LEN);
        assert!(profile_secret_path("alpha", &max_len).is_ok());
    }

    /// The asymmetry is the design: these two names are rejected as slugs and accepted as
    /// leaves. A future unification of the two rules fails this test in one direction or the
    /// other.
    #[test]
    fn slug_rule_is_stricter_than_the_leaf_rule() {
        assert!(profile_secret_prefix("My_Provider").is_err());
        assert!(profile_secret_prefix("vLLM").is_err());
        assert!(profile_secret_path("alpha", "My_Provider").is_ok());
        assert!(profile_secret_path("alpha", "vLLM").is_ok());
    }

    /// Every rejection's rendered error text names the rule that fired and does NOT contain the
    /// rejected input (CR-05 reflex, applied preemptively).
    #[test]
    fn rejection_errors_never_echo_the_input() {
        let bad_slug = "UPPERCASE-SLUG";
        let err = profile_secret_prefix(bad_slug)
            .expect_err("uppercase slug must be refused")
            .to_string();
        assert!(
            !err.contains(bad_slug),
            "error must not echo the rejected slug, got: {err}"
        );
        assert!(err.contains("must"), "error must name the rule: {err}");

        let bad_leaf = "has/a/slash";
        let err = profile_secret_path("alpha", bad_leaf)
            .expect_err("slash-containing leaf must be refused")
            .to_string();
        assert!(
            !err.contains(bad_leaf),
            "error must not echo the rejected leaf, got: {err}"
        );
        assert!(err.contains("must"), "error must name the rule: {err}");
    }
}
