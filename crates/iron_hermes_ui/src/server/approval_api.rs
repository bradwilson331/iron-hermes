//! Phase 57 D-14 (folds backlog 999.3): the browser's way to answer a parked approval.
//!
//! # Security posture
//!
//! [`resolve_web_approval`] is the ONLY way an approval is granted on the web. It is a
//! `#[server]` function, so it sits behind `auth::require_auth` like every server
//! function in this crate (when `web_ui.auth` is enabled; with it off the surface is
//! loopback-only by the bind guard). This module adds no auth code of its own and does
//! not weaken that boundary.
//!
//! Nothing in the chat can reach it. A model's output and a user's typed message are
//! text on the turn's event channel; the registry is keyed by a random UUID the server
//! minted and sent only in the `ApprovalRequested` event. The core
//! ([`resolve_web_approval_inner`]) refuses a non-UUID, an unknown id, an expired id and
//! an id that was already resolved, and in each case changes nothing.
//!
//! The module is declared WITHOUT a cfg gate (like `provider_config_api`) so the wasm
//! client gets the `#[server]` call stubs; the `_inner` cores and their server-only
//! imports are gated.

use dioxus::prelude::*;

/// Resolve the approval parked under `approval_id`: `approved == true` runs the gated
/// command, `false` refuses it. Returns `Ok(true)` when a live approval was resolved and
/// `Ok(false)` when nothing was (unknown, expired, already resolved, or malformed id).
#[server]
pub async fn resolve_web_approval(
    approval_id: String,
    approved: bool,
) -> Result<bool, ServerFnError> {
    #[cfg(feature = "server")]
    {
        Ok(resolve_web_approval_inner(
            &crate::server::state::global_app_state().web_approvals,
            &approval_id,
            approved,
        ))
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (approval_id, approved);
        Err(ServerFnError::new(
            "resolve_web_approval is a server function",
        ))
    }
}

/// The testable core of [`resolve_web_approval`]. `approval_id` must be a UUID string;
/// anything else is `false` without touching the registry.
#[cfg(feature = "server")]
pub fn resolve_web_approval_inner(
    registry: &crate::server::web_approval_gate::WebApprovalRegistry,
    approval_id: &str,
    approved: bool,
) -> bool {
    // A malformed id never reaches the registry: only ids this server minted are UUIDs.
    if uuid::Uuid::parse_str(approval_id).is_err() {
        return false;
    }
    registry.resolve_by_id(approval_id, approved)
}

/// Longest session id `session_defcon_level` accepts. The registry creates an entry on
/// first lookup, so an unbounded id is an unbounded allocation (T-57-76).
#[cfg(feature = "server")]
const MAX_SESSION_ID_CHARS: usize = 128;

/// Read a session's live DEFCON level (UI-SPEC R9c). The chat header calls this on mount
/// and on every session switch; level changes made while connected arrive as
/// `ChatStreamEvent::DefconLevel` pushes. `bypass` is computed here, from the server's own
/// `DefconLevel::yolo_bypass_active()`, so the client never re-derives the D-21 rule.
///
/// Behind `require_auth` like every server function. An empty or oversized id is an
/// error before the registry is touched.
#[server]
pub async fn session_defcon_level(
    session_id: String,
) -> Result<crate::protocol::SessionDefconInfo, ServerFnError> {
    #[cfg(feature = "server")]
    {
        session_defcon_level_inner(&session_id).map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = session_id;
        Err(ServerFnError::new(
            "session_defcon_level is a server function",
        ))
    }
}

/// The testable core of [`session_defcon_level`].
#[cfg(feature = "server")]
pub fn session_defcon_level_inner(
    session_id: &str,
) -> Result<crate::protocol::SessionDefconInfo, String> {
    // Reject before the lookup: `handle_for` creates an entry for an id it has not seen.
    if session_id.is_empty() {
        return Err("session id must not be empty".to_string());
    }
    if session_id.chars().count() > MAX_SESSION_ID_CHARS {
        return Err(format!(
            "session id must be at most {MAX_SESSION_ID_CHARS} characters"
        ));
    }
    let level = ironhermes_core::defcon::session_defcon()
        .handle_for(session_id)
        .get();
    Ok(crate::protocol::SessionDefconInfo {
        level: level.level(),
        bypass: level.yolo_bypass_active(),
    })
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::resolve_web_approval_inner;
    use crate::server::web_approval_gate::WebApprovalRegistry;
    use ironhermes_core::commands::context::CommandContext;
    use ironhermes_core::commands::{CommandResult, CommandRouter, ResolveResult};
    use ironhermes_core::types::Platform;

    /// The text of a source file before its first `#[cfg(test)]`, so a scan sees only
    /// production code and never the test that scans it.
    fn production_text(src: &str) -> &str {
        src.split("#[cfg(test)]").next().unwrap_or(src)
    }

    /// UI-SPEC R9c: the server function reads the session's LIVE level and computes
    /// `bypass` itself; a never-seen id gets the process default; a bad id is rejected
    /// before any registry lookup (a lookup would create an entry).
    #[test]
    fn session_defcon_level_reads_live_level() {
        use super::session_defcon_level_inner;
        use crate::protocol::SessionDefconInfo;
        use ironhermes_core::config::DefconLevel;
        use ironhermes_core::defcon::session_defcon;

        let _env = crate::server::test_support::env_lock();
        let registry = session_defcon();
        let saved_default = registry.process_default();
        registry.set_process_default(DefconLevel::THREE);

        registry.set_session_level("s-web-chip", DefconLevel::FIVE);
        assert_eq!(
            session_defcon_level_inner("s-web-chip"),
            Ok(SessionDefconInfo {
                level: 5,
                bypass: true
            })
        );
        registry.set_session_level("s-web-chip", DefconLevel::ONE);
        assert_eq!(
            session_defcon_level_inner("s-web-chip"),
            Ok(SessionDefconInfo {
                level: 1,
                bypass: false
            }),
            "the level is read live, not cached"
        );

        assert_eq!(
            session_defcon_level_inner("s-web-chip-never-seen"),
            Ok(SessionDefconInfo {
                level: 3,
                bypass: false
            }),
            "a never-seen id reads the process default"
        );

        // Rejected before the registry is touched: nothing may be created for them.
        let long = "x".repeat(129);
        assert!(session_defcon_level_inner("").is_err());
        assert!(session_defcon_level_inner(&long).is_err());
        let ok_edge = "y".repeat(128);
        assert!(
            session_defcon_level_inner(&ok_edge).is_ok(),
            "128 is the limit"
        );

        for id in ["s-web-chip", "s-web-chip-never-seen", ok_edge.as_str()] {
            registry.forget(id);
        }
        registry.set_process_default(saved_default);
    }

    /// UI-SPEC R9c: the DEFCON push is wired, not just defined. In ws.rs's production
    /// code `web_level_change_events(` appears at its definition and at the slash-dispatch
    /// call (at the base commit it appeared nowhere), and `/yolo` gets its auth facts.
    #[test]
    fn web_level_push_is_wired_into_slash_dispatch() {
        let ws = production_text(include_str!("ws.rs"));
        assert!(
            ws.matches("web_level_change_events(").count() >= 2,
            "the slash dispatch must call web_level_change_events"
        );
        assert!(
            ws.contains("with_defcon_auth("),
            "the slash dispatch must supply the D-22 facts"
        );
        assert!(
            ws.matches("web_inbound_gate(").count() >= 3,
            "typed message, STT transcript and queue drain must each be screened"
        );
    }

    /// D-08/D-21: the web surface reads the live session level, never a boot-time copy of
    /// the yolo flag. The needle is built with `concat!` so this file does not contain it.
    #[test]
    fn no_web_yolo_snapshot() {
        let needle = concat!("autonomous", ".yolo");
        let live: Vec<&str> = include_str!("state.rs")
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| l.contains(needle))
            .collect();
        assert!(
            live.is_empty(),
            "state.rs must not capture the boot-time yolo flag: {live:?}"
        );
    }

    /// T-57-48: an approval can never be granted by chat text or model output. The
    /// only caller of the registry's resolve functions is `resolve_web_approval`.
    #[test]
    fn chat_text_cannot_approve() {
        // 1. `/approve <id>` and `/deny` do resolve on the web surface (the registry
        //    marks them for gateway-style platforms), but the core handler is a stub that
        //    reports there is no approval queue: the command context carries no handle to
        //    the web registry. Dispatch them for real against a registry with a live
        //    approval and show it is untouched.
        let registry = WebApprovalRegistry::new();
        let (parked_id, mut receiver) = registry
            .park("sess-chat", std::time::Duration::from_secs(60))
            .expect("park");
        let router = CommandRouter::new(ironhermes_core::commands::registry::build_registry());
        for text in [
            format!("/approve {parked_id}"),
            format!("/approve {parked_id} always"),
            "/approve".to_string(),
            format!("/deny {parked_id}"),
            "/deny".to_string(),
        ] {
            if let ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) =
                router.resolve(&text, &Platform::Web)
            {
                let parts: Vec<&str> = text.split_whitespace().collect();
                let ctx = CommandContext::new(Platform::Web, "sess-chat".to_string());
                let result =
                    ironhermes_core::commands::handlers::dispatch(def, &parts[1..], &ctx, &router);
                assert!(
                    !matches!(result, CommandResult::Handled),
                    "{text:?} must not report a handled approval"
                );
            }
        }
        // Plain words typed into the chat are just message text; nothing parses them.
        assert!(
            registry.has_pending("sess-chat"),
            "chat text must leave the approval parked"
        );
        assert!(
            receiver.try_recv().is_err(),
            "no decision may have reached the waiting gate"
        );
        // Only the server function's core resolves it.
        assert!(resolve_web_approval_inner(&registry, &parked_id, true));
        assert_eq!(receiver.try_recv(), Ok(true));

        // 2. Structurally: nothing on the chat path reaches the registry's resolve
        //    functions, and ws.rs touches the registry only to cancel on /stop.
        let ws = production_text(include_str!("ws.rs"));
        let state = production_text(include_str!("state.rs"));
        for (name, text) in [("ws.rs", ws), ("state.rs", state)] {
            for needle in [
                "resolve_by_id(",
                "resolve_by_session(",
                "resolve_web_approval",
            ] {
                assert!(
                    !text.contains(needle),
                    "{name} must not call {needle}: only the authenticated server function may"
                );
            }
        }
        for (idx, _) in ws.match_indices("web_approvals") {
            assert!(
                ws[idx..].starts_with("web_approvals.cancel("),
                "ws.rs may only cancel a parked approval: {}",
                &ws[idx..(idx + 40).min(ws.len())]
            );
        }
    }
}
