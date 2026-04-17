---
phase: 22-cli-feature-parity
reviewed: 2026-04-17T16:50:00Z
depth: standard
files_reviewed: 2
files_reviewed_list:
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-cli/tests/cli_tool_parity.rs
findings:
  critical: 0
  warning: 3
  info: 3
  total: 6
status: issues_found
---

# Phase 22: Code Review Report

**Reviewed:** 2026-04-17T16:50:00Z
**Depth:** standard
**Files Reviewed:** 2
**Status:** issues_found

## Summary

Reviewed `main.rs` (1133 lines, CLI entrypoint with chat/single/gateway modes) and `cli_tool_parity.rs` (161 lines, static-grep regression tests). The CLI correctly wires all Phase 22 tools (cron, skills, execute_code, hooks, guardrails, webhooks) across `run_single`, `run_chat`, and `run_gateway` with strong consistency. The parity test file is well-designed, using brace-balanced function body extraction to guard against accidental removal of tool registrations.

Three warnings were identified: an inconsistent error handling pattern in `run_gateway` that uses `.expect()` where sibling functions use `.context()?`, an uncancellable `CancellationToken` in the gateway path, and the `stream` CLI flag defaulting to `true` which makes it impossible for users to disable streaming. Three informational items note code duplication across the three entrypoint functions, a potential robustness concern in the test parser, and dead-code potential for the `gateway_cancel_token`.

No security vulnerabilities or critical bugs were found.

## Warnings

### WR-01: Inconsistent error handling -- `.expect()` vs `.context()?` for RetryQueue in `run_gateway`

**File:** `crates/ironhermes-cli/src/main.rs:1040`
**Issue:** In `run_gateway`, the `RetryQueue::new()` call uses `.expect("Failed to initialize webhook retry queue")` which will panic on failure. Both `run_single` (line 357) and `run_chat` (line 617) use `.context("Failed to initialize webhook retry queue")?` which propagates the error cleanly via `Result`. A panic in `run_gateway` would crash the long-running Telegram bot ungracefully instead of allowing the caller to handle the error.
**Fix:**
```rust
let retry_queue = Arc::new(
    ironhermes_hooks::RetryQueue::new(
        ironhermes_hooks::RetryQueue::default_path()
    ).context("Failed to initialize webhook retry queue")?
);
```

### WR-02: `gateway_cancel_token` is created but never wired to a signal handler

**File:** `crates/ironhermes-cli/src/main.rs:1004`
**Issue:** A `CancellationToken` is created and passed to `register_delegate_task_tool`, but no signal handler (e.g., `tokio::signal::ctrl_c()`) or shutdown hook ever calls `.cancel()` on it. In contrast, `run_chat` properly wires ctrl-c to `chat_cancel_token.cancel()` (lines 767, 775). This means subagent tasks spawned via the gateway can never be gracefully cancelled via the token -- the token will remain live forever. If `GatewayRunner::start()` handles shutdown internally this may be intentional, but it is inconsistent with the chat path and worth verifying.
**Fix:** Either wire `gateway_cancel_token.cancel()` into the gateway's shutdown path, or if `GatewayRunner` manages its own cancellation, add a comment explaining why the token is a no-op placeholder:
```rust
// NOTE: GatewayRunner manages its own shutdown; this token is a
// structural placeholder required by register_delegate_task_tool's API.
let gateway_cancel_token = CancellationToken::new();
```

### WR-03: `--stream` flag defaults to `true`, making it impossible to disable

**File:** `crates/ironhermes-cli/src/main.rs:43`
**Issue:** The `stream` field uses `#[arg(short, long, default_value_t = true)]` which means `-s` and `--stream` are boolean flags that default to `true`. Clap boolean flags toggle presence, so passing `--stream` still results in `true`, and there is no `--no-stream` generated. The field is never read anywhere in the code (no reference to `cli.stream`), so it is both unreachable and unusable. If streaming control is intended, this needs a different approach.
**Fix:** Either remove the unused flag entirely, or switch to a negatable pattern:
```rust
/// Disable streaming output
#[arg(long = "no-stream")]
no_stream: bool,
```
Then use `!cli.no_stream` where streaming decisions are made.

## Info

### IN-01: Significant code duplication across `run_single`, `run_chat`, and `run_gateway`

**File:** `crates/ironhermes-cli/src/main.rs:261-1081`
**Issue:** All three entrypoint functions repeat nearly identical blocks for: memory manager construction, tool registration (cron, skills, execute_code, memory), RPC registry setup (6 identical tool registrations), guardrail wiring, hook registry construction, JSONL/webhook listener registration, and retry queue draining. This is approximately 60-80 lines duplicated three times. While the parity test (`cli_tool_parity.rs`) mitigates the risk of drift, the duplication increases maintenance burden and makes inconsistencies like WR-01 more likely.
**Fix:** Extract a shared setup function, e.g.:
```rust
struct ToolWiring {
    registry: Arc<ToolRegistry>,
    hook_registry: Arc<ironhermes_hooks::HookRegistry>,
    memory_manager: Arc<tokio::sync::Mutex<MemoryManager>>,
    // ...
}

async fn build_tool_wiring(config: &Config, ...) -> Result<ToolWiring> { ... }
```

### IN-02: Test brace-balance parser does not handle braces inside string literals or comments

**File:** `crates/ironhermes-cli/tests/cli_tool_parity.rs:28-51`
**Issue:** The `extract_function_body` function counts `{` and `}` bytes without distinguishing those inside string literals, comments, or raw strings. For example, a string like `"{"` or a comment containing `}` would cause incorrect brace depth tracking. Currently this works because the functions being parsed do not have such pathological content, but it is fragile against future changes (e.g., adding a format string with braces in a function body).
**Fix:** This is acceptable for a regression test that parses known source, but adding a comment documenting the limitation would improve maintainability:
```rust
/// Extract the body of a top-level `async fn NAME` block from main.rs.
/// Uses naive brace-balanced extraction -- does NOT handle braces inside
/// string literals or comments. Sufficient for structural grep tests.
fn extract_function_body(source: &str, fn_name: &str) -> String {
```

### IN-03: `.ok()` silently swallows `.env` load errors

**File:** `crates/ironhermes-cli/src/main.rs:119`
**Issue:** `dotenvy::from_path(&env_path).ok()` silently discards any error from loading the `.env` file (e.g., permission denied, malformed file). Since the file existence is already checked on line 118, the only errors would be parse errors or permission issues -- both of which the user would want to know about to debug configuration problems.
**Fix:** Log a warning instead of silently swallowing:
```rust
if let Err(e) = dotenvy::from_path(&env_path) {
    tracing::warn!("Failed to load .env file at {}: {}", env_path.display(), e);
}
```

---

_Reviewed: 2026-04-17T16:50:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
