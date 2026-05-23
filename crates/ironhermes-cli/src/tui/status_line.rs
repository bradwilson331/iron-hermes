//! Status line state + pure render function.
//!
//! Per D-03: `{mode} · {model_short} · {provider} · {tokens}/{limit} ({pct}%) · {hint}`
//! Per D-04: pills rotate cyan/magenta/green/yellow/dimmed; dots are dimmed; hint is dimmed.
//! Per D-05: state carries live token + limit from the agent's `AggregatedUsage` snapshot
//!           (field type `usize` — see `ironhermes_agent::AgentResult.total_usage.total_tokens`).
//!
//! This module is PURE — it produces a `String` (ANSI-colored) from a state struct.
//! The render task in `render.rs` (Plan 21-02) calls this and writes to stderr via crossterm.

use crate::tui::pills::rotate_pill_colors;
use colored::Colorize;

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

/// Produce the dot-separated, color-rotated status line as a single String
/// ready to be written to stderr by the render task.
pub fn render_status_line(state: &StatusLineState) -> String {
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
        Some(state.hint.as_str())
    };
    let colored_cells = rotate_pill_colors(&pills, hint);

    let dot_sep = format!(" {} ", "·".dimmed());
    colored_cells
        .iter()
        .map(|cs| cs.to_string())
        .collect::<Vec<_>>()
        .join(&dot_sep)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn renders_all_pills_and_hint() {
        let state = StatusLineState {
            mode: "Agent".to_string(),
            model_short: "claude-sonnet-4".to_string(),
            provider: "anthropic".to_string(),
            tokens_used: 107_700,
            tokens_limit: 200_000,
            hint: "ctrl+p commands".to_string(),
            active_subagents: 0,
            max_subagents: 4,
        };
        let out = render_status_line(&state);
        assert!(out.contains("Agent"), "missing Agent: {}", out);
        assert!(out.contains("claude-sonnet-4"));
        assert!(out.contains("anthropic"));
        assert!(out.contains("107.7K"));
        assert!(out.contains("200.0K"));
        assert!(out.contains("54%"));
        assert!(out.contains("ctrl+p commands"));
        assert!(out.contains("·"));
    }

    #[test]
    fn percentage_rounds_to_integer() {
        let state = StatusLineState {
            tokens_used: 107_700,
            tokens_limit: 200_000,
            ..StatusLineState::default()
        };
        let out = render_status_line(&state);
        assert!(out.contains("54%"), "expected 54%, got: {}", out);
    }

    #[test]
    fn handles_zero_limit_without_panic() {
        let state = StatusLineState {
            tokens_limit: 0,
            tokens_used: 5,
            ..StatusLineState::default()
        };
        let out = render_status_line(&state);
        assert!(out.contains("0%"), "expected 0% for zero limit: {}", out);
    }

    #[test]
    fn empty_hint_omits_trailing_hint_pill() {
        let state = StatusLineState {
            mode: "Chat".to_string(),
            model_short: "m".to_string(),
            provider: "p".to_string(),
            tokens_used: 0,
            tokens_limit: 100,
            hint: String::new(),
            active_subagents: 0,
            max_subagents: 0,
        };
        let out = render_status_line(&state);
        // With empty hint, rotate_pill_colors gets None; resulting pill count
        // should be 4 (mode, model, provider, tokens) — so 3 separators.
        let sep_count = out.matches('·').count();
        assert_eq!(sep_count, 3, "expected 3 dots for 4 pills, got: {}", out);
    }

    // ── Plan 21.7-07 (D-04 / Pitfall 8 / R-8 / E-11): agents pill ────────────

    /// E-11 / Pitfall 8: the `agents: N/M` pill MUST NOT render when
    /// `active_subagents == 0`. Idle users should see zero visual noise.
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
        let out = render_status_line(&state);
        assert!(
            !out.contains("agents:"),
            "E-11 / Pitfall 8: pill MUST be hidden when active_subagents == 0. Got: {}",
            out
        );
    }

    /// D-04: when active_subagents > 0, the pill renders as `agents: N/M`.
    #[test]
    fn agents_pill_shows_active_slash_max_when_positive() {
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
        let out = render_status_line(&state);
        assert!(
            out.contains("agents: 2/4"),
            "D-04: pill format must be 'agents: N/M'. Got: {}",
            out
        );
    }

    /// D-04: ordering — tokens pill → agents pill → hint pill.
    #[test]
    fn agents_pill_placement_after_tokens() {
        let state = StatusLineState {
            mode: "chat".into(),
            model_short: "sonnet".into(),
            provider: "anthropic".into(),
            tokens_used: 100,
            tokens_limit: 1000,
            hint: "running".into(),
            active_subagents: 1,
            max_subagents: 4,
        };
        let out = render_status_line(&state);
        // Tokens pill renders as "100/1.0K (10%)"; match the leading 100.
        let tokens_idx = out.find("100").expect("tokens pill present");
        let agents_idx = out.find("agents:").expect("agents pill present");
        let hint_idx = out.find("running").expect("hint pill present");
        assert!(
            tokens_idx < agents_idx && agents_idx < hint_idx,
            "D-04: ordering must be tokens → agents → hint. Got: {}",
            out
        );
    }

    /// Separator count parity: with 1 active subagent + hint, we have 6
    /// pills (mode, model, provider, tokens, agents, hint) = 5 separators.
    #[test]
    fn agents_pill_contributes_one_separator_when_shown() {
        let state_off = StatusLineState {
            mode: "m".into(),
            model_short: "s".into(),
            provider: "p".into(),
            tokens_used: 0,
            tokens_limit: 100,
            hint: "h".into(),
            active_subagents: 0,
            max_subagents: 4,
        };
        let state_on = StatusLineState {
            active_subagents: 1,
            ..state_off.clone()
        };
        let off = render_status_line(&state_off).matches('·').count();
        let on = render_status_line(&state_on).matches('·').count();
        assert_eq!(
            on - off,
            1,
            "adding the agents pill must add exactly one '·' separator (off={}, on={})",
            off,
            on
        );
    }
}
