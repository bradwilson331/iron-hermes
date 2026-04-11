---
phase: 09-subagent-delegation
reviewed: 2026-04-11T12:00:00Z
depth: standard
files_reviewed: 14
files_reviewed_list:
  - crates/ironhermes-agent/Cargo.toml
  - crates/ironhermes-agent/src/agent_loop.rs
  - crates/ironhermes-agent/src/lib.rs
  - crates/ironhermes-agent/src/subagent_runner.rs
  - crates/ironhermes-cli/Cargo.toml
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-core/src/config.rs
  - crates/ironhermes-core/src/lib.rs
  - crates/ironhermes-tools/Cargo.toml
  - crates/ironhermes-tools/src/delegate_task.rs
  - crates/ironhermes-tools/src/lib.rs
  - crates/ironhermes-tools/src/memory_tool.rs
  - crates/ironhermes-tools/src/registry.rs
  - crates/ironhermes-tools/src/terminal.rs
findings:
  critical: 2
  warning: 3
  info: 2
  total: 7
status: issues_found
---

# Phase 9: Code Review Report

**Reviewed:** 2026-04-11T12:00:00Z
**Depth:** standard
**Files Reviewed:** 14
**Status:** issues_found

## Summary

Reviewed the subagent delegation subsystem spanning ironhermes-agent, ironhermes-tools, ironhermes-cli, and ironhermes-core. The architecture is well-designed: dependency inversion via the SubagentRunner trait avoids circular crate dependencies, recursive delegation is structurally prevented (AGENT-05), memory is properly read-only for children with a separate schema, terminal CWD is isolated, and concurrency is semaphore-controlled with elapsed-time logging.

Several issues from the prior review (2026-04-10) have been fixed: terminal.rs char-boundary truncation is now safe, the read-only memory schema is properly restricted, and semaphore wait logging uses elapsed time instead of a racy flag. Two new critical issues remain: unsafe byte-offset string slicing in three locations that can panic on multi-byte UTF-8 input from LLM-generated content. Three warnings cover a schema/validation mismatch, batch error indexing, and a guardrail check inconsistency.

## Critical Issues

### CR-01: Unsafe string slice at byte offset panics on multi-byte UTF-8

**File:** `crates/ironhermes-agent/src/agent_loop.rs:366`
**Issue:** The expression `&args_str[..100]` slices a `String` at byte offset 100 for the tool progress preview. If `args_str` contains multi-byte UTF-8 characters (CJK text, emoji, accented characters in LLM-generated tool arguments) and byte 100 falls in the middle of a multi-byte sequence, this panics with "byte index 100 is not a char boundary." The terminal.rs truncation was already fixed with `is_char_boundary` but this location was missed.
**Fix:**
```rust
let preview = if args_str.len() > 100 {
    let mut end = 100;
    while !args_str.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &args_str[..end])
} else {
    args_str.clone()
};
```

### CR-02: Unsafe string slice at byte offset panics on multi-byte UTF-8 (two more locations)

**File:** `crates/ironhermes-tools/src/delegate_task.rs:228` and `crates/ironhermes-tools/src/delegate_task.rs:509`
**Issue:** Both `&goal[..50]` (line 228, batch mode progress callback) and `&task[..50]` (line 509, single mode progress callback) slice at byte offset 50. Since task descriptions originate from LLM output, non-ASCII content is expected. If byte 50 falls mid-character, this panics at runtime.
**Fix:** Apply the same `is_char_boundary` pattern used in terminal.rs:
```rust
let summary = if goal.len() > 50 {
    let mut end = 50;
    while !goal.is_char_boundary(end) {
        end -= 1;
    }
    &goal[..end]
} else {
    &goal
};
```
Apply to both line 228 (`goal`) and line 509 (`task`).

## Warnings

### WR-01: Schema declares "task" as required but batch mode uses "tasks" without it

**File:** `crates/ironhermes-tools/src/delegate_task.rs:422`
**Issue:** The JSON schema specifies `"required": ["task"]`, but the `execute()` method (line 429) first checks for a `tasks` array for batch mode. When the LLM sends `{"tasks": [...]}` without a `"task"` field, strict schema validation by LLM providers would reject the call before it reaches execute(). The schema does not express the task/tasks mutual exclusivity.
**Fix:** Remove `"task"` from `required` and validate in `execute()`:
```rust
// In schema():
// Remove: "required": ["task"]
// Add: "required": []

// At top of execute():
if args.get("tasks").is_none() && args.get("task").is_none() {
    anyhow::bail!("Either 'task' (single mode) or 'tasks' (batch mode) is required");
}
```

### WR-02: Batch error collection uses wrong index for failed/panicked tasks

**File:** `crates/ironhermes-tools/src/delegate_task.rs:289-290`
**Issue:** When a spawned batch task returns `Err` or panics, the index is computed as `results.len()` instead of the actual task index. Since results arrive in non-deterministic order, `results.len()` does not correspond to the original task index. This causes incorrect "Task N Result" numbering in the output and can produce duplicate indices that break the sort-by-index guarantee (D-07).
**Fix:** Use `enumerate()` on the handles to track expected indices:
```rust
for (expected_idx, handle) in handles.into_iter().enumerate() {
    match handle.await {
        Ok(Ok((idx, response))) => results.push((idx, response)),
        Ok(Err(e)) => results.push((expected_idx, format!("Error: {}", e))),
        Err(e) => results.push((expected_idx, format!("Task panicked: {}", e))),
    }
}
```

### WR-03: Guardrail check_guardrails() returns early on Warn, missing later Block decisions

**File:** `crates/ironhermes-tools/src/registry.rs:91-109`
**Issue:** `check_guardrails()` returns immediately when it encounters a `Warn` decision (line 100-101), without checking remaining guardrails. In contrast, `dispatch_with_hook()` (lines 166-188) continues iterating on `Warn`. If guardrail A returns `Warn` and guardrail B returns `Block`, `check_guardrails()` returns `Warn` (missing the block), while `dispatch_with_hook()` correctly returns `Block`. Since `agent_loop.rs` uses the split API (`check_guardrails` + `execute_tool`), a Block from a later guardrail can be bypassed.
**Fix:** Make `check_guardrails` continue iterating on `Warn`, matching `dispatch_with_hook` behavior:
```rust
pub fn check_guardrails(&self, name: &str, args: &serde_json::Value) -> GuardrailDecision {
    let mut last_warn = None;
    for guardrail in &self.guardrails {
        match guardrail.check(name, args) {
            GuardrailDecision::Allow => {}
            GuardrailDecision::Warn { reason } => {
                tracing::warn!(
                    tool = %name,
                    guardrail = %guardrail.name(),
                    reason = %reason,
                    "Guardrail warning (proceeding)"
                );
                last_warn = Some(GuardrailDecision::Warn { reason });
                // Continue -- a later guardrail might Block
            }
            GuardrailDecision::Block { reason } => {
                return GuardrailDecision::Block { reason };
            }
        }
    }
    last_warn.unwrap_or(GuardrailDecision::Allow)
}
```

## Info

### IN-01: Memory tool "get" action in read-only schema has no execute() handler

**File:** `crates/ironhermes-tools/src/memory_tool.rs:58` and `crates/ironhermes-tools/src/memory_tool.rs:128-168`
**Issue:** The read-only schema (line 58) declares `"enum": ["get"]` as the only valid action, but `execute()` has no match arm for `"get"`. A subagent calling `{"action": "get", "target": "memory"}` falls through to the catch-all (line 164), returning an error about unknown action. The schema advertises a capability the implementation does not support. Memory content is injected via system prompt, so this is low-impact, but the schema is misleading.
**Fix:** Either add a `"get"` match arm that returns the current memory contents, or change the read-only schema description to clarify that memory is read via system prompt only and remove the `"get"` enum value.

### IN-02: `resolve_api_key().unwrap_or_default()` silently produces empty API key

**File:** `crates/ironhermes-cli/src/main.rs:239` and `crates/ironhermes-cli/src/main.rs:303`
**Issue:** When constructing `AgentSubagentRunner`, `config.resolve_api_key().unwrap_or_default()` converts a missing API key to an empty string. Subagent child clients will be constructed with an empty key, leading to opaque authentication failures from the LLM provider rather than a clear error at setup time. This only affects subagent calls (the parent client fails properly via `.context()` on line 607), so impact is limited to the delegation path.

---

_Reviewed: 2026-04-11T12:00:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
