//! Phase 54.9 Plan 01 (D-03/D-05/D-07/D-08): the menu layout value.
//!
//! The catalog of the 16 shipped screens, the wire/YAML shape of a saved
//! layout, the shipped default, and the single validator that decides
//! "saved layout" vs "shipped default plus notice". No dioxus, axum, web_sys or
//! serde_yaml here: this module compiles for wasm and native alike; the YAML
//! conversion lives in `server/menu_api.rs`.
//!
//! Stored shape (`ui.menu` in the server-process-home `config.yaml`):
//! `{ version: 3, wheel: [{screen, label}..], header: [{screen, label}..] }`
//! with stable lowercase screen ids, never Rust variant names, so an enum
//! rename cannot invalidate saved layouts.

// Plans 02-10 consume the rest of this surface (header, wheel, Settings Menu tab).
#![allow(dead_code)]

use crate::components::hermes_app::header::HEADER_SHORTCUTS;
use crate::components::hermes_app::wheel::N as WHEEL_DEFAULT_N;
use crate::state::{Screen, WheelWedge};
use serde::{Deserialize, Serialize};

/// Version 2 (Phase 54.9 Plan 09) added the `openui` catalog entry; version 3 (Phase 54.10
/// Plan 01) added `dashboard`. A saved older layout is upgraded in place by [`migrate`]
/// instead of being rejected (RESEARCH Pitfall 3).
pub const MENU_LAYOUT_VERSION: u32 = 3;
pub const WHEEL_MIN: usize = 6;
pub const WHEEL_MAX: usize = 11;
pub const LABEL_MAX_CHARS: usize = 9;

/// One shipped, always-reachable screen.
pub struct CatalogEntry {
    pub id: &'static str,
    pub screen: Screen,
    pub name: &'static str,
    pub glyph: &'static str,
    pub sub: &'static str,
    /// The layout version that introduced this entry (1 for the screens shipped with the menu).
    /// [`migrate`] appends entries newer than a saved layout's version to its header.
    pub added_in: u32,
}

const fn entry(
    id: &'static str,
    screen: Screen,
    name: &'static str,
    glyph: &'static str,
    sub: &'static str,
) -> CatalogEntry {
    CatalogEntry {
        id,
        screen,
        name,
        glyph,
        sub,
        added_in: 1,
    }
}

/// The 17 shipped screens (D-03: every one must stay reachable). The first ten
/// carry the `WheelWedge` glyph/sub verbatim; the next five (kanban, artifacts,
/// schedules, soul, logs) are RESEARCH assumption A1, pending plan 05 visual QA; `openui`
/// (Phase 54.9 Plan 09, version 2) is the generated-views screen; `dashboard` (Phase 54.10
/// Plan 01, version 3) is the live-boards screen.
pub const MENU_CATALOG: &[CatalogEntry] = &[
    entry("chat", Screen::Chat, "Chat", "▓", "INTELLIGENCE CONSOLE"),
    entry(
        "agents",
        Screen::Agents,
        "Agents",
        "◆",
        "AUTONOMOUS WORKERS",
    ),
    entry("models", Screen::Models, "Models", "◇", "LANGUAGE CORES"),
    entry("tools", Screen::Tools, "Tools", "◈", "INSTRUMENT BAY"),
    entry(
        "skills",
        Screen::Skills,
        "Skills",
        "✦",
        "CAPABILITY LATTICE",
    ),
    entry(
        "memory",
        Screen::Memory,
        "Memory",
        "⬢",
        "PERSISTENT CONTEXT",
    ),
    entry(
        "sessions",
        Screen::Sessions,
        "Sessions",
        "▣",
        "ACTIVE TRANSCRIPTS",
    ),
    entry(
        "providers",
        Screen::Providers,
        "Providers",
        "◉",
        "INFERENCE GATEWAYS",
    ),
    entry("gateway", Screen::Gateway, "Gateway", "⌬", "NETWORK BRIDGE"),
    entry(
        "settings",
        Screen::Settings,
        "Settings",
        "⚙",
        "CONFIGURATION",
    ),
    entry("kanban", Screen::Kanban, "Kanban", "▦", "TASK BOARD"),
    entry(
        "artifacts",
        Screen::Artifacts,
        "Artifacts",
        "▤",
        "RENDERED OUTPUT",
    ),
    entry(
        "schedules",
        Screen::Schedules,
        "Schedules",
        "◷",
        "TIMED JOBS",
    ),
    entry("soul", Screen::Soul, "Soul", "✧", "PERSONA CORE"),
    entry("logs", Screen::Logs, "Logs", "≣", "SYSTEM RECORDS"),
    CatalogEntry {
        id: "openui",
        screen: Screen::OpenUi,
        name: "OpenUI",
        glyph: "❖",
        sub: "GENERATED VIEWS",
        added_in: 2,
    },
    CatalogEntry {
        id: "dashboard",
        screen: Screen::Dashboard,
        name: "Dashboard",
        glyph: "◫",
        sub: "LIVE BOARDS",
        added_in: 3,
    },
];

pub fn catalog_entry(id: &str) -> Option<&'static CatalogEntry> {
    MENU_CATALOG.iter().find(|e| e.id == id)
}

pub fn catalog_entry_for(screen: Screen) -> Option<&'static CatalogEntry> {
    MENU_CATALOG.iter().find(|e| e.screen == screen)
}

/// Chat and Settings are locked onto the wheel (D-05).
pub fn is_locked(id: &str) -> bool {
    id == "chat" || id == "settings"
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuEntry {
    pub screen: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuLayout {
    pub version: u32,
    pub wheel: Vec<MenuEntry>,
    pub header: Vec<MenuEntry>,
}

/// Why a layout was rejected. `Display` is the exact UI-SPEC reason clause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuLayoutError {
    UnknownScreen(String),
    ChatMissing,
    SettingsMissing,
    LockedOffWheel,
    WheelCount(usize),
    Unreachable(&'static str),
    Unreadable,
}

impl std::fmt::Display for MenuLayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownScreen(id) => write!(f, "unknown screen \"{id}\""),
            Self::ChatMissing => write!(f, "Chat is missing"),
            Self::SettingsMissing => write!(f, "Settings is missing"),
            Self::LockedOffWheel => write!(f, "Chat or Settings is off the wheel"),
            Self::WheelCount(n) => write!(
                f,
                "wheel has {n} sections (allowed {WHEEL_MIN} to {WHEEL_MAX})"
            ),
            Self::Unreachable(name) => write!(f, "{name} is not reachable"),
            Self::Unreadable => write!(f, "could not be read"),
        }
    }
}

/// The one-line fallback notice (UI-SPEC Copywriting Contract).
pub fn invalid_notice(err: &MenuLayoutError) -> String {
    format!("Saved menu layout ignored: {err}. Showing the default layout.")
}

fn entry_of(screen: Screen, label: &str) -> MenuEntry {
    MenuEntry {
        // Every Screen passed here is a shipped one; the fallback keeps this panic-free.
        screen: catalog_entry_for(screen)
            .map_or("chat", |c| c.id)
            .to_string(),
        label: label.to_string(),
    }
}

/// The shipped layout: the 10 wheel wedges in `WheelWedge` order, the 5
/// `HEADER_SHORTCUTS`, then every catalog entry added after version 1 (OpenUI) appended to
/// the header with its name as label.
pub fn default_layout() -> MenuLayout {
    MenuLayout {
        version: MENU_LAYOUT_VERSION,
        wheel: (0..WHEEL_DEFAULT_N)
            .map(WheelWedge::from_index)
            .map(|w| entry_of(w.to_screen(), w.label()))
            .collect(),
        header: HEADER_SHORTCUTS
            .iter()
            .map(|(screen, label)| entry_of(*screen, label))
            .chain(
                MENU_CATALOG
                    .iter()
                    .filter(|c| c.added_in > 1)
                    .map(|c| entry_of(c.screen, c.name)),
            )
            .collect(),
    }
}

/// Upgrade a saved layout from an older version by appending each catalog entry newer than
/// the layout's version (and not already present) to the END of its header, keeping every
/// other entry and label as saved. Version 0 and versions above the current one are left
/// untouched for [`validate`] to reject.
pub fn migrate(mut layout: MenuLayout) -> MenuLayout {
    if layout.version == 0 || layout.version >= MENU_LAYOUT_VERSION {
        return layout;
    }
    let from = layout.version;
    for c in MENU_CATALOG.iter().filter(|c| c.added_in > from) {
        let present = layout
            .wheel
            .iter()
            .chain(layout.header.iter())
            .any(|e| e.screen == c.id);
        if !present {
            layout.header.push(MenuEntry {
                screen: c.id.to_string(),
                label: c.name.to_string(),
            });
        }
    }
    layout.version = MENU_LAYOUT_VERSION;
    layout
}

/// Decide whether `layout` may be saved or shown. Rules run in a fixed order so
/// the first failure names the reason (UI-SPEC A4).
///
/// A duplicate-id rule is deliberately absent: once the entry count is capped at
/// the catalog size and every catalog screen must be present, a duplicate cannot
/// reach it.
pub fn validate(layout: &MenuLayout) -> Result<(), MenuLayoutError> {
    if layout.version != MENU_LAYOUT_VERSION {
        return Err(MenuLayoutError::Unreadable);
    }
    // Bounded work on hostile input.
    if layout.wheel.len() + layout.header.len() > MENU_CATALOG.len() {
        return Err(MenuLayoutError::Unreadable);
    }
    let all = || layout.wheel.iter().chain(layout.header.iter());
    if let Some(e) = all().find(|e| catalog_entry(&e.screen).is_none()) {
        return Err(MenuLayoutError::UnknownScreen(
            e.screen.chars().take(32).collect(),
        ));
    }
    let has = |list: &[MenuEntry], id: &str| list.iter().any(|e| e.screen == id);
    for (id, missing) in [
        ("chat", MenuLayoutError::ChatMissing),
        ("settings", MenuLayoutError::SettingsMissing),
    ] {
        if !all().any(|e| e.screen == id) {
            return Err(missing);
        }
        if has(&layout.header, id) {
            return Err(MenuLayoutError::LockedOffWheel);
        }
    }
    if !(WHEEL_MIN..=WHEEL_MAX).contains(&layout.wheel.len()) {
        return Err(MenuLayoutError::WheelCount(layout.wheel.len()));
    }
    if let Some(c) = MENU_CATALOG
        .iter()
        .find(|c| !all().any(|e| e.screen == c.id))
    {
        return Err(MenuLayoutError::Unreachable(c.name));
    }
    let bad_label = |l: &str| {
        let t = l.trim();
        t.is_empty() || t.chars().count() > LABEL_MAX_CHARS || l.chars().any(char::is_control)
    };
    if all().any(|e| bad_label(&e.label)) {
        return Err(MenuLayoutError::Unreadable);
    }
    Ok(())
}

/// Which of the two lists an editor row belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuList {
    Wheel,
    Header,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveDir {
    Up,
    Down,
}

/// Swap entry `index` of `list` with its neighbour. `None` at either end or out
/// of range, so a caller never saves a no-op.
pub fn reorder(
    layout: &MenuLayout,
    list: MenuList,
    index: usize,
    dir: MoveDir,
) -> Option<MenuLayout> {
    let mut next = layout.clone();
    let entries = match list {
        MenuList::Wheel => &mut next.wheel,
        MenuList::Header => &mut next.header,
    };
    let other = match dir {
        MoveDir::Up => index.checked_sub(1)?,
        MoveDir::Down => index + 1,
    };
    if index >= entries.len() || other >= entries.len() {
        return None;
    }
    entries.swap(index, other);
    Some(next)
}

/// Why an entry cannot change lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveBlocked {
    /// Chat and Settings stay on the wheel (D-05); also returned for an
    /// out-of-range index, where there is nothing to move.
    Locked,
    WheelAtMin,
    WheelAtMax,
}

/// Move entry `index` of `from` to the END of the other list.
pub fn move_entry(
    layout: &MenuLayout,
    from: MenuList,
    index: usize,
) -> Result<MenuLayout, MoveBlocked> {
    let mut next = layout.clone();
    let (src, dst) = match from {
        MenuList::Wheel => (&mut next.wheel, &mut next.header),
        MenuList::Header => (&mut next.header, &mut next.wheel),
    };
    let entry = src.get(index).ok_or(MoveBlocked::Locked)?;
    if is_locked(&entry.screen) {
        return Err(MoveBlocked::Locked);
    }
    match from {
        MenuList::Wheel if src.len() <= WHEEL_MIN => return Err(MoveBlocked::WheelAtMin),
        MenuList::Header if dst.len() >= WHEEL_MAX => return Err(MoveBlocked::WheelAtMax),
        _ => {}
    }
    dst.push(src.remove(index));
    Ok(next)
}

/// The label an entry ships with; what an emptied label field falls back to.
pub fn shipped_label(id: &str) -> Option<String> {
    default_layout()
        .wheel
        .into_iter()
        .chain(default_layout().header)
        .find(|e| e.screen == id)
        .map(|e| e.label)
}

#[derive(Debug, PartialEq, Eq)]
pub enum RenameResult {
    Renamed(MenuLayout),
    /// The field was empty: the shipped label was put back (carried for the announcement).
    Restored(MenuLayout, String),
}

#[derive(Debug, PartialEq, Eq)]
pub struct RenameError;

/// Trim, then accept 1 to `LABEL_MAX_CHARS` chars without control characters.
/// An empty value restores the shipped label. Labels need not be unique.
pub fn rename_entry(
    layout: &MenuLayout,
    list: MenuList,
    index: usize,
    raw: &str,
) -> Result<RenameResult, RenameError> {
    let t = raw.trim();
    if t.chars().count() > LABEL_MAX_CHARS || t.chars().any(char::is_control) {
        return Err(RenameError);
    }
    let mut next = layout.clone();
    let entries = match list {
        MenuList::Wheel => &mut next.wheel,
        MenuList::Header => &mut next.header,
    };
    let entry = entries.get_mut(index).ok_or(RenameError)?;
    if t.is_empty() {
        let shipped = shipped_label(&entry.screen).ok_or(RenameError)?;
        entry.label = shipped.clone();
        return Ok(RenameResult::Restored(next, shipped));
    }
    entry.label = t.to_string();
    Ok(RenameResult::Renamed(next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reorder_moves_one_step_and_refuses_at_ends() {
        let d = default_layout();
        let up = reorder(&d, MenuList::Wheel, 2, MoveDir::Up).unwrap();
        assert_eq!(up.wheel[1].screen, "models");
        assert_eq!(up.wheel[2].screen, "agents");
        assert_eq!(validate(&up), Ok(()));
        let down = reorder(&d, MenuList::Header, 0, MoveDir::Down).unwrap();
        assert_eq!(down.header[1].screen, "kanban");
        assert_eq!(validate(&down), Ok(()));
        assert!(reorder(&d, MenuList::Wheel, 0, MoveDir::Up).is_none());
        assert!(reorder(&d, MenuList::Header, 6, MoveDir::Down).is_none());
        assert!(reorder(&d, MenuList::Wheel, 99, MoveDir::Up).is_none());
    }

    #[test]
    fn move_entry_lands_at_end_and_respects_locks_and_bounds() {
        let d = default_layout();
        let up = move_entry(&d, MenuList::Header, 0).unwrap();
        assert_eq!(up.wheel.len(), 11);
        assert_eq!(up.wheel[10].screen, "kanban");
        assert_eq!(up.header.len(), 6);
        assert_eq!(validate(&up), Ok(()));
        let down = move_entry(&d, MenuList::Wheel, 1).unwrap();
        assert_eq!(down.header.last().unwrap().screen, "agents");
        assert_eq!(validate(&down), Ok(()));
        for id in ["chat", "settings"] {
            let i = d.wheel.iter().position(|e| e.screen == id).unwrap();
            assert_eq!(move_entry(&d, MenuList::Wheel, i), Err(MoveBlocked::Locked));
        }
        let mut min = d.clone();
        for id in ["agents", "models", "tools", "skills"] {
            let i = min.wheel.iter().position(|e| e.screen == id).unwrap();
            min = move_entry(&min, MenuList::Wheel, i).unwrap();
        }
        assert_eq!(min.wheel.len(), 6);
        assert_eq!(validate(&min), Ok(()));
        let i = min.wheel.iter().position(|e| e.screen == "memory").unwrap();
        assert_eq!(
            move_entry(&min, MenuList::Wheel, i),
            Err(MoveBlocked::WheelAtMin)
        );
        assert_eq!(
            move_entry(&up, MenuList::Header, 0),
            Err(MoveBlocked::WheelAtMax)
        );
        assert_eq!(
            move_entry(&d, MenuList::Header, 99),
            Err(MoveBlocked::Locked)
        );
    }

    #[test]
    fn rename_entry_trims_bounds_and_restores() {
        let d = default_layout();
        match rename_entry(&d, MenuList::Wheel, 0, "  Talk  ").unwrap() {
            RenameResult::Renamed(l) => {
                assert_eq!(l.wheel[0].label, "Talk");
                assert_eq!(validate(&l), Ok(()));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(shipped_label("chat").as_deref(), Some("CHAT"));
        assert_eq!(shipped_label("nope"), None);
        let mut renamed = d.clone();
        renamed.wheel[0].label = "Talk".into();
        for blank in ["", "   "] {
            match rename_entry(&renamed, MenuList::Wheel, 0, blank).unwrap() {
                RenameResult::Restored(l, shipped) => {
                    assert_eq!(shipped, "CHAT");
                    assert_eq!(l.wheel[0].label, "CHAT");
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            rename_entry(&d, MenuList::Wheel, 0, "ABCDEFGHIJ"),
            Err(RenameError)
        );
        assert_eq!(
            rename_entry(&d, MenuList::Wheel, 0, "a\u{7}"),
            Err(RenameError)
        );
        assert_eq!(rename_entry(&d, MenuList::Wheel, 99, "x"), Err(RenameError));
        assert!(matches!(
            rename_entry(&d, MenuList::Wheel, 0, "ABCDEFGHI"),
            Ok(RenameResult::Renamed(_))
        ));
    }

    #[test]
    fn default_layout_is_valid() {
        let d = default_layout();
        assert_eq!(validate(&d), Ok(()));
        assert_eq!(d.wheel.len(), 10);
        for (i, e) in d.wheel.iter().enumerate() {
            let w = WheelWedge::from_index(i);
            assert_eq!(e.label, w.label());
            assert_eq!(catalog_entry(&e.screen).unwrap().screen, w.to_screen());
        }
        assert_eq!(d.header.len(), HEADER_SHORTCUTS.len() + 2);
        for (e, (screen, label)) in d.header.iter().zip(HEADER_SHORTCUTS) {
            assert_eq!(e.label, *label);
            assert_eq!(catalog_entry(&e.screen).unwrap().screen, *screen);
        }
    }

    fn mv(l: &mut MenuLayout, id: &str, to_header: bool) {
        let pos = l.wheel.iter().position(|e| e.screen == id).unwrap();
        let e = l.wheel.remove(pos);
        if to_header {
            l.header.push(e);
        }
    }

    fn drop_id(l: &mut MenuLayout, id: &str) {
        l.wheel.retain(|e| e.screen != id);
        l.header.retain(|e| e.screen != id);
    }

    #[test]
    fn validate_rejects_unknown_screen() {
        let mut l = default_layout();
        l.wheel[1].screen = "nope".into();
        assert_eq!(
            validate(&l),
            Err(MenuLayoutError::UnknownScreen("nope".into()))
        );
        l.wheel[1].screen = "x".repeat(40);
        assert_eq!(
            validate(&l),
            Err(MenuLayoutError::UnknownScreen("x".repeat(32)))
        );
    }

    #[test]
    fn validate_rejects_missing_chat() {
        let mut l = default_layout();
        drop_id(&mut l, "chat");
        assert_eq!(validate(&l), Err(MenuLayoutError::ChatMissing));
    }

    #[test]
    fn validate_rejects_missing_settings() {
        let mut l = default_layout();
        drop_id(&mut l, "settings");
        assert_eq!(validate(&l), Err(MenuLayoutError::SettingsMissing));
    }

    #[test]
    fn validate_rejects_locked_entry_off_wheel() {
        for id in ["chat", "settings"] {
            let mut l = default_layout();
            mv(&mut l, id, true);
            assert_eq!(validate(&l), Err(MenuLayoutError::LockedOffWheel), "{id}");
        }
    }

    #[test]
    fn validate_rejects_wheel_count_outside_6_to_11() {
        let mut l = default_layout();
        for id in ["agents", "models", "tools", "skills", "memory"] {
            mv(&mut l, id, true);
        }
        assert_eq!(l.wheel.len(), 5);
        assert_eq!(validate(&l), Err(MenuLayoutError::WheelCount(5)));
        let mut l = default_layout();
        for id in ["kanban", "artifacts"] {
            let pos = l.header.iter().position(|e| e.screen == id).unwrap();
            let e = l.header.remove(pos);
            l.wheel.push(e);
        }
        assert_eq!(l.wheel.len(), 12);
        assert_eq!(validate(&l), Err(MenuLayoutError::WheelCount(12)));
    }

    #[test]
    fn validate_accepts_6_and_11_section_wheels() {
        let mut l = default_layout();
        for id in ["agents", "models", "tools", "skills"] {
            mv(&mut l, id, true);
        }
        assert_eq!(l.wheel.len(), 6);
        assert_eq!(validate(&l), Ok(()));
        let mut l = default_layout();
        let e = l.header.remove(0);
        l.wheel.push(e);
        assert_eq!(l.wheel.len(), 11);
        assert_eq!(validate(&l), Ok(()));
    }

    #[test]
    fn validate_rejects_unreachable_screen() {
        let mut l = default_layout();
        drop_id(&mut l, "kanban");
        assert_eq!(validate(&l), Err(MenuLayoutError::Unreachable("Kanban")));
    }

    #[test]
    fn validate_rejects_duplicates_and_bad_labels() {
        let mut l = default_layout();
        l.header.push(l.header[0].clone());
        assert_eq!(validate(&l), Err(MenuLayoutError::Unreadable));
        for bad in ["", "   ", "0123456789", "a\u{7}b"] {
            let mut l = default_layout();
            l.wheel[0].label = bad.into();
            assert_eq!(validate(&l), Err(MenuLayoutError::Unreadable), "{bad:?}");
        }
        let mut l = default_layout();
        l.version = MENU_LAYOUT_VERSION + 1;
        assert_eq!(validate(&l), Err(MenuLayoutError::Unreadable));
    }

    #[test]
    fn error_messages_match_ui_spec_clauses() {
        use MenuLayoutError::*;
        assert_eq!(
            UnknownScreen("q".into()).to_string(),
            "unknown screen \"q\""
        );
        assert_eq!(ChatMissing.to_string(), "Chat is missing");
        assert_eq!(SettingsMissing.to_string(), "Settings is missing");
        assert_eq!(
            LockedOffWheel.to_string(),
            "Chat or Settings is off the wheel"
        );
        assert_eq!(
            WheelCount(5).to_string(),
            "wheel has 5 sections (allowed 6 to 11)"
        );
        assert_eq!(Unreachable("Kanban").to_string(), "Kanban is not reachable");
        assert_eq!(Unreadable.to_string(), "could not be read");
        assert_eq!(
            invalid_notice(&ChatMissing),
            "Saved menu layout ignored: Chat is missing. Showing the default layout."
        );
    }

    #[test]
    fn catalog_matches_shipped_wheel_and_header() {
        assert_eq!(MENU_CATALOG.len(), 17);
        let mut ids: Vec<_> = MENU_CATALOG.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 17);
        for i in 0..10 {
            let w = WheelWedge::from_index(i);
            let c = catalog_entry_for(w.to_screen()).unwrap();
            assert_eq!((c.glyph, c.sub), (w.glyph(), w.sub()));
        }
        for (screen, _) in HEADER_SHORTCUTS {
            assert!(catalog_entry_for(*screen).is_some());
        }
        assert!(is_locked("chat") && is_locked("settings") && !is_locked("logs"));
    }

    #[test]
    fn default_layout_has_openui_and_dashboard_last_in_header() {
        let d = default_layout();
        let ids: Vec<&str> = d.header.iter().map(|e| e.screen.as_str()).collect();
        assert_eq!(
            ids,
            [
                "kanban",
                "artifacts",
                "schedules",
                "soul",
                "logs",
                "openui",
                "dashboard"
            ]
        );
        assert_eq!(d.version, 3);
        assert_eq!(d.header[5].label, "OpenUI");
        assert_eq!(d.header[6].label, "Dashboard");
        assert_eq!(MENU_CATALOG.len(), 17);
        let c = catalog_entry("openui").unwrap();
        assert_eq!(
            (c.glyph, c.sub, c.name, c.added_in, c.screen),
            ("❖", "GENERATED VIEWS", "OpenUI", 2, Screen::OpenUi)
        );
        let c = catalog_entry("dashboard").unwrap();
        assert_eq!(
            (c.glyph, c.sub, c.name, c.added_in, c.screen),
            ("◫", "LIVE BOARDS", "Dashboard", 3, Screen::Dashboard)
        );
        // HEADER_SHORTCUTS keeps its five literal entries (pinned by the kanban tests).
        assert_eq!(HEADER_SHORTCUTS.len(), 5);
    }

    #[test]
    fn migrate_appends_new_entries_to_header_end() {
        // A valid version-1 layout: reordered, renamed, one header entry moved to the wheel.
        let mut v1 = default_layout();
        v1.version = 1;
        v1.header.pop(); // dashboard is not in a version-1 layout
        v1.header.pop(); // openui is not in a version-1 layout
        v1.header.reverse();
        v1.wheel[1].label = "Bots".into();
        let moved = v1.header.remove(0);
        v1.wheel.push(moved);
        let before = v1.clone();
        assert_eq!(
            validate(&v1),
            Err(MenuLayoutError::Unreadable),
            "v1 no longer validates as-is"
        );

        let m = migrate(v1);
        assert_eq!(m.version, 3);
        let n = m.header.len();
        assert_eq!(m.header[n - 2].screen, "openui");
        assert_eq!(m.header[n - 2].label, "OpenUI");
        assert_eq!(m.header[n - 1].screen, "dashboard");
        assert_eq!(m.header[n - 1].label, "Dashboard");
        assert_eq!(m.wheel, before.wheel);
        assert_eq!(&m.header[..n - 2], &before.header[..]);
        assert_eq!(validate(&m), Ok(()));
        // Idempotent, and a current layout is untouched.
        assert_eq!(migrate(m.clone()), m);
        // Hostile versions are left for validate to reject.
        for v in [0, MENU_LAYOUT_VERSION + 1] {
            let mut bad = m.clone();
            bad.version = v;
            assert_eq!(migrate(bad.clone()), bad);
            assert_eq!(validate(&bad), Err(MenuLayoutError::Unreadable));
        }
    }

    #[test]
    fn migrate_v2_layout_appends_dashboard() {
        let mut v2 = default_layout();
        v2.version = 2;
        v2.header.pop(); // dashboard is not in a version-2 layout
        v2.header[0].label = "Board".into();
        let before = v2.clone();
        let m = migrate(v2);
        assert_eq!(m.version, 3);
        assert_eq!(m.header.last().unwrap().screen, "dashboard");
        assert_eq!(m.header.last().unwrap().label, "Dashboard");
        assert_eq!(&m.header[..m.header.len() - 1], &before.header[..]);
        assert_eq!(m.wheel, before.wheel);
        assert_eq!(validate(&m), Ok(()));
    }

    #[test]
    fn validate_rejects_missing_dashboard_in_v3() {
        let mut l = default_layout();
        drop_id(&mut l, "dashboard");
        assert_eq!(validate(&l), Err(MenuLayoutError::Unreachable("Dashboard")));
    }

    #[test]
    fn validate_rejects_missing_openui_in_v2() {
        let mut l = default_layout();
        drop_id(&mut l, "openui");
        assert_eq!(validate(&l), Err(MenuLayoutError::Unreachable("OpenUI")));
    }
}
