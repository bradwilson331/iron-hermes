//! Overlay/modal rendering for the tui_rata REPL (Phase 36.6.2 Plan 01).
//!
//! This module is the shared seam every overlay surface in this phase renders
//! through: the `OverlayKind` enum, the `centered_rect` popup-centering recipe
//! (RESEARCH Pattern 1), and the `render` dispatcher called from
//! `event_loop.rs`'s draw closure strictly AFTER the base `ui(f, app)` call
//! (UI-SPEC §2 — Clear + render-after-base, no alpha/dimming; ratatui buffers
//! have no blending, so "floating on top" is achieved by clearing the target
//! cells then drawing the overlay's own widgets over them, in the same
//! `terminal.draw` closure).
//!
//! This plan wires exactly one variant end-to-end: the browse-only Skills Hub
//! (TUI-03, D-06/D-07). `OverlayKind::Approval`/`Secret`/`Sudo` (Plan 03) and
//! `::Help` (Plan 04) are added by the plan that first constructs them, per
//! this phase's convention of introducing a variant only when something
//! renders it (avoids dead-code arising under `-D warnings` before those
//! plans land).

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};

use crate::tui_rata::app::App;

// ── OverlayKind ────────────────────────────────────────────────────────────────

/// Which overlay (if any) is currently active. `App.active_overlay` is a
/// single `Option`, never a stack — only one overlay may be visible at a
/// time (UI-SPEC §4 "Overlay exclusivity").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayKind {
    /// Browse-only Skills Hub (TUI-03, D-06/D-07).
    SkillsHub,
    /// Approval request for a gated `terminal`/`execute_code` call
    /// (TUI-02, D-03/D-04). `cache_key` scopes the `[s]ession` grant to the
    /// SAME `ApprovalsStore` key the headless flow uses.
    Approval {
        /// `normalize_command`-derived key for the `[s]ession` grant.
        cache_key: String,
        /// The command / tool label shown on the first detail line.
        label: String,
        /// The guardrail reason shown on the second detail line.
        detail: String,
    },
    /// Secret-input request (TUI-02, D-03). The typed value is held in a
    /// redacting buffer that can NEVER `Debug`/`Display` its plaintext (T-36.6.2-03-02).
    Secret {
        /// The prompt shown on the first detail line.
        prompt: String,
        /// The masked buffer — rendered only as `•` bullets sized by its length.
        masked_input: RedactedSecret,
    },
    /// Privileged-action confirmation for a `sudo`-prefixed command
    /// (TUI-02, D-03/D-05). Shares the approval chrome; only `[y]es`/`[n]o`/Esc.
    Sudo {
        /// `normalize_command`-derived key (parity with `Approval`).
        cache_key: String,
        /// The command label shown in the detail line.
        label: String,
    },
    /// The `?` Help overlay (TUI-02, D-08/D-09) — lists every registered
    /// keybinding from the display-only `build_help_registry()`. Scrollable
    /// via `app.help_scroll`; no security-critical semantics (Esc closes
    /// with no side effect, unlike Approval/Secret/Sudo).
    Help,
    /// The `/model`/`/provider` picker (TUI-INPUT-02, D-06/D-07). `step` and
    /// `selected_provider` are the ONLY state this variant carries — the
    /// mutable filter/selection index live on `App`
    /// (`model_picker_filter`/`model_picker_selected`), mirroring the
    /// `skills_hub_filter`/`skills_hub_selected` convention.
    ModelPicker {
        /// Which step of the flow is active (see [`PickerStep`]).
        step: PickerStep,
        /// The provider chosen at step 1, once `/model` has advanced past
        /// it. `None` while still on step 1 or in the single-step
        /// `/provider` flow.
        selected_provider: Option<String>,
    },
    /// A model-invoked `clarify` question (Phase 41.1 Plan 10, G-41.1-1).
    /// Renders as a ratatui overlay instead of `clarify_tool.rs`'s raw
    /// `println!` fallback — the fix for the terminal-corruption bug. The
    /// mutable selection index lives on `App` (`clarify_selected`), mirroring
    /// the `skills_hub_selected`/`palette_selected` convention.
    Clarify {
        /// The question text to present to the user.
        question: String,
        /// The option labels the user chooses from.
        choices: Vec<String>,
        /// The `PendingClarifyRegistry` key — threaded through to
        /// `App::answer_clarify`/`cancel_clarify` on Enter/Esc.
        clarify_id: String,
    },
}

/// Which step of the `/model`/`/provider` picker is active (Phase 36.6.3
/// Plan 03, D-06/D-07). `/model` (bare) is a two-step flow: `Provider` then
/// `Model`. `/provider` (bare) is a single-step flow using `ProviderOnly` —
/// Enter applies the selected row's `default_model` immediately and never
/// advances to `Model`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerStep {
    /// `/model` step 1 — pick a provider.
    Provider,
    /// `/model` step 2 — pick that provider's model.
    Model,
    /// `/provider`'s single-step flow.
    ProviderOnly,
}

/// A secret-input buffer that can be appended to and measured, but whose
/// plaintext can NEVER be `Debug`- or `Display`-formatted, logged, or
/// interpolated (T-36.6.2-03-02). It exposes only `push`/`pop` (mutation) and
/// `display_len`/`is_empty` (length) — never a `&str`/`String` accessor — so
/// `OverlayKind::Secret` can be safely `{:?}`-formatted (the plaintext is
/// redacted) and the render layer can size the `•` mask without ever reading
/// the characters.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RedactedSecret {
    inner: String,
}

impl RedactedSecret {
    /// Append one typed character to the buffer.
    pub fn push(&mut self, c: char) {
        self.inner.push(c);
    }

    /// Remove the last character (Backspace). Returns it (discarded by callers).
    pub fn pop(&mut self) -> Option<char> {
        self.inner.pop()
    }

    /// Number of `•` bullets to render — the buffer's character count. This is
    /// the ONLY thing the render layer may learn about the buffer.
    pub fn display_len(&self) -> usize {
        self.inner.chars().count()
    }

    /// `true` when nothing has been typed yet.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl std::fmt::Debug for RedactedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // NEVER prints the plaintext — only a redacted length marker.
        write!(f, "RedactedSecret([REDACTED; {} chars])", self.display_len())
    }
}

// ── centered_rect ───────────────────────────────────────────────────────────────

/// Standard ratatui "popup" centering recipe (RESEARCH Pattern 1): nested
/// Vertical `[Min(0), Length(height), Min(0)]` then Horizontal
/// `[Percentage((100-percent_x)/2), Percentage(percent_x), Percentage((100-percent_x)/2)]`,
/// taking the middle cell of both splits. `height` is always a literal row
/// count (a `Length`) — callers who want a percentage-of-frame height (e.g.
/// the Skills Hub's `Percentage(70)`, UI-SPEC §3) compute that row count
/// from `area.height` before calling this fn, so the centering math itself
/// never branches on percent-vs-fixed height.
///
/// Deterministic: repeated calls with the same inputs return the same `Rect`,
/// and the result always falls inside `area` (no overflow/panic), including
/// on odd terminal widths where `(100 - percent_x) / 2` truncates.
pub(crate) fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

// ── render dispatcher ────────────────────────────────────────────────────────────

/// Overlay render dispatcher — called from `event_loop.rs`'s draw closure
/// strictly AFTER the base `ui(f, app)` call. Renders nothing when
/// `app.active_overlay` is `None` (base frame is left untouched).
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    match &app.active_overlay {
        Some(OverlayKind::SkillsHub) => render_skills_hub(frame, app, area),
        Some(OverlayKind::Approval { .. }) => render_approval(frame, app),
        Some(OverlayKind::Secret { .. }) => render_secret(frame, app),
        Some(OverlayKind::Sudo { .. }) => render_sudo(frame, app),
        Some(OverlayKind::Help) => render_help(frame, app, area),
        Some(OverlayKind::ModelPicker { .. }) => render_model_picker(frame, app, area),
        Some(OverlayKind::Clarify { .. }) => render_clarify(frame, app),
        None => {}
    }
}

// ── Skills Hub (TUI-03, D-06/D-07) ──────────────────────────────────────────────

/// `true` when the underlying `SkillRegistry` (if any) has zero skills at
/// all — independent of the live filter query. Drives the "No skills
/// installed." empty state (UI-SPEC §3), distinct from "filter matches
/// nothing" (`skills_hub_filtered` returning empty while the registry is
/// non-empty).
fn skills_registered_at_all(app: &App) -> bool {
    app.skill_registry
        .as_ref()
        .map(|sr| sr.list().is_empty())
        .unwrap_or(true)
}

/// The Skills Hub's live-filtered record list. Filtering goes ENTIRELY
/// through `ironhermes_core::skills::search_matches` (D-06 verbatim reuse —
/// no bespoke/byte-index filter logic) so unicode skill names/descriptions
/// match safely. This is the single source of truth for both rendering
/// (`render_skills_hub_list`/`render_skills_hub_detail`) and key routing
/// (`App::handle_skills_hub_key`'s selection movement/clamping and Enter's
/// trigger-name lookup) — one filtered view, never two independently
/// computed lists that could drift.
pub(crate) fn skills_hub_filtered(app: &App) -> Vec<&ironhermes_core::SkillRecord> {
    match &app.skill_registry {
        Some(sr) => sr
            .list()
            .iter()
            .filter(|r| {
                ironhermes_core::skills::search_matches(
                    &r.name,
                    &r.description,
                    &app.skills_hub_filter,
                )
            })
            .collect(),
        None => Vec::new(),
    }
}

/// Renders the Skills Hub overlay: outer Double/Yellow/BOLD block (UI-SPEC
/// §3), split into a `Percentage(35)` filter/list pane and a `Percentage(65)`
/// detail pane. Centered at `Percentage(80)` width, `Percentage(70)` height
/// of the frame (computed here as a row count and passed to `centered_rect`).
fn render_skills_hub(frame: &mut Frame, app: &App, area: Rect) {
    // UI-SPEC §3: Percentage(80) x Percentage(70) — centered_rect takes a
    // literal row count, so compute 70% of the frame height here.
    let modal_height = ((area.height as u32) * 70 / 100) as u16;
    let rect = centered_rect(80, modal_height.max(5), area);
    frame.render_widget(Clear, rect);

    let outer = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(
            Line::from(Span::styled(
                " Skills Hub ",
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .left_aligned(),
        )
        .title(
            Line::from(Span::styled(
                " Ctrl+K / Esc close ",
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .right_aligned(),
        );
    let inner = outer.inner(rect);
    frame.render_widget(outer, rect);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(inner);

    render_skills_hub_list(frame, app, cols[0]);
    render_skills_hub_detail(frame, app, cols[1]);
}

/// Left pane: filter prompt row + filtered skill name rows (selected row
/// REVERSED, UI-SPEC §3). Only the outer container gets the Double accent —
/// this inner pane keeps `BorderType::Plain` ("one overlay = one accent
/// frame").
fn render_skills_hub_list(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().border_type(BorderType::Plain);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    let filter_line = if app.skills_hub_filter.is_empty() {
        Line::from(vec![
            Span::raw("> "),
            Span::styled(
                "type to filter…",
                Style::default().add_modifier(Modifier::DIM),
            ),
        ])
    } else {
        Line::from(format!("> {}_", app.skills_hub_filter))
    };
    lines.push(filter_line);

    if skills_registered_at_all(app) {
        lines.push(Line::from("No skills installed."));
    } else {
        let filtered = skills_hub_filtered(app);
        if filtered.is_empty() {
            lines.push(Line::from(format!(
                "No skills match \"{}\".",
                app.skills_hub_filter
            )));
        } else {
            for (i, rec) in filtered.iter().enumerate() {
                let style = if i == app.skills_hub_selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(rec.name.clone(), style)));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Right pane: fixed field order per UI-SPEC §3 — name (BOLD), description
/// (wrapped), `Trigger: /{name}` (Cyan), `Source: {source:?}`,
/// `Enabled: yes` (`SkillRegistry::list()` only ever returns already-enabled
/// skills — RESEARCH Open Question 3, settled), blank pad, footer (DIM).
fn render_skills_hub_detail(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().border_type(BorderType::Plain);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    if skills_registered_at_all(app) {
        lines.push(Line::from("No skills installed."));
    } else {
        let filtered = skills_hub_filtered(app);
        if filtered.is_empty() {
            lines.push(Line::from(format!(
                "No skills match \"{}\".",
                app.skills_hub_filter
            )));
        } else if let Some(rec) = filtered.get(app.skills_hub_selected) {
            lines.push(Line::from(Span::styled(
                rec.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(rec.description.clone()));
            lines.push(Line::from(Span::styled(
                format!("Trigger: /{}", rec.name),
                Style::default().fg(Color::Cyan),
            )));
            lines.push(Line::from(format!("Source: {:?}", rec.source)));
            lines.push(Line::from("Enabled: yes"));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "[Enter] insert trigger   [Esc] close",
                Style::default().add_modifier(Modifier::DIM),
            )));
            // Phase 41.1 Plan 02 (D-03): discoverability hint clarifying that
            // Enter is browse/insert-only (never runs) — the Skills Hub stays
            // compose-first; running one-shot is the `/` palette / typed path.
            lines.push(Line::from(Span::styled(
                "Enter: insert into input (won't run) · edit before sending",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// ── Approval / Secret / Sudo modals (TUI-02, D-03/D-04/D-05, UI-SPEC §2) ─────────

/// Hard-wrap `raw` logical lines to `width` columns (char boundaries), capping at
/// exactly 3 detail rows: a longer body truncates to 3 rows with `…` on the 3rd
/// (UI-SPEC §2). Fewer than 3 rows are NOT padded here — `render_modal` pads.
fn detail_rows(raw: &[String], width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows: Vec<String> = Vec::new();
    for s in raw {
        if s.is_empty() {
            rows.push(String::new());
            continue;
        }
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let end = (i + width).min(chars.len());
            rows.push(chars[i..end].iter().collect());
            i = end;
        }
    }
    if rows.len() > 3 {
        rows.truncate(3);
        if let Some(last) = rows.last_mut() {
            let mut c: Vec<char> = last.chars().collect();
            if c.len() >= width && !c.is_empty() {
                c.pop();
            }
            c.push('…');
            *last = c.into_iter().collect();
        }
    }
    rows
}

/// The one deliberate non-neutral footer color: the fail-closed/cancel segment.
fn deny_span(text: &str) -> Span<'static> {
    Span::styled(
        text.to_string(),
        Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
    )
}

/// A BOLD bracketed hotkey letter (default fg), e.g. `[y]`.
fn hotkey_span(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::default().add_modifier(Modifier::BOLD))
}

/// Append the `(+N more pending)` DIM queue hint when other requests are queued
/// (UI-SPEC §2 queue discipline). Phase 41.1 Plan 10: counts BOTH
/// `approval_queue` and `clarify_queue` — the two queues are now drained by
/// one unified `surface_next_overlay` (approval first, then clarify), so the
/// hint reflects the true total pending count regardless of which overlay is
/// currently showing.
fn append_queue_hint(spans: &mut Vec<Span<'static>>, app: &App) {
    let pending = app.approval_queue.len() + app.clarify_queue.len();
    if pending > 0 {
        spans.push(Span::styled(
            format!("  (+{pending} more pending)"),
            Style::default().add_modifier(Modifier::DIM),
        ));
    }
}

/// The shared centered modal: `Clear` + Double/Yellow/BOLD `Block`, then the
/// 9-row layout (top border+title / pad / 3 detail / pad / footer / pad / bottom
/// border). `detail` is truncated/padded to exactly 3 rows here (D-03: all three
/// variants share this chrome; only title/detail/footer differ).
fn render_modal(
    frame: &mut Frame,
    rect: Rect,
    title: &str,
    detail: Vec<Line<'static>>,
    footer: Line<'static>,
) {
    frame.render_widget(Clear, rect);

    let outer = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Line::from(Span::styled(
            format!(" {title} "),
            Style::default().add_modifier(Modifier::BOLD),
        )));
    let inner = outer.inner(rect);
    frame.render_widget(outer, rect);

    let mut detail = detail;
    detail.truncate(3);
    while detail.len() < 3 {
        detail.push(Line::from(""));
    }

    // Inner is 7 rows (9 minus the two borders): pad / 3 detail / pad / footer / pad.
    let mut lines: Vec<Line> = Vec::with_capacity(7);
    lines.push(Line::from(""));
    lines.extend(detail);
    lines.push(Line::from(""));
    lines.push(footer);
    lines.push(Line::from(""));

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Approval modal (D-03/D-04). Title `"Approve Action"`; detail `Run: {label}` +
/// `{detail}`; footer `[y]es [n]o [s]ession Esc=deny`.
fn render_approval(frame: &mut Frame, app: &App) {
    if let Some(OverlayKind::Approval { label, detail, .. }) = &app.active_overlay {
        let rect = centered_rect(60, 9, frame.area());
        let width = rect.width.saturating_sub(2) as usize;
        let raw = vec![format!("Run: {label}"), detail.clone()];
        let detail_lines: Vec<Line> = detail_rows(&raw, width)
            .into_iter()
            .map(Line::from)
            .collect();

        let mut footer = vec![
            hotkey_span("[y]"),
            Span::raw("es"),
            Span::raw("   "),
            hotkey_span("[n]"),
            Span::raw("o"),
            Span::raw("   "),
            hotkey_span("[s]"),
            Span::raw("ession"),
            Span::raw("   "),
            deny_span("Esc=deny"),
        ];
        append_queue_hint(&mut footer, app);
        render_modal(frame, rect, "Approve Action", detail_lines, Line::from(footer));
    }
}

/// Secret modal (D-03). Title `"Secret Required"`; detail `{prompt}` + a line of
/// `•` bullets sized by the buffer's length (never the literal characters); footer
/// `Enter submit   Esc=cancel`.
fn render_secret(frame: &mut Frame, app: &App) {
    if let Some(OverlayKind::Secret {
        prompt,
        masked_input,
    }) = &app.active_overlay
    {
        let rect = centered_rect(60, 9, frame.area());
        // Read ONLY the length — never the plaintext (T-36.6.2-03-02).
        let bullets = "•".repeat(masked_input.display_len());
        let detail_lines: Vec<Line> = vec![Line::from(prompt.clone()), Line::from(bullets)];

        let mut footer = vec![
            hotkey_span("Enter"),
            Span::raw(" submit"),
            Span::raw("   "),
            deny_span("Esc=cancel"),
        ];
        append_queue_hint(&mut footer, app);
        render_modal(frame, rect, "Secret Required", detail_lines, Line::from(footer));
    }
}

/// Sudo modal (D-03/D-05). Title `"Confirm Privileged Action"`; detail
/// `{label} requires elevated privileges.`; footer `[y]es [n]o Esc=deny`.
fn render_sudo(frame: &mut Frame, app: &App) {
    if let Some(OverlayKind::Sudo { label, .. }) = &app.active_overlay {
        let rect = centered_rect(60, 9, frame.area());
        let width = rect.width.saturating_sub(2) as usize;
        let raw = vec![format!("{label} requires elevated privileges.")];
        let detail_lines: Vec<Line> = detail_rows(&raw, width)
            .into_iter()
            .map(Line::from)
            .collect();

        let mut footer = vec![
            hotkey_span("[y]"),
            Span::raw("es"),
            Span::raw("   "),
            hotkey_span("[n]"),
            Span::raw("o"),
            Span::raw("   "),
            deny_span("Esc=deny"),
        ];
        append_queue_hint(&mut footer, app);
        render_modal(
            frame,
            rect,
            "Confirm Privileged Action",
            detail_lines,
            Line::from(footer),
        );
    }
}

// ── Clarify overlay (Phase 41.1 Plan 10, G-41.1-1) ──────────────────────────────

/// Clarify modal — the fix for G-41.1-1 (model-invoked clarify corrupting the
/// transcript via a raw stdout `println!`). Shares the Double/Yellow modal
/// chrome with Approval/Sudo (UI-SPEC §2), but is built directly here rather
/// than through `render_modal`: `render_modal`'s layout is a fixed 3 detail
/// rows, while `choices` is model-controlled (2-10 per the `clarify` tool
/// schema) and a fixed 3-row cap would truncate legitimate options. The
/// `app.clarify_selected` row is REVERSED (mirrors `render_skills_hub_list`'s
/// selected-row convention).
fn render_clarify(frame: &mut Frame, app: &App) {
    if let Some(OverlayKind::Clarify {
        question,
        choices,
        ..
    }) = &app.active_overlay
    {
        let area = frame.area();
        let width_pct = 60u16;
        let inner_width = ((area.width as u32 * width_pct as u32 / 100).saturating_sub(2)).max(1) as usize;
        let question_rows = detail_rows(std::slice::from_ref(question), inner_width);

        // Layout: top/bottom border (2) + pad (1) + question rows + pad (1) +
        // one row per choice + pad (1) + footer (1) + pad (1).
        let height = 2
            + 1
            + question_rows.len() as u16
            + 1
            + choices.len() as u16
            + 1
            + 1
            + 1;
        let rect = centered_rect(width_pct, height, area);
        frame.render_widget(Clear, rect);

        let outer = Block::bordered()
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::Yellow))
            .title(Line::from(Span::styled(
                " Clarify ",
                Style::default().add_modifier(Modifier::BOLD),
            )));
        let inner = outer.inner(rect);
        frame.render_widget(outer, rect);

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(""));
        lines.extend(question_rows.into_iter().map(Line::from));
        lines.push(Line::from(""));
        for (i, choice) in choices.iter().enumerate() {
            let style = if i == app.clarify_selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(format!("{i}: {choice}"), style)));
        }
        lines.push(Line::from(""));

        let mut footer = vec![
            hotkey_span("Up/Down"),
            Span::raw(" select"),
            Span::raw("   "),
            hotkey_span("Enter"),
            Span::raw(" answer"),
            Span::raw("   "),
            deny_span("Esc=cancel"),
        ];
        append_queue_hint(&mut footer, app);
        lines.push(Line::from(footer));
        lines.push(Line::from(""));

        frame.render_widget(Paragraph::new(lines), inner);
    }
}

// ── Help overlay (TUI-02, D-08/D-09, UI-SPEC §4) ────────────────────────────────

/// Renders the `?` Help overlay: centered `Percentage(60)` x `Percentage(50)`,
/// shared Double/Yellow/BOLD chrome, title `"Help — Keybindings"`. One line
/// per `build_help_registry().help_entries()` entry — key display BOLD, then
/// `"  —  "` plain, then the description plain. Scrollable via
/// `app.help_scroll` (clamped so `PageDown` past the end never scrolls into
/// blank space).
fn render_help(frame: &mut Frame, app: &App, area: Rect) {
    let modal_height = ((area.height as u32) * 50 / 100) as u16;
    let rect = centered_rect(60, modal_height.max(5), area);
    frame.render_widget(Clear, rect);

    let outer = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Line::from(Span::styled(
            " Help — Keybindings ",
            Style::default().add_modifier(Modifier::BOLD),
        )));
    let inner = outer.inner(rect);
    frame.render_widget(outer, rect);

    let registry = crate::tui_rata::keybindings::build_help_registry();
    let entries = registry.help_entries();

    let lines: Vec<Line> = entries
        .iter()
        .map(|(key_display, description, _ctx)| {
            Line::from(vec![
                Span::styled(key_display.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("  —  "),
                Span::raw(description.to_string()),
            ])
        })
        .collect();

    let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
    let scroll = app.help_scroll.min(max_scroll);

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((scroll, 0)),
        inner,
    );
}

/// Number of Help entries — used by `App::handle_key`'s PageUp/PageDown
/// clamping so `help_scroll` never scrolls past the last entry.
pub(crate) fn help_entry_count() -> usize {
    crate::tui_rata::keybindings::build_help_registry()
        .help_entries()
        .len()
}

// ── Model/Provider Picker (TUI-INPUT-02, D-06/D-07) ─────────────────────────

/// The picker's live-filtered provider list (`/model` step 1, and the
/// entirety of the single-step `/provider` flow). Filtering goes through
/// `ironhermes_core::skills::search_matches` (Don't Hand-Roll — same
/// discipline as `skills_hub_filtered`), matching against the provider name
/// and base_url. Sorted, credential-free — straight from
/// `ProviderResolver::providers()` (D-10).
pub(crate) fn model_picker_providers_filtered(
    app: &App,
) -> Vec<ironhermes_core::commands::context::ProviderPickerRow> {
    app.resolver
        .providers()
        .into_iter()
        .filter(|row| {
            ironhermes_core::skills::search_matches(
                &row.name,
                &row.base_url,
                &app.model_picker_filter,
            )
        })
        .collect()
}

/// `/model` step 2's live-filtered model list for `selected_provider` — the
/// sparse, honest "your configured models" list (D-10). Empty when the
/// provider name isn't found (shouldn't happen in practice — step 2 is only
/// reachable after step 1 picked a real row).
pub(crate) fn model_picker_models_filtered(app: &App, selected_provider: &str) -> Vec<String> {
    app.resolver
        .providers()
        .into_iter()
        .find(|row| row.name == selected_provider)
        .map(|row| row.models)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| ironhermes_core::skills::search_matches(m, "", &app.model_picker_filter))
        .collect()
}

/// Left-aligned step title (UI-SPEC "`/model` + `/provider` picker overlay").
fn step_title_left(step: PickerStep, selected_provider: &Option<String>) -> String {
    match step {
        PickerStep::Provider => " Select Provider — Step 1 of 2 ".to_string(),
        PickerStep::Model => format!(
            " Select Model — {} ",
            selected_provider.as_deref().unwrap_or("")
        ),
        PickerStep::ProviderOnly => " Select Provider ".to_string(),
    }
}

/// Right-aligned step title — the Esc affordance ("close" vs "back").
fn step_title_right(step: PickerStep) -> &'static str {
    match step {
        PickerStep::Provider | PickerStep::ProviderOnly => " Esc close ",
        PickerStep::Model => " Esc back ",
    }
}

/// Renders `entries` (label, is_current) into `lines`: `Modifier::REVERSED`
/// on the selected index, a Cyan `*` prefix on the row matching
/// `is_current` (the codebase's existing "current value" convention, see
/// Skills Hub's `Trigger:` line), and a DIM
/// `"(+N more — narrow your filter)"` hint (mirrors `append_queue_hint`'s
/// convention) when `entries` exceeds `visible_rows`.
fn push_picker_rows<'a>(
    lines: &mut Vec<Line<'static>>,
    entries: impl Iterator<Item = (&'a str, bool)>,
    selected: usize,
    visible_rows: usize,
) {
    let entries: Vec<(&str, bool)> = entries.collect();
    let visible_rows = visible_rows.max(1);
    let (shown, overflow) = if entries.len() > visible_rows {
        (visible_rows.saturating_sub(1).max(1), true)
    } else {
        (entries.len(), false)
    };
    for (i, (name, is_current)) in entries.iter().take(shown).enumerate() {
        let marker = if *is_current {
            Span::styled("* ", Style::default().fg(Color::Cyan))
        } else {
            Span::raw("  ")
        };
        let style = if i == selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            marker,
            Span::styled(name.to_string(), style),
        ]));
    }
    if overflow {
        lines.push(Line::from(Span::styled(
            format!("(+{} more — narrow your filter)", entries.len() - shown),
            Style::default().add_modifier(Modifier::DIM),
        )));
    }
}

/// Renders the `/model`/`/provider` picker overlay: outer Double/Yellow/BOLD
/// block (IDENTICAL accent to every other modal overlay), split into a
/// `Percentage(35)` filter/list pane and a `Percentage(65)` detail pane.
/// Centered at `Percentage(80)` width, `Percentage(70)` height of the frame —
/// a byte-for-byte structural clone of `render_skills_hub` (D-07).
fn render_model_picker(frame: &mut Frame, app: &App, area: Rect) {
    let (step, selected_provider) = match &app.active_overlay {
        Some(OverlayKind::ModelPicker {
            step,
            selected_provider,
        }) => (*step, selected_provider.clone()),
        _ => return,
    };

    let modal_height = ((area.height as u32) * 70 / 100) as u16;
    let rect = centered_rect(80, modal_height.max(5), area);
    frame.render_widget(Clear, rect);

    let outer = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(
            Line::from(Span::styled(
                step_title_left(step, &selected_provider),
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .left_aligned(),
        )
        .title(
            Line::from(Span::styled(
                step_title_right(step),
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .right_aligned(),
        );
    let inner = outer.inner(rect);
    frame.render_widget(outer, rect);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(inner);

    render_model_picker_list(frame, app, cols[0], step, &selected_provider);
    render_model_picker_detail(frame, app, cols[1], step, &selected_provider);
}

/// Left pane: filter prompt row + filtered provider/model rows (selected row
/// REVERSED, Cyan `*` on the current provider/model). Only the outer
/// container gets the Double accent — this inner pane keeps
/// `BorderType::Plain` ("one overlay = one accent frame").
fn render_model_picker_list(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    step: PickerStep,
    selected_provider: &Option<String>,
) {
    let block = Block::bordered().border_type(BorderType::Plain);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    let filter_line = if app.model_picker_filter.is_empty() {
        Line::from(vec![
            Span::raw("> "),
            Span::styled(
                "type to filter…",
                Style::default().add_modifier(Modifier::DIM),
            ),
        ])
    } else {
        Line::from(format!("> {}_", app.model_picker_filter))
    };
    lines.push(filter_line);

    // Rows available below the filter prompt, sized to the pane's actual
    // inner height (UI-SPEC: "sized to the pane's actual inner height, not a
    // fixed constant like the palette's 6").
    let visible_rows = (inner.height as usize).saturating_sub(1);

    match step {
        PickerStep::Provider | PickerStep::ProviderOnly => {
            let providers = model_picker_providers_filtered(app);
            if providers.is_empty() {
                lines.push(Line::from(format!(
                    "No providers match \"{}\".",
                    app.model_picker_filter
                )));
            } else {
                push_picker_rows(
                    &mut lines,
                    providers.iter().map(|r| (r.name.as_str(), r.is_current)),
                    app.model_picker_selected,
                    visible_rows,
                );
            }
        }
        PickerStep::Model => {
            let provider = selected_provider.as_deref().unwrap_or("");
            let current_model = app.resolver.resolve_for_main().default_model.clone();
            let models = model_picker_models_filtered(app, provider);
            if models.is_empty() {
                lines.push(Line::from(format!(
                    "No models match \"{}\".",
                    app.model_picker_filter
                )));
            } else {
                push_picker_rows(
                    &mut lines,
                    models.iter().map(|m| (m.as_str(), *m == current_model)),
                    app.model_picker_selected,
                    visible_rows,
                );
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Right pane: fixed field order per step — provider/model name (BOLD),
/// base_url (or a graceful fallback when absent — `ProviderPickerRow`
/// carries no context-length data, so an empty `base_url` renders an
/// explicit "(provider default)" line rather than a blank row), default
/// model, blank pad, footer (DIM). Step 2 additionally shows the D-10
/// honest-framing note. Detail pane renders ONLY provider/model identity +
/// base_url + default-model info — NEVER credential material
/// (T-36.6.3-03-01).
fn render_model_picker_detail(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    step: PickerStep,
    selected_provider: &Option<String>,
) {
    let block = Block::bordered().border_type(BorderType::Plain);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    match step {
        PickerStep::Provider | PickerStep::ProviderOnly => {
            let providers = model_picker_providers_filtered(app);
            if providers.is_empty() {
                lines.push(Line::from(format!(
                    "No providers match \"{}\".",
                    app.model_picker_filter
                )));
            } else if let Some(row) = providers.get(app.model_picker_selected) {
                lines.push(Line::from(Span::styled(
                    row.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                if row.base_url.is_empty() {
                    lines.push(Line::from("Base URL: (provider default)"));
                } else {
                    lines.push(Line::from(format!("Base URL: {}", row.base_url)));
                }
                lines.push(Line::from(format!("Default model: {}", row.default_model)));
                lines.push(Line::from(""));
                let hint = match step {
                    PickerStep::Provider => "[Enter] next: choose model   [Esc] close",
                    _ => "[Enter] switch provider   [Esc] close",
                };
                lines.push(Line::from(Span::styled(
                    hint,
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
        }
        PickerStep::Model => {
            let provider = selected_provider.as_deref().unwrap_or("");
            let models = model_picker_models_filtered(app, provider);
            if models.is_empty() {
                lines.push(Line::from(format!(
                    "No models match \"{}\".",
                    app.model_picker_filter
                )));
            } else if let Some(model) = models.get(app.model_picker_selected) {
                lines.push(Line::from(Span::styled(
                    model.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(format!("Provider: {provider}")));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "These are your configured models, not a full catalog.",
                    Style::default().add_modifier(Modifier::DIM),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "[Enter] apply & switch   [Esc] back",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// ── Tests ────────────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    /// Renders `f` into a `width`x`height` TestBackend buffer and flattens
    /// every cell symbol into a single string (row-major, newline-separated)
    /// for substring assertions. Mirrors `ui.rs`'s existing TestBackend
    /// discipline: render a REAL frame, assert on buffer content — never
    /// re-derive the layout math inside the assertion (RESEARCH Anti-Pattern).
    fn render_to_text(width: u16, height: u16, draw: impl FnOnce(&mut ratatui::Frame)) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(draw).unwrap();
        let buf = terminal.backend().buffer();
        let mut s = String::new();
        for y in 0..height {
            for x in 0..width {
                if let Some(cell) = buf.cell((x, y)) {
                    s.push_str(cell.symbol());
                }
            }
            s.push('\n');
        }
        s
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap()
    }

    /// Builds an `App` with `skill_registry` populated from `skills`
    /// (name, description) pairs, each written as a real `SKILL.md` under an
    /// isolated fake `$IRONHERMES_HOME`/`$HOME` (mirrors the
    /// `tui_attach_at_path::app_with_store` convention in `app.rs` —
    /// `SkillRegistry` has no in-memory/from-records constructor reachable
    /// from this crate, so populated-registry tests must go through the real
    /// on-disk scan against an isolated root; this also keeps the test from
    /// picking up this dev machine's real `~/.ironhermes/skills` /
    /// `~/.agents/skills` content).
    fn app_with_skills(skills: &[(&str, &str)]) -> (App, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        let skills_dir = home.path().join("skills");
        for (name, desc) in skills {
            let dir = skills_dir.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {desc}\n---\nbody\n"),
            )
            .unwrap();
        }
        // SAFETY: mutates process-global env state; guarded by `env_lock()`
        // and this crate's tests run `--test-threads=1` (see project memory
        // on the cross-module IRONHERMES_HOME race).
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home.path());
            std::env::set_var("HOME", home.path());
        }
        let config = ironhermes_core::config::SkillsConfig::default();
        let registry = ironhermes_core::SkillRegistry::load_with_config(home.path(), &config);
        let mut app = App::new_test_empty();
        app.skill_registry = Some(std::sync::Arc::new(registry));
        (app, home)
    }

    fn press(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent {
            code,
            modifiers: crossterm::event::KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    // ── Task 1: foundation ───────────────────────────────────────────────────

    #[test]
    fn centered_rect_is_deterministic_on_odd_widths() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 81,
            height: 41,
        };
        let r1 = centered_rect(80, 12, area);
        let r2 = centered_rect(80, 12, area);
        assert_eq!(r1, r2, "centered_rect must be deterministic");
        assert!(r1.x >= area.x, "rect must not overflow left of parent");
        assert!(r1.y >= area.y, "rect must not overflow above parent");
        assert!(
            r1.x + r1.width <= area.x + area.width,
            "rect must not overflow right of parent (odd width={})",
            area.width
        );
        assert!(
            r1.y + r1.height <= area.y + area.height,
            "rect must not overflow below parent"
        );
    }

    #[test]
    fn overlay_render_dispatch_matches_active_kind() {
        let app = App::new_test_empty();
        assert!(app.active_overlay.is_none());

        let text_without = render_to_text(80, 24, |f| {
            crate::tui_rata::ui::ui(f, &app);
        });
        let text_with_dispatch = render_to_text(80, 24, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert_eq!(
            text_without, text_with_dispatch,
            "dispatcher must render nothing when active_overlay is None"
        );
    }

    #[test]
    fn esc_with_no_overlay_still_clears_textarea() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("hello");
        assert!(app.active_overlay.is_none());

        app.handle_key(press(crossterm::event::KeyCode::Esc));

        assert_eq!(
            app.textarea.lines().join(""),
            "",
            "Esc with no active overlay must still clear_textarea() (existing behavior preserved)"
        );
    }

    // ── Task 2: Skills Hub ───────────────────────────────────────────────────

    #[test]
    fn skills_hub_filter_narrows_list_live() {
        let _guard = env_lock();
        let (mut app, _home) = app_with_skills(&[
            ("alpha-widgets", "does alpha things"),
            ("beta-gizmos", "does beta things"),
            ("gamma-tools", "does gamma things"),
        ]);
        app.active_overlay = Some(OverlayKind::SkillsHub);
        app.skills_hub_filter = "beta".to_string();

        let text = render_to_text(100, 30, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(
            text.contains("beta-gizmos"),
            "expected matching skill name in buffer:\n{text}"
        );
        assert!(
            !text.contains("alpha-widgets"),
            "non-matching skill must be filtered out:\n{text}"
        );
        assert!(
            !text.contains("gamma-tools"),
            "non-matching skill must be filtered out:\n{text}"
        );
    }

    #[test]
    fn skills_hub_enter_inserts_trigger_never_executes() {
        let _guard = env_lock();
        let (mut app, _home) = app_with_skills(&[("solo-skill", "the only skill")]);
        app.active_overlay = Some(OverlayKind::SkillsHub);
        app.skills_hub_selected = 0;

        let history_len_before = app.history.len();
        let overlays_before = app.pending_skill_overlays.len();

        app.handle_key(press(crossterm::event::KeyCode::Enter));

        assert_eq!(
            app.textarea.lines().join(""),
            "/solo-skill",
            "Enter must insert the literal trigger text"
        );
        assert!(
            app.active_overlay.is_none(),
            "Enter must close the Skills Hub overlay"
        );
        assert_eq!(
            app.history.len(),
            history_len_before,
            "browse-only: must NOT push a history message (that only happens via \
             apply_slash_outcome/SlashOutcome::SkillActivated, T-36.6.2-01-01)"
        );
        assert_eq!(
            app.pending_skill_overlays.len(),
            overlays_before,
            "browse-only: must NOT activate the skill (D-07)"
        );
    }

    #[test]
    fn skills_hub_empty_states() {
        // Zero skills registered at all.
        let mut app = App::new_test_empty();
        assert!(app.skill_registry.is_none());
        app.active_overlay = Some(OverlayKind::SkillsHub);

        let text = render_to_text(100, 30, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });
        assert!(text.contains("No skills installed."), "buffer:\n{text}");

        // Filter matches nothing.
        let _guard = env_lock();
        let (mut app2, _home) = app_with_skills(&[("real-skill", "a real skill")]);
        app2.active_overlay = Some(OverlayKind::SkillsHub);
        app2.skills_hub_filter = "zzz-no-match".to_string();

        let text2 = render_to_text(100, 30, |f| {
            crate::tui_rata::ui::ui(f, &app2);
            render(f, &app2);
        });
        assert!(
            text2.contains("No skills match \"zzz-no-match\"."),
            "buffer:\n{text2}"
        );
    }

    #[test]
    fn skills_hub_selection_clamps_on_shrink() {
        let _guard = env_lock();
        let (mut app, _home) = app_with_skills(&[
            ("aaa-one", "first"),
            ("aaa-two", "second"),
            ("aaa-three", "third"),
        ]);
        app.active_overlay = Some(OverlayKind::SkillsHub);
        app.skills_hub_selected = 2; // last row while unfiltered (3 matches)

        // Tighten the filter, one keystroke at a time, down to exactly 1 match.
        for c in "aaa-o".chars() {
            app.handle_key(press(crossterm::event::KeyCode::Char(c)));
        }

        assert_eq!(app.skills_hub_filter, "aaa-o");
        let filtered_len = skills_hub_filtered(&app).len();
        assert_eq!(
            filtered_len, 1,
            "expected the filter to narrow to exactly 1 match"
        );
        assert!(
            app.skills_hub_selected < filtered_len,
            "selection must clamp within the filtered list's bounds (got {}, len {})",
            app.skills_hub_selected,
            filtered_len
        );
    }

    // ── Task 2: approval / secret / sudo render ─────────────────────────────

    fn approval(label: &str, detail: &str) -> OverlayKind {
        OverlayKind::Approval {
            cache_key: label.to_string(),
            label: label.to_string(),
            detail: detail.to_string(),
        }
    }

    #[test]
    fn approval_overlay_renders_centered_with_double_border() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(approval("echo hi", "network access"));

        let text = render_to_text(80, 24, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(
            text.contains("Approve Action"),
            "approval title missing:\n{text}"
        );
        assert!(
            text.contains('╔') || text.contains('═') || text.contains('║'),
            "Double-border glyphs missing:\n{text}"
        );
        assert!(text.contains("echo hi"), "command label missing:\n{text}");
        assert!(
            text.contains("[y]") && text.contains("Esc=deny"),
            "approval footer missing:\n{text}"
        );
    }

    #[test]
    fn secret_overlay_masks_input_as_bullets() {
        let mut masked = RedactedSecret::default();
        // A secret whose characters do NOT appear anywhere in the modal chrome
        // or the base UI, so an accidental leak is unambiguous.
        for c in "ZXQJ".chars() {
            masked.push(c);
        }
        let mut app = App::new_test_empty();
        app.active_overlay = Some(OverlayKind::Secret {
            prompt: "provide token".to_string(),
            masked_input: masked,
        });

        let text = render_to_text(80, 24, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(text.contains("Secret Required"), "secret title missing:\n{text}");
        assert!(text.contains('•'), "masked bullets missing:\n{text}");
        for c in ['Z', 'X', 'Q', 'J'] {
            assert!(
                !text.contains(c),
                "literal secret character {c:?} LEAKED into the rendered buffer:\n{text}"
            );
        }
    }

    #[test]
    fn sudo_overlay_shares_approval_chrome() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(OverlayKind::Sudo {
            cache_key: "sudo rm".to_string(),
            label: "sudo rm -rf /tmp/x".to_string(),
        });

        let text = render_to_text(80, 24, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(
            text.contains("Confirm Privileged Action"),
            "sudo title missing:\n{text}"
        );
        // Same Double-border chrome as approval (D-03).
        assert!(
            text.contains('╔') || text.contains('═') || text.contains('║'),
            "sudo must share the Double-border chrome:\n{text}"
        );
        assert!(
            text.contains("[y]") && text.contains("Esc=deny"),
            "sudo footer missing:\n{text}"
        );
    }

    #[test]
    fn approval_detail_truncates_to_three_lines() {
        let mut app = App::new_test_empty();
        let long_detail = "x".repeat(500);
        app.active_overlay = Some(approval("cmd", &long_detail));

        let text = render_to_text(80, 24, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(
            text.contains('…'),
            "over-long detail must truncate the 3rd line with …:\n{text}"
        );
    }

    #[test]
    fn approval_footer_shows_queue_hint() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(approval("cmd", "detail"));
        // Two queued requests behind the active one.
        for _ in 0..2 {
            let (tx, _rx) = tokio::sync::oneshot::channel();
            app.approval_queue.push(crate::tui_rata::approval_gate_tui::ApprovalRequest {
                session_id: "s".to_string(),
                tool_name: "terminal".to_string(),
                reason: "r".to_string(),
                command: "c".to_string(),
                cache_key: "c".to_string(),
                resolve: tx,
            });
        }

        // Wide terminal so the full footer + hint fits the modal (60% width)
        // without clipping — the hint is appended after `Esc=deny`.
        let text = render_to_text(120, 24, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(
            text.contains("(+2 more pending)"),
            "queue hint missing:\n{text}"
        );
    }

    // ── Task 1: Help overlay ────────────────────────────────────────────────

    #[test]
    fn help_overlay_lists_all_registered_keybindings() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(OverlayKind::Help);

        let text = render_to_text(100, 30, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(text.contains("Help"), "help title missing:\n{text}");
        assert!(text.contains("Ctrl+T"), "Ctrl+T missing from help:\n{text}");
        assert!(text.contains("Ctrl+K"), "Ctrl+K missing from help:\n{text}");
        assert!(text.contains("Ctrl+B"), "existing Ctrl+B binding missing:\n{text}");
        assert!(text.contains("Esc"), "existing Esc binding missing:\n{text}");
    }

    /// Phase 36.6.3 Plan 04 (D-08): the `?` Help overlay lists the 2 new
    /// palette discoverability entries — `/` (open the palette) and `Tab`
    /// (palette: insert highlighted command) — as display-only
    /// `build_help_registry` rows. Asserts on the RENDERED buffer (not the
    /// registry vector's own contents), following this test's own
    /// `help_overlay_lists_all_registered_keybindings` precedent.
    #[test]
    fn help_lists_palette_entries() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(OverlayKind::Help);

        // A taller frame than `help_overlay_lists_all_registered_keybindings`
        // (100x30) — the modal's inner height is `area.height * 50 / 100`
        // minus borders, and the 2 new entries are the LAST rows in the
        // registry, so they scroll out of view on a 30-row frame. 40 rows
        // gives an 18-row inner viewport, comfortably fitting all 15 entries
        // unscrolled.
        let text = render_to_text(100, 40, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(
            text.contains("command palette"),
            "palette `/` entry missing from help:\n{text}"
        );
        assert!(
            text.contains("Palette: insert highlighted command"),
            "Tab insert entry missing from help:\n{text}"
        );
    }

    /// Render-layer safety net: an out-of-range `help_scroll` (as could arise
    /// from a future caller bypassing `handle_help_key`'s clamp) must never
    /// panic the `Paragraph::scroll` call — `render_help` re-clamps against
    /// `inner.height` before rendering. The authoritative
    /// PageDown-clamps-`app.help_scroll` behavior is locked by
    /// `app.rs::help_scroll_clamps_at_end` (drives real key events).
    #[test]
    fn help_overlay_render_tolerates_out_of_range_scroll() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(OverlayKind::Help);
        app.help_scroll = 9999;

        // Must not panic even with a tiny viewport (inner.height << entry count).
        let text = render_to_text(100, 12, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(
            text.contains("Help"),
            "help overlay must still render its chrome with an out-of-range scroll:\n{text}"
        );
    }

    // ── Task 1 (36.6.3 Plan 03): Model/Provider Picker ──────────────────────

    /// Build a `ProviderResolver` whose `openrouter` entry has EXACTLY the
    /// given override `models` configured (plus the always-present
    /// `default_model`, deduped) — gives tests full control over step 2's
    /// list without touching `$IRONHERMES_HOME`.
    fn resolver_with_openrouter_models(models: &[&str]) -> ironhermes_core::ProviderResolver {
        let mut config = ironhermes_core::Config::default();
        let mut overrides = std::collections::HashMap::new();
        for m in models {
            overrides.insert(
                m.to_string(),
                ironhermes_core::config_extras::ProviderModelConfig::default(),
            );
        }
        config.providers.insert(
            "openrouter".to_string(),
            ironhermes_core::config::ProviderConfig {
                models: overrides,
                ..Default::default()
            },
        );
        ironhermes_core::ProviderResolver::build(&config)
            .expect("resolver_with_openrouter_models: build must not fail")
    }

    /// `true` when the rendered buffer contains a list ROW for `name` marked
    /// current (`"* {name}"`, `push_picker_rows`'s exact marker+name
    /// sequence) — distinguishes a marked LIST row from `name` merely
    /// appearing elsewhere (e.g. inside the detail pane's `Base URL:` text).
    fn row_marked_current(text: &str, name: &str) -> bool {
        text.contains(&format!("* {name}"))
    }

    /// `true` when the rendered buffer contains a list ROW for `name` NOT
    /// marked current (`"  {name}"`, the two-space non-current prefix).
    fn row_marked_not_current(text: &str, name: &str) -> bool {
        text.contains(&format!("  {name}"))
    }

    #[test]
    fn model_picker_step1_renders() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(OverlayKind::ModelPicker {
            step: PickerStep::Provider,
            selected_provider: None,
        });

        let text = render_to_text(100, 30, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(text.contains("Select Provider — Step 1 of 2"), "buffer:\n{text}");
        assert!(text.contains("openrouter"), "buffer:\n{text}");
        assert!(text.contains("anthropic"), "buffer:\n{text}");
        assert!(text.contains("openai"), "buffer:\n{text}");

        // openrouter is the default main provider (test_deps/Config::default) —
        // the Cyan `*` marker must appear on ITS row only.
        assert!(
            row_marked_current(&text, "openrouter"),
            "current provider must be marked with '*':\n{text}"
        );
        assert!(
            row_marked_not_current(&text, "anthropic"),
            "non-current provider must NOT be marked with '*':\n{text}"
        );
    }

    #[test]
    fn model_picker_step2_renders() {
        let mut app = App::new_test_empty();
        app.resolver = resolver_with_openrouter_models(&["model-a", "model-b"]);
        let current_model = app.resolver.resolve_for_main().default_model.clone();
        app.active_overlay = Some(OverlayKind::ModelPicker {
            step: PickerStep::Model,
            selected_provider: Some("openrouter".to_string()),
        });

        // Wide viewport — the OpenRouter-slug default model name (e.g.
        // "anthropic/claude-sonnet-4-20250514") is longer than a 35%-of-100-col
        // pane; a wide frame ensures it renders unclipped for the marker check.
        let text = render_to_text(160, 30, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(text.contains("Select Model — openrouter"), "buffer:\n{text}");
        assert!(text.contains("model-a"), "buffer:\n{text}");
        assert!(text.contains("model-b"), "buffer:\n{text}");
        assert!(
            row_marked_current(&text, &current_model),
            "current model must be marked with '*':\n{text}"
        );
        assert!(
            row_marked_not_current(&text, "model-a"),
            "non-current model must NOT be marked with '*':\n{text}"
        );
    }

    #[test]
    fn provider_picker_renders() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(OverlayKind::ModelPicker {
            step: PickerStep::ProviderOnly,
            selected_provider: None,
        });

        let text = render_to_text(100, 30, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(text.contains("Select Provider "), "buffer:\n{text}");
        assert!(
            !text.contains("Step 1 of 2"),
            "single-step /provider must NOT show the two-step title:\n{text}"
        );
        assert!(text.contains("openrouter"), "buffer:\n{text}");
        assert!(text.contains("Esc close"), "buffer:\n{text}");
    }

    #[test]
    fn model_picker_step2_sparse_single_model() {
        let mut app = App::new_test_empty();
        // No override models configured for openrouter — sparse case: exactly
        // the default_model, one selectable row.
        app.active_overlay = Some(OverlayKind::ModelPicker {
            step: PickerStep::Model,
            selected_provider: Some("openrouter".to_string()),
        });

        let models = model_picker_models_filtered(&app, "openrouter");
        assert_eq!(
            models.len(),
            1,
            "sparse provider must yield exactly one (default) model, got {models:?}"
        );

        let text = render_to_text(100, 30, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });
        assert!(
            text.contains(&models[0]),
            "the single sparse model must still render:\n{text}"
        );
    }

    #[test]
    fn model_picker_empty_filter_state() {
        let mut app = App::new_test_empty();
        app.active_overlay = Some(OverlayKind::ModelPicker {
            step: PickerStep::Provider,
            selected_provider: None,
        });
        app.model_picker_filter = "zzz-no-such-provider".to_string();

        let text = render_to_text(100, 30, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(
            text.contains("No providers match \"zzz-no-such-provider\"."),
            "buffer:\n{text}"
        );
    }

    #[test]
    fn model_picker_overflow_hint() {
        let mut app = App::new_test_empty();
        app.resolver = resolver_with_openrouter_models(&[
            "model-a", "model-b", "model-c", "model-d", "model-e", "model-f", "model-g",
            "model-h", "model-i", "model-j",
        ]);
        app.active_overlay = Some(OverlayKind::ModelPicker {
            step: PickerStep::Model,
            selected_provider: Some("openrouter".to_string()),
        });

        // Short viewport (small inner height, forces the overflow hint per
        // UI-SPEC "sized to actual inner height, not a fixed constant") but
        // wide enough (150 cols) that the hint's full text isn't ALSO
        // clipped by pane width — isolates the height-driven truncation this
        // test targets from the width-driven truncation `render_skills_hub`
        // already accepts as expected (ratatui's default no-wrap discipline).
        let text = render_to_text(150, 14, |f| {
            crate::tui_rata::ui::ui(f, &app);
            render(f, &app);
        });

        assert!(
            text.contains("more — narrow your filter"),
            "expected overflow hint in buffer:\n{text}"
        );
    }
}
