//! Phase 54.4 Plan 05 (D-10, D-12, D-21, D-22, D-23) — the root-level agent
//! tab model.
//!
//! `AgentTabsState { open, focused }` is the set of open agent tabs and which
//! one is focused. It lives in a root-provided [`AgentTabsCtx`] and is kept
//! deliberately SEPARATE from the chat singletons (D-10): no reducer or focus
//! path here reads or writes the transcript, the chat session or the
//! WebSocket, so focusing a tab is display-only (D-21) — it re-sources the
//! header strip and the env chip, never the running chat. The struct is plain
//! data so phase 54.6 can attach per-tab sessions to the same state without
//! undoing anything here.
//!
//! The root agent is the reserved tab id [`DEFAULT_AGENT`] (D-22); it is the
//! first Agent dropdown row and the first-run layout opens it.
//!
//! [`focus_agent`] is the one entry point every picker uses. It applies the
//! focus FIRST and only then makes the best-effort activation write (D-23):
//! a failed write never reverts or blocks the focus, it only raises the
//! non-blocking notice.
//!
//! The three context newtypes are provided ONLY in `HermesApp`'s own body
//! (`hermes_app/mod.rs`). A provider declared in a child compiles and then
//! panics consumers outside its subtree at runtime, so this module declares
//! none.

use crate::components::hermes_app::screens::profile_shared::create_dialog::validate_name_client_side;
use crate::protocol::ActivationScope;
use crate::state::Screen;
use crate::server::profile_activation_api::{activate_profile, clear_active_profile};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// The reserved tab id of the root agent (D-22).
pub const DEFAULT_AGENT: &str = "default";

/// Hard cap on open tabs, enforced on open and again on hydration (D-15).
pub const MAX_TABS: usize = 16;

/// Open agent tabs plus the focused one. Persisted as the `ih.ui.dashboard`
/// blob (D-15): agent slugs only, never session ids or secrets. Later plans
/// add fields; `#[serde(default)]` keeps older blobs loading.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentTabsState {
    pub open: Vec<String>,
    pub focused: Option<String>,
    /// Parked tabs (D-14): a subset of `open` shown as cards in the widget
    /// column instead of in the tab strip. Invariants: `parked` is a subset of
    /// `open`, and `focused` is never in `parked`.
    pub parked: Vec<String>,
    /// Phase 54.4 Plan 11 (D-14/D-15): whether the Logs mini-panel is shown in
    /// the Chat widget column. Persisted with the layout; older blobs without
    /// the key load as `false` (struct-level `serde(default)`).
    pub logs_widget: bool,
    /// Phase 54.4 Plan 11 (D-14): set while the header's Logs shortcut is
    /// being dragged toward the Chat drop zone. TRANSIENT (`serde(skip)`): a
    /// drag cannot survive a reload, and the Chat drop zone acts only while
    /// this in-app flag is set, never on a foreign drop (T-54.4-37).
    #[serde(skip)]
    pub dragging_logs_widget: bool,
    /// Phase 54.4 Plan 09 (D-16): the per-agent settings slide-over's
    /// TRANSIENT state. Never persisted (`serde(skip)`): a reload starts with
    /// the panel closed, like every other unsaved form in the app.
    #[serde(skip)]
    pub panel: PanelState,
}

/// Transient state of the per-agent settings slide-over (Phase 54.4 Plan 09).
/// `settings_for` is the agent whose panel is open, `dirty` mirrors whether
/// that panel holds unsaved edits (the panel component owns the working copy
/// and writes this flag), and `pending` is a guarded action waiting on the
/// operator's confirmation.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct PanelState {
    pub settings_for: Option<String>,
    pub dirty: bool,
    pub pending: Option<PanelPending>,
}

/// An action the dirty guard is holding until the operator confirms or
/// cancels it (UI-SPEC Interaction Rules 2 and 4).
#[derive(Clone, PartialEq, Debug)]
pub enum PanelPending {
    /// Focus (retarget the panel to) another agent.
    Focus(String),
    /// Close the edited agent.
    Close(String),
    /// Hide the panel.
    Hide,
    /// Leave Chat for another screen (any `Signal<Screen>` change). The
    /// router holds Chat on screen until this is resolved.
    Navigate(Screen),
}

/// What a guarded request did (Plan 09 dirty guard).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GuardOutcome {
    /// The change was applied immediately.
    Applied,
    /// Unsaved edits are at risk: the change is parked in
    /// `panel.pending` until the operator confirms or cancels it.
    Pending,
    /// Nothing to do (no-op request).
    Unchanged,
}

/// Whether `id` may name a tab: the reserved root id or a valid profile slug
/// (portable mirror of the server's `validate_profile_name`; the server
/// re-validates every name it receives).
pub fn is_valid_agent_id(id: &str) -> bool {
    id == DEFAULT_AGENT || validate_name_client_side(id).is_ok()
}

impl AgentTabsState {
    /// First-run layout (D-22): only the root agent, focused.
    pub fn default_layout() -> Self {
        Self {
            open: vec![DEFAULT_AGENT.to_string()],
            focused: Some(DEFAULT_AGENT.to_string()),
            ..Default::default()
        }
    }

    fn is_parked(&self, agent: &str) -> bool {
        self.parked.iter().any(|a| a == agent)
    }

    /// The nearest unparked open tab: the first one at or after `fwd_start`,
    /// else the last one before `back_end`. Used to re-home focus when the
    /// focused tab is closed, parked or reconciled away.
    fn nearest_unparked(&self, fwd_start: usize, back_end: usize) -> Option<String> {
        let unparked = |a: &&String| !self.is_parked(a);
        self.open
            .iter()
            .skip(fwd_start)
            .find(unparked)
            .or_else(|| self.open.iter().take(back_end).rev().find(unparked))
            .cloned()
    }

    /// Focus `agent`, opening a tab for it first when none is open (one tab
    /// per agent) and restoring it first when it is parked (D-14). Rejects
    /// invalid ids and refuses to grow past [`MAX_TABS`]. Returns whether
    /// anything changed, so callers skip `.set()` on a no-op.
    pub fn open_or_focus(&mut self, agent: &str) -> bool {
        if !is_valid_agent_id(agent) {
            return false;
        }
        let mut changed = false;
        if !self.open.iter().any(|a| a == agent) {
            if self.open.len() >= MAX_TABS {
                return false;
            }
            self.open.push(agent.to_string());
            changed = true;
        }
        if self.is_parked(agent) {
            self.parked.retain(|a| a != agent);
            changed = true;
        }
        if self.focused.as_deref() != Some(agent) {
            self.focused = Some(agent.to_string());
            changed = true;
        }
        changed
    }

    /// Close `agent`'s tab or card (D-11): remove it from `open` and `parked`
    /// and, when it was the focused tab, focus the nearest remaining unparked
    /// tab (the one now at its old index, else the previous one, else
    /// nothing). Display-only: no server call, no profile or session is
    /// touched. Returns whether anything changed, so callers skip `.set()` on
    /// a no-op.
    pub fn close(&mut self, agent: &str) -> bool {
        let Some(idx) = self.open.iter().position(|a| a == agent) else {
            return false;
        };
        self.open.remove(idx);
        self.parked.retain(|a| a != agent);
        if self.focused.as_deref() == Some(agent) {
            self.focused = self.nearest_unparked(idx, idx);
        }
        true
    }

    /// Park `agent` (D-14): move its tab into `parked`, shown as a card. When
    /// it was the focused tab, focus moves to the nearest unparked tab (the
    /// next one, else the previous one, else nothing). A no-op (not open, or
    /// already parked) returns `false`.
    pub fn park(&mut self, agent: &str) -> bool {
        let Some(idx) = self.open.iter().position(|a| a == agent) else {
            return false;
        };
        if self.is_parked(agent) {
            return false;
        }
        self.parked.push(agent.to_string());
        if self.focused.as_deref() == Some(agent) {
            self.focused = self.nearest_unparked(idx + 1, idx);
        }
        true
    }

    /// The tabs the strip shows: `open` minus `parked`, order kept.
    pub fn visible_tabs(&self) -> Vec<String> {
        self.open
            .iter()
            .filter(|a| !self.is_parked(a))
            .cloned()
            .collect()
    }

    /// Whether the chat page's widget column renders (D-13): when at least one
    /// tab is parked or the Logs mini-panel is on; otherwise the chat spans
    /// the full width.
    pub fn widget_column_visible(&self) -> bool {
        !self.parked.is_empty() || self.logs_widget
    }

    /// Show or hide the Logs mini-panel (D-14). Returns whether anything
    /// changed, so callers skip `.set()` on a no-op.
    pub fn set_logs_widget(&mut self, on: bool) -> bool {
        if self.logs_widget == on {
            return false;
        }
        self.logs_widget = on;
        true
    }

    /// Record whether the header's Logs shortcut is mid-drag. Returns whether
    /// anything changed, so callers skip `.set()` on a no-op.
    pub fn set_dragging(&mut self, on: bool) -> bool {
        if self.dragging_logs_widget == on {
            return false;
        }
        self.dragging_logs_widget = on;
        true
    }

    /// Open the settings panel for `agent` when no panel is open. Returns
    /// whether anything changed. Retargeting an already-open panel is
    /// [`Self::request_focus`]'s job, not this fn's.
    pub fn open_settings(&mut self, agent: &str) -> bool {
        if self.panel.settings_for.is_some() {
            return false;
        }
        self.panel.settings_for = Some(agent.to_string());
        true
    }

    /// Hide the settings panel unconditionally and drop its dirty flag and any
    /// pending confirmation (callers that must protect unsaved edits go
    /// through [`Self::request_hide`]). Returns whether anything changed.
    pub fn hide_settings(&mut self) -> bool {
        if self.panel == PanelState::default() {
            return false;
        }
        self.panel = PanelState::default();
        true
    }

    /// Whether the settings panel holds unsaved edits for `agent`.
    fn panel_dirty_for(&self, agent: &str) -> bool {
        self.panel.dirty && self.panel.settings_for.as_deref() == Some(agent)
    }

    /// Park `action` as the pending confirmation. A pending NAVIGATION is
    /// never replaced by a tab action: the screen the operator asked for is
    /// already requested, so dropping its record would leave the router
    /// holding Chat with a confirm bar that no longer says why.
    fn park_pending(&mut self, action: PanelPending) -> GuardOutcome {
        if !matches!(self.panel.pending, Some(PanelPending::Navigate(_))) {
            self.panel.pending = Some(action);
        }
        GuardOutcome::Pending
    }

    /// Focus `agent` through the dirty guard (Interaction Rule 2). While the
    /// panel holds unsaved edits for a DIFFERENT agent the switch is parked in
    /// `panel.pending` and nothing moves; otherwise the tab is focused (via
    /// [`Self::open_or_focus`]) and an open panel retargets to that agent.
    /// Focusing the agent already focused and shown is [`GuardOutcome::Unchanged`].
    pub fn request_focus(&mut self, agent: &str) -> GuardOutcome {
        if !is_valid_agent_id(agent) {
            return GuardOutcome::Unchanged;
        }
        let target = self.panel.settings_for.clone();
        if self.panel.dirty && target.as_deref().is_some_and(|t| t != agent) {
            return self.park_pending(PanelPending::Focus(agent.to_string()));
        }
        let changed = self.open_or_focus(agent);
        // Retarget only to a tab that is actually open (the cap can refuse).
        let retarget = self.open.iter().any(|a| a == agent)
            && target.as_deref().is_some_and(|t| t != agent);
        if retarget {
            self.panel.settings_for = Some(agent.to_string());
        }
        if changed || retarget {
            GuardOutcome::Applied
        } else {
            GuardOutcome::Unchanged
        }
    }

    /// Close `agent` through the dirty guard (Interaction Rule 4). Closing the
    /// agent whose panel holds unsaved edits is parked for confirmation; any
    /// other close happens at once, and closing the panel's own agent hides
    /// the panel with it.
    pub fn request_close(&mut self, agent: &str) -> GuardOutcome {
        if self.panel_dirty_for(agent) {
            return self.park_pending(PanelPending::Close(agent.to_string()));
        }
        let was_target = self.panel.settings_for.as_deref() == Some(agent);
        if !self.close(agent) {
            return GuardOutcome::Unchanged;
        }
        if was_target {
            self.hide_settings();
        }
        GuardOutcome::Applied
    }

    /// Hide the panel through the dirty guard (Esc, the title bar ✕).
    pub fn request_hide(&mut self) -> GuardOutcome {
        if self.panel.settings_for.is_none() {
            return GuardOutcome::Unchanged;
        }
        if self.panel.dirty {
            return self.park_pending(PanelPending::Hide);
        }
        self.hide_settings();
        GuardOutcome::Applied
    }

    /// Leave Chat for `target` through the dirty guard. Leaving Chat unmounts
    /// the panel and its working copy, so with unsaved edits the navigation is
    /// recorded as [`PanelPending::Navigate`] (a later request replaces an
    /// earlier one) and the router keeps Chat on screen; with a clean or
    /// closed panel it applies immediately. Chat itself is never a navigation.
    pub fn request_navigate(&mut self, target: Screen) -> GuardOutcome {
        if target == Screen::Chat {
            return GuardOutcome::Unchanged;
        }
        if self.panel.settings_for.is_some() && self.panel.dirty {
            self.panel.pending = Some(PanelPending::Navigate(target));
            return GuardOutcome::Pending;
        }
        GuardOutcome::Applied
    }

    /// The operator confirmed the pending action: apply it with the dirty flag
    /// cleared. Returns the newly focused agent for a [`PanelPending::Focus`]
    /// (the caller makes the D-23 activation write); `None` for everything
    /// else, including "nothing was pending". A confirmed navigation closes
    /// the panel — Chat is being left, so its working copy goes with it.
    pub fn confirm_pending(&mut self) -> Option<String> {
        let action = self.panel.pending.take()?;
        self.panel.dirty = false;
        match action {
            PanelPending::Focus(agent) => {
                self.open_or_focus(&agent);
                if self.open.iter().any(|a| a == &agent) {
                    self.panel.settings_for = Some(agent.clone());
                    Some(agent)
                } else {
                    self.hide_settings();
                    None
                }
            }
            PanelPending::Close(agent) => {
                self.close(&agent);
                self.hide_settings();
                None
            }
            PanelPending::Hide | PanelPending::Navigate(_) => {
                self.hide_settings();
                None
            }
        }
    }

    /// The operator chose to keep editing: drop the pending action and keep
    /// everything else. Returns the cancelled action so the caller can undo a
    /// navigation (set the screen back to Chat).
    pub fn cancel_pending(&mut self) -> Option<PanelPending> {
        self.panel.pending.take()
    }

    /// Drop every open tab that is neither [`DEFAULT_AGENT`] nor in
    /// `live_names`. When the focused tab vanishes, focus moves to the tab now
    /// at its old index, else the previous one, else nothing. Returns whether
    /// anything changed.
    pub fn reconcile(&mut self, live_names: &[String]) -> bool {
        let keep = |a: &String| a == DEFAULT_AGENT || live_names.iter().any(|n| n == a);
        if self.open.iter().all(keep) {
            return false;
        }
        let old_focus_idx = self
            .focused
            .as_ref()
            .and_then(|f| self.open.iter().position(|a| a == f));
        self.open.retain(keep);
        let still_open: Vec<String> = self.open.clone();
        self.parked.retain(|p| still_open.contains(p));
        let focus_vanished = self
            .focused
            .as_ref()
            .is_some_and(|f| !self.open.iter().any(|a| a == f));
        if focus_vanished {
            self.focused = old_focus_idx.and_then(|i| self.nearest_unparked(i, i));
        }
        // A settings panel for an agent that no longer exists has nothing
        // left to edit (and must not hold the router on Chat).
        if self
            .panel
            .settings_for
            .as_ref()
            .is_some_and(|p| !self.open.iter().any(|a| a == p))
        {
            self.hide_settings();
        }
        true
    }

    /// Normalise a hydrated blob (T-54.4-15): keep only valid ids,
    /// de-duplicate preserving order, cap at [`MAX_TABS`], clear a focus that
    /// is not open, and fall back to [`Self::default_layout`] when nothing
    /// valid is left.
    pub fn sanitized(mut self) -> Self {
        let mut seen: Vec<String> = Vec::new();
        for id in self.open.drain(..) {
            if is_valid_agent_id(&id) && !seen.contains(&id) {
                seen.push(id);
            }
        }
        seen.truncate(MAX_TABS);
        if seen.is_empty() {
            return Self::default_layout();
        }
        if self
            .focused
            .as_ref()
            .is_some_and(|f| !seen.iter().any(|a| a == f))
        {
            self.focused = None;
        }
        // Parked is a valid, de-duplicated subset of open; a parked tab can
        // never be the focused one.
        let mut parked: Vec<String> = Vec::new();
        for id in self.parked.drain(..) {
            if seen.contains(&id) && !parked.contains(&id) {
                parked.push(id);
            }
        }
        if self
            .focused
            .as_ref()
            .is_some_and(|f| parked.iter().any(|p| p == f))
        {
            self.focused = None;
        }
        self.parked = parked;
        self.open = seen;
        self
    }
}

/// The screen the router actually renders for a requested screen.
///
/// Leaving Chat unmounts the settings panel and, with it, its unsaved working
/// copy. So while the panel is open AND holds edits, any other requested
/// screen is held back to Chat until the operator resolves the confirmation
/// (`confirm_pending` clears `dirty`, which releases the hold; Keep editing
/// sets the requested screen back to Chat). The hold keys on `dirty` rather
/// than on `pending == Navigate(_)` because the router renders BEFORE the
/// navigation effect has recorded `pending`: holding only on `pending` would
/// unmount Chat for one frame and lose the edits anyway. A dirty panel is
/// always on screen with a Hide control and the confirm bar, so the hold can
/// never trap the operator.
pub fn effective_screen(requested: Screen, panel: &PanelState) -> Screen {
    if requested != Screen::Chat && panel.dirty && panel.settings_for.is_some() {
        Screen::Chat
    } else {
        requested
    }
}

/// The root-provided tab model (D-10). Provided ONLY in `HermesApp`'s body.
#[allow(dead_code)] // field read via `.0` at the consume sites; dead_code fires on isolated compilation units
#[derive(Clone, Copy)]
pub struct AgentTabsCtx(pub Signal<AgentTabsState>);

/// The live agent's name (`ironhermes_core::current_profile()`: a slug, or
/// `"default"` for the root home). Provided ONLY in `HermesApp`'s body.
#[allow(dead_code)] // field read via `.0` at the consume sites; dead_code fires on isolated compilation units
#[derive(Clone, Copy)]
pub struct LiveAgentCtx(pub Resource<Result<String, ServerFnError>>);

/// The non-blocking notice slot (D-23). Provided ONLY in `HermesApp`'s body.
#[allow(dead_code)] // field read via `.0` at the consume sites; dead_code fires on isolated compilation units
#[derive(Clone, Copy)]
pub struct AgentNoticeCtx(pub Signal<Option<String>>);

/// The live agent's name from the resolved resource value: `Some` only for an
/// `Ok` non-empty name.
pub fn live_agent_name(res: Option<Result<String, ServerFnError>>) -> Option<String> {
    match res {
        Some(Ok(name)) if !name.is_empty() => Some(name),
        _ => None,
    }
}

/// Focus `agent` (opening its tab when absent), then record the choice
/// best-effort via [`spawn_activation_write`]. The focus goes through the
/// dirty guard ([`AgentTabsState::request_focus`]): it is applied BEFORE the
/// write and never reverted (D-23), but a focus change that would discard
/// unsaved settings edits is parked as a pending confirmation instead and
/// writes nothing. A no-op pick (invalid id, tab cap, or the agent is already
/// focused) changes nothing and writes nothing. Never touches chat state
/// (D-10).
pub fn focus_agent(
    mut tabs: Signal<AgentTabsState>,
    notice: Signal<Option<String>>,
    refresh: Signal<u32>,
    agent: String,
) {
    let mut next = tabs.peek().clone();
    match next.request_focus(&agent) {
        GuardOutcome::Unchanged => {}
        GuardOutcome::Pending => tabs.set(next),
        GuardOutcome::Applied => {
            tabs.set(next);
            spawn_activation_write(notice, refresh, agent);
        }
    }
}

/// The agent focus moved TO between `before` and `after`, or `None` when the
/// focus did not change or was cleared (nothing focused means nothing to
/// record). Pure.
pub fn focus_moved_to(before: Option<&str>, after: Option<&str>) -> Option<String> {
    match after {
        Some(a) if before != Some(a) => Some(a.to_string()),
        _ => None,
    }
}

/// Record a focus change made by a path other than [`focus_agent`] (close,
/// park, reconcile, a confirmed discard): when `after` differs from `before`,
/// make the same best-effort activation write `focus_agent` makes, so the
/// persisted active-profile record (what Skills and Soul re-derive from) keeps
/// following the focused tab (WR-02).
pub fn record_focus_move(
    notice: Signal<Option<String>>,
    refresh: Signal<u32>,
    before: Option<&str>,
    after: Option<&str>,
) {
    if let Some(agent) = focus_moved_to(before, after) {
        spawn_activation_write(notice, refresh, agent);
    }
}

/// Store `next` as the tab model and record the focus move it caused, if any.
/// The shared `tabs.set(next)` for every close / park / reconcile site that
/// builds `next` from a clone.
pub fn set_tabs_recording_focus(
    mut tabs: Signal<AgentTabsState>,
    notice: Signal<Option<String>>,
    refresh: Signal<u32>,
    next: AgentTabsState,
) {
    let before = tabs.peek().focused.clone();
    let after = next.focused.clone();
    tabs.set(next);
    record_focus_move(notice, refresh, before.as_deref(), after.as_deref());
}

/// Run `change` on the tab model in place and record the focus move it
/// caused, if any. For the `tabs.with_mut(..)` sites (the settings panel's
/// Close agent, a confirmed discard).
pub fn update_tabs_recording_focus<R>(
    mut tabs: Signal<AgentTabsState>,
    notice: Signal<Option<String>>,
    refresh: Signal<u32>,
    change: impl FnOnce(&mut AgentTabsState) -> R,
) -> R {
    let before = tabs.peek().focused.clone();
    let result = tabs.with_mut(change);
    let after = tabs.peek().focused.clone();
    record_focus_move(notice, refresh, before.as_deref(), after.as_deref());
    result
}

/// What [`ActivationQueue::submit`] decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActivationSubmit {
    /// Nothing is running: the caller must start the runner.
    StartRunner,
    /// A runner is active; it will pick this agent up (latest wins).
    Queued,
    /// The server's write gate is known closed: no write is attempted.
    Skipped,
}

/// Serialises and coalesces the activation writes (WR-03). Every focus change
/// submits its agent; a single runner drains the queue so writes reach the
/// server one at a time and in click order, and a burst of rapid clicks
/// collapses to the LAST agent instead of racing overlapping read-modify-write
/// requests. Once the server reports its write gate closed
/// (`security.web_config_write_enabled` off) no further write is attempted.
/// Pure data, unit-tested natively.
#[derive(Debug)]
pub struct ActivationQueue {
    pending: Option<String>,
    running: bool,
    gate_closed: bool,
}

impl ActivationQueue {
    pub const fn new() -> Self {
        Self {
            pending: None,
            running: false,
            gate_closed: false,
        }
    }

    /// Queue `agent` as the agent to record (replacing any not-yet-sent one).
    pub fn submit(&mut self, agent: String) -> ActivationSubmit {
        if self.gate_closed {
            return ActivationSubmit::Skipped;
        }
        self.pending = Some(agent);
        if self.running {
            ActivationSubmit::Queued
        } else {
            self.running = true;
            ActivationSubmit::StartRunner
        }
    }

    /// The next agent to write, or `None` (and the runner stops) when the
    /// queue is drained or the gate has closed.
    pub fn next(&mut self) -> Option<String> {
        if self.gate_closed {
            self.pending = None;
        }
        let next = self.pending.take();
        if next.is_none() {
            self.running = false;
        }
        next
    }

    /// Record that the server refused writes. Returns `true` only the first
    /// time, so the operator sees one notice rather than one per click.
    pub fn note_gate_closed(&mut self) -> bool {
        let first = !self.gate_closed;
        self.gate_closed = true;
        first
    }
}

impl Default for ActivationQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// The one process-wide queue (the client is a single-threaded wasm event
/// loop; the mutex only satisfies `static`).
static ACTIVATION_QUEUE: std::sync::Mutex<ActivationQueue> =
    std::sync::Mutex::new(ActivationQueue::new());

fn with_activation_queue<R>(f: impl FnOnce(&mut ActivationQueue) -> R) -> R {
    let mut guard = ACTIVATION_QUEUE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut guard)
}

/// Whether a failed activation write was the server's closed write gate.
pub fn is_gate_closed_error(message: &str) -> bool {
    message.contains(crate::server::profile_api::CONFIG_WRITES_DISABLED)
}

/// The best-effort activation write behind a focus change (D-23):
/// `clear_active_profile()` for [`DEFAULT_AGENT`], `activate_profile(..,
/// ChatOnly)` otherwise. A failure only raises the non-blocking notice; it
/// never touches the tabs.
///
/// The write runs on the ROOT scope (`spawn_forever`), not the caller's: the
/// callers (a parked card, the Open-settings button, the discard bar) change
/// state that unmounts their own component in the same event, and a task
/// spawned on a dropped scope is cancelled before its first poll (WR-01). The
/// writes are funnelled through [`ActivationQueue`] (serialised, last click
/// wins), and are skipped once the server's write gate is known closed, with a
/// single notice instead of one per focus (WR-03).
pub fn spawn_activation_write(
    mut notice: Signal<Option<String>>,
    mut refresh: Signal<u32>,
    agent: String,
) {
    if with_activation_queue(|q| q.submit(agent)) != ActivationSubmit::StartRunner {
        return;
    }
    dioxus::core::spawn_forever(async move {
        while let Some(agent) = with_activation_queue(|q| q.next()) {
            let result = if agent == DEFAULT_AGENT {
                clear_active_profile().await
            } else {
                activate_profile(agent.clone(), ActivationScope::ChatOnly).await
            };
            match result {
                // Same as the retired topbar switcher: bump the shared tick so the
                // Soul/Skills readers re-derive from the persisted record.
                Ok(()) => {
                    let cur = *refresh.peek();
                    refresh.set(cur + 1);
                }
                Err(e) => {
                    let reason = format!("{e}");
                    if is_gate_closed_error(&reason) {
                        if with_activation_queue(|q| q.note_gate_closed()) {
                            notice.set(Some(format!(
                                "Switched to {agent}. Config writes are disabled on this server, so Skills keeps following the previously recorded agent."
                            )));
                        }
                    } else {
                        notice.set(Some(format!(
                            "Switched to {agent}. Couldn't record it for Skills: {reason}."
                        )));
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT_TABS_RS: &str = include_str!("agent_tabs.rs");
    const HEADER_RS: &str = include_str!("header.rs");
    const AGENT_DROPDOWN_RS: &str = include_str!("header/agent_dropdown.rs");

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// Source text before the first `#[cfg(test)]`, so a test module's own
    /// needles can never satisfy (or trip) a check on the component code.
    fn code_of(src: &str) -> &str {
        &src[..src.find("#[cfg(test)]").unwrap_or(src.len())]
    }

    #[test]
    fn agent_tabs_default_layout_is_default_tab() {
        let d = AgentTabsState::default_layout();
        assert_eq!(d.open, s(&["default"]));
        assert_eq!(d.focused.as_deref(), Some("default"));
    }

    #[test]
    fn agent_tabs_open_or_focus_adds_once() {
        let mut t = AgentTabsState::default_layout();
        assert!(t.open_or_focus("dev"));
        assert!(!t.open_or_focus("dev"));
        assert_eq!(t.open.iter().filter(|a| a.as_str() == "dev").count(), 1);
        assert_eq!(t.focused.as_deref(), Some("dev"));
    }

    #[test]
    fn agent_tabs_open_or_focus_existing_only_focuses() {
        let mut t = AgentTabsState {
            open: s(&["default", "dev"]),
            focused: Some("default".into()),
            ..Default::default()
        };
        assert!(t.open_or_focus("dev"));
        assert_eq!(t.open, s(&["default", "dev"]));
        assert_eq!(t.focused.as_deref(), Some("dev"));
    }

    #[test]
    fn agent_tabs_rejects_invalid_ids() {
        let mut t = AgentTabsState::default_layout();
        let before = t.clone();
        assert!(!t.open_or_focus("../x"));
        assert!(!t.open_or_focus("Has Caps"));
        assert!(!t.open_or_focus(""));
        assert_eq!(t, before);
        let mut t = AgentTabsState {
            open: s(&["dev"]),
            focused: Some("dev".into()),
            ..Default::default()
        };
        assert!(t.open_or_focus("default"));
        assert_eq!(t.focused.as_deref(), Some("default"));
    }

    #[test]
    fn agent_tabs_open_or_focus_respects_the_tab_cap() {
        let mut t = AgentTabsState::default();
        for i in 0..MAX_TABS {
            assert!(t.open_or_focus(&format!("agent-{i}")));
        }
        let before = t.clone();
        assert!(!t.open_or_focus("one-too-many"));
        assert_eq!(t, before);
        // An already-open tab can still be focused at the cap.
        assert!(t.open_or_focus("agent-0"));
        assert_eq!(t.focused.as_deref(), Some("agent-0"));
    }

    #[test]
    fn agent_tabs_reconcile_drops_vanished_keeps_default() {
        let mut t = AgentTabsState {
            open: s(&["default", "dev", "gone"]),
            focused: Some("dev".into()),
            ..Default::default()
        };
        assert!(t.reconcile(&s(&["dev"])));
        assert_eq!(t.open, s(&["default", "dev"]));
        assert_eq!(t.focused.as_deref(), Some("dev"));
    }

    #[test]
    fn agent_tabs_reconcile_refocuses_when_focused_vanished() {
        // The tab that took the vanished tab's index wins.
        let mut t = AgentTabsState {
            open: s(&["default", "dev", "gone", "zed"]),
            focused: Some("gone".into()),
            ..Default::default()
        };
        assert!(t.reconcile(&s(&["dev", "zed"])));
        assert_eq!(t.focused.as_deref(), Some("zed"));
        // Otherwise the previous one.
        let mut t = AgentTabsState {
            open: s(&["default", "dev", "gone"]),
            focused: Some("gone".into()),
            ..Default::default()
        };
        assert!(t.reconcile(&s(&["dev"])));
        assert_eq!(t.focused.as_deref(), Some("dev"));
        // Otherwise nothing.
        let mut t = AgentTabsState {
            open: s(&["gone"]),
            focused: Some("gone".into()),
            ..Default::default()
        };
        assert!(t.reconcile(&[]));
        assert!(t.open.is_empty());
        assert_eq!(t.focused, None);
    }

    #[test]
    fn agent_tabs_reconcile_noop_returns_false() {
        let mut t = AgentTabsState {
            open: s(&["default", "dev"]),
            focused: Some("dev".into()),
            ..Default::default()
        };
        let before = t.clone();
        assert!(!t.reconcile(&s(&["dev", "other"])));
        assert_eq!(t, before);
    }

    #[test]
    fn agent_tabs_live_agent_name_requires_a_non_empty_ok() {
        assert_eq!(live_agent_name(Some(Ok("dev".into()))), Some("dev".into()));
        assert_eq!(live_agent_name(Some(Ok(String::new()))), None);
        assert_eq!(live_agent_name(Some(Err(ServerFnError::new("x")))), None);
        assert_eq!(live_agent_name(None), None);
    }

    #[test]
    fn agent_tab_modules_declare_no_context_provider() {
        let needle = ["use_context", "_provider"].concat();
        for (name, src) in [
            ("agent_tabs.rs", AGENT_TABS_RS),
            ("header.rs", HEADER_RS),
            ("header/agent_dropdown.rs", AGENT_DROPDOWN_RS),
        ] {
            assert!(
                !code_of(src).contains(&needle),
                "{name} must not provide any context (providers live at the HermesApp root)"
            );
        }
    }

    #[test]
    fn agent_tabs_focus_applies_before_the_activation_write() {
        let code = code_of(AGENT_TABS_RS);
        let region_of = |name: &str| -> &str {
            let start = code.find(name).unwrap_or_else(|| panic!("{name} must exist"));
            let rest = &code[start + 1..];
            let end = rest.find("\npub fn ").map_or(code.len(), |e| start + 1 + e);
            &code[start..end]
        };
        let focus = region_of("pub fn focus_agent");
        let applied_at = focus
            .find("GuardOutcome::Applied")
            .expect("focus_agent must branch on the Applied outcome");
        let set_at = focus[applied_at..]
            .find("tabs.set(")
            .map(|i| applied_at + i)
            .expect("the Applied arm must set the tabs");
        let write_at = focus
            .find("spawn_activation_write(")
            .expect("focus_agent must record the activation");
        assert!(
            applied_at < set_at && set_at < write_at,
            "the focus must be applied BEFORE the activation write, and the write only on Applied (D-23)"
        );
        assert!(
            !focus[..applied_at].contains("spawn_activation_write("),
            "a Pending or Unchanged outcome must never write"
        );

        let write = region_of("pub fn spawn_activation_write");
        let first_write = ["activate_profile(", "clear_active_profile("]
            .iter()
            .filter_map(|n| write.find(n))
            .min()
            .expect("spawn_activation_write must record the activation");
        let err_at = write.find("Err(").expect("it must handle a failed write");
        assert!(first_write < err_at);
        assert!(
            !write[err_at..].contains("tabs.set("),
            "the Err branch must only set the notice, never touch the tabs"
        );
        assert!(write[err_at..].contains("notice.set("));
        // WR-01: the write must be spawned on the ROOT scope. A plain
        // `spawn` ties the task to the caller's scope, and the callers (a
        // parked card, the Open-settings button, the discard bar) unmount in
        // the same event, which cancels the task before its first poll.
        assert!(
            write.contains("spawn_forever("),
            "the activation write must be spawned with spawn_forever (root scope)"
        );
        assert!(
            !write.replace("spawn_forever(", "").contains("spawn("),
            "the activation write must not be spawned on the caller's scope"
        );
    }

    #[test]
    fn agent_tabs_focus_moved_to_reports_only_a_real_move() {
        assert_eq!(focus_moved_to(Some("a"), Some("b")), Some("b".to_string()));
        assert_eq!(focus_moved_to(None, Some("b")), Some("b".to_string()));
        assert_eq!(focus_moved_to(Some("a"), Some("a")), None);
        // Nothing focused afterwards: nothing to record.
        assert_eq!(focus_moved_to(Some("a"), None), None);
        assert_eq!(focus_moved_to(None, None), None);
    }

    /// WR-02: close / park / reconcile each move focus in the model, and the
    /// recorded move is exactly the new focus.
    #[test]
    fn agent_tabs_close_park_and_reconcile_focus_moves_are_reportable() {
        let base = AgentTabsState {
            open: s(&["default", "dev", "ops"]),
            focused: Some("dev".into()),
            ..Default::default()
        };

        let mut closed = base.clone();
        assert_eq!(closed.request_close("dev"), GuardOutcome::Applied);
        assert_eq!(
            focus_moved_to(base.focused.as_deref(), closed.focused.as_deref()),
            Some("ops".to_string())
        );

        let mut parked = base.clone();
        assert!(parked.park("dev"));
        assert_eq!(
            focus_moved_to(base.focused.as_deref(), parked.focused.as_deref()),
            Some("ops".to_string())
        );

        let mut reconciled = base.clone();
        assert!(reconciled.reconcile(&s(&["ops"])));
        assert_eq!(
            focus_moved_to(base.focused.as_deref(), reconciled.focused.as_deref()),
            Some("ops".to_string())
        );

        // Closing an unfocused tab moves nothing.
        let mut other = base.clone();
        assert!(other.close("ops"));
        assert_eq!(
            focus_moved_to(base.focused.as_deref(), other.focused.as_deref()),
            None
        );
    }

    /// WR-02: every site that can move focus other than `focus_agent` routes
    /// through the recording helpers.
    #[test]
    fn agent_tabs_every_focus_moving_site_records_the_activation() {
        const STRIP: &str = include_str!("screens/chat/agent_tab_strip.rs");
        const WIDGETS: &str = include_str!("screens/chat/agent_widgets.rs");
        const MOD_RS: &str = include_str!("mod.rs");
        const SETTINGS: &str = include_str!("screens/profile_shared/agent_settings.rs");
        // Park + Close on the strip; Close on a parked card.
        for (name, src, calls) in [
            ("agent_tab_strip.rs", STRIP, 2),
            ("agent_widgets.rs", WIDGETS, 1),
        ] {
            let code = code_of(src);
            assert!(
                code.matches("set_tabs_recording_focus(tabs, notice, refresh, next)")
                    .count()
                    >= calls,
                "{name}: park/close must record the focus move through set_tabs_recording_focus"
            );
        }
        let mod_code = code_of(MOD_RS);
        assert!(
            mod_code.contains("set_tabs_recording_focus("),
            "reconcile must record the focus move"
        );
        let settings = code_of(SETTINGS);
        assert!(settings.contains("update_tabs_recording_focus("));
        assert!(settings.contains("record_focus_move("));
        assert!(
            !settings.contains("tabs.with_mut(|s| s.request_close("),
            "Close agent must record the focus move"
        );
    }

    #[test]
    fn activation_queue_starts_one_runner_and_coalesces_to_the_last_agent() {
        let mut q = ActivationQueue::new();
        assert_eq!(q.submit("a".into()), ActivationSubmit::StartRunner);
        // Rapid clicks while the runner is busy queue; the latest wins.
        assert_eq!(q.submit("b".into()), ActivationSubmit::Queued);
        assert_eq!(q.submit("c".into()), ActivationSubmit::Queued);
        assert_eq!(q.next().as_deref(), Some("c"));
        assert_eq!(q.next(), None, "drained: the runner stops");
        // After the runner stopped, the next submit starts a new one.
        assert_eq!(q.submit("d".into()), ActivationSubmit::StartRunner);
        assert_eq!(q.next().as_deref(), Some("d"));
        assert_eq!(q.next(), None);
    }

    #[test]
    fn activation_queue_skips_writes_once_the_gate_is_known_closed() {
        let mut q = ActivationQueue::new();
        assert_eq!(q.submit("a".into()), ActivationSubmit::StartRunner);
        assert_eq!(q.submit("b".into()), ActivationSubmit::Queued);
        assert_eq!(q.next().as_deref(), Some("b"));
        // The server refused: exactly one notice, then nothing more is sent.
        assert!(q.note_gate_closed(), "the first closed-gate report notices");
        assert!(!q.note_gate_closed(), "later reports stay silent");
        assert_eq!(q.next(), None);
        assert_eq!(q.submit("c".into()), ActivationSubmit::Skipped);
        assert_eq!(q.submit("d".into()), ActivationSubmit::Skipped);
        assert_eq!(q.next(), None);
    }

    #[test]
    fn activation_gate_closed_error_is_recognised_by_the_servers_text() {
        assert!(is_gate_closed_error("Config writes are disabled"));
        assert!(is_gate_closed_error(
            "error running server function: Config writes are disabled"
        ));
        assert!(!is_gate_closed_error("Config load failed: no such file"));
        assert!(!is_gate_closed_error("profile 'x' does not exist"));
    }

    #[test]
    fn agent_tabs_never_touch_the_chat_singletons() {
        // D-10 / Interaction Rule 2, checked on the code regions only.
        for src in [AGENT_TABS_RS, AGENT_DROPDOWN_RS] {
            let code = code_of(src);
            for frag in [["session", "_id"].concat(), ["Session", "IdContext"].concat(), ["create", "_session"].concat()] {
                assert!(!code.contains(&frag), "tab code must not reference `{frag}`");
            }
        }
    }

    #[test]
    fn agent_tabs_close_refocuses_nearest() {
        let mut t = AgentTabsState {
            open: s(&["default", "a", "b"]),
            focused: Some("a".into()),
            ..Default::default()
        };
        assert!(t.close("a"));
        assert_eq!(t.open, s(&["default", "b"]));
        assert_eq!(t.focused.as_deref(), Some("b"));
    }

    #[test]
    fn agent_tabs_close_last_tab_clears_focus() {
        let mut t = AgentTabsState {
            open: s(&["a"]),
            focused: Some("a".into()),
            ..Default::default()
        };
        assert!(t.close("a"));
        assert!(t.open.is_empty());
        assert_eq!(t.focused, None);
    }

    #[test]
    fn agent_tabs_close_unknown_is_noop() {
        let mut t = AgentTabsState {
            open: s(&["default", "a"]),
            focused: Some("a".into()),
            ..Default::default()
        };
        let before = t.clone();
        assert!(!t.close("zzz"));
        assert_eq!(t, before);
    }

    #[test]
    fn agent_tabs_park_moves_focus_to_nearest_unparked() {
        let mut t = AgentTabsState {
            open: s(&["default", "a", "b"]),
            focused: Some("a".into()),
            ..Default::default()
        };
        assert!(t.park("a"));
        assert_eq!(t.parked, s(&["a"]));
        assert_eq!(t.focused.as_deref(), Some("b"));
        // The last visible tab parks onto the previous visible one.
        assert!(t.park("b"));
        assert_eq!(t.focused.as_deref(), Some("default"));
        // Parking everything leaves nothing focused.
        assert!(t.park("default"));
        assert_eq!(t.focused, None);
        assert_eq!(t.parked.len(), 3);
    }

    #[test]
    fn agent_tabs_park_twice_is_noop() {
        let mut t = AgentTabsState {
            open: s(&["default", "a"]),
            focused: Some("default".into()),
            ..Default::default()
        };
        assert!(t.park("a"));
        let before = t.clone();
        assert!(!t.park("a"));
        assert!(!t.park("not-open"));
        assert_eq!(t, before);
    }

    #[test]
    fn agent_tabs_open_or_focus_unparks() {
        let mut t = AgentTabsState {
            open: s(&["default", "a"]),
            focused: Some("default".into()),
            parked: s(&["a"]),
            ..Default::default()
        };
        assert!(t.open_or_focus("a"));
        assert!(t.parked.is_empty());
        assert_eq!(t.focused.as_deref(), Some("a"));
    }

    #[test]
    fn agent_tabs_close_removes_from_parked() {
        let mut t = AgentTabsState {
            open: s(&["default", "a", "b"]),
            focused: Some("default".into()),
            parked: s(&["a"]),
            ..Default::default()
        };
        assert!(t.close("a"));
        assert_eq!(t.open, s(&["default", "b"]));
        assert!(t.parked.is_empty());
        // Closing the focused tab never lands focus on a parked tab.
        let mut t = AgentTabsState {
            open: s(&["default", "a", "b"]),
            focused: Some("default".into()),
            parked: s(&["a"]),
            ..Default::default()
        };
        assert!(t.close("default"));
        assert_eq!(t.focused.as_deref(), Some("b"));
    }

    #[test]
    fn agent_tabs_reconcile_drops_vanished_from_parked() {
        let mut t = AgentTabsState {
            open: s(&["default", "dev", "gone"]),
            focused: Some("dev".into()),
            parked: s(&["gone"]),
            ..Default::default()
        };
        assert!(t.reconcile(&s(&["dev"])));
        assert_eq!(t.open, s(&["default", "dev"]));
        assert!(t.parked.is_empty());
    }

    #[test]
    fn dashboard_layout_sanitized_keeps_parked_subset() {
        let t = AgentTabsState {
            open: s(&["default", "a", "b"]),
            focused: Some("a".into()),
            parked: s(&["a", "a", "zzz", "../x", "b"]),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(t.parked, s(&["a", "b"]), "only valid, unique members of open");
        assert_eq!(t.focused, None, "a parked focused id is cleared");
        // A blob that predates `parked` still loads.
        let old: AgentTabsState =
            serde_json::from_str(r#"{"open":["default"],"focused":"default"}"#).expect("old blob");
        assert!(old.parked.is_empty());
        // Parked round-trips through the persisted blob.
        let json = serde_json::to_string(&t).expect("serialize");
        let back: AgentTabsState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, t);
    }

    #[test]
    fn widget_column_visible_only_when_something_is_parked() {
        let mut t = AgentTabsState::default_layout();
        assert!(!t.widget_column_visible());
        t.open.push("a".into());
        assert!(t.park("a"));
        assert!(t.widget_column_visible());
    }

    #[test]
    fn agent_tabs_set_logs_widget_reports_change() {
        let mut t = AgentTabsState::default_layout();
        assert!(!t.logs_widget, "the Logs panel starts off");
        assert!(t.set_logs_widget(true));
        assert!(!t.set_logs_widget(true), "setting the same value is a no-op");
        assert!(t.set_logs_widget(false));
        assert!(!t.logs_widget);
    }

    #[test]
    fn agent_tabs_widget_column_visible_with_logs_widget() {
        let mut t = AgentTabsState::default_layout();
        assert!(!t.widget_column_visible(), "neither parked nor Logs panel");
        t.set_logs_widget(true);
        assert!(t.widget_column_visible(), "the Logs panel alone shows the column");
        t.set_logs_widget(false);
        t.open.push("a".into());
        assert!(t.park("a"));
        assert!(t.widget_column_visible(), "a parked tab alone shows the column");
    }

    /// D-15: the Logs panel persists; a blob stored before the key existed
    /// loads with the panel off.
    #[test]
    fn dashboard_layout_persists_logs_widget() {
        let mut t = AgentTabsState::default_layout();
        t.set_logs_widget(true);
        let json = serde_json::to_string(&t).expect("serialize");
        let back: AgentTabsState = serde_json::from_str(&json).expect("deserialize");
        assert!(back.logs_widget);
        assert!(back.sanitized().logs_widget, "sanitized keeps the Logs panel");
        let old: AgentTabsState =
            serde_json::from_str(r#"{"open":["default"],"focused":"default"}"#)
                .expect("a blob without the key still loads");
        assert!(!old.logs_widget);
    }

    /// D-15: the drag-in-progress flag is transient. It never reaches the
    /// persisted blob and a reloaded state always starts with it off.
    #[test]
    fn agent_tabs_dragging_flag_is_transient() {
        let mut t = AgentTabsState::default_layout();
        assert!(!t.dragging_logs_widget);
        assert!(t.set_dragging(true));
        assert!(!t.set_dragging(true), "setting the same value is a no-op");
        let json = serde_json::to_string(&t).expect("serialize");
        assert!(!json.contains("dragging"), "dragging flag leaked into: {json}");
        let back: AgentTabsState = serde_json::from_str(&json).expect("deserialize");
        assert!(!back.dragging_logs_widget);
        assert!(t.set_dragging(false));
        assert!(!t.dragging_logs_widget);
    }

    #[test]
    fn visible_tabs_excludes_parked() {
        let t = AgentTabsState {
            open: s(&["default", "a", "b"]),
            focused: Some("default".into()),
            parked: s(&["a"]),
            ..Default::default()
        };
        assert_eq!(t.visible_tabs(), s(&["default", "b"]));
    }

    /// D-10 / Interaction Rule 3: no tab, card, banner or dropdown code reads
    /// or writes the chat singletons. Needles are assembled at runtime so this
    /// test never matches itself; only the code regions are inspected.
    #[test]
    fn tabs_do_not_touch_chat_singletons() {
        const STRIP_RS: &str = include_str!("screens/chat/agent_tab_strip.rs");
        const WIDGETS_RS: &str = include_str!("screens/chat/agent_widgets.rs");
        const BANNER_RS: &str = include_str!("screens/chat/non_live_banner.rs");
        let needles = [
            ["Session", "IdContext"].concat(),
            ["session", "_id"].concat(),
            ["bubb", "les"].concat(),
            ["streaming", "_id"].concat(),
            ["create", "_session"].concat(),
        ];
        for (name, src) in [
            ("agent_tabs.rs", AGENT_TABS_RS),
            ("chat/agent_tab_strip.rs", STRIP_RS),
            ("chat/agent_widgets.rs", WIDGETS_RS),
            ("chat/non_live_banner.rs", BANNER_RS),
            ("header/agent_dropdown.rs", AGENT_DROPDOWN_RS),
        ] {
            let code = code_of(src);
            for needle in &needles {
                assert!(
                    !code.contains(needle.as_str()),
                    "{name} must not reference the chat singleton `{needle}`"
                );
            }
        }
    }

    #[test]
    fn dashboard_layout_round_trips() {
        let t = AgentTabsState {
            open: s(&["default", "dev"]),
            focused: Some("dev".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&t).expect("serialize");
        let back: AgentTabsState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, t);
    }

    #[test]
    fn dashboard_layout_tolerates_unknown_and_missing_fields() {
        let t: AgentTabsState =
            serde_json::from_str(r#"{"open":["dev"],"future":1}"#).expect("unknown field ignored");
        assert_eq!(t.open, s(&["dev"]));
        assert_eq!(t.focused, None);
        let e: AgentTabsState = serde_json::from_str("{}").expect("empty object");
        assert_eq!(e, AgentTabsState::default());
        assert!(e.open.is_empty());
    }

    #[test]
    fn dashboard_layout_sanitized_drops_invalid_and_caps() {
        let mut open = s(&["../x", "dev", "dev"]);
        open.extend((0..20).map(|i| format!("agent-{i}")));
        let t = AgentTabsState {
            open,
            focused: Some("not-open".into()),
            ..Default::default()
        }
        .sanitized();
        assert!(t.open.iter().all(|a| is_valid_agent_id(a)));
        assert!(!t.open.iter().any(|a| a == "../x"));
        let mut dedup = t.open.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), t.open.len(), "no duplicates");
        assert!(t.open.len() <= MAX_TABS);
        assert_eq!(t.open.first().map(String::as_str), Some("dev"));
        assert_eq!(t.focused, None, "focused is cleared when it is not open");
    }

    #[test]
    fn dashboard_layout_sanitized_keeps_a_focused_open_tab() {
        let t = AgentTabsState {
            open: s(&["default", "dev"]),
            focused: Some("dev".into()),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(t.focused.as_deref(), Some("dev"));
    }

    #[test]
    fn dashboard_layout_empty_after_sanitize_falls_back_to_default() {
        let t = AgentTabsState {
            open: s(&["../x", "Has Caps", ""]),
            focused: Some("../x".into()),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(t, AgentTabsState::default_layout());
    }

    #[test]
    fn agent_tabs_open_settings_sets_target() {
        let mut t = AgentTabsState::default_layout();
        assert!(t.open_settings("dev"));
        assert_eq!(t.panel.settings_for.as_deref(), Some("dev"));
        assert!(!t.open_settings("dev"), "a second open is a no-op");
        assert!(!t.open_settings("ops"), "opening never retargets an open panel");
        assert_eq!(t.panel.settings_for.as_deref(), Some("dev"));
    }

    #[test]
    fn agent_tabs_hide_settings_clears_target_and_dirty() {
        let mut t = AgentTabsState::default_layout();
        assert!(!t.hide_settings(), "hiding a closed panel is a no-op");
        t.panel = PanelState {
            settings_for: Some("dev".into()),
            dirty: true,
            pending: Some(PanelPending::Hide),
        };
        assert!(t.hide_settings());
        assert_eq!(t.panel, PanelState::default());
    }

    #[test]
    fn agent_tabs_panel_state_is_not_persisted() {
        let mut t = AgentTabsState::default_layout();
        t.panel = PanelState {
            settings_for: Some("dev".into()),
            dirty: true,
            pending: Some(PanelPending::Focus("ops".into())),
        };
        let json = serde_json::to_string(&t).expect("serialize");
        assert!(!json.contains("panel"), "panel state must not persist: {json}");
        assert!(!json.contains("settings_for"), "panel state must not persist: {json}");
        let back: AgentTabsState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.panel, PanelState::default());
    }

    /// A tab model with `open_agents` open, `focused` focused, and the
    /// settings panel open for `panel_for` with the given dirtiness.
    fn with_panel(
        open_agents: &[&str],
        focused: &str,
        panel_for: &str,
        dirty: bool,
    ) -> AgentTabsState {
        AgentTabsState {
            open: s(open_agents),
            focused: Some(focused.into()),
            panel: PanelState {
                settings_for: Some(panel_for.into()),
                dirty,
                pending: None,
            },
            ..Default::default()
        }
    }

    #[test]
    fn agent_tabs_request_focus_retargets_open_clean_panel() {
        let mut t = with_panel(&["default", "dev", "ops"], "dev", "dev", false);
        assert_eq!(t.request_focus("ops"), GuardOutcome::Applied);
        assert_eq!(t.focused.as_deref(), Some("ops"));
        assert_eq!(t.panel.settings_for.as_deref(), Some("ops"));
        // A closed panel is never opened by a focus change.
        let mut t = AgentTabsState {
            open: s(&["default", "dev"]),
            focused: Some("default".into()),
            ..Default::default()
        };
        assert_eq!(t.request_focus("dev"), GuardOutcome::Applied);
        assert_eq!(t.panel.settings_for, None);
    }

    #[test]
    fn agent_tabs_request_focus_parks_a_pending_switch_when_dirty() {
        let mut t = with_panel(&["default", "dev", "ops"], "dev", "dev", true);
        assert_eq!(t.request_focus("ops"), GuardOutcome::Pending);
        assert_eq!(t.focused.as_deref(), Some("dev"), "focus must not move yet");
        assert_eq!(t.panel.settings_for.as_deref(), Some("dev"));
        assert_eq!(t.panel.pending, Some(PanelPending::Focus("ops".into())));
        assert!(t.panel.dirty);
    }

    #[test]
    fn agent_tabs_request_focus_same_agent_is_not_pending() {
        let mut t = with_panel(&["default", "dev"], "dev", "dev", true);
        assert_eq!(t.request_focus("dev"), GuardOutcome::Unchanged);
        assert_eq!(t.panel.pending, None);
    }

    #[test]
    fn agent_tabs_request_focus_rejects_invalid_and_never_retargets_to_a_closed_tab() {
        let mut t = with_panel(&["default", "dev"], "dev", "dev", true);
        assert_eq!(t.request_focus("Has Caps"), GuardOutcome::Unchanged);
        assert_eq!(t.panel.pending, None);
        let mut t = with_panel(&["dev"], "dev", "dev", false);
        for i in 0..MAX_TABS {
            t.open.push(format!("agent-{i}"));
        }
        t.open.truncate(MAX_TABS);
        assert_eq!(t.request_focus("one-too-many"), GuardOutcome::Unchanged);
        assert_eq!(t.panel.settings_for.as_deref(), Some("dev"));
    }

    #[test]
    fn agent_tabs_request_close_confirms_only_for_the_dirty_agent() {
        let mut t = with_panel(&["default", "dev", "ops"], "dev", "dev", true);
        assert_eq!(t.request_close("dev"), GuardOutcome::Pending);
        assert_eq!(t.panel.pending, Some(PanelPending::Close("dev".into())));
        assert!(t.open.iter().any(|a| a == "dev"), "nothing is closed yet");
        t.panel.pending = None;
        assert_eq!(t.request_close("ops"), GuardOutcome::Applied);
        assert!(!t.open.iter().any(|a| a == "ops"));
        assert_eq!(t.panel.settings_for.as_deref(), Some("dev"), "the panel stays for dev");
        // A clean panel closes its own agent immediately and hides with it.
        let mut t = with_panel(&["default", "dev"], "dev", "dev", false);
        assert_eq!(t.request_close("dev"), GuardOutcome::Applied);
        assert_eq!(t.panel, PanelState::default());
        assert_eq!(t.request_close("dev"), GuardOutcome::Unchanged);
    }

    #[test]
    fn agent_tabs_request_hide_confirms_when_dirty() {
        let mut t = with_panel(&["default", "dev"], "dev", "dev", true);
        assert_eq!(t.request_hide(), GuardOutcome::Pending);
        assert_eq!(t.panel.pending, Some(PanelPending::Hide));
        assert_eq!(t.panel.settings_for.as_deref(), Some("dev"));
        let mut t = with_panel(&["default", "dev"], "dev", "dev", false);
        assert_eq!(t.request_hide(), GuardOutcome::Applied);
        assert_eq!(t.panel, PanelState::default());
        assert_eq!(t.request_hide(), GuardOutcome::Unchanged);
    }

    #[test]
    fn agent_tabs_confirm_pending_applies_and_clears_dirty() {
        let mut t = with_panel(&["default", "dev", "ops"], "dev", "dev", true);
        assert_eq!(t.request_focus("ops"), GuardOutcome::Pending);
        assert_eq!(t.confirm_pending().as_deref(), Some("ops"));
        assert_eq!(t.focused.as_deref(), Some("ops"));
        assert_eq!(t.panel.settings_for.as_deref(), Some("ops"));
        assert!(!t.panel.dirty);
        assert_eq!(t.panel.pending, None);
        // Close: the agent closes and the panel hides.
        let mut t = with_panel(&["default", "dev"], "dev", "dev", true);
        assert_eq!(t.request_close("dev"), GuardOutcome::Pending);
        assert_eq!(t.confirm_pending(), None);
        assert!(!t.open.iter().any(|a| a == "dev"));
        assert_eq!(t.panel, PanelState::default());
        // Hide: the panel hides, the tab stays.
        let mut t = with_panel(&["default", "dev"], "dev", "dev", true);
        assert_eq!(t.request_hide(), GuardOutcome::Pending);
        assert_eq!(t.confirm_pending(), None);
        assert!(t.open.iter().any(|a| a == "dev"));
        assert_eq!(t.panel, PanelState::default());
        // Nothing pending: nothing happens.
        assert_eq!(t.confirm_pending(), None);
    }

    #[test]
    fn agent_tabs_cancel_pending_keeps_everything() {
        let mut t = with_panel(&["default", "dev", "ops"], "dev", "dev", true);
        assert_eq!(t.request_focus("ops"), GuardOutcome::Pending);
        let before_open = t.open.clone();
        assert_eq!(t.cancel_pending(), Some(PanelPending::Focus("ops".into())));
        assert_eq!(t.panel.pending, None);
        assert!(t.panel.dirty, "the edits are still there");
        assert_eq!(t.panel.settings_for.as_deref(), Some("dev"));
        assert_eq!(t.focused.as_deref(), Some("dev"));
        assert_eq!(t.open, before_open);
        assert_eq!(t.cancel_pending(), None);
    }

    #[test]
    fn agent_tabs_request_navigate_pends_when_dirty() {
        let mut t = with_panel(&["default", "dev"], "dev", "dev", true);
        assert_eq!(t.request_navigate(Screen::Kanban), GuardOutcome::Pending);
        assert_eq!(t.panel.pending, Some(PanelPending::Navigate(Screen::Kanban)));
        // A later navigation replaces the earlier one.
        assert_eq!(t.request_navigate(Screen::Models), GuardOutcome::Pending);
        assert_eq!(t.panel.pending, Some(PanelPending::Navigate(Screen::Models)));
        // A clean or closed panel navigates immediately.
        let mut t = with_panel(&["default", "dev"], "dev", "dev", false);
        assert_eq!(t.request_navigate(Screen::Kanban), GuardOutcome::Applied);
        assert_eq!(t.panel.pending, None);
        let mut t = AgentTabsState::default_layout();
        assert_eq!(t.request_navigate(Screen::Kanban), GuardOutcome::Applied);
        // Staying on Chat is never a navigation.
        let mut t = with_panel(&["default", "dev"], "dev", "dev", true);
        assert_eq!(t.request_navigate(Screen::Chat), GuardOutcome::Unchanged);
        assert_eq!(t.panel.pending, None);
    }

    #[test]
    fn agent_tabs_effective_screen_holds_chat_while_navigation_pending() {
        let mut t = with_panel(&["default", "dev"], "dev", "dev", true);
        // The router renders BEFORE the navigation effect records `pending`,
        // so a dirty open panel must already hold Chat on screen.
        assert_eq!(effective_screen(Screen::Kanban, &t.panel), Screen::Chat);
        assert_eq!(t.request_navigate(Screen::Kanban), GuardOutcome::Pending);
        assert_eq!(effective_screen(Screen::Kanban, &t.panel), Screen::Chat);
        let clean = with_panel(&["default", "dev"], "dev", "dev", false);
        assert_eq!(effective_screen(Screen::Kanban, &clean.panel), Screen::Kanban);
        assert_eq!(
            effective_screen(Screen::Kanban, &PanelState::default()),
            Screen::Kanban
        );
        assert_eq!(effective_screen(Screen::Chat, &t.panel), Screen::Chat);
        assert_eq!(effective_screen(Screen::Chat, &clean.panel), Screen::Chat);
    }

    #[test]
    fn agent_tabs_confirm_navigate_closes_panel_and_clears_dirty() {
        let mut t = with_panel(&["default", "dev"], "dev", "dev", true);
        assert_eq!(t.request_navigate(Screen::Kanban), GuardOutcome::Pending);
        assert_eq!(t.confirm_pending(), None);
        assert!(!t.panel.dirty);
        assert_eq!(t.panel.pending, None);
        assert_eq!(t.panel.settings_for, None, "leaving Chat closes the panel");
        assert_eq!(effective_screen(Screen::Kanban, &t.panel), Screen::Kanban);
    }

    #[test]
    fn agent_tabs_cancel_navigate_returns_the_pending_action() {
        let mut t = with_panel(&["default", "dev"], "dev", "dev", true);
        assert_eq!(t.request_navigate(Screen::Kanban), GuardOutcome::Pending);
        assert_eq!(
            t.cancel_pending(),
            Some(PanelPending::Navigate(Screen::Kanban))
        );
        assert!(t.panel.dirty, "dirty stays true so the edits survive");
        assert_eq!(t.panel.settings_for.as_deref(), Some("dev"));
    }

    #[test]
    fn agent_tabs_a_pending_navigate_is_not_replaced_by_a_focus_close_or_hide() {
        let mut t = with_panel(&["default", "dev", "ops"], "dev", "dev", true);
        assert_eq!(t.request_navigate(Screen::Kanban), GuardOutcome::Pending);
        let nav = Some(PanelPending::Navigate(Screen::Kanban));
        assert_eq!(t.request_focus("ops"), GuardOutcome::Pending);
        assert_eq!(t.panel.pending, nav);
        assert_eq!(t.request_close("dev"), GuardOutcome::Pending);
        assert_eq!(t.panel.pending, nav);
        assert_eq!(t.request_hide(), GuardOutcome::Pending);
        assert_eq!(t.panel.pending, nav);
    }

    #[test]
    fn agent_tabs_reconcile_closes_a_panel_whose_agent_vanished() {
        let mut t = with_panel(&["default", "dev", "gone"], "dev", "gone", true);
        t.panel.pending = Some(PanelPending::Hide);
        assert!(t.reconcile(&s(&["dev"])));
        assert_eq!(t.panel, PanelState::default());
        // A panel for a surviving agent is untouched.
        let mut t = with_panel(&["default", "dev", "gone"], "dev", "dev", true);
        assert!(t.reconcile(&s(&["dev"])));
        assert_eq!(t.panel.settings_for.as_deref(), Some("dev"));
        assert!(t.panel.dirty);
    }

    #[test]
    fn dashboard_layout_persists_no_session_fields() {
        let json = serde_json::to_string(&AgentTabsState::default_layout()).expect("serialize");
        assert!(!json.contains("session"), "persisted layout must hold slugs only: {json}");
    }
}
