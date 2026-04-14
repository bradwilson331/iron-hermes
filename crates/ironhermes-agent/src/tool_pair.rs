// Phase 18 Plan 02: tool-pair atomicity primitives (D-14, D-15, D-16).
//
// Pure functions used by `LocalPruningEngine` (and future engines) to (a) detect
// assistant↔tool-result pairings including parallel tool_calls, (b) apply the
// adaptive shift rule so a pair straddling the protect boundary is either
// expanded-in or prose-summarized, and (c) enforce the post-compression
// invariant that every `tool_calls[i].id` has a matching `tool.tool_call_id`
// and vice-versa. A violation returns `ContextError::OrphanedToolPair` to
// block the LLM API 400 failure mode called out in T-18-02.

use crate::context_compressor::estimate_message_tokens;
use crate::context_engine::ContextError;
use ironhermes_core::{ChatMessage, MessageContent, Role};

#[derive(Debug, Clone)]
pub struct ToolPair {
    pub assistant_idx: usize,
    pub tool_result_indices: Vec<usize>,
    pub tool_call_ids: Vec<String>,
}

/// Detect every assistant/tool_result pairing, including parallel tool_calls
/// produced by a single assistant message.
pub fn detect_tool_pairs(messages: &[ChatMessage]) -> Vec<ToolPair> {
    let mut pairs = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg.role != Role::Assistant {
            continue;
        }
        let Some(calls) = msg.tool_calls.as_ref() else {
            continue;
        };
        if calls.is_empty() {
            continue;
        }
        let ids: Vec<String> = calls.iter().map(|c| c.id.clone()).collect();
        let mut found: Vec<usize> = Vec::with_capacity(ids.len());
        for j in (i + 1)..messages.len() {
            let m = &messages[j];
            if m.role != Role::Tool {
                continue;
            }
            if let Some(ref tcid) = m.tool_call_id
                && ids.contains(tcid)
            {
                found.push(j);
            }
            if found.len() == ids.len() {
                break;
            }
        }
        if !found.is_empty() {
            pairs.push(ToolPair {
                assistant_idx: i,
                tool_result_indices: found,
                tool_call_ids: ids,
            });
        }
    }
    pairs
}

/// Apply the D-15 adaptive shift for a single pair straddling `protect_start`.
/// Returns the possibly-adjusted protect boundary. When the tool_result body is
/// large the content is rewritten in place to a prose summary instead.
pub fn apply_adaptive_shift(
    messages: &mut [ChatMessage],
    pair: &ToolPair,
    protect_start: usize,
    shift_threshold_tokens: usize,
) -> usize {
    let straddles = pair.assistant_idx < protect_start
        && pair
            .tool_result_indices
            .iter()
            .any(|&idx| idx >= protect_start);
    if !straddles {
        return protect_start;
    }
    let body_tokens: usize = pair
        .tool_result_indices
        .iter()
        .map(|&idx| estimate_message_tokens(&messages[idx]))
        .sum();
    if body_tokens <= shift_threshold_tokens {
        return pair.assistant_idx;
    }
    for &idx in &pair.tool_result_indices {
        let (name, args_preview, orig_tokens) = {
            let tc = messages[pair.assistant_idx]
                .tool_calls
                .as_ref()
                .and_then(|v| {
                    v.iter()
                        .find(|c| Some(&c.id) == messages[idx].tool_call_id.as_ref())
                });
            let name = tc
                .map(|c| c.function.name.clone())
                .unwrap_or_else(|| "<unknown>".into());
            let args_full = tc
                .map(|c| c.function.arguments.clone())
                .unwrap_or_default();
            let args_preview = if args_full.len() > 80 {
                format!("{}…", &args_full[..80])
            } else {
                args_full
            };
            (name, args_preview, estimate_message_tokens(&messages[idx]))
        };
        let prose = format!(
            "[Tool result summarized] Agent called {} with args {} and received output of ~{} tokens.",
            name, args_preview, orig_tokens
        );
        messages[idx].content = Some(MessageContent::Text(prose));
    }
    protect_start
}

/// Phase 18 Plan 11: compute the effective `protect_first_n` boundary for
/// `SummarizingEngine::compress` given the configured value and the detected
/// tool pairs in the message list.
///
/// Rule (safety-over-recovery, per 18-10 precedent): when an assistant
/// `tool_use` message is pinned inside the front-protected region
/// (`asst_idx < configured_first_n`) and has at least one matching
/// `tool_result` OUTSIDE that region (`max(tool_result_indices) >=
/// configured_first_n`), the effective boundary auto-shrinks to `asst_idx` so
/// the whole pair falls into the prunable range and can be summarized
/// atomically. The helper NEVER grows the boundary above the configured
/// value — operator intent is an upper bound.
///
/// Invariants:
///   1. `configured_first_n == 0` → returns `0` (nothing to shrink).
///   2. If no pair conflicts exist, the configured value is returned unchanged.
///   3. With multiple conflicting pairs, returns the MINIMUM of their
///      `asst_idx` (most protective; releases the earliest pinned pair).
///   4. Result is always `<= configured_first_n`.
///
/// `_messages` is reserved for future extensions (e.g. inspecting
/// message-level flags) and is intentionally unused today.
pub fn compute_effective_protect_first_n(
    _messages: &[ChatMessage],
    configured_first_n: usize,
    pairs: &[ToolPair],
) -> usize {
    if configured_first_n == 0 {
        return 0;
    }
    let mut effective = configured_first_n;
    for pair in pairs {
        if pair.assistant_idx >= configured_first_n {
            continue;
        }
        let any_outside = pair
            .tool_result_indices
            .iter()
            .any(|&i| i >= configured_first_n);
        if any_outside && pair.assistant_idx < effective {
            effective = pair.assistant_idx;
        }
    }
    effective
}

/// Post-compression invariant (D-16): every assistant tool_call id must have a
/// subsequent matching tool message, and every tool message must refer to a
/// preceding assistant tool_call.
pub fn check_orphan_invariant(messages: &[ChatMessage]) -> Result<(), ContextError> {
    use std::collections::HashSet;
    let mut unmatched_calls: HashSet<String> = HashSet::new();
    for msg in messages {
        match msg.role {
            Role::Assistant => {
                if let Some(ref calls) = msg.tool_calls {
                    for c in calls {
                        unmatched_calls.insert(c.id.clone());
                    }
                }
            }
            Role::Tool => match msg.tool_call_id.as_ref() {
                Some(id) if unmatched_calls.contains(id) => {
                    unmatched_calls.remove(id);
                }
                _ => return Err(ContextError::OrphanedToolPair),
            },
            _ => {}
        }
    }
    if !unmatched_calls.is_empty() {
        return Err(ContextError::OrphanedToolPair);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::{FunctionCall, ToolCall};

    fn tc(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: args.into(),
            },
        }
    }

    #[test]
    fn tool_pair_detection() {
        let msgs = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("hi"),
            ChatMessage::assistant_tool_calls(vec![tc("tc1", "fn_a", "{}"), tc("tc2", "fn_b", "{}")]),
            ChatMessage::tool_result("tc1", "r1"),
            ChatMessage::tool_result("tc2", "r2"),
            ChatMessage::assistant("done"),
        ];
        let pairs = detect_tool_pairs(&msgs);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].assistant_idx, 2);
        assert_eq!(pairs[0].tool_result_indices, vec![3, 4]);
        assert_eq!(pairs[0].tool_call_ids, vec!["tc1", "tc2"]);
    }

    #[test]
    fn tool_pair_detection_no_calls() {
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant("a"),
        ];
        assert!(detect_tool_pairs(&msgs).is_empty());
    }

    #[test]
    fn adaptive_shift_forward() {
        // Pair straddles boundary; tool_result body is small.
        let mut msgs = vec![
            ChatMessage::assistant_tool_calls(vec![tc("x1", "fn", "{}")]),
            ChatMessage::tool_result("x1", "tiny"),
        ];
        let pair = detect_tool_pairs(&msgs).remove(0);
        let new_start = apply_adaptive_shift(&mut msgs, &pair, 1, 500);
        assert_eq!(new_start, 0, "small pair should expand protect boundary");
        // Content preserved (no backward summarization)
        assert_eq!(msgs[1].content_text(), Some("tiny"));
    }

    #[test]
    fn adaptive_shift_backward() {
        let big_body = "x".repeat(4_000); // ~1000+ estimated tokens
        let mut msgs = vec![
            ChatMessage::assistant_tool_calls(vec![tc("b1", "big_fn", "{\"a\":1}")]),
            ChatMessage::tool_result("b1", big_body.clone()),
        ];
        let pair = detect_tool_pairs(&msgs).remove(0);
        let new_start = apply_adaptive_shift(&mut msgs, &pair, 1, 500);
        assert_eq!(new_start, 1, "large pair keeps boundary, content rewritten");
        let text = msgs[1].content_text().unwrap_or("");
        assert!(text.starts_with("[Tool result summarized] Agent called big_fn"));
        assert!(!text.contains(&big_body));
    }

    #[test]
    fn orphaned_pair_rejection() {
        let msgs = vec![
            ChatMessage::assistant_tool_calls(vec![tc("tc1", "fn", "{}")]),
            ChatMessage::user("hi"),
        ];
        assert!(matches!(
            check_orphan_invariant(&msgs),
            Err(ContextError::OrphanedToolPair)
        ));
    }

    #[test]
    fn orphaned_pair_reverse() {
        let msgs = vec![
            ChatMessage::tool_result("tc1", "r"),
            ChatMessage::user("hi"),
        ];
        assert!(matches!(
            check_orphan_invariant(&msgs),
            Err(ContextError::OrphanedToolPair)
        ));
    }

    #[test]
    fn adaptive_shift_return_value_applied_by_caller_contract() {
        // Contract: apply_adaptive_shift returns the adjusted protect_start that
        // the caller MUST use as the effective prune boundary. Phase 18 Plan 10
        // closes the defect where SummarizingEngine discarded this value.
        //
        // Arm 1: small body straddling → returned < original (shift forward).
        let mut msgs_small = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant("a"),
            ChatMessage::assistant_tool_calls(vec![tc("s1", "fn", "{}")]),
            ChatMessage::tool_result("s1", "tiny"),
            ChatMessage::user("tail"),
        ];
        let pair = detect_tool_pairs(&msgs_small).remove(0);
        let original = 4; // tool_result index — pair straddles 3→4
        let returned_small = apply_adaptive_shift(&mut msgs_small, &pair, original, 500);
        assert!(
            returned_small < original,
            "small body must shift boundary FORWARD (returned={} original={})",
            returned_small, original
        );
        // Content is preserved in-place (no backward summarization).
        assert_eq!(msgs_small[4].content_text(), Some("tiny"));

        // Arm 2: large body straddling → returned == original AND content rewritten in place.
        let big_body = "x".repeat(4_000);
        let mut msgs_big = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant("a"),
            ChatMessage::assistant_tool_calls(vec![tc("b1", "big_fn", "{\"a\":1}")]),
            ChatMessage::tool_result("b1", big_body.clone()),
            ChatMessage::user("tail"),
        ];
        let pair = detect_tool_pairs(&msgs_big).remove(0);
        let original = 4;
        let returned_big = apply_adaptive_shift(&mut msgs_big, &pair, original, 500);
        assert_eq!(
            returned_big, original,
            "large body must KEEP boundary (returned unchanged)"
        );
        let rewritten = msgs_big[4].content_text().unwrap_or("");
        assert!(
            rewritten.starts_with("[Tool result summarized]"),
            "large body must be rewritten in place, got: {}", rewritten
        );
        assert!(!rewritten.contains(&big_body));
    }

    // ── Phase 18 Plan 11: compute_effective_protect_first_n unit tests ──────

    #[test]
    fn effective_protect_first_n_single_pair_front_straddle_shrinks() {
        // asst at idx 2, result at idx 3, configured=3 → asst_idx < 3 and
        // result outside → shrink to asst_idx=2.
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant_tool_calls(vec![tc("a", "fn", "{}")]),
            ChatMessage::tool_result("a", "r"),
        ];
        let pairs = detect_tool_pairs(&msgs);
        assert_eq!(pairs.len(), 1);
        let eff = compute_effective_protect_first_n(&msgs, 3, &pairs);
        assert_eq!(eff, 2, "front-straddle must shrink effective to asst_idx");
    }

    #[test]
    fn effective_protect_first_n_parallel_tool_calls_shrinks() {
        // asst at idx 2, results at idx 3 and 4, configured=3 → shrink to 2.
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant_tool_calls(vec![
                tc("p1", "fn_a", "{}"),
                tc("p2", "fn_b", "{}"),
            ]),
            ChatMessage::tool_result("p1", "r1"),
            ChatMessage::tool_result("p2", "r2"),
        ];
        let pairs = detect_tool_pairs(&msgs);
        assert_eq!(pairs.len(), 1);
        let eff = compute_effective_protect_first_n(&msgs, 3, &pairs);
        assert_eq!(eff, 2, "parallel tool_calls must shrink to asst_idx");
    }

    #[test]
    fn effective_protect_first_n_pair_fully_inside_protect_no_change() {
        // asst at idx 1, result at idx 2, configured=4 → both inside
        // [0, 4) → no shrink.
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::assistant_tool_calls(vec![tc("a", "fn", "{}")]),
            ChatMessage::tool_result("a", "r"),
            ChatMessage::user("u"),
            ChatMessage::user("tail"),
        ];
        let pairs = detect_tool_pairs(&msgs);
        assert_eq!(pairs.len(), 1);
        let eff = compute_effective_protect_first_n(&msgs, 4, &pairs);
        assert_eq!(
            eff, 4,
            "pair fully inside front-protect must not shrink (asst_idx=1, result=2, configured=4)"
        );
    }

    #[test]
    fn effective_protect_first_n_pair_fully_outside_protect_no_change() {
        // asst at idx 5, result at idx 6, configured=3 → pair entirely
        // outside front-protect → no change.
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::user("u"),
            ChatMessage::user("u"),
            ChatMessage::user("u"),
            ChatMessage::assistant_tool_calls(vec![tc("a", "fn", "{}")]),
            ChatMessage::tool_result("a", "r"),
        ];
        let pairs = detect_tool_pairs(&msgs);
        assert_eq!(pairs.len(), 1);
        let eff = compute_effective_protect_first_n(&msgs, 3, &pairs);
        assert_eq!(eff, 3, "pair fully outside front-protect must not shrink");
    }

    #[test]
    fn effective_protect_first_n_no_pairs_returns_configured() {
        // Empty pairs list, configured=3 → returns 3.
        let msgs: Vec<ChatMessage> = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant("a"),
        ];
        let pairs: Vec<ToolPair> = Vec::new();
        let eff = compute_effective_protect_first_n(&msgs, 3, &pairs);
        assert_eq!(eff, 3, "no pairs must return configured value");
    }

    #[test]
    fn effective_protect_first_n_multiple_pairs_picks_min() {
        // Two front-straddling pairs: asst at 1 (result at 4) and asst at 2
        // (result at 5), configured=4 → shrink to min(1, 2) = 1.
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::assistant_tool_calls(vec![tc("a1", "fn", "{}")]),
            ChatMessage::assistant_tool_calls(vec![tc("a2", "fn", "{}")]),
            ChatMessage::user("u"),
            ChatMessage::tool_result("a1", "r1"),
            ChatMessage::tool_result("a2", "r2"),
        ];
        let pairs = detect_tool_pairs(&msgs);
        assert_eq!(pairs.len(), 2, "two pairs expected");
        let eff = compute_effective_protect_first_n(&msgs, 4, &pairs);
        assert_eq!(
            eff, 1,
            "multiple conflicting pairs must pick min(asst_idx)"
        );
    }

    #[test]
    fn effective_protect_first_n_malformed_asst_at_boundary_no_result_no_change() {
        // Assistant tool_calls at idx 2, but NO matching tool_result exists
        // in the message list. `detect_tool_pairs` returns no pair for it.
        // Helper sees empty pairs list → returns configured value unchanged.
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant_tool_calls(vec![tc("orphan", "fn", "{}")]),
            ChatMessage::user("next user"),
        ];
        let pairs = detect_tool_pairs(&msgs);
        assert!(
            pairs.is_empty(),
            "orphan tool_calls must not yield a pair (detect requires matching result)"
        );
        let eff = compute_effective_protect_first_n(&msgs, 3, &pairs);
        assert_eq!(
            eff, 3,
            "no detected pair → no shrink even if malformed asst lives in front-protect"
        );
    }

    #[test]
    fn effective_protect_first_n_zero_configured_returns_zero() {
        // configured_first_n == 0 short-circuits to 0 regardless of pairs.
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant_tool_calls(vec![tc("a", "fn", "{}")]),
            ChatMessage::tool_result("a", "r"),
        ];
        let pairs = detect_tool_pairs(&msgs);
        let eff = compute_effective_protect_first_n(&msgs, 0, &pairs);
        assert_eq!(eff, 0);
    }

    #[test]
    fn orphan_invariant_passes_clean_list() {
        let msgs = vec![
            ChatMessage::user("hi"),
            ChatMessage::assistant_tool_calls(vec![tc("ok", "fn", "{}")]),
            ChatMessage::tool_result("ok", "result"),
            ChatMessage::assistant("done"),
        ];
        assert!(check_orphan_invariant(&msgs).is_ok());
    }
}
