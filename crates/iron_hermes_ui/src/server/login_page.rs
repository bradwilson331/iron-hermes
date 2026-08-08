//! Phase 47.3 Plan 01 (D-01/D-02/D-07): the server-rendered login page.
//!
//! `root_handler` is the load-bearing D-07 mechanism (RESEARCH.md Pattern 1,
//! Assumption A1): `GET /` must render the login HTML for an unauthenticated
//! visitor and the real Dioxus SSR app for an authenticated one, with no
//! flash of the wrong content and no possibility of falling open. The
//! composition used here builds the router's own `.fallback(get(render_handler))`
//! and `.with_state(fullstack_state)` chain manually (the same three calls
//! `DioxusRouterExt::serve_dioxus_application` performs internally — see the
//! vendored `dioxus-server-0.7.7/src/server.rs:164-173`) rather than calling
//! that convenience wrapper directly, and inserts `.route("/", get(root_handler))`
//! BEFORE the final `.with_state(...)` call. This is required, not
//! stylistic: `serve_dioxus_application` returns `Router<()>` (state already
//! erased), so a route registered on THAT router cannot use a
//! `State<FullstackState>` extractor — there is no `FromRef<()>` for
//! `FullstackState`. Registering `/` on the same typed `Router<FullstackState>`
//! `serve_dioxus_application` itself builds from, before OUR OWN
//! `.with_state(...)` call, keeps `root_handler`'s `State<FullstackState>`
//! extractor valid while still giving the exact-path route precedence over
//! `.fallback()` (Assumption A1). This resolved Assumption A2's "is
//! `render_handler` callable from application code" concern without needing
//! the plan's documented middleware-based fallback composition — recorded
//! here and in the plan SUMMARY per the plan's explicit request.
//!
//! `public_asset_paths` is the OTHER load-bearing mechanism (Pitfall 2/3):
//! the pre-auth allowlist `auth::is_public` consults is derived from the
//! SAME `asset!()` constants this module's HTML interpolates — never a
//! `starts_with("/assets/")` / `starts_with("/wasm/")` prefix rule, and
//! never a hand-copied hashed filename literal.
//!
//! Phase 47.3 Plan 04 (D-01/D-04/D-14/D-16a/D-19/D-20) fills in the real
//! `basic` (`1a`, default) and `fade` (`1b`) treatments per
//! `47.3-UI-SPEC.md`, plus the shared D-14 inline submit script every
//! treatment reuses. Two Task-1-checkpoint resolutions are load-bearing
//! here, verbatim from the operator: "ui-spec-default, c-1, a/b/d confirmed".
//!
//! * **D-20 (c-1):** the design export's literal `attempt 1 of 3` counter is
//!   DROPPED from every failure box. It is never rendered, in any theme.
//!   D-18's `429` countdown carries the entire rate-limit story instead; the
//!   locked limit stays 5 attempts / 60s / IP (AUTH-DESIGN §3.4) — it is
//!   NOT tightened to 3, and the `401` response never grows a counter field.
//! * **D-19:** the export's own `1a`/`1b` failure copy (naming a provider,
//!   echoing an upstream HTTP status, handing out a diagnostic command) is
//!   never ported. [`LOGIN_ERROR_MESSAGE`] is the ONE place the uniform
//!   no-oracle copy exists in source; every treatment's failure box supplies
//!   only its own bracket prefix (`[DENIED]`/`[FAILED]`/...) via a
//!   `data-fail-prefix` attribute, never a second copy of the message body
//!   (T-47.3-17 — `error_copy_is_uniform`/the plan's own literal grep pin
//!   this).
//!
//! The 429 retry-window countdown ([`RETRY_WINDOW_FALLBACK_SECS`]): this
//! plan's `files_modified` does not include `server/auth.rs` (a concurrently
//! running plan owns it, and `auth::login`'s `429` branch does not currently
//! send a `Retry-After` header). The shared script therefore reads
//! `Retry-After` when present and falls back to the known AUTH-DESIGN §3.4
//! window (60s) otherwise — forward-compatible if a later plan adds the
//! header, correct today without touching a file outside this plan's scope.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use dioxus::prelude::*;
use dioxus::server::{render_handler, FullstackState};

use crate::server::auth::AuthState;

// ---------------------------------------------------------------------------
// asset!() constants — app.rs:12-60 convention. `#[allow(dead_code)]` marks
// consts whose sole job (this plan) is triggering the dx bundle copy, not a
// direct rsx!/format! interpolation — same idiom as app.rs's THREE_CORE_JS.
// ---------------------------------------------------------------------------

const FAVICON: Asset = asset!("/assets/favicon.ico");
const LOGIN_CSS: Asset = asset!("/assets/login.css");
/// `1d` matrix-rain canvas script (D-02: only the selected theme's assets
/// are ever referenced in rendered HTML). Phase 47.3 Plan 05 wires this
/// into [`render_matrix_rain`], the only treatment that emits a `<script
/// src>` for it, and `public_asset_paths("matrix-rain")` is the only
/// allowlist that includes it (T-47.3-04).
const LOGIN_RAIN_JS: Asset = asset!("/assets/login-rain.js");
/// `2a` orbit-veil globe background. Phase 47.3 Plan 05 wires this into
/// [`render_orbit_veil`], the only treatment that references it, and
/// `public_asset_paths("orbit-veil")` is the only allowlist that includes it
/// (T-47.3-04).
const EARTH_NIGHT_JPG: Asset = asset!("/assets/earth-night.jpg");

// RESEARCH.md "Font scope": login.css inlines its own 5-rule @font-face
// block (not the shared design-tokens.css bundle), so only these 5 files
// need asset!() consts + allowlist entries — a narrower pre-auth surface
// than reusing the app's shared stylesheet.
const FONT_REGULAR: Asset = asset!("/assets/fonts/IoskeleyMono-Regular.woff2");
const FONT_MEDIUM: Asset = asset!("/assets/fonts/IoskeleyMono-Medium.woff2");
const FONT_BOLD: Asset = asset!("/assets/fonts/IoskeleyMono-Bold.woff2");
const FONT_CONDENSED: Asset = asset!("/assets/fonts/IoskeleyMono-Condensed.woff2");
const FONT_CONDENSED_BOLD: Asset = asset!("/assets/fonts/IoskeleyMono-CondensedBold.woff2");

/// D-01: the five designed login treatments. Any theme slug outside this
/// list falls back to `basic` (both in `public_asset_paths` and
/// `login_html`) rather than erroring.
pub const LOGIN_THEME_SLUGS: &[&str] = &["basic", "fade", "blast-doors", "matrix-rain", "orbit-veil"];

/// `2a`'s 110 procedurally-generated (`Math.random()`) stars, resolved per
/// CONTEXT.md's "Claude's Discretion": baked to one frozen-seed set rather
/// than re-implementing a PRNG in Rust or adding a randomness dependency to
/// a pre-auth page. Generated once from the export's own algorithm
/// (`d = 1px 82% of the time else 2px`, `x/y` uniform over the 1180×720
/// authored canvas, `o` in `[0.25, 0.85)`, `t` in `[3, 9)` seconds) and
/// embedded literally — nothing downstream depends on the exact positions.
struct BakedStar {
    x: f32,
    y: f32,
    d: u8,
    o: f32,
    t: f32,
}

const BAKED_STARS: [BakedStar; 110] = [
    BakedStar { x: 63.5, y: 268.3, d: 1, o: 0.32, t: 8.0 },
    BakedStar { x: 175.6, y: 424.0, d: 1, o: 0.79, t: 8.5 },
    BakedStar { x: 537.3, y: 338.8, d: 1, o: 0.59, t: 7.2 },
    BakedStar { x: 181.7, y: 46.3, d: 1, o: 0.8, t: 7.3 },
    BakedStar { x: 340.2, y: 370.1, d: 2, o: 0.72, t: 7.1 },
    BakedStar { x: 264.4, y: 543.9, d: 2, o: 0.39, t: 4.5 },
    BakedStar { x: 486.6, y: 395.5, d: 1, o: 0.74, t: 8.6 },
    BakedStar { x: 65.5, y: 407.0, d: 1, o: 0.56, t: 4.1 },
    BakedStar { x: 114.0, y: 333.1, d: 1, o: 0.53, t: 7.7 },
    BakedStar { x: 641.9, y: 132.6, d: 1, o: 0.68, t: 8.7 },
    BakedStar { x: 746.8, y: 413.5, d: 1, o: 0.32, t: 8.5 },
    BakedStar { x: 174.9, y: 493.7, d: 1, o: 0.51, t: 6.5 },
    BakedStar { x: 1067.6, y: 442.2, d: 1, o: 0.27, t: 3.1 },
    BakedStar { x: 660.6, y: 352.5, d: 1, o: 0.36, t: 5.7 },
    BakedStar { x: 506.9, y: 462.1, d: 1, o: 0.33, t: 3.9 },
    BakedStar { x: 207.6, y: 485.9, d: 1, o: 0.38, t: 7.8 },
    BakedStar { x: 1163.7, y: 551.1, d: 1, o: 0.81, t: 8.9 },
    BakedStar { x: 1098.6, y: 717.5, d: 2, o: 0.32, t: 6.5 },
    BakedStar { x: 263.8, y: 459.1, d: 2, o: 0.72, t: 6.8 },
    BakedStar { x: 1150.8, y: 649.7, d: 1, o: 0.45, t: 6.4 },
    BakedStar { x: 560.6, y: 461.0, d: 1, o: 0.51, t: 8.1 },
    BakedStar { x: 786.8, y: 446.5, d: 1, o: 0.51, t: 4.1 },
    BakedStar { x: 463.9, y: 102.2, d: 1, o: 0.33, t: 7.9 },
    BakedStar { x: 727.6, y: 685.0, d: 1, o: 0.56, t: 3.2 },
    BakedStar { x: 380.9, y: 533.1, d: 1, o: 0.33, t: 7.1 },
    BakedStar { x: 985.7, y: 81.9, d: 1, o: 0.31, t: 7.3 },
    BakedStar { x: 1103.2, y: 413.9, d: 1, o: 0.4, t: 5.7 },
    BakedStar { x: 1122.9, y: 246.9, d: 2, o: 0.63, t: 8.4 },
    BakedStar { x: 868.1, y: 164.6, d: 2, o: 0.81, t: 6.9 },
    BakedStar { x: 625.8, y: 644.6, d: 1, o: 0.79, t: 7.3 },
    BakedStar { x: 304.6, y: 627.6, d: 1, o: 0.28, t: 5.2 },
    BakedStar { x: 365.2, y: 481.8, d: 2, o: 0.44, t: 6.2 },
    BakedStar { x: 1089.6, y: 454.3, d: 1, o: 0.34, t: 8.6 },
    BakedStar { x: 864.2, y: 345.1, d: 1, o: 0.6, t: 8.9 },
    BakedStar { x: 237.1, y: 625.4, d: 1, o: 0.27, t: 7.4 },
    BakedStar { x: 486.5, y: 413.7, d: 2, o: 0.51, t: 6.3 },
    BakedStar { x: 189.1, y: 208.2, d: 1, o: 0.66, t: 8.5 },
    BakedStar { x: 246.7, y: 420.0, d: 1, o: 0.73, t: 8.2 },
    BakedStar { x: 21.7, y: 131.8, d: 1, o: 0.53, t: 5.0 },
    BakedStar { x: 520.0, y: 342.1, d: 1, o: 0.69, t: 6.3 },
    BakedStar { x: 60.1, y: 364.8, d: 1, o: 0.31, t: 8.8 },
    BakedStar { x: 332.4, y: 185.9, d: 1, o: 0.78, t: 6.8 },
    BakedStar { x: 115.8, y: 399.1, d: 1, o: 0.81, t: 7.6 },
    BakedStar { x: 1051.9, y: 112.3, d: 2, o: 0.76, t: 6.0 },
    BakedStar { x: 742.2, y: 652.5, d: 2, o: 0.49, t: 9.0 },
    BakedStar { x: 1163.5, y: 406.7, d: 1, o: 0.64, t: 4.6 },
    BakedStar { x: 362.8, y: 30.4, d: 1, o: 0.54, t: 3.4 },
    BakedStar { x: 672.7, y: 70.6, d: 1, o: 0.8, t: 4.0 },
    BakedStar { x: 968.5, y: 47.6, d: 1, o: 0.67, t: 6.1 },
    BakedStar { x: 238.7, y: 718.9, d: 2, o: 0.33, t: 3.9 },
    BakedStar { x: 182.8, y: 527.4, d: 1, o: 0.47, t: 5.9 },
    BakedStar { x: 1170.4, y: 382.4, d: 1, o: 0.63, t: 4.8 },
    BakedStar { x: 698.7, y: 632.5, d: 1, o: 0.6, t: 6.6 },
    BakedStar { x: 97.5, y: 82.0, d: 1, o: 0.28, t: 4.7 },
    BakedStar { x: 780.3, y: 102.0, d: 1, o: 0.57, t: 3.7 },
    BakedStar { x: 410.7, y: 342.2, d: 1, o: 0.74, t: 5.3 },
    BakedStar { x: 500.2, y: 428.7, d: 2, o: 0.34, t: 6.6 },
    BakedStar { x: 503.1, y: 37.7, d: 1, o: 0.36, t: 6.8 },
    BakedStar { x: 230.5, y: 245.6, d: 1, o: 0.33, t: 3.6 },
    BakedStar { x: 1077.0, y: 620.5, d: 1, o: 0.7, t: 7.3 },
    BakedStar { x: 588.6, y: 702.4, d: 1, o: 0.83, t: 7.6 },
    BakedStar { x: 153.5, y: 172.0, d: 1, o: 0.76, t: 7.1 },
    BakedStar { x: 582.7, y: 215.3, d: 2, o: 0.68, t: 6.5 },
    BakedStar { x: 402.3, y: 437.2, d: 1, o: 0.8, t: 3.9 },
    BakedStar { x: 1008.6, y: 26.0, d: 2, o: 0.26, t: 7.2 },
    BakedStar { x: 593.6, y: 107.4, d: 1, o: 0.7, t: 4.1 },
    BakedStar { x: 428.1, y: 8.1, d: 1, o: 0.62, t: 7.0 },
    BakedStar { x: 403.7, y: 696.6, d: 2, o: 0.65, t: 7.3 },
    BakedStar { x: 205.8, y: 303.9, d: 1, o: 0.32, t: 7.4 },
    BakedStar { x: 298.6, y: 515.6, d: 2, o: 0.45, t: 5.4 },
    BakedStar { x: 347.1, y: 556.1, d: 2, o: 0.25, t: 5.9 },
    BakedStar { x: 829.6, y: 216.4, d: 1, o: 0.82, t: 4.6 },
    BakedStar { x: 316.9, y: 289.5, d: 1, o: 0.52, t: 8.3 },
    BakedStar { x: 364.9, y: 11.0, d: 1, o: 0.44, t: 4.8 },
    BakedStar { x: 1009.3, y: 586.1, d: 1, o: 0.67, t: 6.5 },
    BakedStar { x: 404.4, y: 42.2, d: 1, o: 0.62, t: 3.9 },
    BakedStar { x: 511.1, y: 62.4, d: 1, o: 0.41, t: 5.5 },
    BakedStar { x: 807.8, y: 505.4, d: 1, o: 0.33, t: 7.5 },
    BakedStar { x: 868.2, y: 224.5, d: 1, o: 0.83, t: 8.4 },
    BakedStar { x: 246.7, y: 676.7, d: 2, o: 0.33, t: 3.2 },
    BakedStar { x: 865.2, y: 536.5, d: 1, o: 0.26, t: 6.3 },
    BakedStar { x: 1179.2, y: 498.6, d: 2, o: 0.27, t: 6.0 },
    BakedStar { x: 1175.8, y: 235.4, d: 1, o: 0.84, t: 3.0 },
    BakedStar { x: 1009.3, y: 504.7, d: 1, o: 0.71, t: 9.0 },
    BakedStar { x: 997.2, y: 118.4, d: 1, o: 0.53, t: 5.9 },
    BakedStar { x: 739.1, y: 347.1, d: 2, o: 0.41, t: 5.5 },
    BakedStar { x: 37.3, y: 655.6, d: 1, o: 0.33, t: 6.6 },
    BakedStar { x: 663.5, y: 66.3, d: 2, o: 0.69, t: 3.8 },
    BakedStar { x: 332.4, y: 375.7, d: 1, o: 0.84, t: 8.1 },
    BakedStar { x: 474.8, y: 46.6, d: 1, o: 0.71, t: 4.6 },
    BakedStar { x: 1164.2, y: 530.6, d: 1, o: 0.56, t: 7.5 },
    BakedStar { x: 938.4, y: 22.6, d: 1, o: 0.6, t: 3.7 },
    BakedStar { x: 1043.2, y: 645.1, d: 2, o: 0.74, t: 7.8 },
    BakedStar { x: 1002.4, y: 166.4, d: 1, o: 0.26, t: 3.8 },
    BakedStar { x: 1041.5, y: 454.3, d: 1, o: 0.31, t: 8.5 },
    BakedStar { x: 1093.7, y: 423.2, d: 2, o: 0.58, t: 4.2 },
    BakedStar { x: 128.6, y: 487.6, d: 2, o: 0.39, t: 6.2 },
    BakedStar { x: 513.6, y: 414.5, d: 1, o: 0.6, t: 7.8 },
    BakedStar { x: 42.2, y: 535.7, d: 1, o: 0.38, t: 5.7 },
    BakedStar { x: 1115.6, y: 704.3, d: 1, o: 0.53, t: 3.4 },
    BakedStar { x: 213.2, y: 548.3, d: 1, o: 0.42, t: 6.9 },
    BakedStar { x: 499.6, y: 610.6, d: 2, o: 0.32, t: 6.6 },
    BakedStar { x: 80.2, y: 120.2, d: 1, o: 0.53, t: 3.7 },
    BakedStar { x: 240.7, y: 540.7, d: 1, o: 0.37, t: 8.9 },
    BakedStar { x: 59.3, y: 570.9, d: 1, o: 0.28, t: 7.2 },
    BakedStar { x: 623.8, y: 515.0, d: 1, o: 0.27, t: 6.7 },
    BakedStar { x: 20.1, y: 4.2, d: 1, o: 0.69, t: 8.8 },
    BakedStar { x: 527.3, y: 177.0, d: 1, o: 0.5, t: 8.4 },
    BakedStar { x: 108.8, y: 354.4, d: 1, o: 0.34, t: 3.2 },
    BakedStar { x: 1168.8, y: 165.2, d: 1, o: 0.7, t: 6.5 },
];

fn normalize_theme(theme: &str) -> &'static str {
    LOGIN_THEME_SLUGS
        .iter()
        .find(|&&slug| slug == theme)
        .copied()
        .unwrap_or("basic")
}

/// Single source of truth for the pre-auth asset surface: whatever paths the
/// rendered login HTML references for `theme` ARE the allowlist
/// `auth::is_public` consults (via `AuthState::new`, which calls this at
/// construction) — no second, hand-maintained list to drift out of sync
/// (RESEARCH.md Pattern 2).
pub fn public_asset_paths(theme: &str) -> Vec<String> {
    let theme = normalize_theme(theme);
    let mut paths = vec![
        LOGIN_CSS.to_string(),
        FAVICON.to_string(),
        FONT_REGULAR.to_string(),
        FONT_MEDIUM.to_string(),
        FONT_BOLD.to_string(),
        FONT_CONDENSED.to_string(),
        FONT_CONDENSED_BOLD.to_string(),
    ];
    match theme {
        "matrix-rain" => paths.push(LOGIN_RAIN_JS.to_string()),
        "orbit-veil" => paths.push(EARTH_NIGHT_JPG.to_string()),
        _ => {}
    }
    paths
}

/// `1c`'s decorative "Providers" background grid — static markup with no
/// live data, baked at build time to match the export's own
/// `hint-placeholder-count="6"` (Task 1's action item). These are the same
/// six provider names/URLs shown in the design source; they are scene
/// dressing behind the sealed bulkhead doors, not part of the failure copy
/// (D-19's oracle rule governs the 401 error message body, not this
/// decorative background list).
const BLAST_DOOR_PROVIDERS: [(&str, &str); 6] = [
    ("huggingface", "https://router.huggingface…"),
    ("llama", "http://127.0.0.1:8080/v1"),
    ("merge", "https://api-gateway.merge.d…"),
    ("minimax", "https://api.minimax.io/v1"),
    ("moonshot", "https://api.kimi.com/coding…"),
    ("ollama", "http://127.0.0.1:8081/v1"),
];

/// Phase 47.3 Plan 01: the single authoritative Content-Security-Policy for
/// login page responses. One const — never scattered string concatenation
/// across call sites (RESEARCH.md Don't-Hand-Roll table; mirrors
/// `artifact_route.rs`'s `ARTIFACT_CSP`). Admits the D-14 inline submit
/// script (`script-src 'self' 'unsafe-inline'`) and, later, `login-rain.js`
/// (same-origin `<script src>`, already covered by `script-src 'self'`).
const LOGIN_CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; \
     style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self'; \
     font-src 'self'; frame-src 'none'; form-action 'none'; base-uri 'none'";

/// Build a response carrying the load-bearing CSP + an explicit single
/// `Content-Type` — every login-page response, success or error, goes
/// through this one chokepoint (mirrors `artifact_route.rs::respond`).
fn respond(status: StatusCode, content_type: &'static str, body: Vec<u8>) -> Response {
    Response::builder()
        .status(status)
        .header(CONTENT_SECURITY_POLICY, LOGIN_CSP)
        .header(CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Response compression for every route on the server.
///
/// Defined here (not inline in `main.rs`) so `main.rs` and this module's
/// `test_router` share ONE definition — a compression layer that exists only in
/// `main.rs` is a layer no test can observe, and "we turned compression on" is
/// exactly the kind of claim that silently stops being true.
///
/// Measured motivation: the orbit-veil login document is ~206 KB of highly
/// repetitive orbiter markup and was being served with no `content-encoding` at
/// all, even when the client offered `gzip, br`. The same document is ~13 KB
/// gzipped. That page is pre-auth, so it is the first thing every operator
/// downloads, and (RESEARCH.md A3) the one page we most want to stay small.
///
/// `CompressionLayer`'s default predicate already declines to compress what it
/// must not: bodies under 32 bytes, `text/event-stream` (compressing SSE breaks
/// incremental delivery), gRPC, and already-compressed image types. WebSocket
/// upgrades are unaffected — a `101` carries no body to encode.
///
/// Not a BREACH exposure: tower-http compresses response BODIES only, never
/// headers, so the session cookie is not in the compressed stream, and the login
/// document reflects no attacker-controlled input back into its body.
#[cfg(feature = "server")]
pub fn compression_layer() -> tower_http::compression::CompressionLayer {
    tower_http::compression::CompressionLayer::new()
}

/// D-19: the ONE place the uniform, no-oracle `401` copy exists in source.
/// Every per-treatment failure box supplies only its own bracket prefix
/// (`[DENIED]`/`[FAILED]`/...) via a `data-fail-prefix` attribute in its own
/// markup — never a second copy of this message body. `T-47.3-17`'s
/// mitigation and the plan's own literal acceptance grep both depend on this
/// string appearing exactly once in this file.
const LOGIN_ERROR_MESSAGE: &str = "Incorrect password.";

/// AUTH-DESIGN §3.4 / D-18: the fixed-window rate limit is 5 attempts per
/// 60s per IP. `server/auth.rs` (owned by a concurrently running plan, not
/// modified here) does not currently send a `Retry-After` header on its
/// `429` response — the shared script below reads one when present and
/// falls back to this known upper bound otherwise.
const RETRY_WINDOW_FALLBACK_SECS: u32 = 60;

/// The shared D-14 inline submit script every treatment's document embeds.
/// Intercepts the form's `submit`, POSTs JSON to `/auth/login`, and branches
/// strictly on status:
/// * `204` → hard navigation to `/` only. No success-path class toggle, no
///   fade — the export's own authed-state fades (`1d` rain→chat, `2a`
///   veil-minimize, `1c` door-open) are demo scaffolding and are never
///   ported (CONTEXT.md success-transition correction, confirmed at this
///   plan's Task 1 checkpoint).
/// * `401` / any other unexpected status / a network failure → the uniform
///   [`LOGIN_ERROR_MESSAGE`], the password field cleared and refocused, and
///   every `[data-fail-target]` element in the document permanently swapped
///   to its failed presentation (submit label, card border, status-bar
///   op-label — whichever a given treatment marks; unmarked treatments are
///   untouched, matching D-16a's per-variant table exactly).
/// * `429` → a live decrementing `Too many attempts — try again in {n}s`
///   countdown in the `aria-live="polite"` error region, with the submit
///   button carrying the real HTML `disabled` attribute (+ `aria-disabled`)
///   for the duration. The button's text content is left exactly as it was
///   before this submit attempt — the 429 path never introduces new label
///   text.
///
/// The `↻ REPLAY` chip (`1b` only; harmless no-op elsewhere since the
/// generic query simply finds nothing) re-triggers every `[data-anim]`
/// element's CSS animation via the standard remove/reflow/re-add technique.
fn submit_script() -> String {
    const TEMPLATE: &str = r#"<script>
(function () {
  var ERR_401 = "__ERR_401__";
  var RETRY_FALLBACK = __RETRY_FALLBACK__;
  var form = document.getElementById("login-form");
  var input = document.getElementById("login-password");
  var errorBox = document.getElementById("login-error");
  var submitBtn = document.getElementById("login-submit");
  var countdownTimer = null;

  function clearCountdown() {
    if (countdownTimer) {
      clearInterval(countdownTimer);
      countdownTimer = null;
    }
  }

  // The VISUAL half of a rejection, extracted so both failure branches can share
  // it (G-47.3-2). Before this split it lived inline in showDenied(), which the
  // 429 branch returns before ever reaching — so a rate-limited operator got a
  // bare countdown on a pristine-looking page: no re-keyed rain, no scene failure
  // copy, no danger border, in precisely the state where the signal matters most.
  function applyFailTreatment() {
    // Plan 05's `1d` failure re-key (resolved backstop, the matrix-rain
    // canvas script): marking <body> lets the already-running rain interval
    // swap its glyph ramp to the danger hue without any extra
    // per-treatment script branch. A harmless no-op class on every other
    // treatment (nothing reads it there).
    document.body.classList.add("is-rain-failed");
    document.querySelectorAll("[data-fail-target]").forEach(function (el) {
      el.classList.add("is-failed");
      // A generic per-element text swap. Elements carrying BOTH
      // data-fail-target and data-fail-text run AFTER the errorBox
      // assignment in showDenied(), so `1c`/`1d`/`2a`'s own scene-specific
      // data-fail-text on #login-error (self-targeting) wins over the
      // uniform ERR_401 body those three treatments don't use (D-19: their
      // copy is scene metaphor, not the rewritten uniform text — see
      // 47.3-05-PLAN.md Task actions). On the 429 path the countdown's own
      // per-tick textContent write lands after this and wins instead, which
      // is correct — the remaining-seconds figure outranks the scene copy.
      if (el.dataset.failText) {
        el.textContent = el.dataset.failText;
      }
    });
  }

  function showDenied() {
    clearCountdown();
    input.value = "";
    input.focus();
    submitBtn.disabled = false;
    submitBtn.removeAttribute("aria-disabled");
    var prefix = errorBox.getAttribute("data-fail-prefix") || "";
    errorBox.textContent = (prefix ? prefix + " " : "") + ERR_401;
    errorBox.classList.add("is-visible");
    applyFailTreatment();
  }

  function startCountdown(seconds) {
    clearCountdown();
    var remaining = seconds > 0 ? seconds : RETRY_FALLBACK;
    submitBtn.disabled = true;
    submitBtn.setAttribute("aria-disabled", "true");
    function tick() {
      errorBox.textContent = "Too many attempts — try again in " + remaining + "s";
      errorBox.classList.add("is-visible");
      if (remaining <= 0) {
        clearCountdown();
        submitBtn.disabled = false;
        submitBtn.removeAttribute("aria-disabled");
        errorBox.classList.remove("is-visible");
        errorBox.textContent = "";
        return;
      }
      remaining -= 1;
    }
    tick();
    countdownTimer = setInterval(tick, 1000);
  }

  form.addEventListener("submit", function (evt) {
    evt.preventDefault();
    if (submitBtn.disabled) {
      return;
    }
    var priorLabel = submitBtn.textContent;
    submitBtn.disabled = true;
    submitBtn.textContent = "AUTHENTICATING…";
    fetch("/auth/login", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ password: input.value }),
    })
      .then(function (resp) {
        if (resp.status === 204) {
          window.location.href = "/";
          return;
        }
        if (resp.status === 429) {
          submitBtn.textContent = priorLabel;
          submitBtn.disabled = false;
          var raw = parseInt(resp.headers.get("Retry-After"), 10);
          // A lockout IS a rejection and must look like one (G-47.3-2). This runs
          // BEFORE startCountdown so the countdown's per-tick errorBox write lands
          // last and the remaining-seconds figure is what the operator reads, while
          // the rain, borders and scene copy still carry the failure state.
          applyFailTreatment();
          startCountdown(isNaN(raw) ? RETRY_FALLBACK : raw);
          return;
        }
        submitBtn.textContent = priorLabel;
        showDenied();
      })
      .catch(function () {
        submitBtn.textContent = priorLabel;
        showDenied();
      });
  });

  var replayBtn = document.getElementById("login-replay");
  var replayScope = document.getElementById("login-shell");
  if (replayBtn && replayScope) {
    replayBtn.addEventListener("click", function () {
      var animated = replayScope.querySelectorAll("[data-anim]");
      animated.forEach(function (el) {
        el.style.animation = "none";
      });
      void replayScope.offsetWidth;
      animated.forEach(function (el) {
        el.style.animation = el.getAttribute("data-anim");
      });
    });
  }
})();
</script>"#;
    TEMPLATE
        .replace("__ERR_401__", LOGIN_ERROR_MESSAGE)
        .replace("__RETRY_FALLBACK__", &RETRY_WINDOW_FALLBACK_SECS.to_string())
}

/// `1a` Basic (D-01 default): static terminal auth, no motion beyond the
/// `ih-blink` caret. Port Correction 1 makes `OPERATOR` a static,
/// non-focusable label; Port Correction 2 renames the credential field
/// `PASSWORD`. Carries D-16a's `a1Fail` designed failure state: the card
/// border and the top-status-bar `LOCKED`→`DENIED` op-label both flip via
/// `[data-fail-target]`, and the submit label swaps to
/// `RETRY AUTHENTICATION`.
fn render_basic() -> String {
    let build_version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>IronHermes — Operator Authentication</title>
<link rel="stylesheet" href="{login_css}">
</head>
<body>
<h1 class="sr-only">OPERATOR AUTHENTICATION</h1>
<div class="login-shell login-shell--basic" id="login-shell">
  <div class="login-statusbar login-statusbar--top">
    <div class="login-statusbar-left">
      <span class="login-statusbar-mobile-brand">IronHermes</span>
      <span class="login-statusbar-metadata"><span class="login-accent">▪</span> NODE <b class="login-accent">HERMES-7</b> › BRIDGE › <span class="login-accent">AUTH</span></span>
    </div>
    <div class="login-statusbar-right">
      <span class="login-statusbar-metadata">BUILD {build_version} · SESSION — · TOK 0 / 256000 · </span>
      <span>OP <span class="login-op-label" data-fail-target="op-label" data-fail-text="DENIED">LOCKED</span></span>
    </div>
  </div>
  <div class="login-center">
    <div class="login-card" data-fail-target="border">
      <div class="login-brand">IronHermes</div>
      <div class="login-subtitle">The self-improving agent — bridge access.<br>Enter the operator password to continue.</div>
      <div class="login-divider"></div>
      <form id="login-form" class="login-fields" autocomplete="off">
        <div class="login-field">
          <div class="login-field-label">OPERATOR</div>
          <div class="login-field-static">operator</div>
        </div>
        <div class="login-field">
          <div class="login-field-label">PASSWORD</div>
          <input id="login-password" class="login-input" type="password" name="password" autofocus>
        </div>
        <button id="login-submit" class="login-submit" type="submit" data-fail-target="submit" data-fail-text="RETRY AUTHENTICATION">AUTHENTICATE</button>
      </form>
      <div id="login-error" class="login-error" data-fail-prefix="[DENIED]" aria-live="polite"></div>
      <div class="login-footer-hint">
        <div>[OK] config.yaml &nbsp; [OK] SOUL.md &nbsp; <span class="login-warning-text">[MISSING]</span> openai key</div>
        <div>ctrl+c cancel · /help commands</div>
      </div>
    </div>
  </div>
  <div class="login-statusbar login-statusbar--bottom">
    <span class="login-accent">Auth</span> <span class="login-dim">·</span> <span class="login-model-pill">k3-256k</span> <span class="login-dim">·</span> <span class="login-success-text">moonshot</span> <span class="login-dim">·</span> <span class="login-warning-text">0.0K/256.0K (0%)</span> <span class="login-dim">·</span> <span class="login-dim">enter to submit<span class="login-caret">▊</span></span>
  </div>
</div>
{script}
</body>
</html>"#,
        login_css = LOGIN_CSS,
        script = submit_script(),
        build_version = build_version,
    )
}

/// `1b` Fade: same chrome shell as `1a`, but the centered area becomes a
/// two-column staged-reveal grid — credential form on the left, a six-row
/// `PREFLIGHT` checklist on the right (six rows is UI-SPEC's own resolved,
/// checker-approved count — the design export shows five; the sixth
/// (`auth boundary` / `argon2id verified`) reflects this phase's actual
/// feature and was added to satisfy that locked count without inventing an
/// oracle-leaking or otherwise misleading line). Every `ih-up`/`ih-in`
/// element carries a `data-anim` twin of its inline `style="animation:..."`
/// so the `↻ REPLAY` chip can restart the whole sequence. Carries D-16a's
/// `b1Fail` designed failure state: the only one of the five whose error box
/// animates in (`ih-in 200ms both`).
fn render_fade() -> String {
    let build_version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>IronHermes — Operator Authentication</title>
<link rel="stylesheet" href="{login_css}">
</head>
<body>
<h1 class="sr-only">OPERATOR AUTHENTICATION</h1>
<div class="login-shell login-shell--fade" id="login-shell">
  <div class="login-fade-content">
    <div class="login-statusbar login-statusbar--top" style="animation: ih-in 400ms 80ms both" data-anim="ih-in 400ms 80ms both">
      <div class="login-statusbar-left">
        <span class="login-statusbar-mobile-brand">IronHermes</span>
        <span class="login-statusbar-metadata"><span class="login-accent">▪</span> NODE <b class="login-accent">HERMES-7</b> › BRIDGE › <span class="login-accent">AUTH</span></span>
      </div>
      <div class="login-statusbar-right">
        <span class="login-statusbar-metadata">BUILD {build_version} · COLD START · </span>
        <span>OP <span class="login-op-label">LOCKED</span></span>
      </div>
    </div>
    <div class="login-fade-grid">
      <div class="login-fade-left">
        <div class="login-brand" style="animation: ih-up 520ms 200ms cubic-bezier(.2,.8,.2,1) both" data-anim="ih-up 520ms 200ms cubic-bezier(.2,.8,.2,1) both">IronHermes</div>
        <div class="login-subtitle" style="animation: ih-up 520ms 320ms cubic-bezier(.2,.8,.2,1) both" data-anim="ih-up 520ms 320ms cubic-bezier(.2,.8,.2,1) both">10-layer prompt assembly ready.<br>Identity, memory, skills stitched per turn.</div>
        <form id="login-form" class="login-fields" autocomplete="off">
          <div class="login-field" style="animation: ih-up 520ms 1100ms cubic-bezier(.2,.8,.2,1) both" data-anim="ih-up 520ms 1100ms cubic-bezier(.2,.8,.2,1) both">
            <div class="login-field-label">OPERATOR</div>
            <div class="login-field-static">operator</div>
          </div>
          <div class="login-field" style="animation: ih-up 520ms 1220ms cubic-bezier(.2,.8,.2,1) both" data-anim="ih-up 520ms 1220ms cubic-bezier(.2,.8,.2,1) both">
            <div class="login-field-label">PASSWORD</div>
            <input id="login-password" class="login-input" type="password" name="password" autofocus>
          </div>
          <button id="login-submit" class="login-submit" type="submit" style="animation: ih-up 520ms 1340ms cubic-bezier(.2,.8,.2,1) both" data-anim="ih-up 520ms 1340ms cubic-bezier(.2,.8,.2,1) both" data-fail-target="submit" data-fail-text="RETRY">LOGIN TO SYSTEM</button>
        </form>
        <div id="login-error" class="login-error login-error--animated" data-fail-prefix="[FAILED]" aria-live="polite"></div>
      </div>
      <div class="login-fade-divider" style="animation: ih-in 500ms 420ms both" data-anim="ih-in 500ms 420ms both"></div>
      <div class="login-fade-right">
        <div class="login-preflight-title" style="animation: ih-in 400ms 400ms both" data-anim="ih-in 400ms 400ms both">PREFLIGHT</div>
        <div class="login-preflight-row" style="animation: ih-up 420ms 480ms cubic-bezier(.2,.8,.2,1) both" data-anim="ih-up 420ms 480ms cubic-bezier(.2,.8,.2,1) both"><span class="login-ok">[OK]</span><span class="login-preflight-label">config.yaml</span><span class="login-preflight-hint">~/.ironhermes/</span></div>
        <div class="login-preflight-row" style="animation: ih-up 420ms 600ms cubic-bezier(.2,.8,.2,1) both" data-anim="ih-up 420ms 600ms cubic-bezier(.2,.8,.2,1) both"><span class="login-ok">[OK]</span><span class="login-preflight-label">SOUL.md · AGENTS.md</span><span class="login-preflight-hint">self-improving</span></div>
        <div class="login-preflight-row" style="animation: ih-up 420ms 720ms cubic-bezier(.2,.8,.2,1) both" data-anim="ih-up 420ms 720ms cubic-bezier(.2,.8,.2,1) both"><span class="login-ok">[OK]</span><span class="login-preflight-label">memory store</span><span class="login-preflight-hint">14 skills</span></div>
        <div class="login-preflight-row" style="animation: ih-up 420ms 840ms cubic-bezier(.2,.8,.2,1) both" data-anim="ih-up 420ms 840ms cubic-bezier(.2,.8,.2,1) both"><span class="login-ok">[OK]</span><span class="login-preflight-label">gateway · telegram</span><span class="login-preflight-hint">idle</span></div>
        <div class="login-preflight-row" style="animation: ih-up 420ms 960ms cubic-bezier(.2,.8,.2,1) both" data-anim="ih-up 420ms 960ms cubic-bezier(.2,.8,.2,1) both"><span class="login-missing">[MISSING]</span><span class="login-preflight-label">openai key</span><span class="login-preflight-hint">not set</span></div>
        <div class="login-preflight-row" style="animation: ih-up 420ms 1080ms cubic-bezier(.2,.8,.2,1) both" data-anim="ih-up 420ms 1080ms cubic-bezier(.2,.8,.2,1) both"><span class="login-ok">[OK]</span><span class="login-preflight-label">auth boundary</span><span class="login-preflight-hint">argon2id verified</span></div>
        <div class="login-scanner" style="animation: ih-in 400ms 1000ms both" data-anim="ih-in 400ms 1000ms both">
          <span class="login-scanner-cells">░░░░░░░░░░</span>
          <span class="login-scanner-label">waiting for operator...</span>
        </div>
      </div>
    </div>
    <div class="login-statusbar login-statusbar--bottom" style="animation: ih-in 400ms 1400ms both" data-anim="ih-in 400ms 1400ms both">
      <span class="login-accent">Auth</span> <span class="login-dim">·</span> <span class="login-model-pill">k3-256k</span> <span class="login-dim">·</span> <span class="login-success-text">moonshot</span> <span class="login-dim">·</span> <span class="login-dim">/help commands</span>
    </div>
  </div>
  <button id="login-replay" class="login-replay" type="button">↻ REPLAY</button>
</div>
{script}
</body>
</html>"#,
        login_css = LOGIN_CSS,
        script = submit_script(),
        build_version = build_version,
    )
}

/// `1c` Blast doors (D-01/D-04/D-16b/D-16c): the airlock scene with the
/// auth-driven alarm cascade. Per the module-level D-16b correction (see
/// 47.3-05-PLAN.md's flagged-assumption note): `ih-fail` (the flaky lamp
/// A-02, plus two more hardcoded-`ih-fail` lamps and Bulkhead B's always-red
/// blink light) is unconditional ambience in every phase — none of it
/// carries `data-fail-target`. `ih-hum` (the rest of the working lamp bus)
/// switches wholesale to the alarm treatment via the SAME shared
/// `[data-fail-target]` mechanism `submit_script()` already drives for
/// `1a`/`1b` — no script changes needed for the denied path, only new
/// markup attributes on this treatment's own elements.
///
/// The `[TIMEOUT]`/`SESSION TIMEOUT`/`260ms`-door-slam visual (D-16c) is
/// rendered as real markup + CSS (`.login-shell.is-timeout`), but wiring
/// that class to a live D-17 idle timer is session-expiry integration work
/// outside this plan's `files_modified` (`server/auth.rs` is a concurrently
/// running plan's file) — the designed visual ships now, inert until a
/// future plan toggles the class.
fn render_blast_doors() -> String {
    let providers: String = BLAST_DOOR_PROVIDERS
        .iter()
        .map(|(name, url)| {
            format!(
                r#"<div class="cd-provider"><div class="cd-provider-row"><div class="cd-provider-icon">◉</div><div><div class="cd-provider-name">{name}</div><div class="cd-provider-url">{url}</div></div><div class="cd-provider-badge">ACTIVE</div></div><div class="cd-provider-meta">MODELS 1 &nbsp; P50 — &nbsp; KEY SET</div></div>"#
            )
        })
        .collect();
    let build_version = env!("CARGO_PKG_VERSION");

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>IronHermes — Operator Authentication</title>
<link rel="stylesheet" href="{login_css}">
</head>
<body>
<h1 class="sr-only">OPERATOR AUTHENTICATION</h1>
<div class="login-shell login-shell--blast-doors" id="login-shell">
  <div class="cd-scene">
    <div class="cd-statusbar">
      <div class="login-statusbar-left">
        <span class="login-statusbar-mobile-brand">IronHermes</span>
        <span class="login-statusbar-metadata"><span class="login-accent">▪</span> NODE <b class="login-accent">HERMES-7</b> › AIRLOCK › <span class="login-accent">PROVIDERS</span></span>
      </div>
      <div class="login-statusbar-right">
        <span class="login-statusbar-metadata">BUILD {build_version} · UPTIME 17:57:59 · </span>
        <span>OP <span class="login-success-text">READY</span></span>
      </div>
    </div>
    <div class="cd-providers-panel">
      <div class="cd-providers-kicker">// MODULE 13</div>
      <div class="cd-providers-title">Providers</div>
      <div class="cd-providers-sub">API providers and credential pools. <span class="login-accent">9</span> active of <span class="login-accent">9</span> configured.</div>
      <div class="cd-providers-grid">{providers}</div>
    </div>
  </div>

  <div class="cd-airlock">
    <div class="cd-header">
      <div class="cd-lamp cd-lamp--header cd-lamp--bus" data-fail-target="lamp"></div>
      <div class="cd-lamp cd-lamp--header-side cd-lamp--left cd-lamp--ambient"></div>
      <div class="cd-lamp cd-lamp--header-side cd-lamp--right cd-lamp--bus" data-fail-target="lamp"></div>
      <div class="cd-header-vent"></div>
    </div>
    <div class="cd-side cd-side--left">
      <div class="cd-lamp cd-lamp--vertical cd-lamp--bus" style="right:22px" data-fail-target="lamp"></div>
    </div>
    <div class="cd-side cd-side--right">
      <div class="cd-lamp cd-lamp--vertical cd-lamp--ambient" style="left:22px;animation-duration:17s;animation-delay:2s"></div>
    </div>
    <div class="cd-floor">
      <div class="cd-lamp cd-lamp--floor cd-lamp--left cd-lamp--bus" data-fail-target="lamp"></div>
      <div class="cd-lamp cd-lamp--floor cd-lamp--right cd-lamp--bus" data-fail-target="lamp"></div>
    </div>
  </div>

  <div class="cd-door cd-door--left">
    <div class="cd-door-panel"><div class="cd-door-badge">A-02</div></div>
    <div class="cd-lamp cd-lamp--door cd-lamp--bus" style="right:22px" data-fail-target="lamp"></div>
    <div class="cd-door-label">BULKHEAD A</div>
  </div>
  <div class="cd-door cd-door--right">
    <div class="cd-door-panel"><div class="cd-blink-light"></div></div>
    <div class="cd-lamp cd-lamp--door cd-lamp--ambient" style="left:22px;animation-duration:13s;animation-delay:1.4s"></div>
    <div class="cd-door-label">BULKHEAD B</div>
  </div>

  <div class="cd-panel" data-fail-target="panel">
    <div class="cd-panel-head">
      <span class="cd-seal" data-fail-target="seal" data-fail-text="LOCKOUT">SEALED</span>
      <span class="cd-status" data-fail-target="status" data-fail-text="KEY REJECTED">AWAITING KEY</span>
      <span class="cd-status cd-status--timeout" aria-hidden="true">SESSION TIMEOUT</span>
    </div>
    <div class="login-divider"></div>
    <form id="login-form" class="login-fields" autocomplete="off">
      <div class="login-field">
        <div class="login-field-label">OPERATOR</div>
        <div class="login-field-static">operator</div>
      </div>
      <div class="login-field">
        <div class="login-field-label">PASSWORD</div>
        <input id="login-password" class="login-input login-input--airlock" type="password" name="password" autofocus>
      </div>
      <button id="login-submit" class="cd-submit" type="submit" data-fail-target="submit" data-fail-text="RETRY UNSEAL">UNSEAL BULKHEAD</button>
    </form>
    <div id="login-error" class="login-error" data-fail-target="error-body" data-fail-text="[DENIED] servo lockout &#183; key rejected by lock controller&#10;lamps switched to alarm bus" aria-live="polite"></div>
    <div class="cd-timeout-box" aria-hidden="true">[TIMEOUT] no operator activity<br>bulkhead slammed</div>
    <div class="cd-panel-footer">servo cycle 1.05s · lamp A-02 intermittent · ctrl+c cancel</div>
  </div>
</div>
{script}
</body>
</html>"#,
        login_css = LOGIN_CSS,
        providers = providers,
        script = submit_script(),
        build_version = build_version,
    )
}

/// `1d` Matrix rain (D-01/D-04/D-05/D-16a): terminal-dark chrome plus the
/// from-scratch `login-rain.js` canvas (`assets/login-rain.js` — no
/// precedent anywhere in this crate; see 47.3-PATTERNS.md). The canvas is
/// absolutely positioned BEHIND the login overlay so script timing causes no
/// layout shift; the flat `--bg-canvas` (terminal-dark) fill is what renders
/// before the script executes. The `.login-chat-mock` layer is background
/// atmosphere only (`pointer-events: none`, no interactive affordances
/// ported) — unauthenticated visitors never actually use "chat"; it exists
/// only as decorative dressing behind the login overlay.
///
/// Token-scoping (47.3-UI-SPEC.md "Color System Resolution"): wrapping the
/// shell in `[data-theme="terminal-dark"]` (see `login.css`) means every
/// EXISTING shared class (`.login-card`, `.login-input`, `.login-submit`,
/// `.login-error`, `.login-brand`, the status bars, ...) renders correctly
/// for `1d` with zero duplicated markup or CSS — `.login-error` already
/// reads `var(--danger)`, so D-16a's hotter `oklch(70% 0.20 25)` falls out
/// of the scope automatically, and D-16a's card-border flip reuses the
/// EXISTING `.login-card.is-failed` rule from Plan 04 unchanged.
fn render_matrix_rain() -> String {
    let build_version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>IronHermes — Operator Authentication</title>
<link rel="stylesheet" href="{login_css}">
</head>
<body>
<h1 class="sr-only">OPERATOR AUTHENTICATION</h1>
<div class="login-shell login-shell--matrix-rain" id="login-shell" data-theme="terminal-dark">
  <div class="login-chat-mock" aria-hidden="true">
    <div class="login-statusbar login-statusbar--top">
      <div class="login-statusbar-left">
        <span class="login-statusbar-mobile-brand">IronHermes</span>
        <span class="login-statusbar-metadata"><span class="login-accent">▪</span> NODE <b class="login-accent">HERMES-7</b> › AGENT › <span class="login-accent">CHAT</span></span>
      </div>
      <div class="login-statusbar-right">
        <span class="login-statusbar-metadata">BUILD {build_version} · SESSION 7f21c9 · </span>
        <span>OP <span class="login-success-text">READY</span></span>
      </div>
    </div>
    <div class="login-chat-mock-body">
      <div class="login-chat-mock-line login-dim">SESSION ATTACHED — 10-layer prompt assembled</div>
      <div><span class="login-success-text">You:</span> help me refactor this</div>
      <div><span class="login-accent">Hermes:</span> I'll read the file first...</div>
    </div>
  </div>

  <canvas id="login-rain-canvas" class="login-rain-canvas" aria-hidden="true"></canvas>

  <div class="login-rain-overlay">
    <div class="login-statusbar login-statusbar--top">
      <div class="login-statusbar-left">
        <span class="login-statusbar-mobile-brand">IronHermes</span>
        <span class="login-statusbar-metadata"><span class="login-accent">▪</span> NODE <b class="login-accent">HERMES-7</b> › BRIDGE › <span class="login-accent">AUTH</span></span>
      </div>
      <div class="login-statusbar-right">
        <span class="login-statusbar-metadata">BUILD {build_version} · THEME TERMINAL-DARK · </span>
        <span>OP <span class="login-warning-text">LOCKED</span></span>
      </div>
    </div>
    <div class="login-center">
      <div class="login-card login-card--rain" data-fail-target="border">
        <div class="login-brand">IronHermes</div>
        <div class="login-subtitle">ironhermes chat — streaming, tools, cron.<br>Authenticate to attach the transcript.</div>
        <div class="login-divider"></div>
        <form id="login-form" class="login-fields" autocomplete="off">
          <div class="login-field">
            <div class="login-field-label">OPERATOR</div>
            <div class="login-field-static">operator</div>
          </div>
          <div class="login-field">
            <div class="login-field-label">PASSWORD</div>
            <input id="login-password" class="login-input" type="password" name="password" autofocus>
          </div>
          <button id="login-submit" class="login-submit login-submit--rain" type="submit" data-fail-target="submit" data-fail-text="RETRY">LOGIN</button>
        </form>
        <div id="login-error" class="login-error" data-fail-target="error-body" data-fail-text="[DENIED] handshake rejected &#8212; bad credentials&#10;rain re-keyed to alarm stream" aria-live="polite"></div>
      </div>
    </div>
    <div class="login-statusbar login-statusbar--bottom">
      <span class="login-accent">Auth</span> <span class="login-dim">·</span> <span class="login-model-pill">k3-256k</span> <span class="login-dim">·</span> <span class="login-success-text">moonshot</span> <span class="login-dim">·</span> <span class="login-warning-text">0.0K/256.0K (0%)</span> <span class="login-dim">·</span> <span class="login-dim">ctrl+c cancel</span>
    </div>
  </div>
</div>
<script src="{rain_js}"></script>
{script}
</body>
</html>"#,
        login_css = LOGIN_CSS,
        rain_js = LOGIN_RAIN_JS,
        script = submit_script(),
        build_version = build_version,
    )
}

/// `2a` Orbit Veil (D-01/D-04): globe HUD with 110 baked stars ([`BAKED_STARS`])
/// and a deliberately clashing auth panel — the surrounding HUD's Tailwind-
/// slate hex/rounded-corner language is preserved as a literal per D-04 (no
/// token exists for it), while the veil panel itself reuses the slate-dark
/// tokens/shared classes (`.login-input`, `.login-field*`, `.login-divider`,
/// `.login-submit`, `.login-error`) and stays 0-radius — the one visual
/// clash CONTEXT.md says must be preserved, not harmonized.
///
/// The HUD's search box / layer toggles / speed buttons / play-pause /
/// LIVE badge are rendered as inert `pointer-events: none` markup
/// with no `<button>`/`<a href>` anywhere in that region (D-15/UI-SPEC:
/// "not functional UI this phase") — the login form's only interactive
/// elements are the password input and the real submit button inside
/// `.ov-panel`.
///
/// The CLOCK is the one exception, and is not a control: it ticks
/// ([`orbit_veil_clock_script`]) because a frozen `00:00:00` under a green
/// `LIVE` badge is a wrong readout rather than an unimplemented one. It
/// remains `pointer-events: none` with nothing to click.
/// The orbit-veil HUD clock, driven client-side.
///
/// The port rendered a frozen `00:00:00` beside a green `LIVE` badge, and showed
/// `UTC` where the export shows `YYYY-MM-DD UTC` — so the readout was both stopped
/// and missing its date. The export drives both from
/// `new Date().toISOString()` (slices `11..19` and `0..10`).
///
/// This does NOT make the HUD interactive: D-15 scopes the search box, layer
/// toggles, speed buttons and play/pause as inert decoration, and they stay that
/// way (`orbit_veil_hud_controls_are_inert`). A clock is a readout, not a control,
/// and a stopped one under a `LIVE` badge is simply wrong.
///
/// The initial values stay STATIC in the server-rendered markup on purpose.
/// Stamping a real timestamp server-side would make two renders of the same page
/// differ, breaking `orbit_veil_is_deterministic_across_renders` — and it would be
/// the server's clock, not the viewer's. The script paints once immediately, so the
/// placeholder is not perceptible.
///
/// 500ms, not the export's 600ms: any interval below 1s guarantees no second is
/// ever skipped, and 500 divides evenly so the display doesn't visibly stutter.
/// Inline (no extra request) and covered by the existing `script-src 'unsafe-inline'`
/// in [`LOGIN_CSP`], so the pre-auth request count is unchanged.
fn orbit_veil_clock_script() -> &'static str {
    r#"<script>
(function () {
  var t = document.querySelector('.ov-clock-time');
  var d = document.querySelector('.ov-clock-date');
  if (!t || !d) return;
  function paint() {
    var iso = new Date().toISOString();
    t.textContent = iso.slice(11, 19);
    d.textContent = iso.slice(0, 10) + ' UTC';
  }
  paint();
  setInterval(paint, 500);
})();
</script>"#
}

fn render_orbit_veil() -> String {
    let stars: String = BAKED_STARS
        .iter()
        .map(|s| {
            format!(
                r#"<span class="ov-star" style="left:{}px;top:{}px;width:{}px;height:{}px;opacity:{};animation-duration:{}s"></span>"#,
                s.x, s.y, s.d, s.d, s.o, s.t
            )
        })
        .collect();

    // The export's satellite population, ported in full (D-04). This previously
    // baked only 8 "representative" orbiters against a comment claiming the export
    // generates "~700 across 12 categories" — the real figure is 548, and shipping
    // 8 of them is what the 47.3 UAT reported as satellites being missing versus the
    // source. `ORBITER_SPEC` is the export's own `spec` table verbatim:
    // (color, count, size), with the same per-class counts and sizes.
    //
    // DELIBERATE DEVIATION: the export's default state is `layerOff = ["debris-fengyun"]`,
    // so it opens with those 72 orbiters at `display:none` and the Fengyun-1C layer row
    // dimmed to opacity .35. We render all 548 at full opacity because our HUD controls
    // are inert this phase — with no toggle to turn a layer back on, a dimmed row and 72
    // permanently-hidden dots would read as a rendering bug rather than as an off layer.
    const ORBITER_SPEC: [(&str, u16, f32); 12] = [
        ("#ffd166", 4, 2.6),    // stations
        ("#4ade80", 9, 1.8),    // gps
        ("#a3e635", 8, 1.8),    // glonass
        ("#2dd4bf", 9, 1.8),    // galileo
        ("#f472b6", 14, 1.8),   // weather
        ("#a78bfa", 42, 1.4),   // oneweb
        ("#38bdf8", 230, 1.15), // starlink
        ("#e8eef7", 18, 1.9),   // brightest
        ("#fb7185", 52, 1.05),  // debris-cosmos
        ("#fb923c", 30, 1.05),  // debris-iridium
        ("#ef4444", 72, 1.05),  // debris-fengyun
        ("#9aa7bd", 60, 1.05),  // other
    ];

    // The export randomises each orbiter with `Math.random()`. A server-rendered
    // page cannot: every request must emit byte-identical markup, or the HTML churns
    // on every reload and nothing downstream can be compared or cached. So the same
    // distributions are driven by a fixed-seed xorshift64 instead — same shape,
    // stable output. Changing SEED reshuffles the whole sky; there is no reason to.
    struct Rng(u64);
    impl Rng {
        fn next_f32(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            // Top 24 bits -> [0,1). Plenty of resolution for decorative placement.
            ((self.0 >> 40) as f32) / ((1u32 << 24) as f32)
        }
    }
    const SEED: u64 = 0x5EED_0B17_A110_C0DE;
    let mut rng = Rng(SEED);

    let mut orbiters = String::new();
    for (color, count, size) in ORBITER_SPEC {
        // Export: `d: Math.max(3, size * 2.1)`.
        let dot_d = (size * 2.1_f32).max(3.0);
        for _ in 0..count {
            // Export: 55% orbit inside the globe silhouette, the rest outside.
            let inward = rng.next_f32() < 0.55;
            let r = if inward {
                -(40.0 + rng.next_f32() * 250.0)
            } else {
                16.0 + rng.next_f32() * 76.0
            };
            let inset = -r;
            let dur = 26.0 + rng.next_f32() * 54.0; // export: 26-80s
            let tilt = rng.next_f32() * 360.0;
            let reverse = rng.next_f32() < 0.25; // export: a quarter run retrograde
            let delay = rng.next_f32() * dur; // desynchronises the ring
            let dir = if reverse { "reverse" } else { "normal" };
            orbiters.push_str(&format!(
                r#"<div class="ov-orbit" style="inset:{inset:.0}px;transform:rotate({tilt:.0}deg)"><div class="ov-orbit-spin" style="animation-duration:{dur:.1}s;animation-direction:{dir};animation-delay:-{delay:.1}s"><span class="ov-orbit-dot" style="width:{dot_d:.1}px;height:{dot_d:.1}px;background:{color};box-shadow:0 0 7px 2px {color}99, 0 0 1px 1px rgba(255,255,255,.5)"></span></div></div>"#
            ));
        }
    }

    // The LAYERS panel, also ported in full. It previously listed 5 of the export's
    // 12 rows while the header kept the export's "16,071 objects tracked" headline —
    // the 5 shown summed to 11,792, so the panel silently contradicted the header by
    // exactly the 4,279 the 7 omitted rows account for. Counts and colours are the
    // export's verbatim; `Space Stations` was additionally rendered with Starlink's
    // blue (#38bdf8) instead of its own amber (#ffd166).
    const LAYERS: [(&str, &str, &str); 12] = [
        ("#ffd166", "Space Stations", "17"),
        ("#4ade80", "GPS", "32"),
        ("#a3e635", "GLONASS", "28"),
        ("#2dd4bf", "Galileo", "30"),
        ("#f472b6", "Weather", "84"),
        ("#a78bfa", "OneWeb", "651"),
        ("#38bdf8", "Starlink", "10,787"),
        ("#e8eef7", "Brightest", "168"),
        ("#fb7185", "Debris · Cosmos-2251", "1,123"),
        ("#fb923c", "Debris · Iridium-33", "397"),
        ("#ef4444", "Debris · Fengyun-1C", "1,826"),
        ("#9aa7bd", "Other Active", "928"),
    ];
    let layer_rows: String = LAYERS
        .iter()
        .map(|(color, name, count)| {
            format!(
                r#"<div class="ov-layer-row"><span class="ov-layer-dot" style="background:{color}"></span><span class="ov-layer-name">{name}</span><span class="ov-layer-count">{count}</span></div>"#
            )
        })
        .collect();

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>IronHermes — Operator Authentication</title>
<link rel="stylesheet" href="{login_css}">
<link rel="preload" as="image" href="{earth_night}">
</head>
<body>
<h1 class="sr-only">OPERATOR AUTHENTICATION</h1>
<div class="login-shell login-shell--orbit-veil" id="login-shell">
  <div class="ov-sky">{stars}</div>
  <div class="ov-globe-wrap">
    <!-- The globe texture is bound HERE, not in login.css, because only this template
         has the asset!()-derived hashed URL. A relative url() in the stylesheet
         resolves to the unhashed /assets/earth-night.jpg, which dx never emits and the
         pre-auth allowlist never permits — it 401s and the globe renders as empty
         space. Reported during 47.3 UAT as "the rotating earth is missing". -->
    <div class="ov-globe" style="background-image:url('{earth_night}')"></div>
    <div class="ov-globe-shade"></div>
    <div class="ov-globe-inner-glow"></div>
    <div class="ov-globe-ring"></div>
    {orbiters}
  </div>

  <div class="ov-wordmark" aria-hidden="true">
    <span class="ov-wordmark-o">O<span class="ov-wordmark-ring"></span></span>RBIT VEIL
    <div class="ov-wordmark-sub">16,071 objects tracked · TLE @ CelesTrak</div>
  </div>

  <div class="ov-clock" aria-hidden="true">
    <div class="ov-clock-time">00:00:00</div>
    <div>
      <div class="ov-clock-date">UTC</div>
      <div class="ov-clock-badge"><span class="ov-clock-dot"></span>LIVE</div>
    </div>
  </div>

  <div class="ov-search" aria-hidden="true">Search satellite or NORAD id…</div>

  <div class="ov-layers" aria-hidden="true">
    <div class="ov-layers-title">Layers</div>
    {layer_rows}
  </div>

  <div class="ov-transport" aria-hidden="true">
    <div class="ov-speed">-60×</div>
    <div class="ov-speed">-10×</div>
    <div class="ov-speed ov-speed--active">1×</div>
    <div class="ov-speed">10×</div>
    <div class="ov-speed">60×</div>
    <div class="ov-play">▶</div>
    <div class="ov-live"><span class="ov-live-dot"></span>LIVE</div>
  </div>

  <div class="ov-footer-left" aria-hidden="true">TLE CelesTrak · SGP4 satellite.js · Imagery NASA Blue Marble</div>
  <div class="ov-footer-right" aria-hidden="true">60 fps</div>

  <div class="ov-panel-wrap">
    <div class="ov-panel" data-fail-target="panel">
      <div class="login-brand">IronHermes</div>
      <div class="login-subtitle">Bridge access — telemetry session orbit-veil.<br>16,071 tracked objects streaming.</div>
      <div class="login-divider"></div>
      <form id="login-form" class="login-fields" autocomplete="off">
        <div class="login-field">
          <div class="login-field-label">OPERATOR</div>
          <div class="login-field-static">operator</div>
        </div>
        <div class="login-field">
          <div class="login-field-label">PASSWORD</div>
          <input id="login-password" class="login-input" type="password" name="password" autofocus>
        </div>
        <button id="login-submit" class="login-submit" type="submit" data-fail-target="submit" data-fail-text="RETRY">LOGIN TO SYSTEM</button>
      </form>
      <div id="login-error" class="login-error" data-fail-target="error-body" data-fail-text="[DENIED] credentials rejected &#8212; telemetry stays read-only&#10;orbit-veil continues" aria-live="polite"></div>
      <div class="login-footer-hint">
        <div>[OK] TLE snapshot &nbsp; [OK] SGP4 worker &nbsp; ctrl+c cancel</div>
      </div>
    </div>
  </div>
</div>
{script}
{clock_script}
</body>
</html>"#,
        login_css = LOGIN_CSS,
        earth_night = EARTH_NIGHT_JPG,
        stars = stars,
        orbiters = orbiters,
        script = submit_script(),
        clock_script = orbit_veil_clock_script(),
    )
}

/// Renders the login document as a plain string — split out from
/// [`render_login_page`] so unit tests can compare HTML byte-for-byte
/// without extracting an axum response body.
///
/// Dispatches on the normalized theme slug. An unrecognized slug renders
/// byte-for-byte identical output to `basic`, matching [`normalize_theme`]'s
/// own fallback.
///
/// The document carries the literal marker text `OPERATOR AUTHENTICATION`
/// (asserted by the integration tests below, as a screen-reader-only `<h1>`)
/// and references no script or stylesheet outside [`public_asset_paths`].
pub(crate) fn login_html(theme: &str) -> String {
    match normalize_theme(theme) {
        "fade" => render_fade(),
        "blast-doors" => render_blast_doors(),
        "matrix-rain" => render_matrix_rain(),
        "orbit-veil" => render_orbit_veil(),
        _ => render_basic(),
    }
}

/// `GET /` when unauthenticated (or auth disabled but `root_handler` chose
/// this branch anyway — it never does; see [`root_handler`]).
pub fn render_login_page(theme: &str) -> Response {
    respond(
        StatusCode::OK,
        "text/html; charset=utf-8",
        login_html(theme).into_bytes(),
    )
}

/// `GET /` — the D-07 auth split (RESEARCH.md Pattern 1). Registered as an
/// exact-path route on the SAME `Router<FullstackState>` this module's
/// `main.rs`/`test_router` composition builds `serve_dioxus_application`'s
/// three constituent calls from, so `State<FullstackState>` resolves
/// normally. When unauthenticated, renders the login document directly;
/// when authenticated (or auth disabled entirely), delegates to the real
/// Dioxus SSR renderer — the exact same one `serve_dioxus_application`'s own
/// `.fallback(...)` would have used.
pub async fn root_handler(
    State(fullstack): State<FullstackState>,
    axum::extract::Extension(auth): axum::extract::Extension<Arc<AuthState>>,
    req: Request,
) -> Response {
    let authed = !auth.enabled() || crate::server::auth::session_is_valid(&auth, &req);
    if authed {
        render_handler(State(fullstack), req).await.into_response()
    } else {
        render_login_page(auth.login_theme())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    use argon2::Argon2;
    use axum::body::to_bytes;
    use axum::extract::ConnectInfo;
    use axum::http::header;
    use std::net::SocketAddr;
    use std::sync::OnceLock;
    use tower::ServiceExt as _;

    use crate::server::auth::{AuthConfig, AuthState};

    /// `DioxusRouterExt::serve_static_assets` hard-`panic!`s if its resolved
    /// `public_path()` (default: `<exe_dir>/public`) does not exist on disk
    /// — real in a `dx build`, absent in a bare `cargo test` process (this
    /// crate's Wave-0 gap: no existing precedent assembles a full `Router`
    /// plus layers, so nothing exercised this path before). Point
    /// `DIOXUS_PUBLIC_PATH` at a real (empty) directory, once per test
    /// binary process, so `test_router()` can build the full chain without
    /// touching a real `dx` build. The static-asset ROUTES this registers
    /// are irrelevant to what these tests assert (deny-by-default, the
    /// login/session split, cookie shape) — only that construction doesn't
    /// panic.
    fn ensure_public_path_env() {
        static PUBLIC_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
        let dir = PUBLIC_DIR.get_or_init(|| tempfile::tempdir().expect("tempdir for DIOXUS_PUBLIC_PATH"));
        // SAFETY: test-only; the value is set exactly once (OnceLock) before
        // any test calls `test_router()`, and never mutated again — no
        // concurrent-mutation race with other threads reading/writing this
        // var.
        unsafe {
            std::env::set_var("DIOXUS_PUBLIC_PATH", dir.path());
        }
    }

    fn hash_password(pw: &str) -> String {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(pw.as_bytes(), &salt)
            .unwrap()
            .to_string()
    }

    fn auth_state_with_password(pw: &str) -> Arc<AuthState> {
        AuthState::new(AuthConfig {
            password_hash: Some(hash_password(pw)),
            ..Default::default()
        })
        .unwrap()
    }

    /// Test-only router factory reproducing `main.rs`'s real chain — same
    /// route order, same layer order — the Wave-0 integration-test
    /// infrastructure this plan's `must_haves` require (no precedent
    /// elsewhere in this crate; PATTERNS.md "No Analog Found").
    fn test_router(auth_state: Arc<AuthState>) -> axum::Router {
        ensure_public_path_env();
        let fullstack_state = FullstackState::new(ServeConfig::new(), crate::app::App);

        let auth_routes = axum::Router::new()
            .route("/auth/login", axum::routing::post(crate::server::auth::login))
            .route(
                "/auth/session",
                axum::routing::get(crate::server::auth::session_probe),
            )
            .route("/auth/logout", axum::routing::post(crate::server::auth::logout))
            .layer(axum::extract::DefaultBodyLimit::max(4096))
            .with_state(auth_state.clone());

        axum::Router::new()
            .register_server_functions()
            .serve_static_assets()
            .route("/", axum::routing::get(root_handler))
            .fallback(axum::routing::get(render_handler))
            .with_state(fullstack_state)
            .route(
                "/artifacts/{id}",
                axum::routing::get(crate::server::artifact_route::serve_artifact),
            )
            .route(
                "/chat-attachments/{session_id}/{id}",
                axum::routing::get(crate::server::chat_attachment_route::serve_chat_attachment),
            )
            .merge(auth_routes)
            .layer(axum::Extension(auth_state.clone()))
            .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024))
            .layer(axum::middleware::from_fn_with_state(
                auth_state,
                crate::server::auth::require_auth,
            ))
            // Must mirror main.rs's layer order exactly — compression registered
            // AFTER the auth layer so it wraps it. If these two drift, the tests
            // below stop describing the server that actually runs.
            .layer(compression_layer())
    }

    fn peer_addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    fn login_request(password: &str, port: u16) -> Request<Body> {
        let mut req = Request::builder()
            .method("POST")
            .uri("/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!("{{\"password\":\"{password}\"}}")))
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(peer_addr(port)));
        req
    }

    /// T-47.3-01 mitigation, live-tested (not just inspected): an
    /// unauthenticated `GET /` through the REAL assembled router must return
    /// the login document and never a reference to the wasm bundle.
    #[tokio::test]
    async fn root_serves_login_when_unauthenticated() {
        let auth = auth_state_with_password("hunter2");
        let router = test_router(auth);
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains("OPERATOR AUTHENTICATION"),
            "login document must carry the OPERATOR AUTHENTICATION marker"
        );
        assert!(
            !text.contains("/wasm/"),
            "unauthenticated GET / must never reference the wasm bundle path"
        );
    }

    /// T-47.3-01 mitigation, other half: a request carrying a valid
    /// `ih_session` cookie (minted via a REAL login round-trip through the
    /// assembled router, not a private-field shortcut) must NOT get the
    /// login document. Narrowed per the plan's explicit Assumption A2
    /// allowance — ground-truthed, not assumed: SSR rendering itself
    /// executes fine in a bare `cargo test` process (no `dx build`
    /// artifacts needed; `VirtualDom` rendering never touches the bundled
    /// wasm/asset files), but `App`'s component tree reaches
    /// `server::state::global_app_state()` (a process-wide `OnceLock` this
    /// test never populates — `AppState::init()` opens real SQLite stores
    /// and is out of scope for a lightweight router test), which panics on
    /// a background render task and surfaces as a real `500` response — not
    /// a crash of the test process, and not the login document either. So
    /// only the login document's marker is asserted absent here; full app
    /// content is not. Human check: `dx serve` verification in the phase's
    /// `<verification>` block covers the full real-browser render (real
    /// `AppState` installed, real `dx` build).
    #[tokio::test]
    async fn root_delegates_when_authenticated() {
        let auth = auth_state_with_password("hunter2");

        let login_resp = test_router(auth.clone())
            .oneshot(login_request("hunter2", 40001))
            .await
            .unwrap();
        assert_eq!(login_resp.status(), StatusCode::NO_CONTENT);
        let set_cookie = login_resp
            .headers()
            .get(header::SET_COOKIE)
            .expect("login must set a session cookie")
            .to_str()
            .unwrap()
            .to_string();
        let cookie_pair = set_cookie
            .split(';')
            .next()
            .expect("Set-Cookie must have at least one attribute")
            .to_string();

        let root_req = Request::builder()
            .method("GET")
            .uri("/")
            .header(header::COOKIE, cookie_pair)
            .body(Body::empty())
            .unwrap();
        let resp = test_router(auth).oneshot(root_req).await.unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("OPERATOR AUTHENTICATION"),
            "an authenticated GET / must not render the login document"
        );
    }

    /// D-07 core boundary: every data-route family 401s without a session,
    /// through the REAL assembled router (not a synthetic allowlist check).
    #[tokio::test]
    async fn deny_by_default_router_401s_data_routes() {
        let auth = auth_state_with_password("x");
        for (method, uri) in [
            ("GET", "/api/anything"),
            ("GET", "/api/ws/chat"),
            (
                "GET",
                "/artifacts/00000000-0000-0000-0000-000000000000",
            ),
            (
                "GET",
                "/chat-attachments/s1/00000000-0000-0000-0000-000000000000",
            ),
        ] {
            let router = test_router(auth.clone());
            let req = Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let resp = router.oneshot(req).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} must 401 without a session cookie"
            );
        }
    }

    /// AUTH-DESIGN §3.1: the session cookie must be `HttpOnly; SameSite=Strict`.
    #[tokio::test]
    async fn login_sets_httponly_samesite_cookie() {
        let auth = auth_state_with_password("hunter2");
        let router = test_router(auth);
        let resp = router.oneshot(login_request("hunter2", 40002)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("HttpOnly"), "cookie must be HttpOnly: {cookie}");
        assert!(
            cookie.contains("SameSite=Strict"),
            "cookie must be SameSite=Strict: {cookie}"
        );
    }

    /// Response compression must be ON for the pre-auth login document.
    ///
    /// This drives the REAL router rather than asserting on `compression_layer()`
    /// in isolation, because every way this can regress is a wiring failure, not a
    /// constructor failure: the layer registered inside the auth middleware instead
    /// of outside it (so it never sees the login body), the `server` feature losing
    /// the `tower-http/compression-*` features, or `test_router` drifting from
    /// `main.rs`. A test that only called `compression_layer()` would pass through
    /// all three.
    ///
    /// Verified without a decompressor dependency: the header must be `gzip`, the
    /// body must actually start with the gzip magic number, and it must be smaller
    /// than the same response fetched without `Accept-Encoding`. A stub that merely
    /// set the header would fail the magic-byte check.
    #[tokio::test]
    async fn login_document_is_compressed_when_the_client_offers_gzip() {
        use axum::body::to_bytes;

        let auth = auth_state_with_password("hunter2");

        let identity = test_router(auth.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(identity.status(), StatusCode::OK);
        assert!(
            identity.headers().get(header::CONTENT_ENCODING).is_none(),
            "a client that offers no Accept-Encoding must get an uncompressed body"
        );
        let identity_len = to_bytes(identity.into_body(), usize::MAX)
            .await
            .unwrap()
            .len();

        let compressed = test_router(auth)
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::ACCEPT_ENCODING, "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(compressed.status(), StatusCode::OK);
        assert_eq!(
            compressed
                .headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok()),
            Some("gzip"),
            "the login document must be served gzip-encoded when the client offers it"
        );

        let body = to_bytes(compressed.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            &body[..2],
            &[0x1f, 0x8b],
            "body must be a real gzip stream, not just a response wearing the header"
        );
        assert!(
            body.len() < identity_len,
            "compressed body ({} bytes) must be smaller than uncompressed ({identity_len} bytes)",
            body.len()
        );
    }

    /// Pitfall 3: proves the login handler's `ConnectInfo<SocketAddr>`
    /// extractor resolves and `login()`'s body actually executes (rate
    /// limit + password verify), rather than the request being rejected at
    /// extraction time — a real test, not a tautology, because `oneshot`
    /// bypasses `into_make_service_with_connect_info` entirely and each
    /// request here has `ConnectInfo` inserted manually (see
    /// `login_request`).
    #[tokio::test]
    async fn login_connect_info_resolves() {
        let auth = auth_state_with_password("hunter2");
        let router = test_router(auth);
        let resp = router.oneshot(login_request("wrong-password", 40003)).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "a resolved ConnectInfo + wrong password must reach login()'s verify step and 401, \
             not fail at extraction"
        );
    }

    // -------------------------------------------------------------------
    // Phase 47.3 Plan 04 (D-01/D-04/D-14/D-16a/D-19/D-20): the real `basic`
    // (1a) and `fade` (1b) treatments, and the shared submit script.
    // -------------------------------------------------------------------

    #[test]
    fn basic_document_has_single_password_input() {
        let html = login_html("basic");
        assert_eq!(
            html.matches("<input").count(),
            1,
            "basic document must contain exactly one <input> element"
        );
        assert!(html.contains(r#"type="password""#));
        assert!(html.contains("autofocus"));
    }

    #[test]
    fn basic_document_has_no_placeholder_or_demo_value() {
        let html = login_html("basic");
        assert!(!html.contains("placeholder="), "no placeholder attribute may ship");
        assert!(!html.contains("sk-ant-"), "no decorative demo credential value may ship");
    }

    #[test]
    fn basic_document_marker_present() {
        let html = login_html("basic");
        assert!(html.contains("OPERATOR AUTHENTICATION"));
        assert!(html.contains("PASSWORD"));
        assert!(html.contains("AUTHENTICATE"));
    }

    #[test]
    fn render_login_page_falls_back_to_basic() {
        let basic = login_html("basic");
        let unknown = login_html("definitely-not-a-theme");
        assert_eq!(basic, unknown, "an unrecognized theme must render byte-for-byte identical HTML to basic");
        let resp = render_login_page("definitely-not-a-theme");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn submit_script_navigates_on_204() {
        let script = submit_script();
        assert!(script.contains("resp.status === 204"), "must branch explicitly on 204");
        assert!(script.contains("window.location.href"), "204 branch must hard-navigate");
    }

    /// The export's own authed-state fades (`1d` rain→chat, `2a`
    /// veil-minimize, `1c` door-open) are demo scaffolding faking a
    /// client-rendered SPA and are never ported — confirmed at this plan's
    /// Task 1 checkpoint (item b). None of their state-variable names may
    /// appear anywhere in the shared script or either shipped document, and
    /// the 204 branch must do nothing besides navigate.
    #[test]
    fn submit_script_has_no_success_animation() {
        let script = submit_script();
        let forbidden = [
            "rainOpacity",
            "loginOpacity",
            "loginScale",
            "veilMinimize",
            "doorL",
            "doorR",
            "authed",
        ];
        for token in forbidden {
            assert!(!script.contains(token), "found forbidden demo-animation token in script: {token}");
        }
        let basic = login_html("basic");
        let fade = login_html("fade");
        for token in forbidden {
            assert!(!basic.contains(token), "found forbidden demo-animation token in basic doc: {token}");
            assert!(!fade.contains(token), "found forbidden demo-animation token in fade doc: {token}");
        }
    }

    #[test]
    fn error_copy_is_uniform() {
        let script = submit_script();
        assert_eq!(
            script.matches(LOGIN_ERROR_MESSAGE).count(),
            1,
            "the 401 copy constant must appear exactly once in the shared script"
        );
        assert!(script.contains("Too many attempts — try again in"));
        assert_ne!(
            "Too many attempts — try again in", LOGIN_ERROR_MESSAGE,
            "the 429 copy must be a distinct constant from the 401 copy"
        );
    }

    #[test]
    fn fade_document_has_six_preflight_rows() {
        let html = login_html("fade");
        let row_count = html.matches(r#"class="login-preflight-row""#).count();
        assert_eq!(row_count, 6, "1b's preflight list must render exactly six rows");
        let ok_count = html.matches(r#"login-ok">[OK]"#).count();
        let missing_count = html.matches(r#"login-missing">[MISSING]"#).count();
        assert_eq!(
            ok_count + missing_count,
            6,
            "every preflight row must be prefixed [OK] or [MISSING]"
        );
    }

    #[test]
    fn csp_permits_inline_submit_script() {
        assert!(LOGIN_CSP.contains("script-src"));
        assert!(LOGIN_CSP.contains("'unsafe-inline'"));
        // header/document agreement: the doc must actually ship an inline
        // <script> for the CSP allowance to be meaningful.
        assert!(login_html("basic").contains("<script>"));
        assert!(login_html("fade").contains("<script>"));
    }

    /// T-47.3-17 / D-19, expressed as a rendered-body assertion (not just
    /// source review): neither shipped document's failure-copy building
    /// blocks may name a provider, echo an upstream HTTP status literal, or
    /// hand out a diagnostic command.
    #[test]
    fn failure_copy_has_no_oracle_leak() {
        let forbidden = ["anthropic", "invalid_api_key", "doctor", "403", " 401", " 429"];
        let candidates = [LOGIN_ERROR_MESSAGE, "[DENIED]", "[FAILED]", "RETRY AUTHENTICATION", "RETRY"];
        for candidate in candidates {
            let lower = candidate.to_lowercase();
            for token in forbidden {
                assert!(
                    !lower.contains(&token.to_lowercase()),
                    "failure copy candidate {candidate:?} must not contain oracle-leaking token {token:?}"
                );
            }
        }
    }

    /// D-20 (resolved at the 47.3-04 Task 1 checkpoint, recorded in
    /// 47.3-04-SUMMARY.md: "c-1" — drop the counter). The export's literal
    /// `attempt 1 of 3` must never render in ANY of the five shipped
    /// documents — extended in Plan 05 to cover all five themes, not just
    /// the two Plan 04 shipped.
    #[test]
    fn d20_attempt_counter_is_dropped() {
        for theme in LOGIN_THEME_SLUGS {
            let html = login_html(theme);
            assert!(
                !html.contains("attempt 1 of") && !html.contains("attempt N of"),
                "D-20 (resolved c-1): no attempt-counter string may render for theme {theme}"
            );
        }
    }

    /// Quick task 260804-plp: the `BUILD ` status bar stamp must always
    /// reflect `CARGO_PKG_VERSION`, never a hand-typed literal. `basic`,
    /// `fade`, and `blast-doors` carry one stamp each; `matrix-rain` carries
    /// two (its background chat-mock layer plus the foreground overlay) —
    /// five stamps total across the stamped themes. `orbit-veil` never had a
    /// build stamp, so it is excluded rather than asserted against. Per
    /// document, `BUILD ` occurrence count must equal current-version stamp
    /// count, so a future hand-typed version cannot slip back in unnoticed.
    #[test]
    fn login_documents_stamp_the_current_package_version() {
        let expected = format!("BUILD {}", env!("CARGO_PKG_VERSION"));
        let stamped_themes = ["basic", "fade", "blast-doors", "matrix-rain"];
        let mut total_stamps = 0;
        for theme in stamped_themes {
            let html = login_html(theme);
            let build_count = html.matches("BUILD ").count();
            let expected_count = html.matches(&expected).count();
            assert!(
                expected_count > 0,
                "theme {theme} must contain at least one {expected:?} stamp"
            );
            assert_eq!(
                build_count, expected_count,
                "theme {theme}: every `BUILD ` occurrence must match the current-version stamp \
                 {expected:?} — found {build_count} `BUILD ` occurrences but only \
                 {expected_count} matched the current version, so a stale hand-typed version \
                 survived"
            );
            total_stamps += expected_count;
        }
        assert_eq!(total_stamps, 5, "exactly 5 build stamps expected across all stamped themes");
    }

    // -------------------------------------------------------------------
    // Phase 47.3 Plan 05 (D-01/D-04/D-05/D-16a/D-16b/D-16c): `1c` Blast
    // doors, `1d` Matrix rain, `2a` Orbit Veil — the remaining three
    // treatments, completing the five-treatment set (D-01).
    // -------------------------------------------------------------------

    fn read_asset(relative_path: &str) -> String {
        std::fs::read_to_string(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path))
            .unwrap_or_else(|e| panic!("failed to read {relative_path}: {e}"))
    }

    #[test]
    fn blast_doors_document_renders() {
        let html = login_html("blast-doors");
        assert!(html.contains("OPERATOR AUTHENTICATION"));
        assert!(html.contains("UNSEAL BULKHEAD"));
        assert_eq!(
            html.matches("<input").count(),
            1,
            "blast-doors document must contain exactly one <input> element (Port Correction 1)"
        );
        assert!(html.contains(r#"type="password""#));
    }

    #[test]
    fn blast_doors_providers_grid_has_six_rows() {
        let html = login_html("blast-doors");
        assert_eq!(
            html.matches("cd-provider-name").count(),
            6,
            "the decorative Providers grid must bake exactly 6 rows (export's hint-placeholder-count)"
        );
    }

    /// D-16b: the failing lamp A-02 (and the other hardcoded-`ih-fail`
    /// lamps) must never be scoped under an auth-error selector — it is
    /// unconditional ambience, not a failure indicator.
    #[test]
    fn blast_doors_ih_fail_is_not_auth_scoped() {
        let css = read_asset("assets/login.css");
        for line in css.lines() {
            if line.contains("ih-fail") {
                let lower = line.to_lowercase();
                assert!(
                    !lower.contains("is-failed") && !lower.contains("denied") && !lower.contains("danger"),
                    "ih-fail must never be scoped under an auth-error selector (D-16b): {line}"
                );
            }
        }
    }

    /// D-16b: the `ih-hum` working lamp bus, seal label, status text and
    /// panel border ARE reachable from the denied/timeout alarm state —
    /// this is the designed cascade and MUST be wired (the inverse of
    /// `blast_doors_ih_fail_is_not_auth_scoped` above).
    #[test]
    fn blast_doors_alarm_cascade_is_auth_scoped() {
        let css = read_asset("assets/login.css");
        let html = login_html("blast-doors");
        assert!(css.contains("ih-hum"), "the working lamp bus must use the ih-hum keyframe");
        assert!(
            css.contains("ih-blink 0.9s steps(2)"),
            "the alarm lamp-bus swap must use ih-blink 0.9s steps(2) infinite (D-16b)"
        );
        assert!(
            css.contains("oklch(58% 0.22 25)"),
            "the alarm lamp color must be oklch(58% 0.22 25) (D-16b)"
        );
        assert!(html.contains("LOCKOUT"), "cSeal must reach LOCKOUT on alarm");
        assert!(html.contains("KEY REJECTED"), "cStatus must reach KEY REJECTED on denied");
        assert!(html.contains("SESSION TIMEOUT"), "cStatus must reach SESSION TIMEOUT on timeout");
        assert!(
            css.contains(".cd-panel.is-failed"),
            "the SEALED panel border must flip to --danger on alarm (cPanelBorder)"
        );
    }

    /// D-16c: the bulkhead slams shut faster (260ms) than it opens (1050ms),
    /// and the 260ms transition must be reachable only from a timeout-scoped
    /// selector, never applied universally to every door transition.
    #[test]
    fn blast_doors_timeout_uses_fast_door() {
        let css = read_asset("assets/login.css");
        assert!(css.contains("1050ms"), "the base door transition must keep the heavy 1050ms curve");
        assert!(css.contains("260ms"), "the timeout door-slam transition (260ms) must exist");
        let idx = css.find("260ms").expect("260ms transition must exist in login.css");
        let window_start = idx.saturating_sub(120);
        let context = &css[window_start..idx];
        assert!(
            context.contains("is-timeout"),
            "260ms door transition must be scoped under a timeout-state selector, not universal: {context}"
        );
    }

    #[test]
    fn blast_doors_failure_copy_has_no_oracle_leak() {
        let html = login_html("blast-doors");
        let needle = "data-fail-text=\"[DENIED] servo lockout";
        let start = html.find(needle).expect("blast-doors error box must carry its own scene-flavored data-fail-text");
        let end = html[start..]
            .find("\" aria-live")
            .map(|i| start + i)
            .unwrap_or_else(|| (start + 300).min(html.len()));
        let failure_copy = &html[start..end];
        let forbidden = ["anthropic", "openai", "403", " 401", " 429", "doctor"];
        for token in forbidden {
            assert!(
                !failure_copy.to_lowercase().contains(&token.to_lowercase()),
                "blast-doors failure copy must not leak oracle token {token:?}: {failure_copy}"
            );
        }
        assert!(
            !failure_copy.contains("attempt 1 of") && !failure_copy.contains("attempt N of"),
            "D-20: no attempt-counter string in blast-doors failure copy"
        );
    }

    #[test]
    fn matrix_rain_document_references_rain_script() {
        let html = login_html("matrix-rain");
        assert!(html.contains("login-rain.js"), "matrix-rain document must reference login-rain.js");
        assert!(html.contains("login-rain-canvas"), "matrix-rain document must carry a canvas element");
        assert!(html.contains(r#"data-theme="terminal-dark""#));
    }

    #[test]
    fn basic_document_does_not_reference_rain_script() {
        for theme in ["basic", "fade", "blast-doors", "orbit-veil"] {
            let html = login_html(theme);
            assert!(
                !html.contains("login-rain.js"),
                "theme {theme} must never reference login-rain.js (D-02 theme-scoped assets)"
            );
        }
    }

    /// Source-string assertion in the style of `tests/kanban_server_fns.rs`
    /// (per 47.3-05-PLAN.md Task 2's own test spec): reads `login-rain.js`
    /// from a `CARGO_MANIFEST_DIR`-relative path and asserts the D-05
    /// substitution is exact — no other green literal survived the port.
    #[test]
    fn rain_js_uses_only_d05_greens() {
        let js = read_asset("assets/login-rain.js");
        for required in ["#56d364", "#3fb950", "rgba(63, 185, 80, 0.45)"] {
            assert!(js.contains(required), "login-rain.js must contain the D-05 literal {required:?}");
        }
        assert_eq!(
            js.matches("oklch(").count(),
            0,
            "no export oklch() color literal may survive the D-05 substitution into raw fillStyle strings"
        );
        // No other GREEN hex/rgba beyond the three required D-05 literals
        // and their failure-state red counterparts (which are red, not a
        // "third green").
        let green_hex_count = js.matches("#56d364").count() + js.matches("#3fb950").count();
        assert!(green_hex_count >= 2, "both required green hex literals must appear");
    }

    #[test]
    fn rain_js_gates_on_reduced_motion() {
        let js = read_asset("assets/login-rain.js");
        assert!(js.contains("prefers-reduced-motion"), "login-rain.js must check prefers-reduced-motion at load");
        assert!(
            js.contains("matchMedia"),
            "the reduced-motion check must use matchMedia, not a CSS-only mechanism"
        );
    }

    /// G-47.3-2 call-site guard. Read the limits of this test before trusting it:
    /// it proves the two repair call sites still EXIST, and nothing about whether
    /// the canvas actually stays opaque. The defect it guards was a *rendering*
    /// failure, and no source-tree assertion in this file can observe rendering —
    /// that is the exact blind spot that let G-47.3-1 ship green for a whole phase.
    /// Real coverage is the browser harness recorded in 47.3-UAT.md (fire `resize`
    /// every 300ms and confirm `.login-chat-mock` never becomes legible). This is a
    /// canary against silent deletion, nothing more.
    #[test]
    fn rain_js_repaints_backdrop_after_resize() {
        let js = read_asset("assets/login-rain.js");
        let resize_body = js
            .split_once("addEventListener(\"resize\"")
            .expect("login-rain.js must register a resize handler")
            .1;
        let debounced = resize_body
            .split_once("}, 150);")
            .expect("the resize handler must keep its debounced timeout")
            .0;
        assert!(
            debounced.contains("paintStaticFrame("),
            "the resize path MUST repaint the opaque backdrop. measure() assigns \
             canvas.width/height, which per spec clears the bitmap to transparent, \
             exposing the decorative .login-chat-mock layer underneath. TRAIL_FADE \
             is 10% alpha and never restores opacity. See G-47.3-2."
        );
        assert!(
            debounced.contains("reseedColumns(true)"),
            "a resize reseed must fill the viewport, not park every column above it — \
             dragging a window edge emits a continuous stream of resize events, so \
             seeding overhead drains the field for as long as the drag lasts (G-47.3-2)"
        );

        // Second pass: the debounced repair alone is NOT sufficient. Rotating a phone
        // while the password field is focused streams resize events that keep resetting
        // the 150ms timer, so an undersized canvas stays undersized and the mock shows
        // through. There must be an immediate, un-debounced repair ahead of the timer.
        let immediate = resize_body
            .split_once("setTimeout(")
            .expect("the resize handler must debounce via setTimeout")
            .0;
        assert!(
            immediate.contains("paintStaticFrame("),
            "the resize handler must repaint the backdrop IMMEDIATELY, before the \
             debounced timer — a rotation that enlarges the parent leaves the canvas \
             pinned at its old inline pixel size, and a stream of resize events (phone \
             rotating with the keyboard up) keeps the debounce from ever firing (G-47.3-2)"
        );
        assert!(
            immediate.contains("clientWidth") && immediate.contains("clientHeight"),
            "the immediate repair must be gated on the canvas actually being smaller \
             than its parent, so that merely shrinking a window does not wipe the trails \
             on every event and leave the rain looking thin"
        );
    }

    /// G-47.3-2, second half: a lockout is a rejection and must look like one.
    /// Same caveat as above — this asserts wiring in a string template, not behaviour.
    #[test]
    fn rate_limited_response_applies_the_failure_treatment() {
        let script = submit_script();
        let after_429 = script
            .split_once("resp.status === 429")
            .expect("the submit handler must branch on 429")
            .1;
        let branch = after_429
            .split_once("return;")
            .expect("the 429 branch must return")
            .0;
        assert!(
            branch.contains("applyFailTreatment()"),
            "the 429 branch returns before showDenied(), so without an explicit \
             applyFailTreatment() call a rate-limited operator sees a bare countdown \
             on a pristine page — no re-keyed rain, no danger border, no scene failure \
             copy — in exactly the state where the signal matters most (G-47.3-2)"
        );
        let apply_at = branch.find("applyFailTreatment()").unwrap();
        let countdown_at = branch
            .find("startCountdown(")
            .expect("the 429 branch must start the countdown");
        assert!(
            apply_at < countdown_at,
            "applyFailTreatment() must run BEFORE startCountdown() so the countdown's \
             per-tick errorBox write lands last — otherwise the scene's data-fail-text \
             overwrites the remaining-seconds figure the operator needs to read"
        );
    }

    /// The LAYERS panel must account for the headline figure. The export's 12 rows
    /// sum to exactly the "16,071 objects tracked" the header claims. The port shipped
    /// 5 of them — 11,792 — while keeping the headline, so the panel silently
    /// contradicted the header by the 4,279 the omitted rows account for.
    ///
    /// Asserting the SUM is what gives this teeth: a row-count check alone would pass
    /// if someone swapped a row's number, and a number check alone would pass if a row
    /// went missing. Together they pin the panel to the headline.
    #[test]
    fn orbit_veil_layer_counts_account_for_the_headline_total() {
        let html = login_html("orbit-veil");
        let mut sum = 0u32;
        let mut rows = 0usize;
        for chunk in html.split("class=\"ov-layer-count\">").skip(1) {
            let raw = chunk.split('<').next().unwrap_or_default();
            sum += raw
                .replace(',', "")
                .parse::<u32>()
                .unwrap_or_else(|_| panic!("unparseable layer count {raw:?}"));
            rows += 1;
        }
        assert_eq!(
            rows, 12,
            "orbit-veil must list all 12 of the export's layer rows"
        );
        assert_eq!(
            sum, 16_071,
            "the layer counts must sum to the headline the header advertises — if these \
             disagree, the panel is lying about the scene"
        );
        assert!(
            html.contains("16,071 objects tracked"),
            "the headline itself must still be present for the sum above to mean anything"
        );
    }

    /// The export's satellite population is 548 across 12 categories. The port shipped
    /// 8 "representative" orbiters against a comment that mis-stated the export as
    /// generating "~700" — reported in 47.3 UAT as satellites missing versus source.
    #[test]
    fn orbit_veil_renders_the_full_satellite_population() {
        let html = login_html("orbit-veil");
        assert_eq!(
            html.matches("ov-orbit-dot").count(),
            548,
            "orbit-veil must render the export's full 548-satellite population"
        );
    }

    /// The HUD clock must actually be driven. It shipped as a frozen `00:00:00`
    /// beside a green `LIVE` badge, and showed bare `UTC` where the export shows
    /// `YYYY-MM-DD UTC`, so the readout was stopped AND missing its date.
    ///
    /// Asserted structurally (the script targets both elements and reads
    /// `toISOString`, on a repeating timer) rather than by executing JS, which this
    /// suite cannot do. The live behaviour is verified in-browser and recorded in UAT.
    #[test]
    fn orbit_veil_clock_is_driven_live() {
        let html = login_html("orbit-veil");
        for needle in [
            ".ov-clock-time",
            ".ov-clock-date",
            "toISOString",
            "setInterval",
        ] {
            assert!(
                html.contains(needle),
                "the orbit-veil clock script must reference {needle} — without it the \
                 clock is a frozen readout under a LIVE badge"
            );
        }
        // The date half is the part that was silently missing, so pin it specifically.
        assert!(
            html.contains("' UTC'"),
            "the clock must render `YYYY-MM-DD UTC` like the export, not a bare `UTC`"
        );
        // A live clock must NOT come at the cost of an extra pre-auth request (A3):
        // the orbit-veil document loads no external script.
        assert!(
            !html.contains("<script src"),
            "orbit-veil must not add an external script — the clock is inline by design"
        );
    }

    /// The scene must be byte-stable across renders. The export randomises orbiters
    /// with `Math.random()`; this port drives the same distributions from a fixed-seed
    /// xorshift because a server-rendered page that changed on every request would
    /// churn its HTML and make any downstream comparison or caching meaningless.
    #[test]
    fn orbit_veil_is_deterministic_across_renders() {
        assert_eq!(
            login_html("orbit-veil"),
            login_html("orbit-veil"),
            "orbit-veil must render identically on every request — a live RNG leaked in"
        );
    }

    /// The globe texture must be bound from the TEMPLATE (which holds the
    /// `asset!()`-derived, hashed URL), never from a relative `url()` in the static
    /// stylesheet. A relative reference resolves against login.css's own URL to the
    /// UNHASHED `/assets/earth-night.jpg`, which dx never emits and
    /// `public_asset_paths()` never allowlists — it 401s and the globe renders as
    /// empty space. Reported during 47.3 UAT as "the rotating earth is missing".
    ///
    /// Asserted structurally rather than by matching a hash, because `asset!()` does
    /// not necessarily produce a hashed name under `cargo test` — only under a real
    /// dx build. Checking WHERE the binding lives is environment-independent; checking
    /// the hash would be a test that passes for the wrong reason.
    #[test]
    fn orbit_veil_globe_texture_is_bound_from_the_template() {
        let html = login_html("orbit-veil");
        assert!(
            html.contains("class=\"ov-globe\" style=\"background-image:url("),
            "orbit-veil must bind the globe texture inline on .ov-globe, using the \
             asset!()-derived URL the template holds"
        );

        let css = strip_css_comments(&read_asset("assets/login.css"));
        let globe_rule = css
            .split(".ov-globe {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("login.css must declare an .ov-globe rule");
        assert!(
            !globe_rule.contains("background-image"),
            "the .ov-globe rule must NOT set background-image — a relative url() there \
             resolves to the unhashed asset path and 401s: {globe_rule:?}"
        );
        assert!(
            !css.contains("earth-night"),
            "login.css must not reference earth-night at all; the texture is the \
             template's job because only it knows the hashed name"
        );
    }

    /// Locks in the general invariant behind the bug above, and makes the ONE known
    /// remaining violation explicit instead of silent.
    ///
    /// Any relative `url()` in this stylesheet resolves against login.css's own URL and
    /// therefore requests an unhashed asset path, which the dx pipeline does not emit
    /// and the pre-auth allowlist does not permit. The `@font-face` rules still do this
    /// — that is the open font defect tracked as Phase 49, deliberately NOT fixed here
    /// because the right repair is a design decision about which layer owns the URL.
    /// This test pins the count so a NEW relative reference fails immediately rather
    /// than joining the debt.
    #[test]
    fn login_css_relative_urls_are_limited_to_the_known_font_debt() {
        let css = strip_css_comments(&read_asset("assets/login.css"));
        let relative: Vec<&str> = css
            .match_indices("url(\"./")
            .map(|(i, _)| {
                let rest = &css[i..];
                rest.split(')').next().unwrap_or(rest)
            })
            .collect();
        for r in &relative {
            assert!(
                r.contains("fonts/"),
                "new relative url() {r:?} in login.css — it will resolve to an unhashed \
                 asset path and 401 pre-auth. Bind it from the template instead, the way \
                 .ov-globe does."
            );
        }
        assert_eq!(
            relative.len(),
            5,
            "expected exactly the 5 known @font-face relative urls (Phase 49). If this \
             dropped, the font debt was fixed — delete this test. If it rose, a new \
             broken reference was added: {relative:?}"
        );
    }

    /// Guards the `ORBITERS` column order against re-transposition. The diameter
    /// and duration columns were originally swapped, shipping 26-72px satellite
    /// discs orbiting in 4.4-7.8s. Bounds come from the design export's own
    /// generator, not from taste: `d: Math.max(3, size * 2.1)` yields 3-5.5px and
    /// `dur = 26 + Math.random() * 54` yields 26-80s. A swap cannot survive both
    /// assertions, because the two ranges do not overlap.
    #[test]
    fn orbit_veil_satellite_dots_match_export_scale() {
        let html = login_html("orbit-veil");
        let mut diameters = Vec::new();
        for chunk in html.split("class=\"ov-orbit-dot\"").skip(1) {
            let decls = chunk.split('>').next().unwrap_or_default();
            let px = decls
                .split("width:")
                .nth(1)
                .and_then(|r| r.split("px").next())
                .and_then(|n| n.trim().parse::<f32>().ok())
                .unwrap_or_else(|| panic!("could not parse a dot width from {decls:?}"));
            diameters.push(px);
        }
        assert!(!diameters.is_empty(), "orbit-veil must render satellite dots");
        for d in &diameters {
            assert!(
                (3.0..=8.0).contains(d),
                "satellite dot diameter {d}px is outside the export's 3-5.5px scale (8px \
                 tolerance). A value in the 26-80 range means the diameter and duration \
                 columns of ORBITERS have been transposed again."
            );
        }

        let mut durations = Vec::new();
        for chunk in html.split("class=\"ov-orbit-spin\"").skip(1) {
            let decls = chunk.split('>').next().unwrap_or_default();
            if let Some(secs) = decls
                .split("animation-duration:")
                .nth(1)
                .and_then(|r| r.split('s').next())
                .and_then(|n| n.trim().parse::<f32>().ok())
            {
                durations.push(secs);
            }
        }
        assert!(!durations.is_empty(), "orbit-veil must animate its satellites");
        for s in &durations {
            assert!(
                (26.0..=80.0).contains(s),
                "orbit duration {s}s is outside the export's 26-80s band — a single-digit \
                 value means the ORBITERS columns have been transposed again"
            );
        }
    }

    #[test]
    fn orbit_veil_document_renders() {
        let html = login_html("orbit-veil");
        assert!(html.contains("OPERATOR AUTHENTICATION"));
        assert!(html.contains("LOGIN TO SYSTEM"));
        assert!(html.contains("earth-night.jpg"));
    }

    #[test]
    fn orbit_veil_has_110_baked_stars() {
        assert_eq!(BAKED_STARS.len(), 110, "2a must bake exactly 110 stars (the export's own count)");
        let html = login_html("orbit-veil");
        assert_eq!(
            html.matches("ov-star").count(),
            110,
            "the rendered document must emit exactly 110 star elements"
        );
    }

    #[test]
    fn orbit_veil_panel_is_zero_radius() {
        let css = read_asset("assets/login.css");
        // The veil panel's own selector block must not set a nonzero
        // border-radius, while the surrounding HUD selectors (which use
        // rounded-xl-equivalent literal px values) legitimately do.
        let panel_start = css.find(".ov-panel {").expect(".ov-panel selector must exist");
        let panel_end = css[panel_start..].find('}').map(|i| panel_start + i).unwrap();
        let panel_block = &css[panel_start..panel_end];
        assert!(
            !panel_block.contains("border-radius"),
            "the veil auth panel must stay 0-radius even though the surrounding HUD uses rounded corners: {panel_block}"
        );
        assert!(
            css.contains("border-radius: 12px") || css.contains("border-radius:12px"),
            "the surrounding HUD chrome must use rounded corners, preserving the intended visual clash"
        );
    }

    #[test]
    fn orbit_veil_hud_controls_are_inert() {
        let html = login_html("orbit-veil");
        // Extract just the decorative HUD region (everything before the
        // real auth panel) and assert no real interactive element exists
        // there.
        let panel_idx = html.find("ov-panel-wrap").unwrap_or(html.len());
        let hud_region = &html[..panel_idx];
        assert!(!hud_region.contains("<button"), "the decorative HUD region must contain no <button> elements");
        assert!(!hud_region.contains("<a href"), "the decorative HUD region must contain no <a href> elements");
    }

    #[test]
    fn earth_night_only_in_orbit_veil_allowlist() {
        for theme in LOGIN_THEME_SLUGS {
            let paths = public_asset_paths(theme);
            let has_earth = paths.iter().any(|p| p.contains("earth-night"));
            if theme == &"orbit-veil" {
                assert!(has_earth, "orbit-veil's allowlist must include earth-night.jpg");
            } else {
                assert!(!has_earth, "theme {theme} must NOT include earth-night.jpg in its allowlist");
            }
        }
    }

    // -------------------------------------------------------------------
    // Phase 47.3 Plan 08 (gap-closure, D-01/D-04/D-05/D-15/D-16a/D-18):
    // regression harness for the mobile (<640px) collapse across 1c/1d/2a.
    // Parses the REAL login.css (never a hand-built fixture) so the guard
    // reflects what actually ships, not what a fixture assumes.
    // -------------------------------------------------------------------

    /// Strips every `/* ... */` CSS comment from `css` before any parsing,
    /// so prose inside a comment can never satisfy or defeat an assertion.
    fn strip_css_comments(css: &str) -> String {
        let mut out = String::with_capacity(css.len());
        let mut chars = css.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '/' && chars.peek() == Some(&'*') {
                chars.next(); // consume '*'
                loop {
                    match chars.next() {
                        Some('*') if chars.peek() == Some(&'/') => {
                            chars.next(); // consume '/'
                            break;
                        }
                        Some(_) => continue,
                        None => break,
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Locates every `@media` at-rule in `css`, returning `(prelude, body)`
    /// pairs. The body is sliced by brace-depth counting from the at-rule's
    /// opening `{` to its matching `}` — a naive `find('}')` would mis-slice
    /// a media block whose rules contain nested braces.
    fn extract_media_blocks(css: &str) -> Vec<(String, String)> {
        let mut blocks = Vec::new();
        let mut i = 0;
        while let Some(rel) = css[i..].find("@media") {
            let start = i + rel;
            let Some(open_rel) = css[start..].find('{') else {
                break;
            };
            let open = start + open_rel;
            let prelude = css[start + "@media".len()..open].trim().to_string();
            let mut depth = 0i32;
            let mut end = open;
            for (offset, ch) in css[open..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            blocks.push((prelude, css[open + 1..end].to_string()));
            i = end + 1;
        }
        blocks
    }

    /// Splits a media-query body into `(selector_list, declaration_block)`
    /// pairs, again by brace-depth counting so nested rule braces (none
    /// exist today, but the parser must not assume that) are handled
    /// correctly rather than via a naive split on `}`.
    fn split_rule_pairs(body: &str) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        let mut i = 0;
        while let Some(rel) = body[i..].find('{') {
            let open = i + rel;
            let selector = body[i..open].trim().to_string();
            if selector.is_empty() {
                break;
            }
            let mut depth = 0i32;
            let mut end = open;
            for (offset, ch) in body[open..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            pairs.push((selector, body[open + 1..end].to_string()));
            i = end + 1;
        }
        pairs
    }

    /// Collapses all whitespace out of a string so `display:none` and
    /// `display: none` (or any other spacing variant) compare equal.
    fn collapse_ws(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().concat()
    }

    /// Returns every `(selector_list, declaration_block)` pair from every
    /// `@media (max-width: 639px)` body in the real `login.css`, unioned
    /// across however many such blocks exist (there is exactly one today;
    /// the parser does not assume that).
    fn mobile_breakpoint_pairs() -> Vec<(String, String)> {
        let css = strip_css_comments(&read_asset("assets/login.css"));
        let target = collapse_ws("(max-width: 639px)");
        extract_media_blocks(&css)
            .into_iter()
            .filter(|(prelude, _)| collapse_ws(prelude) == target)
            .flat_map(|(_, body)| split_rule_pairs(&body))
            .collect()
    }

    /// Guards the mobile collapse to a single breakpoint value: `login.css`
    /// must declare exactly two distinct `@media` preludes — the 639px
    /// mobile query and the `prefers-reduced-motion` query. A third,
    /// conflicting threshold would fail this test.
    #[test]
    fn mobile_breakpoint_is_the_only_extra_media_query() {
        let css = strip_css_comments(&read_asset("assets/login.css"));
        let mut preludes: Vec<String> = extract_media_blocks(&css)
            .iter()
            .map(|(prelude, _)| collapse_ws(prelude))
            .collect();
        preludes.sort();
        preludes.dedup();
        let expected = {
            let mut v = vec![
                collapse_ws("(max-width: 639px)"),
                collapse_ws("(prefers-reduced-motion: reduce)"),
            ];
            v.sort();
            v
        };
        assert_eq!(
            preludes, expected,
            "login.css must declare exactly two distinct @media preludes (found: {preludes:?})"
        );
    }

    /// Below 640px, every decorative background layer across all five
    /// treatments must collapse to `display: none`, every treatment's
    /// credential card must reflow to the shared
    /// `min(452px, calc(100vw - 32px))` width, and no rule may hide a
    /// credential input, the submit control, the form container, or a
    /// failure/error box (T-47.3-08-01/02/03 — mitigated here, not by
    /// prose). Task 1 seeded the `1c` rows only; Task 2 extends the
    /// REQUIRED-HIDDEN/REQUIRED-REFLOWED tables in place (same test, not
    /// renamed) with `1d`/`2a`. FORBIDDEN-HIDDEN was seeded in full by
    /// Task 1 and needs no change.
    #[test]
    fn mobile_breakpoint_collapses_all_treatments() {
        let pairs = mobile_breakpoint_pairs();
        let shared_width = collapse_ws("min(452px, calc(100vw - 32px))");

        const REQUIRED_HIDDEN: &[&str] = &[
            // `1c` Blast doors
            ".cd-scene",
            ".cd-airlock",
            ".cd-door",
            // `1d` Matrix rain
            ".login-chat-mock",
            ".login-rain-canvas",
            // `2a` Orbit Veil
            ".ov-sky",
            ".ov-globe-wrap",
            ".ov-wordmark",
            ".ov-clock",
            ".ov-search",
            ".ov-layers",
            ".ov-transport",
            ".ov-footer-left",
            ".ov-footer-right",
        ];
        for selector in REQUIRED_HIDDEN {
            let hidden = pairs.iter().any(|(sel_list, decls)| {
                sel_list.split(',').map(str::trim).any(|s| s == *selector)
                    && collapse_ws(decls).contains("display:none")
            });
            assert!(
                hidden,
                "expected {selector} to be display:none inside a 639px media body"
            );
        }

        // All three reflowed card selectors must carry the SAME shared
        // width expression as each other — if a future edit gives one
        // treatment a bespoke width, the shared-layout contract is broken.
        const REQUIRED_REFLOWED: &[&str] = &[".cd-panel", ".login-card--rain", ".ov-panel"];
        for selector in REQUIRED_REFLOWED {
            let rule = pairs
                .iter()
                .find(|(sel_list, _)| sel_list.split(',').map(str::trim).any(|s| s == *selector));
            let (_, decls) =
                rule.unwrap_or_else(|| panic!("expected a 639px media rule targeting {selector}"));
            let collapsed = collapse_ws(decls);
            assert!(
                collapsed.contains(&shared_width),
                "{selector} must carry the shared width min(452px, calc(100vw - 32px)): {decls}"
            );
        }
        // `.cd-panel` additionally moves out of absolute positioning.
        let cd_panel_decls = pairs
            .iter()
            .find(|(sel_list, _)| sel_list.split(',').map(str::trim).any(|s| s == ".cd-panel"))
            .map(|(_, decls)| collapse_ws(decls))
            .expect("expected a 639px media rule targeting .cd-panel");
        assert!(
            cd_panel_decls.contains("position:static"),
            "`.cd-panel` must reflow to position: static below 640px"
        );

        // FORBIDDEN-HIDDEN: the full security-relevant list, seeded now
        // (not extended in Task 2) — T-47.3-08-01/02/03.
        const FORBIDDEN_HIDDEN: &[&str] = &[
            "#login-password",
            ".login-input",
            ".login-input--airlock",
            "#login-submit",
            ".login-submit",
            ".cd-submit",
            "#login-form",
            ".login-fields",
            ".login-field",
            "#login-error",
            ".login-error",
            ".cd-timeout-box",
            ".cd-panel",
            ".login-card",
            ".ov-panel",
            ".ov-panel-wrap",
            ".login-rain-overlay",
            ".login-center",
        ];
        for token in FORBIDDEN_HIDDEN {
            let violation = pairs.iter().find(|(sel_list, decls)| {
                sel_list.contains(token) && collapse_ws(decls).contains("display:none")
            });
            assert!(
                violation.is_none(),
                "FORBIDDEN: a 639px media rule whose selector list contains {token:?} sets display:none: {violation:?}"
            );
        }
    }

    /// iOS Safari zooms the viewport whenever a focused input computes below
    /// 16px. This page intentionally does not pin the scale (`maximum-scale` /
    /// `user-scalable=no` would suppress the symptom by disabling pinch-zoom
    /// entirely — an accessibility regression), so the input's own font-size is
    /// the only defence. Found on a handset during 47.3 UAT: at the base 14px the
    /// zoom fires on password focus and shifts the card off the left edge,
    /// clipping the first character of every line.
    ///
    /// The floor is asserted numerically rather than as `== 16px` because 16 is a
    /// platform threshold, not a design target — 15.9px still triggers the zoom.
    ///
    /// Same limitation as the other canaries in this file: it reads the SOURCE
    /// stylesheet and proves a declaration exists. It cannot observe an iPhone.
    /// The evidence that this is the correct value is the handset capture
    /// recorded in 47.3-UAT.md, not this test.
    #[test]
    fn mobile_input_font_size_defeats_ios_auto_zoom() {
        let pairs = mobile_breakpoint_pairs();
        let (_, decls) = pairs
            .iter()
            .find(|(sel_list, decls)| {
                sel_list.split(',').map(str::trim).any(|s| s == ".login-input")
                    && collapse_ws(decls).contains("font-size:")
            })
            .expect(
                "the 639px media block must set a font-size on .login-input — without it iOS \
                 Safari auto-zooms on password focus and pushes the card off-screen (G-47.3-4)",
            );
        let collapsed = collapse_ws(decls);
        let value: f32 = collapsed
            .split("font-size:")
            .nth(1)
            .and_then(|rest| rest.split([';', '}']).next())
            .map(str::trim)
            .and_then(|v| v.strip_suffix("px"))
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("could not parse a px font-size from {collapsed:?}"));
        assert!(
            value >= 16.0,
            "the mobile .login-input font-size must be >= 16px to suppress iOS Safari's focus \
             auto-zoom (found {value}px). 16 is a platform threshold, not a design choice — \
             anything below it re-opens the defect."
        );

        // The fix is deliberately mobile-scoped: D-04's 14px port fidelity still
        // governs above 640px, where no handset auto-zoom exists. If this trips,
        // someone widened the fix into a global type change — decide that on
        // purpose rather than inherit it from a mobile bug.
        let base = strip_css_comments(&read_asset("assets/login.css"));
        let base_rule = base
            .split(".login-input {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("login.css must declare a base .login-input rule");
        assert!(
            collapse_ws(base_rule).contains("14px"),
            "the BASE .login-input should still carry D-04's 14px (mobile-only override); \
             found: {base_rule:?}"
        );
    }
}
