//! Phase 54.9 Plan 08 (D-13, D-11): `GET /openui/frame` — the trusted half of the
//! OpenUI React island.
//!
//! Serves ONE static HTML document that inlines the vendored OpenUI renderer
//! (`assets/openui/openui-bundle.min.js`), its stylesheet and the frame bootstrap, so a
//! `<iframe sandbox="allow-scripts">` (opaque origin) needs no subresource request. Mounted
//! as a raw axum route in `main.rs` (and mirrored in `login_page.rs`), BEFORE the auth
//! layer, so `require_auth` wraps it; it is deliberately NOT in `auth::is_public`.
//!
//! Containment is the response header: `frame_csp()` is the artifact CSP directive
//! list plus `frame-ancestors 'self'` (ADR-0007 "Sandbox and bridge"). `connect-src 'none'`
//! blocks all outbound network from the frame, `img-src data:` blocks the bundle's Google
//! favicon URLs, and there is no `'unsafe-eval'`.
//!
//! No code path here talks to api.thesys.dev (D-09).

#[cfg(feature = "server")]
use axum::body::Body;
#[cfg(feature = "server")]
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
};
#[cfg(feature = "server")]
use axum::http::StatusCode;
#[cfg(feature = "server")]
use axum::response::{IntoResponse, Response};

/// The authoritative CSP for the OpenUI frame document: `ARTIFACT_CSP`'s directives
/// (`artifact_route.rs`, untouched so CI gate 6 still sees one definition) plus
/// `frame-ancestors 'self'`, with `default-src 'none'` and the pinned `script-src` built once in
/// [`frame_assets`] (never assembled at call sites).
///
/// WR-03: `script-src` is NOT a literal. It pins the SHA-256 of the two inlined `<script>` bodies
/// (see [`frame_assets`]), so an injected `<script>` or an inline event handler such as
/// `<img onerror=...>` does not run. `style-src` keeps `'unsafe-inline'` (the renderer sets
/// inline styles).
#[cfg(feature = "server")]
const OPENUI_FRAME_CSP_REST: &str = "style-src 'unsafe-inline'; img-src data:; font-src data:; \
     connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'; \
     frame-ancestors 'self'";

#[cfg(feature = "server")]
const OPENUI_BUNDLE_JS: &str = include_str!("../../assets/openui/openui-bundle.min.js");
#[cfg(feature = "server")]
const OPENUI_STYLES_CSS: &str = include_str!("../../assets/openui/openui-styles.css");
#[cfg(feature = "server")]
const OPENUI_BOOTSTRAP_JS: &str = include_str!("../../assets/openui/frame-bootstrap.js");

/// The checked-in system prompt (D-09): `library.prompt(openuiChatPromptOptions)` generated
/// OFFLINE by `scripts/openui/gen-system-prompt.mjs`, never the managed hosted prompt.
/// Consumed by the generation overlay (`ws::openui_overlay_for`).
#[cfg(feature = "server")]
pub(crate) const OPENUI_SYSTEM_PROMPT: &str = include_str!("../../assets/openui/system-prompt.txt");

/// ADR-0007 "Generation path / Overlay content": the checked-in prompt advises image URLs and
/// documents `@OpenUrl`, `Query()` and `Mutation()`, none of which can work in the sandboxed
/// frame (images are CSP-blocked, `@OpenUrl` is dropped by the bridge, `Query`/`Mutation` need
/// a tool provider the frame never gets). Appended after the unmodified prompt so the model
/// emits none of them.
#[cfg(feature = "server")]
pub(crate) const OPENUI_OVERLAY_SUFFIX: &str =
    "\n\n## Host restrictions (these override anything above)\n\
- Do not use images, image URLs or any component that loads an image.\n\
- Do not use @OpenUrl or any link that navigates away.\n\
- Do not use Query() or Mutation(); there is no data source.\n\
- Use @ToAssistant for every button action.\n\
- Reply with OpenUI Lang only, no prose and no code fences.\n";

/// The one-shot overlay body for an OpenUI turn: the prompt plus the host restrictions.
#[cfg(feature = "server")]
pub(crate) fn openui_overlay_body() -> String {
    format!("{OPENUI_SYSTEM_PROMPT}{OPENUI_OVERLAY_SUFFIX}")
}

/// SHA-256 pins of the vendored files (ADR-0007 "Telemetry and supply chain"; also recorded in
/// `assets/openui/NOTICE`). Enforced by `openui_vendored_assets_match_pinned_sha256`.
#[cfg(all(test, feature = "server"))]
const OPENUI_BUNDLE_SHA256: &str =
    "bba936ff93f31a2955460f40d80bcd329b6a5848d6a05df54427e5dd137181df";
#[cfg(all(test, feature = "server"))]
const OPENUI_STYLES_SHA256: &str =
    "93b69b218203bd054a88b7c8821d6c0dc6944af38119ada37e8082e59c0a18f3";

/// Frame page paint, appended to the vendored stylesheet inside the one `<style>` element.
///
/// Why this exists (operator QA 2026-10-29, "the fonts are terrible"): the stock stylesheet
/// switches its token palette on `prefers-color-scheme` (dark OS = light text tokens) but
/// paints nothing behind them, so in a dark OS the iframe canvas stayed white under near-white
/// text. The frame now paints its own background and text from the host's theme variables
/// (posted as `openui-theme`); before the first message it falls back to the stock tokens,
/// which at least pair a background with its own text colour.
#[cfg(feature = "server")]
const OPENUI_FRAME_BASE_CSS: &str = "html,body{margin:0}\
body{padding:12px;background:var(--bg-card,var(--openui-background));\
color:var(--text,var(--openui-text-neutral-primary));font-family:var(--font,sans-serif)}";

/// The inlined frame document and its CSP, assembled once. A literal `</script` / `</style`
/// inside the inlined text would end its element early, so each is escaped before insertion.
/// The CSP pins the SHA-256 of the exact text placed inside each `<script>` element.
/// shortcut: lowercase end tags only (the vendored files are SHA-256 pinned and contain
/// none); revisit if a vendored upgrade ever introduces an upper-case one.
#[cfg(feature = "server")]
fn frame_assets() -> &'static (String, String) {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    static ASSETS: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    ASSETS.get_or_init(|| {
        let js = |s: &str| s.replace("</script", "<\\/script");
        let css = OPENUI_STYLES_CSS.replace("</style", "<\\/style");
        let (bundle, boot) = (js(OPENUI_BUNDLE_JS), js(OPENUI_BOOTSTRAP_JS));
        let pin = |body: &str| {
            let digest = Sha256::digest(body.as_bytes());
            format!(
                "'sha256-{}'",
                base64::engine::general_purpose::STANDARD.encode(digest)
            )
        };
        let csp = format!(
            "default-src 'none'; script-src {} {}; {OPENUI_FRAME_CSP_REST}",
            pin(&bundle),
            pin(&boot)
        );
        let doc = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><style>{css}{base}</style></head>\
             <body><div id=\"openui-root\"></div><script>{bundle}</script>\
             <script>{boot}</script></body></html>",
            base = OPENUI_FRAME_BASE_CSS,
        );
        (doc, csp)
    })
}

#[cfg(feature = "server")]
fn frame_document() -> &'static str {
    &frame_assets().0
}

#[cfg(feature = "server")]
fn frame_csp() -> &'static str {
    &frame_assets().1
}

/// `GET /openui/frame` — the sandbox-ready OpenUI renderer document.
///
/// Built via `Response::builder` + `Body::from` so exactly one CSP, one content type and
/// one nosniff header is set. `private` keeps the ~3.6 MB document out of shared caches.
#[cfg(feature = "server")]
pub async fn serve_openui_frame() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_SECURITY_POLICY, frame_csp())
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(CACHE_CONTROL, "private, max-age=86400")
        .header(REFERRER_POLICY, "no-referrer")
        .body(Body::from(frame_document()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn openui_frame_response_headers_are_exact() {
        let resp = serve_openui_frame().await;
        assert_eq!(resp.status(), StatusCode::OK);
        let h = resp.headers();
        let csp = h
            .get_all(CONTENT_SECURITY_POLICY)
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(csp.len(), 1, "exactly one CSP header");
        assert_eq!(csp[0].to_str().unwrap(), frame_csp());
        assert_eq!(h.get_all(CONTENT_TYPE).iter().count(), 1);
        assert_eq!(h[CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(h[X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(h[CACHE_CONTROL], "private, max-age=86400");
        assert_eq!(h[REFERRER_POLICY], "no-referrer");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), frame_document().as_bytes());
    }

    #[test]
    fn openui_frame_csp_keeps_containment() {
        for must in [
            "default-src 'none'",
            "connect-src 'none'",
            "img-src data:",
            "frame-src 'none'",
            "form-action 'none'",
            "base-uri 'none'",
            "frame-ancestors 'self'",
        ] {
            assert!(frame_csp().contains(must), "CSP lost `{must}`");
        }
        assert!(!frame_csp().contains("unsafe-eval"));
        // WR-03: scripts run only by hash, never `'unsafe-inline'`.
        let script_src = frame_csp()
            .split("; ")
            .find(|d| d.starts_with("script-src "))
            .expect("script-src directive");
        assert!(!script_src.contains("unsafe-inline"), "{script_src}");
        assert_eq!(script_src.matches("'sha256-").count(), 2, "{script_src}");
    }

    #[test]
    fn openui_frame_csp_pins_each_inline_script() {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        // Recompute from the served document: every inline script body must be pinned.
        let doc = frame_document();
        // The vendored bundle contains a literal `<script>` string, so anchor on the document's
        // own structure and on `</script>` (which is escaped inside the inlined JS).
        let start = doc.find("<div id=\"openui-root\"></div><script>").expect("root")
            + "<div id=\"openui-root\"></div><script>".len();
        let bodies: Vec<&str> = doc[start..]
            .split("</script>")
            .take(2)
            .map(|b| b.strip_prefix("<script>").unwrap_or(b))
            .collect();
        assert_eq!(bodies.len(), 2);
        for body in bodies {
            let pin = format!(
                "'sha256-{}'",
                base64::engine::general_purpose::STANDARD.encode(Sha256::digest(body.as_bytes()))
            );
            assert!(frame_csp().contains(&pin), "script body not pinned: {pin}");
        }
    }

    #[test]
    fn openui_frame_document_closes_tags_once() {
        let doc = frame_document();
        assert_eq!(doc.matches("</script>").count(), 2);
        assert_eq!(doc.matches("</style>").count(), 1);
        assert_eq!(
            doc.matches("</script").count(),
            2,
            "no stray </script inside inlined JS"
        );
        assert_eq!(
            doc.matches("</style").count(),
            1,
            "no stray </style inside inlined CSS"
        );
    }

    #[test]
    fn openui_bootstrap_has_no_tool_provider() {
        let src = OPENUI_BOOTSTRAP_JS;
        assert!(
            !src.contains("toolProvider"),
            "D-15: Renderer must get no tool provider"
        );
        assert!(!src.contains("fetch("));
        assert!(src.contains("event.source !== window.parent"));
        // ADR-0007 "changed": only the @ToAssistant action shape is forwarded.
        assert!(src.contains("continue_conversation"));
        assert!(!src.contains("open_url"));
    }

    #[test]
    fn bootstrap_has_fallback_and_partial_copy() {
        let src = OPENUI_BOOTSTRAP_JS;
        for must in [
            "The reply was not valid OpenUI. Showing it as text instead.",
            "TEXT FALLBACK",
            "Some components were skipped because they were not valid.",
            "PARTIAL",
        ] {
            assert!(src.contains(must), "bootstrap lost `{must}`");
        }
        assert_eq!(
            src.matches("postMessage(").count(),
            1,
            "the frame has exactly one outbound message type"
        );
    }

    #[test]
    fn frame_paints_its_own_themed_background_and_text() {
        // Regression for the pale-text-on-white defect: the document must paint a background
        // and text colour from the same source, and the bootstrap must pin the palette.
        let doc = frame_document();
        assert!(doc.contains("background:var(--bg-card,var(--openui-background))"));
        assert!(doc.contains("color:var(--text,var(--openui-text-neutral-primary))"));
        assert!(OPENUI_BOOTSTRAP_JS.contains("data-openui-theme"));
    }

    #[test]
    fn openui_frame_is_not_public() {
        let auth = include_str!("auth.rs");
        let start = auth.find("fn is_public(").expect("is_public exists");
        let region = &auth[start..];
        let region = &region[..region.find("\n}\n").expect("is_public ends")];
        assert!(
            !region.contains("/openui"),
            "/openui/frame must stay behind require_auth"
        );
    }

    /// Harness hook: `OPENUI_FRAME_DUMP=<path>` writes the served document to `<path>` and
    /// the CSP header to `<path>.csp` for `spike/frame-harness.mjs`. No-op otherwise.
    #[test]
    fn openui_frame_dump_for_harness() {
        if let Ok(path) = std::env::var("OPENUI_FRAME_DUMP") {
            std::fs::write(&path, frame_document()).unwrap();
            std::fs::write(format!("{path}.csp"), frame_csp()).unwrap();
        }
    }

    #[test]
    fn openui_vendored_assets_match_pinned_sha256() {
        use sha2::{Digest, Sha256};
        let hex = |b: &[u8]| format!("{:x}", Sha256::digest(b));
        assert_eq!(hex(OPENUI_BUNDLE_JS.as_bytes()), OPENUI_BUNDLE_SHA256);
        assert_eq!(hex(OPENUI_STYLES_CSS.as_bytes()), OPENUI_STYLES_SHA256);
    }

    #[test]
    fn openui_system_prompt_is_local_and_nonempty() {
        assert!(OPENUI_SYSTEM_PROMPT.len() > 10_000);
        assert!(
            !OPENUI_SYSTEM_PROMPT.contains("openui:config"),
            "managed-config marker found"
        );
    }

    #[test]
    fn openui_licence_files_ship() {
        let licence = include_str!("../../assets/openui/LICENSE");
        assert!(licence.starts_with("The MIT License"));
        assert!(licence.contains("Thesys"));
        let notice = include_str!("../../assets/openui/NOTICE");
        assert!(notice.contains("@openuidev/browser-bundle@0.2.1"));
        assert!(notice.contains("185  MIT") && notice.contains("BSD-3-Clause"));
        assert!(notice.contains(OPENUI_BUNDLE_SHA256) && notice.contains(OPENUI_STYLES_SHA256));
    }
}
