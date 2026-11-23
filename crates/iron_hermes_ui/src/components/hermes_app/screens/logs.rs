//! Phase 54.4 Plan 07 (D-08 / D-24): the Logs screen.
//!
//! Three sources behind `.tabs/.tab` buttons:
//!
//! - **SERVER** (`web.log`) and **AUDIT** (`audit.jsonl`) are file sources read
//!   through plan 04's bounded `read_log_tail` server fn. They poll every 3 s
//!   while this screen is mounted and not paused, with exactly one request in
//!   flight: the ticker only bumps `poll_tick` when the previous read has
//!   finished, and the resource restarts rather than stacking requests. The
//!   cursor returned by the server is PEEKED when the next request is built,
//!   so storing it never refetches and each poll fetches only new bytes.
//! - **AGENT ACTIVITY** reads the root `ActivityLogCtx` ring and never polls.
//!
//! The screen is global by default (D-08): it does not follow the header Agent
//! dropdown. Neither file source carries an agent (D-24), so the in-screen
//! agent filter is enabled on AGENT ACTIVITY only.
//!
//! Every log string is rendered as a Dioxus text node or attribute; this file
//! never injects HTML (T-54.4-23). Leaving the screen unmounts it and the
//! ticker `use_future` with it, so the poll stops.

use crate::activity_log::{ActivityLogCtx, ActivityRow};
use crate::components::hermes_app::agent_tabs::DEFAULT_AGENT;
use crate::components::hermes_app::profile_topbar::SharedProfilesCtx;
use crate::server::logs_api::{read_log_tail, AuditRowWire, LogCursor, LogLine, LogSource};
use dioxus::prelude::*;
use std::collections::HashSet;

/// Which source the Logs screen is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LogsTab {
    Server,
    Audit,
    Activity,
}

/// The client keeps at most this many rows per file source (UIC E12-overflow).
pub(crate) const LOGS_CLIENT_CAP: usize = 500;

/// Collapsed rows are cut to this many characters (UIC E12-long-text).
pub(crate) const ROW_DISPLAY_CHARS: usize = 240;

/// File-source poll cadence.
const POLL_INTERVAL_MS: u32 = 3000;

/// Tab order and labels.
const TABS: [(LogsTab, &str); 3] = [
    (LogsTab::Server, "SERVER"),
    (LogsTab::Audit, "AUDIT"),
    (LogsTab::Activity, "AGENT ACTIVITY"),
];

/// Apply one polled page to the client buffer: `reset` clears first, the new
/// lines are appended, and the oldest rows past `cap` are dropped.
pub(crate) fn apply_page(buf: &mut Vec<LogLine>, new: Vec<LogLine>, reset: bool, cap: usize) {
    if reset {
        buf.clear();
    }
    buf.extend(new);
    if buf.len() > cap {
        let excess = buf.len() - cap;
        buf.drain(..excess);
    }
}

/// `s` cut to `max` characters plus an ellipsis; shorter strings are
/// returned unchanged. Char-boundary safe.
pub(crate) fn display_cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((byte_idx, _)) => format!("{}…", &s[..byte_idx]),
        None => s.to_string(),
    }
}

/// Split a server line around its first level token so the token can be
/// coloured: `(before, token, after)`. No level (or a token not found in the
/// text) yields the whole line as `before`.
pub(crate) fn split_level<'a>(text: &'a str, level: Option<&str>) -> (&'a str, &'a str, &'a str) {
    if let Some(l) = level {
        if !l.is_empty() {
            if let Some(i) = text.find(l) {
                return (&text[..i], &text[i..i + l.len()], &text[i + l.len()..]);
            }
        }
    }
    (text, "", "")
}

/// The `audit.jsonl` kind of 54.3's aggregated yolo-bypass row.
const ROLLUP_KIND: &str = "yolo_bypass_rollup";

/// Whether an audit row is the yolo-bypass rollup (gets the amber badge).
pub(crate) fn is_rollup(kind: &str) -> bool {
    kind == ROLLUP_KIND
}

/// `HH:MM:SS` from an RFC3339 timestamp; anything shorter or malformed is
/// returned as-is (a bad row degrades to raw text, never panics).
fn hms_of(ts: &str) -> String {
    let b = ts.as_bytes();
    let looks_rfc3339 = b.len() >= 19
        && (b[10] == b'T' || b[10] == b' ')
        && b[13] == b':'
        && b[16] == b':'
        && ts.is_char_boundary(11)
        && ts.is_char_boundary(19);
    if looks_rfc3339 {
        ts[11..19].to_string()
    } else {
        ts.to_string()
    }
}

/// The collapsed one-line summary of an audit row:
/// `HH:MM:SS  {kind}  {tool}  {decision/resolution}  {session short}`.
pub(crate) fn audit_summary(row: &AuditRowWire) -> String {
    format!(
        "{}  {}  {}  {}/{}  {}",
        hms_of(&row.ts),
        row.kind,
        row.tool,
        row.decision,
        row.resolution,
        row.session.chars().take(8).collect::<String>()
    )
}

/// Expansion key of an audit row: stable across the 500-row cap, unlike an
/// index.
fn audit_key(row: &AuditRowWire) -> String {
    format!("{}|{}|{}|{}", row.ts, row.kind, row.tool, row.session)
}

/// The error copy shown when a poll fails (UIC E12-error).
pub(crate) fn fetch_error_copy(tab: LogsTab, reason: &str) -> String {
    let source = match tab {
        LogsTab::Server => "server",
        LogsTab::Audit => "audit",
        LogsTab::Activity => "activity",
    };
    format!("Couldn't read the {source} log: {reason}. Retrying every 3 seconds.")
}

/// What the file-source body shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BodyState {
    /// Before the first read resolves.
    Loading,
    /// Loaded, no rows, no error.
    Empty,
    /// Loaded with rows.
    Rows,
    /// The last read failed; earlier rows stay visible.
    ErrorWithRows,
    /// The last read failed and there is nothing to show.
    ErrorEmpty,
}

/// Precedence: not loaded -> Loading; then error (with or without rows);
/// then Empty vs Rows.
pub(crate) fn body_state(loaded: bool, error: Option<&str>, rows: usize) -> BodyState {
    if !loaded {
        return BodyState::Loading;
    }
    match (error.is_some(), rows) {
        (true, 0) => BodyState::ErrorEmpty,
        (true, _) => BodyState::ErrorWithRows,
        (false, 0) => BodyState::Empty,
        (false, _) => BodyState::Rows,
    }
}

/// Whether the scroll container `dom_id` is within `threshold` px of its
/// bottom. Off wasm there is no DOM, so it reports "at the bottom".
#[cfg(target_arch = "wasm32")]
pub(crate) fn is_near_bottom(dom_id: &str, threshold: i32) -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(dom_id))
        .map(|el| el.scroll_height() - (el.scroll_top() + el.client_height()) <= threshold)
        .unwrap_or(true)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn is_near_bottom(_dom_id: &str, _threshold: i32) -> bool {
    true
}

/// Scroll the container `dom_id` to its bottom. No-op off wasm.
#[cfg(target_arch = "wasm32")]
pub(crate) fn scroll_to_bottom(dom_id: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(dom_id))
    {
        el.set_scroll_top(el.scroll_height());
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn scroll_to_bottom(_dom_id: &str) {}

/// One AUDIT row: a one-line summary; clicking toggles its redacted `args`.
/// A standalone component so unchanged rows are skipped on re-render.
#[component]
fn AuditRow(row: AuditRowWire, expanded: Signal<HashSet<String>>) -> Element {
    let key = audit_key(&row);
    let open = expanded.read().contains(&key);
    let full = audit_summary(&row);
    let shown = display_cut(&full, ROW_DISPLAY_CHARS);
    let rollup = is_rollup(&row.kind);
    let args = row.args.clone();
    rsx! {
        div {
            class: "ih-log-row ih-log-row--audit",
            title: "{full}",
            "aria-expanded": "{open}",
            onclick: move |_| {
                expanded
                    .with_mut(|set| {
                        if !set.remove(&key) {
                            set.insert(key.clone());
                        }
                    });
            },
            "{shown}"
            if rollup {
                span { class: "ih-log-badge ih-log-badge--rollup", "ROLLUP" }
            }
        }
        if open {
            pre { class: "ih-log-args", "{args}" }
        }
    }
}

/// Whether the in-screen agent filter applies to `tab`. Only AGENT ACTIVITY
/// carries an agent (D-24): neither file source is attributed, and no
/// text-match heuristic invents one.
pub(crate) fn filter_enabled(tab: LogsTab) -> bool {
    tab == LogsTab::Activity
}

/// Keep only the rows of `agent` (all rows when `None`).
pub(crate) fn filter_activity(rows: &[ActivityRow], agent: Option<&str>) -> Vec<ActivityRow> {
    match agent {
        Some(a) => rows.iter().filter(|r| r.agent == a).cloned().collect(),
        None => rows.to_vec(),
    }
}

/// The Agent filter select. A standalone component, never an inline select
/// inside the screen's rsx (the Soul inline-dropdown revert 073d4e671).
#[component]
fn LogsAgentFilter(value: Signal<Option<String>>, enabled: bool) -> Element {
    let profiles = use_context::<SharedProfilesCtx>();
    let names: Vec<String> = match &*profiles.0.read() {
        Some(Ok(rows)) => rows
            .iter()
            .map(|r| r.name.clone())
            .filter(|n| n != DEFAULT_AGENT)
            .collect(),
        _ => Vec::new(),
    };
    let current = value.read().clone().unwrap_or_default();
    let title = if enabled {
        "Show activity for one agent"
    } else {
        "This source has no agent attribution, so it cannot be filtered by agent"
    };
    rsx! {
        select {
            class: "ih-log-filter",
            disabled: !enabled,
            title: "{title}",
            "aria-label": "Filter by agent",
            onchange: move |evt| {
                let v = evt.value();
                value.set(if v.is_empty() { None } else { Some(v) });
            },
            option { value: "", selected: current.is_empty(), "All agents" }
            option { value: "{DEFAULT_AGENT}", selected: current == DEFAULT_AGENT, "default" }
            for n in names {
                option {
                    key: "{n}",
                    value: "{n}",
                    selected: current == n,
                    "{n}"
                }
            }
        }
    }
}

/// One AGENT ACTIVITY row.
fn render_activity_row(r: &ActivityRow) -> Element {
    let full = format!("{}  {}  {}  {}", r.hms, r.agent, r.kind.label(), r.summary);
    let shown = display_cut(&full, ROW_DISPLAY_CHARS);
    rsx! {
        div { class: "ih-log-row", title: "{full}", "{shown}" }
    }
}

/// Sleep one poll interval (cfg-split like the uptime ticker in `mod.rs`).
async fn poll_sleep() {
    #[cfg(target_arch = "wasm32")]
    {
        gloo_timers::future::TimeoutFuture::new(POLL_INTERVAL_MS).await;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::sleep(std::time::Duration::from_millis(u64::from(POLL_INTERVAL_MS))).await;
    }
}

/// Empty-state block: a 14px/700 heading and a 12px body (UIC E12-empty).
fn empty_state(heading: &str, body: &str) -> Element {
    rsx! {
        div { class: "ih-log-empty",
            div { class: "ih-log-empty-heading", "{heading}" }
            div { class: "ih-log-empty-body", "{body}" }
        }
    }
}

/// One file-source row. Text nodes and attributes only.
fn render_line(line: &LogLine) -> Element {
    match line {
        LogLine::Server { text, level } => {
            let shown = display_cut(text, ROW_DISPLAY_CHARS);
            let (before, token, after) = split_level(&shown, level.as_deref());
            let lvl_class = level.clone().unwrap_or_default();
            rsx! {
                div { class: "ih-log-row", title: "{text}",
                    "{before}"
                    if !token.is_empty() {
                        span { class: "ih-log-level ih-log-level--{lvl_class}", "{token}" }
                    }
                    "{after}"
                }
            }
        }
        LogLine::Raw { text } => {
            let shown = display_cut(text, ROW_DISPLAY_CHARS);
            rsx! {
                div { class: "ih-log-row", title: "{text}", "{shown}" }
            }
        }
        // Audit rows are rendered by `render_file_row` (they need the
        // expansion state); one never reaches here.
        LogLine::Audit(_) => rsx! {},
    }
}

/// One file-source row: audit rows are expandable components, the rest are
/// plain lines.
fn render_file_row(line: &LogLine, expanded: Signal<HashSet<String>>) -> Element {
    match line {
        LogLine::Audit(row) => rsx! {
            AuditRow { row: row.clone(), expanded }
        },
        other => render_line(other),
    }
}

#[component]
pub fn ScreenLogs(is_active: bool) -> Element {
    // All hooks unconditional (no hooks after `?` or in branches).
    let mut tab = use_signal(|| LogsTab::Server);
    let mut lines = use_signal(Vec::<LogLine>::new);
    let mut cursor = use_signal(|| None::<LogCursor>);
    let mut loaded = use_signal(|| false);
    let mut fetch_error = use_signal(|| None::<String>);
    let mut paused = use_signal(|| false);
    let mut poll_tick = use_signal(|| 0u64);
    let agent_filter = use_signal(|| None::<String>);
    let activity = use_context::<ActivityLogCtx>();
    // Audit rows the operator expanded, keyed by `audit_key`.
    let expanded = use_signal(HashSet::<String>::new);
    // Auto-follow: true while the body is scrolled to the bottom.
    let mut following = use_signal(|| true);
    // AGENT ACTIVITY snapshot captured when pause was pressed.
    let mut activity_snapshot = use_signal(|| None::<Vec<ActivityRow>>);

    // `poll_tick` and `tab` are read in the sync prefix, so bumping either
    // restarts the read. The cursor is PEEKED: storing the returned cursor
    // never refetches.
    let page = use_resource(move || {
        let _tick = poll_tick();
        let t = tab();
        let cur = cursor.peek().clone();
        async move {
            match t {
                LogsTab::Server => Some(
                    read_log_tail(LogSource::Server, 500, cur)
                        .await
                        .map_err(|e| e.to_string()),
                ),
                LogsTab::Audit => Some(
                    read_log_tail(LogSource::Audit, 500, cur)
                        .await
                        .map_err(|e| e.to_string()),
                ),
                LogsTab::Activity => None,
            }
        }
    });

    // 3 s ticker. It only bumps when the previous read finished (one request
    // in flight; a slow server can never be starved by restarts), and never
    // while paused or on the activity tab. It is dropped with this component.
    use_future(move || async move {
        loop {
            poll_sleep().await;
            if *paused.peek() || *tab.peek() == LogsTab::Activity || !page.finished() {
                continue;
            }
            let n = *poll_tick.peek();
            poll_tick.set(n + 1);
        }
    });

    // Apply a resolved read. Reads only `page`; every write targets a signal
    // it does not read, so it cannot retrigger itself.
    use_effect(move || {
        let resolved = page.read().clone();
        if let Some(Some(result)) = resolved {
            match result {
                Ok(p) => {
                    lines.with_mut(|l| apply_page(l, p.lines, p.reset, LOGS_CLIENT_CAP));
                    cursor.set(p.cursor);
                    if !*loaded.peek() {
                        loaded.set(true);
                    }
                    if fetch_error.peek().is_some() {
                        fetch_error.set(None);
                    }
                }
                Err(e) => {
                    fetch_error.set(Some(e));
                    if !*loaded.peek() {
                        loaded.set(true);
                    }
                }
            }
        }
    });

    // Auto-follow: after rows change, keep the body pinned to the bottom while
    // `following`. Reads the row counts, the tab and `following`; writes no
    // signal, so it cannot retrigger itself.
    use_effect(move || {
        let _tab = *tab.read();
        let _file_rows = lines.read().len();
        let _activity_rows = activity.0.read().len();
        if *following.read() {
            scroll_to_bottom("ih-log-body");
        }
    });

    // Owned locals before rsx! so no signal borrow is held across the tree.
    let cur_tab = *tab.read();
    let is_loaded = *loaded.read();
    let is_paused = *paused.read();
    let is_following = *following.read();
    let error: Option<String> = fetch_error.read().clone();
    let rows: Vec<LogLine> = lines.read().clone();
    let caption = caption_for(cur_tab);
    let state = body_state(is_loaded, error.as_deref(), rows.len());
    let body: Element = match cur_tab {
        LogsTab::Server | LogsTab::Audit => match state {
            BodyState::Loading => rsx! {
                div { class: "ih-log-dim", "Loading log…" }
            },
            BodyState::Empty => empty_state(EMPTY_FILE_HEADING, EMPTY_FILE_BODY),
            BodyState::ErrorEmpty => rsx! {},
            BodyState::Rows | BodyState::ErrorWithRows => rsx! {
                for (i , line) in rows.iter().enumerate() {
                    Fragment { key: "{i}", {render_file_row(line, expanded)} }
                }
            },
        },
        LogsTab::Activity => {
            // Owned clone before rsx!; read only on this tab so other tabs do
            // not re-render on every activity event. While paused the rows
            // come from the snapshot taken when pause was pressed.
            let ring: Vec<ActivityRow> = match (is_paused, activity_snapshot.read().clone()) {
                (true, Some(snap)) => snap,
                _ => activity.0.read().clone(),
            };
            let filter: Option<String> = agent_filter.read().clone();
            let visible = filter_activity(&ring, filter.as_deref());
            if visible.is_empty() {
                empty_state(EMPTY_ACTIVITY_HEADING, EMPTY_ACTIVITY_BODY)
            } else {
                rsx! {
                    for r in visible.iter() {
                        Fragment { key: "{r.seq}", {render_activity_row(r)} }
                    }
                }
            }
        }
    };
    let filter_on = filter_enabled(cur_tab);
    let error_copy: Option<String> = match (cur_tab, error.as_deref()) {
        (LogsTab::Server | LogsTab::Audit, Some(reason)) => {
            Some(fetch_error_copy(cur_tab, reason))
        }
        _ => None,
    };

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-logs",
            "data-screen-label": "Logs",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    h1 { class: "screen-title", "Logs" }
                }
                div { class: "screen-actions",
                    LogsAgentFilter { value: agent_filter, enabled: filter_on }
                    button {
                        class: "btn btn--ghost btn--sm",
                        r#type: "button",
                        "aria-pressed": "{is_paused}",
                        onclick: move |_| {
                            if *paused.peek() {
                                paused.set(false);
                                activity_snapshot.set(None);
                                let n = *poll_tick.peek();
                                poll_tick.set(n + 1);
                            } else {
                                activity_snapshot.set(Some(activity.0.peek().clone()));
                                paused.set(true);
                            }
                        },
                        if is_paused {
                            "Resume log"
                        } else {
                            "Pause log"
                        }
                    }
                }
            }

            div { class: "tabs", role: "tablist",
                for (t , label) in TABS {
                    button {
                        key: "{label}",
                        class: "tab",
                        class: if cur_tab == t { "is-active" },
                        r#type: "button",
                        role: "tab",
                        "aria-selected": "{cur_tab == t}",
                        onclick: move |_| {
                            if *tab.peek() != t {
                                lines.set(Vec::new());
                                cursor.set(None);
                                loaded.set(false);
                                fetch_error.set(None);
                                tab.set(t);
                            }
                        },
                        "{label}"
                    }
                }
            }

            div { class: "ih-log-caption", "{caption}" }

            if let Some(copy) = error_copy {
                div { class: "ih-log-error", role: "alert",
                    span { "{copy}" }
                    button {
                        class: "btn btn--ghost btn--sm",
                        r#type: "button",
                        onclick: move |_| {
                            let n = *poll_tick.peek();
                            poll_tick.set(n + 1);
                        },
                        "Retry now"
                    }
                }
            }

            div { class: "ih-log-frame",
                div {
                    id: "ih-log-body",
                    class: "ih-log-body",
                    onscroll: move |_| {
                        let near = is_near_bottom("ih-log-body", 24);
                        if *following.peek() != near {
                            following.set(near);
                        }
                    },
                    {body}
                }
                if !is_following {
                    button {
                        class: "ih-log-jump",
                        r#type: "button",
                        onclick: move |_| {
                            scroll_to_bottom("ih-log-body");
                            following.set(true);
                        },
                        "Jump to latest"
                    }
                }
            }
        }
    }
}

/// The source caption under the tabs (D-24: file sources say they are not
/// agent-scoped).
fn caption_for(tab: LogsTab) -> &'static str {
    match tab {
        LogsTab::Server => "Server log (web.log) · not agent-scoped",
        LogsTab::Audit => "Audit log (audit.jsonl) · not agent-scoped",
        LogsTab::Activity => "Agent activity · live since this page loaded",
    }
}

const EMPTY_FILE_HEADING: &str = "Nothing logged yet";
const EMPTY_FILE_BODY: &str = "This log has no lines yet. New lines appear here within a few seconds.";
const EMPTY_ACTIVITY_HEADING: &str = "No agent activity yet";
const EMPTY_ACTIVITY_BODY: &str = "Turns, tool calls and subagent events appear here as they happen. Showing activity since this page loaded.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity_log::ActivityEventKind;
    use crate::components::hermes_app::agent_tabs::LiveAgentCtx;
    use crate::protocol::ProfileRow;
    use crate::state::Screen;

    fn raw(n: usize) -> LogLine {
        LogLine::Raw {
            text: format!("line {n}"),
        }
    }

    #[test]
    fn logs_apply_page_appends_and_caps() {
        let mut buf: Vec<LogLine> = (0..499).map(raw).collect();
        apply_page(&mut buf, vec![raw(499), raw(500), raw(501)], false, 500);
        assert_eq!(buf.len(), 500);
        assert_eq!(buf.last(), Some(&raw(501)));
        assert_eq!(buf.first(), Some(&raw(2)));

        apply_page(&mut buf, vec![raw(900)], true, 500);
        assert_eq!(buf, vec![raw(900)]);
    }

    const LOGS_RS: &str = include_str!("logs.rs");

    fn code_of(src: &str) -> &str {
        &src[..src.find("#[cfg(test)]").unwrap_or(src.len())]
    }

    fn act(seq: u64, agent: &str) -> ActivityRow {
        ActivityRow {
            seq,
            hms: "00:00:00".to_string(),
            agent: agent.to_string(),
            kind: ActivityEventKind::ToolStart,
            summary: format!("tool {seq}"),
        }
    }

    const APP_RS: &str = include_str!("../../../app.rs");
    const LOGS_CSS_SRC: &str = include_str!("../../../../assets/logs.css");

    fn audit_row(kind: &str) -> AuditRowWire {
        AuditRowWire {
            ts: "2026-10-08T01:02:03Z".to_string(),
            kind: kind.to_string(),
            tool: "ls".to_string(),
            server: None,
            session: "sess-abcdef123".to_string(),
            surface: "web".to_string(),
            decision: "approved".to_string(),
            resolution: "operator".to_string(),
            reason: String::new(),
            args: "{}".to_string(),
        }
    }

    #[test]
    fn logs_audit_row_summary_line() {
        assert_eq!(
            audit_summary(&audit_row("shell_command")),
            "01:02:03  shell_command  ls  approved/operator  sess-abc"
        );
        // A malformed timestamp degrades to the raw text, never panics.
        let mut r = audit_row("k");
        r.ts = "n/a".to_string();
        assert!(audit_summary(&r).starts_with("n/a  k"));
    }

    #[test]
    fn logs_audit_rollup_is_flagged() {
        assert!(is_rollup("yolo_bypass_rollup"));
        assert!(!is_rollup("shell_command"));
    }

    #[test]
    fn logs_display_cut_is_240_chars() {
        let long = "é".repeat(300);
        let cut = display_cut(&long, 240);
        assert_eq!(cut.chars().count(), 241);
        assert!(cut.ends_with('…'));
        assert_eq!(cut.chars().filter(|c| *c == 'é').count(), 240);
        assert_eq!(display_cut("short", 240), "short");
    }

    #[test]
    fn logs_error_copy_names_the_source() {
        assert_eq!(
            fetch_error_copy(LogsTab::Audit, "boom"),
            "Couldn't read the audit log: boom. Retrying every 3 seconds."
        );
        assert_eq!(
            fetch_error_copy(LogsTab::Server, "x"),
            "Couldn't read the server log: x. Retrying every 3 seconds."
        );
    }

    #[test]
    fn logs_body_state_precedence() {
        let err = Some("e".to_string());
        assert_eq!(body_state(false, None, 0), BodyState::Loading);
        assert_eq!(body_state(true, None, 0), BodyState::Empty);
        assert_eq!(body_state(true, err.as_deref(), 3), BodyState::ErrorWithRows);
        assert_eq!(body_state(true, None, 3), BodyState::Rows);
        assert_eq!(body_state(true, err.as_deref(), 0), BodyState::ErrorEmpty);
    }

    #[test]
    fn dashboard_and_logs_css_registered_unconditionally() {
        assert!(APP_RS.contains("asset!(\"/assets/logs.css\")"));
        assert!(APP_RS.contains("href: LOGS_CSS"));
    }

    #[test]
    fn logs_css_clears_the_wheel_and_sits_beside_it() {
        // User-requested layout (2026-10-08): the header row sits in the
        // overhang band to the right of the docked wheel; the content below
        // starts at or below the wheel's bottom edge. CSS variables only.
        assert!(LOGS_CSS_SRC.contains("#screen-logs"));
        assert!(LOGS_CSS_SRC.contains("var(--ih-row-inset)"));
        assert!(LOGS_CSS_SRC.contains("var(--ih-overhang-h)"));
        assert!(LOGS_CSS_SRC.contains(".ih-log-row"));
    }

    #[test]
    fn logs_agent_filter_applies_to_activity_only() {
        let rows = vec![act(1, "default"), act(2, "dev"), act(3, "default"), act(4, "dev")];
        let dev = filter_activity(&rows, Some("dev"));
        assert_eq!(dev.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![2, 4]);
        assert_eq!(filter_activity(&rows, None).len(), 4);
        assert!(!filter_enabled(LogsTab::Server));
        assert!(!filter_enabled(LogsTab::Audit));
        assert!(filter_enabled(LogsTab::Activity));
    }

    #[test]
    fn logs_screen_does_not_follow_the_agent_dropdown() {
        let needle = ["Agent", "Tabs", "Ctx"].concat();
        assert!(
            !code_of(LOGS_RS).contains(&needle),
            "the Logs screen is global (D-08): it must not read the header Agent dropdown's context"
        );
        let dangerous = ["dangerous_", "inner_html"].concat();
        assert!(!code_of(LOGS_RS).contains(&dangerous));
    }

    #[test]
    fn smoke_logs_screen_mounts_without_panic() {
        fn test_app() -> Element {
            let screen = use_signal(|| Screen::Logs);
            use_context_provider(|| screen);
            let activity = use_signal(Vec::<ActivityRow>::new);
            use_context_provider(|| ActivityLogCtx(activity));
            let profiles =
                use_resource(|| async { Ok::<Vec<ProfileRow>, ServerFnError>(Vec::new()) });
            use_context_provider(|| SharedProfilesCtx(profiles));
            let live =
                use_resource(|| async { Ok::<String, ServerFnError>("default".to_string()) });
            use_context_provider(|| LiveAgentCtx(live));
            rsx! {
                ScreenLogs { is_active: true }
            }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_to_vec();
    }
}
