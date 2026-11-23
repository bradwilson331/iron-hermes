//! Phase 54.10 Plan 12 (D-14, D-15): Plan with AI.
//!
//! Two steps and one explicit spend point. Step 1 sends ONE planning turn (a plain chat turn,
//! never an OpenUI turn); its reply is parsed as data by `dashboard_model::parse_plan`. Step 2 is
//! an editable review. Only `Generate {n} widgets` / `Add to board` places anything: native tiles
//! and notes at once, one pending view and one `Rerun` job per AI widget (T-54.10-38/39).

use super::SAVE_FAILED;
use super::canvas::BoardState;
use crate::canvas_math::{flow_pack, snap};
use crate::components::hermes_app::dashboard_runtime::{
    DashboardGenCtx, GenJob, GenKind, GenStatus, PLAN_ITEM_ID,
};
use crate::components::hermes_app::header::focus_element_by_id;
use crate::components::hermes_app::screens::openui::{default_view_name, prompt_rows};
use crate::dashboard_model::{
    PLAN_BRIEF_MAX_CHARS, PLAN_ITEMS_MAX, PLAN_PROMPT_MAX_CHARS, PLAN_TILE_NAMES, PlanItem,
    default_tile, parse_plan, plan_prompt, tile_label,
};
use crate::server::dashboard_api::{
    BOARD_ITEMS_FULL_ERROR, BOARD_ITEMS_MAX, BoardItem, ItemKind, NOTE_TEXT_MAX_CHARS,
    item_default_size,
};
use crate::server::openui_views_api::{create_pending_openui_view, delete_openui_view};
use dioxus::prelude::*;

/// DOM id of the header `Plan with AI` button (focus returns here when the panel closes).
pub(crate) const PLAN_TRIGGER_ID: &str = "ih-plan-with-ai";
const BRIEF_ID: &str = "ih-plan-brief";
const KEEP_EDITING_ID: &str = "ih-plan-keep-editing";
/// Placed items sit this far below the existing content before snapping to the grid.
const BELOW_GAP: i32 = 32;
/// Plan items flow-pack inside this width.
const PLAN_WIDTH: i32 = 1600;

const BRIEF_PLACEHOLDER: &str =
    "For example: a weekly view of spend, failed jobs and open tasks for a two-person team.";
const DISCARD_COPY: &str =
    "Discard plan: Throw away the planned items? Nothing has been generated.";
pub(crate) const PLAN_CAP_COPY: &str = "A plan can hold at most 8 items.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Brief,
    Review,
}

/// UI-SPEC A12 failure copy; the brief is kept.
pub(crate) fn failure_copy(reason: &str) -> String {
    format!("Could not plan this board: {reason}. Your brief is kept. Try again.")
}

/// The jobs step 1 produces: exactly one planning turn and no widget turn (T-54.10-39).
pub(crate) fn plan_jobs(brief: &str) -> Vec<GenJob> {
    vec![GenJob {
        item_id: PLAN_ITEM_ID.to_string(),
        view_id: String::new(),
        message: plan_prompt(brief),
        kind: GenKind::Plan,
    }]
}

/// The prompts of the AI items, in order: one pending view each.
pub(crate) fn ai_prompts(items: &[PlanItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            PlanItem::Ai { prompt } => Some(prompt.clone()),
            _ => None,
        })
        .collect()
}

/// The new items for the current board, flow-packed (16px gaps) below the existing content and
/// starting at its left edge. `view_ids` are the pending views of the AI items, in order.
pub(crate) fn place_plan(
    existing: &[BoardItem],
    items: &[PlanItem],
    view_ids: &[String],
) -> Vec<BoardItem> {
    let mut views = view_ids.iter();
    let kinds: Vec<ItemKind> = items
        .iter()
        .filter_map(|i| match i {
            PlanItem::Ai { .. } => views
                .next()
                .map(|v| ItemKind::Widget { view_id: v.clone() }),
            PlanItem::Tile(t) => Some(ItemKind::Tile { tile: t.clone() }),
            PlanItem::Note { text } => Some(ItemKind::Note { text: text.clone() }),
        })
        .collect();
    let x0 = existing.iter().map(|i| i.x).min().map_or(0, snap);
    let y0 = existing
        .iter()
        .map(|i| i.y + i.h)
        .max()
        .map_or(0, |bottom| snap(bottom + BELOW_GAP));
    let sizes: Vec<(i32, i32)> = kinds.iter().map(item_default_size).collect();
    flow_pack(&sizes, (x0, y0), PLAN_WIDTH)
        .into_iter()
        .zip(kinds)
        .map(|(r, kind)| BoardItem {
            id: uuid::Uuid::new_v4().to_string(),
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            kind,
        })
        .collect()
}

/// One `Rerun` job per placed AI widget, in order, each with its plan prompt.
pub(crate) fn confirm_jobs(placed: &[BoardItem], prompts: &[String]) -> Vec<GenJob> {
    placed
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Widget { view_id } => Some((i.id.clone(), view_id.clone())),
            _ => None,
        })
        .zip(prompts)
        .map(|((item_id, view_id), prompt)| GenJob {
            item_id,
            view_id,
            message: prompt.clone(),
            kind: GenKind::Rerun,
        })
        .collect()
}

pub(crate) fn overflows(existing: usize, planned: usize) -> bool {
    existing + planned > BOARD_ITEMS_MAX
}

/// Append a row; refused at the plan cap.
pub(crate) fn add_row(rows: &mut Vec<PlanItem>, item: PlanItem) -> Result<(), &'static str> {
    if rows.len() >= PLAN_ITEMS_MAX {
        return Err(PLAN_CAP_COPY);
    }
    rows.push(item);
    Ok(())
}

/// Edit one row's prompt or note text; tile rows are read-only.
pub(crate) fn edit_row(rows: &mut [PlanItem], i: usize, text: String) {
    match rows.get_mut(i) {
        Some(PlanItem::Ai { prompt }) => {
            *prompt = text.chars().take(PLAN_PROMPT_MAX_CHARS).collect()
        }
        Some(PlanItem::Note { text: t }) => *t = text.chars().take(NOTE_TEXT_MAX_CHARS).collect(),
        _ => {}
    }
}

pub(crate) fn remove_row(rows: &mut Vec<PlanItem>, i: usize) {
    if i < rows.len() {
        rows.remove(i);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseAction {
    Close,
    ConfirmDiscard,
}

/// `Close plan` closes at once unless the review was edited.
pub(crate) fn close_action(edited: bool) -> CloseAction {
    if edited {
        CloseAction::ConfirmDiscard
    } else {
        CloseAction::Close
    }
}

/// `{n} AI widgets will run {n} agent turns.` (singular at 1); `None` when nothing is spent.
pub(crate) fn cost_line(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("1 AI widget will run 1 agent turn.".to_string()),
        n => Some(format!("{n} AI widgets will run {n} agent turns.")),
    }
}

/// The footer's primary label.
pub(crate) fn confirm_label(n: usize) -> String {
    match n {
        0 => "Add to board".to_string(),
        1 => "Generate 1 widget".to_string(),
        n => format!("Generate {n} widgets"),
    }
}

fn pill(item: &PlanItem) -> &'static str {
    match item {
        PlanItem::Ai { .. } => "AI",
        PlanItem::Tile(_) => "TILE",
        PlanItem::Note { .. } => "NOTE",
    }
}

fn capitalised(name: &str) -> String {
    let mut c = name.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect::<String>())
        .unwrap_or_default()
}

/// The inline discard confirm; focus lands on `Keep editing`, Esc keeps editing.
#[component]
fn DiscardConfirm(on_discard: EventHandler<()>, on_keep: EventHandler<()>) -> Element {
    use_effect(move || focus_element_by_id(KEEP_EDITING_ID));
    rsx! {
        div {
            class: "ih-plan__discard",
            role: "alertdialog",
            "aria-label": "Discard plan",
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Escape {
                    e.stop_propagation();
                    on_keep.call(());
                }
            },
            p { "{DISCARD_COPY}" }
            div { class: "ih-chooser__actions",
                button {
                    class: "btn btn--sm btn--danger",
                    r#type: "button",
                    onclick: move |_| on_discard.call(()),
                    "Discard plan"
                }
                button {
                    id: KEEP_EDITING_ID,
                    class: "btn btn--sm btn--ghost",
                    r#type: "button",
                    onclick: move |_| on_keep.call(()),
                    "Keep editing"
                }
            }
        }
    }
}

/// Shown when the open board changed (or was deleted) while a plan was being confirmed.
const PLAN_BOARD_CHANGED_ERROR: &str =
    "The board changed while the plan was being created. Nothing was added.";

/// The panel body with plain props, so an SSR test renders every state without a runtime.
#[component]
pub(crate) fn PlanView(
    stage: Stage,
    brief: String,
    error: Option<String>,
    planning: bool,
    rows: Vec<PlanItem>,
    /// Items already on the board.
    existing: usize,
    confirming_discard: bool,
    busy: bool,
    confirm_error: Option<String>,
    on_brief: EventHandler<String>,
    on_plan: EventHandler<()>,
    on_stop: EventHandler<()>,
    on_edit: EventHandler<(usize, String)>,
    on_remove: EventHandler<usize>,
    on_add: EventHandler<PlanItem>,
    on_back: EventHandler<()>,
    on_close: EventHandler<()>,
    on_confirm: EventHandler<()>,
    on_discard: EventHandler<()>,
    on_keep: EventHandler<()>,
) -> Element {
    let mut add_open = use_signal(|| false);
    let n_ai = rows
        .iter()
        .filter(|r| matches!(r, PlanItem::Ai { .. }))
        .count();
    let over = overflows(existing, rows.len());
    let blank_ai = rows
        .iter()
        .any(|r| matches!(r, PlanItem::Ai { prompt } if prompt.trim().is_empty()));
    let cannot_confirm = busy || rows.is_empty() || over || blank_ai;
    let at_cap = rows.len() >= PLAN_ITEMS_MAX;
    let cost = cost_line(n_ai);
    let label = confirm_label(n_ai);
    let brief_empty = brief.trim().is_empty();
    rsx! {
        match stage {
            Stage::Brief => rsx! {
                h2 { class: "panel-title", "Plan with AI" }
                p { class: "panel-sub",
                    "Describe what this board is for. The agent proposes widgets and notes. Nothing is generated until you confirm."
                }
                textarea {
                    id: BRIEF_ID,
                    class: "field-input",
                    rows: "{prompt_rows(&brief)}",
                    maxlength: "{PLAN_BRIEF_MAX_CHARS}",
                    placeholder: BRIEF_PLACEHOLDER,
                    "aria-label": "Board brief",
                    readonly: planning,
                    value: "{brief}",
                    oninput: move |e| on_brief.call(e.value()),
                    onkeydown: move |e: KeyboardEvent| {
                        let m = e.modifiers();
                        if e.key() == Key::Enter && (m.ctrl() || m.meta()) && !planning {
                            e.prevent_default();
                            on_plan.call(());
                        }
                    },
                }
                if let Some(e) = error {
                    p { class: "ih-plan__error", role: "alert", "{e}" }
                }
                div { class: "ih-chooser__actions",
                    if planning {
                        span { class: "meta", role: "status", "Planning…" }
                        button {
                            class: "btn btn--sm btn--ghost",
                            r#type: "button",
                            onclick: move |_| on_stop.call(()),
                            "Stop"
                        }
                    } else {
                        button {
                            class: "btn",
                            r#type: "button",
                            disabled: brief_empty,
                            onclick: move |_| on_plan.call(()),
                            "Plan board"
                        }
                        span { class: "help", "Planning uses 1 agent turn." }
                    }
                    button {
                        class: "btn btn--sm btn--ghost",
                        r#type: "button",
                        disabled: busy,
                        onclick: move |_| on_close.call(()),
                        "Close plan"
                    }
                }
            },
            Stage::Review => rsx! {
                h2 { class: "panel-title", "Review the plan" }
                p { class: "panel-sub",
                    "Edit, remove or add items. Native tiles and notes are free. Each AI widget runs 1 agent turn."
                }
                ul { class: "ih-plan__rows",
                    for (i , row) in rows.iter().cloned().enumerate() {
                        li { key: "{i}", class: "ih-plan__row",
                            span { class: "pill", "{pill(&row)}" }
                            match row {
                                PlanItem::Ai { prompt } => rsx! {
                                    textarea {
                                        class: "field-input",
                                        rows: "2",
                                        maxlength: "{PLAN_PROMPT_MAX_CHARS}",
                                        "aria-label": "AI widget prompt",
                                        value: "{prompt}",
                                        oninput: move |e| on_edit.call((i, e.value())),
                                    }
                                },
                                PlanItem::Note { text } => rsx! {
                                    textarea {
                                        class: "field-input",
                                        rows: "2",
                                        maxlength: "{NOTE_TEXT_MAX_CHARS}",
                                        "aria-label": "Note text",
                                        value: "{text}",
                                        oninput: move |e| on_edit.call((i, e.value())),
                                    }
                                },
                                PlanItem::Tile(t) => {
                                    let (name, setting) = tile_label(&t);
                                    rsx! {
                                        span { class: "ih-plan__tile",
                                            "{name}"
                                            if !setting.is_empty() {
                                                span { class: "help", " · {setting}" }
                                            }
                                        }
                                    }
                                }
                            }
                            button {
                                class: "btn btn--sm btn--ghost",
                                r#type: "button",
                                "aria-label": "Remove item",
                                onclick: move |_| on_remove.call(i),
                                "✕"
                            }
                        }
                    }
                }
                div { class: "ih-plan__add",
                    button {
                        class: "btn btn--sm btn--ghost",
                        r#type: "button",
                        disabled: at_cap,
                        "aria-expanded": "{*add_open.read()}",
                        onclick: move |_| add_open.toggle(),
                        "Add item ▾"
                    }
                    if at_cap {
                        span { class: "help", "{PLAN_CAP_COPY}" }
                    }
                    if *add_open.read() && !at_cap {
                        div { class: "panel ih-plan__menu", role: "menu",
                            button {
                                class: "item",
                                r#type: "button",
                                role: "menuitem",
                                onclick: move |_| {
                                    add_open.set(false);
                                    on_add.call(PlanItem::Ai { prompt: String::new() });
                                },
                                "AI widget"
                            }
                            button {
                                class: "item",
                                r#type: "button",
                                role: "menuitem",
                                onclick: move |_| {
                                    add_open.set(false);
                                    on_add.call(PlanItem::Note { text: String::new() });
                                },
                                "Note"
                            }
                            for name in PLAN_TILE_NAMES {
                                button {
                                    key: "{name}",
                                    class: "item",
                                    r#type: "button",
                                    role: "menuitem",
                                    onclick: move |_| {
                                        add_open.set(false);
                                        if let Some(t) = default_tile(name) {
                                            on_add.call(PlanItem::Tile(t));
                                        }
                                    },
                                    "{capitalised(name)}"
                                }
                            }
                        }
                    }
                }
                if over {
                    p { class: "ih-plan__error", role: "alert", "{BOARD_ITEMS_FULL_ERROR}" }
                }
                if let Some(e) = confirm_error {
                    p { class: "ih-plan__error", role: "alert", "{e}" }
                }
                if let Some(c) = cost {
                    p { class: "help", "{c}" }
                }
                if confirming_discard {
                    DiscardConfirm { on_discard, on_keep }
                } else {
                    div { class: "ih-chooser__actions",
                        button {
                            class: "btn ih-plan__confirm",
                            r#type: "button",
                            disabled: cannot_confirm,
                            "aria-busy": if busy { "true" },
                            onclick: move |_| on_confirm.call(()),
                            "{label}"
                        }
                        button {
                            class: "btn btn--sm btn--ghost",
                            r#type: "button",
                            disabled: busy,
                            onclick: move |_| on_back.call(()),
                            "Back to brief"
                        }
                        button {
                            class: "btn btn--sm btn--ghost",
                            r#type: "button",
                            disabled: busy,
                            onclick: move |_| on_close.call(()),
                            "Close plan"
                        }
                    }
                }
            },
        }
    }
}

/// The docked panel. Mounted by the Dashboard only while open. Closing it while the planning turn
/// runs stops that turn (nobody would be waiting for its result), and drops any unread result, so
/// a later panel never opens on a plan from an earlier brief or board. Widget jobs a confirmed
/// plan enqueued are not touched (the runtime is root-owned).
#[component]
pub(crate) fn PlanPanel(
    st: BoardState,
    /// Owned by the screen so the header toggle can see a confirm in flight: closing the panel
    /// cancels the confirm task (it is scoped to this component) and orphans its pending views.
    busy: Signal<bool>,
    on_close: EventHandler<()>,
) -> Element {
    let ctx = use_context::<DashboardGenCtx>();
    let mut stage = use_signal(|| Stage::Brief);
    let mut brief = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut rows = use_signal(Vec::<PlanItem>::new);
    let mut original = use_signal(Vec::<PlanItem>::new);
    let mut discarding = use_signal(|| false);
    let mut busy = busy;
    let mut confirm_error = use_signal(|| None::<String>);

    // The planning turn's result arrives on the root context; take it and parse it as data.
    use_effect(move || {
        let Some(result) = ctx.plan_result.read().clone() else {
            return;
        };
        let mut plan_result = ctx.plan_result;
        plan_result.set(None);
        match result.and_then(|text| parse_plan(&text)) {
            Ok(items) => {
                original.set(items.clone());
                rows.set(items);
                error.set(None);
                stage.set(Stage::Review);
            }
            Err(reason) => {
                error.set(Some(failure_copy(&reason)));
                stage.set(Stage::Brief);
            }
        }
    });

    // C-07: the planning turn belongs to this panel instance.
    dioxus::core::use_drop(move || {
        let running = matches!(
            ctx.statuses.peek().get(PLAN_ITEM_ID),
            Some(GenStatus::Queued | GenStatus::Generating)
        );
        if running {
            ctx.stop.call(PLAN_ITEM_ID.to_string());
        }
        let mut plan_result = ctx.plan_result;
        plan_result.set(None);
    });

    let planning = matches!(
        ctx.statuses.read().get(PLAN_ITEM_ID),
        Some(GenStatus::Queued | GenStatus::Generating)
    );
    let existing = st.board.read().as_ref().map_or(0, |b| b.items.len());
    let edited = *stage.read() == Stage::Review && *rows.read() != *original.read();

    rsx! {
        section {
            class: "panel ih-plan",
            "aria-label": "Plan with AI",
            // Keys typed here are not the canvas's.
            onkeydown: move |e| e.stop_propagation(),
            PlanView {
                stage: *stage.read(),
                brief: brief.read().clone(),
                error: error.read().clone(),
                planning,
                rows: rows.read().clone(),
                existing,
                confirming_discard: *discarding.read(),
                busy: *busy.read(),
                confirm_error: confirm_error.read().clone(),
                on_brief: move |v: String| brief.set(v),
                on_plan: move |_| {
                    if planning || brief.peek().trim().is_empty() {
                        return;
                    }
                    error.set(None);
                    let mut plan_result = ctx.plan_result;
                    plan_result.set(None);
                    for job in plan_jobs(&brief.peek()) {
                        ctx.enqueue.call(job);
                    }
                },
                on_stop: move |_| ctx.stop.call(PLAN_ITEM_ID.to_string()),
                on_edit: move |(i, text): (usize, String)| edit_row(&mut rows.write(), i, text),
                on_remove: move |i: usize| remove_row(&mut rows.write(), i),
                on_add: move |item: PlanItem| {
                    let _ = add_row(&mut rows.write(), item);
                },
                on_back: move |_| {
                    if *busy.peek() {
                        return;
                    }
                    discarding.set(false);
                    stage.set(Stage::Brief);
                },
                on_close: move |_| match close_action(edited) {
                    _ if *busy.peek() => {}
                    CloseAction::Close => on_close.call(()),
                    CloseAction::ConfirmDiscard => discarding.set(true),
                },
                on_confirm: move |_| {
                    if *busy.peek() {
                        return;
                    }
                    let items = rows.peek().clone();
                    if items.is_empty() || overflows(st.items().len(), items.len()) {
                        return;
                    }
                    busy.set(true);
                    confirm_error.set(None);
                    // The panel survives a board switch, so the board this plan was made for is
                    // compared again after the awaits (C-04).
                    let board_id = st.board.peek().as_ref().map(|b| b.id.clone());
                    spawn(async move {
                        let prompts = ai_prompts(&items);
                        let mut view_ids: Vec<String> = Vec::new();
                        let mut failed = false;
                        for p in &prompts {
                            match create_pending_openui_view(default_view_name(p), p.clone()).await {
                                Ok(v) => view_ids.push(v.id),
                                Err(_) => {
                                    failed = true;
                                    break;
                                }
                            }
                        }
                        // The board may have changed while the views were saved: place against
                        // what is there now.
                        let existing = st.items();
                        let moved = st.board.peek().as_ref().map(|b| b.id.clone()) != board_id;
                        if failed || moved || overflows(existing.len(), items.len()) {
                            for id in view_ids {
                                let _ = delete_openui_view(id).await;
                            }
                            confirm_error.set(Some(if failed {
                                SAVE_FAILED.to_string()
                            } else if moved {
                                PLAN_BOARD_CHANGED_ERROR.to_string()
                            } else {
                                BOARD_ITEMS_FULL_ERROR.to_string()
                            }));
                            busy.set(false);
                            return;
                        }
                        let placed = place_plan(&existing, &items, &view_ids);
                        let jobs = confirm_jobs(&placed, &prompts);
                        let n = placed.len();
                        st.edit(|b| b.extend(placed));
                        let mut views_rev = ctx.views_rev;
                        views_rev += 1;
                        st.say(if n == 1 {
                            "1 item added to the board.".to_string()
                        } else {
                            format!("{n} items added to the board.")
                        });
                        // The single place a plan spends: one Rerun per AI widget, one by one.
                        for job in jobs {
                            ctx.enqueue.call(job);
                        }
                        busy.set(false);
                        on_close.call(());
                    });
                },
                on_discard: move |_| {
                    discarding.set(false);
                    on_close.call(());
                },
                on_keep: move |_| discarding.set(false),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::dashboard_api::{TileSpec, UsageWindow};

    fn existing_note(y: i32, h: i32) -> BoardItem {
        BoardItem {
            id: "e".into(),
            x: 64,
            y,
            w: 224,
            h,
            kind: ItemKind::Note {
                text: String::new(),
            },
        }
    }

    fn sample() -> Vec<PlanItem> {
        vec![
            PlanItem::Ai {
                prompt: "failed jobs".into(),
            },
            PlanItem::Tile(TileSpec::Usage {
                window: UsageWindow::Week,
            }),
            PlanItem::Ai {
                prompt: "spend".into(),
            },
            PlanItem::Note {
                text: "goals".into(),
            },
        ]
    }

    #[test]
    fn plan_step_one_enqueues_no_widget_jobs() {
        let jobs = plan_jobs("a spend board");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, GenKind::Plan);
        assert_eq!(jobs[0].item_id, PLAN_ITEM_ID);
        assert!(jobs[0].message.ends_with("a spend board"));
        assert!(jobs.iter().all(|j| j.kind != GenKind::Rerun));
        // The only places that create views or Rerun jobs are the confirm path.
        const SRC: &str = include_str!("plan.rs");
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        assert_eq!(
            code.matches(concat!("create_pending_openui_view", "("))
                .count(),
            1
        );
        assert_eq!(code.matches(concat!("GenKind::", "Rerun,")).count(), 1);
    }

    #[test]
    fn plan_confirm_enqueues_exactly_n_ai_jobs() {
        let items = sample();
        let prompts = ai_prompts(&items);
        assert_eq!(prompts.len(), 2, "two pending views are created");
        let views = vec!["v1".to_string(), "v2".to_string()];
        let placed = place_plan(&[], &items, &views);
        assert_eq!(placed.len(), 4);
        let jobs = confirm_jobs(&placed, &prompts);
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|j| j.kind == GenKind::Rerun));
        assert_eq!(
            (jobs[0].view_id.as_str(), jobs[0].message.as_str()),
            ("v1", "failed jobs")
        );
        assert_eq!(
            (jobs[1].view_id.as_str(), jobs[1].message.as_str()),
            ("v2", "spend")
        );
        let ids: std::collections::HashSet<_> = placed.iter().map(|i| i.id.clone()).collect();
        assert_eq!(ids.len(), 4);
        assert!(jobs.iter().all(|j| ids.contains(&j.item_id)));
    }

    #[test]
    fn plan_confirm_places_below_existing_content() {
        let existing = [existing_note(160, 240)];
        let placed = place_plan(&existing, &sample(), &["a".into(), "b".into()]);
        assert_eq!(placed.len(), 4);
        for i in &placed {
            assert!(i.y >= 432, "{i:?}");
            assert_eq!((i.x % 16, i.y % 16, i.w % 16, i.h % 16), (0, 0, 0, 0));
            assert!(i.x >= 64, "starts at the leftmost existing x");
        }
        assert_eq!(placed[0].x, 64);
        for (n, a) in placed.iter().enumerate() {
            for b in &placed[n + 1..] {
                assert!(
                    a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y
                );
            }
        }
        // An empty board starts at the origin.
        let fresh = place_plan(&[], &sample(), &["a".into(), "b".into()]);
        assert_eq!((fresh[0].x, fresh[0].y), (0, 0));
    }

    #[test]
    fn review_rows_edit_remove_add() {
        let mut rows = sample();
        edit_row(&mut rows, 0, "new prompt".into());
        assert_eq!(
            rows[0],
            PlanItem::Ai {
                prompt: "new prompt".into()
            }
        );
        assert_eq!(rows[2], sample()[2], "only that row changes");
        edit_row(&mut rows, 1, "ignored".into());
        assert_eq!(rows[1], sample()[1], "tile rows are read-only");
        remove_row(&mut rows, 1);
        assert_eq!(rows.len(), 3);
        for _ in 0..PLAN_ITEMS_MAX {
            let _ = add_row(
                &mut rows,
                PlanItem::Note {
                    text: String::new(),
                },
            );
        }
        assert_eq!(rows.len(), PLAN_ITEMS_MAX);
        assert_eq!(
            add_row(&mut rows, PlanItem::Tile(TileSpec::Cron {})),
            Err("A plan can hold at most 8 items.")
        );
        assert_eq!(rows.len(), PLAN_ITEMS_MAX);
    }

    fn view(
        stage: Stage,
        rows: Vec<PlanItem>,
        existing: usize,
        discarding: bool,
        error: Option<String>,
        brief: &str,
    ) -> String {
        let brief = brief.to_string();
        let mut dom = VirtualDom::new_with_props(
            |p: (Stage, Vec<PlanItem>, usize, bool, Option<String>, String)| {
                let (stage, rows, existing, discarding, error, brief) = p;
                rsx! {
                    PlanView {
                        stage,
                        brief,
                        error,
                        planning: false,
                        rows,
                        existing,
                        confirming_discard: discarding,
                        busy: false,
                        confirm_error: None,
                        on_brief: |_| {},
                        on_plan: |_| {},
                        on_stop: |_| {},
                        on_edit: |_| {},
                        on_remove: |_| {},
                        on_add: |_| {},
                        on_back: |_| {},
                        on_close: |_| {},
                        on_confirm: |_| {},
                        on_discard: |_| {},
                        on_keep: |_| {},
                    }
                }
            },
            (stage, rows, existing, discarding, error, brief),
        );
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn close_plan_with_edits_opens_discard_confirm() {
        assert_eq!(close_action(false), CloseAction::Close);
        assert_eq!(close_action(true), CloseAction::ConfirmDiscard);
        let html = view(Stage::Review, sample(), 0, true, None, "");
        assert!(
            html.contains(
                "Discard plan: Throw away the planned items? Nothing has been generated."
            ),
            "{html}"
        );
        assert!(
            html.contains(">Discard plan<") && html.contains(">Keep editing<"),
            "{html}"
        );
        assert!(html.contains("btn--danger"), "{html}");
        let plain = view(Stage::Review, sample(), 0, false, None, "");
        assert!(!plain.contains("Keep editing"), "{plain}");
        assert!(plain.contains(">Close plan<"), "{plain}");
    }

    #[test]
    fn close_and_back_are_disabled_while_a_confirm_is_in_flight() {
        let mut dom = VirtualDom::new(|| {
            rsx! {
                PlanView {
                    stage: Stage::Review,
                    brief: String::new(),
                    error: None,
                    planning: false,
                    rows: vec![PlanItem::Note { text: "n".into() }],
                    existing: 0,
                    confirming_discard: false,
                    busy: true,
                    confirm_error: None,
                    on_brief: |_| {},
                    on_plan: |_| {},
                    on_stop: |_| {},
                    on_edit: |_| {},
                    on_remove: |_| {},
                    on_add: |_| {},
                    on_back: |_| {},
                    on_close: |_| {},
                    on_confirm: |_| {},
                    on_discard: |_| {},
                    on_keep: |_| {},
                }
            }
        });
        dom.rebuild_in_place();
        let html = dioxus::ssr::render(&dom);
        for label in [">Close plan<", ">Back to brief<"] {
            let end = html.find(label).expect(label);
            let tag_start = html[..end].rfind("<button").unwrap();
            assert!(html[tag_start..end].contains("disabled"), "{label}: {html}");
        }
    }

    #[test]
    fn zero_ai_items_button_reads_add_to_board() {
        let rows = vec![
            PlanItem::Tile(TileSpec::Gateway {}),
            PlanItem::Note { text: "n".into() },
        ];
        let html = view(Stage::Review, rows, 0, false, None, "");
        assert!(html.contains(">Add to board<"), "{html}");
        assert!(!html.contains("will run"), "no cost line: {html}");
        let with_ai = view(Stage::Review, sample(), 0, false, None, "");
        assert!(with_ai.contains(">Generate 2 widgets<"), "{with_ai}");
        assert!(
            with_ai.contains("2 AI widgets will run 2 agent turns."),
            "{with_ai}"
        );
    }

    #[test]
    fn confirm_disabled_when_board_would_overflow() {
        let rows: Vec<PlanItem> = (0..5)
            .map(|i| PlanItem::Note {
                text: format!("n{i}"),
            })
            .collect();
        let html = view(Stage::Review, rows.clone(), 36, false, None, "");
        let at = html.find("ih-plan__confirm").expect("confirm button");
        let tag = &html[at..at + html[at..].find('>').unwrap()];
        assert!(tag.contains("disabled"), "{tag}");
        assert!(
            html.contains("This board is full (40 items). Remove one to add another."),
            "{html}"
        );
        let ok = view(Stage::Review, rows, 35, false, None, "");
        let at = ok.find("ih-plan__confirm").unwrap();
        assert!(
            !ok[at..at + ok[at..].find('>').unwrap()].contains("disabled"),
            "{ok}"
        );
    }

    #[test]
    fn plan_failure_keeps_brief() {
        let copy = failure_copy("the reply was not valid JSON");
        assert_eq!(
            copy,
            "Could not plan this board: the reply was not valid JSON. Your brief is kept. Try again."
        );
        let html = view(
            Stage::Brief,
            vec![],
            0,
            false,
            Some(copy.clone()),
            "my board brief",
        );
        assert!(html.contains(&copy), "{html}");
        assert!(html.contains("my board brief"), "{html}");
        assert!(html.contains("Planning uses 1 agent turn."), "{html}");
    }

    #[test]
    fn plan_board_is_disabled_on_an_empty_brief() {
        let empty = view(Stage::Brief, vec![], 0, false, None, "   ");
        let at = empty.find(">Plan board<").unwrap();
        assert!(
            empty[..at]
                .rsplit("<button")
                .next()
                .unwrap()
                .contains("disabled"),
            "{empty}"
        );
        let full = view(Stage::Brief, vec![], 0, false, None, "x");
        let at = full.find(">Plan board<").unwrap();
        assert!(
            !full[..at]
                .rsplit("<button")
                .next()
                .unwrap()
                .contains("disabled"),
            "{full}"
        );
    }
}
