---
phase: 09-subagent-delegation
reviewed: 2026-04-10T18:15:00Z
depth: standard
files_reviewed: 11
files_reviewed_list:
  - crates/ironhermes-agent/src/lib.rs
  - crates/ironhermes-agent/src/subagent_runner.rs
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
  critical: 1
  warning: 4
  info: 2
  total: 7
status: issues_found
---

# Phase 9: Code Review Report

**Reviewed:** 2026-04-10T18:15:00Z
**Depth:** standard
**Files Reviewed:** 11
**Status:** issues_found

## Summary

Phase 09 implements subagent delegation with a well-structured dependency-inversion pattern (SubagentRunner trait) to avoid circular crate dependencies. The core security requirements are met: recursive delegation is structurally prevented (AGENT-05), memory is read-only for children, terminal CWD is isolated, and concurrency is semaphore-controlled.

The review identified one critical issue (string truncation at a non-char-boundary causing a panic), several warnings around edge cases in concurrency logging and memory-tool schema inconsistency, and minor info items.

## Critical Issues

### CR-01: String truncation at non-char-boundary causes panic

**File:** `crates/ironhermes-tools/src/terminal.rs:113-114`
**Issue:** The output truncation `&result[..MAX_OUTPUT_LEN]` slices by byte index, not by character boundary. If the output contains multi-byte UTF-8 characters (common in international output, emoji, or binary-like data), slicing at byte 50000 may land in the middle of a multi-byte character, causing a panic at runtime.
**Fix:**
```rust
if result.len() > MAX_OUTPUT_LEN {
    // Find the nearest char boundary at or before MAX_OUTPUT_LEN
    let mut end = MAX_OUTPUT_LEN;
    while !result.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = &result[..end];
    Ok(format!("{}\n[truncated]", truncated))
} else {
    Ok(result)
}
```

## Warnings

### WR-01: Semaphore "waited" flag is racy and may produce incorrect log messages

**File:** `crates/ironhermes-tools/src/delegate_task.rs:193-199`
**Issue:** The `waited` flag is computed by checking `available_permits() == 0` before `acquire().await`. Between the check and the acquire, other tasks may release or acquire permits, so the flag can be wrong in both directions: (1) permits were 0 but one freed before acquire completed (false positive -- "waited" message prepended when no wait occurred), or (2) permits were >0 but another task grabbed the last one before this acquire (false negative -- no message when there was a wait). This is a cosmetic issue only (no correctness impact), but it could confuse operators reviewing logs.
**Fix:** Remove the "waited" flag entirely and instead measure actual wait time:
```rust
let start = std::time::Instant::now();
let _permit = self.semaphore.acquire().await
    .map_err(|e| anyhow::anyhow!("Semaphore closed: {}", e))?;
let wait_duration = start.elapsed();
let waited = wait_duration > Duration::from_millis(50);
if waited {
    info!(
        "Acquired subagent slot after waiting {}ms",
        wait_duration.as_millis()
    );
}
```

### WR-02: Memory tool schema advertises write actions in read-only mode

**File:** `crates/ironhermes-tools/src/memory_tool.rs:49-78`
**Issue:** When `MemoryTool::new_read_only()` is used, the tool schema still advertises `"add"`, `"replace"`, and `"remove"` as valid actions in the `enum`. The child agent's LLM will see these as valid options, attempt them, and get a runtime error string. This wastes LLM turns and tokens. The schema should reflect the actual capabilities.
**Fix:** Override `schema()` to return a read-only variant when `self.read_only` is true, or define a separate schema that only lists available actions. A simpler approach: adjust the description when read-only:
```rust
fn description(&self) -> &str {
    if self.read_only {
        "Query persistent facts from memory. This is a read-only view; add/replace/remove are not available in subagent context."
    } else {
        "Save, update, or remove persistent facts..."
    }
}
```
Additionally, consider filtering the `enum` values in the schema when read-only.

### WR-03: Child subagent has no hook registry -- tool events are silently lost

**File:** `crates/ironhermes-agent/src/subagent_runner.rs:39`
**Issue:** `AgentSubagentRunner::run_child()` creates a child `AgentLoop` without calling `.with_hook_registry()`. This means any tool calls made by the child agent will not emit `ToolCalled` or `ToolCompleted` hook events. For operators using webhook listeners or JSONL event logs (Phase 7 features), child agent activity is invisible. This was noted as acceptable in the plan but is worth tracking.
**Fix:** Pass an `Option<Arc<HookRegistry>>` through the `SubagentRunner` trait and `AgentSubagentRunner`, and chain `.with_hook_registry()` when available:
```rust
pub trait SubagentRunner: Send + Sync {
    async fn run_child(
        &self,
        registry: Arc<ToolRegistry>,
        system_prompt: String,
        max_iterations: usize,
        hook_registry: Option<Arc<HookRegistry>>,
    ) -> anyhow::Result<Option<String>>;
}
```

### WR-04: Missing `memory` tool silently ignored when no MemoryStore provided

**File:** `crates/ironhermes-tools/src/delegate_task.rs:108-114`
**Issue:** When `"memory"` is in the allowed_tools list but `memory_store` is `None` (as in CLI chat and single modes), the memory tool is silently not registered. The child agent's LLM will be told it can use "memory" (via the task or default safe tools list) but the tool won't exist, causing dispatch errors. This happens in `run_single()` and `run_chat()` where `memory_store` is `None`.
**Fix:** Either skip `"memory"` from `DEFAULT_SAFE_TOOLS` when `memory_store` is `None` at the call site, or emit a warning log in `build_child_registry`:
```rust
"memory" => {
    if let Some(ref store) = memory_store {
        registry.register(Box::new(
            crate::memory_tool::MemoryTool::new_read_only(store.clone()),
        ));
    } else {
        tracing::warn!("memory tool requested but no MemoryStore available; skipping");
    }
}
```

## Info

### IN-01: MemoryTool description does not mention read capabilities

**File:** `crates/ironhermes-tools/src/memory_tool.rs:136-139`
**Issue:** The `execute()` match for unknown actions says `"Valid actions: add, replace, remove"` but there is no read/query action implemented. The schema `enum` only lists `["add", "replace", "remove"]`. The read-only error message references "query and get actions" that do not exist in the tool. Memory content is injected into the system prompt, not queried via tool calls -- the error message is misleading.
**Fix:** Update the read-only error message to accurately reflect how memory works:
```rust
"Error: memory is read-only in subagent context. Memory facts are available in the system prompt; add/replace/remove actions are disabled."
```

### IN-02: Unused import potential in ironhermes-core/src/lib.rs

**File:** `crates/ironhermes-core/src/lib.rs:10`
**Issue:** `SubagentConfig` is re-exported from the crate root alongside `Config` and `ExecConfig`. This is consistent with the existing pattern and correct. No issue -- this is a positive observation confirming the export is properly structured.

---

_Reviewed: 2026-04-10T18:15:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
