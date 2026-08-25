//! Phase 48.2 Plan 04 Task 1 (D-01/D-03/D-04/D-12/D-14/D-19/D-20): the
//! MCP SERVERS section — every configured server as a row with honest
//! status and full lifecycle actions (enable/disable, CONNECT for OAuth
//! servers, REMOVE), driven by the Plan 02 admin API
//! (`crate::server::mcp_admin_api`).
//!
//! # Honest status, never paraphrased
//!
//! [`mcp_status_label`]/[`mcp_status_pill_class`] render exactly the six D-03
//! locked strings (`CONNECTED`/`AUTH REQUIRED`/`AUTHORIZE`/`CONNECTING`/
//! `SPAWN FAILED`/`DISABLED` — the latter two net-new in Phase 48.2 Plan 09)
//! — `SPAWN FAILED` NEVER collapses into the `DISABLED` treatment; its
//! underlying reason is always reachable from the row via a title attribute,
//! never discarded. `AwaitingAuthorization` and `Connecting` (Plan 09) are
//! two distinguishable non-terminal facts — "the operator must act" versus
//! "the server is doing something" — never collapsed into one ambiguous
//! answer either.
//!
//! # Blast radius (D-12)
//!
//! `McpManager` exposes only bulk reload primitives — any row action here
//! restarts every configured MCP server, not only the one being changed.
//! `McpSection` renders that fact once, beneath the section header, so the
//! operator is told rather than surprised at UAT.
//!
//! # refresh_tick is the PAGE-LEVEL signal, not a local one
//!
//! `McpSection` takes the shell's `Signal<u32>` directly (not split into a
//! `ReadSignal` + `EventHandler` pair) because Task 3's MCP catalog display
//! group ALSO writes `mcp_servers.<name>.enabled` and bumps the same tick —
//! a write from either surface must refresh both. Mirrors the
//! `mut refresh_tick: Signal<u32>` prop shape already established by
//! `bot_roster/group_chat_workspace.rs`.

use crate::server::mcp_admin_api::{McpServerRow, McpServerStatus, McpTransportKind};
use crate::server::tools_config_api::ConfigScope;
use dioxus::prelude::*;

// =============================================================================
// Pure functions — status vocabulary (D-03), directly unit-testable.
// =============================================================================

/// The six D-03 locked status strings — never paraphrased, never collapsed
/// into one another.
#[allow(dead_code)] // used inside McpServerRowView rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn mcp_status_label(status: &McpServerStatus) -> &'static str {
    match status {
        McpServerStatus::Connected => "CONNECTED",
        McpServerStatus::AuthRequired => "AUTH REQUIRED",
        McpServerStatus::AwaitingAuthorization { .. } => "AUTHORIZE",
        McpServerStatus::Connecting => "CONNECTING",
        McpServerStatus::SpawnFailed { .. } => "SPAWN FAILED",
        McpServerStatus::Disabled => "DISABLED",
    }
}

/// Status-to-pill-class mapping. `SpawnFailed` MUST NOT map to the same
/// class as `Disabled` — a real failure is never rendered as a mere
/// disabled state (acceptance criterion, Task 1). `AwaitingAuthorization`
/// reuses the existing amber `.pill.amber` treatment (screens.css) — the
/// same visual weight `AuthRequired` already carries, since both mean "the
/// operator has something to do". `Connecting` reuses the dimmed
/// `mcp-status-pending` treatment already established for the loading
/// skeleton (Plan 09).
#[allow(dead_code)] // used inside McpServerRowView rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn mcp_status_pill_class(status: &McpServerStatus) -> &'static str {
    match status {
        McpServerStatus::Connected => "pill teal",
        McpServerStatus::AuthRequired => "pill amber",
        McpServerStatus::AwaitingAuthorization { .. } => "pill amber mcp-status-authorize",
        McpServerStatus::Connecting => "pill mcp-status-pending",
        McpServerStatus::SpawnFailed { .. } => "pill red",
        McpServerStatus::Disabled => "pill",
    }
}

/// The title-attribute text for a status pill — for `SpawnFailed`, the
/// row's real failure reason (never discarded); otherwise the label itself,
/// except for `AwaitingAuthorization`/`Connecting`, whose title tells the
/// operator what to do rather than repeating the pill text.
#[allow(dead_code)] // used inside McpServerRowView rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn mcp_status_title(status: &McpServerStatus) -> String {
    match status {
        McpServerStatus::SpawnFailed { reason } => reason.clone(),
        McpServerStatus::AwaitingAuthorization { .. } => {
            "Open the authorization link and complete sign-in at the authorization server".to_string()
        }
        McpServerStatus::Connecting => "Connecting — no action needed".to_string(),
        other => mcp_status_label(other).to_string(),
    }
}

#[allow(dead_code)] // used inside McpServerRowView rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn mcp_transport_label(transport: &McpTransportKind) -> &'static str {
    match transport {
        McpTransportKind::Stdio => "stdio",
        McpTransportKind::Http => "http",
    }
}

// =============================================================================
// Components
// =============================================================================

/// The MCP SERVERS section — mounted beneath the toolset groups on the
/// Tools screen. Owns its own `list_mcp_servers` resource; `scope` and
/// `refresh_tick` are read in the sync prefix so either changing (a bump
/// from THIS section's own writes, or from Task 3's catalog MCP master
/// toggle) re-fetches.
#[component]
pub fn McpSection(scope: ConfigScope, writable: bool, mut refresh_tick: Signal<u32>) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let mut wizard_open: Signal<bool> = use_signal(|| false);

    let scope_for_resource = scope.clone();
    let rows_resource = use_resource(move || {
        let scope_value = scope_for_resource.clone();
        let _tick = refresh_tick();
        async move { crate::server::mcp_admin_api::list_mcp_servers(scope_value).await }
    });

    // Extract every value out of the resource BEFORE rsx! — no signal
    // borrow held across the macro (iron_hermes_ui/clippy.toml).
    let is_loading = rows_resource().is_none();
    let load_error: Option<String> = match rows_resource() {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let rows: Vec<McpServerRow> = match rows_resource() {
        Some(Ok(r)) => r,
        _ => Vec::new(),
    };
    let count = rows.len();
    let scope_for_rows = scope.clone();

    rsx! {
        div { class: "mcp-section", id: "mcp-servers-section",
            div { class: "mcp-section-header",
                div { class: "section-label", "MCP SERVERS ({count})" }
                if writable {
                    button {
                        class: "btn",
                        onclick: move |_| wizard_open.set(true),
                        "IMPORT SERVER"
                    }
                }
            }
            div { class: "mcp-section-blast-radius-note",
                "Any change here reconnects every configured MCP server, not just this one."
            }
            if is_loading {
                div { class: "mcp-servers-list",
                    for i in 0..2 {
                        div { key: "{i}", class: "mcp-server-row mcp-server-row--skeleton",
                            span { class: "pill", "···" }
                        }
                    }
                }
            } else if let Some(reason) = load_error {
                div { class: "tools-error-banner",
                    span { "COULD NOT LOAD MCP SERVERS — {reason}" }
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| {
                            let cur = *refresh_tick.read();
                            refresh_tick.set(cur + 1);
                        },
                        "RETRY"
                    }
                }
            } else if rows.is_empty() {
                div { class: "tools-empty-state",
                    div { class: "tools-empty-heading", "NO MCP SERVERS YET" }
                    div { class: "tools-empty-body",
                        "Import a server to extend Hermes with new tools — paste a Claude-Desktop-style JSON snippet or fill the form."
                    }
                    if writable {
                        button {
                            class: "btn",
                            onclick: move |_| wizard_open.set(true),
                            "IMPORT SERVER"
                        }
                    }
                }
            } else {
                div { class: "mcp-servers-list ih-scroll",
                    for row in rows.iter().cloned() {
                        McpServerRowView {
                            key: "{row.name}",
                            row,
                            scope: scope_for_rows.clone(),
                            writable,
                            refresh_tick,
                        }
                    }
                }
            }
            if *wizard_open.read() {
                super::import_wizard::ImportWizard {
                    scope: scope.clone(),
                    refresh_tick,
                    on_close: move |_| wizard_open.set(false),
                }
            }
        }
    }
}

/// Read `window.location.origin` (Phase 48.2 Plan 09, D-09). The crate's
/// enabled `web-sys` features do not include `"Location"`
/// (`Cargo.toml:29-42`), so `web_sys::Window::location()` would not compile
/// — this follows `ui_prefs.rs`'s established `js_sys::Reflect`
/// property-access workaround exactly (no `Cargo.toml` edit, no new
/// `web-sys` feature). The non-wasm arm returns an empty string, which
/// `resolve_redirect_base`/`validate_web_redirect_base` on the server side
/// honestly rejects rather than silently guessing an origin.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn browser_origin() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return String::new();
        };
        let Ok(location) = js_sys::Reflect::get(&window, &wasm_bindgen::JsValue::from_str("location"))
        else {
            return String::new();
        };
        let Ok(origin) = js_sys::Reflect::get(&location, &wasm_bindgen::JsValue::from_str("origin"))
        else {
            return String::new();
        };
        origin.as_string().unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        String::new()
    }
}

/// Fire-and-forget clipboard write for the authorization link — mirrors
/// `bot_roster/npub_row.rs`'s `write_npub_to_clipboard` (no toast/modal
/// feedback beyond the label flip the copy button itself provides).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn write_auth_url_to_clipboard(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let _ = window.navigator().clipboard().write_text(text);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = text;
    }
}

/// One MCP server row: name, transport, honest status pill, CONNECT (OAuth
/// servers only), enable/disable toggle, REMOVE.
#[component]
fn McpServerRowView(
    row: McpServerRow,
    scope: ConfigScope,
    writable: bool,
    mut refresh_tick: Signal<u32>,
) -> Element {
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let mut confirm_remove_open: Signal<bool> = use_signal(|| false);
    let mut connecting: Signal<bool> = use_signal(|| false);
    let mut poll_status: Signal<Option<McpServerStatus>> = use_signal(|| None);
    let mut copied: Signal<bool> = use_signal(|| false);

    let name = row.name.clone();
    let transport_label = mcp_transport_label(&row.transport);
    let enabled = row.enabled;
    let has_oauth = row.draft.oauth_provider.is_some();

    // While a CONNECT attempt is in flight and no poll result has resolved
    // yet, show the dimmed "···" pending pill rather than the stale
    // pre-attempt status.
    let showing_pending = *connecting.read() && poll_status.read().is_none();
    let effective_status = poll_status
        .read()
        .clone()
        .unwrap_or_else(|| row.status.clone());
    let status_label = mcp_status_label(&effective_status);
    let status_class = mcp_status_pill_class(&effective_status);
    let status_title = mcp_status_title(&effective_status);
    let awaiting_auth_url = match &effective_status {
        McpServerStatus::AwaitingAuthorization { auth_url } => Some(auth_url.clone()),
        _ => None,
    };

    rsx! {
        div { class: "mcp-server-row",
            div { class: "mcp-server-name", title: "{name}", "{name}" }
            div { class: "mcp-server-transport", "{transport_label}" }
            if showing_pending {
                span { class: "pill mcp-status-pending", "···" }
            } else {
                span { class: "{status_class}", title: "{status_title}", "{status_label}" }
            }
            div { class: "mcp-server-actions",
                if has_oauth {
                    button {
                        class: "btn btn--ghost btn--sm",
                        "aria-label": "Connect {name}",
                        disabled: !writable || *connecting.read(),
                        onclick: {
                            let scope_for_connect = scope.clone();
                            let name_for_connect = name.clone();
                            move |_| {
                                if !writable || *connecting.read() {
                                    return;
                                }
                                connecting.set(true);
                                poll_status.set(None);
                                save_error.set(None);
                                copied.set(false);
                                let scope_value = scope_for_connect.clone();
                                let name_value = name_for_connect.clone();
                                let mut connecting_sig = connecting;
                                let mut poll_status_sig = poll_status;
                                let mut save_error_sig = save_error;
                                let mut refresh_tick_sig = refresh_tick;
                                spawn(async move {
                                    let origin = browser_origin();
                                    let started = crate::server::mcp_admin_api::connect_mcp_oauth(
                                        scope_value.clone(),
                                        name_value.clone(),
                                        origin,
                                    )
                                        .await;
                                    if let Err(e) = started {
                                        connecting_sig.set(false);
                                        save_error_sig.set(Some(e.to_string()));
                                        return;
                                    }
                                    // Bounded client-side poll (Phase 48.2
                                    // Plan 09): 1s cadence for the first ten
                                    // attempts, then 2s, with a hard ~320s
                                    // wall-time ceiling — comfortably under
                                    // the attempt map's 360s self-heal and
                                    // giving the operator the full window of
                                    // 48.2-08's 300s pending-authorization
                                    // TTL to complete the browser round trip.
                                    // Stops the instant a terminal status
                                    // (Connected/SpawnFailed/Disabled) is
                                    // read back; AwaitingAuthorization and
                                    // Connecting keep the poll running.
                                    const CEILING: std::time::Duration =
                                        std::time::Duration::from_secs(320);
                                    // `web_time::Instant`, NOT `std::time::Instant`: this loop
                                    // runs on the wasm client, where `std::time::Instant::now()`
                                    // panics with "time not implemented on this platform" and
                                    // kills the whole app. Same UAT-4 hazard already documented
                                    // on this crate's `web-time` dependency and worked around in
                                    // `screens/agents.rs`. `web_time::Instant` is a drop-in
                                    // (`std::time::Instant` on native, `Performance.now()` shim
                                    // on wasm32), so `.elapsed()` against `CEILING` is unchanged.
                                    let poll_start = web_time::Instant::now();
                                    let mut attempt: u32 = 0;
                                    loop {
                                        if poll_start.elapsed() >= CEILING {
                                            break;
                                        }
                                        let delay_ms = if attempt < 10 { 1000 } else { 2000 };
                                        crate::platform::timer::sleep(delay_ms).await;
                                        attempt += 1;
                                        match crate::server::mcp_admin_api::mcp_server_status(
                                            scope_value.clone(),
                                            name_value.clone(),
                                        )
                                            .await
                                        {
                                            Ok(status) => {
                                                let terminal = matches!(
                                                    status,
                                                    McpServerStatus::Connected
                                                        | McpServerStatus::SpawnFailed { .. }
                                                        | McpServerStatus::Disabled
                                                );
                                                poll_status_sig.set(Some(status));
                                                if terminal {
                                                    break;
                                                }
                                            }
                                            Err(_e) => break,
                                        }
                                        if poll_start.elapsed() >= CEILING {
                                            break;
                                        }
                                    }
                                    connecting_sig.set(false);
                                    let cur = *refresh_tick_sig.read();
                                    refresh_tick_sig.set(cur + 1);
                                });
                            }
                        },
                        "CONNECT"
                    }
                }
                div {
                    class: if enabled { "tgl on" } else { "tgl" },
                    class: if !writable { "tgl--disabled" },
                    "aria-label": "Toggle {name} enabled",
                    "aria-disabled": if !writable { "true" },
                    onclick: {
                        let scope_for_toggle = scope.clone();
                        let name_for_toggle = name.clone();
                        move |_| {
                            if !writable {
                                return;
                            }
                            let next_enabled = !enabled;
                            let scope_value = scope_for_toggle.clone();
                            let name_value = name_for_toggle.clone();
                            let mut save_error_sig = save_error;
                            let mut refresh_tick_sig = refresh_tick;
                            spawn(async move {
                                match crate::server::mcp_admin_api::set_mcp_server_enabled(
                                    scope_value,
                                    name_value,
                                    next_enabled,
                                )
                                    .await
                                {
                                    Ok(_row) => {
                                        save_error_sig.set(None);
                                        let cur = *refresh_tick_sig.read();
                                        refresh_tick_sig.set(cur + 1);
                                    }
                                    Err(e) => {
                                        save_error_sig.set(Some(e.to_string()));
                                    }
                                }
                            });
                        }
                    },
                }
                button {
                    class: "btn btn--danger btn--sm",
                    "aria-label": "Remove {name}",
                    disabled: !writable,
                    onclick: move |_| {
                        if !writable {
                            return;
                        }
                        confirm_remove_open.set(true);
                    },
                    "REMOVE"
                }
            }
            if let Some(auth_url) = awaiting_auth_url.clone() {
                div { class: "mcp-authorize-panel",
                    div { class: "mcp-authorize-copy",
                        "Authorize in the new tab, then return here — this row updates on its own."
                    }
                    div { class: "mcp-authorize-actions",
                        a {
                            class: "btn btn--sm mcp-authorize-link",
                            href: "{auth_url}",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            "aria-label": "Open authorization link for {name}",
                            "OPEN AUTHORIZATION LINK"
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            "aria-label": "Copy authorization link for {name}",
                            onclick: {
                                let url_for_copy = auth_url.clone();
                                move |_| {
                                    write_auth_url_to_clipboard(&url_for_copy);
                                    copied.set(true);
                                }
                            },
                            if *copied.read() { "COPIED" } else { "COPY LINK" }
                        }
                    }
                    div { class: "mcp-authorize-fallback-note",
                        "If the tab was blocked by your browser's pop-up blocker, use the copy control and paste the link into a new tab yourself."
                    }
                }
            }
            if let Some(err) = save_error() {
                span { class: "pill red tools-save-failed-chip", "SAVE FAILED — {err}." }
            }
            if *confirm_remove_open.read() {
                RemoveServerConfirm {
                    server_name: name.clone(),
                    scope: scope.clone(),
                    refresh_tick,
                    on_close: move |_| confirm_remove_open.set(false),
                }
            }
        }
    }
}

/// D-14 destructive confirmation for REMOVE — exact copy, never
/// paraphrased.
#[component]
fn RemoveServerConfirm(
    server_name: String,
    scope: ConfigScope,
    mut refresh_tick: Signal<u32>,
    on_close: EventHandler<()>,
) -> Element {
    let mut removing: Signal<bool> = use_signal(|| false);
    let error: Signal<Option<String>> = use_signal(|| None);
    let name_for_title = server_name.clone();
    let error_val = error();

    rsx! {
        div { class: "mcp-confirm-overlay", role: "presentation",
            div {
                class: "mcp-confirm-dialog",
                role: "dialog",
                aria_modal: "true",
                "aria-labelledby": "mcp-confirm-title",
                div { class: "mcp-confirm-title", id: "mcp-confirm-title", "REMOVE {name_for_title}?" }
                div { class: "mcp-confirm-body",
                    "This disconnects it immediately and deletes its config.yaml entry. This cannot be undone from this page."
                }
                if let Some(err) = error_val {
                    span { class: "pill red tools-save-failed-chip", "SAVE FAILED — {err}." }
                }
                div { class: "mcp-confirm-actions",
                    button {
                        class: "btn btn--ghost btn--sm",
                        disabled: *removing.read(),
                        onclick: move |_| on_close.call(()),
                        "CANCEL"
                    }
                    button {
                        class: "btn btn--danger btn--sm",
                        "aria-label": "Confirm remove {name_for_title}",
                        disabled: *removing.read(),
                        onclick: {
                            let scope_for_remove = scope.clone();
                            let name_for_remove = server_name.clone();
                            move |_| {
                                removing.set(true);
                                let scope_value = scope_for_remove.clone();
                                let name_value = name_for_remove.clone();
                                let mut removing_sig = removing;
                                let mut error_sig = error;
                                let mut refresh_tick_sig = refresh_tick;
                                let on_close_handler = on_close;
                                spawn(async move {
                                    match crate::server::mcp_admin_api::remove_mcp_server(
                                        scope_value,
                                        name_value,
                                    )
                                        .await
                                    {
                                        Ok(()) => {
                                            let cur = *refresh_tick_sig.read();
                                            refresh_tick_sig.set(cur + 1);
                                            on_close_handler.call(());
                                        }
                                        Err(e) => {
                                            removing_sig.set(false);
                                            error_sig.set(Some(e.to_string()));
                                        }
                                    }
                                });
                            }
                        },
                        if *removing.read() { "REMOVING…" } else { "REMOVE" }
                    }
                }
            }
        }
    }
}

// =============================================================================
// Pure-fn tests
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_pill_class_covers_all_four_statuses() {
        assert_eq!(mcp_status_pill_class(&McpServerStatus::Connected), "pill teal");
        assert_eq!(mcp_status_pill_class(&McpServerStatus::AuthRequired), "pill amber");
        assert_eq!(
            mcp_status_pill_class(&McpServerStatus::SpawnFailed { reason: "boom".to_string() }),
            "pill red"
        );
        assert_eq!(mcp_status_pill_class(&McpServerStatus::Disabled), "pill");
    }

    #[test]
    fn spawn_failed_never_maps_to_the_disabled_class() {
        let spawn_failed = mcp_status_pill_class(&McpServerStatus::SpawnFailed {
            reason: "connection refused".to_string(),
        });
        let disabled = mcp_status_pill_class(&McpServerStatus::Disabled);
        assert_ne!(
            spawn_failed, disabled,
            "a real handshake failure must never render as the mere-disabled treatment"
        );
    }

    #[test]
    fn status_labels_are_the_four_locked_strings() {
        assert_eq!(mcp_status_label(&McpServerStatus::Connected), "CONNECTED");
        assert_eq!(mcp_status_label(&McpServerStatus::AuthRequired), "AUTH REQUIRED");
        assert_eq!(
            mcp_status_label(&McpServerStatus::SpawnFailed { reason: "x".to_string() }),
            "SPAWN FAILED"
        );
        assert_eq!(mcp_status_label(&McpServerStatus::Disabled), "DISABLED");
    }

    #[test]
    fn spawn_failed_title_carries_the_real_reason_never_discarded() {
        let title = mcp_status_title(&McpServerStatus::SpawnFailed {
            reason: "connection refused on port 9".to_string(),
        });
        assert_eq!(title, "connection refused on port 9");
    }
}
