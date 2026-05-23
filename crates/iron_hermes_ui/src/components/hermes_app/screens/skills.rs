//! Skills screen — wired to the live `api::list_skills()` server fn
//! (Phase 26.7 Plan 03 / D-09, R-1, R-4).
//!
//! Renders the full SkillRegistry catalog with per-skill enabled state
//! driven by optimistic toggle_states HashMap signal (Phase 26.7.3 Plan 03).
//! Toggle persists via toggle_skill #[server] fn; on Err the flip reverts
//! and an inline error message appears inside the card.

use dioxus::prelude::*;
use std::collections::HashMap;

fn tab_predicate(category: &str, enabled: bool, tab: &str) -> bool {
    match tab {
        "bundled" => category == "bundled",
        "installed" => category != "bundled",
        "enabled" => enabled,
        _ => true,
    }
}

fn search_matches(name: &str, description: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    name.to_lowercase().contains(&q) || description.to_lowercase().contains(&q)
}

#[component]
pub fn ScreenSkills(is_active: bool) -> Element {
    let skills_resource = use_server_future(crate::server::api::list_skills)?;

    // Extract data and error flag BEFORE rsx! — signal borrow discipline
    // per iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let skills_list: Vec<crate::server::api::SkillInfo> = match skills_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };
    let load_error = matches!(skills_resource(), Some(Err(_)));

    // Tab and search signals — let mut so event handlers can .set()
    let mut active_tab = use_signal(|| "all");
    let mut search_query = use_signal(|| String::new());

    // Optimistic toggle state — HashMap<name, enabled> owned by this screen
    let mut toggle_states: Signal<HashMap<String, bool>> = use_signal(|| HashMap::new());
    // Per-skill error messages — populated on server Err, cleared on next click
    let mut toggle_errors: Signal<HashMap<String, String>> = use_signal(|| HashMap::new());

    // Seed toggle_states from skills_list on first non-empty load (Pitfall 3).
    // use_effect re-runs each render; guard ensures we only seed once and
    // do not overwrite optimistic flips after the initial seed.
    {
        let sl = skills_list.clone();
        use_effect(move || {
            if !sl.is_empty() && toggle_states.read().is_empty() {
                let mut map = toggle_states.write();
                for s in &sl {
                    map.insert(s.name.clone(), s.enabled);
                }
            }
        });
    }

    // Live tab counts — computed from skills_list (server-sourced per Pitfall 6)
    let count_all = skills_list.len();
    let count_bundled = skills_list.iter().filter(|s| s.category == "bundled").count();
    let count_installed = skills_list.iter().filter(|s| s.category != "bundled").count();
    let count_enabled = skills_list.iter().filter(|s| s.enabled).count();

    // Header sub-copy uses optimistic count (tracks live flips);
    // the ENABLED tab label uses server-sourced count_enabled (Pitfall 6).
    let enabled_count_live = toggle_states.read().values().filter(|&&v| v).count();
    // borrow ends at ; — safe before rsx!

    // Extract owned values BEFORE rsx! — no GenerationalRef crossing the macro boundary
    let tab_val = active_tab(); // &'static str — Copy

    // Clone skills_list before use_memo — closure must own its data ('static capture, Pitfall 5)
    let skills_for_memo = skills_list.clone();
    let filtered_skills = use_memo(move || {
        let tab = active_tab();
        let query = search_query();
        skills_for_memo
            .iter()
            .filter(|s| tab_predicate(&s.category, s.enabled, tab) && search_matches(&s.name, &s.description, &query))
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

    // Optimistic toggle closure — captures toggle_states and toggle_errors by move.
    // Called from SkillCard's on_toggle EventHandler with the skill name.
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
                Ok(_) => {} // optimistic state is already correct
                Err(_) => {
                    // Revert optimistic flip and surface inline error
                    toggle_states.write().insert(name.clone(), current);
                    toggle_errors.write().insert(
                        name.clone(),
                        "Toggle failed — try again.".to_string(),
                    );
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
                    p { class: "screen-sub",
                        "{count_all} loaded · {enabled_count_live} enabled for "
                        code { style: "color:var(--teal)", "default" }
                        "."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "⇣ IMPORT" }
                    button { class: "btn btn--sm", "+ NEW SKILL" }
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
                        SkillCard {
                            key: "{skill.name}",
                            skill: skill.clone(),
                            enabled: is_enabled,
                            error_msg: err_msg,
                            on_toggle: move |_| on_toggle(skill.name.clone()),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SkillCard(
    skill: crate::server::api::SkillInfo,
    enabled: bool,               // plain bool — NOT Signal<bool>; parent owns toggle_states
    error_msg: Option<String>,   // None = no error; Some = revert error text
    on_toggle: EventHandler<()>, // fires on .tgl click; parent owns the spawn
) -> Element {
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
        }
    }
}
