//! Phase 50.1 Plan 01 (D-08/D-09/D-11/D-13): the bot roster section, mounted
//! by `ScreenAgents` directly beneath the screen header and ABOVE the
//! pre-existing subagent-turn grid (D-09 ordering — that grid, `AgentCard`,
//! `TurnCard`, PRUNE ENDED, CANCEL and the HOLD-N decay sweep are untouched
//! by this plan).
//!
//! Data source: `list_profiles()` (existing enumeration), `list_bot_meta()`
//! (this plan's UI-owned display-metadata cache — a single whole-map read,
//! never a per-profile fetch, per D-13) and `live_profile_name()` (D-04's
//! server-derived LIVE identity), joined client-side by profile name into
//! `BotRosterEntry` rows. `refresh_tick` is read in the SYNC prefix of each
//! `use_resource`, the crate's proven refresh idiom (`profile_switcher.rs:
//! 55-58`) — never the other Dioxus resource hook plus a `.restart()` call.
//!
//! Task 2 completes the UI-SPEC State Matrix (loading / empty / error rows
//! with Copywriting Contract copy verbatim); this task renders the
//! populated/loading/error/empty shapes with the empty-state inline CTA.
//!
//! Phase 50.2 Plan 01 (D-21): the group-chat social layer — a fourth
//! `use_resource` (SAME `refresh_tick` sync-prefix idiom) joins in every
//! group-chat room; `open_room`/`create_room_open` are owned here exactly
//! like `drawer_target`/`chat_target` above. A room's own actions
//! (create/send) never trigger the other Dioxus resource hook either — they
//! bump this module's own local `rooms_refresh_tick` instead.

use crate::components::hermes_app::screens::bot_roster::group_settings::GroupSettingsDrawer;
use crate::components::hermes_app::screens::profile_shared::edit_dialog::ProfileDetailDrawer;
use crate::components::hermes_app::screens::profile_shared::ProfileDialogContext;
use crate::components::hermes_app::widgets::active_now_strip::ActiveNowStrip;
use crate::protocol::{BotBinding, BotMeta, BotRosterEntry, ProfileRow};
use crate::server::bot_binding_api::{list_bot_bindings, set_bot_binding};
use crate::server::bot_meta_api::{list_bot_meta, live_profile_name};
use crate::server::group_chat_api::list_group_rooms;
use crate::server::profile_api::list_profiles;
use dioxus::prelude::*;

pub mod card;
pub mod chat_window;
// Phase 50.2 Plan 01 (D-21/UI-SPEC): the group-chat social layer — Create
// Room modal, one room's roster row, and the room workspace that replaces
// the grid in place when a room is open.
pub mod create_room_modal;
pub mod delete_confirm;
// Phase 50.2 Plan 06 (D-18 divergence, UI-SPEC Component Inventory §8):
// Delete Room's confirmation modal — a sibling file of
// `group_chat_workspace.rs` (which is where it is actually mounted, from
// the room header's overflow control). Rust's module system has no path
// that registers a sibling file without a `mod` line in its parent module
// file — the same structurally-unavoidable declaration plan 05's own
// `group_settings` module needed.
pub mod delete_room_confirm;
// Phase 50.2 Plan 17 (G-50.2-2a): the `Edit members` modal — a sibling file
// of `group_chat_workspace.rs` (where it is actually mounted, from the room
// header's overflow control, next to `Delete room`). Rust's module system
// has no path that registers a sibling file without a `mod` line in its
// parent module file — the same structurally-unavoidable declaration
// `delete_room_confirm`/`group_settings` above already needed.
pub mod edit_members_modal;
pub mod group_chat_workspace;
pub mod group_row;
// Phase 50.2 Plan 05 (D-21): the Group Chat Settings drawer
// (`GroupSettingsDrawer`) — required module declaration for the new
// sibling file `group_settings.rs`; Rust's module system has no path that
// registers this file without a `mod` line in its parent. This plan's own
// files_modified list does not name `bot_roster.rs`, but the declaration
// is unavoidable — recorded as a Rule 3 auto-fix (blocking issue: missing
// module declaration) in the plan summary. Mounted from
// `group_chat_workspace.rs`'s room header this plan; plan 06 adds the
// roster-header entry point when it owns this file's own mounts.
pub mod group_settings;
// Phase 50.2 Plan 07 (D-20, UI-SPEC Component Inventory §2): the shared
// `@mention` handoff quote block (`MentionHandoffBlock`) — required module
// declaration for the new sibling file `mention_handoff.rs`; Rust's module
// system has no path that registers this file without a `mod` line in its
// parent. This plan's own `files_modified` list does not name
// `bot_roster.rs`, but the declaration is unavoidable — recorded as a
// Rule 3 auto-fix (blocking issue: missing module declaration), the exact
// same precedent `group_settings`/`delete_room_confirm` above already
// established for this crate.
pub mod mention_handoff;
pub mod npub_row;
pub mod routines;

// Under `--all-features`, `legacy-shell` swaps the root to WarpHermes,
// leaving HermesApp (and therefore BotRoster) unreferenced — a binary
// crate's unreferenced `const` does not suppress dead_code, so the
// `-D warnings` clippy gate fires there even though this is live in the
// default (non-legacy-shell) build. Same pattern as `KANBAN_CSS` in
// `screens/kanban.rs` and `ARTIFACT_SANDBOX` in `screens/artifact_viewer.rs`.
#[allow(dead_code)]
const BOTS_CSS: Asset = asset!("/assets/bots.css");

/// Phase 50.1 OF-4 (gap task): pure join — folds each `ProfileRow` into a
/// `BotRosterEntry`, joining the kanban-worker badge flag from the
/// SEPARATE `kanban_worker_names` set (server/bot_meta_api.rs's
/// `list_bot_meta` computes this with ONE `fetch_board` read, never a
/// per-profile fan-out — D-13) and applying the single `unassigned`-hides
/// filter point directed by OF-4's pending sub-decision. Pure and
/// disk/DOM-I/O-free so the join/filter logic is directly unit-testable
/// without a renderer.
//
// Under `--all-features`, `legacy-shell` swaps the root to WarpHermes,
// leaving HermesApp (and therefore BotRoster and this helper) unreferenced
// from `main()` — same reachability gap as `BOTS_CSS` above and
// `card.rs`'s `NO_CONVERSATIONS_YET` doc note; the `#[allow(dead_code)]`
// preempts the `-D warnings` clippy gate that fires there even though this
// is live in the default (non-legacy-shell) build.
#[allow(dead_code)]
pub(crate) fn build_roster_entries(
    profiles: Vec<ProfileRow>,
    meta_map: &std::collections::BTreeMap<String, BotMeta>,
    kanban_worker_names: &std::collections::BTreeSet<String>,
    live_name: Option<&str>,
) -> Vec<BotRosterEntry> {
    profiles
        .into_iter()
        // OF-4 sub-decision (pending operator confirmation; recommended
        // default applied here): the `unassigned` placeholder assignee
        // profile is hidden from the bot roster entirely. It remains
        // fully visible, untouched, on the Kanban screen — this filter
        // only affects THIS roster's join. TO REVERT (show it badged like
        // any other board-assignee profile instead of hiding it): delete
        // this `.filter(...)` call.
        .filter(|row| row.name != "unassigned")
        .map(|row| {
            let is_live = live_name == Some(row.name.as_str());
            let is_kanban_worker = kanban_worker_names.contains(&row.name);
            let meta = meta_map.get(&row.name).cloned();
            BotRosterEntry {
                row,
                meta,
                is_live,
                is_kanban_worker,
            }
        })
        .collect()
}

/// Phase 50.2 Plan 03 (D-11/D-21, UI-SPEC Component Inventory §3): one
/// labeled roster section. `label: None` marks the flat-grid case — the
/// single section returned when no group label exists anywhere in the
/// roster (see `build_roster_sections`'s own doc for the exact
/// single-vs-multi-section rule the render side keys off).
#[allow(dead_code)] // used in BotRoster rsx!; dead_code fires under --all-features (legacy-shell)
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RosterSection {
    pub label: Option<String>,
    pub entries: Vec<RosterSectionEntry>,
}

/// Phase 50.2 Plan 03: one card-grid slot inside a `RosterSection` — either
/// a bot or a group-chat room, joined into the SAME section by their
/// respective `group` field (D-03 behavioral reference: rooms never get
/// their own "Rooms" section, they interleave with bot cards).
#[allow(dead_code)] // used in BotRoster rsx!; dead_code fires under --all-features (legacy-shell)
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RosterSectionEntry {
    Bot(BotRosterEntry),
    Room(crate::protocol::GroupRoomSummary),
}

/// Phase 50.2 Plan 03 (UI-SPEC Copywriting Contract): the trailing
/// section's exact label string — used only when at least one OTHER
/// labeled section exists in the same result (see `build_roster_sections`).
#[allow(dead_code)] // used in build_roster_sections; dead_code fires under --all-features (legacy-shell)
pub(crate) const UNGROUPED_SECTION_LABEL: &str = "UNGROUPED";

/// Phase 50.2 Plan 03 (UI-SPEC Component Inventory §3): sections the
/// already-joined roster entries and room summaries by their `group` field.
/// Pure and disk/DOM-I/O-free so the ordering rules are directly
/// unit-testable without a renderer.
///
/// Rules:
/// - one section per distinct group label, in FIRST-SEEN order over the
///   input slices (bots then rooms) — never alphabetical;
/// - every bot/room with no label collects into one trailing bucket;
/// - a room joins by its OWN `group` field — never a separate "Rooms"
///   section (D-03);
/// - if the trailing bucket is the ONLY thing to render (no label appears
///   anywhere in either input), it is returned as the SOLE section with
///   `label: None` — the caller renders 50.1's flat grid unchanged: no
///   header, no separator, and the `UNGROUPED_SECTION_LABEL` string never
///   appears anywhere in the result (State Matrix: "no redundant Ungrouped
///   label on a roster that has no groups at all");
/// - otherwise the trailing bucket is appended LAST with
///   `label: Some(UNGROUPED_SECTION_LABEL)`, and only when it is
///   non-empty — an empty section would render a header/count-chip over
///   zero cards, the same "dimmed placeholder for an absent state" pattern
///   this crate's own LIVE-badge/needs-you convention rules out elsewhere
///   (absence, not a placeholder).
#[allow(dead_code)] // used in BotRoster rsx!; dead_code fires under --all-features (legacy-shell)
pub(crate) fn build_roster_sections(
    entries: &[BotRosterEntry],
    rooms: &[crate::protocol::GroupRoomSummary],
) -> Vec<RosterSection> {
    let mut label_order: Vec<String> = Vec::new();
    let mut labeled: std::collections::HashMap<String, Vec<RosterSectionEntry>> =
        std::collections::HashMap::new();
    let mut ungrouped: Vec<RosterSectionEntry> = Vec::new();

    for entry in entries {
        match entry.meta.as_ref().and_then(|m| m.group.clone()) {
            Some(label) => {
                if !label_order.contains(&label) {
                    label_order.push(label.clone());
                }
                labeled
                    .entry(label)
                    .or_default()
                    .push(RosterSectionEntry::Bot(entry.clone()));
            }
            None => ungrouped.push(RosterSectionEntry::Bot(entry.clone())),
        }
    }
    for room in rooms {
        match room.group.clone() {
            Some(label) => {
                if !label_order.contains(&label) {
                    label_order.push(label.clone());
                }
                labeled
                    .entry(label)
                    .or_default()
                    .push(RosterSectionEntry::Room(room.clone()));
            }
            None => ungrouped.push(RosterSectionEntry::Room(room.clone())),
        }
    }

    if label_order.is_empty() {
        return vec![RosterSection {
            label: None,
            entries: ungrouped,
        }];
    }

    let mut sections: Vec<RosterSection> = label_order
        .into_iter()
        .map(|label| {
            let section_entries = labeled.remove(&label).unwrap_or_default();
            RosterSection {
                label: Some(label),
                entries: section_entries,
            }
        })
        .collect();
    if !ungrouped.is_empty() {
        sections.push(RosterSection {
            label: Some(UNGROUPED_SECTION_LABEL.to_string()),
            entries: ungrouped,
        });
    }
    sections
}

/// Phase 50.2 Plan 03: renders one `RosterSectionEntry` — the SAME
/// `BotCard`/`GroupRow` mount either the single-section (flat 50.1 grid) or
/// the multi-section render branch uses in `BotRoster`, so the Bot/Room
/// match arm is written exactly once. Mirrors `edit_dialog.rs`'s own
/// `render_verify_doctor_block` precedent (a plain fn returning `Element`,
/// interpolated via `{fn_call(...)}`).
#[allow(dead_code)] // used in BotRoster rsx!; dead_code fires under --all-features (legacy-shell)
fn render_roster_section_entry(
    section_entry: RosterSectionEntry,
    meta_map: &std::collections::BTreeMap<String, BotMeta>,
    drawer_target: Signal<Option<String>>,
    chat_target: Signal<Option<String>>,
    mut open_room: Signal<Option<String>>,
) -> Element {
    match section_entry {
        RosterSectionEntry::Bot(entry) => rsx! {
            card::BotCard {
                key: "{entry.row.name}",
                entry,
                drawer_target,
                chat_target,
            }
        },
        RosterSectionEntry::Room(room) => rsx! {
            group_row::GroupRow {
                key: "{room.id}",
                room,
                meta_map: meta_map.clone(),
                on_open: move |room_id: String| {
                    open_room.set(Some(room_id));
                },
            }
        },
    }
}

/// Phase 50.1 Plan 01: the roster section. `on_create` is invoked by the
/// empty-state inline CTA and opens the SAME create flow the `+ NEW AGENT`
/// header button opens (an empty grid is never a dead end).
///
/// Phase 50.1 Plan 02 Task 3 (D-10 extension): `drawer_target` is owned by
/// the caller (`ScreenAgents`, mirroring `kanban.rs`'s own `detail_profile`
/// idiom) so the create wizard's post-create hand-off — mounted as a
/// sibling of this component in `agents.rs`, not inside it — can open the
/// same drawer instance a roster card's `EDIT` action opens. Passed down
/// as a plain `Signal<Option<String>>` prop, never a context provider.
/// `on_profile_updated` bumps the caller's own refresh tick after a drawer
/// save, since this component only receives a `ReadSignal`.
#[component]
pub fn BotRoster(
    refresh_tick: ReadSignal<u32>,
    on_create: EventHandler<()>,
    drawer_target: Signal<Option<String>>,
    on_profile_updated: EventHandler<()>,
) -> Element {
    // Phase 50.1 Plan 01: `refresh_tick` READ in the SYNC prefix of each
    // `use_resource` closure (call syntax subscribes) before the
    // `async move` — the same shape `ProfileSwitcher` uses
    // (`profile_switcher.rs:55-58`). A bump re-runs all three fetches.
    let profiles_resource = use_resource(move || {
        let _tick = refresh_tick();
        async move { list_profiles().await }
    });
    let bot_meta_resource = use_resource(move || {
        let _tick = refresh_tick();
        async move { list_bot_meta().await }
    });
    let live_resource = use_resource(move || {
        let _tick = refresh_tick();
        async move { live_profile_name().await }
    });
    // Phase 50.2 Plan 01: the fourth `use_resource`, SAME `refresh_tick`
    // sync-prefix idiom as the three above. `rooms_refresh_tick` is a
    // SECOND, workspace-local tick this component owns itself (never
    // exposed as a prop) — `refresh_tick` above is a caller-owned
    // `ReadSignal` this component cannot write to, but a room's own
    // create/send actions originate entirely within this component's
    // subtree and need to force-refetch the room list without a caller
    // round-trip. Reading BOTH ticks in the sync prefix means either one
    // bumping re-runs this fetch — never the other Dioxus resource hook
    // plus a `.restart()` call.
    let rooms_refresh_tick: Signal<u32> = use_signal(|| 0);
    let group_rooms_resource = use_resource(move || {
        let _tick = refresh_tick();
        let _rooms_tick = rooms_refresh_tick();
        async move { list_group_rooms().await }
    });

    // Extract data BEFORE rsx! — signal-borrow discipline per
    // iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let is_loading = profiles_resource().is_none()
        || bot_meta_resource().is_none()
        || live_resource().is_none()
        || group_rooms_resource().is_none();
    let load_error = matches!(profiles_resource(), Some(Err(_)))
        || matches!(bot_meta_resource(), Some(Err(_)))
        || matches!(group_rooms_resource(), Some(Err(_)));

    let profiles: Vec<ProfileRow> = match profiles_resource() {
        Some(Ok(rows)) => rows,
        _ => Vec::new(),
    };
    let (meta_map, kanban_worker_names): (
        std::collections::BTreeMap<String, BotMeta>,
        std::collections::BTreeSet<String>,
    ) = match bot_meta_resource() {
        Some(Ok(response)) => (response.meta, response.kanban_worker_names),
        _ => (
            std::collections::BTreeMap::new(),
            std::collections::BTreeSet::new(),
        ),
    };
    let live_name: Option<String> = match live_resource() {
        Some(Ok(name)) => Some(name),
        _ => None,
    };

    // Phase 49.4 Plan 12: `PlatformBindings`' own PROFILE: selector needs
    // the full profile-name list too — cloned here, BEFORE `profiles` is
    // moved into `build_roster_entries` below.
    let profiles_for_bindings = profiles.clone();

    let entries: Vec<BotRosterEntry> =
        build_roster_entries(profiles, &meta_map, &kanban_worker_names, live_name.as_deref());
    let is_empty = entries.is_empty();

    // Phase 50.2 Plan 01: the room list, joined the same way the three bot
    // resources above are — a plain `Vec` extracted before `rsx!`.
    let rooms: Vec<crate::protocol::GroupRoomSummary> = match group_rooms_resource() {
        Some(Ok(rows)) => rows,
        _ => Vec::new(),
    };

    // Phase 50.2 Plan 03 (D-11/D-21): the roster's group sectioning — a
    // pure client-side re-layout of the already-loaded `entries`/`rooms`,
    // computed BEFORE `rsx!` per this crate's signal-borrow discipline.
    // `multi_section` is the render fork's single decision point: true
    // renders N labeled `.kn-roster-group-section` blocks, false renders
    // 50.1's unchanged flat `.grid.wide`.
    let sections = build_roster_sections(&entries, &rooms);
    let multi_section = sections.len() > 1;

    // Phase 50.2 Plan 01: `open_room`/`create_room_open` are owned HERE at
    // the roster screen level — mirrors `drawer_target`/`chat_target`'s own
    // ownership placement exactly (Signal state owned by the screen,
    // threaded down as plain props, never a context provider).
    let open_room: Signal<Option<String>> = use_signal(|| None);
    let create_room_open: Signal<bool> = use_signal(|| false);
    let open_room_id: Option<String> = open_room.read().clone();
    // Phase 50.2 Plan 06 (D-21, UI-SPEC Navigation & Placement): the
    // roster section header's OWN settings gear — a second entry point
    // onto the SAME persisted `GroupChatSettings` record the room
    // workspace's own gear (`group_chat_workspace.rs`) already opens, each
    // owning its own local `Signal<bool>` and its own `GroupSettingsDrawer`
    // instance (plan 05's own recorded interpretation: two entry points,
    // never one signal shared across two different files/plans).
    let mut roster_settings_open: Signal<bool> = use_signal(|| false);

    // Phase 50.1 OF-5 (gap task): the floating chat window's target + its
    // per-bot session-transient thread state, owned HERE at the roster
    // screen level — mirrors `drawer_target`'s ownership placement exactly
    // (Signal state owned by the screen, threaded down as plain props,
    // never a context provider). Neither signal is written to disk; both
    // reset to empty the moment this component unmounts (e.g. navigating
    // away from the Agents screen), which is the intended session-transient
    // lifetime (D-01's PERSISTENT canonical conversation is Phase 50.2).
    let chat_target: Signal<Option<String>> = use_signal(|| None);
    let chat_threads: Signal<std::collections::BTreeMap<String, Vec<chat_window::ChatWindowEntry>>> =
        use_signal(std::collections::BTreeMap::new);
    let chat_entry: Option<BotRosterEntry> = chat_target
        .read()
        .as_deref()
        .and_then(|name| entries.iter().find(|e| e.row.name == name))
        .cloned();

    rsx! {
        div { class: "kn-bot-roster",
            document::Link { rel: "stylesheet", href: BOTS_CSS }
            // Phase 50.2 Plan 01 (UI-SPEC "drill into a room"): when a room
            // is open, it REPLACES the roster grid (and the presence strip
            // above it) in place — a multi-round transcript needs the full
            // vertical space a `.kn-modal`'s bounded height cannot give it.
            // `key: "{room_id}"` forces a fresh mount (fresh `use_resource`/
            // `Signal` state) on every distinct room, since `open_room`
            // changing from `Some(a)` to `Some(b)` without passing through
            // `None` would otherwise reuse the same component instance.
            if let Some(room_id) = open_room_id {
                group_chat_workspace::GroupChatWorkspace {
                    key: "{room_id}",
                    room_id: room_id.clone(),
                    meta_map: meta_map.clone(),
                    // Phase 50.2 Plan 07 (D-20): every actual bot in the
                    // roster — `mention_targets_for_room`'s roster filter.
                    roster_names: entries.iter().map(|e| e.row.name.clone()).collect::<Vec<String>>(),
                    // Phase 50.2 Plan 17 (G-50.2-2a): the SAME joined roster
                    // this screen already computed — `EditMembersModal`'s
                    // picker source, threaded down rather than re-fetched.
                    roster_entries: entries.clone(),
                    on_back: move |_| {
                        let mut target = open_room;
                        target.set(None);
                        // A room's own send/preview/needs-you state may have
                        // changed while the operator was inside it — refetch
                        // the room list on return so the row reflects it.
                        let mut tick = rooms_refresh_tick;
                        let cur = *tick.read();
                        tick.set(cur + 1);
                    },
                }
            } else {
                // Phase 50.1 Plan 08 (D-19): the presence strip, positioned
                // above the card grid. Fed the SAME joined `entries` the grid
                // below reads — empty during a fetch and after an enumeration
                // failure, which is what makes the strip's loading/error
                // "renders nothing" behavior automatic rather than a second
                // state check.
                ActiveNowStrip { roster: entries.clone() }
                // Phase 50.2 Plan 01: `+ NEW ROOM` sits beside `+ NEW AGENT`
                // — the header button itself lives in `agents.rs` (outside
                // this task's file scope), so this row renders directly
                // beneath it as the closest achievable "beside" placement.
                div {
                    style: "display: flex; justify-content: flex-end; gap: var(--sp-2); margin-bottom: var(--sp-2);",
                    button {
                        class: "kn-action-btn",
                        "aria-label": "Group chat settings",
                        onclick: move |_| roster_settings_open.set(true),
                        "⚙"
                    }
                    button {
                        class: "btn btn--sm",
                        onclick: move |_| {
                            let mut open = create_room_open;
                            open.set(true);
                        },
                        "+ NEW ROOM"
                    }
                }
                // Phase 49.4 Plan 12 (D-16): the roster-side half of the
                // profile-to-bot binding editor — an ADDITIVE panel for the
                // fixed ≤6 platform-adapter keys, each with its own PROFILE:
                // selector. Deliberately NOT retrofitted onto every existing
                // roster row: those rows already ARE profiles (per the
                // checkpoint decision's own framing), so a "which profile"
                // selector on a profile row would be circular. Independent
                // of the roster grid's own loading/empty/error state below —
                // it fetches and renders regardless of how many (or how
                // few) profile rows exist.
                PlatformBindings { profiles: profiles_for_bindings }
                if is_loading {
                    div { class: "kn-drawer-loading", "Loading bots…" }
                } else if load_error {
                    div { class: "kn-modal-error",
                        "Could not read ~/.ironhermes/profiles/. Check permissions and retry."
                    }
                } else if is_empty {
                    div { class: "kn-drawer-empty",
                        div { "No bots yet." }
                        div {
                            "Create one to give it its own look, chat, and schedule — press + NEW AGENT above."
                        }
                        button {
                            class: "btn btn--sm",
                            onclick: move |_| on_create.call(()),
                            "+ NEW AGENT"
                        }
                    }
                } else if multi_section {
                    // Phase 50.2 Plan 03 (UI-SPEC Component Inventory §3):
                    // N labeled `.kn-roster-group-section` blocks, one per
                    // distinct group label plus a trailing Ungrouped
                    // section — never a separate "Rooms" section, rooms
                    // interleave into their own labeled section by their
                    // OWN `group` field (D-03).
                    for section in sections.clone().into_iter() {
                        {
                            let label = section.label.clone().unwrap_or_default();
                            let count = section.entries.len();
                            rsx! {
                                div { class: "kn-roster-group-section", key: "{label}",
                                    div { class: "kn-roster-group-header",
                                        span { "{label}" }
                                        span { class: "kn-roster-group-count", "{count}" }
                                    }
                                    div { class: "grid wide",
                                        for section_entry in section.entries.iter().cloned() {
                                            {render_roster_section_entry(section_entry, &meta_map, drawer_target, chat_target, open_room)}
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // Phase 50.2 Plan 03 (State Matrix: "no groups assigned
                    // yet" -> no header/separator rendered, behaves exactly
                    // like 50.1's flat grid): `sections` is the single
                    // `label: None` section in this branch.
                    div { class: "grid wide",
                        for section_entry in sections.clone().into_iter().flat_map(|s| s.entries) {
                            {render_roster_section_entry(section_entry, &meta_map, drawer_target, chat_target, open_room)}
                        }
                    }
                }
                // Phase 50.1 Plan 02 Task 3 (D-10 extension): the shared detail
                // drawer, mounted the way `kanban.rs:676` mounts it today —
                // unconditionally, rendering nothing until `drawer_target` is
                // `Some`. Opened by a roster card's `EDIT` action (`card.rs`)
                // and by the create wizard's post-create hand-off (`agents.rs`).
                ProfileDetailDrawer {
                    profile_id: drawer_target,
                    context: ProfileDialogContext::Bot,
                    on_close: move |_| {
                        let mut target = drawer_target;
                        target.set(None);
                    },
                    on_profile_updated: move |_| on_profile_updated.call(()),
                }
                // Phase 50.1 OF-5 (gap task): the floating one-on-one chat
                // window — renders nothing until `chat_target` names a bot still
                // present in `entries` (same "renders nothing until targeted"
                // idiom as `ProfileDetailDrawer` above; a bot removed mid-chat
                // simply closes the window rather than rendering stale data).
                if let Some(entry) = chat_entry {
                    chat_window::BotChatWindow {
                        entry,
                        chat_threads,
                        // Phase 50.2 Plan 07 (D-20): every actual bot in the
                        // roster — `resolve_roster_mentions_client`'s roster
                        // filter, so a typo or unrelated `@handle` never
                        // dispatches a subprocess.
                        roster_names: entries.iter().map(|e| e.row.name.clone()).collect::<Vec<String>>(),
                        on_close: move |_| {
                            let mut target = chat_target;
                            target.set(None);
                        },
                        // A successful send bumps the SAME refresh mechanism a
                        // drawer save / plan 03's composer already used — no
                        // second refresh idiom — so the card's D-13 preview line
                        // repaints with the sanitized reply.
                        on_message_sent: move |_| on_profile_updated.call(()),
                    }
                }
                // Phase 50.2 Plan 01 (UI-SPEC Component Inventory §5):
                // renders nothing until `create_room_open` is `true` — same
                // "unconditional mount, renders nothing until targeted"
                // idiom as `ProfileDetailDrawer`/`BotChatWindow` above.
                if *create_room_open.read() {
                    create_room_modal::CreateRoomModal {
                        roster_entries: entries.clone(),
                        on_close: move |_| {
                            let mut open = create_room_open;
                            open.set(false);
                        },
                        on_created: move |room_id: String| {
                            let mut open = create_room_open;
                            open.set(false);
                            let mut target = open_room;
                            target.set(Some(room_id));
                            let mut tick = rooms_refresh_tick;
                            let cur = *tick.read();
                            tick.set(cur + 1);
                        },
                    }
                }
                // Phase 50.2 Plan 06: the roster section header's own
                // settings entry point — mounted unconditionally, renders
                // nothing while `roster_settings_open` is `false` (D-10
                // mount-unconditionally idiom, same as `GroupSettingsDrawer`'s
                // other mount site in `group_chat_workspace.rs`).
                GroupSettingsDrawer { open: roster_settings_open }
            }
        }
    }
}

/// Phase 49.4 Plan 12 (D-16): the fixed, closed platform-adapter key set —
/// duplicated from `bot_binding_api::PLATFORM_KEYS` (that `const` lives in
/// a `#[cfg(feature = "server")]`-agnostic but native-only-CALLER module;
/// this component runs on the wasm client too, so it carries its own
/// literal copy per this crate's own established small-constant-
/// duplication precedent).
const PLATFORM_KEYS: [&str; 6] = ["telegram", "discord", "slack", "buzz", "webhook", "api_server"];

/// Phase 49.4 Plan 12 (UI-SPEC E14 partial): the literal profile name an
/// unbound bot displays — duplicated from `bot_binding_api::DEFAULT_BOUND_PROFILE`
/// for the same wasm-client-reachability reason `PLATFORM_KEYS` above is.
const DEFAULT_PROFILE_LABEL: &str = "default";

/// Phase 49.4 Plan 12 (D-16, UI-SPEC E14): the roster-side profile-to-bot
/// binding editor — one labelled `PROFILE:` select per fixed platform-
/// adapter key. Follows the crate's resource-plus-refresh-tick idiom with
/// its OWN locally-owned tick (mirrors `rooms_refresh_tick`'s placement
/// exactly — a change originating entirely within this component's own
/// subtree that needs to force a re-fetch without a caller round-trip).
/// Never calls the resource restart method on a `use_resource`, and mounts
/// no context provider (both crate-wide hard constraints).
#[component]
fn PlatformBindings(profiles: Vec<ProfileRow>) -> Element {
    let bindings_tick: Signal<u32> = use_signal(|| 0);
    // `Signal<Option<BTreeMap<...>>>` (not the resource's own `Option`
    // directly) so the diff effect below can tell "never fetched yet" apart
    // from "fetched, and it happens to be empty" — the latter must never be
    // misread as a mass-revert of every binding.
    let mut prior_bindings: Signal<Option<std::collections::BTreeMap<String, String>>> =
        use_signal(|| None);
    let mut saving_key: Signal<Option<String>> = use_signal(|| None);
    let mut inline_error: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut notice: Signal<Option<(String, String)>> = use_signal(|| None);

    let bindings_resource = use_resource(move || {
        let _tick = bindings_tick();
        async move { list_bot_bindings().await }
    });

    let fetched: Vec<BotBinding> = match bindings_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };
    let current_map: std::collections::BTreeMap<String, String> = fetched
        .iter()
        .map(|b| (b.bot_key.0.clone(), b.profile_name.clone()))
        .collect();

    // Diff against the previous fetch to detect an externally-caused
    // revert (UI-SPEC E14 error: a bound profile was archived elsewhere —
    // Soul page, or another browser tab — and this store entry silently
    // flipped to the default profile). This is the ONLY place that
    // transient notice can originate from for that case; a same-tab
    // set-binding success shows its own notice inline at the point of
    // change instead (see `onchange` below).
    // Phase 49.4 hotfix: this effect writes `prior_bindings` every run, so it
    // MUST NOT subscribe to `prior_bindings` — Dioxus 0.7 does not dedupe
    // identical signal writes (see the same warning in `screens/agents.rs`'s
    // UAT-3 fix), so reading it with `.read()` here made the effect re-trigger
    // itself forever, pegging the single-threaded WASM client and freezing the
    // Agents screen (BotRoster mounts this component) with no console error.
    // The crate's rule is `.read()` in render, `.peek()` in effects. We
    // subscribe to `bindings_resource` instead so a genuinely new fetch still
    // re-runs the revert diff, and `.peek()` the prior value.
    use_effect(move || {
        let current_map: std::collections::BTreeMap<String, String> = match bindings_resource() {
            Some(Ok(v)) => v
                .iter()
                .map(|b| (b.bot_key.0.clone(), b.profile_name.clone()))
                .collect(),
            _ => std::collections::BTreeMap::new(),
        };
        if let Some(old_map) = prior_bindings.peek().clone() {
            for (key, new_profile) in current_map.iter() {
                let reverted_here = old_map
                    .get(key)
                    .is_some_and(|old_profile| old_profile != new_profile)
                    && new_profile == DEFAULT_PROFILE_LABEL;
                if reverted_here {
                    let old_profile = old_map.get(key).cloned().unwrap_or_default();
                    notice.set(Some((
                        key.clone(),
                        format!("{key} reverted to default — {old_profile} was archived"),
                    )));
                }
            }
        }
        prior_bindings.set(Some(current_map));
    });

    let saving_key_val = saving_key.read().clone();
    let inline_error_val = inline_error.read().clone();
    let notice_val = notice.read().clone();

    rsx! {
        div { class: "panel", style: "margin-bottom: var(--sp-2);",
            div { class: "panel-title", "Platform bindings" }
            // Phase 49.4: the six bindings were one full-width row each, so the
            // panel ate most of the Agents viewport. Lay them out in a
            // responsive grid instead — as many columns as fit at a 260px
            // minimum, collapsing to a single column on narrow viewports.
            div {
                style: "display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:4px 20px;",
                for key in PLATFORM_KEYS {
                    {
                        let key_s = key.to_string();
                        let bound = current_map
                            .get(key)
                            .cloned()
                            .unwrap_or_else(|| DEFAULT_PROFILE_LABEL.to_string());
                        let is_saving = saving_key_val.as_deref() == Some(key);
                        let row_error = inline_error_val
                            .as_ref()
                            .filter(|(k, _)| k == key)
                            .map(|(_, m)| m.clone());
                        let row_notice = notice_val
                            .as_ref()
                            .filter(|(k, _)| k == key)
                            .map(|(_, m)| m.clone());
                        let onchange_key = key_s.clone();
                        rsx! {
                            div {
                                key: "{key}",
                                style: "display:flex;align-items:center;gap:8px;",
                                span {
                                    style: "min-width:76px;text-transform:uppercase;font-size:10px;letter-spacing:0.1em;color:var(--gray);",
                                    "{key}"
                                }
                                // The platform name + the profile value are
                                // self-describing; the separate "PROFILE:" label
                                // on every row was pure width. The select keeps
                                // its accessible name via aria-label instead.
                                select {
                                    class: "voice-settings-select",
                                    "aria-label": "Profile bound to {key}",
                                    style: "flex:1;min-width:0;max-width:180px;padding-block:2px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;",
                                    disabled: is_saving,
                                    onchange: move |evt| {
                                        let new_profile = evt.value();
                                        let key_local = onchange_key.clone();
                                        notice.set(None);
                                        inline_error.set(None);
                                        saving_key.set(Some(key_local.clone()));
                                        spawn(async move {
                                            match set_bot_binding(key_local.clone(), new_profile.clone()).await {
                                                Ok(()) => {
                                                    saving_key.set(None);
                                                    notice.set(Some((
                                                        key_local.clone(),
                                                        format!("{key_local} now uses {new_profile}"),
                                                    )));
                                                    let cur = *bindings_tick.read();
                                                    let mut tick = bindings_tick;
                                                    tick.set(cur + 1);
                                                }
                                                Err(e) => {
                                                    saving_key.set(None);
                                                    inline_error.set(Some((key_local, e.to_string())));
                                                }
                                            }
                                        });
                                    },
                                    // Phase 49.4: the default/root agent as an
                                    // explicit choice. An unbound platform
                                    // already REPORTS "default" (it is the
                                    // read-side fallback), but without this
                                    // option the value had no matching entry and
                                    // could never be chosen again once a real
                                    // profile was picked.
                                    option {
                                        key: "{DEFAULT_PROFILE_LABEL}",
                                        value: "{DEFAULT_PROFILE_LABEL}",
                                        selected: bound == DEFAULT_PROFILE_LABEL,
                                        "default (root agent)"
                                    }
                                    for p in profiles.iter() {
                                        option {
                                            key: "{p.name}",
                                            value: "{p.name}",
                                            selected: p.name == bound,
                                            "{p.name}"
                                        }
                                    }
                                }
                                if let Some(msg) = row_notice {
                                    span { style: "color:var(--teal);font-size:11px;", "{msg}" }
                                } else if let Some(msg) = row_error {
                                    span { style: "color:var(--red);font-size:11px;", "{msg}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ProfileHealth;
    use std::collections::{BTreeMap, BTreeSet};

    fn row(name: &str) -> ProfileRow {
        ProfileRow {
            name: name.to_string(),
            health: ProfileHealth::Configured,
            gaps: vec![],
            provider: None,
            model_default: None,
            key_count: 0,
        }
    }

    // -------------------------------------------------------------------
    // build_roster_entries — OF-4 badge join + unassigned-hides filter
    // -------------------------------------------------------------------

    #[test]
    fn board_assignee_is_badged_kanban_worker() {
        let profiles = vec![row("bdev01"), row("real-bot")];
        let meta_map = BTreeMap::new();
        let kanban_worker_names: BTreeSet<String> = ["bdev01".to_string()].into();
        let entries = build_roster_entries(profiles, &meta_map, &kanban_worker_names, None);
        let bdev01 = entries.iter().find(|e| e.row.name == "bdev01").unwrap();
        assert!(bdev01.is_kanban_worker);
    }

    #[test]
    fn non_assignee_profile_stays_unbadged() {
        let profiles = vec![row("real-bot")];
        let meta_map = BTreeMap::new();
        let kanban_worker_names = BTreeSet::new();
        let entries = build_roster_entries(profiles, &meta_map, &kanban_worker_names, None);
        assert!(!entries[0].is_kanban_worker);
    }

    #[test]
    fn missing_bot_meta_is_not_the_badge_signal() {
        // OF-4 directive: absence of bot-meta must NEVER badge a profile —
        // only board-assignee membership does. `buzz-agent` and hand-made
        // CLI profiles commonly have no `BotMeta` sidecar and must stay
        // unbadged unless they are also a board assignee.
        let profiles = vec![row("buzz-agent")];
        let meta_map = BTreeMap::new();
        let kanban_worker_names = BTreeSet::new();
        let entries = build_roster_entries(profiles, &meta_map, &kanban_worker_names, None);
        assert!(!entries[0].is_kanban_worker);
    }

    #[test]
    fn unassigned_placeholder_is_hidden_entirely_not_merely_badged() {
        let profiles = vec![row("unassigned"), row("real-bot")];
        let meta_map = BTreeMap::new();
        let kanban_worker_names: BTreeSet<String> = ["unassigned".to_string()].into();
        let entries = build_roster_entries(profiles, &meta_map, &kanban_worker_names, None);
        assert!(entries.iter().all(|e| e.row.name != "unassigned"));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].row.name, "real-bot");
    }

    #[test]
    fn live_bot_can_also_be_badged_kanban_worker() {
        let profiles = vec![row("dev")];
        let meta_map = BTreeMap::new();
        let kanban_worker_names: BTreeSet<String> = ["dev".to_string()].into();
        let entries =
            build_roster_entries(profiles, &meta_map, &kanban_worker_names, Some("dev"));
        assert!(entries[0].is_live);
        assert!(entries[0].is_kanban_worker);
    }
}

#[cfg(test)]
mod roster_section_tests {
    use super::*;
    use crate::protocol::{GroupRoomSummary, ProfileHealth};

    fn bot(name: &str, group: Option<&str>) -> BotRosterEntry {
        BotRosterEntry {
            row: ProfileRow {
                name: name.to_string(),
                health: ProfileHealth::Configured,
                gaps: vec![],
                provider: None,
                model_default: None,
                key_count: 0,
            },
            meta: Some(BotMeta {
                group: group.map(|g| g.to_string()),
                ..Default::default()
            }),
            is_live: false,
            is_kanban_worker: false,
        }
    }

    fn unlabeled_bot(name: &str) -> BotRosterEntry {
        BotRosterEntry {
            row: ProfileRow {
                name: name.to_string(),
                health: ProfileHealth::Configured,
                gaps: vec![],
                provider: None,
                model_default: None,
                key_count: 0,
            },
            meta: None,
            is_live: false,
            is_kanban_worker: false,
        }
    }

    fn room(id: &str, group: Option<&str>) -> GroupRoomSummary {
        GroupRoomSummary {
            id: id.to_string(),
            name: id.to_string(),
            members: vec!["a".to_string(), "b".to_string()],
            group: group.map(|g| g.to_string()),
            needs_you: false,
            preview: None,
            preview_at_ms: None,
            active_round: None,
        }
    }

    /// `<behavior>` bullet 1 / acceptance criteria: zero labeled bots
    /// returns exactly one section with `label == None`, and no section
    /// carries the Ungrouped string in that case.
    #[test]
    fn zero_labels_returns_one_flat_section() {
        let entries = vec![bot("scout", None), unlabeled_bot("recon")];
        let sections = build_roster_sections(&entries, &[]);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].label, None);
        assert!(sections
            .iter()
            .all(|s| s.label.as_deref() != Some(UNGROUPED_SECTION_LABEL)));
        assert_eq!(sections[0].entries.len(), 2);
    }

    /// `<behavior>` bullet 2: 2 labeled bots under one label plus 1
    /// unlabeled bot returns two sections, the labeled one first and
    /// `UNGROUPED` last.
    #[test]
    fn labeled_and_unlabeled_returns_labeled_first_ungrouped_last() {
        let entries = vec![
            bot("scout", Some("standup")),
            bot("recon", Some("standup")),
            unlabeled_bot("solo"),
        ];
        let sections = build_roster_sections(&entries, &[]);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].label, Some("standup".to_string()));
        assert_eq!(sections[0].entries.len(), 2);
        assert_eq!(sections[1].label, Some(UNGROUPED_SECTION_LABEL.to_string()));
        assert_eq!(sections[1].entries.len(), 1);
    }

    /// `<behavior>` bullet 3 / acceptance criteria: labels appear in
    /// FIRST-SEEN order, not alphabetical — fed deliberately out of
    /// alphabetical order.
    #[test]
    fn labels_appear_in_first_seen_order_not_alphabetical() {
        let entries = vec![bot("z-bot", Some("zephyr")), bot("a-bot", Some("alpha"))];
        let sections = build_roster_sections(&entries, &[]);
        assert_eq!(sections[0].label, Some("zephyr".to_string()));
        assert_eq!(sections[1].label, Some("alpha".to_string()));
    }

    /// `<behavior>` bullet 4 / acceptance criteria: a room whose `group`
    /// matches a bot's label lands in the SAME section as that bot — never
    /// a separate "Rooms" section (D-03).
    #[test]
    fn room_with_matching_label_joins_the_bots_section() {
        let entries = vec![bot("scout", Some("standup"))];
        let rooms = vec![room("room-1", Some("standup"))];
        let sections = build_roster_sections(&entries, &rooms);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].label, Some("standup".to_string()));
        assert_eq!(sections[0].entries.len(), 2);
        assert!(sections[0].entries.iter().any(
            |e| matches!(e, RosterSectionEntry::Room(r) if r.id == "room-1")
        ));
    }

    /// `<behavior>` bullet 5 / acceptance criteria: the section count chip
    /// equals the number of entries in that section, counting bot cards
    /// and room rows together.
    #[test]
    fn section_entry_count_includes_bots_and_rooms_together() {
        let entries = vec![bot("scout", Some("standup")), bot("recon", Some("standup"))];
        let rooms = vec![room("room-1", Some("standup")), room("room-2", None)];
        let sections = build_roster_sections(&entries, &rooms);
        let standup = sections
            .iter()
            .find(|s| s.label.as_deref() == Some("standup"))
            .unwrap();
        assert_eq!(standup.entries.len(), 3);
    }

    /// Edge case (design decision, this plan): when every entry carries a
    /// label, the trailing Ungrouped bucket is empty and is therefore
    /// omitted entirely rather than rendered as an empty section.
    #[test]
    fn all_entries_labeled_omits_empty_ungrouped_section() {
        let entries = vec![bot("scout", Some("standup"))];
        let sections = build_roster_sections(&entries, &[]);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].label, Some("standup".to_string()));
    }
}
