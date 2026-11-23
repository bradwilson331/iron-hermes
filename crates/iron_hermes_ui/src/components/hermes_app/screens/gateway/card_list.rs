//! Reorderable Gateway card list (Phase 54.10 Plan 07, D-18).
//!
//! Every top-level Gateway card is one keyed entry in a single flat list rendered in the saved
//! order, so DOM order and Tab order equal visual order (WCAG 2.4.3; operator resolution 4: a
//! flat keyed list, not CSS `order`). The order is persisted per profile by
//! `dashboard_api::{get,set}_gateway_card_order`. Position 1 is the full-width banner
//! (`gw-card-slot--banner`), applied by position.
//!
//! Reorder is by keyboard on the `≡` handle (Space picks up, arrows move, Space/Enter drops,
//! Esc cancels) or by pointer drag on the handle with pointer capture. The pure parts
//! (`default_card_ids`, `apply_saved_order`, `keyboard_reorder`, `pointer_drop_slot`) carry the
//! logic and the unit tests; the components only wire events to them.
//!
//! Shortcut: pointer drop slots are measured once at pickup, so scrolling the screen mid-drag
//! skews the insertion bar; and a picked card is not auto-cancelled on blur (Esc cancels).

use std::collections::HashMap;
use std::rc::Rc;

use dioxus::prelude::*;

use super::api_server_card::ApiServerCard;
use super::chat_config_form::ChatConfigForm;
use super::chat_platform_cards::{
    BuzzCard, ChatPlatformKind, DiscordCard, SlackCard, TelegramCard,
};
use super::schedules_card::GatewaySchedulesCard;
use super::teaser_cards::TeaserCard;
use super::webhook_route_cards::{RemoveRouteConfirm, WebhookRouteCard};
use super::webhook_wizard::RouteEditorModal;
use crate::canvas_math::{drop_index, reorder};
use crate::platform::pointer::{capture_pointer, release_pointer};
use crate::server::dashboard_api::{
    GATEWAY_ORDER_MAX, get_gateway_card_order, set_gateway_card_order,
};
use crate::server::gateway_platform_status_api::PlatformStatusMap;
use crate::server::tools_config_api::ConfigScope;
use crate::server::webhook_route_api::{self, WebhookRouteView};

/// UI-SPEC E13: shown under the screen header when the order could not be saved.
pub(crate) const SAVE_FAILED_LINE: &str =
    "Save failed. Check server logs. The new order is active until you reload.";

// ---------------------------------------------------------------------------------------------
// Pure order logic
// ---------------------------------------------------------------------------------------------

/// Today's DOM order: Schedules, the four chat platforms, routes in API order, the API server,
/// then the teasers.
pub(crate) fn default_card_ids(route_names: &[String]) -> Vec<String> {
    let mut ids: Vec<String> = ["schedules", "platform:telegram", "platform:discord"]
        .into_iter()
        .chain(["platform:slack", "platform:buzz"])
        .map(String::from)
        .collect();
    ids.extend(route_names.iter().map(|n| format!("route:{n}")));
    ids.extend(["api_server", "teaser:email", "teaser:voice"].map(String::from));
    ids
}

/// Human title used in aria labels and announcements.
pub(crate) fn card_title(id: &str) -> String {
    match id {
        "schedules" => "Schedules".to_string(),
        "api_server" => "API server".to_string(),
        "platform:telegram" => "Telegram".to_string(),
        "platform:discord" => "Discord".to_string(),
        "platform:slack" => "Slack".to_string(),
        "platform:buzz" => "Buzz".to_string(),
        "teaser:email" => "Email".to_string(),
        "teaser:voice" => "Voice".to_string(),
        other => other.strip_prefix("route:").unwrap_or(other).to_string(),
    }
}

/// Saved ids that still exist (first occurrence wins), then the cards the saved list does not
/// know, in default order.
pub(crate) fn apply_saved_order(default: &[String], saved: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(default.len());
    for id in saved.iter().filter(|s| default.contains(s)) {
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    let rest: Vec<String> = default
        .iter()
        .filter(|d| !out.contains(d))
        .cloned()
        .collect();
    out.extend(rest);
    out
}

/// The order to persist after the operator reordered the cards currently shown. Saved ids that
/// are not shown right now (routes not loaded yet, a failed route fetch, the other config scope)
/// stay, each right after the id that preceded it in the previous order, so no route position is
/// lost. If that would pass the server cap, the hidden ids at the tail go first.
pub(crate) fn merge_saved_order(prev_saved: &[String], visible: &[String]) -> Vec<String> {
    let mut out: Vec<String> = visible.to_vec();
    let mut hidden: Vec<String> = Vec::new();
    for (i, id) in prev_saved.iter().enumerate() {
        if visible.contains(id) || hidden.contains(id) {
            continue;
        }
        let at = prev_saved[..i]
            .iter()
            .rev()
            .find_map(|p| out.iter().position(|o| o == p))
            .map_or(0, |pos| pos + 1);
        out.insert(at, id.clone());
        hidden.push(id.clone());
    }
    while out.len() > GATEWAY_ORDER_MAX {
        let Some(last) = hidden.pop() else { break };
        out.retain(|o| *o != last);
    }
    out
}

pub(crate) fn picked_text(title: &str, pos: usize, n: usize) -> String {
    format!("{title} picked up, position {pos} of {n}.")
}
pub(crate) fn moved_text(title: &str, pos: usize, n: usize) -> String {
    format!("{title} moved to position {pos} of {n}.")
}
pub(crate) fn dropped_text(title: &str, pos: usize, n: usize) -> String {
    format!("{title} dropped at position {pos} of {n}.")
}
pub(crate) const CANCELLED_TEXT: &str = "Move cancelled.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReorderKey {
    /// Space: picks up, or drops when already picked up.
    Pick,
    Earlier,
    Later,
    Drop,
    Cancel,
}

/// A card held by the keyboard; `original` is restored on cancel.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pickup {
    pub id: String,
    pub original: Vec<String>,
}

/// Result of one key press. An empty `announcement` means the key was not handled.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReorderStep {
    pub order: Vec<String>,
    pub pickup: Option<Pickup>,
    pub announcement: String,
    /// The order changed and was dropped: persist it.
    pub commit: bool,
}

pub(crate) fn keyboard_reorder(
    current: &[String],
    pickup: Option<&Pickup>,
    id: &str,
    key: ReorderKey,
) -> ReorderStep {
    let noop = |pickup: Option<&Pickup>| ReorderStep {
        order: current.to_vec(),
        pickup: pickup.cloned(),
        announcement: String::new(),
        commit: false,
    };
    let n = current.len();
    let Some(idx) = current.iter().position(|c| c == id) else {
        return noop(pickup);
    };
    let title = card_title(id);
    match (pickup, key) {
        (None, ReorderKey::Pick) => ReorderStep {
            order: current.to_vec(),
            pickup: Some(Pickup {
                id: id.to_string(),
                original: current.to_vec(),
            }),
            announcement: picked_text(&title, idx + 1, n),
            commit: false,
        },
        (Some(p), ReorderKey::Earlier | ReorderKey::Later) if p.id == id => {
            let to = if key == ReorderKey::Earlier {
                idx.saturating_sub(1)
            } else {
                (idx + 1).min(n - 1)
            };
            ReorderStep {
                order: reorder(current, idx, to),
                pickup: Some(p.clone()),
                announcement: moved_text(&title, to + 1, n),
                commit: false,
            }
        }
        (Some(p), ReorderKey::Pick | ReorderKey::Drop) if p.id == id => ReorderStep {
            order: current.to_vec(),
            pickup: None,
            announcement: dropped_text(&title, idx + 1, n),
            commit: current != p.original.as_slice(),
        },
        (Some(p), ReorderKey::Cancel) if p.id == id => ReorderStep {
            order: p.original.clone(),
            pickup: None,
            announcement: CANCELLED_TEXT.to_string(),
            commit: false,
        },
        _ => noop(pickup),
    }
}

/// A card's client rectangle: left, top, width, height.
pub(crate) type CardRect = (f64, f64, f64, f64);

/// Insertion slot (0..=len) for a pointer at `(px, py)` over cards laid out in a wrapping grid.
/// Cards sharing a top form a row. In a row of several cards the pointer's x decides; a row of
/// one card (a single column, or the banner) uses the vertical midpoint. The result feeds
/// [`drop_index`] through one scalar per card, row-major.
pub(crate) fn pointer_drop_slot(rects: &[CardRect], px: f64, py: f64) -> usize {
    let mut tops: Vec<f64> = rects.iter().map(|r| r.1).collect();
    tops.sort_by(f64::total_cmp);
    tops.dedup_by(|a, b| (*a - *b).abs() < 2.0);
    let row_of = |top: f64| tops.iter().filter(|t| **t < top - 2.0).count();
    let mut bottoms = vec![f64::MIN; tops.len()];
    let mut lens = vec![0_usize; tops.len()];
    for r in rects {
        let row = row_of(r.1);
        bottoms[row] = bottoms[row].max(r.1 + r.3);
        lens[row] += 1;
    }
    const ROW: f64 = 1e6;
    const HALF: f64 = 5e4;
    let keys: Vec<f64> = rects
        .iter()
        .map(|r| {
            let row = row_of(r.1);
            let within = if lens[row] > 1 { r.0 + r.2 / 2.0 } else { HALF };
            row as f64 * ROW + within
        })
        .collect();
    let pointer_row = bottoms.iter().filter(|b| **b < py).count();
    let within = match (lens.get(pointer_row), tops.get(pointer_row)) {
        (Some(len), _) if *len > 1 => px,
        (Some(_), Some(top)) => {
            let h = (bottoms[pointer_row] - top).max(1.0);
            (py - top).clamp(0.0, h) / h * 2.0 * HALF
        }
        _ => 0.0,
    };
    drop_index(&keys, pointer_row as f64 * ROW + within)
}

/// Final index of the card moved from `from` into insertion slot `slot` (see [`drop_index`]).
fn slot_to_index(from: usize, slot: usize) -> usize {
    if slot > from {
        slot - 1
    } else {
        slot
    }
}

// ---------------------------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------------------------

/// `Reset card order` (UI-SPEC C): absent at the default order.
#[component]
pub(crate) fn ResetOrderButton(visible: bool, on_reset: EventHandler<()>) -> Element {
    if !visible {
        return rsx! {};
    }
    rsx! {
        button {
            class: "btn btn--ghost btn--sm",
            onclick: move |_| on_reset.call(()),
            "Reset card order"
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct PointerDrag {
    id: String,
    from: usize,
    rects: Vec<CardRect>,
    slot: usize,
}

/// The flat list of slots with the reorder interaction. `cards` renders one card by id.
#[component]
pub(crate) fn CardSlots(
    ids: Vec<String>,
    on_commit: EventHandler<Vec<String>>,
    cards: Callback<String, Element>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let mut pickup: Signal<Option<Pickup>> = use_signal(|| None);
    // The order shown while a card is held by the keyboard; `None` shows `ids`.
    let mut working: Signal<Option<Vec<String>>> = use_signal(|| None);
    let mut announce: Signal<String> = use_signal(String::new);
    let mut drag: Signal<Option<PointerDrag>> = use_signal(|| None);
    let mut handles: Signal<HashMap<String, Rc<MountedData>>> = use_signal(HashMap::new);
    let mut slots: Signal<HashMap<String, Rc<MountedData>>> = use_signal(HashMap::new);

    let shown: Vec<String> = working.read().clone().unwrap_or_else(|| ids.clone());
    let n = shown.len();
    let held = pickup.read().as_ref().map(|p| p.id.clone());
    let dragging = drag.read().clone();

    rsx! {
        div { class: "gw-live", role: "status", "aria-live": "polite", "{announce}" }
        for (i , id) in shown.iter().enumerate() {
            {
                let id = id.clone();
                let title = card_title(&id);
                let is_held = held.as_deref() == Some(id.as_str()) || dragging.as_ref().is_some_and(|d| d.id == id);
                let drop_class = match &dragging {
                    Some(d) if d.slot == i && d.slot != d.from && d.slot != d.from + 1 => " gw-card-slot--drop-before",
                    Some(d) if d.slot == n && i == n - 1 && d.from != n - 1 => " gw-card-slot--drop-after",
                    _ => "",
                };
                let class = format!(
                    "gw-card-slot{}{}{}",
                    if i == 0 { " gw-card-slot--banner" } else { "" },
                    if is_held { " gw-card-slot--held" } else { "" },
                    drop_class,
                );
                let label = format!(
                    "Move {title}. Press Space to pick up, arrow keys to move, Space to drop, Escape to cancel.",
                );
                let (ids_key, ids_down, ids_up) = (ids.clone(), ids.clone(), ids.clone());
                let (id_slot, id_handle, id_key, id_down, id_move, id_up) = (
                    id.clone(),
                    id.clone(),
                    id.clone(),
                    id.clone(),
                    id.clone(),
                    id.clone(),
                );
                rsx! {
                    div {
                        key: "{id}",
                        class: "{class}",
                        "data-card-id": "{id}",
                        onmounted: move |e| {
                            slots.write().insert(id_slot.clone(), e.data());
                        },
                        button {
                            class: "btn btn--sm btn--icon btn--ghost gw-card-handle",
                            "aria-label": "{label}",
                            onmounted: move |e| {
                                handles.write().insert(id_handle.clone(), e.data());
                            },
                            onkeydown: move |evt| {
                                let key = evt.key();
                                if key == Key::Escape && drag.peek().is_some() {
                                    drag.set(None);
                                    announce.set(CANCELLED_TEXT.to_string());
                                    return;
                                }
                                let rk = match key {
                                    Key::Character(c) if c == " " => ReorderKey::Pick,
                                    Key::Enter => ReorderKey::Drop,
                                    Key::Escape => ReorderKey::Cancel,
                                    Key::ArrowUp | Key::ArrowLeft => ReorderKey::Earlier,
                                    Key::ArrowDown | Key::ArrowRight => ReorderKey::Later,
                                    _ => return,
                                };
                                let current = working.peek().clone().unwrap_or_else(|| ids_key.clone());
                                let step = keyboard_reorder(&current, pickup.peek().as_ref(), &id_key, rk);
                                if step.announcement.is_empty() {
                                    return;
                                }
                                evt.prevent_default();
                                working.set(step.pickup.is_some().then(|| step.order.clone()));
                                pickup.set(step.pickup);
                                announce.set(step.announcement);
                                if step.commit {
                                    on_commit.call(step.order);
                                }
                                // A keyed move can drop focus in some browsers; put it back.
                                let handle = handles.peek().get(&id_key).cloned();
                                spawn(async move {
                                    if let Some(h) = handle {
                                        let _ = h.set_focus(true).await;
                                    }
                                });
                            },
                            onpointerdown: move |evt| {
                                let current = working.peek().clone().unwrap_or_else(|| ids_down.clone());
                                let Some(from) = current.iter().position(|c| *c == id_down) else {
                                    return;
                                };
                                let handle = handles.peek().get(&id_down).cloned();
                                if let Some(h) = &handle {
                                    capture_pointer(h, evt.pointer_id());
                                }
                                let id = id_down.clone();
                                spawn(async move {
                                    let mut rects = Vec::with_capacity(current.len());
                                    for cid in &current {
                                        let el = slots.peek().get(cid).cloned();
                                        let Some(el) = el else { return };
                                        let Ok(r) = el.get_client_rect().await else { return };
                                        rects.push((r.origin.x, r.origin.y, r.size.width, r.size.height));
                                    }
                                    drag.set(Some(PointerDrag { id, from, rects, slot: from }));
                                });
                            },
                            onpointermove: move |evt| {
                                let Some(d) = drag.peek().clone() else { return };
                                if d.id != id_move {
                                    return;
                                }
                                let p = evt.client_coordinates();
                                let slot = pointer_drop_slot(&d.rects, p.x, p.y);
                                if slot != d.slot {
                                    drag.set(Some(PointerDrag { slot, ..d }));
                                }
                            },
                            onpointerup: move |evt| {
                                let Some(d) = drag.peek().clone() else { return };
                                if d.id != id_up {
                                    return;
                                }
                                drag.set(None);
                                if let Some(h) = handles.peek().get(&id_up).cloned() {
                                    release_pointer(&h, evt.pointer_id());
                                }
                                let to = slot_to_index(d.from, d.slot);
                                if to != d.from {
                                    let current = ids_up.clone();
                                    announce.set(dropped_text(&card_title(&id_up), to + 1, current.len()));
                                    on_commit.call(reorder(&current, d.from, to));
                                }
                            },
                            onpointercancel: move |_| drag.set(None),
                            onlostpointercapture: move |_| drag.set(None),
                            "≡"
                        }
                        {cards.call(id.clone())}
                    }
                }
            }
        }
    }
}

/// All Gateway cards as one reorderable list, plus the modals the cards share. Renders nothing
/// until the saved order has loaded, so there is no default-order flash (E13).
#[component]
pub(crate) fn GatewayCardList(
    scope: ReadSignal<ConfigScope>,
    refresh_tick: Signal<u32>,
    platform_status: Option<PlatformStatusMap>,
    mut order_customized: Signal<bool>,
    reset_req: Signal<u32>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let open_configure: Signal<Option<ChatPlatformKind>> = use_signal(|| None);
    let routes_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move { webhook_route_api::list_webhook_routes(scope_value).await }
    });
    let mut editing_route: Signal<Option<WebhookRouteView>> = use_signal(|| None);
    let confirm_remove: Signal<Option<String>> = use_signal(|| None);
    let remove_error: Signal<Option<String>> = use_signal(|| None);

    // Fetched once per mount: the order is independent of the config-scope selector.
    let order_resource = use_resource(|| async { get_gateway_card_order().await });
    // The order the operator last set this session; wins over the fetched one.
    let mut local_saved: Signal<Option<Vec<String>>> = use_signal(|| None);
    let mut save_failed: Signal<bool> = use_signal(|| false);

    let routes: Vec<WebhookRouteView> = match routes_resource() {
        Some(Ok(list)) => list,
        _ => Vec::new(),
    };
    let route_names: Vec<String> = routes.iter().map(|r| r.name.clone()).collect();
    let default_ids = default_card_ids(&route_names);
    // A failed read renders the default order (logged server-side).
    let fetched: Option<Vec<String>> = order_resource().map(|r| r.unwrap_or_default());
    let saved = local_saved.read().clone().or(fetched.clone());
    let ids: Option<Vec<String>> = saved.map(|s| apply_saved_order(&default_ids, &s));

    let customized = ids.as_ref().is_some_and(|ids| *ids != default_ids);
    use_effect(use_reactive!(|customized| {
        if *order_customized.peek() != customized {
            order_customized.set(customized);
        }
    }));

    let mut persist = move |order: Vec<String>| {
        local_saved.set(Some(order.clone()));
        spawn(async move {
            save_failed.set(set_gateway_card_order(order).await.is_err());
        });
    };

    // A reorder only knows the cards shown now: merge it into the saved order (C-05).
    let on_commit = move |visible: Vec<String>| {
        let prev = local_saved.peek().clone().or_else(|| fetched.clone());
        persist(merge_saved_order(&prev.unwrap_or_default(), &visible));
    };

    let mut last_reset = use_signal(|| *reset_req.peek());
    use_effect(move || {
        let n = reset_req();
        if n != *last_reset.peek() {
            last_reset.set(n);
            persist(Vec::new());
        }
    });

    let Some(ids) = ids else {
        return rsx! {};
    };

    let status = platform_status;
    let cards = Callback::new(move |id: String| -> Element {
        match id.as_str() {
            "schedules" => rsx! { GatewaySchedulesCard { scope, refresh_tick } },
            "platform:telegram" => {
                rsx! { TelegramCard { scope, refresh_tick, platform_status: status.clone(), open_configure } }
            }
            "platform:discord" => {
                rsx! { DiscordCard { scope, refresh_tick, platform_status: status.clone(), open_configure } }
            }
            "platform:slack" => {
                rsx! { SlackCard { scope, refresh_tick, platform_status: status.clone(), open_configure } }
            }
            "platform:buzz" => {
                rsx! { BuzzCard { scope, refresh_tick, platform_status: status.clone(), open_configure } }
            }
            "api_server" => rsx! { ApiServerCard { scope, refresh_tick } },
            "teaser:email" => rsx! { TeaserCard { name: "Email" } },
            "teaser:voice" => rsx! { TeaserCard { name: "Voice" } },
            other => match other
                .strip_prefix("route:")
                .and_then(|n| routes.iter().find(|r| r.name == n))
            {
                Some(route) => rsx! {
                    WebhookRouteCard { route: route.clone(), editing_route, confirm_remove }
                },
                None => rsx! {},
            },
        }
    });

    let editing_val = editing_route.read().clone();
    let confirm_remove_val = confirm_remove.read().clone();

    rsx! {
        if *save_failed.read() {
            div { class: "pill red gw-save-failed", role: "alert", "{SAVE_FAILED_LINE}" }
        }
        div { class: "grid wide",
            CardSlots { ids, on_commit, cards }
        }
        if let Some(kind) = *open_configure.read() {
            ChatConfigForm {
                kind,
                scope,
                refresh_tick,
                on_close: {
                    let mut open_configure_sig = open_configure;
                    move |_: ()| open_configure_sig.set(None)
                },
            }
        }
        if let Some(name) = confirm_remove_val {
            RemoveRouteConfirm { name, scope, refresh_tick, confirm_remove, remove_error }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn v(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn apply_saved_order_drops_unknown_and_appends_new() {
        let default = v(&["schedules", "platform:telegram", "route:a", "api_server"]);
        let saved = v(&["api_server", "route:gone", "schedules"]);
        assert_eq!(
            apply_saved_order(&default, &saved),
            v(&["api_server", "schedules", "platform:telegram", "route:a"])
        );
        assert_eq!(apply_saved_order(&default, &[]), default);
    }

    #[test]
    fn merge_keeps_saved_route_positions_that_are_not_shown() {
        let prev = v(&[
            "api_server",
            "route:a",
            "schedules",
            "route:b",
            "platform:slack",
        ]);
        // Routes not loaded: only the fixed cards are shown, and the operator swaps two. Each
        // hidden route stays right after the card that preceded it.
        let visible = v(&["schedules", "api_server", "platform:slack"]);
        assert_eq!(
            merge_saved_order(&prev, &visible),
            v(&[
                "schedules",
                "route:b",
                "api_server",
                "route:a",
                "platform:slack"
            ])
        );
        // Nothing hidden: the new order is the order.
        assert_eq!(merge_saved_order(&prev, &prev), prev);
        assert_eq!(merge_saved_order(&[], &visible), visible);
        // A hidden id that led the old order keeps the front.
        assert_eq!(
            merge_saved_order(&v(&["route:a", "schedules"]), &v(&["schedules"])),
            v(&["route:a", "schedules"])
        );
    }

    #[test]
    fn merge_never_exceeds_the_server_cap() {
        let prev: Vec<String> = (0..GATEWAY_ORDER_MAX)
            .map(|i| format!("route:{i}"))
            .collect();
        let visible = v(&["schedules", "api_server"]);
        let merged = merge_saved_order(&prev, &visible);
        assert_eq!(merged.len(), GATEWAY_ORDER_MAX);
        assert!(visible.iter().all(|id| merged.contains(id)));
    }

    #[test]
    fn keyboard_reorder_moves_and_cancels() {
        let order = v(&["a", "b", "c", "d"])
            .into_iter()
            .map(|s| format!("route:{s}"))
            .collect::<Vec<_>>();
        let id = "route:b";
        let picked = keyboard_reorder(&order, None, id, ReorderKey::Pick);
        assert_eq!(picked.announcement, "b picked up, position 2 of 4.");
        let mut cur = picked.order.clone();
        let mut pickup = picked.pickup.clone();
        for _ in 0..2 {
            let s = keyboard_reorder(&cur, pickup.as_ref(), id, ReorderKey::Later);
            cur = s.order;
            pickup = s.pickup;
            assert!(!s.commit);
        }
        assert_eq!(cur.iter().position(|c| c == id), Some(3));
        let moved = keyboard_reorder(&cur, pickup.as_ref(), id, ReorderKey::Later);
        assert_eq!(moved.announcement, "b moved to position 4 of 4.");
        assert_eq!(moved.order, cur, "already last: unchanged");
        let cancelled = keyboard_reorder(&cur, pickup.as_ref(), id, ReorderKey::Cancel);
        assert_eq!(cancelled.order, order);
        assert_eq!(cancelled.announcement, "Move cancelled.");
        assert!(cancelled.pickup.is_none() && !cancelled.commit);
        let dropped = keyboard_reorder(&cur, pickup.as_ref(), id, ReorderKey::Drop);
        assert!(dropped.commit && dropped.pickup.is_none());
        assert_eq!(dropped.announcement, "b dropped at position 4 of 4.");
        // Picking up and dropping in place persists nothing.
        let p = keyboard_reorder(&order, None, id, ReorderKey::Pick);
        assert!(!keyboard_reorder(&order, p.pickup.as_ref(), id, ReorderKey::Pick).commit);
        // A one-card list.
        let one = v(&["schedules"]);
        assert_eq!(
            keyboard_reorder(&one, None, "schedules", ReorderKey::Pick).announcement,
            "Schedules picked up, position 1 of 1."
        );
        // Keys with nothing held are not handled.
        assert!(keyboard_reorder(&order, None, id, ReorderKey::Later)
            .announcement
            .is_empty());
    }

    #[test]
    fn announcement_copy_matches_spec() {
        assert_eq!(
            picked_text("Telegram", 2, 9),
            "Telegram picked up, position 2 of 9."
        );
        assert_eq!(
            moved_text("Telegram", 3, 9),
            "Telegram moved to position 3 of 9."
        );
        assert_eq!(
            dropped_text("Telegram", 3, 9),
            "Telegram dropped at position 3 of 9."
        );
        assert_eq!(CANCELLED_TEXT, "Move cancelled.");
        assert_eq!(card_title("route:my hook"), "my hook");
        assert_eq!(card_title("api_server"), "API server");
    }

    #[test]
    fn save_failure_line_copy() {
        assert_eq!(
            SAVE_FAILED_LINE,
            "Save failed. Check server logs. The new order is active until you reload."
        );
    }

    #[test]
    fn pointer_drop_index_maps_to_new_order() {
        // One column of three cards.
        let col = [
            (0.0, 0.0, 300.0, 100.0),
            (0.0, 114.0, 300.0, 100.0),
            (0.0, 228.0, 300.0, 100.0),
        ];
        let order = v(&["x", "y", "z"]);
        // Past the third card's midpoint (278): slot 3, card 0 ends at index 2.
        let slot = pointer_drop_slot(&col, 100.0, 300.0);
        assert_eq!(slot, 3);
        assert_eq!(
            reorder(&order, 0, slot_to_index(0, slot)),
            v(&["y", "z", "x"])
        );
        // Above the first midpoint: slot 0.
        assert_eq!(pointer_drop_slot(&col, 100.0, 10.0), 0);
        // Between the first and second midpoints: slot 1.
        assert_eq!(pointer_drop_slot(&col, 100.0, 100.0), 1);
        // Banner then a row of two: x decides inside the row.
        let grid = [
            (0.0, 0.0, 700.0, 100.0),
            (0.0, 114.0, 340.0, 100.0),
            (360.0, 114.0, 340.0, 100.0),
        ];
        assert_eq!(pointer_drop_slot(&grid, 50.0, 160.0), 1);
        assert_eq!(pointer_drop_slot(&grid, 300.0, 160.0), 2);
        assert_eq!(pointer_drop_slot(&grid, 600.0, 160.0), 3);
        assert_eq!(pointer_drop_slot(&grid, 600.0, 900.0), 3);
    }

    fn render(root: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn reset_button_hidden_at_default() {
        fn hidden() -> Element {
            rsx! { ResetOrderButton { visible: false, on_reset: |_| {} } }
        }
        fn shown() -> Element {
            rsx! { ResetOrderButton { visible: true, on_reset: |_| {} } }
        }
        assert!(!render(hidden).contains("Reset card order"));
        assert!(render(shown).contains("Reset card order"));
    }

    #[test]
    fn default_gateway_cards_render_in_today_order() {
        fn root() -> Element {
            let ids = default_card_ids(&["hook".to_string()]);
            let cards = Callback::new(|id: String| rsx! { span { "card-{id}" } });
            rsx! { CardSlots { ids, on_commit: |_| {}, cards } }
        }
        let html = render(root);
        let order: Vec<&str> = html
            .split("data-card-id=\"")
            .skip(1)
            .map(|s| s.split('"').next().unwrap())
            .collect();
        assert_eq!(
            order,
            default_card_ids(&["hook".to_string()])
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(html.matches("gw-card-slot--banner").count(), 1);
        let first_slot = html.find("data-card-id=\"schedules\"").unwrap();
        assert!(html[..first_slot].contains("gw-card-slot--banner"));
    }
}
