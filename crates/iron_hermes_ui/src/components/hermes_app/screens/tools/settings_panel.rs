//! Phase 48.2 Plan 03 (D-05/D-11/D-13/D-21): staged settings panel for the
//! rest of `tools:` — the global timeout + per-tool timeout overrides
//! (Task 2) and the three reorderable provider chains (Task 3). Mounted as
//! a collapsible inline section of the Tools screen, below the toolset
//! groups (an inline panel, not a slide-in drawer — the page already
//! scrolls and a drawer would hide the chain lists behind an overlay while
//! the operator compares them against the tool cards).
//!
//! # Staged-write discipline (D-11)
//!
//! Every field here edits a LOCAL signal only. Nothing reaches
//! `config.yaml` until SAVE calls
//! [`crate::server::tools_config_api::update_tools_settings`] —
//! reorder/add/remove/edit all mutate the staged copy in place. A SAVE
//! failure keeps the staged values exactly as the operator left them and
//! shows the returned message(s) inline; the panel is never silently
//! reset (`staged-settings-panel/error`).
//!
//! # Seed / re-seed discipline
//!
//! The panel's own `use_resource` reads `scope` and `refresh_tick` in its
//! sync prefix — the crate's proven refresh_tick idiom (`tools.rs`,
//! `kanban/drawer.rs`) — and re-fetches on either changing. A `use_effect`
//! seeds the staged signals from a fresh successful load ONLY when the
//! panel is not `dirty`: an unrelated `refresh_tick` bump elsewhere on the
//! page (a catalog toggle shares the same signal) must never discard an
//! in-progress edit here. `dirty` is read via `.peek()` inside that effect
//! — never `.read()` — so toggling `dirty` does not itself retrigger the
//! effect; only a genuine resource change does. The SAVE handler sets the
//! staged/last-saved signals directly from the just-written payload
//! instead of waiting on a refetch, sidestepping the race a `.read()`
//! dependency on `dirty` would otherwise create between "SAVE cleared
//! dirty" and "the refetch actually resolved."
//!
//! # Last-saved chain order (Task 3, D-21)
//!
//! `last_saved_web_*` tracks the chain order as of the last successful
//! load/save, separate from the staged (possibly-reordered, unsaved)
//! copy. `ProviderChainList` reverts the VISUAL order to this snapshot on
//! a failed SAVE, without discarding the rest of the staged edit.

use super::credential_form::ToolCredentialForm;
use crate::server::tools_config_api::{
    ChainView, ConfigScope, TimeoutOverrideRow, ToolsSettingsPayload, ToolsSettingsView,
};
use crate::server::tools_credentials_api::CANONICAL_TOOL_CREDENTIAL_KEYS;
use dioxus::prelude::*;

/// UI-SPEC Copywriting Contract — write/save failure prefix (D-11),
/// verbatim, never paraphrased.
#[allow(dead_code)] // used inside ToolsSettingsPanel's rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
const SAVE_FAILED_PREFIX: &str = "SAVE FAILED — ";

/// Pull one named chain's provider list out of a loaded view.
#[allow(dead_code)] // used inside ToolsSettingsPanel's use_effect; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn chain_names(view: &ToolsSettingsView, tool: &str) -> Vec<String> {
    view.chains
        .iter()
        .find(|c| c.tool == tool)
        .map(|c| c.entries.iter().map(|e| e.provider.clone()).collect())
        .unwrap_or_default()
}

/// The staged settings panel — timeout editor (Task 2) and provider chain
/// lists (Task 3), sharing one staged working copy and one SAVE handler.
#[component]
pub fn ToolsSettingsPanel(
    scope: ReadSignal<ConfigScope>,
    writable: bool,
    refresh_tick: ReadSignal<u32>,
    known_tool_names: Vec<String>,
    // Phase 48.2 Plan 07 (D-05/D-16): every env_var-shaped credential key
    // currently named by an unmet prerequisite anywhere in the catalog —
    // unioned with `CANONICAL_TOOL_CREDENTIAL_KEYS` below so this section
    // lists "every canonical tool credential key plus every key named by a
    // currently-unmet prerequisite, de-duplicated."
    missing_credential_keys: Vec<String>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).
    let mut expanded: Signal<bool> = use_signal(|| false);

    let settings_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move { crate::server::tools_config_api::get_tools_settings(scope_value).await }
    });

    let is_loading = settings_resource().is_none();
    let load_error: Option<String> = match settings_resource() {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };

    // Staged working copy (D-11) — every field here is local until SAVE.
    let mut staged_timeout_secs: Signal<u64> = use_signal(|| 60);
    let mut staged_overrides: Signal<Vec<TimeoutOverrideRow>> = use_signal(Vec::new);
    let mut staged_web_search: Signal<Vec<String>> = use_signal(Vec::new);
    let mut staged_web_answer: Signal<Vec<String>> = use_signal(Vec::new);
    let mut staged_web_extract: Signal<Vec<String>> = use_signal(Vec::new);
    // Last-saved order (Task 3, D-21) — see module doc.
    let mut last_saved_web_search: Signal<Vec<String>> = use_signal(Vec::new);
    let mut last_saved_web_answer: Signal<Vec<String>> = use_signal(Vec::new);
    let mut last_saved_web_extract: Signal<Vec<String>> = use_signal(Vec::new);
    // legal_providers per chain, for Task 3's add-provider picker.
    let mut chain_legal: Signal<Vec<ChainView>> = use_signal(Vec::new);
    // Credential-presence answers, keyed by provider NAME (not position) —
    // skipped-ness does not depend on chain order, so a reorder never
    // requires re-deriving these (Task 3, D-21).
    let mut skipped_by_provider: Signal<std::collections::HashMap<String, bool>> =
        use_signal(std::collections::HashMap::new);
    let mut credential_by_provider: Signal<std::collections::HashMap<String, String>> =
        use_signal(std::collections::HashMap::new);
    let mut dirty: Signal<bool> = use_signal(|| false);
    let mut saving: Signal<bool> = use_signal(|| false);
    let mut save_errors: Signal<Vec<String>> = use_signal(Vec::new);

    // Seed/re-seed from a fresh successful load — never while the operator
    // has an in-progress edit (module doc: the `.peek()` race note).
    use_effect(move || {
        if let Some(Ok(view)) = settings_resource() {
            if !*dirty.peek() {
                staged_timeout_secs.set(view.timeout_secs);
                staged_overrides.set(view.timeout_overrides.clone());
                let ws = chain_names(&view, "web_search");
                let wa = chain_names(&view, "web_answer");
                let we = chain_names(&view, "web_extract");
                staged_web_search.set(ws.clone());
                staged_web_answer.set(wa.clone());
                staged_web_extract.set(we.clone());
                last_saved_web_search.set(ws);
                last_saved_web_answer.set(wa);
                last_saved_web_extract.set(we);
                chain_legal.set(view.chains.clone());

                let mut skipped_map = std::collections::HashMap::new();
                let mut credential_map = std::collections::HashMap::new();
                for chain_view in &view.chains {
                    for entry in &chain_view.entries {
                        skipped_map.insert(entry.provider.clone(), entry.skipped);
                        if let Some(cred) = &entry.credential {
                            credential_map.insert(entry.provider.clone(), cred.clone());
                        }
                    }
                }
                skipped_by_provider.set(skipped_map);
                credential_by_provider.set(credential_map);

                save_errors.set(Vec::new());
            }
        }
    });

    // Extract every value out of signals BEFORE the rsx! block — no
    // GenerationalRef borrow held across the macro (iron_hermes_ui/clippy.toml).
    let expanded_val = *expanded.read();
    let timeout_secs_val = *staged_timeout_secs.read();
    let overrides_val = staged_overrides.read().clone();
    let save_errors_val = save_errors.read().clone();
    let saving_val = *saving.read();
    let dirty_val = *dirty.read();
    let staged_web_search_val = staged_web_search.read().clone();
    let staged_web_answer_val = staged_web_answer.read().clone();
    let staged_web_extract_val = staged_web_extract.read().clone();
    let skipped_by_provider_val = skipped_by_provider.read().clone();
    let credential_by_provider_val = credential_by_provider.read().clone();
    let legal_for = |tool: &str| -> Vec<String> {
        chain_legal
            .read()
            .iter()
            .find(|c| c.tool == tool)
            .map(|c| c.legal_providers.clone())
            .unwrap_or_default()
    };
    let legal_web_search = legal_for("web_search");
    let legal_web_answer = legal_for("web_answer");
    let legal_web_extract = legal_for("web_extract");

    // Phase 48.2 Plan 07 (D-05/D-16): the credentials section's key list —
    // every canonical tool credential key plus every key named by a
    // currently-unmet prerequisite, de-duplicated. One row each, so an
    // operator can manage keys away from a specific card.
    let credential_keys: Vec<String> = {
        let mut keys: Vec<String> = CANONICAL_TOOL_CREDENTIAL_KEYS
            .iter()
            .map(|k| k.to_string())
            .chain(missing_credential_keys.iter().cloned())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    };
    let scope_for_credentials = scope();

    rsx! {
        section { class: "tools-settings-panel",
            button {
                class: "tools-settings-header",
                "aria-expanded": if expanded_val { "true" } else { "false" },
                "aria-label": "Toggle tools settings panel",
                onclick: move |_| {
                    let next = !*expanded.read();
                    expanded.set(next);
                },
                span { class: "tools-settings-title", "TOOLS SETTINGS" }
                span { class: "tools-settings-caret", "aria-hidden": "true",
                    if expanded_val { "▾" } else { "▸" }
                }
            }
            if expanded_val {
                div { class: "tools-settings-body",
                    if is_loading {
                        ToolsSettingsSkeleton {}
                    } else if let Some(reason) = load_error.clone() {
                        div { class: "tools-error-banner",
                            span { "COULD NOT LOAD TOOLS SETTINGS — {reason}" }
                        }
                    } else {
                        TimeoutOverrideEditor {
                            timeout_secs: timeout_secs_val,
                            overrides: overrides_val,
                            known_tool_names: known_tool_names.clone(),
                            writable,
                            on_timeout_secs_change: move |v: u64| {
                                staged_timeout_secs.set(v);
                                dirty.set(true);
                            },
                            on_overrides_change: move |rows: Vec<TimeoutOverrideRow>| {
                                staged_overrides.set(rows);
                                dirty.set(true);
                            },
                        }

                        ProviderChainList {
                            heading: "WEB SEARCH CHAIN".to_string(),
                            tool: "web_search".to_string(),
                            chain: staged_web_search_val.clone(),
                            legal_providers: legal_web_search.clone(),
                            skipped_by_provider: skipped_by_provider_val.clone(),
                            credential_by_provider: credential_by_provider_val.clone(),
                            writable,
                            on_change: move |next: Vec<String>| {
                                staged_web_search.set(next);
                                dirty.set(true);
                            },
                        }
                        ProviderChainList {
                            heading: "WEB ANSWER CHAIN".to_string(),
                            tool: "web_answer".to_string(),
                            chain: staged_web_answer_val.clone(),
                            legal_providers: legal_web_answer.clone(),
                            skipped_by_provider: skipped_by_provider_val.clone(),
                            credential_by_provider: credential_by_provider_val.clone(),
                            writable,
                            on_change: move |next: Vec<String>| {
                                staged_web_answer.set(next);
                                dirty.set(true);
                            },
                        }
                        ProviderChainList {
                            heading: "WEB EXTRACT CHAIN".to_string(),
                            tool: "web_extract".to_string(),
                            chain: staged_web_extract_val.clone(),
                            legal_providers: legal_web_extract.clone(),
                            skipped_by_provider: skipped_by_provider_val.clone(),
                            credential_by_provider: credential_by_provider_val.clone(),
                            writable,
                            on_change: move |next: Vec<String>| {
                                staged_web_extract.set(next);
                                dirty.set(true);
                            },
                        }

                        // Phase 48.2 Plan 07 (D-05/D-16): the credentials
                        // section — the SAME ToolCredentialForm mounted on
                        // the amber card, so there is one credential form on
                        // this page, not two.
                        div { class: "tools-settings-credentials",
                            div { class: "section-label", "CREDENTIALS" }
                            for key in credential_keys.iter().cloned() {
                                ToolCredentialForm {
                                    key: "{key}",
                                    scope: scope_for_credentials.clone(),
                                    key_name: key,
                                    writable,
                                    on_saved: move |_| {},
                                }
                            }
                        }

                        if !save_errors_val.is_empty() {
                            div { class: "tools-settings-errors",
                                for (i , msg) in save_errors_val.iter().enumerate() {
                                    div {
                                        key: "{i}",
                                        class: "pill red tools-save-failed-chip",
                                        "{SAVE_FAILED_PREFIX}{msg}."
                                    }
                                }
                            }
                        }

                        if writable {
                            button {
                                class: "btn btn--primary",
                                disabled: saving_val || !dirty_val,
                                "aria-label": "Save tools settings",
                                onclick: move |_| {
                                    if *saving.peek() {
                                        return;
                                    }
                                    let scope_value = scope();
                                    let payload = ToolsSettingsPayload {
                                        timeout_secs: *staged_timeout_secs.peek(),
                                        timeout_overrides: staged_overrides.peek().clone(),
                                        web_search: staged_web_search.peek().clone(),
                                        web_answer: staged_web_answer.peek().clone(),
                                        web_extract: staged_web_extract.peek().clone(),
                                    };
                                    saving.set(true);
                                    save_errors.set(Vec::new());
                                    spawn(async move {
                                        match crate::server::tools_config_api::update_tools_settings(
                                                scope_value,
                                                payload.clone(),
                                            )
                                            .await
                                        {
                                            Ok(()) => {
                                                saving.set(false);
                                                dirty.set(false);
                                                last_saved_web_search.set(payload.web_search.clone());
                                                last_saved_web_answer.set(payload.web_answer.clone());
                                                last_saved_web_extract.set(payload.web_extract.clone());
                                                save_errors.set(Vec::new());
                                            }
                                            Err(e) => {
                                                saving.set(false);
                                                save_errors.set(vec![e.to_string()]);
                                                // D-21: revert the VISUAL chain order to the
                                                // last-saved snapshot — a chain list must
                                                // never show an order the config does not
                                                // have. Timeout fields stay exactly as
                                                // staged (D-11 — the in-progress edit is
                                                // never discarded).
                                                staged_web_search.set(last_saved_web_search.peek().clone());
                                                staged_web_answer.set(last_saved_web_answer.peek().clone());
                                                staged_web_extract.set(last_saved_web_extract.peek().clone());
                                            }
                                        }
                                    });
                                },
                                if saving_val { "SAVING…" } else { "SAVE" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// While the panel's `use_resource` is unresolved: 3 dimmed placeholder
/// field rows, matching the tool-catalog-grid loading pattern
/// (`staged-settings-panel/loading`).
#[component]
fn ToolsSettingsSkeleton() -> Element {
    rsx! {
        div { class: "tools-settings-skeleton",
            for i in 0..3 {
                div { key: "{i}", class: "tools-settings-skeleton-row" }
            }
        }
    }
}

/// The global timeout field plus an add/remove list of per-tool overrides
/// (Task 2). Every field is pre-filled from the loaded config — the panel
/// never opens blank (`staged-settings-panel/empty`).
#[component]
fn TimeoutOverrideEditor(
    timeout_secs: u64,
    overrides: Vec<TimeoutOverrideRow>,
    known_tool_names: Vec<String>,
    writable: bool,
    on_timeout_secs_change: EventHandler<u64>,
    on_overrides_change: EventHandler<Vec<TimeoutOverrideRow>>,
) -> Element {
    let overrides_for_add = overrides.clone();

    rsx! {
        div { class: "tools-settings-field-group",
            label {
                class: "tools-settings-label",
                r#for: "tools-settings-timeout-secs",
                "GLOBAL TIMEOUT (SECONDS)"
            }
            input {
                id: "tools-settings-timeout-secs",
                class: "tools-settings-input",
                r#type: "number",
                min: "1",
                disabled: !writable,
                value: "{timeout_secs}",
                oninput: move |evt| {
                    if let Ok(v) = evt.value().parse::<u64>() {
                        on_timeout_secs_change.call(v);
                    }
                },
            }
            p { class: "tools-settings-help",
                "Applies to any tool with no per-tool override below."
            }
        }

        div { class: "tools-settings-field-group",
            div { class: "tools-settings-label", "PER-TOOL TIMEOUT OVERRIDES" }
            p { class: "tools-settings-help",
                "A value at or below zero removes the timeout bound for that tool."
            }
            div { class: "tools-settings-override-list",
                for (idx , row) in overrides.iter().cloned().enumerate() {
                    div { key: "{idx}", class: "tools-settings-override-row",
                        select {
                            class: "tools-settings-select",
                            "aria-label": "Tool for override row {idx}",
                            disabled: !writable,
                            value: "{row.tool}",
                            onchange: {
                                let overrides_next = overrides.clone();
                                move |evt: FormEvent| {
                                    let mut next = overrides_next.clone();
                                    if idx < next.len() {
                                        next[idx].tool = evt.value();
                                        on_overrides_change.call(next);
                                    }
                                }
                            },
                            option { value: "", "— select tool —" }
                            for name in known_tool_names.iter().cloned() {
                                option { key: "{name}", value: "{name}", "{name}" }
                            }
                        }
                        input {
                            class: "tools-settings-input tools-settings-input--sm",
                            "aria-label": "Timeout seconds for override row {idx}",
                            r#type: "number",
                            disabled: !writable,
                            value: "{row.seconds}",
                            oninput: {
                                let overrides_next = overrides.clone();
                                move |evt: FormEvent| {
                                    if let Ok(v) = evt.value().parse::<i64>() {
                                        let mut next = overrides_next.clone();
                                        if idx < next.len() {
                                            next[idx].seconds = v;
                                            on_overrides_change.call(next);
                                        }
                                    }
                                }
                            },
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            "aria-label": "Remove override row {idx}",
                            disabled: !writable,
                            onclick: {
                                let overrides_next = overrides.clone();
                                move |_| {
                                    let mut next = overrides_next.clone();
                                    if idx < next.len() {
                                        next.remove(idx);
                                        on_overrides_change.call(next);
                                    }
                                }
                            },
                            "REMOVE"
                        }
                    }
                }
            }
            button {
                class: "btn btn--ghost btn--sm",
                "aria-label": "Add per-tool timeout override",
                disabled: !writable,
                onclick: move |_| {
                    let mut next = overrides_for_add.clone();
                    next.push(TimeoutOverrideRow {
                        tool: String::new(),
                        seconds: 60,
                    });
                    on_overrides_change.call(next);
                },
                "ADD OVERRIDE"
            }
        }
    }
}

// =============================================================================
// Task 3 (D-21): provider chain lists — drag-and-keyboard reordering,
// skipped-entry notes, add/remove constrained to the legal provider set.
// =============================================================================

/// Move the entry at `idx` up by one position (toward index 0). A no-op
/// when `idx` is already 0 or out of range.
#[allow(dead_code)] // used inside ProviderChainList's on_move_up handler; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn move_entry_up(items: &mut [String], idx: usize) {
    if idx == 0 || idx >= items.len() {
        return;
    }
    items.swap(idx, idx - 1);
}

/// Move the entry at `idx` down by one position (toward the end). A no-op
/// when `idx` is already the last index or out of range.
#[allow(dead_code)] // used inside ProviderChainList's on_move_down handler; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn move_entry_down(items: &mut [String], idx: usize) {
    if idx + 1 >= items.len() {
        return;
    }
    items.swap(idx, idx + 1);
}

/// Move the entry at `from` to position `to` (drag-and-drop reorder). A
/// no-op when either index is out of range.
#[allow(dead_code)] // used inside ProviderChainList's on_drop handler; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn move_entry_to(items: &mut Vec<String>, from: usize, to: usize) {
    if from >= items.len() || to >= items.len() {
        return;
    }
    let entry = items.remove(from);
    items.insert(to, entry);
}

/// One reorderable list per web provider chain (`web_search`/`web_answer`/
/// `web_extract`), a first-class section with its own heading — never
/// folded into the generic field grid (D-21). Reordering only mutates the
/// staged copy via `on_change`; nothing is written until SAVE.
#[component]
fn ProviderChainList(
    heading: String,
    tool: String,
    chain: Vec<String>,
    legal_providers: Vec<String>,
    skipped_by_provider: std::collections::HashMap<String, bool>,
    credential_by_provider: std::collections::HashMap<String, String>,
    writable: bool,
    on_change: EventHandler<Vec<String>>,
) -> Element {
    // Component-local drag source — scoped to THIS chain's list only, so a
    // drag started in one chain can never be dropped into a sibling chain's
    // list (each ProviderChainList instance owns its own index space).
    let mut drag_source: Signal<Option<usize>> = use_signal(|| None);

    // zero-one-many (D-21 backstop): one entry disables reordering; two or
    // more enable the drag handle and both move controls.
    let can_reorder = chain.len() >= 2;

    let chain_for_add = chain.clone();
    let addable: Vec<String> = legal_providers
        .iter()
        .filter(|p| !chain.contains(p))
        .cloned()
        .collect();

    rsx! {
        div { class: "provider-chain-section",
            div { class: "provider-chain-heading", "{heading}" }
            div { class: "provider-chain-list ih-scroll",
                for (idx , provider) in chain.iter().cloned().enumerate() {
                    ChainEntryRow {
                        key: "{provider}-{idx}",
                        tool: tool.clone(),
                        idx,
                        provider: provider.clone(),
                        skipped: *skipped_by_provider.get(&provider).unwrap_or(&false),
                        credential: credential_by_provider.get(&provider).cloned(),
                        can_reorder,
                        writable,
                        on_move_up: {
                            let chain_next = chain.clone();
                            move |_| {
                                let mut next = chain_next.clone();
                                move_entry_up(&mut next, idx);
                                on_change.call(next);
                            }
                        },
                        on_move_down: {
                            let chain_next = chain.clone();
                            move |_| {
                                let mut next = chain_next.clone();
                                move_entry_down(&mut next, idx);
                                on_change.call(next);
                            }
                        },
                        on_remove: {
                            let chain_next = chain.clone();
                            move |_| {
                                let mut next = chain_next.clone();
                                if idx < next.len() {
                                    next.remove(idx);
                                    on_change.call(next);
                                }
                            }
                        },
                        on_drag_start: move |_| {
                            drag_source.set(Some(idx));
                        },
                        on_drop: {
                            let chain_next = chain.clone();
                            move |_| {
                                if let Some(from) = *drag_source.read() {
                                    let mut next = chain_next.clone();
                                    move_entry_to(&mut next, from, idx);
                                    on_change.call(next);
                                }
                                drag_source.set(None);
                            }
                        },
                    }
                }
            }
            if writable && !addable.is_empty() {
                select {
                    class: "tools-settings-select",
                    "aria-label": "Add provider to {tool} chain",
                    value: "",
                    onchange: move |evt: FormEvent| {
                        let value = evt.value();
                        if !value.is_empty() {
                            let mut next = chain_for_add.clone();
                            next.push(value);
                            on_change.call(next);
                        }
                    },
                    option { value: "", "— add provider —" }
                    for p in addable.iter().cloned() {
                        option { key: "{p}", value: "{p}", "{p}" }
                    }
                }
            }
        }
    }
}

/// One entry in a `ProviderChainList` (D-21). Draggable via native HTML5
/// DnD (`draggable` + `ondragstart`/`ondragover`/`ondrop`, mirroring the
/// kanban board's proven pattern — `kanban/card.rs`/`kanban/column.rs`)
/// alongside keyboard-reachable move-up/move-down buttons operating on the
/// SAME staged vector, so reordering is never mouse-only. A skipped entry
/// (credential absent) stays in the chain and stays draggable — it is a
/// diagnosis, not a removal.
#[allow(clippy::too_many_arguments)]
#[component]
fn ChainEntryRow(
    tool: String,
    idx: usize,
    provider: String,
    skipped: bool,
    credential: Option<String>,
    can_reorder: bool,
    writable: bool,
    on_move_up: EventHandler<()>,
    on_move_down: EventHandler<()>,
    on_remove: EventHandler<()>,
    on_drag_start: EventHandler<()>,
    on_drop: EventHandler<()>,
) -> Element {
    let can_drag = can_reorder && writable;
    let credential_name = credential.clone().unwrap_or_default();

    rsx! {
        div {
            class: "chain-entry-row",
            class: if skipped { "chain-entry-row--skipped" },
            draggable: if can_drag { "true" } else { "false" },
            ondragstart: move |_| {
                on_drag_start.call(());
            },
            ondragover: move |evt: DragEvent| {
                evt.prevent_default();
            },
            ondrop: move |evt: DragEvent| {
                evt.prevent_default();
                on_drop.call(());
            },
            span {
                class: "chain-entry-drag-handle",
                class: if !can_drag { "chain-entry-drag-handle--disabled" },
                "aria-label": "Drag {provider} to reorder in {tool} chain",
                "aria-disabled": if !can_drag { "true" },
                "⠿"
            }
            span { class: "chain-entry-provider", "{provider}" }
            if skipped {
                span { class: "chain-entry-skipped-note",
                    "SKIPPED — missing {credential_name}"
                }
            }
            div { class: "chain-entry-move-controls",
                button {
                    class: "btn btn--ghost btn--sm chain-entry-move-btn",
                    "aria-label": "Move {provider} up in {tool} chain",
                    disabled: !can_reorder || !writable,
                    onclick: move |_| on_move_up.call(()),
                    "▲"
                }
                button {
                    class: "btn btn--ghost btn--sm chain-entry-move-btn",
                    "aria-label": "Move {provider} down in {tool} chain",
                    disabled: !can_reorder || !writable,
                    onclick: move |_| on_move_down.call(()),
                    "▼"
                }
            }
            button {
                class: "btn btn--ghost btn--sm",
                "aria-label": "Remove {provider} from {tool} chain",
                disabled: !writable,
                onclick: move |_| on_remove.call(()),
                "REMOVE"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// move-up from index 0 is a no-op.
    #[test]
    fn move_up_from_index_zero_is_a_no_op() {
        let mut items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        move_entry_up(&mut items, 0);
        assert_eq!(items, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    /// move-down from the last index is a no-op.
    #[test]
    fn move_down_from_last_index_is_a_no_op() {
        let mut items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        move_entry_down(&mut items, 2);
        assert_eq!(items, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    /// A drag from index 2 to index 0 produces the expected order.
    #[test]
    fn drag_from_index_two_to_index_zero_produces_expected_order() {
        let mut items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        move_entry_to(&mut items, 2, 0);
        assert_eq!(items, vec!["c".to_string(), "a".to_string(), "b".to_string()]);
    }

    /// move-up on a normal middle index swaps with its predecessor.
    #[test]
    fn move_up_swaps_with_predecessor() {
        let mut items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        move_entry_up(&mut items, 1);
        assert_eq!(items, vec!["b".to_string(), "a".to_string(), "c".to_string()]);
    }

    /// move-down on a normal middle index swaps with its successor.
    #[test]
    fn move_down_swaps_with_successor() {
        let mut items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        move_entry_down(&mut items, 1);
        assert_eq!(items, vec!["a".to_string(), "c".to_string(), "b".to_string()]);
    }
}
