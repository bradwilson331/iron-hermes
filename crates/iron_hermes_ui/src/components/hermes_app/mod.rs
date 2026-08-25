//! Phase 26.2.1 — HermesApp root composer.
//!
//! This is the root component of the wheel-driven shell. It owns the four
//! root signals (`UiPrefs`, theme string, `WheelState`, active `Screen`),
//! provides them via `use_context_provider`, runs the localStorage
//! hydration gate exactly once on mount, and then keeps localStorage in
//! sync via three persistence effects (RESEARCH Pattern 5).
//!
//! Plan 04 mounted the wheel SVG + wheel-rail. Plan 05 inserted the
//! tweaks-panel + theme-effects. Plan 06 (this commit) HOISTS the chat
//! WebSocket here — `use_websocket` lives at the root component which
//! never unmounts, avoiding RESEARCH Pitfall 5 (per-screen unmount would
//! tear down the connection on every wheel-launch). The receive loop,
//! session bootstrap, and the user-input send-fn all live here; the
//! resulting `Signal<Vec<ChatBubble>>`, `Signal<Option<u64>>`,
//! `SessionIdContext`, `Signal<(u32, u32)>`, and `ChatSendHandler` are
//! provided via context so ScreenChat (and any future screen that wants
//! to peek at chat state) consumes them by lookup.

use crate::components::hermes_app::screens::chat::{ChatBubble, ChatSendHandler, ToolRow};
use crate::state::ThemeContext;
use crate::state::{Screen, SessionIdContext, WheelState};
use crate::ui_prefs::{self, UiPrefs};
use dioxus::prelude::*;

/// Phase 36.17.9 (D-14): client-side mirror of the server's VoiceStatus snapshot.
///
/// Populated on every `ChatStreamEvent::VoiceStatus` frame received from the server.
/// Provided via `use_context_provider` so all voice components (ScreenChat, VoiceModeScreen,
/// VoiceSettings) consume it by lookup. Defaults to all-false / None so pre-connect
/// renders show the mic as unavailable rather than crashing.
///
/// Server-driven only — client MUST NOT write availability back to the server
/// (T-36.17.9-01-01 mitigated here by the one-directional data flow).
#[derive(Clone, Default, PartialEq)]
// Populated from the protocol VoiceStatus event by the web WS client handler and
// read by the web voice UI; native dead-code analysis cannot reach those web-gated
// sites. Keep it (web-live) and silence native-only dead_code.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub struct VoiceStatusState {
    pub stt_available: bool,
    pub stt_provider: Option<String>,
    /// Plan 03: active STT model name derived from provider + per-provider config.
    pub stt_model: Option<String>,
    pub tts_available: bool,
    pub tts_provider: Option<String>,
    pub ffmpeg_present: bool,
    /// Plan 03 (VOICE-02): VAD silence duration in seconds (config.voice.silence_duration).
    /// None until first VoiceStatus event — voice_loop falls back to vad_params::SILENCE_POLLS.
    pub silence_duration_secs: Option<f64>,
    /// Plan 03 (VOICE-02): Web Audio RMS threshold (config.voice.web_silence_threshold_rms).
    pub web_silence_threshold_rms: Option<f32>,
    /// Plan 03 (VOICE-02): Speech-confirm window in ms (hardcoded 500ms from VoiceStatus).
    pub speech_confirm_ms: Option<u32>,
    /// Plan 03 (VOICE-02): Auto-TTS flag (config.voice.auto_tts).
    pub auto_tts: Option<bool>,
}

/// Phase 47.3 Plan 06 (D-17, CONTEXT.md Correction 2): whether the WASM
/// shell currently believes it holds a live session.
///
/// Initialized `true` — by construction, the shell only ever loaded because
/// `root_handler` (Plan 01, D-07) already authenticated the `GET /` that
/// served it. There is NO client-side login screen and NO mount-time
/// `/auth/session` probe (the superseded `login_gate.rs` draft did this;
/// reintroducing it would signal the WASM shell is expected to load before
/// auth, which D-07 explicitly rejects). This context exists ONLY for two
/// consumers: the D-17 session-death redirect (flips to `false` on a 401 or
/// WS close, then hard-navigates to `/`) and the `/logout` palette action.
///
/// Newtype (not a bare `Signal<bool>`) per the context-newtype discipline
/// this file already documents for `Signal<bool>`/`Signal<u64>` providers
/// (Phase 36.17.9 post-mortem, see `is_ws_connected` / `voice_mode_active` /
/// `wake_word_matched` below) — without it, `use_context::<Signal<bool>>()`
/// would silently resolve to whichever provider was registered last.
#[allow(dead_code)] // context-provider newtype; consumed via use_context::<AuthedContext>() in ScreenChat (D-17 expiry redirect / /logout)
#[derive(Clone, Copy)]
pub struct AuthedContext(pub Signal<bool>);

pub mod app_footer;
pub mod avatar_logic;
pub mod breadcrumb;
// Phase 41.1 Plan 06 (D-05): Web `/` command palette, ported into hermes_app
// (adapted from the feature-gated shell_legacy::CommandPalette).
pub mod command_palette;
pub mod hud_chrome;
// Phase 46.9 Plan 05 (D-02): brand-new Kanban config embedded panel.
pub mod kanban_config;
pub mod mic_button;
pub mod orb_canvas;
pub mod realtime_session;
pub mod screen_router;
pub mod screens;
pub mod sys_meta;
pub mod theme_effects;
pub mod tweaks_panel;
pub mod voice_loop;
pub mod voice_settings;
pub mod wheel;
pub mod wheel_rail;
// Phase 50.1 Plan 04 (D-12): shared presentational widgets — BotFace, the
// deterministic geometric avatar renderer. Declares no context provider
// (see widgets.rs's own module doc).
pub mod widgets;

/// Root component of the Phase 26.2.1 wheel-driven shell.
///
/// Owns the four root signals and gates persistence through a one-shot
/// hydration effect — defaults never clobber stored values (RESEARCH
/// Pitfall 5 / Pattern 5). Plan 06 adds five more chat-state signals
/// (bubbles / streaming_id / session_id / tokens / send-handler) and the
/// hoisted `use_websocket` + receive loop.
#[component]
pub fn HermesApp() -> Element {
    let mut prefs = use_signal(UiPrefs::default);
    let mut theme = use_signal(|| "slate-dark".to_string());
    let mut wheel_state = use_signal(WheelState::default);
    let active_screen = use_signal(|| Screen::Chat);
    // Phase 50.1 Plan 08 (D-22): the routines deep-link's prefill seam —
    // see `ScheduleNamePrefillCtx`'s own doc comment (state.rs) for the
    // full contract.
    let schedule_name_prefill: Signal<Option<String>> = use_signal(|| None);
    let mut hydrated = use_signal(|| false);
    // Phase 47.3 Plan 06 (D-17): starts authenticated — see AuthedContext's
    // doc comment above for why. Only the D-17 expiry redirect and /logout
    // ever flip this to `false`.
    let authed = use_signal(|| true);
    // Phase 40.2 Plan 04 (FE-01, D-01, D-03): avatar mode preferences signal.
    // Declared here (before the hydration use_effect) so the effect can capture it.
    let mut avatar_prefs = use_signal(crate::ui_prefs::AvatarPrefs::default);
    // Phase 40.2 Plan 04 (FE-05, D-11): per-session avatar-error notice flag.
    // Set by orb_canvas when window.__ihAvatarError fires; consumed by voice_mode.
    // NOT persisted — once-per-session (Research Open Q 3).
    let avatar_error_notice = use_signal(|| false);

    // Phase 40.5 Plan 01 (D-04, D-05, D-07, D-09, D-10): orb customisation + per-identity
    // voice signals. Declared BEFORE the hydration use_effect so the effect can write
    // identity-derived defaults on first load (ORB_PRESET_REGISTRY lookup + active_identity).
    // All 11 are provided via context at the root providers block below (Dioxus context-panic
    // rule: child providers panic ancestor/sibling consumers — see MEMORY.md).
    //
    // Identity voice-edit target — default "orb_classic" until localStorage is read.
    // `mut` needed: .set() is called on these inside the hydration use_effect below.
    let mut voice_edit_target = use_signal(|| "orb_classic".to_string());
    // Per-identity TTS/realtime overrides — None = inherit global config.tts/voice.
    // Not seeded in hydration (no localStorage for per-identity overrides in this plan);
    // Plans 02 and 06 write to these via VoiceEditTargetCtx consumers.
    let identity_tts_provider: Signal<Option<String>> = use_signal(|| None);
    let identity_tts_voice: Signal<Option<String>> = use_signal(|| None);
    let identity_realtime_voice: Signal<Option<String>> = use_signal(|| None);
    // Orb appearance — seeded from ORB_PRESET_REGISTRY["orb_classic"] defaults.
    // `mut` needed: hydration effect overwrites these from the stored active_identity.
    let mut orb_style = use_signal(|| "classic".to_string());
    let mut orb_base_hue: Signal<u16> = use_signal(|| 186u16);
    let mut orb_size: Signal<f32> = use_signal(|| 1.0f32);
    let mut orb_glow: Signal<f32> = use_signal(|| 0.5f32);
    // Phase 41.2 Plan 01 (D-03): orb style-switch "settling" flag — ephemeral,
    // never persisted. Written by the orb_canvas.rs OrbStyleCtx bridge, read
    // by OrbAppearanceSection's transient "Switching style…" label.
    let orb_settling = use_signal(|| false);
    // Session lifecycle flags — ephemeral (once-per-session, never persisted).
    let wake_session_active = use_signal(|| false);
    let wake_session_stop = use_signal(|| false);
    let audio_playback_active = use_signal(|| false);
    // Beep/chime toggle. The SIGNAL must live at the HermesApp root (not inside the
    // VoiceSettings child) because voice_loop — turn-based, running under
    // VoiceModeScreen, a non-descendant of VoiceSettings — consumes BeepEnabledCtx
    // for the wake chime. Previously provided in VoiceSettings → "Could not find
    // context BeepEnabledCtx" panic on entering turn-based voice. VoiceSettings now
    // consumes this signal and sets it from the snapshot.
    let beep_enabled = use_signal(|| true);

    // G-41.2-11 (full persistence): load the SAVED per-identity orb appearance from
    // config at startup so the live voice-mode orb reflects persisted
    // Style/hue/size/glow on EVERY reload — not only after the Voice Settings panel
    // opens. The hydration effect below seeds ONLY ORB_PRESET_REGISTRY defaults; this
    // applies the active identity's saved `IdentityOverride.appearance` on top when
    // present. Applied exactly once (via `orb_appearance_hydrated`); `.peek()` on
    // `avatar_prefs` avoids re-running on later active-identity changes (D-12: those
    // take effect next conversation, not mid-session) and never fights the Settings
    // panel's own edit-target rehydrate (it writes the same shared orb signals).
    let mut orb_appearance_hydrated = use_signal(|| false);
    let orb_config_resource =
        use_resource(move || async move { crate::server::api::get_voice_config().await });
    use_effect(move || {
        if *orb_appearance_hydrated.peek() {
            return;
        }
        if let Some(Ok(snapshot)) = orb_config_resource.read().as_ref() {
            let active = avatar_prefs.peek().active_identity.clone();
            if let Some(ov) = snapshot.identity_overrides.iter().find(|o| o.slug == active) {
                if let Some(ref style) = ov.appearance_style {
                    orb_style.set(style.clone());
                }
                if let Some(hue) = ov.appearance_base_hue {
                    orb_base_hue.set(hue);
                }
                if let Some(size) = ov.appearance_size {
                    orb_size.set(size);
                }
                if let Some(glow) = ov.appearance_glow {
                    orb_glow.set(glow);
                }
            }
            orb_appearance_hydrated.set(true);
        }
    });

    // Context providers — four root signals exposed to descendants.
    //
    // The theme signal is wrapped in `ThemeContext(theme)` (B-03 newtype,
    // D-26) so it does not type-collide with Plan 06's `SessionIdContext`
    // (also a `Signal<String>`). The other three signals have unique
    // types in the tree and are provided as bare `Signal<T>`.
    use_context_provider(|| prefs);
    use_context_provider(|| crate::state::ThemeContext(theme));
    use_context_provider(|| wheel_state);
    use_context_provider(|| active_screen);
    // Phase 50.1 Plan 08 (D-22): the ONE legal home for this provider —
    // declaring it in a child screen/component compiles clean and panics
    // every consumer at runtime (this file's own module doc / D-10).
    use_context_provider(|| crate::state::ScheduleNamePrefillCtx(schedule_name_prefill));

    // Suppress unused-variable warnings on the wrapper read path — the
    // ThemeContext newtype constructor uses the `theme` signal by move,
    // and Plan 05's theme-effects consumer reads it via context.
    let _ = ThemeContext;

    // Hydration gate: read localStorage exactly once. On non-WASM hosts
    // (server / unit-test builds) the read helpers return `None`, so
    // `hydrated` simply flips to `true` without overwriting any defaults.
    use_effect(move || {
        if *hydrated.read() {
            return;
        }
        if let Some(p) = ui_prefs::read_json::<UiPrefs>(ui_prefs::KEY_TWEAKS) {
            prefs.set(p);
        }
        if let Some(t) = ui_prefs::read_string(ui_prefs::KEY_THEME) {
            theme.set(t);
        }
        if let Some(ws) = ui_prefs::read_json::<WheelState>(ui_prefs::KEY_WHEEL) {
            wheel_state.set(ws);
        }
        // Phase 40.2 Plan 04 (FE-01, D-03, T-40.2-04-01): hydrate avatar prefs.
        // Phase 40.5 Plan 01 (D-17): also seed orb appearance from ORB_PRESET_REGISTRY
        // using the stored active_identity, and seed voice_edit_target.
        //
        // Validate head_id against PRESET_REGISTRY after deserialization — a tampered
        // blob with an unknown head_id falls back to the registry default (defense-in-depth).
        if let Some(ap) = ui_prefs::read_json::<crate::ui_prefs::AvatarPrefs>(ui_prefs::KEY_AVATAR)
        {
            // Phase 40.5 (D-17): seed orb appearance BEFORE consuming `ap` via .set().
            // Clone to avoid borrow-after-move when avatar_prefs.set(ap) consumes `ap`.
            let active_slug = ap.active_identity.clone();
            if let Some(preset) = crate::components::hermes_app::avatar_logic::ORB_PRESET_REGISTRY
                .iter()
                .find(|p| p.id == active_slug.as_str())
            {
                // orb-type identity: seed all four appearance signals from the preset.
                voice_edit_target.set(active_slug.clone());
                orb_style.set(preset.default_style.to_string());
                orb_base_hue.set(preset.default_hue);
                orb_size.set(preset.default_size);
                orb_glow.set(preset.default_glow);
            } else if crate::components::hermes_app::avatar_logic::is_known_identity(&active_slug) {
                // Head-rig identity — no orb preset to seed; just update the edit target.
                // Orb appearance signals keep their "orb_classic" defaults.
                voice_edit_target.set(active_slug);
            }
            // else: unknown/tampered slug — voice_edit_target stays "orb_classic" default.

            let valid_id = crate::components::hermes_app::avatar_logic::PRESET_REGISTRY
                .iter()
                .any(|p| p.id == ap.head_id);
            if valid_id {
                avatar_prefs.set(ap);
            } else {
                // Unknown head_id — keep enabled flag but reset to default head.
                avatar_prefs.set(crate::ui_prefs::AvatarPrefs {
                    enabled: ap.enabled,
                    head_id: "facecap".to_string(),
                    active_identity: "orb_classic".to_string(),
                });
            }
        }
        hydrated.set(true);
    });

    // Persist-on-change effects — gated on `hydrated` so the initial
    // `UiPrefs::default()` never overwrites a stored blob.
    //
    // Signal-borrow safety (clippy.toml): read into a local, drop the
    // borrow at the `;`, then call the side-effecting write helper.
    use_effect(move || {
        if !*hydrated.read() {
            return;
        }
        let p = prefs.read().clone();
        ui_prefs::write_json(ui_prefs::KEY_TWEAKS, &p);
    });
    use_effect(move || {
        if !*hydrated.read() {
            return;
        }
        let t = theme.read().clone();
        ui_prefs::write_string(ui_prefs::KEY_THEME, &t);
    });
    use_effect(move || {
        if !*hydrated.read() {
            return;
        }
        let ws = wheel_state.read().clone();
        ui_prefs::write_json(ui_prefs::KEY_WHEEL, &ws);
    });

    // Phase 40.2 Plan 04 (FE-01, D-03): persist avatar prefs on change.
    // Mirrors the wheel_state persist-on-change pattern above (Analog C).
    // Pattern B: read into owned local, drop borrow at `;` — no borrow-across-await.
    use_effect(move || {
        if !*hydrated.read() {
            return;
        }
        let ap = avatar_prefs.read().clone();
        ui_prefs::write_json(ui_prefs::KEY_AVATAR, &ap);
    });

    // -----------------------------------------------------------------
    // Plan 06 — Chat signal hub (HOISTED at HermesApp root per
    // RESEARCH Pitfall 5: `use_websocket` MUST live in a component that
    // never unmounts. HermesApp is the root, so it's the safe placement.
    // -----------------------------------------------------------------

    let mut bubbles = use_signal(Vec::<ChatBubble>::new);
    let mut streaming_id = use_signal(|| Option::<u64>::None);
    // Phase 50.2 Plan 07 (D-20): the roster's bot names — the send
    // closure's `@mention` detection (below) reads this via call syntax
    // (a plain synchronous read, never held across an `.await`) so a
    // mention target list is known BEFORE the outbound WS send.
    let roster_names_resource =
        use_resource(move || async move { crate::server::profile_api::list_profiles().await });
    let mut next_id = use_signal(|| 1u64);
    let mut session_id = use_signal(|| "pending".to_string());
    // Phase 46.9 Plan 03 (D-08/D-09): denominator starts at 0 (pre-resolve
    // placeholder — renders "TOK 0 / 0" for the few ms before the fetch
    // below resolves) and is overwritten with the active model's real
    // context_length once `config_summary` resolves. No hardcoded default literal.
    let mut tokens = use_signal(|| (0u32, 0u32));

    // Phase 46.9 Plan 03 (D-08/D-09): fetch the active model's real
    // context_length ONCE on mount from the existing `get_config_summary`
    // server fn. RESEARCH Open Question 1 (resolved): no web-UI mid-session
    // model-switch affordance exists (`switch_model`/`set_model` grep for
    // `iron_hermes_ui` returns nothing) and config writes require a restart
    // (D-10), so the active model is frozen for the session — a single
    // mount-time fetch is the correct source, not a new `ChatStreamEvent`
    // field on every `ws.rs` Finished emit site.
    let config_summary = use_server_future(crate::server::api::get_config_summary)?;
    use_effect(move || {
        if let Some(Ok(summary)) = config_summary() {
            tokens.with_mut(|t| t.1 = summary.context_length);
        }
    });

    // Phase 26.7.1 Plan 01 — context for ScreenAgents (D-07 / D-08).
    // Phase 26.7.1 Plan 02 — recv loop wires .set() calls on both signals.
    // subagent_events drives push-restart in Plan 02; is_ws_connected drives
    // dynamic poll cadence. Both declared with `let mut` so Plan 02 can call
    // `.set()` from the recv loop. Initial values: counter = 0, connected = false.
    let mut subagent_events = use_signal(|| 0u64);
    let mut is_ws_connected = use_signal(|| false);

    // Phase 36.17.4 (D-03a): queue pill state — (depth, paused). Updated on every
    // QueueUpdated event from the server (Plan 02 protocol variant). Consumed by
    // AppFooter via use_context.
    let mut queue_state = use_signal(|| (0u32, false));

    // Phase 36.17.9 (D-14): voice availability from server VoiceStatus snapshot.
    // Defaults to all-false so pre-connect renders show mic as unavailable.
    let mut voice_status = use_signal(VoiceStatusState::default);

    // Phase 36.17.9 (D-14): voice mode overlay active flag. False by default;
    // set to true by the voice entry button in ScreenChat.
    let mut voice_mode_active = use_signal(|| false);

    // Phase 36.17.9 (D-12, Wave D): wake-word match result signal.
    // Set to true by the WakeWordResult { matched: true } WS arm;
    // voice_loop observes this to transition Armed → Listening for a full turn.
    // Reset to false by voice_loop after it arms the turn.
    let mut wake_word_matched = use_signal(|| false);

    // Phase 36.17.10 (UAT crash fix): wake-word config signals must be provided
    // at the HermesApp root — a common ancestor of BOTH the VoiceSettings panel
    // (which edits them) AND the VoiceModeScreen overlay / start_voice_loop
    // (which read them via use_context). They were previously provided inside
    // VoiceSettings only, so launching hands-free mode from the composer "Free"
    // segment (without the settings panel mounted) panicked with
    // "Could not find context WakeWordPhraseCtx". Single source of truth here.
    let wake_word_enabled = use_signal(|| false);
    let wake_word_phrase = use_signal(|| "hey hermes".to_string());

    // Bootstrap the chat session via the existing server fn from
    // Phase 25.5 (D-02 — no edits to the server file). Mirrors
    // warp_hermes.rs:104-129 adapted for the new bubble shape.
    use_effect(move || {
        spawn(async move {
            match crate::server::api::create_session().await {
                Ok(sid) => {
                    session_id.set(sid);
                }
                Err(e) => {
                    #[cfg(target_arch = "wasm32")]
                    web_sys::console::log_1(&format!("Session creation failed: {e}").into());
                    let _ = e;
                }
            }
        });
    });

    // Open the WebSocket with automatic reconnect — preserves the
    // Phase 26.1 keepalive + close-frame contract byte-for-byte (D-02 /
    // D-18). `ws_chat` itself is unchanged.
    let mut ws = dioxus_fullstack::use_websocket(move || {
        crate::server::ws::ws_chat(
            dioxus_fullstack::WebSocketOptions::new().with_automatic_reconnect(),
        )
    });

    // Receive loop — ports the warp_hermes.rs:139-410 structure adapted
    // for the new ChatBubble shape. Every signal read is dropped before
    // any `.await` (clippy.toml signal-borrow-across-await guard).
    use_future(move || async move {
        loop {
            let _state = ws.connect().await;
            if ws.is_err() {
                streaming_id.set(None);
                // Phase 26.7.1 Plan 02 D-08: ws unavailable — poll cadence stays at 1500 ms baseline.
                is_ws_connected.set(false);
                continue;
            }
            // Phase 26.7.1 Plan 02 D-08: ws is up — poll cadence widens to 5000 ms.
            is_ws_connected.set(true);

            loop {
                match ws.recv_raw().await {
                    Ok(dioxus_fullstack::Message::Text(t)) => {
                        let event: crate::protocol::ChatStreamEvent = match serde_json::from_str(&t)
                        {
                            Ok(e) => e,
                            Err(_) => continue, // Skip malformed frames silently.
                        };
                        match event {
                            crate::protocol::ChatStreamEvent::Delta { text, .. } => {
                                let sid = *streaming_id.read();
                                if let Some(id) = sid {
                                    let mut bs = bubbles.write();
                                    if let Some(b) = bs.iter_mut().find(|b| b.id == id) {
                                        b.text.push_str(&text);
                                    }
                                } else {
                                    let id = {
                                        let n = *next_id.read();
                                        next_id.set(n + 1);
                                        n
                                    };
                                    streaming_id.set(Some(id));
                                    bubbles.write().push(ChatBubble::assistant(id, text));
                                }
                            }
                            crate::protocol::ChatStreamEvent::ToolCallStart {
                                name, args, ..
                            } => {
                                let sid = *streaming_id.read();
                                if let Some(id) = sid {
                                    let mut bs = bubbles.write();
                                    if let Some(b) = bs.iter_mut().find(|b| b.id == id) {
                                        b.tool_rows.push(ToolRow {
                                            name,
                                            args,
                                            done: false,
                                            success: false,
                                        });
                                    }
                                }
                            }
                            crate::protocol::ChatStreamEvent::ToolCallEnd {
                                name, success, ..
                            } => {
                                let sid = *streaming_id.read();
                                if let Some(id) = sid {
                                    let mut bs = bubbles.write();
                                    if let Some(b) = bs.iter_mut().find(|b| b.id == id) {
                                        for row in b.tool_rows.iter_mut().rev() {
                                            if row.name == name && !row.done {
                                                row.done = true;
                                                row.success = success;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            crate::protocol::ChatStreamEvent::Finished { total_tokens, .. } => {
                                // T-46.9-09: clamp the numerator into [0, denominator] so a
                                // compression race (this Finished frame racing the next
                                // denominator write) never renders a numerator above the
                                // real context window. u32 cannot go negative; the risk is
                                // an over-denominator value, not NaN.
                                tokens.with_mut(|t| t.0 = total_tokens.min(t.1));
                                streaming_id.set(None);
                            }
                            crate::protocol::ChatStreamEvent::Error { message, .. } => {
                                let id = {
                                    let n = *next_id.read();
                                    next_id.set(n + 1);
                                    n
                                };
                                bubbles.write().push(ChatBubble::error(id, message));
                                streaming_id.set(None);
                            }
                            crate::protocol::ChatStreamEvent::SubagentEvent {} => {
                                // Phase 26.7.1 Plan 02 D-07: counter-only — read into Copy u64 before set,
                                // satisfies clippy.toml signal-borrow-across-await rule.
                                let cur = *subagent_events.read();
                                subagent_events.set(cur + 1);
                            }
                            // Phase 36.17.4 (D-03a): (u32, bool) is Copy — direct .set call satisfies signal-borrow-across-await clippy rule (no .await in this arm).
                            crate::protocol::ChatStreamEvent::QueueUpdated { depth, paused } => {
                                queue_state.set((depth, paused));
                            }
                            // Phase 36.17.9 (D-14): VoiceStatus snapshot pushed by the server
                            // on connect. All five fields are Copy/Clone — destructured into locals
                            // so no borrow is held across any .await (Pattern B / clippy.toml rule).
                            crate::protocol::ChatStreamEvent::VoiceStatus {
                                stt_available,
                                stt_provider,
                                stt_model,
                                tts_available,
                                tts_provider,
                                ffmpeg_present,
                                silence_duration_secs,
                                web_silence_threshold_rms,
                                speech_confirm_ms,
                                auto_tts,
                            } => {
                                voice_status.set(VoiceStatusState {
                                    stt_available,
                                    stt_provider,
                                    stt_model,
                                    tts_available,
                                    tts_provider,
                                    ffmpeg_present,
                                    silence_duration_secs,
                                    web_silence_threshold_rms,
                                    speech_confirm_ms,
                                    auto_tts,
                                });
                            }
                            // Phase 36.17.7 D-02-a: AudioOut arrives as Message::Binary (see arm below).
                            // This Text-path arm is a silent no-op — if somehow AudioOut arrives as Text,
                            // it is acknowledged here for exhaustive-match compliance only.
                            crate::protocol::ChatStreamEvent::AudioOut { .. } => {}
                            // Phase 01-04 (DLV-03 web): ImageOut arrives as Message::Binary
                            // (see the Binary arm below). This Text-path arm is a silent
                            // no-op for exhaustive-match compliance only.
                            crate::protocol::ChatStreamEvent::ImageOut { .. } => {}
                            // Phase 36.3.3 (D-08 web): VideoOut arrives as Message::Binary
                            // (see the Binary arm below). This Text-path arm is a silent
                            // no-op for exhaustive-match compliance only.
                            crate::protocol::ChatStreamEvent::VideoOut { .. } => {}
                            // Phase 36.17.9 (D-12, Wave D): wake-word STT-polling result.
                            // matched=true → transition Armed state to Listening for a full turn.
                            // matched=false → remain in Armed waiting state (loop idles on).
                            // Signalled via wake_word_matched context signal read by voice_loop.
                            crate::protocol::ChatStreamEvent::WakeWordResult { matched } => {
                                let matched_val = matched;
                                // Read current signal value before set (Pattern B: no borrow across .await).
                                let cur = *wake_word_matched.read();
                                // Only update when the value changes to avoid spurious re-renders.
                                if cur != matched_val {
                                    wake_word_matched.set(matched_val);
                                }
                            }
                            // Phase 36.17.9: voice turns are transcribed server-side and run
                            // there directly, so the client never saw the user's text. The
                            // server echoes it here purely for display — push a user bubble,
                            // mirroring the typed-message path. Do NOT submit (the server
                            // already ran the turn); the assistant reply streams in via the
                            // Delta arm above and lands in its own bubble.
                            crate::protocol::ChatStreamEvent::UserTranscript { text } => {
                                let id = {
                                    let n = *next_id.read();
                                    next_id.set(n + 1);
                                    n
                                };
                                bubbles.write().push(ChatBubble::user(id, text));
                            }
                            // Phase 41.1 Plan 03 (SKILL-13 web / D-06, UI-SPEC §C): DIM
                            // run-turn meta chip for a one-shot skill run. Emitted by the
                            // server BEFORE the run turn's first Delta, so pushing it here
                            // (it does NOT touch streaming_id) lands it ABOVE the assistant
                            // reply bubble the next Delta opens. Metadata only — never a
                            // user/assistant message bubble.
                            crate::protocol::ChatStreamEvent::RunTurnMeta { text } => {
                                let id = {
                                    let n = *next_id.read();
                                    next_id.set(n + 1);
                                    n
                                };
                                bubbles.write().push(ChatBubble::meta(id, text));
                            }
                            // Phase 39.1 Plan 02 (R39.1-08): concurrent turn lifecycle events.
                            // HermesApp does not yet render per-turn concurrency indicators
                            // in the chat surface — future plan will wire these to UI state.
                            // Silent no-ops for exhaustive-match compliance.
                            crate::protocol::ChatStreamEvent::TurnStarted { .. } => {}
                            crate::protocol::ChatStreamEvent::TurnEnded { .. } => {}
                            crate::protocol::ChatStreamEvent::TurnCancelled { .. } => {}
                        }
                    }
                    // Phase 36.17.7 D-02-a/b HIGH 4 + HIGH 7 fix:
                    // ChatStreamEvent::AudioOut arrives as Message::Binary.
                    // The binary frame payload is JSON (uuid + mime + bytes array).
                    // We deserialize, then create a Blob URL via web_sys::Url::create_object_url_with_blob
                    // for first-play. The HTTP /audio/:uuid route ships in Plan 05 for replay only.
                    Ok(dioxus_fullstack::Message::Binary(bytes)) => {
                        let event: crate::protocol::ChatStreamEvent =
                            match serde_json::from_slice(&bytes) {
                                Ok(e) => e,
                                Err(_) => continue,
                            };
                        if let crate::protocol::ChatStreamEvent::AudioOut {
                            mime,
                            uuid: _uuid,
                            bytes: audio_bytes,
                        } = event
                        {
                            #[cfg(target_arch = "wasm32")]
                            {
                                let uint8_array = js_sys::Uint8Array::from(audio_bytes.as_slice());
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
                                        let id = {
                                            let n = *next_id.read();
                                            next_id.set(n + 1);
                                            n
                                        };
                                        // Phase 36.17.9 Plan 03 (D-03 / T-36.17.9-03-03):
                                        // Tap the speaking-state AnalyserNode for orb FFT.
                                        // Construct an HtmlAudioElement from the Blob URL so
                                        // createMediaElementSource can route it through the
                                        // Web Audio graph (source → analyser → destination).
                                        // MUST connect to destination or audio goes silent.
                                        // Guard: VOICE_LOOP_SLOT empty → tap is a no-op.
                                        #[cfg(target_arch = "wasm32")]
                                        if let Ok(audio_el) =
                                            web_sys::HtmlAudioElement::new_with_src(&url)
                                        {
                                            use wasm_bindgen::JsCast as _;
                                            crate::components::hermes_app::voice_loop::tap_speaking_analyser(&audio_el);
                                            // Plan 04 (D-22): set AudioPlaybackActiveCtx = true so
                                            // the voice_loop 'session / 'outer half-duplex gate pauses
                                            // mic capture while TTS audio plays. Cleared on ended/error.
                                            let mut pb_start = audio_playback_active;
                                            pb_start.set(true);
                                            // ended: playback completed normally → resume capture.
                                            let ended_cb = {
                                                let mut pb = audio_playback_active;
                                                wasm_bindgen::closure::Closure::<dyn FnMut()>::wrap(
                                                    Box::new(move || {
                                                        pb.set(false);
                                                    }),
                                                )
                                            };
                                            // error: playback failed → also resume capture.
                                            let error_cb = {
                                                let mut pb = audio_playback_active;
                                                wasm_bindgen::closure::Closure::<dyn FnMut()>::wrap(
                                                    Box::new(move || {
                                                        pb.set(false);
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
                                            // Keep closures alive until they fire (not dropped early).
                                            ended_cb.forget();
                                            error_cb.forget();
                                        }
                                        bubbles.write().push(ChatBubble::audio(id, url, mime));
                                    }
                                }
                            }
                            #[cfg(not(target_arch = "wasm32"))]
                            {
                                // Server-side render path: no Blob API; suppress unused warnings.
                                let _ = (mime, audio_bytes);
                            }
                        }
                        // Phase 01-04 (DLV-03 web): ImageOut arrives as Message::Binary too
                        // (mirrors AudioOut). Build a Blob URL from the bytes and push an
                        // image bubble so the generated image renders inline as an <img> —
                        // never as raw `<MEDIA:>` text (the tag was extracted server-side).
                        else if let crate::protocol::ChatStreamEvent::ImageOut {
                            mime,
                            uuid: _uuid,
                            bytes: image_bytes,
                        } = event
                        {
                            #[cfg(target_arch = "wasm32")]
                            {
                                let uint8_array = js_sys::Uint8Array::from(image_bytes.as_slice());
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
                                        let id = {
                                            let n = *next_id.read();
                                            next_id.set(n + 1);
                                            n
                                        };
                                        bubbles.write().push(ChatBubble::image(id, url));
                                    }
                                }
                            }
                            #[cfg(not(target_arch = "wasm32"))]
                            {
                                // Server-side render path: no Blob API; suppress unused warnings.
                                let _ = (mime, image_bytes);
                            }
                        }
                        // Phase 36.3.3 (D-08 web): VideoOut arrives as Message::Binary too
                        // (mirrors ImageOut). Build a Blob URL from the bytes and push a
                        // video bubble so the generated video renders inline as a
                        // <video controls> — never as raw `<MEDIA:>` text (extracted server-side).
                        // Dioxus reactivity: use .read() (not .peek()) for render-path reads.
                        else if let crate::protocol::ChatStreamEvent::VideoOut {
                            mime,
                            uuid: _uuid,
                            bytes: video_bytes,
                        } = event
                        {
                            #[cfg(target_arch = "wasm32")]
                            {
                                let uint8_array = js_sys::Uint8Array::from(video_bytes.as_slice());
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
                                        let id = {
                                            let n = *next_id.read();
                                            next_id.set(n + 1);
                                            n
                                        };
                                        bubbles.write().push(ChatBubble::video(id, url));
                                    }
                                }
                            }
                            #[cfg(not(target_arch = "wasm32"))]
                            {
                                // Server-side render path: no Blob API; suppress unused warnings.
                                let _ = (mime, video_bytes);
                            }
                        }
                    }
                    Ok(dioxus_fullstack::Message::Close { .. }) => {
                        streaming_id.set(None);
                        is_ws_connected.set(false);
                        break; // Outer loop will reconnect via with_automatic_reconnect.
                    }
                    Err(_) => {
                        is_ws_connected.set(false);
                        break;
                    }
                    _ => continue, // Skip ping/pong.
                }
            }
        }
    });

    // Send-handler — invoked by ScreenChat's input box on Enter (or the
    // submit button). Serialization failure does NOT silently drop — it
    // surfaces as an error bubble so the user always gets feedback (B-05).
    //
    // Plan 26.2.1-11 — client-side slash-command dispatch (GAP-26.2.1-04).
    // Messages beginning with `/` are intercepted here BEFORE the
    // WebSocket send path: `/clear` clears the local bubble list with no
    // server round-trip; unknown slash commands append a single
    // "unknown command" error bubble. Plain messages still flow through
    // `ws.send_raw(ChatRequest)` exactly as before. This amends D-20 from
    // "server-side CommandRouter dispatch" to "client-side dispatch for
    // purely-visual commands; server-side dispatch reserved for future
    // server-effecting commands and requires a follow-up phase that lifts
    // D-02" (the 26.2.1 D-02 server-untouched constraint).
    //
    // Phase 46.7 Plan 05 (D-05/D-06/D-07): the handler now takes a
    // `(text, attachments)` tuple — ChatSendHandler's inner EventHandler
    // type widened in chat.rs to carry the composer's settled
    // pending-attached rows alongside the message text (D-06 sent-bubble
    // render + D-07 attachment-only send).
    let send = EventHandler::new(
        move |(text, attachments): (String, Vec<crate::protocol::ChatAttachmentRow>)| {
            let trimmed = text.trim().to_string();
            // D-07: an attachment-only message (empty text, non-empty
            // attachments) is a valid turn — only bail when BOTH are empty.
            if trimmed.is_empty() && attachments.is_empty() {
                return;
            }
            if trimmed.starts_with('/') && dispatch_slash(&trimmed, &mut bubbles, &mut next_id) {
                return;
            }
            let sid = session_id.read().clone();
            let id = {
                let n = *next_id.read();
                next_id.set(n + 1);
                n
            };
            bubbles.write().push(ChatBubble::user_with_attachments(
                id,
                trimmed.clone(),
                attachments.clone(),
            ));
            // Phase 50.2 Plan 07 (D-20): `@mention` handoff detection — the
            // SINGLE wiring point `ChatSendHandler`'s own doc comment
            // names, before the outbound text goes to the WebSocket. A
            // pending block is appended for EACH target, then every
            // target is dispatched in PARALLEL (2026-08-19 operator
            // ruling), independently of the live turn's own WS round trip.
            let roster_names: Vec<String> = roster_names_resource()
                .and_then(|r| r.ok())
                .map(|rows| rows.into_iter().map(|p| p.name).collect())
                .unwrap_or_default();
            let mention_targets = crate::server::mention_handoff_api::resolve_roster_mentions_client(
                &trimmed,
                &roster_names,
                "operator",
            );
            for target in &mention_targets {
                let mention_id = {
                    let n = *next_id.read();
                    next_id.set(n + 1);
                    n
                };
                bubbles.write().push(ChatBubble::mention_handoff(
                    mention_id,
                    target.clone(),
                    trimmed.clone(),
                    crate::protocol::MentionHandoffState::Pending,
                ));
            }
            for target in mention_targets {
                let message = trimmed.clone();
                let mut bubbles_for_mention = bubbles;
                spawn(async move {
                    let outcome = crate::server::mention_handoff_api::dispatch_mention_handoff(
                        crate::protocol::MentionHandoffRequest {
                            target: target.clone(),
                            message,
                        },
                    )
                    .await;
                    let mut list = bubbles_for_mention.write();
                    if let Some(bubble) = list.iter_mut().rev().find(|b| {
                        b.kind
                            == crate::components::hermes_app::screens::chat::ChatBubbleKind::MentionHandoff
                            && b.mention_target.as_deref() == Some(target.as_str())
                            && matches!(
                                b.mention_state,
                                Some(crate::protocol::MentionHandoffState::Pending)
                            )
                    }) {
                        match outcome {
                            Ok(res) => {
                                bubble.mention_state =
                                    Some(crate::protocol::MentionHandoffState::Resolved);
                                bubble.mention_reply = Some(res.reply);
                            }
                            Err(e) => {
                                bubble.mention_state = Some(
                                    crate::protocol::MentionHandoffState::Failed {
                                        reason: e.to_string(),
                                    },
                                );
                            }
                        }
                    }
                });
            }
            // Phase 40.5 Plan 08 (D-17): read active identity (Pattern B — owned local,
            // borrow dropped before the spawn so no GenerationalRef crosses async).
            let active_ident: Option<String> = {
                let slug = avatar_prefs.read().active_identity.clone();
                if crate::components::hermes_app::avatar_logic::is_known_identity(&slug) {
                    Some(slug)
                } else {
                    None
                }
            };
            let req = crate::protocol::ChatRequest {
                session_id: sid,
                message: trimmed,
                active_identity: active_ident,
                // Phase 46.7 Plan 05 (D-05/D-06/D-07): the composer's settled,
                // queued-to-send attachment ids — wired end-to-end.
                attachment_ids: attachments.iter().map(|a| a.id.clone()).collect(),
            };
            // Let-else early-return on serialization failure — no silent
            // unwrap_or_default(); surface the failure as an error bubble.
            let Ok(json) = serde_json::to_string(&req) else {
                let err_id = {
                    let n = *next_id.read();
                    next_id.set(n + 1);
                    n
                };
                bubbles.write().push(ChatBubble::error(
                    err_id,
                    "Failed to send message: serialization error".to_string(),
                ));
                return;
            };
            spawn(async move {
                let _ = ws.send_raw(dioxus_fullstack::Message::Text(json)).await;
            });
        },
    );
    let send_handler = ChatSendHandler(send);

    // Phase 36.17.8 Plan 06 (D-13/D-14): audio binary send handler.
    // Receives the JSON-encoded AudioInFrame bytes from MicButton and sends
    // them as Message::Binary to the server's STT pipeline.
    let audio_send = EventHandler::new(move |bytes: Vec<u8>| {
        spawn(async move {
            let _ = ws
                .send_raw(dioxus_fullstack::Message::Binary(bytes.into()))
                .await;
        });
    });
    let audio_send_handler = mic_button::AudioSendHandler(audio_send);

    // Plan 06 context providers — five new signals reachable from any
    // descendant screen. `session_id` is wrapped in the B-03
    // `SessionIdContext` newtype so it does not collide with Plan 03's
    // `ThemeContext(theme)` (also `Signal<String>`).
    use_context_provider(|| bubbles);
    use_context_provider(|| streaming_id);
    use_context_provider(|| SessionIdContext(session_id));
    use_context_provider(|| tokens);
    use_context_provider(|| send_handler);
    use_context_provider(|| audio_send_handler);
    // Phase 26.7.2 (D-06): next_id exposed via context so ScreenChat's
    // history-load use_effect can allocate bubble IDs that don't collide
    // with IDs already assigned by the WS receive loop.
    // Newtype-wrapped per the freeze post-mortem (see state.rs): next_id,
    // subagent_events, is_ws_connected, voice_mode_active, and wake_word_matched
    // are all `Signal<bool>`/`Signal<u64>` and MUST be disambiguated by type or
    // `use_context` resolves to whichever was registered last.
    use_context_provider(|| crate::state::NextIdContext(next_id));
    // Phase 26.7.1 Plan 01 — context for ScreenAgents (D-07 / D-08). subagent_events drives push-restart in Plan 02; is_ws_connected drives dynamic poll cadence.
    use_context_provider(|| crate::state::SubagentEventsContext(subagent_events));
    use_context_provider(|| crate::state::WsConnectedContext(is_ws_connected));
    // Phase 36.17.4 (D-03a): expose queue_state to AppFooter via context.
    use_context_provider(|| queue_state);

    // Phase 36.17.9 (D-14): expose voice availability + mode flag to all descendants.
    // VoiceStatusState is consumed by: ScreenChat (stt_available → MicButton),
    // VoiceModeScreen, VoiceSettings. voice_mode_active drives the overlay render.
    use_context_provider(|| voice_status);
    use_context_provider(|| crate::state::VoiceModeActiveContext(voice_mode_active));

    // Phase 36.17.9 (D-12, Wave D): expose wake-word match result to voice_loop.
    // voice_loop polls this via use_context to transition Armed → Listening on match.
    use_context_provider(|| crate::state::WakeWordMatchedContext(wake_word_matched));

    // Phase 36.17.10 (UAT crash fix): provide wake-word config at the root so the
    // hands-free overlay (VoiceModeScreen / start_voice_loop) and the VoiceSettings
    // panel share one source of truth. VoiceSettings consumes these via use_context.
    use_context_provider(|| voice_settings::WakeWordEnabledCtx(wake_word_enabled));
    use_context_provider(|| voice_settings::WakeWordPhraseCtx(wake_word_phrase));

    // Phase 40.2 Plan 04 (FE-01, D-03): provide avatar contexts at HermesApp root.
    // AvatarModeCtx wraps the avatar_prefs signal so VoiceSettings (toggle/dropdown)
    // and orb_canvas (init/swap) share one source of truth. MUST be here — a child
    // provider panics ancestor/sibling consumers (Dioxus context-panic rule / MEMORY.md).
    // AvatarErrorNoticeCtx is the per-session orb-restore notice flag (FE-05/D-11).
    use_context_provider(|| voice_settings::AvatarModeCtx(avatar_prefs));
    use_context_provider(|| voice_settings::AvatarErrorNoticeCtx(avatar_error_notice));

    // Phase 36.17.12 Plan 04 (CR-01 gap closure): provide barge-in mode + realtime-
    // degraded flag at the root so the VoiceModeScreen overlay and the VoiceSettings
    // panel share one source of truth. VoiceSettings consumes these via use_context.
    // Previously they were only provided inside VoiceSettings (a child of
    // VoiceModeScreen), which caused a Dioxus "Could not find context BargeInModeCtx"
    // panic on every voice-mode entry — identical to the prior WakeWordPhraseCtx panic
    // fixed above. Initialized to the same defaults VoiceSettings used previously.
    let barge_in_mode = use_signal(|| "push_to_interrupt".to_string());
    let realtime_degraded = use_signal(|| false);
    use_context_provider(|| voice_settings::BargeInModeCtx(barge_in_mode));
    use_context_provider(|| voice_settings::RealtimeDegradedCtx(realtime_degraded));

    // Phase 39.3 Plan 05 (D-03/D-05a): approval-pending + in-flight contexts for the
    // realtime voice card (Plan 05). Hoisted to HermesApp root — MUST live here so
    // VoiceModeScreen (consumer) resolves them without "could not find context" panic
    // (Dioxus context-panic rule per MEMORY.md: child-provided contexts panic
    // ancestor/sibling consumers; only root providers are safe for cross-tree consumers).
    //
    // RealtimeApprovalCtx: Some(ApprovalPendingInfo{call_id, turn_id, tool_name,
    //   arguments}) while a tool call awaits operator Approve/Deny; None at rest.
    //   Plan 04 raises it on ApprovalPending; Plan 05 renders the approval card from it.
    // RealtimeInFlightCtx: true while a background async tool/research turn is running;
    //   false at rest. Plan 04 sets it on relay-start/clear; Plan 05 renders the badge.
    let approval_pending_sig: Signal<
        Option<crate::components::hermes_app::realtime_session::ApprovalPendingInfo>,
    > = use_signal(|| None);
    let in_flight_sig: Signal<bool> = use_signal(|| false);
    use_context_provider(|| voice_settings::RealtimeApprovalCtx(approval_pending_sig));
    use_context_provider(|| voice_settings::RealtimeInFlightCtx(in_flight_sig));

    // Phase 46.6 Plan 05 (D-07): selected-artifact signal for the
    // Artifacts gallery → viewer row-click navigation. MUST live at the
    // HermesApp root (not inside ScreenArtifacts) — a child provider
    // would panic the sibling ArtifactViewer consumer (Dioxus
    // context-panic rule / MEMORY.md: child providers panic
    // ancestor/sibling consumers).
    let selected_artifact_sig: Signal<Option<crate::server::api::ArtifactInfo>> =
        use_signal(|| None);
    use_context_provider(|| crate::state::SelectedArtifactCtx(selected_artifact_sig));

    // Phase 47.3 Plan 06 (D-17): AuthedContext MUST live at the HermesApp
    // root — ScreenChat (the D-17 expiry-redirect + /logout consumer) is not
    // in an ancestor/descendant relationship with any earlier provider that
    // could otherwise host it, and a child-level provider panics sibling
    // consumers (Dioxus context-panic rule, see MEMORY.md).
    use_context_provider(|| AuthedContext(authed));

    // Phase 40.5 Plan 01 (D-04, D-05, D-07, D-09, D-10): orb customisation + per-identity
    // voice contexts. Provided at HermesApp root (Dioxus context-panic rule: child providers
    // panic ancestor/sibling consumers — see MEMORY.md). Signals declared early in this
    // function so the hydration use_effect could seed orb appearance from ORB_PRESET_REGISTRY.
    use_context_provider(|| voice_settings::VoiceEditTargetCtx(voice_edit_target));
    use_context_provider(|| voice_settings::IdentityTtsProviderCtx(identity_tts_provider));
    use_context_provider(|| voice_settings::IdentityTtsVoiceCtx(identity_tts_voice));
    use_context_provider(|| voice_settings::IdentityRealtimeVoiceCtx(identity_realtime_voice));
    use_context_provider(|| voice_settings::OrbStyleCtx(orb_style));
    use_context_provider(|| voice_settings::OrbBaseHueCtx(orb_base_hue));
    use_context_provider(|| voice_settings::OrbSizeCtx(orb_size));
    use_context_provider(|| voice_settings::OrbGlowCtx(orb_glow));
    // Phase 41.2 Plan 01 (D-03): must live at HermesApp root (Dioxus
    // context-panic rule) — the orb_canvas.rs bridge (writer) and
    // OrbAppearanceSection (reader) are not in an ancestor/descendant
    // relationship, so a child-level provider would panic one of them.
    use_context_provider(|| voice_settings::OrbSettlingCtx(orb_settling));
    use_context_provider(|| voice_settings::WakeSessionActiveCtx(wake_session_active));
    use_context_provider(|| voice_settings::WakeSessionStopCtx(wake_session_stop));
    use_context_provider(|| voice_settings::AudioPlaybackActiveCtx(audio_playback_active));
    use_context_provider(|| voice_settings::BeepEnabledCtx(beep_enabled));

    // Phase 36.17.9 (D-06): read voice signals into locals before rsx!
    // Pattern B -- no signal borrow held across the rsx! tree.
    let voice_active = *voice_mode_active.read();

    // Phase 46.9 Plan 03 (D-07/D-09), extended Plan 10 (GAP-2/GAP-3): drop the
    // config_summary resource into owned locals before the render tree so
    // `SysMeta` renders the real active-model id, active provider, and real
    // server uptime — the SAME `get_config_summary` fetch that seeds the
    // tokens denominator above (single source, D-09).
    let (active_model, active_provider, server_uptime_secs) = match config_summary() {
        Some(Ok(summary)) => (summary.model, summary.provider, summary.uptime_secs),
        _ => ("…".to_string(), "…".to_string(), 0u64),
    };

    // Phase 46.9 Plan 10 (GAP-2/D-07): client-side 1Hz uptime ticker, seeded
    // once from the mount-time `server_uptime_secs` value above so the web
    // "UP" value advances live instead of freezing until reload. Mirrors
    // `app_footer.rs`'s clock ticker (`use_future` + wasm/native cfg split,
    // borrow-safe: no signal borrow across `.await`). HermesApp never
    // unmounts, so the loop never leaks (T-46.9-25).
    let mut uptime_ticker = use_signal(|| server_uptime_secs);
    use_future(move || async move {
        loop {
            #[cfg(target_arch = "wasm32")]
            {
                gloo_timers::future::TimeoutFuture::new(1000).await;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            }
            uptime_ticker.with_mut(|u| *u += 1);
        }
    });
    let ticking_uptime_secs = *uptime_ticker.read();

    rsx! {
        hud_chrome::HudChrome {}
        breadcrumb::Breadcrumb {}
        div { class: "app", id: "app",
            // GAP-11 (47.4-16 Task 3): SysMeta now lives inside `div.app`
            // rather than as its sibling. `.app` (screens.css) is
            // `position: relative` with a non-`auto` z-index, which makes
            // it a stacking context — a sibling readout can never be
            // layered against anything inside that context no matter what
            // z-index it carries. That's why a `position: fixed` drawer
            // (z-index: 200, inside `.app`) was painting UNDER `.sys-meta`
            // (z-index: 70, a root sibling) despite the numbers saying
            // otherwise. Reparenting puts both in one stacking context so
            // the authored layer values finally resolve as intended.
            sys_meta::SysMeta {
                model: active_model,
                provider: active_provider,
                uptime_secs: ticking_uptime_secs,
            }
            screen_router::ScreenRouter {}
        }
        app_footer::AppFooter {}
        wheel_rail::WheelRail {}
        wheel::Wheel {}
        theme_effects::ThemeEffects {}
        tweaks_panel::TweaksPanel {}

        // Phase 36.17.10 UAT: the global floating voice-entry-btn was removed —
        // hands-free voice mode is now launched from the composer action oval's
        // "✦ FREE" segment (screens/chat.rs), which performs the same
        // AudioContext-resume user-gesture unlock before setting voice_mode_active.

        // Phase 36.17.9 (D-04): Full-screen voice-mode overlay.
        // Rendered on top of the entire app tree when voice_mode_active is true.
        if voice_active {
            screens::voice_mode::VoiceModeScreen {
                on_exit: move |_| voice_mode_active.set(false),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Plan 26.2.1-11 — Client-side slash-command dispatch (GAP-26.2.1-04).
// ---------------------------------------------------------------------------
//
// `/clear` empties the local bubble list with no LLM round-trip. Unknown
// slash commands append a single error-styled bubble locally. Plain
// (non-slash) messages still flow through the existing WebSocket path in
// the `send` EventHandler closure — this helper is only reached after the
// `text.starts_with('/')` early-return branch.
//
// Signals are passed by `&mut` even though they implement `Copy` in Dioxus
// 0.7; this preserves the call-site spelling `&mut bubbles` / `&mut next_id`
// for clarity at the only call site, and `.write()` / `.set()` take `&self`
// internally so this compiles cleanly under both default and `legacy-shell`
// features.
// Called from HermesApp's `send` EventHandler (mod.rs:347). In a binary crate an
// unreferenced `pub fn` does not suppress dead_code, so when `--all-features`
// enables `legacy-shell` (app.rs mounts WarpHermes, leaving HermesApp and this
// helper unreferenced) dead_code fires. Live in the default build.
#[allow(dead_code)]
fn dispatch_slash(
    cmd: &str,
    bubbles: &mut Signal<Vec<ChatBubble>>,
    _next_id: &mut Signal<u64>,
) -> bool {
    let mut parts = cmd.trim().splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("");
    match head {
        "/clear" => {
            bubbles.write().clear();
            true
        }
        // Per D-26.2.1-13-B: unknown slashes are NOT handled
        // client-side. Fall through to ws.send_raw so the server
        // (or LLM) routes them. /research weather, /help (future),
        // /status (future) all reach the backend via this path.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    //! Plan 26.2.1-11 — static-grep regression guards for
    //! GAP-26.2.1-04 (client-side slash-command interception).
    //!
    //! Dioxus 0.7 `Signal<T>` is component-scoped; constructing one outside
    //! a component requires a `dioxus::prelude::ScopeId` and a live
    //! VirtualDom, which would substantially expand this crate's test
    //! footprint. The crate's existing test bootstrap (see e.g.
    //! `wheel.rs` tests) does not yet construct signals outside a
    //! component context. Per the plan: "If the existing test bootstrap
    //! pattern doesn't easily support signals outside a component …
    //! skip the unit test and rely on a static-grep regression test".
    //! That is the choice made here.
    const MOD_RS: &str = include_str!("mod.rs");

    // GAP-26.2.1-07-R3 / GAP-26.2.1-09-R3 — Plan 14 regression sources.
    const SITE_CSS: &str = include_str!("../../../assets/site.css");
    const API_RS: &str = include_str!("../../server/api.rs");
    const HUD_CHROME_RS: &str = include_str!("hud_chrome.rs");
    const UI_PREFS_RS: &str = include_str!("../../ui_prefs.rs");

    #[test]
    fn dispatch_slash_helper_exists() {
        assert!(
            MOD_RS.contains("fn dispatch_slash"),
            "dispatch_slash helper must exist in mod.rs (GAP-26.2.1-04 regression)",
        );
        assert!(
            MOD_RS.contains(") -> bool"),
            "dispatch_slash must return bool per GAP-26.2.1-08 (D-26.2.1-13-B)",
        );
    }

    #[test]
    fn send_eventhandler_intercepts_slash_prefix_before_websocket() {
        assert!(
            MOD_RS.contains("trimmed.starts_with('/') && dispatch_slash(&trimmed, &mut bubbles, &mut next_id)"),
            "send EventHandler must combine starts_with('/') with dispatch_slash bool return per GAP-26.2.1-08",
        );
    }

    #[test]
    fn clear_arm_empties_bubble_list_locally() {
        assert!(
            MOD_RS.contains("\"/clear\""),
            "dispatch_slash must match on the literal `/clear` head",
        );
        assert!(
            MOD_RS.contains("bubbles.write().clear()"),
            "the `/clear` arm must empty the bubble list locally",
        );
    }

    #[test]
    fn unknown_command_arm_falls_through_to_websocket() {
        // Per GAP-26.2.1-08 (D-26.2.1-13-B): unknown slashes must NOT
        // be handled client-side. The `_` arm returns false so the
        // call site falls through to ws.send_raw(ChatRequest).
        //
        // Note: we check for the specific `format!("unknown command:` string
        // rather than a ChatBubble::error call because other error paths in
        // the send handler legitimately use ChatBubble::error.
        // The old dispatch_slash code built a local error message whose
        // format string began with the two words below (split here to avoid
        // the include_str! self-match):
        let pat = ["unknown", " command: {head}"].concat();
        assert!(
            !MOD_RS.contains(&pat),
            "GAP-26.2.1-08: dispatch_slash must not build a local error message for unhandled slashes",
        );
        // Positive assertion: the `_` arm must yield `false` so the
        // call site short-circuits and falls through.
        assert!(
            MOD_RS.contains("_ => false"),
            "GAP-26.2.1-08: unknown-slash arm must return false to enable fall-through to ws.send_raw",
        );
    }

    #[test]
    fn slash_branch_precedes_websocket_send_path() {
        // The early-return slash branch MUST appear before `let sid =
        // session_id.read().clone();` — otherwise a `/clear` would
        // re-emit a user bubble + WS frame before the helper runs.
        let idx_slash = MOD_RS
            .find("trimmed.starts_with('/')")
            .expect("starts_with('/') branch must be present");
        let idx_sid = MOD_RS
            .find("let sid = session_id.read().clone();")
            .expect("WebSocket-path `let sid` must still be present");
        assert!(
            idx_slash < idx_sid,
            "slash-prefix branch must come BEFORE the WebSocket path so /clear never round-trips",
        );
    }

    #[test]
    fn scanlines_feature_is_fully_removed() {
        // GAP-26.2.1-07-R3-FEATURE-REMOVAL regression (Plan 15):
        // After 4 failed attempts to make the SCANLINES toggle engage (Plans
        // 10, 13, 14 Branch (c), and the Plan 14 round-3 UAT confirmed the
        // triple-guard CSS still did not hide the overlay), the user decision
        // 2026-05-14 was to remove the scanlines feature entirely. This test
        // guards against accidental re-introduction.
        //
        // Round-5 (2026-05-14): user reported the scrolling-line effect was
        // still visible after Plan 15 shipped. Root cause: the visible
        // animation lived under a synonym — `.scan-bar` (a 2px teal line
        // animating translateY(0vh → 100vh) over 7s, infinite loop) added
        // by Plan 26.2.1-03's HudChrome composer. The original 3-surface
        // guard never considered the synonym. Scope expanded here to also
        // forbid `.scan-bar` and its `scan-bar-move` keyframes.

        // (1) site.css — no `.scanlines` selectors, no `no-scanlines` body class
        assert!(
            !SITE_CSS.contains(".scanlines"),
            "GAP-26.2.1-07-R3-FEATURE-REMOVAL: site.css must NOT contain `.scanlines` selector",
        );
        assert!(
            !SITE_CSS.contains("no-scanlines"),
            "GAP-26.2.1-07-R3-FEATURE-REMOVAL: site.css must NOT contain `no-scanlines` selector",
        );

        // (2) hud_chrome.rs — no `class: \"scanlines\"` RSX element
        assert!(
            !HUD_CHROME_RS.contains("class: \"scanlines\""),
            "GAP-26.2.1-07-R3-FEATURE-REMOVAL: hud_chrome.rs must NOT render `class: \"scanlines\"`",
        );

        // (3) ui_prefs.rs — no `pub scanlines: bool` struct field
        assert!(
            !UI_PREFS_RS.contains("pub scanlines: bool"),
            "GAP-26.2.1-07-R3-FEATURE-REMOVAL: ui_prefs.rs must NOT contain `pub scanlines: bool` field",
        );

        // (4) Round-5 synonym closure — no `.scan-bar` selector or
        //     `scan-bar-move` keyframes in site.css; no `class: \"scan-bar\"`
        //     RSX in hud_chrome.rs. `.scan-bar` is the renamed scrolling-line
        //     overlay that produced the same visual effect as `.scanlines`.
        assert!(
            !SITE_CSS.contains(".scan-bar"),
            "GAP-26.2.1-07-R3-FEATURE-REMOVAL (round-5): site.css must NOT contain `.scan-bar` selector",
        );
        assert!(
            !SITE_CSS.contains("scan-bar-move"),
            "GAP-26.2.1-07-R3-FEATURE-REMOVAL (round-5): site.css must NOT contain `scan-bar-move` keyframes",
        );
        assert!(
            !HUD_CHROME_RS.contains("class: \"scan-bar\""),
            "GAP-26.2.1-07-R3-FEATURE-REMOVAL (round-5): hud_chrome.rs must NOT render `class: \"scan-bar\"`",
        );
    }

    #[test]
    fn list_sessions_filters_by_message_count_in_api_rs() {
        // GAP-26.2.1-09-R3 regression: list_sessions must filter the Vec<Session>
        // by message_count > 0 before mapping to SessionInfo, otherwise foreign-
        // format directories (only trajectories.jsonl) leak into the SESSIONS
        // wedge and produce dead row-clicks.
        assert!(
            API_RS.contains(".filter(|s| s.message_count > 0)"),
            "GAP-26.2.1-09-R3: api.rs list_sessions must include `.filter(|s| s.message_count > 0)` per D-26.2.1-14-B",
        );
        assert!(
            API_RS.contains("GAP-26.2.1-09-R3"),
            "GAP-26.2.1-09-R3: api.rs must cite the gap ID in the filter's explanatory comment",
        );
    }
}
