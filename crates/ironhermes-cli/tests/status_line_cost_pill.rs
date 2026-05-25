//! Phase 36.2 Plan 10 Task 2 — TUI status-bar cost pill behavior tests.
//!
//! Verifies the `StatusLineState` extension (`cost_usd_micros: i64`,
//! `session_total_tokens: usize`) and the conditional pill insertion in
//! `build_pills` / `render_status_line_ratatui` per CONTEXT.md D-USAGE-03
//! sketch:  `│ ironhermes │ Anthropic Opus 4.7 │ $0.234 │ 12.4K tok │ … │`
//!
//! Behaviors:
//!   t1 — StatusLineState exposes `cost_usd_micros: i64`.
//!   t2 — StatusLineState exposes `session_total_tokens: usize`.
//!   t3 — Default constructor zeros both new fields.
//!   t4 — Zero-cost zero-tokens => no cost or token pill rendered.
//!   t5 — cost_usd_micros = 234_000 => "$0.234" pill content present.
//!   t6 — session_total_tokens = 12_400 => "12.4K tok" pill content present.
//!   t7 — Large numbers (1500 USD, 1.5M tokens) format with the documented
//!        K-only notation (no rolling to M); cost prints as `$1500.000`.

use ironhermes_cli::tui_rata::status_line::{StatusLineState, render_status_line_ratatui};

fn pill_content_strings(state: &StatusLineState) -> Vec<String> {
    let line = render_status_line_ratatui(state);
    line.spans
        .iter()
        .map(|s| s.content.to_string())
        .collect()
}

fn rendered_text(state: &StatusLineState) -> String {
    pill_content_strings(state).concat()
}

#[test]
fn t1_status_line_state_has_cost_usd_micros_field() {
    // Compile-time check via struct-literal pattern; if the field is missing
    // this assertion fails to compile, which is the failing-test surface.
    let s = StatusLineState {
        cost_usd_micros: 0_i64,
        ..StatusLineState::default()
    };
    assert_eq!(s.cost_usd_micros, 0_i64);
}

#[test]
fn t2_status_line_state_has_session_total_tokens_field() {
    let s = StatusLineState {
        session_total_tokens: 0_usize,
        ..StatusLineState::default()
    };
    assert_eq!(s.session_total_tokens, 0_usize);
}

#[test]
fn t3_default_zeros_new_fields() {
    let s = StatusLineState::default();
    assert_eq!(s.cost_usd_micros, 0);
    assert_eq!(s.session_total_tokens, 0);
}

#[test]
fn t4_zero_cost_and_zero_tokens_render_no_pill() {
    let s = StatusLineState {
        cost_usd_micros: 0,
        session_total_tokens: 0,
        ..StatusLineState::default()
    };
    let text = rendered_text(&s);
    assert!(
        !text.contains('$'),
        "no '$' character should appear when cost_usd_micros == 0; got: {text}"
    );
    assert!(
        !text.contains("K tok"),
        "no 'K tok' should appear when session_total_tokens == 0; got: {text}"
    );
}

#[test]
fn t5_cost_pill_renders_three_decimals() {
    let s = StatusLineState {
        cost_usd_micros: 234_000_i64,
        ..StatusLineState::default()
    };
    let text = rendered_text(&s);
    assert!(
        text.contains("$0.234"),
        "expected '$0.234' pill content in rendered text: {text}"
    );
}

#[test]
fn t6_token_pill_renders_one_decimal_K() {
    let s = StatusLineState {
        session_total_tokens: 12_400_usize,
        ..StatusLineState::default()
    };
    let text = rendered_text(&s);
    assert!(
        text.contains("12.4K tok"),
        "expected '12.4K tok' pill content in rendered text: {text}"
    );
}

#[test]
fn t7_large_values_format_in_dollars_and_K_tokens() {
    // $1500 = 1_500_000_000 micros; 1_500_000 tokens = "1500.0K tok"
    let s = StatusLineState {
        cost_usd_micros: 1_500_000_000_i64,
        session_total_tokens: 1_500_000_usize,
        ..StatusLineState::default()
    };
    let text = rendered_text(&s);
    assert!(
        text.contains("$1500.000"),
        "expected '$1500.000' for $1500 cost: {text}"
    );
    assert!(
        text.contains("1500.0K tok"),
        "expected '1500.0K tok' (K notation, not rolled to M): {text}"
    );
}
