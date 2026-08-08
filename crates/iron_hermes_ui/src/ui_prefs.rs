//! UI preferences for the Phase 26.2.1 wheel-menu shell (Plan 02).
//!
//! `UiPrefs` is the typed Rust counterpart of the prototype's
//! `window.APP_TWEAKS` JSON object (`app.html` Tweaks panel). It is held in a
//! Dioxus context provider at the `App` root (Plan 05) and serialised to
//! browser localStorage under three keys (CONTEXT D-13..D-16, RESEARCH
//! Pattern 5):
//!
//! * `ih.ui.tweaks` — the full `UiPrefs` JSON blob
//! * `ih.ui.theme`  — the active theme slug (string)
//! * `ih.ui.wheel`  — `WheelState` JSON blob
//!
//! All localStorage helpers are gated on `target_arch = "wasm32"`; non-WASM
//! builds get no-op stubs so unit tests link on the host target.
//!
//! Module-level `#![allow(dead_code)]` because Wave 1 lands the types but
//! Wave 2+ wires the consumers — without this the default `cargo check`
//! would otherwise reject the unused factories under `-D warnings`.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Persistence keys (D-13 .. D-16, RESEARCH Pattern 5)
// ---------------------------------------------------------------------------

/// localStorage key for the serialised `UiPrefs` blob.
pub const KEY_TWEAKS: &str = "ih.ui.tweaks";

/// localStorage key for the active theme slug.
pub const KEY_THEME: &str = "ih.ui.theme";

/// localStorage key for the serialised `WheelState` blob.
pub const KEY_WHEEL: &str = "ih.ui.wheel";

/// localStorage key for the serialised `AvatarPrefs` blob (Phase 40.2, FE-01).
pub const KEY_AVATAR: &str = "ih.ui.avatar";

/// Phase 47.3 Plan 06 (D-17): localStorage key for the session-death
/// composer-draft stash. This is the ONLY thing D-17 ever writes to
/// JS-readable storage — the session token itself never crosses into
/// localStorage; it lives solely in the `HttpOnly` cookie (T-47.3-09).
/// Deliberately a distinct, unrelated-looking key from the session cookie
/// name (`ih_session`, server/auth.rs::SESSION_COOKIE) so a source-level
/// scan can never confuse the two.
pub const KEY_SESSION_DRAFT: &str = "ih.ui.session_expiry_draft";

// ---------------------------------------------------------------------------
// UiPrefs (D-16) — typed mirror of `window.APP_TWEAKS`
// ---------------------------------------------------------------------------

/// Runtime UI toggles surfaced by the Tweaks panel (Plan 05).
///
/// Defaults match the prototype's `window.APP_TWEAKS` initial values in
/// `app.html`. `wheel_size: 240.0` aligns with `WheelState::default().size`
/// (RESEARCH Pitfall 4 — avoids first-resize jump).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct UiPrefs {
    /// Accent colour swap (D-13 default = teal `#39c5cf`).
    pub accent: AccentColor,
    /// Wheel diameter in CSS pixels — written to the `--wheel-size`
    /// custom property by Plan 05.
    pub wheel_size: f64,
    /// Breadcrumb chip (`NODE HERMES-7 › BRIDGE › CHAT`) toggle.
    pub breadcrumb: bool,
    /// App-footer strip toggle.
    pub footer: bool,
    /// Per-row vertical density.
    pub density: Density,
    /// Optional vertical rail on the chat screen (`body.has-rail.on-chat`).
    pub rail: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            accent: AccentColor::Teal,
            wheel_size: 240.0,
            breadcrumb: true,
            footer: true,
            density: Density::Comfy,
            rail: true,
        }
    }
}

// ---------------------------------------------------------------------------
// AvatarPrefs (Phase 40.2, FE-01) — orb ↔ avatar toggle preference
// ---------------------------------------------------------------------------

/// Persisted avatar-mode preferences for the orb ↔ avatar runtime toggle.
///
/// Stored at `KEY_AVATAR` (`"ih.ui.avatar"`) in localStorage. Defaults to
/// orb-mode with the `"facecap"` head preset (D-01: orb is the default).
///
/// # Security (T-40.2-01-01)
///
/// Fields are **not** annotated with `#[serde(default)]`. A partial or
/// tampered localStorage blob therefore fails `serde_json::from_str`, causing
/// hydration's `.ok()` to return `None` and fall back to `AvatarPrefs::default()`.
/// This mirrors the T-DESERIALIZE mitigation used by `UiPrefs`.
///
/// # Phase 40.5 (D-17)
///
/// `active_identity` is the persisted pointer to the active communication-path
/// identity — an orb preset slug (e.g. `"orb_bloom"`) OR a head preset id
/// (e.g. `"facecap"`). It is **separate** from `head_id` so an orb-type
/// identity (which has no head rig) can be the active voice path. Validated
/// against [`avatar_logic::is_known_identity`] before seeding signals.
///
/// A legacy localStorage blob that pre-dates this field lacks `active_identity`
/// and therefore fails `serde_json::from_str` (no `#[serde(default)]`), causing
/// hydration to fall back to `AvatarPrefs::default()` where
/// `active_identity = "orb_classic"` — backward-safe, no migration needed.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct AvatarPrefs {
    /// `false` = show orb (default, D-01); `true` = show avatar.
    pub enabled: bool,
    /// ID of the selected head preset. Must be a valid `PRESET_REGISTRY` id.
    /// Default: `"facecap"`. Unknown ids fall back to `"facecap"` at call sites.
    pub head_id: String,
    /// Phase 40.5 (D-17): Active communication-path identity slug.
    ///
    /// May be an orb preset id (`"orb_classic"`, `"orb_bloom"`, …) or a head
    /// preset id (`"facecap"`, `"groovy"`). Validated with `is_known_identity`
    /// before use; unknown slugs fall back to `"orb_classic"` at call sites.
    ///
    /// Plans 03 and 08 freeze this value at session start to select TTS voice
    /// and realtime voice for the turn (D-12: locked for session).
    pub active_identity: String,
}

impl Default for AvatarPrefs {
    fn default() -> Self {
        Self {
            enabled: false,
            head_id: "facecap".to_string(),
            active_identity: "orb_classic".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// AccentColor (D-13)
// ---------------------------------------------------------------------------

/// Five accent presets exposed by the Tweaks panel.
///
/// `hex_pair` returns `(primary, hover)` colour pairs sourced from
/// `site.css` line 21 (`--teal: #39c5cf`) plus the prototype's accent
/// swatches.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum AccentColor {
    #[default]
    Teal,
    Orange,
    Green,
    Violet,
    Amber,
}

impl AccentColor {
    /// Returns `(base, hover)` RGB hex pair for this accent (D-13).
    pub fn hex_pair(self) -> (&'static str, &'static str) {
        match self {
            AccentColor::Teal => ("#39c5cf", "#56d4dd"),
            AccentColor::Orange => ("#f0883e", "#ffa657"),
            AccentColor::Green => ("#3fb950", "#56d364"),
            AccentColor::Violet => ("#a370f7", "#bf8bff"),
            AccentColor::Amber => ("#d29922", "#e3b341"),
        }
    }
}

// ---------------------------------------------------------------------------
// Density
// ---------------------------------------------------------------------------

/// Per-row vertical density toggle exposed by the Tweaks panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Density {
    #[default]
    Comfy,
    Dense,
}

// ---------------------------------------------------------------------------
// localStorage helpers (RESEARCH Pattern 5)
// ---------------------------------------------------------------------------
//
// Plan 02 cannot extend `web-sys` to enable the `Storage` feature (the
// orchestrator's success-criteria forbid Cargo.toml edits — that's Plan
// 01's territory and Wave 1 runs in parallel worktrees). To compile against
// the base commit without that feature, we reach `window.localStorage`
// through `js_sys::Reflect` instead of `web_sys::Window::local_storage`.
//
// Both code paths read/write the same DOM `Storage` object, so behaviour
// matches the RESEARCH §Security T-DESERIALIZE mitigation: corrupt blobs
// silently fall back to `T::default()` via `.ok()` rather than panicking.

#[cfg(target_arch = "wasm32")]
mod storage {
    use wasm_bindgen::JsValue;

    /// Return the global `window.localStorage` `JsValue`, or `None` if the
    /// runtime is non-browser or storage is disabled by the user.
    pub(super) fn ls() -> Option<JsValue> {
        let window = web_sys::window()?;
        let val = js_sys::Reflect::get(&window, &JsValue::from_str("localStorage")).ok()?;
        if val.is_undefined() || val.is_null() {
            None
        } else {
            Some(val)
        }
    }

    pub(super) fn get_item(key: &str) -> Option<String> {
        let ls = ls()?;
        let get_item = js_sys::Reflect::get(&ls, &JsValue::from_str("getItem")).ok()?;
        let f: js_sys::Function = get_item.dyn_into().ok()?;
        let result = f.call1(&ls, &JsValue::from_str(key)).ok()?;
        if result.is_null() || result.is_undefined() {
            None
        } else {
            result.as_string()
        }
    }

    pub(super) fn set_item(key: &str, val: &str) {
        let Some(ls) = ls() else { return };
        let Ok(set_item) = js_sys::Reflect::get(&ls, &JsValue::from_str("setItem")) else {
            return;
        };
        if let Ok(f) = set_item.dyn_into::<js_sys::Function>() {
            let _ = f.call2(&ls, &JsValue::from_str(key), &JsValue::from_str(val));
        }
    }

    // Required for `JsValue::dyn_into` to resolve.
    use wasm_bindgen::JsCast as _;
}

/// Read a JSON-serialised value at `key`. Returns `None` on missing key,
/// non-browser host, or any deserialisation error (per T-DESERIALIZE
/// mitigation — corrupt blobs fall back to the caller's default).
#[cfg(target_arch = "wasm32")]
pub fn read_json<T: serde::de::DeserializeOwned>(key: &str) -> Option<T> {
    let raw = storage::get_item(key)?;
    serde_json::from_str(&raw).ok()
}

/// Serialise `val` as JSON and write it to `key`. Silently no-ops if
/// serialisation fails or localStorage is unavailable.
#[cfg(target_arch = "wasm32")]
pub fn write_json<T: serde::Serialize>(key: &str, val: &T) {
    if let Ok(s) = serde_json::to_string(val) {
        storage::set_item(key, &s);
    }
}

/// Read a raw string value at `key`. Returns `None` on missing key or
/// non-browser host.
#[cfg(target_arch = "wasm32")]
pub fn read_string(key: &str) -> Option<String> {
    storage::get_item(key)
}

/// Write a raw string value to `key`. Silently no-ops on non-browser host.
#[cfg(target_arch = "wasm32")]
pub fn write_string(key: &str, val: &str) {
    storage::set_item(key, val);
}

/// Phase 47.3 Plan 06 (D-17): stash the composer's unsent text before a
/// session-death redirect. Built directly on the existing `write_string`
/// helper above (same `cfg(target_arch = "wasm32")` gate, own dedicated
/// key) — no new storage primitive. A no-op when `text` is empty: "when no
/// unsent composer draft exists at session death, nothing is stashed"
/// (must_haves truth) — an empty stash would otherwise make
/// `restore_composer_draft` indistinguishable from "nothing to restore".
///
/// Best-effort by construction: `write_string` never surfaces a failure
/// (quota exceeded / storage disabled both silently no-op inside
/// `storage::set_item`). This IS the chosen D-17 backstop behavior for that
/// edge case — the session-death redirect below always proceeds regardless
/// of whether this write actually landed, since blocking the redirect on a
/// storage failure would strand the operator on an already-dead session,
/// which is strictly worse than losing an unsent draft.
pub fn stash_composer_draft(text: &str) {
    if text.is_empty() {
        return;
    }
    write_string(KEY_SESSION_DRAFT, text);
}

/// Phase 47.3 Plan 06 (D-17): restore a stashed composer draft after
/// re-authentication. Returns `None` when nothing was stashed (either no
/// draft ever existed, or a previous call already consumed it).
///
/// Consumes (clears) the draft on read — a restored draft must not
/// resurrect on every subsequent load. Clearing is done by overwriting with
/// an empty string (there is no localStorage `removeItem` helper in this
/// module; an empty string is treated identically to "absent" here since
/// `stash_composer_draft` never writes one).
pub fn restore_composer_draft() -> Option<String> {
    let draft = read_string(KEY_SESSION_DRAFT)?;
    if draft.is_empty() {
        return None;
    }
    write_string(KEY_SESSION_DRAFT, "");
    Some(draft)
}

// Non-WASM stubs: keep the public signatures so `cargo test` on the host
// target (where unit tests run) links cleanly. Callers that try to use
// these on native get well-typed no-ops.

#[cfg(not(target_arch = "wasm32"))]
pub fn read_json<T: serde::de::DeserializeOwned>(_key: &str) -> Option<T> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub fn write_json<T: serde::Serialize>(_key: &str, _val: &T) {}

#[cfg(not(target_arch = "wasm32"))]
pub fn read_string(_key: &str) -> Option<String> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub fn write_string(_key: &str, _val: &str) {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_prefs_default_matches_plan_spec() {
        let p = UiPrefs::default();
        assert_eq!(p.accent, AccentColor::Teal);
        assert_eq!(p.wheel_size, 240.0);
        assert!(p.breadcrumb);
        assert!(p.footer);
        assert_eq!(p.density, Density::Comfy);
        assert!(p.rail);
    }

    #[test]
    fn round_trip_via_serde_json() {
        // Plan 09 Wave-0 contract: UiPrefs::default() serialises and
        // deserialises through serde_json without data loss. Test name is
        // grep-locked by VALIDATION.md line 64.
        let original = UiPrefs::default();
        let json = serde_json::to_string(&original).expect("serialize UiPrefs::default()");
        let parsed: UiPrefs = serde_json::from_str(&json).expect("deserialize UiPrefs JSON blob");
        assert_eq!(parsed, original);
    }

    #[test]
    fn unknown_field_falls_back_to_default() {
        // T-26.2.1-05 mitigation: a partial / malformed UiPrefs JSON blob
        // must fail to deserialize (returning Err) so hydration's `.ok()`
        // swallows the error and falls back to `UiPrefs::default()`. If
        // serde silently filled in missing fields, a tampered blob could
        // partially overwrite live prefs.
        let partial = r#"{ "accent": "Teal" }"#;
        let result: Result<UiPrefs, _> = serde_json::from_str(partial);
        assert!(
            result.is_err(),
            "partial UiPrefs JSON should fail to deserialize; got {result:?}"
        );
    }

    #[test]
    fn legacy_scanlines_blob_round_trips_without_panic() {
        // GAP-26.2.1-07-R3-FEATURE-REMOVAL migration test (Plan 15):
        // Existing users who hydrated under Plan 14 have a localStorage
        // `ih.ui.tweaks` blob that includes `"scanlines": true`. After Plan 15
        // removes the field, the legacy blob must still deserialize successfully
        // — serde's default `deny_unknown_fields = false` posture silently
        // ignores the unknown `scanlines` key. Per D-26.2.1-15-C, we do NOT add
        // `#[serde(default)]` to the struct; this test asserts the no-change
        // posture is sufficient.
        let legacy_blob = serde_json::json!({
            "accent": "Teal",
            "wheel_size": 240.0,
            "scanlines": true,        // ← removed in Plan 15; must be ignored
            "breadcrumb": true,
            "footer": true,
            "density": "Comfy",
            "rail": true,
        })
        .to_string();
        let parsed: UiPrefs = serde_json::from_str(&legacy_blob)
            .expect("legacy blob with scanlines key must deserialize after Plan 15");
        assert_eq!(parsed, UiPrefs::default());
    }

    #[test]
    fn accent_color_teal_is_the_canonical_pair() {
        // D-13: default accent is teal `#39c5cf` (site.css line 21).
        assert_eq!(AccentColor::Teal.hex_pair(), ("#39c5cf", "#56d4dd"));
    }

    #[test]
    fn accent_color_all_variants_have_distinct_hex_pairs() {
        let pairs = [
            AccentColor::Teal.hex_pair(),
            AccentColor::Orange.hex_pair(),
            AccentColor::Green.hex_pair(),
            AccentColor::Violet.hex_pair(),
            AccentColor::Amber.hex_pair(),
        ];
        // Every base hex must be unique.
        for i in 0..pairs.len() {
            for j in (i + 1)..pairs.len() {
                assert_ne!(pairs[i].0, pairs[j].0);
            }
        }
    }

    #[test]
    fn density_default_is_comfy() {
        assert_eq!(Density::default(), Density::Comfy);
    }

    #[test]
    fn persistence_keys_are_namespaced_under_ih_ui() {
        assert_eq!(KEY_TWEAKS, "ih.ui.tweaks");
        assert_eq!(KEY_THEME, "ih.ui.theme");
        assert_eq!(KEY_WHEEL, "ih.ui.wheel");
    }

    /// Phase 47.3 Plan 06 (D-17): the session-draft key must be textually
    /// distinct from the session cookie name so a source-level scan can
    /// never mistake one for the other.
    #[test]
    fn session_draft_key_is_distinct_from_session_cookie_name() {
        assert_eq!(KEY_SESSION_DRAFT, "ih.ui.session_expiry_draft");
        assert_ne!(KEY_SESSION_DRAFT, "ih_session");
    }

    /// Phase 47.3 Plan 06 (D-17): on the host (non-wasm) target, stash/
    /// restore resolve to the no-op stub branch — verify they don't panic
    /// and behave as documented (nothing stashed = nothing restored).
    #[test]
    fn stash_and_restore_composer_draft_host_stubs_are_no_ops() {
        stash_composer_draft("unsent message");
        assert!(restore_composer_draft().is_none());
    }

    /// Phase 47.3 Plan 06 (D-17 must_haves): stashing empty text is a no-op
    /// — this is exercised at the API-contract level here (the host stub
    /// can't distinguish "no-op because empty" from "no-op because
    /// non-wasm", but the empty-string early return is dead-simple enough
    /// that this test documents the contract regardless of target).
    #[test]
    fn stash_composer_draft_empty_text_is_a_documented_no_op() {
        stash_composer_draft("");
        assert!(restore_composer_draft().is_none());
    }

    #[test]
    fn host_target_stubs_are_no_ops() {
        // On the host target these resolve to the stub branch above —
        // verify they don't panic and return the documented sentinels.
        let v: Option<UiPrefs> = read_json(KEY_TWEAKS);
        assert!(v.is_none());
        write_json(KEY_TWEAKS, &UiPrefs::default());
        assert!(read_string(KEY_THEME).is_none());
        write_string(KEY_THEME, "slate-dark");
    }

    // --- Phase 40.2 Plan 01 Task 1: AvatarPrefs tests (RED) ---

    #[test]
    fn avatar_prefs_default() {
        // FE-01: default is orb-mode (enabled=false) with facecap head (D-01).
        // Phase 40.5 (D-17): active_identity defaults to "orb_classic".
        let p = AvatarPrefs::default();
        assert!(!p.enabled);
        assert_eq!(p.head_id, "facecap");
        assert_eq!(p.active_identity, "orb_classic");
    }

    #[test]
    fn avatar_prefs_missing_active_identity_is_err() {
        // Phase 40.5 backward-safety: a legacy blob that lacks active_identity must
        // fail serde so hydration falls back to AvatarPrefs::default().
        // This exercises the T-40.2-01-01 no-#[serde(default)] invariant for the
        // new field: partial blobs are rejected, not silently accepted.
        let result: Result<AvatarPrefs, _> =
            serde_json::from_str(r#"{"enabled":false,"head_id":"facecap"}"#);
        assert!(
            result.is_err(),
            "blob missing active_identity must fail deserialization; got {result:?}"
        );
    }

    #[test]
    fn avatar_prefs_round_trip() {
        // FE-01: AvatarPrefs round-trips losslessly through serde_json.
        let p = AvatarPrefs::default();
        let json = serde_json::to_string(&p).unwrap();
        let q: AvatarPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(p, q);
    }

    #[test]
    fn avatar_key_namespaced() {
        // FE-01: localStorage key uses the ih.ui.* namespace (mirrors KEY_TWEAKS/KEY_WHEEL).
        assert_eq!(KEY_AVATAR, "ih.ui.avatar");
    }

    #[test]
    fn avatar_prefs_partial_blob_is_err() {
        // T-40.2-01-01 mitigation: a partial blob (missing head_id) must fail
        // deserialization so hydration's .ok() falls back to AvatarPrefs::default().
        // AvatarPrefs must NOT use #[serde(default)] on fields.
        let result: Result<AvatarPrefs, _> = serde_json::from_str(r#"{"enabled":true}"#);
        assert!(
            result.is_err(),
            "partial AvatarPrefs JSON (missing head_id) must fail deserialization; got {result:?}"
        );
    }
}
