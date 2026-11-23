//! Phase 58.3.1 D-09: auxiliary model calls (summarizer, vision, web_extract,
//! tier2 memory, kanban judge and decomposers) recover from a context-length
//! rejection the same way the agent loop does: learn the window, shrink the
//! payload, retry exactly once.

use std::future::Future;

use anyhow::Result;
use ironhermes_core::observed_windows::{self, ObservedWindowSource};
use ironhermes_core::types::MessageContent;
use ironhermes_core::{ChatMessage, ChatResponse, Role};

use crate::context_compressor::{estimate_messages_tokens, estimate_tokens};
use crate::error_classifier::{
    ProviderError, classify_llm_error_typed, parse_context_overflow, truncate_chars_with_marker,
};
use crate::fit::{reply_reserve, safety_margin};

/// Provider detail embedded in the final error (T-RKZ-02 budget).
const DETAIL_CHAR_BUDGET: usize = 400;
/// Texts shorter than this are never truncated.
const MIN_TRUNCATABLE_CHARS: usize = 400;
const MAX_SHRINK_ROUNDS: usize = 8;

/// Shrink `messages` until their estimate is `<= budget_tokens` by cutting the
/// middle out of the longest non-system text message, leaving a visible marker.
/// System messages (the caller's own instructions) and `Parts` (image payloads)
/// are never edited; `None` when the budget cannot be reached.
pub fn shrink_messages_to_budget(
    messages: &[ChatMessage],
    budget_tokens: usize,
) -> Option<Vec<ChatMessage>> {
    let mut out = messages.to_vec();
    for _ in 0..MAX_SHRINK_ROUNDS {
        let total = estimate_messages_tokens(&out);
        if total <= budget_tokens {
            return Some(out);
        }
        let (idx, len) = out
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role != Role::System)
            .filter_map(|(i, m)| match &m.content {
                Some(MessageContent::Text(t)) => Some((i, t.chars().count())),
                _ => None,
            })
            .max_by_key(|&(_, len)| len)?;
        if len < MIN_TRUNCATABLE_CHARS {
            return None;
        }
        let Some(MessageContent::Text(text)) = out[idx].content.as_mut() else {
            return None;
        };
        // Cut in proportion to the message's own chars-per-token, so dense text is not over-cut.
        let overshoot = total - budget_tokens;
        let msg_tokens = estimate_tokens(text).max(1);
        let remove = (len * overshoot)
            .div_ceil(msg_tokens)
            .saturating_add(64)
            .min(len);
        let kept = len - remove;
        let (head, tail) = (kept * 2 / 3, kept - kept * 2 / 3);
        let chars: Vec<char> = text.chars().collect();
        let mut s: String = chars[..head].iter().collect();
        s.push_str(&format!(
            "\n[... {remove} characters removed to fit the model's context window ...]\n"
        ));
        s.extend(&chars[len - tail..]);
        *text = s;
    }
    (estimate_messages_tokens(&out) <= budget_tokens).then_some(out)
}

/// Cap `max_tokens` at a quarter of `window` and shrink `messages` to what is
/// left after the reply and the safety margin. `None` when it cannot be done.
fn size_for_window(
    messages: &[ChatMessage],
    max_tokens: Option<usize>,
    window: usize,
) -> Option<(Vec<ChatMessage>, Option<usize>)> {
    let capped = max_tokens.map(|m| m.min(window / 4));
    let budget = window
        .checked_sub(capped.unwrap_or_else(|| reply_reserve(window)))?
        .checked_sub(safety_margin(window))?;
    Some((shrink_messages_to_budget(messages, budget)?, capped))
}

/// Run `call` with learn-shrink-retry around a context-length rejection.
/// Non-context errors are returned unchanged.
pub async fn chat_with_context_recovery<F, Fut>(
    base_url: &str,
    model: &str,
    messages: &[ChatMessage],
    max_tokens: Option<usize>,
    call: F,
) -> Result<ChatResponse>
where
    F: Fn(Vec<ChatMessage>, Option<usize>) -> Fut,
    Fut: Future<Output = Result<ChatResponse>>,
{
    // A window already known for this endpoint + model is applied before the first send.
    let known = observed_windows::lookup(base_url, model).map(|w| w.tokens);
    let (first_messages, first_max) = match known {
        Some(w) => {
            let over =
                estimate_messages_tokens(messages) + max_tokens.unwrap_or(0) + safety_margin(w) > w;
            if over {
                size_for_window(messages, max_tokens, w)
                    .unwrap_or_else(|| (messages.to_vec(), max_tokens))
            } else {
                (messages.to_vec(), max_tokens)
            }
        }
        None => (messages.to_vec(), max_tokens),
    };

    let first_prompt = estimate_messages_tokens(&first_messages);
    let err = match call(first_messages, first_max).await {
        Ok(r) => return Ok(r),
        Err(e) => e,
    };
    if !matches!(classify_llm_error_typed(&err), ProviderError::ContextLength) {
        return Err(err);
    }

    let text = format!("{err:#}");
    let overflow = parse_context_overflow(&text);
    let assumed = known.unwrap_or(first_prompt + first_max.unwrap_or(0));
    // D-08: no number in the error text halves the assumed window.
    let w = overflow.window.unwrap_or(assumed / 2);
    if overflow.window.is_some() && w < assumed {
        observed_windows::record(base_url, model, w, ObservedWindowSource::Learned);
    }

    // `max_tokens` is the caller's original value so a retry never shrinks the
    // reply cap twice; `size_for_window` re-derives both from the window.
    let Some((retry_messages, retry_max)) = size_for_window(messages, max_tokens, w) else {
        return Err(too_long_error(model, w, &text));
    };
    match call(retry_messages, retry_max).await {
        Err(e2) if matches!(classify_llm_error_typed(&e2), ProviderError::ContextLength) => {
            Err(too_long_error(model, w, &format!("{e2:#}")))
        }
        other => other,
    }
}

fn too_long_error(model: &str, window: usize, provider_text: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "The auxiliary model {model} rejected the request as too long for its {window}-token \
         context window, even after the agent shortened it and retried once. {} Provider \
         detail: {}",
        // The server rejected twice, so a larger config value cannot help.
        crate::fit::fix_it_sentence(true, "shorten the input"),
        truncate_chars_with_marker(provider_text, DETAIL_CHAR_BUDGET)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests must not init the process-wide estimator: it is a OnceLock shared by
    // every lib test in the process, and cl100k makes the repeated-text fixtures in
    // `summarizing_engine` tests spin forever under `--test-threads=1`.
    fn filler(tokens: usize) -> String {
        // Varied words so any estimator sees roughly `tokens` tokens.
        (0..tokens)
            .map(|i| format!("w{} ", i % 977))
            .collect::<String>()
    }

    #[test]
    fn shrink_messages_truncates_largest_text_first() {
        let sys = filler(50);
        let msgs = vec![
            ChatMessage::system(sys.clone()),
            ChatMessage::user(filler(9_000)),
        ];
        let out = shrink_messages_to_budget(&msgs, 6_000).expect("fits after shrink");
        assert!(
            matches!(&out[0].content, Some(MessageContent::Text(t)) if *t == sys),
            "system message unchanged"
        );
        let Some(MessageContent::Text(t)) = &out[1].content else {
            panic!("user text expected")
        };
        assert!(t.contains("characters removed to fit the model's context window"));
        assert!(t.starts_with("w0 w1 "), "keeps the head");
        assert!(
            t.trim_end().ends_with(&format!("w{}", 8_999 % 977)),
            "keeps the tail"
        );
        assert!(estimate_messages_tokens(&out) <= 6_000);
    }

    #[test]
    fn shrink_messages_returns_none_when_impossible() {
        let msgs = vec![ChatMessage::system(filler(7_000))];
        assert!(shrink_messages_to_budget(&msgs, 1_000).is_none());
    }
}
