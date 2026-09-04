//! Skills screen — wired to the live `api::list_skills()` server fn
//! (Phase 26.7 Plan 03 / D-09, R-1, R-4).
//!
//! Renders the full SkillRegistry catalog with per-skill enabled state
//! driven by optimistic toggle_states HashMap signal (Phase 26.7.3 Plan 03).
//! Toggle persists via toggle_skill #[server] fn; on Err the flip reverts
//! and an inline error message appears inside the card.
//!
//! ── ENABLED tab reactivity (Phase 41.1 Plan 09 / G-41.1-4 fix) ──────────
//! The ENABLED tab count/filter deliberately reads server-CONFIRMED state,
//! not the optimistic `toggle_states` flip (26.7.3-RESEARCH Pitfall 6) —
//! `confirmed_states` (same HashMap<name, enabled> shape as toggle_states)
//! is seeded once at mount alongside it and advanced only in `on_toggle`'s
//! `Ok(_)` branch. This does NOT call `skills_resource.restart()` — that is
//! a documented hook-order-crash trap for this screen (see providers.rs
//! header comment + repo MEMORY feedback_dioxus_use_server_future_restart_trap).

use dioxus::prelude::*;
use std::collections::HashMap;

use crate::components::hermes_app::screens::skills_import::{
    EditorTarget, NewSkillWizard, SkillImportWizard, SkillMdEditor,
};

#[allow(dead_code)] // called from use_memo closure in ScreenSkills; dead_code fires on lib target
fn tab_predicate(category: &str, enabled: bool, tab: &str) -> bool {
    match tab {
        "bundled" => category == "bundled",
        "installed" => category != "bundled",
        "enabled" => enabled,
        _ => true,
    }
}

#[allow(dead_code)] // called from use_memo closure in ScreenSkills; dead_code fires on test target
fn search_matches(name: &str, description: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    name.to_lowercase().contains(&q) || description.to_lowercase().contains(&q)
}

#[component]
pub fn ScreenSkills(is_active: bool) -> Element {
    // Phase 49.4 Plan 07 (D-05..D-09): bumped after a successful import,
    // create, or fork so the list re-fetches — read in the SYNC prefix of
    // this resource (the resource + refresh-tick idiom; the hook-order-crash
    // trap this screen documents above is never invoked by this plan).
    let mut refresh_tick: Signal<u32> = use_signal(|| 0);
    let skills_resource = use_server_future(move || {
        let _tick = refresh_tick();
        async move { crate::server::api::list_skills().await }
    })?;

    // Extract data and error flag BEFORE rsx! — signal borrow discipline
    // per iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let skills_list: Vec<crate::server::api::SkillInfo> = match skills_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };
    let load_error = matches!(skills_resource(), Some(Err(_)));

    // Tab and search signals — let mut so event handlers can .set()
    let mut active_tab = use_signal(|| "all");
    let mut search_query = use_signal(String::new);

    // Phase 49.4 Plan 07 (D-05/D-06): IMPORT wizard open/closed, owned here
    // (mirrors kanban/drawer.rs's ReadSignal-prop / EventHandler-callback
    // ownership split — the wizard reads `open`, this screen owns the set).
    let mut import_open: Signal<bool> = use_signal(|| false);
    let import_open_ro: ReadSignal<bool> = import_open.into();
    // Phase 49.4 Plan 07 (D-08): NEW SKILL wizard open/closed — same
    // ownership split as import_open above.
    let mut new_skill_open: Signal<bool> = use_signal(|| false);
    let new_skill_open_ro: ReadSignal<bool> = new_skill_open.into();
    // Phase 49.4 Plan 07 (D-09): the SKILL.md editor's target — None when
    // closed, Some(name, is_bundled) when opened from a row action.
    let mut editing_skill: Signal<Option<EditorTarget>> = use_signal(|| None);
    let editing_skill_ro: ReadSignal<Option<EditorTarget>> = editing_skill.into();

    // Optimistic toggle state — HashMap<name, enabled> owned by this screen
    let mut toggle_states: Signal<HashMap<String, bool>> = use_signal(HashMap::new);
    // Server-confirmed toggle state — same HashMap<name, enabled> shape as
    // toggle_states, but written ONLY in on_toggle's Ok(_) branch (G-41.1-4
    // fix). The ENABLED tab count/filter read this, not toggle_states, per
    // Pitfall 6 — see module header and
    // .planning/debug/41.1-skills-enabled-tab-reactivity.md.
    let mut confirmed_states: Signal<HashMap<String, bool>> = use_signal(HashMap::new);
    // Per-skill error messages — populated on server Err, cleared on next click
    let mut toggle_errors: Signal<HashMap<String, String>> = use_signal(HashMap::new);

    // Seed toggle_states and confirmed_states from skills_list on first
    // non-empty load (Pitfall 3). use_effect re-runs each render; guard
    // ensures we only seed once and do not overwrite optimistic flips (or
    // already-confirmed toggles) after the initial seed.
    {
        let sl = skills_list.clone();
        use_effect(move || {
            if !sl.is_empty() && toggle_states.read().is_empty() {
                let mut map = toggle_states.write();
                let mut confirmed = confirmed_states.write();
                for s in &sl {
                    map.insert(s.name.clone(), s.enabled);
                    confirmed.insert(s.name.clone(), s.enabled);
                }
            }
        });
    }

    // Live tab counts — ALL/BUNDLED/INSTALLED read skills_list directly
    // (category is static per load). Computed from skills_list.
    let count_all = skills_list.len();
    let count_bundled = skills_list
        .iter()
        .filter(|s| s.category == "bundled")
        .count();
    let count_installed = skills_list
        .iter()
        .filter(|s| s.category != "bundled")
        .count();
    // ENABLED count reads confirmed_states (server-confirmed per Pitfall 6),
    // not the frozen skills_list snapshot — G-41.1-4 fix. Falls back to the
    // skill's own server-sourced `enabled` for any name not yet seeded.
    // Extracted to a plain usize before rsx! per signal-borrow discipline.
    let count_enabled = {
        let confirmed = confirmed_states.read();
        skills_list
            .iter()
            .filter(|s| *confirmed.get(&s.name).unwrap_or(&s.enabled))
            .count()
    };

    // Header sub-copy uses optimistic count (tracks live flips);
    // the ENABLED tab label uses confirmed-state count_enabled (Pitfall 6).
    let enabled_count_live = toggle_states.read().values().filter(|&&v| v).count();
    // borrow ends at ; — safe before rsx!

    // Extract owned values BEFORE rsx! — no GenerationalRef crossing the macro boundary
    let tab_val = active_tab(); // &'static str — Copy

    // Clone skills_list before use_memo — closure must own its data ('static capture, Pitfall 5)
    let skills_for_memo = skills_list.clone();
    let filtered_skills = use_memo(move || {
        let tab = active_tab();
        let query = search_query();
        // Read confirmed_states inside the memo so it subscribes — recomputes
        // whenever a toggle is server-confirmed (G-41.1-4 fix). Deliberately
        // NOT toggle_states (optimistic) — ENABLED stays scoped to
        // confirmed state per Pitfall 6.
        let confirmed = confirmed_states.read();
        skills_for_memo
            .iter()
            .filter(|s| {
                let is_enabled = *confirmed.get(&s.name).unwrap_or(&s.enabled);
                tab_predicate(&s.category, is_enabled, tab)
                    && search_matches(&s.name, &s.description, &query)
            })
            .cloned()
            .collect::<Vec<crate::server::api::SkillInfo>>()
    });

    // Pre-compute per-card data BEFORE rsx! — rsx! for loops cannot contain let bindings.
    // Borrows from toggle_states and toggle_errors end at ; (each read() call).
    let card_data: Vec<(crate::server::api::SkillInfo, bool, Option<String>)> = {
        let states = toggle_states.read();
        let errors = toggle_errors.read();
        filtered_skills
            .read()
            .iter()
            .map(|skill| {
                let is_enabled = *states.get(&skill.name).unwrap_or(&skill.enabled);
                let err_msg = errors.get(&skill.name).cloned();
                (skill.clone(), is_enabled, err_msg)
            })
            .collect()
    };
    // All borrows (states, errors, filtered_skills read guards) dropped here.

    // Optimistic toggle closure — captures toggle_states, confirmed_states,
    // and toggle_errors by move. Called from SkillCard's on_toggle
    // EventHandler with the skill name.
    let mut on_toggle = move |name: String| {
        // Capture current state (Copy bool — borrow ends at ;)
        let current = *toggle_states.read().get(&name).unwrap_or(&false);
        // Optimistic flip
        toggle_states.write().insert(name.clone(), !current);
        // Clear prior error for this skill
        toggle_errors.write().remove(&name);
        // Spawn async server call — onclick cannot be async in Dioxus 0.7
        spawn(async move {
            match crate::server::api::toggle_skill(name.clone()).await {
                Ok(_) => {
                    // Server confirmed — advance confirmed_states so the
                    // ENABLED tab count/filter update live (G-41.1-4 fix).
                    // Do NOT call skills_resource.restart() — hook-order-
                    // crash trap (see module header / providers.rs).
                    confirmed_states.write().insert(name.clone(), !current);
                }
                Err(_) => {
                    // Revert optimistic flip and surface inline error
                    toggle_states.write().insert(name.clone(), current);
                    toggle_errors
                        .write()
                        .insert(name.clone(), "Toggle failed — try again.".to_string());
                }
            }
        });
    };

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-skills",
            "data-screen-label": "04 Skills",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 04" }
                    h1 { class: "screen-title", "Skills" }
                    span { class: "screen-status",
                        "· {count_all} loaded · {enabled_count_live} enabled for "
                        code { style: "color:var(--teal)", "default" }
                    }
                }
                div { class: "screen-actions",
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| import_open.set(true),
                        "⇣ IMPORT"
                    }
                    button {
                        class: "btn btn--sm",
                        onclick: move |_| new_skill_open.set(true),
                        "+ NEW SKILL"
                    }
                }
            }

            div { class: "search",
                span { class: "search-glyph", "⌕" }
                input {
                    placeholder: "Search skills, tags, capabilities…",
                    oninput: move |e| search_query.set(e.value()),
                }
            }

            div { class: "tabs",
                button {
                    class: if tab_val == "all" { "tab is-active" } else { "tab" },
                    onclick: move |_| active_tab.set("all"),
                    "ALL · {count_all}"
                }
                button {
                    class: if tab_val == "bundled" { "tab is-active" } else { "tab" },
                    onclick: move |_| active_tab.set("bundled"),
                    "BUNDLED · {count_bundled}"
                }
                button {
                    class: if tab_val == "installed" { "tab is-active" } else { "tab" },
                    onclick: move |_| active_tab.set("installed"),
                    "INSTALLED · {count_installed}"
                }
                button {
                    class: if tab_val == "enabled" { "tab is-active" } else { "tab" },
                    onclick: move |_| active_tab.set("enabled"),
                    "ENABLED · {count_enabled}"
                }
                button {
                    class: "tab",
                    style: "opacity:0.5; pointer-events:none;",
                    disabled: true,
                    "UPDATES · 0"
                }
            }

            div { class: "grid",
                if load_error {
                    div {
                        style: "color:var(--danger);font-size:var(--fs-12);",
                        "Could not load skills — check server connection."
                    }
                } else if card_data.is_empty() && !skills_list.is_empty() {
                    div {
                        style: "color:var(--gray);font-size:var(--fs-12);",
                        "No skills match."
                    }
                } else {
                    for (skill, is_enabled, err_msg) in card_data.iter().cloned() {
                        {
                            // Separate owned locals per closure — two `move`
                            // closures cannot both move-capture the same
                            // `skill.name` field out of one shared `skill`.
                            let toggle_name = skill.name.clone();
                            let edit_target = EditorTarget {
                                name: skill.name.clone(),
                                is_bundled: skill.category == "bundled",
                            };
                            rsx! {
                                SkillCard {
                                    key: "{skill.name}",
                                    skill: skill.clone(),
                                    enabled: is_enabled,
                                    error_msg: err_msg,
                                    on_toggle: move |_| on_toggle(toggle_name.clone()),
                                    on_edit: move |_| editing_skill.set(Some(edit_target.clone())),
                                }
                            }
                        }
                    }
                }
            }

            SkillImportWizard {
                open: import_open_ro,
                on_close: move |_| import_open.set(false),
                on_installed: move |_| refresh_tick.set(refresh_tick() + 1),
            }

            NewSkillWizard {
                open: new_skill_open_ro,
                on_close: move |_| new_skill_open.set(false),
                on_created: move |_| refresh_tick.set(refresh_tick() + 1),
            }

            SkillMdEditor {
                target: editing_skill_ro,
                on_close: move |_| editing_skill.set(None),
                on_saved: move |_| refresh_tick.set(refresh_tick() + 1),
            }
        }
    }
}

#[component]
fn SkillCard(
    skill: crate::server::api::SkillInfo,
    enabled: bool, // plain bool — NOT Signal<bool>; parent owns toggle_states
    error_msg: Option<String>, // None = no error; Some = revert error text
    on_toggle: EventHandler<()>, // fires on .tgl click; parent owns the spawn
    // Phase 49.4 Plan 07 (D-09): opens the SKILL.md editor for this skill;
    // parent owns the editing_skill signal (same ownership split as on_toggle).
    on_edit: EventHandler<()>,
) -> Element {
    // Phase 41.1 Plan 06 (D-07): the Run affordance consumes the SAME chat send
    // handler + active-screen signal provided at the HermesApp root, so one
    // click navigates to chat and submits /<skill> through the EXISTING WS
    // chat-input path (no new #[server] execution fn — RESEARCH Pitfall 6).
    let mut active_screen = use_context::<Signal<crate::state::Screen>>();
    let send = use_context::<crate::components::hermes_app::screens::chat::ChatSendHandler>();
    // Inline start-failure flag (mirrors the toggle_errors convention). A
    // client-side navigate+submit cannot report an async failure, so this is
    // raised only on the one detectable precondition — an empty skill name.
    let mut run_error = use_signal(|| false);
    // Cloned for the onclick closure (moved in); `skill` stays available for
    // the render tree below.
    let run_name = skill.name.clone();

    rsx! {
        div {
            class: "card",
            class: if enabled { "is-active" },
            div { class: "card-head",
                div {
                    class: if enabled { "card-icon" } else { "card-icon gray" },
                    "⊕"
                }
                div { style: "flex:1",
                    div { class: "card-title", "{skill.name}" }
                    div { class: "card-meta", "{skill.category}" }
                }
                // Phase 41.1 Plan 06 (D-07): one-click Run — LEFT of the toggle.
                // Reuses the existing .btn--ghost.btn--sm class (no new button
                // style). A disabled skill renders the button at 50% opacity +
                // pointer-events:none (the exact "UPDATES · 0" tab pattern) —
                // running a disabled skill is unsupported this phase.
                button {
                    class: "btn btn--ghost btn--sm",
                    r#type: "button",
                    "aria-label": "Run skill {skill.name}",
                    style: if !enabled { "opacity:0.5; pointer-events:none;" },
                    disabled: !enabled,
                    onclick: move |_| {
                        if run_name.is_empty() {
                            run_error.set(true);
                        } else {
                            active_screen.set(crate::state::Screen::Chat);
                            send.0.call((format!("/{run_name}"), Vec::new()));
                        }
                    },
                    "▶ Run"
                }
                // Phase 49.4 Plan 07 (D-09): opens the SKILL.md editor.
                button {
                    class: "btn btn--ghost btn--sm",
                    r#type: "button",
                    "aria-label": "Edit skill {skill.name}",
                    onclick: move |_| on_edit.call(()),
                    "✎ Edit"
                }
                div {
                    class: if enabled { "tgl on" } else { "tgl" },
                    role: "switch",
                    aria_checked: "{enabled}",
                    onclick: move |_| on_toggle.call(()),
                }
            }
            div { class: "card-body", "{skill.description}" }
            if let Some(ref err) = error_msg {
                div {
                    style: "color:var(--danger);font-size:var(--fs-12);",
                    "{err}"
                }
            }
            // Phase 41.1 Plan 06 (D-07): inline start-failure copy (UI-SPEC §B /
            // Copywriting Contract), matching the toggle_errors --danger convention.
            if *run_error.read() {
                div {
                    style: "color:var(--danger);font-size:var(--fs-12);",
                    "Could not start skill — try typing /{skill.name} in chat."
                }
            }
        }
    }
}
