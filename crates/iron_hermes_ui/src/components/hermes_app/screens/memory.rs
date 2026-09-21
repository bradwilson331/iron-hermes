//! Memory screen — ported from `app.html` `<section id="screen-memory">`
//! (lines 797-882). Wired to the live `api::get_memory()` server fn
//! (Phase 26.7 Plan 02 / D-07, D-08, R-3). Made editable in Phase 50.4 Plan
//! 04 (D-01): per-row inline edit/delete plus a per-panel add form, each
//! writing through the gated `add_memory_entry`/`replace_memory_entry`/
//! `remove_memory_entry` server fns.
//!
//! Two-panel split layout:
//! - Left `.panel`: agent memory (MEMORY.md text blocks) + filter input
//! - Right column TOP `.panel`: user memory (USER.md text blocks)
//! - Right column BOTTOM `.panel`: per-store stats (Task 3)
//!
//! Filter is purely client-side (no server round-trip per UI-SPEC §"Filter
//! input") and lives inside `MemoryPanel` (agent panel only — the user
//! panel has no filter box, matching the pre-existing layout).
//! MemoryManager-disabled path (state.memory_manager == None) renders empty
//! panels without error — `get_memory()` returns `MemoryInfo::default()`.
//!
//! D-05: the screen carries a permanent, non-dismissible warning banner —
//! unlike the Providers restart banner it visually echoes, manual memory
//! edits are a standing caution, not a one-time notice.

use dioxus::prelude::*;

/// Extracts the operator-facing message from a memory write's JSON error
/// envelope (`ironhermes_core::memory_store::MemoryResult`'s `Err` shape:
/// `{"error": "...", "reason": "...", ["details": "..."]}`). Prefers
/// `details` — the injection scanner's OWN `[BLOCKED: ...]` message (D-02:
/// "surface the scanner's own message verbatim, not a paraphrase") — over
/// `reason`, which every other refusal (D-04's stale-entry message, the
/// over-limit/duplicate errors) uses instead. Falls back to the raw string
/// unmodified if it isn't the expected JSON shape (also tolerant of a
/// `ServerFnError` Display wrapper around the JSON, since the exact
/// wrapping is not part of this crate's stable contract).
fn extract_memory_error_message(raw: &str) -> String {
    let json_slice = match (raw.find('{'), raw.rfind('}')) {
        (Some(start), Some(end)) if end > start => &raw[start..=end],
        _ => raw,
    };
    match serde_json::from_str::<serde_json::Value>(json_slice) {
        Ok(serde_json::Value::Object(map)) => map
            .get("details")
            .or_else(|| map.get("reason"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| raw.to_string()),
        _ => raw.to_string(),
    }
}

/// Same thousands-separator algorithm as the store's own capacity header
/// (`ironhermes_core::memory_store`'s private `format_with_commas`,
/// `memory_store.rs:539`) — reimplemented here (client-safe, no
/// `ironhermes_core` dependency) so the number the operator reads in the
/// stats panel always matches the number the agent sees in its system
/// prompt.
fn format_with_commas(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    let len = bytes.len();
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}

#[component]
pub fn ScreenMemory(is_active: bool) -> Element {
    // Phase 50.4 Plan 04 (D-01): the memory resource must support a
    // post-mount refresh (after a successful add/edit/delete) without
    // suspending the whole screen on every write — `use_server_future`'s
    // `?` early-returns an `Element` while pending, which skips every hook
    // declared textually after it on that render and re-registers it on
    // the next: the exact positional-hook-identity crash this crate has
    // already fixed twice (agents.rs, models.rs). `refresh_tick` is read in
    // the resource closure's SYNC prefix (call syntax subscribes) so a
    // bump re-runs the fetch; `.restart()` is deliberately NOT used.
    let refresh_tick = use_signal(|| 0u32);
    let memory_resource = use_resource(move || {
        let _tick = refresh_tick();
        async move { crate::server::api::get_memory().await }
    });

    // Extract data/loading/error BEFORE rsx! — signal borrow discipline
    // per iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let memory_snapshot = memory_resource();
    let is_loading = memory_snapshot.is_none();
    let memory_info: crate::server::api::MemoryInfo = match &memory_snapshot {
        Some(Ok(v)) => v.clone(),
        _ => crate::server::api::MemoryInfo::default(),
    };
    let load_error = matches!(memory_snapshot, Some(Err(_)));

    // Split by store — each panel does its own optional client-side filter
    // (agent panel only) and idx-preserving row rendering.
    let agent_rows: Vec<crate::server::api::MemoryEntry> = memory_info
        .entries
        .iter()
        .filter(|e| e.store == "agent")
        .cloned()
        .collect();
    let user_rows: Vec<crate::server::api::MemoryEntry> = memory_info
        .entries
        .iter()
        .filter(|e| e.store == "user")
        .cloned()
        .collect();

    // Phase 50.4 Plan 04 Task 3: real per-store stats for the right-bottom
    // panel, driven by MemoryInfo.stats (empty on the memory-disabled or
    // load-error path — dashes, not an error).
    let agent_stats = memory_info.stats.iter().find(|s| s.store == "agent");
    let user_stats = memory_info.stats.iter().find(|s| s.store == "user");
    let agent_entries_display = agent_stats
        .map(|s| s.entries.to_string())
        .unwrap_or_else(|| "—".to_string());
    let agent_chars_display = agent_stats
        .map(|s| {
            format!(
                "{} / {}",
                format_with_commas(s.chars_used),
                format_with_commas(s.chars_limit)
            )
        })
        .unwrap_or_else(|| "—".to_string());
    let user_entries_display = user_stats
        .map(|s| s.entries.to_string())
        .unwrap_or_else(|| "—".to_string());
    let user_chars_display = user_stats
        .map(|s| {
            format!(
                "{} / {}",
                format_with_commas(s.chars_used),
                format_with_commas(s.chars_limit)
            )
        })
        .unwrap_or_else(|| "—".to_string());

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-memory",
            "data-screen-label": "06 Memory",

            // Phase 50.4 Plan 04 (D-01) Task 3: the two dead screen-level
            // action buttons this header used to carry — the download
            // affordance with no capability to wire to, and the add-entry
            // trigger superseded once each panel gained its own (Task 2) —
            // plus their now-empty action-button container, are gone.
            // Neither is a dead affordance left standing on a now-writable
            // screen.
            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 06" }
                    h1 { class: "screen-title", "Memory" }
                }
            }

            // D-05: permanent, non-dismissible warning — same visual family
            // as the Providers restart banner (padding/border/background
            // shell reused verbatim, UI-SPEC Spacing Scale "Named
            // exceptions"), but with no DISMISS button.
            div {
                style: "border: 1px solid rgba(210,153,34,0.45); background: rgba(210,153,34,0.06); padding: 10px 16px; border-radius: 8px; margin-bottom: 14px;",
                span {
                    style: "font-size: 11px; color: var(--amber);",
                    "Manual memory edits are not recommended."
                }
            }

            div { class: "split",

                // ── Left panel: Agent memory (MEMORY.md) ────────────────
                MemoryPanel {
                    title: "Entries".to_string(),
                    store: "agent".to_string(),
                    rows: agent_rows,
                    is_loading,
                    load_error,
                    show_filter: true,
                    refresh_tick,
                }

                div { style: "display: flex; flex-direction: column; gap: 14px;",

                    // ── Right-top panel: User memory (USER.md) ───────────
                    MemoryPanel {
                        title: "User Profile".to_string(),
                        store: "user".to_string(),
                        rows: user_rows,
                        is_loading,
                        load_error,
                        show_filter: false,
                        refresh_tick,
                    }

                    // ── Right-bottom panel: real per-store stats (Task 3) ──
                    // Replaces the fabricated vector-store panel (Qdrant/
                    // embed-model/dimensions/retention) with the flat-file
                    // store's own accounting. Empty `stats` (memory_manager
                    // == None, or a load error) renders dashes, never an
                    // error — get_memory's disabled-path contract stays
                    // intact.
                    div { class: "panel",
                        div { class: "panel-title", "Memory" }
                        dl { class: "kv",
                            dt { "Agent — entries" } dd { "{agent_entries_display}" }
                            dt { "Agent — chars" } dd { "{agent_chars_display}" }
                            dt { "User — entries" } dd { "{user_entries_display}" }
                            dt { "User — chars" } dd { "{user_chars_display}" }
                        }
                    }
                }
            }
        }
    }
}

/// One store panel (agent or user). Owns its own filter (agent only) and
/// per-row interactive edit-state signals — two independent mounts means
/// two independent edit-state signals, which is what makes editing a row
/// in one panel not lock the other (a UI-SPEC partial-state requirement a
/// single shared signal would silently violate).
#[component]
fn MemoryPanel(
    title: String,
    store: String,
    rows: Vec<crate::server::api::MemoryEntry>,
    is_loading: bool,
    load_error: bool,
    show_filter: bool,
    mut refresh_tick: Signal<u32>,
) -> Element {
    let mut filter_text = use_signal(String::new);
    // At most one row's inline form open at a time per panel — two
    // conflicting `expected_text` snapshots are never in flight (D-04).
    let editing: Signal<Option<usize>> = use_signal(|| None);
    let delete_confirm: Signal<Option<usize>> = use_signal(|| None);
    let row_busy: Signal<Option<usize>> = use_signal(|| None);
    let row_error: Signal<Option<String>> = use_signal(|| None);
    let mut add_open: Signal<bool> = use_signal(|| false);

    // Drop read borrow immediately by calling to_lowercase() (returns owned String).
    let needle = filter_text.read().to_lowercase();
    let filtered_rows: Vec<crate::server::api::MemoryEntry> = if show_filter {
        rows.iter()
            .filter(|e| needle.is_empty() || e.body.to_lowercase().contains(&needle))
            .cloned()
            .collect()
    } else {
        rows
    };
    let count_display = if is_loading {
        "—".to_string()
    } else {
        filtered_rows.len().to_string()
    };
    let add_open_val = *add_open.read();

    rsx! {
        div { class: "panel",
            div { style: "display:flex;justify-content:space-between;align-items:center;gap:10px;",
                div { class: "panel-title",
                    "{title} "
                    span { class: "count", style: "color:var(--teal)", "· {count_display}" }
                }
                div { style: "display:flex;align-items:center;gap:10px;",
                    if show_filter {
                        div { class: "search", style: "width: 220px; padding: 5px 12px;",
                            span { class: "search-glyph", "⌕" }
                            input {
                                placeholder: "Filter…",
                                value: "{filter_text}",
                                oninput: move |evt| filter_text.set(evt.value()),
                            }
                        }
                    }
                    button {
                        class: "btn btn--sm",
                        onclick: move |_| add_open.set(true),
                        "+ ENTRY"
                    }
                }
            }

            if add_open_val {
                MemoryAddForm {
                    store: store.clone(),
                    refresh_tick,
                    add_open,
                }
            }

            div { class: "row-list", style: "gap: 0;",
                if load_error {
                    div {
                        style: "color:var(--danger);font-size:var(--fs-12);",
                        "Could not load memory — check server connection."
                    }
                } else if is_loading {
                    div { style: "color:var(--gray);font-size:12px;", "Loading memory…" }
                } else if filtered_rows.is_empty() {
                    div { style: "color:var(--gray);font-size:12px;", "No entries yet." }
                } else {
                    for entry in filtered_rows.iter() {
                        MemoryEntryRow {
                            key: "{store}-{entry.idx}",
                            entry: entry.clone(),
                            store: store.clone(),
                            editing,
                            delete_confirm,
                            row_busy,
                            row_error,
                            refresh_tick,
                        }
                    }
                }
            }
        }
    }
}

/// Per-panel add form, opened by `+ ENTRY`. Closes itself (via `add_open`)
/// on a successful save or CANCEL.
#[component]
fn MemoryAddForm(
    store: String,
    mut refresh_tick: Signal<u32>,
    mut add_open: Signal<bool>,
) -> Element {
    let mut text = use_signal(String::new);
    let mut saving = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let text_val = text.read().clone();
    let saving_val = *saving.read();
    let can_save = !text_val.trim().is_empty() && !saving_val;
    let error_val = error.read().clone();

    let store_for_save = store.clone();

    rsx! {
        div { style: "display:flex;flex-direction:column;gap:12px;margin:10px 0;",
            textarea {
                class: "field-input",
                style: "font-size:12px;overflow-wrap:break-word;",
                rows: "3",
                disabled: saving_val,
                value: "{text_val}",
                oninput: move |e| {
                    error.set(None);
                    text.set(e.value());
                },
            }
            if let Some(ref msg) = error_val {
                div {
                    style: "color:var(--danger);font-size:var(--fs-12);",
                    "{extract_memory_error_message(msg)}"
                }
            }
            div { style: "display:flex;gap:10px;",
                button {
                    class: "btn btn--sm",
                    disabled: !can_save,
                    onclick: move |_| {
                        // T-50.4-18: refuse a second concurrent submit even
                        // if the `disabled` attribute hasn't re-rendered yet.
                        if *saving.read() {
                            return;
                        }
                        let store = store_for_save.clone();
                        let content = text.read().clone();
                        saving.set(true);
                        error.set(None);
                        spawn(async move {
                            match crate::server::api::add_memory_entry(store, content).await {
                                Ok(_) => {
                                    saving.set(false);
                                    text.set(String::new());
                                    add_open.set(false);
                                    let next = refresh_tick.peek().wrapping_add(1);
                                    refresh_tick.set(next);
                                }
                                Err(e) => {
                                    saving.set(false);
                                    error.set(Some(e.to_string()));
                                }
                            }
                        });
                    },
                    if saving_val { "SAVING…" } else { "SAVE" }
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    disabled: saving_val,
                    onclick: move |_| {
                        add_open.set(false);
                        text.set(String::new());
                        error.set(None);
                    },
                    "CANCEL"
                }
            }
        }
    }
}

/// One row in the memory entry list. Click-to-edit-in-place, hover-reveal
/// EDIT/DELETE, and an inline delete confirm — never a browser-native JS
/// confirm dialog.
/// `mem-ts` shows em dash (U+2014) — no per-block timestamp in underlying API (R-3).
#[component]
fn MemoryEntryRow(
    entry: crate::server::api::MemoryEntry,
    store: String,
    mut editing: Signal<Option<usize>>,
    mut delete_confirm: Signal<Option<usize>>,
    mut row_busy: Signal<Option<usize>>,
    mut row_error: Signal<Option<String>>,
    mut refresh_tick: Signal<u32>,
) -> Element {
    let idx = entry.idx;
    // Seeded once at mount; explicitly re-seeded (via `edit_text.set(...)`)
    // at the moment editing opens, so a stale mount-time value can never
    // leak into a later edit session on the same row (Dioxus keeps this
    // component instance alive across refreshes since `key` is stable).
    let mut edit_text = use_signal(|| entry.body.clone());

    let is_editing = *editing.read() == Some(idx);
    let is_confirming = *delete_confirm.read() == Some(idx);
    let is_busy = *row_busy.read() == Some(idx);
    let error_val = if is_editing || is_confirming {
        row_error.read().clone()
    } else {
        None
    };

    if is_editing {
        let edit_val = edit_text.read().clone();
        let store_for_save = store.clone();
        let expected_for_save = entry.body.clone();

        return rsx! {
            div { class: "mem-entry",
                div { class: "mem-ts", "—" }
                div { style: "display:flex;flex-direction:column;gap:12px;width:100%;",
                    textarea {
                        class: "field-input",
                        style: "font-size:12px;overflow-wrap:break-word;",
                        rows: "3",
                        disabled: is_busy,
                        value: "{edit_val}",
                        oninput: move |e| {
                            row_error.set(None);
                            edit_text.set(e.value());
                        },
                    }
                    if let Some(ref msg) = error_val {
                        div {
                            style: "color:var(--danger);font-size:var(--fs-12);",
                            "{extract_memory_error_message(msg)}"
                        }
                    }
                    div { style: "display:flex;gap:10px;",
                        button {
                            class: "btn btn--sm",
                            disabled: is_busy,
                            onclick: move |_| {
                                if row_busy.peek().is_some() {
                                    return;
                                }
                                let store = store_for_save.clone();
                                let expected = expected_for_save.clone();
                                let new_text = edit_text.read().clone();
                                row_busy.set(Some(idx));
                                row_error.set(None);
                                spawn(async move {
                                    match crate::server::api::replace_memory_entry(store, idx, expected, new_text).await {
                                        Ok(_) => {
                                            row_busy.set(None);
                                            editing.set(None);
                                            let next = refresh_tick.peek().wrapping_add(1);
                                            refresh_tick.set(next);
                                        }
                                        Err(e) => {
                                            row_busy.set(None);
                                            row_error.set(Some(e.to_string()));
                                        }
                                    }
                                });
                            },
                            "SAVE"
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            disabled: is_busy,
                            onclick: move |_| {
                                editing.set(None);
                                row_error.set(None);
                            },
                            "CANCEL"
                        }
                    }
                }
            }
        };
    }

    let open_edit_body = entry.body.clone();
    let open_edit_body_2 = entry.body.clone();
    let store_for_delete = store.clone();
    let expected_for_delete = entry.body.clone();

    rsx! {
        div { class: "mem-entry",
            onclick: move |_| {
                edit_text.set(open_edit_body.clone());
                row_error.set(None);
                delete_confirm.set(None);
                editing.set(Some(idx));
            },
            div { class: "mem-ts", "—" }
            div { class: "mem-body",
                "{entry.body} "
                span { class: "mem-tag", "{entry.store.to_uppercase()}" }
            }
            div {
                class: "mem-row-actions",
                onclick: move |e| e.stop_propagation(),
                button {
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| {
                        edit_text.set(open_edit_body_2.clone());
                        row_error.set(None);
                        delete_confirm.set(None);
                        editing.set(Some(idx));
                    },
                    "EDIT"
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| {
                        row_error.set(None);
                        editing.set(None);
                        delete_confirm.set(Some(idx));
                    },
                    "DELETE"
                }
            }
            if is_confirming {
                div {
                    style: "display:flex;flex-direction:column;gap:8px;margin-top:8px;",
                    onclick: move |e| e.stop_propagation(),
                    span { style: "font-size:12px;color:var(--text);", "Delete this entry?" }
                    if let Some(ref msg) = error_val {
                        div {
                            style: "color:var(--danger);font-size:var(--fs-12);",
                            "{extract_memory_error_message(msg)}"
                        }
                    }
                    div { style: "display:flex;gap:10px;",
                        button {
                            class: "btn btn--danger btn--sm",
                            disabled: is_busy,
                            onclick: move |_| {
                                if row_busy.peek().is_some() {
                                    return;
                                }
                                let store = store_for_delete.clone();
                                let expected = expected_for_delete.clone();
                                row_busy.set(Some(idx));
                                row_error.set(None);
                                spawn(async move {
                                    match crate::server::api::remove_memory_entry(store, idx, expected).await {
                                        Ok(_) => {
                                            row_busy.set(None);
                                            delete_confirm.set(None);
                                            let next = refresh_tick.peek().wrapping_add(1);
                                            refresh_tick.set(next);
                                        }
                                        Err(e) => {
                                            row_busy.set(None);
                                            row_error.set(Some(e.to_string()));
                                        }
                                    }
                                });
                            },
                            "DELETE"
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            disabled: is_busy,
                            onclick: move |_| delete_confirm.set(None),
                            "CANCEL"
                        }
                    }
                }
            }
        }
    }
}
