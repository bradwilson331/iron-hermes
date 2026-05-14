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
        let get_item =
            js_sys::Reflect::get(&ls, &JsValue::from_str("getItem")).ok()?;
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
        let parsed: UiPrefs =
            serde_json::from_str(&json).expect("deserialize UiPrefs JSON blob");
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
}
