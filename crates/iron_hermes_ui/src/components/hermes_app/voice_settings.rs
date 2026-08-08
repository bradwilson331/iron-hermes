//! Phase 36.17.10 Plan 02 — Server-fn-backed voice settings panel.
//!
//! Replaces dead local signals from Plan 36.17.9 D-13 with a real, server-backed
//! form bound to Plan 01's `get_voice_config` / `update_voice_config`. Renders
//! VOICE → STT → TTS sections per UI-SPEC Surfaces 1–3. VAD TIMING section
//! (Surface 4) is inserted in Plan 03.
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

use crate::components::hermes_app::VoiceStatusState;
use crate::server::api::{
    get_voice_config, synth_voice_sample, update_voice_config, IdentityOverride,
    VoiceSampleAudio, VoiceWritePayload,
};

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

/// Phase 36.17.12 Plan 01: Barge-in mode string — maps to
/// `config.voice.barge_in_mode` serialized form ("push_to_interrupt" | "open_mic").
/// Newtype required to avoid `use_context::<Signal<String>>` collisions (Pattern D).
#[derive(Clone, Copy, PartialEq)]
pub struct BargeInModeCtx(pub Signal<String>);

/// Phase 36.17.12 Plan 01: Realtime-degraded flag — true when `open_mic` was
/// selected but the realtime session failed to start (D-07 fallback). Plan 03
/// sets this from the voice mode screen; this panel reads it to show the
/// non-error degrade hint. Default false.
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
#[component]
pub fn VoiceSettings(on_close: EventHandler<()>) -> Element {
    // ── Context lookups (borrows dropped into locals — Pattern B) ─────────────
    // `mut` so the VAD "live" inputs can push edits straight into VoiceStatusState,
    // which the running voice_loop reads each poll (Phase 36.17.10 live-tuning).
    let mut voice_status = use_context::<Signal<VoiceStatusState>>();
    let stt_available = voice_status.read().stt_available;
    let tts_available = voice_status.read().tts_available;

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
    // Phase 36.17.12 Plan 04 (CR-01 gap closure): consume barge-in mode + realtime-degraded
    // from the HermesApp root provider (mod.rs) via use_context — mirroring how
    // wake_enabled/wake_phrase are consumed above. The provider was moved to the root so
    // VoiceModeScreen (the ancestor) can resolve them without panic.
    let mut barge_in_mode_sig = use_context::<BargeInModeCtx>().0;
    let realtime_degraded_sig = use_context::<RealtimeDegradedCtx>().0;
    let mut stt_provider_sig = use_signal(|| "auto".to_string());
    let mut stt_model_sig = use_signal(String::new);
    let mut stt_groq_model_sig = use_signal(|| "whisper-large-v3-turbo".to_string());
    let mut stt_openai_model_sig = use_signal(|| "whisper-1".to_string());
    let mut tts_provider_sig = use_signal(|| "edge".to_string());
    let mut tts_voice_sig = use_signal(String::new);
    let mut tts_format_sig = use_signal(String::new);
    let mut silence_duration_sig = use_signal(|| 3.0f64);
    let mut web_silence_threshold_sig = use_signal(|| 5.0f32);
    let mut max_recording_sig = use_signal(|| 120u32);
    // Consumed from the HermesApp root provider (mod.rs) — NOT created/provided here.
    // voice_loop (turn-based) reads BeepEnabledCtx from a non-descendant scope, so the
    // signal must be owned at the root (context-panic rule). Set from snapshot below.
    let mut beep_enabled_sig = use_context::<BeepEnabledCtx>().0;

    // Provide all Plan 02 contexts for Plan 03+ consumers
    use_context_provider(|| AutoTtsCtx(auto_tts_sig));
    // BargeInModeCtx and RealtimeDegradedCtx are NO LONGER provided here —
    // they are consumed from the HermesApp root provider (mod.rs, Plan 04 CR-01 fix).
    use_context_provider(|| SttProviderCtx(stt_provider_sig));
    use_context_provider(|| SttModelCtx(stt_model_sig));
    use_context_provider(|| TtsProviderCtx(tts_provider_sig));
    use_context_provider(|| TtsVoiceCtx(tts_voice_sig));
    use_context_provider(|| TtsFormatCtx(tts_format_sig));
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
    let mut loaded_stt_provider = use_signal(|| "auto".to_string());
    let mut loaded_tts_provider = use_signal(|| "edge".to_string());
    // Phase 36.17.12 Plan 01: loaded snapshot of barge_in_mode for dirty detection.
    let mut loaded_barge_in_mode = use_signal(|| "push_to_interrupt".to_string());
    let mut loaded_auto_tts = use_signal(|| false);
    let mut loaded_stt_groq_model = use_signal(|| "whisper-large-v3-turbo".to_string());
    let mut loaded_stt_openai_model = use_signal(|| "whisper-1".to_string());
    let mut loaded_tts_voice = use_signal(String::new);
    let mut loaded_tts_format = use_signal(String::new);
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
            // Populate editable signals from snapshot
            auto_tts_sig.set(snapshot.auto_tts);
            stt_provider_sig.set(snapshot.stt_provider.clone());
            stt_model_sig.set(snapshot.stt_model.clone());
            stt_groq_model_sig.set(snapshot.stt_groq_model.clone());
            stt_openai_model_sig.set(snapshot.stt_openai_model.clone());
            tts_provider_sig.set(snapshot.tts_provider.clone());
            tts_voice_sig.set(snapshot.tts_edge_voice.clone());
            tts_format_sig.set(snapshot.tts_edge_format.clone());
            // Phase 36.17.12 Plan 01: populate barge-in mode from snapshot.
            barge_in_mode_sig.set(snapshot.barge_in_mode.clone());
            silence_duration_sig.set(snapshot.silence_duration);
            web_silence_threshold_sig.set(snapshot.web_silence_threshold_rms);
            max_recording_sig.set(snapshot.max_recording_seconds);
            beep_enabled_sig.set(snapshot.beep_enabled);
            wake_enabled.set(snapshot.wake_word_enabled);
            wake_phrase.set(snapshot.wake_word_phrase.clone());
            loaded_wake_word_enabled.set(snapshot.wake_word_enabled);
            loaded_wake_word_phrase.set(snapshot.wake_word_phrase.clone());

            // Populate loaded-value snapshots for dirty detection
            loaded_stt_provider.set(snapshot.stt_provider.clone());
            loaded_tts_provider.set(snapshot.tts_provider.clone());
            // Phase 36.17.12 Plan 01: loaded snapshot for barge-in mode dirty detection.
            loaded_barge_in_mode.set(snapshot.barge_in_mode.clone());
            loaded_auto_tts.set(snapshot.auto_tts);
            loaded_stt_groq_model.set(snapshot.stt_groq_model.clone());
            loaded_stt_openai_model.set(snapshot.stt_openai_model.clone());
            loaded_tts_voice.set(snapshot.tts_edge_voice.clone());
            loaded_tts_format.set(snapshot.tts_edge_format.clone());
            // Plan 03: VAD loaded-value snapshots
            loaded_silence_duration.set(snapshot.silence_duration);
            loaded_web_silence_threshold.set(snapshot.web_silence_threshold_rms);

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
    // the preset default. `should_rehydrate_appearance` gates the re-derivation
    // to REAL switches only; a no-op re-set now early-returns before touching
    // any orb_*_sig.
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
        if !*loaded.read() {
            return;
        }
        if !should_rehydrate_appearance(&last_hydrated_target.peek(), &target) {
            return;
        }
        if let Some(preset) = crate::components::hermes_app::avatar_logic::ORB_PRESET_REGISTRY
            .iter()
            .find(|p| p.id == target.as_str())
        {
            let (style, hue, size, glow) = {
                let overrides = identity_overrides_sig.peek();
                let ov = overrides.iter().find(|o| o.slug == target);
                let style = ov
                    .and_then(|o| o.appearance_style.clone())
                    .unwrap_or_else(|| preset.default_style.to_string());
                let hue = ov
                    .and_then(|o| o.appearance_base_hue)
                    .unwrap_or(preset.default_hue);
                let size = ov
                    .and_then(|o| o.appearance_size)
                    .unwrap_or(preset.default_size);
                let glow = ov
                    .and_then(|o| o.appearance_glow)
                    .unwrap_or(preset.default_glow);
                (style, hue, size, glow)
            }; // peek guard dropped before the .set() calls below
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

    // ── Read all signal values into Copy/Clone locals (Pattern B) ────────────
    let auto_tts_on = *auto_tts_sig.read();
    // Phase 36.17.12 Plan 01: barge-in mode + realtime-degraded owned locals (Pattern B).
    let barge_in_mode_val = barge_in_mode_sig.read().clone();
    let realtime_degraded_val = *realtime_degraded_sig.read();
    let stt_provider_val = stt_provider_sig.read().clone();
    let stt_groq_model_val = stt_groq_model_sig.read().clone();
    let stt_openai_model_val = stt_openai_model_sig.read().clone();
    let tts_provider_val = tts_provider_sig.read().clone();
    let tts_voice_val = tts_voice_sig.read().clone();
    let tts_format_val = tts_format_sig.read().clone();
    let write_enabled_val = *write_enabled.read();
    let secret_fields_val = secret_fields.read().clone();
    let save_saved_val = *save_label_saved.read();
    let save_error_val = save_error_msg.read().clone();
    let wake_on = *wake_enabled.read();
    let wake_phrase_val = wake_phrase.read().clone();

    // Phase 40.5 Plan 06: identity-related locals (Pattern B — borrow dropped at `;`).
    let edit_target_val = edit_target_sig.read().clone();
    let identity_tts_provider_val = identity_tts_provider_sig.read().clone();
    let identity_tts_voice_val = identity_tts_voice_sig.read().clone();
    let identity_realtime_voice_val = identity_realtime_voice_sig.read().clone();
    let identity_list_val = identity_list_sig.read().clone();
    let identity_overrides_val = identity_overrides_sig.read().clone();

    // Phase 40.5 Plan 07: orb appearance locals (Pattern B — borrow dropped at `;`).
    let orb_style_val = orb_style_sig.read().clone();
    let orb_hue_val = *orb_hue_sig.read();
    let orb_size_val = *orb_size_sig.read();
    let orb_glow_val = *orb_glow_sig.read();

    // Is the edit target an orb identity? Gates OrbAppearanceSection visibility + save fields.
    let is_orb_target = crate::components::hermes_app::avatar_logic::ORB_PRESET_REGISTRY
        .iter()
        .any(|p| p.id == edit_target_val.as_str());

    // Loaded snapshot locals for dirty-state computation
    let loaded_stt_provider_val = loaded_stt_provider.read().clone();
    let loaded_tts_provider_val = loaded_tts_provider.read().clone();
    // Phase 36.17.12 Plan 01: loaded barge-in mode local for dirty computation.
    let loaded_barge_in_mode_val = loaded_barge_in_mode.read().clone();
    let loaded_auto_tts_val = *loaded_auto_tts.read();
    let loaded_stt_groq_model_val = loaded_stt_groq_model.read().clone();
    let loaded_stt_openai_model_val = loaded_stt_openai_model.read().clone();
    let loaded_tts_voice_val = loaded_tts_voice.read().clone();
    let loaded_tts_format_val = loaded_tts_format.read().clone();
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
    let orb_appearance_dirty = is_orb_target
        && (orb_style_val != loaded_orb_style_val
            || orb_hue_val != loaded_orb_hue_val
            || (orb_size_val - loaded_orb_size_val).abs() > f32::EPSILON
            || (orb_glow_val - loaded_orb_glow_val).abs() > f32::EPSILON);

    let is_dirty = auto_tts_on != loaded_auto_tts_val
        || barge_in_mode_val != loaded_barge_in_mode_val
        || stt_provider_val != loaded_stt_provider_val
        || stt_groq_model_val != loaded_stt_groq_model_val
        || stt_openai_model_val != loaded_stt_openai_model_val
        || tts_provider_val != loaded_tts_provider_val
        || tts_voice_val != loaded_tts_voice_val
        || tts_format_val != loaded_tts_format_val
        // Plan 03: VAD fields also mark dirty
        || (*silence_duration_sig.read() - loaded_silence_duration_val).abs() > f64::EPSILON
        || (*web_silence_threshold_sig.read() - loaded_web_silence_threshold_val).abs() > f32::EPSILON
        || wake_on != *loaded_wake_word_enabled.read()
        || wake_phrase_val != *loaded_wake_word_phrase.read()
        // Phase 40.5 Plan 06: identity override dirty — any staging Ctx is explicitly set.
        || (edit_target_val != "global"
            && (identity_tts_provider_val.is_some()
                || identity_tts_voice_val.is_some()
                || identity_realtime_voice_val.is_some()))
        // Phase 40.5 Plan 07: orb appearance dirty.
        || orb_appearance_dirty;

    // ── Restart-required badges: STT/TTS provider changed from loaded value ───
    let stt_provider_changed = *loaded.read() && stt_provider_val != loaded_stt_provider_val;
    let tts_provider_changed = *loaded.read() && tts_provider_val != loaded_tts_provider_val;

    // ── STT Model row: visible only when provider ≠ "auto" ───────────────────
    let show_stt_model_row = stt_provider_val != "auto";

    // ── STT model per-provider placeholder ───────────────────────────────────
    let stt_model_placeholder = if stt_provider_val == "openai" {
        "e.g. whisper-1"
    } else {
        "e.g. whisper-large-v3-turbo"
    };

    // ── Clones for closures ───────────────────────────────────────────────────
    let stt_provider_val_c = stt_provider_val.clone();
    let tts_provider_val_c = tts_provider_val.clone();
    let stt_groq_model_val_c = stt_groq_model_val.clone();
    let stt_openai_model_val_c = stt_openai_model_val.clone();
    let tts_voice_val_c = tts_voice_val.clone();
    let tts_format_val_c = tts_format_val.clone();

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

            // ── VOICE section ─────────────────────────────────────────────
            p {
                class: "voice-settings-section-header",
                role: "heading",
                "aria-level": "3",
                "VOICE"
            }
            hr { class: "voice-settings-separator" }

            // STT status row (read-only from VoiceStatusState)
            div { class: "voice-settings-row",
                span { class: "voice-settings-label", "STT Status" }
                span {
                    class: if stt_available { "voice-status-pill is-active" } else { "voice-status-pill is-unavailable" },
                    if stt_available { "ACTIVE" } else { "UNAVAILABLE" }
                }
            }
            // TTS status row
            div { class: "voice-settings-row",
                span { class: "voice-settings-label", "TTS Status" }
                span {
                    class: if tts_available { "voice-status-pill is-active" } else { "voice-status-pill is-unavailable" },
                    if tts_available { "ACTIVE" } else { "UNAVAILABLE" }
                }
            }

            // ── VOICE MODE section (Phase 36.17.12 Plan 01, D-02/D-06) ────
            hr { class: "voice-settings-separator" }
            p {
                class: "voice-settings-section-header",
                role: "heading",
                "aria-level": "3",
                "VOICE MODE"
            }
            hr { class: "voice-settings-separator" }

            // Voice mode segmented control (Turn-based / Open mic) — maps to
            // config.voice.barge_in_mode. "live" pill = applies on next voice
            // mode entry (not mid-session). Conditional degrade hint below.
            div { class: "voice-settings-row", style: "flex-direction: column; align-items: flex-start;",
                label { class: "voice-settings-label", style: "margin-bottom:var(--space-1);",
                    "Voice mode"
                    // "live" pill — barge-in mode change takes effect on next session start.
                    span { class: "voice-pill-live", style: "margin-left:var(--space-1);", "live" }
                }
                div {
                    class: "voice-seg-control",
                    role: "radiogroup",
                    "aria-label": "Voice mode",
                    button {
                        class: if barge_in_mode_val == "push_to_interrupt" { "voice-seg is-active" } else { "voice-seg" },
                        r#type: "button",
                        role: "radio",
                        "aria-checked": if barge_in_mode_val == "push_to_interrupt" { "true" } else { "false" },
                        disabled: !write_enabled_val,
                        onclick: move |_| {
                            save_error_msg.set(None);
                            barge_in_mode_sig.set("push_to_interrupt".to_string());
                        },
                        "Turn-based"
                    }
                    button {
                        class: if barge_in_mode_val == "open_mic" { "voice-seg is-active" } else { "voice-seg" },
                        r#type: "button",
                        role: "radio",
                        "aria-checked": if barge_in_mode_val == "open_mic" { "true" } else { "false" },
                        disabled: !write_enabled_val,
                        onclick: move |_| {
                            save_error_msg.set(None);
                            barge_in_mode_sig.set("open_mic".to_string());
                        },
                        "Open mic"
                    }
                }
                // Helper text — changes with selected mode.
                p { class: "voice-settings-hint", style: "margin-top:var(--space-1);",
                    if barge_in_mode_val == "open_mic" {
                        "Full-duplex — speak while the agent is speaking (barge-in)."
                    } else {
                        "Agent takes turns — waits for you to finish speaking."
                    }
                }
                // Conditional degrade hint (D-07): open_mic selected but realtime
                // failed to start. NOT an error color — expected fallback.
                if realtime_degraded_val {
                    p {
                        class: "voice-settings-hint",
                        role: "status",
                        "aria-live": "polite",
                        style: "color:var(--fg-dim);",
                        "Realtime unavailable — falling back to turn-based voice."
                    }
                }
            }

            hr { class: "voice-settings-separator" }

            // ── STT section ───────────────────────────────────────────────
            p {
                class: "voice-settings-section-header",
                role: "heading",
                "aria-level": "3",
                "STT"
            }
            hr { class: "voice-settings-separator" }

            // STT Provider select
            div { class: "voice-settings-row",
                label {
                    r#for: "stt-provider-select",
                    class: "voice-settings-label",
                    "Provider"
                }
                div { style: "display:flex; align-items:center; gap:var(--space-1);",
                    select {
                        id: "stt-provider-select",
                        class: "voice-settings-select",
                        disabled: !write_enabled_val,
                        "aria-expanded": if show_stt_model_row { "true" } else { "false" },
                        value: "{stt_provider_val_c}",
                        onchange: move |evt| {
                            save_error_msg.set(None);
                            stt_provider_sig.set(evt.value());
                        },
                        option { value: "auto", "auto" }
                        option { value: "groq", "groq" }
                        option { value: "openai", "openai" }
                    }
                    // Restart-required badge (appears when provider changed from loaded value)
                    if stt_provider_changed {
                        span { class: "voice-pill-restart", "restart required" }
                    }
                }
            }
            // STT provider hint when "auto"
            if !show_stt_model_row {
                p { class: "voice-settings-hint",
                    "Hermes selects the provider automatically."
                }
            }

            // STT Model row: conditionally rendered (opacity+max-height transition via CSS)
            if show_stt_model_row {
                div { class: "voice-settings-row",
                    label {
                        r#for: "stt-model-input",
                        class: "voice-settings-label",
                        "Model"
                    }
                    input {
                        id: "stt-model-input",
                        r#type: "text",
                        class: "voice-wake-phrase-input",
                        disabled: !write_enabled_val,
                        placeholder: "{stt_model_placeholder}",
                        value: if stt_provider_val == "openai" { "{stt_openai_model_val_c}" } else { "{stt_groq_model_val_c}" },
                        "aria-label": "STT model",
                        oninput: {
                            let stt_provider_for_input = stt_provider_val.clone();
                            move |evt| {
                                save_error_msg.set(None);
                                if stt_provider_for_input == "openai" {
                                    stt_openai_model_sig.set(evt.value());
                                } else {
                                    stt_groq_model_sig.set(evt.value());
                                }
                            }
                        },
                    }
                }
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

            // ── Orb appearance editor (Phase 40.5 Plan 07, D-04) ──────────────
            // Rendered only when the AppliesTo target is an orb-type identity
            // (ORB_PRESET_REGISTRY membership). Hidden for head avatar identities.
            if edit_target_val != "global" && is_orb_target {
                OrbAppearanceSection { write_enabled: write_enabled_val }
                hr { class: "voice-settings-separator" }
            }

            // ── Identity voice override sections (Phase 40.5 Plan 06, D-09/D-10) ──
            // Rendered only when a non-global identity is selected in AppliesTo (D-14).
            {
                let current_override = identity_overrides_val.iter()
                    .find(|o| o.slug == edit_target_val)
                    .cloned();
                let (res_provider, res_voice, res_rt) = if let Some(ref ov) = current_override {
                    (ov.free_mode_provider.clone(), ov.free_mode_voice.clone(), ov.realtime_voice.clone())
                } else {
                    (tts_provider_val.clone(), None, "alloy".to_string())
                };
                rsx! {
                    if edit_target_val != "global" {
                        IdentityVoiceSection {
                            resolved_provider: res_provider,
                            resolved_voice: res_voice,
                            resolved_realtime_voice: res_rt,
                            write_enabled: write_enabled_val,
                        }
                        hr { class: "voice-settings-separator" }
                    }
                }
            }

            // ── TTS section ───────────────────────────────────────────────
            p {
                class: "voice-settings-section-header",
                role: "heading",
                "aria-level": "3",
                "TTS"
            }
            hr { class: "voice-settings-separator" }

            // TTS Provider select
            div { class: "voice-settings-row",
                label {
                    r#for: "tts-provider-select",
                    class: "voice-settings-label",
                    "Provider"
                }
                div { style: "display:flex; align-items:center; gap:var(--space-1);",
                    select {
                        id: "tts-provider-select",
                        class: "voice-settings-select",
                        disabled: !write_enabled_val,
                        value: "{tts_provider_val_c}",
                        onchange: move |evt| {
                            save_error_msg.set(None);
                            tts_provider_sig.set(evt.value());
                        },
                        option { value: "edge", "edge" }
                        option { value: "elevenlabs", "elevenlabs" }
                    }
                    // Restart-required badge
                    if tts_provider_changed {
                        span { class: "voice-pill-restart", "restart required" }
                    }
                }
            }

            // TTS Voice input
            div { class: "voice-settings-row",
                label {
                    r#for: "tts-voice-input",
                    class: "voice-settings-label",
                    "Voice"
                }
                input {
                    id: "tts-voice-input",
                    r#type: "text",
                    class: "voice-wake-phrase-input",
                    disabled: !write_enabled_val,
                    placeholder: "e.g. en-US-AriaNeural",
                    value: "{tts_voice_val_c}",
                    "aria-label": "TTS voice",
                    oninput: move |evt| {
                        save_error_msg.set(None);
                        tts_voice_sig.set(evt.value());
                    },
                }
            }

            // TTS Format input
            div { class: "voice-settings-row",
                label {
                    r#for: "tts-format-input",
                    class: "voice-settings-label",
                    "Format"
                }
                input {
                    id: "tts-format-input",
                    r#type: "text",
                    class: "voice-wake-phrase-input",
                    disabled: !write_enabled_val,
                    placeholder: "e.g. audio-24khz-48kbitrate-mono-mp3",
                    value: "{tts_format_val_c}",
                    "aria-label": "TTS output format",
                    oninput: move |evt| {
                        save_error_msg.set(None);
                        tts_format_sig.set(evt.value());
                    },
                }
            }

            // Auto-speak toggle (auto_tts)
            div { class: "voice-settings-row",
                label {
                    r#for: "auto-tts-toggle",
                    class: "voice-settings-label",
                    "Auto-speak"
                }
                div { style: "display:flex; align-items:center; gap:var(--space-1);",
                    button {
                        id: "auto-tts-toggle",
                        class: if auto_tts_on { "voice-toggle is-on" } else { "voice-toggle" },
                        r#type: "button",
                        role: "switch",
                        "aria-checked": if auto_tts_on { "true" } else { "false" },
                        "aria-label": "Auto-speak",
                        disabled: !write_enabled_val,
                        onclick: move |_| {
                            save_error_msg.set(None);
                            auto_tts_sig.set(!auto_tts_on);
                        },
                        span { class: "voice-toggle-thumb" }
                    }
                    // "live" pill — auto_tts takes effect without restart
                    span { class: "voice-pill-live", "live" }
                }
            }

            // Speak mode segmented control (maps to auto_tts — two presentations of same field)
            div { class: "voice-settings-row", style: "flex-direction: column; align-items: flex-start;",
                label { class: "voice-settings-label", style: "margin-bottom:var(--space-1);",
                    "Speak mode"
                }
                div {
                    class: "voice-seg-control",
                    role: "radiogroup",
                    "aria-label": "Speak mode",
                    button {
                        class: if !auto_tts_on { "voice-seg is-active" } else { "voice-seg" },
                        r#type: "button",
                        role: "radio",
                        "aria-checked": if !auto_tts_on { "true" } else { "false" },
                        disabled: !write_enabled_val,
                        onclick: move |_| {
                            save_error_msg.set(None);
                            auto_tts_sig.set(false);
                        },
                        "Tool-driven"
                    }
                    button {
                        class: if auto_tts_on { "voice-seg is-active" } else { "voice-seg" },
                        r#type: "button",
                        role: "radio",
                        "aria-checked": if auto_tts_on { "true" } else { "false" },
                        disabled: !write_enabled_val,
                        onclick: move |_| {
                            save_error_msg.set(None);
                            auto_tts_sig.set(true);
                        },
                        "Auto (every reply)"
                    }
                }
                // Helper text — changes with mode (0.12s opacity cross-fade via CSS)
                p { class: "voice-settings-hint", style: "margin-top:var(--space-1);",
                    if auto_tts_on {
                        "Hermes speaks every reply automatically."
                    } else {
                        "Hermes speaks only when it calls the audio tool."
                    }
                }
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
            // in open_mic mode. The open_mic path routes to the WebRTC realtime session
            // (start_realtime_session), which streams audio continuously and has no
            // concept of wake_word_check frames or WakeWordResult events. The controls
            // are disabled with an explanatory hint when open_mic is active.
            {
                let open_mic_active = barge_in_mode_val == "open_mic";
                rsx! {
                    p {
                        class: "voice-settings-section-header",
                        role: "heading",
                        "aria-level": "3",
                        "WAKE WORD"
                    }
                    // Open-mic incompatibility hint (shown instead of controls when open_mic).
                    if open_mic_active {
                        p { class: "voice-settings-hint",
                            style: "color:var(--fg-dim);",
                            role: "note",
                            "Wake word is not supported in Open mic mode. Switch to Turn-based to enable."
                        }
                    }
                    div { class: "voice-settings-row",
                        span { class: "voice-settings-label", "Enable wake word" }
                        button {
                            class: if wake_on && !open_mic_active { "voice-toggle is-on" } else { "voice-toggle" },
                            r#type: "button",
                            role: "switch",
                            "aria-checked": if wake_on && !open_mic_active { "true" } else { "false" },
                            "aria-label": "Enable wake word",
                            disabled: open_mic_active || !write_enabled_val,
                            onclick: move |_| {
                                if !open_mic_active {
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
                        disabled: !wake_on || open_mic_active,
                        "aria-label": "Wake phrase",
                        oninput: move |evt| {
                            if !open_mic_active {
                                wake_phrase.set(evt.value());
                            }
                        },
                    }
                    if wake_on && !open_mic_active {
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

            // ── Save bar / lock notice ────────────────────────────────────
            if write_enabled_val {
                // Save button (sticky at panel bottom)
                div { class: "voice-settings-save-bar",
                    // Save error (shown below button on failure)
                    if let Some(ref err) = save_error_val {
                        p { class: "voice-settings-hint", style: "color:var(--danger); margin-bottom:var(--space-1);",
                            "{err}"
                        }
                    }
                    button {
                        class: "voice-settings-save-btn",
                        r#type: "button",
                        style: if is_dirty { "opacity:1; cursor:pointer;" } else { "opacity:0.5; cursor:default;" },
                        disabled: !is_dirty,
                        onclick: move |_| {
                            // Pattern B: read ALL signal values into owned locals BEFORE spawn
                            let auto_tts_local         = *auto_tts_sig.read();
                            let barge_in_mode_local    = barge_in_mode_sig.read().clone();
                            let stt_provider_local     = stt_provider_sig.read().clone();
                            let stt_groq_local         = stt_groq_model_sig.read().clone();
                            let stt_openai_local       = stt_openai_model_sig.read().clone();
                            let tts_provider_local     = tts_provider_sig.read().clone();
                            let tts_voice_local        = tts_voice_sig.read().clone();
                            let tts_format_local       = tts_format_sig.read().clone();
                            let silence_dur_local      = *silence_duration_sig.read();
                            let rms_threshold_local    = *web_silence_threshold_sig.read();
                            let max_rec_local          = *max_recording_sig.read();
                            let beep_local             = *beep_enabled_sig.read();
                            let wake_word_enabled_local = *wake_enabled.read();
                            let wake_word_phrase_local  = wake_phrase.read().clone();

                            // Phase 40.5 Plan 06 (D-09/D-10): identity signal locals.
                            // Read BEFORE spawn — Pattern B (no signal borrow across .await).
                            let edit_target_local        = edit_target_sig.read().clone();
                            let identity_provider_local  = identity_tts_provider_sig.read().clone();
                            let identity_voice_local     = identity_tts_voice_sig.read().clone();
                            let identity_rt_voice_local  = identity_realtime_voice_sig.read().clone();

                            // Phase 40.5 Plan 07 (D-17): orb appearance locals — read BEFORE spawn.
                            // Pattern B: owned values; signal borrows drop at `;` before any .await.
                            let identity_style_local = orb_style_sig.read().clone();
                            let identity_hue_local   = *orb_hue_sig.read();
                            let identity_size_local  = *orb_size_sig.read();
                            let identity_glow_local  = *orb_glow_sig.read();
                            // Gate: only send appearance fields when the target is an orb identity.
                            let is_orb_save = is_orb_target;

                            if edit_target_local != "global" {
                                // ── Identity-scoped save path ─────────────────────
                                // Only identity fields are set; global fields are None
                                // so merge_voice_payload touches only the identity entry.
                                let payload = VoiceWritePayload {
                                    auto_tts: None,
                                    barge_in_mode: None,
                                    stt_provider: None,
                                    stt_groq_model: None,
                                    stt_openai_model: None,
                                    tts_provider: None,
                                    tts_edge_voice: None,
                                    tts_edge_format: None,
                                    silence_duration: None,
                                    web_silence_threshold_rms: None,
                                    max_recording_seconds: None,
                                    beep_enabled: None,
                                    wake_word_enabled: None,
                                    wake_word_phrase: None,
                                    identity_slug: Some(edit_target_local),
                                    identity_tts_provider: identity_provider_local,
                                    identity_tts_voice: identity_voice_local,
                                    identity_realtime_voice: identity_rt_voice_local,
                                    // D-17: appearance fields — present only for orb identities.
                                    // Server validate_voice_payload range-checks these before write.
                                    identity_appearance_style: if is_orb_save { Some(identity_style_local.clone()) } else { None },
                                    identity_base_hue: if is_orb_save { Some(identity_hue_local) } else { None },
                                    identity_size: if is_orb_save { Some(identity_size_local) } else { None },
                                    identity_glow: if is_orb_save { Some(identity_glow_local) } else { None },
                                };
                                spawn(async move {
                                    match update_voice_config(payload).await {
                                        Ok(()) => {
                                            // Approach A (seeding strategy): reset all identity
                                            // Ctx to None after a successful identity save so
                                            // the next target switch seeds clean.
                                            identity_tts_provider_sig.set(None);
                                            identity_tts_voice_sig.set(None);
                                            identity_realtime_voice_sig.set(None);
                                            // Phase 40.5 Plan 07: update loaded orb appearance
                                            // snapshot so dirty goes false after a successful save.
                                            if is_orb_save {
                                                loaded_orb_style.set(identity_style_local);
                                                loaded_orb_hue.set(identity_hue_local);
                                                loaded_orb_size.set(identity_size_local);
                                                loaded_orb_glow.set(identity_glow_local);
                                            }
                                            save_error_msg.set(None);
                                            save_label_saved.set(true);
                                            gloo_timers::future::TimeoutFuture::new(1500).await;
                                            save_label_saved.set(false);
                                        }
                                        Err(e) => {
                                            save_error_msg.set(Some(
                                                "Save failed. Check server logs.".to_string()
                                            ));
                                            let _ = e;
                                        }
                                    }
                                });
                            } else {
                                // ── Global save path (unchanged from pre-Plan-06) ──
                                let payload = VoiceWritePayload {
                                    auto_tts: Some(auto_tts_local),
                                    barge_in_mode: Some(barge_in_mode_local.clone()),
                                    stt_provider: Some(stt_provider_local.clone()),
                                    stt_groq_model: Some(stt_groq_local),
                                    stt_openai_model: Some(stt_openai_local),
                                    tts_provider: Some(tts_provider_local.clone()),
                                    tts_edge_voice: Some(tts_voice_local.clone()),
                                    tts_edge_format: Some(tts_format_local.clone()),
                                    silence_duration: Some(silence_dur_local),
                                    web_silence_threshold_rms: Some(rms_threshold_local),
                                    max_recording_seconds: Some(max_rec_local),
                                    beep_enabled: Some(beep_local),
                                    wake_word_enabled: Some(wake_word_enabled_local),
                                    wake_word_phrase: Some(wake_word_phrase_local.clone()),
                                    identity_slug: None,
                                    identity_tts_provider: None,
                                    identity_tts_voice: None,
                                    identity_realtime_voice: None,
                                    identity_appearance_style: None,
                                    identity_base_hue: None,
                                    identity_size: None,
                                    identity_glow: None,
                                };
                                spawn(async move {
                                    match update_voice_config(payload).await {
                                        Ok(()) => {
                                            // Update loaded-value snapshots (dirty → clean)
                                            loaded_stt_provider.set(stt_provider_local);
                                            loaded_tts_provider.set(tts_provider_local);
                                            loaded_auto_tts.set(auto_tts_local);
                                            loaded_barge_in_mode.set(barge_in_mode_local);
                                            loaded_tts_voice.set(tts_voice_local);
                                            loaded_tts_format.set(tts_format_local);
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
                                            save_error_msg.set(Some(
                                                "Save failed. Check server logs.".to_string()
                                            ));
                                            let _ = e; // error detail in server logs
                                        }
                                    }
                                });
                            }
                        },
                        if save_saved_val {
                            "Settings saved."
                        } else {
                            "SAVE SETTINGS"
                        }
                    }
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

/// Surface 2 (D-09 / D-10): per-identity voice override sections.
///
/// Rendered inside `VoiceSettings` **only** when the `AppliesTo` target is not `"global"`.
/// Exposes two independent edit groups injected between the global STT and TTS sections:
///
/// - **Voice (Free Mode)** — Provider `<select>` (Edge | OpenAI | ElevenLabs) bound to
///   `IdentityTtsProviderCtx` + a Voice text input bound to `IdentityTtsVoiceCtx`.
/// - **Voice (Realtime)** — Voice `<select>` bound to `IdentityRealtimeVoiceCtx` with
///   options sourced from the server-validated `REALTIME_ALLOWED_VOICES` set
///   (alloy/shimmer/echo/verse/ash/ballad/coral/sage).  **D-18 deviation:** the UI-SPEC
///   listed fable/onyx/nova which are standard-TTS voices that `issue_realtime_token`
///   (Plan 03) would reject; using the server list keeps Save→reject from happening.
///
/// Each field shows `InheritedFieldBadge` when its Ctx is `None` (i.e., the field inherits
/// the Global setting).  Changing the free-mode provider resets the voice field to that
/// provider's default to prevent format mismatches (T-40.5-06-02 / Pitfall 6).
///
/// `resolved_provider` / `resolved_voice` / `resolved_realtime_voice` are the server-side
/// resolved values for the selected identity (from `VoiceConfigSnapshot.identity_overrides`
/// if an override exists, or the global fallback otherwise).  They are used as display
/// placeholders when the Ctx is `None`.
#[component]
fn IdentityVoiceSection(
    resolved_provider: String,
    resolved_voice: Option<String>,
    resolved_realtime_voice: String,
    write_enabled: bool,
) -> Element {
    let mut provider_ctx = use_context::<IdentityTtsProviderCtx>().0;
    let mut voice_ctx = use_context::<IdentityTtsVoiceCtx>().0;
    let mut rt_voice_ctx = use_context::<IdentityRealtimeVoiceCtx>().0;

    // Pattern G: .read() in render path (subscribes; NOT .peek()).
    let provider_override = provider_ctx.read().clone();
    let voice_override = voice_ctx.read().clone();
    let rt_voice_override = rt_voice_ctx.read().clone();

    // Displayed control values — use override if explicitly set, else resolved global fallback.
    let shown_provider = provider_override
        .clone()
        .unwrap_or_else(|| resolved_provider.clone());
    let shown_voice = voice_override
        .clone()
        .or_else(|| resolved_voice.clone())
        .unwrap_or_default();
    let shown_rt_voice = rt_voice_override
        .clone()
        .unwrap_or_else(|| resolved_realtime_voice.clone());

    // Badge visibility — shown when Ctx is None (field inherits from global).
    let provider_inherited = provider_override.is_none();
    let voice_inherited = voice_override.is_none();
    let voice_not_set = voice_override.is_none() && resolved_voice.is_none();
    let rt_inherited = rt_voice_override.is_none();

    rsx! {
        // ── Voice (Free Mode) ─────────────────────────────────────────────
        p {
            class: "voice-settings-section-header",
            role: "heading",
            "aria-level": "3",
            "Voice (Free Mode)"
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
                        let new_provider = evt.value();
                        // T-40.5-06-02 (Pitfall 6): reset voice to provider's default on
                        // provider switch so an Edge voice name is never sent as an ElevenLabs
                        // voice ID (format mismatch causes a silent server-side reject).
                        let default_voice = match new_provider.as_str() {
                            "edge"    => Some("en-US-AriaNeural".to_string()),
                            "openai"  => Some("alloy".to_string()),
                            _         => None,
                        };
                        provider_ctx.set(Some(new_provider));
                        voice_ctx.set(default_voice);
                    },
                    option { value: "edge", "Edge" }
                    option { value: "openai", "OpenAI" }
                    option { value: "elevenlabs", "ElevenLabs" }
                }
                InheritedFieldBadge { shown: provider_inherited, not_set: false }
            }
        }
        p { class: "voice-settings-hint",
            "Overrides the global TTS provider for this identity only."
        }

        div { class: "voice-settings-row",
            label {
                r#for: "identity-tts-voice-input",
                class: "voice-settings-label",
                "Voice"
            }
            div { style: "display:flex; align-items:center; gap:var(--space-1);",
                input {
                    id: "identity-tts-voice-input",
                    r#type: "text",
                    class: "voice-wake-phrase-input",
                    disabled: !write_enabled,
                    placeholder: "e.g. en-US-AriaNeural",
                    value: "{shown_voice}",
                    "aria-label": "Identity TTS voice",
                    oninput: move |evt| {
                        let v = evt.value();
                        // Empty input → revert to inherited (badge reappears).
                        if v.is_empty() {
                            voice_ctx.set(None);
                        } else {
                            voice_ctx.set(Some(v));
                        }
                    },
                }
                InheritedFieldBadge { shown: voice_inherited, not_set: voice_not_set }
            }
        }

        // ── Test voice (Phase 41.2 Plan 04, D-06) ──────────────────────────
        // Placed within Voice (Free Mode), near the activation control above
        // (UI-SPEC §4 placement). Reads VoiceEditTargetCtx directly.
        TestVoiceButton {}

        // ── Voice (Realtime) ──────────────────────────────────────────────
        p {
            class: "voice-settings-section-header",
            role: "heading",
            "aria-level": "3",
            "Voice (Realtime)"
        }
        hr { class: "voice-settings-separator" }

        div { class: "voice-settings-row",
            label {
                r#for: "identity-rt-voice-select",
                class: "voice-settings-label",
                "Voice"
            }
            div { style: "display:flex; align-items:center; gap:var(--space-1);",
                select {
                    id: "identity-rt-voice-select",
                    class: "voice-settings-select",
                    disabled: !write_enabled,
                    value: "{shown_rt_voice}",
                    onchange: move |evt| {
                        rt_voice_ctx.set(Some(evt.value()));
                    },
                    // D-18: options use REALTIME_ALLOWED_VOICES (server-validated set) rather
                    // than the UI-SPEC's fable/onyx/nova (standard-TTS voices that
                    // issue_realtime_token would reject).
                    option { value: "alloy",   "alloy"   }
                    option { value: "shimmer",  "shimmer"  }
                    option { value: "echo",    "echo"    }
                    option { value: "verse",   "verse"   }
                    option { value: "ash",     "ash"     }
                    option { value: "ballad",  "ballad"  }
                    option { value: "coral",   "coral"   }
                    option { value: "sage",    "sage"    }
                }
                InheritedFieldBadge { shown: rt_inherited, not_set: false }
            }
        }
        p { class: "voice-settings-hint",
            "Realtime voice is always OpenAI Realtime — only the voice selection is per-identity."
        }
        p { class: "voice-settings-hint",
            "Changes take effect next conversation."
        }
    }
}

/// Phase 41.2 Plan 04 (D-06): "Test voice" button — auditions the edit-target
/// identity's free-mode TTS voice via `synth_voice_sample` (Plan 02's
/// `#[server]` seam) and plays it back through the EXISTING AudioOut playback
/// path (`mod.rs:557-604`, mirrored here in the encode-received direction),
/// reusing the shared `AudioPlaybackActiveCtx` half-duplex guard. This is a
/// direct synth-and-play, not a conversation turn (D-12's "next conversation"
/// framing does not apply here) — it plays immediately.
///
/// Rendered inside [`IdentityVoiceSection`]'s Voice (Free Mode) subsection,
/// near [`SetActiveIdentityControl`] (UI-SPEC §4 placement: "within the Voice
/// (Free Mode) section for the edit-target identity").
///
/// Reads `VoiceEditTargetCtx` directly (mirrors `SetActiveIdentityControl`)
/// rather than taking a prop — renders nothing for the synthetic "global"
/// target (T-41.2-05: the slug driving `synth_voice_sample` must always come
/// from a real, already-validated identity).
#[component]
fn TestVoiceButton() -> Element {
    let edit_target_sig = use_context::<VoiceEditTargetCtx>().0;
    let mut playback_active = use_context::<AudioPlaybackActiveCtx>().0;
    let mut test_voice_playing = use_signal(|| false);
    let mut test_current_audio: Signal<Option<web_sys::HtmlAudioElement>> = use_signal(|| None);
    // Phase 41.2 Plan 04 Task 2 (D-06): transient retriable-error message —
    // distinct from AudioPlaybackActiveCtx/test_voice_playing (UI-SPEC §4
    // Error state), cleared on the next interaction.
    let mut test_voice_error: Signal<Option<String>> = use_signal(|| None);

    // Pattern G: .read() in render subscribes (NOT .peek()).
    let edit_target = edit_target_sig.read().clone();
    let is_playing = *test_voice_playing.read();
    // Half-duplex guard (UI-SPEC §4 Disabled state): true only when SOME
    // OTHER voice audio (main conversation TTS or another invocation) is
    // playing — never true for this button's own in-flight playback.
    let other_playing = *playback_active.read() && !is_playing;
    let error_msg = test_voice_error.read().clone();

    if edit_target == "global" {
        return rsx! {};
    }

    let label = if is_playing { "■ Playing…" } else { "▶ Test voice" };
    let btn_style = if other_playing {
        "display:block; min-height:44px; padding:0 var(--space-3); \
         border-radius:var(--w-radius-block); font-family:var(--font-mono); \
         font-size:var(--type-body,14px); cursor:not-allowed; opacity:0.5; \
         background:transparent; border:1px solid var(--accent-primary); \
         color:var(--accent-primary); white-space:nowrap; box-sizing:border-box;"
    } else {
        "display:block; min-height:44px; padding:0 var(--space-3); \
         border-radius:var(--w-radius-block); font-family:var(--font-mono); \
         font-size:var(--type-body,14px); cursor:pointer; \
         background:transparent; border:1px solid var(--accent-primary); \
         color:var(--accent-primary); white-space:nowrap; box-sizing:border-box;"
    };

    rsx! {
        div {
            class: "voice-settings-row",
            style: "flex-direction:column; align-items:flex-start; margin-top:var(--space-3);",
            button {
                r#type: "button",
                disabled: other_playing,
                style: "{btn_style}",
                onclick: move |_| {
                    // Belt-and-suspenders: the `disabled` attribute already stops the
                    // browser from dispatching this click while other audio plays.
                    if *playback_active.peek() && !*test_voice_playing.peek() {
                        return;
                    }
                    test_voice_error.set(None);
                    if *test_voice_playing.peek() {
                        // D-06: clicking while playing stops immediately, returns to idle.
                        #[cfg(target_arch = "wasm32")]
                        {
                            if let Some(el) = test_current_audio.peek().clone() {
                                let _ = el.pause();
                            }
                        }
                        test_current_audio.set(None);
                        test_voice_playing.set(false);
                        playback_active.set(false);
                        return;
                    }
                    // Pattern B: read the target fresh at click time, owned local — no
                    // signal borrow spans the .await below.
                    let slug_for_call = edit_target_sig.peek().clone();
                    test_voice_playing.set(true);
                    playback_active.set(true);
                    spawn(async move {
                        match synth_voice_sample(slug_for_call).await {
                            Ok(VoiceSampleAudio { audio_b64, mime }) => {
                                // Mirrors mod.rs:557-604 (existing AudioOut handler)
                                // verbatim for the Blob/URL/audio steps — the only
                                // difference is these bytes come from a #[server] fn
                                // return, not a WS Binary frame.
                                #[cfg(target_arch = "wasm32")]
                                {
                                    use base64::Engine as _;
                                    let mut played = false;
                                    if let Ok(bytes) =
                                        base64::engine::general_purpose::STANDARD.decode(&audio_b64)
                                    {
                                        let uint8_array = js_sys::Uint8Array::from(bytes.as_slice());
                                        let parts = js_sys::Array::new();
                                        parts.push(&uint8_array);
                                        let opts = web_sys::BlobPropertyBag::new();
                                        opts.set_type(&mime);
                                        if let Ok(blob) =
                                            web_sys::Blob::new_with_u8_array_sequence_and_options(
                                                &parts, &opts,
                                            )
                                        {
                                            if let Ok(url) =
                                                web_sys::Url::create_object_url_with_blob(&blob)
                                            {
                                                if let Ok(audio_el) =
                                                    web_sys::HtmlAudioElement::new_with_src(&url)
                                                {
                                                    use wasm_bindgen::JsCast as _;
                                                    crate::components::hermes_app::voice_loop::tap_speaking_analyser(&audio_el);
                                                    test_current_audio.set(Some(audio_el.clone()));

                                                    // ended: playback completed normally → idle.
                                                    let ended_cb = {
                                                        let mut pb = playback_active;
                                                        let mut tv = test_voice_playing;
                                                        let mut cur = test_current_audio;
                                                        wasm_bindgen::closure::Closure::<dyn FnMut()>::wrap(
                                                            Box::new(move || {
                                                                pb.set(false);
                                                                tv.set(false);
                                                                cur.set(None);
                                                            }),
                                                        )
                                                    };
                                                    // error: playback failed → idle + transient
                                                    // retriable error message (UI-SPEC §4 Error state).
                                                    let error_cb = {
                                                        let mut pb = playback_active;
                                                        let mut tv = test_voice_playing;
                                                        let mut cur = test_current_audio;
                                                        let mut err = test_voice_error;
                                                        wasm_bindgen::closure::Closure::<dyn FnMut()>::wrap(
                                                            Box::new(move || {
                                                                pb.set(false);
                                                                tv.set(false);
                                                                cur.set(None);
                                                                err.set(Some(
                                                                    "Couldn't play sample. Try again."
                                                                        .to_string(),
                                                                ));
                                                            }),
                                                        )
                                                    };
                                                    let _ = audio_el.add_event_listener_with_callback(
                                                        "ended",
                                                        ended_cb.as_ref().unchecked_ref(),
                                                    );
                                                    let _ = audio_el.add_event_listener_with_callback(
                                                        "error",
                                                        error_cb.as_ref().unchecked_ref(),
                                                    );
                                                    // Keep closures alive until they fire.
                                                    ended_cb.forget();
                                                    error_cb.forget();
                                                    let _ = audio_el.play();
                                                    played = true;
                                                }
                                            }
                                        }
                                    }
                                    if !played {
                                        playback_active.set(false);
                                        test_voice_playing.set(false);
                                        test_voice_error.set(Some(
                                            "Couldn't play sample. Try again.".to_string(),
                                        ));
                                    }
                                }
                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    // Server-side render path: no Blob/audio API.
                                    let _ = (audio_b64, mime);
                                    playback_active.set(false);
                                    test_voice_playing.set(false);
                                }
                            }
                            Err(_e) => {
                                // Synth request itself failed (network/server/TTS
                                // provider error) — revert to idle, stay retriable.
                                playback_active.set(false);
                                test_voice_playing.set(false);
                                test_voice_error.set(Some(
                                    "Couldn't play sample. Try again.".to_string(),
                                ));
                            }
                        }
                    });
                },
                "{label}"
            }
            // UI-SPEC §4 / Interaction & Feedback Contract: never silently inert —
            // a disabled button always carries a visible reason; a synth failure
            // always carries a retriable, visible reason too.
            if other_playing {
                p {
                    style: "font-size:11px; color:var(--fg-dim); margin-top:var(--space-1);",
                    "Can't test while voice is playing."
                }
            } else if let Some(msg) = error_msg {
                p {
                    style: "font-size:11px; color:var(--fg-dim); margin-top:var(--space-1);",
                    "{msg}"
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 40.5 Plan 07 — Orb appearance editor (D-04 / D-05 / D-17)
// ─────────────────────────────────────────────────────────────────────────────

/// Surface 3 (D-04): Orb appearance editor — 2×2 preset card grid + free-form knobs.
///
/// Rendered only when the AppliesTo target is an orb-type identity (gated in VoiceSettings
/// on `is_orb_target`). Hidden for head avatar identities.
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
        ("var(--fg-dim)", "var(--fg)", "var(--fg-dim)", "var(--accent-primary)")
    };
    let icon_style = format!("font-size:18px; color:{icon_color};");
    let name_style = format!("font-size:var(--type-body,14px); color:{text_color}; font-weight:700;");
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
