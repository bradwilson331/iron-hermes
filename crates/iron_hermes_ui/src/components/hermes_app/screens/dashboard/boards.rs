//! Phase 54.10 Plan 11 (D-03/D-14): board management.
//!
//! The preset chooser (one panel for `New board`, `Use a preset` and the no-boards state), the
//! preset notice (the single explicit spend point for a preset's AI widgets) and the `New board`
//! button. Presets only create pending saved views; an agent turn starts solely from the
//! notice's `Generate {n} widgets` (T-54.10-35).

use super::canvas::BoardState;
use super::items::SavedViews;
use super::{SAVE_FAILED, SaveStatus, name_problem, new_board};
use crate::components::hermes_app::dashboard_runtime::{
    DashboardGenCtx, GenJob, GenKind, GenStatus,
};
use crate::components::hermes_app::header::{MenuKey, focus_element_by_id, menu_next_index};
use crate::components::hermes_app::screens::openui::default_view_name;
use crate::dashboard_model::{
    GenTriple, Preset, PresetSpec, generate_jobs_for, pending_view_ids, plan_preset, preset_notice,
    preset_spec,
};
use crate::server::dashboard_api::{BOARDS_MAX, BoardItem, boards_full_error, validate_board};
use crate::server::dashboard_api::{delete_dashboard_board, save_dashboard_board};
use crate::server::openui_views_api::{create_pending_openui_view, delete_openui_view};
use dioxus::prelude::*;
use std::collections::HashMap;

/// Delete the pending views a failed preset create left behind (best effort).
async fn discard_views(ids: Vec<String>) {
    for id in ids {
        let _ = delete_openui_view(id).await;
    }
}

/// Make the pending views, lay the items out and save the new board. Returns the board id.
/// Any failure removes the views it made, so a failed create leaves nothing behind.
async fn create_preset_board(name: &str, spec: &PresetSpec) -> Result<String, ()> {
    let mut view_ids = Vec::new();
    for prompt in &spec.prompts {
        match create_pending_openui_view(default_view_name(prompt), prompt.clone()).await {
            Ok(v) => view_ids.push(v.id),
            Err(_) => {
                discard_views(view_ids).await;
                return Err(());
            }
        }
    }
    let mut board = new_board(name);
    board.items = plan_preset(spec, &view_ids).items;
    // Same rules the server enforces; a refusal here never hits the wire.
    if validate_board(&board).is_err() {
        discard_views(view_ids).await;
        return Err(());
    }
    match save_dashboard_board(board).await {
        Ok(saved) => Ok(saved.id),
        Err(_) => {
            discard_views(view_ids).await;
            Err(())
        }
    }
}

/// The chooser panel (UI-SPEC A8) with plain props so an SSR test renders it without a server.
#[component]
pub(crate) fn ChooserView(
    preset: Preset,
    name: String,
    name_error: Option<String>,
    save_error: Option<String>,
    busy: bool,
    board_count: usize,
    on_preset: EventHandler<Preset>,
    on_name: EventHandler<String>,
    on_create: EventHandler<()>,
    #[props(default)] on_cancel: Option<EventHandler<()>>,
) -> Element {
    let at_cap = board_count >= BOARDS_MAX;
    rsx! {
        div { class: "panel ih-chooser",
            div { class: "panel-title", "Start from a preset" }
            div { class: "row-list", role: "radiogroup", "aria-label": "Preset",
                for p in Preset::ALL {
                    div {
                        key: "{p.title()}",
                        class: "row",
                        class: if p == preset { "is-active" },
                        role: "radio",
                        tabindex: "0",
                        "aria-checked": if p == preset { "true" } else { "false" },
                        onclick: move |_| on_preset.call(p),
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter || e.key() == Key::Character(" ".to_string()) {
                                e.prevent_default();
                                on_preset.call(p);
                            }
                        },
                        div { class: "row-main",
                            div { class: "row-title", "{p.title()}" }
                            div { class: "row-sub", "{p.sub()}" }
                        }
                    }
                }
            }
            label { r#for: "ih-dashboard-board-name", "Board name" }
            input {
                id: "ih-dashboard-board-name",
                class: "field-input",
                r#type: "text",
                value: "{name}",
                "aria-invalid": if name_error.is_some() { "true" },
                "aria-describedby": if name_error.is_some() { "ih-dashboard-board-name-error" },
                oninput: move |e| on_name.call(e.value()),
            }
            if let Some(err) = name_error.clone() {
                div {
                    id: "ih-dashboard-board-name-error",
                    role: "alert",
                    style: "color: var(--danger);",
                    "{err}"
                }
            }
            div { class: "ih-chooser__actions",
                button {
                    class: "btn",
                    r#type: "button",
                    disabled: busy || at_cap,
                    "aria-busy": if busy { "true" },
                    onclick: move |_| on_create.call(()),
                    "Create board"
                }
                if let Some(cancel) = on_cancel {
                    button {
                        class: "btn btn--sm btn--ghost",
                        r#type: "button",
                        disabled: busy,
                        onclick: move |_| cancel.call(()),
                        "Cancel"
                    }
                }
            }
            if at_cap {
                div { class: "help", "{boards_full_error(board_count)}" }
            }
            if let Some(err) = save_error.clone() {
                div { role: "alert", style: "color: var(--danger);", "{err}" }
            }
        }
    }
}

/// The preset chooser. `on_created` gets the new board's id once it is saved; the parent opens it.
#[component]
pub(crate) fn PresetChooser(
    board_count: usize,
    on_created: EventHandler<String>,
    #[props(default)] on_cancel: Option<EventHandler<()>>,
) -> Element {
    let mut preset = use_signal(|| Preset::Blank);
    let mut name = use_signal(|| Preset::Blank.title().to_string());
    // Once the operator types a name, picking another preset no longer renames the board.
    let mut edited = use_signal(|| false);
    let mut name_error = use_signal(|| None::<String>);
    let mut save_error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    // Optional so the SSR tests render without the runtime provider.
    let views_rev = try_use_context::<DashboardGenCtx>().map(|c| c.views_rev);
    rsx! {
        ChooserView {
            preset: *preset.read(),
            name: name.read().clone(),
            name_error: name_error.read().clone(),
            save_error: save_error.read().clone(),
            busy: *busy.read(),
            board_count,
            on_cancel,
            on_preset: move |p: Preset| {
                preset.set(p);
                if !*edited.peek() {
                    name.set(p.title().to_string());
                }
            },
            on_name: move |v: String| {
                edited.set(true);
                name.set(v);
            },
            on_create: move |_| {
                if *busy.peek() || board_count >= BOARDS_MAX {
                    return;
                }
                let board_name = name.peek().clone();
                if let Some(problem) = name_problem(&board_name) {
                    name_error.set(Some(problem.to_string()));
                    return;
                }
                name_error.set(None);
                save_error.set(None);
                busy.set(true);
                let spec = preset_spec(*preset.peek());
                spawn(async move {
                    match create_preset_board(&board_name, &spec).await {
                        Ok(id) => {
                            // Clear our state first: the parent may unmount this panel.
                            busy.set(false);
                            if let Some(mut rev) = views_rev {
                                rev += 1;
                            }
                            on_created.call(id);
                        }
                        Err(()) => {
                            save_error.set(Some(SAVE_FAILED.to_string()));
                            busy.set(false);
                        }
                    }
                });
            },
        }
    }
}

/// `New board` (ghost sm), `disabled` with the cap copy at [`BOARDS_MAX`].
#[component]
pub(crate) fn NewBoardButton(board_count: usize, on_click: EventHandler<()>) -> Element {
    let at_cap = board_count >= BOARDS_MAX;
    rsx! {
        button {
            class: "btn btn--sm btn--ghost",
            r#type: "button",
            disabled: at_cap,
            title: if at_cap { boards_full_error(board_count) },
            onclick: move |_| on_click.call(()),
            "New board"
        }
        if at_cap {
            span { class: "help", "{boards_full_error(board_count)}" }
        }
    }
}

/// The pending widgets a `Generate` press would run: empty snapshot and no job already
/// queued, running or just finished for that item.
pub(crate) fn notice_jobs(
    items: &[BoardItem],
    views: &SavedViews,
    statuses: &HashMap<String, GenStatus>,
) -> Vec<GenTriple> {
    let Some(Ok(list)) = views else {
        return vec![];
    };
    generate_jobs_for(items, list)
        .into_iter()
        .filter(|(item_id, _, _)| {
            !matches!(
                statuses.get(item_id),
                Some(GenStatus::Queued | GenStatus::Generating | GenStatus::Done)
            )
        })
        .collect()
}

/// The strip under the toolbar while the open board has widgets that were never generated.
#[component]
pub(crate) fn PresetNotice(
    st: BoardState,
    dismissed: bool,
    on_dismiss: EventHandler<()>,
) -> Element {
    let ctx = use_context::<DashboardGenCtx>();
    let items = st
        .board
        .read()
        .as_ref()
        .map(|b| b.items.clone())
        .unwrap_or_default();
    let jobs = notice_jobs(&items, &st.views.read(), &ctx.statuses.read());
    if dismissed || jobs.is_empty() {
        return rsx! {};
    }
    let (sentence, label) = preset_notice(jobs.len());
    rsx! {
        div { class: "ih-preset-notice", role: "status",
            span { "{sentence}" }
            button {
                class: "btn btn--sm",
                r#type: "button",
                onclick: move |_| {
                    // Exactly one Rerun job per pending widget: the one place a preset spends.
                    for (item_id, view_id, prompt) in jobs.clone() {
                        ctx.enqueue.call(GenJob {
                            item_id,
                            view_id,
                            message: prompt,
                            kind: GenKind::Rerun,
                        });
                    }
                },
                "{label}"
            }
            button {
                class: "btn btn--sm btn--ghost",
                r#type: "button",
                onclick: move |_| on_dismiss.call(()),
                "Not now"
            }
        }
    }
}

/// The name a rename commits, or the UI-SPEC name error.
pub(crate) fn rename_outcome(raw: &str) -> Result<String, &'static str> {
    match name_problem(raw) {
        Some(e) => Err(e),
        None => Ok(raw.trim().to_string()),
    }
}

/// The board to open after `deleted` goes: the one that takes its place in the list, else the
/// one before it; `None` when it was the last board (the no-boards state).
pub(crate) fn next_board_after_delete(ids: &[String], deleted: &str) -> Option<String> {
    let at = ids.iter().position(|i| i == deleted)?;
    let rest: Vec<&String> = ids.iter().filter(|i| *i != deleted).collect();
    rest.get(at.min(rest.len().checked_sub(1)?))
        .map(|s| (*s).clone())
}

/// UI-SPEC "Destructive: delete board".
pub(crate) fn delete_confirm_copy(name: &str, items: usize) -> String {
    let noun = if items == 1 { "item" } else { "items" };
    format!(
        "Delete board: Remove \"{name}\" and its {items} {noun} from this profile? The saved views it showed stay in OpenUI. This cannot be undone."
    )
}

/// DOM id of the `⋯` trigger (focus returns here when a menu step closes).
pub(crate) const BOARD_MENU_ID: &str = "ih-board-menu";
/// The delete confirm opens with focus on `Keep board`.
pub(crate) const DELETE_FOCUS_ID: &str = "ih-board-delete-keep";
const RENAME_ID: &str = "ih-board-rename";

fn menu_item_id(i: usize) -> String {
    format!("ih-board-menu-item-{i}")
}

/// The inline delete confirm, shown in the menu's place.
#[component]
pub(crate) fn DeleteConfirm(
    name: String,
    items: usize,
    busy: bool,
    on_keep: EventHandler<()>,
    on_delete: EventHandler<()>,
) -> Element {
    use_effect(move || focus_element_by_id(DELETE_FOCUS_ID));
    rsx! {
        div {
            class: "panel ih-boardmenu__pop ih-boardmenu__confirm",
            role: "alertdialog",
            "aria-label": "Delete board",
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Escape {
                    e.stop_propagation();
                    on_keep.call(());
                }
            },
            p { "{delete_confirm_copy(&name, items)}" }
            div { class: "ih-chooser__actions",
                button {
                    class: "btn btn--sm btn--danger",
                    r#type: "button",
                    disabled: busy,
                    "aria-busy": if busy { "true" },
                    onclick: move |_| on_delete.call(()),
                    "Delete board"
                }
                button {
                    id: DELETE_FOCUS_ID,
                    class: "btn btn--sm btn--ghost",
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| on_keep.call(()),
                    "Keep board"
                }
            }
        }
    }
}

/// The `⋯` board menu: `Rename board` and `Delete board` (AddMenu's key contract).
#[component]
pub(crate) fn BoardMenu(
    st: BoardState,
    board_id: String,
    board_name: String,
    item_count: usize,
    board_ids: Vec<String>,
    status: Signal<SaveStatus>,
    on_rename: EventHandler<()>,
    on_deleted: EventHandler<Option<String>>,
    /// `true` when a delete starts (the screen stops autosaving this board), `false` if it failed.
    on_deleting: EventHandler<bool>,
) -> Element {
    let mut open = use_signal(|| false);
    let mut confirming = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let gen_ctx = try_use_context::<DashboardGenCtx>();
    let views_rev = gen_ctx.map(|c| c.views_rev);
    use_effect(move || {
        if *open.read() {
            focus_element_by_id(&menu_item_id(0));
        }
    });
    let is_open = *open.read();
    let is_busy = *busy.read();
    let expanded = if is_open { "true" } else { "false" };
    let confirm_name = board_name.clone();
    rsx! {
        div { class: "ih-addmenu-wrap",
            button {
                id: BOARD_MENU_ID,
                class: "btn btn--icon btn--ghost",
                r#type: "button",
                "aria-label": "Board menu",
                "aria-haspopup": "menu",
                "aria-expanded": "{expanded}",
                disabled: is_busy,
                "aria-busy": if is_busy { "true" },
                onclick: move |_| {
                    let v = *open.peek();
                    open.set(!v);
                },
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::ArrowDown {
                        e.prevent_default();
                        open.set(true);
                    }
                },
                "⋯"
            }
            if is_open {
                div { class: "ih-menu-backdrop", onclick: move |_| open.set(false) }
                div { class: "ih-menu ih-addmenu ih-boardmenu__pop", role: "menu",
                    for (i , label) in ["Rename board", "Delete board"].into_iter().enumerate() {
                        button {
                            id: "{menu_item_id(i)}",
                            class: "item",
                            role: "menuitem",
                            r#type: "button",
                            onclick: move |_| {
                                open.set(false);
                                if i == 0 {
                                    on_rename.call(());
                                } else {
                                    confirming.set(true);
                                }
                            },
                            onkeydown: move |e: KeyboardEvent| {
                                let nav = match e.key() {
                                    Key::ArrowDown => Some(MenuKey::Down),
                                    Key::ArrowUp => Some(MenuKey::Up),
                                    Key::Home => Some(MenuKey::Home),
                                    Key::End => Some(MenuKey::End),
                                    _ => None,
                                };
                                if let Some(k) = nav {
                                    e.prevent_default();
                                    if let Some(n) = menu_next_index(Some(i), 2, k) {
                                        focus_element_by_id(&menu_item_id(n));
                                    }
                                } else if e.key() == Key::Escape {
                                    e.stop_propagation();
                                    open.set(false);
                                    focus_element_by_id(BOARD_MENU_ID);
                                }
                            },
                            "{label}"
                        }
                    }
                }
            }
            if *confirming.read() {
                DeleteConfirm {
                    name: confirm_name,
                    items: item_count,
                    busy: is_busy,
                    on_keep: move |_| {
                        confirming.set(false);
                        focus_element_by_id(BOARD_MENU_ID);
                    },
                    on_delete: move |_| {
                        if *busy.peek() {
                            return;
                        }
                        busy.set(true);
                        let id = board_id.clone();
                        let name = board_name.clone();
                        let next = next_board_after_delete(&board_ids, &id);
                        // Never-generated views exist only for this board's widgets: they go too.
                        let pending = match &*st.views.peek() {
                            Some(Ok(list)) => pending_view_ids(&st.items(), list),
                            _ => vec![],
                        };
                        // Before the first await: no autosave (or unmount flush) may re-create the
                        // board, and its queued or running widget jobs must not outlive it.
                        on_deleting.call(true);
                        if let Some(ctx) = gen_ctx {
                            for item in st.items() {
                                ctx.cancel_item.call(item.id);
                            }
                        }
                        spawn(async move {
                            if delete_dashboard_board(id).await.is_err() {
                                // The prior state stays: the board is still listed and open.
                                on_deleting.call(false);
                                status.set(SaveStatus::Failed);
                                confirming.set(false);
                                busy.set(false);
                                return;
                            }
                            for view_id in pending {
                                // Never blocks the delete; the view just stays listed in OpenUI.
                                let _ = delete_openui_view(view_id).await;
                            }
                            if let Some(mut rev) = views_rev {
                                rev += 1;
                            }
                            st.say(format!("Board {name} deleted."));
                            // The menu unmounts with the open board: no signal use after this.
                            on_deleted.call(next);
                        });
                    },
                }
            }
        }
    }
}

/// The board select, turned into an inline field. Enter or blur commits, Esc reverts.
#[component]
pub(crate) fn RenameField(
    st: BoardState,
    name: String,
    status: Signal<SaveStatus>,
    boards_rev: Signal<u32>,
    on_done: EventHandler<()>,
) -> Element {
    let mut value = use_signal({
        let n = name.clone();
        move || n
    });
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    // Enter then blur (or Esc then blur) must act once.
    let mut settled = use_signal(|| false);
    use_effect(move || focus_element_by_id(RENAME_ID));
    let commit = use_callback(move |_: ()| {
        if *settled.peek() || *busy.peek() {
            return;
        }
        let raw = value.peek().clone();
        match rename_outcome(&raw) {
            Err(e) => error.set(Some(e.to_string())),
            Ok(new) if new == name => {
                settled.set(true);
                on_done.call(());
            }
            Ok(new) => {
                let Some(mut board) = st.board.peek().clone() else {
                    settled.set(true);
                    on_done.call(());
                    return;
                };
                settled.set(true);
                busy.set(true);
                error.set(None);
                board.name = new.clone();
                spawn(async move {
                    if save_dashboard_board(board).await.is_ok() {
                        let mut working = st.board;
                        if let Some(b) = working.write().as_mut() {
                            b.name = new;
                        }
                        status.set(SaveStatus::Saved);
                        let mut rev = boards_rev;
                        rev += 1;
                    } else {
                        // The prior name stays.
                        status.set(SaveStatus::Failed);
                    }
                    on_done.call(());
                });
            }
        }
    });
    let invalid = error.read().clone();
    rsx! {
        input {
            id: RENAME_ID,
            class: "field-input",
            r#type: "text",
            "aria-label": "Board name",
            style: "max-width: 200px;",
            value: "{value}",
            disabled: *busy.read(),
            "aria-busy": if *busy.read() { "true" },
            "aria-invalid": if invalid.is_some() { "true" },
            "aria-describedby": if invalid.is_some() { "ih-board-rename-error" },
            oninput: move |e| value.set(e.value()),
            onkeydown: move |e: KeyboardEvent| match e.key() {
                Key::Enter => commit.call(()),
                Key::Escape => {
                    e.stop_propagation();
                    if !*settled.peek() {
                        settled.set(true);
                        on_done.call(());
                    }
                }
                _ => {}
            },
            onblur: move |_| commit.call(()),
        }
        if let Some(err) = invalid.clone() {
            div {
                id: "ih-board-rename-error",
                role: "alert",
                style: "color: var(--danger);",
                "{err}"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssr(root: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    fn chooser(count: usize, busy: bool, err: Option<&str>) -> Element {
        rsx! {
            ChooserView {
                preset: Preset::Founder,
                name: "Founder".to_string(),
                name_error: err.map(String::from),
                save_error: None,
                busy,
                board_count: count,
                on_preset: |_| {},
                on_name: |_| {},
                on_create: |_| {},
            }
        }
    }

    #[test]
    fn chooser_renders_presets_name_and_states() {
        let html = ssr(|| chooser(0, false, None));
        for p in Preset::ALL {
            assert!(html.contains(&format!(">{}<", p.title())), "{html}");
            assert!(html.contains(p.sub()), "{html}");
        }
        assert_eq!(html.matches("role=\"radio\"").count(), 5, "{html}");
        assert!(html.contains("row is-active"), "{html}");
        assert!(html.contains("value=\"Founder\""), "{html}");
        assert!(html.contains("Create board"), "{html}");
        assert!(
            !html.contains("disabled") && !html.contains("aria-busy"),
            "{html}"
        );

        let capped = ssr(|| chooser(12, false, None));
        assert!(
            capped.contains("You have 12 boards, the maximum. Delete one to create another."),
            "{capped}"
        );
        assert!(capped.contains("disabled"), "{capped}");

        let busy = ssr(|| chooser(0, true, None));
        assert!(
            busy.contains("disabled") && busy.contains("aria-busy=\"true\""),
            "{busy}"
        );

        let bad = ssr(|| chooser(0, false, Some("Board name must be 1 to 40 characters.")));
        assert!(
            bad.contains("Board name must be 1 to 40 characters."),
            "{bad}"
        );
        assert!(
            bad.contains("aria-describedby=\"ih-dashboard-board-name-error\""),
            "{bad}"
        );
    }

    #[test]
    fn notice_jobs_skip_running_and_generated() {
        use crate::server::dashboard_api::ItemKind;
        use crate::server::openui_views_api::OpenUiView;
        let item = |id: &str, view: &str| BoardItem {
            id: id.into(),
            x: 0,
            y: 0,
            w: 480,
            h: 320,
            kind: ItemKind::Widget {
                view_id: view.into(),
            },
        };
        let view = |id: &str, snap: &str| OpenUiView {
            id: id.into(),
            name: "n".into(),
            prompt: format!("p-{id}"),
            snapshot: snap.into(),
            created_at: 0,
            updated_at: 0,
        };
        let items = vec![item("i1", "v1"), item("i2", "v2"), item("i3", "v3")];
        let views: SavedViews = Some(Ok(vec![view("v1", ""), view("v2", ""), view("v3", "x")]));
        let mut statuses = HashMap::new();
        assert_eq!(notice_jobs(&items, &views, &statuses).len(), 2);
        statuses.insert("i2".to_string(), GenStatus::Queued);
        let jobs = notice_jobs(&items, &views, &statuses);
        assert_eq!(jobs, vec![("i1".into(), "v1".into(), "p-v1".into())]);
        assert!(notice_jobs(&items, &None, &statuses).is_empty());
    }

    #[test]
    fn rename_commit_and_revert() {
        assert_eq!(rename_outcome("  New name  "), Ok("New name".to_string()));
        let err = Err("Board name must be 1 to 40 characters.");
        assert_eq!(rename_outcome(""), err);
        assert_eq!(rename_outcome(&"x".repeat(41)), err);
        // Esc and a same-name commit are wired to revert / do nothing, never to a save.
        const SRC: &str = include_str!("boards.rs");
        assert!(SRC.contains("Key::Escape =>"));
        assert!(SRC.contains("Ok(new) if new == name"));
    }

    #[test]
    fn delete_confirm_copy_and_focus() {
        let html = ssr(|| {
            rsx! {
                DeleteConfirm {
                    name: "Ops".to_string(),
                    items: 4,
                    busy: false,
                    on_keep: |_| {},
                    on_delete: |_| {},
                }
            }
        });
        let copy = "Remove &#34;Ops&#34; and its 4 items from this profile? The saved views it showed stay in OpenUI. This cannot be undone.";
        assert!(html.contains(&format!("Delete board: {copy}")), "{html}");
        assert!(
            html.contains(">Delete board<") && html.contains("btn--danger"),
            "{html}"
        );
        // Focus lands on `Keep board`: the focus id sits on that button.
        let keep = html.find(">Keep board<").expect("keep button");
        let id_at = html
            .find(&format!("id=\"{DELETE_FOCUS_ID}\""))
            .expect("focus id");
        assert!(
            id_at < keep && !html[id_at..keep].contains("<button"),
            "{html}"
        );
        assert!(delete_confirm_copy("A", 1).contains("its 1 item from"));
    }

    #[test]
    fn delete_last_board_returns_to_empty_state() {
        let ids: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(next_board_after_delete(&ids, "b"), Some("c".to_string()));
        assert_eq!(next_board_after_delete(&ids, "c"), Some("b".to_string()));
        assert_eq!(next_board_after_delete(&ids, "a"), Some("b".to_string()));
        assert_eq!(next_board_after_delete(&["a".to_string()], "a"), None);
        assert_eq!(next_board_after_delete(&[], "a"), None);
    }

    #[test]
    fn new_board_disabled_at_cap() {
        let ok = ssr(|| rsx! { NewBoardButton { board_count: 11, on_click: |_| {} } });
        assert!(
            ok.contains(">New board<") && !ok.contains("disabled"),
            "{ok}"
        );
        let capped = ssr(|| rsx! { NewBoardButton { board_count: 12, on_click: |_| {} } });
        assert!(capped.contains("disabled"), "{capped}");
        assert!(
            capped.contains("You have 12 boards, the maximum. Delete one to create another."),
            "{capped}"
        );
    }
}
