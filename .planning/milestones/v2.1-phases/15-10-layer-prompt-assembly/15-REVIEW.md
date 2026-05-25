---
phase: 15-10-layer-prompt-assembly
reviewed: 2026-04-12T19:42:00Z
depth: standard
files_reviewed: 8
files_reviewed_list:
  - crates/ironhermes-agent/src/context_loader.rs
  - crates/ironhermes-agent/src/lib.rs
  - crates/ironhermes-agent/src/personality.rs
  - crates/ironhermes-agent/src/prompt_builder.rs
  - crates/ironhermes-agent/src/subdir_discovery.rs
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-core/src/config.rs
  - crates/ironhermes-gateway/src/handler.rs
findings:
  critical: 0
  warning: 5
  info: 3
  total: 8
status: issues_found
---

# Phase 15: Code Review Report

**Reviewed:** 2026-04-12T19:42:00Z
**Depth:** standard
**Files Reviewed:** 8
**Status:** issues_found

## Summary

Reviewed 8 files comprising the 10-layer prompt assembly system (Phase 15), including the PromptBuilder, PersonalityRegistry, SubdirDiscovery, context_loader, CLI main entry point, core config types, and the gateway message handler. The code is generally well-structured with good test coverage, consistent security scanning of context files, and clear slot-based prompt assembly. No critical vulnerabilities found. Five warnings relate to a logic bug in git root discovery, CRLF handling fragility, redundant method calls, a leaked cancellation token, and API key storage in config. Three informational items noted.

## Warnings

### WR-01: find_git_root comment contradicts behavior at HOME boundary

**File:** `crates/ironhermes-agent/src/context_loader.rs:17-24`
**Issue:** The comment on line 21 says "do not check $HOME itself for .git, just stop" but the code checks for `.git` on line 17 *before* comparing the current directory to `$HOME` on lines 22-24. If `$HOME` itself contains a `.git` directory, `find_git_root` will return `Some($HOME)` instead of `None`. If the design decision (D-01, D-03) truly intends to exclude `$HOME`, this is a logic bug. If `$HOME` should be included, the comment is misleading.
**Fix:** Move the HOME check before the `.git` check, or correct the comment:
```rust
loop {
    // Stop at $HOME before checking for .git
    if let Some(ref h) = home {
        if &current == h {
            return None;
        }
    }

    if current.join(".git").exists() {
        return Some(current.clone());
    }
    // ...
}
```

### WR-02: strip_yaml_frontmatter assumes LF line endings, breaks on CRLF

**File:** `crates/ironhermes-agent/src/context_loader.rs:54-72`
**Issue:** The byte offset tracking uses `line.len() + 1` to account for the newline delimiter, assuming `\n` (1 byte). The `lines()` iterator strips both `\n` and `\r\n`, but `line.len()` does not include the `\r`. On files with `\r\n` endings (common on Windows or in mixed-platform repos), the computed `offset` will drift by 1 per line, causing `rest_start` on line 59 to point to the wrong position. This can result in the returned body including a stray `\n` prefix or, in edge cases, a panic if the offset exceeds `after_open.len()`.
**Fix:** Account for the actual line ending width, or normalize line endings before processing:
```rust
pub fn strip_yaml_frontmatter(content: &str) -> &str {
    // Normalize CRLF to LF before processing, or use a searcher:
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    // Use find("---\n") or find("---\r\n") for the closing marker
    // instead of manual offset tracking
}
```

### WR-03: load_memory() and load_skills() called twice per session

**File:** `crates/ironhermes-agent/src/prompt_builder.rs:177-178` and `crates/ironhermes-cli/src/main.rs:268-269`
**Issue:** `load_context()` internally calls `self.load_memory()` and `self.load_skills()` (lines 177-178 of prompt_builder.rs). The CLI then calls them again explicitly (main.rs lines 268-269 for `run_single`, lines 394-395 for `run_chat`). The gateway handler in handler.rs lines 295-296 does the same. Since `set_slot` uses `BTreeMap::insert`, the second call silently overwrites the first with identical content. This is not a correctness bug but performs redundant I/O (memory store lock acquisition, skill catalog generation) on every session start.
**Fix:** Remove the explicit `load_memory()` / `load_skills()` calls after `load_context()`, since `load_context()` already calls them:
```rust
// In main.rs run_single() and run_chat():
let mut prompt_builder = PromptBuilder::new(client.model(), "cli")
    .with_provider(&config.model.provider)
    .load_context(&cwd);
// These two lines are redundant — load_context() already calls them:
// prompt_builder.load_memory();
// prompt_builder.load_skills();
```

### WR-04: chat_cancel_token created but never cancelled in run_chat

**File:** `crates/ironhermes-cli/src/main.rs:344`
**Issue:** A `CancellationToken` is created at line 344 and passed to `register_delegate_task_tool` at line 381, but `chat_cancel_token.cancel()` is never called anywhere in the `run_chat` function. When the user types `/quit` or hits EOF, the function returns without cancelling the token. Any in-flight subagent tasks referencing this token will not receive a cancellation signal. The token is eventually dropped (which does not trigger cancellation in `tokio_util::sync::CancellationToken`).
**Fix:** Cancel the token before returning from `run_chat`:
```rust
// Before the final Ok(()) in run_chat:
chat_cancel_token.cancel();

state_store.end_session(&session_id, "completed")
    .context("failed to end CLI session")?;
Ok(())
```

### WR-05: API key stored in plaintext in SubagentConfig

**File:** `crates/ironhermes-core/src/config.rs:365`
**Issue:** `SubagentConfig.api_key: Option<String>` allows API keys to be written directly into `config.yaml`. Since config files are commonly committed to version control or readable by other processes, this creates a risk of credential exposure. The main `ModelConfig.api_key` field (line 113) has the same concern but is more established; the subagent variant is newer and worth flagging.
**Fix:** Document that this field should reference an environment variable name rather than a literal key, or support env var expansion (e.g., `api_key: "${SUBAGENT_API_KEY}"`). At minimum, add a comment warning against committing literal keys:
```rust
/// Optional custom API key for subagents (D-23). None = use parent's.
/// WARNING: Prefer environment variables over literal keys in config files.
pub api_key: Option<String>,
```

## Info

### IN-01: build_timestamp_block leaks full overlay text

**File:** `crates/ironhermes-agent/src/prompt_builder.rs:492-494`
**Issue:** When an overlay is active, `build_timestamp_block()` emits `"Active personality: {overlay_text}"` where `overlay_text` is the full personality instruction string (could be multiple sentences). This duplicates the overlay content since it already appears in slot 8 (SessionOverlay). The timestamp block likely intends to show a personality *name*, not the full text.
**Fix:** Store the personality name separately from the overlay text, or truncate the preview:
```rust
if let Some(ref overlay) = self.active_overlay {
    // Show first 50 chars as a label, not the full instruction text
    let label: String = overlay.chars().take(50).collect();
    parts.push(format!("Active personality: {}...", label));
}
```

### IN-02: Silently swallowed .env parse errors

**File:** `crates/ironhermes-cli/src/main.rs:98`
**Issue:** `dotenvy::from_path(&env_path).ok()` discards any parse error from the `.env` file. If the file exists but has syntax errors (e.g., unquoted values with spaces, missing `=`), the user gets no feedback and environment variables silently fail to load, leading to confusing "API key not set" errors downstream.
**Fix:**
```rust
if env_path.exists() {
    if let Err(e) = dotenvy::from_path(&env_path) {
        warn!("Failed to parse {}: {}", env_path.display(), e);
    }
}
```

### IN-03: Unused chat_cancel_token in run_chat scope

**File:** `crates/ironhermes-cli/src/main.rs:344`
**Issue:** Related to WR-04 -- the `chat_cancel_token` variable is created and registered but no code in `run_chat` ever reads or cancels it outside the registration call. The `#[allow(unused)]` suppression is not present, suggesting the compiler may already warn about this (or the registration counts as a "use"). This is a code smell indicating incomplete cancellation wiring.
**Fix:** See WR-04 fix.

---

_Reviewed: 2026-04-12T19:42:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
