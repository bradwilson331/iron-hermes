//! Phase 54.9 Plan 04 (D-01/D-02/D-03/D-05/D-06/D-08): the Settings > Menu tab.
//!
//! Two lists (Wheel, Header shortcuts) edited with the keyboard-first controls of
//! UI-SPEC A1. Every committed edit writes `MenuCtx.layout` first (so the docked
//! wheel and the header change at once) and persists second through
//! `set_menu_layout`, with saves coalesced so an older layout can never land
//! after a newer one. There is no draft: nothing here is held back for a Save
//! button, and there is no hide control and no drag-and-drop (D-03, A1).
//!
//! Every label is rendered as a text node or an input value, never as HTML.

use crate::components::hermes_app::header::{focus_element_by_id, HEADER_INLINE_MAX};
use crate::components::hermes_app::MenuCtx;
use crate::menu_layout::{
    catalog_entry, default_layout, is_locked, move_entry, rename_entry, reorder, MenuEntry,
    MenuLayout, MenuList, MoveBlocked, MoveDir, RenameResult, WHEEL_MAX, WHEEL_MIN,
};
use dioxus::prelude::*;

pub(crate) const INTRO: &str =
    "Changes show in the header right away and are saved to this profile.";
pub(crate) const WHEEL_TITLE: &str = "Wheel";
pub(crate) const HEADER_TITLE: &str = "Header shortcuts";
pub(crate) const DIVIDER: &str = "Below this line: shown in the MORE menu";
pub(crate) const LOCKED_PILL: &str = "LOCKED";
pub(crate) const LOCKED_TITLE: &str =
    "Stays on the wheel so you can always reach Settings and Chat.";
pub(crate) const MOVE_TO_HEADER: &str = "Move to header";
pub(crate) const MOVE_TO_WHEEL: &str = "Move to wheel";
pub(crate) const BOUND_MIN: &str =
    "The wheel needs at least 6 sections. Move another entry onto it first.";
pub(crate) const BOUND_MAX: &str =
    "The wheel holds at most 11 sections. Move one to the header first.";
pub(crate) const RENAME_ERROR: &str = "Label must be 1 to 9 characters.";
pub(crate) const RESET_BUTTON: &str = "Reset to default";
/// UI-SPEC Reset confirmation, with the counts read from the shipped default so a catalog
/// addition (Phase 54.9 Plan 09: OpenUI) cannot leave the copy stale.
pub(crate) fn reset_confirm() -> String {
    let d = crate::menu_layout::default_layout();
    format!(
        "Reset to default: Replace your layout and labels with the shipped one ({} wheel sections, {} header shortcuts)?",
        d.wheel.len(),
        d.header.len()
    )
}
pub(crate) const RESET_YES: &str = "Reset layout";
pub(crate) const RESET_NO: &str = "Keep my layout";
pub(crate) const RESET_DONE: &str = "Layout reset to default.";
pub(crate) const SAVING: &str = "Saving…";
pub(crate) const SAVED: &str = "Saved.";
pub(crate) const SAVE_FAILED: &str =
    "Save failed. Check server logs. The new layout is active until you restart.";
pub(crate) const WRITES_DISABLED: &str = "Config writes are disabled.";
pub(crate) const WRITES_DISABLED_HINT: &str =
    "Set security.web_config_write_enabled: true in config.yaml, then retry.";

/// DOM id prefix of every focusable control in the editor.
const ID_PREFIX: &str = "ih-menuedit-";
const RESET_ID: &str = "ih-menuedit-reset";
const KEEP_ID: &str = "ih-menuedit-keep";

/// Where the last save attempt stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SaveStatus {
    Idle,
    Saving,
    Saved,
    Failed,
    Disabled,
}

/// One save in flight plus one trailing latest (T-54.9-08).
#[derive(Clone, Copy)]
pub struct SaveQueue {
    in_flight: Signal<bool>,
    again: Signal<bool>,
    status: Signal<SaveStatus>,
}

/// RR-01: called once from `HermesApp`'s body (root scope), so the signals are root-owned and
/// outlive the Menu tab; the `spawn_forever` save loop can still write them after it unmounts.
pub(crate) fn use_save_queue() -> SaveQueue {
    SaveQueue {
        in_flight: use_signal(|| false),
        again: use_signal(|| false),
        status: use_signal(|| SaveStatus::Idle),
    }
}

/// The live preview: the docked wheel and header re-render from this signal.
pub(crate) fn apply_edit(ctx: MenuCtx, next: MenuLayout) {
    let mut layout = ctx.layout;
    layout.set(next);
}

/// Persist the current live layout. A request made while a save is in flight
/// only raises `again`; the running loop then sends the latest layout once more,
/// so saves reach the server in order and the last one wins. A failed write
/// never rolls the live layout back.
pub(crate) fn request_save(ctx: MenuCtx, q: SaveQueue) {
    let SaveQueue {
        mut in_flight,
        mut again,
        mut status,
    } = q;
    if *in_flight.peek() {
        again.set(true);
        return;
    }
    in_flight.set(true);
    status.set(SaveStatus::Saving);
    let mut notice = ctx.notice;
    // WR-05: on the ROOT scope, so leaving the Menu tab mid-save does not cancel the trailing
    // save of the latest layout. The queue signals are root-owned too (see `MenuSettingsPanel`).
    dioxus::core::spawn_forever(async move {
        let outcome = loop {
            again.set(false);
            let layout = ctx.layout.peek().clone();
            let result = crate::server::menu_api::set_menu_layout(layout).await;
            let outcome = match result {
                Ok(()) => SaveStatus::Saved,
                Err(e)
                    if e.to_string()
                        .contains(crate::server::menu_api::MENU_WRITE_DISABLED) =>
                {
                    SaveStatus::Disabled
                }
                Err(_) => SaveStatus::Failed,
            };
            if !*again.peek() {
                break outcome;
            }
        };
        in_flight.set(false);
        if outcome == SaveStatus::Saved {
            notice.set(None);
        }
        status.set(outcome);
    });
}

fn list_word(list: MenuList) -> &'static str {
    match list {
        MenuList::Wheel => "wheel",
        MenuList::Header => "header",
    }
}

/// `{label} moved to position {pos} of {n} on the {wheel/header}.` (`pos` is 1-based).
pub(crate) fn announce_reorder(label: &str, pos: usize, n: usize, list: MenuList) -> String {
    format!(
        "{label} moved to position {pos} of {n} on the {}.",
        list_word(list)
    )
}

/// `{label} moved to the {wheel/header}.` (`list` is the destination).
pub(crate) fn announce_move(label: &str, list: MenuList) -> String {
    format!("{label} moved to the {}.", list_word(list))
}

/// Everything an editor handler needs; all `Copy`.
#[derive(Clone, Copy)]
struct PanelState {
    ctx: MenuCtx,
    q: SaveQueue,
    announce: Signal<String>,
    pending_focus: Signal<Option<String>>,
    last_moved: Signal<Option<String>>,
    label_err: Signal<Option<String>>,
    /// Bumped to remount the label inputs so they show the committed label again.
    rev: Signal<u32>,
}

fn entries_of(layout: &MenuLayout, list: MenuList) -> &[MenuEntry] {
    match list {
        MenuList::Wheel => &layout.wheel,
        MenuList::Header => &layout.header,
    }
}

fn arrow_id(dir: MoveDir, list: MenuList, screen: &str) -> String {
    let d = match dir {
        MoveDir::Up => "up",
        MoveDir::Down => "down",
    };
    format!("{ID_PREFIX}{d}-{}-{screen}", list_word(list))
}

fn move_id(list: MenuList, screen: &str) -> String {
    format!("{ID_PREFIX}move-{}-{screen}", list_word(list))
}

fn label_id(screen: &str) -> String {
    format!("{ID_PREFIX}label-{screen}")
}

fn bump(mut sig: Signal<u32>) {
    let next = *sig.peek() + 1;
    sig.set(next);
}

/// After a reorder: announce, mark the row, and keep focus on the same arrow of
/// the moved row (or the other arrow when this one just became disabled).
fn after_reorder(st: PanelState, list: MenuList, index: usize, dir: MoveDir, screen: &str) {
    let new_pos = match dir {
        MoveDir::Up => index.saturating_sub(1),
        MoveDir::Down => index + 1,
    };
    let (label, n) = {
        let layout = st.ctx.layout.peek();
        let entries = entries_of(&layout, list);
        (
            entries
                .get(new_pos)
                .map(|e| e.label.clone())
                .unwrap_or_default(),
            entries.len(),
        )
    };
    let focus_dir = match dir {
        MoveDir::Up if new_pos == 0 => MoveDir::Down,
        MoveDir::Down if new_pos + 1 >= n => MoveDir::Up,
        d => d,
    };
    let (mut announce, mut pending_focus, mut last_moved, mut label_err) =
        (st.announce, st.pending_focus, st.last_moved, st.label_err);
    announce.set(announce_reorder(&label, new_pos + 1, n, list));
    pending_focus.set(Some(arrow_id(focus_dir, list, screen)));
    last_moved.set(Some(screen.to_string()));
    label_err.set(None);
}

fn commit_rename(st: PanelState, list: MenuList, screen: &str, raw: &str) {
    let cur = st.ctx.layout.peek().clone();
    let Some(index) = entries_of(&cur, list)
        .iter()
        .position(|e| e.screen == screen)
    else {
        return;
    };
    let (mut announce, mut last_moved, mut label_err) = (st.announce, st.last_moved, st.label_err);
    match rename_entry(&cur, list, index, raw) {
        Ok(result) => {
            label_err.set(None);
            last_moved.set(None);
            let next = match result {
                RenameResult::Renamed(next) => next,
                RenameResult::Restored(next, shipped) => {
                    announce.set(format!("Label restored to {shipped}."));
                    // The field text was emptied by the operator; remount it so it shows
                    // the restored label even when that equals the previous one.
                    bump(st.rev);
                    next
                }
            };
            if next != cur {
                apply_edit(st.ctx, next);
                request_save(st.ctx, st.q);
            }
        }
        Err(_) => label_err.set(Some(screen.to_string())),
    }
}

fn do_move(st: PanelState, from: MenuList, screen: &str) {
    let cur = st.ctx.layout.peek().clone();
    let Some(index) = entries_of(&cur, from)
        .iter()
        .position(|e| e.screen == screen)
    else {
        return;
    };
    let Ok(next) = move_entry(&cur, from, index) else {
        return;
    };
    let dest = match from {
        MenuList::Wheel => MenuList::Header,
        MenuList::Header => MenuList::Wheel,
    };
    let label = entries_of(&cur, from)[index].label.clone();
    apply_edit(st.ctx, next);
    request_save(st.ctx, st.q);
    let (mut announce, mut pending_focus, mut last_moved, mut label_err) =
        (st.announce, st.pending_focus, st.last_moved, st.label_err);
    announce.set(announce_move(&label, dest));
    pending_focus.set(Some(move_id(dest, screen)));
    last_moved.set(Some(screen.to_string()));
    label_err.set(None);
}

fn status_line(status: SaveStatus) -> Element {
    match status {
        SaveStatus::Idle | SaveStatus::Disabled => rsx! {},
        SaveStatus::Saving => {
            rsx! { span { style: "color:var(--gray);font-size:11px;", "{SAVING}" } }
        }
        SaveStatus::Saved => {
            rsx! { span { style: "color:var(--green);font-size:11px;", "{SAVED}" } }
        }
        SaveStatus::Failed => {
            rsx! { p { style: "color:var(--red);font-size:11px;margin:0;", "{SAVE_FAILED}" } }
        }
    }
}

fn write_disabled_copy() -> Element {
    rsx! {
        p { class: "help", style: "color:var(--gray);", "{WRITES_DISABLED}" }
        p { class: "help", style: "color:var(--gray);", "{WRITES_DISABLED_HINT}" }
    }
}

/// The Reset control: a button, or the inline two-step confirm (no modal).
pub(crate) fn render_reset_area(
    confirming: bool,
    on_start: Callback<()>,
    on_confirm: Callback<()>,
    on_cancel: Callback<()>,
) -> Element {
    rsx! {
        if confirming {
            div {
                class: "ih-menuedit-confirm",
                onkeydown: move |evt: KeyboardEvent| {
                    if evt.key() == Key::Escape {
                        on_cancel.call(());
                    }
                },
                p { class: "help", style: "margin:0;", "{reset_confirm()}" }
                div { class: "ih-menuedit-actions",
                    button {
                        id: "ih-menuedit-confirm",
                        class: "btn btn--sm btn--danger",
                        r#type: "button",
                        onclick: move |_| on_confirm.call(()),
                        "{RESET_YES}"
                    }
                    button {
                        id: KEEP_ID,
                        class: "btn btn--sm btn--ghost",
                        r#type: "button",
                        onclick: move |_| on_cancel.call(()),
                        "{RESET_NO}"
                    }
                }
            }
        } else {
            button {
                id: RESET_ID,
                class: "btn btn--sm btn--ghost",
                r#type: "button",
                onclick: move |_| on_start.call(()),
                "{RESET_BUTTON}"
            }
        }
    }
}

/// Wasm: is DOM focus still on one of the editor's own controls?
#[cfg(target_arch = "wasm32")]
fn focus_inside_menu_list() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element())
        .is_some_and(|e| e.id().starts_with(ID_PREFIX))
}

/// Native twin: there is no DOM focus, so the highlight is never cleared by blur.
#[cfg(not(target_arch = "wasm32"))]
fn focus_inside_menu_list() -> bool {
    true
}

/// One editor row, positioned at `index` of `count` inside `list`.
struct RowCtx<'a> {
    list: MenuList,
    index: usize,
    count: usize,
    entry: &'a MenuEntry,
    active: bool,
    blocked: Option<MoveBlocked>,
    has_err: bool,
}

fn entry_row(st: PanelState, row: RowCtx<'_>) -> Element {
    let RowCtx {
        list,
        index,
        count,
        entry,
        active,
        blocked,
        has_err,
    } = row;
    let Some(cat) = catalog_entry(&entry.screen) else {
        return rsx! {};
    };
    let screen = entry.screen.clone();
    let label = entry.label.clone();
    let rev = *st.rev.read();
    let up_id = arrow_id(MoveDir::Up, list, &screen);
    let down_id = arrow_id(MoveDir::Down, list, &screen);
    let move_btn_id = move_id(list, &screen);
    let input_id = label_id(&screen);
    let help_id = format!("{ID_PREFIX}help-{screen}");
    let err_id = format!("{ID_PREFIX}label-err-{screen}");
    let described_by = if has_err {
        err_id.clone()
    } else {
        help_id.clone()
    };
    let up_label = format!("Move {label} up");
    let down_label = format!("Move {label} down");
    let field_label = format!("Label for {}", cat.name);
    let opens = format!("Opens {}", cat.name);
    let locked = is_locked(&screen);
    let (move_text, move_disabled) = match list {
        MenuList::Wheel => (
            MOVE_TO_HEADER,
            matches!(blocked, Some(MoveBlocked::WheelAtMin)),
        ),
        MenuList::Header => (
            MOVE_TO_WHEEL,
            matches!(blocked, Some(MoveBlocked::WheelAtMax)),
        ),
    };
    let (s_up, s_down, s_move, s_label) = (
        screen.clone(),
        screen.clone(),
        screen.clone(),
        screen.clone(),
    );
    let esc_focus = input_id.clone();
    rsx! {
        div {
            key: "{screen}",
            class: "row ih-menuedit-row",
            class: if active { "is-active" },
            span { style: "color:var(--text-dim);font-size:12px;", "{cat.glyph}" }
            div { class: "row-main",
                input {
                    key: "{screen}-{rev}",
                    id: "{input_id}",
                    class: "field-input",
                    r#type: "text",
                    maxlength: "9",
                    value: "{label}",
                    "aria-label": "{field_label}",
                    "aria-describedby": "{described_by}",
                    onchange: move |evt: FormEvent| commit_rename(st, list, &s_label, &evt.value()),
                    onkeydown: move |evt: KeyboardEvent| {
                        if evt.key() == Key::Escape {
                            bump(st.rev);
                            let (mut err, mut focus) = (st.label_err, st.pending_focus);
                            err.set(None);
                            focus.set(Some(esc_focus.clone()));
                        }
                    },
                }
                span { id: "{help_id}", class: "help", "{opens}" }
                if has_err {
                    span {
                        id: "{err_id}",
                        style: "color:var(--red);font-size:11px;",
                        "{RENAME_ERROR}"
                    }
                }
            }
            div { class: "ih-menuedit-actions",
                button {
                    id: "{up_id}",
                    class: "btn btn--sm btn--icon btn--ghost",
                    r#type: "button",
                    "aria-label": "{up_label}",
                    disabled: index == 0,
                    onclick: move |_| {
                        let cur = st.ctx.layout.peek().clone();
                        if let Some(next) = reorder(&cur, list, index, MoveDir::Up) {
                            apply_edit(st.ctx, next);
                            request_save(st.ctx, st.q);
                            after_reorder(st, list, index, MoveDir::Up, &s_up);
                        }
                    },
                    "▲"
                }
                button {
                    id: "{down_id}",
                    class: "btn btn--sm btn--icon btn--ghost",
                    r#type: "button",
                    "aria-label": "{down_label}",
                    disabled: index + 1 >= count,
                    onclick: move |_| {
                        let cur = st.ctx.layout.peek().clone();
                        if let Some(next) = reorder(&cur, list, index, MoveDir::Down) {
                            apply_edit(st.ctx, next);
                            request_save(st.ctx, st.q);
                            after_reorder(st, list, index, MoveDir::Down, &s_down);
                        }
                    },
                    "▼"
                }
                if locked {
                    span { class: "pill", title: LOCKED_TITLE, "{LOCKED_PILL}" }
                } else {
                    button {
                        id: "{move_btn_id}",
                        class: "btn btn--sm btn--ghost ih-menuedit-move",
                        r#type: "button",
                        disabled: move_disabled,
                        onclick: move |_| do_move(st, list, &s_move),
                        "{move_text}"
                    }
                }
            }
        }
    }
}

fn list_panel(st: PanelState, list: MenuList, layout: &MenuLayout) -> Element {
    let entries = entries_of(layout, list);
    let n = entries.len();
    let (title, count_copy) = match list {
        MenuList::Wheel => (WHEEL_TITLE, format!("{n} of 6 to 11 sections")),
        MenuList::Header => (HEADER_TITLE, format!("{n} shortcuts")),
    };
    let bound = match list {
        MenuList::Wheel if layout.wheel.len() <= WHEEL_MIN => Some(BOUND_MIN),
        MenuList::Header if layout.wheel.len() >= WHEEL_MAX => Some(BOUND_MAX),
        _ => None,
    };
    let last_moved = st.last_moved.read().clone();
    let err_for = st.label_err.read().clone();
    let word = list_word(list);
    rsx! {
        div { class: "panel", "data-menuedit-list": "{word}",
            div { class: "panel-title", "{title}" }
            div { class: "panel-sub", "{count_copy}" }
            div { class: "row-list",
                for (i, e) in entries.iter().enumerate() {
                    {
                        entry_row(
                            st,
                            RowCtx {
                                list,
                                index: i,
                                count: n,
                                entry: e,
                                active: last_moved.as_deref() == Some(e.screen.as_str()),
                                blocked: move_entry(layout, list, i).err(),
                                has_err: err_for.as_deref() == Some(e.screen.as_str()),
                            },
                        )
                    }
                    if list == MenuList::Header && n > HEADER_INLINE_MAX && i + 1 == HEADER_INLINE_MAX {
                        div { class: "ih-menuedit-divider", "{DIVIDER}" }
                    }
                }
            }
            if let Some(msg) = bound {
                p { class: "help", style: "margin:0;color:var(--text-dim);", "{msg}" }
            }
        }
    }
}

/// The Menu tab body. Hooks live here (not in `ScreenSettings`, whose
/// `use_server_future(...)?` early return would reorder them).
#[component]
pub fn MenuSettingsPanel() -> Element {
    let ctx = use_context::<MenuCtx>();
    let q = ctx.save;
    let st = PanelState {
        ctx,
        q,
        announce: use_signal(String::new),
        pending_focus: use_signal(|| None::<String>),
        last_moved: use_signal(|| None::<String>),
        label_err: use_signal(|| None::<String>),
        rev: use_signal(|| 0_u32),
    };
    let mut confirming = use_signal(|| false);

    // Reads `pending_focus`, never writes it, so it cannot retrigger itself.
    let pending_focus = st.pending_focus;
    use_effect(move || {
        if let Some(id) = pending_focus.read().clone() {
            focus_element_by_id(&id);
        }
    });

    let layout = ctx.layout.read().clone();
    let notice = ctx.notice.read().clone();
    let status = *q.status.read();
    let writes_off = !*ctx.write_enabled.read() || status == SaveStatus::Disabled;
    let announce = st.announce.read().clone();
    let is_confirming = *confirming.read();

    let on_start = Callback::new(move |()| {
        confirming.set(true);
        let mut focus = pending_focus;
        focus.set(Some(KEEP_ID.to_string()));
    });
    let on_cancel = Callback::new(move |()| {
        confirming.set(false);
        let mut focus = pending_focus;
        focus.set(Some(RESET_ID.to_string()));
    });
    let on_confirm = Callback::new(move |()| {
        confirming.set(false);
        apply_edit(ctx, default_layout());
        let (mut notice, mut announce, mut last_moved, mut label_err, mut focus) = (
            ctx.notice,
            st.announce,
            st.last_moved,
            st.label_err,
            pending_focus,
        );
        notice.set(None);
        announce.set(RESET_DONE.to_string());
        last_moved.set(None);
        label_err.set(None);
        focus.set(Some(RESET_ID.to_string()));
        request_save(ctx, q);
    });

    rsx! {
        div { class: "panel",
            div { class: "panel-title", "Menu" }
            div { class: "panel-sub", "{INTRO}" }
            if let Some(text) = notice {
                p { style: "color:var(--amber);font-size:11px;margin:0;", "{text}" }
            }
        }
        div {
            class: "split ih-menuedit-split",
            onfocusout: move |_| {
                let mut last_moved = st.last_moved;
                spawn(async move {
                    crate::platform::timer::sleep(0).await;
                    if !focus_inside_menu_list() {
                        last_moved.set(None);
                    }
                });
            },
            {list_panel(st, MenuList::Wheel, &layout)}
            {list_panel(st, MenuList::Header, &layout)}
        }
        div { class: "ih-sr-only", role: "status", "aria-live": "polite", "{announce}" }
        div { class: "ih-menuedit-footer",
            div { class: "ih-menuedit-status",
                {status_line(status)}
                if writes_off {
                    {write_disabled_copy()}
                }
            }
            {render_reset_area(is_confirming, on_start, on_confirm, on_cancel)}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Screen, WheelState};
    use std::cell::RefCell;

    const SRC: &str = include_str!("settings_menu.rs");
    const SCREENS_CSS: &str = include_str!("../../../../assets/screens.css");

    thread_local! {
        static TEST_CTX: RefCell<(MenuLayout, Option<String>, bool)> =
            RefCell::new((default_layout(), None, true));
    }

    fn ctx_root() -> MenuCtx {
        let (l, n, w) = TEST_CTX.with(|c| c.borrow().clone());
        let layout = use_signal(|| l);
        let notice = use_signal(|| n);
        let write_enabled = use_signal(|| w);
        let save = use_save_queue();
        use_context_provider(|| MenuCtx {
            layout,
            notice,
            write_enabled,
            save,
        })
    }

    fn render_with(layout: MenuLayout, notice: Option<String>, write_enabled: bool) -> String {
        TEST_CTX.with(|c| *c.borrow_mut() = (layout, notice, write_enabled));
        fn root() -> Element {
            ctx_root();
            rsx! { MenuSettingsPanel {} }
        }
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    fn render_panel(layout: MenuLayout) -> String {
        render_with(layout, None, true)
    }

    /// The substring of `html` for one list panel.
    fn list_html<'a>(html: &'a str, list: &str) -> &'a str {
        let start = html
            .find(&format!("data-menuedit-list=\"{list}\""))
            .expect("list panel present");
        let rest = &html[start..];
        let end = rest[1..]
            .find("data-menuedit-list=")
            .map_or(rest.len(), |i| i + 1);
        &rest[..end]
    }

    /// The opening `<button ...>` tags whose text is `text`.
    fn buttons_with_text<'a>(html: &'a str, text: &str) -> Vec<&'a str> {
        let needle = format!(">{text}<");
        html.match_indices(&needle)
            .filter_map(|(at, _)| {
                let start = html[..at].rfind("<button")?;
                Some(&html[start..at])
            })
            .collect()
    }

    fn move_to_header(layout: &MenuLayout, ids: &[&str]) -> MenuLayout {
        let mut l = layout.clone();
        for id in ids {
            let i = l.wheel.iter().position(|e| e.screen == *id).unwrap();
            l = move_entry(&l, MenuList::Wheel, i).unwrap();
        }
        l
    }

    #[test]
    fn menu_panel_renders_both_lists() {
        let html = render_panel(default_layout());
        let wheel = list_html(&html, "wheel");
        let header = list_html(&html, "header");
        assert_eq!(wheel.matches("ih-menuedit-row").count(), 10, "{wheel}");
        assert_eq!(header.matches("ih-menuedit-row").count(), 7, "{header}");
        assert!(
            wheel.contains(">Wheel<") && header.contains(">Header shortcuts<"),
            "{html}"
        );
        assert!(html.contains("10 of 6 to 11 sections"), "{html}");
        assert!(html.contains("7 shortcuts"), "{html}");
        assert!(html.contains(INTRO), "{html}");
        // The row anatomy: label input with maxlength, help line, arrows.
        assert!(wheel.contains("maxlength=\"9\""), "{wheel}");
        assert!(wheel.contains("Opens Models"), "{wheel}");
        assert!(wheel.contains("aria-label=\"Move MODELS up\""), "{wheel}");
        assert!(wheel.contains("aria-label=\"Label for Models\""), "{wheel}");
        assert!(wheel.contains("value=\"MODELS\""), "{wheel}");
    }

    #[test]
    fn menu_edit_updates_the_live_wheel() {
        fn root() -> Element {
            let screen = use_signal(|| Screen::Chat);
            use_context_provider(|| screen);
            let wheel = use_signal(WheelState::default);
            use_context_provider(|| wheel);
            let cards = use_signal(Default::default);
            use_context_provider(|| crate::components::hermes_app::ApprovalCardsCtx(cards));
            let ctx = ctx_root();
            use_hook(|| {
                let next = reorder(&default_layout(), MenuList::Wheel, 2, MoveDir::Up).unwrap();
                apply_edit(ctx, next);
            });
            rsx! {
                div { id: "t-wheel", crate::components::hermes_app::wheel::Wheel { docked: true } }
                div { id: "t-panel", MenuSettingsPanel {} }
            }
        }
        TEST_CTX.with(|c| *c.borrow_mut() = (default_layout(), None, true));
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        let html = dioxus::ssr::render(&dom);
        let (wheel, panel) = html.split_once("id=\"t-panel\"").expect("both mounted");
        let agents = wheel.find(">AGENTS<").expect("AGENTS wedge");
        let models = wheel.find(">MODELS<").expect("MODELS wedge");
        assert!(
            models < agents,
            "MODELS is now ahead of AGENTS on the wheel"
        );
        let rows = list_html(panel, "wheel");
        let up_models = rows
            .find("ih-menuedit-up-wheel-models")
            .expect("models row");
        let up_agents = rows
            .find("ih-menuedit-up-wheel-agents")
            .expect("agents row");
        assert!(
            up_models < up_agents,
            "Models is the second wheel row, Agents the third"
        );
    }

    #[test]
    fn menu_arrow_handlers_apply_then_save() {
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).unwrap()];
        let chunks: Vec<&str> = code
            .split("onclick:")
            .filter(|c| c.contains("reorder(&cur"))
            .map(|c| &c[..c.find("\"▲\"").or(c.find("\"▼\"")).expect("arrow glyph")])
            .collect();
        assert_eq!(chunks.len(), 2, "one onclick per arrow");
        for c in chunks {
            let a = c.find("apply_edit(").expect("handler applies the edit");
            let s = c.find("request_save(").expect("handler saves");
            assert!(a < s, "apply_edit must precede request_save");
        }
    }

    #[test]
    fn save_loop_outlives_the_panel() {
        // WR-05: the save loop runs on the root scope over root-owned signals.
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        let fn_start = code.find("pub(crate) fn request_save").expect("request_save");
        let body = &code[fn_start..code[fn_start..].find("\n}\n").unwrap() + fn_start];
        assert!(body.contains("spawn_forever("));
        assert!(!body.contains(&["    ", "spawn("].concat()));
        // RR-01: the queue is built once at the root (MenuCtx), never per panel mount.
        assert!(code.contains("let q = ctx.save;"));
        assert_eq!(code.matches("use_save_queue()").count(), 1, "only the definition");
    }

    #[test]
    fn locked_rows_show_pill_not_move() {
        let html = render_panel(default_layout());
        let wheel = list_html(&html, "wheel");
        for id in ["chat", "settings"] {
            let at = wheel
                .find(&format!("ih-menuedit-up-wheel-{id}"))
                .expect("locked row");
            let row_end = wheel[at..]
                .find("ih-menuedit-row")
                .map_or(wheel.len(), |i| at + i);
            let row = &wheel[at..row_end];
            assert!(row.contains(">LOCKED<"), "{id}: {row}");
            assert!(row.contains(LOCKED_TITLE), "{id}: {row}");
            assert!(!row.contains(MOVE_TO_HEADER), "{id}: {row}");
        }
        let agents = wheel.find("ih-menuedit-up-wheel-agents").unwrap();
        let agents_end = wheel[agents..]
            .find("ih-menuedit-row")
            .map_or(wheel.len(), |i| agents + i);
        assert!(wheel[agents..agents_end].contains(MOVE_TO_HEADER));
        assert_eq!(buttons_with_text(wheel, MOVE_TO_HEADER).len(), 8);
    }

    #[test]
    fn bound_messages_render_at_min_and_max() {
        let min = move_to_header(&default_layout(), &["agents", "models", "tools", "skills"]);
        assert_eq!(min.wheel.len(), 6);
        let html = render_panel(min);
        assert!(html.contains(BOUND_MIN), "{html}");
        assert!(!html.contains(BOUND_MAX), "{html}");
        let btns = buttons_with_text(&html, MOVE_TO_HEADER);
        assert_eq!(btns.len(), 4, "{html}");
        assert!(btns.iter().all(|b| b.contains("disabled")), "{btns:?}");
        // Header rows can still move to the wheel at the minimum.
        let to_wheel = buttons_with_text(&html, MOVE_TO_WHEEL);
        assert!(
            to_wheel.iter().all(|b| !b.contains("disabled")),
            "{to_wheel:?}"
        );

        let mut max = default_layout();
        let kanban = max.header.remove(0);
        max.wheel.push(kanban);
        assert_eq!(max.wheel.len(), 11);
        let html = render_panel(max);
        assert!(html.contains(BOUND_MAX), "{html}");
        assert!(!html.contains(BOUND_MIN), "{html}");
        let btns = buttons_with_text(&html, MOVE_TO_WHEEL);
        assert_eq!(btns.len(), 6, "{html}");
        assert!(btns.iter().all(|b| b.contains("disabled")), "{btns:?}");

        let html = render_panel(default_layout());
        assert!(
            !html.contains(BOUND_MIN) && !html.contains(BOUND_MAX),
            "{html}"
        );
        let to_header = buttons_with_text(&html, MOVE_TO_HEADER);
        assert!(
            to_header.iter().all(|b| !b.contains("disabled")),
            "{to_header:?}"
        );
    }

    #[test]
    fn header_divider_after_fifth_row() {
        let seven = move_to_header(&default_layout(), &["tools"]);
        assert_eq!(seven.header.len(), 8);
        let html = render_panel(seven);
        let header = list_html(&html, "header");
        assert_eq!(header.matches(DIVIDER).count(), 1, "{header}");
        let rows: Vec<usize> = header
            .match_indices("class=\"row ih-menuedit-row")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(rows.len(), 8, "{header}");
        let d = header.find(DIVIDER).unwrap();
        assert!(
            rows[4] < d && d < rows[5],
            "divider sits between rows 5 and 6"
        );
        // Phase 54.10 Plan 01: the seven-entry default has OpenUI and Dashboard behind the
        // divider; a five-entry header (render-only, it would not validate) has no divider.
        let html = render_panel(default_layout());
        assert_eq!(list_html(&html, "header").matches(DIVIDER).count(), 1);
        let mut five = default_layout();
        five.header.truncate(5);
        let html = render_panel(five);
        assert!(!list_html(&html, "header").contains(DIVIDER));
    }

    #[test]
    fn invalid_notice_renders_from_ctx() {
        let text = "Saved menu layout ignored: Chat is missing. Showing the default layout.";
        let html = render_with(default_layout(), Some(text.to_string()), true);
        assert_eq!(html.matches(text).count(), 1, "{html}");
        assert!(html.find(text).unwrap() < html.find("data-menuedit-list").unwrap());
        assert!(html.contains("color:var(--amber)"), "{html}");
        let html = render_with(default_layout(), None, true);
        assert!(!html.contains("Saved menu layout ignored"), "{html}");
    }

    #[test]
    fn write_disabled_copy_when_gate_closed() {
        let html = render_with(default_layout(), None, false);
        assert_eq!(html.matches(WRITES_DISABLED).count(), 1, "{html}");
        assert_eq!(html.matches(WRITES_DISABLED_HINT).count(), 1, "{html}");
        let html = render_with(default_layout(), None, true);
        assert!(!html.contains(WRITES_DISABLED), "{html}");
    }

    fn render_reset_area_html(confirming: bool) -> String {
        thread_local! { static CONFIRMING: RefCell<bool> = const { RefCell::new(false) }; }
        CONFIRMING.with(|c| *c.borrow_mut() = confirming);
        fn root() -> Element {
            let noop = Callback::new(|()| {});
            render_reset_area(CONFIRMING.with(|c| *c.borrow()), noop, noop, noop)
        }
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn reset_confirm_copy_and_buttons() {
        let html = render_reset_area_html(true);
        assert!(html.contains(&reset_confirm()), "{html}");
        assert!(html.contains(">Reset layout<"), "{html}");
        assert!(html.contains(">Keep my layout<"), "{html}");
        assert!(html.contains("btn--danger"), "{html}");
        assert!(!html.contains(">Reset to default<"), "{html}");
        let html = render_reset_area_html(false);
        assert!(html.contains(">Reset to default<"), "{html}");
        assert!(!html.contains(&reset_confirm()), "{html}");
    }

    #[test]
    fn reset_copy_counts_follow_the_default() {
        assert_eq!(
            reset_confirm(),
            "Reset to default: Replace your layout and labels with the shipped one (10 wheel sections, 7 header shortcuts)?"
        );
        let d = crate::menu_layout::default_layout();
        assert!(reset_confirm().contains(&format!(
            "({} wheel sections, {} header",
            d.wheel.len(),
            d.header.len()
        )));
    }

    #[test]
    fn announcements_match_copy() {
        assert_eq!(
            announce_reorder("Models", 2, 10, MenuList::Wheel),
            "Models moved to position 2 of 10 on the wheel."
        );
        assert_eq!(
            announce_reorder("Logs", 1, 5, MenuList::Header),
            "Logs moved to position 1 of 5 on the header."
        );
        assert_eq!(
            announce_move("Kanban", MenuList::Wheel),
            "Kanban moved to the wheel."
        );
        assert_eq!(RESET_DONE, "Layout reset to default.");
    }

    #[test]
    fn menu_row_css_uses_theme_variables_only() {
        let mut css = String::new();
        let mut rest = SCREENS_CSS;
        while let Some(i) = rest.find("/*") {
            css.push_str(&rest[..i]);
            rest = rest[i..].split_once("*/").map_or("", |x| x.1);
        }
        css.push_str(rest);
        let mut found = 0;
        for chunk in css.split('}') {
            let Some((selector, body)) = chunk.rsplit_once('{') else {
                continue;
            };
            if selector.contains(".ih-menuedit-") {
                found += 1;
                assert!(!body.contains('#'), "colour literal in {selector}: {body}");
                assert!(!body.contains("rgb("), "rgb() in {selector}: {body}");
            }
        }
        assert!(
            found >= 4,
            "expected the .ih-menuedit- rules, found {found}"
        );
        assert!(
            css.contains(".ih-sr-only"),
            "visually-hidden helper missing"
        );
    }

    #[test]
    fn menu_tab_has_no_drag_or_hide_controls() {
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).unwrap()];
        for banned in [
            concat!("ondrag", "start"),
            concat!("on", "drop"),
            concat!("dangerous_", "inner_html"),
            concat!("vis", "ible"),
        ] {
            assert!(
                !code.contains(banned),
                "settings_menu.rs contains `{banned}`"
            );
        }
    }
}
