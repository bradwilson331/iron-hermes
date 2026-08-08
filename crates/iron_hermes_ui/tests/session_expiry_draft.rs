//! Phase 47.3 Plan 06 (D-17) — session-death redirect with composer-draft
//! stash and restore.
//!
//! Locks:
//! - `ui_prefs.rs` exposes both `stash_composer_draft` and
//!   `restore_composer_draft`, built on the existing `write_string`/
//!   `read_string` helpers under the same `cfg(target_arch = "wasm32")` gate.
//! - The stash key is textually distinct from the session cookie name.
//! - No file under `crates/iron_hermes_ui/src/` writes the session cookie
//!   name into web storage.
//! - The redirect path (`trigger_session_expiry_redirect`) is reachable
//!   from BOTH the server-fn 401 handler (`is_unauthorized_error` /
//!   `palette_data`) and the WebSocket close handler (`is_ws_connected`,
//!   fed by mod.rs's receive loop).
//!
//! Pattern: source-read assertion (mirrors tests/kanban_server_fns.rs).

mod session_expiry_draft {
    use std::fs;
    use std::path::{Path, PathBuf};

    fn crate_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn read(path: &str) -> String {
        fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
    }

    const SESSION_COOKIE_NAME: &str = "ih_session";

    #[test]
    fn ui_prefs_exposes_both_stash_and_restore_helpers() {
        let src = read("src/ui_prefs.rs");
        assert!(
            src.contains("fn stash_composer_draft"),
            "ui_prefs.rs must expose stash_composer_draft"
        );
        assert!(
            src.contains("fn restore_composer_draft"),
            "ui_prefs.rs must expose restore_composer_draft"
        );
        // Built on the existing helpers, not a new storage primitive.
        assert!(
            src.contains("write_string(KEY_SESSION_DRAFT"),
            "stash_composer_draft must delegate to the existing write_string helper"
        );
        assert!(
            src.contains("read_string(KEY_SESSION_DRAFT"),
            "restore_composer_draft must delegate to the existing read_string helper"
        );
    }

    #[test]
    fn stash_key_is_distinct_from_session_cookie_name() {
        let src = read("src/ui_prefs.rs");
        assert!(
            src.contains("KEY_SESSION_DRAFT"),
            "ui_prefs.rs must define a dedicated KEY_SESSION_DRAFT constant"
        );
        assert!(
            !src.contains(&format!("KEY_SESSION_DRAFT: &str = \"{SESSION_COOKIE_NAME}\"")),
            "KEY_SESSION_DRAFT must never literally equal the session cookie name"
        );
    }

    #[test]
    fn no_file_under_src_writes_the_session_cookie_name_into_web_storage() {
        // Scoped to ui_prefs.rs specifically (the module that owns every
        // localStorage write in this crate) — the broader "no component
        // reads/writes the cookie name" invariant is covered by
        // logout_palette.rs's scan over src/components/.
        //
        // Checks for an ACTUAL storage call with the cookie-name literal
        // (`write_string("ih_session"`, `read_string("ih_session"`, or the
        // raw `get_item`/`set_item` calls the storage module wraps), not
        // just any mention of the string — a doc comment explaining WHY
        // the keys are distinct would otherwise trip a bare substring
        // match (the same false-positive class as Plan 01's `@import`
        // grep lesson).
        let src = read("src/ui_prefs.rs");
        let cookie_literal = format!("\"{SESSION_COOKIE_NAME}\"");
        for call_prefix in ["write_string(", "read_string(", "get_item(", "set_item("] {
            assert!(
                !src.contains(&format!("{call_prefix}{cookie_literal}")),
                "ui_prefs.rs must never call {call_prefix}{cookie_literal} — \
                 only the composer draft crosses into localStorage, never the token"
            );
        }
    }

    #[test]
    fn redirect_path_is_reachable_from_the_server_fn_401_handler() {
        let src = read("src/components/hermes_app/screens/chat.rs");
        assert!(
            src.contains("fn is_unauthorized_error"),
            "chat.rs must define is_unauthorized_error"
        );
        assert!(
            src.contains("fn trigger_session_expiry_redirect"),
            "chat.rs must define trigger_session_expiry_redirect"
        );
        // palette_data must check both calls' results with is_unauthorized_error
        // and call trigger_session_expiry_redirect on a positive match — this is
        // the server-fn 401 trigger point.
        let palette_data_idx = src
            .find("let palette_data = use_resource")
            .expect("palette_data resource must exist");
        let palette_data_body = &src[palette_data_idx..];
        let resource_end = palette_data_body
            .find("});")
            .map(|i| i + 3)
            .unwrap_or(palette_data_body.len());
        let palette_data_body = &palette_data_body[..resource_end];
        assert!(
            palette_data_body.contains("is_unauthorized_error(&cmds_result)"),
            "palette_data must check list_slash_commands()'s result with is_unauthorized_error"
        );
        assert!(
            palette_data_body.contains("is_unauthorized_error(&skills_result)"),
            "palette_data must check list_skills()'s result with is_unauthorized_error"
        );
        assert!(
            palette_data_body.contains("trigger_session_expiry_redirect(authed"),
            "palette_data must call trigger_session_expiry_redirect on a 401"
        );
    }

    #[test]
    fn redirect_path_is_reachable_from_the_websocket_close_handler() {
        let chat_src = read("src/components/hermes_app/screens/chat.rs");
        // chat.rs watches WsConnectedContext and calls the same redirect fn
        // on a true->false transition.
        assert!(
            chat_src.contains("use_context::<crate::state::WsConnectedContext>()"),
            "chat.rs must consume WsConnectedContext to observe WS close"
        );
        let watcher_idx = chat_src
            .find("was_ws_connected")
            .expect("the WS-close watcher (was_ws_connected) must exist in chat.rs");
        assert!(
            chat_src[watcher_idx..].contains("trigger_session_expiry_redirect(authed"),
            "the WS-close watcher must call trigger_session_expiry_redirect"
        );

        // mod.rs's hoisted receive loop is what actually flips
        // is_ws_connected false on Message::Close / Err — confirming the
        // other end of this reachability chain.
        let mod_src = read("src/components/hermes_app/mod.rs");
        assert!(
            mod_src.contains("Message::Close") && mod_src.contains("is_ws_connected.set(false)"),
            "mod.rs's receive loop must set is_ws_connected false on WS close"
        );
    }

    #[test]
    fn no_polling_probe_was_added() {
        // D-17 explicitly forbids a polling probe — the redirect must be
        // purely event-driven (401 detection + WS-close observation).
        let src = read("src/components/hermes_app/screens/chat.rs");
        assert!(
            !src.contains("use_interval") && !src.contains("set_interval"),
            "D-17 forbids a polling probe for session-death detection"
        );
    }

    #[test]
    fn restore_happens_once_on_mount_and_populates_the_composer() {
        let src = read("src/components/hermes_app/screens/chat.rs");
        assert!(
            src.contains("restore_composer_draft()"),
            "ScreenChat must call restore_composer_draft() on mount"
        );
        let restore_idx = src
            .find("draft_restored")
            .expect("the one-shot restore guard (draft_restored) must exist");
        assert!(
            src[restore_idx..].contains("input.set(draft)"),
            "a restored draft must populate the composer's input signal"
        );
    }

    #[test]
    fn no_component_source_writes_the_session_cookie_name() {
        let components_dir = crate_root().join("src/components");
        let mut offenders = Vec::new();
        visit_rs_files(&components_dir, &mut offenders);
        assert!(
            offenders.is_empty(),
            "no file under src/components/ may reference the session cookie \
             name '{SESSION_COOKIE_NAME}'. Offenders: {offenders:?}"
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
}
