//! Phase 47.4 Plan 05 Task 3 — `profile_api.rs`'s `fetch_profile_detail` /
//! `update_profile_config` / `save_profile_key` shape locks + registration
//! checks.
//!
//! `iron_hermes_ui` is a bin-only crate (no `src/lib.rs`), so integration
//! tests under `tests/` cannot reach `pub(crate)` items in
//! `src/server/profile_api.rs` — the identical, already-established
//! constraint `tests/profile_health.rs` and `tests/profile_scaffold.rs`
//! document for Plans 01 and 03 (see those files' own doc comments, and
//! `PATTERNS.md`'s "Second correction" section, which documents this exact
//! split as the codebase's established precedent: `provider_config_api.rs`
//! / `provider_secrets_api.rs` keep their real functional tests as an
//! inline `#[cfg(test)]` module in the SAME file as the logic under test,
//! never in a separate `tests/*.rs` file that cannot see `pub(crate)`
//! items). The real fixture-directory D-02/D-04/D-07/D-11/D-13 functional
//! tests for `fetch_profile_detail_impl` / `classify_key_status` /
//! `update_profile_config_impl` / `merge_profile_config_payload` /
//! `save_profile_key_impl` / `validate_key_name` / `validate_key_value` —
//! every case in the plan's `<behavior>` blocks, asserted against real
//! files on disk, including the D-13 no-leak serialized-wire-form
//! assertions and the mask length-invariance check — live as `#[cfg(test)]`
//! unit tests inside `src/server/profile_api.rs` itself (`mod
//! profile_key_masking_tests`), where `pub(crate)` visibility actually
//! reaches them. Run them with:
//!
//!   cargo nextest run -p iron_hermes_ui --features server profile_key_masking_tests --test-threads=1
//!
//! This file only locks the module's *shape* — the fn/const names, the
//! D-13 no-`value`-field invariant, the write gate reuse, and the
//! `render_profile_env`/`write_env_atomic_0600` single-write-path reuse —
//! via source-string assertions, mirroring `tests/profile_health.rs` /
//! `tests/profile_scaffold.rs` / `tests/provider_config_api.rs`. (The
//! plan's own literal automated verify command, `cargo nextest run -p
//! iron_hermes_ui profile_key_masking --test-threads=1` with no
//! `--features server`, does not build on its own — this crate's `main.rs`
//! unconditionally references `#[cfg(feature = "server")]`-gated modules
//! regardless of feature selection, the same finding Plans 01 and 03
//! document; the corrected invocation is documented above.)
//!
//! Note on this file's literal test count / `to_string(&` assertion count:
//! because the real serialized-wire-form no-leak tests live in
//! `profile_api.rs` (where `ProfileDetail`/`KeyRow` instances can actually
//! be constructed and serialized), this file has fewer than the plan's
//! literal acceptance-criteria numbers for those two greps — see the
//! plan's SUMMARY.md for the documented deviation (identical in kind to
//! Plans 01/03's own documented deviations for the same structural reason).

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

#[test]
fn profile_api_declares_fetch_profile_detail() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("pub async fn fetch_profile_detail("),
        "profile_api.rs must declare `pub async fn fetch_profile_detail(`"
    );
    assert!(
        src.contains("#[server]\npub async fn fetch_profile_detail("),
        "fetch_profile_detail must be a #[server] fn"
    );
}

#[test]
fn profile_api_declares_update_profile_config() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("pub async fn update_profile_config("),
        "profile_api.rs must declare `pub async fn update_profile_config(`"
    );
}

#[test]
fn profile_api_declares_save_profile_key() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("pub async fn save_profile_key("),
        "profile_api.rs must declare `pub async fn save_profile_key(`"
    );
}

#[test]
fn profile_api_declares_classify_key_status() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("fn classify_key_status("),
        "profile_api.rs must declare `classify_key_status`"
    );
}

#[test]
fn profile_api_declares_merge_profile_config_payload() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("fn merge_profile_config_payload("),
        "profile_api.rs must declare `merge_profile_config_payload`"
    );
}

/// D-11: `fetch_profile_detail` must reuse the single `classify_profile_health`
/// call site — one classification rule shared with `list_profiles`, never a
/// second implementation that could drift.
#[test]
fn fetch_profile_detail_reuses_classify_profile_health_not_a_reimplementation() {
    let src = read("src/server/profile_api.rs");
    let occurrences = src.matches("classify_profile_health").count();
    assert!(
        occurrences >= 3,
        "expected classify_profile_health referenced at least 3 times \
         (definition + list_profiles + fetch_profile_detail), got {occurrences}"
    );
}

/// D-06/D-07: `save_profile_key` must reuse the same atomic-0600 write path
/// `create_profile` established — never a second secret-write
/// implementation.
#[test]
fn save_profile_key_reuses_write_env_atomic_0600_not_a_second_write_path() {
    let src = read("src/server/profile_api.rs");
    let occurrences = src.matches("write_env_atomic_0600").count();
    assert!(
        occurrences >= 3,
        "expected write_env_atomic_0600 referenced at least 3 times \
         (definition + create_profile + save_profile_key), got {occurrences}"
    );
}

/// T-47.4-05-E1: every mutating fn added by this plan must gate on the same
/// flag `create_profile` already enforces.
#[test]
fn write_fns_gate_on_web_config_write_enabled() {
    let src = read("src/server/profile_api.rs");
    let occurrences = src.matches("web_config_write_enabled").count();
    assert!(
        occurrences >= 3,
        "expected web_config_write_enabled referenced at least 3 times \
         (create_profile's gate + the ProfileDetail field/read + this \
         plan's update_profile_config/save_profile_key gates), got {occurrences}"
    );
}

/// D-13: the plaintext key value is wrapped in `secrecy::SecretString`
/// before it can reach a `Debug`-deriving struct — `save_profile_key` must
/// use it, mirroring `create_profile`'s existing precedent.
#[test]
fn save_profile_key_wraps_value_in_secret_string() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("SecretString"),
        "profile_api.rs must wrap key material in SecretString (D-13)"
    );
}

/// D-13: `KeyRow` still carries only `name` / `status` / `masked` — never a
/// raw key value. Redundant with `tests/profile_health.rs`'s own lock, but
/// kept here so this file is self-contained for the invariant this plan's
/// read/write surface depends on most directly.
#[test]
fn key_row_still_has_no_value_field() {
    let src = read("src/protocol.rs");
    let start = src
        .find("pub struct KeyRow {")
        .expect("KeyRow must be declared");
    let end = src[start..]
        .find('}')
        .map(|i| start + i)
        .expect("KeyRow struct body must close");
    let body = &src[start..end];
    assert!(
        !body.contains("value"),
        "KeyRow must not have a `value` field — key material is write-only \
         across the HTTP boundary (D-13)"
    );
}

/// D-04: `ProfileDetail` must not carry a reachability field — the detail
/// surface's health tier means CONFIGURED, never "reachable" (that claim
/// belongs only to the D-09 probe in a later plan).
#[test]
fn profile_detail_carries_no_reachability_field() {
    let src = read("src/protocol.rs");
    let start = src
        .find("pub struct ProfileDetail {")
        .expect("ProfileDetail must be declared");
    let end = src[start..]
        .find('}')
        .map(|i| start + i)
        .expect("ProfileDetail struct body must close");
    let body = &src[start..end];
    assert!(
        !body.to_lowercase().contains("reachable"),
        "ProfileDetail must never gain a reachability field (D-11)"
    );
}

/// D-06: no per-profile secret-storage path anywhere in this file — not a
/// namespaced write, not even a call into the vault crate.
///
/// Phase 49.4.1 Plan 02 (D-10): narrowed from a blanket "no mention of the
/// word `vault`" ban — this phase's own CONTEXT.md/threat model sanctions a
/// READ-ONLY vault-availability check (`compute_secrets_source_availability`
/// inspecting `config.vault.enabled` and `cfg!(feature = "rusty-vault")`) so
/// the picker's Vault row can report WHY it's disabled instead of silently
/// omitting it (T-49.4.1-08: "no vault call site is added — D-10 only READS
/// whether the feature and flag are present"). What must still never appear
/// is an actual vault STORAGE call site — the strings below are exactly the
/// ones a `RustyVaultStore`/`ironhermes_vault` integration would introduce.
#[test]
fn profile_api_never_mentions_vault_storage() {
    let src = read("src/server/profile_api.rs");
    for forbidden in [
        "ironhermes_vault",
        "RustyVault",
        "resolve_vault_config",
        "apply_vault_fallback",
    ] {
        assert!(
            !src.contains(forbidden),
            "profile_api.rs must never reference vault storage — D-06 keeps keys \
             in the plaintext profile .env only (found {forbidden:?})"
        );
    }
}
