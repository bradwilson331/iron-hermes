//! Memory screen, `Tier 2` tab (Phase 58.2 Plan 10, D-13 / D-23). Contract: `58.2-UI-SPEC.md`.
//!
//! Read-mostly view of Tier 2 long-term memory: committed search, claim list, and the only two
//! writes, `Forget` (stage) and `Undo` (unstage), via `server::tier2_api`. Everything below
//! `Tier2Panel` is presentational (plain values plus `EventHandler`s) so the SSR render tests can
//! drive it without a server; `Tier2Panel` only wires resources and signals into those parts.
//!
//! Hook discipline (see `memory.rs`): every hook is declared before any branch, no `?` on a
//! resource, `.read()`-style access in render and `.peek()` only inside handlers.

use std::rc::Rc;

use dioxus::prelude::*;

use super::memory::extract_memory_error_message;
use crate::server::tier2_api::{
    Tier2ClaimRow, Tier2ExplainView, Tier2PurgePreviewView, Tier2PurgeResultView, tier2_explain,
    tier2_list, tier2_list_staged, tier2_purge_now, tier2_purge_preview, tier2_search,
    tier2_stage_forget, tier2_status, tier2_unstage_forget,
};

/// Recent-list cap (UI-SPEC States table).
const RECENT_LIMIT: u32 = 50;
/// Search-result cap (UI-SPEC States table).
const SEARCH_LIMIT: u32 = 20;

const META: &str = "font-size:10px;line-height:1.4;color:var(--gray);";
const META_CAUTION: &str = "font-size:10px;line-height:1.4;color:var(--amber);";
const STRONG: &str = "font-size:12px;font-weight:700;color:var(--text);";
const FORGET_OFF_TITLE: &str = "Forgetting is off. Set memory.forgetting_enabled to true in config.yaml, then restart the server.";
const LOAD_ERROR: &str = "Could not load Tier 2 memory — check server connection.";

// ── Pure view-model helpers ─────────────────────────────────────────────────

/// Whole-percent confidence, e.g. `0.954` -> `95%`.
fn format_confidence(c: f64) -> String {
    format!("{}%", (c * 100.0).round() as i64)
}

/// `amber` below 80% (UI-SPEC G10), neutral otherwise.
fn confidence_tone(c: f64) -> &'static str {
    if c < 0.8 { "amber" } else { "" }
}

/// `memory://{path}#L{line}` -> `{path}:{line}`; anything else is returned raw.
fn format_source(s: &str) -> String {
    s.strip_prefix("memory://")
        .and_then(|rest| rest.split_once("#L"))
        .map(|(path, line)| format!("{path}:{line}"))
        .unwrap_or_else(|| s.to_string())
}

/// First `n` characters, plus `…` only when something was cut.
fn clip_chars(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    } else {
        s.to_string()
    }
}

/// Heading and body for an empty list (no claims, or no search match).
fn empty_state(query: &str) -> (String, String) {
    if query.is_empty() {
        (
            "No claims yet".to_string(),
            "Claims appear here as memory is indexed and consolidated.".to_string(),
        )
    } else {
        (
            format!("No claims match \"{query}\""),
            "Try different words, or clear the search to see recent claims.".to_string(),
        )
    }
}

/// The cap line, only when the list is exactly as long as its limit.
fn cap_line(len: usize, searching: bool) -> Option<String> {
    if searching {
        (len == SEARCH_LIMIT as usize)
            .then(|| format!("Showing the first {SEARCH_LIMIT}. Narrow your search to see others."))
    } else {
        (len == RECENT_LIMIT as usize)
            .then(|| format!("Showing the {RECENT_LIMIT} most recent claims."))
    }
}

/// The committed query: trimmed and capped at 500 characters; blank means the recent list.
fn commit_query(s: &str) -> String {
    s.trim().chars().take(500).collect()
}

/// The WRITE actions a row offers (UI-SPEC G13): `Forget` on an active claim, `Undo` on a staged one.
// `allow(dead_code)`: pinned by `tier2_panel_actions_are_only_forget_and_undo`; the native "bin"
// dead-code pass does not trace through the Dioxus component tree (same note as agents.rs).
#[allow(dead_code)]
fn row_actions(staged: bool) -> &'static [&'static str] {
    if staged { &["Undo"] } else { &["Forget"] }
}

/// `2026-11-08T14:03:22.123Z` -> `2026-11-08 14:03` (no date library, UI-SPEC section 4).
/// Anything not shaped `YYYY-MM-DDTHH:MM…` is returned raw.
fn format_timestamp(s: &str) -> String {
    let b = s.as_bytes();
    let digit = |i: usize| b[i].is_ascii_digit();
    let shaped = b.len() >= 16
        && [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15]
            .iter()
            .all(|&i| digit(i))
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':';
    if shaped {
        format!("{} {}", &s[..10], &s[11..16])
    } else {
        s.to_string()
    }
}

/// The Staged panel's amber consequence line (D-21, D-28): staged claims are hidden now and
/// deleted only when a human runs a purge.
fn staged_consequence(forgetting_enabled: bool) -> &'static str {
    if forgetting_enabled {
        "Hidden from search now. Nothing is deleted until someone runs Purge now or ironhermes memory purge; that deletes them permanently, with any matching line in MEMORY.md, USER.md and the daily notes. Undo restores a claim until then."
    } else {
        "Forgetting is off, so nothing is purged. Staged claims stay hidden until memory.forgetting_enabled is true."
    }
}

const WRITES_OFF_TITLE: &str = "Memory edits are disabled while web config writes are off.";
const NOTHING_STAGED: &str = "Nothing is staged to forget.";
const PURGE_LOGIN_TITLE: &str = "Purging from the web needs web login. With login off, run `ironhermes memory purge` in a terminal.";
const WRITES_LOCKED_TITLE: &str = "Cancel Purge or finish it first.";
const PURGE_WORD: &str = "purge";

/// `1 claim`, `2 claims`, `0 claims`: every purge plural goes through here (UI-SPEC R6).
fn count_noun(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The typed word must be exactly `purge`: no trimming, stricter than the server on purpose.
fn purge_confirm_enabled(value: &str) -> bool {
    value == PURGE_WORD
}

/// Why `Purge now…` is disabled; the first match wins (UI-SPEC section 10).
fn purge_entry_title(
    forgetting_enabled: bool,
    writes_enabled: bool,
    purge_login: bool,
    staged: bool,
) -> Option<&'static str> {
    if !forgetting_enabled {
        Some(FORGET_OFF_TITLE)
    } else if !writes_enabled {
        Some(WRITES_OFF_TITLE)
    } else if !purge_login {
        Some(PURGE_LOGIN_TITLE)
    } else if !staged {
        Some(NOTHING_STAGED)
    } else {
        None
    }
}

/// The success note and whether it is an amber caution. `sent` is how many ids the purge was
/// asked for; ids neither purged nor reported in `not_staged` are still staged.
fn purge_result_note(sent: usize, r: &Tier2PurgeResultView) -> (String, bool) {
    let mut s = format!(
        "Purged {}. Removed {} from MEMORY.md and USER.md and cleared {}.",
        count_noun(r.purged, "claim", "claims"),
        count_noun(r.tier1_lines_removed, "line", "lines"),
        count_noun(
            r.live_lines_retracted,
            "daily-note line",
            "daily-note lines"
        ),
    );
    if !r.not_staged.is_empty() {
        s.push_str(" Some claims were cancelled since the preview and were kept.");
    }
    let k = sent
        .saturating_sub(r.purged)
        .saturating_sub(r.not_staged.len());
    if k > 0 {
        s.push_str(&format!(
            " {} could not be purged and {} still staged. Run Purge now again.",
            count_noun(k, "claim", "claims"),
            if k == 1 { "is" } else { "are" }
        ));
    }
    if let Some(why) = r.scrub_incomplete.as_deref() {
        s.push_str(&format!(
            " On-disk cleanup incomplete: {why}. Stop other IronHermes processes and see docs/MEMORY-TIER2.md for a full scrub."
        ));
    }
    (s, r.scrub_incomplete.is_some())
}

/// The confirm view's error line. A `skipped` purge deleted nothing, and says so.
fn purge_error_line(msg: &str, skipped: bool) -> String {
    if skipped {
        format!("Could not purge: {msg}. Nothing was deleted.")
    } else {
        format!("Could not purge: {msg}")
    }
}

/// The Staged panel's purge state machine (UI-SPEC section 10).
#[derive(Clone, PartialEq, Debug)]
enum PurgeView {
    Idle,
    Previewing,
    Confirming(Tier2PurgePreviewView),
    Deleting(Tier2PurgePreviewView),
}

#[derive(Clone, PartialEq, Debug)]
enum ExplainState {
    Prompt,
    Loading,
    NotFound,
    Error(String),
    Loaded(Tier2ExplainView),
}

/// Maps the selection and the explain resource snapshot to what the drawer shows. A loaded view
/// for a different id is the previous claim's stale load, so it reads as `Loading`.
fn explain_state(
    selected: Option<&str>,
    snapshot: Option<Result<Option<Tier2ExplainView>, String>>,
) -> ExplainState {
    let Some(id) = selected else {
        return ExplainState::Prompt;
    };
    match snapshot {
        None => ExplainState::Loading,
        Some(Err(reason)) => ExplainState::Error(reason),
        Some(Ok(None)) => ExplainState::NotFound,
        Some(Ok(Some(v))) if v.claim_id == id => ExplainState::Loaded(v),
        Some(Ok(Some(_))) => ExplainState::Loading,
    }
}

// ── Presentational components ───────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum StatusNotice {
    Loading,
    LoadError,
    Off,
}

#[derive(Clone, PartialEq)]
enum ListState {
    Loading,
    LoadError,
    Rows(Vec<Tier2ClaimRow>),
}

/// The shared load-error line with its `Retry`.
#[component]
fn LoadErrorLine(on_retry: EventHandler<()>) -> Element {
    rsx! {
        div { style: "display:flex;align-items:center;gap:8px;color:var(--danger);font-size:12px;",
            span { "{LOAD_ERROR}" }
            button {
                class: "btn btn--ghost btn--sm",
                r#type: "button",
                onclick: move |_| on_retry.call(()),
                "Retry"
            }
        }
    }
}

#[component]
fn Tier2StatusNotice(kind: StatusNotice, on_retry: EventHandler<()>) -> Element {
    rsx! {
        div { class: "panel",
            match kind {
                StatusNotice::Loading => rsx! {
                    div { style: "color:var(--gray);font-size:12px;", "Loading claims…" }
                },
                StatusNotice::LoadError => rsx! {
                    LoadErrorLine { on_retry }
                },
                StatusNotice::Off => rsx! {
                    div { style: STRONG, "Tier 2 memory is off" }
                    div { style: "font-size:12px;color:var(--text-dim);",
                        "Set memory.tier2_enabled to true in config.yaml, then restart the server. Tier 1 memory is unaffected."
                    }
                },
            }
        }
    }
}

#[component]
fn SearchForm(
    query: String,
    searching: bool,
    on_input: EventHandler<String>,
    on_submit: EventHandler<()>,
    on_clear: EventHandler<()>,
    on_mounted: EventHandler<Rc<MountedData>>,
) -> Element {
    rsx! {
        form {
            style: "display:flex;gap:8px;",
            onsubmit: move |evt| {
                evt.prevent_default();
                on_submit.call(());
            },
            input {
                class: "field-input t2-search",
                r#type: "search",
                maxlength: "500",
                value: "{query}",
                placeholder: "Search long-term memory…",
                "aria-label": "Search Tier 2 claims",
                oninput: move |e| on_input.call(e.value()),
                onmounted: move |e: Event<MountedData>| on_mounted.call(e.data()),
            }
            button { class: "btn btn--sm", r#type: "submit", "Search" }
            if searching {
                button {
                    class: "btn btn--ghost btn--sm",
                    r#type: "button",
                    onclick: move |_| on_clear.call(()),
                    "Clear"
                }
            }
        }
    }
}

/// `mode` is None for the recent list; `Some("semantic" | "keyword")` for search results.
#[component]
fn ModeLine(submitted: String, mode: Option<String>, note: Option<String>) -> Element {
    let left = if submitted.is_empty() {
        "Recent claims".to_string()
    } else {
        format!("Results for \"{}\"", clip_chars(&submitted, 40))
    };
    let keyword = mode.as_deref() == Some("keyword");
    rsx! {
        div { style: "display:flex;flex-direction:column;gap:4px;",
            div { style: "display:flex;justify-content:space-between;align-items:center;",
                span { style: META, "{left}" }
                if let Some(m) = mode.as_deref() {
                    if m == "keyword" {
                        span { class: "pill amber", "KEYWORD" }
                    } else {
                        span { class: "pill", "SEMANTIC" }
                    }
                }
            }
            if keyword {
                if let Some(n) = note.as_deref() {
                    div { style: "font-size:10px;line-height:1.4;color:var(--amber);", "{n}" }
                }
            }
        }
    }
}

/// Action feedback (UI-SPEC G9). Keyed on the text so each new note remounts and takes focus
/// (the vanished row leaves focus nowhere otherwise); `aria-live` announces it regardless.
#[component]
fn StatusNote(note: Option<String>, #[props(default)] caution: bool) -> Element {
    let style = if caution { META_CAUTION } else { META };
    rsx! {
        for n in note.iter() {
            div {
                key: "{n}",
                style,
                tabindex: "-1",
                role: "status",
                "aria-live": "polite",
                onmounted: move |e: Event<MountedData>| {
                    spawn(async move {
                        let _ = e.data().set_focus(true).await;
                    });
                },
                "{n}"
            }
        }
    }
}

#[component]
fn ActionError(message: Option<String>) -> Element {
    rsx! {
        if let Some(m) = message {
            div { role: "alert", style: "color:var(--danger);font-size:12px;", "{m}" }
        }
    }
}

#[component]
fn ClaimRowView(
    row: Tier2ClaimRow,
    selected: bool,
    busy: bool,
    forgetting_enabled: bool,
    writes_locked: bool,
    on_details: EventHandler<Option<Rc<MountedData>>>,
    on_forget: EventHandler<()>,
) -> Element {
    let forget_title = if !forgetting_enabled {
        Some(FORGET_OFF_TITLE)
    } else if writes_locked {
        Some(WRITES_LOCKED_TITLE)
    } else {
        None
    };
    // The Details button's own handle, passed up so Close can return focus to it.
    let mut details_el: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let tone = confidence_tone(row.confidence);
    let pct = format_confidence(row.confidence);
    let source_label = format_source(&row.source);
    let clipped = clip_chars(&row.text, 60);
    let details_label = format!("Show evidence for claim: {clipped}");
    let forget_label = format!("Forget claim: {clipped}");
    let row_style = if selected {
        "border-left:2px solid var(--teal);padding-left:8px;"
    } else {
        ""
    };
    let text_style = if selected {
        "overflow-wrap:anywhere;color:var(--text);"
    } else {
        "overflow-wrap:anywhere;"
    };
    rsx! {
        div { class: "mem-entry", style: "{row_style}",
            div { class: "mem-ts",
                span { class: "pill", class: if !tone.is_empty() { "amber" }, "{pct}" }
            }
            div { class: "mem-body",
                div { style: "{text_style}", "{row.text}" }
                div {
                    title: "{row.source}",
                    style: "{META}overflow:hidden;text-overflow:ellipsis;white-space:nowrap;",
                    "{source_label}"
                }
                div { style: "display:flex;gap:4px;margin-top:8px;",
                    button {
                        class: "btn btn--ghost btn--sm",
                        r#type: "button",
                        "aria-label": "{details_label}",
                        "aria-controls": "t2-explain",
                        "aria-expanded": "{selected}",
                        onmounted: move |e: Event<MountedData>| details_el.set(Some(e.data())),
                        onclick: move |_| on_details.call(details_el.peek().clone()),
                        "Details"
                    }
                    button {
                        class: "btn btn--ghost btn--sm",
                        r#type: "button",
                        "aria-label": "{forget_label}",
                        disabled: busy || !forgetting_enabled || writes_locked,
                        title: forget_title,
                        onclick: move |_| on_forget.call(()),
                        if busy {
                            "Staging…"
                        } else {
                            "Forget"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ClaimsListBody(
    state: ListState,
    submitted: String,
    selected: Option<String>,
    busy_id: Option<String>,
    forgetting_enabled: bool,
    writes_locked: bool,
    on_details: EventHandler<(Tier2ClaimRow, Option<Rc<MountedData>>)>,
    on_forget: EventHandler<Tier2ClaimRow>,
    on_clear: EventHandler<()>,
    on_retry: EventHandler<()>,
) -> Element {
    let searching = !submitted.is_empty();
    match state {
        ListState::Loading => rsx! {
            div { style: "color:var(--gray);font-size:12px;", "Loading claims…" }
        },
        ListState::LoadError => rsx! {
            LoadErrorLine { on_retry }
        },
        ListState::Rows(rows) if rows.is_empty() => {
            let (heading, body) = empty_state(&submitted);
            rsx! {
                div { style: "padding:24px 0;display:flex;flex-direction:column;gap:8px;align-items:flex-start;",
                    div { style: STRONG, "{heading}" }
                    div { style: "font-size:12px;color:var(--text-dim);", "{body}" }
                    if searching {
                        button {
                            class: "btn btn--ghost btn--sm",
                            r#type: "button",
                            onclick: move |_| on_clear.call(()),
                            "Clear"
                        }
                    }
                }
            }
        }
        ListState::Rows(rows) => {
            let cap = cap_line(rows.len(), searching);
            rsx! {
                div { class: "row-list",
                    for r in rows.iter() {
                        ClaimRowView {
                            key: "{r.claim_id}",
                            row: r.clone(),
                            selected: selected.as_deref() == Some(r.claim_id.as_str()),
                            busy: busy_id.as_deref() == Some(r.claim_id.as_str()),
                            forgetting_enabled,
                            writes_locked,
                            on_details: {
                                let r = r.clone();
                                move |m| on_details.call((r.clone(), m))
                            },
                            on_forget: {
                                let r = r.clone();
                                move |_| on_forget.call(r.clone())
                            },
                        }
                    }
                }
                if let Some(line) = cap {
                    div { style: META, "{line}" }
                }
            }
        }
    }
}

const LABEL: &str = "font-size:10px;font-weight:700;line-height:1.4;color:var(--text-dim);";

/// Body of the "Why this claim" panel (UI-SPEC section 4). Evidence quotes come from
/// conversation content, so they are plain text nodes in a scrolling box, never raw HTML.
#[component]
fn ExplainBody(
    state: ExplainState,
    on_close: EventHandler<()>,
    on_retry: EventHandler<()>,
) -> Element {
    match state {
        ExplainState::Prompt => rsx! {
            div { style: "{META}", "Select Details on a claim to see its evidence and what it replaced." }
        },
        ExplainState::Loading => rsx! {
            div { style: "color:var(--gray);font-size:12px;", "Loading evidence…" }
        },
        ExplainState::NotFound => rsx! {
            div { style: "font-size:12px;color:var(--text-dim);",
                "This claim is no longer available. It may have been forgotten or replaced."
            }
            div {
                button {
                    class: "btn btn--ghost btn--sm",
                    r#type: "button",
                    onclick: move |_| on_close.call(()),
                    "Close"
                }
            }
        },
        ExplainState::Error(reason) => rsx! {
            div { style: "color:var(--danger);font-size:12px;",
                "Could not load this claim's evidence: {reason}."
            }
            div {
                button {
                    class: "btn btn--ghost btn--sm",
                    r#type: "button",
                    onclick: move |_| on_retry.call(()),
                    "Retry"
                }
            }
        },
        ExplainState::Loaded(v) => {
            let tone = confidence_tone(v.confidence);
            let pct = format_confidence(v.confidence);
            let n = v.evidence.len();
            rsx! {
                div { style: "display:flex;justify-content:space-between;align-items:flex-start;gap:8px;",
                    div { style: "font-size:12px;color:var(--text);overflow-wrap:anywhere;", "{v.text}" }
                    div { style: "display:flex;align-items:center;gap:8px;flex-shrink:0;",
                        span { class: "pill", class: if !tone.is_empty() { "amber" }, "{pct}" }
                        button {
                            class: "btn btn--ghost btn--sm",
                            r#type: "button",
                            onclick: move |_| on_close.call(()),
                            "Close"
                        }
                    }
                }
                div { style: LABEL, "EVIDENCE · {n}" }
                if v.evidence.is_empty() {
                    div { style: "font-size:12px;color:var(--text-dim);", "No evidence is recorded for this claim." }
                } else {
                    for (i , e) in v.evidence.iter().enumerate() {
                        div { key: "{i}", style: "display:flex;flex-direction:column;gap:4px;",
                            div {
                                style: "background:var(--bg-input);font-size:12px;padding:8px;white-space:pre-wrap;max-height:160px;overflow:auto;overflow-wrap:anywhere;",
                                "{e.quote}"
                            }
                            div { style: META, "{format_source(&e.source)} · {format_timestamp(&e.timestamp)}" }
                        }
                    }
                }
                div { style: LABEL, "REPLACES" }
                if v.supersedes.is_empty() {
                    div { style: "font-size:12px;color:var(--text-dim);", "Replaces no earlier claims." }
                } else {
                    for id in v.supersedes.iter() {
                        div { key: "{id}", style: "{META}overflow-wrap:anywhere;", "{id}" }
                    }
                }
            }
        }
    }
}

/// The Staged to forget panel (UI-SPEC section 5). `Undo` is never disabled by
/// `forgetting_enabled`: it relays the server's answer, and the amber line explains the state.
#[component]
fn StagedPanelView(
    rows: Vec<Tier2ClaimRow>,
    loading: bool,
    load_error: bool,
    busy_id: Option<String>,
    forgetting_enabled: bool,
    writes_enabled: bool,
    purge_login: bool,
    purge_view: PurgeView,
    typed: String,
    purge_error: Option<String>,
    preview_error: Option<String>,
    input_gen: u32,
    focus_entry: bool,
    on_undo: EventHandler<Tier2ClaimRow>,
    on_retry: EventHandler<()>,
    on_purge_open: EventHandler<()>,
    on_purge_input: EventHandler<String>,
    on_purge_confirm: EventHandler<()>,
    on_purge_cancel: EventHandler<()>,
    on_group_mounted: EventHandler<Rc<MountedData>>,
) -> Element {
    // The panel itself takes focus when `Purge now…` is disabled at Cancel time (UI-SPEC section 10).
    let mut panel_el: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let count = if loading || load_error {
        "—".to_string()
    } else {
        rows.len().to_string()
    };
    let confirm_view = match &purge_view {
        PurgeView::Confirming(v) | PurgeView::Deleting(v) => Some(v.clone()),
        _ => None,
    };
    let deleting = matches!(purge_view, PurgeView::Deleting(_));
    let previewing = purge_view == PurgeView::Previewing;
    let entry_title = purge_entry_title(
        forgetting_enabled,
        writes_enabled,
        purge_login,
        !rows.is_empty(),
    );
    let entry_disabled = entry_title.is_some() || previewing;
    rsx! {
        div {
            class: "panel",
            tabindex: "-1",
            onmounted: move |e: Event<MountedData>| panel_el.set(Some(e.data())),
            div { style: "display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;",
                div { class: "panel-title",
                    "Staged to forget "
                    span { class: "count", style: "color:var(--teal)", "· {count}" }
                }
                if confirm_view.is_none() {
                    button {
                        class: "btn btn--danger btn--sm",
                        r#type: "button",
                        disabled: entry_disabled,
                        title: entry_title,
                        onmounted: move |e: Event<MountedData>| {
                            if focus_entry {
                                let target = if entry_disabled {
                                    panel_el.peek().clone()
                                } else {
                                    Some(e.data())
                                };
                                if let Some(t) = target {
                                    spawn(async move {
                                        let _ = t.set_focus(true).await;
                                    });
                                }
                            }
                        },
                        onclick: move |_| on_purge_open.call(()),
                        if previewing {
                            "Loading preview…"
                        } else {
                            "Purge now…"
                        }
                    }
                }
            }
            if let Some(v) = confirm_view {
                PurgeConfirmView {
                    view: v,
                    typed,
                    deleting,
                    error: purge_error,
                    input_gen,
                    on_input: on_purge_input,
                    on_confirm: on_purge_confirm,
                    on_cancel: on_purge_cancel,
                    on_group_mounted,
                }
            } else {
                ActionError { message: preview_error }
                div { class: "panel-sub", style: "color:var(--amber);", "{staged_consequence(forgetting_enabled)}" }
                if loading {
                    div { style: "color:var(--gray);font-size:12px;", "Loading claims…" }
                } else if load_error {
                    LoadErrorLine { on_retry }
                } else if rows.is_empty() {
                    div { style: "color:var(--gray);font-size:12px;", "{NOTHING_STAGED}" }
                } else {
                    div { class: "row-list",
                        for r in rows.iter() {
                            StagedRow {
                                key: "{r.claim_id}",
                                row: r.clone(),
                                busy: busy_id.as_deref() == Some(r.claim_id.as_str()),
                                on_undo: {
                                    let r = r.clone();
                                    move |_| on_undo.call(r.clone())
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The purge confirm view (UI-SPEC section 10), docked in the Staged panel. Every string from the
/// server (claim text, Tier 1 entries) is a text node (T-58.2-87). There is deliberately no
/// `<form>`: Enter in the input must not start an irreversible delete.
#[component]
fn PurgeConfirmView(
    view: Tier2PurgePreviewView,
    typed: String,
    deleting: bool,
    error: Option<String>,
    input_gen: u32,
    on_input: EventHandler<String>,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
    on_group_mounted: EventHandler<Rc<MountedData>>,
) -> Element {
    let heading = format!(
        "Permanently delete {}?",
        count_noun(view.rows.len(), "claim", "claims")
    );
    let confirm_ok = purge_confirm_enabled(&typed);
    let delete_title = if confirm_ok || deleting {
        None
    } else {
        Some("Type purge to enable this button.")
    };
    rsx! {
        div {
            role: "group",
            "aria-labelledby": "t2-purge-title",
            "aria-busy": if deleting { "true" },
            tabindex: "-1",
            style: "display:flex;flex-direction:column;gap:16px;",
            onmounted: move |e: Event<MountedData>| on_group_mounted.call(e.data()),
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Escape && !deleting {
                    on_cancel.call(());
                }
            },
            div { id: "t2-purge-title", style: "font-size:12px;font-weight:700;color:var(--text);",
                "{heading}"
            }
            div {
                role: "region",
                "aria-label": "Claims to be deleted",
                tabindex: "0",
                style: "max-height:320px;overflow:auto;display:flex;flex-direction:column;gap:16px;",
                for r in view.rows.iter() {
                    div { key: "{r.claim_id}", style: "display:flex;flex-direction:column;gap:4px;",
                        div { style: "font-size:12px;overflow-wrap:anywhere;", "{r.text}" }
                        for (i , entry) in r.tier1_entries.iter().enumerate() {
                            div {
                                key: "{i}",
                                style: "background:var(--bg-input);font-size:12px;padding:8px;white-space:pre-wrap;overflow-wrap:anywhere;",
                                "{entry}"
                            }
                        }
                        if !r.predecessors.is_empty() {
                            div { style: META,
                                {format!("Also deletes {}", count_noun(r.predecessors.len(), "older wording", "older wordings"))}
                            }
                        }
                        if r.live_lines > 0 {
                            div { style: META,
                                {format!("Clears {}", count_noun(r.live_lines, "daily-note line", "daily-note lines"))}
                            }
                        }
                    }
                }
            }
            div { style: "display:flex;flex-direction:column;gap:4px;",
                div { style: META, "Other claims' quotes to redact: {view.redacted_quotes}" }
                div { style: META,
                    "Also removed wherever they are cited: the bank, people and groups files, dreams and ALIGNMENT_SYNTHESIS.md. The search index is rebuilt."
                }
                div { style: META, "Not touched: chat transcripts (session search) and backups." }
            }
            div {
                id: "t2-purge-warn",
                style: "font-size:12px;font-weight:700;color:var(--danger);",
                "This cannot be undone."
            }
            div { style: "display:flex;flex-direction:column;gap:4px;",
                label { r#for: "t2-purge-input", style: LABEL, "Type purge to confirm" }
                // Keyed on `input_gen` so a refused purge remounts the input, which focuses it.
                input {
                    key: "{input_gen}",
                    id: "t2-purge-input",
                    class: "field-input t2-purge-input",
                    r#type: "text",
                    maxlength: "16",
                    autocomplete: "off",
                    autocapitalize: "off",
                    spellcheck: "false",
                    "aria-label": "Type purge to confirm",
                    "aria-describedby": "t2-purge-title t2-purge-warn",
                    value: "{typed}",
                    disabled: deleting,
                    oninput: move |e| on_input.call(e.value()),
                    onmounted: move |e: Event<MountedData>| {
                        let d = e.data();
                        spawn(async move {
                            let _ = d.set_focus(true).await;
                        });
                    },
                }
            }
            ActionError { message: error }
            if deleting {
                div { role: "status", style: META, "Deleting…" }
            }
            div { style: "display:flex;gap:8px;flex-wrap:wrap;",
                button {
                    class: "btn btn--danger btn--sm",
                    r#type: "button",
                    disabled: !confirm_ok || deleting,
                    title: delete_title,
                    onclick: move |_| on_confirm.call(()),
                    if deleting {
                        "Deleting…"
                    } else {
                        "Delete permanently"
                    }
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    r#type: "button",
                    disabled: deleting,
                    onclick: move |_| on_cancel.call(()),
                    "Cancel Purge"
                }
            }
        }
    }
}

#[component]
fn StagedRow(row: Tier2ClaimRow, busy: bool, on_undo: EventHandler<()>) -> Element {
    let tone = confidence_tone(row.confidence);
    let pct = format_confidence(row.confidence);
    let label = format!("Restore claim: {}", clip_chars(&row.text, 60));
    rsx! {
        div { class: "mem-entry",
            div { class: "mem-ts",
                span { class: "pill", class: if !tone.is_empty() { "amber" }, "{pct}" }
            }
            div { class: "mem-body",
                div { style: "overflow-wrap:anywhere;", "{row.text}" }
                div {
                    title: "{row.source}",
                    style: "{META}overflow:hidden;text-overflow:ellipsis;white-space:nowrap;",
                    "{format_source(&row.source)}"
                }
                div { style: "display:flex;gap:4px;margin-top:8px;",
                    button {
                        class: "btn btn--ghost btn--sm",
                        r#type: "button",
                        "aria-label": "{label}",
                        disabled: busy,
                        onclick: move |_| on_undo.call(()),
                        if busy {
                            "Restoring…"
                        } else {
                            "Undo"
                        }
                    }
                }
            }
        }
    }
}

// ── The panel ───────────────────────────────────────────────────────────────

#[component]
pub fn Tier2Panel() -> Element {
    let mut query = use_signal(String::new);
    let mut submitted = use_signal(String::new);
    let mut refresh_tick = use_signal(|| 0u32);
    let mut selected: Signal<Option<String>> = use_signal(|| None);
    let mut busy: Signal<Option<String>> = use_signal(|| None);
    let mut action_error: Signal<Option<String>> = use_signal(|| None);
    let mut status_note: Signal<Option<String>> = use_signal(|| None);
    let mut return_focus: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let mut search_el: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let mut status_caution = use_signal(|| false);
    // Purge now (UI-SPEC section 10): the state machine, the typed word and its feedback.
    let mut purge_view = use_signal(|| PurgeView::Idle);
    let mut purge_typed = use_signal(String::new);
    let mut purge_error: Signal<Option<String>> = use_signal(|| None);
    let mut preview_error: Signal<Option<String>> = use_signal(|| None);
    let mut input_gen = use_signal(|| 0u32);
    let mut focus_entry = use_signal(|| false);
    let mut purge_group_el: Signal<Option<Rc<MountedData>>> = use_signal(|| None);

    // `use_resource` with a tick, never `use_server_future` + `.restart()` (positional-hook crash).
    let status = use_resource(move || {
        let _tick = refresh_tick();
        async move { tier2_status().await }
    });
    // Reads `submitted` and the tick, never `query`: typing must not refetch (a semantic query
    // may call a remote embedding endpoint, UI-SPEC G1).
    let rows_res = use_resource(move || {
        let _tick = refresh_tick();
        let q = submitted();
        async move {
            if q.is_empty() {
                tier2_list(RECENT_LIMIT).await
            } else {
                tier2_search(q, SEARCH_LIMIT).await
            }
        }
    });
    // The Staged panel renders independently of the search, so a search never hides it.
    let staged = use_resource(move || {
        let _tick = refresh_tick();
        async move { tier2_list_staged().await }
    });
    // Reads `selected` and the tick; no server call while nothing is selected.
    let explain = use_resource(move || {
        let _tick = refresh_tick();
        let id = selected();
        async move {
            match id {
                None => Ok(None),
                Some(id) => tier2_explain(id).await,
            }
        }
    });

    let status_snap = status();
    let rows_snap = rows_res();
    let staged_snap = staged();
    let explain_snap =
        explain().map(|r| r.map_err(|e| extract_memory_error_message(&e.to_string())));
    let query_val = query();
    let submitted_val = submitted();
    let selected_val = selected();
    let busy_val = busy();
    let error_val = action_error();
    let note_val = status_note();
    let caution_val = status_caution() && note_val.is_some();
    let purge_view_val = purge_view();
    let purge_typed_val = purge_typed();
    let purge_error_val = purge_error();
    let preview_error_val = preview_error();
    let input_gen_val = input_gen();
    let focus_entry_val = focus_entry();
    // The left-column Forget is locked while the confirm view is open or a purge runs (R7).
    let writes_locked = matches!(
        purge_view_val,
        PurgeView::Confirming(_) | PurgeView::Deleting(_)
    ) || busy_val.as_deref() == Some("purge");

    let mut forgetting_enabled = false;
    let mut writes_enabled = false;
    let mut purge_login = false;
    let notice = match &status_snap {
        None => Some(StatusNotice::Loading),
        Some(Err(_)) => Some(StatusNotice::LoadError),
        Some(Ok(s)) if !s.enabled => Some(StatusNotice::Off),
        Some(Ok(s)) => {
            forgetting_enabled = s.forgetting_enabled;
            writes_enabled = s.writes_enabled;
            purge_login = s.purge_login;
            None
        }
    };
    let (list_state, mode, mode_note) = match &rows_snap {
        None => (ListState::Loading, None, None),
        Some(Err(_)) => (ListState::LoadError, None, None),
        Some(Ok(v)) => (
            ListState::Rows(v.rows.clone()),
            (!v.mode.is_empty()).then(|| v.mode.clone()),
            v.note.clone(),
        ),
    };
    let count_display = match &list_state {
        ListState::Rows(r) => r.len().to_string(),
        _ => "—".to_string(),
    };
    let (staged_rows, staged_loading, staged_error) = match staged_snap {
        None => (Vec::new(), true, false),
        Some(Err(_)) => (Vec::new(), false, true),
        Some(Ok(v)) => (v, false, false),
    };
    let explain_view = explain_state(selected_val.as_deref(), explain_snap);

    let mut bump = move || {
        let next = refresh_tick.peek().wrapping_add(1);
        refresh_tick.set(next);
    };
    let on_retry = move |_| bump();
    let on_clear = move |_| {
        query.set(String::new());
        submitted.set(String::new());
        action_error.set(None);
        status_note.set(None);
        status_caution.set(false);
        let el = search_el.peek().clone();
        if let Some(el) = el {
            spawn(async move {
                let _ = el.set_focus(true).await;
            });
        }
    };
    let on_details = move |(row, mount): (Tier2ClaimRow, Option<Rc<MountedData>>)| {
        let already = selected.peek().as_deref() == Some(row.claim_id.as_str());
        selected.set(if already { None } else { Some(row.claim_id) });
        return_focus.set(mount);
    };
    let on_forget = move |row: Tier2ClaimRow| {
        // One Tier 2 write at a time (UI-SPEC G2), the `row_busy` discipline of memory.rs.
        if busy.peek().is_some() {
            return;
        }
        busy.set(Some(row.claim_id.clone()));
        action_error.set(None);
        status_note.set(None);
        status_caution.set(false);
        spawn(async move {
            match tier2_stage_forget(row.claim_id.clone()).await {
                Ok(_) => {
                    busy.set(None);
                    // The drawer would point at a claim explain now reports as gone (G5).
                    if selected.peek().as_deref() == Some(row.claim_id.as_str()) {
                        selected.set(None);
                    }
                    status_note.set(Some(format!(
                        "Staged to forget: \"{}\"",
                        clip_chars(&row.text, 40)
                    )));
                    bump();
                }
                Err(e) => {
                    busy.set(None);
                    action_error.set(Some(format!(
                        "Could not stage this claim: {}",
                        extract_memory_error_message(&e.to_string())
                    )));
                }
            }
        });
    };
    let on_undo = move |row: Tier2ClaimRow| {
        if busy.peek().is_some() {
            return;
        }
        busy.set(Some(row.claim_id.clone()));
        action_error.set(None);
        status_note.set(None);
        status_caution.set(false);
        spawn(async move {
            match tier2_unstage_forget(row.claim_id.clone()).await {
                Ok(_) => {
                    busy.set(None);
                    status_note.set(Some(format!("Restored: \"{}\"", clip_chars(&row.text, 40))));
                    bump();
                }
                Err(e) => {
                    // With forgetting off the service refuses; its flag message shows verbatim.
                    busy.set(None);
                    action_error.set(Some(format!(
                        "Could not restore this claim: {}",
                        extract_memory_error_message(&e.to_string())
                    )));
                }
            }
        });
    };

    // The only caller of the purge preview (read-only).
    let on_purge_open = move |_: ()| {
        if busy.peek().is_some() || *purge_view.peek() != PurgeView::Idle {
            return;
        }
        purge_view.set(PurgeView::Previewing);
        purge_typed.set(String::new());
        purge_error.set(None);
        preview_error.set(None);
        action_error.set(None);
        status_note.set(None);
        status_caution.set(false);
        focus_entry.set(false);
        spawn(async move {
            match tier2_purge_preview().await {
                Ok(v) => {
                    if let Some(why) = v.skipped {
                        preview_error.set(Some(format!("Could not load the purge preview: {why}")));
                        purge_view.set(PurgeView::Idle);
                    } else if v.rows.is_empty() {
                        purge_view.set(PurgeView::Idle);
                        status_note.set(Some(NOTHING_STAGED.to_string()));
                        bump();
                    } else {
                        purge_view.set(PurgeView::Confirming(v));
                    }
                }
                Err(e) => {
                    preview_error.set(Some(format!(
                        "Could not load the purge preview: {}",
                        extract_memory_error_message(&e.to_string())
                    )));
                    purge_view.set(PurgeView::Idle);
                }
            }
        });
    };
    // The only caller of the purge itself: it sends the typed word and ONLY the previewed ids.
    let on_purge_confirm = move |_: ()| {
        let PurgeView::Confirming(previewed) = purge_view.peek().clone() else {
            return;
        };
        let typed = purge_typed.peek().clone();
        if !purge_confirm_enabled(&typed) || busy.peek().is_some() {
            return;
        }
        let ids: Vec<String> = previewed.rows.iter().map(|r| r.claim_id.clone()).collect();
        let nonce = previewed.nonce.clone().unwrap_or_default();
        busy.set(Some("purge".to_string()));
        purge_view.set(PurgeView::Deleting(previewed.clone()));
        purge_error.set(None);
        action_error.set(None);
        let group = purge_group_el.peek().clone();
        if let Some(g) = group {
            spawn(async move {
                let _ = g.set_focus(true).await;
            });
        }
        spawn(async move {
            let sent = ids.len();
            let result = tier2_purge_now(typed, nonce, ids).await;
            busy.set(None);
            let refused = match result {
                Err(e) => purge_error_line(&extract_memory_error_message(&e.to_string()), false),
                Ok(r) => match r.skipped.as_deref() {
                    Some(why) => purge_error_line(why, true),
                    None => {
                        let (note, caution) = purge_result_note(sent, &r);
                        purge_view.set(PurgeView::Idle);
                        purge_typed.set(String::new());
                        selected.set(None);
                        status_caution.set(caution);
                        status_note.set(Some(note));
                        bump();
                        return;
                    }
                },
            };
            // Refused or failed: stay open, keep the typed word, refocus the input.
            purge_error.set(Some(refused));
            purge_view.set(PurgeView::Confirming(previewed));
            let next = input_gen.peek().wrapping_add(1);
            input_gen.set(next);
        });
    };
    let on_purge_cancel = move |_: ()| {
        if matches!(*purge_view.peek(), PurgeView::Deleting(_)) {
            return;
        }
        purge_view.set(PurgeView::Idle);
        purge_typed.set(String::new());
        purge_error.set(None);
        focus_entry.set(true);
    };
    // Close/Esc: clear the selection and return focus to the claim's Details button.
    let mut on_close = move |_: ()| {
        selected.set(None);
        let el = return_focus.peek().clone();
        if let Some(el) = el {
            spawn(async move {
                let _ = el.set_focus(true).await;
            });
        }
    };

    rsx! {
        if let Some(kind) = notice {
            Tier2StatusNotice { kind, on_retry }
        } else {
            div { class: "split left-wider",
                div { class: "panel",
                    div { class: "panel-title",
                        "Claims "
                        span { class: "count", style: "color:var(--teal)", "· {count_display}" }
                    }
                    StatusNote { note: note_val, caution: caution_val }
                    SearchForm {
                        query: query_val,
                        searching: !submitted_val.is_empty(),
                        on_input: move |v| query.set(v),
                        on_submit: move |_| {
                            let q = commit_query(&query.peek());
                            submitted.set(q);
                            action_error.set(None);
                            status_note.set(None);
        status_caution.set(false);
                        },
                        on_clear,
                        on_mounted: move |el| search_el.set(Some(el)),
                    }
                    ModeLine {
                        submitted: submitted_val.clone(),
                        mode,
                        note: mode_note,
                    }
                    ActionError { message: error_val }
                    ClaimsListBody {
                        state: list_state,
                        submitted: submitted_val.clone(),
                        selected: selected_val.clone(),
                        busy_id: busy_val.clone(),
                        forgetting_enabled,
                        writes_locked,
                        on_details,
                        on_forget,
                        on_clear,
                        on_retry,
                    }
                }
                div { style: "display:flex;flex-direction:column;gap:16px;",
                    // Docked panel, always rendered so the layout does not jump; no overlay or trap.
                    div {
                        class: "panel",
                        id: "t2-explain",
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Escape {
                                on_close(());
                            }
                        },
                        div { class: "panel-title", "Why this claim" }
                        ExplainBody { state: explain_view, on_close, on_retry }
                    }
                    StagedPanelView {
                        rows: staged_rows,
                        loading: staged_loading,
                        load_error: staged_error,
                        busy_id: busy_val,
                        forgetting_enabled,
                        writes_enabled,
                        purge_login,
                        purge_view: purge_view_val,
                        typed: purge_typed_val,
                        purge_error: purge_error_val,
                        preview_error: preview_error_val,
                        input_gen: input_gen_val,
                        focus_entry: focus_entry_val,
                        on_undo,
                        on_retry,
                        on_purge_open,
                        on_purge_input: move |v| purge_typed.set(v),
                        on_purge_confirm,
                        on_purge_cancel,
                        on_group_mounted: move |el| purge_group_el.set(Some(el)),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tier2_api::{Tier2EvidenceRow, Tier2PurgePreviewRow};
    use std::cell::RefCell;

    const SRC: &str = include_str!("memory_tier2.rs");
    const SCREENS_CSS: &str = include_str!("../../../../assets/screens.css");

    fn code_of(src: &str) -> &str {
        &src[..src.find("#[cfg(test)]").unwrap_or(src.len())]
    }

    /// SSR-render whatever `f` builds (the `defcon_chip.rs` harness, generalised so one test can
    /// render several prop sets). Quote entities are decoded so assertions read like the copy.
    fn render(f: impl Fn() -> Element + 'static) -> String {
        thread_local! {
            static F: RefCell<Option<Rc<dyn Fn() -> Element>>> = const { RefCell::new(None) };
        }
        F.with(|c| *c.borrow_mut() = Some(Rc::new(f)));
        fn root() -> Element {
            let f = F.with(|c| c.borrow().clone()).expect("render fn set");
            f()
        }
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
            .replace("&quot;", "\"")
            .replace("&#34;", "\"")
            .replace("&#x27;", "'")
            .replace("&#39;", "'")
    }

    fn row(text: &str, confidence: f64) -> Tier2ClaimRow {
        Tier2ClaimRow {
            claim_id: "claim:0123456789ab".to_string(),
            text: text.to_string(),
            confidence,
            source: "memory://live/2026-11-08.md#L3".to_string(),
            created_at: "2026-11-08T14:03:22Z".to_string(),
        }
    }

    #[test]
    fn tier2_panel_view_model_formats_rows() {
        assert_eq!(format_confidence(0.95), "95%");
        assert_eq!(format_confidence(0.7), "70%");
        assert_eq!(format_confidence(0.954), "95%");
        assert_eq!(confidence_tone(0.8), "");
        assert_eq!(confidence_tone(0.79), "amber");
        assert_eq!(
            format_source("memory://live/2026-11-08.md#L3"),
            "live/2026-11-08.md:3"
        );
        assert_eq!(format_source("weird"), "weird");
        assert_eq!(format_source(""), "");
    }

    #[test]
    fn tier2_panel_copy_helpers() {
        let long = "a".repeat(45);
        assert_eq!(clip_chars(&long, 40), format!("{}…", "a".repeat(40)));
        assert_eq!(clip_chars("short text", 40), "short text");
        assert_eq!(
            empty_state(""),
            (
                "No claims yet".to_string(),
                "Claims appear here as memory is indexed and consolidated.".to_string()
            )
        );
        assert_eq!(
            empty_state("cats"),
            (
                "No claims match \"cats\"".to_string(),
                "Try different words, or clear the search to see recent claims.".to_string()
            )
        );
        assert_eq!(
            cap_line(20, true).as_deref(),
            Some("Showing the first 20. Narrow your search to see others.")
        );
        assert_eq!(
            cap_line(50, false).as_deref(),
            Some("Showing the 50 most recent claims.")
        );
        assert_eq!(cap_line(3, true), None);
        assert_eq!(commit_query("  cats  "), "cats");
        assert_eq!(commit_query("   "), "");
        assert_eq!(commit_query(&"x".repeat(600)), "x".repeat(500));
    }

    #[test]
    fn tier2_panel_actions_are_forget_undo_and_purge() {
        assert_eq!(row_actions(false), ["Forget"]);
        assert_eq!(row_actions(true), ["Undo"]);
        // No per-row purge (UI-SPEC section 10): `Purge now…` is the panel's only danger button.
        let html = staged_html(vec![row("A staged claim", 0.9)], false, false, None, true);
        assert_eq!(html.matches("btn--danger").count(), 1, "{html}");
        assert!(html.contains(">Purge now…<"), "{html}");
        assert!(!html.contains("Delete permanently") && !html.contains("Cancel Purge"));
    }

    #[test]
    fn tier2_claim_row_render() {
        let html = render(|| {
            rsx! {
                ClaimRowView {
                    row: row("User prefers concise replies", 0.95),
                    selected: false,
                    busy: false,
                    forgetting_enabled: true,
                    writes_locked: false,
                    on_details: move |_| {},
                    on_forget: move |_| {},
                }
            }
        });
        assert!(html.contains(">95%<"), "{html}");
        assert!(html.contains("User prefers concise replies"), "{html}");
        assert!(html.contains("live/2026-11-08.md:3"), "{html}");
        assert!(
            html.contains("title=\"memory://live/2026-11-08.md#L3\""),
            "{html}"
        );
        assert!(
            html.contains(">Details<") && html.contains(">Forget<"),
            "{html}"
        );
        assert!(
            html.contains("aria-label=\"Show evidence for claim: User prefers concise replies\""),
            "{html}"
        );
        assert!(
            html.contains("aria-label=\"Forget claim: User prefers concise replies\""),
            "{html}"
        );
        assert!(html.contains("aria-controls=\"t2-explain\""), "{html}");
        assert!(html.contains("aria-expanded=\"false\""), "{html}");
        assert!(html.contains("type=\"button\""), "{html}");
        assert!(!html.contains("amber"), "0.95 is neutral: {html}");
        assert!(!html.contains("disabled"), "{html}");
        assert!(!html.contains("var(--teal)"), "{html}");

        let long = "w".repeat(80);
        let html = render(move || {
            let text = "w".repeat(80);
            rsx! {
                ClaimRowView {
                    row: row(&text, 0.7),
                    selected: true,
                    busy: true,
                    forgetting_enabled: true,
                    writes_locked: false,
                    on_details: move |_| {},
                    on_forget: move |_| {},
                }
            }
        });
        let clipped = format!("{}…", "w".repeat(60));
        assert!(
            html.contains(&format!("aria-label=\"Forget claim: {clipped}\"")),
            "{html}"
        );
        assert!(
            html.contains(&long),
            "claim text is never truncated: {html}"
        );
        assert!(html.contains("aria-expanded=\"true\""), "{html}");
        assert!(html.contains("border-left:2px solid var(--teal)"), "{html}");
        assert!(html.contains("Staging…"), "{html}");
        assert!(html.contains("disabled"), "{html}");
        assert!(html.contains("pill amber"), "0.7 is amber: {html}");

        let html = render(|| {
            rsx! {
                ClaimRowView {
                    row: row("x", 0.95),
                    selected: false,
                    busy: false,
                    forgetting_enabled: false,
                    writes_locked: false,
                    on_details: move |_| {},
                    on_forget: move |_| {},
                }
            }
        });
        assert!(html.contains(FORGET_OFF_TITLE), "{html}");
        assert!(html.contains("disabled"), "{html}");
    }

    #[test]
    fn tier2_panel_write_calls_only_stage_unstage_and_purge() {
        let code = code_of(SRC);
        assert!(code.contains("tier2_stage_forget("));
        assert!(code.contains("tier2_unstage_forget("));
        // The purge calls live only in their own handlers.
        let open = code.find("let on_purge_open").unwrap();
        let confirm = code.find("let on_purge_confirm").unwrap();
        let cancel = code.find("let on_purge_cancel").unwrap();
        assert_eq!(code.matches("tier2_purge_now(").count(), 1);
        assert_eq!(code.matches("tier2_purge_preview(").count(), 1);
        assert!(code[confirm..cancel].contains("tier2_purge_now("));
        assert!(code[open..confirm].contains("tier2_purge_preview("));
        // Every forget/purge server fn the file calls is one of the allowed four.
        for (i, _) in code.match_indices("tier2_") {
            let call = &code[i..];
            let name: String = call
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if call[name.len()..].starts_with('(')
                && (name.contains("forget") || name.contains("purge"))
            {
                assert!(
                    [
                        "tier2_stage_forget",
                        "tier2_unstage_forget",
                        "tier2_purge_now",
                        "tier2_purge_preview"
                    ]
                    .contains(&name.as_str()),
                    "unexpected write call {name}"
                );
            }
        }
        for banned in [
            "add_memory_entry",
            "replace_memory_entry",
            "remove_memory_entry",
        ] {
            assert!(!code.contains(banned), "{banned} must not be referenced");
        }
    }

    #[test]
    fn tier2_search_focus_ring_css() {
        assert!(SCREENS_CSS.contains(
            ".t2-search:focus-visible { outline: 2px solid var(--teal); outline-offset: 2px; }"
        ));
    }

    #[test]
    fn tier2_status_notice_render() {
        let html = render(|| {
            rsx! {
                Tier2StatusNotice { kind: StatusNotice::Loading, on_retry: move |_| {} }
            }
        });
        assert!(html.contains("Loading claims…"), "{html}");
        let html = render(|| {
            rsx! {
                Tier2StatusNotice { kind: StatusNotice::LoadError, on_retry: move |_| {} }
            }
        });
        assert!(
            html.contains(LOAD_ERROR) && html.contains(">Retry<"),
            "{html}"
        );
        let html = render(|| {
            rsx! {
                Tier2StatusNotice { kind: StatusNotice::Off, on_retry: move |_| {} }
            }
        });
        assert!(html.contains("Tier 2 memory is off"), "{html}");
        assert!(
            html.contains("Set memory.tier2_enabled to true in config.yaml, then restart the server. Tier 1 memory is unaffected."),
            "{html}"
        );
    }

    #[test]
    fn tier2_search_form_render() {
        let html = render(|| {
            rsx! {
                SearchForm {
                    query: String::new(),
                    searching: false,
                    on_input: move |_| {},
                    on_submit: move |_| {},
                    on_clear: move |_| {},
                    on_mounted: move |_| {},
                }
            }
        });
        assert!(html.contains("class=\"field-input t2-search\""), "{html}");
        assert!(html.contains("type=\"search\""), "{html}");
        assert!(html.contains("maxlength=\"500\""), "{html}");
        assert!(
            html.contains("placeholder=\"Search long-term memory…\""),
            "{html}"
        );
        assert!(
            html.contains("aria-label=\"Search Tier 2 claims\""),
            "{html}"
        );
        assert!(
            html.contains("class=\"btn btn--sm\"") && html.contains("type=\"submit\""),
            "{html}"
        );
        assert!(!html.contains(">Clear<"), "{html}");
        let html = render(|| {
            rsx! {
                SearchForm {
                    query: "cats".to_string(),
                    searching: true,
                    on_input: move |_| {},
                    on_submit: move |_| {},
                    on_clear: move |_| {},
                    on_mounted: move |_| {},
                }
            }
        });
        assert!(
            html.contains("class=\"btn btn--ghost btn--sm\"") && html.contains(">Clear<"),
            "{html}"
        );
    }

    #[test]
    fn tier2_mode_line_render() {
        let html = render(|| {
            rsx! {
                ModeLine { submitted: String::new(), mode: None, note: None }
            }
        });
        assert!(
            html.contains("Recent claims") && !html.contains("pill"),
            "{html}"
        );
        let html = render(|| {
            rsx! {
                ModeLine { submitted: "cats".to_string(), mode: Some("semantic".to_string()), note: None }
            }
        });
        assert!(html.contains("Results for \"cats\""), "{html}");
        assert!(
            html.contains("class=\"pill\"")
                && html.contains(">SEMANTIC<")
                && !html.contains("amber"),
            "{html}"
        );
        let note = "semantic search unavailable (no embedder); showing keyword matches";
        let html = render(move || {
            rsx! {
                ModeLine {
                    submitted: "x".repeat(45),
                    mode: Some("keyword".to_string()),
                    note: Some(note.to_string()),
                }
            }
        });
        assert!(
            html.contains("pill amber") && html.contains(">KEYWORD<"),
            "{html}"
        );
        assert!(
            html.contains(note) && html.contains("color:var(--amber)"),
            "{html}"
        );
        assert!(
            html.contains(&format!("Results for \"{}…\"", "x".repeat(40))),
            "{html}"
        );
    }

    #[test]
    fn tier2_feedback_lines_render() {
        let html = render(|| rsx! { StatusNote { note: Some("Restored: \"abc\"".to_string()) } });
        assert!(html.contains("role=\"status\""), "{html}");
        assert!(html.contains("aria-live=\"polite\""), "{html}");
        assert!(html.contains("tabindex=\"-1\""), "{html}");
        assert!(html.contains("Restored: \"abc\""), "{html}");
        let msg = "Could not stage this claim: Memory edits are disabled while web config writes are off.";
        let html = render(move || rsx! { ActionError { message: Some(msg.to_string()) } });
        assert!(
            html.contains("role=\"alert\"") && html.contains("var(--danger)"),
            "{html}"
        );
        assert!(html.contains(msg), "{html}");
        let html = render(|| rsx! { StatusNote { note: None } ActionError { message: None } });
        assert!(!html.contains("role="), "{html}");
    }

    #[test]
    fn tier2_claims_list_body_render() {
        let body = |state: ListState, submitted: &'static str| {
            render(move || {
                let state = state.clone();
                rsx! {
                    ClaimsListBody {
                        state,
                        submitted: submitted.to_string(),
                        selected: None,
                        busy_id: None,
                        forgetting_enabled: true,
                        writes_locked: false,
                        on_details: move |_| {},
                        on_forget: move |_| {},
                        on_clear: move |_| {},
                        on_retry: move |_| {},
                    }
                }
            })
        };
        assert!(body(ListState::Loading, "").contains("Loading claims…"));
        let html = body(ListState::LoadError, "");
        assert!(
            html.contains(LOAD_ERROR) && html.contains(">Retry<"),
            "{html}"
        );
        let html = body(ListState::Rows(vec![]), "");
        assert!(html.contains("No claims yet"), "{html}");
        assert!(
            html.contains("Claims appear here as memory is indexed and consolidated."),
            "{html}"
        );
        assert!(!html.contains(">Clear<"), "{html}");
        let html = body(ListState::Rows(vec![]), "cats");
        assert!(html.contains("No claims match \"cats\""), "{html}");
        assert!(
            html.contains("Try different words, or clear the search to see recent claims."),
            "{html}"
        );
        assert!(html.contains(">Clear<"), "{html}");
        let many: Vec<Tier2ClaimRow> = (0..20)
            .map(|i| Tier2ClaimRow {
                claim_id: format!("claim:{i:012x}"),
                ..row("t", 0.9)
            })
            .collect();
        let html = body(ListState::Rows(many), "cats");
        assert_eq!(html.matches(">Forget<").count(), 20, "{html}");
        assert!(
            html.contains("Showing the first 20. Narrow your search to see others."),
            "{html}"
        );
        let few: Vec<Tier2ClaimRow> = (0..3)
            .map(|i| Tier2ClaimRow {
                claim_id: format!("claim:{i:012x}"),
                ..row("t", 0.9)
            })
            .collect();
        let html = body(ListState::Rows(few), "cats");
        assert!(!html.contains("Showing the"), "{html}");
    }

    /// Smoke: the full panel's first render (resources pending, so the notice branch) declares
    /// every hook and renders without a panic. Resource futures are not polled by SSR.
    #[test]
    fn tier2_panel_first_render_is_loading_notice() {
        let html = render(|| rsx! { Tier2Panel {} });
        assert!(html.contains("Loading claims…"), "{html}");
    }

    fn view(claim_id: &str, n_evidence: usize, supersedes: Vec<String>) -> Tier2ExplainView {
        Tier2ExplainView {
            claim_id: claim_id.to_string(),
            text: "User prefers concise replies".to_string(),
            confidence: 0.95,
            evidence: (0..n_evidence)
                .map(|i| Tier2EvidenceRow {
                    source: "memory://live/2026-11-08.md#L3".to_string(),
                    quote: format!("quote {i}"),
                    timestamp: "2026-11-08T14:03:22.123Z".to_string(),
                })
                .collect(),
            supersedes,
        }
    }

    fn explain_html(state: ExplainState) -> String {
        render(move || {
            let state = state.clone();
            rsx! {
                ExplainBody { state, on_close: move |_| {}, on_retry: move |_| {} }
            }
        })
    }

    #[test]
    fn tier2_format_timestamp() {
        assert_eq!(
            format_timestamp("2026-11-08T14:03:22.123Z"),
            "2026-11-08 14:03"
        );
        assert_eq!(
            format_timestamp("2026-11-08T14:03:22+00:00"),
            "2026-11-08 14:03"
        );
        assert_eq!(format_timestamp("garbage"), "garbage");
        assert_eq!(format_timestamp(""), "");
        assert_eq!(
            format_timestamp("2026-11-08 14:03:22"),
            "2026-11-08 14:03:22"
        );
    }

    #[test]
    fn tier2_explain_state_mapping() {
        let x = "claim:0123456789ab";
        assert_eq!(explain_state(None, None), ExplainState::Prompt);
        assert_eq!(explain_state(Some(x), None), ExplainState::Loading);
        assert_eq!(
            explain_state(
                Some(x),
                Some(Ok(Some(view("claim:ffffffffffff", 0, vec![]))))
            ),
            ExplainState::Loading,
            "a stale load for another claim is not shown"
        );
        let v = view(x, 1, vec![]);
        assert_eq!(
            explain_state(Some(x), Some(Ok(Some(v.clone())))),
            ExplainState::Loaded(v)
        );
        assert_eq!(
            explain_state(Some(x), Some(Ok(None))),
            ExplainState::NotFound
        );
        assert_eq!(
            explain_state(Some(x), Some(Err("boom".to_string()))),
            ExplainState::Error("boom".to_string())
        );
    }

    #[test]
    fn tier2_explain_panel_states_render() {
        let html = explain_html(ExplainState::Prompt);
        assert!(
            html.contains("Select Details on a claim to see its evidence and what it replaced."),
            "{html}"
        );
        assert!(explain_html(ExplainState::Loading).contains("Loading evidence…"));
        let html = explain_html(ExplainState::NotFound);
        assert!(
            html.contains(
                "This claim is no longer available. It may have been forgotten or replaced."
            ),
            "{html}"
        );
        assert!(html.contains(">Close<"), "{html}");
        let html = explain_html(ExplainState::Error("timeout".to_string()));
        assert!(
            html.contains("Could not load this claim's evidence: timeout."),
            "{html}"
        );
        assert!(html.contains(">Retry<"), "{html}");

        let html = explain_html(ExplainState::Loaded(view("claim:0123456789ab", 2, vec![])));
        assert!(html.contains("User prefers concise replies"), "{html}");
        assert!(html.contains(">95%<") && html.contains(">Close<"), "{html}");
        assert!(html.contains("EVIDENCE · 2"), "{html}");
        assert_eq!(
            html.matches("live/2026-11-08.md:3 · 2026-11-08 14:03")
                .count(),
            2,
            "{html}"
        );
        assert!(html.contains("REPLACES"), "{html}");
        assert!(html.contains("Replaces no earlier claims."), "{html}");
        assert!(html.contains("max-height:160px"), "{html}");

        let html = explain_html(ExplainState::Loaded(view(
            "claim:0123456789ab",
            0,
            vec![
                "claim:aaaaaaaaaaaa".to_string(),
                "claim:bbbbbbbbbbbb".to_string(),
            ],
        )));
        assert!(html.contains("EVIDENCE · 0"), "{html}");
        assert!(
            html.contains("No evidence is recorded for this claim."),
            "{html}"
        );
        assert!(
            html.contains("claim:aaaaaaaaaaaa") && html.contains("claim:bbbbbbbbbbbb"),
            "{html}"
        );
        assert!(!html.contains("Replaces no earlier claims."), "{html}");
    }

    #[test]
    fn tier2_explain_quote_is_escaped_text() {
        let mut v = view("claim:0123456789ab", 1, vec![]);
        v.evidence[0].quote = "<b>x</b>".to_string();
        let html = explain_html(ExplainState::Loaded(v));
        // The SSR escaper emits numeric entities (`&#60;`); named ones would also be fine.
        assert!(
            html.contains("&#60;b&#62;") || html.contains("&lt;b&gt;"),
            "{html}"
        );
        assert!(!html.contains("<b>"), "{html}");
    }

    fn staged_html(
        rows: Vec<Tier2ClaimRow>,
        loading: bool,
        load_error: bool,
        busy_id: Option<&'static str>,
        forgetting_enabled: bool,
    ) -> String {
        staged_with(
            rows,
            loading,
            load_error,
            busy_id,
            forgetting_enabled,
            true,
            PurgeView::Idle,
        )
    }

    fn staged_with(
        rows: Vec<Tier2ClaimRow>,
        loading: bool,
        load_error: bool,
        busy_id: Option<&'static str>,
        forgetting_enabled: bool,
        writes_enabled: bool,
        purge_view: PurgeView,
    ) -> String {
        render(move || {
            let rows = rows.clone();
            let purge_view = purge_view.clone();
            rsx! {
                StagedPanelView {
                    rows,
                    loading,
                    load_error,
                    busy_id: busy_id.map(str::to_string),
                    forgetting_enabled,
                    writes_enabled,
                    purge_login: true,
                    purge_view,
                    typed: String::new(),
                    purge_error: None,
                    preview_error: None,
                    input_gen: 0u32,
                    focus_entry: false,
                    on_undo: move |_| {},
                    on_retry: move |_| {},
                    on_purge_open: move |_| {},
                    on_purge_input: move |_| {},
                    on_purge_confirm: move |_| {},
                    on_purge_cancel: move |_| {},
                    on_group_mounted: move |_| {},
                }
            }
        })
    }

    #[test]
    fn tier2_staged_panel_render() {
        let html = staged_html(vec![], false, false, None, true);
        assert!(html.contains("Staged to forget"), "{html}");
        assert!(html.contains("· 0"), "{html}");
        assert!(html.contains(staged_consequence(true)), "{html}");
        assert!(
            staged_consequence(true).contains("Nothing is deleted")
                && staged_consequence(true).starts_with("Hidden from search now."),
        );
        assert!(html.contains("Nothing is staged to forget."), "{html}");
        assert!(html.contains("color:var(--amber)"), "{html}");

        let html = staged_html(vec![], false, false, None, false);
        assert!(
            html.contains("Forgetting is off, so nothing is purged. Staged claims stay hidden until memory.forgetting_enabled is true."),
            "{html}"
        );
        assert!(!html.contains("The nightly purge"), "{html}");
        assert!(!html.contains("until a purge exists"), "{html}");

        // E5 backstop: empty source, forgetting off: Undo is still enabled.
        let mut r = row("A staged claim", 0.7);
        r.source = String::new();
        let html = staged_html(vec![r.clone()], false, false, None, false);
        assert!(html.contains(">Undo<"), "{html}");
        assert!(
            html.contains("aria-label=\"Restore claim: A staged claim\""),
            "{html}"
        );
        // Forgetting is off, so only `Purge now…` is disabled; `Undo` is not.
        assert_eq!(html.matches("disabled").count(), 1, "{html}");
        assert!(
            !html.contains(">Details<"),
            "staged rows have no Details: {html}"
        );
        assert!(html.contains("· 1"), "{html}");

        let html = staged_html(vec![r], false, false, Some("claim:0123456789ab"), true);
        assert!(
            html.contains("Restoring…") && html.contains("disabled"),
            "{html}"
        );

        assert!(staged_html(vec![], true, false, None, true).contains("Loading claims…"));
        let html = staged_html(vec![], false, true, None, true);
        assert!(
            html.contains(LOAD_ERROR) && html.contains(">Retry<"),
            "{html}"
        );
    }

    fn pv_row(text: &str, entries: &[&str], older: usize, live: usize) -> Tier2PurgePreviewRow {
        Tier2PurgePreviewRow {
            claim_id: format!("claim:{:012x}", text.len()),
            text: text.to_string(),
            predecessors: (0..older).map(|i| format!("claim:{i:012x}")).collect(),
            tier1_entries: entries.iter().map(|e| e.to_string()).collect(),
            live_lines: live,
        }
    }

    fn pv(rows: Vec<Tier2PurgePreviewRow>, redacted: usize) -> Tier2PurgePreviewView {
        Tier2PurgePreviewView {
            rows,
            redacted_quotes: redacted,
            skipped: None,
            nonce: None,
        }
    }

    fn confirm_html(view: Tier2PurgePreviewView, typed: &'static str, deleting: bool) -> String {
        render(move || {
            let view = view.clone();
            rsx! {
                PurgeConfirmView {
                    view,
                    typed: typed.to_string(),
                    deleting,
                    error: None,
                    input_gen: 0u32,
                    on_input: move |_| {},
                    on_confirm: move |_| {},
                    on_cancel: move |_| {},
                    on_group_mounted: move |_| {},
                }
            }
        })
    }

    #[test]
    fn tier2_staged_panel_offers_purge_now() {
        let one = vec![row("A staged claim", 0.9)];
        let enabled = staged_with(one.clone(), false, false, None, true, true, PurgeView::Idle);
        assert!(enabled.contains(">Purge now…<"), "{enabled}");
        assert!(!enabled.contains("disabled"), "{enabled}");
        let off = staged_with(
            one.clone(),
            false,
            false,
            None,
            false,
            false,
            PurgeView::Idle,
        );
        assert!(
            off.contains(FORGET_OFF_TITLE) && off.contains("disabled"),
            "{off}"
        );
        let writes = staged_with(one, false, false, None, true, false, PurgeView::Idle);
        assert!(
            writes.contains(WRITES_OFF_TITLE) && writes.contains("disabled"),
            "{writes}"
        );
        let none = staged_with(vec![], false, false, None, true, true, PurgeView::Idle);
        assert!(none.contains(">Purge now…<"), "{none}");
        assert!(
            none.contains("title=\"Nothing is staged to forget.\""),
            "{none}"
        );
        let loading = staged_with(
            vec![row("x", 0.9)],
            false,
            false,
            None,
            true,
            true,
            PurgeView::Previewing,
        );
        assert!(
            loading.contains(">Loading preview…<") && loading.contains("disabled"),
            "{loading}"
        );
        // The confirm view replaces the entry button and the list.
        let open = staged_with(
            vec![row("x", 0.9)],
            false,
            false,
            None,
            true,
            true,
            PurgeView::Confirming(pv(vec![pv_row("x", &[], 0, 0)], 0)),
        );
        assert!(
            !open.contains(">Purge now…<") && !open.contains(">Undo<"),
            "{open}"
        );
        assert!(open.contains("Permanently delete 1 claim?"), "{open}");

        assert_eq!(
            purge_entry_title(false, false, true, false),
            Some(FORGET_OFF_TITLE)
        );
        assert_eq!(
            purge_entry_title(true, false, true, false),
            Some(WRITES_OFF_TITLE)
        );
        assert_eq!(
            purge_entry_title(true, true, true, false),
            Some("Nothing is staged to forget.")
        );
        // D-28 amendment: login off disables Purge now up front, even with claims staged.
        assert_eq!(
            purge_entry_title(true, true, false, true),
            Some(PURGE_LOGIN_TITLE)
        );
        assert_eq!(purge_entry_title(true, true, true, true), None);
    }

    #[test]
    fn tier2_purge_confirm_view_render() {
        let view = pv(
            vec![
                pv_row(
                    "User lives in Lisbon",
                    &["MEMORY.md: User lives in Lisbon"],
                    1,
                    2,
                ),
                pv_row("Bare", &["USER.md: Bare entry"], 0, 0),
            ],
            3,
        );
        let html = confirm_html(view, "", false);
        assert!(html.contains("Permanently delete 2 claims?"), "{html}");
        assert!(html.contains("id=\"t2-purge-title\""), "{html}");
        assert!(
            html.contains("aria-labelledby=\"t2-purge-title\""),
            "{html}"
        );
        assert!(html.contains("MEMORY.md: User lives in Lisbon"), "{html}");
        assert!(html.contains("USER.md: Bare entry"), "{html}");
        assert!(html.contains("Also deletes 1 older wording"), "{html}");
        assert_eq!(html.matches("Also deletes").count(), 1, "{html}");
        assert!(html.contains("Clears 2 daily-note lines"), "{html}");
        assert_eq!(html.matches("Clears").count(), 1, "{html}");
        assert!(html.contains("Other claims' quotes to redact: 3"), "{html}");
        assert!(
            html.contains("Also removed wherever they are cited:"),
            "{html}"
        );
        assert!(html.contains("Not touched:"), "{html}");
        assert!(html.contains("This cannot be undone."), "{html}");
        assert!(html.contains("for=\"t2-purge-input\""), "{html}");
        assert!(html.contains(">Type purge to confirm<"), "{html}");
        assert!(
            html.contains("aria-label=\"Type purge to confirm\""),
            "{html}"
        );
        assert!(html.contains("maxlength=\"16\""), "{html}");
        assert!(html.contains("t2-purge-input"), "{html}");
        assert!(
            html.contains("aria-label=\"Claims to be deleted\""),
            "{html}"
        );
        assert!(
            html.contains("title=\"Type purge to enable this button.\""),
            "{html}"
        );
        assert!(
            html.contains(">Delete permanently<") && html.contains(">Cancel Purge<"),
            "{html}"
        );
        assert!(!html.contains("<form"), "Enter must not submit: {html}");
        assert!(!html.contains("aria-busy"), "{html}");

        let one = confirm_html(pv(vec![pv_row("x", &[], 0, 1)], 0), "purge", false);
        assert!(one.contains("Permanently delete 1 claim?"), "{one}");
        assert!(one.contains("Clears 1 daily-note line<"), "{one}");
        assert!(!one.contains("Type purge to enable this button."), "{one}");
        assert!(!one.contains("disabled"), "{one}");

        let busy = confirm_html(pv(vec![pv_row("x", &[], 0, 0)], 0), "purge", true);
        assert!(
            busy.contains("aria-busy=\"true\"") && busy.contains("role=\"status\""),
            "{busy}"
        );
        assert_eq!(busy.matches("Deleting…").count(), 2, "{busy}");
    }

    #[test]
    fn tier2_purge_confirm_enabled_exact() {
        assert!(purge_confirm_enabled("purge"));
        for bad in ["Purge", " purge", "purge ", "", "purge it"] {
            assert!(!purge_confirm_enabled(bad), "{bad:?}");
        }
    }

    #[test]
    fn tier2_purge_result_note_composition() {
        assert_eq!(count_noun(1, "claim", "claims"), "1 claim");
        assert_eq!(count_noun(2, "claim", "claims"), "2 claims");
        assert_eq!(count_noun(0, "claim", "claims"), "0 claims");
        let r = |purged, not_staged: Vec<&str>, scrub: Option<&str>| Tier2PurgeResultView {
            purged,
            tier1_lines_removed: 1,
            live_lines_retracted: 3,
            not_staged: not_staged.into_iter().map(str::to_string).collect(),
            skipped: None,
            scrub_incomplete: scrub.map(str::to_string),
        };
        let plain = "Purged 2 claims. Removed 1 line from MEMORY.md and USER.md and cleared 3 daily-note lines.";
        assert_eq!(
            purge_result_note(2, &r(2, vec![], None)),
            (plain.to_string(), false)
        );
        let (n, c) = purge_result_note(3, &r(2, vec!["claim:000000000001"], None));
        assert_eq!(
            n,
            format!("{plain} Some claims were cancelled since the preview and were kept.")
        );
        assert!(!c);
        let one = "Purged 1 claim. Removed 1 line from MEMORY.md and USER.md and cleared 3 daily-note lines.";
        let (n, _) = purge_result_note(3, &r(1, vec!["claim:000000000001"], None));
        assert_eq!(
            n,
            format!(
                "{one} Some claims were cancelled since the preview and were kept. 1 claim could not be purged and is still staged. Run Purge now again."
            )
        );
        let (n, _) = purge_result_note(4, &r(1, vec!["claim:000000000001"], None));
        assert!(
            n.ends_with(" 2 claims could not be purged and are still staged. Run Purge now again."),
            "{n}"
        );
        let (n, c) = purge_result_note(2, &r(2, vec![], Some("vacuum busy")));
        assert_eq!(
            n,
            format!(
                "{plain} On-disk cleanup incomplete: vacuum busy. Stop other IronHermes processes and see docs/MEMORY-TIER2.md for a full scrub."
            )
        );
        assert!(c);
        assert_eq!(purge_error_line("boom", false), "Could not purge: boom");
        assert_eq!(
            purge_error_line(
                "consolidation or dreams is running; try again in a minute",
                true
            ),
            "Could not purge: consolidation or dreams is running; try again in a minute. Nothing was deleted."
        );
    }

    #[test]
    fn tier2_status_note_caution_is_amber() {
        let html = render(|| rsx! { StatusNote { note: Some("n".to_string()), caution: true } });
        assert!(html.contains("var(--amber)"), "{html}");
        let html = render(|| rsx! { StatusNote { note: Some("n".to_string()) } });
        assert!(!html.contains("var(--amber)"), "{html}");
    }

    #[test]
    fn tier2_forget_disabled_while_writes_locked() {
        let html = render(|| {
            rsx! {
                ClaimsListBody {
                    state: ListState::Rows(vec![row("A claim", 0.9)]),
                    submitted: String::new(),
                    selected: None,
                    busy_id: None,
                    forgetting_enabled: true,
                    writes_locked: true,
                    on_details: move |_| {},
                    on_forget: move |_| {},
                    on_clear: move |_| {},
                    on_retry: move |_| {},
                }
            }
        });
        assert!(
            html.contains("title=\"Cancel Purge or finish it first.\""),
            "{html}"
        );
        assert_eq!(
            html.matches("disabled").count(),
            1,
            "only Forget is disabled: {html}"
        );
        assert!(html.contains(">Details<"), "{html}");
    }

    #[test]
    fn tier2_purge_preview_text_is_escaped() {
        let html = confirm_html(
            pv(
                vec![pv_row("<script>x</script>", &["MEMORY.md: <b>y</b>"], 0, 0)],
                0,
            ),
            "",
            false,
        );
        assert!(
            html.contains("&#60;script&#62;") || html.contains("&lt;script&gt;"),
            "{html}"
        );
        assert!(
            !html.contains("<script>") && !html.contains("<b>"),
            "{html}"
        );
    }

    #[test]
    fn tier2_purge_css_rules() {
        assert!(SCREENS_CSS.contains(
            ".t2-purge-input:focus-visible { outline: 2px solid var(--teal); outline-offset: 2px; }"
        ));
        assert!(SCREENS_CSS.contains(".btn--danger:disabled, .btn--danger:disabled:hover { background: transparent; color: var(--red); border-color: var(--red); box-shadow: none; opacity: 0.5; cursor: not-allowed; }"));
    }
}
