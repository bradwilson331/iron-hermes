//! Phase 54.10 Plan 09 (D-02, D-04, D-05, D-06): AI widgets on the board.
//!
//! A widget item holds only a saved view's id and its geometry. Its title, prompt and snapshot
//! come from the 54.9 saved view, so Re-run here and the OpenUI saved-views list act on the same
//! object. Each mounted widget renders the unchanged sandboxed frame (`/openui/frame`,
//! `sandbox="allow-scripts"` only) under its own DOM id. [`WidgetBody`] is pure props so every
//! state renders without a server; [`WidgetHost`] wires it to the root-owned dashboard runtime.

use super::items::focus_dom;
use crate::components::hermes_app::dashboard_runtime::{
    DashboardGenCtx, GenJob, GenKind, GenStatus,
};
use crate::components::hermes_app::screens::chat::approval_card::{
    ApprovalCardsCtx, any_approval_pending,
};
use crate::components::hermes_app::screens::openui::{
    action_status, post_theme_to_frame_id, post_to_frame_id, snapshot_frame_cmds,
};
use crate::server::dashboard_api::{BoardItem, ItemKind};
use crate::server::openui_views_api::OpenUiView;
use dioxus::prelude::*;

/// DOM id of one widget's frame (the host bridge looks it up by this id).
pub(crate) fn widget_frame_id(item_id: &str) -> String {
    format!("ih-dash-frame-{item_id}")
}

/// What a widget item's `view_id` resolves to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum WidgetView {
    Present(OpenUiView),
    /// D-04: the saved view was deleted from OpenUI. Nothing cascades.
    Deleted,
}

pub(crate) fn widget_view_state(view_id: &str, views: &[OpenUiView]) -> WidgetView {
    views
        .iter()
        .find(|v| v.id == view_id)
        .cloned()
        .map_or(WidgetView::Deleted, WidgetView::Present)
}

/// Header title: the saved view's name, else `View deleted`.
pub(crate) fn widget_title(view: &WidgetView) -> String {
    match view {
        WidgetView::Present(v) => v.name.clone(),
        WidgetView::Deleted => "View deleted".to_string(),
    }
}

/// `Snapshot {date}` (UTC date of the last time the snapshot was written).
pub(crate) fn snapshot_label(updated_at: i64) -> String {
    let date = chrono::DateTime::from_timestamp(updated_at, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    format!("Snapshot {date}")
}

/// UI-SPEC `AI widget states` error copy.
pub(crate) fn widget_error_copy(reason: &str) -> String {
    format!(
        "Could not generate this widget: {reason}. Check your provider on the Providers screen, then try again."
    )
}

/// First characters of the prompt shown on a never-generated widget (UI-SPEC E4).
const PROMPT_CLIP_CHARS: usize = 80;

/// D-02: a view with no snapshot yet (made by the palette) and no job that is, or just was,
/// filling it. `Done` counts as generated: the refreshed view list lands a moment later.
pub(crate) fn is_not_generated(view: &OpenUiView, status: &Option<GenStatus>) -> bool {
    let running = matches!(
        status,
        Some(GenStatus::Queued | GenStatus::Generating | GenStatus::Done)
    );
    view.snapshot.trim().is_empty() && !running
}

/// What removing a widget does to its saved view (RESEARCH A9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoveEffect {
    /// The view was never generated: it exists only for this widget, so it goes too.
    DeletePendingView,
    /// A generated view stays in OpenUI (D-04).
    None,
}

pub(crate) fn remove_side_effect(view: Option<&OpenUiView>) -> RemoveEffect {
    match view {
        Some(v) if v.snapshot.trim().is_empty() => RemoveEffect::DeletePendingView,
        _ => RemoveEffect::None,
    }
}

/// The pending view to delete when item `id` is removed from `items`: only a never-generated
/// view, and only when no other widget on the board still shows it.
pub(crate) fn pending_view_to_delete(
    items: &[BoardItem],
    id: &str,
    views: &Option<Result<Vec<OpenUiView>, String>>,
) -> Option<String> {
    let view_id = items
        .iter()
        .find(|i| i.id == id)
        .and_then(|i| match &i.kind {
            ItemKind::Widget { view_id } => Some(view_id),
            _ => None,
        })?;
    let shared = items
        .iter()
        .any(|i| i.id != id && matches!(&i.kind, ItemKind::Widget { view_id: v } if v == view_id));
    let Some(Ok(list)) = views else {
        return None;
    };
    let view = list.iter().find(|v| &v.id == view_id);
    (!shared && remove_side_effect(view) == RemoveEffect::DeletePendingView)
        .then(|| view_id.clone())
}

/// What changes which widget (if any) owns pointer input (UI-SPEC A5 pointer isolation).
pub(crate) enum ActivationEvent<'a> {
    /// Double-click or Enter on a widget.
    Activate(&'a str),
    Escape,
    /// A pointerdown anywhere outside the active widget's frame.
    PointerDownOutside,
}

/// The active widget after `ev`. Only one widget is ever active.
pub(crate) fn activation_after(cur: Option<&str>, ev: ActivationEvent<'_>) -> Option<String> {
    match ev {
        ActivationEvent::Activate(id) => Some(id.to_string()),
        ActivationEvent::Escape | ActivationEvent::PointerDownOutside => {
            let _ = cur;
            None
        }
    }
}

/// One widget's body. `status` is the runtime's state for this item; `None` means idle.
#[component]
pub fn WidgetBody(
    item_id: String,
    title: String,
    view: WidgetView,
    status: Option<GenStatus>,
    action_lines: Vec<String>,
    mounted: bool,
    active: bool,
    read_only: bool,
    on_rerun: EventHandler<()>,
    on_stop: EventHandler<()>,
    on_remove: EventHandler<()>,
    on_activate: EventHandler<()>,
    on_deactivate: EventHandler<()>,
    on_frame_load: EventHandler<()>,
) -> Element {
    let view = match view {
        WidgetView::Present(v) => v,
        WidgetView::Deleted => {
            return rsx! {
                div { class: "ih-widget__deleted",
                    h3 { "View deleted" }
                    p { "The saved view this widget showed was deleted from OpenUI." }
                    if !read_only {
                        button {
                            class: "btn btn--sm btn--danger",
                            r#type: "button",
                            onkeydown: move |e| e.stop_propagation(),
                            onclick: move |_| on_remove.call(()),
                            "Remove widget"
                        }
                    }
                }
            };
        }
    };
    let busy = matches!(status, Some(GenStatus::Queued | GenStatus::Generating));
    let error = match &status {
        Some(GenStatus::Error(reason)) => Some(widget_error_copy(reason)),
        _ => None,
    };
    let frame_id = widget_frame_id(&item_id);
    let snapshot = snapshot_label(view.updated_at);
    let pending = is_not_generated(&view, &status);
    let prompt_clip: String = view.prompt.chars().take(PROMPT_CLIP_CHARS).collect();
    let footer = match status {
        Some(GenStatus::Queued) => "Queued…".to_string(),
        Some(GenStatus::Generating) => "Generating…".to_string(),
        _ if pending => "Not generated".to_string(),
        _ => snapshot.clone(),
    };
    rsx! {
        div { class: "ih-widget",
            if !read_only && error.is_none() && !pending {
                div { class: "ih-widget__bar",
                    if busy {
                        button {
                            class: "btn btn--sm btn--ghost",
                            r#type: "button",
                            "aria-label": "Stop generating {title}",
                            onkeydown: move |e| e.stop_propagation(),
                            onclick: move |_| on_stop.call(()),
                            "Stop"
                        }
                    } else {
                        button {
                            class: "btn btn--sm btn--ghost",
                            r#type: "button",
                            "aria-label": "Re-run {title}",
                            onkeydown: move |e| e.stop_propagation(),
                            onclick: move |_| on_rerun.call(()),
                            "Re-run"
                        }
                    }
                }
            }
            div { class: "ih-widget__stage",
                if let Some(copy) = error {
                    div { class: "ih-widget__error", role: "alert",
                        p { "{copy}" }
                        if !read_only {
                            div { class: "ih-widget__error-actions",
                                button {
                                    class: "btn btn--sm",
                                    r#type: "button",
                                    onkeydown: move |e| e.stop_propagation(),
                                    onclick: move |_| on_rerun.call(()),
                                    "Retry widget"
                                }
                                button {
                                    class: "btn btn--sm btn--danger",
                                    r#type: "button",
                                    onkeydown: move |e| e.stop_propagation(),
                                    onclick: move |_| on_remove.call(()),
                                    "Remove widget"
                                }
                            }
                        }
                    }
                } else if pending {
                    // The one control that starts the first turn (D-05).
                    div { class: "ih-widget__poster",
                        h3 { "Not generated yet" }
                        p { "{prompt_clip}" }
                        if !read_only {
                            button {
                                class: "btn btn--sm",
                                r#type: "button",
                                onkeydown: move |e| e.stop_propagation(),
                                onclick: move |_| on_rerun.call(()),
                                "Generate widget"
                            }
                        }
                    }
                } else if mounted {
                    // Allow-scripts only, no other sandbox token (T-54.10-28). Quoted: dioxus-html
                    // has no typed `sandbox` attribute (same workaround as openui.rs).
                    iframe {
                        id: "{frame_id}",
                        class: "ih-widget__frame",
                        "sandbox": "allow-scripts",
                        src: "/openui/frame",
                        title: "{view.name}",
                        onload: move |_| on_frame_load.call(()),
                    }
                    if active {
                        button {
                            class: "ih-widget__chip",
                            r#type: "button",
                            // Space, Enter and Delete must act on the button, not the item; Esc
                            // still bubbles to the item, which leaves the frame.
                            onkeydown: move |e| {
                                if e.key().to_string() != "Escape" {
                                    e.stop_propagation();
                                }
                            },
                            onclick: move |_| on_deactivate.call(()),
                            "Interacting. Press Esc to exit."
                        }
                    } else if !read_only {
                        // Keeps drag, select, pan and zoom working over the frame (A5).
                        div {
                            class: "ih-widget__overlay",
                            title: "Double-click to interact",
                            ondoubleclick: move |_| on_activate.call(()),
                        }
                    }
                } else {
                    div { class: "ih-widget__poster",
                        h3 { "{view.name}" }
                        p { "Paused to save memory. Select to load." }
                        p { class: "ih-widget__date", "{snapshot}" }
                    }
                }
            }
            div { class: "ih-widget__foot",
                span { "{footer}" }
                for line in action_lines {
                    span { class: "ih-widget__action", "{line}" }
                }
            }
        }
    }
}

/// Connects [`WidgetBody`] to the root-owned runtime, the saved-view list and the screen's
/// active-widget signal. Plain-props `WidgetBody` carries the markup and the tests.
#[component]
pub(crate) fn WidgetHost(
    item_id: String,
    view_id: String,
    views: Signal<Option<Result<Vec<OpenUiView>, String>>>,
    active: Signal<Option<String>>,
    mounted: bool,
    read_only: bool,
    on_remove: EventHandler<()>,
) -> Element {
    let ctx = use_context::<DashboardGenCtx>();
    let approvals = use_context::<ApprovalCardsCtx>().0;
    let theme = use_context::<crate::state::ThemeContext>().0;

    let state = match &*views.read() {
        Some(Ok(list)) => Some(widget_view_state(&view_id, list)),
        _ => None,
    };
    let Some(view) = state else {
        return rsx! {
            div { class: "ih-widget__deleted",
                p { "Saved views unavailable." }
            }
        };
    };
    let title = widget_title(&view);
    let status = ctx.statuses.read().get(&item_id).cloned();
    let action_lines = ctx
        .action_labels
        .read()
        .get(&item_id)
        .map(|label| action_status(label, any_approval_pending(&approvals.read())))
        .unwrap_or_default();
    let is_active = active.read().as_deref() == Some(item_id.as_str());

    let rerun = {
        let (item_id, view_id) = (item_id.clone(), view_id.clone());
        move |_| {
            let Some(Ok(list)) = views.peek().clone() else {
                return;
            };
            if let WidgetView::Present(v) = widget_view_state(&view_id, &list) {
                let mut labels = ctx.action_labels;
                labels.with_mut(|m| {
                    m.remove(&item_id);
                });
                ctx.enqueue.call(GenJob {
                    item_id: item_id.clone(),
                    view_id: view_id.clone(),
                    message: v.prompt,
                    kind: GenKind::Rerun,
                });
            }
        }
    };
    let stop = {
        let item_id = item_id.clone();
        move |_| ctx.stop.call(item_id.clone())
    };
    let activate = {
        let item_id = item_id.clone();
        move |_| {
            let mut active = active;
            let next = activation_after(
                active.peek().as_deref(),
                ActivationEvent::Activate(&item_id),
            );
            active.set(next);
            let frame = widget_frame_id(&item_id);
            spawn(async move {
                // The overlay unmounts on the next render; focus the frame then.
                crate::platform::timer::sleep(50).await;
                focus_dom(&frame);
            });
        }
    };
    let deactivate = move |_| {
        let mut active = active;
        let next = activation_after(active.peek().as_deref(), ActivationEvent::Escape);
        active.set(next);
    };
    // The frame (re)loaded: replay the running turn's text if one is active, else the saved
    // snapshot (no model call), then the theme for this frame only.
    let frame_load = {
        let (item_id, view_id) = (item_id.clone(), view_id.clone());
        move |_| {
            let mut loaded = ctx.loaded_frames;
            loaded.with_mut(|s| {
                s.insert(item_id.clone());
            });
            let frame = widget_frame_id(&item_id);
            let running = matches!(
                ctx.statuses.peek().get(&item_id),
                Some(GenStatus::Queued | GenStatus::Generating)
            );
            let cmds = if running {
                ctx.docs
                    .peek()
                    .get(&item_id)
                    .map(|d| d.replay())
                    .unwrap_or_default()
            } else {
                match views.peek().as_ref() {
                    Some(Ok(list)) => match widget_view_state(&view_id, list) {
                        WidgetView::Present(v) => snapshot_frame_cmds(&v.snapshot),
                        WidgetView::Deleted => Vec::new(),
                    },
                    _ => Vec::new(),
                }
            };
            for cmd in &cmds {
                post_to_frame_id(&frame, cmd);
            }
            let light = crate::components::hermes_app::theme_effects::is_light_theme(&theme.peek());
            spawn(async move {
                crate::platform::timer::sleep(50).await;
                post_theme_to_frame_id(&frame, light);
            });
        }
    };

    rsx! {
        WidgetBody {
            item_id,
            title,
            view,
            status,
            action_lines,
            mounted,
            active: is_active,
            read_only,
            on_rerun: rerun,
            on_stop: stop,
            on_remove: move |_| on_remove.call(()),
            on_activate: activate,
            on_deactivate: deactivate,
            on_frame_load: frame_load,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_math::canvas_shortcut;

    const UPDATED: i64 = 1_700_000_000; // 2023-11-14 UTC

    fn view(id: &str, name: &str) -> OpenUiView {
        OpenUiView {
            id: id.into(),
            name: name.into(),
            prompt: "p".into(),
            snapshot: "root = Stack([])".into(),
            created_at: 1,
            updated_at: UPDATED,
        }
    }

    fn ssr(root: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    fn body(v: WidgetView, status: Option<GenStatus>, mounted: bool, active: bool) -> Element {
        rsx! {
            WidgetBody {
                item_id: "abc".to_string(),
                title: widget_title(&v),
                view: v,
                status,
                action_lines: vec![],
                mounted,
                active,
                read_only: false,
                on_rerun: |_| {},
                on_stop: |_| {},
                on_remove: |_| {},
                on_activate: |_| {},
                on_deactivate: |_| {},
                on_frame_load: |_| {},
            }
        }
    }

    fn present() -> WidgetView {
        WidgetView::Present(view("v1", "Cron failures"))
    }

    #[test]
    fn frame_id_for_item() {
        assert_eq!(widget_frame_id("abc"), "ih-dash-frame-abc");
    }

    #[test]
    fn widget_view_state_present_or_deleted() {
        let views = vec![view("v1", "Cron failures"), view("v2", "Other")];
        assert_eq!(
            widget_view_state("v2", &views),
            WidgetView::Present(views[1].clone())
        );
        assert_eq!(widget_view_state("gone", &views), WidgetView::Deleted);
        assert_eq!(widget_view_state("v1", &[]), WidgetView::Deleted);
        assert_eq!(widget_title(&WidgetView::Deleted), "View deleted");
    }

    #[test]
    fn widget_frame_markup() {
        let html = ssr(|| {
            body(
                WidgetView::Present(view("v1", "Cron failures")),
                None,
                true,
                false,
            )
        });
        assert!(html.contains("id=\"ih-dash-frame-abc\""), "{html}");
        assert!(html.contains("sandbox=\"allow-scripts\""), "{html}");
        assert!(html.contains("src=\"/openui/frame\""), "{html}");
        assert!(html.contains("title=\"Cron failures\""), "{html}");
        assert!(html.contains("Snapshot 2023-11-14"), "{html}");
        // No other sandbox token (T-54.10-28).
        assert_eq!(html.matches("allow-").count(), 1, "{html}");
    }

    #[test]
    fn deleted_view_tile() {
        let html = ssr(|| body(WidgetView::Deleted, None, true, false));
        assert!(html.contains("View deleted"), "{html}");
        assert!(
            html.contains("The saved view this widget showed was deleted from OpenUI."),
            "{html}"
        );
        assert!(html.contains("Remove widget"), "{html}");
        assert!(html.contains("btn--danger"), "{html}");
        assert!(
            !html.contains("Re-run") && !html.contains("<iframe"),
            "{html}"
        );
    }

    #[test]
    fn widget_footer_states() {
        let generating = ssr(|| body(present(), Some(GenStatus::Generating), true, false));
        assert!(generating.contains("Generating…"), "{generating}");
        assert!(
            generating.contains("aria-label=\"Stop generating Cron failures\""),
            "{generating}"
        );
        assert!(!generating.contains("Re-run"), "{generating}");

        let error = ssr(|| {
            body(
                present(),
                Some(GenStatus::Error("boom".into())),
                true,
                false,
            )
        });
        assert!(
            error.contains(
                "Could not generate this widget: boom. Check your provider on the Providers screen, then try again."
            ),
            "{error}"
        );
        assert!(
            error.contains("Retry widget") && error.contains("Remove widget"),
            "{error}"
        );

        let done = ssr(|| body(present(), Some(GenStatus::Done), true, false));
        assert!(done.contains("Snapshot 2023-11-14"), "{done}");
        assert!(
            done.contains("aria-label=\"Re-run Cron failures\""),
            "{done}"
        );
        assert!(done.contains(">Re-run<"), "{done}");
        assert!(!done.contains("Stop generating"), "{done}");
    }

    #[test]
    fn paused_poster_markup() {
        let html = ssr(|| {
            body(
                WidgetView::Present(view("v1", "Cron failures")),
                None,
                false,
                false,
            )
        });
        assert!(html.contains("<h3>Cron failures</h3>"), "{html}");
        assert!(
            html.contains("Paused to save memory. Select to load."),
            "{html}"
        );
        assert!(html.contains("Snapshot 2023-11-14"), "{html}");
        assert!(!html.contains("<iframe"), "{html}");
    }

    fn pending_view(prompt: &str) -> WidgetView {
        WidgetView::Present(OpenUiView {
            snapshot: String::new(),
            prompt: prompt.into(),
            ..view("v1", "Weekly spend")
        })
    }

    #[test]
    fn not_generated_widget_body() {
        let html = ssr(|| {
            body(
                pending_view(&format!("{}{}", "x".repeat(80), "y".repeat(40))),
                None,
                true,
                false,
            )
        });
        assert!(html.contains("Not generated yet"), "{html}");
        assert!(
            html.contains(&"x".repeat(80)) && !html.contains("yy"),
            "{html}"
        );
        assert!(html.contains("btn btn--sm"), "{html}");
        assert!(html.contains(">Generate widget<"), "{html}");
        assert!(html.contains("Not generated</span>"), "{html}");
        assert!(
            !html.contains("<iframe") && !html.contains("Re-run"),
            "{html}"
        );
        // A running job shows the frame, not the invitation.
        let busy = ssr(|| body(pending_view("p"), Some(GenStatus::Generating), true, false));
        assert!(
            busy.contains("<iframe") && !busy.contains("Not generated yet"),
            "{busy}"
        );
        // A failed first generation is the error tile (Retry widget), not the invitation.
        let err = ssr(|| {
            body(
                pending_view("p"),
                Some(GenStatus::Error("boom".into())),
                true,
                false,
            )
        });
        assert!(
            err.contains("Retry widget") && !err.contains("Not generated yet"),
            "{err}"
        );
        // A generated view is unchanged.
        let done = ssr(|| body(present(), None, true, false));
        assert!(!done.contains("Not generated yet"), "{done}");
    }

    #[test]
    fn remove_widget_deletes_pending_view_only() {
        let gen_view = view("v1", "Cron failures");
        let pending = OpenUiView {
            snapshot: String::new(),
            ..view("v2", "Weekly spend")
        };
        assert_eq!(
            remove_side_effect(Some(&pending)),
            RemoveEffect::DeletePendingView
        );
        assert_eq!(remove_side_effect(Some(&gen_view)), RemoveEffect::None);
        assert_eq!(remove_side_effect(None), RemoveEffect::None);

        let widget = |id: &str, view_id: &str| BoardItem {
            id: id.into(),
            x: 0,
            y: 0,
            w: 480,
            h: 320,
            kind: ItemKind::Widget {
                view_id: view_id.into(),
            },
        };
        let views = Some(Ok(vec![gen_view, pending]));
        let items = vec![widget("a", "v1"), widget("b", "v2")];
        assert_eq!(
            pending_view_to_delete(&items, "b", &views).as_deref(),
            Some("v2")
        );
        assert_eq!(pending_view_to_delete(&items, "a", &views), None);
        assert_eq!(pending_view_to_delete(&items, "zzz", &views), None);
        // Another widget still shows the pending view: keep it.
        let shared = vec![widget("b", "v2"), widget("c", "v2")];
        assert_eq!(pending_view_to_delete(&shared, "b", &views), None);
        // Views not loaded: never delete on a guess.
        assert_eq!(pending_view_to_delete(&items, "b", &None), None);
    }

    #[test]
    fn activation_state_machine() {
        use ActivationEvent::*;
        let a = activation_after(None, Activate("a"));
        assert_eq!(a.as_deref(), Some("a"));
        // A second widget's activation replaces the first: only one is ever active.
        let b = activation_after(a.as_deref(), Activate("b"));
        assert_eq!(b.as_deref(), Some("b"));
        assert_eq!(activation_after(b.as_deref(), Escape), None);
        assert_eq!(activation_after(b.as_deref(), PointerDownOutside), None);
        // While a frame is active the canvas shortcuts are suspended.
        assert_eq!(canvas_shortcut("n", false, false, false, b.is_some()), None);
        assert!(canvas_shortcut("n", false, false, false, false).is_some());
    }

    #[test]
    fn active_widget_markup() {
        let active = ssr(|| body(present(), None, true, true));
        assert!(
            active.contains("Interacting. Press Esc to exit."),
            "{active}"
        );
        assert!(!active.contains("ih-widget__overlay"), "{active}");
        let idle = ssr(|| body(present(), None, true, false));
        assert!(idle.contains("ih-widget__overlay"), "{idle}");
        assert!(!idle.contains("Interacting."), "{idle}");
        // The overlay is a later sibling of the iframe, so it paints above it.
        assert!(
            idle.find("<iframe").unwrap() < idle.find("ih-widget__overlay").unwrap(),
            "{idle}"
        );
    }
}
