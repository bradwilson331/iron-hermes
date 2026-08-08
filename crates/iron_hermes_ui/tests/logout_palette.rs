//! Phase 47.3 Plan 06 (D-01/D-17, Correction 2) — `/logout` palette item +
//! `AuthedContext`.
//!
//! Locks:
//! - `/logout` is unioned into the LIVE `palette_data` feed in chat.rs, not
//!   the shared `command_router` registry that also feeds the TUI/CLI/
//!   gateway surfaces (logging out of a web session is meaningless there).
//! - `AuthedContext` is provided via `use_context_provider` at the
//!   `HermesApp` root in mod.rs — a child-level provider would panic
//!   sibling/ancestor consumers (Dioxus context-panic rule).
//! - The session cookie name never appears under `src/components/` — the
//!   client only ever observes auth state through a 401 it already
//!   received, never by reading the cookie.
//!
//! Pattern: source-read assertion (mirrors tests/kanban_server_fns.rs).

mod logout_palette {
    use std::fs;
    use std::path::{Path, PathBuf};

    fn crate_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn read(path: &str) -> String {
        fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
    }

    /// The literal session-cookie name (`server/auth.rs::SESSION_COOKIE`).
    /// Duplicated here as a plain string constant (not imported) so this
    /// test doesn't require pulling in server-only code — it only needs the
    /// literal to grep for.
    const SESSION_COOKIE_NAME: &str = "ih_session";

    #[test]
    fn chat_rs_unions_logout_into_the_live_palette_data_feed() {
        let src = read("src/components/hermes_app/screens/chat.rs");
        assert!(
            src.contains("\"/logout\""),
            "chat.rs must reference the literal \"/logout\" palette item"
        );
        // The push must happen after the palette_data resource is built from
        // list_slash_commands()/list_skills(), i.e. it's unioned into that
        // same live pipeline, not a separate hand-rolled list.
        let palette_data_idx = src
            .find("let mut palette_items")
            .expect("palette_items construction must exist");
        let logout_idx = src
            .find("cmd: \"/logout\".into()")
            .expect("the /logout PaletteItem literal must exist");
        assert!(
            logout_idx > palette_data_idx,
            "/logout must be pushed onto palette_items AFTER the live \
             list_slash_commands()/list_skills() union, not before it"
        );
    }

    #[test]
    fn logout_is_not_registered_in_the_shared_command_router_registry() {
        // The shared registry lives outside iron_hermes_ui entirely
        // (ironhermes-core::commands) and this plan's files_modified never
        // touches it — this test pins that invariant so a future change
        // can't silently leak /logout into the TUI/CLI/gateway surfaces.
        let workspace_root = crate_root()
            .parent()
            .and_then(Path::parent)
            .expect("crate root must have a workspace parent")
            .to_path_buf();
        let registry_path = workspace_root.join("crates/ironhermes-core/src/commands/mod.rs");
        if registry_path.exists() {
            let src = fs::read_to_string(&registry_path)
                .expect("failed to read the shared command_router registry");
            assert!(
                !src.to_lowercase().contains("logout"),
                "the shared command_router registry (ironhermes-core::commands) \
                 must NOT contain a logout entry — /logout is web-only"
            );
        }
    }

    #[test]
    fn mod_rs_provides_authed_context_at_the_hermes_app_root() {
        let src = read("src/components/hermes_app/mod.rs");
        assert!(
            src.contains("struct AuthedContext"),
            "mod.rs must define the AuthedContext newtype"
        );
        assert!(
            src.contains("use_context_provider(|| AuthedContext("),
            "mod.rs must provide AuthedContext via use_context_provider at the HermesApp root"
        );
        // Newtype discipline: never a bare Signal<bool> provider for this.
        assert!(
            src.contains("pub struct AuthedContext(pub Signal<bool>)"),
            "AuthedContext must be a newtype wrapper around Signal<bool>, \
             never a bare Signal<bool> provider (context-newtype discipline)"
        );
    }

    #[test]
    fn no_component_source_reads_or_writes_the_session_cookie_name() {
        let components_dir = crate_root().join("src/components");
        let mut offenders = Vec::new();
        visit_rs_files(&components_dir, &mut offenders);
        assert!(
            offenders.is_empty(),
            "no file under src/components/ may reference the session cookie \
             name '{SESSION_COOKIE_NAME}' — the client only ever observes auth \
             state through a 401, never by reading the cookie. Offenders: {offenders:?}"
        );
    }

    fn visit_rs_files(dir: &Path, offenders: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit_rs_files(&path, offenders);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                if let Ok(src) = fs::read_to_string(&path) {
                    if src.contains(SESSION_COOKIE_NAME) {
                        offenders.push(path);
                    }
                }
            }
        }
    }

    #[test]
    fn perform_logout_uses_no_new_wasm_dependency() {
        let src = read("src/components/hermes_app/screens/chat.rs");
        // Check for an actual `use gloo_net...` import, not just any mention
        // of the crate name (a doc comment explaining why gloo-net was NOT
        // added would otherwise trip a literal string match — the same
        // false-positive class as Plan 01's `@import` grep lesson).
        assert!(
            !src.contains("use gloo_net"),
            "perform_logout must use the existing js_sys::Reflect/web_sys \
             precedent, not a new gloo-net import (RESEARCH A4)"
        );
        assert!(
            src.contains("fn perform_logout"),
            "chat.rs must define perform_logout"
        );
        assert!(
            src.contains("js_sys::Reflect"),
            "perform_logout must use js_sys::Reflect (mirrors ui_prefs.rs)"
        );
    }
}
