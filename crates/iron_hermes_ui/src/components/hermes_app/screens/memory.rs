//! Memory screen — ported from `app.html` `<section id="screen-memory">`
//! (lines 797-882). Wired to the live `api::get_memory()` server fn
//! (Phase 26.7 Plan 02 / D-07, D-08, R-3).
//!
//! Two-panel split layout:
//! - Left `.panel`: agent memory (MEMORY.md text blocks) + filter input
//! - Right column TOP `.panel`: user memory (USER.md text blocks)
//! - Right column BOTTOM `.panel`: Provider dl — stays static per D-07
//!
//! Filter is purely client-side (no server round-trip per UI-SPEC §"Filter input").
//! MemoryManager-disabled path (state.memory_manager == None) renders empty
//! panels without error — `get_memory()` returns `MemoryInfo::default()`.

use dioxus::prelude::*;

#[component]
pub fn ScreenMemory(is_active: bool) -> Element {
    // Fetch memory once on mount. `?` suspends until first resolution.
    let memory_resource = use_server_future(crate::server::api::get_memory)?;

    // Client-side filter signal.
    let mut filter_text = use_signal(String::new);

    // Extract data and error flag BEFORE rsx! — signal borrow discipline
    // per iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let memory_info: crate::server::api::MemoryInfo = match memory_resource() {
        Some(Ok(v)) => v,
        _ => crate::server::api::MemoryInfo::default(),
    };
    let load_error = matches!(memory_resource(), Some(Err(_)));

    // Drop read borrow immediately by calling to_lowercase() (returns owned String).
    let needle = filter_text.read().to_lowercase();

    // Partition and filter entries BEFORE rsx! (PATTERNS.md Gotchas — signal borrow safety).
    let agent_rows: Vec<crate::server::api::MemoryEntry> = memory_info
        .entries
        .iter()
        .filter(|e| e.store == "agent")
        .filter(|e| needle.is_empty() || e.body.to_lowercase().contains(&needle))
        .cloned()
        .collect();

    let user_rows: Vec<crate::server::api::MemoryEntry> = memory_info
        .entries
        .iter()
        .filter(|e| e.store == "user")
        .filter(|e| needle.is_empty() || e.body.to_lowercase().contains(&needle))
        .cloned()
        .collect();

    let agent_count = agent_rows.len();
    let user_count = user_rows.len();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-memory",
            "data-screen-label": "06 Memory",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 06" }
                    h1 { class: "screen-title", "Memory" }
                    p { class: "screen-sub",
                        "Persistent context the active agent recalls across sessions — entries, user profile, and embedding-store provider."
                    }
                }
                div { class: "screen-actions",
                    // EXPORT and + ENTRY: static affordance per D-07 / UI-SPEC §"Read-only phase"
                    button { class: "btn btn--ghost btn--sm", "EXPORT" }
                    button { class: "btn btn--sm", "+ ENTRY" }
                }
            }

            div { class: "split",

                // ── Left panel: Agent memory (MEMORY.md) ────────────────
                div { class: "panel",
                    div { style: "display:flex;justify-content:space-between;align-items:center;",
                        div { class: "panel-title",
                            "Entries "
                            span { class: "count", style: "color:var(--teal)", "· {agent_count}" }
                        }
                        div { class: "search", style: "width: 220px; padding: 5px 12px;",
                            span { class: "search-glyph", "⌕" }
                            input {
                                placeholder: "Filter…",
                                value: "{filter_text}",
                                oninput: move |evt| filter_text.set(evt.value()),
                            }
                        }
                    }
                    div { class: "row-list", style: "gap: 0;",
                        if load_error {
                            div {
                                style: "color:var(--danger);font-size:var(--fs-12);",
                                "Could not load memory — check server connection."
                            }
                        } else {
                            for entry in agent_rows.iter() {
                                MemoryEntryRow {
                                    key: "agent-{entry.body.len()}-{entry.body.chars().take(16).collect::<String>()}",
                                    entry: entry.clone(),
                                }
                            }
                        }
                    }
                }

                div { style: "display: flex; flex-direction: column; gap: 14px;",

                    // ── Right-top panel: User memory (USER.md) ───────────
                    div { class: "panel",
                        div { class: "panel-title",
                            "User Profile "
                            span { class: "count", style: "color:var(--teal)", "· {user_count}" }
                        }
                        div { class: "row-list", style: "gap: 0;",
                            if load_error {
                                div {
                                    style: "color:var(--danger);font-size:var(--fs-12);",
                                    "Could not load memory — check server connection."
                                }
                            } else {
                                for entry in user_rows.iter() {
                                    MemoryEntryRow {
                                        key: "user-{entry.body.len()}-{entry.body.chars().take(16).collect::<String>()}",
                                        entry: entry.clone(),
                                    }
                                }
                            }
                        }
                    }

                    // ── Right-bottom panel: Provider dl — STATIC per D-07 ──
                    div { class: "panel",
                        div { class: "panel-title", "Provider" }
                        dl { class: "kv",
                            dt { "Store" } dd { "Qdrant (local)" }
                            dt { "Embed Model" } dd { "all-mpnet-base-v2" }
                            dt { "Dimensions" } dd { "768" }
                            dt { "Entries" } dd { "412 · 14.2 MB" }
                            dt { "Retention" } dd { "infinite" }
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            style: "width:100%;justify-content:center;",
                            "RECONFIGURE"
                        }
                    }
                }
            }
        }
    }
}

/// One row in the memory entry list.
/// `mem-ts` shows em dash (U+2014) — no per-block timestamp in underlying API (R-3).
#[component]
fn MemoryEntryRow(entry: crate::server::api::MemoryEntry) -> Element {
    rsx! {
        div { class: "mem-entry",
            div { class: "mem-ts", "—" }
            div { class: "mem-body",
                "{entry.body} "
                span { class: "mem-tag", "{entry.store.to_uppercase()}" }
            }
        }
    }
}
