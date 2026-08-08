use dioxus::prelude::*;

// ---------------------------------------------------------------------------
// Default-build assets (Phase 26.2.1 new shell)
// ---------------------------------------------------------------------------
//
// Load order matches CONTEXT D-07 / RESEARCH Pattern 1:
//   tokens → site → wheel → screens → components
// `tokens.css` MUST come first because it declares the CSS custom properties
// the other four sheets consume.

const FAVICON: Asset = asset!("/assets/favicon.ico");
const TOKENS_CSS: Asset = asset!("/assets/tokens.css");
const SITE_CSS: Asset = asset!("/assets/site.css");
const WHEEL_CSS: Asset = asset!("/assets/wheel.css");
const SCREENS_CSS: Asset = asset!("/assets/screens.css");
const COMPONENTS_CSS: Asset = asset!("/assets/components.css");
// Phase 36.17.8 Plan 06 (D-14): mic button states, pulse animation,
// inline status, and sr-only live region. Loaded unconditionally per
// project memory rule (CSS tokens must load unconditionally).
const VOICE_CSS: Asset = asset!("/assets/voice.css");
// Phase 36.17.9 Plan 03 (D-02): Three.js r0.184.0 self-hosted + orb.js
// audio-reactive orb. Loaded unconditionally (project memory rule —
// CSS/JS tokens must never be gated behind feature flags).
//
// three.core.js is the new split-build dependency of three.module.js (r0.184.0).
// three.module.js line 6 does `import { ... } from './three.core.js'` so the
// browser fetches it transitively. We register it here so Dioxus bundles and
// serves it at /assets/three.core.js — no extra <script> tag required.
// The constant is intentionally unreferenced in rsx! — its sole job is to
// make the Dioxus asset pipeline copy the file into the output bundle.
#[allow(dead_code)]
const THREE_CORE_JS: Asset = asset!("/assets/three.core.js");
const THREE_JS: Asset = asset!("/assets/three.module.js");
const ORB_JS: Asset = asset!("/assets/orb.js");

// ---------------------------------------------------------------------------
// Phase 47.3 login-page assets — registered HERE, in the client tree, on purpose.
// ---------------------------------------------------------------------------
//
// These three are declared and used exclusively by `server/login_page.rs`, which
// is entirely `#[cfg(feature = "server")]`. That module already registers them
// with the same `#[allow(dead_code)]`-for-bundling idiom (login_page.rs:72-74,
// citing THREE_CORE_JS above) — but a server-side `asset!()` never reaches the
// bundle, because `dx` builds its asset manifest from the CLIENT (wasm) build.
// The server binary still interpolated the hashed URLs into the login HTML, so
// every login asset 404'd in `dx serve` and fell through to the SPA fallback
// (a FALSE 200 serving app-shell HTML) in `dx bundle --release`. The login page
// therefore shipped completely unstyled. Found in 47.3 UAT; see G-47.3-1.
//
// Evidence this is the right lever: `favicon.ico` was the ONLY login asset that
// bundled correctly, and it is the ONLY one that was also declared client-side
// (line 12 above). `matrix-woman.glb` (client-only, added the same week) bundled
// fine; these three (server-only, same week) did not.
//
// Do NOT "clean up" these as unused. Like THREE_CORE_JS, their effect is the
// pipeline copy, not a reference — deleting them silently un-styles the login
// page in a way no test catches, because the CSS assertions in login_page.rs
// read `assets/login.css` from the SOURCE TREE via `read_asset`, never over HTTP.
#[allow(dead_code)]
const LOGIN_CSS: Asset = asset!("/assets/login.css");
#[allow(dead_code)]
const LOGIN_RAIN_JS: Asset = asset!("/assets/login-rain.js");
#[allow(dead_code)]
const EARTH_NIGHT_JPG: Asset = asset!("/assets/earth-night.jpg");

// ---------------------------------------------------------------------------
// Phase 01 (Three.js Viseme Avatar Core) Plan 04 (REND-01): avatar assets.
// ---------------------------------------------------------------------------
//
// `avatar.js` is the ONLY script we emit a <script type="module"> for. It does
// `import { GLTFLoader } from './GLTFLoader.js'`, `import { MeshoptDecoder }
// from './meshopt_decoder.module.js'`, and `import { Lipsync } from
// './wawa-lipsync.js'` — those resolve RELATIVELY at load time, so emitting a
// separate <script> for each would be wrong (it would double-load them as
// classic scripts). We still register them as `asset!()` consts whose sole job
// is to make the Dioxus asset pipeline copy them into the output bundle under
// stable hashed names so the relative imports resolve. The consts are
// intentionally unreferenced (their effect is the pipeline copy), hence
// `#[allow(dead_code)]`. (RESEARCH "Recommended Project Structure"; Pitfall 4.)
//
// `viseme-map.js` is likewise imported relatively by avatar.js (Oculus→ARKit
// table) and only needs to be served, not scripted.
//
// FACECAP_GLB is the binary head mesh. Its hashed URL is the one value we need
// at runtime: it is passed into `ironHermesAvatar.init('orb-canvas', glbUrl)`
// via `facecap_glb_url()` below (the GLB is loaded by GLTFLoader, not a
// <script>). All self-hosted — NO runtime CDN calls (REND-01 / D-12).
#[allow(dead_code)]
const GLTFLOADER_JS: Asset = asset!("/assets/GLTFLoader.js");
#[allow(dead_code)]
const MESHOPT_JS: Asset = asset!("/assets/meshopt_decoder.module.js");
#[allow(dead_code)]
const WAWA_JS: Asset = asset!("/assets/wawa-lipsync.js");
#[allow(dead_code)]
const VISEME_MAP_JS: Asset = asset!("/assets/viseme-map.js");
const AVATAR_JS: Asset = asset!("/assets/avatar.js");
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FACECAP_GLB: Asset = asset!("/assets/facecap.glb");
// Phase 40.2 Plan 02 (ID-01, D-06, D-07): Groovy avatar GLB — second head
// preset. Registered unconditionally (no cfg(feature="avatar") gate per D-06
// / REND-02). The const is pipeline-only until Plan 04 wires groovy_glb_url()
// into the toggle init path, hence #[allow(dead_code)].
#[allow(dead_code)]
const GROOVY_GLB: Asset = asset!("/assets/groovy-avatar.glb");
// Matrix Woman GLB — third head preset (spec 2026-07-13, 3d_models repo).
// User-authored asset (license-clean, self-hosted, no runtime fetch —
// REND-01). Registered unconditionally (no feature gate — D-06/REND-02).
#[allow(dead_code)]
const MATRIX_GLB: Asset = asset!("/assets/matrix-woman.glb");

// ---------------------------------------------------------------------------
// Phase 40.5 Plan 02 (D-07, REND-01): vendored three.js r184 postprocessing
// addon ES modules — self-hosted, no CDN.
// ---------------------------------------------------------------------------
//
// orb.js imports these relatively (e.g. `import { EffectComposer } from
// './EffectComposer.js'`). The Dioxus asset pipeline must copy them into the
// bundle under stable hashed names so those relative imports resolve at runtime.
// Each constant is intentionally unreferenced in rsx! — its sole job is to
// trigger the pipeline copy, hence #[allow(dead_code)]. (Pitfall 4.)
//
// Primary 8 addons (plan-specified):
#[allow(dead_code)]
const EFFECT_COMPOSER_JS: Asset = asset!("/assets/EffectComposer.js");
#[allow(dead_code)]
const RENDER_PASS_JS: Asset = asset!("/assets/RenderPass.js");
#[allow(dead_code)]
const UNREAL_BLOOM_PASS_JS: Asset = asset!("/assets/UnrealBloomPass.js");
#[allow(dead_code)]
const SHADER_PASS_JS: Asset = asset!("/assets/ShaderPass.js");
#[allow(dead_code)]
const OUTPUT_PASS_JS: Asset = asset!("/assets/OutputPass.js");
#[allow(dead_code)]
const LUMINOSITY_HIGH_PASS_SHADER_JS: Asset = asset!("/assets/LuminosityHighPassShader.js");
#[allow(dead_code)]
const COPY_SHADER_JS: Asset = asset!("/assets/CopyShader.js");
#[allow(dead_code)]
const ASCII_EFFECT_JS: Asset = asset!("/assets/AsciiEffect.js");
// Transitive dependencies (Rule 2 deviation): the 8 primary files import these
// via relative specifiers; omitting them would cause 404 errors in the browser.
#[allow(dead_code)]
const PASS_JS: Asset = asset!("/assets/Pass.js");
#[allow(dead_code)]
const MASK_PASS_JS: Asset = asset!("/assets/MaskPass.js");
#[allow(dead_code)]
const OUTPUT_SHADER_JS: Asset = asset!("/assets/OutputShader.js");

/// Resolved (hashed) URL of the self-hosted `facecap.glb` head mesh.
///
/// Called by `orb_canvas.rs` to interpolate into the avatar
/// `init('orb-canvas', glbUrl)` eval string (REND-01, Pitfall 4).
/// Unconditional — the `avatar = []` feature gate is retired (D-06/REND-02).
/// `Asset`'s `Display` yields the hashed URL.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn facecap_glb_url() -> String {
    FACECAP_GLB.to_string()
}

/// Resolved (hashed) URL of the self-hosted `groovy-avatar.glb` head mesh
/// (second preset, Phase 40.2 ID-01). Mirrors `facecap_glb_url()` — no
/// `cfg(feature = "avatar")` gate (D-06). Called by Plan 04's toggle init
/// path once the identity-switch UI lands.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn groovy_glb_url() -> String {
    GROOVY_GLB.to_string()
}

/// Resolved (hashed) URL of the self-hosted `matrix-woman.glb` bust
/// (third preset). Mirrors `groovy_glb_url()` — no feature gate (D-06).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn matrix_glb_url() -> String {
    MATRIX_GLB.to_string()
}

// ---------------------------------------------------------------------------
// Legacy-shell assets (only compiled when the `legacy-shell` feature is on)
// ---------------------------------------------------------------------------
//
// Kept compiling as a UAT fallback per D-25/D-26. Both the asset constants
// and the Link tags below are gated so the default WASM bundle never
// references them.

#[cfg(feature = "legacy-shell")]
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
#[cfg(feature = "legacy-shell")]
const MAIN_CSS: Asset = asset!("/assets/main.css");
// Always-loaded — moved out of the legacy-shell gate so HermesApp can
// resolve --w-bg-*, --accent-primary, --w-border, --w-radius-*, and
// --w-shadow-* tokens (BUG-2 fix from 36.3.7.11 UAT).
const DESIGN_TOKENS_CSS: Asset = asset!("/assets/design-tokens.css");
const WARP_IH_CSS: Asset = asset!("/assets/warp-ih.css");
#[cfg(feature = "legacy-shell")]
const SCANNER_ANIM_CSS: Asset = asset!("/assets/scanner-anim.css");

#[component]
pub fn App() -> Element {
    rsx! {
        document::Meta { name: "viewport", content: "width=device-width, initial-scale=1" }
        document::Link { rel: "icon", href: FAVICON }

        // Phase 47.3 Plan 07 (D-11): the Google Fonts preconnect + stylesheet
        // link that used to sit here has been removed. Ioskeley Mono webfonts
        // are already self-hosted under /assets/fonts/ and loaded by
        // `tokens.css` / `site.css` via @font-face — no third-party fetch is
        // needed. Note: `assets/site.css` lists JetBrains Mono first in its
        // font stack, so removing this stylesheet does change something
        // visual (JetBrains now falls back to the next family in that
        // stack) — expected, not a regression.

        // New bundle CSS — load order is significant (tokens first).
        // DESIGN_TOKENS_CSS + WARP_IH_CSS hold the canonical --w-bg-*,
        // --accent-primary, --w-border, --w-radius-*, and --w-shadow-*
        // tokens that the kanban dashboard (and any future shared module)
        // consume via var(). Loaded unconditionally so non-legacy shells
        // resolve those tokens correctly (BUG-2 fix from 36.3.7.11 UAT —
        // see .planning/HANDOFF.json: token-resolution gap caused
        // color-mix() to fall back to transparent in the drawer + modals).
        document::Link { rel: "stylesheet", href: TOKENS_CSS }
        document::Link { rel: "stylesheet", href: DESIGN_TOKENS_CSS }
        document::Link { rel: "stylesheet", href: WARP_IH_CSS }
        document::Link { rel: "stylesheet", href: SITE_CSS }
        document::Link { rel: "stylesheet", href: WHEEL_CSS }
        document::Link { rel: "stylesheet", href: SCREENS_CSS }
        document::Link { rel: "stylesheet", href: COMPONENTS_CSS }
        document::Link { rel: "stylesheet", href: VOICE_CSS }

        // Phase 36.17.9 Plan 03 (D-02): Three.js + orb.js ES modules.
        // type="module" confirmed via document::Script r#type prop (Task 1 spike).
        // Self-hosted — NO runtime CDN calls (RESEARCH security note).
        document::Script { src: THREE_JS, r#type: "module", defer: true }
        document::Script { src: ORB_JS, r#type: "module", defer: true }

        // Phase 01 Plan 04 (REND-01): avatar.js ES module. Self-hosted, loaded
        // unconditionally alongside orb.js (project memory rule — JS modules
        // must never be gated behind feature flags). avatar.js relatively
        // imports GLTFLoader/meshopt/wawa/viseme-map (registered as asset!()
        // consts above) — so ONLY avatar.js gets a <script> tag here. Whether
        // the avatar actually mounts is decided at compile time in
        // orb_canvas.rs via `#[cfg(feature = "avatar")]`; the orb stays default.
        document::Script { src: AVATAR_JS, r#type: "module", defer: true }

        // Legacy bundle CSS — only emitted when the legacy shell is mounted.
        // Use a nested `legacy_links()` helper so the rsx! parser sees one
        // expression slot; the helper itself is cfg-branched at item scope.
        {legacy_links()}

        // Root child — compile-time branch (not runtime) so the OFF shell is
        // not pulled into the WASM binary. Same helper-fn pattern as above.
        {root_shell()}
    }
}

// ---------------------------------------------------------------------------
// Cfg-branched rsx fragments (compile-time selected — RESEARCH Pattern 1).
// ---------------------------------------------------------------------------

#[cfg(feature = "legacy-shell")]
fn legacy_links() -> Element {
    // DESIGN_TOKENS_CSS + WARP_IH_CSS moved to the always-loaded block
    // above so non-legacy shells resolve --w-* / --accent-primary tokens
    // (BUG-2 fix from 36.3.7.11 UAT). The remaining legacy-only sheets
    // stay gated here.
    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: SCANNER_ANIM_CSS }
    }
}

#[cfg(not(feature = "legacy-shell"))]
fn legacy_links() -> Element {
    rsx! {}
}

#[cfg(feature = "legacy-shell")]
fn root_shell() -> Element {
    rsx! { crate::components::warp_hermes::WarpHermes {} }
}

#[cfg(not(feature = "legacy-shell"))]
fn root_shell() -> Element {
    rsx! { crate::components::hermes_app::HermesApp {} }
}
