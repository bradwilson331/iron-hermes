//! Status pill row renderer for the tui_rata REPL (Phase 22.4).
//!
//! Ports the Phase 21 / D-10 pill derivation from classic
//! `tui/status_line.rs` + `tui/pills.rs`, swapping the `colored` crate
//! output (ANSI strings) for ratatui `Span::styled` + `Line::from(spans)`
//! so the status pill row composes inside a ratatui frame.
//!
//! Business logic (pill content, `agents N/M` conditional, token-count
//! formatting) is lifted VERBATIM; only the output type changes.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

// — StatusLineState lifted verbatim from classic tui/status_line.rs ————————

/// Snapshot of the values shown in the status line. Updated each tick by the
/// render task (Plan 21-02) from the live `AggregatedUsage` counter.
///
/// Revision R1 BLOCKER 2: `tokens_used` / `tokens_limit` are `usize`, matching
/// `ironhermes_agent::AggregatedUsage { total_tokens: usize }` to avoid cast noise.
///
/// Plan 21.7-07 (D-04): `active_subagents` / `max_subagents` drive the
/// `agents: N/M` pill. The pill is HIDDEN when `active_subagents == 0`
/// (Pitfall 8 / R-8 — no visual noise when no subagents are running).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusLineState {
    pub mode: String,
    pub model_short: String,
    pub provider: String,
    pub tokens_used: usize,
    pub tokens_limit: usize,
    pub hint: String,
    /// Plan 21.7-07 (D-04): live count of registered subagents.
    /// Populated off the render path via a spawned task + watch::send_modify
    /// (Pitfall 8: NEVER awaits RwLock on the render path).
    pub active_subagents: usize,
    /// Plan 21.7-07 (D-04): denominator of the `agents: N/M` pill.
    /// Seeded once from `config.delegation.max_concurrent_children` (renamed
    /// in Phase 32.2 D-07; the local struct field name is kept for stability).
    pub max_subagents: usize,
    /// Phase 36.2 Plan 10 (D-USAGE-03): current session cost in micro-USD.
    /// 1 USD = 1_000_000 micros. The render layer is the ONLY place this
    /// becomes f64 (`as f64 / 1_000_000.0`) — every upstream arithmetic op
    /// stays i64 (Pitfall 5: float drift). Populated by the post-turn
    /// ticker that reads the latest `sessions` row's `cost_usd_micros`
    /// column (Plan 02 schema).
    pub cost_usd_micros: i64,
    /// Phase 36.2 Plan 10 (D-USAGE-03): total tokens in the current session
    /// (`in + out + cache_read + cache_create`). Populated alongside
    /// `cost_usd_micros` from the same sessions-row read.
    pub session_total_tokens: usize,
    /// Phase 36.17.3 (D-09): current queue depth read live each render.
    /// Populated in `ui.rs` by `app.queue.len(&app.queue_key)` per frame
    /// so the pill never goes stale by one tick (RESEARCH Pitfall 5).
    pub queue_depth: usize,
    /// Phase 36.17.3 (D-09): true when queue drain is paused. Populated in
    /// `ui.rs` by `app.queue_paused.load(Relaxed)` per frame.
    pub queue_paused: bool,

    /// Phase 36.17.8 (D-08): voice recording phase for the status pill.
    /// `None` = voice mode off (pill hidden); `Some(phase)` = show pill.
    /// Populated per frame in `ui.rs` from `app.voice`.
    pub voice_phase: Option<VoicePillPhase>,
}

/// Recording phase for the voice status pill (mirrors RecordPhase in voice_state.rs
/// but lives here so status_line.rs has no dep on the cli crate's voice_state module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoicePillPhase {
    /// Voice mode on, not yet recording — show `CTRL+B to record` (DarkGray).
    Ready,
    /// cpal stream active — show `● LISTENING` (Red).
    Listening,
    /// STT request in flight — show `◐ TRANSCRIBING` (Yellow).
    Transcribing,
}

impl Default for StatusLineState {
    fn default() -> Self {
        Self {
            mode: "Chat".to_string(),
            model_short: "?".to_string(),
            provider: "?".to_string(),
            tokens_used: 0,
            tokens_limit: 128_000,
            // Phase 36.6.2 Plan 04 (D-09): surface the Ctrl+T/Ctrl+K/? keys
            // alongside the existing hints — pure string edit, no new field.
            // Phase 36.6.3 Plan 04 (D-08): trailing `/help commands` segment
            // replaced with the palette (`/ commands`) + `/model` picker
            // (`/model switch`) mentions, appended LAST so they truncate
            // first on a narrow terminal (UI-SPEC E4 overflow backstop).
            // DUPLICATED at event_loop.rs's runtime-constructed
            // `StatusLineState` default — both sites MUST change together
            // (RESEARCH Pitfall 3).
            hint: "ctrl+c cancel · Ctrl+T thinking · Ctrl+K skills · ? help · / commands · /model switch"
                .to_string(),
            active_subagents: 0,
            max_subagents: 0,
            // Phase 36.2 Plan 10 (D-USAGE-03): zero-state defaults so the
            // ticker can wake up before any usage_events row exists.
            cost_usd_micros: 0,
            session_total_tokens: 0,
            // Phase 36.17.3 (D-09): hide-when-zero discipline → both default
            // to falsy so the pill is absent at session start.
            queue_depth: 0,
            queue_paused: false,
            // Phase 36.17.8 (D-08): voice pill hidden at session start.
            voice_phase: None,
        }
    }
}

// — pure helper lifted verbatim from classic tui/status_line.rs ——————————

/// Format a token count as "107.7K" / "1.2M" / "500".
///
/// Revision R1 W4: The mega threshold is `>= 999_500` (not `>= 1_000_000`) so
/// `999_999` rounds to `"1.0M"` instead of printing the jarring `"1000.0K"`.
pub fn format_token_count(n: usize) -> String {
    if n >= 999_500 {
        let m = (n as f64) / 1_000_000.0;
        format!("{:.1}M", m)
    } else if n >= 1_000 {
        let k = (n as f64) / 1_000.0;
        format!("{:.1}K", k)
    } else {
        format!("{}", n)
    }
}

// — pill derivation lifted verbatim from classic tui/status_line.rs:86–111 ——

/// A pill entry: text plus an optional colour override.
///
/// `None` → use the standard palette rotation; `Some(c)` → override with that colour.
struct PillEntry {
    text: String,
    color_override: Option<Color>,
}

impl PillEntry {
    fn palette(text: String) -> Self {
        Self {
            text,
            color_override: None,
        }
    }
    fn colored(text: String, c: Color) -> Self {
        Self {
            text,
            color_override: Some(c),
        }
    }
}

/// Build the list of pill entries and optional hint text from state.
///
/// Returns `(pill_entries, hint_text)` where each entry carries an optional
/// color override so the voice pill can use its spec'd Red / Yellow / DarkGray
/// without disrupting the palette rotation for other pills.
fn build_pills(state: &StatusLineState) -> (Vec<PillEntry>, Option<String>) {
    // T-46.9-09: clamp the numerator into [0, tokens_limit] before the ratio
    // is computed — a compression race (a stale usize update racing the next
    // tokens_limit write) must never yield a >100% pill or, via the
    // tokens_limit==0 short-circuit below, a NaN/negative percentage.
    let tokens_used_clamped = state.tokens_used.min(state.tokens_limit);
    let pct = if state.tokens_limit == 0 {
        0
    } else {
        ((tokens_used_clamped as f64 / state.tokens_limit as f64) * 100.0).round() as u64
    };
    let tokens_cell = format!(
        "{}/{} ({}%)",
        format_token_count(tokens_used_clamped),
        format_token_count(state.tokens_limit),
        pct
    );

    let mut pills: Vec<PillEntry> = vec![
        PillEntry::palette(state.mode.clone()),
        PillEntry::palette(state.model_short.clone()),
        PillEntry::palette(state.provider.clone()),
        PillEntry::palette(tokens_cell),
    ];

    // Plan 21.7-07 (D-04 / Pitfall 8 / R-8 / E-11): insert the `agents: N/M`
    // pill AFTER the tokens pill. ONLY render when active_subagents > 0 —
    // zero must drop the pill entirely so idle users see no visual noise.
    if state.active_subagents > 0 {
        pills.push(PillEntry::palette(format!(
            "agents: {}/{}",
            state.active_subagents, state.max_subagents
        )));
    }

    // Phase 36.2 Plan 10 (D-USAGE-03): cost + token pills.
    //
    // Hidden when zero — same idle-noise discipline as the agents pill. The
    // ONLY f64 conversion sits here (display layer) per Pitfall 5; every
    // upstream arithmetic stays i64. Per the plan's <action> Step 2, these
    // sit AFTER the existing pills and BEFORE the hint span.
    if state.cost_usd_micros > 0 {
        let usd = state.cost_usd_micros as f64 / 1_000_000.0;
        pills.push(PillEntry::palette(format!("${:.3}", usd)));
    }
    if state.session_total_tokens > 0 {
        pills.push(PillEntry::palette(format!(
            "{:.1}K tok",
            state.session_total_tokens as f64 / 1_000.0
        )));
    }

    // Phase 36.17.3 (D-09): queue-depth pill — hide-when-zero matching the
    // `agents` and cost/token pill discipline. Renders as `Queue: {N}` or
    // `Queue: {N} (paused)`; absent at depth==0 so idle sessions stay quiet.
    if state.queue_depth > 0 {
        if state.queue_paused {
            pills.push(PillEntry::palette(format!(
                "Queue: {} (paused)",
                state.queue_depth
            )));
        } else {
            pills.push(PillEntry::palette(format!("Queue: {}", state.queue_depth)));
        }
    }

    // Phase 36.17.8 (D-08): voice pill — hide-when-idle matching QUEUE pill discipline.
    // Visible only when voice_phase is Some; each phase uses a spec'd colour override
    // so it stands out from the palette rotation of info pills.
    if let Some(phase) = state.voice_phase {
        let (label, color) = match phase {
            VoicePillPhase::Ready => ("CTRL+B to record".to_string(), Color::DarkGray),
            VoicePillPhase::Listening => ("\u{25cf} LISTENING".to_string(), Color::Red), // ●
            VoicePillPhase::Transcribing => ("\u{25d0} TRANSCRIBING".to_string(), Color::Yellow), // ◐
        };
        pills.push(PillEntry::colored(label, color));
    }

    let hint = if state.hint.is_empty() {
        None
    } else {
        Some(state.hint.clone())
    };

    (pills, hint)
}

// — ratatui rendering (NEW for tui_rata) ————————————————————————————————

/// Renders the status pill row as a styled `Line<'static>` for use inside
/// a `ratatui::Paragraph` in `tui_rata/ui.rs`.
///
/// Pill colour rotation: Cyan → Magenta → Green → Yellow (locked per D-10
/// and Phase 21 §specifics). Hint is appended with `Modifier::DIM`.
///
/// Consumer site (plan 22.4-07 `tui_rata/ui.rs`):
/// `frame.render_widget(Paragraph::new(render_status_line_ratatui(&app.status)), chunks[2])`.
pub fn render_status_line_ratatui(state: &StatusLineState) -> Line<'static> {
    let (pill_entries, hint_text) = build_pills(state);

    let palette = [Color::Cyan, Color::Magenta, Color::Green, Color::Yellow];
    let dot_sep = Span::styled(" · ", Style::default().add_modifier(Modifier::DIM));

    let mut spans: Vec<Span<'static>> = Vec::new();
    // Track palette index separately from pill index so colour-overridden pills
    // (voice pill) do NOT advance the rotation — the next palette pill picks up
    // exactly where the last palette pill left off.
    let mut palette_idx: usize = 0;
    for (i, entry) in pill_entries.into_iter().enumerate() {
        if i > 0 {
            spans.push(dot_sep.clone());
        }
        let color = if let Some(c) = entry.color_override {
            c
        } else {
            let c = palette[palette_idx % palette.len()];
            palette_idx += 1;
            c
        };
        spans.push(Span::styled(entry.text, Style::default().fg(color)));
    }
    if let Some(hint) = hint_text {
        spans.push(dot_sep.clone());
        spans.push(Span::styled(
            hint,
            Style::default().add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    // — format_token_count tests lifted verbatim from classic tui/status_line.rs ——

    #[test]
    fn format_token_count_under_1000() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(500), "500");
        assert_eq!(format_token_count(999), "999");
    }

    #[test]
    fn format_token_count_kilo() {
        assert_eq!(format_token_count(1_000), "1.0K");
        assert_eq!(format_token_count(1_500), "1.5K");
        assert_eq!(format_token_count(107_700), "107.7K");
    }

    /// Revision R1 W4: 999_500..=999_999 must round to "1.0M", not "1000.0K".
    #[test]
    fn format_token_count_boundary_999_999_is_one_mega() {
        assert_eq!(format_token_count(999_499), "999.5K");
        assert_eq!(format_token_count(999_500), "1.0M");
        assert_eq!(format_token_count(999_999), "1.0M");
    }

    #[test]
    fn format_token_count_mega() {
        assert_eq!(format_token_count(1_000_000), "1.0M");
        assert_eq!(format_token_count(1_200_000), "1.2M");
    }

    // — render_status_line_ratatui tests (new for tui_rata) ————————————————

    #[test]
    fn render_empty_state_has_no_hint_span() {
        let state = StatusLineState {
            hint: String::new(),
            ..StatusLineState::default()
        };
        let line = render_status_line_ratatui(&state);
        // With empty hint, no hint span; at minimum 4 pill spans + 3 dot seps = 7 spans.
        assert!(
            !line.spans.is_empty(),
            "empty hint state still produces at least one pill span"
        );
        // No DIM spans from hint (only dot separators have DIM in this case)
        // pill spans are at even indices; verify no extra DIM pill at the end
        // by checking span count = 4 pills + 3 separators = 7
        assert_eq!(
            line.spans.len(),
            7,
            "4 pills + 3 dot separators = 7 spans; got {}",
            line.spans.len()
        );
    }

    #[test]
    fn render_with_hint_appends_dim_span() {
        let state = StatusLineState {
            mode: "Chat".to_string(),
            model_short: "m".to_string(),
            provider: "p".to_string(),
            tokens_used: 0,
            tokens_limit: 100,
            hint: "ctrl+c cancel".to_string(),
            active_subagents: 0,
            max_subagents: 0,
            ..StatusLineState::default()
        };
        let line = render_status_line_ratatui(&state);
        // 4 pills + 3 dot seps + 1 dot sep before hint + 1 hint span = 9
        assert_eq!(
            line.spans.len(),
            9,
            "4 pills + 4 dot separators + 1 hint = 9 spans; got {}",
            line.spans.len()
        );
        // Last span should be the hint with DIM modifier
        let last = line.spans.last().unwrap();
        assert!(
            last.style.add_modifier.contains(Modifier::DIM),
            "hint span must have DIM modifier"
        );
    }

    /// Phase 36.6.2 Plan 04 (D-09): the default hint surfaces the new
    /// Ctrl+T/Ctrl+K/`?` keys alongside the pre-existing Ctrl+C/`/help` copy.
    #[test]
    fn status_hint_lists_new_keys() {
        let state = StatusLineState::default();
        assert!(
            state.hint.contains("Ctrl+T thinking"),
            "hint must mention Ctrl+T thinking: {:?}",
            state.hint
        );
        assert!(
            state.hint.contains("Ctrl+K skills"),
            "hint must mention Ctrl+K skills: {:?}",
            state.hint
        );
        assert!(
            state.hint.contains("? help"),
            "hint must mention ? help: {:?}",
            state.hint
        );

        // The hint must actually reach the rendered line (not just the
        // struct default) via the existing DIM hint span path.
        let line = render_status_line_ratatui(&state);
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(rendered.contains("Ctrl+T thinking"), "rendered:\n{rendered}");
        assert!(rendered.contains("Ctrl+K skills"), "rendered:\n{rendered}");
        assert!(rendered.contains("? help"), "rendered:\n{rendered}");
    }

    #[test]
    fn render_four_pills_rotates_palette_cyan_magenta_green_yellow() {
        let state = StatusLineState {
            mode: "Chat".to_string(),
            model_short: "sonnet".to_string(),
            provider: "anthropic".to_string(),
            tokens_used: 1000,
            tokens_limit: 10_000,
            hint: String::new(),
            active_subagents: 0,
            max_subagents: 0,
            ..StatusLineState::default()
        };
        let line = render_status_line_ratatui(&state);
        // pill spans alternate with dot_sep spans; pill spans at indices 0, 2, 4, 6
        let pill_colors: Vec<Option<Color>> = line
            .spans
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 2 == 0)
            .map(|(_, s)| s.style.fg)
            .collect();
        assert_eq!(
            pill_colors.first(),
            Some(&Some(Color::Cyan)),
            "pill[0] must be Cyan"
        );
        assert_eq!(
            pill_colors.get(1),
            Some(&Some(Color::Magenta)),
            "pill[1] must be Magenta"
        );
        assert_eq!(
            pill_colors.get(2),
            Some(&Some(Color::Green)),
            "pill[2] must be Green"
        );
        assert_eq!(
            pill_colors.get(3),
            Some(&Some(Color::Yellow)),
            "pill[3] must be Yellow"
        );
    }

    #[test]
    fn agents_pill_hides_at_zero_active() {
        let state = StatusLineState {
            mode: "chat".into(),
            model_short: "sonnet".into(),
            provider: "anthropic".into(),
            tokens_used: 100,
            tokens_limit: 1000,
            hint: String::new(),
            active_subagents: 0,
            max_subagents: 4,
            ..StatusLineState::default()
        };
        let line = render_status_line_ratatui(&state);
        // No span should contain "agents:"
        let has_agents = line.spans.iter().any(|s| s.content.contains("agents:"));
        assert!(
            !has_agents,
            "E-11 / Pitfall 8: agents pill MUST be hidden when active_subagents == 0"
        );
    }

    #[test]
    fn agents_pill_shows_when_active() {
        let state = StatusLineState {
            mode: "chat".into(),
            model_short: "sonnet".into(),
            provider: "anthropic".into(),
            tokens_used: 100,
            tokens_limit: 1000,
            hint: String::new(),
            active_subagents: 2,
            max_subagents: 4,
            ..StatusLineState::default()
        };
        let line = render_status_line_ratatui(&state);
        let has_agents = line.spans.iter().any(|s| s.content.contains("agents: 2/4"));
        assert!(
            has_agents,
            "D-04: agents pill must render as 'agents: N/M' when active_subagents > 0"
        );
    }

    #[test]
    fn dot_separators_are_dim() {
        let state = StatusLineState {
            hint: String::new(),
            ..StatusLineState::default()
        };
        let line = render_status_line_ratatui(&state);
        // Separator spans are at odd indices (1, 3, 5); they must all have DIM
        for (i, span) in line.spans.iter().enumerate() {
            if i % 2 == 1 {
                assert!(
                    span.style.add_modifier.contains(Modifier::DIM),
                    "dot separator at index {} must be DIM",
                    i
                );
                assert_eq!(span.content, " · ", "separator content must be ' · '");
            }
        }
    }

    // — tokens_limit real-context-length tests (Phase 46.9 Plan 03, D-08/D-09) ——

    /// D-09: the TUI reads the SAME `ModelRegistry` (aka
    /// `ModelMetadataRegistry` in RESEARCH shorthand) context_length the web
    /// surface's `ResolvedEndpoint::context_length()` consults via the
    /// resolver. `"claude-sonnet-4"` resolves to `200_000` in the static
    /// table — proving a `StatusLineState` seeded from a known model id is
    /// NOT the old hardcoded `128_000` default (D-08 bug).
    #[test]
    fn tokens_limit_seeded_from_known_model_is_not_hardcoded_default() {
        let registry = ironhermes_core::model_metadata::ModelRegistry::new();
        let tokens_limit = registry.context_length_or_default("claude-sonnet-4");
        assert_eq!(tokens_limit, 200_000, "claude-sonnet-4 has a 200K window");
        assert_ne!(
            tokens_limit, 128_000,
            "must not equal the old hardcoded default"
        );

        let state = StatusLineState {
            tokens_limit,
            ..StatusLineState::default()
        };
        assert_eq!(state.tokens_limit, 200_000);
    }

    // — provider/model adjacency test (Phase 46.9 Plan 10, GAP-3/D-07/D-09) ——

    /// GAP-3 (D-07/D-09) TUI parity: the status line renders the active
    /// provider directly adjacent to the model pill — mirroring the web
    /// `.sys-meta` "provider · model" adjacency added in this same plan.
    /// `build_pills` already orders pills as `[mode, model_short, provider,
    /// tokens]`, so the model and provider pills sit next to each other
    /// (exactly one dot-separator span apart) with no reordering needed;
    /// this test locks that adjacency in as a regression guard and proves a
    /// seeded provider is not the `Default::default()` `"?"` placeholder.
    #[test]
    fn provider_renders_adjacent_to_model_pill() {
        let state = StatusLineState {
            mode: "Chat".to_string(),
            model_short: "claude-sonnet-4".to_string(),
            provider: "anthropic".to_string(),
            tokens_used: 0,
            tokens_limit: 200_000,
            hint: String::new(),
            ..StatusLineState::default()
        };
        assert_ne!(
            state.provider, "?",
            "seeded provider must not be the Default::default() placeholder"
        );

        let line = render_status_line_ratatui(&state);
        let model_idx = line
            .spans
            .iter()
            .position(|s| s.content == "claude-sonnet-4")
            .expect("model pill must render");
        let provider_idx = line
            .spans
            .iter()
            .position(|s| s.content == "anthropic")
            .expect("provider pill must render");
        assert_eq!(
            provider_idx,
            model_idx + 2,
            "provider pill must sit immediately after the model pill (one dot \
             separator span between them); got model@{} provider@{}",
            model_idx,
            provider_idx
        );
    }

    /// T-46.9-09: a compression-race `tokens_used` that momentarily exceeds
    /// `tokens_limit` must clamp to 100% in the rendered pill, never wrap to
    /// a >100% or NaN percentage.
    #[test]
    fn tokens_used_clamped_never_exceeds_tokens_limit_in_pill() {
        let state = StatusLineState {
            tokens_used: 999_999,
            tokens_limit: 1000,
            ..StatusLineState::default()
        };
        let line = render_status_line_ratatui(&state);
        let tokens_span = line
            .spans
            .iter()
            .find(|s| s.content.contains('%'))
            .expect("tokens pill must be present");
        assert!(
            tokens_span.content.contains("(100%)"),
            "compression-race numerator must clamp to 100%, got: {}",
            tokens_span.content
        );
    }

    // — Phase 36.6.3 Plan 04 (D-08): palette + /model picker discoverability ——

    /// UI-SPEC Copywriting Contract (D-08): the default hint appends the two
    /// NEW discoverability mentions LAST — `/ commands` (palette) and
    /// `/model switch` (picker) — replacing the old trailing `/help commands`
    /// segment so they truncate first on a narrow terminal (E4 overflow
    /// backstop, see `status_line_hint_clips_gracefully_on_narrow_width`
    /// below). Asserts both the raw struct field AND the rendered DIM hint
    /// span carry the new copy.
    #[test]
    fn status_line_hint_mentions_palette() {
        let state = StatusLineState::default();
        assert!(
            state.hint.contains("/ commands"),
            "hint must mention the command palette: {:?}",
            state.hint
        );
        assert!(
            state.hint.contains("/model switch"),
            "hint must mention the /model picker: {:?}",
            state.hint
        );

        let line = render_status_line_ratatui(&state);
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(rendered.contains("/ commands"), "rendered:\n{rendered}");
        assert!(rendered.contains("/model switch"), "rendered:\n{rendered}");
    }

    /// UI-SPEC E4 overflow backstop: renders the SAME `Paragraph::new(line)`
    /// construction `ui.rs::render_status_line` uses (no `.wrap()` — default
    /// no-wrap cell truncation) at a narrow ~40-col width and asserts the
    /// hint clips gracefully at the right edge — no panic, no wrap onto a
    /// second terminal row.
    #[test]
    fn status_line_hint_clips_gracefully_on_narrow_width() {
        use ratatui::{Terminal, backend::TestBackend, widgets::Paragraph};

        let state = StatusLineState::default();
        let line = render_status_line_ratatui(&state);

        let width = 40u16;
        let height = 3u16;
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("TestBackend terminal");
        terminal
            .draw(|f| {
                let area = f.area();
                f.render_widget(Paragraph::new(line.clone()), area);
            })
            .expect("render must not panic at a narrow width");

        let buf = terminal.backend().buffer();
        let row = |y: u16| -> String {
            (0..width)
                .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                .collect()
        };
        let row0 = row(0);
        let row1 = row(1);

        assert_eq!(
            row0.chars().count(),
            width as usize,
            "row 0 must fill the full narrow width (graceful clip, not a shortened row)"
        );
        assert!(
            row1.trim().is_empty(),
            "the hint must clip at the right edge, not wrap onto a second terminal row: {row1:?}"
        );
    }
}
