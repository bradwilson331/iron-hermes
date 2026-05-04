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
    /// Seeded once from `config.subagent.max_subagents`.
    pub max_subagents: usize,
}

impl Default for StatusLineState {
    fn default() -> Self {
        Self {
            mode: "Chat".to_string(),
            model_short: "?".to_string(),
            provider: "?".to_string(),
            tokens_used: 0,
            tokens_limit: 128_000,
            hint: "ctrl+c cancel · /help commands".to_string(),
            active_subagents: 0,
            max_subagents: 0,
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

/// Build the list of pill texts and optional hint text from state.
///
/// Returns `(pill_texts, hint_text)` where:
/// - `pill_texts` is the ordered vec: mode · model · provider · tokens/limit · [agents N/M]
/// - `hint_text` is `Some(hint)` when the hint field is non-empty, else `None`
fn build_pills(state: &StatusLineState) -> (Vec<String>, Option<String>) {
    let pct = if state.tokens_limit == 0 {
        0
    } else {
        ((state.tokens_used as f64 / state.tokens_limit as f64) * 100.0).round() as u64
    };
    let tokens_cell = format!(
        "{}/{} ({}%)",
        format_token_count(state.tokens_used),
        format_token_count(state.tokens_limit),
        pct
    );

    let mut pills: Vec<String> = vec![
        state.mode.clone(),
        state.model_short.clone(),
        state.provider.clone(),
        tokens_cell,
    ];

    // Plan 21.7-07 (D-04 / Pitfall 8 / R-8 / E-11): insert the `agents: N/M`
    // pill AFTER the tokens pill. ONLY render when active_subagents > 0 —
    // zero must drop the pill entirely so idle users see no visual noise.
    if state.active_subagents > 0 {
        pills.push(format!(
            "agents: {}/{}",
            state.active_subagents, state.max_subagents
        ));
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
    let (pill_texts, hint_text) = build_pills(state);

    let palette = [Color::Cyan, Color::Magenta, Color::Green, Color::Yellow];
    let dot_sep = Span::styled(" · ", Style::default().add_modifier(Modifier::DIM));

    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, text) in pill_texts.into_iter().enumerate() {
        if i > 0 {
            spans.push(dot_sep.clone());
        }
        let color = palette[i % palette.len()];
        spans.push(Span::styled(text, Style::default().fg(color)));
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
            line.spans.len() >= 1,
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
            pill_colors.get(0),
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
}
