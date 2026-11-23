//! Phase 54.9 Plan 09 (D-13 / D-09 / D-15) — the OpenUI screen.
//!
//! A prompt box whose request runs as a NORMAL agent turn on this screen's own chat socket
//! and session, with `ChatRequest.openui = true` asking the server to attach its compiled-in
//! OpenUI system prompt as a one-shot overlay. The reply streams into the sandboxed frame
//! served by `GET /openui/frame` (plan 08): only the `Delta` text after the turn's last
//! `ToolCallEnd` is renderable (ADR-0007 "Generation path"), so narration before a tool call
//! is discarded with an `openui-reset`.
//!
//! The frame is an `<iframe sandbox="allow-scripts">` (allow-scripts only, no other sandbox
//! token); generated UI is never injected into the Dioxus DOM. No chat bubble is created for
//! the generation. Approval events raised by this screen's turns are applied to the ROOT
//! approval-card map, so the normal card appears in Chat and the amber badge on the Chat wedge.
//!
//! D-15 (the frame -> host half): a generated button press arrives as ONE `message` event. The
//! host registers a single window listener (removed on unmount), accepts an event only when
//! `event.source` is this iframe's `contentWindow` (the sandbox origin is opaque, so the origin
//! cannot be the check), caps the payload at 8192 bytes before parsing it, and turns a valid
//! action into a plain new chat turn on the same session. An action is only ever text sent to
//! the agent; it is never mapped to an approval decision or a tool call.

use crate::components::hermes_app::profile_topbar::TopbarProfileRefreshCtx;
use crate::protocol::{ChatRequest, ChatStreamEvent};
use crate::server::openui_gateway::openui_gateway_status;
use crate::server::openui_views_api::{
    delete_openui_view, list_openui_views, replace_openui_snapshot, save_openui_view, OpenUiView,
};
#[cfg(target_arch = "wasm32")]
use dioxus::core::use_drop;
use dioxus::prelude::*;

/// DOM id of the sandboxed frame (the host bridge looks it up by this id).
pub(crate) const OPENUI_FRAME_ID: &str = "ih-openui-frame";

/// UI-SPEC Copywriting Part B.
const PROMPT_PLACEHOLDER: &str =
    "Describe the view you want, for example: a table of this week's failed cron jobs.";
const STAGE_EMPTY: &str = "Your generated view appears here.";

/// One command for the frame's host bridge (`frame-bootstrap.js`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FrameCmd {
    Reset,
    Push(String),
    End,
}

// The host bridge (postMessage in, message listener out) is wasm-only; a native build only
// exercises this item from tests, so it reads as dead code there.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
/// The `postMessage` payload (as JSON text) for a [`FrameCmd`]: the exact shapes the frame
/// bootstrap accepts.
pub(crate) fn frame_message(cmd: &FrameCmd) -> String {
    match cmd {
        FrameCmd::Reset => serde_json::json!({ "type": "openui-reset" }),
        FrameCmd::Push(text) => serde_json::json!({ "type": "openui-push", "text": text }),
        FrameCmd::End => serde_json::json!({ "type": "openui-end" }),
    }
    .to_string()
}

/// Folds the chat stream of one OpenUI turn into frame commands (ADR-0007: only `Delta` after
/// the last `ToolCallEnd` is renderable; `Error`/`Finished`/`TurnEnded` end the turn).
#[derive(Default)]
pub(crate) struct OpenUiStream {
    turn: Option<uuid::Uuid>,
    ended: bool,
}

impl OpenUiStream {
    pub(crate) fn apply(&mut self, ev: &ChatStreamEvent) -> Vec<FrameCmd> {
        use ChatStreamEvent as E;
        // Once a turn is known, another turn's events are not ours. A nil id (serde default)
        // is accepted for any turn.
        let ours = |known: &Option<uuid::Uuid>, id: &uuid::Uuid| {
            id.is_nil() || known.is_none_or(|k| k == *id)
        };
        match ev {
            E::TurnStarted { turn_id, .. } => {
                self.turn = Some(*turn_id);
                self.ended = false;
                Vec::new()
            }
            E::Delta { turn_id, text } if ours(&self.turn, turn_id) => {
                self.ended = false;
                vec![FrameCmd::Push(text.clone())]
            }
            E::ToolCallEnd { turn_id, .. } if ours(&self.turn, turn_id) => {
                self.ended = false;
                vec![FrameCmd::Reset]
            }
            E::Finished { turn_id, .. }
            | E::Error { turn_id, .. }
            | E::TurnCancelled { turn_id }
            | E::TurnEnded { turn_id, .. }
                if ours(&self.turn, turn_id) && !self.ended =>
            {
                self.ended = true;
                vec![FrameCmd::End]
            }
            _ => Vec::new(),
        }
    }
}

/// The text the frame should currently show, kept on the host so a frame that finishes
/// loading AFTER the first deltas arrived (the iframe mounts on the first Generate) is
/// replayed to the current state instead of missing them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OpenUiDoc {
    text: String,
    ended: bool,
}

impl OpenUiDoc {
    pub(crate) fn apply(&mut self, cmd: &FrameCmd) {
        match cmd {
            FrameCmd::Reset => *self = Self::default(),
            FrameCmd::Push(t) => self.text.push_str(t),
            FrameCmd::End => self.ended = true,
        }
    }

    /// The text currently shown (after the last reset).
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The commands that bring a freshly loaded frame to this state.
    pub(crate) fn replay(&self) -> Vec<FrameCmd> {
        let mut out = Vec::new();
        if !self.text.is_empty() {
            out.push(FrameCmd::Push(self.text.clone()));
        }
        if self.ended {
            out.push(FrameCmd::End);
        }
        out
    }
}

/// `Generate view` is enabled only for a non-blank prompt, and never while a turn streams.
pub(crate) fn can_generate(prompt: &str, streaming: bool) -> bool {
    !streaming && !prompt.trim().is_empty()
}

// The host bridge (postMessage in, message listener out) is wasm-only; a native build only
// exercises this item from tests, so it reads as dead code there.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
/// D-15 (T-54.9-23): the largest frame message the host will look at. Checked BEFORE the JSON
/// is parsed.
pub(crate) const OPENUI_ACTION_MAX_BYTES: usize = 8192;
/// A button label is 1 to this many characters.
// The host bridge (postMessage in, message listener out) is wasm-only; a native build only
// exercises this item from tests, so it reads as dead code there.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const OPENUI_ACTION_LABEL_MAX_CHARS: usize = 500;

/// The one action shape the frame sends (`openui-action`, `continue_conversation`): the text of
/// the pressed `@ToAssistant` button plus the form fields it carried.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpenUiAction {
    pub action_type: String,
    pub label: String,
    pub form_state: serde_json::Value,
}

// The host bridge (postMessage in, message listener out) is wasm-only; a native build only
// exercises this item from tests, so it reads as dead code there.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
/// Validate one raw frame message. Anything that is not exactly the bridge shape is dropped;
/// the `Err` text is for tests and logs, never shown to the operator.
pub(crate) fn parse_openui_action(raw: &str) -> Result<OpenUiAction, &'static str> {
    if raw.len() > OPENUI_ACTION_MAX_BYTES {
        return Err("message too large");
    }
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|_| "not valid JSON")?;
    let obj = value.as_object().ok_or("not an object")?;
    if obj.get("type").and_then(|t| t.as_str()) != Some("openui-action") {
        return Err("wrong message type");
    }
    // ADR-0007 "changed": the bootstrap forwards only continue_conversation; the host
    // re-checks so a forged frame cannot widen the set.
    let action_type = obj
        .get("action_type")
        .and_then(|t| t.as_str())
        .ok_or("missing action type")?;
    if action_type != "continue_conversation" {
        return Err("unsupported action type");
    }
    let label = obj
        .get("label")
        .and_then(|l| l.as_str())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .ok_or("missing label")?;
    if label.chars().count() > OPENUI_ACTION_LABEL_MAX_CHARS {
        return Err("label too long");
    }
    // CR-02: the label is sent as a chat message and the server runs a leading `/` as a command.
    if label.starts_with('/') {
        return Err("slash commands are not actions");
    }
    let form_state = match obj.get("form_state") {
        None => serde_json::json!({}),
        Some(v) if v.is_object() => v.clone(),
        Some(_) => return Err("form_state is not an object"),
    };
    Ok(OpenUiAction {
        action_type: action_type.to_string(),
        label: label.to_string(),
        form_state,
    })
}

/// The user turn an action becomes: the label, then the compact form state on the next line
/// when there is any.
pub(crate) fn action_turn_message(a: &OpenUiAction) -> String {
    match a.form_state.as_object() {
        Some(o) if !o.is_empty() => format!("{}\n{}", a.label, a.form_state),
        _ => a.label.clone(),
    }
}

/// UI-SPEC Part B "Component action": what was sent, then (while a Chat approval waits) where
/// to answer it.
pub(crate) fn action_status(label: &str, approval_waiting: bool) -> Vec<String> {
    let mut lines = vec![format!("Sent to the agent: {label}.")];
    if approval_waiting {
        lines.push("Waiting for your approval in Chat.".to_string());
    }
    lines
}

/// The chat request for one OpenUI turn (a Generate or an action): a plain user message on the
/// screen's own session with the overlay flag set.
pub(crate) fn openui_request(
    session_id: &str,
    message: String,
    gateway: bool,
    prompt: bool,
) -> ChatRequest {
    ChatRequest {
        session_id: session_id.to_string(),
        message,
        active_identity: None,
        attachment_ids: Vec::new(),
        openui: true,
        // D-09: the hosted Gateway only while the operator's toggle is on.
        openui_gateway: gateway,
        // RR-04: the operator's own Generate text, as opposed to a generated button's action.
        openui_prompt: prompt,
    }
}

/// The `contentWindow` of the sandboxed frame with this DOM id, if it is mounted (Phase 54.10:
/// the Dashboard hosts one frame per widget).
#[cfg(target_arch = "wasm32")]
pub(crate) fn frame_window_for(frame_id: &str) -> Option<web_sys::Window> {
    use wasm_bindgen::JsCast;
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(frame_id))
        .and_then(|e| e.dyn_into::<web_sys::HtmlIFrameElement>().ok())
        .and_then(|f| f.content_window())
}

/// The OpenUI screen's frame.
#[cfg(target_arch = "wasm32")]
fn frame_window() -> Option<web_sys::Window> {
    frame_window_for(OPENUI_FRAME_ID)
}

/// Read one window `message` event as an action: only when `event.source` IS this frame's
/// contentWindow (T-54.9-20; the sandbox origin is "null", so origin cannot be the check), then
/// the size cap and field rules of [`parse_openui_action`].
// shortcut: the payload is stringified before the size check; the sender is our own sandboxed
// frame, so this costs one JSON.stringify of at most what it chose to post. Upgrade if the
// frame ever hosts untrusted widgets that post large objects.
#[cfg(target_arch = "wasm32")]
fn action_from_message(ev: &web_sys::MessageEvent) -> Option<OpenUiAction> {
    let frame = frame_window()?;
    let source: wasm_bindgen::JsValue = ev.source()?.into();
    if source != wasm_bindgen::JsValue::from(frame) {
        return None;
    }
    let raw = js_sys::JSON::stringify(&ev.data()).ok()?.as_string()?;
    parse_openui_action(&raw).ok()
}

/// Post one command to the sandboxed frame. `target "*"` because an opaque-origin frame cannot
/// be addressed by origin (ADR-0007 "Sandbox and bridge"); the frame accepts a message only
/// when `event.source` is its parent.
#[cfg(target_arch = "wasm32")]
pub(crate) fn post_to_frame_id(frame_id: &str, cmd: &FrameCmd) {
    let Some(win) = frame_window_for(frame_id) else {
        return;
    };
    // The bootstrap ignores non-object payloads, so post the parsed object, not the text.
    if let Ok(obj) = js_sys::JSON::parse(&frame_message(cmd)) {
        let _ = win.post_message(&obj, "*");
    }
}

/// Native twin of [`post_to_frame_id`]: there is no frame to post to.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn post_to_frame_id(frame_id: &str, cmd: &FrameCmd) {
    let _ = (frame_id, cmd);
}

fn post_to_frame(cmd: &FrameCmd) {
    post_to_frame_id(OPENUI_FRAME_ID, cmd);
}

/// D-14: clicking a saved row loads its snapshot into the frame without calling the model.
pub(crate) fn snapshot_frame_cmds(snapshot: &str) -> Vec<FrameCmd> {
    vec![
        FrameCmd::Reset,
        FrameCmd::Push(snapshot.to_string()),
        FrameCmd::End,
    ]
}

/// UI-SPEC: the saved-view name defaults to the first 40 characters of the prompt.
pub(crate) fn default_view_name(prompt: &str) -> String {
    prompt.trim().chars().take(40).collect()
}

/// UI-SPEC `.screen-status` copy: `{n} saved views`, singular `1 saved view`.
pub(crate) fn saved_views_status(n: usize) -> String {
    if n == 1 {
        "1 saved view".to_string()
    } else {
        format!("{n} saved views")
    }
}

/// Coarse relative time for a saved row (`now` and `then` are unix seconds).
pub(crate) fn relative_time(now: i64, then: i64) -> String {
    let secs = now.saturating_sub(then).max(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

pub(crate) fn now_unix_secs() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() / 1000.0) as i64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// The saved-views panel body (UI-SPEC B1 "Saved views panel"): newest first, one `.row` per
/// view, or the empty state. Pure render: the screen owns the data and the handlers.
#[component]
fn SavedViews(
    views: Vec<OpenUiView>,
    now: i64,
    on_open: EventHandler<String>,
    on_rerun: EventHandler<String>,
    // Delete is an inline two-step confirm: `on_ask_delete` enters it, `on_delete` confirms,
    // `on_cancel_delete` leaves it. `confirming` is the id awaiting confirmation.
    confirming: Option<String>,
    on_ask_delete: EventHandler<String>,
    on_cancel_delete: EventHandler<()>,
    on_delete: EventHandler<String>,
) -> Element {
    if views.is_empty() {
        return rsx! {
            div { style: "padding:8px 0;",
                div { style: "font-size:12px; font-weight:700; color: var(--text);", "No generated views yet" }
                p { class: "row-sub", style: "margin-top:6px;",
                    "Describe a view below and the agent will build it. Saved views appear here."
                }
            }
        };
    }
    rsx! {
        div { style: "display:flex; flex-direction:column; gap:8px;",
            for view in views {
                {
                    let (open_id, rerun_id, delete_id) = (view.id.clone(), view.id.clone(), view.id.clone());
                    // A dashboard widget's view before its first generation: nothing to show.
                    let pending = view.snapshot.trim().is_empty();
                    if confirming.as_deref() == Some(view.id.as_str()) {
                        delete_confirm_row(&view, on_cancel_delete, on_delete)
                    } else { rsx! {
                        div {
                            key: "{view.id}",
                            class: "row",
                            style: "grid-template-columns: 1fr auto auto;",
                            onclick: move |_| {
                                if !pending {
                                    on_open.call(open_id.clone());
                                }
                            },
                            div { class: "row-main",
                                span { style: "font-size:12px; font-weight:700; color: var(--text); overflow:hidden; text-overflow:ellipsis; white-space:nowrap;",
                                    "{view.name}"
                                }
                                span { class: "row-sub",
                                    if pending {
                                        "Not generated"
                                    } else {
                                        "{relative_time(now, view.updated_at)}"
                                    }
                                }
                            }
                            button {
                                class: "btn btn--sm btn--ghost",
                                r#type: "button",
                                title: "Generate this view again from its saved prompt.",
                                onclick: move |e| {
                                    e.stop_propagation();
                                    on_rerun.call(rerun_id.clone());
                                },
                                "Re-run"
                            }
                            button {
                                class: "btn btn--sm btn--ghost",
                                r#type: "button",
                                onclick: move |e| {
                                    e.stop_propagation();
                                    on_ask_delete.call(delete_id.clone());
                                },
                                "Delete"
                            }
                        }
                    } }
                }
            }
        }
    }
}

/// The inline confirm that replaces a row while its delete awaits confirmation: `Keep view`
/// takes focus and Esc cancels.
fn delete_confirm_row(
    view: &OpenUiView,
    on_cancel: EventHandler<()>,
    on_delete: EventHandler<String>,
) -> Element {
    let id = view.id.clone();
    rsx! {
        div {
            key: "{view.id}",
            class: "row",
            style: "grid-template-columns: 1fr; cursor:default; gap:8px;",
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    on_cancel.call(());
                }
            },
            p { style: "font-size:11.5px; color: var(--text); margin:0;",
                "Delete view: Remove \"{view.name}\" from this profile? This cannot be undone."
            }
            div { style: "display:flex; gap:8px;",
                button {
                    class: "btn btn--sm btn--danger",
                    r#type: "button",
                    onclick: move |_| on_delete.call(id.clone()),
                    "Delete view"
                }
                button {
                    class: "btn btn--sm btn--ghost",
                    r#type: "button",
                    onmounted: move |e| async move {
                        let _ = e.set_focus(true).await;
                    },
                    onclick: move |_| on_cancel.call(()),
                    "Keep view"
                }
            }
        }
    }
}

/// D-16: what a finished Re-run does to the stored snapshot.
#[derive(Debug, PartialEq)]
pub(crate) enum RerunOutcome {
    Replace(String),
    Keep,
}

/// A Re-run replaces the stored snapshot only when the turn finished without error AND produced
/// non-empty text; anything else keeps the old snapshot.
pub(crate) fn rerun_outcome(finished_ok: bool, text: &str) -> RerunOutcome {
    if finished_ok && !text.trim().is_empty() {
        RerunOutcome::Replace(text.to_string())
    } else {
        RerunOutcome::Keep
    }
}

/// The only host CSS variables the frame accepts (mirrors `THEME_VARS` in `frame-bootstrap.js`).
pub(crate) const THEME_VARS: [&str; 10] = [
    "--bg",
    "--bg-card",
    "--text",
    "--text-dim",
    "--teal",
    "--border",
    "--green",
    "--red",
    "--amber",
    "--font",
];

// The theme post is wasm-only; a native build only exercises this item from tests.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
/// UI-SPEC B1 "Theming": the `openui-theme` payload (as JSON text). `vars` carries the host's
/// computed values; anything outside [`THEME_VARS`] or empty is dropped. `light` selects the
/// frame's palette (the stock stylesheet otherwise follows the OS, not the app theme).
pub(crate) fn theme_message(vars: &[(String, String)], light: bool) -> String {
    let vars: serde_json::Map<String, serde_json::Value> = vars
        .iter()
        .filter(|(k, v)| THEME_VARS.contains(&k.as_str()) && !v.trim().is_empty())
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.trim().to_string())))
        .collect();
    serde_json::json!({
        "type": "openui-theme",
        "mode": if light { "light" } else { "dark" },
        "vars": vars,
    })
    .to_string()
}

/// Prompt box height in rows: 3 minimum, growing with content to 8, then it scrolls. Counts
/// newlines plus a rough wrap estimate. shortcut: 90 columns assumed per row; the CSS
/// `max-height` + `overflow-y:auto` keeps the box bounded when the estimate is off.
pub(crate) fn prompt_rows(text: &str) -> usize {
    text.split('\n')
        .map(|l| 1 + l.chars().count() / 90)
        .sum::<usize>()
        .clamp(3, 8)
}

/// Post the host's current theme to the frame (wasm: reads the computed values of the
/// whitelisted variables on `<html>`).
#[cfg(target_arch = "wasm32")]
pub(crate) fn post_theme_to_frame_id(frame_id: &str, light: bool) {
    let Some(win) = frame_window_for(frame_id) else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(html) = window.document().and_then(|d| d.document_element()) else {
        return;
    };
    let Ok(Some(style)) = window.get_computed_style(&html) else {
        return;
    };
    let vars: Vec<(String, String)> = THEME_VARS
        .iter()
        .filter_map(|n| {
            style
                .get_property_value(n)
                .ok()
                .map(|v| ((*n).to_string(), v))
        })
        .collect();
    if let Ok(obj) = js_sys::JSON::parse(&theme_message(&vars, light)) {
        let _ = win.post_message(&obj, "*");
    }
}

/// Native twin of [`post_theme_to_frame_id`]: there is no frame.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn post_theme_to_frame_id(frame_id: &str, light: bool) {
    let _ = (frame_id, light);
}

fn post_theme_to_frame(light: bool) {
    post_theme_to_frame_id(OPENUI_FRAME_ID, light);
}

/// UI-SPEC B1 "Gateway row": the opt-in toggle, its fixed help line, the amber disclosure while
/// on, and the missing-key line when an attempt to turn it on was refused. Pure render.
#[component]
fn GatewayRow(
    on: bool,
    missing_key: bool,
    #[props(default)] disabled: bool,
    on_toggle: EventHandler<()>,
) -> Element {
    rsx! {
        div {
            div {
                class: "field-row",
                style: "grid-template-columns: 1fr auto; padding-top: 0;",
                div { class: "field-label",
                    "Use the Thesys hosted Gateway"
                    span { class: "help", "Off by default. Views are generated with your own providers." }
                }
                div {
                    class: if on { "tgl on" } else { "tgl" },
                    role: "switch",
                    tabindex: "0",
                    aria_checked: if on { "true" } else { "false" },
                    aria_label: "Use the Thesys hosted Gateway",
                    onclick: move |_| on_toggle.call(()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter || e.key() == Key::Character(" ".to_string()) {
                            e.prevent_default();
                            on_toggle.call(());
                        }
                    },
                }
            }
            if on {
                p { class: "row-sub", style: "color: var(--amber); margin: 4px 0 8px;",
                    "Your prompt and conversation content leave this machine and go to api.thesys.dev."
                }
            }
            if disabled {
                p { class: "row-sub", style: "color: var(--red); margin: 4px 0 8px;",
                    "The Gateway is disabled in config.yaml. Set ui.openui_gateway_enabled: true to allow it."
                }
            } else if missing_key {
                p { class: "row-sub", style: "color: var(--red); margin: 4px 0 8px;",
                    "No Thesys API key in the vault. Store THESYS_API_KEY first."
                }
            }
        }
    }
}

/// UI-SPEC Part B "Error state".
fn generate_error(reason: &str) -> String {
    format!(
        "Could not generate a view: {reason}. Check your provider on the Providers screen, then try again."
    )
}

/// Root-owned state of the OpenUI screen (provided at the HermesApp root by
/// [`use_openui_runtime`]). The screen reads and writes these signals; the socket and the
/// receive loop are owned by the root so they outlive the screen (CR-01).
#[derive(Clone, Copy)]
pub(crate) struct OpenUiCtx {
    /// Sends one JSON-serialized `ChatRequest` on the OpenUI socket.
    send: Callback<String>,
    session_id: Signal<String>,
    /// RR-06: set by the screen on its first mount; the root opens the socket only then and keeps
    /// it afterwards (CR-01), so an operator who never visits OpenUI never gets the second socket.
    connect: Signal<bool>,
    started: Signal<bool>,
    streaming: Signal<bool>,
    done: Signal<bool>,
    error: Signal<Option<String>>,
    doc: Signal<OpenUiDoc>,
    frame_loaded: Signal<bool>,
    last_action: Signal<Option<String>>,
    last_prompt: Signal<String>,
    save_name: Signal<String>,
    save_note: Signal<Option<Result<String, String>>>,
    rerun: Signal<Option<String>>,
    views_rev: Signal<u32>,
}

/// Called ONCE from `HermesApp` (the root never unmounts): opens the OpenUI websocket, runs its
/// receive loop and provides [`OpenUiCtx`]. A socket owned by the screen closed when the
/// operator navigated to Chat to answer an approval, and the server cancels a turn whose
/// socket closes. The approval map, transcript and id counter are the root's own signals.
pub(crate) fn use_openui_runtime(
    mut approval_cards: Signal<
        crate::components::hermes_app::screens::chat::approval_card::ApprovalCards,
    >,
    mut bubbles: Signal<Vec<crate::components::hermes_app::screens::chat::ChatBubble>>,
    mut next_id: Signal<u64>,
) {
    use crate::components::hermes_app::screens::chat::approval_card::{
        apply_approval_event, apply_disconnect_for_session,
    };

    let mut streaming = use_signal(|| false);
    let mut done = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut doc = use_signal(OpenUiDoc::default);
    let frame_loaded = use_signal(|| false);
    let mut rerun = use_signal(|| None::<String>);
    let mut views_rev = use_signal(|| 0u32);
    let mut save_note = use_signal(|| None::<Result<String, String>>);
    let session_id = use_signal(String::new);
    let started = use_signal(|| false);
    let connect = use_signal(|| false);
    let last_action = use_signal(|| None::<String>);
    let last_prompt = use_signal(String::new);
    let save_name = use_signal(String::new);

    // RR-06: lazy connect. The connection resource re-runs when `connect` flips (it is read inside
    // this closure); until then the future never resolves, so `ws.connect()` below just waits.
    let mut ws = dioxus_fullstack::use_websocket(move || {
        let wanted = *connect.read();
        async move {
            if !wanted {
                std::future::pending::<()>().await;
            }
            crate::server::ws::ws_chat(
                dioxus_fullstack::WebSocketOptions::new().with_automatic_reconnect(),
            )
            .await
        }
    });

    // Receive loop: no signal borrow is held across an `.await`.
    use_future(move || async move {
        let mut stream = OpenUiStream::default();
        loop {
            let _state = ws.connect().await;
            if ws.is_err() {
                streaming.set(false);
                continue;
            }
            loop {
                match ws.recv_raw().await {
                    Ok(dioxus_fullstack::Message::Text(t)) => {
                        let Ok(event) = serde_json::from_str::<ChatStreamEvent>(&t) else {
                            continue;
                        };
                        for cmd in stream.apply(&event) {
                            doc.with_mut(|d| d.apply(&cmd));
                            if *frame_loaded.peek() {
                                post_to_frame(&cmd);
                            }
                            if cmd == FrameCmd::End {
                                streaming.set(false);
                                done.set(true);
                                // D-16: a Re-run replaces its stored snapshot only on a clean
                                // finish with text; Error / cancel keep the old snapshot.
                                let pending = rerun.peek().clone();
                                if let Some(id) = pending {
                                    rerun.set(None);
                                    let ok = matches!(
                                        event,
                                        ChatStreamEvent::Finished { .. }
                                            | ChatStreamEvent::TurnEnded { .. }
                                    );
                                    let text = doc.peek().text().to_string();
                                    if let RerunOutcome::Replace(text) = rerun_outcome(ok, &text) {
                                        spawn(async move {
                                            if let Err(e) = replace_openui_snapshot(id, text).await
                                            {
                                                save_note.set(Some(Err(e.to_string())));
                                            }
                                            views_rev.with_mut(|r| *r += 1);
                                        });
                                    }
                                }
                            }
                        }
                        match &event {
                            ChatStreamEvent::Error { message, .. } => {
                                error.set(Some(generate_error(message)));
                                done.set(false);
                                streaming.set(false);
                            }
                            // D-15: the approval is raised by an OpenUI turn but lives in Chat
                            // (card) and on the Chat wedge (badge): apply it to the root map.
                            ChatStreamEvent::ApprovalRequested { .. }
                            | ChatStreamEvent::ApprovalResolved { .. } => {
                                let mut local_cards = approval_cards.peek().clone();
                                let mut local_next = *next_id.peek();
                                let changed =
                                    if matches!(event, ChatStreamEvent::ApprovalRequested { .. }) {
                                        apply_approval_event(
                                            &mut bubbles.write(),
                                            &mut local_next,
                                            &mut local_cards,
                                            &event,
                                        )
                                    } else {
                                        apply_approval_event(
                                            &mut Vec::new(),
                                            &mut local_next,
                                            &mut local_cards,
                                            &event,
                                        )
                                    };
                                if local_next != *next_id.peek() {
                                    next_id.set(local_next);
                                }
                                if changed {
                                    approval_cards.set(local_cards);
                                }
                            }
                            _ => {}
                        }
                    }
                    Ok(dioxus_fullstack::Message::Close { .. }) | Err(_) => {
                        streaming.set(false);
                        // RR-03: only the cards this socket's own session raised; Chat's stay.
                        let own = session_id.peek().clone();
                        let mut local_cards = approval_cards.peek().clone();
                        if apply_disconnect_for_session(&mut local_cards, &own) {
                            approval_cards.set(local_cards);
                        }
                        break;
                    }
                    _ => continue,
                }
            }
        }
    });

    let send = use_callback(move |json: String| {
        spawn(async move {
            let _ = ws.send_raw(dioxus_fullstack::Message::Text(json)).await;
        });
    });

    use_context_provider(|| OpenUiCtx {
        send,
        session_id,
        connect,
        started,
        streaming,
        done,
        error,
        doc,
        frame_loaded,
        last_action,
        last_prompt,
        save_name,
        save_note,
        rerun,
        views_rev,
    });
}

/// The OpenUI screen (`// OPENUI`).
#[component]
pub fn ScreenOpenUi(is_active: bool) -> Element {
    use crate::components::hermes_app::screens::chat::approval_card::{
        any_approval_pending, ApprovalCardsCtx,
    };

    // Root-provided state (HermesApp, `use_openui_runtime`): the socket, the running turn and
    // the stage content survive navigating away, so an approval answered in Chat still reaches
    // the OpenUI turn (CR-01).
    let ctx = use_context::<OpenUiCtx>();
    let mut session_id = ctx.session_id;
    let mut connect = ctx.connect;
    let mut started = ctx.started;
    let mut streaming = ctx.streaming;
    let mut done = ctx.done;
    let mut error = ctx.error;
    let mut doc = ctx.doc;
    let mut frame_loaded = ctx.frame_loaded;
    let mut last_action = ctx.last_action;
    let approval_cards = use_context::<ApprovalCardsCtx>().0;

    let mut prompt = use_signal(String::new);
    // The latest action from the host `message` listener, tagged with a sequence number so the
    // effect below handles each exactly once.
    let incoming = use_signal(|| None::<(u32, OpenUiAction)>);
    let mut handled = use_signal(|| 0u32);

    // D-14 saved views. The list re-fetches when the active profile changes (the root
    // TopbarProfileRefreshCtx tick) and after every save / delete.
    let profile_tick = use_context::<TopbarProfileRefreshCtx>().0;
    let mut views_rev = ctx.views_rev;
    let views_res = use_resource(move || async move {
        let _ = (*profile_tick.read(), *views_rev.read());
        list_openui_views().await.map_err(|e| e.to_string())
    });
    // The prompt behind the view on the stage, and the editable name for `Save view`.
    let mut last_prompt = ctx.last_prompt;
    let mut save_name = ctx.save_name;
    // `Ok("Saved.")` or `Err(reason)` from the last save / delete.
    let mut save_note = ctx.save_note;
    // D-16: the id of the view a running Re-run will replace on success; the id awaiting the
    // inline delete confirm.
    let mut rerun = ctx.rerun;
    let mut confirming = use_signal(|| None::<String>);

    // D-09: the hosted Gateway toggle, off by default. It can only turn on when a key is stored
    // (status is presence only); a refused attempt shows the missing-key copy and stays off.
    let mut gateway_on = use_signal(|| false);
    let mut gateway_refused = use_signal(|| false);
    // WR-04: which refusal to show — the server switch is off, or no key is stored.
    let mut gateway_disabled = use_signal(|| false);
    let gateway_status = use_resource(|| async move { openui_gateway_status().await.ok() });

    // UI-SPEC B1 "Theming": post the app theme to the frame once it has loaded and whenever the
    // theme changes. The read is deferred a tick so ThemeEffects has already rewritten
    // `<html data-theme>` and the accent inline vars.
    let theme = use_context::<crate::state::ThemeContext>().0;
    use_effect(move || {
        let light = crate::components::hermes_app::theme_effects::is_light_theme(&theme.read());
        if !*frame_loaded.read() {
            return;
        }
        spawn(async move {
            crate::platform::timer::sleep(50).await;
            post_theme_to_frame(light);
        });
    });

    // RR-06: the first visit asks the root to open the OpenUI socket (it stays open afterwards).
    use_effect(move || {
        if !*connect.peek() {
            connect.set(true);
        }
    });

    // This screen's own session (never the Chat session): created on the first visit and kept
    // across navigation.
    use_effect(move || {
        if !session_id.peek().is_empty() {
            return;
        }
        spawn(async move {
            if let Ok(sid) = crate::server::api::create_session().await {
                session_id.set(sid);
            }
        });
    });

    // The socket and receive loop live in `use_openui_runtime` at the HermesApp root.
    let send_json = ctx.send;

    // Send one request on the root-owned OpenUI socket.
    let mut send = move |req: crate::protocol::ChatRequest| {
        let Ok(json) = serde_json::to_string(&req) else {
            error.set(Some(generate_error("serialization error")));
            return;
        };
        send_json.call(json);
    };

    // Start one OpenUI turn with `message` on this screen's session: clear the frame, mark the
    // turn running, send. Generate and a generated button's action both come through here, so
    // each is an ordinary chat request and every tool, guardrail and approval gate applies.
    let mut start_turn = move |sid: String, message: String, is_prompt: bool| {
        error.set(None);
        done.set(false);
        streaming.set(true);
        started.set(true);
        doc.set(OpenUiDoc::default());
        if *frame_loaded.peek() {
            post_to_frame(&FrameCmd::Reset);
        }
        send(openui_request(&sid, message, *gateway_on.peek(), is_prompt));
    };
    let mut run_turn = move |message: String, is_prompt: bool| {
        let sid = session_id.peek().clone();
        if !sid.is_empty() {
            start_turn(sid, message, is_prompt);
            return;
        }
        // RR-09: the mount-time create failed (or has not finished): create the session now
        // instead of leaving every later Generate dead. `streaming` blocks a second click.
        error.set(None);
        streaming.set(true);
        spawn(async move {
            match crate::server::api::create_session().await {
                Ok(new) => {
                    // A concurrent create (the mount effect) may have won; keep that session.
                    let sid = if session_id.peek().is_empty() {
                        session_id.set(new.clone());
                        new
                    } else {
                        session_id.peek().clone()
                    };
                    start_turn(sid, message, is_prompt);
                }
                Err(_) => {
                    streaming.set(false);
                    error.set(Some(generate_error("the session is not ready yet")));
                }
            }
        });
    };

    let mut generate = move || {
        let text = prompt.peek().trim().to_string();
        if !can_generate(&text, *streaming.peek()) {
            return;
        }
        last_action.set(None);
        save_note.set(None);
        rerun.set(None);
        save_name.set(default_view_name(&text));
        last_prompt.set(text.clone());
        run_turn(text, true);
    };

    // D-14: load a saved snapshot into the stage; the model is not called. The frame mounts on
    // demand and `onload` replays `doc`.
    let mut open_view = move |snapshot: String| {
        if *streaming.peek() {
            return;
        }
        let cmds = snapshot_frame_cmds(&snapshot);
        doc.set(OpenUiDoc::default());
        for cmd in &cmds {
            doc.with_mut(|d| d.apply(cmd));
            if *frame_loaded.peek() {
                post_to_frame(cmd);
            }
        }
        error.set(None);
        done.set(false);
        started.set(true);
    };

    // D-15: a generated button press becomes a NEW agent turn on this session. An action that
    // arrives while a view is still generating is dropped (the buttons of a half-streamed view
    // are not final). Reads use `.peek()` so this effect subscribes only to `incoming`.
    use_effect(move || {
        let Some((seq, action)) = incoming.read().clone() else {
            return;
        };
        if seq <= *handled.peek() {
            return;
        }
        handled.set(seq);
        if *streaming.peek() {
            return;
        }
        last_action.set(Some(action.label.clone()));
        rerun.set(None);
        run_turn(action_turn_message(&action), false);
    });

    // The one host `message` listener (ADR-0007 "changed": ONE registration, removed on
    // unmount, never re-added on remount). Installed through the `listener_slot` + `use_drop`
    // idiom used by the wheel and chat; the callback only records the action, the effect above
    // does the sending inside a component scope.
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;
        type MessageListener = Closure<dyn FnMut(web_sys::MessageEvent)>;

        let mut incoming = incoming;
        let mut listener_slot: Signal<Option<MessageListener>> = use_signal(|| None);
        let seq = use_hook(|| std::rc::Rc::new(std::cell::Cell::new(0u32)));
        use_effect(move || {
            if listener_slot.peek().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let seq = seq.clone();
            let cb = MessageListener::new(move |ev: web_sys::MessageEvent| {
                if let Some(action) = action_from_message(&ev) {
                    seq.set(seq.get() + 1);
                    incoming.set(Some((seq.get(), action)));
                }
            });
            let _ = window.add_event_listener_with_callback("message", cb.as_ref().unchecked_ref());
            listener_slot.set(Some(cb));
        });
        use_drop(move || {
            // The frame element goes with the screen; the next mount reloads it and replays `doc`.
            frame_loaded.set(false);
            if let Some(cb) = listener_slot.write().take() {
                if let Some(window) = web_sys::window() {
                    let _ = window.remove_event_listener_with_callback(
                        "message",
                        cb.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }

    let stop = move |_| {
        let sid = session_id.peek().clone();
        send(crate::protocol::ChatRequest {
            session_id: sid,
            message: "/stop".to_string(),
            active_identity: None,
            attachment_ids: Vec::new(),
            openui: false,
            openui_gateway: false,
            openui_prompt: false,
        });
    };

    let is_streaming = *streaming.read();
    let is_done = *done.read();
    let enabled = can_generate(&prompt.read(), is_streaming);
    let err = error.read().clone();
    let approval_waiting = any_approval_pending(&approval_cards.read());
    let action_lines = last_action
        .read()
        .as_ref()
        .map(|label| action_status(label, approval_waiting))
        .unwrap_or_default();
    let views: Vec<OpenUiView> = match &*views_res.read() {
        Some(Ok(v)) => v.clone(),
        _ => Vec::new(),
    };
    let views_count = views.len();
    let can_save = is_done && err.is_none() && !doc.read().text().trim().is_empty();
    let note = save_note.read().clone();

    let save = move |_| {
        let name = save_name.peek().clone();
        let prompt = last_prompt.peek().clone();
        let snapshot = doc.peek().text().to_string();
        spawn(async move {
            match save_openui_view(name, prompt, snapshot).await {
                Ok(_) => {
                    save_note.set(Some(Ok("Saved.".to_string())));
                    views_rev.with_mut(|r| *r += 1);
                }
                Err(e) => save_note.set(Some(Err(e.to_string()))),
            }
        });
    };
    let delete = move |id: String| {
        confirming.set(None);
        spawn(async move {
            if let Err(e) = delete_openui_view(id).await {
                save_note.set(Some(Err(e.to_string())));
            }
            views_rev.with_mut(|r| *r += 1);
        });
    };
    let open_saved = {
        let views = views.clone();
        move |id: String| {
            if let Some(v) = views
                .iter()
                .find(|v| v.id == id && !v.snapshot.trim().is_empty())
            {
                open_view(v.snapshot.clone());
            }
        }
    };
    // D-16: Re-run regenerates from the saved prompt on this socket (same path as Generate); the
    // stored snapshot is replaced only when the turn ends cleanly (see the receive loop).
    let rerun_view = {
        let views = views.clone();
        move |id: String| {
            if *streaming.peek() {
                return;
            }
            let Some(v) = views.iter().find(|v| v.id == id) else {
                return;
            };
            last_action.set(None);
            save_note.set(None);
            save_name.set(v.name.clone());
            last_prompt.set(v.prompt.clone());
            prompt.set(v.prompt.clone());
            rerun.set(Some(id));
            run_turn(v.prompt.clone(), true);
        }
    };

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-openui",
            "data-screen-label": "OpenUI",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// OPENUI" }
                    h1 { class: "screen-title", "OpenUI" }
                    span { class: "screen-status", "· {saved_views_status(views_count)}" }
                }
            }
            div { class: "split left-wider",
                div { class: "panel",
                    GatewayRow {
                        on: gateway_on(),
                        missing_key: gateway_refused() && !gateway_on(),
                        disabled: gateway_refused() && gateway_disabled() && !gateway_on(),
                        on_toggle: move |_| {
                            if *gateway_on.peek() {
                                gateway_on.set(false);
                                gateway_refused.set(false);
                                return;
                            }
                            let status = gateway_status.read().clone().flatten();
                            match status {
                                Some(s) if s.enabled && s.key_present => {
                                    gateway_refused.set(false);
                                    gateway_on.set(true);
                                }
                                Some(s) if !s.enabled => {
                                    gateway_disabled.set(true);
                                    gateway_refused.set(true);
                                }
                                _ => {
                                    gateway_disabled.set(false);
                                    gateway_refused.set(true);
                                }
                            }
                        },
                    }
                    textarea {
                        class: "field-input",
                        id: "ih-openui-prompt",
                        rows: "{prompt_rows(&prompt.read())}",
                        // Grows from 3 to 8 rows (prompt_rows), then scrolls; no character cap.
                        style: "max-height: 14em; overflow-y: auto; resize: vertical;",
                        placeholder: PROMPT_PLACEHOLDER,
                        readonly: is_streaming,
                        value: "{prompt}",
                        oninput: move |e| prompt.set(e.value()),
                        onkeydown: move |e| {
                            let m = e.modifiers();
                            if e.key() == Key::Enter && (m.meta() || m.ctrl()) {
                                e.prevent_default();
                                generate();
                            }
                        },
                    }
                    div {
                        style: "display:flex; justify-content:flex-end; align-items:center; gap:12px; margin-top:8px;",
                        if is_streaming {
                            span { class: "row-sub", style: "color: var(--text-dim);", "Generating…" }
                        } else if is_done {
                            span { class: "row-sub", style: "color: var(--text-dim);", "Generated" }
                        }
                        if is_streaming {
                            button {
                                class: "btn btn--ghost",
                                r#type: "button",
                                onclick: stop,
                                "Stop"
                            }
                        } else {
                            button {
                                class: "btn",
                                r#type: "button",
                                disabled: !enabled,
                                onclick: move |_| generate(),
                                "Generate view"
                            }
                        }
                    }
                    if can_save {
                        div { style: "display:flex; justify-content:flex-end; align-items:center; gap:8px; margin-top:8px;",
                            input {
                                class: "field-input",
                                r#type: "text",
                                style: "max-width:320px;",
                                maxlength: "80",
                                aria_label: "View name",
                                value: "{save_name}",
                                oninput: move |e| save_name.set(e.value()),
                            }
                            button {
                                class: "btn btn--ghost",
                                r#type: "button",
                                onclick: save,
                                "Save view"
                            }
                        }
                    }
                    match note {
                        Some(Ok(msg)) => rsx! {
                            p { class: "row-sub", style: "color: var(--green); margin-top:8px;", "{msg}" }
                        },
                        Some(Err(msg)) => rsx! {
                            p { class: "row-sub", style: "color: var(--red); margin-top:8px;", "{msg}" }
                        },
                        None => rsx! {},
                    }
                    if let Some(msg) = err {
                        p { class: "row-sub", style: "color: var(--red); margin-top:8px;", "{msg}" }
                    }
                    // The label is shown as a text node only (never markup).
                    for line in action_lines {
                        p { class: "row-sub", style: "margin-top:8px;", "{line}" }
                    }
                    div {
                        style: "margin-top:12px; background: var(--bg-card); border: 1px solid var(--border); min-height: 480px; display:flex; align-items:center; justify-content:center;",
                        if *started.read() {
                            // Allow-scripts only, no other sandbox token: opaque origin, no
                            // cookies, DOM or storage of the app are reachable from the frame.
                            iframe {
                                id: OPENUI_FRAME_ID,
                                // Quoted: dioxus-html 0.7.7 has no typed `sandbox` attribute
                                // (same workaround as artifact_viewer.rs).
                                "sandbox": "allow-scripts",
                                src: "/openui/frame",
                                title: "Generated view",
                                style: "width:100%; min-height:480px; border:0; align-self:stretch;",
                                onload: move |_| {
                                    frame_loaded.set(true);
                                    for cmd in doc.peek().replay() {
                                        post_to_frame(&cmd);
                                    }
                                },
                            }
                        } else {
                            span { style: "color: var(--text-dim);", "{STAGE_EMPTY}" }
                        }
                    }
                }
                // UI-SPEC B1 "Saved views panel": scrolls internally, capped near the stage height.
                div { class: "panel", style: "align-self:start; max-height:80vh; overflow-y:auto;",
                    SavedViews {
                        views,
                        now: now_unix_secs(),
                        on_open: open_saved,
                        on_rerun: rerun_view,
                        confirming: confirming.read().clone(),
                        on_ask_delete: move |id| confirming.set(Some(id)),
                        on_cancel_delete: move |_| confirming.set(None),
                        on_delete: delete,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ChatStreamEvent as E;

    const SRC: &str = include_str!("openui.rs");

    fn nil() -> uuid::Uuid {
        uuid::Uuid::nil()
    }

    #[test]
    fn stream_sends_only_text_after_last_tool_call() {
        let mut s = OpenUiStream::default();
        let evs = [
            E::Delta {
                turn_id: nil(),
                text: "thinking".into(),
            },
            E::ToolCallStart {
                turn_id: nil(),
                name: "cronjob".into(),
            },
            E::ToolCallEnd {
                turn_id: nil(),
                name: "cronjob".into(),
                success: true,
            },
            E::Delta {
                turn_id: nil(),
                text: "root = Card(".into(),
            },
            E::Delta {
                turn_id: nil(),
                text: "...)".into(),
            },
            E::Finished {
                turn_id: nil(),
                total_tokens: 1,
            },
            E::TurnEnded {
                turn_id: nil(),
                session_id: "s".into(),
            },
        ];
        let got: Vec<FrameCmd> = evs.iter().flat_map(|e| s.apply(e)).collect();
        assert_eq!(
            got,
            vec![
                FrameCmd::Push("thinking".into()),
                FrameCmd::Reset,
                FrameCmd::Push("root = Card(".into()),
                FrameCmd::Push("...)".into()),
                FrameCmd::End,
            ],
            "End is emitted once even though Finished and TurnEnded both close the turn"
        );
    }

    #[test]
    fn stream_ignores_other_turns_once_one_is_known() {
        let mut s = OpenUiStream::default();
        let mine = uuid::Uuid::from_u128(1);
        let other = uuid::Uuid::from_u128(2);
        s.apply(&E::TurnStarted {
            turn_id: mine,
            session_id: "s".into(),
            index: 0,
        });
        assert!(s
            .apply(&E::Delta {
                turn_id: other,
                text: "x".into()
            })
            .is_empty());
        assert_eq!(
            s.apply(&E::Delta {
                turn_id: mine,
                text: "y".into()
            }),
            vec![FrameCmd::Push("y".into())]
        );
        assert_eq!(
            s.apply(&E::Error {
                turn_id: mine,
                message: "boom".into()
            }),
            vec![FrameCmd::End]
        );
    }

    #[test]
    fn frame_messages_are_the_bridge_shapes() {
        let parse =
            |c: FrameCmd| serde_json::from_str::<serde_json::Value>(&frame_message(&c)).unwrap();
        assert_eq!(
            parse(FrameCmd::Push("x".into())),
            serde_json::json!({"type":"openui-push","text":"x"})
        );
        assert_eq!(
            parse(FrameCmd::Reset),
            serde_json::json!({"type":"openui-reset"})
        );
        assert_eq!(
            parse(FrameCmd::End),
            serde_json::json!({"type":"openui-end"})
        );
    }

    #[test]
    fn late_loading_frame_is_replayed_to_the_current_state() {
        let mut d = OpenUiDoc::default();
        d.apply(&FrameCmd::Push("a".into()));
        d.apply(&FrameCmd::Reset);
        d.apply(&FrameCmd::Push("root = ".into()));
        d.apply(&FrameCmd::Push("Card()".into()));
        assert_eq!(d.replay(), vec![FrameCmd::Push("root = Card()".into())]);
        d.apply(&FrameCmd::End);
        assert_eq!(
            d.replay(),
            vec![FrameCmd::Push("root = Card()".into()), FrameCmd::End]
        );
    }

    #[test]
    fn generate_needs_a_non_blank_prompt_and_no_running_turn() {
        assert!(!can_generate("", false));
        assert!(!can_generate("  \n ", false));
        assert!(can_generate("a table", false));
        assert!(!can_generate("a table", true));
    }

    fn ssr(root: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    fn with_root_contexts() -> Element {
        let cards = use_signal(Default::default);
        use_context_provider(|| crate::components::hermes_app::ApprovalCardsCtx(cards));
        let bubbles =
            use_signal(Vec::<crate::components::hermes_app::screens::chat::ChatBubble>::new);
        use_context_provider(|| bubbles);
        let next = use_signal(|| 0u64);
        use_context_provider(|| crate::state::NextIdContext(next));
        use_openui_runtime(cards, bubbles, next);
        let tick = use_signal(|| 0u32);
        use_context_provider(|| TopbarProfileRefreshCtx(tick));
        let theme = use_signal(|| "slate-dark".to_string());
        use_context_provider(|| crate::state::ThemeContext(theme));
        rsx! { ScreenOpenUi { is_active: true } }
    }

    #[test]
    fn openui_screen_empty_state_and_disabled_generate() {
        let html = ssr(with_root_contexts);
        assert!(html.contains("// OPENUI"), "{html}");
        assert!(html.contains(">OpenUI<"), "{html}");
        assert!(
            html.contains(&PROMPT_PLACEHOLDER.replace('\'', "&#39;")),
            "{html}"
        );
        assert!(html.contains("Your generated view appears here."), "{html}");
        assert!(html.contains("id=\"screen-openui\""), "{html}");
        let btn = html.find("Generate view").expect("Generate view button");
        let open = html[..btn].rfind("<button").expect("button tag");
        assert!(
            html[open..btn].contains("disabled"),
            "Generate view starts disabled: {html}"
        );
        assert!(
            !html.contains("<iframe"),
            "no iframe before the first Generate: {html}"
        );
    }

    #[test]
    fn row_click_posts_snapshot_as_reset_push_end() {
        assert_eq!(
            snapshot_frame_cmds("root = Card(...)"),
            vec![
                FrameCmd::Reset,
                FrameCmd::Push("root = Card(...)".into()),
                FrameCmd::End
            ]
        );
        // The same commands replay into a late-loading frame as the finished snapshot.
        let mut d = OpenUiDoc::default();
        for c in snapshot_frame_cmds("root = Card(...)") {
            d.apply(&c);
        }
        assert_eq!(d.text(), "root = Card(...)");
    }

    #[test]
    fn saved_view_helpers() {
        assert_eq!(saved_views_status(0), "0 saved views");
        assert_eq!(saved_views_status(1), "1 saved view");
        assert_eq!(saved_views_status(3), "3 saved views");
        assert_eq!(
            default_view_name(&format!("  {}", "p".repeat(60))).len(),
            40
        );
        assert_eq!(relative_time(100, 100), "just now");
        assert_eq!(relative_time(1000, 100), "15 min ago");
        assert_eq!(relative_time(100_000, 0), "1 d ago");
        assert_eq!(relative_time(7200, 0), "2 h ago");
    }

    fn one_view() -> Element {
        let v = OpenUiView {
            id: "11111111-1111-1111-1111-111111111111".into(),
            name: "Cron failures".into(),
            prompt: "a table".into(),
            snapshot: "root = Card([])".into(),
            created_at: 0,
            updated_at: 0,
        };
        rsx! {
            SavedViews {
                views: vec![v],
                now: 120,
                on_open: |_| {},
                on_rerun: |_| {},
                confirming: None::<String>,
                on_ask_delete: |_| {},
                on_cancel_delete: |_| {},
                on_delete: |_| {},
            }
        }
    }

    #[test]
    fn saved_views_panel_states() {
        let empty = ssr(with_root_contexts);
        assert!(empty.contains("No generated views yet"), "{empty}");
        assert!(
            empty.contains(
                "Describe a view below and the agent will build it. Saved views appear here."
            ),
            "{empty}"
        );
        assert!(empty.contains("screen-status"), "{empty}");
        assert!(empty.contains("0 saved views"), "{empty}");
        assert!(
            !empty.contains("Save view"),
            "nothing to save before a generation"
        );

        let one = ssr(one_view);
        assert!(one.contains("Cron failures"), "{one}");
        assert!(one.contains("2 min ago"), "{one}");
        assert!(one.contains("Re-run") && one.contains("Delete"), "{one}");
        assert!(!one.contains("No generated views yet"), "{one}");
    }

    #[test]
    fn saved_views_row_marks_pending() {
        fn pending_view() -> Element {
            let v = OpenUiView {
                id: "22222222-2222-2222-2222-222222222222".into(),
                name: "Weekly spend".into(),
                prompt: "a weekly spend table".into(),
                snapshot: String::new(),
                created_at: 0,
                updated_at: 0,
            };
            rsx! {
                SavedViews {
                    views: vec![v],
                    now: 600,
                    on_open: |_| {},
                    on_rerun: |_| {},
                    confirming: None::<String>,
                    on_ask_delete: |_| {},
                    on_cancel_delete: |_| {},
                    on_delete: |_| {},
                }
            }
        }
        let html = ssr(pending_view);
        assert!(html.contains("Weekly spend"), "{html}");
        assert!(html.contains("Not generated"), "{html}");
        assert!(!html.contains("min ago"), "{html}");
        assert!(html.contains("Delete"), "{html}");
        // Its click never loads an empty snapshot into the frame.
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        assert!(code.contains("if !pending {"));
        assert!(code.contains("&& !v.snapshot.trim().is_empty()"));
    }

    #[test]
    fn rerun_replaces_only_on_success() {
        assert_eq!(rerun_outcome(true, "x"), RerunOutcome::Replace("x".into()));
        assert_eq!(rerun_outcome(false, "x"), RerunOutcome::Keep);
        assert_eq!(rerun_outcome(true, ""), RerunOutcome::Keep);
        assert_eq!(rerun_outcome(true, "  \n"), RerunOutcome::Keep);
    }

    fn confirm_row() -> Element {
        let v = OpenUiView {
            id: "11111111-1111-1111-1111-111111111111".into(),
            name: "Cron failures".into(),
            prompt: "a table".into(),
            snapshot: "root = Card([])".into(),
            created_at: 0,
            updated_at: 0,
        };
        rsx! {
            SavedViews {
                views: vec![v],
                now: 0,
                on_open: |_| {},
                on_rerun: |_| {},
                confirming: Some("11111111-1111-1111-1111-111111111111".to_string()),
                on_ask_delete: |_| {},
                on_cancel_delete: |_| {},
                on_delete: |_| {},
            }
        }
    }

    #[test]
    fn delete_needs_two_steps() {
        // Step 1: the row only offers `Delete`; nothing is removed by that click (the handler
        // is `on_ask_delete`, and `on_delete` is reachable only from the confirm row).
        let first = ssr(one_view);
        assert!(!first.contains("Keep view"), "{first}");
        // Step 2: the confirm state.
        let html = ssr(confirm_row);
        assert!(
            html.contains(
                "Delete view: Remove &#34;Cron failures&#34; from this profile? This cannot be undone."
            ),
            "{html}"
        );
        assert!(
            html.contains(">Delete view<") && html.contains(">Keep view<"),
            "{html}"
        );
        assert!(html.contains("btn--danger"), "{html}");
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        assert!(code.contains("on_ask_delete.call(delete_id.clone())"));
        assert!(code.contains("Key::Escape"));
    }

    #[test]
    fn generate_error_copy() {
        assert_eq!(
            generate_error("timeout"),
            "Could not generate a view: timeout. Check your provider on the Providers screen, then try again."
        );
    }

    #[test]
    fn theme_message_has_only_whitelisted_vars() {
        let vars: Vec<(String, String)> = THEME_VARS
            .iter()
            .filter(|n| **n != "--text-dim")
            .map(|n| (n.to_string(), format!("v{n}")))
            .chain([
                ("--evil".to_string(), "x".to_string()),
                ("--bg-canvas".to_string(), "y".to_string()),
                ("--text-dim".to_string(), "  ".to_string()),
            ])
            .collect();
        let v: serde_json::Value = serde_json::from_str(&theme_message(&vars, true)).unwrap();
        assert_eq!(v["type"], "openui-theme");
        assert_eq!(v["mode"], "light");
        let obj = v["vars"].as_object().unwrap();
        for k in obj.keys() {
            assert!(THEME_VARS.contains(&k.as_str()), "{k} is not whitelisted");
        }
        // The blank `--text-dim` is dropped, the other nine whitelisted names survive.
        assert_eq!(obj.len(), 9);
        let dark: serde_json::Value = serde_json::from_str(&theme_message(&[], false)).unwrap();
        assert_eq!(dark["mode"], "dark");
    }

    #[test]
    fn prompt_rows_grow_from_3_to_8() {
        assert_eq!(prompt_rows(""), 3);
        assert_eq!(prompt_rows("a\nb\nc\nd\ne"), 5);
        assert_eq!(prompt_rows(&"a\n".repeat(30)), 8);
        assert_eq!(prompt_rows(&"x".repeat(2000)), 8);
    }

    fn gateway_row(on: bool, missing: bool) -> impl Fn() -> Element {
        move || {
            rsx! {
                GatewayRow { on, missing_key: missing, on_toggle: |_| {} }
            }
        }
    }

    #[test]
    fn gateway_toggle_copy_and_default_off() {
        // The screen itself: off by default, fixed label and help, no disclosure.
        let screen = ssr(with_root_contexts);
        assert!(screen.contains("Use the Thesys hosted Gateway"), "{screen}");
        assert!(
            screen.contains("Off by default. Views are generated with your own providers."),
            "{screen}"
        );
        assert!(
            screen.contains("class=\"tgl\""),
            "toggle starts off: {screen}"
        );
        assert!(!screen.contains("api.thesys.dev"), "{screen}");
        assert!(!screen.contains("No Thesys API key"), "{screen}");

        let on = ssr(gateway_row_on);
        assert!(on.contains("tgl on"), "{on}");
        assert!(
            on.contains(
                "Your prompt and conversation content leave this machine and go to api.thesys.dev."
            ),
            "{on}"
        );
        assert!(on.contains("color: var(--amber)"), "{on}");

        let refused = ssr(gateway_row_missing);
        assert!(
            refused.contains("No Thesys API key in the vault. Store THESYS_API_KEY first."),
            "{refused}"
        );
        assert!(
            refused.contains("class=\"tgl\""),
            "toggle stays off: {refused}"
        );
        assert!(!refused.contains("api.thesys.dev"), "{refused}");

        // Requests carry the flag only while the toggle is on.
        assert!(!openui_request("s", "m".into(), false, true).openui_gateway);
        assert!(openui_request("s", "m".into(), true, true).openui_gateway);
    }

    #[test]
    fn gateway_row_disabled_in_config_shows_config_copy() {
        let html = ssr(|| rsx! { GatewayRow { on: false, missing_key: false, disabled: true, on_toggle: |_| {} } });
        assert!(html.contains("ui.openui_gateway_enabled: true"), "{html}");
        assert!(!html.contains("No Thesys API key"), "{html}");
        assert!(html.contains("class=\"tgl\""), "{html}");
    }

    #[test]
    fn ui_openui_gateway_enabled_defaults_off_and_parses() {
        let off: ironhermes_core::config::UiConfig = serde_yaml::from_str("{}").unwrap();
        assert!(!off.openui_gateway_enabled);
        assert!(off.is_empty());
        let on: ironhermes_core::config::UiConfig =
            serde_yaml::from_str("openui_gateway_enabled: true").unwrap();
        assert!(on.openui_gateway_enabled);
        assert!(!on.is_empty(), "an enabled switch must survive save");
    }

    fn gateway_row_on() -> Element {
        gateway_row(true, false)()
    }

    fn gateway_row_missing() -> Element {
        gateway_row(false, true)()
    }

    fn bridge(extra: &str) -> String {
        format!(
            r#"{{"type":"openui-action","action_type":"continue_conversation","label":"do the thing"{extra}}}"#
        )
    }

    #[test]
    fn parse_action_accepts_the_bridge_shape() {
        let a = parse_openui_action(&bridge(r#","form_state":{}"#)).expect("valid action");
        assert_eq!(a.label, "do the thing");
        assert_eq!(a.action_type, "continue_conversation");
        assert_eq!(a.form_state, serde_json::json!({}));
        // form_state may be absent; a 500-char label is the longest accepted.
        assert!(parse_openui_action(&bridge("")).is_ok());
        let long = format!(
            r#"{{"type":"openui-action","action_type":"continue_conversation","label":"{}"}}"#,
            "x".repeat(500)
        );
        assert!(parse_openui_action(&long).is_ok());
    }

    #[test]
    fn parse_action_rejects_slash_commands() {
        for label in ["/yolo 5", "/new", "  /flush"] {
            let raw = format!(
                r#"{{"type":"openui-action","action_type":"continue_conversation","label":"{label}"}}"#
            );
            assert!(parse_openui_action(&raw).is_err(), "{label}");
        }
    }

    #[test]
    fn parse_action_rejects_bad_input() {
        let big = format!(
            r#"{{"type":"openui-action","action_type":"continue_conversation","label":"a","form_state":{{"k":"{}"}}}}"#,
            "v".repeat(OPENUI_ACTION_MAX_BYTES)
        );
        assert!(big.len() > OPENUI_ACTION_MAX_BYTES);
        let too_long = format!(
            r#"{{"type":"openui-action","action_type":"continue_conversation","label":"{}"}}"#,
            "x".repeat(501)
        );
        let cases: Vec<(&str, String)> = vec![
            ("oversize", big),
            (
                "wrong type",
                bridge("").replace("openui-action", "openui-push"),
            ),
            (
                "other action",
                bridge("").replace("continue_conversation", "open_url"),
            ),
            (
                "no label",
                r#"{"type":"openui-action","action_type":"continue_conversation"}"#.into(),
            ),
            ("empty label", bridge("").replace("do the thing", "")),
            ("blank label", bridge("").replace("do the thing", "   ")),
            ("label over 500", too_long),
            ("form_state array", bridge(r#","form_state":[1]"#)),
            ("form_state null", bridge(r#","form_state":null"#)),
            ("not json", "{nope".into()),
            ("not an object", "[1,2]".into()),
        ];
        for (name, raw) in cases {
            assert!(
                parse_openui_action(&raw).is_err(),
                "{name} must be rejected"
            );
        }
    }

    #[test]
    fn action_turn_text_is_label_plus_form_state() {
        let a = parse_openui_action(&bridge(r#","form_state":{"n":2}"#)).unwrap();
        assert_eq!(action_turn_message(&a), "do the thing\n{\"n\":2}");
        let empty = parse_openui_action(&bridge(r#","form_state":{}"#)).unwrap();
        assert_eq!(action_turn_message(&empty), "do the thing");
    }

    #[test]
    fn action_becomes_a_plain_turn() {
        let a = parse_openui_action(&bridge("")).unwrap();
        let req = openui_request("sess-1", action_turn_message(&a), false, false);
        assert!(req.openui, "an action re-attaches the OpenUI overlay");
        assert!(!req.openui_prompt, "an action is not the operator's typed prompt");
        assert!(openui_request("s", "m".into(), false, true).openui_prompt);
        assert_eq!(req.session_id, "sess-1");
        assert_eq!(req.message, "do the thing");
        assert!(req.attachment_ids.is_empty() && req.active_identity.is_none());

        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        // T-54.9-22: nothing here resolves an approval, and the action goes out as a chat request.
        let resolver = ["resolve_web_", "approval"].concat();
        assert!(
            !code.contains(&resolver),
            "an action never resolves an approval"
        );
        assert!(code.contains("run_turn(action_turn_message(&action), false)"));
        // One host listener, removed on unmount.
        assert_eq!(code.matches("add_event_listener_with_callback").count(), 1);
        assert!(code.contains("remove_event_listener_with_callback"));
        assert!(code.contains("content_window()"));
    }

    #[test]
    fn openui_socket_drop_only_disconnects_its_own_session_cards() {
        // RR-03: the shared root map also holds Chat's cards.
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        assert!(code.contains("apply_disconnect_for_session(&mut local_cards, &own)"));
        assert!(!code.contains(&["apply_disconnect", "(&mut"].concat()));
    }

    #[test]
    fn generate_creates_the_session_lazily_when_it_is_missing() {
        // RR-09: a failed mount-time create_session must not leave the screen dead.
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        let run = code.find("let mut run_turn = move").expect("run_turn");
        let body = &code[run..code[run..].find("let mut generate").unwrap() + run];
        assert!(body.contains("create_session().await"), "{body}");
        assert!(body.contains("start_turn(sid, message, is_prompt)"));
    }

    #[test]
    fn openui_socket_connects_lazily_on_first_screen_visit() {
        // RR-06: the root owns the socket but only opens it once the screen asks.
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        let runtime = &code[code.find("pub(crate) fn use_openui_runtime").expect("runtime")
            ..code.find("pub fn ScreenOpenUi").expect("screen")];
        assert!(runtime.contains("let wanted = *connect.read();"));
        assert!(runtime.contains("std::future::pending::<()>().await"));
        let screen = &code[code.find("pub fn ScreenOpenUi").expect("screen")..];
        assert!(screen.contains("connect.set(true)"));
    }

    #[test]
    fn openui_socket_is_owned_by_the_root_not_the_screen() {
        // CR-01: a socket opened inside the screen closes when the operator leaves it.
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        let screen = &code[code.find("pub fn ScreenOpenUi").expect("screen")..];
        let hook = ["use_", "websocket("].concat();
        assert!(!screen.contains(&hook), "the screen must not open a socket");
        assert_eq!(code.matches(&hook).count(), 1, "one socket, in use_openui_runtime");
    }

    #[test]
    fn action_status_copy() {
        assert_eq!(
            action_status("do the thing", false),
            vec!["Sent to the agent: do the thing.".to_string()]
        );
        assert_eq!(
            action_status("do the thing", true),
            vec![
                "Sent to the agent: do the thing.".to_string(),
                "Waiting for your approval in Chat.".to_string()
            ]
        );
    }

    #[test]
    fn openui_iframe_sandbox_is_allow_scripts_only() {
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        assert!(code.contains("\"sandbox\": \"allow-scripts\""));
        // Forbidden tokens are assembled from parts so this test module does not contain them.
        for tail in [
            "same-origin",
            "forms",
            "modals",
            "popups",
            "top-navigation",
            "downloads",
            "pointer-lock",
        ] {
            let token = ["allow", "-", tail].concat();
            assert!(!code.contains(&token), "no {token} sandbox token");
        }
        assert_eq!(
            code.matches("allow-").count(),
            code.matches("allow-scripts").count()
        );
    }
}
