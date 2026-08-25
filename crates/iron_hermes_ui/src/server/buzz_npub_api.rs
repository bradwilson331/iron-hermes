//! Phase 50.1 Plan 07 (D-02/D-14): profile-scoped, read-only public identity
//! key (npub) derivation.
//!
//! `fetch_bot_npub(name)` is the ONLY new browser-reachable surface this
//! plan ships. It returns the bot's derived Nostr public key (bech32
//! `npub1…`) or `None` — the secret it is derived from NEVER crosses the
//! HTTP boundary and is never logged, traced, or embedded in an error.
//!
//! # Why this does not call the gateway crate's process-global nsec resolver
//!
//! `ironhermes_gateway::buzz_identity`'s single-secret-producing function
//! (see that module's own doc comment) is process-global — it resolves
//! relative to `get_hermes_home()`, the single live profile (D-04) — so
//! calling it for a NON-live bot would return the live bot's identity or an
//! error (RESEARCH Pitfall 4). This module instead reads the TARGET
//! profile's own `.env` directly via `profile_api`'s existing
//! `profile_dir_for`/`read_env_keys` helpers, which are already
//! profile-scoped by construction. This is a second READER of the
//! `BUZZ_NSEC` value, not a second producer — the gateway module's own doc
//! comment names itself the one place in the workspace that PRODUCES the
//! secret, and this module does not gain a competing public entry point
//! into that crate; it opens the `.env` file the same way
//! `profile_api.rs`'s other key-reading code already does.
//!
//! # Secret-disclosure discipline (T-50.1-07-01/02, CR-05/CR-06 precedent)
//!
//! The resolved secret is wrapped in [`secrecy::SecretString`] the instant
//! it is read (before it can reach any `Debug`-deriving struct) and is
//! never returned, logged, or embedded in any error. `read_env_keys`'s own
//! parse-failure branch already withholds a malformed `.env`'s raw failing
//! line (D-13 discipline, `profile_api.rs`); this module inherits that by
//! reusing the helper rather than re-implementing `.env` parsing, and
//! itself never interpolates any error content into a returned value —
//! every unavailable case (absent file, absent entry, unparseable file,
//! malformed secret) collapses onto a single `Ok(None)`, deliberately: a
//! not-configured bot and a bot whose identity file is broken both
//! remediate outside this surface, and distinguishing them on the wire
//! would be the first step toward reporting file contents.
//!
//! # Feature gating
//!
//! The `#[server]` fn itself is declared unconditionally (like every other
//! server fn in this crate — `profile_api::create_profile` is the
//! precedent) so the wasm client build always has a stub to call. The real
//! derivation needs `nostr-sdk` (this crate's `buzz` feature, forwarding
//! `ironhermes-gateway/buzz`), which is folded into the crate's `server`
//! feature — so any build that serves the UI gets a working row, and the
//! wasm client bundle never pulls `nostr-sdk` in (declared under the
//! non-wasm target dependency table). [`derive_public_key`] additionally
//! carries a `#[cfg(not(feature = "buzz"))]` fallback that always reports
//! unresolvable, so the crate still compiles in the hypothetical case a
//! future change decouples `server` from `buzz` again (this plan's own
//! documented fallback path — no such decoupling was needed this run; the
//! `--features server` build was verified clean).
//!
//! # Scope fence (D-02)
//!
//! This module and `bot_roster::npub_row` are the entire social-protocol
//! surface this phase ships. No key generation, no key import, no
//! allow-list editing, no channel configuration, no relay connection — see
//! `scope_fence_tests` below, which fails if a later phase's roster or
//! drawer source reintroduces one of those affordances into this phase's
//! files without deliberately updating the test's own forbidden-label
//! list.
//!
//! **Known correction for the deferred social-protocol phase:** the
//! canonical desktop client's direct messages use the NIP-29 group-based
//! protocol, NOT the NIP-17 sealed-direct-message protocol — a naive
//! NIP-17 DM implementation is already known wrong before that later phase
//! starts (50.1-RESEARCH.md "Deferred Ideas").
//!
//! # OF-8a (gap task): inherited-npub display fallback
//!
//! `50.1-OPERATOR-FEEDBACK.md`'s OF-8 DECISION: when the bot's OWN `.env`
//! has no identity secret, [`fetch_bot_npub_impl`] falls back to reading
//! the MAIN profile's root `.env` (`get_hermes_home().join(".env")` —
//! [`read_root_identity_secret`], a second READER of the same file
//! `profile_api.rs`'s inherited-provider-key helpers already read, never a
//! second producer) and derives that key's public form instead. This is
//! DISPLAY-ONLY: no secret is ever copied into any bot's own `.env` — the
//! D-17 fence (per-profile vault phase owns real credential provisioning)
//! holds exactly as it did before this gap task. [`BotNpubResult::inherited`]
//! is `true` only when the returned `npub` came from this fallback, so the
//! UI can label it without widening the wire type into anything
//! secret-adjacent (still just a public key string plus a `bool`).

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Phase 50.1 OF-8a (gap task): the wire result of a bot's npub resolution
/// attempt. Deliberately still structurally incapable of carrying secret
/// material — `npub` is the public key (safe to expose by design),
/// `inherited` is provenance-only (never the value it was derived from),
/// and `own_env_unreadable` (MI-08, `50.1-REVIEW.md`) is a content-free
/// flag — never the path, never the failing line. This is the widened
/// successor to plan 07's original `Option<String>` return shape;
/// [`fetch_bot_npub_impl_return_type_pins_bot_npub_result`] (this module's
/// tests) re-asserts the "no secret-shaped field" discipline for the wider
/// shape, mirroring the pin the original single-`Option` return type
/// carried.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BotNpubResult {
    /// The resolved public key (bech32 `npub1…`), or `None` when no key
    /// resolved — either because neither profile has one configured, or
    /// (MI-08) because the bot's OWN `.env` exists but could not be read.
    pub npub: Option<String>,
    /// `true` only when `npub` was resolved from the MAIN profile's root
    /// `.env` because the bot's own `.env` had none. Always `false` when
    /// `npub` is `None` — there is no "inherited nothing" state.
    pub inherited: bool,
    /// MI-08 (`50.1-REVIEW.md`, gap task): `true` when the bot's OWN
    /// `.env` file exists but could not be opened or parsed — distinct
    /// from "no file"/"no entry" (which still fall back to the root
    /// identity normally). When this is `true`, `npub` is always `None`
    /// and `inherited` is always `false`: the root fallback is
    /// deliberately SUPPRESSED for this case, so a broken own identity
    /// file is never masked by silently displaying the main profile's
    /// identity instead.
    pub own_env_unreadable: bool,
}

#[cfg(feature = "server")]
use secrecy::{ExposeSecret, SecretString};

#[cfg(feature = "server")]
use crate::server::profile_api::{profile_dir_for, read_env_keys};

/// The `.env` key name the profile's Nostr secret is read from — the same
/// name `ironhermes_gateway::buzz_identity::BUZZ_NSEC_ENV` declares.
/// Re-declared as a local literal (rather than importing the gateway
/// constant unconditionally) so this module's non-derivation logic
/// compiles the same way regardless of whether `buzz` happens to be
/// enabled alongside `server` — see the module doc's Feature gating
/// section.
#[cfg(feature = "server")]
const IDENTITY_ENV_KEY: &str = "BUZZ_NSEC";

/// Phase 50.1 Plan 07 (T-50.1-07-01/03) / MI-08 (`50.1-REVIEW.md`, gap
/// task): the outcome of a profile-scoped identity-secret read. `Resolved`
/// carries the secret wrapped at the moment of read. `Unavailable` is a
/// unit variant that structurally cannot carry any file content, key name,
/// or error detail — "no file" and "file exists but has no entry / an
/// empty value" are indistinguishable by this type on purpose (module
/// doc), because both are legitimate "not configured yet" states that
/// should still fall back to the root identity. `OwnEnvUnreadable` (MI-08,
/// new) is the ONE outcome that is NOT folded into `Unavailable`: the file
/// EXISTS but could not be opened or parsed — also content-free (no path,
/// no failing line, no underlying error) — kept distinct specifically so
/// [`fetch_bot_npub_impl`] can suppress the root-identity fallback for it
/// (a broken own `.env` must never be masked by silently substituting the
/// main profile's identity).
#[cfg(feature = "server")]
#[derive(Debug)]
pub(crate) enum NpubResolveOutcome {
    Resolved(SecretString),
    Unavailable,
    OwnEnvUnreadable,
}

/// Phase 50.1 Plan 07 (T-50.1-07-03) / MI-08 (gap task): read the TARGET
/// profile's own identity secret from its own `.env` — never the gateway
/// crate's process-global resolver (module doc). Reuses `profile_api`'s
/// `profile_dir_for`/`read_env_keys` verbatim rather than re-implementing
/// `.env` resolution or path joining. Missing dir, missing file, missing
/// entry, and empty value all collapse onto `Unavailable` — never an
/// `Err`, since an unconfigured bot is a valid roster state, not a fault.
/// An UNPARSEABLE file (the file exists but `read_env_keys` itself
/// errors) is `OwnEnvUnreadable` instead (MI-08) — distinct from
/// `Unavailable` so the caller can refuse to treat "broken" the same as
/// "not configured".
#[cfg(feature = "server")]
pub(crate) fn read_profile_identity_secret(name: &str) -> NpubResolveOutcome {
    let env_path = profile_dir_for(name).join(".env");
    let map = match read_env_keys(&env_path) {
        // read_env_keys's own Err already withholds the raw failing line
        // (D-13 discipline) — this module additionally throws the string
        // away entirely rather than forwarding even that safe summary.
        // MI-08: this Err means the file EXISTS but is unreadable/
        // unparseable — `read_env_keys` only ever returns `Ok(empty map)`
        // for a genuinely missing file (its own doc comment), so this
        // branch can never fire for "no file", only for "broken file".
        Ok(m) => m,
        Err(_) => return NpubResolveOutcome::OwnEnvUnreadable,
    };
    match map.get(IDENTITY_ENV_KEY) {
        Some(v) if !v.trim().is_empty() => NpubResolveOutcome::Resolved(SecretString::from(v.clone())),
        _ => NpubResolveOutcome::Unavailable,
    }
}

/// Phase 50.1 OF-8a (gap task): read the MAIN profile's own identity secret
/// from the root `.env` (`get_hermes_home().join(".env")`) — the exact
/// root-env path `profile_api.rs`'s inherited-provider-key helpers
/// (`resolve_inherited_key`-family) already read for the SAME kind of
/// "does the main profile have this configured" question, reused rather
/// than re-derived. A second READER of that file, never a second producer
/// (module doc). Every failure mode (missing root `.env`, missing entry,
/// empty value, unparseable file) collapses onto `Unavailable`, identical
/// discipline to [`read_profile_identity_secret`] — a main profile with no
/// identity configured yet is a valid state, not a fault.
#[cfg(feature = "server")]
fn read_root_identity_secret() -> NpubResolveOutcome {
    let env_path = ironhermes_core::get_hermes_home().join(".env");
    let map = match read_env_keys(&env_path) {
        // read_env_keys's own Err already withholds the raw failing line
        // (D-13 discipline) — thrown away entirely here too, same as
        // read_profile_identity_secret above.
        Ok(m) => m,
        Err(_) => return NpubResolveOutcome::Unavailable,
    };
    match map.get(IDENTITY_ENV_KEY) {
        Some(v) if !v.trim().is_empty() => NpubResolveOutcome::Resolved(SecretString::from(v.clone())),
        _ => NpubResolveOutcome::Unavailable,
    }
}

/// A public-key derivation failure. Carries no input, no partial input,
/// and no underlying-library error detail — `Display` is a fixed string —
/// so this type structurally cannot leak a malformed secret regardless of
/// what the parsing library's own error would have said.
#[cfg(feature = "server")]
#[derive(Debug)]
pub(crate) struct DeriveKeyError;

#[cfg(feature = "server")]
impl std::fmt::Display for DeriveKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "could not derive a public key from the profile's identity secret")
    }
}

/// Phase 50.1 Plan 07 (T-50.1-07-06): parse the profile's identity secret
/// and return its bech32 public form — the exact `Keys::parse(...)
/// .public_key().to_bech32()` two-call derivation
/// `ironhermes-cli/src/buzz_cmd.rs`'s `resolve_pubkey_report` already uses
/// for the same purpose (CLI's `ironhermes buzz pubkey`), reused rather
/// than hand-rolled. Never hand-rolls bech32 or elliptic-curve arithmetic.
#[cfg(feature = "buzz")]
pub(crate) fn derive_public_key(secret: &SecretString) -> Result<String, DeriveKeyError> {
    use nostr_sdk::prelude::ToBech32;
    let keys = nostr_sdk::prelude::Keys::parse(secret.expose_secret()).map_err(|_| DeriveKeyError)?;
    keys.public_key().to_bech32().map_err(|_| DeriveKeyError)
}

/// Fallback used when this crate is compiled with `server` but, contrary
/// to the primary decision recorded in this plan, without `buzz` (module
/// doc's Feature gating section). Always reports unresolvable so the row
/// renders its inline "not available yet" text rather than the crate
/// failing to compile.
#[cfg(all(feature = "server", not(feature = "buzz")))]
pub(crate) fn derive_public_key(_secret: &SecretString) -> Result<String, DeriveKeyError> {
    Err(DeriveKeyError)
}

/// Phase 50.1 Plan 07 (T-50.1-07-04): validates the profile name with the
/// crate's shared validator before any path is resolved. Extracted from
/// [`fetch_bot_npub`] so this ordering is directly unit-testable without
/// invoking the `#[server]` macro machinery — the same "test the logic
/// layer, not the `#[server]`-wrapped fn" precedent
/// `tests/kanban_board_read.rs` and this file's sibling `profile_api.rs`
/// already follow.
#[cfg(feature = "server")]
pub(crate) fn validate_bot_npub_name(name: &str) -> Result<String, String> {
    ironhermes_core::profile::validate_profile_name(name).map_err(|e| format!("invalid profile name: {e}"))
}

/// Phase 50.1 Plan 07 / OF-8a / MI-08 (gap task): the impl-layer dispatch —
/// resolve-then-derive against the bot's OWN profile first.
///
/// - `Resolved` (own key present and valid): returned immediately, own key
///   always wins.
/// - `OwnEnvUnreadable` (MI-08): the root fallback below is SKIPPED
///   entirely — a broken own `.env` must never be masked by silently
///   displaying the main profile's identity instead. Returned immediately
///   with `own_env_unreadable: true`.
/// - `Unavailable` (no file / no entry / empty value — a legitimate
///   "not configured yet" state): falls through to
///   [`read_root_identity_secret`] (the MAIN profile's root `.env`),
///   setting `inherited: true` on the returned [`BotNpubResult`] IFF the
///   fallback is what actually produced the `npub`. A root secret that
///   fails to derive (should not happen in practice — the main profile's
///   own identity is already used elsewhere — but handled structurally)
///   is treated the same as no root secret at all: `npub: None, inherited:
///   false`, never a half-true state.
///
/// Takes an ALREADY-VALIDATED name; [`fetch_bot_npub`] validates before
/// calling this, and [`validate_bot_npub_name`] is the piece under direct
/// test for that ordering.
#[cfg(feature = "server")]
pub(crate) fn fetch_bot_npub_impl(name: &str) -> BotNpubResult {
    match read_profile_identity_secret(name) {
        NpubResolveOutcome::Resolved(secret) => {
            return BotNpubResult {
                npub: derive_public_key(&secret).ok(),
                inherited: false,
                own_env_unreadable: false,
            };
        }
        NpubResolveOutcome::OwnEnvUnreadable => {
            // MI-08: suppress the fallback — never fall through to
            // read_root_identity_secret below.
            return BotNpubResult {
                npub: None,
                inherited: false,
                own_env_unreadable: true,
            };
        }
        NpubResolveOutcome::Unavailable => {
            // Legitimate "not configured own key" — fall through to the
            // root fallback below.
        }
    }
    match read_root_identity_secret() {
        NpubResolveOutcome::Resolved(secret) => match derive_public_key(&secret).ok() {
            Some(npub) => BotNpubResult {
                npub: Some(npub),
                inherited: true,
                own_env_unreadable: false,
            },
            None => BotNpubResult {
                npub: None,
                inherited: false,
                own_env_unreadable: false,
            },
        },
        // read_root_identity_secret() only ever produces Resolved or
        // Unavailable (its own doc comment) — OwnEnvUnreadable is named
        // for the OWN-profile read specifically and is unreachable here,
        // but this match must stay exhaustive over the shared enum.
        NpubResolveOutcome::Unavailable | NpubResolveOutcome::OwnEnvUnreadable => BotNpubResult {
            npub: None,
            inherited: false,
            own_env_unreadable: false,
        },
    }
}

/// Phase 50.1 Plan 07 / OF-8a (gap task, D-14): the browser-reachable
/// read-only npub fetch. Follows `create_profile`'s four-step protocol
/// minus the write gate (this is a read): validate → fresh `Config::load()`
/// → `spawn_blocking` for the filesystem read(s). `npub: None` covers every
/// unavailable case (own AND root); only a malformed NAME (not a malformed
/// profile) produces a real `ServerFnError` (T-50.1-07-04).
#[server]
pub async fn fetch_bot_npub(name: String) -> Result<BotNpubResult, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let validated = validate_bot_npub_name(&name).map_err(ServerFnError::new)?;
        // Fresh config load, matching this plan's protocol — this read has
        // no write gate to check, so the loaded config itself is unused
        // beyond confirming it loads.
        let _config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        let result = tokio::task::spawn_blocking(move || fetch_bot_npub_impl(&validated))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?;
        Ok(result)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = name;
        Err(ServerFnError::new("fetch_bot_npub unavailable without `server` feature"))
    }
}

/// Run with (mutates process-global `IRONHERMES_HOME`, so `--test-threads=1`
/// is required, same constraint `profile_api.rs`'s own env-mutating tests
/// document):
///   `cargo nextest run -p iron_hermes_ui --all-features buzz_npub --test-threads=1`
#[cfg(all(test, feature = "server", feature = "buzz"))]
mod tests {
    use super::*;
    use nostr_sdk::prelude::{Keys, ToBech32};
    use std::fs;

    /// RAII guard that sets an env var and restores the previous value on
    /// drop. Copied verbatim from `ironhermes-kanban/src/paths.rs`, same as
    /// `profile_api.rs`'s and `bot_meta_api.rs`'s own module-local copies.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; env_lock() below
            // serializes every test in this crate that mutates process env.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: still holding env_lock() from the caller.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn seed_profile_env(home: &std::path::Path, name: &str, body: &str) {
        let dir = home.join("profiles").join(name);
        fs::create_dir_all(&dir).expect("create profile dir");
        fs::write(dir.join(".env"), body).expect("write .env");
    }

    fn walk_all(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                out.push(entry.path());
            }
        }
        out.sort();
        out
    }

    #[test]
    fn derive_public_key_known_good_secret_returns_a_different_bech32_pubkey() {
        let keys = Keys::generate();
        let nsec = SecretString::from(keys.secret_key().to_bech32().expect("bech32 nsec"));
        let expected_npub = keys.public_key().to_bech32().expect("bech32 npub");

        let derived = derive_public_key(&nsec).expect("should derive");

        assert_eq!(derived, expected_npub);
        assert_ne!(derived, nsec.expose_secret());
    }

    #[test]
    fn derive_public_key_malformed_secret_returns_error_leaking_no_input_substring() {
        // Two DIFFERENT malformed inputs — if the message ever echoed any
        // part of the input, these two calls would render differently.
        // Asserting the message is the SAME fixed string both times is a
        // stronger guarantee than checking arbitrary substrings (a fixed,
        // input-independent message cannot embed the input by
        // construction, so no substring of the input — of any length —
        // can appear as a *consequence of that input*).
        let input_a = "not-a-valid-nostr-secret-at-all";
        let input_b = "zzz9382-completely-different-garbage-7716qq";

        let err_a = derive_public_key(&SecretString::from(input_a.to_string())).unwrap_err();
        let err_b = derive_public_key(&SecretString::from(input_b.to_string())).unwrap_err();
        let message_a = err_a.to_string();
        let message_b = err_b.to_string();

        assert!(!message_a.contains(input_a));
        assert!(!format!("{err_a:?}").contains(input_a));
        assert!(!message_b.contains(input_b));
        assert!(!format!("{err_b:?}").contains(input_b));
        assert_eq!(
            message_a, message_b,
            "the rendered message must be identical regardless of the malformed input — a \
             fixed, input-independent message cannot embed any part of the input"
        );
    }

    #[test]
    fn read_profile_identity_secret_no_entry_returns_unavailable_not_error() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        seed_profile_env(dir.path(), "bot-no-entry", "OTHER_KEY=value\n");

        let outcome = read_profile_identity_secret("bot-no-entry");
        assert!(matches!(outcome, NpubResolveOutcome::Unavailable));
    }

    #[test]
    fn read_profile_identity_secret_missing_file_returns_unavailable() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        // Profile directory exists but has no .env at all.
        fs::create_dir_all(dir.path().join("profiles").join("bot-no-env")).expect("mkdir");

        let outcome = read_profile_identity_secret("bot-no-env");
        assert!(matches!(outcome, NpubResolveOutcome::Unavailable));
    }

    /// MI-08 (`50.1-REVIEW.md`, gap task): a malformed own `.env` is now
    /// `OwnEnvUnreadable`, NOT `Unavailable` — renamed from this test's
    /// original `..._returns_unavailable_no_leaked_content` name, which
    /// described the pre-MI-08 behavior.
    #[test]
    fn read_profile_identity_secret_malformed_file_returns_own_env_unreadable_no_leaked_content() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        // A raw '=' with no key is a dotenvy parse error (same shape
        // buzz_identity.rs's own malformed-file test uses).
        seed_profile_env(dir.path(), "bot-malformed", "=BUZZ_NSEC_secret_value_should_never_leak\n");

        let outcome = read_profile_identity_secret("bot-malformed");
        assert!(matches!(outcome, NpubResolveOutcome::OwnEnvUnreadable));
        // OwnEnvUnreadable is a unit variant — it structurally cannot carry
        // the line, key or value; assert on the actual rendered form anyway.
        let rendered = format!("{outcome:?}");
        assert!(!rendered.contains("secret_value_should_never_leak"));
    }

    #[test]
    fn read_profile_identity_secret_two_profiles_are_scoped_and_differ() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let keys_a = Keys::generate();
        let keys_b = Keys::generate();
        let nsec_a = keys_a.secret_key().to_bech32().expect("bech32 a");
        let nsec_b = keys_b.secret_key().to_bech32().expect("bech32 b");
        seed_profile_env(dir.path(), "bot-a", &format!("{IDENTITY_ENV_KEY}={nsec_a}\n"));
        seed_profile_env(dir.path(), "bot-b", &format!("{IDENTITY_ENV_KEY}={nsec_b}\n"));

        let outcome_a = read_profile_identity_secret("bot-a");
        let outcome_b = read_profile_identity_secret("bot-b");

        let secret_a = match outcome_a {
            NpubResolveOutcome::Resolved(s) => s,
            NpubResolveOutcome::Unavailable => panic!("bot-a should resolve"),
            NpubResolveOutcome::OwnEnvUnreadable => panic!("bot-a's .env should be well-formed"),
        };
        let secret_b = match outcome_b {
            NpubResolveOutcome::Resolved(s) => s,
            NpubResolveOutcome::Unavailable => panic!("bot-b should resolve"),
            NpubResolveOutcome::OwnEnvUnreadable => panic!("bot-b's .env should be well-formed"),
        };
        assert_ne!(
            secret_a.expose_secret(),
            secret_b.expose_secret(),
            "each profile must resolve its OWN identity, not a shared/process-global one"
        );
    }

    #[test]
    fn fetch_bot_npub_rejects_invalid_name_before_resolving_any_path() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let before = walk_all(dir.path());

        let result = validate_bot_npub_name("../../etc/passwd");

        assert!(result.is_err());
        let after = walk_all(dir.path());
        assert_eq!(
            before, after,
            "an invalid profile name must never cause any path to be resolved or touched"
        );
    }

    /// T-50.1-07-01: a resolved-and-derived npub must not leak the secret
    /// it was derived from — asserted on the actual returned/formatted
    /// value, not on source text (a source inspection would pass even if
    /// the leak arrived through a differently-named path).
    #[test]
    fn success_path_never_leaks_the_seeded_secret_marker() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let keys = Keys::generate();
        let marker = keys.secret_key().to_bech32().expect("bech32 nsec");
        seed_profile_env(dir.path(), "bot-marker", &format!("{IDENTITY_ENV_KEY}={marker}\n"));

        let result = fetch_bot_npub_impl("bot-marker");
        let rendered = format!("{result:?}");

        assert!(result.npub.is_some(), "expected a resolved npub");
        assert!(!result.inherited, "the bot's own key must resolve without falling back");
        assert!(!rendered.contains(&marker), "returned value leaked the seeded secret marker");
        assert_ne!(result.npub.as_deref(), Some(marker.as_str()));
    }

    /// T-50.1-07-02: the malformed-`.env`-file path is the exact CR-05/
    /// CR-06 mechanism (a `dotenvy` parse error's `Display` embeds the raw
    /// failing line) — assert the seeded marker never surfaces through
    /// EITHER this module's resolve outcome or its final fetch result.
    /// MI-08 (`50.1-REVIEW.md`, gap task): also assert this is now
    /// `own_env_unreadable: true`, NOT the plain `Unavailable`-shaped
    /// result it used to collapse onto before that fix.
    #[test]
    fn malformed_file_path_never_leaks_the_seeded_secret_marker() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let marker = "nsec1_marker_should_never_appear_in_any_output_anywhere";
        // A raw '=' with no key is a dotenvy parse error.
        seed_profile_env(dir.path(), "bot-malformed-marker", &format!("={marker}\n"));

        let outcome_rendered = format!("{:?}", read_profile_identity_secret("bot-malformed-marker"));
        let result = fetch_bot_npub_impl("bot-malformed-marker");
        let result_rendered = format!("{result:?}");

        assert!(result.npub.is_none());
        assert!(!result.inherited);
        assert!(
            result.own_env_unreadable,
            "a malformed own .env must set own_env_unreadable: true (MI-08)"
        );
        assert!(!outcome_rendered.contains(marker), "resolve outcome leaked the malformed line's marker");
        assert!(!result_rendered.contains(marker), "fetch result leaked the malformed line's marker");
    }

    /// Pins the wire type: [`BotNpubResult`], now a three-field
    /// `{npub, inherited, own_env_unreadable}` struct (MI-08, `50.1-REVIEW.md`,
    /// widened from OF-8a's original two-field shape) with no field shaped
    /// like a secret. The field list below is exhaustive positional
    /// construction AND destructuring (no `..Default::default()`, no `..`
    /// rest pattern), which breaks if a field is ever added, removed, or
    /// renamed without updating this test — that break is the point.
    #[test]
    fn fetch_bot_npub_impl_return_type_pins_bot_npub_result() {
        fn assert_signature(_f: fn(&str) -> BotNpubResult) {}
        assert_signature(fetch_bot_npub_impl);
        let BotNpubResult {
            npub: _,
            inherited: _,
            own_env_unreadable: _,
        } = BotNpubResult {
            npub: None,
            inherited: false,
            own_env_unreadable: false,
        };
    }

    // -------------------------------------------------------------------
    // OF-8a (gap task): inherited-npub display fallback
    // -------------------------------------------------------------------

    #[test]
    fn read_root_identity_secret_resolves_from_hermes_home_dot_env() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let keys = Keys::generate();
        let nsec = keys.secret_key().to_bech32().expect("bech32 nsec");
        std::fs::write(dir.path().join(".env"), format!("{IDENTITY_ENV_KEY}={nsec}\n"))
            .expect("write root .env");

        let outcome = read_root_identity_secret();
        let secret = match outcome {
            NpubResolveOutcome::Resolved(s) => s,
            NpubResolveOutcome::Unavailable => panic!("root .env should resolve"),
            NpubResolveOutcome::OwnEnvUnreadable => {
                panic!("read_root_identity_secret never produces this variant")
            }
        };
        assert_eq!(secret.expose_secret(), nsec);
    }

    #[test]
    fn read_root_identity_secret_missing_file_returns_unavailable() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        // Deliberately no root .env written at all.

        let outcome = read_root_identity_secret();
        assert!(matches!(outcome, NpubResolveOutcome::Unavailable));
    }

    #[test]
    fn read_root_identity_secret_no_entry_returns_unavailable() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        std::fs::write(dir.path().join(".env"), "OTHER_KEY=value\n").expect("write root .env");

        let outcome = read_root_identity_secret();
        assert!(matches!(outcome, NpubResolveOutcome::Unavailable));
    }

    /// Marker-leak discipline on the new root-env read path, success case.
    #[test]
    fn read_root_identity_secret_success_never_leaks_the_seeded_marker_via_debug() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let keys = Keys::generate();
        let marker = keys.secret_key().to_bech32().expect("bech32 nsec");
        std::fs::write(dir.path().join(".env"), format!("{IDENTITY_ENV_KEY}={marker}\n"))
            .expect("write root .env");

        let outcome = read_root_identity_secret();
        // SecretString's own Debug impl redacts — asserted here so a future
        // change to the wrapped type would be caught rather than silently
        // trusting the library.
        let rendered = format!("{outcome:?}");
        assert!(!rendered.contains(&marker), "root secret Debug output leaked the seeded marker");
    }

    /// Marker-leak discipline on the new root-env read path, malformed-file
    /// case — the exact CR-05/CR-06 mechanism, now exercised against the
    /// root `.env` path instead of a profile's own.
    #[test]
    fn read_root_identity_secret_malformed_file_never_leaks_the_seeded_marker() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let marker = "nsec1_root_marker_should_never_appear_in_any_output_anywhere";
        // A raw '=' with no key is a dotenvy parse error.
        std::fs::write(dir.path().join(".env"), format!("={marker}\n")).expect("write root .env");

        let outcome = read_root_identity_secret();
        assert!(matches!(outcome, NpubResolveOutcome::Unavailable));
        let rendered = format!("{outcome:?}");
        assert!(!rendered.contains(marker), "malformed root .env leaked the seeded marker");
    }

    #[test]
    fn fetch_bot_npub_impl_own_key_wins_over_root_key() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let own_keys = Keys::generate();
        let root_keys = Keys::generate();
        let own_nsec = own_keys.secret_key().to_bech32().expect("bech32 own nsec");
        let root_nsec = root_keys.secret_key().to_bech32().expect("bech32 root nsec");
        let expected_own_npub = own_keys.public_key().to_bech32().expect("bech32 own npub");
        seed_profile_env(dir.path(), "bot-both", &format!("{IDENTITY_ENV_KEY}={own_nsec}\n"));
        std::fs::write(dir.path().join(".env"), format!("{IDENTITY_ENV_KEY}={root_nsec}\n"))
            .expect("write root .env");

        let result = fetch_bot_npub_impl("bot-both");

        assert_eq!(result.npub.as_deref(), Some(expected_own_npub.as_str()));
        assert!(!result.inherited, "a bot with its own key must never be marked inherited");
        assert!(!result.own_env_unreadable);
    }

    #[test]
    fn fetch_bot_npub_impl_falls_back_to_root_when_bot_has_no_key() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let root_keys = Keys::generate();
        let root_nsec = root_keys.secret_key().to_bech32().expect("bech32 root nsec");
        let expected_root_npub = root_keys.public_key().to_bech32().expect("bech32 root npub");
        // Bot profile exists but has no BUZZ_NSEC entry of its own — an
        // EMPTY env, not an unreadable one, so MI-08 must NOT suppress this
        // fallback.
        seed_profile_env(dir.path(), "bot-empty", "OTHER_KEY=value\n");
        std::fs::write(dir.path().join(".env"), format!("{IDENTITY_ENV_KEY}={root_nsec}\n"))
            .expect("write root .env");

        let result = fetch_bot_npub_impl("bot-empty");

        assert_eq!(result.npub.as_deref(), Some(expected_root_npub.as_str()));
        assert!(result.inherited, "falling back to the root key must set inherited: true");
        assert!(!result.own_env_unreadable);
    }

    #[test]
    fn fetch_bot_npub_impl_neither_key_returns_unavailable_not_inherited() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        // Neither the bot's own .env nor the root .env has a key.
        seed_profile_env(dir.path(), "bot-neither", "OTHER_KEY=value\n");

        let result = fetch_bot_npub_impl("bot-neither");

        assert!(result.npub.is_none());
        assert!(!result.inherited, "no npub must never be paired with inherited: true");
        assert!(!result.own_env_unreadable);
    }

    /// The bot has no profile directory at all (never scaffolded), and the
    /// root also has nothing — same as the "neither" case above, exercised
    /// through the missing-directory path instead of an empty-entry file.
    #[test]
    fn fetch_bot_npub_impl_missing_profile_dir_and_no_root_key_returns_unavailable() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

        let result = fetch_bot_npub_impl("bot-never-created");

        assert!(result.npub.is_none());
        assert!(!result.inherited);
        assert!(!result.own_env_unreadable, "an absent .env is not the same as an unreadable one");
    }

    // -------------------------------------------------------------------
    // MI-08 (50.1-REVIEW.md, gap task): malformed own .env suppresses the
    // root-identity fallback
    // -------------------------------------------------------------------

    /// The core regression scenario the reviewer named: a bot's own `.env`
    /// is malformed (still possibly holding its own real `BUZZ_NSEC` on an
    /// unparseable line) AND the root `.env` has a perfectly valid key. The
    /// root fallback must be suppressed — MUST NOT silently display the
    /// main profile's identity in place of the bot's broken one.
    #[test]
    fn fetch_bot_npub_impl_malformed_own_env_suppresses_root_fallback_even_when_root_has_a_key() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let root_keys = Keys::generate();
        let root_nsec = root_keys.secret_key().to_bech32().expect("bech32 root nsec");
        // A raw '=' with no key is a dotenvy parse error — the bot's own
        // `.env` exists but cannot be parsed at all.
        seed_profile_env(dir.path(), "bot-broken", "=BUZZ_NSEC_hidden_in_a_malformed_line\n");
        std::fs::write(dir.path().join(".env"), format!("{IDENTITY_ENV_KEY}={root_nsec}\n"))
            .expect("write root .env");

        let result = fetch_bot_npub_impl("bot-broken");

        assert!(
            result.npub.is_none(),
            "a broken own .env must never resolve to the ROOT's npub — that would silently \
             mask the bot's own (unreadable) identity with the main profile's"
        );
        assert!(!result.inherited, "no npub must never be paired with inherited: true");
        assert!(
            result.own_env_unreadable,
            "the row must be told this is an unreadable-own-file case, not a plain unavailable one"
        );
    }

    /// Marker-leak discipline on the MI-08 path, exercised through the full
    /// `fetch_bot_npub_impl` dispatch (not just `read_profile_identity_secret`
    /// directly) — the final `BotNpubResult`'s `Debug` output must never
    /// contain the malformed line's content.
    #[test]
    fn fetch_bot_npub_impl_malformed_own_env_never_leaks_the_seeded_marker() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let marker = "nsec1_mi08_marker_should_never_appear_in_any_output_anywhere";
        seed_profile_env(dir.path(), "bot-broken-marker", &format!("={marker}\n"));
        // Root has a key too, to prove suppression AND leak-safety together.
        let root_keys = Keys::generate();
        let root_nsec = root_keys.secret_key().to_bech32().expect("bech32 root nsec");
        std::fs::write(dir.path().join(".env"), format!("{IDENTITY_ENV_KEY}={root_nsec}\n"))
            .expect("write root .env");

        let result = fetch_bot_npub_impl("bot-broken-marker");
        let rendered = format!("{result:?}");

        assert!(result.npub.is_none());
        assert!(result.own_env_unreadable);
        assert!(!rendered.contains(marker), "fetch result leaked the malformed line's marker");
    }

    /// Companion to the two "still falls back" tests above, phrased against
    /// the exact MI-08 wording: absent-file and empty-entry are NOT
    /// "unreadable" and must both still produce an inherited fallback when
    /// the root has a key. This test seeds a profile dir with NO `.env`
    /// file at all (distinct from `fetch_bot_npub_impl_falls_back_to_root_when_bot_has_no_key`,
    /// which seeds a `.env` with an unrelated entry).
    #[test]
    fn fetch_bot_npub_impl_absent_own_env_file_still_falls_back_with_inherited_label() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let root_keys = Keys::generate();
        let root_nsec = root_keys.secret_key().to_bech32().expect("bech32 root nsec");
        let expected_root_npub = root_keys.public_key().to_bech32().expect("bech32 root npub");
        // Profile directory exists but has no .env file at all.
        std::fs::create_dir_all(dir.path().join("profiles").join("bot-no-env-file"))
            .expect("mkdir profile dir");
        std::fs::write(dir.path().join(".env"), format!("{IDENTITY_ENV_KEY}={root_nsec}\n"))
            .expect("write root .env");

        let result = fetch_bot_npub_impl("bot-no-env-file");

        assert_eq!(result.npub.as_deref(), Some(expected_root_npub.as_str()));
        assert!(result.inherited);
        assert!(!result.own_env_unreadable, "a missing file is absent, not unreadable");
    }
}

/// Phase 50.1 Plan 07 (D-02, T-50.1-07-05): the scope fence, encoded as a
/// test rather than a review comment. Reads this phase's roster and drawer
/// source files back and asserts none of them contains a control label for
/// bringing an identity into existence, importing one, editing an access
/// list, or configuring a transport endpoint or channel. Mirrors the
/// source-level structural-property-by-reading-the-file-back style already
/// established by `tests/profile_health.rs`'s `CLIENT_LLM_KEY_ALLOWLIST`
/// check and `tests/logout_palette.rs`'s registry-source scan. Runs
/// regardless of `server`/`buzz` — it is pure text-content inspection, no
/// filesystem I/O beyond compile-time `include_str!`.
#[cfg(test)]
mod scope_fence_tests {
    /// Case-sensitive, multi-word phrases only — this crate's own control
    /// labels are ALL-CAPS single/compound words (COPY, DUPLICATE, DELETE
    /// BOT, NPUB…), while this phase's own doc comments describing what is
    /// OUT of scope use lowercase prose — a case-sensitive, specific-phrase
    /// match avoids tripping on that legitimate prose while still catching
    /// a real reintroduced control label. A future phase that legitimately
    /// adds one of these surfaces removes the matching entry here
    /// deliberately.
    const FORBIDDEN_SCOPE_LABELS: &[&str] = &[
        "GENERATE KEYPAIR",
        "IMPORT NSEC",
        "ADD TO WHITELIST",
        "CONFIGURE RELAY",
        "NEW CHANNEL",
    ];

    const ROSTER_SOURCE: &str = include_str!("../components/hermes_app/screens/bot_roster.rs");
    const CARD_SOURCE: &str = include_str!("../components/hermes_app/screens/bot_roster/card.rs");
    const DELETE_CONFIRM_SOURCE: &str =
        include_str!("../components/hermes_app/screens/bot_roster/delete_confirm.rs");
    // OF-5 (gap task): `bot_roster/handoff.rs` (the bare inline composer)
    // was deleted and superseded by the floating chat window — this scan
    // target moved with it so the scope fence still covers the roster's
    // non-live-bot messaging surface.
    const CHAT_WINDOW_SOURCE: &str =
        include_str!("../components/hermes_app/screens/bot_roster/chat_window.rs");
    const NPUB_ROW_SOURCE: &str = include_str!("../components/hermes_app/screens/bot_roster/npub_row.rs");
    const EDIT_DIALOG_SOURCE: &str =
        include_str!("../components/hermes_app/screens/profile_shared/edit_dialog.rs");

    #[test]
    fn roster_and_drawer_sources_contain_no_scope_fenced_affordance() {
        let sources: [(&str, &str); 6] = [
            ("bot_roster.rs", ROSTER_SOURCE),
            ("bot_roster/card.rs", CARD_SOURCE),
            ("bot_roster/delete_confirm.rs", DELETE_CONFIRM_SOURCE),
            ("bot_roster/chat_window.rs", CHAT_WINDOW_SOURCE),
            ("bot_roster/npub_row.rs", NPUB_ROW_SOURCE),
            ("profile_shared/edit_dialog.rs", EDIT_DIALOG_SOURCE),
        ];
        for (file, src) in sources {
            for label in FORBIDDEN_SCOPE_LABELS {
                assert!(
                    !src.contains(label),
                    "{file} contains the scope-fenced label {label:?} — D-02 defers all \
                     social-protocol access beyond the read-only npub row out of this phase"
                );
            }
        }
    }
}
