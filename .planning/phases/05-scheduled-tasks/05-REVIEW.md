---
phase: "05"
status: needs_attention
findings_count: 11
severity_counts:
  critical: 1
  high: 3
  medium: 4
  low: 3
---

# Phase 05: Code Review — Scheduled Tasks

**Reviewed:** 2026-04-08
**Depth:** standard
**Files Reviewed:** 17

---

## Critical

### CR-01: Path Traversal via `job_id` in `save_job_output`

**File:** `crates/ironhermes-cron/src/delivery.rs:75-103`

The `job_id` field of a `CronJob` is written directly into a filesystem path with no sanitisation:

```rust
let output_dir = home.join("cron").join("output").join(job_id);
```

A `job_id` containing `../` sequences (e.g. `../../.ssh`) would let output files escape the intended `cron/output/` tree. `job_id` is currently always a UUID generated via `Uuid::new_v4()`, so the current code path is safe. However:

- The `LegacyCronJob` migration path (store.rs:34-36) calls `parse_schedule` and falls back to the raw legacy `id` field, which is read straight from the JSON file on disk.
- The `CronjobTool::handle_create` and `JobStore::add_job` accept any `name`/`id` strings produced externally and stored in the JSON.
- If a corrupted or hand-edited `jobs.json` contains a crafted `id`, the path traversal opens at next tick.

**Fix:** Sanitise `job_id` before using it in a path. At minimum, assert/strip any path separators:

```rust
pub fn save_job_output(job_id: &str, output: &str) -> Result<PathBuf> {
    // Reject any job_id that looks like a path traversal attempt
    if job_id.contains('/') || job_id.contains('\\') || job_id.contains("..") {
        anyhow::bail!("invalid job_id for filesystem use: {:?}", job_id);
    }
    // ... rest unchanged
}
```

Or use `Path::new(job_id).file_name()` to take only the last component.

---

## High

### HR-01: Mutex Poisoning Panics Across All Lock Sites

**Files:**
- `crates/ironhermes-cron/src/tick.rs:50`
- `crates/ironhermes-cron/src/tick.rs:97`
- `crates/ironhermes-tools/src/cronjob_tool.rs:354`
- `crates/ironhermes-cli/src/cron.rs:491`

All `store.lock().unwrap()` calls will panic if the mutex is poisoned (i.e. a previous holder panicked while holding the lock). In an async context where multiple Tokio tasks share the store, this is a real risk — if the gateway's cron tick task panics, the gateway's message-handling tasks will subsequently panic on their next lock attempt, taking down the entire process.

```rust
// tick.rs:50 — panics on poison
let mut store_guard = store.lock().unwrap();
// cronjob_tool.rs:354 — panics on poison
let mut store = self.store.lock().unwrap();
```

**Fix:** Replace `.unwrap()` with `.map_err(|e| anyhow::anyhow!("store lock poisoned: {}", e))?` at every lock site, so a poison propagates as a recoverable error rather than a process-killing panic:

```rust
let mut store_guard = store.lock()
    .map_err(|e| anyhow::anyhow!("store lock poisoned: {}", e))?;
```

---

### HR-02: `cmd_run` Does Not Actually Execute the Job

**File:** `crates/ironhermes-cli/src/cron.rs:354-373`

The `ironhermes cron run <job_id>` command prints a message but does not trigger any execution path. It returns `Ok(())` silently, misleading the user into thinking the job ran:

```rust
fn cmd_run(job_id: String) -> Result<()> {
    let store = open_store()?;
    let job = store.find_job(&job_id)
        .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;
    let name = job.name.clone();

    println!("{}", format!("Running job: {}", name).dimmed());
    println!(
        "{}",
        format!(
            "Job triggered: {} (use gateway for full agent execution)",
            name
        )
        .yellow()
    );

    Ok(())   // <-- no execution, no error
}
```

Similarly, `CronjobTool::handle_run` (cronjob_tool.rs:258-270) only verifies the job exists and returns `{"status": "triggered"}` without doing anything. If an agent or operator uses this tool to explicitly trigger a job, nothing will happen and the `last_run_at`/`last_status` fields will not update — the user has no way to know execution did not occur.

**Fix:** Either wire `cmd_run` and `handle_run` to call `complete_job_run` (or enqueue the job for the tick runner), or change the status to `"queued"` / `"pending"` and surface a clear note that agent execution is deferred. At minimum, document in the tool schema that `run` only acknowledges the request; do not use `"triggered"` which implies the job started.

---

### HR-03: Stale Lock File Left on Process Crash Causes Permanent Tick Deadlock

**File:** `crates/ironhermes-cron/src/lib.rs:31-80`

The `LockGuard` removes the lock file in its `Drop` implementation, which works for clean shutdown. However, if the process is killed with SIGKILL, crashes with OOM, or panics while holding the guard, the lock file is not cleaned up. On next startup, `acquire_tick_lock` will find the stale file and permanently refuse to run any tick:

```rust
Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
    debug!("Tick lock already held: {}", lock_path.display());
    Ok(None)   // <-- silently skips every tick forever
}
```

The file contains the PID (`write!(f, "{}", std::process::id())`), but there is no code anywhere that reads it and checks whether that PID is still alive.

**Fix:** At acquisition time, read the existing lock file's PID and check if that process is still running. If it is not, remove the stale lock and re-acquire:

```rust
// On AlreadyExists, read PID and check liveness
if let Ok(pid_str) = fs::read_to_string(&lock_path) {
    if let Ok(pid) = pid_str.trim().parse::<u32>() {
        // On Unix: send signal 0 to check liveness
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        if !alive {
            let _ = fs::remove_file(&lock_path);
            // retry acquisition ...
        }
    }
}
```

Alternatively, add a `--force-unlock` CLI subcommand and surface a clear warning when the lock file is detected at startup.

---

## Medium

### MD-01: `format_delivery_message` Can Panic on Multi-Byte UTF-8 Boundaries

**File:** `crates/ironhermes-cron/src/delivery.rs:117-126`

The truncation at `MAX_PLATFORM_OUTPUT` (4000) uses a raw byte index on a `&str`, which panics if the cut falls inside a multi-byte UTF-8 character:

```rust
if output.len() > MAX_PLATFORM_OUTPUT {
    let truncated = &output[..MAX_PLATFORM_OUTPUT];  // byte slice — can panic on UTF-8 boundary
```

Any job output containing non-ASCII text (e.g. CJK characters, emoji, accented Latin) that spans the 4000-byte boundary will crash the gateway tick task.

**Fix:** Use `char_indices` to find a safe truncation point, or use `output.floor_char_boundary(MAX_PLATFORM_OUTPUT)` (stable since Rust 1.77), or simply:

```rust
let truncated = output
    .char_indices()
    .take_while(|(i, _)| *i < MAX_PLATFORM_OUTPUT)
    .last()
    .map(|(i, c)| &output[..i + c.len_utf8()])
    .unwrap_or("");
```

---

### MD-02: Integer Overflow in `parse_duration` for Large Day Values

**File:** `crates/ironhermes-cron/src/parser.rs:29`

```rust
"d" | "day" | "days" => amount * 1440,
```

`amount` is a `u32`. For any value >= 2,979,311 days (~8158 years), `amount * 1440` overflows a `u32` in debug builds (panic) and wraps silently in release builds, producing a nonsensically small interval that would schedule the job to fire immediately and repeatedly.

The regex `(\d+)` accepts an unbounded number of digits, and the `u32::parse()` will cap at `4294967295`, so `2970000 * 1440 = 4,276,800,000` which overflows `u32::MAX` (4,294,967,295) only for values > ~2,979,310 days, but the wrapping result would be a very small number treated as minutes.

**Fix:** Use `checked_mul` and return an error on overflow:

```rust
let minutes = match unit.as_str() {
    "d" | "day" | "days" => amount.checked_mul(1440)
        .ok_or_else(|| anyhow!("duration overflow: {} days", amount))?,
    "h" | "hr" | "hrs" | "hour" | "hours" => amount.checked_mul(60)
        .ok_or_else(|| anyhow!("duration overflow: {} hours", amount))?,
    // ...
};
```

---

### MD-03: `handle_update` in `CronjobTool` Accepts `job_id` by Name But `update_job` Only Matches by ID

**File:** `crates/ironhermes-tools/src/cronjob_tool.rs:161-221`

`handle_update` uses `find_job(&job_id)` to verify existence (which checks both ID and name), but then passes the original `job_id` string directly to `store.update_job(&job_id, ...)`, which only matches by `j.id == id`. If the agent or user supplies a job name instead of a UUID, existence verification passes but the subsequent update silently fails with "job not found":

```rust
// This lookup succeeds for a name
if store.find_job(&job_id).is_none() { ... }

// This update fails for a name — update_job only matches j.id == id
match store.update_job(&job_id, updates) { ... }
```

The same mismatch exists in `handle_pause` and `handle_resume` (both call `toggle_job` which also matches by ID only).

**Fix:** Resolve the canonical ID from `find_job` before calling `update_job`/`toggle_job`:

```rust
let canonical_id = match store.find_job(&job_id) {
    Some(j) => j.id.clone(),
    None => return json!({"status": "error", "message": format!("Job not found: {}", job_id)}),
};
// use canonical_id for all subsequent store operations
match store.update_job(&canonical_id, updates) { ... }
```

---

### MD-04: `CronJob` Name and Prompt Have No Length or Content Validation

**Files:**
- `crates/ironhermes-cron/src/store.rs:154-201` (`add_job`)
- `crates/ironhermes-tools/src/cronjob_tool.rs:80-139` (`handle_create`)

Job names and prompts are accepted without any length limits. A very large prompt (e.g. megabytes of text) will be stored in `jobs.json`, persisted on every save, and loaded into memory on every store open. Because `jobs.json` is loaded as a single JSON blob, a single oversized entry makes every subsequent store operation proportionally slower and can exhaust memory in low-resource environments.

Additionally, job names with no validation can contain characters that break the table display in `cmd_list` (e.g. ANSI escape sequences injected via the Telegram gateway's `handle_create` path).

**Fix:** Add bounded validation in `add_job`:

```rust
const MAX_NAME_LEN: usize = 128;
const MAX_PROMPT_LEN: usize = 8_192;

if name.len() > MAX_NAME_LEN {
    anyhow::bail!("job name too long (max {} bytes)", MAX_NAME_LEN);
}
if prompt.len() > MAX_PROMPT_LEN {
    anyhow::bail!("prompt too long (max {} bytes)", MAX_PROMPT_LEN);
}
```

---

## Low

### LW-01: `resolve_token` Accepts Non-Empty Whitespace-Only Strings as Valid Tokens

**File:** `crates/ironhermes-gateway/src/runner.rs:423-435`

```rust
if !t.is_empty() {
    return Some(t.clone());
}
```

A config value of `"   "` (spaces only) will be returned as a token and passed to Telegram, where it will fail API authentication. The error will surface as a confusing "Failed to authenticate with Telegram" rather than the obvious "token is blank". This is a usability issue that can waste time debugging.

**Fix:**

```rust
if !t.trim().is_empty() {
    return Some(t.clone());
}
```

---

### LW-02: `save_job_output` Uses Second-Granularity Timestamp — Collision Risk

**File:** `crates/ironhermes-cron/src/delivery.rs:82-84`

```rust
let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
let file_path = output_dir.join(format!("{}.md", timestamp));
let tmp_path = output_dir.join(format!("{}.md.tmp", timestamp));
```

If two jobs with the same `job_id` complete within the same second, the second write will `create` the same `tmp_path`, overwrite the first temp file, and then rename it over the first output file. One output will be silently lost. While uncommon in the current architecture (only one tick per minute), it is possible if `cmd_tick` is run manually while the gateway tick is also running.

**Fix:** Include sub-second precision or a random suffix:

```rust
let timestamp = format!("{}-{:06}", Utc::now().format("%Y%m%d_%H%M%S"), Utc::now().timestamp_subsec_micros());
```

Or append a short UUID to guarantee uniqueness.

---

### LW-03: `scan_cron_prompt` Does Not Catch Multi-Line Prompt Injection via Newline Splitting

**File:** `crates/ironhermes-cron/src/scanner.rs:11-26`

The regex for `rm -rf /` is:

```rust
r"(?i)rm\s+-rf\s+/",
```

This matches the literal string. An attacker who knows the pattern can trivially bypass it by splitting the dangerous token across tool call boundaries or using shell metacharacters: `rm\t-rf\t/`, `rm  -rf /` (multiple spaces), or `rm -r -f /`. The `\s+` covers whitespace, but `rm -f -r /` or `rm --recursive --force /` would not match.

More broadly, the scanner is a best-effort heuristic and is documented as such. The concern is that it is the only security control applied before storing and eventually executing prompts, and its coverage is necessarily incomplete.

**Fix:** Document explicitly in comments (and in user-facing docs) that `scan_cron_prompt` is a first-layer heuristic, not a complete sandbox. Consider adding `rm\s+(-\w+\s+)*-[rf]{1,2}\b` and variant patterns. Most importantly, ensure the agent execution layer (AgentLoop) itself runs with restricted filesystem tool access for cron jobs, as defence-in-depth.

---

## Summary

The most significant findings are:

1. **CR-01** (Critical): Path traversal via `job_id` in output file paths — needs sanitisation before any `job_id` from the store is used in a path.
2. **HR-01** (High): Mutex poison panics that can bring down the gateway process — replace all `.unwrap()` on lock results with `?`-propagated errors.
3. **HR-02** (High): `run` action (both CLI and tool) is silently a no-op — misleading to users and agents.
4. **HR-03** (High): Stale tick lock file after crash causes permanent scheduler silence with no operator alerting.
5. **MD-01** (Medium): UTF-8 byte-slice panic in `format_delivery_message` on non-ASCII output at the truncation boundary.

The overall architecture (atomic file writes, RAII lock guard, per-tick lock, security scanner, schema-validated tool) is well-structured. The above issues are fixable without redesign.

---

_Reviewed: 2026-04-08_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
