//! Phase 58.3.1: pure arithmetic for "does this request fit the model window".
//!
//! One estimator only (`context_compressor::estimate_*`, D-16). Everything here
//! is side-effect free so the loop guard in `agent_loop.rs` stays a thin caller.

use ironhermes_core::config::CompactProfileMode;
use ironhermes_core::{ChatMessage, ToolSchema};

use crate::context_compressor::{estimate_messages_tokens, estimate_tokens};
use crate::error_classifier::ContextOverflow;

/// Windows at or below this size are "small" (compact-profile territory, plan 07).
pub const SMALL_WINDOW_TOKENS: usize = 32_768;

/// D-02: `agent.compact_profile = auto` turns the compact profile on when the full
/// prompt floor (system prompt + all tool schemas) exceeds this share of the window.
pub const COMPACT_TRIGGER_RATIO: f32 = 0.5;

/// D-02: should this request use the compact profile? `full_floor_tokens` is the
/// full (non-compact) system prompt plus all tool schemas, same estimator (D-16).
pub fn compact_profile_wanted(
    full_floor_tokens: usize,
    window: usize,
    mode: CompactProfileMode,
) -> bool {
    match mode {
        CompactProfileMode::On => true,
        CompactProfileMode::Off => false,
        CompactProfileMode::Auto => {
            full_floor_tokens as f32 > window as f32 * COMPACT_TRIGGER_RATIO
        }
    }
}

/// D-03 compact tool set (`patch` is the file-edit tool), then the D-24 pair.
pub fn compact_tool_allowlist() -> Vec<String> {
    // kanban_complete / kanban_block self-gate via is_available() to kanban worker
    // sessions and keep a worker able to finish its task (D-24 amends D-03).
    [
        "terminal",
        "read_file",
        "write_file",
        "patch",
        "memory",
        "web_search",
        "web_read",
        "kanban_complete",
        "kanban_block",
    ]
    .map(String::from)
    .to_vec()
}

/// D-14: compaction passes the guard may run before one model request.
pub const MAX_COMPACTION_PASSES_PER_REQUEST: usize = 3;

/// Tokens reserved for the model's reply: 1,024 at 8,192, scaling to 4,096 by 32,768.
pub fn reply_reserve(window: usize) -> usize {
    (window / 8).clamp(1_024, 4_096)
}

/// Safety margin on top of the estimate (12.5% of the window, 256..=8,192) so a
/// server whose tokenizer counts ~10% above cl100k still fits.
pub fn safety_margin(window: usize) -> usize {
    (window / 8).clamp(256, 8_192)
}

/// D-15: compression trigger for a window. A per-model override wins (clamped to
/// 0.05..=0.95); otherwise windows >= 32,768 keep the configured value (agent 0.5
/// / gateway 0.85 unchanged) and smaller ones are capped by a curve that is 0.35
/// at <= 8,192 and log-linear up to 0.50 at 32,768. `min` never raises an
/// operator's lower threshold.
pub fn effective_compression_threshold(
    configured: f32,
    model_override: Option<f32>,
    window: usize,
) -> f32 {
    if let Some(o) = model_override {
        return o.clamp(0.05, 0.95);
    }
    if window >= SMALL_WINDOW_TOKENS {
        return configured;
    }
    let curve = if window <= 8_192 {
        0.35
    } else {
        // log2(8192)=13 .. log2(32768)=15
        0.35 + 0.15 * ((window as f32).log2() - 13.0) / 2.0
    };
    configured.min(curve)
}

/// Protected tail clamped to at most half the trigger level (and a quarter of the
/// window) so a compacted conversation lands below the trigger (Pitfall 3).
pub fn effective_protect_last(configured: usize, window: usize, threshold: f32) -> usize {
    configured
        .min(window / 4)
        .min((window as f32 * threshold / 2.0) as usize)
}

/// D-17: summarizer output budget in tokens: 20% of the content, bounded above by
/// min(5% of the window, 12,000) and below by min(2,000, that bound).
pub fn summary_budget(content_tokens: usize, window: usize) -> usize {
    let upper = (window * 5 / 100).min(12_000);
    // min-then-max, not `clamp(2_000, upper)`: clamp panics when upper < 2,000.
    (content_tokens * 20 / 100).min(upper).max(2_000.min(upper))
}

/// Character cap for the stored summary (4 chars per token).
pub fn summary_char_cap(content_tokens: usize, window: usize) -> usize {
    summary_budget(content_tokens, window) * 4
}

/// Per-session cap on summarizing passes: small windows compact ~4x as often.
pub fn max_compression_passes(window: usize) -> usize {
    if window < SMALL_WINDOW_TOKENS { 40 } else { 10 }
}

/// Estimated tokens of the tool schemas as they travel on the wire.
pub fn tool_schema_tokens(tools: &[ToolSchema]) -> usize {
    tools
        .iter()
        .map(|t| estimate_tokens(&serde_json::to_string(t).unwrap_or_default()))
        .sum()
}

/// Estimated prompt size: messages plus tool schemas.
pub fn prompt_tokens(messages: &[ChatMessage], tool_tokens: usize) -> usize {
    estimate_messages_tokens(messages) + tool_tokens
}

/// True when the prompt plus the reply reserve and margin fits the window.
/// `estimate_scale` in (0, 1] is how much of the server's real count our
/// estimator produces; 1.0 until a server count proves the estimator low.
pub fn fits(prompt_tokens: usize, window: usize, estimate_scale: f32) -> bool {
    let scale = estimate_scale.clamp(0.01, 1.0);
    let scaled = (prompt_tokens as f32 / scale).ceil() as usize;
    scaled
        .saturating_add(reply_reserve(window))
        .saturating_add(safety_margin(window))
        <= window
}

/// The smallest request the agent could send: the fixed prompt plus reserve and margin.
pub fn floor_tokens(system_and_tools_prompt: usize, window: usize) -> usize {
    system_and_tools_prompt + reply_reserve(window) + safety_margin(window)
}

/// `protect_last_tokens` for compaction pass `pass` (1..=3): the conversation
/// budget (window minus tools, reserve and margin) divided by `2^pass`.
pub fn guard_protect_last(window: usize, tool_tokens: usize, pass: usize) -> usize {
    let budget = window
        .saturating_sub(tool_tokens)
        .saturating_sub(reply_reserve(window))
        .saturating_sub(safety_margin(window));
    budget >> pass.clamp(1, MAX_COMPACTION_PASSES_PER_REQUEST)
}

/// D-08: the window to size the single retry for. A parsed window is used but
/// never raises the current one (a lying server cannot grow it); with no number
/// in the error text the retry is sized for half the assumed window.
pub fn retry_window(current_window: usize, overflow: ContextOverflow) -> usize {
    match overflow.window {
        Some(w) => w.min(current_window),
        None => current_window / 2,
    }
}

/// D-21: the server's own count corrects the estimator for the retry. Returns
/// `our / requested` when the server counted more than we did, clamped to
/// `[0.25, 1.0]`; otherwise 1.0.
pub fn estimate_scale_from(our_prompt_tokens: usize, overflow: ContextOverflow) -> f32 {
    match overflow.requested {
        Some(r) if r > our_prompt_tokens && r > 0 => {
            (our_prompt_tokens as f32 / r as f32).clamp(0.25, 1.0)
        }
        _ => 1.0,
    }
}

/// The fix-it sentence shared by every window refusal. A window the model server itself
/// reported cannot be raised from ironhermes, so that wording points at the server; any
/// other source (config, cache, pin, default) may be wrong, so it offers the override,
/// capped at what the server really has loaded (a higher value only hides the learned one).
pub(crate) fn fix_it_sentence(from_server: bool, last_resort: &str) -> String {
    if from_server {
        format!(
            "This window was reported by the model server itself. To fix it, raise the context length in the model server if you run it (LM Studio, Ollama or llama.cpp) and reload the model, pick a model with a larger context window, or {last_resort}, then try again."
        )
    } else {
        format!(
            "To fix it, set providers.<provider>.models.<model>.context_length (or model.context_length) to the context length the model server actually has loaded, never higher, pick a model with a larger context window, or {last_resort}, then try again."
        )
    }
}

/// User-facing refusals. Numbers and fixed guidance only: never provider bodies
/// or prompt text. `from_server` is true when the model server itself reported the
/// window (probed, or learned from a rejection); it selects the fix-it wording.
#[derive(Debug, thiserror::Error)]
pub enum ContextWindowError {
    #[error(
        "This model's context window is {window} tokens, but the smallest prompt the agent can send (system prompt, tool definitions and a {reserve}-token reply reserve) needs about {floor} tokens, so the turn was not sent. {}",
        fix_it_sentence(*.from_server, "shorten SOUL.md")
    )]
    PromptFloorTooLarge {
        window: usize,
        floor: usize,
        reserve: usize,
        from_server: bool,
    },
    #[error(
        "This conversation needs about {request} tokens but the model's context window is {window} tokens, even after compressing it, so the turn was not sent. {}",
        fix_it_sentence(*.from_server, "start a fresh session")
    )]
    RequestTooLarge {
        window: usize,
        request: usize,
        from_server: bool,
    },
    #[error(
        "The provider rejected the request as too long for its {window}-token context window, even after the agent learned that limit, compressed the conversation and retried once. {}",
        fix_it_sentence(*.from_server, "start a fresh session")
    )]
    RetryRejected { window: usize, from_server: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_request_tokens_counts_messages_tools_and_reserve() {
        let msgs = vec![ChatMessage::system("hello world"), ChatMessage::user("hi")];
        let base = estimate_messages_tokens(&msgs);
        assert_eq!(prompt_tokens(&msgs, 100), base + 100);
        // Exactly at the edge fits; one token over does not.
        let room = 8192 - reply_reserve(8192) - safety_margin(8192);
        assert!(fits(room, 8192, 1.0));
        assert!(!fits(room + 1, 8192, 1.0));
        // A server counting double means half the prompt fits.
        assert!(fits(room / 2, 8192, 0.5));
        assert!(!fits(room / 2 + 1, 8192, 0.5));
        assert_eq!(floor_tokens(500, 8192), 500 + 1024 + 1024);
    }

    #[test]
    fn compact_profile_wanted_auto_ratio() {
        use CompactProfileMode::*;
        // half of 8,192 is 4,096; the plan's 4,343 floor is over it, 4,000 is not.
        assert!(compact_profile_wanted(4_343, 8_192, Auto));
        assert!(!compact_profile_wanted(4_000, 8_192, Auto));
        assert!(compact_profile_wanted(10, 128_000, On));
        assert!(!compact_profile_wanted(6_937, 8_192, Off));
        assert!(!compact_profile_wanted(10_000, 128_000, Auto));
    }

    #[test]
    fn compact_tool_allowlist_matches_d03() {
        assert_eq!(
            compact_tool_allowlist(),
            [
                "terminal",
                "read_file",
                "write_file",
                "patch",
                "memory",
                "web_search",
                "web_read",
                "kanban_complete",
                "kanban_block"
            ]
        );
    }

    #[test]
    fn fit_margin_and_reserve_at_8192() {
        assert_eq!(reply_reserve(8192), 1024);
        assert_eq!(safety_margin(8192), 1024);
        assert_eq!(reply_reserve(128_000), 4096);
        assert_eq!(safety_margin(128_000), 8192);
        assert_eq!(reply_reserve(1_000), 1024);
        assert_eq!(safety_margin(1_000), 256);
    }

    #[test]
    fn fit_retry_window_and_scale() {
        let some = |w, r| ContextOverflow {
            window: w,
            requested: r,
        };
        assert_eq!(retry_window(128_000, some(Some(8192), None)), 8192);
        // a server can never raise the window
        assert_eq!(retry_window(8192, some(Some(65_536), None)), 8192);
        assert_eq!(retry_window(16_384, some(None, None)), 8192);
        assert_eq!(estimate_scale_from(1000, some(None, Some(2000))), 0.5);
        assert_eq!(estimate_scale_from(1000, some(None, Some(9000))), 0.25);
        assert_eq!(estimate_scale_from(1000, some(None, Some(500))), 1.0);
        assert_eq!(estimate_scale_from(1000, some(None, None)), 1.0);
    }

    #[test]
    fn fit_guard_protect_halves_per_pass() {
        // budget = 8192 - 0 - 1024 - 1024 = 6144
        assert_eq!(guard_protect_last(8192, 0, 1), 3072);
        assert_eq!(guard_protect_last(8192, 0, 2), 1536);
        assert_eq!(guard_protect_last(8192, 0, 3), 768);
        // tools come out of the budget; never underflows
        assert_eq!(guard_protect_last(8192, 2144, 1), 2000);
        assert_eq!(guard_protect_last(8192, 99_999, 1), 0);
    }

    #[test]
    fn fit_threshold_curve_tightens_below_32k() {
        let t = effective_compression_threshold;
        assert_eq!(t(0.5, None, 200_000), 0.5);
        assert_eq!(t(0.5, None, 32_768), 0.5);
        assert!((t(0.5, None, 16_384) - 0.425).abs() < 0.001);
        assert_eq!(t(0.5, None, 8_192), 0.35);
        assert_eq!(t(0.5, None, 4_096), 0.35);
        assert_eq!(t(0.85, None, 128_000), 0.85);
        assert_eq!(t(0.85, None, 8_192), 0.35);
        assert_eq!(t(0.30, None, 8_192), 0.30);
        assert_eq!(t(0.30, None, 200_000), 0.30);
    }

    #[test]
    fn fit_threshold_model_override_wins() {
        let t = effective_compression_threshold;
        assert_eq!(t(0.5, Some(0.40), 8_192), 0.40);
        assert_eq!(t(0.5, Some(0.40), 128_000), 0.40);
        assert_eq!(t(0.5, Some(0.01), 8_192), 0.05);
        assert_eq!(t(0.5, Some(0.99), 8_192), 0.95);
    }

    #[test]
    fn fit_protect_last_clamped_to_window() {
        assert_eq!(effective_protect_last(20_000, 8_192, 0.35), 1_433);
        assert_eq!(effective_protect_last(20_000, 128_000, 0.5), 20_000);
        assert_eq!(effective_protect_last(500, 8_192, 0.35), 500);
    }

    #[test]
    fn fit_summary_budget_no_panic_small_windows() {
        assert_eq!(summary_budget(0, 1_024), 51);
        assert_eq!(summary_budget(1_000_000, 1_024), 51);
        assert_eq!(summary_budget(1_000, 8_192), 409);
        assert_eq!(summary_budget(100_000, 8_192), 409);
        assert_eq!(summary_budget(1_000, 32_768), 1_638);
        assert_eq!(summary_budget(100_000, 32_768), 1_638);
        assert_eq!(summary_budget(1_000, 200_000), 2_000);
        assert_eq!(summary_budget(100_000, 200_000), 10_000);
        assert_eq!(summary_budget(1_000_000, 1_000_000), 12_000);
        assert_eq!(summary_char_cap(1_000, 8_192), 409 * 4);
    }

    #[test]
    fn fit_max_compression_passes_scales_for_small_windows() {
        assert_eq!(max_compression_passes(8_192), 40);
        assert_eq!(max_compression_passes(32_767), 40);
        assert_eq!(max_compression_passes(32_768), 10);
        assert_eq!(max_compression_passes(128_000), 10);
    }

    #[test]
    fn refusal_wording_follows_window_source() {
        let render = |from_server: bool| {
            [
                (
                    ContextWindowError::PromptFloorTooLarge {
                        window: 8192,
                        floor: 9001,
                        reserve: 2048,
                        from_server,
                    }
                    .to_string(),
                    vec!["8192", "9001"],
                ),
                (
                    ContextWindowError::RequestTooLarge {
                        window: 8192,
                        request: 9100,
                        from_server,
                    }
                    .to_string(),
                    vec!["8192", "9100"],
                ),
                (
                    ContextWindowError::RetryRejected {
                        window: 8192,
                        from_server,
                    }
                    .to_string(),
                    vec!["8192"],
                ),
            ]
        };
        for (text, numbers) in render(true) {
            assert!(
                text.contains("raise the context length in the model server"),
                "{text}"
            );
            assert!(!text.contains("providers.<provider>"), "{text}");
            assert!(!text.contains("model.context_length"), "{text}");
            for n in numbers {
                assert!(text.contains(n), "{text}");
            }
        }
        for (text, numbers) in render(false) {
            assert!(
                text.contains("providers.<provider>.models.<model>.context_length"),
                "{text}"
            );
            assert!(text.contains("never higher"), "{text}");
            for n in numbers {
                assert!(text.contains(n), "{text}");
            }
        }
    }
}
