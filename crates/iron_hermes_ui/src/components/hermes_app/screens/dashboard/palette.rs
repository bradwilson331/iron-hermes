//! Phase 54.10 Plan 10 (D-02, D-05, D-14): the `/` palette.
//!
//! Typing filters native tiles, saved views and the note; the pinned `Generate AI widget:
//! "{text}"` row is the only choice that starts an agent turn (D-05). It first creates a pending
//! saved view (prompt, empty snapshot) so the board item always holds a valid `view_id` (D-02),
//! places the widget, then enqueues ONE `Rerun` job on the dashboard runtime. Filtering, row
//! layout, key handling and the spend effects are pure functions so they test without a server.

use super::add_menu::{AddChoice, native_tile_choices, truncate_label};
use super::canvas::{BoardState, CANVAS_DOM_ID, add_item, focus_placed, place_item};
use super::items::{SavedViews, focus_dom};
use super::widgets::widget_error_copy;
use crate::components::hermes_app::dashboard_runtime::{DashboardGenCtx, GenJob, GenKind};
use crate::components::hermes_app::screens::openui::default_view_name;
use crate::server::dashboard_api::{BOARD_ITEMS_FULL_ERROR, BOARD_ITEMS_MAX, ItemKind, TileSpec};
use crate::server::openui_views_api::{OpenUiView, create_pending_openui_view, delete_openui_view};
use dioxus::prelude::*;
use std::cmp::Reverse;

/// UI-SPEC: at most this many saved views are listed.
const VIEWS_SHOWN: usize = 8;
/// The idea text is a turn prompt, not a document.
const QUERY_MAX_CHARS: usize = 500;
const LIST_ID: &str = "ih-palette-list";

/// Where the palette opened and what it should do on open/close.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaletteOpen {
    /// Client position of the pointer; `None` places at the viewport centre (keyboard / button).
    pub anchor: Option<(f64, f64)>,
    /// Opened from `Generate AI widget…`: the generate row takes the highlight once it exists.
    pub prefer_generate: bool,
    /// DOM id that gets focus back when the palette closes without placing anything.
    pub restore: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PaletteOption {
    Tile(TileSpec),
    View {
        id: String,
        name: String,
    },
    Note,
    /// The idea text, trimmed.
    Generate(String),
}

impl PaletteOption {
    pub(crate) fn label(&self) -> String {
        match self {
            PaletteOption::Tile(t) => t.title(),
            PaletteOption::View { name, .. } => truncate_label(name),
            PaletteOption::Note => "Note".to_string(),
            PaletteOption::Generate(text) => format!("Generate AI widget: \"{text}\""),
        }
    }

    /// The board item this choice places, or `None` for the generate row (its item needs a view).
    fn kind(&self) -> Option<ItemKind> {
        match self {
            PaletteOption::Tile(tile) => Some(ItemKind::Tile { tile: tile.clone() }),
            PaletteOption::View { id, .. } => Some(ItemKind::Widget {
                view_id: id.clone(),
            }),
            PaletteOption::Note => Some(ItemKind::Note {
                text: String::new(),
            }),
            PaletteOption::Generate(_) => None,
        }
    }
}

/// What choosing an option costs (D-05).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChoiceEffects {
    pub starts_turn: bool,
}

/// Only the generate row starts an agent turn.
pub(crate) fn palette_choice_effects(opt: &PaletteOption) -> ChoiceEffects {
    ChoiceEffects {
        starts_turn: matches!(opt, PaletteOption::Generate(_)),
    }
}

/// The options for `query`: native tiles, saved views (newest first, at most 8) and the note,
/// each kept when its label contains the text (case-insensitive); the generate row is appended
/// when the trimmed text is non-empty. `views` is `None` while loading or on a load error.
pub(crate) fn palette_options(query: &str, views: Option<&[OpenUiView]>) -> Vec<PaletteOption> {
    let text = query.trim();
    let needle = text.to_lowercase();
    let hit = |label: &str| needle.is_empty() || label.to_lowercase().contains(&needle);
    let mut out: Vec<PaletteOption> = native_tile_choices()
        .into_iter()
        .filter_map(|c| match c {
            AddChoice::Tile(t) if hit(&t.title()) => Some(PaletteOption::Tile(t)),
            _ => None,
        })
        .collect();
    let mut found: Vec<&OpenUiView> = views
        .unwrap_or(&[])
        .iter()
        .filter(|v| hit(&v.name))
        .collect();
    found.sort_by_key(|v| Reverse(v.updated_at));
    out.extend(
        found
            .into_iter()
            .take(VIEWS_SHOWN)
            .map(|v| PaletteOption::View {
                id: v.id.clone(),
                name: v.name.clone(),
            }),
    );
    if hit("note") {
        out.push(PaletteOption::Note);
    }
    if !text.is_empty() {
        out.push(PaletteOption::Generate(text.to_string()));
    }
    out
}

/// The highlighted option when the list is (re)built: the first one (the generate row only when
/// nothing else matches), or the generate row when the palette opened for it.
pub(crate) fn initial_highlight(options: &[PaletteOption], prefer_generate: bool) -> usize {
    if prefer_generate {
        if let Some(i) = options
            .iter()
            .position(|o| matches!(o, PaletteOption::Generate(_)))
        {
            return i;
        }
    }
    0
}

/// One line of the listbox: a group label, the views-failed note, or an option by index.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PaletteRow {
    Group(&'static str),
    Unavailable,
    Option(usize),
}

/// Group labels and the saved-views states around `options`: an empty saved-views group is
/// omitted; a failed load shows `Saved views unavailable.` in its place.
pub(crate) fn palette_rows(options: &[PaletteOption], views: &SavedViews) -> Vec<PaletteRow> {
    let mut rows = Vec::new();
    let group = |rows: &mut Vec<PaletteRow>, label, pick: fn(&PaletteOption) -> bool| {
        let idx: Vec<usize> = (0..options.len()).filter(|i| pick(&options[*i])).collect();
        if !idx.is_empty() {
            rows.push(PaletteRow::Group(label));
            rows.extend(idx.into_iter().map(PaletteRow::Option));
            return true;
        }
        false
    };
    group(&mut rows, "Native tiles", |o| {
        matches!(o, PaletteOption::Tile(_))
    });
    let has_views = group(&mut rows, "Saved views", |o| {
        matches!(o, PaletteOption::View { .. })
    });
    if !has_views && matches!(views, Some(Err(_))) {
        rows.push(PaletteRow::Group("Saved views"));
        rows.push(PaletteRow::Unavailable);
    }
    group(&mut rows, "Other", |o| matches!(o, PaletteOption::Note));
    rows.extend(
        options
            .iter()
            .enumerate()
            .filter(|(_, o)| matches!(o, PaletteOption::Generate(_)))
            .map(|(i, _)| PaletteRow::Option(i)),
    );
    rows
}

/// What a key does inside the palette input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaletteKey {
    /// Move the highlight to this option index.
    Move(usize),
    Choose,
    Close,
    Ignore,
}

/// Arrows move within bounds (group labels are not options, so they are skipped), Enter
/// chooses, Esc closes.
pub(crate) fn palette_key(key: &Key, highlight: usize, len: usize) -> PaletteKey {
    match key {
        Key::ArrowDown if len > 0 => PaletteKey::Move((highlight + 1).min(len - 1)),
        Key::ArrowUp if len > 0 => PaletteKey::Move(highlight.saturating_sub(1)),
        Key::Enter => PaletteKey::Choose,
        Key::Escape => PaletteKey::Close,
        _ => PaletteKey::Ignore,
    }
}

fn option_dom_id(i: usize) -> String {
    format!("ih-palette-opt-{i}")
}

/// The listbox. Pure props so every state renders without a runtime.
#[component]
pub(crate) fn PaletteList(
    rows: Vec<PaletteRow>,
    options: Vec<PaletteOption>,
    active: usize,
    on_pick: EventHandler<usize>,
) -> Element {
    let active_id = option_dom_id(active);
    rsx! {
        div {
            id: LIST_ID,
            class: "ih-palette__list",
            role: "listbox",
            "aria-label": "Widgets",
            "aria-activedescendant": "{active_id}",
            for row in rows {
                match row {
                    PaletteRow::Group(label) => rsx! {
                        div { class: "group-label", "{label}" }
                    },
                    PaletteRow::Unavailable => rsx! {
                        div { class: "help", "Saved views unavailable." }
                    },
                    PaletteRow::Option(i) => rsx! {
                        div {
                            id: "{option_dom_id(i)}",
                            class: "item ih-palette__opt",
                            class: if i == active { "is-active" },
                            role: "option",
                            "aria-selected": if i == active { "true" } else { "false" },
                            onclick: move |_| on_pick.call(i),
                            span { "{options[i].label()}" }
                            if palette_choice_effects(&options[i]).starts_turn {
                                span { class: "help", "Uses 1 agent turn. Press Enter." }
                            }
                        }
                    },
                }
            }
        }
    }
}

fn push_view(st: BoardState, view: &OpenUiView) {
    let mut views = st.views;
    views.with_mut(|v| match v {
        Some(Ok(list)) => list.insert(0, view.clone()),
        _ => *v = Some(Ok(vec![view.clone()])),
    });
}

fn bump_views(ctx: DashboardGenCtx) {
    let mut rev = ctx.views_rev;
    rev.with_mut(|r| *r += 1);
}

/// Carry out one choice. Tile, saved view and note place an item and spend nothing; the
/// generate row creates a pending view, places the widget and enqueues exactly one job.
fn run_choice(
    st: BoardState,
    ctx: DashboardGenCtx,
    anchor: Option<(f64, f64)>,
    opt: PaletteOption,
    mut error: Signal<Option<String>>,
    mut busy: Signal<bool>,
) {
    if *busy.peek() {
        return;
    }
    let mut palette = st.palette;
    let PaletteOption::Generate(text) = opt else {
        palette.set(None);
        if let Some(kind) = opt.kind() {
            add_item(st, kind, anchor);
        }
        return;
    };
    if st.items().len() >= BOARD_ITEMS_MAX {
        error.set(Some(BOARD_ITEMS_FULL_ERROR.to_string()));
        return;
    }
    busy.set(true);
    error.set(None);
    spawn(async move {
        let view = match create_pending_openui_view(default_view_name(&text), text.clone()).await {
            Ok(view) => view,
            Err(e) => {
                // The text stays so the operator can retry.
                error.set(Some(widget_error_copy(&e.to_string())));
                busy.set(false);
                return;
            }
        };
        push_view(st, &view);
        let kind = ItemKind::Widget {
            view_id: view.id.clone(),
        };
        match place_item(st, kind, anchor).await {
            Some(item_id) => {
                ctx.enqueue.call(GenJob {
                    item_id: item_id.clone(),
                    view_id: view.id,
                    message: text,
                    kind: GenKind::Rerun,
                });
                bump_views(ctx);
                palette.set(None);
                focus_placed(item_id).await;
            }
            None => {
                // The board filled up in the meantime: leave no orphan pending view.
                let _ = delete_openui_view(view.id).await;
                bump_views(ctx);
                palette.set(None);
            }
        }
    });
}

/// The palette popover. Mounted by the screen while `st.palette` is set.
#[component]
pub(crate) fn Palette(st: BoardState, open: PaletteOpen) -> Element {
    let ctx = use_context::<DashboardGenCtx>();
    let mut query = use_signal(String::new);
    let mut prefer = use_signal({
        let want = open.prefer_generate;
        move || want
    });
    let mut highlight = use_signal(|| 0usize);
    let mut error = use_signal(|| None::<String>);
    let busy = use_signal(|| false);

    let text = query.read().clone();
    let (options, rows) = {
        let views = st.views.read();
        let list = match &*views {
            Some(Ok(l)) => Some(l.as_slice()),
            _ => None,
        };
        let options = palette_options(&text, list);
        let rows = palette_rows(&options, &views);
        (options, rows)
    };
    let active = if *prefer.read() {
        initial_highlight(&options, true)
    } else {
        (*highlight.read()).min(options.len().saturating_sub(1))
    };
    let active_id = option_dom_id(active);
    let (anchor, restore) = (open.anchor, open.restore);
    let style = match anchor {
        Some((x, y)) => format!(
            "left: clamp(8px, {x:.0}px, calc(100vw - 336px)); top: clamp(8px, {y:.0}px, calc(100vh - 380px));"
        ),
        None => "left: 50%; top: 20%; transform: translateX(-50%);".to_string(),
    };
    let close = move || {
        // The create/place/enqueue task lives in this component: closing now would cancel it
        // part-way and orphan the pending view.
        if *busy.peek() {
            return;
        }
        let mut palette = st.palette;
        palette.set(None);
        focus_dom(restore);
    };
    let by_key = options.clone();
    let by_click = options.clone();
    let err = error.read().clone();
    let is_busy = *busy.read();

    rsx! {
        div { class: "ih-menu-backdrop", onclick: move |_| close() }
        div {
            class: "panel ih-palette",
            role: "dialog",
            "aria-label": "Add widget",
            style: "{style}",
            input {
                class: "field-input",
                r#type: "text",
                role: "combobox",
                placeholder: "Type a widget idea, or pick a tile",
                "aria-label": "Widget idea",
                "aria-expanded": "true",
                "aria-controls": LIST_ID,
                "aria-activedescendant": "{active_id}",
                "aria-busy": if is_busy { "true" },
                maxlength: "{QUERY_MAX_CHARS}",
                value: "{text}",
                onmounted: move |e| async move {
                    let _ = e.set_focus(true).await;
                },
                oninput: move |e| {
                    let v: String = e.value().chars().take(QUERY_MAX_CHARS).collect();
                    error.set(None);
                    query.set(v);
                    // The highlight follows the generate row while it was asked for.
                    if !*prefer.peek() {
                        highlight.set(0);
                    }
                },
                onkeydown: move |e: KeyboardEvent| {
                    let key = e.key();
                    match palette_key(&key, active, by_key.len()) {
                        PaletteKey::Move(i) => {
                            e.prevent_default();
                            prefer.set(false);
                            highlight.set(i);
                        }
                        PaletteKey::Choose => {
                            e.prevent_default();
                            if let Some(opt) = by_key.get(active).cloned() {
                                run_choice(st, ctx, anchor, opt, error, busy);
                            }
                        }
                        PaletteKey::Close => {
                            e.prevent_default();
                            e.stop_propagation();
                            close();
                        }
                        PaletteKey::Ignore => {
                            // Typing `/` etc. must not reach the canvas shortcuts.
                            e.stop_propagation();
                        }
                    }
                },
            }
            if let Some(msg) = err {
                div { class: "ih-palette__error", role: "alert", "{msg}" }
            }
            PaletteList {
                rows,
                options,
                active,
                on_pick: move |i: usize| {
                    if let Some(opt) = by_click.get(i).cloned() {
                        run_choice(st, ctx, anchor, opt, error, busy);
                    }
                },
            }
        }
    }
}

/// Open the palette. `restore` is the DOM id that gets focus back on close.
pub(crate) fn open_palette(
    st: BoardState,
    anchor: Option<(f64, f64)>,
    prefer_generate: bool,
    restore: &'static str,
) {
    let mut palette = st.palette;
    palette.set(Some(PaletteOpen {
        anchor,
        prefer_generate,
        restore,
    }));
}

/// Open from the canvas keyboard (`/`): at the last pointer position when known.
pub(crate) fn open_palette_from_canvas(st: BoardState, last_ptr: Option<(f64, f64)>) {
    open_palette(st, last_ptr, false, CANVAS_DOM_ID);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(id: &str, name: &str, updated: i64) -> OpenUiView {
        OpenUiView {
            id: id.into(),
            name: name.into(),
            prompt: "p".into(),
            snapshot: "s".into(),
            created_at: 0,
            updated_at: updated,
        }
    }

    #[test]
    fn palette_generate_row_only_when_text() {
        let none = palette_options("", None);
        assert!(!none.iter().any(|o| matches!(o, PaletteOption::Generate(_))));
        assert_eq!(none.len(), 7, "six tiles and the note");
        let ws = palette_options("   ", None);
        assert!(!ws.iter().any(|o| matches!(o, PaletteOption::Generate(_))));
        let some = palette_options("spend", None);
        assert_eq!(
            some.last().unwrap().label(),
            "Generate AI widget: \"spend\""
        );
        assert_eq!(
            some.iter()
                .filter(|o| matches!(o, PaletteOption::Generate(_)))
                .count(),
            1
        );
    }

    #[test]
    fn palette_generate_enqueues_exactly_one_job() {
        let views = vec![view("v1", "Cron failures", 1)];
        let all = palette_options("c", Some(&views));
        let spends: Vec<bool> = all
            .iter()
            .map(|o| palette_choice_effects(o).starts_turn)
            .collect();
        assert_eq!(spends.iter().filter(|s| **s).count(), 1);
        assert!(*spends.last().unwrap(), "the generate row is the one");
        assert!(all.iter().any(|o| matches!(o, PaletteOption::View { .. })));
        assert!(all.iter().any(|o| matches!(o, PaletteOption::Tile(_))));
        assert!(!palette_choice_effects(&PaletteOption::Note).starts_turn);
        // The wiring: only the generate path reaches the runtime's enqueue.
        const SRC: &str = include_str!("palette.rs");
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        assert_eq!(code.matches("ctx.enqueue.call(").count(), 1);
        assert!(code.contains("create_pending_openui_view(default_view_name("));
    }

    #[test]
    fn palette_filters_case_insensitively_and_caps_views() {
        let views: Vec<OpenUiView> = (0..12)
            .map(|i| view(&format!("v{i}"), &format!("Spend {i}"), i))
            .collect();
        let found = palette_options("SPEND", Some(&views));
        let shown: Vec<&PaletteOption> = found
            .iter()
            .filter(|o| matches!(o, PaletteOption::View { .. }))
            .collect();
        assert_eq!(shown.len(), 8);
        assert_eq!(shown[0].label(), "Spend 11", "newest first");
        assert!(!found.iter().any(|o| matches!(o, PaletteOption::Tile(_))));
        assert!(palette_options("nOtE", None).contains(&PaletteOption::Note));
    }

    #[test]
    fn palette_groups_and_default_highlight() {
        let us = palette_options("us", None);
        assert_eq!(us[0].label(), "Usage");
        assert_eq!(initial_highlight(&us, false), 0);
        let zzz = palette_options("zzz", None);
        assert_eq!(zzz.len(), 1);
        assert!(matches!(
            zzz[initial_highlight(&zzz, false)],
            PaletteOption::Generate(_)
        ));

        // A failed load keeps the group name with the note; an empty list omits the group.
        let err_rows = palette_rows(&palette_options("", None), &Some(Err("boom".into())));
        let label = |r: &PaletteRow| match r {
            PaletteRow::Group(g) => g.to_string(),
            PaletteRow::Unavailable => "UNAVAILABLE".into(),
            PaletteRow::Option(i) => format!("#{i}"),
        };
        let seq: Vec<String> = err_rows.iter().map(label).collect();
        let at = |s: &str| {
            seq.iter()
                .position(|x| x == s)
                .unwrap_or_else(|| panic!("{s}: {seq:?}"))
        };
        assert!(at("Native tiles") < at("Saved views"));
        assert!(at("Saved views") + 1 == at("UNAVAILABLE"));
        assert!(at("UNAVAILABLE") < at("Other"));
        let empty_rows = palette_rows(&palette_options("", Some(&[])), &Some(Ok(vec![])));
        assert!(
            !empty_rows.contains(&PaletteRow::Group("Saved views")),
            "{empty_rows:?}"
        );
        // Opened for the generate row: it takes the highlight once text exists.
        let typed = palette_options("spend", None);
        assert_eq!(initial_highlight(&typed, true), typed.len() - 1);
        assert_eq!(initial_highlight(&palette_options("", None), true), 0);
    }

    #[test]
    fn palette_keyboard_navigation() {
        assert_eq!(palette_key(&Key::ArrowDown, 0, 3), PaletteKey::Move(1));
        assert_eq!(palette_key(&Key::ArrowDown, 2, 3), PaletteKey::Move(2));
        assert_eq!(palette_key(&Key::ArrowUp, 0, 3), PaletteKey::Move(0));
        assert_eq!(palette_key(&Key::ArrowUp, 2, 3), PaletteKey::Move(1));
        assert_eq!(palette_key(&Key::ArrowDown, 0, 0), PaletteKey::Ignore);
        assert_eq!(palette_key(&Key::Enter, 1, 3), PaletteKey::Choose);
        assert_eq!(palette_key(&Key::Escape, 1, 3), PaletteKey::Close);
        assert_eq!(
            palette_key(&Key::Character("a".into()), 1, 3),
            PaletteKey::Ignore
        );
        // Esc hands focus back to where the palette was opened from.
        let open = PaletteOpen {
            anchor: None,
            prefer_generate: false,
            restore: CANVAS_DOM_ID,
        };
        assert_eq!(open.restore, "ih-dash-canvas");
        const SRC: &str = include_str!("palette.rs");
        assert!(SRC.contains("focus_dom(restore)"));
    }

    fn list_root() -> Element {
        let options = palette_options("spend", None);
        let rows = palette_rows(&options, &Some(Err("x".into())));
        rsx! {
            PaletteList { rows, options, active: 0, on_pick: |_| {} }
        }
    }

    #[test]
    fn palette_list_markup() {
        let mut dom = VirtualDom::new(list_root);
        dom.rebuild_in_place();
        let html = dioxus::ssr::render(&dom);
        assert!(html.contains("role=\"listbox\""), "{html}");
        assert!(
            html.contains("aria-activedescendant=\"ih-palette-opt-0\""),
            "{html}"
        );
        assert!(html.contains("role=\"option\""), "{html}");
        assert!(html.contains("Saved views unavailable."), "{html}");
        assert!(html.contains("Uses 1 agent turn. Press Enter."), "{html}");
        assert_eq!(html.matches("aria-selected=\"true\"").count(), 1, "{html}");
    }
}
