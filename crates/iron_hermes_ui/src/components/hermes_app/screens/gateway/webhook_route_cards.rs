//! Webhook-route card subset (E4, D-04) — one grid card per configured
//! webhook route, carrying the route's HTTP path to the agent. Reads
//! `webhook_route_api.rs`'s live `Vec<WebhookRoute>` CRUD surface.
//!
//! `refresh_tick` is `Signal<u32>` (mutable, not `ReadSignal`) — the
//! established contract for a child that will WRITE (route add/edit/
//! remove) starting Plan 04; `mod.rs`'s call site and this signature must
//! not change when Plan 04 fills the body.
//!
//! # CONFIGURE reuses `webhook_wizard::RouteEditorModal` (D-03)
//!
//! CONFIGURE does not open the shared `+ ADD PLATFORM` modal (that modal's
//! `open: Signal<bool>` is wired exclusively to `mod.rs`'s header button and
//! is not threaded down to this sibling component — this plan does not
//! touch `mod.rs`). Instead, CONFIGURE mounts its OWN instance of
//! [`super::webhook_wizard::RouteEditorModal`] — the SAME form component
//! `AddRouteWizard`'s editor step uses — with `is_new: false` and the
//! clicked route as `initial`. "Existing routes open in the same form"
//! (D-03) is satisfied by sharing this one component across both call
//! sites.
//!
//! # E4 pagination — FLAGGED ASSUMPTION, decided here
//!
//! No max-route-count/pagination decision exists anywhere upstream
//! (CONTEXT.md and D-04 are silent — `49.3-UI-SPEC.md`'s E4 "populated" row
//! is explicitly `⚠ unresolved — planner must treat as assumption`). This
//! plan DECIDES: no pagination this phase — the grid auto-wraps
//! (`.grid.wide`'s existing `repeat(auto-fill, minmax(340px,1fr))`), and a
//! search/pagination affordance is deferred until route counts are
//! observed to be unwieldy in practice. Recorded in the Plan 04 SUMMARY per
//! the plan's own `<flagged_assumptions>` instruction.
//!
//! # SMS is a standard route card, inbound-only (D-01/D-02, Task 3)
//!
//! The Twilio SMS worked example (`AddRouteWizard`'s `TWILIO SMS` preset
//! tile, `webhook_route_api::twilio_sms_preset`) is surfaced through the
//! SAME `WebhookRouteCard` every other route uses — there is no
//! SMS-specific branch anywhere in this file. That is deliberate: an
//! SMS-specific card would be the natural place to add a reply-by-SMS
//! control, and D-02 explicitly defers two-way/outbound SMS. Rendering it
//! through the generic path means there is structurally nowhere for such a
//! control to be added without a person consciously special-casing this
//! route — the card only ever shows CONFIGURE/REMOVE, identical to every
//! other webhook route.

use crate::server::tools_config_api::ConfigScope;
use crate::server::webhook_route_api::{self, WebhookRouteView};
use dioxus::prelude::*;

use super::webhook_wizard::RouteEditorModal;

// `WebhookRouteView::signature`/`deliver` are already plain, human-readable
// `snake_case` strings (`"generic_v2"`, `"url"`, ...) — no enum <-> label
// mapping is needed here; `ironhermes_core::webhook_route`'s typed enums
// are native-only (`webhook_route_api.rs`'s module doc) and unavailable on
// this file's wasm compile target anyway.

#[component]
pub fn WebhookRouteCards(scope: ReadSignal<ConfigScope>, refresh_tick: Signal<u32>) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let routes_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move { webhook_route_api::list_webhook_routes(scope_value).await }
    });
    // E4 empty/error: an in-flight or failed fetch renders zero extra
    // cards (covered by the parent grid's own skeleton/fallback, E4
    // loading/error rows) — never a broken/blank card of its own.
    let routes: Vec<WebhookRouteView> = match routes_resource() {
        Some(Ok(list)) => list,
        _ => Vec::new(),
    };

    // CONFIGURE target — `None` means no editor is open. Local to this
    // component (see module doc's "CONFIGURE reuses RouteEditorModal"
    // section).
    let mut editing_route: Signal<Option<WebhookRouteView>> = use_signal(|| None);
    // REMOVE ROUTE confirm — the route NAME pending destructive
    // confirmation, or `None`.
    let confirm_remove: Signal<Option<String>> = use_signal(|| None);
    let remove_error: Signal<Option<String>> = use_signal(|| None);

    let editing_val = editing_route.read().clone();
    let confirm_remove_val = confirm_remove.read().clone();

    rsx! {
        for route in routes.iter() {
            WebhookRouteCard {
                key: "{route.name}",
                route: route.clone(),
                editing_route,
                confirm_remove,
            }
        }
        if let Some(name) = confirm_remove_val {
            RemoveRouteConfirm {
                name,
                scope,
                refresh_tick,
                confirm_remove,
                remove_error,
            }
        }
        if let Some(route) = editing_val {
            RouteEditorModal {
                initial: route,
                is_new: false,
                scope,
                refresh_tick,
                on_close: move |_| editing_route.set(None),
            }
        }
    }
}

/// One route's grid card. `.plat-name`/path subtitle each carry a native
/// `title` attribute holding the untruncated value plus an inline
/// ellipsis-overflow style — the card never grows or wraps (E4
/// overflow/long-text). Reuses `.plat-card`/`.plat-head`/`.plat-glyph`/
/// `.plat-name`/`.plat-state`/`dl.kv` unmodified (D-10 — no new CSS class
/// needed for this card; it renders WITHOUT the `connected` modifier since
/// a configured webhook route has no chat-count/connection concept, the
/// same as the stub's disconnected pattern).
#[component]
fn WebhookRouteCard(
    route: WebhookRouteView,
    mut editing_route: Signal<Option<WebhookRouteView>>,
    mut confirm_remove: Signal<Option<String>>,
) -> Element {
    const TRUNCATE_STYLE: &str =
        "overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:100%;";
    let route_for_configure = route.clone();
    let route_name_for_remove = route.name.clone();

    rsx! {
        div { class: "plat-card",
            div { class: "plat-head",
                div { class: "plat-glyph", "▦" }
                div { style: "flex:1;min-width:0;",
                    div {
                        class: "plat-name",
                        style: "{TRUNCATE_STYLE}",
                        title: "{route.name}",
                        "{route.name}"
                    }
                    div {
                        class: "plat-state",
                        style: "{TRUNCATE_STYLE}",
                        title: "{route.path}",
                        "{route.path}"
                    }
                }
            }
            dl { class: "kv",
                dt { "Signature" }
                dd { "{route.signature}" }
                dt { "Deliver" }
                dd { "{route.deliver}" }
            }
            div { style: "display:flex;gap:8px;",
                button {
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| editing_route.set(Some(route_for_configure.clone())),
                    "CONFIGURE →"
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| confirm_remove.set(Some(route_name_for_remove.clone())),
                    "REMOVE"
                }
            }
        }
    }
}

/// REMOVE ROUTE destructive confirm — Copywriting Contract's exact copy,
/// naming the route and its path.
#[component]
fn RemoveRouteConfirm(
    name: String,
    scope: ReadSignal<ConfigScope>,
    mut refresh_tick: Signal<u32>,
    mut confirm_remove: Signal<Option<String>>,
    mut remove_error: Signal<Option<String>>,
) -> Element {
    let removing: Signal<bool> = use_signal(|| false);
    let error_val = remove_error.read().clone();

    rsx! {
        div { class: "mcp-wizard-overlay", role: "presentation",
            div { class: "mcp-wizard", role: "dialog", aria_modal: "true",
                div { class: "mcp-wizard-header",
                    h3 { class: "mcp-wizard-title", "REMOVE ROUTE" }
                }
                div { class: "mcp-wizard-body",
                    p {
                        "This deletes \"{name}\"'s config entry. Any integration still POSTing to this path will get 404s after the next restart. This can't be undone."
                    }
                    if let Some(err) = error_val {
                        div { class: "mcp-wizard-probe-error", "SAVE FAILED — {err}. Check your connection and retry." }
                    }
                }
                div { class: "mcp-wizard-footer",
                    button {
                        class: "btn btn--ghost btn--sm",
                        disabled: *removing.read(),
                        onclick: move |_| confirm_remove.set(None),
                        "CANCEL"
                    }
                    button {
                        class: "btn",
                        disabled: *removing.read(),
                        onclick: move |_| {
                            let scope_value = scope();
                            let name_value = name.clone();
                            let mut removing_sig = removing;
                            let mut remove_error_sig = remove_error;
                            let mut confirm_remove_sig = confirm_remove;
                            let mut refresh_tick_sig = refresh_tick;
                            removing_sig.set(true);
                            spawn(async move {
                                let result = webhook_route_api::delete_webhook_route(scope_value, name_value).await;
                                removing_sig.set(false);
                                match result {
                                    Ok(()) => {
                                        remove_error_sig.set(None);
                                        confirm_remove_sig.set(None);
                                        let cur = *refresh_tick_sig.read();
                                        refresh_tick_sig.set(cur + 1);
                                    }
                                    Err(e) => remove_error_sig.set(Some(e.to_string())),
                                }
                            });
                        },
                        if *removing.read() { "REMOVING…" } else { "REMOVE ROUTE" }
                    }
                }
            }
        }
    }
}

