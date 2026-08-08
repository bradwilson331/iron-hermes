//! Phase 36.17.9 Plan 03 (Wave C) — OrbCanvas Dioxus component.
//!
//! Renders the `<canvas id="orb-canvas">` element and wires the JS bridge:
//!
//! - On mount: waits 50 ms for Dioxus to flush the canvas to the DOM (Pitfall 3),
//!   then calls `window.<global>.init('orb-canvas')` (orb) or
//!   `window.<global>.init('orb-canvas', glbUrl, preset)` (avatar) via `document::eval`.
//! - FFT pump: one long-lived `document::eval` handle running a JS
//!   `while(true){ let bins = await dioxus.recv(); window.<global>.updateFFT(bins); }`
//!   loop. The Rust side pushes FFT bins via `eval.send()` — never creates a new
//!   eval handle per frame (RESEARCH Pitfall 5).
//! - setState: called with the current `VoiceModeState` string whenever the state
//!   signal changes.
//! - On unmount: `use_drop` calls `window.<global>.destroy()`.
//! - WebGL fallback: if WebGL is unavailable the component renders a text
//!   notification (UI-SPEC: "3D orb unavailable in this browser.").
//!
//! # Phase 40.2 Plan 04 — Runtime global selection (FE-02, REND-02)
//!
//! The compile-time `select_avatar_global(cfg!(feature = "avatar"))` is retired.
//! All four eval sites (init/pump/setState/destroy) now read `AvatarModeCtx` at
//! runtime so the scene swaps live in the shared canvas with no page reload.
//!
//! # WASM gating
//! All browser API calls live inside `#[cfg(target_arch = "wasm32")]` blocks.
//! The non-wasm path (server SSR) renders the canvas RSX and is a no-op for JS.

use dioxus::core::use_drop;
use dioxus::prelude::*;

use crate::components::hermes_app::screens::voice_mode::VoiceModeState;
// Phase 40.2 Plan 04: runtime context replaces compile-time select_avatar_global.
#[cfg(target_arch = "wasm32")]
use crate::components::hermes_app::avatar_logic::{AVATAR_GLOBAL, ORB_GLOBAL, PRESET_REGISTRY};
use crate::components::hermes_app::voice_settings::{
    AvatarErrorNoticeCtx, AvatarModeCtx, OrbBaseHueCtx, OrbGlowCtx, OrbSettlingCtx, OrbSizeCtx,
    OrbStyleCtx,
};

// ── thread_local eval slots ───────────────────────────────────────────────────
// FFT_EVAL_SLOT: long-lived pump handle (Rust → JS direction).
// AVATAR_ERROR_EVAL_SLOT: error-poll handle (JS → Rust direction via dioxus.send()).
// Both mirror the RECORDER_SLOT pattern in mic_button.rs.

#[cfg(target_arch = "wasm32")]
thread_local! {
    static FFT_EVAL_SLOT: std::cell::RefCell<Option<dioxus::prelude::document::Eval>> =
        std::cell::RefCell::new(None);
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// Phase 40.2 Plan 04 (FE-05): error-poll eval handle — distinct from FFT_EVAL_SLOT.
    /// JS polls `window.__ihAvatarError` every 500 ms; on error it sends via dioxus.send()
    /// then breaks (Pitfall 7: send before break avoids Err(Finished)).
    static AVATAR_ERROR_EVAL_SLOT: std::cell::RefCell<Option<dioxus::prelude::document::Eval>> =
        std::cell::RefCell::new(None);
}

// ── Component ─────────────────────────────────────────────────────────────────

/// OrbCanvas — mounts the Three.js orb canvas and wires the FFT eval bridge.
///
/// Props:
/// - `state`: current `VoiceModeState` — drives `window.<global>.setState`.
/// - `fft_bins`: current 64-element FFT bin array (byte domain 0-255) — pumped
///   to JS each frame via the long-lived eval handle.
#[component]
pub fn OrbCanvas(state: VoiceModeState, fft_bins: Vec<u8>) -> Element {
    // ── WebGL availability check (non-wasm always renders canvas) ────────────
    // Runs exactly once per component instance (use_hook caches the returned
    // bool in the hook arena and returns the stored value on every subsequent
    // render). The probe context is released immediately via WEBGL_lose_context
    // so it does not count against the browser's ~16 live-context cap.
    #[cfg(target_arch = "wasm32")]
    let webgl_ok = use_hook(|| {
        use wasm_bindgen::JsCast;
        let doc = web_sys::window().and_then(|w| w.document());
        let test_canvas = doc
            .as_ref()
            .and_then(|d| d.create_element("canvas").ok())
            .and_then(|el| el.dyn_into::<web_sys::HtmlCanvasElement>().ok());
        let ctx_obj = test_canvas.as_ref().and_then(|c| {
            c.get_context("webgl2")
                .ok()
                .flatten()
                .or_else(|| c.get_context("webgl").ok().flatten())
        });
        let available = ctx_obj.is_some();
        // Release the probe context immediately to avoid exhausting the
        // browser's WebGL context limit (~16 contexts). We call
        // ctx.getExtension("WEBGL_lose_context") and then ext.loseContext()
        // via js_sys::Reflect so we do not need the WebGl2RenderingContext
        // web-sys feature (which is not enabled for this crate).
        if let Some(obj) = ctx_obj {
            let get_ext_key = wasm_bindgen::JsValue::from_str("getExtension");
            let ext_name = wasm_bindgen::JsValue::from_str("WEBGL_lose_context");
            if let Ok(get_ext_fn) = js_sys::Reflect::get(&obj, &get_ext_key) {
                if let Ok(f) = get_ext_fn.dyn_into::<js_sys::Function>() {
                    if let Ok(ext) = f.call1(&obj, &ext_name) {
                        if ext.is_truthy() {
                            let lose_key = wasm_bindgen::JsValue::from_str("loseContext");
                            if let Ok(lose_fn) = js_sys::Reflect::get(&ext, &lose_key) {
                                if let Ok(lf) = lose_fn.dyn_into::<js_sys::Function>() {
                                    let _ = lf.call0(&ext);
                                }
                            }
                        }
                    }
                }
            }
        }
        available
    });

    #[cfg(not(target_arch = "wasm32"))]
    let webgl_ok = true;

    // ── Phase 40.2 Plan 04: read AvatarModeCtx + AvatarErrorNoticeCtx ───────
    // Both are provided at HermesApp root (mod.rs). Consume here by use_context.
    let avatar_ctx = use_context::<AvatarModeCtx>();
    // mut + usage are wasm32-only (set inside cfg blocks); allow on native.
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut, unused_variables))]
    let mut avatar_error_ctx = use_context::<AvatarErrorNoticeCtx>();

    // ── Phase 41.2 (G-41.2-11): orb appearance seeds for born-correct init ────
    // Consumed (via .peek(), NOT .read()) inside the init effect so the orb is
    // born with the persisted Style/hue/size/glow. The init effect must NOT
    // subscribe to these — subscribing would re-init the shared WebGL canvas on
    // every slider edit and white-lock the orb (see avatar_scene_key). Live edits
    // after mount flow through the setStyle/setBaseHue/setSize/setGlow bridges
    // below instead. These context values are provided at the HermesApp root.
    let orb_style_seed = use_context::<OrbStyleCtx>().0;
    let orb_hue_seed = use_context::<OrbBaseHueCtx>().0;
    let orb_size_seed = use_context::<OrbSizeCtx>().0;
    let orb_glow_seed = use_context::<OrbGlowCtx>().0;

    // ── Phase 41.2 gap-fix: scene-relevant subscription (Set-as-active crash) ─
    // The init effect below re-inits the WebGL scene on every change it reads.
    // It must depend ONLY on fields that change what is *rendered* — `enabled`
    // (orb vs avatar) and `head_id` (which avatar) — NOT on `active_identity`,
    // which is a voice-routing pointer with no bearing on the rendered scene.
    // Re-initing runs destroy() → renderer.forceContextLoss() then rebuilds on
    // the SAME <canvas id="orb-canvas">, whose force-lost WebGL context cannot
    // be reacquired → getShaderPrecisionFormat() returns null →
    // `TypeError: Cannot read properties of null` in getMaxPrecision → the orb
    // white-locks. Subscribing to a value-compared memo (recomputes on any
    // AvatarPrefs change but only NOTIFIES when this tuple changes) instead of
    // the whole AvatarPrefs makes an active_identity write a no-op for this
    // effect. The Set-as-active control added in Phase 41.2 Plan 03 is the
    // writer that surfaced this.
    let avatar_scene_key = use_memo(move || {
        let p = avatar_ctx.0.read();
        (p.enabled, p.head_id.clone())
    });

    // ── Init effect: 50 ms delay then call <global>.init ────────────────────
    //
    // Phase 40.2 Plan 04 (FE-02, REND-02): Runtime branch replaces the two
    // compile-time #[cfg(feature = "avatar")] / #[cfg(not(...))] branches.
    // Pattern B: read AvatarModeCtx into owned locals BEFORE the first .await
    // so no GenerationalRef is held across the await point (clippy.toml rule).
    use_effect(move || {
        // Pattern B: owned locals, borrow dropped at `;` before any .await.
        // Subscribe ONLY to the scene-relevant memo (enabled, head_id) so an
        // active_identity write does NOT re-init the WebGL scene and white-lock
        // the orb (Phase 41.2 Set-as-active gap-fix — see avatar_scene_key).
        let (enabled, head_id) = avatar_scene_key();
        // Peek (NON-subscribing) the persisted appearance so the orb is born on
        // the saved look. .peek() is required here — .read() would subscribe and
        // re-init the WebGL scene on every appearance edit (white-lock). Read into
        // owned locals up front (Pattern B): no signal borrow crosses the .await.
        let seed_style = orb_style_seed.peek().clone();
        let seed_hue = *orb_hue_seed.peek();
        let seed_size = *orb_size_seed.peek();
        let seed_glow = *orb_glow_seed.peek();

        spawn(async move {
            #[cfg(target_arch = "wasm32")]
            {
                let global = if enabled { AVATAR_GLOBAL } else { ORB_GLOBAL };

                // Exclusive init (UAT fix — "two avatars"): tear down BOTH globals
                // before building so no prior scene lingers in the shared
                // <canvas id="orb-canvas">. This single effect re-runs on mount,
                // on hydration, and on every enabled/head_id change (it subscribes
                // to AvatarModeCtx), so destroying both first guarantees exactly
                // one scene. forceContextLoss in each destroy() + the 50ms flush
                // keep the WebGL swap clean (FE-02, no frozen frame). This replaces
                // the former separate swap-on-change effect (which double-initialised).
                let destroy_both = format!(
                    "window.{AVATAR_GLOBAL} && window.{AVATAR_GLOBAL}.destroy(); \
                     window.{ORB_GLOBAL} && window.{ORB_GLOBAL}.destroy();"
                );
                let _ = document::eval(&destroy_both).await;
                gloo_timers::future::sleep(std::time::Duration::from_millis(50)).await;

                let init_script = if enabled {
                    // Resolve preset from PRESET_REGISTRY (fallback to [0] = facecap).
                    let preset = PRESET_REGISTRY
                        .iter()
                        .find(|p| p.id == head_id)
                        .unwrap_or(&PRESET_REGISTRY[0]);

                    // Resolve GLB url: groovy_glb_url() when id=="groovy", else facecap.
                    // Both functions are now unconditional (no cfg gate — D-06/REND-02).
                    let glb = if preset.id == "groovy" {
                        crate::app::groovy_glb_url()
                    } else if preset.id == "matrix" {
                        crate::app::matrix_glb_url()
                    } else {
                        crate::app::facecap_glb_url()
                    };

                    // Build the richer preset JSON (addendum §6 / Plan 04 notes).
                    // camPos / lookAt: from preset.framing.
                    // bodyType: from preset.body_type (enum → &str).
                    // visemeMap: from preset.viseme_map (Option<&[(&str,&str)]> → obj | null).
                    // blinkMorphs: from preset.blink_morphs (Option<(&str,&str)> → arr | null).
                    let (cx, cy, cz) = preset.framing.cam_pos;
                    let (lx, ly, lz) = preset.framing.look_at;
                    let fov = preset.framing.fov;
                    let body_type_str = match preset.body_type {
                        crate::components::hermes_app::avatar_logic::BodyType::Head => "head",
                        crate::components::hermes_app::avatar_logic::BodyType::Half => "half",
                        crate::components::hermes_app::avatar_logic::BodyType::Full => "full",
                    };

                    // Build visemeMap JSON object or "null".
                    let viseme_map_json = match preset.viseme_map {
                        None => "null".to_string(),
                        Some(entries) => {
                            let pairs: Vec<String> = entries
                                .iter()
                                .map(|(oculus, morph)| format!("\"{}\":\"{}\"", oculus, morph))
                                .collect();
                            format!("{{{}}}", pairs.join(","))
                        }
                    };

                    // Build blinkMorphs JSON array or "null".
                    let blink_json = match preset.blink_morphs {
                        None => "null".to_string(),
                        Some((l, r)) => format!("[\"{}\",\"{}\"]", l, r),
                    };

                    // Material treatment string (compile-time const — no
                    // injection surface, same argument as T-40.2-04-03).
                    let material_str = match preset.material {
                        crate::components::hermes_app::avatar_logic::MaterialKind::Normal => "normal",
                        crate::components::hermes_app::avatar_logic::MaterialKind::Pbr => "pbr",
                        crate::components::hermes_app::avatar_logic::MaterialKind::MatrixHologram => "matrix",
                    };

                    // Build expressionMorphs JSON object or "null".
                    let expr_json = match preset.expression_morphs {
                        None => "null".to_string(),
                        Some(entries) => {
                            let pairs: Vec<String> = entries
                                .iter()
                                .map(|(state, morph)| format!("\"{}\":\"{}\"", state, morph))
                                .collect();
                            format!("{{{}}}", pairs.join(","))
                        }
                    };

                    // The preset object passed as the third arg to init().
                    // Morph names are compile-time consts — no injection surface (T-40.2-04-03).
                    let preset_json = format!(
                        "{{\"camPos\":[{},{},{}],\"lookAt\":[{},{},{}],\"fov\":{},\"bodyType\":\"{}\",\"visemeMap\":{},\"blinkMorphs\":{},\"material\":\"{}\",\"expressionMorphs\":{}}}",
                        cx, cy, cz, lx, ly, lz, fov, body_type_str, viseme_map_json, blink_json, material_str, expr_json
                    );

                    format!(
                        "window.{global} && window.{global}.init('orb-canvas', '{glb}', {preset_json});"
                    )
                } else {
                    // Orb path: init WITH the persisted appearance so the live orb
                    // is born on the saved Style/hue/size/glow (G-41.2-11). Passing
                    // it here — rather than leaving it to the post-init setter
                    // bridges — is load-bearing: those bridges fire ~before this
                    // init (this effect sleeps 50ms first), so setStyle/setGlow hit
                    // a null renderer and no-op, and never re-fire without a later
                    // signal change → the orb was stuck on classic/default on every
                    // fresh reload. Style is validated against the canonical preset
                    // registry before interpolation (defense-in-depth at the eval
                    // sink, mirroring the setStyle bridge); unknown/legacy → null,
                    // which orb.js treats as the classic default. hue/size/glow are
                    // bounded numerics (orb.js re-clamps) — no injection surface.
                    let style_json =
                        if crate::components::hermes_app::avatar_logic::ORB_PRESET_REGISTRY
                            .iter()
                            .any(|p| p.default_style == seed_style.as_str())
                        {
                            format!("\"{seed_style}\"")
                        } else {
                            "null".to_string()
                        };
                    format!(
                        "window.{global} && window.{global}.init('orb-canvas', \
                         {{\"style\":{style_json},\"baseHue\":{seed_hue},\
                         \"size\":{seed_size},\"glow\":{seed_glow}}});"
                    )
                };

                let eval_init = document::eval(&init_script);
                // Await init completion (resolves when JS returns).
                let _ = eval_init.await;

                // Establish long-lived FFT pump eval handle.
                // JS runs an infinite recv loop — Rust pushes bins via eval.send().
                // updateFFT targets the SAME selected global (Pitfall 5).
                //
                // CRITICAL (Phase 41.2 "no talk pulse" root cause): do NOT wrap this
                // body in a self-invoking `(async function(){...})()`. Dioxus already
                // wraps every eval body in `(async function(){ <BODY>; dioxus.close(); })()`
                // (PROMISE_WRAPPER in dioxus web `document.rs`). An inner IIFE is fired
                // but NOT awaited, so its `await dioxus.recv()` suspends a *detached*
                // promise while control falls straight through to `dioxus.close()` —
                // which closes the send/recv channel immediately. `eval.send()` then
                // lands on a dead channel and `updateFFT` is never called (observed:
                // `pump fire slot=true` in Rust, zero `updateFFT` lines in JS).
                // Keeping the infinite loop at the eval-body top level means
                // `dioxus.close()` is never reached and the Rust→JS channel stays open.
                let pump_script = format!(
                    r#"while (true) {{
                        const bins = await dioxus.recv();
                        if (window.{global}) {{
                            window.{global}.updateFFT(bins);
                        }}
                    }}"#,
                );
                let pump_handle = document::eval(&pump_script);
                FFT_EVAL_SLOT.with(|slot| {
                    *slot.borrow_mut() = Some(pump_handle);
                });

                // Launch error-poll eval when avatar is enabled (FE-05).
                // Separate slot from FFT_EVAL_SLOT (JS→Rust direction).
                //
                // CRITICAL (same root cause as the FFT pump): do NOT wrap this body
                // in a self-invoking `(async function(){...})()`. Dioxus already wraps
                // every eval body in `(async(){ <body>; dioxus.close(); })()`
                // (PROMISE_WRAPPER). An inner IIFE is fired but NOT awaited, so
                // `dioxus.close()` runs IMMEDIATELY — before the 500 ms poll, before any
                // error is sent — closing the JS→Rust channel. The Rust `recv().await`
                // below then resolves to `Err(Finished)` and the `if result.is_ok()`
                // recovery guard never runs (avatar-error recovery silently dead).
                // Keeping the loop at the eval-body top level means `dioxus.close()`
                // fires only after the loop `break`s, i.e. AFTER `dioxus.send()`
                // delivers the payload (Pitfall 7: send before break).
                if enabled {
                    let error_poll_script = r#"
                        while (true) {
                            await new Promise(r => setTimeout(r, 500));
                            const e = window.__ihAvatarError;
                            if (e) {
                                window.__ihAvatarError = null;
                                dioxus.send(JSON.stringify(e));
                                break;
                            }
                        }
                    "#;
                    let error_handle = document::eval(error_poll_script);
                    AVATAR_ERROR_EVAL_SLOT.with(|slot| {
                        *slot.borrow_mut() = Some(error_handle);
                    });
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = (enabled, head_id, seed_style, seed_hue, seed_size, seed_glow);
            }
        });
    });

    // ── Error-poll recv effect: handle __ihAvatarError (FE-05, D-11) ─────────
    // When the error-poll JS sends a payload, this effect receives it and:
    //   1. Destroys the avatar eval (context freed, forceContextLoss in avatar.js).
    //   2. Waits 50ms DOM flush.
    //   3. Inits the orb (silent restore — no page reload, no preference change).
    //   4. Sets AvatarErrorNoticeCtx to true (one-time per-session notice).
    // AvatarPrefs.enabled is intentionally NOT changed (D-11: keep preference).
    use_effect(move || {
        // Pattern B: owned local before spawn — no borrow across .await.
        let prefs = avatar_ctx.0.read().clone();
        let enabled = prefs.enabled;

        if !enabled {
            return;
        }

        spawn(async move {
            #[cfg(target_arch = "wasm32")]
            {
                // Wait for an error payload from the JS poll.
                // recv() takes &mut self, so we take the handle out of the slot,
                // await it, then put it back (or leave it None if it finished).
                let mut error_handle = AVATAR_ERROR_EVAL_SLOT.with(|slot| slot.borrow_mut().take());
                if let Some(ref mut handle) = error_handle {
                    // recv() resolves when JS calls dioxus.send() with the error payload string.
                    let result: Result<serde_json::Value, _> = handle.recv().await;
                    if result.is_ok() {
                        // Error received: destroy avatar, restore orb silently.
                        let destroy_script =
                            format!("window.{AVATAR_GLOBAL} && window.{AVATAR_GLOBAL}.destroy();");
                        let _ = document::eval(&destroy_script).await;

                        gloo_timers::future::sleep(std::time::Duration::from_millis(50)).await;

                        // Init orb (preference unchanged — D-11).
                        let orb_init = format!(
                            "window.{ORB_GLOBAL} && window.{ORB_GLOBAL}.init('orb-canvas');"
                        );
                        let _ = document::eval(&orb_init).await;

                        // Clear the error-poll slot (it broke out of its loop).
                        AVATAR_ERROR_EVAL_SLOT.with(|slot| {
                            *slot.borrow_mut() = None;
                        });

                        // Raise the one-time per-session notice (FE-05).
                        avatar_error_ctx.0.set(true);
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = enabled;
            }
        });
    });

    // ── (removed) Swap-on-change effect — merged into the init effect above ──
    // The live orb ↔ avatar swap is now handled entirely by the single reactive
    // init effect above: it subscribes to AvatarModeCtx and, on every change,
    // destroys BOTH globals then inits the selected one. A separate swap effect
    // here re-ran in parallel and double-initialised the scene (the "two avatars"
    // UAT bug — one frozen, one live), so it was removed.

    // ── FFT pump: send bins whenever fft_bins changes ────────────────────────
    // `fft_bins` is a plain `Vec<u8>` PROP (read from the AnalyserNode by
    // voice_mode.rs), NOT a Signal. A bare `use_effect` only re-runs when the
    // SIGNALS it reads change — capturing a plain value is not a subscription —
    // so it would fire exactly ONCE (sending the initial empty frame) and the orb
    // would never receive live audio: it shows only the idle breath and never
    // pulses to speech. That is the Phase 41.2 "talk pulse doesn't work" root
    // cause, confirmed against the Dioxus effect-reactivity docs (non-signal deps
    // must be declared via `use_reactive!`). Declaring `fft_bins` as an explicit
    // reactive dependency makes the effect re-fire on every new FFT frame
    // (~100 ms from the voice_mode poll), so live bins reach orb.js each frame.
    use_effect(use_reactive!(|fft_bins| {
        #[cfg(target_arch = "wasm32")]
        {
            let bins_json =
                serde_json::to_value(&fft_bins).unwrap_or(serde_json::Value::Array(vec![]));
            FFT_EVAL_SLOT.with(|slot| {
                if let Some(eval) = slot.borrow().as_ref() {
                    let _ = eval.send(bins_json);
                }
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = &fft_bins;
        }
    }));

    // ── setState: push state string to JS when state changes ─────────────────
    let state_str = match &state {
        VoiceModeState::Idle => "idle",
        VoiceModeState::Listening => "listening",
        VoiceModeState::Thinking => "thinking",
        VoiceModeState::Speaking => "speaking",
        VoiceModeState::Armed => "idle", // armed uses idle color
        VoiceModeState::Unavailable => "idle",
    };
    // Read state_str into an owned String before the spawn so no borrow crosses
    // into the async block (Pattern B — clippy.toml enforced).
    let state_owned = state_str.to_string();
    use_effect(move || {
        // use_effect callback is FnMut (may re-run on state change); clone the
        // owned String per-run so nothing moves out of the captured variable.
        let value = state_owned.clone();
        spawn(async move {
            #[cfg(target_arch = "wasm32")]
            {
                // Phase 40.2 Plan 04: runtime global selection from AvatarModeCtx.
                // Pattern B: Copy bool from .read() — borrow released immediately at `;`.
                let enabled = avatar_ctx.0.read().enabled;
                let global = if enabled { AVATAR_GLOBAL } else { ORB_GLOBAL };
                let script = format!("window.{global} && window.{global}.setState('{value}');",);
                let _ = document::eval(&script).await;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = value;
            }
        });
    });

    // ── Phase 41.2 Plan 01: OrbStyleCtx → setStyle eval bridge (D-03) ───────
    // Re-runs whenever OrbStyleCtx changes (signal read subscribes). Only fires
    // when the orb is active (not a head avatar) — no-op before mount.
    // Pattern B: read ctx into owned local before the async block (no borrow
    // across .await).
    //
    // `use_resource` (NOT `use_effect` + `spawn()`) is load-bearing here: if
    // OrbStyleCtx changes again while this future is still awaiting the
    // TimeoutFuture or the document::eval, Dioxus cancels the stale in-flight
    // task automatically — a rapid re-click can never "lose the race" to a
    // slower earlier click (RESEARCH.md Pitfall 2; `use_effect`+`spawn()` does
    // NOT auto-cancel a prior task on re-run).
    let orb_style_ctx = use_context::<OrbStyleCtx>();
    // `mut` is only exercised inside the `#[cfg(target_arch = "wasm32")]` arm
    // below (`.set()` requires `&mut self` in this Signal API) — the native
    // build never calls `.set()` on it, so `#[allow(unused_mut)]` is scoped
    // to non-wasm to keep the wasm build's real mut-usage lint intact
    // (wasm-scoped cfg_attr(not(wasm), allow(...)) is this crate's established
    // pattern for cfg-only-used bindings).
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut))]
    let mut orb_settling = use_context::<OrbSettlingCtx>().0;
    let _style_task = use_resource(move || {
        let style_val = orb_style_ctx.0.read().clone(); // subscribes; owned local
        async move {
            #[cfg(target_arch = "wasm32")]
            {
                // Only push to orb while the orb is the active render (not a head avatar).
                let enabled = avatar_ctx.0.read().enabled;
                if !enabled {
                    orb_settling.set(true);

                    // Bounded fallback (T-41.2-02 / E2-error backstop): guarantees
                    // the "Switching style…" label clears even if the eval below
                    // hangs or never resolves. Spawned as a sibling task so it runs
                    // concurrently with the main path below — whichever finishes
                    // first clears the flag (idempotent double-clear is harmless).
                    // Guarded by re-checking OrbStyleCtx still matches this
                    // generation's style_val so a stale fallback from a
                    // since-superseded click can never clobber a newer in-flight
                    // switch's settling state.
                    let style_for_fallback = style_val.clone();
                    let fallback_ctx = orb_style_ctx;
                    let mut fallback_settling = orb_settling;
                    spawn(async move {
                        gloo_timers::future::TimeoutFuture::new(1500).await;
                        if fallback_ctx.0.peek().as_str() == style_for_fallback.as_str() {
                            fallback_settling.set(false);
                        }
                    });

                    // Yield one macrotask so the browser paints the tile-selection
                    // + settling-label DOM patch BEFORE the synchronous three.js
                    // rebuild inside setStyle() runs (RESEARCH.md Pitfall 1 — a
                    // microtask/already-resolved await does NOT guarantee a paint
                    // boundary; TimeoutFuture is backed by setTimeout(0), a real
                    // macrotask).
                    gloo_timers::future::TimeoutFuture::new(0).await;

                    // Guard on window global before call: no-op if orb not yet mounted.
                    // Security (T-40.5-07-01 hardening, defense-in-depth; T-41.2-01):
                    // validate the style token against the canonical preset registry
                    // at the eval sink before interpolating into JS, so a persisted /
                    // legacy / hand-edited config value can never inject into
                    // document::eval. (Server-side save validation is the first
                    // layer; this is the second, at the dangerous sink.) The
                    // hue/size/glow bridges interpolate bounded numerics and need
                    // no such guard.
                    // OrbStyleCtx holds the render-mode NAME ("classic"/"bloom"/
                    // "ascii"/"network" — the tile `style_key` and the identity
                    // preset's `default_style`), NOT the preset id ("orb_ascii"
                    // etc.). The original hardening (40.5 fb684d71a) validated
                    // against `p.id`, which is prefixed and therefore NEVER
                    // matched a style name — so setStyle was silently never
                    // dispatched and the orb was stuck on classic for every
                    // non-default style (41.2 gap-fix). Validate against
                    // `default_style` — still a canonical-registry allowlist, so
                    // the JS-injection defense at this eval sink is preserved.
                    if crate::components::hermes_app::avatar_logic::ORB_PRESET_REGISTRY
                        .iter()
                        .any(|p| p.default_style == style_val.as_str())
                    {
                        let script = format!(
                            "if (window.{ORB_GLOBAL}) {{ window.{ORB_GLOBAL}.setStyle('{style_val}'); }}"
                        );
                        let _ = document::eval(&script).await;
                    }
                    orb_settling.set(false);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = style_val;
                let _ = &orb_settling;
            }
        }
    });

    // ── OrbBaseHueCtx → setBaseHue eval bridge (D-05) ────────────────────────
    // Drives setBaseHue (not per-state absolute hex) — per-state color offsets preserved.
    let orb_hue_ctx = use_context::<OrbBaseHueCtx>();
    use_effect(move || {
        let hue_val = *orb_hue_ctx.0.read(); // subscribes; borrow drops at `;`
        spawn(async move {
            #[cfg(target_arch = "wasm32")]
            {
                let enabled = avatar_ctx.0.read().enabled;
                if !enabled {
                    let script = format!(
                        "if (window.{ORB_GLOBAL}) {{ window.{ORB_GLOBAL}.setBaseHue({hue_val}); }}"
                    );
                    let _ = document::eval(&script).await;
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = hue_val;
            }
        });
    });

    // ── OrbSizeCtx → setSize eval bridge ────────────────────────────────────
    let orb_size_ctx = use_context::<OrbSizeCtx>();
    use_effect(move || {
        let size_val = *orb_size_ctx.0.read(); // subscribes; borrow drops at `;`
        spawn(async move {
            #[cfg(target_arch = "wasm32")]
            {
                let enabled = avatar_ctx.0.read().enabled;
                if !enabled {
                    // Pass with one decimal: orb.js clamps to [0.5, 2.0] on its side too.
                    let script = format!(
                        "if (window.{ORB_GLOBAL}) {{ window.{ORB_GLOBAL}.setSize({size_val}); }}"
                    );
                    let _ = document::eval(&script).await;
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = size_val;
            }
        });
    });

    // ── OrbGlowCtx → setGlow eval bridge ─────────────────────────────────────
    let orb_glow_ctx = use_context::<OrbGlowCtx>();
    use_effect(move || {
        let glow_val = *orb_glow_ctx.0.read(); // subscribes; borrow drops at `;`
        spawn(async move {
            #[cfg(target_arch = "wasm32")]
            {
                let enabled = avatar_ctx.0.read().enabled;
                if !enabled {
                    // orb.js clamps to [0.0, 1.0] on its side; pass clean value.
                    let script = format!(
                        "if (window.{ORB_GLOBAL}) {{ window.{ORB_GLOBAL}.setGlow({glow_val}); }}"
                    );
                    let _ = document::eval(&script).await;
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = glow_val;
            }
        });
    });

    // ── Cleanup on unmount ────────────────────────────────────────────────────
    // .peek() is acceptable in a drop closure (not a render path — Dioxus trap 1).
    use_drop(move || {
        #[cfg(target_arch = "wasm32")]
        {
            // Remove eval slots first.
            FFT_EVAL_SLOT.with(|slot| {
                *slot.borrow_mut() = None;
            });
            AVATAR_ERROR_EVAL_SLOT.with(|slot| {
                *slot.borrow_mut() = None;
            });
            // Phase 40.2 Plan 04: runtime global selection (.peek() in drop — ok).
            let enabled = avatar_ctx.0.peek().enabled;
            let global = if enabled { AVATAR_GLOBAL } else { ORB_GLOBAL };
            let _eval = document::eval(&format!("window.{global} && window.{global}.destroy();"));
        }
    });

    // ── RSX ──────────────────────────────────────────────────────────────────
    rsx! {
        if webgl_ok {
            canvas {
                id: "orb-canvas",
                "aria-hidden": "true",
                // Width/height are set by the JS resize observer after mount.
                // CSS in voice.css controls the display dimensions via
                // width/height on .orb-region > canvas.
            }
        } else {
            p {
                class: "orb-unavailable-notice",
                "3D orb unavailable in this browser. Voice mode still works."
            }
        }
    }
}
