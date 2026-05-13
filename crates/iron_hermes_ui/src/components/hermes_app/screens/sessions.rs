//! Phase 26.2.1 Plan 07 — Sessions screen (LIVE-wired).
//!
//! Replaces the Plan 03 placeholder with a working session list backed by the
//! existing `list_sessions()` server fn (`src/server/api.rs`). Clicking a row
//! writes the `SessionIdContext` newtype (B-03 — defined in Plan 02, provided
//! in Plan 06) and the `active_screen` `Signal<Screen>` back to `Screen::Chat`
//! so the Chat screen picks up the switch.
//!
//! Per D-02 the server tree is byte-for-byte untouched. A `delete_session`
//! server fn does NOT exist today (the legacy `on_tab_close` in
//! `warp_hermes.rs:653-690` is purely client-side too), so the close button
//! removes the row from the local "hidden" set instead of issuing a network
//! call. This is documented in `26.2.1-deferred-items.md` and the Plan 07
//! SUMMARY — adding a server-side delete is deferred to a follow-up phase.
//!
//! Per Phase 26.2 D-09 / `title_bar.rs:65-83`, the close button uses
//! `evt.stop_propagation()` so the click does not bubble to the row's
//! `onclick` handler (which would otherwise select the row that is about to
//! disappear).

use dioxus::prelude::*;
use std::collections::HashSet;

/// Sessions screen — `<section id="screen-sessions">` ported from
/// `app.html` line 416. Lists sessions via `list_sessions()` and writes
/// `SessionIdContext` + `active_screen` on row click.
#[component]
pub fn ScreenSessions(is_active: bool) -> Element {
    // Fetch the session list once on mount. `use_server_future` returns
    // `Option<Result<Vec<SessionInfo>, ServerFnError>>` — the `?` operator
    // suspends the component until the future has resolved at least once
    // (Dioxus 0.7 PATTERNS Cross-cutting: use_server_future).
    let sessions_resource = use_server_future(crate::server::api::list_sessions)?;

    // Context — `Signal<Screen>` (provided by HermesApp::mod.rs) and the
    // B-03 newtype-wrapped session id (provided by Plan 06 in HermesApp).
    let mut active_screen = use_context::<Signal<crate::state::Screen>>();
    let mut session_id = use_context::<crate::state::SessionIdContext>().0;

    // Local optimistic-delete set — entries the user has clicked "×" on.
    // Until a backend `delete_session` server fn exists, removing a row
    // from the rendered list is the best we can do without violating D-02.
    let mut hidden = use_signal(HashSet::<String>::new);

    // Drop the inner reads before constructing the event closures so they
    // do not collide with the writes inside `with_mut` / `set` calls below
    // (clippy.toml signal-borrow rules).
    let sessions: Vec<crate::server::api::SessionInfo> = match sessions_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };
    let current = session_id.read().clone();
    let hidden_snapshot = hidden.read().clone();

    // Filter out optimistically-deleted rows.
    let visible: Vec<crate::server::api::SessionInfo> = sessions
        .into_iter()
        .filter(|s| !hidden_snapshot.contains(&s.id))
        .collect();
    let count = visible.len();

    // Row select: switch the current session and jump to Chat.
    let on_select = move |sid: String| {
        session_id.set(sid);
        active_screen.set(crate::state::Screen::Chat);
    };

    // Row delete (optimistic, client-side only — see file-level docstring).
    let on_delete = move |sid: String| {
        hidden.with_mut(|set| {
            set.insert(sid);
        });
        // Future hook: when a `delete_session(sid)` server fn lands, spawn
        // the call here. For now the deletion is purely visual; reloading
        // the page restores the full server-side list.
    };

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-sessions",
            "data-screen-label": "02 Sessions",

            // ── Header ────────────────────────────────────────────────
            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 02" }
                    h1 { class: "screen-title", "Sessions" }
                    p { class: "screen-sub",
                        "Browse and resume past conversations. {count} live transcript",
                        if count == 1 { "" } else { "s" },
                        " from this profile."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "⊞ FILTER" }
                    button { class: "btn btn--sm", "+ NEW SESSION" }
                }
            }

            // ── Section label ────────────────────────────────────────
            div { class: "section-label",
                "Sessions "
                span { class: "count", "· {count}" }
            }

            // ── Row list ─────────────────────────────────────────────
            div { class: "row-list", style: "grid-template-columns: 1fr;",
                for s in visible.iter().cloned() {
                    SessionRow {
                        key: "{s.id}",
                        session: s.clone(),
                        is_current: s.id == current,
                        on_select: on_select,
                        on_delete: on_delete,
                    }
                }
            }
        }
    }
}

/// One row in the sessions list. Click anywhere on the row to switch
/// sessions; click the close "×" to hide it (with `evt.stop_propagation()`
/// so the row click handler does not also fire — Phase 26.2 D-09 /
/// `title_bar.rs:65-83`).
#[component]
fn SessionRow(
    session: crate::server::api::SessionInfo,
    is_current: bool,
    on_select: EventHandler<String>,
    on_delete: EventHandler<String>,
) -> Element {
    let sid_for_select = session.id.clone();
    let sid_for_delete = session.id.clone();

    // Title fallback: SessionInfo.title is Option<String>; fall back to
    // the id when the server has not assigned a human label yet.
    let title = session
        .title
        .clone()
        .unwrap_or_else(|| session.id.clone());

    // Keep the last 8 characters of long server-side session keys so two
    // parallel sessions are still distinguishable in the row sub-text.
    let id_tail: String = if session.id.len() <= 12 {
        session.id.clone()
    } else {
        let tail = &session.id[session.id.len() - 8..];
        format!("…{tail}")
    };
    let sub = format!(
        "{msgs} msg{plural} · session {id_tail}",
        msgs = session.message_count,
        plural = if session.message_count == 1 { "" } else { "s" },
    );

    rsx! {
        div {
            class: "row",
            class: if is_current { "is-active" },
            style: "grid-template-columns: 1fr auto auto;",
            onclick: move |_| on_select.call(sid_for_select.clone()),
            div { class: "row-main",
                span { class: "row-title", "{title}" }
                span { class: "row-sub", "{sub}" }
            }
            if is_current {
                span { class: "pill green", "ACTIVE" }
            } else {
                span { class: "pill", "—" }
            }
            button {
                class: "btn btn--ghost btn--sm",
                "aria-label": "Delete session",
                title: "Delete session",
                onclick: move |evt| {
                    evt.stop_propagation();
                    on_delete.call(sid_for_delete.clone());
                },
                "×"
            }
        }
    }
}

