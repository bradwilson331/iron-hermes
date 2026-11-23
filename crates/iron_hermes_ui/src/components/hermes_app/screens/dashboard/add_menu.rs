//! Phase 54.10 Plan 08 (UI-SPEC A8): the `Add ▾` popup.
//!
//! `.ih-menu-item` is header-specific and `MenuOverflow` is not reusable (RESEARCH Pitfall 2),
//! so this is a small menu of its own that copies MenuOverflow's key contract: Up/Down wrap,
//! Home/End jump, Enter/Space activate, Esc closes and restores focus to the trigger. The
//! `Saved views` group (plan 54.10-09) lists the newest saved OpenUI views; `Generate AI
//! widget…` opens the `/` palette focused on its generate row (plan 54.10-10).

use super::items::SavedViews;
use crate::components::hermes_app::header::{MenuKey, focus_element_by_id, menu_next_index};
use crate::server::dashboard_api::{
    BOARD_ITEMS_FULL_ERROR, ItemKind, TileSpec, UsageWindow, all_kanban_columns,
};
use crate::server::openui_views_api::OpenUiView;
use dioxus::prelude::*;

/// UI-SPEC "Add menu groups": at most this many saved views, newest first.
const SAVED_VIEWS_SHOWN: usize = 8;
/// Menu labels are cut at this many characters.
const SAVED_LABEL_CHARS: usize = 28;

/// What the operator picked.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AddChoice {
    Tile(TileSpec),
    /// An AI widget showing this saved view.
    Widget(OpenUiView),
    Note,
    /// Opens the palette for a typed idea; places nothing by itself.
    Generate,
}

/// The views the menu shows: newest 8 by `updated_at`. Snapshots are dropped (the menu needs the
/// name and id only), so the prop stays small.
pub(crate) fn menu_views(views: &SavedViews) -> SavedViews {
    views.as_ref().map(|r| {
        r.as_ref()
            .map(|list| {
                let mut v: Vec<OpenUiView> = list
                    .iter()
                    .map(|v| OpenUiView {
                        snapshot: String::new(),
                        prompt: String::new(),
                        ..v.clone()
                    })
                    .collect();
                v.sort_by_key(|v| std::cmp::Reverse(v.updated_at));
                v.truncate(SAVED_VIEWS_SHOWN);
                v
            })
            .map_err(|e| e.clone())
    })
}

pub(crate) fn truncate_label(name: &str) -> String {
    if name.chars().count() <= SAVED_LABEL_CHARS {
        return name.to_string();
    }
    let cut: String = name.chars().take(SAVED_LABEL_CHARS - 1).collect();
    format!("{cut}…")
}

impl AddChoice {
    /// The item this choice places, or `None` for [`AddChoice::Generate`].
    pub(crate) fn kind(&self) -> Option<ItemKind> {
        Some(match self {
            AddChoice::Tile(tile) => ItemKind::Tile { tile: tile.clone() },
            AddChoice::Widget(v) => ItemKind::Widget {
                view_id: v.id.clone(),
            },
            AddChoice::Note => ItemKind::Note {
                text: String::new(),
            },
            AddChoice::Generate => return None,
        })
    }

    fn label(&self) -> String {
        match self {
            AddChoice::Tile(t) => t.title(),
            AddChoice::Widget(v) => truncate_label(&v.name),
            AddChoice::Note => "Note".to_string(),
            AddChoice::Generate => "Generate AI widget…".to_string(),
        }
    }
}

/// The six native tiles with their default settings, in menu order.
pub(crate) fn native_tile_choices() -> Vec<AddChoice> {
    [
        TileSpec::Usage {
            window: UsageWindow::default(),
        },
        TileSpec::Sessions {
            show: Default::default(),
        },
        TileSpec::Kanban {
            columns: all_kanban_columns(),
        },
        TileSpec::Cron {},
        TileSpec::Gateway {},
        TileSpec::Reports { report: None },
    ]
    .into_iter()
    .map(AddChoice::Tile)
    .collect()
}

fn item_dom_id(i: usize) -> String {
    format!("ih-add-item-{i}")
}

pub(crate) const TRIGGER_ID: &str = "ih-add-trigger";

/// The trigger and its popup. `full` disables every option and shows the cap copy.
#[component]
pub(crate) fn AddMenu(
    full: bool,
    #[props(default)] views: SavedViews,
    on_choose: EventHandler<AddChoice>,
) -> Element {
    let mut open = use_signal(|| false);
    use_effect(move || {
        if *open.read() {
            focus_element_by_id(&item_dom_id(0));
        }
    });
    let is_open = *open.read();
    let expanded = if is_open { "true" } else { "false" };
    rsx! {
        div { class: "ih-addmenu-wrap",
            button {
                id: TRIGGER_ID,
                class: "btn btn--sm btn--ghost",
                r#type: "button",
                "aria-haspopup": "menu",
                "aria-expanded": "{expanded}",
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
                "Add ▾"
            }
            if is_open {
                div { class: "ih-menu-backdrop", onclick: move |_| open.set(false) }
                AddMenuList {
                    full,
                    views,
                    on_choose: move |c| {
                        open.set(false);
                        on_choose.call(c);
                    },
                    on_close: move |_| {
                        open.set(false);
                        focus_element_by_id(TRIGGER_ID);
                    },
                }
            }
        }
    }
}

/// The popup body, split out so it renders without the trigger state.
#[component]
pub(crate) fn AddMenuList(
    full: bool,
    #[props(default)] views: SavedViews,
    on_choose: EventHandler<AddChoice>,
    on_close: EventHandler<()>,
) -> Element {
    let views = menu_views(&views);
    let mut entries = native_tile_choices();
    let native = entries.len();
    let unavailable = matches!(views, Some(Err(_)));
    if let Some(Ok(list)) = &views {
        entries.extend(list.iter().cloned().map(AddChoice::Widget));
    }
    let other = entries.len();
    entries.push(AddChoice::Note);
    entries.push(AddChoice::Generate);
    let len = entries.len();
    rsx! {
        div { class: "ih-menu ih-addmenu", role: "menu",
            div { class: "group-label", "Native tiles" }
            for (i , choice) in entries.into_iter().enumerate() {
                if i == native && other > native {
                    div { class: "group-label", "Saved views" }
                }
                if i == native && unavailable {
                    div { class: "group-label", "Saved views" }
                    div { class: "help", "Saved views unavailable." }
                }
                if i == other {
                    div { class: "group-label", "Other" }
                }
                button {
                    id: "{item_dom_id(i)}",
                    class: "item",
                    role: "menuitem",
                    r#type: "button",
                    disabled: full,
                    onclick: {
                        let choice = choice.clone();
                        move |_| on_choose.call(choice.clone())
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
                            if let Some(n) = menu_next_index(Some(i), len, k) {
                                focus_element_by_id(&item_dom_id(n));
                            }
                        } else if e.key() == Key::Escape {
                            e.stop_propagation();
                            on_close.call(());
                        }
                    },
                    "{choice.label()}"
                }
            }
            if full {
                div { class: "help ih-addmenu__cap", "{BOARD_ITEMS_FULL_ERROR}" }
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

    #[test]
    fn add_menu_lists_native_tiles_and_note() {
        let html =
            ssr(|| rsx! { AddMenuList { full: false, on_choose: |_| {}, on_close: |_| {} } });
        let at = |needle: &str| {
            html.find(needle)
                .unwrap_or_else(|| panic!("{needle}: {html}"))
        };
        // Groups in order, with the tiles under the first and Note under the second.
        let order = [
            "Native tiles",
            ">Usage<",
            ">Sessions<",
            ">Kanban<",
            ">Cron<",
            ">Gateway health<",
            ">Reports<",
            ">Other<",
            ">Note<",
            ">Generate AI widget…<",
        ];
        let places: Vec<usize> = order.iter().map(|n| at(n)).collect();
        assert!(places.windows(2).all(|w| w[0] < w[1]), "{html}");
        assert_eq!(html.matches("role=\"menuitem\"").count(), 8, "{html}");
        assert!(!html.contains("disabled"), "{html}");
        assert!(!html.contains("This board is full"), "{html}");

        let full = ssr(|| rsx! { AddMenuList { full: true, on_choose: |_| {}, on_close: |_| {} } });
        assert_eq!(full.matches("disabled").count(), 8, "{full}");
        assert!(
            full.contains("This board is full (40 items). Remove one to add another."),
            "{full}"
        );
    }

    fn views_of(n: usize) -> SavedViews {
        Some(Ok((0..n)
            .map(|i| OpenUiView {
                id: format!("00000000-0000-4000-8000-{i:012}"),
                name: format!("View {i}"),
                prompt: String::new(),
                snapshot: String::new(),
                created_at: 0,
                updated_at: i as i64,
            })
            .collect()))
    }

    #[test]
    fn add_menu_saved_views_group() {
        let ten = views_of(10);
        let html = ssr_with(ten.clone());
        // Newest 8 (updated_at 9 down to 2), between Native tiles and Other.
        for n in 2..10 {
            assert!(html.contains(&format!(">View {n}<")), "{html}");
        }
        assert!(
            !html.contains(">View 1<") && !html.contains(">View 0<"),
            "{html}"
        );
        let order = [
            ">Reports<",
            ">Saved views<",
            ">View 9<",
            ">View 2<",
            ">Other<",
            ">Note<",
            ">Generate AI widget…<",
        ];
        let places: Vec<usize> = order.iter().map(|n| html.find(n).expect(n)).collect();
        assert!(places.windows(2).all(|w| w[0] < w[1]), "{html}");
        assert_eq!(
            html.matches("role=\"menuitem\"").count(),
            6 + 8 + 2,
            "{html}"
        );

        let err = ssr_with(Some(Err("boom".into())));
        assert!(err.contains("Saved views unavailable."), "{err}");
        let empty = ssr_with(Some(Ok(vec![])));
        assert!(!empty.contains("Saved views"), "{empty}");
        assert_eq!(empty.matches("role=\"menuitem\"").count(), 8, "{empty}");
        // The choice places a widget for that view.
        let AddChoice::Widget(v) = AddChoice::Widget(views_of(1).unwrap().unwrap().remove(0))
        else {
            unreachable!()
        };
        assert!(
            matches!(AddChoice::Widget(v.clone()).kind(), Some(ItemKind::Widget { view_id }) if view_id == v.id)
        );
        assert_eq!(truncate_label(&"x".repeat(40)).chars().count(), 28);
    }

    fn ssr_with(views: SavedViews) -> String {
        let mut dom = VirtualDom::new_with_props(
            |v: SavedViews| rsx! { AddMenuList { full: false, views: v, on_choose: |_| {}, on_close: |_| {} } },
            views,
        );
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn choices_build_the_expected_items() {
        let tiles = native_tile_choices();
        assert_eq!(tiles.len(), 6);
        assert_eq!(tiles[5].label(), "Reports");
        assert!(matches!(
            AddChoice::Note.kind(),
            Some(ItemKind::Note { text }) if text.is_empty()
        ));
        match tiles[2].kind() {
            Some(ItemKind::Tile {
                tile: TileSpec::Kanban { columns },
            }) => assert_eq!(columns.len(), 6),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn add_menu_generate_opens_palette_focused_on_generate() {
        use super::super::palette::{PaletteOption, initial_highlight, palette_options};
        // The menu item places nothing itself: the screen opens the palette for it.
        assert_eq!(AddChoice::Generate.kind(), None);
        assert_eq!(AddChoice::Generate.label(), "Generate AI widget…");
        // With the palette opened for it, the generate row takes the highlight as soon as the
        // operator has typed an idea.
        let typed = palette_options("spend", None);
        let at = initial_highlight(&typed, true);
        assert!(matches!(typed[at], PaletteOption::Generate(_)));
        // The wiring in the screen asks for exactly that.
        const MOD_RS: &str = include_str!("mod.rs");
        assert!(MOD_RS.contains("open_palette(st, None, true, TRIGGER_ID)"));
    }
}
