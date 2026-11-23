//! Phase 54.13: the "Config file (YAML)" panel in Settings > General.
//!
//! Loads the (masked) `config.yaml` through `get_config_yaml`, shows it in a
//! monospace textarea, and is read-only unless the ROOT
//! `security.web_config_write_enabled` gate is open (D-05). `Check Config`
//! validates the editor text on the server without saving (D-12). No JS
//! editor dependency (D-11) and no hot-apply into the running server.
//!
//! Dioxus rules honoured here: every hook is declared unconditionally at the
//! top; the load is a `use_resource` keyed on signals read in the closure's
//! sync prefix (never the server-future hook plus `.restart()`); `.read()` in
//! render; owned locals before any `spawn`. Server text is only ever rendered
//! as text nodes.

use dioxus::prelude::*;

use crate::components::hermes_app::screens::tools::runtime_section::{
    runtime_confirm_body, runtime_outcome_line, RuntimeAction,
};
use crate::server::config_yaml_api::{
    check_config_yaml, get_config_backup, get_config_yaml, list_config_backups,
    list_config_targets, save_config_yaml, BackupText, CheckOutcome, ConfigBackup, ConfigTarget,
    ConfigYamlIssue, ConfigYamlLoad, SaveOutcome,
};
use crate::server::gateway_control_api::{restart_gateway, GatewayLifecycleOutcome};
use crate::server::tools_config_api::ConfigScope;

/// Home-relative file name shown before the server answers.
pub(crate) fn file_label(scope: &ConfigScope) -> String {
    match scope {
        ConfigScope::Root => "config.yaml".to_string(),
        ConfigScope::Profile(name) => format!("profiles/{name}/config.yaml"),
    }
}

/// `Line {l}, column {c}: {message}`, or just the message with no position;
/// the position is printed exactly once. Prefixed `Not saved. ` when saving.
pub(crate) fn issue_message(issue: &ConfigYamlIssue, saving: bool) -> String {
    let prefix = if saving { "Not saved. " } else { "" };
    match (issue.line, issue.column) {
        (Some(l), Some(c)) => format!("{prefix}Line {l}, column {c}: {}", issue.message),
        _ => format!("{prefix}{}", issue.message),
    }
}

const GATE_CLOSED_HINT: &str =
    "Set security.web_config_write_enabled: true in config.yaml, then retry.";

/// UI-SPEC copy for a non-saved save outcome. `Saved` and `LockoutAckRequired`
/// have their own views and return no lines.
pub(crate) fn save_outcome_lines(outcome: &SaveOutcome, file: &str) -> Vec<String> {
    match outcome {
        SaveOutcome::Invalid(issue) => vec![issue_message(issue, true)],
        SaveOutcome::ChangedOnDisk => vec![format!(
            "Not saved. {file} changed on disk after you opened it. Copy your edits, click Reload, then apply them again."
        )],
        SaveOutcome::UnmatchedPlaceholder { key_path } => vec![format!(
            "Not saved. {key_path} still shows the placeholder, but the saved file has no secret at that key. Type the new value, or put the key back where it was."
        )],
        SaveOutcome::GateClosed => vec![
            "Not saved. Config writes are disabled.".to_string(),
            GATE_CLOSED_HINT.to_string(),
        ],
        SaveOutcome::Failed => vec![
            format!("Could not save {file}."),
            "Your edits are still in the editor. Check the server connection and retry."
                .to_string(),
        ],
        SaveOutcome::Saved { .. } | SaveOutcome::LockoutAckRequired { .. } => Vec::new(),
    }
}

/// `Save and Lock` enables only on a typed `lock` (trimmed, ASCII
/// case-insensitive), mirroring `purge_confirm_enabled`.
pub(crate) fn lockout_confirm_enabled(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("lock")
}

/// Lockout dialog body (UI-SPEC), by whether the key is removed or set false.
pub(crate) fn lockout_body(removes_key: bool) -> &'static str {
    if removes_key {
        "This save removes security.web_config_write_enabled, and a missing key means off. The editor and every other Settings control that writes config become read-only. Turning it back on needs access to the host."
    } else {
        "This save sets security.web_config_write_enabled to false. The editor and every other Settings control that writes config become read-only. Turning it back on needs access to the host."
    }
}

/// `top` of the error-line band: 8px padding plus the line offset, minus the
/// textarea scroll.
pub(crate) fn errline_top_css(line: u32, scroll_top: f64) -> String {
    format!(
        "calc(8px + {} * var(--cfg-lh) - {scroll_top}px)",
        line.saturating_sub(1)
    )
}

/// UTF-16 offset of the 1-based `line:column` in `text`, as a textarea's
/// `setSelectionRange` counts it. A textarea value holds `\n` only, so a `\r`
/// before `\n` is not counted. A column past the line end clamps to it; a line
/// past the end clamps to the text end.
pub(crate) fn caret_utf16_offset(text: &str, line: u32, column: u32) -> usize {
    let mut offset = 0;
    for (i, raw) in text.split('\n').enumerate() {
        let l = raw.strip_suffix('\r').unwrap_or(raw);
        if i + 1 == line as usize {
            let before: usize = l
                .chars()
                .take(column.saturating_sub(1) as usize)
                .map(char::len_utf16)
                .sum();
            return offset + before;
        }
        offset += l.encode_utf16().count() + 1;
    }
    offset.saturating_sub(1)
}

/// Scroll the error line to the centre and place the caret. Only two integers
/// are interpolated (`u32` line, `usize` offset): no user text reaches the
/// script. `focus` also focuses the textarea (the Go to line link).
fn goto_script(line: u32, offset: usize, focus: bool) -> String {
    let focus_stmt = if focus {
        "ta.focus({ preventScroll: true });"
    } else {
        ""
    };
    format!(
        r#"
const ta = document.getElementById("cfg-textarea");
if (ta) {{
  const lh = parseFloat(getComputedStyle(ta).lineHeight) || 20;
  {focus_stmt}
  ta.setSelectionRange({offset}, {offset});
  ta.scrollTop = Math.max(0, ({line} - 1) * lh - ta.clientHeight / 2);
}}
"#
    )
}

const SERVER_RESTART_NOTE: &str = "The web server keeps the old config until you restart it from the host. Restart Gateway only restarts the gateway process.";

/// Saved-banner text lines: heading (locking variant after a lockout save),
/// the web-server note, then every unknown key (no cap) under its count line.
pub(crate) fn saved_banner_lines(unknown_keys: &[String], locked: bool) -> Vec<String> {
    let heading = if locked {
        "Saved. Config writes are now off. Restart the gateway and the web server from the host."
    } else {
        "Saved. Restart to apply."
    };
    let mut lines = vec![heading.to_string(), SERVER_RESTART_NOTE.to_string()];
    match unknown_keys.len() {
        0 => {}
        1 => lines.push("Saved with 1 unknown key (it is ignored):".to_string()),
        n => lines.push(format!("Saved with {n} unknown keys (they are ignored):")),
    }
    lines.extend(unknown_keys.iter().cloned());
    lines
}

const MAX_SHOWN_UNKNOWN_KEYS: usize = 10;

/// Heading plus the unknown-key lines (at most 10, then `and {n} more`).
pub(crate) fn unknown_keys_summary(keys: &[String]) -> (String, Vec<String>) {
    let heading = match keys.len() {
        0 => "Valid. No problems found.".to_string(),
        1 => "Valid, with 1 unknown key.".to_string(),
        n => format!("Valid, with {n} unknown keys. They are not errors, but nothing reads them."),
    };
    let mut lines: Vec<String> = keys.iter().take(MAX_SHOWN_UNKNOWN_KEYS).cloned().collect();
    if keys.len() > MAX_SHOWN_UNKNOWN_KEYS {
        lines.push(format!("and {} more", keys.len() - MAX_SHOWN_UNKNOWN_KEYS));
    }
    (heading, lines)
}

/// Gutter row count: one per textarea line, so a trailing newline adds the
/// textarea's empty last row.
pub(crate) fn gutter_rows(text: &str) -> usize {
    text.split('\n').count()
}

/// Footer line count (a trailing newline does not add a line).
pub(crate) fn footer_line_count(text: &str) -> usize {
    text.lines().count()
}

/// `1 line` / `{n} lines`.
pub(crate) fn line_count_label(n: usize) -> String {
    if n == 1 {
        "1 line".to_string()
    } else {
        format!("{n} lines")
    }
}

/// Gutter width in `ch`: `max(4, digits + 1)`.
pub(crate) fn gutter_width_ch(rows: usize) -> usize {
    (rows.to_string().len() + 1).max(4)
}

/// Editor signals derived from a file load. Anything but `Loaded` yields the
/// empty, read-only default so a previous file never shows under a new name.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct EditorState {
    pub text: String,
    pub digest: String,
    pub display_path: String,
    pub write_enabled: bool,
}

pub(crate) fn editor_state_for<E>(load: Option<&Result<ConfigYamlLoad, E>>) -> EditorState {
    match load {
        Some(Ok(ConfigYamlLoad::Loaded(r))) => EditorState {
            text: r.text.clone(),
            digest: r.sha256.clone(),
            display_path: r.display_path.clone(),
            write_enabled: r.write_enabled,
        },
        _ => EditorState::default(),
    }
}

/// Dirty when the text differs from the last loaded or saved text.
pub(crate) fn is_dirty(text: &str, baseline: &str) -> bool {
    text != baseline
}

/// `YYYYMMDDTHHMMSSmmmZ[-n]` -> `YYYY-MM-DD HH:MM:SS UTC` (the collision
/// suffix and the milliseconds are dropped).
pub(crate) fn backup_display_ts(id: &str) -> String {
    // Server ids are strict ASCII timestamps; anything else is shown as-is.
    match (
        id.get(0..4),
        id.get(4..6),
        id.get(6..8),
        id.get(9..11),
        id.get(11..13),
        id.get(13..15),
    ) {
        (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) => {
            format!("{y}-{mo}-{d} {h}:{mi}:{se} UTC")
        }
        _ => id.to_string(),
    }
}

/// `{n.n} KB` (KiB, one decimal).
pub(crate) fn backup_size_kb(bytes: u64) -> String {
    format!("{:.1} KB", bytes as f64 / 1024.0)
}

/// `Backups` until the count is known, then `Backups ({n})`.
pub(crate) fn backups_button_label(count: Option<usize>) -> String {
    match count {
        Some(n) => format!("Backups ({n})"),
        None => "Backups".to_string(),
    }
}

/// `(disabled, title)` for Restart Gateway. A locking save turned config
/// writes off, and `restart_gateway` needs that gate, so it cannot work (D-15).
pub(crate) fn restart_button_state(locked: bool) -> (bool, Option<&'static str>) {
    if locked {
        (
            true,
            Some(
                "Config writes are now off, so the web cannot restart the gateway. Restart it from the host.",
            ),
        )
    } else {
        (false, None)
    }
}

/// Extra Meta line for a restart refused by the process-control gate.
pub(crate) fn restart_gate_hint(outcome: &GatewayLifecycleOutcome) -> Option<&'static str> {
    match outcome {
        GatewayLifecycleOutcome::GateClosed { .. } => Some(
            "Set security.web_process_control_enabled: true in config.yaml to allow restarts from the web.",
        ),
        _ => None,
    }
}

/// `<option value>` for a scope: `root` / `profile:{name}`.
pub(crate) fn scope_option_value(scope: &ConfigScope) -> String {
    match scope {
        ConfigScope::Root => "root".to_string(),
        ConfigScope::Profile(name) => format!("profile:{name}"),
    }
}

/// Inverse of [`scope_option_value`]; `None` for an empty name or any other value.
pub(crate) fn scope_from_option_value(v: &str) -> Option<ConfigScope> {
    if v == "root" {
        return Some(ConfigScope::Root);
    }
    match v.strip_prefix("profile:") {
        Some(name) if !name.is_empty() => Some(ConfigScope::Profile(name.to_string())),
        _ => None,
    }
}

/// Picker row label.
pub(crate) fn option_label(target: &ConfigTarget) -> String {
    match (&target.scope, target.exists) {
        (ConfigScope::Root, _) => "Root".to_string(),
        (ConfigScope::Profile(name), true) => format!("Profile: {name}"),
        (ConfigScope::Profile(name), false) => format!("Profile: {name} (no config.yaml)"),
    }
}

/// An action that would replace the editor text, held while the operator
/// confirms discarding unsaved edits.
#[derive(Clone, Debug, PartialEq)]
enum PendingDiscard {
    Switch(ConfigScope),
    Reload,
    /// Put this backup's masked text in the editor (D-14).
    LoadBackup(String),
}

/// `None` = apply now; `Some(action)` = confirm first (only when dirty).
fn request_action(dirty: bool, action: PendingDiscard) -> Option<PendingDiscard> {
    dirty.then_some(action)
}

/// Fixed script (no interpolation): scroll the panel into view, then focus the
/// textarea once it is mounted (it is absent while a file is loading).
const FOCUS_SCRIPT: &str = r#"
const panel = document.getElementById("settings-config-file");
if (panel) {
  const calm = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  panel.scrollIntoView({ block: "start", behavior: calm ? "auto" : "smooth" });
}
let tries = 0;
const timer = setInterval(() => {
  const ta = document.getElementById("cfg-textarea");
  if (ta) { ta.focus({ preventScroll: true }); clearInterval(timer); }
  else if (++tries > 40) { clearInterval(timer); }
}, 50);
"#;

const GHOST_LINES: usize = 6;

/// What the result region shows after Check Config.
#[derive(Clone, Debug, PartialEq)]
enum PanelResult {
    Valid(Vec<String>),
    Issue(ConfigYamlIssue),
    Unmatched(String),
    Unavailable(String),
    Failed,
    /// A successful save (the amber banner).
    Saved {
        unknown_keys: Vec<String>,
        locked: bool,
        /// The file just saved: Restart Gateway targets this scope.
        scope: ConfigScope,
    },
    /// Any other non-saved save outcome (copy from `save_outcome_lines`).
    SaveRefused(SaveOutcome),
    /// The save turns the root write gate off and needs the typed confirm.
    Lockout {
        removes_key: bool,
    },
    /// Load Backup could not read the backup.
    BackupFailed,
}

/// A validation message. With a position it starts with a `Line {l}, column
/// {c}` link (Go to line); the rest of the message follows as text.
fn issue_view(issue: &ConfigYamlIssue, saving: bool, goto: Callback<(u32, u32)>) -> Element {
    let prefix = if saving { "Not saved. " } else { "" };
    match (issue.line, issue.column) {
        (Some(l), Some(c)) => rsx! {
            div { role: "alert",
                "{prefix}"
                button {
                    r#type: "button",
                    class: "cfg-hintlink",
                    onclick: move |_| goto.call((l, c)),
                    "Line {l}, column {c}"
                }
                ": {issue.message}"
            }
        },
        _ => {
            let msg = issue_message(issue, saving);
            rsx! {
                div { role: "alert", "{msg}" }
            }
        }
    }
}

/// Restart state shared with the banner: confirm open, in flight, and the
/// last outcome (`Err` = the call itself failed).
#[derive(Clone, Copy)]
struct RestartState {
    confirm: Signal<bool>,
    busy: Signal<bool>,
    outcome: Signal<Option<Result<GatewayLifecycleOutcome, ()>>>,
}

/// The amber `role=status` banner shown after a successful save (D-13), with
/// the inline Restart Gateway confirm (no overlay) and its outcome line.
fn saved_banner_view(
    unknown_keys: &[String],
    locked: bool,
    saved_scope: &ConfigScope,
    mut result: Signal<Option<PanelResult>>,
    mut restart: RestartState,
) -> Element {
    let lines = saved_banner_lines(unknown_keys, locked);
    let (restart_disabled, restart_title) = restart_button_state(locked);
    let confirming = *restart.confirm.read();
    let busy = *restart.busy.read();
    let outcome = restart.outcome.read().clone();
    let confirm_body = runtime_confirm_body(RuntimeAction::Restart, None);
    let saved_scope = saved_scope.clone();
    rsx! {
        div {
            role: "status",
            "aria-busy": if busy { "true" },
            style: "border: 1px solid rgba(210,153,34,0.45); background: rgba(210,153,34,0.06); display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 8px 16px; padding: 8px 16px;",
            div { style: "flex: 1 1 240px;",
                for (i , line) in lines.iter().enumerate() {
                    div { key: "{i}", class: "cfg-msg", "{line}" }
                }
                if busy {
                    div { class: "cfg-msg", "Restarting Gateway…" }
                }
                if confirming {
                    div {
                        role: "group",
                        "aria-label": "Restart the gateway",
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Escape && !*restart.busy.peek() {
                                restart.confirm.set(false);
                            }
                        },
                        div { style: "font-weight: 700;", "Restart the gateway?" }
                        div { class: "cfg-msg", "{confirm_body}" }
                        div { class: "cfg-actions",
                            button {
                                r#type: "button",
                                class: "btn btn--danger btn--sm",
                                disabled: busy,
                                onclick: move |_| {
                                    if *restart.busy.peek() {
                                        return;
                                    }
                                    let s = saved_scope.clone();
                                    restart.busy.set(true);
                                    spawn(async move {
                                        let r = restart_gateway(s).await.map_err(|_| ());
                                        restart.outcome.set(Some(r));
                                        restart.confirm.set(false);
                                        restart.busy.set(false);
                                    });
                                },
                                "Confirm Restart"
                            }
                            button {
                                r#type: "button",
                                class: "btn btn--ghost btn--sm",
                                disabled: busy,
                                onmounted: move |e: Event<MountedData>| {
                                    let d = e.data();
                                    spawn(async move {
                                        let _ = d.set_focus(true).await;
                                    });
                                },
                                onclick: move |_| restart.confirm.set(false),
                                "Cancel Restart"
                            }
                        }
                    }
                }
            }
            div { class: "cfg-actions",
                button {
                    r#type: "button",
                    class: "btn btn--ghost btn--sm",
                    disabled: restart_disabled || busy || confirming,
                    title: restart_title,
                    onclick: move |_| restart.confirm.set(true),
                    "Restart Gateway"
                }
                button {
                    r#type: "button",
                    class: "btn btn--ghost btn--sm",
                    disabled: busy,
                    onclick: move |_| result.set(None),
                    "Dismiss Banner"
                }
            }
        }
        match outcome {
            None => rsx! {},
            Some(Err(())) => rsx! {
                div { role: "status", class: "cfg-msg", "Could not restart the gateway." }
            },
            Some(Ok(o)) => rsx! {
                div { role: "status", class: "cfg-msg", "{runtime_outcome_line(&o)}" }
                if let Some(hint) = restart_gate_hint(&o) {
                    div { class: "cfg-msg", style: "color: var(--text-dim);", "{hint}" }
                }
            },
        }
    }
}

fn check_result_view(res: &PanelResult, file: &str, goto: Callback<(u32, u32)>) -> Element {
    match res {
        // Rendered by `saved_banner_view` / the lockout view (they need signals).
        PanelResult::Saved { .. } | PanelResult::Lockout { .. } => rsx! {},
        PanelResult::SaveRefused(SaveOutcome::Invalid(issue)) => issue_view(issue, true, goto),
        PanelResult::SaveRefused(outcome) => {
            let lines = save_outcome_lines(outcome, file);
            rsx! {
                div { role: "alert",
                    for (i , line) in lines.iter().enumerate() {
                        div { key: "{i}", "{line}" }
                    }
                }
            }
        }
        PanelResult::Valid(keys) => {
            let (heading, lines) = unknown_keys_summary(keys);
            rsx! {
                div { role: "status",
                    div { "{heading}" }
                    for (i , line) in lines.iter().enumerate() {
                        div { key: "{i}", style: "font-family: var(--font);", "{line}" }
                    }
                }
            }
        }
        PanelResult::Issue(issue) => issue_view(issue, false, goto),
        PanelResult::Unmatched(key_path) => rsx! {
            div { role: "alert",
                "{key_path} still shows the placeholder, but the saved file has no secret at that key. Type the new value, or put the key back where it was."
            }
        },
        PanelResult::Unavailable(reason) => rsx! {
            div { role: "alert",
                div { "Could not check {file}." }
                div { "{reason}" }
            }
        },
        PanelResult::BackupFailed => rsx! {
            div { role: "alert",
                div { "Could not load {file}." }
                div { "Check the server connection and click Reload." }
            }
        },
        PanelResult::Failed => rsx! {
            div { role: "alert",
                div { "Could not check {file}." }
                div { "Your edits are still in the editor. Check the server connection and retry." }
            }
        },
    }
}

#[component]
pub fn ConfigYamlPanel(focus_tick: Signal<u32>) -> Element {
    let mut scope = use_signal(ConfigScope::default);
    let mut reload = use_signal(|| 0u32);
    let mut pending_discard: Signal<Option<PendingDiscard>> = use_signal(|| None);
    // Bumped to remount the picker so it snaps back to the current scope.
    let mut select_gen = use_signal(|| 0u32);
    // Last handled tick: a tick bumped before this mount must not scroll.
    let mut seen_tick = use_signal(|| *focus_tick.peek());
    let mut text = use_signal(String::new);
    let mut baseline = use_signal(String::new);
    let mut digest = use_signal(String::new);
    let mut display_path = use_signal(String::new);
    let mut write_enabled = use_signal(|| false);
    let mut checking = use_signal(|| false);
    let mut saving = use_signal(|| false);
    let mut result: Signal<Option<PanelResult>> = use_signal(|| None);
    // Textarea scroll offset, mirrored into the gutter (pure Rust, no eval).
    let mut scroll_top = use_signal(|| 0.0_f64);
    // 1-based (line, column) of the failed Check/Save, for the error band (D-11).
    let mut error_line: Signal<Option<(u32, u32)>> = use_signal(|| None);
    // The lockout confirm's typed value, and Save Config's handle for focus return.
    let mut lock_input = use_signal(String::new);
    let mut save_btn: Signal<Option<std::rc::Rc<MountedData>>> = use_signal(|| None);
    // Go to line: focus the textarea, put the caret at line:column, centre it.
    let restart = RestartState {
        confirm: use_signal(|| false),
        busy: use_signal(|| false),
        outcome: use_signal(|| None),
    };
    let mut backups_open = use_signal(|| false);
    let mut backups_nonce = use_signal(|| 0u32);
    // (id, masked text as loaded): marks the row while the editor still equals it.
    let mut loaded_backup: Signal<Option<(String, String)>> = use_signal(|| None);
    let goto = use_callback(move |(l, c): (u32, u32)| {
        let off = caret_utf16_offset(&text.peek(), l, c);
        let _ = document::eval(&goto_script(l, off, true));
    });

    let targets = use_resource(|| async move { list_config_targets().await });

    // Keyed on `scope` and `reload`, read in the sync prefix so the resource
    // re-runs when either changes.
    let load = use_resource(move || {
        let s = scope();
        let _ = reload();
        async move { get_config_yaml(s).await }
    });

    // Re-fetched on file switch and after every successful save (nonce).
    let backups = use_resource(move || {
        let s = scope();
        let _ = backups_nonce();
        async move { list_config_backups(s).await }
    });

    use_effect(move || {
        let st = editor_state_for(load.read().as_ref());
        text.set(st.text.clone());
        baseline.set(st.text);
        digest.set(st.digest);
        display_path.set(st.display_path);
        write_enabled.set(st.write_enabled);
    });

    // Applies a (possibly confirmed) action; every action drops the check result.
    let mut apply = move |action: PendingDiscard| {
        match action {
            PendingDiscard::Switch(next) => {
                if next != *scope.peek() {
                    scope.set(next);
                }
            }
            PendingDiscard::Reload => reload += 1,
            PendingDiscard::LoadBackup(id) => {
                let s = scope.peek().clone();
                spawn(async move {
                    match get_config_backup(s, id.clone()).await {
                        Ok(BackupText::Loaded { text: t }) => {
                            // Text only: the baseline and digest stay the CURRENT file's.
                            text.set(t.clone());
                            loaded_backup.set(Some((id, t)));
                        }
                        _ => result.set(Some(PanelResult::BackupFailed)),
                    }
                });
            }
        }
        loaded_backup.set(None);
        result.set(None);
        error_line.set(None);
        pending_discard.set(None);
    };

    // Picker, Reload File and the hint links all go through the dirty guard.
    let mut request = move |action: PendingDiscard| {
        let dirty = is_dirty(&text.peek(), &baseline.peek());
        match request_action(dirty, action.clone()) {
            None => apply(action),
            Some(confirm) => {
                pending_discard.set(Some(confirm));
                select_gen += 1;
            }
        }
    };

    // The one effect that subscribes to `focus_tick` (D-10).
    use_effect(move || {
        let tick = focus_tick();
        if tick == *seen_tick.peek() {
            return;
        }
        seen_tick.set(tick);
        if *scope.peek() != ConfigScope::Root {
            request(PendingDiscard::Switch(ConfigScope::Root));
        }
        let _ = document::eval(FOCUS_SCRIPT);
    });

    // After a Check/Save: mark the failed line and scroll/caret to it without
    // stealing focus; `None` (success, no position) clears the band.
    let mut show_error_line = move |pos: Option<(u32, u32)>| {
        error_line.set(pos);
        if let Some((l, c)) = pos {
            let off = caret_utf16_offset(&text.peek(), l, c);
            let _ = document::eval(&goto_script(l, off, false));
        }
    };

    // The one call site of the server-side check: the button click (D-12).
    let on_check = move |_| {
        if *checking.peek() {
            return;
        }
        let s = scope.peek().clone();
        let body = text.peek().clone();
        checking.set(true);
        result.set(None);
        spawn(async move {
            let next = match check_config_yaml(s, body).await {
                Ok(CheckOutcome::Valid { unknown_keys }) => PanelResult::Valid(unknown_keys),
                Ok(CheckOutcome::Invalid(issue)) => PanelResult::Issue(issue),
                Ok(CheckOutcome::UnmatchedPlaceholder { key_path }) => {
                    PanelResult::Unmatched(key_path)
                }
                Ok(CheckOutcome::Unavailable { reason }) => PanelResult::Unavailable(reason),
                Err(_) => PanelResult::Failed,
            };
            let pos = match &next {
                PanelResult::Issue(ConfigYamlIssue {
                    line: Some(l),
                    column: Some(c),
                    ..
                }) => Some((*l, *c)),
                _ => None,
            };
            result.set(Some(next));
            show_error_line(pos);
            checking.set(false);
        });
    };

    // Save Config (ack = false) and Save and Lock (ack = true): the server
    // validates again, so no client-side shortcut (D-12).
    let mut do_save = move |ack: bool| {
        if *saving.peek() || *checking.peek() {
            return;
        }
        let s = scope.peek().clone();
        let body = text.peek().clone();
        let expected = digest.peek().clone();
        saving.set(true);
        if !ack {
            // The lockout dialog stays up while its own save is in flight.
            result.set(None);
        }
        spawn(async move {
            let outcome = save_config_yaml(s.clone(), body, expected, ack).await;
            let mut pos = None;
            match outcome {
                Ok(SaveOutcome::Saved {
                    unknown_keys,
                    locked,
                    ..
                }) => {
                    let mut restart = restart;
                    restart.confirm.set(false);
                    restart.outcome.set(None);
                    result.set(Some(PanelResult::Saved {
                        unknown_keys,
                        locked,
                        scope: s,
                    }));
                    // Reload so the editor shows the masked on-disk text and new digest.
                    reload += 1;
                    loaded_backup.set(None);
                    backups_nonce += 1;
                }
                Ok(SaveOutcome::LockoutAckRequired { removes_key }) => {
                    lock_input.set(String::new());
                    result.set(Some(PanelResult::Lockout { removes_key }));
                }
                Ok(other) => {
                    if let SaveOutcome::Invalid(ConfigYamlIssue {
                        line: Some(l),
                        column: Some(c),
                        ..
                    }) = &other
                    {
                        pos = Some((*l, *c));
                    }
                    result.set(Some(PanelResult::SaveRefused(other)));
                }
                Err(_) => result.set(Some(PanelResult::SaveRefused(SaveOutcome::Failed))),
            }
            show_error_line(pos);
            saving.set(false);
        });
    };
    let on_save = move |_| do_save(false);

    // Close the lockout dialog and hand focus back to Save Config.
    let mut close_lockout = move || {
        result.set(None);
        lock_input.set(String::new());
        let handle = save_btn.peek().clone();
        if let Some(h) = handle {
            spawn(async move {
                let _ = h.set_focus(true).await;
            });
        }
    };

    // Render-time snapshot: owned locals only.
    let (pending, loaded, load_error) = match &*load.read() {
        None => (true, false, None),
        Some(Err(_)) => (false, false, Some(String::new())),
        Some(Ok(ConfigYamlLoad::Unavailable { reason })) => (false, false, Some(reason.clone())),
        Some(Ok(ConfigYamlLoad::Loaded(_))) => (false, true, None),
    };
    let can_write = *write_enabled.read();
    let is_checking = *checking.read();
    let is_saving = *saving.read();
    let file = {
        let dp = display_path.read().clone();
        if dp.is_empty() {
            file_label(&scope.read())
        } else {
            dp
        }
    };
    let text_val = text.read().clone();
    let read_only_msg = loaded && !can_write;
    let rows = gutter_rows(&text_val);
    let gutter_w = gutter_width_ch(rows);
    let top = *scroll_top.read();
    let line_label = line_count_label(footer_line_count(&text_val));
    let dirty = loaded && is_dirty(&text_val, &baseline.read());
    let cur_scope_value = scope_option_value(&scope.read());
    let select_gen_val = *select_gen.read();
    let confirm_open = pending_discard.read().is_some();
    // Root always renders; a failed listing leaves only Root (D-05b).
    let (extra_targets, list_failed): (Vec<ConfigTarget>, bool) = match &*targets.read() {
        Some(Ok(t)) => (
            t.iter()
                .filter(|t| t.scope != ConfigScope::Root)
                .cloned()
                .collect(),
            false,
        ),
        Some(Err(_)) => (Vec::new(), true),
        None => (Vec::new(), false),
    };
    let check_view = result.read().as_ref().map(|r| match r {
        PanelResult::Saved {
            unknown_keys,
            locked,
            scope,
        } => saved_banner_view(unknown_keys, *locked, scope, result, restart),
        other => check_result_view(other, &file, goto),
    });
    let lockout_removes_key = match &*result.read() {
        Some(PanelResult::Lockout { removes_key }) => Some(*removes_key),
        _ => None,
    };
    let lock_val = lock_input.read().clone();
    let lock_ready = lockout_confirm_enabled(&lock_val);
    let err_line = (*error_line.read()).map(|(l, _)| l);
    let backups_open_val = *backups_open.read();
    // `None` = still listing; `Some(Err(()))` = the listing failed.
    let backup_list: Option<Result<Vec<ConfigBackup>, ()>> = match &*backups.read() {
        None => None,
        Some(Ok(v)) => Some(Ok(v.clone())),
        Some(Err(_)) => Some(Err(())),
    };
    let backups_label = backups_button_label(match &backup_list {
        Some(Ok(v)) if !v.is_empty() => Some(v.len()),
        _ => None,
    });
    // The loaded backup counts only while the editor still holds its text.
    let loaded_now: Option<(String, String)> = match &*loaded_backup.read() {
        Some((id, t)) if *t == text_val => Some((id.clone(), backup_display_ts(id))),
        _ => None,
    };
    let loaded_id = loaded_now.as_ref().map(|(id, _)| id.clone());

    rsx! {
        div { class: "panel", id: "settings-config-file",
            div { class: "panel-title",
                "Config file (YAML)"
                if read_only_msg {
                    " "
                    span { class: "pill amber", "READ-ONLY" }
                }
            }
            div { class: "panel-sub",
                "Edit the file on disk. Comments and key order are kept. Changes apply after a restart."
            }
            div { class: "cfg-actions",
                label { r#for: "cfg-file", "File" }
                select {
                    key: "{select_gen_val}",
                    id: "cfg-file",
                    class: "field-input",
                    style: "max-width: 280px;",
                    value: "{cur_scope_value}",
                    onchange: move |e| {
                        if let Some(next) = scope_from_option_value(&e.value()) {
                            if next != *scope.peek() {
                                request(PendingDiscard::Switch(next));
                            }
                        }
                    },
                    option { value: "root", "Root" }
                    for t in extra_targets.iter() {
                        option {
                            key: "{scope_option_value(&t.scope)}",
                            value: "{scope_option_value(&t.scope)}",
                            disabled: !t.exists,
                            "{option_label(t)}"
                        }
                    }
                }
                button {
                    r#type: "button",
                    class: "btn btn--ghost btn--sm",
                    disabled: pending,
                    onclick: move |_| request(PendingDiscard::Reload),
                    "Reload File"
                }
                button {
                    r#type: "button",
                    class: "btn btn--ghost btn--sm",
                    "aria-expanded": "{backups_open_val}",
                    "aria-controls": "cfg-backups",
                    onclick: move |_| backups_open.toggle(),
                    "{backups_label}"
                }
                button {
                    r#type: "button",
                    class: "btn btn--ghost btn--sm",
                    disabled: pending || is_checking || is_saving || !can_write,
                    title: if read_only_msg { "Config writes are disabled" },
                    onclick: on_check,
                    if is_checking {
                        "Checking Config…"
                    } else {
                        "Check Config"
                    }
                }
                button {
                    r#type: "button",
                    class: "btn btn--sm",
                    disabled: pending || is_checking || is_saving || !can_write,
                    title: if read_only_msg { "Config writes are disabled" },
                    onmounted: move |e: Event<MountedData>| save_btn.set(Some(e.data())),
                    onclick: on_save,
                    if is_saving {
                        "Saving Config…"
                    } else {
                        "Save Config"
                    }
                }
            }
            if list_failed {
                div { class: "cfg-msg", "Could not list profiles." }
            }
            div { class: "cfg-msg", "File: {file}" }
            div {
                id: "cfg-result",
                class: "cfg-msg",
                "aria-busy": if is_checking || is_saving { "true" },
                if confirm_open {
                    div {
                        role: "group",
                        "aria-label": "Discard unsaved edits",
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Escape {
                                pending_discard.set(None);
                            }
                        },
                        div { "Discard your unsaved edits?" }
                        div { class: "cfg-actions",
                            button {
                                r#type: "button",
                                class: "btn btn--ghost btn--sm",
                                onclick: move |_| {
                                    let action = pending_discard.peek().clone();
                                    if let Some(action) = action {
                                        apply(action);
                                    }
                                },
                                "Discard Edits"
                            }
                            button {
                                r#type: "button",
                                class: "btn btn--ghost btn--sm",
                                onmounted: move |e: Event<MountedData>| {
                                    let d = e.data();
                                    spawn(async move {
                                        let _ = d.set_focus(true).await;
                                    });
                                },
                                onclick: move |_| pending_discard.set(None),
                                "Keep Editing"
                            }
                        }
                    }
                } else if let Some(removes_key) = lockout_removes_key {
                    div {
                        role: "alertdialog",
                        "aria-label": "Turn off config writes",
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Escape && !*saving.peek() {
                                close_lockout();
                            }
                        },
                        div { style: "font-weight: 700;", "Turn off config writes?" }
                        div { "{lockout_body(removes_key)}" }
                        div { style: "color: var(--danger); font-weight: 700;",
                            "This locks you out of web config edits."
                        }
                        label { r#for: "cfg-lock-input", "Type lock to confirm" }
                        input {
                            id: "cfg-lock-input",
                            class: "field-input",
                            maxlength: "16",
                            autocomplete: "off",
                            autocapitalize: "off",
                            spellcheck: "false",
                            value: "{lock_val}",
                            onmounted: move |e: Event<MountedData>| {
                                let d = e.data();
                                spawn(async move {
                                    let _ = d.set_focus(true).await;
                                });
                            },
                            oninput: move |e| lock_input.set(e.value()),
                        }
                        div { class: "cfg-actions",
                            button {
                                r#type: "button",
                                class: "btn btn--danger btn--sm",
                                disabled: is_saving || !lock_ready,
                                title: if !lock_ready { "Type lock to enable this button." },
                                onclick: move |_| do_save(true),
                                if is_saving {
                                    "Saving Config…"
                                } else {
                                    "Save and Lock"
                                }
                            }
                            button {
                                r#type: "button",
                                class: "btn btn--ghost btn--sm",
                                disabled: is_saving,
                                onclick: move |_| close_lockout(),
                                "Keep Writes On"
                            }
                        }
                    }
                } else if let Some(view) = check_view {
                    {view}
                } else if read_only_msg {
                    div { role: "status",
                        div { "Config writes are disabled." }
                        div { "Set security.web_config_write_enabled: true in config.yaml, then retry." }
                    }
                }
                if let Some((_, ts)) = &loaded_now {
                    div { role: "status", style: "color: var(--amber);",
                        "Loaded backup from {ts}. Save to restore it."
                    }
                }
                if let Some(detail) = load_error {
                    div { role: "alert",
                        div { "Could not load {file}." }
                        if detail.is_empty() {
                            div { "Check the server connection and click Reload." }
                        } else {
                            div { "{detail}" }
                        }
                    }
                }
            }
            div { class: "cfg-editor",
                if pending {
                    div { role: "status", style: "grid-column: 1 / -1;",
                        for i in 0..GHOST_LINES {
                            div {
                                key: "{i}",
                                style: "opacity: 0.35; height: 12px; margin: 8px; background: var(--border-faint);",
                            }
                        }
                        "Loading {file}…"
                    }
                } else {
                    div {
                        class: "cfg-gutter",
                        "aria-hidden": "true",
                        style: "width: {gutter_w}ch;",
                        div { style: "transform: translateY(-{top}px);",
                            for n in 1..=rows {
                                div {
                                    key: "{n}",
                                    style: if err_line == Some(n as u32) { "color: var(--danger); font-weight: 700;" },
                                    "{n}"
                                }
                            }
                        }
                    }
                    textarea {
                        id: "cfg-textarea",
                        class: "cfg-textarea",
                        wrap: "off",
                        spellcheck: "false",
                        autocomplete: "off",
                        autocapitalize: "off",
                        "aria-label": "Contents of {file}",
                        "aria-describedby": "cfg-result cfg-footer",
                        readonly: !can_write || is_saving,
                        placeholder: "# This file is empty. Add settings here, then choose Save.",
                        value: "{text_val}",
                        oninput: move |e| {
                            text.set(e.value());
                            // The first edit clears the error-line band.
                            if error_line.peek().is_some() {
                                error_line.set(None);
                            }
                            // The first edit after saving clears the banner.
                            if matches!(&*result.peek(), Some(PanelResult::Saved { .. })) {
                                result.set(None);
                            }
                        },
                        onscroll: move |e| scroll_top.set(e.data().scroll_top()),
                    }
                    if let Some(l) = err_line {
                        div {
                            class: "cfg-errline",
                            "aria-hidden": "true",
                            style: "top: {errline_top_css(l, top)};",
                        }
                    }
                }
            }
            div { id: "cfg-footer", class: "cfg-msg",
                div {
                    "{line_label}"
                    if dirty {
                        " · Unsaved changes"
                    }
                }
                div {
                    "Secret values show as •••• (kept). Leave the placeholder to keep the saved value, or type a new one."
                }
            }
            if backups_open_val {
                div { id: "cfg-backups",
                    div { class: "panel-sub", "Backups" }
                    div { class: "cfg-msg", "Newest 10 are kept." }
                    div { class: "row-list",
                        match &backup_list {
                            None => rsx! {
                                div { role: "status",
                                    for i in 0..3 {
                                        div {
                                            key: "{i}",
                                            style: "opacity: 0.35; height: 12px; margin: 8px; background: var(--border-faint);",
                                        }
                                    }
                                }
                            },
                            Some(Err(())) => rsx! {
                                div { role: "alert",
                                    div { "Could not list backups." }
                                    div { "Check the server connection and click Backups again." }
                                }
                            },
                            Some(Ok(list)) if list.is_empty() => rsx! {
                                div { role: "status",
                                    div { "No backups yet" }
                                    div { "A backup is saved here each time you save this file." }
                                }
                            },
                            Some(Ok(list)) => rsx! {
                                for b in list.iter() {
                                    div {
                                        key: "{b.id}",
                                        class: if loaded_id.as_deref() == Some(b.id.as_str()) { "row cfg-backup-row is-active" } else { "row cfg-backup-row" },
                                        span { "{backup_display_ts(&b.id)}" }
                                        span { class: "cfg-msg", "{backup_size_kb(b.size_bytes)}" }
                                        button {
                                            r#type: "button",
                                            class: "btn btn--ghost btn--sm",
                                            disabled: pending || is_saving,
                                            onclick: {
                                                let id = b.id.clone();
                                                move |_| request(PendingDiscard::LoadBackup(id.clone()))
                                            },
                                            "Load Backup"
                                        }
                                    }
                                }
                            },
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

    #[test]
    fn config_yaml_panel_file_label_for_scope() {
        assert_eq!(file_label(&ConfigScope::Root), "config.yaml");
        assert_eq!(
            file_label(&ConfigScope::Profile("alpha".to_string())),
            "profiles/alpha/config.yaml"
        );
    }

    #[test]
    fn config_yaml_panel_check_message_formats_position() {
        let with_pos = ConfigYamlIssue {
            message: "msg".to_string(),
            line: Some(2),
            column: Some(19),
        };
        assert_eq!(issue_message(&with_pos, false), "Line 2, column 19: msg");
        assert_eq!(
            issue_message(&with_pos, true),
            "Not saved. Line 2, column 19: msg"
        );
        let no_pos = ConfigYamlIssue {
            message: "msg".to_string(),
            line: None,
            column: None,
        };
        assert_eq!(issue_message(&no_pos, false), "msg");
    }

    #[test]
    fn config_yaml_panel_unknown_key_list_caps_at_ten() {
        let keys: Vec<String> = (0..12).map(|i| format!("k{i}")).collect();
        let (_, lines) = unknown_keys_summary(&keys);
        assert_eq!(lines.len(), 11);
        assert_eq!(lines[0], "k0");
        assert_eq!(lines[9], "k9");
        assert_eq!(lines[10], "and 2 more");
        let (_, few) = unknown_keys_summary(&keys[..10]);
        assert_eq!(few.len(), 10);
    }

    #[test]
    fn config_yaml_panel_gutter_width_is_max_four_or_digits_plus_one() {
        assert_eq!(gutter_width_ch(9), 4);
        assert_eq!(gutter_width_ch(999), 4);
        assert_eq!(gutter_width_ch(1000), 5);
        assert_eq!(gutter_width_ch(12345), 6);
    }

    #[test]
    fn config_yaml_panel_gutter_rows_follow_textarea_lines() {
        assert_eq!(gutter_rows(""), 1);
        assert_eq!(gutter_rows("a"), 1);
        assert_eq!(gutter_rows("a\n"), 2);
        assert_eq!(gutter_rows("a\nb"), 2);
    }

    #[test]
    fn config_yaml_panel_footer_line_count() {
        assert_eq!(line_count_label(footer_line_count("")), "0 lines");
        assert_eq!(line_count_label(footer_line_count("a\n")), "1 line");
        assert_eq!(line_count_label(footer_line_count("a\nb\n")), "2 lines");
    }

    #[test]
    fn config_yaml_panel_failed_load_clears_previous_editor_state() {
        let read = crate::server::config_yaml_api::ConfigYamlRead {
            text: "a: 1\n".to_string(),
            sha256: "abc".to_string(),
            display_path: "config.yaml".to_string(),
            write_enabled: true,
        };
        let loaded: Result<ConfigYamlLoad, ()> = Ok(ConfigYamlLoad::Loaded(read));
        let st = editor_state_for(Some(&loaded));
        assert_eq!(st.text, "a: 1\n");
        assert!(st.write_enabled && st.digest == "abc");
        let unavailable: Result<ConfigYamlLoad, ()> = Ok(ConfigYamlLoad::Unavailable {
            reason: "too large".to_string(),
        });
        for failed in [Some(&unavailable), Some(&Err(())), None] {
            assert_eq!(editor_state_for(failed), EditorState::default());
        }
    }

    #[test]
    fn config_yaml_panel_dirty_when_text_differs_from_baseline() {
        assert!(!is_dirty("a: 1\n", "a: 1\n"));
        assert!(is_dirty("a: 2\n", "a: 1\n"));
        assert!(is_dirty("", "a"));
    }

    #[test]
    fn config_yaml_panel_valid_copy_pluralizes() {
        let keys = |n: usize| -> Vec<String> { (0..n).map(|i| format!("k{i}")).collect() };
        assert_eq!(
            unknown_keys_summary(&keys(0)).0,
            "Valid. No problems found."
        );
        assert_eq!(
            unknown_keys_summary(&keys(1)).0,
            "Valid, with 1 unknown key."
        );
        assert_eq!(
            unknown_keys_summary(&keys(3)).0,
            "Valid, with 3 unknown keys. They are not errors, but nothing reads them."
        );
    }

    #[test]
    fn config_yaml_panel_picker_option_labels() {
        let t = |scope, exists| ConfigTarget { scope, exists };
        assert_eq!(option_label(&t(ConfigScope::Root, true)), "Root");
        assert_eq!(
            option_label(&t(ConfigScope::Profile("alpha".into()), true)),
            "Profile: alpha"
        );
        assert_eq!(
            option_label(&t(ConfigScope::Profile("beta".into()), false)),
            "Profile: beta (no config.yaml)"
        );
    }

    #[test]
    fn config_yaml_panel_scope_option_value_round_trips() {
        assert_eq!(scope_option_value(&ConfigScope::Root), "root");
        let a = ConfigScope::Profile("alpha".into());
        assert_eq!(scope_option_value(&a), "profile:alpha");
        assert_eq!(scope_from_option_value("root"), Some(ConfigScope::Root));
        assert_eq!(scope_from_option_value("profile:alpha"), Some(a));
        assert_eq!(scope_from_option_value("profile:"), None);
        assert_eq!(scope_from_option_value("bogus"), None);
    }

    #[test]
    fn config_yaml_panel_discard_confirm_only_when_dirty() {
        assert_eq!(request_action(false, PendingDiscard::Reload), None);
        assert_eq!(
            request_action(true, PendingDiscard::Switch(ConfigScope::Root)),
            Some(PendingDiscard::Switch(ConfigScope::Root))
        );
    }

    #[test]
    fn config_yaml_panel_saved_banner_lines() {
        let note = "The web server keeps the old config until you restart it from the host. Restart Gateway only restarts the gateway process.";
        assert_eq!(
            saved_banner_lines(&[], false),
            vec!["Saved. Restart to apply.".to_string(), note.to_string()]
        );
        let one = saved_banner_lines(&["bogus".to_string()], false);
        assert_eq!(one.len(), 4);
        assert_eq!(one[2], "Saved with 1 unknown key (it is ignored):");
        assert_eq!(one[3], "bogus");
        let keys: Vec<String> = (0..3).map(|i| format!("k{i}")).collect();
        let three = saved_banner_lines(&keys, false);
        assert_eq!(three[2], "Saved with 3 unknown keys (they are ignored):");
        assert_eq!(&three[3..], &keys[..], "every key shown, no cap");
        let many: Vec<String> = (0..12).map(|i| format!("k{i}")).collect();
        assert_eq!(saved_banner_lines(&many, false).len(), 2 + 1 + 12);
    }

    #[test]
    fn config_yaml_panel_save_outcome_copy_matches_ui_spec() {
        let f = "config.yaml";
        assert_eq!(
            save_outcome_lines(&SaveOutcome::ChangedOnDisk, f),
            vec![
                "Not saved. config.yaml changed on disk after you opened it. Copy your edits, click Reload, then apply them again."
            ]
        );
        assert_eq!(
            save_outcome_lines(
                &SaveOutcome::UnmatchedPlaceholder {
                    key_path: "a.b".to_string()
                },
                f
            ),
            vec![
                "Not saved. a.b still shows the placeholder, but the saved file has no secret at that key. Type the new value, or put the key back where it was."
            ]
        );
        assert_eq!(
            save_outcome_lines(&SaveOutcome::GateClosed, f),
            vec![
                "Not saved. Config writes are disabled.",
                "Set security.web_config_write_enabled: true in config.yaml, then retry."
            ]
        );
        assert_eq!(
            save_outcome_lines(&SaveOutcome::Failed, f),
            vec![
                "Could not save config.yaml.",
                "Your edits are still in the editor. Check the server connection and retry."
            ]
        );
        let no_pos = ConfigYamlIssue {
            message: "too big".to_string(),
            line: None,
            column: None,
        };
        assert_eq!(
            save_outcome_lines(&SaveOutcome::Invalid(no_pos), f),
            vec!["Not saved. too big"]
        );
    }

    #[test]
    fn config_yaml_panel_lockout_confirm_enables_only_on_lock() {
        for ok in ["lock", " LOCK ", "Lock"] {
            assert!(lockout_confirm_enabled(ok), "{ok:?}");
        }
        for bad in ["", "lo", "locked", "lock it"] {
            assert!(!lockout_confirm_enabled(bad), "{bad:?}");
        }
    }

    #[test]
    fn config_yaml_panel_lockout_body_variants() {
        assert_eq!(
            lockout_body(false),
            "This save sets security.web_config_write_enabled to false. The editor and every other Settings control that writes config become read-only. Turning it back on needs access to the host."
        );
        assert_eq!(
            lockout_body(true),
            "This save removes security.web_config_write_enabled, and a missing key means off. The editor and every other Settings control that writes config become read-only. Turning it back on needs access to the host."
        );
    }

    #[test]
    fn config_yaml_panel_errline_top_tracks_scroll() {
        assert_eq!(
            errline_top_css(1, 0.0),
            "calc(8px + 0 * var(--cfg-lh) - 0px)"
        );
        assert_eq!(
            errline_top_css(5, 40.5),
            "calc(8px + 4 * var(--cfg-lh) - 40.5px)"
        );
    }

    #[test]
    fn config_yaml_panel_caret_offset_counts_utf16_units() {
        let text = "a\u{1F600}\nbc\n";
        assert_eq!(caret_utf16_offset(text, 2, 2), 5);
        assert_eq!(caret_utf16_offset(text, 1, 1), 0);
        assert_eq!(caret_utf16_offset(text, 1, 3), 3);
        // A column past the line end clamps to the line end.
        assert_eq!(caret_utf16_offset(text, 2, 99), 6);
        // CRLF is one unit in a textarea value.
        assert_eq!(caret_utf16_offset("ab\r\ncd\r\n", 2, 2), 4);
    }

    #[test]
    fn config_yaml_panel_saved_banner_heading_for_locking_save() {
        assert_eq!(
            saved_banner_lines(&[], true)[0],
            "Saved. Config writes are now off. Restart the gateway and the web server from the host."
        );
    }

    #[test]
    fn config_yaml_panel_backup_timestamp_formats_utc() {
        assert_eq!(
            backup_display_ts("20261116T120000123Z"),
            "2026-11-16 12:00:00 UTC"
        );
        assert_eq!(
            backup_display_ts("20261116T120000123Z-2"),
            "2026-11-16 12:00:00 UTC"
        );
    }

    #[test]
    fn config_yaml_panel_backup_size_formats_kb() {
        assert_eq!(backup_size_kb(0), "0.0 KB");
        assert_eq!(backup_size_kb(1536), "1.5 KB");
    }

    #[test]
    fn config_yaml_panel_backups_button_label() {
        assert_eq!(backups_button_label(None), "Backups");
        assert_eq!(backups_button_label(Some(3)), "Backups (3)");
    }

    #[test]
    fn config_yaml_panel_restart_disabled_after_locking_save() {
        assert_eq!(restart_button_state(false), (false, None));
        assert_eq!(
            restart_button_state(true),
            (
                true,
                Some(
                    "Config writes are now off, so the web cannot restart the gateway. Restart it from the host."
                )
            )
        );
    }

    #[test]
    fn config_yaml_panel_restart_gate_hint_only_for_gate_closed() {
        assert_eq!(
            restart_gate_hint(&GatewayLifecycleOutcome::GateClosed {
                message: "x".to_string()
            }),
            Some(
                "Set security.web_process_control_enabled: true in config.yaml to allow restarts from the web."
            )
        );
        assert_eq!(
            restart_gate_hint(&GatewayLifecycleOutcome::CooldownActive),
            None
        );
        assert_eq!(
            restart_gate_hint(&GatewayLifecycleOutcome::NotRunning),
            None
        );
    }
}
