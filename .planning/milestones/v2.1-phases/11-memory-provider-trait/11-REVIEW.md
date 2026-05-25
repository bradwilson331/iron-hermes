---
phase: 11-memory-provider-trait
reviewed: 2026-04-11T12:00:00Z
depth: standard
files_reviewed: 11
files_reviewed_list:
  - crates/ironhermes-agent/src/prompt_builder.rs
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-core/src/config.rs
  - crates/ironhermes-core/src/lib.rs
  - crates/ironhermes-core/src/memory_provider.rs
  - crates/ironhermes-core/src/memory_store.rs
  - crates/ironhermes-gateway/src/handler.rs
  - crates/ironhermes-gateway/src/runner.rs
  - crates/ironhermes-tools/src/delegate_task.rs
  - crates/ironhermes-tools/src/memory_tool.rs
  - crates/ironhermes-tools/src/registry.rs
findings:
  critical: 1
  warning: 3
  info: 2
  total: 6
status: issues_found
---

# Phase 11: Code Review Report

**Reviewed:** 2026-04-11T12:00:00Z
**Depth:** standard
**Files Reviewed:** 11
**Status:** issues_found

## Summary

Reviewed the memory provider trait extraction (MEM-07, MEM-08, MEM-12) and its integration across the codebase. The trait definition, MemoryStore impl, factory function, and config plumbing are well-structured. One critical bug exists in the read-only memory tool where the advertised "get" action is unreachable at runtime. Several warnings around panic-on-poisoned-mutex in production paths and a byte-vs-char mismatch in capacity enforcement.

## Critical Issues

### CR-01: Read-only MemoryTool advertises "get" action but execute() does not handle it

**File:** `crates/ironhermes-tools/src/memory_tool.rs:64`
**Issue:** The read-only schema (lines 56-75) declares `"enum": ["get"]` as the only valid action, but `execute()` (line 108) has no match arm for `"get"`. When a subagent calls `memory(action="get", target="memory")`, the read-only guard on line 122 passes (it only blocks `add|replace|remove`), and the match on line 128 falls through to the `other` arm (line 164), returning `Unknown action 'get'`. The tool is entirely unusable in read-only/subagent context.
**Fix:**
```rust
// Add this arm in the match block at line 128, before the "add" arm:
"get" => {
    let store = self.store.lock().unwrap();
    let prompt = store.format_for_system_prompt(target);
    Ok(prompt.unwrap_or_else(|| format!("No {} entries found.", target.label())))
}
```

## Warnings

### WR-01: char_count uses byte length, not character count

**File:** `crates/ironhermes-core/src/memory_store.rs:149-151`
**Issue:** The capacity check in `add()` uses `content.len()` and `ENTRY_DELIMITER.len()`, which return byte counts, not character counts. The function is named `char_count` and constants are named `MEMORY_CHAR_LIMIT` / `USER_CHAR_LIMIT`, implying character semantics. For multi-byte UTF-8 content (non-ASCII user input is common), the limit is reached earlier than expected. The test on line 658 even acknowledges the discrepancy: the section sign delimiter is 4 bytes but 3 chars. This means users writing in non-Latin scripts get roughly half the effective storage.
**Fix:** Either rename to `byte_count` / `MEMORY_BYTE_LIMIT` to make the contract explicit, or switch to `.chars().count()` for true character counting:
```rust
fn char_count(entries: &[String], delimiter: &str) -> usize {
    if entries.is_empty() {
        return 0;
    }
    let entry_chars: usize = entries.iter().map(|e| e.chars().count()).sum();
    let delimiter_chars = delimiter.chars().count() * (entries.len() - 1);
    entry_chars + delimiter_chars
}
```
And update the capacity check in `add()` to use `content.chars().count()` instead of `content.len()`.

### WR-02: Mutex lock().unwrap() in production tool execution path

**File:** `crates/ironhermes-tools/src/memory_tool.rs:135`
**Issue:** `self.store.lock().unwrap()` on lines 135, 148, and 159 will panic if the mutex is poisoned (i.e., a previous holder panicked while holding the lock). In the gateway, a panic in a tokio task takes down only that task, but the memory store becomes permanently unusable for all subsequent requests since every lock attempt will panic. The same pattern appears in `prompt_builder.rs:185`.
**Fix:** Convert to a recoverable error:
```rust
let mut store = self.store.lock()
    .map_err(|e| anyhow::anyhow!("Memory store lock poisoned: {}", e))?;
```

### WR-03: with_file_lock panics on file system errors instead of returning Result

**File:** `crates/ironhermes-core/src/memory_store.rs:390-403`
**Issue:** `with_file_lock` uses `.expect()` for opening the lock file (line 395), acquiring the lock (line 399), and releasing the lock (line 403). If the filesystem is full, permissions change, or NFS has issues, this panics the entire process. Since this is called from `add()`, `replace()`, and `remove()` which already return `MemoryResult`, the error could be propagated instead.
**Fix:** Change `with_file_lock` signature to return `Result<R, String>` and propagate errors:
```rust
fn with_file_lock<F, R>(path: &Path, f: F) -> std::result::Result<R, String>
where
    F: FnOnce() -> R,
{
    let lock_path = path.with_extension("md.lock");
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| format!("{{\"error\": \"Failed to open lock file: {}\"}}", e))?;

    lock_file.lock_exclusive()
        .map_err(|e| format!("{{\"error\": \"Failed to acquire lock: {}\"}}", e))?;

    let result = f();

    let _ = lock_file.unlock(); // best-effort unlock
    Ok(result)
}
```

## Info

### IN-01: MemoryProvider trait mixes async and sync methods

**File:** `crates/ironhermes-core/src/memory_provider.rs:42-62`
**Issue:** The trait has async lifecycle hooks (`initialize`, `prefetch`, `sync_turn`, `on_session_end`, `shutdown`) and sync operational methods (`load_from_disk`, `add`, `replace`, `remove`, `format_for_system_prompt`, `to_memory_entries`). Current callers only use the sync methods; the async hooks are all no-ops in the MemoryStore impl. This is likely intentional for future backends (sqlite, grafeo, duckdb) but worth noting that the async methods are currently dead code paths.
**Fix:** No action needed now. Consider adding integration tests for the async lifecycle hooks when the first non-file backend is implemented.

### IN-02: Unused import MemoryProvider in main.rs

**File:** `crates/ironhermes-cli/src/main.rs:5`
**Issue:** `MemoryProvider` is imported on line 5 but only used in the `run_gateway` function to annotate the `Arc<Mutex<dyn MemoryProvider + Send>>` type. The import itself is valid, but `MemoryStore` (also imported) is also used directly. The `build_memory_provider` import on the same line is used only in `run_gateway` for validation (line 467) but the result is immediately discarded with `let _ =`. Consider whether this validation-then-discard pattern is intentional or if the returned provider should be used instead of constructing a new `MemoryStore` on line 471.
**Fix:** If the intent is to validate the config early, the pattern is fine. If the intent is to use the configured provider, replace lines 470-471 with:
```rust
let mut provider = build_memory_provider(&config.memory)?;
provider.load_from_disk().map_err(|e| { warn!("..."); e }).ok();
let memory_store: Arc<Mutex<dyn MemoryProvider + Send>> = Arc::new(Mutex::new(/* use provider */));
```
Note: This requires `build_memory_provider` to return an owned type that can be wrapped. Current signature already returns `Box<dyn MemoryProvider + Send>` which would need conversion. This is a design consideration for when non-file backends are added.

---

_Reviewed: 2026-04-11T12:00:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
