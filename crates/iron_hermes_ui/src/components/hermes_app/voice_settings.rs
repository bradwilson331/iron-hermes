//! Phase 36.17.10 Plan 02 — Server-fn-backed voice settings panel.
//!
//! Replaces dead local signals from Plan 36.17.9 D-13 with a real, server-backed
//! form bound to Plan 01's `get_voice_config` / `update_voice_config`. Renders
//! VOICE SERVICE → the Realtime and Turn-based family groups (Phase 54.7 Plan 08;
//! `voice_realtime.rs` / `voice_turn_based.rs`, both always mounted) → VAD TIMING
//! (Plan 03) → WAKE WORD → AVATAR.
//!
//! # Context provided
//! - `WakeWordEnabledCtx` — wake-word toggle state (newtype, Plan 04 seam)
//! - `WakeWordPhraseCtx`  — wake-word phrase string (newtype, Plan 04 seam)
//! - `AutoTtsCtx`, `SttProviderCtx`, `SttModelCtx`, `TtsProviderCtx`,
//!   `TtsVoiceCtx`, `TtsFormatCtx`, `SilenceDurationCtx`,
//!   `WebSilenceThresholdCtx`, `MaxRecordingCtx`, `BeepEnabledCtx`
//!
//! # Signal borrow discipline (Pattern B — clippy.toml enforced)
//! Never hold a `.read()` / `.write()` lock across an `.await` boundary.
//! Read all signal values into owned Copy/Clone locals BEFORE `spawn(async move {…})`.

// These voice-settings context newtypes are constructed at the HermesApp root and
// consumed by the web voice overlay. Native (server/desktop) dead-code analysis
// cannot reach those web-gated provide/consume sites, so it reports them as never
// constructed. Silence native-only dead_code without deleting web-live code; the
// wasm build still lints dead_code normally.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use dioxus::prelude::*;
use std::collections::HashMap;

use crate::components::hermes_app::voice_fields::{ListCache, OptionPickerField, VoiceField};
use crate::components::hermes_app::voice_realtime::{
    use_realtime_form, RealtimeFamily, RealtimeForm,
};
use crate::components::hermes_app::voice_test_controls;
use crate::components::hermes_app::voice_turn_based::{
    use_turn_based_form, TurnBasedFamily, TurnBasedForm,
};
use crate::components::hermes_app::VoiceStatusState;
use crate::server::api::{
    get_voice_config, update_voice_config, IdentityOverride, TtsTestSettings, VoiceConfigSnapshot,
    VoiceWritePayload,
};
use crate::voice_registry::{
    stt_provider, tts_provider, validate_model_id, validate_voice_id, FieldError, OptionKind,
    ProviderSpec, REALTIME_PROVIDERS, TTS_PROVIDERS,
};

/// Registry default model for an STT provider id (empty when the provider has
/// no model field). D-06: the panel never seeds a model literal of its own.
pub(super) fn stt_default_model(id: &str) -> String {
    stt_provider(id)
        .map(|s| s.default_model.to_string())
        .unwrap_or_default()
}

// ── Context newtypes (Pattern D) ─────────────────────────────────────────────
// Each new Signal<T> provided via context MUST use a newtype wrapper to avoid
// use_context::<Signal<T>>() collisions (RESEARCH Pitfall 2, CONTEXT constraint).

/// Newtype wrapping the wake-word enabled toggle so `use_context` lookups
/// are unambiguous (avoids collision with other `Signal<bool>` contexts).
#[derive(Clone, Copy, PartialEq)]
pub struct WakeWordEnabledCtx(pub Signal<bool>);

/// Newtype wrapping the wake-word phrase string for context disambiguation.
#[derive(Clone, Copy, PartialEq)]
pub struct WakeWordPhraseCtx(pub Signal<String>);

/// Auto-TTS (speak every reply) toggle — maps to `config.voice.auto_tts`.
#[derive(Clone, Copy, PartialEq)]
pub struct AutoTtsCtx(pub Signal<bool>);

/// Phase 54.7 Plan 08 (D-14/D-15): the staged voice service
/// (`"realtime"` | `"turn_based"`). Replaces `BargeInModeCtx`: the family
/// choice is no longer derived from `barge_in_mode`. Provided ONLY at the
/// HermesApp root (mod.rs) and seeded once from `get_voice_config.voice_service`;
/// the panel edits it (staged until SAVE SETTINGS) and voice-mode entry reads it.
/// Newtype required to avoid `use_context::<Signal<String>>` collisions (Pattern D).
#[derive(Clone, Copy, PartialEq)]
pub struct VoiceServiceCtx(pub Signal<String>);

/// Debug session `avatar-not-hearing-voice` (carried over, 54.7 Plan 08): flips
/// true exactly once, after the HermesApp root's config fetch settles (Ok OR
/// Err) and seeds `VoiceServiceCtx`. Read REACTIVELY (`.read()`, not
/// `.peek()`) by voice_mode.rs's entry effect so that effect defers its
/// (spawn-exactly-once) branch decision instead of deciding on
/// `VoiceServiceCtx`'s default before the seed lands. Root-provided only.
#[derive(Clone, Copy, PartialEq)]
pub struct VoiceServiceLoadedCtx(pub Signal<bool>);

/// Phase 36.17.12 Plan 01: Realtime-degraded flag — true when the realtime
/// service was selected but the realtime session failed to start (D-07
/// fallback). Plan 03 sets this from the voice mode screen; this panel reads
/// it to show the non-error degrade hint. Default false.
#[derive(Clone, Copy, PartialEq)]
pub struct RealtimeDegradedCtx(pub Signal<bool>);

/// Phase 39.3 Plan 05 (D-03): approval-pending context for the realtime voice
/// approval card. Carries `Some(ApprovalPendingInfo{...})` while a tool call
/// awaits operator Approve/Deny; `None` at rest and after resolution.
///
/// Newtype required to disambiguate from bare `Signal<Option<T>>` at context lookup
/// (Pattern D). Provided at HermesApp root (mod.rs) — MUST NOT be provided in any
/// child component (voice_mode.rs, voice_settings.rs) or ancestor/sibling consumers
/// panic with "could not find context" (Dioxus context-panic rule per MEMORY.md).
#[derive(Clone, Copy, PartialEq)]
pub struct RealtimeApprovalCtx(
    pub Signal<Option<crate::components::hermes_app::realtime_session::ApprovalPendingInfo>>,
);

/// Phase 39.3 Plan 05 (D-05a): in-flight badge context. True when a background
/// async tool/research turn is running in the realtime session; false at rest.
///
/// Set true by `realtime_session.rs` when a function_call relay starts; cleared to
/// false on Output (auto-approved path), on Rejected, on server error, or (in
/// voice_mode.rs) after the Approve/Deny handler resolves.
///
/// Newtype required for `Signal<bool>` context disambiguation (Pattern D).
/// Provided at HermesApp root (mod.rs) alongside RealtimeApprovalCtx.
#[derive(Clone, Copy, PartialEq)]
pub struct RealtimeInFlightCtx(pub Signal<bool>);

/// VAD silence duration (seconds) — maps to `config.voice.silence_duration`.
#[derive(Clone, Copy, PartialEq)]
pub struct SilenceDurationCtx(pub Signal<f64>);

/// Web Audio RMS silence threshold — maps to `config.voice.web_silence_threshold_rms`.
/// DISTINCT from `silence_threshold: i32` (native PCM domain) — never aliased.
#[derive(Clone, Copy, PartialEq)]
pub struct WebSilenceThresholdCtx(pub Signal<f32>);

/// STT provider name — maps to `config.stt.provider`.
#[derive(Clone, Copy, PartialEq)]
pub struct SttProviderCtx(pub Signal<String>);

/// Active STT model string (derived from provider + per-provider model).
#[derive(Clone, Copy, PartialEq)]
pub struct SttModelCtx(pub Signal<String>);

/// TTS provider name — maps to `config.tts.provider`.
#[derive(Clone, Copy, PartialEq)]
pub struct TtsProviderCtx(pub Signal<String>);

/// TTS voice string — maps to `config.tts.edge.voice`.
#[derive(Clone, Copy, PartialEq)]
pub struct TtsVoiceCtx(pub Signal<String>);

/// TTS output format — maps to `config.tts.edge.output_format`.
#[derive(Clone, Copy, PartialEq)]
pub struct TtsFormatCtx(pub Signal<String>);

/// Max recording seconds — maps to `config.voice.max_recording_seconds`.
#[derive(Clone, Copy, PartialEq)]
pub struct MaxRecordingCtx(pub Signal<u32>);

/// Beep-on-listen-start toggle — maps to `config.voice.beep_enabled`.
#[derive(Clone, Copy, PartialEq)]
pub struct BeepEnabledCtx(pub Signal<bool>);

/// Phase 40.2 Plan 04 (FE-01, D-03): avatar-mode preferences context.
///
/// Wraps `Signal<AvatarPrefs>` so `use_context::<AvatarModeCtx>()` lookups are
/// unambiguous (avoids collision with other `Signal<T>` contexts — Pattern D).
///
/// MUST be provided at the HermesApp root (mod.rs), NOT in any child component.
/// A child provider panics ancestor/sibling consumers at runtime (Dioxus context-
/// panic rule / MEMORY.md). Consume here; provide only in mod.rs.
#[derive(Clone, Copy, PartialEq)]
pub struct AvatarModeCtx(pub Signal<crate::ui_prefs::AvatarPrefs>);

/// Phase 40.2 Plan 04 (FE-05, D-11): per-session avatar-error notice context.
///
/// Set to `true` by `orb_canvas.rs` when `window.__ihAvatarError` fires;
/// consumed by `voice_mode.rs` to render the one-time "Avatar unavailable"
/// notice. Ephemeral — never persisted to localStorage (once-per-session).
///
/// Provided at HermesApp root (mod.rs) alongside AvatarModeCtx.
#[derive(Clone, Copy, PartialEq)]
pub struct AvatarErrorNoticeCtx(pub Signal<bool>);

// ── Phase 40.5 Plan 01 context newtypes ──────────────────────────────────────

/// Phase 40.5 (D-05): the identity slug being edited in the voice-settings panel.
///
/// Defaults to the `active_identity` from `AvatarPrefs` at session start. Plans
/// 02 and 06 use this to drive the per-identity form sections without mutating
/// the live `active_identity` until the user explicitly confirms.
///
/// Pattern D: newtype avoids collision with other `Signal<String>` contexts.
/// Provided at HermesApp root (mod.rs). MUST NOT be provided in a child component.
#[derive(Clone, Copy, PartialEq)]
pub struct VoiceEditTargetCtx(pub Signal<String>);

/// Phase 40.5 (D-07): per-identity TTS provider override (free-mode).
///
/// `None` = inherit global `config.tts.provider`; `Some(name)` = override for
/// this identity. Written by Plan 02's identity voice-settings form; read by
/// Plan 03's `build_tts_registry` call-site via `resolve_tts_override`.
///
/// Pattern D: newtype disambiguates `Signal<Option<String>>` at context lookup.
#[derive(Clone, Copy, PartialEq)]
pub struct IdentityTtsProviderCtx(pub Signal<Option<String>>);

/// Phase 40.5 (D-07): per-identity TTS voice override (free-mode).
///
/// `None` = inherit global voice for the active provider; `Some(voice)` = use
/// this voice string when synthesising for this identity. Mirrors the shape of
/// `IdentityVoiceProfile.free_mode_tts_voice` in `ironhermes-core`.
///
/// Pattern D: disambiguates `Signal<Option<String>>` at context lookup.
#[derive(Clone, Copy, PartialEq)]
pub struct IdentityTtsVoiceCtx(pub Signal<Option<String>>);

/// Phase 40.5 (D-07): per-identity realtime voice override.
///
/// `None` = use the global realtime voice from `config.voice`; `Some(voice)` =
/// use this OpenAI realtime voice for this identity. Frozen at session start
/// (D-12) in Plan 03/08.
///
/// Pattern D: disambiguates `Signal<Option<String>>` at context lookup.
#[derive(Clone, Copy, PartialEq)]
pub struct IdentityRealtimeVoiceCtx(pub Signal<Option<String>>);

/// Phase 54.7 Plan 15 (D-16): per-identity TTS model override.
///
/// `None` = inherit the global block's model for the identity's provider;
/// `Some(model)` = the model this identity speaks with. Maps to
/// `IdentityVoiceProfile.free_mode_tts_model`.
///
/// Pattern D newtype. Provided at the HermesApp root (mod.rs) ONLY; a child
/// provider panics sibling consumers (Dioxus context-panic rule).
#[derive(Clone, Copy, PartialEq)]
pub struct IdentityTtsModelCtx(pub Signal<Option<String>>);

/// Phase 54.7 Plan 15 (D-16): per-identity TTS output-format override. `None` =
/// inherit. Maps to `IdentityVoiceProfile.free_mode_tts_format`.
///
/// Pattern D newtype. Root-provided only (see [`IdentityTtsModelCtx`]).
#[derive(Clone, Copy, PartialEq)]
pub struct IdentityTtsFormatCtx(pub Signal<Option<String>>);

/// Phase 54.7 Plan 15 (D-16): per-identity realtime model override. `None` =
/// inherit `realtime.model`. Maps to `IdentityVoiceProfile.realtime_model`.
///
/// Pattern D newtype. Root-provided only (see [`IdentityTtsModelCtx`]).
#[derive(Clone, Copy, PartialEq)]
pub struct IdentityRealtimeModelCtx(pub Signal<Option<String>>);

/// Phase 40.5 (D-04): orb render-style key for the current identity.
///
/// One of `"classic"` | `"bloom"` | `"ascii"` | `"network"`. Hydrated from
/// `OrbPreset.default_style` in `ORB_PRESET_REGISTRY` when the identity is an
/// orb preset; left as the global default `"classic"` for head-rig identities.
///
/// Pattern D: disambiguates `Signal<String>` at context lookup.
#[derive(Clone, Copy, PartialEq)]
pub struct OrbStyleCtx(pub Signal<String>);

/// Phase 40.5 (D-04): orb base hue (0–359°) for the current identity.
///
/// Hydrated from `OrbPreset.default_hue`; overridden by a user-saved hue in
/// `IdentityRecord.appearance.base_hue`. Plan 05 binds a hue picker to this
/// signal so the orb canvas re-renders live.
///
/// Pattern D: disambiguates `Signal<u16>` at context lookup.
#[derive(Clone, Copy, PartialEq)]
pub struct OrbBaseHueCtx(pub Signal<u16>);

/// Phase 40.5 (D-04): orb scale factor for the current identity.
///
/// Hydrated from `OrbPreset.default_size`; range `[0.5, 2.0]`. Plan 05 binds
/// a size slider to this signal so the orb canvas re-renders live.
///
/// Pattern D: disambiguates `Signal<f32>` (size vs glow) at context lookup.
#[derive(Clone, Copy, PartialEq)]
pub struct OrbSizeCtx(pub Signal<f32>);

/// Phase 40.5 (D-04): orb glow intensity for the current identity.
///
/// Hydrated from `OrbPreset.default_glow`; range `[0.0, 1.0]`. Plan 05 binds
/// a glow slider to this signal so the orb canvas re-renders live.
///
/// Pattern D: disambiguates `Signal<f32>` (glow vs size) at context lookup.
#[derive(Clone, Copy, PartialEq)]
pub struct OrbGlowCtx(pub Signal<f32>);

/// Phase 41.2 Plan 01 (D-03): orb style-switch "settling" flag.
///
/// `true` while the `OrbStyleCtx` → `setStyle` bridge (`orb_canvas.rs`) is
/// mid-rebuild — the yield-before-eval + `use_resource` bridge sets this
/// true immediately on a style change and false once the rebuild settles
/// (eval resolves, or the bounded fallback timer fires). Drives the
/// transient "Switching style…" label in `OrbAppearanceSection` — never
/// persisted.
///
/// Pattern D: disambiguates from other `Signal<bool>` contexts.
#[derive(Clone, Copy, PartialEq)]
pub struct OrbSettlingCtx(pub Signal<bool>);

/// Phase 40.5 (D-09): wake-word / hands-free session active flag.
///
/// `true` while the wake-word listen loop is active (the user is in a hands-free
/// voice session). Set by voice_mode.rs; consumed by Plans 03/07 to gate TTS
/// synthesis routing decisions (don't swap identity mid-session — D-12).
///
/// Pattern D: disambiguates from other `Signal<bool>` contexts.
#[derive(Clone, Copy, PartialEq)]
pub struct WakeSessionActiveCtx(pub Signal<bool>);

/// Phase 40.5 (D-09): stop-signal for the wake-word listen loop.
///
/// Set to `true` by UI controls to request a clean loop exit; the loop polls
/// this signal on each iteration and exits when it sees `true`. The UI resets
/// it to `false` before starting the next session. Plan 07 provides stop logic.
///
/// Pattern D: disambiguates from other `Signal<bool>` contexts.
#[derive(Clone, Copy, PartialEq)]
pub struct WakeSessionStopCtx(pub Signal<bool>);

/// Phase 40.5 (D-10): audio-playback active flag.
///
/// `true` while TTS audio is being played back through the web audio graph.
/// The orb canvas reads this via context to trigger the "speaking" pulsation
/// animation variant (D-04 orb speaking state). Ephemeral — never persisted.
///
/// Pattern D: disambiguates from other `Signal<bool>` contexts.
#[derive(Clone, Copy, PartialEq)]
pub struct AudioPlaybackActiveCtx(pub Signal<bool>);

// ── Reset-fight guard (Phase 41.2 Plan 01) ─────────────────────────────────

/// Phase 41.2 Plan 01: reset-fight guard predicate for the `edit_target`
/// hydrate effect inside `VoiceSettings` below.
///
/// `Signal::set()` notifies subscribers on every call, including a no-op
/// re-set to the SAME value (Dioxus does not skip notification on
/// value-equality by default). The hydrate effect subscribes to
/// `edit_target_sig` and unconditionally re-derives + `.set()`s the orb
/// appearance signals from the saved override/preset default on every run —
/// so a UI change elsewhere that happens to re-`.set()` the edit-target to
/// its OWN current value can silently snap a just-picked Style back to the
/// preset default (RESEARCH.md Pitfall 3). This pure fn is the "should this
/// re-derivation actually run" decision, extracted so it is unit-testable
/// outside wasm.
///
/// Returns `true` only on a REAL switch (`prev != next`).
fn should_rehydrate_appearance(prev: &str, next: &str) -> bool {
    prev != next
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod should_rehydrate_appearance_tests {
    use super::should_rehydrate_appearance;

    /// A no-op re-set (edit_target re-set to the SAME value it already held)
    /// must be skipped — this is the reset-fight guard's whole purpose.
    #[test]
    fn should_rehydrate_appearance_skips_noop_reset() {
        assert!(!should_rehydrate_appearance("bloom", "bloom"));
    }

    /// A real target switch must still re-derive appearance.
    #[test]
    fn should_rehydrate_appearance_runs_on_real_switch() {
        assert!(should_rehydrate_appearance("bloom", "classic"));
    }
}

// ── Identity activation control (Phase 41.2 Plan 03, D-01/D-02) ────────────

/// Phase 41.2 Plan 03: which state `SetActiveIdentityControl` renders for the
/// identity currently in the edit-target.
///
/// `Passive` when the edit-target IS already `AvatarPrefs.active_identity`
/// (renders as a "{Name} is your active identity ✓" readout, not a button).
/// `Actionable` otherwise (renders as a "Set {Name} as active" button).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationState {
    Actionable,
    Passive,
}

/// D-01/D-02: pure decision fn — no Dioxus/signal types in its signature —
/// so it is unit-testable outside wasm. `edit_target == active_identity`
/// selects `Passive`; any inequality selects `Actionable`.
fn activation_state(edit_target: &str, active_identity: &str) -> ActivationState {
    if edit_target == active_identity {
        ActivationState::Passive
    } else {
        ActivationState::Actionable
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod activation_state_tests {
    use super::{activation_state, ActivationState};

    /// Edit-target already active → passive confirmation, not a button.
    #[test]
    fn activation_state_passive_when_edit_target_is_active() {
        assert_eq!(activation_state("bloom", "bloom"), ActivationState::Passive);
    }

    /// Edit-target differs from active identity → actionable button.
    #[test]
    fn activation_state_actionable_when_edit_target_not_active() {
        assert_eq!(
            activation_state("bloom", "orb_classic"),
            ActivationState::Actionable
        );
    }
}

// ── Voice service (Phase 54.7 Plan 08, D-14/D-15) ───────────────────────────

/// The two voice services in segment order (Realtime first). `(id, label,
/// DOM id)`; the labels are fixed strings so a segment never wraps.
const SERVICE_SEGMENTS: [(&str, &str, &str); 2] = [
    ("realtime", "Realtime", "voice-service-realtime"),
    ("turn_based", "Turn-based", "voice-service-turn-based"),
];

/// Roving-selection step for the two-segment `Voice service` radiogroup.
/// ArrowRight / ArrowDown advance and wrap, ArrowLeft / ArrowUp go back and
/// wrap, Home jumps to the first, End to the last; any other key leaves the
/// selection alone.
fn service_seg_next(current: usize, key: &str) -> usize {
    let last = SERVICE_SEGMENTS.len() - 1;
    match key {
        "ArrowRight" | "ArrowDown" => {
            if current >= last {
                0
            } else {
                current + 1
            }
        }
        "ArrowLeft" | "ArrowUp" => {
            if current == 0 {
                last
            } else {
                current - 1
            }
        }
        "Home" => 0,
        "End" => last,
        _ => current,
    }
}

/// Clamp any stored service string to a value the server accepts and the
/// entry gate understands (`realtime` or `turn_based`).
fn normalize_service(raw: &str) -> &'static str {
    if raw == "realtime" {
        "realtime"
    } else {
        "turn_based"
    }
}

/// The plain values of every GLOBAL (non-identity) field the save handler
/// sends EXCEPT the two voice families' fields, which their own form objects
/// ([`TurnBasedForm`], [`RealtimeForm`]) own. Seeded from a snapshot by
/// [`GlobalVoiceValues::from_snapshot`] (the panel's seed effect uses it too,
/// so a test seeds exactly as the panel does) and turned into a payload by
/// [`global_voice_payload`] / [`assemble_voice_payload`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GlobalVoiceValues {
    pub auto_tts: bool,
    /// The stored/staged interruption mode (form state for the Interruption row).
    pub barge_in_mode: String,
    /// `realtime` | `turn_based`.
    pub voice_service: String,
    pub silence_duration: f64,
    pub web_silence_threshold_rms: f32,
    pub max_recording_seconds: u32,
    pub beep_enabled: bool,
    pub wake_word_enabled: bool,
    pub wake_word_phrase: String,
}

impl GlobalVoiceValues {
    /// Every field exactly as the panel seeds its signals from `snapshot`.
    pub(crate) fn from_snapshot(snapshot: &VoiceConfigSnapshot) -> Self {
        Self {
            auto_tts: snapshot.auto_tts,
            barge_in_mode: snapshot.barge_in_mode.clone(),
            voice_service: normalize_service(&snapshot.voice_service).to_string(),
            silence_duration: snapshot.silence_duration,
            web_silence_threshold_rms: snapshot.web_silence_threshold_rms,
            max_recording_seconds: snapshot.max_recording_seconds,
            beep_enabled: snapshot.beep_enabled,
            wake_word_enabled: snapshot.wake_word_enabled,
            wake_word_phrase: snapshot.wake_word_phrase.clone(),
        }
    }
}

/// Whether the Interruption row renders: the snapshot must offer a real choice
/// (at least two valid modes). The same predicate gates `barge_in_mode` in the
/// save payload, so a rendered row always saves what it shows and an unrendered
/// row never sends anything (no control writes a value the schema ignores).
pub(crate) fn interruption_row_visible(modes: &[String]) -> bool {
    modes.len() >= 2
}

/// The migration notice shows while the loaded snapshot still reports legacy
/// voice keys AND the edit target is Global: only a global save performs the
/// one-way rewrite (an identity save sends identity fields only).
pub(crate) fn migration_notice_visible(snapshot: &VoiceConfigSnapshot, edit_target: &str) -> bool {
    snapshot.legacy_voice_keys_present && edit_target == "global"
}

/// State transition after a successful global save: the config no longer holds
/// legacy keys, so the loaded snapshot stops reporting them and the migration
/// notice disappears.
pub(crate) fn after_save_ok(snapshot: &mut VoiceConfigSnapshot) {
    snapshot.legacy_voice_keys_present = false;
}

/// Build the global save payload (D-15). Every field the save sent before is
/// kept; `voice_service` is added. `barge_in_mode` is sent ONLY when the
/// snapshot offers a choice (at least two modes, so the Interruption row is
/// rendered) AND the staged value is one of them: the server rejects any other
/// value, so echoing a stored legacy `open_mic` would fail every save and the
/// one-way migration would never run (T-54.7-72).
pub(crate) fn global_voice_payload(
    values: &GlobalVoiceValues,
    snapshot: &VoiceConfigSnapshot,
) -> VoiceWritePayload {
    let barge_in_mode = (interruption_row_visible(&snapshot.barge_in_modes)
        && snapshot
            .barge_in_modes
            .iter()
            .any(|m| m == &values.barge_in_mode))
    .then(|| values.barge_in_mode.clone());
    VoiceWritePayload {
        auto_tts: Some(values.auto_tts),
        barge_in_mode,
        voice_service: Some(normalize_service(&values.voice_service).to_string()),
        silence_duration: Some(values.silence_duration),
        web_silence_threshold_rms: Some(values.web_silence_threshold_rms),
        max_recording_seconds: Some(values.max_recording_seconds),
        beep_enabled: Some(values.beep_enabled),
        wake_word_enabled: Some(values.wake_word_enabled),
        wake_word_phrase: Some(values.wake_word_phrase.clone()),
        ..Default::default()
    }
}

/// The full global payload the panel saves: [`global_voice_payload`] followed
/// by each family form's `apply_to`, so BOTH families' values ride every save
/// regardless of which one is visible (D-14: hidden but kept in config).
pub(crate) fn assemble_voice_payload(
    values: &GlobalVoiceValues,
    snapshot: &VoiceConfigSnapshot,
    turn_based: &TurnBasedForm,
    realtime: &RealtimeForm,
) -> VoiceWritePayload {
    let mut payload = global_voice_payload(values, snapshot);
    turn_based.apply_to(&mut payload);
    realtime.apply_to(&mut payload);
    payload
}

// ── Save bar (Phase 54.7 Plan 17: D-13/D-15/D-18) ───────────────────────────

/// The fixed line for any save failure that is not a field validation.
const SAVE_FAILED_COPY: &str = "Save failed. Check server logs.";

/// Whether SAVE SETTINGS may be pressed: something changed, no field blocks the
/// save, and no save request is already in flight (double-submit guard).
pub(crate) fn save_enabled(dirty: bool, no_blocking: bool, saving: bool) -> bool {
    dirty && no_blocking && !saving
}

/// Map a save failure to the operator-facing line (UI-SPEC "Error state
/// (save)"). Plan 06's validator reports a rejected field as
/// `voice_invalid:{field}:{code}`; a known `{code}` becomes that field's fixed
/// `FieldError` copy inside the save sentence. Anything else (an unknown code,
/// a transport or server error) is the generic line. The server text itself is
/// never echoed: only fixed copy reaches the DOM (T-54.7-38).
pub(crate) fn save_error_copy(err: &str) -> String {
    const TAG: &str = "voice_invalid:";
    let Some(at) = err.find(TAG) else {
        return SAVE_FAILED_COPY.to_string();
    };
    // `{field}:{code}`: the code is the run of identifier characters after the
    // second colon; the transport may append text after it.
    let code: String = err[at + TAG.len()..]
        .split_once(':')
        .map_or("", |(_field, code)| code)
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    match FieldError::from_code(&code) {
        Some(field_error) => {
            // The copy ends in its own period; the sentence supplies it.
            let copy = field_error.copy();
            let reason = copy.strip_suffix('.').unwrap_or(copy);
            format!(
                "Couldn't save: {reason}. Your edits are still here. Fix the field and press SAVE SETTINGS again."
            )
        }
        None => SAVE_FAILED_COPY.to_string(),
    }
}

/// The save bar: the save button, the blocking-error summary beside it, and
/// the mapped save error under it. A standalone component (not an inline block
/// of the panel) so each state has an SSR smoke test.
#[component]
fn VoiceSaveBar(
    dirty: bool,
    blocking: usize,
    saving: bool,
    saved: bool,
    #[props(default)] error: Option<String>,
    #[props(default)] legacy_keys: bool,
    on_save: EventHandler<()>,
) -> Element {
    let enabled = save_enabled(dirty, blocking == 0, saving);
    rsx! {
        div { class: "voice-settings-save-bar",
            // D-13 / D-15 are one-way: tell the operator before the first save.
            if legacy_keys {
                p { class: "voice-settings-hint", role: "note",
                    "Saving moves your realtime settings into a new realtime: block in config.yaml and sets the voice service. Older IronHermes versions won't read it."
                }
            }
            button {
                class: "voice-settings-save-btn",
                r#type: "button",
                style: if enabled { "opacity:1; cursor:pointer;" } else { "opacity:0.5; cursor:default;" },
                disabled: !enabled,
                onclick: move |_| on_save.call(()),
                if saved {
                    "Settings saved."
                } else {
                    "SAVE SETTINGS"
                }
            }
            if blocking > 0 {
                p {
                    class: "voice-settings-hint",
                    role: "status",
                    style: "color:var(--danger); margin-top:var(--space-1);",
                    "Fix {blocking} field(s) before saving."
                }
            }
            if let Some(ref err) = error {
                p {
                    class: "voice-settings-hint",
                    role: "alert",
                    style: "color:var(--danger); margin-top:var(--space-1);",
                    "{err}"
                }
            }
            // D-18: nothing needs a restart; say when a saved change lands.
            p { class: "voice-settings-hint", style: "margin-top:var(--space-1);",
                "Saved changes apply to the next voice turn or session. A turn in progress finishes on the old settings. Text-to-speech used by the agent picks up changes after Apply config now."
            }
        }
    }
}

/// The note shown under `VOICE SERVICE` while an identity is the edit target.
const IDENTITY_GLOBAL_NOTE: &str =
    "These are global settings. Switch \"Applies to\" to Global to change them.";

/// D-16: the global controls (service, both voice families, Interruption) are
/// editable only with the write gate open AND the edit target on Global. An
/// identity save sends identity fields only, so an editable global field would
/// be silently dropped.
pub(crate) fn global_fields_locked(write_enabled: bool, edit_target: &str) -> bool {
    !write_enabled || edit_target != "global"
}

/// The Realtime service streams audio continuously and has no wake-word frames,
/// so a staged Realtime service takes the wake-word controls away.
pub(crate) fn wake_word_blocked(service: &str) -> bool {
    normalize_service(service) == "realtime"
}

/// Whether the wake-word toggle is disabled: the write gate is closed or the
/// staged service cannot honour a wake word.
pub(crate) fn wake_controls_disabled(service: &str, write_enabled: bool) -> bool {
    wake_word_blocked(service) || !write_enabled
}

/// Short segment label for a `barge_in_mode` id (fits a 320px panel).
fn barge_mode_label(id: &str) -> String {
    match id {
        "push_to_interrupt" => "Agent takes turns".to_string(),
        other => {
            let spaced = other.replace('_', " ");
            let mut chars = spaced.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// `INTERRUPTION` row (UI-SPEC "Barge-in slot"): a segmented control over the
/// snapshot's valid `barge_in_mode` values. Renders NOTHING unless the snapshot
/// lists at least two (`interruption_row_visible`, the same predicate the save
/// payload uses), so it can never show a control whose value is not saved.
#[component]
fn InterruptionRow(
    modes: Vec<String>,
    current: String,
    disabled: bool,
    on_change: EventHandler<String>,
) -> Element {
    if !interruption_row_visible(&modes) {
        return rsx! {};
    }
    rsx! {
        p {
            class: "voice-settings-section-header",
            role: "heading",
            "aria-level": "3",
            "INTERRUPTION"
        }
        hr { class: "voice-settings-separator" }
        div {
            class: "voice-settings-row",
            style: "flex-direction: column; align-items: flex-start;",
            div {
                class: "voice-seg-control",
                role: "radiogroup",
                "aria-label": "Interruption",
                for mode in modes.iter().cloned() {
                    {
                        let selected = mode == current;
                        let label = barge_mode_label(&mode);
                        let value = mode.clone();
                        rsx! {
                            button {
                                key: "{mode}",
                                class: if selected { "voice-seg is-active" } else { "voice-seg" },
                                r#type: "button",
                                role: "radio",
                                "aria-checked": if selected { "true" } else { "false" },
                                disabled,
                                onclick: move |_| on_change.call(value.clone()),
                                "{label}"
                            }
                        }
                    }
                }
            }
            p { class: "voice-settings-hint", style: "margin-top:var(--space-1);",
                "Applies to the Turn-based service. Realtime always allows interrupting."
            }
        }
        hr { class: "voice-settings-separator" }
    }
}

/// WR-01: the service to put back into the root-provided [`VoiceServiceCtx`]
/// when the panel unmounts. `loaded` is the last loaded/saved service (empty
/// before the config has loaded); `staged` is the context's current value.
/// `Some(loaded)` only when an unsaved edit is being abandoned.
fn service_to_restore(loaded: &str, staged: &str) -> Option<String> {
    if loaded.is_empty() || loaded == staged {
        None
    } else {
        Some(loaded.to_string())
    }
}

/// `VOICE SERVICE` heading + `Realtime | Turn-based` segmented control
/// (D-14). The selection is STAGED in the root-provided [`VoiceServiceCtx`];
/// nothing saves until SAVE SETTINGS. Until `loaded` (the root's
/// [`VoiceServiceLoadedCtx`]) is true the control renders with no segment
/// selected and both segments disabled, so a user can never pick against a
/// default that the config fetch is about to replace.
///
/// Consumes the context; it is provided only at the HermesApp root.
#[component]
fn VoiceServiceControl(
    loaded: bool,
    disabled: bool,
    #[props(default)] identity_note: bool,
    #[props(default)] on_change: Option<EventHandler<String>>,
) -> Element {
    let mut service = use_context::<VoiceServiceCtx>().0;
    // Pattern B: owned local; the borrow ends at the `;`.
    let current = service.read().clone();
    let inert = disabled || !loaded;
    let helper = if !loaded {
        ""
    } else if current == "realtime" {
        "Streaming voice-to-voice. You talk and the agent answers as you speak, and you can interrupt it."
    } else {
        "Your speech is transcribed, the agent replies, then the reply is spoken. The agent takes turns."
    };

    rsx! {
        p {
            class: "voice-settings-section-header",
            role: "heading",
            "aria-level": "3",
            "VOICE SERVICE"
        }
        hr { class: "voice-settings-separator" }
        if identity_note {
            p { class: "voice-settings-hint", role: "note", "{IDENTITY_GLOBAL_NOTE}" }
        }
        div {
            class: "voice-settings-row",
            style: "flex-direction: column; align-items: flex-start;",
            div {
                class: "voice-seg-control voice-service",
                role: "radiogroup",
                "aria-label": "Voice service",
                for (idx , (id , label , dom_id)) in SERVICE_SEGMENTS.iter().copied().enumerate() {
                    {
                        let selected = loaded && current == id;
                        rsx! {
                            button {
                                key: "{id}",
                                id: "{dom_id}",
                                class: if selected { "voice-seg is-active" } else { "voice-seg" },
                                r#type: "button",
                                role: "radio",
                                "aria-checked": if selected { "true" } else { "false" },
                                tabindex: if selected { "0" } else { "-1" },
                                disabled: inert,
                                onclick: move |_| {
                                    service.set(id.to_string());
                                    if let Some(handler) = on_change.as_ref() {
                                        handler.call(id.to_string());
                                    }
                                },
                                onkeydown: move |evt: KeyboardEvent| {
                                    let key = evt.key().to_string();
                                    let next = service_seg_next(idx, &key);
                                    let handled = matches!(
                                        key.as_str(),
                                        "ArrowRight" | "ArrowLeft" | "ArrowUp" | "ArrowDown" | "Home" | "End"
                                    );
                                    if handled {
                                        evt.prevent_default();
                                    }
                                    if next != idx {
                                        let (next_id, _, next_dom_id) = SERVICE_SEGMENTS[next];
                                        service.set(next_id.to_string());
                                        if let Some(handler) = on_change.as_ref() {
                                            handler.call(next_id.to_string());
                                        }
                                        // Static: `next_dom_id` is one of the two compile-time
                                        // DOM ids in SERVICE_SEGMENTS, never user input. This
                                        // only moves keyboard focus (roving tabindex).
                                        let _ = document::eval(&format!(
                                            "document.getElementById('{next_dom_id}')?.focus();"
                                        ));
                                    }
                                },
                                "{label}"
                            }
                        }
                    }
                }
            }
            p { class: "voice-settings-hint", style: "margin-top:var(--space-1);", "{helper}" }
        }
    }
}

// ── Component ────────────────────────────────────────────────────────────────

/// Server-fn-backed voice settings panel.
///
/// On mount, loads live config via `get_voice_config()`. Renders VOICE, STT, and
/// TTS sections per UI-SPEC Surfaces 1–3. Includes explicit Save button with
/// dirty / saved / error states, locked state when `web_config_write_enabled` is
/// false, and masked secret-field rows.
///
/// Props:
/// - `on_close`: called when user closes the panel.
/// - `show_orb_appearance` (default `true`): renders the nested orb Appearance
///   section and lets this form own the shared orb signals. The System screen's
///   General-tab mount passes `false` (G-54.4-34): the System > Orb tab is the
///   home of orb appearance. The voice-mode overlay keeps the default, because
///   it is the only surface with a live orb preview.
#[component]
pub fn VoiceSettings(
    on_close: EventHandler<()>,
    #[props(default = true)] show_orb_appearance: bool,
) -> Element {
    // ── Context lookups (borrows dropped into locals — Pattern B) ─────────────
    // `mut` so the VAD "live" inputs can push edits straight into VoiceStatusState,
    // which the running voice_loop reads each poll (Phase 36.17.10 live-tuning).
    let mut voice_status = use_context::<Signal<VoiceStatusState>>();

    // ── Wake-word config: CONSUMED from the HermesApp root provider ───────────
    // (Phase 36.17.10 UAT crash fix.) These were previously created + provided
    // here, which meant the hands-free overlay launched from the composer "Free"
    // segment had no provider and panicked. They now live at the HermesApp root
    // (mod.rs) so the overlay and this panel share one source of truth; we edit
    // them here via .set() and the live voice_loop reads the same signals.
    let mut wake_enabled = use_context::<WakeWordEnabledCtx>().0;
    let mut wake_phrase = use_context::<WakeWordPhraseCtx>().0;

    // Phase 40.2 Plan 04 (FE-01, D-02, D-05): avatar mode context.
    // Consumed from the HermesApp root provider (mod.rs) — NOT provided here.
    // Use .read() (not .peek()) so the toggle/dropdown re-render on change.
    let mut avatar_ctx = use_context::<AvatarModeCtx>();
    // Pattern B: read into owned clone; borrow dropped at `;` before any .await.
    let avatar_prefs = avatar_ctx.0.read().clone();

    // Phase 40.5 Plan 06 (D-14/D-15/D-09/D-10): per-identity voice override contexts.
    // Consumed from HermesApp root (mod.rs) — NOT provided here (Dioxus context-panic rule).
    // Use .read() in render path (Pattern G) to subscribe for re-render on .set().
    let edit_target_sig = use_context::<VoiceEditTargetCtx>().0;
    let mut identity_tts_provider_sig = use_context::<IdentityTtsProviderCtx>().0;
    let mut identity_tts_voice_sig = use_context::<IdentityTtsVoiceCtx>().0;
    let mut identity_realtime_voice_sig = use_context::<IdentityRealtimeVoiceCtx>().0;
    // Phase 54.7 Plan 15 (D-16): model / format overrides, root-provided like the three above.
    let mut identity_tts_model_sig = use_context::<IdentityTtsModelCtx>().0;
    let mut identity_tts_format_sig = use_context::<IdentityTtsFormatCtx>().0;
    let mut identity_realtime_model_sig = use_context::<IdentityRealtimeModelCtx>().0;

    // Phase 40.5 Plan 07 (D-04): orb appearance ctx — read for dirty detection and save.
    // Consumed from HermesApp root (mod.rs) — NOT provided here (context-panic rule).
    // mut: read in render path, and .set() in the identity-switch hydration effect below
    // (gap-fix) so selecting an orb identity loads ITS saved appearance into the cards + orb.
    let mut orb_style_sig = use_context::<OrbStyleCtx>().0;
    let mut orb_hue_sig = use_context::<OrbBaseHueCtx>().0;
    let mut orb_size_sig = use_context::<OrbSizeCtx>().0;
    let mut orb_glow_sig = use_context::<OrbGlowCtx>().0;

    // Server-fn-backed voice settings signals (Plan 02)
    let mut auto_tts_sig = use_signal(|| false);
    // Phase 54.7 Plan 08 (D-14/D-15): the staged voice service and its Loaded
    // flag are consumed from the HermesApp root provider (mod.rs) — VoiceModeScreen
    // (an ancestor) reads them for the entry gate, so they must NOT originate in
    // this conditionally-mounted panel. The realtime-degraded flag is root-provided
    // the same way (Phase 36.17.12 Plan 04, CR-01 gap closure).
    let mut service_sig = use_context::<VoiceServiceCtx>().0;
    let service_loaded_sig = use_context::<VoiceServiceLoadedCtx>().0;
    let realtime_degraded_sig = use_context::<RealtimeDegradedCtx>().0;
    // Interruption mode: panel-local FORM state only (no consumer reads it from
    // context any more). It is sent on save only when the snapshot offers a
    // choice that includes it (see `global_voice_payload`).
    let mut barge_in_mode_sig = use_signal(|| "push_to_interrupt".to_string());
    // The last loaded/saved snapshot: the payload builder needs its
    // `barge_in_modes`, and the seeds and dirty comparison share one source.
    let mut loaded_snapshot: Signal<VoiceConfigSnapshot> = use_signal(VoiceConfigSnapshot::default);
    let mut loaded_service = use_signal(|| "turn_based".to_string());
    // WR-01: the staged service lives in the ROOT-provided `VoiceServiceCtx`, which
    // the voice-mode entry gate also reads. An abandoned (unsaved) edit must not
    // survive the panel: on unmount the root context is restored to the last
    // loaded/saved service. A plain `Rc<RefCell>` (not a Signal) so the drop hook
    // never reads a panel-local signal whose owner scope is already gone. Empty
    // until the config has loaded, which makes the restore a no-op.
    let loaded_service_mirror = use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(String::new())));
    {
        let mirror = loaded_service_mirror.clone();
        dioxus::core::use_drop(move || {
            let staged = service_sig.peek().clone();
            if let Some(restore) = service_to_restore(&mirror.borrow(), &staged) {
                service_sig.set(restore);
            }
        });
    }
    // Phase 54.7 Plan 08 (D-14): each voice family owns its form state. Both are
    // created unconditionally (hook order) and both families stay mounted, so a
    // value typed in the hidden family survives flipping the service control.
    let turn_based_form = use_turn_based_form();
    let realtime_form = use_realtime_form();
    let mut stt_model_sig = use_signal(String::new);
    let mut silence_duration_sig = use_signal(|| 3.0f64);
    let mut web_silence_threshold_sig = use_signal(|| 5.0f32);
    let mut max_recording_sig = use_signal(|| 120u32);
    // Consumed from the HermesApp root provider (mod.rs) — NOT created/provided here.
    // voice_loop (turn-based) reads BeepEnabledCtx from a non-descendant scope, so the
    // signal must be owned at the root (context-panic rule). Set from snapshot below.
    let mut beep_enabled_sig = use_context::<BeepEnabledCtx>().0;

    // Provide all Plan 02 contexts for Plan 03+ consumers
    use_context_provider(|| AutoTtsCtx(auto_tts_sig));
    // VoiceServiceCtx, VoiceServiceLoadedCtx and RealtimeDegradedCtx are NOT provided
    // here — they are consumed from the HermesApp root provider (mod.rs).
    use_context_provider(|| SttProviderCtx(turn_based_form.stt_provider));
    use_context_provider(|| SttModelCtx(stt_model_sig));
    use_context_provider(|| TtsProviderCtx(turn_based_form.tts_provider));
    use_context_provider(|| TtsVoiceCtx(turn_based_form.tts_voice));
    use_context_provider(|| TtsFormatCtx(turn_based_form.tts_format));
    use_context_provider(|| SilenceDurationCtx(silence_duration_sig));
    use_context_provider(|| WebSilenceThresholdCtx(web_silence_threshold_sig));
    use_context_provider(|| MaxRecordingCtx(max_recording_sig));
    // BeepEnabledCtx is NO LONGER provided here — it is provided at the HermesApp
    // root (mod.rs) so voice_loop (a non-descendant) can consume it without panic.

    // ── Config-write-enabled gate (locked state) ─────────────────────────────
    let mut write_enabled = use_signal(|| false);

    // ── Secret fields list (populated from snapshot on mount) ────────────────
    let mut secret_fields: Signal<Vec<String>> = use_signal(Vec::new);

    // ── Loaded-values snapshot for dirty-state comparison ───────────────────
    // These hold the "last saved" values so we can detect dirty state.
    // Phase 36.17.12 Plan 01: loaded snapshot of barge_in_mode for dirty detection.
    let mut loaded_barge_in_mode = use_signal(|| "push_to_interrupt".to_string());
    let mut loaded_auto_tts = use_signal(|| false);
    // Plan 03: VAD loaded-value snapshots for dirty detection
    let mut loaded_silence_duration = use_signal(|| 3.0f64);
    let mut loaded_web_silence_threshold = use_signal(|| 5.0f32);
    let mut loaded_wake_word_enabled = use_signal(|| false);
    let mut loaded_wake_word_phrase = use_signal(|| "hey hermes".to_string());

    // Phase 40.5 Plan 06: identity list and overrides snapshot (cached from get_voice_config).
    let mut identity_list_sig: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let mut identity_overrides_sig: Signal<Vec<IdentityOverride>> = use_signal(Vec::new);

    // Phase 40.5 Plan 07: loaded orb appearance snapshots for dirty-state comparison.
    // Seeded from current OrbStyleCtx/etc. in the load latch (which are already seeded
    // from ORB_PRESET_REGISTRY in mod.rs); updated after a successful identity save.
    let mut loaded_orb_style = use_signal(|| "classic".to_string());
    let mut loaded_orb_hue: Signal<u16> = use_signal(|| 186u16);
    let mut loaded_orb_size: Signal<f32> = use_signal(|| 1.0f32);
    let mut loaded_orb_glow: Signal<f32> = use_signal(|| 0.5f32);

    // ── Load-latch: populate signals once per mount (prevents Pitfall 3 loop) ─
    let mut loaded = use_signal(|| false);

    // ── Fetch config on mount ─────────────────────────────────────────────────
    let config_resource = use_resource(move || async move { get_voice_config().await });

    // Populate signals when resource resolves (once, via loaded latch)
    if !*loaded.read() {
        if let Some(Ok(snapshot)) = config_resource.read().as_ref() {
            // Populate editable signals from snapshot. One definition of the seed
            // (`GlobalVoiceValues::from_snapshot`) shared with the save tests.
            let seed = GlobalVoiceValues::from_snapshot(snapshot);
            auto_tts_sig.set(seed.auto_tts);
            stt_model_sig.set(snapshot.stt_model.clone());
            // The two families seed from the same snapshot (one-shot latch).
            turn_based_form.seed(snapshot);
            realtime_form.seed(snapshot);
            // Phase 36.17.12 Plan 01: populate barge-in mode from snapshot.
            barge_in_mode_sig.set(seed.barge_in_mode.clone());
            // Phase 54.7 Plan 08 (D-15): the staged service, seeded exactly like the
            // root seed; the root-provided context is edited here, never provided.
            service_sig.set(seed.voice_service.clone());
            loaded_service.set(seed.voice_service.clone());
            loaded_snapshot.set(snapshot.clone());
            silence_duration_sig.set(seed.silence_duration);
            web_silence_threshold_sig.set(seed.web_silence_threshold_rms);
            max_recording_sig.set(seed.max_recording_seconds);
            beep_enabled_sig.set(seed.beep_enabled);
            wake_enabled.set(seed.wake_word_enabled);
            wake_phrase.set(seed.wake_word_phrase.clone());
            loaded_wake_word_enabled.set(seed.wake_word_enabled);
            loaded_wake_word_phrase.set(seed.wake_word_phrase.clone());

            // Populate loaded-value snapshots for dirty detection
            // Phase 36.17.12 Plan 01: loaded snapshot for barge-in mode dirty detection.
            loaded_barge_in_mode.set(seed.barge_in_mode.clone());
            loaded_auto_tts.set(seed.auto_tts);
            // Plan 03: VAD loaded-value snapshots
            loaded_silence_duration.set(seed.silence_duration);
            loaded_web_silence_threshold.set(seed.web_silence_threshold_rms);

            write_enabled.set(snapshot.web_config_write_enabled);
            secret_fields.set(snapshot.secret_fields_present.clone());

            // Phase 40.5 Plan 06: cache identity list + overrides for AppliesTo/IdentityVoiceSection.
            identity_list_sig.set(snapshot.identity_list.clone());
            identity_overrides_sig.set(snapshot.identity_overrides.clone());

            // Phase 40.5 Plan 07: seed loaded orb appearance from current ctx values.
            // OrbStyleCtx/etc. are already seeded from ORB_PRESET_REGISTRY in mod.rs at mount,
            // so this establishes the "clean" baseline: loaded = ORB defaults → not dirty.
            loaded_orb_style.set(orb_style_sig.read().clone());
            loaded_orb_hue.set(*orb_hue_sig.read());
            loaded_orb_size.set(*orb_size_sig.read());
            loaded_orb_glow.set(*orb_glow_sig.read());

            loaded.set(true);
        }
    }

    // ── Phase 41.2 Plan 01: reset-fight guard state ────────────────────────
    // Tracks the last `edit_target` value the hydrate effect below actually
    // re-derived FROM. Read with `.peek()` inside the effect (not `.read()`)
    // so tracking this value never adds a re-subscription of its own.
    let mut last_hydrated_target = use_signal(String::new);

    // ── Phase 40.5 gap-fix: hydrate orb appearance on identity switch ─────────
    // When the AppliesTo target switches to an orb identity, load THAT identity's
    // saved appearance (snapshot override, else ORB_PRESET_REGISTRY default for the
    // slug) into OrbStyleCtx/hue/size/glow. This drives both the preset-card
    // selection AND the live orb (orb_canvas eval bridges). Without it, selecting
    // Bloom/ASCII/Network left the editor on the mount-seeded "classic" and the orb
    // never changed (UAT 40.5). Re-baselines loaded_orb_* so a fresh selection is clean.
    //
    // Phase 41.2 Plan 01 (reset-fight guard): `edit_target_sig.set()` notifies
    // on every call, even a no-op re-set to the SAME value — without a guard,
    // that would re-run this whole body and snap a just-picked Style back to
    // the preset default. The `voice_form_should_hydrate_appearance` gate (which
    // wraps the 41.2 guard) limits the re-derivation to REAL switches only; a
    // no-op re-set now early-returns before touching any orb_*_sig.
    use_effect(move || {
        let target = edit_target_sig.read().clone(); // subscribe to target switches
                                                     // G-41.2-11 (persistence reset-on-open): do NOT derive appearance until the
                                                     // config has loaded. `identity_overrides_sig` is read via `.peek()` below (no
                                                     // subscription), so if this effect ran on the FIRST render — before
                                                     // `get_voice_config` resolves and populates the overrides — it would find no
                                                     // override, fall back to ORB_PRESET_REGISTRY defaults, CLOBBER the app-level
                                                     // -loaded saved appearance (e.g. bloom/77/0.8 → preset defaults), cache
                                                     // `last_hydrated_target`, and never re-derive once the overrides arrive: the
                                                     // "opening Settings resets the orb to defaults" bug. Subscribing to `loaded`
                                                     // (set true in the load latch when the config resolves) re-runs this effect at
                                                     // that point, so the SAVED override is applied instead of a preset default.
                                                     // The gate keeps that not-loaded guard. A form mounted with
                                                     // `show_orb_appearance: false` (System > General) returns here every
                                                     // time, so it never writes the shared orb signals (G-54.4-34).
        let is_loaded = *loaded.read();
        if !voice_form_should_hydrate_appearance(
            show_orb_appearance,
            is_loaded,
            &last_hydrated_target.peek(),
            &target,
        ) {
            return;
        }
        // G-54.4-25: the saved-over-preset derivation is shared with the System
        // screen's Orb tab (one definition of "saved appearance").
        // The peek guard is a statement temporary: dropped before any .set().
        let saved_override = identity_overrides_sig
            .peek()
            .iter()
            .find(|o| o.slug == target)
            .cloned();
        let resolved = resolve_orb_appearance(&target, saved_override.as_ref());
        if let Some(OrbAppearance {
            style,
            base_hue: hue,
            size,
            glow,
        }) = resolved
        {
            orb_style_sig.set(style.clone());
            orb_hue_sig.set(hue);
            orb_size_sig.set(size);
            orb_glow_sig.set(glow);
            loaded_orb_style.set(style);
            loaded_orb_hue.set(hue);
            loaded_orb_size.set(size);
            loaded_orb_glow.set(glow);
        }
        last_hydrated_target.set(target);
    });

    // ── Save lifecycle signals ────────────────────────────────────────────────
    // "saved"/"error" are transient UI states; dirty is derived from signal diff.
    let mut save_label_saved = use_signal(|| false); // true → show "Settings saved."
    let mut save_error_msg: Signal<Option<String>> = use_signal(|| None);
    // True from the moment a save request is sent until it settles; blocks a second submit.
    let mut saving = use_signal(|| false);

    // ── Read all signal values into Copy/Clone locals (Pattern B) ────────────
    let auto_tts_on = *auto_tts_sig.read();
    // Phase 36.17.12 Plan 01: barge-in mode + realtime-degraded owned locals (Pattern B).
    let barge_in_mode_val = barge_in_mode_sig.read().clone();
    let service_val = service_sig.read().clone();
    let service_loaded_val = *service_loaded_sig.read();
    let loaded_service_val = loaded_service.read().clone();
    // WR-01: mirror the last LOADED/SAVED service into a plain cell so the
    // unmount hook below can read it without touching a (possibly already
    // dropped) panel-local signal.
    *loaded_service_mirror.borrow_mut() = if *loaded.read() {
        loaded_service_val.clone()
    } else {
        String::new()
    };
    let realtime_degraded_val = *realtime_degraded_sig.read();
    let tts_provider_val = turn_based_form.tts_provider.read().clone();
    let write_enabled_val = *write_enabled.read();
    let secret_fields_val = secret_fields.read().clone();
    let save_saved_val = *save_label_saved.read();
    let saving_val = *saving.read();
    let save_error_val = save_error_msg.read().clone();
    let wake_on = *wake_enabled.read();
    let wake_phrase_val = wake_phrase.read().clone();

    // Phase 40.5 Plan 06: identity-related locals (Pattern B — borrow dropped at `;`).
    let edit_target_val = edit_target_sig.read().clone();
    let identity_tts_provider_val = identity_tts_provider_sig.read().clone();
    let identity_tts_voice_val = identity_tts_voice_sig.read().clone();
    let identity_realtime_voice_val = identity_realtime_voice_sig.read().clone();
    // Phase 54.7 Plan 15 (D-16): every staged identity value as one plain struct.
    let identity_fields_val = IdentityVoiceFields {
        tts_provider: identity_tts_provider_val.clone(),
        tts_model: identity_tts_model_sig.read().clone(),
        tts_voice: identity_tts_voice_val.clone(),
        tts_format: identity_tts_format_sig.read().clone(),
        realtime_voice: identity_realtime_voice_val.clone(),
        realtime_model: identity_realtime_model_sig.read().clone(),
    };
    let identity_list_val = identity_list_sig.read().clone();
    let identity_overrides_val = identity_overrides_sig.read().clone();

    // Phase 40.5 Plan 07: orb appearance locals (Pattern B — borrow dropped at `;`).
    let orb_style_val = orb_style_sig.read().clone();
    let orb_hue_val = *orb_hue_sig.read();
    let orb_size_val = *orb_size_sig.read();
    let orb_glow_val = *orb_glow_sig.read();

    // True only when this form shows the Appearance section AND the Applies-to
    // target is an orb; it gates the section, the orb dirty term and the
    // identity save's appearance fields (G-54.4-34).
    let owns_orb_appearance = voice_form_owns_orb_appearance(show_orb_appearance, &edit_target_val);

    // Loaded snapshot locals for dirty-state computation
    let loaded_snapshot_val = loaded_snapshot.read().clone();
    // Phase 36.17.12 Plan 01: loaded barge-in mode local for dirty computation.
    let loaded_barge_in_mode_val = loaded_barge_in_mode.read().clone();
    let loaded_auto_tts_val = *loaded_auto_tts.read();
    // Plan 03: VAD loaded-value locals
    let loaded_silence_duration_val = *loaded_silence_duration.read();
    let loaded_web_silence_threshold_val = *loaded_web_silence_threshold.read();

    // ── Dirty state: any field differs from loaded snapshot ──────────────────
    // Phase 40.5 Plan 07: loaded orb appearance locals for dirty comparison.
    let loaded_orb_style_val = loaded_orb_style.read().clone();
    let loaded_orb_hue_val = *loaded_orb_hue.read();
    let loaded_orb_size_val = *loaded_orb_size.read();
    let loaded_orb_glow_val = *loaded_orb_glow.read();

    // Orb appearance dirty when the edit target is an orb identity and any ctx
    // value differs from the loaded baseline (ORB_PRESET_REGISTRY default or last save).
    let orb_appearance_dirty = voice_form_orb_dirty(
        owns_orb_appearance,
        &OrbAppearance {
            style: orb_style_val,
            base_hue: orb_hue_val,
            size: orb_size_val,
            glow: orb_glow_val,
        },
        &OrbAppearance {
            style: loaded_orb_style_val,
            base_hue: loaded_orb_hue_val,
            size: loaded_orb_size_val,
            glow: loaded_orb_glow_val,
        },
    );

    let is_dirty = auto_tts_on != loaded_auto_tts_val
        || barge_in_mode_val != loaded_barge_in_mode_val
        || (*loaded.read() && service_val != loaded_service_val)
        // Both families are in the dirty comparison regardless of which is visible
        // (D-14: hidden but kept in config).
        || (*loaded.read()
            && (turn_based_form.is_dirty(&loaded_snapshot_val)
                || realtime_form.is_dirty(&loaded_snapshot_val)))
        // Plan 03: VAD fields also mark dirty
        || (*silence_duration_sig.read() - loaded_silence_duration_val).abs() > f64::EPSILON
        || (*web_silence_threshold_sig.read() - loaded_web_silence_threshold_val).abs() > f32::EPSILON
        || wake_on != *loaded_wake_word_enabled.read()
        || wake_phrase_val != *loaded_wake_word_phrase.read()
        // Phase 40.5 Plan 06: identity override dirty — any staging Ctx is explicitly set.
        || (edit_target_val != "global" && identity_fields_val.is_staged())
        // Phase 40.5 Plan 07: orb appearance dirty.
        || orb_appearance_dirty;

    // Fields that currently block saving (each would fail the server's rule too).
    // An identity save sends identity fields only, so only they can block it.
    let blocking_errors = if edit_target_val == "global" {
        turn_based_form.blocking_error_count() + realtime_form.blocking_error_count()
    } else {
        identity_fields_val.blocking_error_count()
    };
    // Which family is visible; a stored odd value clamps to turn_based.
    let active_service = normalize_service(&service_val);
    // D-16: with an identity as the edit target (or the write gate closed) the
    // service control and both family groups are disabled, not silently ignored.
    let families_locked = global_fields_locked(write_enabled_val, &edit_target_val);

    rsx! {
        div {
            class: "voice-settings-panel",
            role: "dialog",
            "aria-label": "Voice settings",

            // ── AppliesTo target selector (Phase 40.5 Plan 06, D-14) ──────
            // Always visible at the top of the panel. Selecting "Global (defaults)"
            // preserves today's behaviour; selecting an identity retargets the
            // editable fields below without auto-saving (D-14 staging contract).
            AppliesTo {
                identity_list: identity_list_val.clone(),
            }

            // ── Set-as-active identity control (Phase 41.2 Plan 03, D-01/D-02) ──
            // Adjacent to AppliesTo above so the "activates the identity you're
            // editing" relationship reads without a caption. Distinct from the
            // AppliesTo dropdown (D-14 preserved: edit-target-only, never activation).
            SetActiveIdentityControl {
                identity_list: identity_list_val.clone(),
            }

            // ── VOICE SERVICE section (Phase 54.7 Plan 08, D-14/D-15) ─────
            // Replaces the old VOICE MODE Turn-based / Open mic control. The
            // selection is staged in the root-provided VoiceServiceCtx and saved
            // as voice.service by SAVE SETTINGS; voice-mode entry reads it.
            hr { class: "voice-settings-separator" }
            VoiceServiceControl {
                loaded: service_loaded_val,
                disabled: families_locked,
                identity_note: edit_target_val != "global",
                on_change: move |_| save_error_msg.set(None),
            }
            // Conditional degrade hint (D-07): the realtime service was selected
            // but the session failed to start. NOT an error color — expected fallback.
            if realtime_degraded_val {
                p {
                    class: "voice-settings-hint",
                    role: "status",
                    "aria-live": "polite",
                    style: "color:var(--fg-dim);",
                    "Realtime unavailable — falling back to turn-based voice."
                }
            }

            hr { class: "voice-settings-separator" }

            // ── Voice families (Phase 54.7 Plan 08, D-14) ──────────────────
            // BOTH groups are ALWAYS mounted (never an `if service ==` branch): the
            // non-selected one only gains the `hidden` attribute, so its signals,
            // pending edits and any fetched lists survive a flip of the control.
            RealtimeFamily {
                form: realtime_form,
                hidden: active_service != "realtime",
                disabled: families_locked,
                on_edit: move |_| save_error_msg.set(None),
            }
            TurnBasedFamily {
                form: turn_based_form,
                auto_tts: auto_tts_sig,
                hidden: active_service != "turn_based",
                disabled: families_locked,
                on_edit: move |_| save_error_msg.set(None),
                tts_key_status: loaded_snapshot_val.tts_compat_key_status.clone(),
            }

            // Secret fields (names from snapshot — masked rows, no inputs)
            for field_name in &secret_fields_val {
                div { class: "voice-settings-secret-row",
                    div { class: "voice-settings-row",
                        span { class: "voice-settings-label", "{field_name}" }
                        span { class: "voice-settings-value", style: "color:var(--fg-disabled);",
                            "••••••••"
                        }
                    }
                    p { class: "voice-settings-hint",
                        "API key managed via config.yaml — not editable here."
                    }
                }
            }

            hr { class: "voice-settings-separator" }

            // ── INTERRUPTION (Phase 54.7 Plan 17, UI-SPEC "Barge-in slot") ──
            // Rendered only when the loaded snapshot lists two or more valid
            // modes; with D-15's single push_to_interrupt it never renders.
            InterruptionRow {
                modes: loaded_snapshot_val.barge_in_modes.clone(),
                current: barge_in_mode_val.clone(),
                disabled: global_fields_locked(write_enabled_val, &edit_target_val),
                on_change: move |mode: String| {
                    save_error_msg.set(None);
                    barge_in_mode_sig.set(mode);
                },
            }

            // ── VAD TIMING section (Plan 03, VOICE-02) ───────────────────
            // Read VAD signal locals (Pattern B — already in scope above as
            // silence_duration_sig and web_silence_threshold_sig).
            hr { class: "voice-settings-separator" }
            p {
                class: "voice-settings-section-header",
                role: "heading",
                "aria-level": "3",
                "VAD TIMING"
            }
            hr { class: "voice-settings-separator" }

            // End-of-speech delay (config.voice.silence_duration, f64, seconds)
            {
                let silence_dur_val = *silence_duration_sig.read();
                let silence_dur_out_of_range = !(0.5..=10.0).contains(&silence_dur_val);
                rsx! {
                    div { class: "voice-settings-row",
                        label {
                            r#for: "vad-silence-duration",
                            class: "voice-settings-label",
                            style: "flex:1;",
                            "End-of-speech delay"
                            // "live" pill — takes effect on next voice-loop connect, no restart
                            span { class: "voice-pill-live", style: "margin-left:var(--space-1);", "live" }
                        }
                        div { style: "display:flex; align-items:center; gap:var(--space-1);",
                            input {
                                id: "vad-silence-duration",
                                r#type: "number",
                                class: "voice-settings-number-input",
                                style: if silence_dur_out_of_range {
                                    "border-color:var(--warn);"
                                } else {
                                    ""
                                },
                                disabled: !write_enabled_val,
                                min: "0.5",
                                max: "10.0",
                                step: "0.5",
                                value: "{silence_dur_val}",
                                "aria-label": "End-of-speech delay in seconds",
                                "aria-describedby": "vad-silence-duration-unit",
                                oninput: move |evt| {
                                    save_error_msg.set(None);
                                    if let Ok(v) = evt.value().parse::<f64>() {
                                        silence_duration_sig.set(v);
                                        // Live-tune the running voice_loop (Phase 36.17.10).
                                        voice_status.with_mut(|s| s.silence_duration_secs = Some(v));
                                    }
                                },
                            }
                            span {
                                id: "vad-silence-duration-unit",
                                class: "voice-settings-label",
                                "s"
                            }
                        }
                    }
                    if silence_dur_out_of_range {
                        p { class: "voice-settings-hint", style: "color:var(--warn);",
                            "Value must be between 0.5 and 10.0."
                        }
                    }
                }
            }

            // Silence threshold (config.voice.web_silence_threshold_rms, f32, Web Audio byte-domain)
            // DISTINCT from silence_threshold: i32 (native PCM) — never aliased (Plan 01 decision).
            {
                let rms_val = *web_silence_threshold_sig.read();
                let rms_out_of_range = !(0.5..=50.0).contains(&rms_val);
                rsx! {
                    div { class: "voice-settings-row",
                        label {
                            r#for: "vad-rms-threshold",
                            class: "voice-settings-label",
                            style: "flex:1;",
                            "Silence threshold"
                            span { class: "voice-pill-live", style: "margin-left:var(--space-1);", "live" }
                        }
                        div { style: "display:flex; align-items:center; gap:var(--space-1);",
                            input {
                                id: "vad-rms-threshold",
                                r#type: "number",
                                class: "voice-settings-number-input",
                                style: if rms_out_of_range {
                                    "border-color:var(--warn);"
                                } else {
                                    ""
                                },
                                disabled: !write_enabled_val,
                                min: "0.5",
                                max: "50.0",
                                step: "0.5",
                                value: "{rms_val}",
                                "aria-label": "Silence threshold in Web Audio RMS units",
                                "aria-describedby": "vad-rms-threshold-unit",
                                oninput: move |evt| {
                                    save_error_msg.set(None);
                                    if let Ok(v) = evt.value().parse::<f32>() {
                                        web_silence_threshold_sig.set(v);
                                        // Live-tune the running voice_loop (Phase 36.17.10):
                                        // push into VoiceStatusState, which the loop reads
                                        // each poll — this is what the "live" pill promises.
                                        voice_status.with_mut(|s| s.web_silence_threshold_rms = Some(v));
                                    }
                                },
                            }
                            span {
                                id: "vad-rms-threshold-unit",
                                class: "voice-settings-label",
                                "rms"
                            }
                        }
                    }
                    if rms_out_of_range {
                        p { class: "voice-settings-hint", style: "color:var(--warn);",
                            "Value must be between 0.5 and 50.0."
                        }
                    }
                }
            }

            hr { class: "voice-settings-separator" }

            // ── WAKE WORD section (preserved from 36.17.9 Plan 02) ────────
            // Bug A (open-mic design decision — Option 2): wake-word is not supported
            // with the realtime service. It routes to the WebRTC realtime session
            // (start_realtime_session), which streams audio continuously and has no
            // concept of wake_word_check frames or WakeWordResult events. The controls
            // are disabled with an explanatory hint when the Realtime service is staged.
            {
                let realtime_active = wake_word_blocked(&service_val);
                rsx! {
                    p {
                        class: "voice-settings-section-header",
                        role: "heading",
                        "aria-level": "3",
                        "WAKE WORD"
                    }
                    // Realtime incompatibility hint (shown above the controls when the Realtime service is staged).
                    if realtime_active {
                        p { class: "voice-settings-hint",
                            style: "color:var(--fg-dim);",
                            role: "note",
                            "Wake word is not supported with the Realtime service. Switch to Turn-based to enable."
                        }
                    }
                    div { class: "voice-settings-row",
                        span { class: "voice-settings-label", "Enable wake word" }
                        button {
                            class: if wake_on && !realtime_active { "voice-toggle is-on" } else { "voice-toggle" },
                            r#type: "button",
                            role: "switch",
                            "aria-checked": if wake_on && !realtime_active { "true" } else { "false" },
                            "aria-label": "Enable wake word",
                            disabled: wake_controls_disabled(&service_val, write_enabled_val),
                            onclick: move |_| {
                                if !realtime_active {
                                    wake_enabled.set(!wake_on);
                                }
                            },
                            span { class: "voice-toggle-thumb" }
                        }
                    }
                    input {
                        r#type: "text",
                        class: "voice-wake-phrase-input",
                        value: "{wake_phrase_val}",
                        maxlength: "64",
                        placeholder: "hey hermes",
                        disabled: !wake_on || realtime_active,
                        "aria-label": "Wake phrase",
                        oninput: move |evt| {
                            if !realtime_active {
                                wake_phrase.set(evt.value());
                            }
                        },
                    }
                    if wake_on && !realtime_active {
                        p { class: "voice-settings-hint",
                            "Voice turns start only after you say the wake phrase."
                        }
                    }
                }
            }

            hr { class: "voice-settings-separator" }

            // ── AVATAR section (Phase 40.2, FE-01, D-02, D-05) ───────────
            // Avatar on/off toggle is always visible.
            // Head dropdown is gated on `avatar_prefs.enabled` and locked
            // while a call is live (!write_enabled_val proxy — D-05).
            p {
                class: "voice-settings-section-header",
                role: "heading",
                "aria-level": "3",
                "AVATAR"
            }
            hr { class: "voice-settings-separator" }

            // Avatar enable/disable toggle (always visible — D-02)
            div { class: "voice-settings-row",
                label {
                    r#for: "avatar-toggle",
                    class: "voice-settings-label",
                    "Show avatar"
                }
                button {
                    id: "avatar-toggle",
                    class: if avatar_prefs.enabled { "voice-toggle is-on" } else { "voice-toggle" },
                    r#type: "button",
                    role: "switch",
                    "aria-checked": if avatar_prefs.enabled { "true" } else { "false" },
                    "aria-label": "Show avatar",
                    onclick: move |_| {
                        let checked = !avatar_ctx.0.read().enabled;
                        avatar_ctx.0.write().enabled = checked;
                    },
                    span { class: "voice-toggle-thumb" }
                }
            }

            // Visual selector — Orb / Morph Head / Groovy Girl (always visible).
            // "Orb" turns the avatar off; selecting a head turns it on with that
            // identity. The "Show avatar" toggle above stays in sync (D-04 live
            // orb↔avatar swap). The selector is locked while a call is live
            // (D-05 — head identity is session-locked; the toggle remains the
            // live orb fallback mid-call). onchange validates the chosen head
            // against PRESET_REGISTRY before writing (T-40.2-04-02 — user input
            // must never bypass the registry).
            div { class: "voice-settings-row",
                label {
                    r#for: "avatar-head-select",
                    class: "voice-settings-label",
                    "Visual"
                }
                select {
                    id: "avatar-head-select",
                    class: "voice-settings-select",
                    disabled: !write_enabled_val,
                    "aria-label": "Avatar visual",
                    value: if avatar_prefs.enabled { avatar_prefs.head_id.clone() } else { "orb".to_string() },
                    onchange: move |evt| {
                        let val = evt.value();
                        if val == "orb" {
                            avatar_ctx.0.write().enabled = false;
                        } else if crate::components::hermes_app::avatar_logic::PRESET_REGISTRY
                            .iter()
                            .any(|p| p.id == val)
                        {
                            let mut w = avatar_ctx.0.write();
                            w.enabled = true;
                            w.head_id = val;
                        }
                    },
                    // Orb is the default + graceful fallback (REND-02) — first option.
                    option {
                        value: "orb",
                        selected: !avatar_prefs.enabled,
                        "Orb"
                    }
                    for preset in crate::components::hermes_app::avatar_logic::PRESET_REGISTRY {
                        option {
                            value: "{preset.id}",
                            selected: avatar_prefs.enabled && preset.id == avatar_prefs.head_id,
                            "{preset.display_name}"
                        }
                    }
                }
            }

            hr { class: "voice-settings-separator" }

            // ── Orb appearance editor (Phase 40.5 Plan 07, D-04) ──────────────
            // Rendered by the voice-mode overlay (the prop default) when the
            // AppliesTo target is an orb-type identity (ORB_PRESET_REGISTRY
            // membership); hidden for head avatar identities. The System
            // screen's General tab opts out (G-54.4-34): the Orb tab is the
            // home of orb appearance there.
            if owns_orb_appearance {
                OrbAppearanceSection { write_enabled: write_enabled_val }
                hr { class: "voice-settings-separator" }
            }

            // ── Identity voice override sections (Phase 40.5 Plan 06, D-09/D-10) ──
            // Rendered only when a non-global identity is selected in AppliesTo (D-14).
            {
                let current_override = identity_overrides_val.iter()
                    .find(|o| o.slug == edit_target_val)
                    .cloned();
                let (res_provider, res_voice, res_model, res_format, res_rt, res_rt_model) =
                    if let Some(ref ov) = current_override {
                        (
                            ov.free_mode_provider.clone(),
                            ov.free_mode_voice.clone(),
                            ov.free_mode_model.clone(),
                            ov.free_mode_format.clone(),
                            ov.realtime_voice.clone(),
                            ov.realtime_model.clone(),
                        )
                    } else {
                        (
                            tts_provider_val.clone(),
                            None,
                            None,
                            None,
                            REALTIME_PROVIDERS[0].default_voice.to_string(),
                            REALTIME_PROVIDERS[0].default_model.to_string(),
                        )
                    };
                rsx! {
                    if edit_target_val != "global" {
                        IdentityVoiceSection {
                            // Keyed by the edit target: switching identity remounts the
                            // section so its typing buffers re-seed from the new identity.
                            key: "{edit_target_val}",
                            resolved_provider: res_provider,
                            resolved_voice: res_voice,
                            resolved_model: res_model,
                            resolved_format: res_format,
                            resolved_realtime_voice: res_rt,
                            resolved_realtime_model: res_rt_model,
                            write_enabled: write_enabled_val,
                            usable_providers: loaded_snapshot_val.tts_providers_usable.clone(),
                            compat_base_url: loaded_snapshot_val.tts_compat_base_url.clone(),
                            compat_path: loaded_snapshot_val.tts_compat_path.clone(),
                            compat_key_ref: loaded_snapshot_val.tts_compat_key_ref.clone(),
                        }
                        hr { class: "voice-settings-separator" }
                    }
                }
            }

            // ── Save bar / lock notice ────────────────────────────────────
            if write_enabled_val {
                // Save bar (sticky at panel bottom): button, blocking summary,
                // mapped save error (Phase 54.7 Plan 17).
                VoiceSaveBar {
                    dirty: is_dirty,
                    blocking: blocking_errors,
                    saving: saving_val,
                    saved: save_saved_val,
                    error: save_error_val.clone(),
                    legacy_keys: migration_notice_visible(&loaded_snapshot_val, &edit_target_val),
                    on_save: move |_| {
                            // Double-submit guard: a request already in flight owns the save.
                            if *saving.peek() {
                                return;
                            }
                            // Pattern B: read ALL signal values into owned locals BEFORE spawn
                            let auto_tts_local         = *auto_tts_sig.read();
                            let barge_in_mode_local    = barge_in_mode_sig.read().clone();
                            let service_local          = service_sig.read().clone();
                            let snapshot_local         = loaded_snapshot.read().clone();
                            let silence_dur_local      = *silence_duration_sig.read();
                            let rms_threshold_local    = *web_silence_threshold_sig.read();
                            let max_rec_local          = *max_recording_sig.read();
                            let beep_local             = *beep_enabled_sig.read();
                            let wake_word_enabled_local = *wake_enabled.read();
                            let wake_word_phrase_local  = wake_phrase.read().clone();

                            // Phase 40.5 Plan 06 (D-09/D-10): identity signal locals.
                            // Read BEFORE spawn — Pattern B (no signal borrow across .await).
                            let edit_target_local        = edit_target_sig.read().clone();
                            let identity_fields_local = IdentityVoiceFields {
                                tts_provider: identity_tts_provider_sig.read().clone(),
                                tts_model: identity_tts_model_sig.read().clone(),
                                tts_voice: identity_tts_voice_sig.read().clone(),
                                tts_format: identity_tts_format_sig.read().clone(),
                                realtime_voice: identity_realtime_voice_sig.read().clone(),
                                realtime_model: identity_realtime_model_sig.read().clone(),
                            };

                            // Phase 40.5 Plan 07 (D-17) / G-54.4-34: the live orb appearance, read
                            // BEFORE spawn as ONE owned local. `Some` only when this form owns the
                            // orb appearance (shows the Appearance section for an orb target), so a
                            // form that does not (System > General) never reads the shared orb
                            // signals into a save. Each borrow drops at the end of its own expression.
                            let appearance_local: Option<OrbAppearance> =
                                owns_orb_appearance.then(|| OrbAppearance {
                                    style: orb_style_sig.read().clone(),
                                    base_hue: *orb_hue_sig.read(),
                                    size: *orb_size_sig.read(),
                                    glow: *orb_glow_sig.read(),
                                });

                            if edit_target_local != "global" {
                                // ── Identity-scoped save path ─────────────────────
                                // Only identity fields are set; global fields are None
                                // so merge_voice_payload touches only the identity entry.
                                // D-17: appearance fields are present only when this form
                                // owns the orb appearance (G-54.4-34). Server
                                // validate_voice_payload range-checks them before write.
                                let saved_slug = edit_target_local.clone();
                                let saved_fields = identity_fields_local.clone();
                                let payload = identity_voice_payload(
                                    edit_target_local,
                                    identity_fields_local,
                                    appearance_local.as_ref(),
                                );
                                saving.set(true);
                                spawn(async move {
                                    let result = update_voice_config(payload).await;
                                    saving.set(false);
                                    match result {
                                        Ok(()) => {
                                            // The section shows what was just saved: fold the
                                            // staged values into the cached snapshot override.
                                            identity_overrides_sig.with_mut(|all| {
                                                if let Some(ov) =
                                                    all.iter_mut().find(|o| o.slug == saved_slug)
                                                {
                                                    saved_fields.apply_to_override(ov);
                                                }
                                            });
                                            // Approach A (seeding strategy): reset all identity
                                            // Ctx to None after a successful identity save so
                                            // the next target switch seeds clean.
                                            identity_tts_provider_sig.set(None);
                                            identity_tts_voice_sig.set(None);
                                            identity_realtime_voice_sig.set(None);
                                            identity_tts_model_sig.set(None);
                                            identity_tts_format_sig.set(None);
                                            identity_realtime_model_sig.set(None);
                                            // Phase 40.5 Plan 07: update loaded orb appearance
                                            // snapshot so dirty goes false after a successful save.
                                            if let Some(a) = appearance_local {
                                                loaded_orb_style.set(a.style);
                                                loaded_orb_hue.set(a.base_hue);
                                                loaded_orb_size.set(a.size);
                                                loaded_orb_glow.set(a.glow);
                                            }
                                            save_error_msg.set(None);
                                            save_label_saved.set(true);
                                            gloo_timers::future::TimeoutFuture::new(1500).await;
                                            save_label_saved.set(false);
                                        }
                                        Err(e) => {
                                            // Fixed UI-SPEC copy only; edits stay in their signals.
                                            save_error_msg.set(Some(save_error_copy(&e.to_string())));
                                        }
                                    }
                                });
                            } else {
                                // ── Global save path ──────────────────────────────
                                // Phase 54.7 Plan 08 (D-15): the payload comes from the
                                // plain-value builder, which adds `voice_service` and
                                // never sends a barge_in_mode the server would reject.
                                let saved_service = normalize_service(&service_local).to_string();
                                let values = GlobalVoiceValues {
                                    auto_tts: auto_tts_local,
                                    barge_in_mode: barge_in_mode_local.clone(),
                                    voice_service: service_local,
                                    silence_duration: silence_dur_local,
                                    web_silence_threshold_rms: rms_threshold_local,
                                    max_recording_seconds: max_rec_local,
                                    beep_enabled: beep_local,
                                    wake_word_enabled: wake_word_enabled_local,
                                    wake_word_phrase: wake_word_phrase_local.clone(),
                                };
                                // Both families' values ride every save, visible or not.
                                let payload = assemble_voice_payload(
                                    &values,
                                    &snapshot_local,
                                    &turn_based_form,
                                    &realtime_form,
                                );
                                saving.set(true);
                                spawn(async move {
                                    let result = update_voice_config(payload).await;
                                    saving.set(false);
                                    match result {
                                        Ok(()) => {
                                            // Update loaded-value snapshots (dirty → clean)
                                            loaded_service.set(saved_service);
                                            loaded_snapshot.with_mut(|snap| {
                                                turn_based_form.commit_to(snap);
                                                realtime_form.commit_to(snap);
                                                // The migrating save rewrote the legacy keys away.
                                                after_save_ok(snap);
                                            });
                                            loaded_auto_tts.set(auto_tts_local);
                                            loaded_barge_in_mode.set(barge_in_mode_local);
                                            // Plan 03: update VAD loaded-value snapshots
                                            loaded_silence_duration.set(silence_dur_local);
                                            loaded_web_silence_threshold.set(rms_threshold_local);
                                            loaded_wake_word_enabled.set(wake_word_enabled_local);
                                            loaded_wake_word_phrase.set(wake_word_phrase_local);

                                            // Show "Settings saved." for 1.5 s then revert
                                            save_error_msg.set(None);
                                            save_label_saved.set(true);
                                            gloo_timers::future::TimeoutFuture::new(1500).await;
                                            save_label_saved.set(false);
                                        }
                                        Err(e) => {
                                            // Fixed UI-SPEC copy only; edits stay in their signals.
                                            save_error_msg.set(Some(save_error_copy(&e.to_string())));
                                        }
                                    }
                                });
                            }
                    },
                }
            } else {
                // Locked state: write gate is closed
                p { class: "voice-settings-hint",
                    style: "color:var(--fg-dim);",
                    "Config writes are disabled. Set security.web_config_write_enabled: true in config.yaml to unlock."
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 40.5 Plan 06 — per-identity voice UI components (D-14 / D-15 / D-09/D-10)
// ─────────────────────────────────────────────────────────────────────────────

/// Surface 1 (D-14): Applies-To target selector.
///
/// A native `<select>` placed at the top of the Voice Settings panel.  Selecting
/// an identity updates `VoiceEditTargetCtx` and resets all three identity Ctx
/// signals to `None` (Approach A seeding strategy — review concern #7) so the
/// panel starts from a clean "inherited" state; no auto-save is triggered.
/// Options: "Global (defaults)" first, then one per identity from the snapshot
/// `identity_list` (orb presets + head presets + custom identities).
///
/// When `identity_list` is empty the selector is hidden and a hint is shown.
#[component]
fn AppliesTo(identity_list: Vec<(String, String)>) -> Element {
    let mut edit_target_sig = use_context::<VoiceEditTargetCtx>().0;
    let mut identity_tts_provider_sig = use_context::<IdentityTtsProviderCtx>().0;
    let mut identity_tts_voice_sig = use_context::<IdentityTtsVoiceCtx>().0;
    let mut identity_realtime_voice_sig = use_context::<IdentityRealtimeVoiceCtx>().0;
    let mut identity_tts_model_sig = use_context::<IdentityTtsModelCtx>().0;
    let mut identity_tts_format_sig = use_context::<IdentityTtsFormatCtx>().0;
    let mut identity_realtime_model_sig = use_context::<IdentityRealtimeModelCtx>().0;

    // Pattern G: .read() in render path (subscribes to signal; NOT .peek()).
    let current_target = edit_target_sig.read().clone();

    rsx! {
        div {
            class: "voice-settings-row",
            style: "border-bottom:1px solid var(--border-subtle); padding-bottom:var(--space-2); margin-bottom:var(--space-2);",
            label {
                r#for: "voice-applies-to-select",
                class: "voice-settings-label",
                style: "color:var(--fg-dim);",
                "Applies to:"
            }
            if identity_list.is_empty() {
                span {
                    style: "color:var(--fg-dim); font-size:var(--type-small);",
                    "Global (defaults) — no identities configured"
                }
            } else {
                select {
                    id: "voice-applies-to-select",
                    class: "voice-settings-select",
                    value: "{current_target}",
                    onchange: move |evt| {
                        let new_val = evt.value();
                        // Approach A: always seed all identity Ctx as None on target
                        // switch (avoids false-dirty from resolved-but-not-explicit values).
                        identity_tts_provider_sig.set(None);
                        identity_tts_voice_sig.set(None);
                        identity_realtime_voice_sig.set(None);
                        identity_tts_model_sig.set(None);
                        identity_tts_format_sig.set(None);
                        identity_realtime_model_sig.set(None);
                        edit_target_sig.set(new_val);
                    },
                    option { value: "global", "Global (defaults)" }
                    for (slug, display_name) in &identity_list {
                        option { value: "{slug}", "{display_name}" }
                    }
                }
            }
        }
    }
}

/// Phase 41.2 Plan 03 (D-01/D-02): "Set {Name} as active" identity-activation
/// control — closes the confirmed D-12 gap (Finding C) by adding the
/// previously-missing runtime writer of `AvatarPrefs.active_identity`.
///
/// Rendered immediately below [`AppliesTo`] so the "this activates the
/// identity you're currently editing" relationship reads without a caption
/// (UI-SPEC §3). The "Applies to:" dropdown itself stays an edit-target-only
/// selector (Phase 40.5 D-14 preserved) — this is a distinct control.
///
/// Writes `AvatarPrefs.active_identity` by mutating the EXISTING
/// `AvatarModeCtx` signal (`avatar_ctx.0.with_mut`) — this is the ONLY write
/// path; the existing persist-on-change effect (mod.rs) fires automatically
/// and writes the `ih.ui.avatar` localStorage pointer. No new `#[server]` fn,
/// no new persistence plumbing, no direct `ui_prefs::write_json` call
/// (RESEARCH.md anti-pattern — a second write path would race the existing
/// persist effect).
///
/// D-12 preserved: this control's write path never touches `voice_loop.rs`
/// or `voice_mode.rs` — the two session-start read sites stay byte-for-byte
/// unchanged. Activation takes effect on the NEXT conversation only; the
/// "Applies to your next conversation." hint is the only signal of the
/// deferred effect (RESEARCH.md Pitfall 5).
///
/// Renders nothing when the edit-target is `"global"` — the AppliesTo
/// dropdown's synthetic "no identity" option is never a valid
/// `active_identity` value (`avatar_logic::is_known_identity` rejects it),
/// so there is no identity to activate and no button to show (T-41.2-04:
/// the write must only ever draw its slug from a real, already-validated
/// identity in `identity_list`).
#[component]
fn SetActiveIdentityControl(identity_list: Vec<(String, String)>) -> Element {
    let edit_target_sig = use_context::<VoiceEditTargetCtx>().0;
    let mut avatar_ctx = use_context::<AvatarModeCtx>();

    // Pattern G: .read() in render path — subscribes so the control re-derives
    // to the passive state in the SAME interaction immediately after the write
    // below (UI-SPEC §3: "the operator must see confirmation without leaving
    // the panel or waiting").
    let edit_target = edit_target_sig.read().clone();
    let active_identity = avatar_ctx.0.read().active_identity.clone();

    if edit_target == "global" {
        return rsx! {};
    }

    // Same source the "Applies to:" dropdown uses for display names.
    let display_name = identity_list
        .iter()
        .find(|(slug, _)| slug == &edit_target)
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| edit_target.clone());

    match activation_state(&edit_target, &active_identity) {
        // State B (UI-SPEC §3): passive confirmation, not a button. A status
        // readout — uses --success, not accent (nothing to apply, no hint).
        ActivationState::Passive => rsx! {
            div {
                class: "voice-settings-row",
                style: "margin-bottom:var(--space-3);",
                span {
                    style: "font-size:var(--type-body,14px); color:var(--success); \
                            white-space:nowrap;",
                    "{display_name} is your active identity ✓"
                }
            }
        },
        // State A (UI-SPEC §3): actionable button + "next conversation" hint.
        // Mirrors .voice-settings-save-btn's fill/text/radius/min-height.
        ActivationState::Actionable => {
            let target = edit_target.clone();
            rsx! {
                div {
                    class: "voice-settings-row",
                    style: "flex-direction:column; align-items:flex-start; margin-bottom:var(--space-3);",
                    button {
                        r#type: "button",
                        style: "display:block; min-height:44px; padding:0 var(--space-3); \
                                border-radius:var(--w-radius-block); font-family:var(--font-mono); \
                                font-size:var(--type-body,14px); cursor:pointer; \
                                background:var(--accent-primary); border:1px solid var(--accent-primary); \
                                color:var(--bg); white-space:nowrap; box-sizing:border-box;",
                        onclick: move |_| {
                            let target = target.clone();
                            // D-02: the ONLY write path — mutates the existing
                            // AvatarModeCtx signal. mod.rs's persist-on-change
                            // effect (subscribed to this same signal) fires
                            // automatically and writes ih.ui.avatar localStorage.
                            avatar_ctx.0.with_mut(|ap| ap.active_identity = target);
                        },
                        "Set {display_name} as active"
                    }
                    p {
                        style: "font-size:var(--type-small,12px); color:var(--fg-dim); \
                                margin-top:var(--space-1); white-space:nowrap;",
                        "Applies to your next conversation."
                    }
                }
            }
        }
    }
}

/// Surface 1 badge (D-15): inline inheritance indicator rendered after a field control.
///
/// - `shown = false` → renders nothing (field has an explicit identity override).
/// - `shown = true, not_set = false` → "(from Global)" in `--fg-dim` (inherited value).
/// - `shown = true, not_set = true`  → "(not set)" in `--warn`  (required field empty).
///
/// Informational only — no click action.
#[component]
fn InheritedFieldBadge(shown: bool, not_set: bool) -> Element {
    if !shown {
        return rsx! {};
    }
    if not_set {
        rsx! {
            span {
                style: "color:var(--warn); font-size:var(--type-small); margin-left:var(--space-2);",
                "(not set)"
            }
        }
    } else {
        rsx! {
            span {
                style: "color:var(--fg-dim); font-size:var(--type-small); margin-left:var(--space-2);",
                "(from Global)"
            }
        }
    }
}

/// D-16 staging rule for a typed identity value: blank means "inherit" and a
/// value equal to the one the identity already resolves to stages nothing. Any
/// other value is staged. `inherited` is `None` once the provider is staged
/// (a resolved value of the old provider is not an inheritance for the new one).
fn staged_from_text(text: &str, inherited: Option<&str>) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || inherited == Some(t) {
        None
    } else {
        Some(t.to_string())
    }
}

/// A registry default as a staged value: an empty default (a provider without
/// that setting) stages nothing.
fn registry_default(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// D-16: the providers the identity Provider select offers -- only ones this
/// server has configured. Edge needs no key and is always offered; ElevenLabs
/// and OpenAI when the snapshot lists them as usable; OpenAI-compatible only
/// when a Base URL is saved (an identity never gets an endpoint of its own).
/// Registry order.
pub(crate) fn identity_provider_options(
    usable: &[String],
    compat_base_url_saved: bool,
) -> Vec<&'static ProviderSpec> {
    TTS_PROVIDERS
        .iter()
        .filter(|spec| match spec.id {
            "edge" => true,
            "openai_compatible" => compat_base_url_saved && usable.iter().any(|id| id == spec.id),
            id => usable.iter().any(|u| u == id),
        })
        .collect()
}

/// What an identity provider switch stages (D-09): the new provider with its
/// model, voice and format reset to that provider's registry defaults, plus the
/// reset note. A provider without a model (Edge) stages no model.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IdentityProviderSwitch {
    pub fields: IdentityVoiceFields,
    pub note: String,
}

/// The registry-driven reset for switching an identity to `new_provider`, or
/// `None` for an id the registry does not know. Replaces the hard-coded
/// default-voice `match` of the old Free Mode section.
pub(crate) fn identity_provider_switch(new_provider: &str) -> Option<IdentityProviderSwitch> {
    let spec = tts_provider(new_provider)?;
    Some(IdentityProviderSwitch {
        fields: IdentityVoiceFields {
            tts_provider: Some(spec.id.to_string()),
            tts_model: registry_default(spec.default_model),
            tts_voice: registry_default(spec.default_voice),
            tts_format: registry_default(spec.default_format),
            ..Default::default()
        },
        note: format!(
            "Switched to {}: model, voice and format reset to its defaults.",
            spec.label
        ),
    })
}

/// The reset note stays until the operator edits the block: it is visible only
/// while the model and voice text and the staged format still equal the
/// defaults the switch staged.
pub(crate) fn identity_switch_note_visible(
    switch: &IdentityProviderSwitch,
    model_text: &str,
    voice_text: &str,
    staged_format: Option<&str>,
) -> bool {
    model_text == switch.fields.tts_model.as_deref().unwrap_or("")
        && voice_text == switch.fields.tts_voice.as_deref().unwrap_or("")
        && staged_format == switch.fields.tts_format.as_deref()
}

/// D-16 ordering: the active service's group is listed first.
fn identity_realtime_first(service: &str) -> bool {
    normalize_service(service) == "realtime"
}

/// Surface 2 (D-09 / D-10 / D-16): per-identity voice override sections.
///
/// Rendered inside `VoiceSettings` **only** when the `AppliesTo` target is not `"global"`.
/// `VoiceSettings` keys it by the edit target, so switching identity remounts it and
/// the typing buffers below are seeded from the new identity's resolved values.
/// Exposes two independent edit groups:
///
/// - **Voice (Turn-based)** — Provider `<select>`, Model and Voice comboboxes
///   ([`OptionPickerField`]), Format `<select>` and Test voice, bound to
///   `IdentityTtsProviderCtx` / `IdentityTtsModelCtx` / `IdentityTtsVoiceCtx` /
///   `IdentityTtsFormatCtx`. An identity never gets an endpoint field.
/// - **Voice (Realtime)** — Voice and Model comboboxes bound to
///   `IdentityRealtimeVoiceCtx` / `IdentityRealtimeModelCtx` (curated lists: the
///   realtime provider and its address are server-wide).
///
/// The active service's group (`VoiceServiceCtx`) is listed first. Each field shows
/// `InheritedFieldBadge` while nothing is staged for it. The Provider select lists
/// only configured providers (`usable_providers`, see [`identity_provider_options`]).
/// Changing the provider resets model, voice and format to that provider's registry
/// defaults (D-09 / T-40.5-06-02, [`identity_provider_switch`]): an OpenAI model is
/// never sent to ElevenLabs.
///
/// `resolved_*` are the server-resolved values for the selected identity (from
/// `VoiceConfigSnapshot.identity_overrides`); they are what a field shows while its
/// context is `None`. `compat_*` are the saved OpenAI-compatible endpoint, used only
/// to list that server's models and voices.
#[component]
fn IdentityVoiceSection(
    resolved_provider: String,
    resolved_voice: Option<String>,
    resolved_model: Option<String>,
    resolved_format: Option<String>,
    resolved_realtime_voice: String,
    resolved_realtime_model: String,
    write_enabled: bool,
    usable_providers: Vec<String>,
    compat_base_url: String,
    compat_path: String,
    compat_key_ref: String,
) -> Element {
    let mut provider_ctx = use_context::<IdentityTtsProviderCtx>().0;
    let mut voice_ctx = use_context::<IdentityTtsVoiceCtx>().0;
    let rt_voice_ctx = use_context::<IdentityRealtimeVoiceCtx>().0;
    let mut model_ctx = use_context::<IdentityTtsModelCtx>().0;
    let mut format_ctx = use_context::<IdentityTtsFormatCtx>().0;
    let rt_model_ctx = use_context::<IdentityRealtimeModelCtx>().0;
    let edit_target_sig = use_context::<VoiceEditTargetCtx>().0;
    let service_sig = use_context::<VoiceServiceCtx>().0;

    // Pattern G: .read() in render path (subscribes; NOT .peek()).
    let edit_target = edit_target_sig.read().clone();
    let realtime_first = identity_realtime_first(&service_sig.read());
    let provider_override = provider_ctx.read().clone();
    let voice_override = voice_ctx.read().clone();
    let model_override = model_ctx.read().clone();
    let format_override = format_ctx.read().clone();
    let rt_voice_override = rt_voice_ctx.read().clone();
    let rt_model_override = rt_model_ctx.read().clone();

    // Displayed control values — the staged value if set, else the resolved one.
    let shown_provider = provider_override
        .clone()
        .unwrap_or_else(|| resolved_provider.clone());
    let shown_voice = voice_override
        .clone()
        .or_else(|| resolved_voice.clone())
        .unwrap_or_default();
    let shown_model = model_override
        .clone()
        .or_else(|| resolved_model.clone())
        .unwrap_or_default();
    let shown_format = format_override
        .clone()
        .or_else(|| resolved_format.clone())
        .unwrap_or_default();
    let shown_rt_voice = rt_voice_override
        .clone()
        .unwrap_or_else(|| resolved_realtime_voice.clone());
    let shown_rt_model = rt_model_override
        .clone()
        .unwrap_or_else(|| resolved_realtime_model.clone());

    // Badge visibility — shown while nothing is staged AND the identity has no
    // value of its own.
    let provider_inherited = provider_override.is_none();
    let voice_inherited = voice_override.is_none() && resolved_voice.is_none();
    let model_inherited = model_override.is_none() && resolved_model.is_none();
    let format_inherited = format_override.is_none() && resolved_format.is_none();
    let rt_inherited = rt_voice_override.is_none();
    let rt_model_inherited = rt_model_override.is_none();

    // D-16: only providers this server has configured are selectable. An
    // identity already on an unconfigured one (a seeded ElevenLabs persona on a
    // server with no key) keeps that value visible, labelled, instead of the
    // select silently showing another provider.
    let provider_options =
        identity_provider_options(&usable_providers, !compat_base_url.trim().is_empty());
    let current_provider_listed = provider_options.iter().any(|s| s.id == shown_provider);

    // The selected provider's registry entry drives which rows exist (Edge has
    // no model) and every default, list and format.
    let spec = tts_provider(&shown_provider);
    let show_model = spec.is_some_and(|s| s.model_field);
    let formats: &'static [&'static str] = spec.map(|s| s.formats).unwrap_or(&[]);
    let default_model = spec.map(|s| s.default_model).unwrap_or("").to_string();
    let default_voice = spec.map(|s| s.default_voice).unwrap_or("").to_string();
    let model_curated: Vec<String> = spec
        .map(|s| s.curated_models.iter().map(|m| m.to_string()).collect())
        .unwrap_or_default();
    let voice_curated: Vec<String> = spec
        .map(|s| s.curated_voices.iter().map(|v| v.to_string()).collect())
        .unwrap_or_default();
    let provider_label = spec
        .map(|s| s.label.to_string())
        .unwrap_or_else(|| shown_provider.clone());
    // The saved OpenAI-compatible endpoint is only meaningful for that provider.
    let (endpoint_base, endpoint_path, endpoint_key) = if spec.is_some_and(|s| s.endpoint_fields) {
        (
            compat_base_url.clone(),
            compat_path.clone(),
            compat_key_ref.clone(),
        )
    } else {
        (String::new(), String::new(), String::new())
    };

    // E13 error: the same validation copy as the global fields.
    let model_error = model_override
        .as_deref()
        .and_then(|v| validate_model_id(v.trim()).err())
        .map(|e| e.copy().to_string());
    let voice_error = voice_override
        .as_deref()
        .and_then(|v| validate_voice_id(v.trim()).err())
        .map(|e| e.copy().to_string());
    let rt_voice_error = rt_voice_override
        .as_deref()
        .and_then(|v| validate_voice_id(v.trim()).err())
        .map(|e| e.copy().to_string());
    let rt_model_error = rt_model_override
        .as_deref()
        .and_then(|v| validate_model_id(v.trim()).err())
        .map(|e| e.copy().to_string());
    // The realtime provider is server-wide; its curated lists feed the pickers.
    let realtime_spec = &REALTIME_PROVIDERS[0];
    let rt_voice_curated: Vec<String> = realtime_spec
        .curated_voices
        .iter()
        .map(|v| v.to_string())
        .collect();
    let rt_model_curated: Vec<String> = realtime_spec
        .curated_models
        .iter()
        .map(|m| m.to_string())
        .collect();

    // Typing buffers for the four comboboxes. The picker edits a plain
    // `Signal<String>`; the effects below stage that text into the contexts
    // (`None` = inherit). Seeded once: the section is keyed by the edit target.
    let mut model_text = use_signal(|| shown_model.clone());
    let mut voice_text = use_signal(|| shown_voice.clone());
    let rt_voice_text = use_signal(|| shown_rt_voice.clone());
    let rt_model_text = use_signal(|| shown_rt_model.clone());
    let mut list_cache: ListCache = use_signal(HashMap::new);
    // The reset note of the last provider switch (D-09); see
    // `identity_switch_note_visible` for when it clears.
    let mut switch_note: Signal<Option<IdentityProviderSwitch>> = use_signal(|| None);

    use_effect(use_reactive!(|resolved_model| {
        let mut model_ctx = model_ctx;
        let text = model_text.read().clone();
        let inherited = if provider_ctx.peek().is_none() {
            resolved_model.clone()
        } else {
            None
        };
        let desired = staged_from_text(&text, inherited.as_deref());
        if *model_ctx.peek() != desired {
            model_ctx.set(desired);
        }
    }));
    use_effect(use_reactive!(|resolved_voice| {
        let mut voice_ctx = voice_ctx;
        let text = voice_text.read().clone();
        let inherited = if provider_ctx.peek().is_none() {
            resolved_voice.clone()
        } else {
            None
        };
        let desired = staged_from_text(&text, inherited.as_deref());
        if *voice_ctx.peek() != desired {
            voice_ctx.set(desired);
        }
    }));
    // The realtime provider is not identity-selectable, so an identity's
    // resolved value is always the one to inherit.
    use_effect(use_reactive!(|resolved_realtime_voice| {
        let mut rt_voice_ctx = rt_voice_ctx;
        let text = rt_voice_text.read().clone();
        let desired = staged_from_text(&text, Some(resolved_realtime_voice.as_str()));
        if *rt_voice_ctx.peek() != desired {
            rt_voice_ctx.set(desired);
        }
    }));
    use_effect(use_reactive!(|resolved_realtime_model| {
        let mut rt_model_ctx = rt_model_ctx;
        let text = rt_model_text.read().clone();
        let desired = staged_from_text(&text, Some(resolved_realtime_model.as_str()));
        if *rt_model_ctx.peek() != desired {
            rt_model_ctx.set(desired);
        }
    }));

    // The reset note: visible only while the block still holds the defaults
    // the switch staged.
    let note_text = {
        let model_now = model_text.read().clone();
        let voice_now = voice_text.read().clone();
        switch_note
            .read()
            .as_ref()
            .filter(|n| {
                identity_switch_note_visible(n, &model_now, &voice_now, format_override.as_deref())
            })
            .map(|n| n.note.clone())
    };

    // D-17: Test voice auditions the unsaved values shown here.
    let test_settings = identity_test_settings(
        &shown_provider,
        &IdentityVoiceFields {
            tts_model: Some(shown_model.clone()),
            tts_voice: Some(shown_voice.clone()),
            tts_format: Some(shown_format.clone()),
            ..Default::default()
        },
    );

    let turn_based_group = rsx! {
        // ── Voice (Turn-based) ────────────────────────────────────────────
        p {
            class: "voice-settings-section-header",
            role: "heading",
            "aria-level": "3",
            "Voice (Turn-based)"
        }
        hr { class: "voice-settings-separator" }

        div { class: "voice-settings-row",
            label {
                r#for: "identity-tts-provider-select",
                class: "voice-settings-label",
                "Provider"
            }
            div { style: "display:flex; align-items:center; gap:var(--space-1);",
                select {
                    id: "identity-tts-provider-select",
                    class: "voice-settings-select",
                    disabled: !write_enabled,
                    value: "{shown_provider}",
                    onchange: move |evt| {
                        // D-09 / T-40.5-06-02: a provider switch resets model, voice
                        // and format to the NEW provider's registry defaults, so an
                        // OpenAI model is never sent to ElevenLabs.
                        let Some(switch) = identity_provider_switch(&evt.value()) else {
                            return;
                        };
                        let staged = switch.fields.clone();
                        let new_provider = staged.tts_provider.clone().unwrap_or_default();
                        model_text.set(staged.tts_model.clone().unwrap_or_default());
                        voice_text.set(staged.tts_voice.clone().unwrap_or_default());
                        provider_ctx.set(staged.tts_provider);
                        model_ctx.set(staged.tts_model);
                        voice_ctx.set(staged.tts_voice);
                        format_ctx.set(staged.tts_format);
                        list_cache
                            .write()
                            .retain(|key, _| !(key.family == "tts" && key.provider == new_provider));
                        switch_note.set(Some(switch));
                    },
                    for p_spec in provider_options.iter() {
                        option {
                            key: "{p_spec.id}",
                            value: "{p_spec.id}",
                            selected: p_spec.id == shown_provider,
                            "{p_spec.label}"
                        }
                    }
                    if !current_provider_listed {
                        option { value: "{shown_provider}", selected: true, "{provider_label} (not configured)" }
                    }
                }
                InheritedFieldBadge { shown: provider_inherited, not_set: false }
            }
        }
        p { class: "voice-settings-hint",
            "Overrides the global TTS provider for this identity only."
        }
        if let Some(note) = note_text.clone() {
            p {
                id: "identity-tts-provider-status",
                class: "voice-field-status",
                role: "status",
                "aria-live": "polite",
                "{note}"
            }
        }

        if show_model {
            OptionPickerField {
                label: "Model".to_string(),
                field_id: "identity-tts-model".to_string(),
                kind: OptionKind::Models,
                family: "tts".to_string(),
                provider: shown_provider.clone(),
                provider_label: provider_label.clone(),
                value: model_text,
                curated: model_curated.clone(),
                base_url: endpoint_base.clone(),
                path: endpoint_path.clone(),
                key_ref: endpoint_key.clone(),
                model_for_voices: String::new(),
                fetchable: spec.is_some_and(|s| s.discovers_models),
                visible: true,
                disabled: !write_enabled,
                cache: list_cache,
                seeded: default_model.clone(),
                placeholder: "Pick or type a model".to_string(),
                title: "Identity TTS model".to_string(),
                error: model_error.clone(),
            }
            if model_inherited {
                div { style: "margin-top:var(--space-1);",
                    InheritedFieldBadge { shown: true, not_set: false }
                }
            }
        }

        OptionPickerField {
            label: "Voice".to_string(),
            field_id: "identity-tts-voice".to_string(),
            kind: OptionKind::Voices,
            family: "tts".to_string(),
            provider: shown_provider.clone(),
            provider_label: provider_label.clone(),
            value: voice_text,
            curated: voice_curated.clone(),
            base_url: endpoint_base.clone(),
            path: endpoint_path.clone(),
            key_ref: endpoint_key.clone(),
            model_for_voices: shown_model.clone(),
            fetchable: spec.is_some_and(|s| s.discovers_voices),
            visible: true,
            disabled: !write_enabled,
            cache: list_cache,
            seeded: default_voice.clone(),
            placeholder: "Pick or type a voice".to_string(),
            title: "Identity TTS voice".to_string(),
            noun: "voice".to_string(),
            error: voice_error.clone(),
        }
        if voice_inherited {
            div { style: "margin-top:var(--space-1);",
                InheritedFieldBadge { shown: true, not_set: true }
            }
        }

        if !formats.is_empty() {
            VoiceField {
                label: "Format".to_string(),
                field_id: "identity-tts-format-select".to_string(),
                div { style: "display:flex; align-items:center; gap:var(--space-1);",
                    select {
                        id: "identity-tts-format-select",
                        class: "voice-settings-select",
                        disabled: !write_enabled,
                        value: "{shown_format}",
                        "aria-label": "Identity TTS output format",
                        onchange: move |evt| {
                            let v = evt.value();
                            format_ctx.set((!v.is_empty()).then_some(v));
                        },
                        // Blank = inherit the Global format.
                        option { value: "", selected: shown_format.is_empty(), "Default" }
                        if !shown_format.is_empty() && !formats.contains(&shown_format.as_str()) {
                            option {
                                value: "{shown_format}",
                                selected: true,
                                "{shown_format}"
                            }
                        }
                        for f in formats.iter() {
                            option {
                                key: "{f}",
                                value: "{f}",
                                selected: *f == shown_format,
                                "{f}"
                            }
                        }
                    }
                    InheritedFieldBadge { shown: format_inherited, not_set: false }
                }
            }
        }

        // ── Test voice (Phase 54.7 Plan 12/15, D-17) ───────────────────────
        // The one shared button, auditioning THIS identity's unsaved values.
        voice_test_controls::TestVoiceButton {
            identity_slug: Some(edit_target.clone()),
            settings: Some(test_settings.clone()),
            disabled_reason: None,
        }
    };

    let realtime_group = rsx! {
        // ── Voice (Realtime) ──────────────────────────────────────────────
        p {
            class: "voice-settings-section-header",
            role: "heading",
            "aria-level": "3",
            "Voice (Realtime)"
        }
        hr { class: "voice-settings-separator" }

        // D-18: the curated voices are the server-validated set rather than the
        // UI-SPEC's fable/onyx/nova (standard-TTS voices that issue_realtime_token
        // would reject).
        OptionPickerField {
            label: "Voice".to_string(),
            field_id: "identity-rt-voice".to_string(),
            kind: OptionKind::Voices,
            family: "realtime".to_string(),
            provider: realtime_spec.id.to_string(),
            provider_label: realtime_spec.label.to_string(),
            value: rt_voice_text,
            curated: rt_voice_curated.clone(),
            base_url: String::new(),
            path: String::new(),
            key_ref: String::new(),
            model_for_voices: String::new(),
            fetchable: false,
            visible: true,
            disabled: !write_enabled,
            cache: list_cache,
            seeded: realtime_spec.default_voice.to_string(),
            placeholder: "Pick or type a voice".to_string(),
            title: "Identity realtime voice".to_string(),
            noun: "voice".to_string(),
            error: rt_voice_error.clone(),
        }
        if rt_inherited {
            div { style: "margin-top:var(--space-1);",
                InheritedFieldBadge { shown: true, not_set: false }
            }
        }

        OptionPickerField {
            label: "Model".to_string(),
            field_id: "identity-rt-model".to_string(),
            kind: OptionKind::Models,
            family: "realtime".to_string(),
            provider: realtime_spec.id.to_string(),
            provider_label: realtime_spec.label.to_string(),
            value: rt_model_text,
            curated: rt_model_curated.clone(),
            base_url: String::new(),
            path: String::new(),
            key_ref: String::new(),
            model_for_voices: String::new(),
            fetchable: false,
            visible: true,
            disabled: !write_enabled,
            cache: list_cache,
            seeded: realtime_spec.default_model.to_string(),
            placeholder: "Pick or type a model".to_string(),
            title: "Identity realtime model".to_string(),
            error: rt_model_error.clone(),
        }
        if rt_model_inherited {
            div { style: "margin-top:var(--space-1);",
                InheritedFieldBadge { shown: true, not_set: false }
            }
        }

        p { class: "voice-settings-hint",
            "Realtime voice and model come from the realtime provider set on the server. Only the voice and model are per-identity."
        }
        p { class: "voice-settings-hint",
            "Changes take effect next conversation."
        }
    };

    // D-16: the active service's group is listed first.
    rsx! {
        if realtime_first {
            {realtime_group}
            {turn_based_group}
        } else {
            {turn_based_group}
            {realtime_group}
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 40.5 Plan 07 — Orb appearance editor (D-04 / D-05 / D-17)
// ─────────────────────────────────────────────────────────────────────────────

/// Surface 3 (D-04): Orb appearance editor — 2×2 preset card grid + free-form knobs.
///
/// Rendered by VoiceSettings only when that form owns the orb appearance
/// (`voice_form_owns_orb_appearance`): the voice-mode overlay's form, for an orb
/// Applies-to target. The System screen's General-tab Voice panel opts out
/// (G-54.4-34); in System the orb appearance lives on the Orb tab
/// (`OrbSettingsPanel`), which mounts this same section through
/// `OrbAppearanceEditor`.
///
/// Knobs write to OrbStyleCtx / OrbBaseHueCtx / OrbSizeCtx / OrbGlowCtx;
/// orb_canvas.rs bridges those signals to orb.js via document::eval (Task 2, live preview).
/// The identity Save handler (Plan 06, extended here) persists all four appearance fields
/// to config.identities[slug].appearance via VoiceWritePayload (D-17; next conversation).
#[component]
fn OrbAppearanceSection(write_enabled: bool) -> Element {
    let style_ctx = use_context::<OrbStyleCtx>().0;
    // Phase 41.2 Plan 01 (D-03): settling flag written by the orb_canvas.rs
    // bridge; drives the transient "Switching style…" label below.
    let settling_ctx = use_context::<OrbSettlingCtx>().0;

    // Pattern G: .read() in render path (subscribes to re-render on change; NOT .peek()).
    let current_style = style_ctx.read().clone();
    let is_settling = *settling_ctx.read();

    // Preset definitions: (style_key, icon, name, sublabel) — UI-SPEC preset table.
    let preset_defs = [
        ("classic", "◉", "Classic", "IcosahedronGeometry"),
        ("bloom", "✦", "Bloom", "Speech-synced glow"),
        ("ascii", "▓", "ASCII", "AsciiEffect"),
        ("network", "⋯", "Network", "Animated lines"),
    ];

    rsx! {
        // Section heading
        p {
            class: "voice-settings-section-header",
            role: "heading",
            "aria-level": "3",
            "Appearance"
        }
        hr { class: "voice-settings-separator" }

        // D-05: read-only affordance — one line above the tile grid (not per-tile)
        // so a disabled click doesn't silently no-op with zero visual explanation.
        if !write_enabled {
            p {
                style: "font-size:var(--type-small,12px); color:var(--fg-dim); \
                        white-space:nowrap; margin-bottom:var(--space-2);",
                "Appearance editing is disabled."
            }
        }

        // "Style" subheading
        p {
            class: "voice-settings-label",
            style: "margin-bottom:var(--space-2); color:var(--fg-dim);",
            "Style"
        }

        // 2×2 preset card grid (D-04 — four ORB_PRESET_REGISTRY entries)
        div {
            style: "display:grid; grid-template-columns:1fr 1fr; gap:var(--space-2); margin-bottom:var(--space-3);",
            for (style_key, icon, name, sublabel) in preset_defs {
                OrbStyleCard {
                    key: "{style_key}",
                    style_key: style_key.to_string(),
                    icon: icon.to_string(),
                    name: name.to_string(),
                    sublabel: sublabel.to_string(),
                    selected: style_key == current_style.as_str(),
                    write_enabled,
                }
            }
        }

        // D-03: transient "Switching style…" settling label — appears the
        // instant a style switch begins, disappears the instant the rebuild
        // settles (eval resolves or the bounded fallback fires). Mirrors the
        // `.voice-state-label.is-thinking` transitional semantic (--warn, no
        // spinner asset).
        if is_settling {
            p {
                style: "font-size:var(--type-small,12px); color:var(--warn); \
                        margin-top:calc(-1 * var(--space-2)); margin-bottom:var(--space-3);",
                "Switching style…"
            }
        }

        // Free-form knobs (shown below preset grid for all presets)
        OrbColorKnob {}
        OrbSizeKnob {}
        OrbGlowKnob {}
    }
}

/// Single orb preset card (icon glyph / name weight-700 / sublabel).
///
/// Selected state: `--accent-primary` border + 6% accent fill (matches `.wh-tab:is-active`).
/// Hover: `--w-bg-3` background (handled via CSS `button:hover`).
/// Clicking sets `OrbStyleCtx` to `style_key`; the orb_canvas.rs eval bridge then calls
/// `window.<ORB>.setStyle(style_key)` so the live orb switches render mode (D-03).
/// D-04: card style string for the **selected** tile — bold accent fill
/// (raised from the old weak 6%) + full accent border. Extracted as a
/// standalone pure fn (not inlined in the branch) so a native
/// `#[cfg(test)]` unit test can assert the exact style string without a
/// browser — a regression guard against reverting the fill back to 6%
/// (Finding B / the original "which tile is see-through" bug).
fn orb_style_card_selected_style() -> &'static str {
    "background:color-mix(in oklab, var(--accent-primary) 22%, transparent); \
     border:1px solid var(--accent-primary); border-radius:var(--w-radius-block,6px); \
     padding:12px 16px; cursor:pointer; text-align:left; display:flex; \
     flex-direction:column; gap:var(--space-1); width:100%; position:relative;"
}

/// Card style string for the **unselected** tile — recessive solid
/// `--w-bg-2` fill, unchanged from the pre-D-04 baseline (Finding B
/// established the bug was the selected side being too weak, not this side
/// being too strong). `position:relative` matches the selected variant so
/// selecting a tile never shifts its box model.
fn orb_style_card_unselected_style() -> &'static str {
    "background:var(--w-bg-2); border:1px solid var(--border-subtle); \
     border-radius:var(--w-radius-block,6px); padding:12px 16px; \
     cursor:pointer; text-align:left; display:flex; flex-direction:column; \
     gap:var(--space-1); width:100%; position:relative;"
}

#[component]
fn OrbStyleCard(
    style_key: String,
    icon: String,
    name: String,
    sublabel: String,
    selected: bool,
    write_enabled: bool,
) -> Element {
    let mut style_ctx = use_context::<OrbStyleCtx>().0;
    let key_clone = style_key.clone();

    let card_style = if selected {
        orb_style_card_selected_style()
    } else {
        orb_style_card_unselected_style()
    };

    // D-05: read-only tiles recolor icon/text to `--fg-disabled` (not
    // `--fg-dim`/`--fg`) so a disabled tile reads as intentionally inert,
    // not "stuck". A selected read-only tile keeps its accent border (still
    // shows WHICH is selected) but the checkmark renders at reduced
    // (`--fg-disabled`) intensity — the selection signal is preserved, only
    // dimmed.
    let (icon_color, text_color, sublabel_color, marker_color) = if !write_enabled {
        (
            "var(--fg-disabled)",
            "var(--fg-disabled)",
            "var(--fg-disabled)",
            "var(--fg-disabled)",
        )
    } else if selected {
        (
            "var(--accent-primary)",
            "var(--fg)",
            "var(--fg-dim)",
            "var(--accent-primary)",
        )
    } else {
        (
            "var(--fg-dim)",
            "var(--fg)",
            "var(--fg-dim)",
            "var(--accent-primary)",
        )
    };
    let icon_style = format!("font-size:18px; color:{icon_color};");
    let name_style =
        format!("font-size:var(--type-body,14px); color:{text_color}; font-weight:700;");
    let sublabel_style = format!("font-size:var(--type-small,12px); color:{sublabel_color};");
    let marker_style = format!("position:absolute; top:8px; right:8px; font-size:14px; color:{marker_color}; line-height:1;");

    rsx! {
        button {
            r#type: "button",
            style: "{card_style}",
            disabled: !write_enabled,
            onclick: move |_| {
                style_ctx.set(key_clone.clone());
            },
            span { style: "{icon_style}", "{icon}" }
            span { style: "{name_style}", "{name}" }
            span { style: "{sublabel_style}", "{sublabel}" }
            // D-04 checkmark/radio marker — selected only, absolute-positioned
            // (relative to the button's `position:relative`) so selecting a
            // tile never shifts layout.
            if selected {
                span { style: "{marker_style}", "✓" }
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod orb_style_card_tests {
    use super::orb_style_card_selected_style;

    /// D-04: the selected-state card style string must use the bold 22%
    /// accent fill and keep the full accent border.
    #[test]
    fn orb_style_card_selected_uses_bold_accent_fill() {
        let style = orb_style_card_selected_style();
        assert!(
            style.contains("accent-primary) 22%"),
            "selected-tile style must use the D-04 22% accent fill, got: {style}"
        );
        assert!(
            style.contains("border:1px solid var(--accent-primary)"),
            "selected-tile style must keep the full accent border, got: {style}"
        );
    }

    /// Regression guard: must not silently revert to the old, too-weak 6%
    /// fill that caused Finding B (the selected tile reading as "the
    /// see-through one").
    #[test]
    fn orb_style_card_selected_style_does_not_regress_to_weak_fill() {
        let style = orb_style_card_selected_style();
        assert!(
            !style.contains("accent-primary) 6%"),
            "selected-tile style must not regress to the old weak 6% fill, got: {style}"
        );
    }
}

/// Base hue range slider (0–360°) with hue-gradient track + degree readout.
///
/// Writes to `OrbBaseHueCtx` (D-05). Driving `setBaseHue` (not per-state absolute hex)
/// preserves the relative listening/thinking/speaking color offsets (D-05 prohibition).
#[component]
fn OrbColorKnob() -> Element {
    let mut hue_ctx = use_context::<OrbBaseHueCtx>().0;
    // Pattern G: .read() in render path (subscribes; NOT .peek()).
    let hue_val = *hue_ctx.read();

    rsx! {
        div {
            class: "voice-settings-row",
            style: "flex-direction:column; align-items:flex-start; gap:var(--space-1); margin-bottom:var(--space-2);",
            label {
                r#for: "orb-color-knob",
                class: "voice-settings-label",
                style: "color:var(--fg-dim);",
                "Color (base hue)"
            }
            div { style: "display:flex; align-items:center; gap:var(--space-2); width:100%;",
                input {
                    id: "orb-color-knob",
                    r#type: "range",
                    min: "0",
                    max: "360",
                    step: "1",
                    value: "{hue_val}",
                    // Hue gradient track per UI-SPEC; accent-color drives thumb color.
                    style: "flex:1; min-height:24px; accent-color:var(--accent-primary); \
                            background:linear-gradient(to right, \
                              hsl(0,100%,60%),hsl(30,100%,60%),hsl(60,100%,60%),\
                              hsl(90,100%,60%),hsl(120,100%,60%),hsl(150,100%,60%),\
                              hsl(180,100%,60%),hsl(210,100%,60%),hsl(240,100%,60%),\
                              hsl(270,100%,60%),hsl(300,100%,60%),hsl(330,100%,60%),\
                              hsl(360,100%,60%)); border-radius:2px;",
                    "aria-label": "Orb base hue",
                    oninput: move |evt| {
                        if let Ok(v) = evt.value().parse::<u16>() {
                            hue_ctx.set(v.min(360));
                        }
                    },
                }
                span {
                    style: "font-size:var(--type-body,14px); color:var(--fg); \
                            font-variant-numeric:tabular-nums; min-width:4ch; text-align:right;",
                    "{hue_val}°"
                }
            }
            p { class: "voice-settings-hint",
                "Sets the idle color. Listening, thinking, and speaking colors shift relative to this."
            }
        }
    }
}

/// Orb scale range slider (0.5–2.0×) with multiplier readout.
///
/// Writes to `OrbSizeCtx`; orb_canvas.rs bridges to `window.<ORB>.setSize(scale)`.
#[component]
fn OrbSizeKnob() -> Element {
    let mut size_ctx = use_context::<OrbSizeCtx>().0;
    // Pattern G: .read() in render path.
    let size_val = *size_ctx.read();

    rsx! {
        div {
            class: "voice-settings-row",
            style: "flex-direction:column; align-items:flex-start; gap:var(--space-1); margin-bottom:var(--space-2);",
            label {
                r#for: "orb-size-knob",
                class: "voice-settings-label",
                style: "color:var(--fg-dim);",
                "Size"
            }
            div { style: "display:flex; align-items:center; gap:var(--space-2); width:100%;",
                input {
                    id: "orb-size-knob",
                    r#type: "range",
                    min: "0.5",
                    max: "2.0",
                    step: "0.1",
                    value: "{size_val}",
                    style: "flex:1; min-height:24px; accent-color:var(--accent-primary);",
                    "aria-label": "Orb size",
                    oninput: move |evt| {
                        if let Ok(v) = evt.value().parse::<f32>() {
                            size_ctx.set(v.clamp(0.5, 2.0));
                        }
                    },
                }
                span {
                    style: "font-size:var(--type-body,14px); color:var(--fg); \
                            font-variant-numeric:tabular-nums; min-width:5ch; text-align:right;",
                    "{size_val:.1}×"
                }
            }
        }
    }
}

/// Orb glow intensity range slider (0.0–1.0) with two-decimal readout.
///
/// Writes to `OrbGlowCtx`; orb_canvas.rs bridges to `window.<ORB>.setGlow(intensity)`.
#[component]
fn OrbGlowKnob() -> Element {
    let mut glow_ctx = use_context::<OrbGlowCtx>().0;
    // Pattern G: .read() in render path.
    let glow_val = *glow_ctx.read();

    rsx! {
        div {
            class: "voice-settings-row",
            style: "flex-direction:column; align-items:flex-start; gap:var(--space-1); margin-bottom:var(--space-2);",
            label {
                r#for: "orb-glow-knob",
                class: "voice-settings-label",
                style: "color:var(--fg-dim);",
                "Glow intensity"
            }
            div { style: "display:flex; align-items:center; gap:var(--space-2); width:100%;",
                input {
                    id: "orb-glow-knob",
                    r#type: "range",
                    min: "0.0",
                    max: "1.0",
                    step: "0.05",
                    value: "{glow_val}",
                    style: "flex:1; min-height:24px; accent-color:var(--accent-primary);",
                    "aria-label": "Orb glow intensity",
                    oninput: move |evt| {
                        if let Ok(v) = evt.value().parse::<f32>() {
                            glow_ctx.set(v.clamp(0.0, 1.0));
                        }
                    },
                }
                span {
                    style: "font-size:var(--type-body,14px); color:var(--fg); \
                            font-variant-numeric:tabular-nums; min-width:4ch; text-align:right;",
                    "{glow_val:.2}"
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 54.4 Plan 16 (G-54.4-25): standalone Orb settings tab
// ─────────────────────────────────────────────────────────────────────────────

use crate::components::hermes_app::avatar_logic::ORB_PRESET_REGISTRY;

/// One orb's appearance: the four values the Orb tab edits and saves.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OrbAppearance {
    pub style: String,
    pub base_hue: u16,
    pub size: f32,
    pub glow: f32,
}

/// The saved appearance of orb `slug`: each saved override field, else the
/// preset default. `None` unless `slug` is an `ORB_PRESET_REGISTRY` id.
///
/// This is the one definition of "saved appearance". The Orb tab and
/// `VoiceSettings`' identity-switch hydrate effect both call it, so the two
/// surfaces cannot disagree about an orb's saved look.
pub(crate) fn resolve_orb_appearance(
    slug: &str,
    saved: Option<&IdentityOverride>,
) -> Option<OrbAppearance> {
    let preset = ORB_PRESET_REGISTRY.iter().find(|p| p.id == slug)?;
    Some(OrbAppearance {
        style: saved
            .and_then(|o| o.appearance_style.clone())
            .unwrap_or_else(|| preset.default_style.to_string()),
        base_hue: saved
            .and_then(|o| o.appearance_base_hue)
            .unwrap_or(preset.default_hue),
        size: saved
            .and_then(|o| o.appearance_size)
            .unwrap_or(preset.default_size),
        glow: saved
            .and_then(|o| o.appearance_glow)
            .unwrap_or(preset.default_glow),
    })
}

/// One resolved entry per orb preset, in registry order. Overrides for any
/// other slug (head avatars, custom identities) are ignored.
pub(crate) fn saved_orb_appearances(
    overrides: &[IdentityOverride],
) -> Vec<(String, OrbAppearance)> {
    ORB_PRESET_REGISTRY
        .iter()
        .filter_map(|p| {
            let ov = overrides.iter().find(|o| o.slug == p.id);
            resolve_orb_appearance(p.id, ov).map(|a| (p.id.to_string(), a))
        })
        .collect()
}

/// The orb the Orb tab opens on: the active identity when it is an orb, else
/// the first registry entry (`orb_classic`).
pub(crate) fn initial_orb_selection(active_identity: &str) -> String {
    if ORB_PRESET_REGISTRY.iter().any(|p| p.id == active_identity) {
        active_identity.to_string()
    } else {
        ORB_PRESET_REGISTRY[0].id.to_string()
    }
}

/// True when the live values differ from the saved baseline. Style and hue are
/// exact; size and glow compare by absolute difference above `f32::EPSILON`.
pub(crate) fn orb_appearance_is_dirty(live: &OrbAppearance, saved: &OrbAppearance) -> bool {
    live.style != saved.style
        || live.base_hue != saved.base_hue
        || (live.size - saved.size).abs() > f32::EPSILON
        || (live.glow - saved.glow).abs() > f32::EPSILON
}

/// Write payload for one orb's appearance. Only `identity_slug` and the four
/// appearance fields are `Some`; `merge_voice_payload` leaves every `None`
/// field untouched, so this cannot reach the global voice config or the
/// identity's voice overrides.
pub(crate) fn orb_appearance_payload(slug: &str, a: &OrbAppearance) -> VoiceWritePayload {
    VoiceWritePayload {
        identity_slug: Some(slug.to_string()),
        identity_appearance_style: Some(a.style.clone()),
        identity_base_hue: Some(a.base_hue),
        identity_size: Some(a.size),
        identity_glow: Some(a.glow),
        ..Default::default()
    }
}

/// Replace the slug's saved entry, or push it when absent.
pub(crate) fn upsert_saved_appearance(
    saved: &mut Vec<(String, OrbAppearance)>,
    slug: &str,
    a: OrbAppearance,
) {
    if let Some(entry) = saved.iter_mut().find(|(s, _)| s == slug) {
        entry.1 = a;
    } else {
        saved.push((slug.to_string(), a));
    }
}

// ── Phase 54.4 Plan 17 (G-54.4-34): the Voice form's orb-appearance opt-out ──

/// The one decision for "this Voice form edits the orb appearance".
///
/// True only when the form shows the Appearance section
/// (`show_orb_appearance`) AND the Applies-to target is an orb preset. It
/// drives the section mount, the orb dirty term and the identity save's
/// appearance fields. `"global"` is not an `ORB_PRESET_REGISTRY` id, so the old
/// `edit_target != "global" && is_orb_target` gate equals
/// `voice_form_owns_orb_appearance(true, ..)`.
pub(crate) fn voice_form_owns_orb_appearance(show_orb_appearance: bool, edit_target: &str) -> bool {
    show_orb_appearance && ORB_PRESET_REGISTRY.iter().any(|p| p.id == edit_target)
}

/// The identity-switch hydrate effect's one decision.
///
/// A form that does not show the Appearance section never re-derives, so it
/// never writes the shared root `Orb*` signals (G-54.4-34). The G-41.2-11
/// not-loaded guard (`loaded`) and the 41.2 reset-fight guard
/// (`should_rehydrate_appearance`) are kept.
pub(crate) fn voice_form_should_hydrate_appearance(
    show_orb_appearance: bool,
    loaded: bool,
    prev: &str,
    next: &str,
) -> bool {
    show_orb_appearance && loaded && should_rehydrate_appearance(prev, next)
}

/// True when this form owns the orb appearance AND the live values differ from
/// its loaded baseline. Reuses `orb_appearance_is_dirty`, which is identical to
/// the inline comparison it replaces.
pub(crate) fn voice_form_orb_dirty(
    owns: bool,
    live: &OrbAppearance,
    loaded: &OrbAppearance,
) -> bool {
    owns && orb_appearance_is_dirty(live, loaded)
}

/// Phase 54.7 Plan 15 (D-16): the six identity voice values one identity save
/// can carry. `None` = inherit the Global value (nothing staged). An identity
/// never carries an endpoint, key reference or STT setting, so none exists here.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct IdentityVoiceFields {
    pub tts_provider: Option<String>,
    pub tts_model: Option<String>,
    pub tts_voice: Option<String>,
    pub tts_format: Option<String>,
    pub realtime_voice: Option<String>,
    pub realtime_model: Option<String>,
}

impl IdentityVoiceFields {
    /// True when any identity value is staged (the identity save is dirty).
    pub(crate) fn is_staged(&self) -> bool {
        self.tts_provider.is_some()
            || self.tts_model.is_some()
            || self.tts_voice.is_some()
            || self.tts_format.is_some()
            || self.realtime_voice.is_some()
            || self.realtime_model.is_some()
    }

    /// Staged values the server's shared rules would reject (E13 error): the
    /// same `validate_model_id` / `validate_voice_id` the global fields use.
    pub(crate) fn blocking_error_count(&self) -> usize {
        let model = |v: &Option<String>| {
            v.as_deref()
                .is_some_and(|v| validate_model_id(v.trim()).is_err())
        };
        let voice = |v: &Option<String>| {
            v.as_deref()
                .is_some_and(|v| validate_voice_id(v.trim()).is_err())
        };
        [
            model(&self.tts_model),
            voice(&self.tts_voice),
            voice(&self.realtime_voice),
            model(&self.realtime_model),
        ]
        .into_iter()
        .filter(|bad| *bad)
        .count()
    }

    /// Fold a successful save into a snapshot override so the section shows
    /// what was saved (a staged value replaces the resolved one).
    pub(crate) fn apply_to_override(&self, ov: &mut IdentityOverride) {
        if let Some(v) = &self.tts_provider {
            ov.free_mode_provider = v.clone();
        }
        if let Some(v) = &self.tts_model {
            ov.free_mode_model = Some(v.clone());
        }
        if let Some(v) = &self.tts_voice {
            ov.free_mode_voice = Some(v.clone());
        }
        if let Some(v) = &self.tts_format {
            ov.free_mode_format = Some(v.clone());
        }
        if let Some(v) = &self.realtime_voice {
            ov.realtime_voice = v.clone();
        }
        if let Some(v) = &self.realtime_model {
            ov.realtime_model = v.clone();
        }
    }
}

/// D-17: the Test voice request for an identity's UNSAVED values under
/// `provider` (the provider the section shows). Only the fields the provider's
/// registry entry uses are sent, and a blank value is never sent -- the server
/// then falls back to the identity's saved / Global value for it. An identity
/// has no endpoint, so none is carried.
pub(crate) fn identity_test_settings(
    provider: &str,
    fields: &IdentityVoiceFields,
) -> TtsTestSettings {
    let mut settings = TtsTestSettings {
        provider: Some(provider.to_string()),
        ..Default::default()
    };
    let Some(spec) = tts_provider(provider) else {
        return settings;
    };
    let present = |v: &Option<String>| {
        v.as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    if spec.model_field {
        settings.model = present(&fields.tts_model);
    }
    if spec.voice_field {
        settings.voice = present(&fields.tts_voice);
    }
    if !spec.formats.is_empty() {
        settings.format = present(&fields.tts_format);
    }
    settings
}

/// Write payload for the Voice form's identity-scoped SAVE SETTINGS.
///
/// The global fields are `None`. The appearance fields come from `appearance`:
/// `merge_voice_payload` leaves a `None` field untouched, so a form that does
/// not own the appearance (`appearance == None`) cannot reach
/// `identities.<slug>.appearance`.
pub(crate) fn identity_voice_payload(
    slug: String,
    fields: IdentityVoiceFields,
    appearance: Option<&OrbAppearance>,
) -> VoiceWritePayload {
    VoiceWritePayload {
        identity_slug: Some(slug),
        identity_tts_provider: fields.tts_provider,
        identity_tts_model: fields.tts_model,
        identity_tts_voice: fields.tts_voice,
        identity_tts_format: fields.tts_format,
        identity_realtime_voice: fields.realtime_voice,
        identity_realtime_model: fields.realtime_model,
        identity_appearance_style: appearance.map(|a| a.style.clone()),
        identity_base_hue: appearance.map(|a| a.base_hue),
        identity_size: appearance.map(|a| a.size),
        identity_glow: appearance.map(|a| a.glow),
        ..Default::default()
    }
}

/// Picker option text: the preset's display name, plus ` (active)` on the
/// active identity.
fn orb_option_label(display_name: &str, is_active: bool) -> String {
    if is_active {
        format!("{display_name} (active)")
    } else {
        display_name.to_string()
    }
}

/// The System screen's Orb tab: loads the saved orb appearances itself and
/// hands them to [`OrbAppearanceEditor`].
///
/// `use_resource`, not `use_server_future(...)?`: there is no early return, so
/// no hook can be skipped while the config loads.
#[component]
pub fn OrbSettingsPanel() -> Element {
    let config_resource = use_resource(move || async move { get_voice_config().await });

    // Owned snapshot before `rsx!` (Pattern B): no resource borrow in the tree.
    let load = match config_resource.read().as_ref() {
        None => OrbLoad::Loading,
        Some(Ok(snapshot)) => OrbLoad::Ready {
            saved: saved_orb_appearances(&snapshot.identity_overrides),
            write_enabled: snapshot.web_config_write_enabled,
        },
        Some(Err(_)) => OrbLoad::Failed,
    };

    rsx! {
        match load {
            OrbLoad::Loading => rsx! {
                p { class: "voice-settings-hint", "Loading orb settings…" }
            },
            OrbLoad::Failed => rsx! {
                p {
                    class: "voice-settings-hint",
                    role: "alert",
                    style: "color:var(--danger);",
                    "Orb settings could not be loaded. Check server logs."
                }
            },
            OrbLoad::Ready { saved, write_enabled } => rsx! {
                OrbAppearanceEditor { saved, write_enabled }
            },
        }
    }
}

/// Owned load state of the Orb tab's `get_voice_config` resource.
enum OrbLoad {
    Loading,
    Failed,
    Ready {
        saved: Vec<(String, OrbAppearance)>,
        write_enabled: bool,
    },
}

/// The Orb tab's editor: picks one orb, hydrates the root Orb* signals from its
/// saved look, reuses [`OrbAppearanceSection`], tracks dirty state against that
/// orb's saved baseline and saves an appearance-only payload.
///
/// Consumes HermesApp-root contexts only and provides none.
#[component]
fn OrbAppearanceEditor(saved: Vec<(String, OrbAppearance)>, write_enabled: bool) -> Element {
    let avatar_ctx = use_context::<AvatarModeCtx>();
    let mut style_sig = use_context::<OrbStyleCtx>().0;
    let mut hue_sig = use_context::<OrbBaseHueCtx>().0;
    let mut size_sig = use_context::<OrbSizeCtx>().0;
    let mut glow_sig = use_context::<OrbGlowCtx>().0;

    let mut saved_sig = use_signal(|| saved);
    let mut selected = use_signal(|| initial_orb_selection(&avatar_ctx.0.peek().active_identity));
    let mut last_hydrated = use_signal(String::new);
    let mut saved_label = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);

    // Hydrate the shared Orb* signals from the selected orb's SAVED look. Runs
    // on mount (so the knobs never show whatever another editor left behind)
    // and on a real orb switch; the guard skips a no-op re-set.
    use_effect(move || {
        let target = selected.read().clone(); // subscribe to orb switches
        if !should_rehydrate_appearance(&last_hydrated.peek(), &target) {
            return;
        }
        let entry = saved_sig
            .peek()
            .iter()
            .find(|(s, _)| *s == target)
            .map(|(_, a)| a.clone()); // peek guard dropped before any .set()
        if let Some(a) = entry {
            style_sig.set(a.style);
            hue_sig.set(a.base_hue);
            size_sig.set(a.size);
            glow_sig.set(a.glow);
        }
        last_hydrated.set(target);
    });

    // ── Render locals (Pattern B / G: `.read()` in render, owned values) ──────
    let selected_slug = selected.read().clone();
    let live = OrbAppearance {
        style: style_sig.read().clone(),
        base_hue: *hue_sig.read(),
        size: *size_sig.read(),
        glow: *glow_sig.read(),
    };
    let baseline = saved_sig
        .read()
        .iter()
        .find(|(s, _)| *s == selected_slug)
        .map(|(_, a)| a.clone())
        .or_else(|| resolve_orb_appearance(&selected_slug, None))
        .unwrap_or_else(|| live.clone());
    let dirty = orb_appearance_is_dirty(&live, &baseline);
    let active_identity = avatar_ctx.0.read().active_identity.clone();
    let baseline_for_discard = baseline.clone();
    let saved_label_val = *saved_label.read();
    let save_error_val = save_error.read().clone();

    rsx! {
        div {
            class: "voice-settings-panel",
            role: "region",
            "aria-label": "Orb appearance",

            // Orb picker: exactly the four ORB_PRESET_REGISTRY identities.
            div { class: "voice-settings-row",
                label {
                    r#for: "ih-orb-identity",
                    class: "voice-settings-label",
                    style: "color:var(--fg-dim);",
                    "Orb"
                }
                select {
                    id: "ih-orb-identity",
                    class: "voice-settings-select",
                    // No `value` on the <select>: each <option> carries `selected`,
                    // so `value="<slug>"` occurs once per option, not twice.
                    onchange: move |evt: FormEvent| {
                        let value = evt.value();
                        // Ignore anything that is not a known orb slug. A real
                        // switch re-runs the hydrate effect, which drops unsaved
                        // edits (accepted: the Voice panel has no guard either).
                        if ORB_PRESET_REGISTRY.iter().any(|p| p.id == value) {
                            save_error.set(None);
                            selected.set(value);
                        }
                    },
                    for preset in ORB_PRESET_REGISTRY {
                        option {
                            key: "{preset.id}",
                            value: "{preset.id}",
                            selected: preset.id == selected_slug,
                            "{orb_option_label(preset.display_name, preset.id == active_identity)}"
                        }
                    }
                }
            }
            p { class: "voice-settings-hint",
                "Changes apply to the selected orb. Voice mode shows the active orb."
            }

            OrbAppearanceSection { write_enabled }

            if write_enabled {
                div { class: "voice-settings-save-bar",
                    if let Some(ref err) = save_error_val {
                        p {
                            class: "voice-settings-hint",
                            style: "color:var(--danger); margin-bottom:var(--space-1);",
                            "{err}"
                        }
                    }
                    button {
                        id: "ih-orb-save",
                        class: "voice-settings-save-btn",
                        r#type: "button",
                        style: if dirty { "opacity:1; cursor:pointer;" } else { "opacity:0.5; cursor:default;" },
                        disabled: !dirty,
                        onclick: move |_| {
                            // Pattern B: owned locals BEFORE spawn; no signal
                            // borrow is held across the await.
                            let slug = selected.peek().clone();
                            let a = OrbAppearance {
                                style: style_sig.peek().clone(),
                                base_hue: *hue_sig.peek(),
                                size: *size_sig.peek(),
                                glow: *glow_sig.peek(),
                            };
                            let payload = orb_appearance_payload(&slug, &a);
                            spawn(async move {
                                match update_voice_config(payload).await {
                                    Ok(()) => {
                                        saved_sig.with_mut(|v| upsert_saved_appearance(v, &slug, a));
                                        save_error.set(None);
                                        saved_label.set(true);
                                        gloo_timers::future::TimeoutFuture::new(1500).await;
                                        saved_label.set(false);
                                    }
                                    Err(_e) => {
                                        save_error
                                            .set(Some("Save failed. Check server logs.".to_string()));
                                    }
                                }
                            });
                        },
                        if saved_label_val {
                            "Appearance saved."
                        } else {
                            "Save appearance"
                        }
                    }
                    button {
                        id: "ih-orb-discard",
                        class: "btn btn--ghost btn--sm",
                        r#type: "button",
                        disabled: !dirty,
                        onclick: move |_| {
                            // Revert the live signals to the selected orb's
                            // saved baseline (owned clone: no borrow held).
                            let a = baseline_for_discard.clone();
                            style_sig.set(a.style);
                            hue_sig.set(a.base_hue);
                            size_sig.set(a.size);
                            glow_sig.set(a.glow);
                            save_error.set(None);
                        },
                        "Discard edits"
                    }
                }
            } else {
                p { class: "voice-settings-hint", style: "color:var(--fg-dim);",
                    "Config writes are disabled. Set security.web_config_write_enabled: true in config.yaml to unlock."
                }
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod orb_settings_tests {
    use super::*;
    use crate::components::hermes_app::avatar_logic::{ORB_PRESET_REGISTRY, PRESET_REGISTRY};
    use crate::ui_prefs::AvatarPrefs;
    use std::cell::{Cell, RefCell};

    fn no_voice_override(slug: &str) -> IdentityOverride {
        IdentityOverride {
            slug: slug.to_string(),
            free_mode_provider: "edge".to_string(),
            free_mode_voice: None,
            free_mode_model: None,
            free_mode_format: None,
            realtime_voice: "marin".to_string(),
            realtime_model: String::new(),
            appearance_style: None,
            appearance_base_hue: None,
            appearance_size: None,
            appearance_glow: None,
        }
    }

    fn appearance(style: &str, hue: u16, size: f32, glow: f32) -> OrbAppearance {
        OrbAppearance {
            style: style.to_string(),
            base_hue: hue,
            size,
            glow,
        }
    }

    #[test]
    fn resolve_orb_appearance_prefers_saved_fields_over_preset_defaults() {
        let mut ov = no_voice_override("orb_bloom");
        ov.appearance_style = Some("ascii".to_string());
        ov.appearance_size = Some(1.5);
        let bloom = resolve_orb_appearance("orb_bloom", Some(&ov)).expect("orb_bloom is an orb");
        assert_eq!(bloom.style, "ascii", "saved style wins");
        assert_eq!(
            bloom.base_hue, 280,
            "unset hue falls back to the bloom preset"
        );
        assert!((bloom.size - 1.5).abs() < f32::EPSILON, "saved size wins");
        assert!(
            (bloom.glow - 0.8).abs() < f32::EPSILON,
            "unset glow falls back to the preset"
        );

        let classic = resolve_orb_appearance("orb_classic", None).expect("orb_classic is an orb");
        assert_eq!(classic, appearance("classic", 186, 1.0, 0.5));

        assert_eq!(resolve_orb_appearance(PRESET_REGISTRY[0].id, None), None);
        assert_eq!(resolve_orb_appearance("no-such-orb", None), None);
    }

    #[test]
    fn saved_orb_appearances_cover_every_orb_preset() {
        let mut ascii = no_voice_override("orb_ascii");
        ascii.appearance_base_hue = Some(42);
        let head = no_voice_override(PRESET_REGISTRY[0].id);
        let saved = saved_orb_appearances(&[ascii, head]);
        assert_eq!(saved.len(), ORB_PRESET_REGISTRY.len());
        for (entry, preset) in saved.iter().zip(ORB_PRESET_REGISTRY.iter()) {
            assert_eq!(entry.0, preset.id, "registry order");
        }
        let ascii = saved
            .iter()
            .find(|(s, _)| s == "orb_ascii")
            .expect("orb_ascii entry");
        assert_eq!(ascii.1.base_hue, 42);
        assert!(
            saved.iter().all(|(s, _)| s != PRESET_REGISTRY[0].id),
            "a head slug is never an Orb-tab entry"
        );
    }

    #[test]
    fn initial_orb_selection_prefers_the_active_orb() {
        assert_eq!(initial_orb_selection("orb_network"), "orb_network");
        assert_eq!(initial_orb_selection(PRESET_REGISTRY[0].id), "orb_classic");
        assert_eq!(initial_orb_selection(""), "orb_classic");
    }

    #[test]
    fn orb_appearance_is_dirty_compares_all_four_fields() {
        let base = appearance("classic", 186, 1.0, 0.5);
        assert!(!orb_appearance_is_dirty(&base, &base.clone()));
        assert!(orb_appearance_is_dirty(
            &appearance("bloom", 186, 1.0, 0.5),
            &base
        ));
        assert!(orb_appearance_is_dirty(
            &appearance("classic", 187, 1.0, 0.5),
            &base
        ));
        assert!(orb_appearance_is_dirty(
            &appearance("classic", 186, 1.1, 0.5),
            &base
        ));
        assert!(orb_appearance_is_dirty(
            &appearance("classic", 186, 1.0, 0.55),
            &base
        ));
        let tiny = f32::EPSILON / 2.0;
        assert!(!orb_appearance_is_dirty(
            &appearance("classic", 186, 1.0 + tiny, 0.5 + tiny),
            &base
        ));
    }

    #[test]
    fn orb_appearance_payload_writes_only_one_identity_appearance() {
        let a = appearance("bloom", 77, 1.4, 0.65);
        let expected = VoiceWritePayload {
            identity_slug: Some("orb_bloom".to_string()),
            identity_appearance_style: Some("bloom".to_string()),
            identity_base_hue: Some(77),
            identity_size: Some(1.4),
            identity_glow: Some(0.65),
            ..Default::default()
        };
        assert_eq!(orb_appearance_payload("orb_bloom", &a), expected);
    }

    #[test]
    fn upsert_saved_appearance_replaces_only_that_orb() {
        let mut saved = saved_orb_appearances(&[]);
        assert_eq!(
            saved.len(),
            ORB_PRESET_REGISTRY.len(),
            "precondition: one entry per orb"
        );
        let before = saved.clone();
        let new = appearance("network", 10, 0.7, 0.1);
        upsert_saved_appearance(&mut saved, "orb_bloom", new.clone());
        assert_eq!(saved.len(), before.len(), "length unchanged");
        assert!(
            saved.iter().any(|(s, a)| s == "orb_bloom" && *a == new),
            "the upserted orb carries the new value"
        );
        for (after, was) in saved.iter().zip(before.iter()) {
            if after.0 == "orb_bloom" {
                assert_eq!(after.1, new);
            } else {
                assert_eq!(after, was, "other orbs untouched");
            }
        }
    }

    thread_local! {
        static TEST_ACTIVE: RefCell<String> = const { RefCell::new(String::new()) };
        static TEST_WRITE: Cell<bool> = const { Cell::new(true) };
    }

    /// SSR the real `OrbAppearanceEditor` under the root-shaped contexts only.
    fn render_editor(active: &str, write_enabled: bool) -> String {
        TEST_ACTIVE.with(|a| *a.borrow_mut() = active.to_string());
        TEST_WRITE.with(|w| w.set(write_enabled));

        fn test_app() -> Element {
            let active = TEST_ACTIVE.with(|a| a.borrow().clone());
            let preset = ORB_PRESET_REGISTRY
                .iter()
                .find(|p| p.id == active)
                .unwrap_or(&ORB_PRESET_REGISTRY[0]);
            let style = use_signal(|| preset.default_style.to_string());
            let hue = use_signal(|| preset.default_hue);
            let size = use_signal(|| preset.default_size);
            let glow = use_signal(|| preset.default_glow);
            let settling = use_signal(|| false);
            let prefs = use_signal(|| AvatarPrefs {
                active_identity: active.clone(),
                ..Default::default()
            });
            use_context_provider(|| OrbStyleCtx(style));
            use_context_provider(|| OrbBaseHueCtx(hue));
            use_context_provider(|| OrbSizeCtx(size));
            use_context_provider(|| OrbGlowCtx(glow));
            use_context_provider(|| OrbSettlingCtx(settling));
            use_context_provider(|| AvatarModeCtx(prefs));
            rsx! {
                OrbAppearanceEditor {
                    saved: saved_orb_appearances(&[]),
                    write_enabled: TEST_WRITE.with(|w| w.get()),
                }
            }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// The opening `<tag ...>` that holds `needle` (an attribute fragment).
    fn opening_tag_holding<'a>(html: &'a str, tag: &str, needle: &str) -> &'a str {
        let at = html
            .find(needle)
            .unwrap_or_else(|| panic!("`{needle}` not in html: {html}"));
        let open = format!("<{tag}");
        let start = html[..at]
            .rfind(&open)
            .unwrap_or_else(|| panic!("no `{open}` before `{needle}`"));
        let end = at + html[at..].find('>').expect("tag closes");
        &html[start..=end]
    }

    fn assert_true_disabled(tag: &str) {
        assert!(
            tag.contains("disabled"),
            "expected a disabled button: {tag}"
        );
        assert!(
            !tag.contains("disabled=false"),
            "SSR printed `disabled=false`, which is not a true disabled: {tag}"
        );
    }

    #[test]
    fn orb_editor_renders_the_colour_control_with_a_clean_save() {
        let html = render_editor("orb_classic", true);
        assert!(
            html.contains("id=\"orb-color-knob\""),
            "colour knob missing: {html}"
        );
        assert!(html.contains("Color (base hue)"));
        assert!(
            html.contains("id=\"ih-orb-save\""),
            "save button missing: {html}"
        );
        assert!(html.contains("Save appearance"));
        let tag = opening_tag_holding(&html, "button", "id=\"ih-orb-save\"");
        assert_true_disabled(tag);
    }

    #[test]
    fn orb_editor_is_locked_without_config_writes() {
        let html = render_editor("orb_classic", false);
        assert!(html.contains("Config writes are disabled."), "{html}");
        assert!(html.contains("Appearance editing is disabled."), "{html}");
        assert!(
            !html.contains("id=\"ih-orb-save\""),
            "no Save while locked: {html}"
        );
    }

    fn assert_true_selected(tag: &str) {
        assert!(
            tag.contains("selected"),
            "expected a selected option: {tag}"
        );
        assert!(
            !tag.contains("selected=false"),
            "SSR printed `selected=false`, which is not a true selected: {tag}"
        );
    }

    #[test]
    fn orb_editor_lists_every_orb_and_marks_the_active_one() {
        let html = render_editor("orb_bloom", true);
        assert!(
            html.contains("id=\"ih-orb-identity\""),
            "picker missing: {html}"
        );
        let label = opening_tag_holding(&html, "label", "for=\"ih-orb-identity\"");
        assert!(label.starts_with("<label"), "{label}");
        for preset in ORB_PRESET_REGISTRY {
            let needle = format!("value=\"{}\"", preset.id);
            assert_eq!(html.matches(&needle).count(), 1, "{needle} once: {html}");
        }
        assert_eq!(html.matches("(active)").count(), 1, "{html}");
        let bloom_tag = opening_tag_holding(&html, "option", "value=\"orb_bloom\"");
        let bloom_at = html.find(bloom_tag).expect("bloom option present");
        let close_at = bloom_at + html[bloom_at..].find("</option>").expect("option closes");
        assert!(
            html[bloom_at..close_at].contains("(active)"),
            "(active) belongs to the bloom option: {html}"
        );
        assert_true_selected(bloom_tag);
        let classic_tag = opening_tag_holding(&html, "option", "value=\"orb_classic\"");
        assert!(
            !classic_tag.contains("selected") || classic_tag.contains("selected=false"),
            "classic must not be selected: {classic_tag}"
        );
        for head in PRESET_REGISTRY {
            let needle = format!("value=\"{}\"", head.id);
            assert!(
                !html.contains(&needle),
                "head avatar {needle} offered: {html}"
            );
        }
    }

    #[test]
    fn orb_editor_offers_discard_edits() {
        let html = render_editor("orb_classic", true);
        assert!(
            html.contains("id=\"ih-orb-discard\""),
            "discard missing: {html}"
        );
        assert!(html.contains("Discard edits"));
        assert_true_disabled(opening_tag_holding(
            &html,
            "button",
            "id=\"ih-orb-discard\"",
        ));
    }

    const VOICE_SETTINGS_RS: &str = include_str!("voice_settings.rs");

    #[test]
    fn voice_settings_hydrate_uses_resolve_orb_appearance() {
        let start_needle = ["pub fn Voice", "Settings("].concat();
        let end_needle = ["#[comp", "onent]"].concat();
        let start = VOICE_SETTINGS_RS
            .find(&start_needle)
            .expect("VoiceSettings signature present");
        let rest = &VOICE_SETTINGS_RS[start + start_needle.len()..];
        let end = rest
            .find(&end_needle)
            .expect("a component follows VoiceSettings");
        let region = &VOICE_SETTINGS_RS[start..start + start_needle.len() + end];
        let call = ["resolve_orb_", "appearance("].concat();
        assert_eq!(
            region.matches(&call).count(),
            1,
            "VoiceSettings' identity-switch effect must resolve through the shared fn"
        );
    }
}

/// Phase 54.4 Plan 17 (G-54.4-34): the Voice form's orb-appearance opt-out.
/// Needles in the source-text tests are assembled from fragments so a test can
/// never match its own source.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod voice_form_orb_gate_tests {
    use super::*;
    use crate::components::hermes_app::avatar_logic::{ORB_PRESET_REGISTRY, PRESET_REGISTRY};
    use crate::ui_prefs::AvatarPrefs;
    use std::cell::Cell;

    #[test]
    fn voice_form_owns_orb_appearance_requires_the_prop_and_an_orb_target() {
        for preset in ORB_PRESET_REGISTRY {
            assert!(
                voice_form_owns_orb_appearance(true, preset.id),
                "prop on + orb target {} owns the appearance",
                preset.id
            );
            assert!(
                !voice_form_owns_orb_appearance(false, preset.id),
                "prop off + orb target {} must not own the appearance",
                preset.id
            );
        }
        assert!(!voice_form_owns_orb_appearance(true, "global"));
        assert!(!voice_form_owns_orb_appearance(true, ""));
        assert!(!voice_form_owns_orb_appearance(true, PRESET_REGISTRY[0].id));
    }

    thread_local! {
        /// 0 = prop omitted (default), 1 = `show_orb_appearance: true`,
        /// 2 = `show_orb_appearance: false`.
        static TEST_SHOW: Cell<u8> = const { Cell::new(0) };
        /// The staged voice service the shell seeds `VoiceServiceCtx` with.
        static TEST_SERVICE: std::cell::RefCell<String> =
            const { std::cell::RefCell::new(String::new()) };
    }

    /// SSR `VoiceSettings` with the staged service seeded as `service`.
    fn render_voice_settings_with_service(service: &str) -> String {
        TEST_SERVICE.with(|s| *s.borrow_mut() = service.to_string());
        let html = render_voice_settings(None);
        TEST_SERVICE.with(|s| s.borrow_mut().clear());
        html
    }

    /// The opening `<div ...>` of the family group `data-family="{family}"`.
    fn family_tag<'a>(html: &'a str, family: &str) -> &'a str {
        let marker = format!("data-family=\"{family}\"");
        let at = html
            .find(&marker)
            .unwrap_or_else(|| panic!("family group {family} must always be mounted: {html}"));
        let start = html[..at].rfind("<div").expect("div opens");
        let end = at + html[at..].find('>').expect("tag closes");
        &html[start..=end]
    }

    /// D-14: both family groups are ALWAYS mounted; only the non-selected one
    /// carries `hidden` (never an `if service ==` branch).
    #[test]
    fn families_both_mounted_hidden_toggles() {
        for (service, shown, hidden) in [
            ("realtime", "realtime", "turn_based"),
            ("turn_based", "turn_based", "realtime"),
        ] {
            let html = render_voice_settings_with_service(service);
            let shown_tag = family_tag(&html, shown);
            let hidden_tag = family_tag(&html, hidden);
            assert!(
                !shown_tag.contains(" hidden"),
                "service {service}: {shown} must be visible: {shown_tag}"
            );
            assert!(
                hidden_tag.contains(" hidden"),
                "service {service}: {hidden} must carry `hidden`: {hidden_tag}"
            );
            assert!(
                !hidden_tag.contains("hidden=false"),
                "SSR printed `hidden=false`, which is not a true hidden: {hidden_tag}"
            );
            assert!(shown_tag.contains("voice-family"), "{shown_tag}");
        }
    }

    /// The old top-of-panel VOICE block with the two status rows is gone; the
    /// pills now live in the STT / TTS family headings.
    #[test]
    fn top_voice_status_block_is_gone() {
        let html = render_voice_settings(None);
        assert!(!html.contains("STT Status"), "{html}");
        assert!(!html.contains("TTS Status"), "{html}");
        assert!(!html.contains(">VOICE MODE<"), "{html}");
        assert!(html.contains(">VOICE SERVICE<"), "{html}");
    }

    /// SSR the real `VoiceSettings` under the root-shaped contexts only.
    /// `None` omits the prop so the default applies.
    fn render_voice_settings(show: Option<bool>) -> String {
        TEST_SHOW.with(|s| {
            s.set(match show {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            })
        });

        fn test_app() -> Element {
            let voice_status = use_signal(VoiceStatusState::default);
            let wake_enabled = use_signal(|| false);
            let wake_phrase = use_signal(|| "hey hermes".to_string());
            let prefs = use_signal(|| AvatarPrefs {
                active_identity: "orb_classic".into(),
                ..Default::default()
            });
            let edit_target = use_signal(|| "orb_classic".to_string());
            let id_provider: Signal<Option<String>> = use_signal(|| None);
            let id_voice: Signal<Option<String>> = use_signal(|| None);
            let id_rt_voice: Signal<Option<String>> = use_signal(|| None);
            let id_model: Signal<Option<String>> = use_signal(|| None);
            let id_format: Signal<Option<String>> = use_signal(|| None);
            let id_rt_model: Signal<Option<String>> = use_signal(|| None);
            let style = use_signal(|| "classic".to_string());
            let hue = use_signal(|| 186u16);
            let size = use_signal(|| 1.0f32);
            let glow = use_signal(|| 0.5f32);
            let settling = use_signal(|| false);
            let service = use_signal(|| {
                let staged = TEST_SERVICE.with(|s| s.borrow().clone());
                if staged.is_empty() {
                    "turn_based".to_string()
                } else {
                    staged
                }
            });
            let service_loaded = use_signal(|| true);
            let degraded = use_signal(|| false);
            let beep = use_signal(|| true);
            let playing = use_signal(|| false);
            use_context_provider(|| voice_status);
            use_context_provider(|| WakeWordEnabledCtx(wake_enabled));
            use_context_provider(|| WakeWordPhraseCtx(wake_phrase));
            use_context_provider(|| AvatarModeCtx(prefs));
            use_context_provider(|| VoiceEditTargetCtx(edit_target));
            use_context_provider(|| IdentityTtsProviderCtx(id_provider));
            use_context_provider(|| IdentityTtsVoiceCtx(id_voice));
            use_context_provider(|| IdentityRealtimeVoiceCtx(id_rt_voice));
            use_context_provider(|| IdentityTtsModelCtx(id_model));
            use_context_provider(|| IdentityTtsFormatCtx(id_format));
            use_context_provider(|| IdentityRealtimeModelCtx(id_rt_model));
            use_context_provider(|| OrbStyleCtx(style));
            use_context_provider(|| OrbBaseHueCtx(hue));
            use_context_provider(|| OrbSizeCtx(size));
            use_context_provider(|| OrbGlowCtx(glow));
            use_context_provider(|| OrbSettlingCtx(settling));
            use_context_provider(|| VoiceServiceCtx(service));
            use_context_provider(|| VoiceServiceLoadedCtx(service_loaded));
            use_context_provider(|| RealtimeDegradedCtx(degraded));
            use_context_provider(|| BeepEnabledCtx(beep));
            use_context_provider(|| AudioPlaybackActiveCtx(playing));
            match TEST_SHOW.with(|s| s.get()) {
                0 => rsx! { VoiceSettings { on_close: move |_| {} } },
                1 => rsx! { VoiceSettings { on_close: move |_| {}, show_orb_appearance: true } },
                _ => rsx! { VoiceSettings { on_close: move |_| {}, show_orb_appearance: false } },
            }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// Phase 54.7 Plan 02 (D-01, Pitfall 13): the panel's provider selects are
    /// rendered from `voice_registry`, so OpenAI and OpenAI-compatible are
    /// selectable for TTS and OpenAI-compatible for STT.
    #[test]
    fn panel_provider_selects_render_from_registry() {
        let html = render_voice_settings(None);
        let select_body = |id: &str| -> String {
            let marker = format!("id=\"{id}\"");
            let start = html
                .find(&marker)
                .unwrap_or_else(|| panic!("select {id} must render: {html}"));
            let rest = &html[start..];
            let end = rest.find("</select>").expect("select must close");
            rest[..end].to_string()
        };
        let tts = select_body("tts-provider-select");
        for needle in [
            "value=\"edge\"",
            "value=\"elevenlabs\"",
            "value=\"openai\"",
            "value=\"openai_compatible\"",
            ">OpenAI-compatible<",
        ] {
            assert!(tts.contains(needle), "TTS select missing {needle}: {tts}");
        }
        let stt = select_body("stt-provider-select");
        for needle in [
            "value=\"auto\"",
            "value=\"groq\"",
            "value=\"openai\"",
            "value=\"openai_compatible\"",
            ">OpenAI-compatible<",
        ] {
            assert!(stt.contains(needle), "STT select missing {needle}: {stt}");
        }
    }

    /// D-06: no inline provider/voice option literal survives in this file.
    /// Needles are assembled from fragments so the test cannot match itself.
    #[test]
    fn voice_settings_has_no_inline_provider_option_literals() {
        let src = include_str!("voice_settings.rs");
        let needles = [
            ["option { value: \"", "groq", "\""].concat(),
            ["option { value: \"", "elevenlabs", "\""].concat(),
            ["option { value: \"", "shimmer", "\""].concat(),
            ["option { value: \"", "ballad", "\""].concat(),
            ["\"edge\"", "    => Some("].concat(),
        ];
        for needle in &needles {
            assert!(
                !src.contains(needle.as_str()),
                "voice_settings.rs still holds an inline option literal: {needle}"
            );
        }
    }

    /// D-06 (hard-coded defaults): every default model/voice comes from the
    /// registry. The needle list is shared with the later panel files' tests.
    #[test]
    fn voice_settings_has_no_hardcoded_voice_defaults() {
        let src = include_str!("voice_settings.rs");
        for needle in crate::voice_registry::hardcoded_voice_default_needles() {
            assert!(
                !src.contains(needle.as_str()),
                "voice_settings.rs still holds a hard-coded voice default: {needle}"
            );
        }
    }

    #[test]
    fn general_voice_form_renders_no_orb_appearance_section() {
        let html = render_voice_settings(Some(false));
        assert!(
            html.contains("aria-label=\"Voice settings\""),
            "positive control: the Voice form itself must render: {html}"
        );
        assert!(
            !html.contains("id=\"orb-color-knob\""),
            "the General Voice form must not render the colour knob: {html}"
        );
        assert!(
            !html.contains("Color (base hue)"),
            "the General Voice form must not render the colour label: {html}"
        );
    }

    /// Regression guard: the overlay mount omits the prop, so the default
    /// (true) must keep the Appearance section. Passes at RED by design.
    #[test]
    fn overlay_voice_form_keeps_the_orb_appearance_section() {
        let html = render_voice_settings(None);
        assert!(
            html.contains("aria-label=\"Voice settings\""),
            "positive control: the Voice form itself must render: {html}"
        );
        assert!(
            html.contains("id=\"orb-color-knob\""),
            "the overlay Voice form keeps the colour knob: {html}"
        );
        assert!(
            html.contains("Color (base hue)"),
            "the overlay Voice form keeps the colour label: {html}"
        );
    }

    #[test]
    fn voice_form_never_hydrates_a_hidden_orb_section() {
        // A form that hides the section never re-derives (never writes the
        // shared Orb* signals).
        assert!(!voice_form_should_hydrate_appearance(
            false,
            true,
            "",
            "orb_bloom"
        ));
        assert!(!voice_form_should_hydrate_appearance(
            false,
            true,
            "orb_classic",
            "orb_bloom"
        ));
        // The G-41.2-11 not-loaded guard is kept.
        assert!(!voice_form_should_hydrate_appearance(
            true,
            false,
            "",
            "orb_bloom"
        ));
        // A real switch on a loaded, section-showing form re-derives.
        assert!(voice_form_should_hydrate_appearance(
            true,
            true,
            "",
            "orb_bloom"
        ));
        assert!(voice_form_should_hydrate_appearance(
            true,
            true,
            "orb_classic",
            "orb_bloom"
        ));
        // The 41.2 reset-fight guard is kept.
        assert!(!voice_form_should_hydrate_appearance(
            true,
            true,
            "orb_bloom",
            "orb_bloom"
        ));
    }

    fn appearance(style: &str, hue: u16, size: f32, glow: f32) -> OrbAppearance {
        OrbAppearance {
            style: style.to_string(),
            base_hue: hue,
            size,
            glow,
        }
    }

    #[test]
    fn voice_form_orb_dirty_requires_ownership() {
        let loaded = appearance("classic", 186, 1.0, 0.5);
        let live = appearance("bloom", 280, 1.2, 0.8);
        assert!(!voice_form_orb_dirty(false, &live, &loaded));
        assert!(voice_form_orb_dirty(true, &live, &loaded));
        assert!(!voice_form_orb_dirty(true, &loaded, &loaded));
    }

    /// The identity save names only its identity fields; every other payload
    /// field stays at its `Default` (None), which the merge leaves untouched.
    fn expected_identity_payload(appearance: Option<(&str, u16, f32, f32)>) -> VoiceWritePayload {
        VoiceWritePayload {
            identity_slug: Some("orb_bloom".to_string()),
            identity_tts_provider: Some("openai".to_string()),
            identity_tts_voice: Some("nova".to_string()),
            identity_appearance_style: appearance.map(|a| a.0.to_string()),
            identity_base_hue: appearance.map(|a| a.1),
            identity_size: appearance.map(|a| a.2),
            identity_glow: appearance.map(|a| a.3),
            ..Default::default()
        }
    }

    #[test]
    fn identity_voice_payload_without_appearance_sends_no_appearance_field() {
        let payload = identity_voice_payload(
            "orb_bloom".into(),
            IdentityVoiceFields {
                tts_provider: Some("openai".into()),
                tts_voice: Some("nova".into()),
                ..Default::default()
            },
            None,
        );
        assert_eq!(payload, expected_identity_payload(None));
    }

    #[test]
    fn identity_voice_payload_with_appearance_sends_all_four_fields() {
        let a = appearance("ascii", 42, 1.5, 0.3);
        let payload = identity_voice_payload(
            "orb_bloom".into(),
            IdentityVoiceFields {
                tts_provider: Some("openai".into()),
                tts_voice: Some("nova".into()),
                ..Default::default()
            },
            Some(&a),
        );
        assert_eq!(
            payload,
            expected_identity_payload(Some(("ascii", 42, 1.5, 0.3)))
        );
    }

    const VOICE_SETTINGS_RS: &str = include_str!("voice_settings.rs");

    /// The text of `VoiceSettings` only: from its signature to the next
    /// `#[component]`.
    fn voice_settings_region() -> &'static str {
        let start_needle = ["pub fn Voice", "Settings("].concat();
        let end_needle = ["#[comp", "onent]"].concat();
        let start = VOICE_SETTINGS_RS
            .find(&start_needle)
            .expect("VoiceSettings signature present");
        let rest = &VOICE_SETTINGS_RS[start + start_needle.len()..];
        let end = rest
            .find(&end_needle)
            .expect("a component follows VoiceSettings");
        &VOICE_SETTINGS_RS[start..start + start_needle.len() + end]
    }

    #[test]
    fn voice_settings_routes_orb_state_through_the_ownership_gates() {
        let region = voice_settings_region();
        let once = [
            ["voice_form_owns_orb_", "appearance("].concat(),
            ["voice_form_should_hydrate_", "appearance("].concat(),
            ["voice_form_orb_", "dirty("].concat(),
            ["identity_voice_", "payload("].concat(),
            ["OrbAppearance", "Section {"].concat(),
        ];
        for needle in &once {
            assert_eq!(
                region.matches(needle.as_str()).count(),
                1,
                "VoiceSettings must contain `{needle}` exactly once"
            );
        }
        let direct = ["should_rehydrate_", "appearance("].concat();
        assert_eq!(
            region.matches(direct.as_str()).count(),
            0,
            "the hydrate effect reaches the 41.2 guard only through the gate"
        );
    }
}

// ── Phase 54.7 Plan 15 (D-16) — identity overrides ──────────────────────────
//
// Declared after `voice_form_orb_gate_tests` on purpose (see the note above
// `voice_service_tests`): the root-provider source-text test cuts this file at
// that module's name, so these shells stay out of the scanned production text.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod identity_voice_tests_547_15 {
    use super::*;

    const VOICE_SETTINGS_SRC_15: &str = include_str!("voice_settings.rs");

    fn strings(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    /// Everything a section render varies.
    #[derive(Clone, PartialEq)]
    struct SectionCase {
        provider: String,
        model: Option<String>,
        format: Option<String>,
        usable: Vec<String>,
        compat_base_url: String,
        service: String,
    }

    impl SectionCase {
        /// A turn-based server with every key-based provider usable.
        fn on(provider: &str) -> Self {
            Self {
                provider: provider.to_string(),
                model: None,
                format: None,
                usable: strings(&["edge", "elevenlabs", "openai"]),
                compat_base_url: String::new(),
                service: "turn_based".to_string(),
            }
        }
    }

    fn render_section(case: SectionCase) -> String {
        fn app(case: SectionCase) -> Element {
            let edit_target = use_signal(|| "orb_classic".to_string());
            let id_provider: Signal<Option<String>> = use_signal(|| None);
            let id_voice: Signal<Option<String>> = use_signal(|| None);
            let id_rt_voice: Signal<Option<String>> = use_signal(|| None);
            let id_model: Signal<Option<String>> = use_signal(|| None);
            let id_format: Signal<Option<String>> = use_signal(|| None);
            let id_rt_model: Signal<Option<String>> = use_signal(|| None);
            let service = use_signal(|| case.service.clone());
            let playing = use_signal(|| false);
            use_context_provider(|| VoiceEditTargetCtx(edit_target));
            use_context_provider(|| IdentityTtsProviderCtx(id_provider));
            use_context_provider(|| IdentityTtsVoiceCtx(id_voice));
            use_context_provider(|| IdentityRealtimeVoiceCtx(id_rt_voice));
            use_context_provider(|| IdentityTtsModelCtx(id_model));
            use_context_provider(|| IdentityTtsFormatCtx(id_format));
            use_context_provider(|| IdentityRealtimeModelCtx(id_rt_model));
            use_context_provider(|| VoiceServiceCtx(service));
            use_context_provider(|| AudioPlaybackActiveCtx(playing));
            rsx! {
                IdentityVoiceSection {
                    resolved_provider: case.provider.clone(),
                    resolved_voice: None,
                    resolved_model: case.model.clone(),
                    resolved_format: case.format.clone(),
                    resolved_realtime_voice: REALTIME_PROVIDERS[0].default_voice.to_string(),
                    resolved_realtime_model: REALTIME_PROVIDERS[0].default_model.to_string(),
                    write_enabled: true,
                    usable_providers: case.usable.clone(),
                    compat_base_url: case.compat_base_url.clone(),
                    compat_path: String::new(),
                    compat_key_ref: String::new(),
                }
            }
        }
        let mut dom = VirtualDom::new_with_props(app, case);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// The turn-based group's slice of the section HTML (the groups are
    /// adjacent siblings, so a group is the text between the two headings).
    fn group<'a>(html: &'a str, heading: &str) -> &'a str {
        let own = html
            .find(heading)
            .unwrap_or_else(|| panic!("`{heading}` must render: {html}"));
        let other = ["Voice (Turn-based)", "Voice (Realtime)"]
            .into_iter()
            .filter(|h| *h != heading)
            .find_map(|h| html.find(h).filter(|at| *at > own));
        &html[own..other.unwrap_or(html.len())]
    }

    fn turn_based_group(html: &str) -> &str {
        group(html, "Voice (Turn-based)")
    }

    fn realtime_group(html: &str) -> &str {
        group(html, "Voice (Realtime)")
    }

    /// D-16: the renamed `Voice (Turn-based)` group edits provider, model,
    /// voice and format and carries Test voice; an unset field wears the
    /// `(from Global)` badge.
    #[test]
    fn identity_section_renders_turn_based_group() {
        let html = render_section(SectionCase::on("openai"));
        assert!(
            html.contains("Voice (Turn-based)"),
            "heading renamed from Voice (Free Mode): {html}"
        );
        assert!(
            !html.contains("Voice (Free Mode)"),
            "the old heading must be gone: {html}"
        );
        let group = turn_based_group(&html);
        for label in [">Provider<", ">Model<", ">Voice<", ">Format<"] {
            assert!(group.contains(label), "{label} row must render: {group}");
        }
        assert!(
            group.contains("Test voice"),
            "the Test voice button belongs to the group: {group}"
        );
        // Provider, Model and Format inherit the Global value; Voice has no
        // value anywhere yet, so it reads `(not set)`.
        assert_eq!(
            group.matches("(from Global)").count(),
            3,
            "provider, model and format inherit while unset: {group}"
        );
        assert_eq!(
            group.matches("(not set)").count(),
            1,
            "an unset voice reads (not set): {group}"
        );
    }

    /// Edge has no model setting, so no Model row renders for it.
    #[test]
    fn identity_section_hides_model_row_for_edge() {
        let html = render_section(SectionCase::on("edge"));
        let group = turn_based_group(&html);
        assert!(!group.contains(">Model<"), "{group}");
        assert!(group.contains(">Voice<"), "{group}");
    }

    /// The saved identity model and format show as the section's values.
    #[test]
    fn identity_section_shows_saved_model_and_format() {
        let html = render_section(SectionCase {
            model: Some("tts-1-hd".to_string()),
            format: Some("opus".to_string()),
            ..SectionCase::on("openai")
        });
        assert!(html.contains("tts-1-hd"), "{html}");
        let group = turn_based_group(&html);
        let at = group
            .find("<option value=\"opus\"")
            .unwrap_or_else(|| panic!("the opus option renders: {group}"));
        let tag_end = at + group[at..].find('>').expect("tag closes");
        assert!(
            group[at..tag_end].contains("selected"),
            "the saved format is the selected option: {group}"
        );
    }

    /// D-16: the identity payload names exactly its identity fields plus
    /// `identity_slug`; every global field stays None.
    #[test]
    fn identity_payload_builder_carries_new_fields() {
        let fields = IdentityVoiceFields {
            tts_provider: Some("openai".into()),
            tts_model: Some("tts-1-hd".into()),
            tts_voice: Some("nova".into()),
            tts_format: Some("opus".into()),
            realtime_voice: Some("marin".into()),
            realtime_model: Some("gpt-realtime-2".into()),
        };
        let payload = identity_voice_payload("orb_bloom".into(), fields, None);
        assert_eq!(
            payload,
            VoiceWritePayload {
                identity_slug: Some("orb_bloom".into()),
                identity_tts_provider: Some("openai".into()),
                identity_tts_model: Some("tts-1-hd".into()),
                identity_tts_voice: Some("nova".into()),
                identity_tts_format: Some("opus".into()),
                identity_realtime_voice: Some("marin".into()),
                identity_realtime_model: Some("gpt-realtime-2".into()),
                ..Default::default()
            }
        );
    }

    /// The Test voice request carries only the fields the provider uses and
    /// never a blank one.
    #[test]
    fn identity_test_settings_follow_the_provider() {
        let fields = IdentityVoiceFields {
            tts_model: Some("tts-1-hd".into()),
            tts_voice: Some("nova".into()),
            tts_format: Some("opus".into()),
            ..Default::default()
        };
        let openai = identity_test_settings("openai", &fields);
        assert_eq!(openai.provider.as_deref(), Some("openai"));
        assert_eq!(openai.model.as_deref(), Some("tts-1-hd"));
        assert_eq!(openai.voice.as_deref(), Some("nova"));
        assert_eq!(openai.format.as_deref(), Some("opus"));
        assert_eq!(
            openai.base_url, None,
            "an identity never carries an endpoint"
        );

        let edge = identity_test_settings("edge", &fields);
        assert_eq!(edge.model, None, "Edge has no model setting");
        assert_eq!(edge.voice.as_deref(), Some("nova"));

        let blank = identity_test_settings("openai", &IdentityVoiceFields::default());
        assert_eq!((blank.model, blank.voice, blank.format), (None, None, None));
    }

    /// A typed value stages only when it differs from what the identity already
    /// resolves to; blank means inherit.
    #[test]
    fn staging_rule_blank_and_unchanged_stage_nothing() {
        assert_eq!(staged_from_text("", Some("tts-1")), None);
        assert_eq!(staged_from_text("   ", None), None);
        assert_eq!(staged_from_text("tts-1", Some("tts-1")), None);
        assert_eq!(
            staged_from_text(" tts-1-hd ", Some("tts-1")),
            Some("tts-1-hd".to_string())
        );
        // After a provider switch the old provider's value is not inherited.
        assert_eq!(staged_from_text("tts-1", None), Some("tts-1".to_string()));
    }

    /// E13 error: staged values the shared rules reject block the save.
    #[test]
    fn staged_identity_values_use_the_shared_rules() {
        let ok = IdentityVoiceFields {
            tts_model: Some("tts-1-hd".into()),
            tts_voice: Some("af_heart".into()),
            realtime_voice: Some("marin".into()),
            realtime_model: Some("gpt-realtime-2.1".into()),
            ..Default::default()
        };
        assert_eq!(ok.blocking_error_count(), 0);
        assert!(ok.is_staged());
        let bad = IdentityVoiceFields {
            tts_model: Some("a\"b".into()),
            tts_voice: Some("a/b".into()),
            ..Default::default()
        };
        assert_eq!(bad.blocking_error_count(), 2);
        assert!(!IdentityVoiceFields::default().is_staged());
    }

    /// A successful save is folded into the cached snapshot override, so the
    /// section shows what was saved without a refetch.
    #[test]
    fn saved_fields_fold_into_the_snapshot_override() {
        let mut ov = IdentityOverride {
            slug: "orb_bloom".into(),
            free_mode_provider: "edge".into(),
            free_mode_voice: None,
            free_mode_model: None,
            free_mode_format: None,
            realtime_voice: "marin".into(),
            realtime_model: "gpt-realtime".into(),
            appearance_style: None,
            appearance_base_hue: None,
            appearance_size: None,
            appearance_glow: None,
        };
        IdentityVoiceFields {
            tts_provider: Some("openai".into()),
            tts_model: Some("tts-1-hd".into()),
            tts_format: Some("opus".into()),
            ..Default::default()
        }
        .apply_to_override(&mut ov);
        assert_eq!(ov.free_mode_provider, "openai");
        assert_eq!(ov.free_mode_model.as_deref(), Some("tts-1-hd"));
        assert_eq!(ov.free_mode_format.as_deref(), Some("opus"));
        assert_eq!(ov.free_mode_voice, None, "an unstaged field is left alone");
        assert_eq!(ov.realtime_model, "gpt-realtime");
    }

    /// Dioxus root-provider rule: the three identity contexts are provided by
    /// HermesApp (mod.rs) and only consumed here (production text only).
    #[test]
    fn identity_ctx_provided_only_at_root() {
        let mod_rs = include_str!("mod.rs");
        let cut = ["mod voice_form_orb_", "gate_tests"].concat();
        let at = VOICE_SETTINGS_SRC_15
            .find(&cut)
            .expect("test module marker present");
        let settings = &VOICE_SETTINGS_SRC_15[..at];
        for ctx in [
            "IdentityTtsModelCtx",
            "IdentityTtsFormatCtx",
            "IdentityRealtimeModelCtx",
        ] {
            let at_root = format!("use_context_provider(|| voice_settings::{ctx}(");
            assert!(
                mod_rs.contains(&at_root),
                "HermesApp root must provide {ctx}"
            );
            let bare = format!("use_context_provider(|| {ctx}(");
            assert!(
                !settings.contains(&bare),
                "voice_settings.rs must consume {ctx}, never provide it"
            );
        }
    }

    // ── Task 3: provider filter, switch reset, realtime group, ordering ──────

    fn ids(options: &[&ProviderSpec]) -> Vec<&'static str> {
        options.iter().map(|spec| spec.id).collect()
    }

    /// D-16: the identity Provider select lists only providers the server has
    /// configured -- Edge always, the key-based ones when usable, and
    /// OpenAI-compatible only when a Base URL is saved.
    #[test]
    fn identity_provider_options_are_configured_only() {
        let options = identity_provider_options(&strings(&["edge", "openai"]), false);
        assert_eq!(ids(&options), ["edge", "openai"]);

        let with_compat = identity_provider_options(
            &strings(&["edge", "elevenlabs", "openai", "openai_compatible"]),
            true,
        );
        assert_eq!(
            ids(&with_compat),
            ["edge", "elevenlabs", "openai", "openai_compatible"]
        );

        // No saved Base URL: OpenAI-compatible is never offered, even if a
        // stale snapshot lists it.
        let no_url = identity_provider_options(&strings(&["edge", "openai_compatible"]), false);
        assert_eq!(ids(&no_url), ["edge"]);

        // Edge needs no key: present even when the snapshot lists nothing.
        let none = identity_provider_options(&[], false);
        assert_eq!(ids(&none), ["edge"]);
    }

    /// The rendered Provider select follows the pure rule, and an identity
    /// already on an unconfigured provider keeps that value visible instead of
    /// silently showing another provider.
    #[test]
    fn identity_provider_select_lists_only_configured() {
        let html = render_section(SectionCase {
            usable: strings(&["edge", "openai"]),
            ..SectionCase::on("edge")
        });
        let group = turn_based_group(&html);
        let select_end = group.find("</select>").expect("provider select closes");
        let select = &group[..select_end];
        assert!(select.contains("<option value=\"edge\""), "{select}");
        assert!(select.contains("<option value=\"openai\""), "{select}");
        assert!(!select.contains("value=\"elevenlabs\""), "{select}");
        assert!(!select.contains("value=\"openai_compatible\""), "{select}");

        // The seeded personas default to ElevenLabs; with no key it is not
        // configured, but the identity's current value must stay visible.
        let stale = render_section(SectionCase {
            usable: strings(&["edge"]),
            ..SectionCase::on("elevenlabs")
        });
        let stale_group = turn_based_group(&stale);
        let stale_select = &stale_group[..stale_group.find("</select>").unwrap()];
        assert!(
            stale_select.contains("<option value=\"elevenlabs\""),
            "the current provider stays visible: {stale_select}"
        );
        assert!(stale_select.contains("not configured"), "{stale_select}");

        // A saved Base URL makes OpenAI-compatible selectable.
        let compat = render_section(SectionCase {
            usable: strings(&["edge", "openai_compatible"]),
            compat_base_url: "http://tts.lan:8880/v1".to_string(),
            ..SectionCase::on("edge")
        });
        let compat_group = turn_based_group(&compat);
        let compat_select = &compat_group[..compat_group.find("</select>").unwrap()];
        assert!(
            compat_select.contains("<option value=\"openai_compatible\""),
            "{compat_select}"
        );
    }

    /// D-09: a provider switch resets model, voice and format to the NEW
    /// provider's registry defaults and carries the reset note.
    #[test]
    fn identity_provider_switch_resets_with_note() {
        // The defaults come from the registry, never from a literal here.
        let openai = tts_provider("openai").expect("registry provider");
        let switch = identity_provider_switch("openai").expect("a registry provider");
        assert_eq!(switch.fields.tts_provider.as_deref(), Some("openai"));
        assert_eq!(
            switch.fields.tts_model.as_deref(),
            Some(openai.default_model)
        );
        assert_eq!(
            switch.fields.tts_voice.as_deref(),
            Some(openai.default_voice)
        );
        assert_eq!(
            switch.fields.tts_format.as_deref(),
            Some(openai.default_format)
        );
        assert_eq!(
            switch.note,
            "Switched to OpenAI: model, voice and format reset to its defaults."
        );

        // Edge has no model: nothing is staged for it.
        let edge_spec = tts_provider("edge").expect("registry provider");
        let edge = identity_provider_switch("edge").expect("edge");
        assert_eq!(edge.fields.tts_model, None);
        assert_eq!(
            edge.fields.tts_voice.as_deref(),
            Some(edge_spec.default_voice)
        );
        assert_eq!(
            edge.note,
            "Switched to Edge: model, voice and format reset to its defaults."
        );

        assert!(identity_provider_switch("not_a_provider").is_none());

        // The note stays until the operator edits the block.
        let (model, voice, format) = (
            openai.default_model,
            openai.default_voice,
            Some(openai.default_format),
        );
        assert!(identity_switch_note_visible(&switch, model, voice, format));
        assert!(!identity_switch_note_visible(
            &switch,
            "a-different-model",
            voice,
            format
        ));
        assert!(!identity_switch_note_visible(
            &switch,
            model,
            "a-different-voice",
            format
        ));
        assert!(!identity_switch_note_visible(
            &switch,
            model,
            voice,
            Some("a-different-format")
        ));
    }

    /// The handler resets through the registry, not a hard-coded match.
    #[test]
    fn identity_provider_change_uses_the_registry_reset() {
        let cut = ["mod voice_form_orb_", "gate_tests"].concat();
        let production = &VOICE_SETTINGS_SRC_15[..VOICE_SETTINGS_SRC_15.find(&cut).unwrap()];
        let section = production
            .find("fn IdentityVoiceSection(")
            .map(|at| &production[at..])
            .expect("IdentityVoiceSection present");
        assert!(
            section.contains("identity_provider_switch("),
            "uses the pure reset"
        );
        let literal_match = ["match new_", "provider {"].concat();
        assert!(
            !section.contains(&literal_match),
            "no hard-coded provider match"
        );
    }

    /// D-16: Voice (Realtime) edits voice and model, each with a badge, and
    /// the hint copy is replaced.
    #[test]
    fn identity_realtime_group_has_voice_and_model() {
        let html = render_section(SectionCase::on("openai"));
        let group = realtime_group(&html);
        assert!(group.contains(">Voice<"), "{group}");
        assert!(group.contains(">Model<"), "{group}");
        assert_eq!(
            group.matches("(from Global)").count(),
            2,
            "voice and model inherit while unset: {group}"
        );
        assert!(
            group.contains("Realtime voice and model come from the realtime provider set on the server. Only the voice and model are per-identity."),
            "{group}"
        );
        assert!(
            group.contains("Changes take effect next conversation."),
            "{group}"
        );
        assert!(
            !html.contains(&["Realtime voice is always ", "OpenAI Realtime"].concat()),
            "the old hint is gone: {html}"
        );
    }

    /// The active service's group is listed first.
    #[test]
    fn identity_groups_ordered_by_service() {
        let realtime = render_section(SectionCase {
            service: "realtime".to_string(),
            ..SectionCase::on("openai")
        });
        assert!(
            realtime.find("Voice (Realtime)").unwrap()
                < realtime.find("Voice (Turn-based)").unwrap(),
            "realtime service: Realtime group first: {realtime}"
        );
        let turn_based = render_section(SectionCase::on("openai"));
        assert!(
            turn_based.find("Voice (Turn-based)").unwrap()
                < turn_based.find("Voice (Realtime)").unwrap(),
            "turn-based service: Turn-based group first: {turn_based}"
        );
    }
}

// ── Phase 54.7 Plan 08 (D-14/D-15) — the voice service contract ─────────────
//
// Declared after `voice_form_orb_gate_tests` on purpose: the root-provider
// source-text test below cuts this file at that module's name, so the test
// shells (which legitimately provide the contexts) stay out of the scanned
// production text.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod voice_service_tests {
    use super::*;
    use crate::server::api::{
        apply_voice_payload, build_voice_snapshot, validate_voice_payload, VoiceConfigSnapshot,
        VoiceKeyStatuses,
    };
    use ironhermes_core::config::{BargeInMode, Config, VoiceService};
    use std::cell::RefCell;

    thread_local! {
        static CONTROL_CASE: RefCell<(bool, String)> = const { RefCell::new((false, String::new())) };
    }

    fn snapshot_with_modes(modes: &[&str]) -> VoiceConfigSnapshot {
        VoiceConfigSnapshot {
            barge_in_mode: "open_mic".to_string(),
            barge_in_modes: modes.iter().map(|m| m.to_string()).collect(),
            voice_service: "turn_based".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn service_seg_key_navigation() {
        // Two segments: Realtime (0), Turn-based (1).
        assert_eq!(service_seg_next(0, "ArrowRight"), 1);
        assert_eq!(service_seg_next(1, "ArrowRight"), 0);
        assert_eq!(service_seg_next(0, "ArrowDown"), 1);
        assert_eq!(service_seg_next(1, "ArrowDown"), 0);
        assert_eq!(service_seg_next(1, "ArrowLeft"), 0);
        assert_eq!(service_seg_next(0, "ArrowLeft"), 1);
        assert_eq!(service_seg_next(1, "ArrowUp"), 0);
        assert_eq!(service_seg_next(0, "ArrowUp"), 1);
        assert_eq!(service_seg_next(1, "Home"), 0);
        assert_eq!(service_seg_next(0, "End"), 1);
        // Any other key leaves the selection alone.
        assert_eq!(service_seg_next(0, "Tab"), 0);
        assert_eq!(service_seg_next(1, "a"), 1);
    }

    fn render_control(loaded: bool, service: &str) -> String {
        CONTROL_CASE.with(|c| *c.borrow_mut() = (loaded, service.to_string()));

        fn test_app() -> Element {
            let (loaded, service) = CONTROL_CASE.with(|c| c.borrow().clone());
            let staged = use_signal(|| service.clone());
            use_context_provider(|| VoiceServiceCtx(staged));
            rsx! { VoiceServiceControl { loaded, disabled: false } }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// The opening `<button ...>` carrying the `id="{id}"` attribute.
    fn radio_tag<'a>(html: &'a str, id: &str) -> &'a str {
        let marker = format!("id=\"{id}\"");
        let at = html
            .find(&marker)
            .unwrap_or_else(|| panic!("radio {id} missing: {html}"));
        let start = html[..at].rfind("<button").expect("button opens");
        let end = at + html[at..].find('>').expect("tag closes");
        &html[start..=end]
    }

    #[test]
    fn service_control_disabled_and_unselected_until_loaded() {
        let html = render_control(false, "realtime");
        assert!(html.contains("role=\"radiogroup\""), "{html}");
        assert!(html.contains("aria-label=\"Voice service\""), "{html}");
        assert!(
            !html.contains("aria-checked=\"true\""),
            "no segment may be selected before the config loads: {html}"
        );
        for id in ["voice-service-realtime", "voice-service-turn-based"] {
            let tag = radio_tag(&html, id);
            assert!(tag.contains("disabled"), "{id} must be disabled: {tag}");
            assert!(!tag.contains("disabled=false"), "{id}: {tag}");
        }

        let html = render_control(true, "realtime");
        let realtime = radio_tag(&html, "voice-service-realtime");
        assert!(realtime.contains("aria-checked=\"true\""), "{realtime}");
        assert!(realtime.contains("tabindex=\"0\""), "{realtime}");
        assert!(!realtime.contains("disabled=true"), "{realtime}");
        let turn_based = radio_tag(&html, "voice-service-turn-based");
        assert!(
            turn_based.contains("aria-checked=\"false\""),
            "{turn_based}"
        );
        assert!(turn_based.contains("tabindex=\"-1\""), "{turn_based}");
    }

    thread_local! {
        static PANEL_SNAP: RefCell<VoiceConfigSnapshot> = RefCell::new(VoiceConfigSnapshot::default());
        static PANEL_OUT: RefCell<Option<VoiceWritePayload>> = const { RefCell::new(None) };
    }

    /// The panel's REAL payload: both family forms seeded from `snap` inside a
    /// VirtualDom (hooks need one), the global values seeded as the panel
    /// seeds them, then assembled exactly as the save handler assembles them.
    fn panel_payload(snap: &VoiceConfigSnapshot) -> VoiceWritePayload {
        PANEL_SNAP.with(|s| *s.borrow_mut() = snap.clone());
        PANEL_OUT.with(|o| *o.borrow_mut() = None);

        fn app() -> Element {
            let snap = PANEL_SNAP.with(|s| s.borrow().clone());
            let turn_based = use_turn_based_form();
            let realtime = use_realtime_form();
            turn_based.seed(&snap);
            realtime.seed(&snap);
            let values = GlobalVoiceValues::from_snapshot(&snap);
            let payload = assemble_voice_payload(&values, &snap, &turn_based, &realtime);
            PANEL_OUT.with(|o| *o.borrow_mut() = Some(payload));
            rsx! {}
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        PANEL_OUT
            .with(|o| o.borrow_mut().take())
            .expect("the panel harness ran")
    }

    /// D-14: values typed in BOTH families survive flipping the service control
    /// (hidden, not unmounted) and are both carried by the saved payload.
    #[test]
    fn family_values_survive_service_flip() {
        fn app() -> Element {
            let snap = VoiceConfigSnapshot {
                barge_in_modes: vec!["push_to_interrupt".to_string()],
                ..Default::default()
            };
            let turn_based = use_turn_based_form();
            let realtime = use_realtime_form();
            let mut service = use_signal(|| "realtime".to_string());
            let mut stt = turn_based.stt_provider;
            let mut tts_voice = turn_based.tts_voice;
            let mut rt_model = realtime.model;
            let mut rt_voice = realtime.voice;
            stt.set("groq".to_string());
            tts_voice.set("voice-typed".to_string());
            rt_model.set("model-typed".to_string());
            rt_voice.set("rt-voice-typed".to_string());
            // Flip the control twice (realtime -> turn_based -> realtime).
            service.set("turn_based".to_string());
            service.set("realtime".to_string());
            let mut values = GlobalVoiceValues::from_snapshot(&snap);
            values.voice_service = service.peek().clone();
            let payload = assemble_voice_payload(&values, &snap, &turn_based, &realtime);
            PANEL_OUT.with(|o| *o.borrow_mut() = Some(payload));
            rsx! {}
        }

        PANEL_OUT.with(|o| *o.borrow_mut() = None);
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let payload = PANEL_OUT
            .with(|o| o.borrow_mut().take())
            .expect("the flip harness ran");
        assert_eq!(payload.voice_service.as_deref(), Some("realtime"));
        assert_eq!(payload.stt_provider.as_deref(), Some("groq"));
        assert_eq!(payload.tts_edge_voice.as_deref(), Some("voice-typed"));
        assert_eq!(payload.realtime_model.as_deref(), Some("model-typed"));
        assert_eq!(payload.realtime_voice.as_deref(), Some("rt-voice-typed"));
    }

    const VOICE_CSS: &str = include_str!("../../../assets/voice.css");

    #[test]
    fn voice_css_has_phase_classes() {
        for needle in [
            ".voice-family",
            ".voice-field",
            ".voice-field-status",
            ".voice-test-btn",
            ".voice-refresh-btn",
            ".voice-test-result",
            "[hidden]",
            "max-width: 560px",
            "max-width: 360px",
        ] {
            assert!(
                VOICE_CSS.contains(needle),
                "voice.css is missing `{needle}`"
            );
        }
        // The restart pill rule was deleted together with the pills in plan 17
        // (the needle is assembled so this test never matches its own source).
        assert!(!VOICE_CSS.contains(&[".voice-pill", "-restart"].concat()));
        // The phase block uses only the token family: no colour literals.
        let marker = "Phase 54.7 Plan 08";
        let at = VOICE_CSS.find(marker).expect("phase block marker present");
        let block = &VOICE_CSS[at..];
        assert!(!block.contains("rgb"), "no rgb literal in the phase block");
        assert!(
            !block.contains('#'),
            "no hex colour literal (or id selector) in the phase block"
        );
    }

    #[test]
    fn global_payload_carries_voice_service() {
        let snap = snapshot_with_modes(&["push_to_interrupt"]);
        let mut values = GlobalVoiceValues::from_snapshot(&snap);
        values.voice_service = "realtime".to_string();
        let payload = global_voice_payload(&values, &snap);
        assert_eq!(payload.voice_service.as_deref(), Some("realtime"));
        values.voice_service = "turn_based".to_string();
        let payload = global_voice_payload(&values, &snap);
        assert_eq!(payload.voice_service.as_deref(), Some("turn_based"));
    }

    #[test]
    fn global_payload_omits_unlisted_barge_in_mode() {
        // One advertised mode: the Interruption row is not rendered, so a
        // stored `open_mic` must never be echoed back (the server rejects it).
        let one = snapshot_with_modes(&["push_to_interrupt"]);
        let mut values = GlobalVoiceValues::from_snapshot(&one);
        assert_eq!(values.barge_in_mode, "open_mic");
        assert_eq!(global_voice_payload(&values, &one).barge_in_mode, None);

        // Two advertised modes: a staged, listed value is sent.
        let two = snapshot_with_modes(&["push_to_interrupt", "half_duplex"]);
        values.barge_in_mode = "half_duplex".to_string();
        assert_eq!(
            global_voice_payload(&values, &two).barge_in_mode.as_deref(),
            Some("half_duplex")
        );
        // A staged value the snapshot does not list is dropped.
        values.barge_in_mode = "open_mic".to_string();
        assert_eq!(global_voice_payload(&values, &two).barge_in_mode, None);
    }

    /// The operator's live config: legacy `open_mic` plus the three legacy
    /// realtime keys, no `voice.service`.
    const OPERATOR_YAML: &str = "voice:\n  barge_in_mode: open_mic\n  realtime_model: gpt-realtime\n  realtime_voice: shimmer\n  realtime_transcription_model: gpt-4o-mini-transcribe\n";

    #[test]
    fn open_mic_snapshot_global_save_completes_migration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, OPERATOR_YAML).expect("seed operator config");

        // The panel's real payload: seeded from the snapshot of that config.
        let config = Config::load_from(&path).expect("load operator config");
        let snap = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
        assert_eq!(
            snap.barge_in_mode, "open_mic",
            "fixture must be the open_mic case"
        );
        let payload = panel_payload(&snap);
        // The folded legacy realtime values ride the payload into `realtime:`.
        assert_eq!(payload.realtime_model.as_deref(), Some("gpt-realtime"));
        assert_eq!(payload.realtime_voice.as_deref(), Some("shimmer"));
        assert_eq!(
            payload.realtime_transcription_model.as_deref(),
            Some("gpt-4o-mini-transcribe")
        );
        assert_eq!(payload.barge_in_mode, None, "open_mic must not be echoed");

        // Plan 06's real save path.
        validate_voice_payload(&payload).expect("the panel payload validates");
        let mut loaded = Config::load_from(&path).expect("reload for save");
        apply_voice_payload(&mut loaded, &payload).expect("the panel payload applies");
        loaded.save_to(&path).expect("save");

        let config = Config::load_from(&path).expect("reload after save");
        assert_eq!(config.voice.service, Some(VoiceService::Realtime));
        assert_eq!(config.voice.barge_in_mode, BargeInMode::PushToInterrupt);
        assert_eq!(config.realtime.model(), "gpt-realtime");
        assert_eq!(config.realtime.voice(), "shimmer");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).expect("yaml");
        for legacy in [
            "realtime_model",
            "realtime_voice",
            "realtime_transcription_model",
        ] {
            assert!(
                yaml["voice"].get(legacy).is_none(),
                "legacy key voice.{legacy} must be gone after the migrating save: {yaml:?}"
            );
        }
        assert_eq!(
            yaml["realtime"]["transcription_model"].as_str(),
            Some("gpt-4o-mini-transcribe")
        );
    }

    const VOICE_SETTINGS_SRC: &str = include_str!("voice_settings.rs");

    /// Production text only: everything before the test shell module.
    fn production_text(src: &'static str, cut: &str) -> &'static str {
        let at = src.find(cut).expect("test module marker present");
        &src[..at]
    }

    /// WR-01: an abandoned (unsaved) service edit must not leak into the
    /// root-provided context the voice-mode entry gate reads.
    #[test]
    fn unsaved_service_edit_is_restored_when_panel_unmounts() {
        // Staged Realtime -> Turn-based, never saved: restore the loaded service.
        assert_eq!(
            service_to_restore("realtime", "turn_based"),
            Some("realtime".to_string())
        );
        assert_eq!(
            service_to_restore("turn_based", "realtime"),
            Some("turn_based".to_string())
        );
        // Saved (loaded == staged) or never loaded: leave the context alone.
        assert_eq!(service_to_restore("realtime", "realtime"), None);
        assert_eq!(service_to_restore("", "realtime"), None);
        // The panel actually wires the hook (production text only).
        let cut = ["mod voice_form_orb_", "gate_tests"].concat();
        let settings = production_text(VOICE_SETTINGS_SRC, &cut);
        assert!(
            settings.contains("dioxus::core::use_drop(")
                && settings.contains("service_to_restore(&mirror.borrow()"),
            "VoiceSettings must restore VoiceServiceCtx on unmount"
        );
    }

    #[test]
    fn voice_service_ctx_provided_only_at_root() {
        let mod_rs = include_str!("mod.rs");
        let cut = ["mod voice_form_orb_", "gate_tests"].concat();
        let settings = production_text(VOICE_SETTINGS_SRC, &cut);
        for ctx in ["VoiceServiceCtx", "VoiceServiceLoadedCtx"] {
            // The root provides it through the module path...
            let at_root = format!("use_context_provider(|| voice_settings::{ctx}(");
            assert!(
                mod_rs.contains(&at_root),
                "HermesApp root must provide {ctx}"
            );
            // ...and the panel (a child) never provides it, qualified or not.
            let bare = format!("use_context_provider(|| {ctx}(");
            assert!(
                !settings.contains(&bare),
                "voice_settings.rs must consume {ctx}, never provide it"
            );
        }
        // The family components provide no context at all (production text only).
        let test_cut = ["#[cfg(all(", "test"].concat();
        for (name, src) in [
            ("voice_turn_based.rs", include_str!("voice_turn_based.rs")),
            ("voice_realtime.rs", include_str!("voice_realtime.rs")),
        ] {
            let prod = production_text(src, &test_cut);
            assert!(
                !prod.contains("use_context_provider("),
                "{name}: the family components must provide no context"
            );
            assert!(
                !prod.contains("VoiceServiceCtx"),
                "{name}: the family components must not touch the service context"
            );
        }
    }
}

// ── Phase 54.7 Plan 17 (D-13/D-15/D-16/D-18) — the save-bar contract ────────
//
// Declared after `voice_form_orb_gate_tests` on purpose (see the note above
// `voice_service_tests`): the source-text tests cut this file at that module's
// name, so these shells stay out of the scanned production text.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod voice_save_bar_tests {
    use super::*;
    use crate::server::api::{build_voice_snapshot, validate_voice_payload, VoiceKeyStatuses};
    use ironhermes_core::config::Config;
    use std::cell::RefCell;

    const BAD_URL_COPY: &str = "Couldn't save: Enter a full address starting with http:// or https://. Your edits are still here. Fix the field and press SAVE SETTINGS again.";
    const GENERIC_SAVE_FAILURE: &str = "Save failed. Check server logs.";

    /// Every field code plan 02's `FieldError` can put on the wire.
    const FIELD_CODES: [&str; 9] = [
        "base_url_empty",
        "base_url_scheme",
        "base_url_userinfo",
        "base_url_too_long",
        "path_shape",
        "key_ref_looks_secret",
        "key_ref_namespace",
        "model_charset",
        "voice_charset",
    ];

    /// The opening `<button ...>` of SAVE SETTINGS.
    fn save_button_tag(html: &str) -> &str {
        let at = html
            .find("voice-settings-save-btn")
            .unwrap_or_else(|| panic!("the save button must render: {html}"));
        let start = html[..at].rfind("<button").expect("button opens");
        let end = at + html[at..].find('>').expect("tag closes");
        &html[start..=end]
    }

    #[derive(Clone, Default)]
    struct BarCase {
        dirty: bool,
        blocking: usize,
        saving: bool,
        saved: bool,
        error: Option<String>,
        legacy_keys: bool,
    }

    thread_local! {
        static BAR_CASE: RefCell<BarCase> = RefCell::new(BarCase::default());
    }

    fn render_bar(case: BarCase) -> String {
        BAR_CASE.with(|c| *c.borrow_mut() = case);

        fn app() -> Element {
            let c = BAR_CASE.with(|c| c.borrow().clone());
            rsx! {
                VoiceSaveBar {
                    dirty: c.dirty,
                    blocking: c.blocking,
                    saving: c.saving,
                    saved: c.saved,
                    error: c.error.clone(),
                    legacy_keys: c.legacy_keys,
                    on_save: move |_| {},
                }
            }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// Tracer: a global payload with a bad TTS Base URL goes through plan 06's
    /// REAL validator; the error text it produces maps to the field's UI-SPEC
    /// copy through the client mapping.
    #[test]
    fn save_rejection_maps_to_field_copy_end_to_end() {
        let config = Config::default();
        let snap = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
        let values = GlobalVoiceValues::from_snapshot(&snap);
        let mut payload = global_voice_payload(&values, &snap);
        validate_voice_payload(&payload).expect("the untouched default payload validates");

        payload.tts_compat_base_url = Some(["ft", "p://x"].concat());
        let err = validate_voice_payload(&payload).expect_err("a non-http scheme is rejected");
        assert_eq!(save_error_copy(&err.to_string()), BAD_URL_COPY);
    }

    #[test]
    fn save_error_maps_field_code_to_copy() {
        assert_eq!(
            save_error_copy("voice_invalid:tts_compat_base_url:base_url_scheme"),
            BAD_URL_COPY
        );
        // The transport may wrap the text; the tag is still found.
        assert_eq!(
            save_error_copy(
                "error running server function: voice_invalid:tts_compat_base_url:base_url_scheme"
            ),
            BAD_URL_COPY
        );
        // Every known code maps; the reason's own period never doubles.
        for code in FIELD_CODES {
            let copy = save_error_copy(&format!("voice_invalid:some_field:{code}"));
            assert!(copy.starts_with("Couldn't save: "), "{code}: {copy}");
            assert!(
                copy.ends_with(
                    ". Your edits are still here. Fix the field and press SAVE SETTINGS again."
                ),
                "{code}: {copy}"
            );
            assert!(!copy.contains(".."), "{code}: doubled period in {copy}");
        }
        // Unknown code, other errors, empty text and markup all fall back to
        // the fixed generic line (T-54.7-38: server text is never echoed).
        for other in [
            "voice_invalid:x:unknown_code",
            "voice_invalid:x:<script>alert(1)</script>",
            "voice_invalid:",
            "boom",
            "",
        ] {
            assert_eq!(save_error_copy(other), GENERIC_SAVE_FAILURE, "{other}");
        }
    }

    #[test]
    fn blocking_summary_counts_and_disables_save() {
        let html = render_bar(BarCase {
            dirty: true,
            blocking: 2,
            ..Default::default()
        });
        assert!(html.contains("Fix 2 field(s) before saving."), "{html}");
        assert!(html.contains("role=\"status\""), "{html}");
        let tag = save_button_tag(&html);
        assert!(
            tag.contains("disabled"),
            "blocked save must be disabled: {tag}"
        );

        let html = render_bar(BarCase {
            dirty: true,
            blocking: 0,
            ..Default::default()
        });
        assert!(!html.contains("before saving."), "{html}");
        let tag = save_button_tag(&html);
        assert!(!tag.contains("disabled"), "a dirty clean form saves: {tag}");
    }

    #[test]
    fn save_button_disabled_while_saving() {
        assert!(save_enabled(true, true, false));
        assert!(!save_enabled(true, true, true), "in flight");
        assert!(!save_enabled(false, true, false), "not dirty");
        assert!(!save_enabled(true, false, false), "blocking errors");

        let html = render_bar(BarCase {
            dirty: true,
            saving: true,
            ..Default::default()
        });
        assert!(save_button_tag(&html).contains("disabled"), "{html}");
        let html = render_bar(BarCase {
            dirty: false,
            ..Default::default()
        });
        let tag = save_button_tag(&html);
        assert!(tag.contains("disabled"), "{tag}");
        assert!(tag.contains("opacity:0.5"), "not dirty dims to 0.5: {tag}");
    }

    #[test]
    fn save_error_renders_as_an_alert_text_node() {
        let html = render_bar(BarCase {
            dirty: true,
            error: Some(BAD_URL_COPY.to_string()),
            ..Default::default()
        });
        assert!(html.contains("role=\"alert\""), "{html}");
        assert!(html.contains("press SAVE SETTINGS again."), "{html}");
        // T-54.7-38: any text is a text node, never markup.
        let html = render_bar(BarCase {
            dirty: true,
            error: Some("<b>x</b>".to_string()),
            ..Default::default()
        });
        assert!(
            !html.contains("<b>x</b>"),
            "error text must be escaped: {html}"
        );
    }

    // ── Task 2: migration notice, apply-timing note, restart pills, Interruption row ──

    const MIGRATION_COPY: &str = "Saving moves your realtime settings into a new realtime: block in config.yaml and sets the voice service. Older IronHermes versions won't read it.";
    const TIMING_COPY: &str = "Saved changes apply to the next voice turn or session. A turn in progress finishes on the old settings. Text-to-speech used by the agent picks up changes after Apply config now.";
    const INTERRUPTION_HELPER: &str =
        "Applies to the Turn-based service. Realtime always allows interrupting.";

    /// SSR escapes apostrophes and quotes; compare against the authored copy.
    fn unescaped(html: &str) -> String {
        html.replace("&#x27;", "'")
            .replace("&#39;", "'")
            .replace("&quot;", "\"")
            .replace("&#34;", "\"")
            .replace("&#x22;", "\"")
    }

    #[test]
    fn migration_notice_shown_only_with_legacy_keys() {
        let with = render_bar(BarCase {
            dirty: true,
            legacy_keys: true,
            ..Default::default()
        });
        let with = unescaped(&with);
        assert!(with.contains(MIGRATION_COPY), "{with}");
        // Above the Save button, as a normal paragraph.
        let notice_at = with.find(MIGRATION_COPY).unwrap();
        let button_at = with.find("voice-settings-save-btn").unwrap();
        assert!(
            notice_at < button_at,
            "notice must precede the button: {with}"
        );

        let without = render_bar(BarCase {
            dirty: true,
            legacy_keys: false,
            ..Default::default()
        });
        assert!(
            !unescaped(&without).contains("new realtime: block"),
            "{without}"
        );

        // The snapshot flag drives it, for the Global target only (an identity
        // save never migrates), and a successful save clears it.
        let mut snap = VoiceConfigSnapshot {
            legacy_voice_keys_present: true,
            ..Default::default()
        };
        assert!(migration_notice_visible(&snap, "global"));
        assert!(!migration_notice_visible(&snap, "orb_classic"));
        after_save_ok(&mut snap);
        assert!(!snap.legacy_voice_keys_present);
        assert!(!migration_notice_visible(&snap, "global"));
    }

    #[test]
    fn apply_timing_note_always_visible() {
        for case in [
            BarCase::default(),
            BarCase {
                dirty: true,
                blocking: 1,
                ..Default::default()
            },
            BarCase {
                dirty: true,
                saving: true,
                ..Default::default()
            },
            BarCase {
                saved: true,
                ..Default::default()
            },
            BarCase {
                dirty: true,
                legacy_keys: true,
                error: Some("Save failed. Check server logs.".to_string()),
                ..Default::default()
            },
        ] {
            let html = render_bar(case);
            assert!(html.contains(TIMING_COPY), "{html}");
            // Under the button.
            let note_at = html.find(TIMING_COPY).unwrap();
            let button_at = html.find("voice-settings-save-btn").unwrap();
            assert!(button_at < note_at, "note must follow the button: {html}");
        }
    }

    thread_local! {
        static INT_CASE: RefCell<(Vec<String>, String)> =
            const { RefCell::new((Vec::new(), String::new())) };
    }

    fn render_interruption(modes: &[&str], current: &str) -> String {
        INT_CASE.with(|c| {
            *c.borrow_mut() = (
                modes.iter().map(|m| m.to_string()).collect(),
                current.to_string(),
            )
        });

        fn app() -> Element {
            let (modes, current) = INT_CASE.with(|c| c.borrow().clone());
            rsx! {
                InterruptionRow {
                    modes,
                    current,
                    disabled: false,
                    on_change: move |_| {},
                }
            }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn interruption_row_hidden_with_one_mode() {
        for modes in [&[][..], &["push_to_interrupt"][..]] {
            let html = render_interruption(modes, "push_to_interrupt");
            assert!(!html.contains("INTERRUPTION"), "{modes:?}: {html}");
            assert!(!html.contains("radiogroup"), "{modes:?}: {html}");
        }

        let html = render_interruption(&["push_to_interrupt", "half_duplex"], "half_duplex");
        assert!(html.contains("INTERRUPTION"), "{html}");
        assert!(html.contains("role=\"radiogroup\""), "{html}");
        assert!(html.contains(INTERRUPTION_HELPER), "{html}");
        assert_eq!(html.matches("role=\"radio\"").count(), 2, "{html}");
        assert_eq!(
            html.matches("aria-checked=\"true\"").count(),
            1,
            "exactly the staged mode is selected: {html}"
        );

        // A rendered row always saves what it shows; an unrendered row sends
        // nothing (the payload rule shares the row's visibility predicate).
        let snap = VoiceConfigSnapshot {
            barge_in_modes: vec!["push_to_interrupt".to_string(), "half_duplex".to_string()],
            ..Default::default()
        };
        let mut values = GlobalVoiceValues::from_snapshot(&snap);
        values.barge_in_mode = "half_duplex".to_string();
        assert!(interruption_row_visible(&snap.barge_in_modes));
        assert_eq!(
            global_voice_payload(&values, &snap)
                .barge_in_mode
                .as_deref(),
            Some("half_duplex")
        );
        let one = VoiceConfigSnapshot {
            barge_in_modes: vec!["push_to_interrupt".to_string()],
            ..Default::default()
        };
        assert!(!interruption_row_visible(&one.barge_in_modes));
        assert_eq!(global_voice_payload(&values, &one).barge_in_mode, None);
    }

    #[test]
    fn restart_pills_are_gone() {
        let pill_text = ["restart ", "required"].concat();
        let class = ["voice-pill", "-restart"].concat();
        let locals = [
            ["stt_provider", "_changed"].concat(),
            ["tts_provider", "_changed"].concat(),
        ];
        for (name, src) in [
            ("voice_settings.rs", include_str!("voice_settings.rs")),
            ("voice_turn_based.rs", include_str!("voice_turn_based.rs")),
        ] {
            assert!(
                !src.contains(&pill_text),
                "{name} still renders a restart pill"
            );
            assert!(!src.contains(&class), "{name} still uses the pill class");
            for local in &locals {
                assert!(!src.contains(local.as_str()), "{name} still holds {local}");
            }
        }
        assert!(
            !include_str!("../../../assets/voice.css").contains(&class),
            "voice.css still holds the restart pill rule"
        );
    }

    // ── Task 3: Realtime wake-word rule, identity-target lock note ──────────

    const WAKE_HINT: &str =
        "Wake word is not supported with the Realtime service. Switch to Turn-based to enable.";
    const IDENTITY_NOTE: &str =
        "These are global settings. Switch \"Applies to\" to Global to change them.";

    thread_local! {
        /// (staged voice service, Applies-to edit target) the panel shell seeds.
        static PANEL_CASE: RefCell<(String, String)> =
            const { RefCell::new((String::new(), String::new())) };
        /// (loaded, staged service, disabled, identity note) for the control.
        static CONTROL_CASE: RefCell<(bool, String, bool, bool)> =
            const { RefCell::new((false, String::new(), false, false)) };
    }

    /// SSR the real `VoiceSettings` under root-shaped contexts, with the staged
    /// service and the edit target seeded as given.
    fn render_panel(service: &str, edit_target: &str) -> String {
        PANEL_CASE.with(|c| *c.borrow_mut() = (service.to_string(), edit_target.to_string()));

        fn app() -> Element {
            let (service_case, target_case) = PANEL_CASE.with(|c| c.borrow().clone());
            let voice_status = use_signal(VoiceStatusState::default);
            let wake_enabled = use_signal(|| false);
            let wake_phrase = use_signal(|| "hey hermes".to_string());
            let prefs = use_signal(|| crate::ui_prefs::AvatarPrefs {
                active_identity: "orb_classic".into(),
                ..Default::default()
            });
            let edit_target = use_signal(|| target_case.clone());
            let id_provider: Signal<Option<String>> = use_signal(|| None);
            let id_voice: Signal<Option<String>> = use_signal(|| None);
            let id_rt_voice: Signal<Option<String>> = use_signal(|| None);
            let id_model: Signal<Option<String>> = use_signal(|| None);
            let id_format: Signal<Option<String>> = use_signal(|| None);
            let id_rt_model: Signal<Option<String>> = use_signal(|| None);
            let style = use_signal(|| "classic".to_string());
            let hue = use_signal(|| 186u16);
            let size = use_signal(|| 1.0f32);
            let glow = use_signal(|| 0.5f32);
            let settling = use_signal(|| false);
            let service = use_signal(|| service_case.clone());
            let service_loaded = use_signal(|| true);
            let degraded = use_signal(|| false);
            let beep = use_signal(|| true);
            let playing = use_signal(|| false);
            use_context_provider(|| voice_status);
            use_context_provider(|| WakeWordEnabledCtx(wake_enabled));
            use_context_provider(|| WakeWordPhraseCtx(wake_phrase));
            use_context_provider(|| AvatarModeCtx(prefs));
            use_context_provider(|| VoiceEditTargetCtx(edit_target));
            use_context_provider(|| IdentityTtsProviderCtx(id_provider));
            use_context_provider(|| IdentityTtsVoiceCtx(id_voice));
            use_context_provider(|| IdentityRealtimeVoiceCtx(id_rt_voice));
            use_context_provider(|| IdentityTtsModelCtx(id_model));
            use_context_provider(|| IdentityTtsFormatCtx(id_format));
            use_context_provider(|| IdentityRealtimeModelCtx(id_rt_model));
            use_context_provider(|| OrbStyleCtx(style));
            use_context_provider(|| OrbBaseHueCtx(hue));
            use_context_provider(|| OrbSizeCtx(size));
            use_context_provider(|| OrbGlowCtx(glow));
            use_context_provider(|| OrbSettlingCtx(settling));
            use_context_provider(|| VoiceServiceCtx(service));
            use_context_provider(|| VoiceServiceLoadedCtx(service_loaded));
            use_context_provider(|| RealtimeDegradedCtx(degraded));
            use_context_provider(|| BeepEnabledCtx(beep));
            use_context_provider(|| AudioPlaybackActiveCtx(playing));
            rsx! { VoiceSettings { on_close: move |_| {} } }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    fn render_control(loaded: bool, service: &str, disabled: bool, identity_note: bool) -> String {
        CONTROL_CASE
            .with(|c| *c.borrow_mut() = (loaded, service.to_string(), disabled, identity_note));

        fn app() -> Element {
            let (loaded, service, disabled, identity_note) =
                CONTROL_CASE.with(|c| c.borrow().clone());
            let staged = use_signal(|| service.clone());
            use_context_provider(|| VoiceServiceCtx(staged));
            rsx! { VoiceServiceControl { loaded, disabled, identity_note } }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// The opening tag (`<button ...>`) of the element carrying `marker`.
    fn button_tag<'a>(html: &'a str, marker: &str) -> &'a str {
        let at = html
            .find(marker)
            .unwrap_or_else(|| panic!("`{marker}` must render: {html}"));
        let start = html[..at].rfind("<button").expect("button opens");
        let end = at + html[at..].find('>').expect("tag closes");
        &html[start..=end]
    }

    #[test]
    fn wake_word_hint_and_disable_for_realtime() {
        // The pure rule.
        assert!(wake_word_blocked("realtime"));
        assert!(!wake_word_blocked("turn_based"));
        assert!(wake_controls_disabled("realtime", true));
        assert!(!wake_controls_disabled("turn_based", true));
        assert!(
            wake_controls_disabled("turn_based", false),
            "write gate closed"
        );

        // The rendered panel.
        let html = unescaped(&render_panel("realtime", "global"));
        assert!(html.contains(WAKE_HINT), "{html}");
        let toggle = button_tag(&html, "aria-label=\"Enable wake word\"");
        assert!(
            toggle.contains("disabled"),
            "realtime disables the toggle: {toggle}"
        );

        let html = unescaped(&render_panel("turn_based", "global"));
        assert!(!html.contains("Wake word is not supported"), "{html}");
    }

    #[test]
    fn identity_target_disables_global_families_with_note() {
        // The pure rule: the write gate OR a non-Global target locks the globals.
        assert!(!global_fields_locked(true, "global"));
        assert!(global_fields_locked(true, "orb_classic"));
        assert!(global_fields_locked(false, "global"));

        // The control receives exactly that: disabled radios plus the note under
        // the VOICE SERVICE heading for an identity; neither for Global.
        let identity = unescaped(&render_control(
            true,
            "turn_based",
            global_fields_locked(true, "orb_classic"),
            true,
        ));
        assert!(identity.contains(IDENTITY_NOTE), "{identity}");
        assert!(
            identity.find("VOICE SERVICE").unwrap() < identity.find(IDENTITY_NOTE).unwrap(),
            "note sits under the heading: {identity}"
        );
        for id in ["voice-service-realtime", "voice-service-turn-based"] {
            assert!(button_tag(&identity, &format!("id=\"{id}\"")).contains("disabled"));
        }
        let global = unescaped(&render_control(
            true,
            "turn_based",
            global_fields_locked(true, "global"),
            false,
        ));
        assert!(!global.contains(IDENTITY_NOTE), "{global}");
        for id in ["voice-service-realtime", "voice-service-turn-based"] {
            assert!(!button_tag(&global, &format!("id=\"{id}\"")).contains("disabled"));
        }

        // The whole panel: the note appears with an identity target only, and
        // sits between the VOICE SERVICE heading and the family groups.
        let html = unescaped(&render_panel("turn_based", "orb_classic"));
        assert!(html.contains(IDENTITY_NOTE), "{html}");
        assert!(
            html.find(IDENTITY_NOTE).unwrap() < html.find("data-family=\"realtime\"").unwrap(),
            "{html}"
        );
        let html = unescaped(&render_panel("turn_based", "global"));
        assert!(!html.contains(IDENTITY_NOTE), "{html}");

        // The panel feeds that lock to the control and BOTH family groups.
        let cut = ["mod voice_form_orb_", "gate_tests"].concat();
        let src = include_str!("voice_settings.rs");
        let production = &src[..src.find(&cut).expect("test module marker present")];
        let wiring = ["disabled: families", "_locked"].concat();
        assert_eq!(
            production.matches(&wiring).count(),
            3,
            "VoiceServiceControl, RealtimeFamily and TurnBasedFamily must all take the lock"
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod identity_test_voice_tests {
    use super::*;

    /// SSR the identity section under root-shaped contexts, with a known
    /// identity as the edit target.
    fn render_identity_section() -> String {
        fn app() -> Element {
            let edit_target = use_signal(|| "orb_classic".to_string());
            let id_provider: Signal<Option<String>> = use_signal(|| None);
            let id_voice: Signal<Option<String>> = use_signal(|| None);
            let id_rt_voice: Signal<Option<String>> = use_signal(|| None);
            let id_model: Signal<Option<String>> = use_signal(|| None);
            let id_format: Signal<Option<String>> = use_signal(|| None);
            let id_rt_model: Signal<Option<String>> = use_signal(|| None);
            let playing = use_signal(|| false);
            use_context_provider(|| VoiceEditTargetCtx(edit_target));
            use_context_provider(|| IdentityTtsProviderCtx(id_provider));
            use_context_provider(|| IdentityTtsVoiceCtx(id_voice));
            use_context_provider(|| IdentityRealtimeVoiceCtx(id_rt_voice));
            use_context_provider(|| IdentityTtsModelCtx(id_model));
            use_context_provider(|| IdentityTtsFormatCtx(id_format));
            use_context_provider(|| IdentityRealtimeModelCtx(id_rt_model));
            let service = use_signal(|| "turn_based".to_string());
            use_context_provider(|| VoiceServiceCtx(service));
            use_context_provider(|| AudioPlaybackActiveCtx(playing));
            rsx! {
                IdentityVoiceSection {
                    resolved_provider: TTS_PROVIDERS[0].id.to_string(),
                    resolved_voice: None,
                    resolved_model: None,
                    resolved_format: None,
                    resolved_realtime_voice: REALTIME_PROVIDERS[0].default_voice.to_string(),
                    resolved_realtime_model: REALTIME_PROVIDERS[0].default_model.to_string(),
                    write_enabled: true,
                    usable_providers: Vec::new(),
                    compat_base_url: String::new(),
                    compat_path: String::new(),
                    compat_key_ref: String::new(),
                }
            }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// Plan 12 (D-17): the identity section mounts the one shared Test voice
    /// button, and this file no longer defines a button of its own.
    #[test]
    fn identity_section_uses_new_test_voice_button() {
        let html = render_identity_section();
        assert!(
            html.contains("Voice (Turn-based)"),
            "positive control: the identity section renders: {html}"
        );
        let at = html
            .find("voice-test-btn")
            .unwrap_or_else(|| panic!("the shared Test voice button must render: {html}"));
        let start = html[..at].rfind("<button").expect("a button element");
        let end = at + html[at..].find("</button>").expect("a closed button");
        let button = &html[start..end];
        assert!(button.contains("▶ Test voice"), "{button}");
        assert!(
            !button.contains("style="),
            "no inline style literal: {button}"
        );

        let src = include_str!("voice_settings.rs");
        let old_definition = ["fn TestVoice", "Button("].concat();
        assert!(
            !src.contains(&old_definition),
            "the legacy component must be gone from voice_settings.rs"
        );
        let mount = ["voice_test_controls::TestVoice", "Button {"].concat();
        assert!(
            src.contains(&mount),
            "the identity section mounts the shared button"
        );
    }
}
