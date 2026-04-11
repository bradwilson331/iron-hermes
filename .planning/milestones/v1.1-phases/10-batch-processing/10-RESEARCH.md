# Phase 10: Batch Processing - Research

**Researched:** 2026-04-10
**Domain:** Rust async batch execution, ShareGPT JSONL format, checkpointing, quality filtering
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** Subcommand group pattern — `ironhermes batch run|status|cancel|list`. Follows cron subcommand precedent. `run` takes positional input path, `-o` for output, `--workers` for concurrency, `--model` for override.
- **D-02:** `batch status` shows progress of the current/last batch run (entries completed/total, workers active, elapsed time, ETA).
- **D-03:** `batch cancel` sends graceful shutdown signal — in-flight workers finish their current entry, no new entries start. Checkpoint is saved.
- **D-04:** `batch list` shows past batch runs with summary (input file, entries, pass/reject counts, duration).
- **D-05:** Always checkpoint — every completed entry is persisted immediately. No `--resume` flag needed. Content hash (SHA-256 of input prompt) identifies completed entries. Rerunning the same command automatically skips completed entries.
- **D-06:** Checkpoint file stored alongside output — `{output_path}.checkpoint.json` containing hash→status mapping. Deleted when batch completes successfully with all entries.
- **D-07:** Tool calls as separate conversation turns — `tool_call` and `tool_response` as distinct `from` values. Maps directly from AgentLoop's ChatMessage sequence.
- **D-08:** Conversations use `human`/`gpt`/`tool_call`/`tool_response` role names. Standard ShareGPT `from` field convention.
- **D-09:** Each trajectory line includes metadata: `id` (content hash), `model`, `timestamp`, `usage` (prompt/completion tokens), `turns` count, `quality` object, plus `conversations` array.
- **D-10:** Output is JSONL — one trajectory per line. Channel-based serialization to avoid concurrent write corruption.
- **D-11:** Separate reject file — filtered trajectories written to `{output_path%.jsonl}_rejected.jsonl` with `rejection_reason` field. Nothing silently discarded.
- **D-12:** Four built-in rejection criteria (all enabled by default):
  1. Hallucinated tool names — agent called a tool not in the registry
  2. No reasoning steps — final response with zero tool calls and no chain-of-thought
  3. Error-only trajectories — every tool call errored, no successful actions
  4. Secrets in output — API keys, tokens, or credentials detected in tool results (reuses existing security scanning from SELF-03)
- **D-13:** Quality filter results included in trajectory metadata as `quality: { passed: bool, reasons: [string] }` for both passed and rejected entries.

### Claude's Discretion

- JSONL input format — minimum is `{"prompt": "..."}` per line, but additional fields (system prompt override, tool allowlist) are implementation details
- Worker concurrency default and config key name
- Checkpoint file format internals (hash algorithm, storage structure)
- Batch run state persistence (SQLite vs file-based)
- Progress reporting format for `batch status`
- How `batch cancel` signal is delivered (file flag, Unix signal, etc.)
- Whether to create a new `ironhermes-batch` crate or extend existing crates
- How secrets scanning integrates with the existing SSRF/security module

### Deferred Ideas (OUT OF SCOPE)

None — discussion stayed within phase scope.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| BATCH-01 | User can run batch prompt execution from JSONL input with semaphore-bounded parallel workers | tokio::task::JoinSet + Semaphore pattern (verified in runner.rs); clap derive subcommand group (cron precedent) |
| BATCH-02 | Batch output is in ShareGPT format (human/assistant/tool roles) for HuggingFace compatibility | ShareGPT `conversations` array with `from`/`value` pairs — maps directly from `AgentResult.messages`; JSONL serialization via serde_json + writeln (log_writer.rs pattern) |
| BATCH-03 | Batch jobs support checkpointing — survive restarts by tracking completed entries by content hash | SHA-256 via `sha2` crate (already in workspace deps); JSON file alongside output (cron JobStore precedent); skip-on-hash-match pattern |
| BATCH-04 | Automatic quality filtering discards trajectories with hallucinated tool names or missing reasoning | ToolRegistry::list_tools() for hallucination check; context_scanner.rs/THREAT_PATTERNS for secrets; pure filter function pipeline |
</phase_requirements>

## Summary

Phase 10 builds the `ironhermes batch` subcommand group: a parallel batch executor that reads JSONL prompts, runs each through an `AgentLoop`, serialises trajectories as ShareGPT-format JSONL, supports automatic checkpointing/resume, and applies quality filtering with reject separation. All of the hard problems (concurrency, AgentLoop execution, JSONL writes, secret scanning, SHA-256 hashing) are solved by existing infrastructure — this phase is predominantly assembly work.

The concurrency model is proven: `tokio::task::JoinSet` with an `Arc<Semaphore>` is already used in `runner.rs` (gateway) and `delegate_task.rs` (Phase 9). A single MPSC channel serialises writes to the output file so concurrent workers never corrupt JSONL. The `AgentResult` struct from Phase 9 maps directly to the ShareGPT conversation schema — no new API surfaces are required from the agent layer.

The only genuinely new work is: (1) a `BatchConfig` struct added to `ironhermes-core/src/config.rs` following the `SubagentConfig` pattern, (2) a secrets pattern scanner for tool output (different from the context-file injection scanner, this one detects credential strings in content), and (3) the batch CLI module in `ironhermes-cli` mirroring the `crates/ironhermes-cli/src/cron.rs` structure.

**Primary recommendation:** Extend `ironhermes-cli` with a `batch.rs` module (no new crate needed). Use `JoinSet` + `Semaphore` + `mpsc` channel writer. Map `ChatMessage` variants to ShareGPT roles inline. Reuse `sha2` from workspace deps for content hashing.

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `tokio` | 1 (workspace) | Async runtime, `JoinSet`, `Semaphore`, `mpsc` | Already the project runtime [VERIFIED: Cargo.toml] |
| `serde_json` | 1 (workspace) | JSONL serialisation/deserialisation | Already used everywhere in codebase [VERIFIED: Cargo.toml] |
| `sha2` | workspace | SHA-256 content hashing for checkpoint keys | Already in workspace deps (ironhermes-hooks uses it) [VERIFIED: Cargo.toml grep] |
| `clap` | 4 derive (workspace) | CLI subcommand group (`batch run|status|cancel|list`) | Project-standard CLI library [VERIFIED: Cargo.toml] |
| `anyhow` | 1 (workspace) | Error propagation | Project-standard [VERIFIED: Cargo.toml] |
| `chrono` | 0.4 (workspace) | Timestamps in trajectory metadata | Already in workspace [VERIFIED: Cargo.toml] |
| `serde` + derive | 1 (workspace) | Struct serialisation for checkpoint JSON and trajectory JSONL | Project-standard [VERIFIED: Cargo.toml] |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `uuid` | 1 v4 (workspace) | Batch run IDs for `batch list` history | Already in workspace [VERIFIED: Cargo.toml] |
| `regex` | 1 (workspace) | Secrets pattern matching in tool output (BATCH-04 criterion 4) | Already in workspace, used by context_scanner.rs [VERIFIED: Cargo.toml] |
| `tracing` | 0.1 (workspace) | Progress/status logging per worker | Project-standard [VERIFIED: Cargo.toml] |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `JoinSet` + `Semaphore` | `rayon` thread pool | Rayon is CPU-bound; agent runs are I/O-bound (HTTP). Tokio is correct. |
| File-based checkpoint JSON | SQLite (`rusqlite`) | SQLite adds complexity; flat JSON file matches cron JobStore precedent and is inspectable. |
| New `ironhermes-batch` crate | Extend `ironhermes-cli` | New crate has no consumers outside CLI; adds build graph complexity. Extend CLI with `batch.rs` module. |

**Installation:** No new dependencies needed — all required crates are in workspace deps. Add `sha2` to `ironhermes-cli/Cargo.toml` if not already a direct dep (currently only `ironhermes-hooks` declares it; the CLI will need to declare it directly).

## Architecture Patterns

### Recommended Project Structure

```
crates/ironhermes-cli/src/
├── main.rs              # Add Batch(BatchCommands) variant to Commands enum
├── batch.rs             # NEW: BatchCommands enum + all batch subcommand handlers
├── cron.rs              # Existing precedent for subcommand module pattern
└── ...

crates/ironhermes-core/src/
├── config.rs            # Add BatchConfig struct (workers, max_turns, output_dir)
└── ...
```

No new crate. The `batch.rs` module in `ironhermes-cli` follows the identical structure as `cron.rs`.

### Pattern 1: Bounded Parallel Worker Pool

**What:** JoinSet spawns one task per JSONL entry; Semaphore limits active workers; completed trajectories sent to a channel writer task.

**When to use:** All batch execution in this phase.

```rust
// Source: adapted from crates/ironhermes-gateway/src/runner.rs (JoinSet usage)
//         and crates/ironhermes-tools/src/delegate_task.rs (Semaphore usage)
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use std::sync::Arc;

let semaphore = Arc::new(Semaphore::new(config.workers));
let (tx, mut rx) = mpsc::channel::<TrajectoryLine>(256);

// Single writer task — serialises all JSONL writes
let writer_handle = tokio::spawn(async move {
    let mut output = tokio::fs::OpenOptions::new()
        .create(true).append(true)
        .open(&output_path).await?;
    while let Some(traj) = rx.recv().await {
        let line = serde_json::to_string(&traj)?;
        output.write_all(line.as_bytes()).await?;
        output.write_all(b"\n").await?;
    }
    Ok::<_, anyhow::Error>(())
});

let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();

for entry in entries {
    let permit = semaphore.clone().acquire_owned().await?;
    let tx = tx.clone();
    let client = client.clone();
    let registry = registry.clone();

    join_set.spawn(async move {
        let _permit = permit; // released when task completes
        let result = run_batch_entry(client, registry, &entry).await?;
        tx.send(result).await?;
        Ok(())
    });
}

drop(tx); // signal writer that no more items are coming
join_set.join_all().await;
writer_handle.await??;
```

### Pattern 2: Content Hash Checkpointing

**What:** SHA-256 of the input prompt string; checkpoint file is a JSON map of hash → status. Load on startup, skip entries whose hash is present.

**When to use:** Every completed entry before writing trajectory.

```rust
// Source: sha2 crate usage from crates/ironhermes-hooks/src/webhook.rs
use sha2::{Sha256, Digest};

fn prompt_hash(prompt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    format!("{:x}", hasher.finalize())
}

// Checkpoint file: HashMap<String, CheckpointEntry> serialised as JSON
#[derive(Serialize, Deserialize)]
struct CheckpointEntry {
    status: String,   // "completed" | "rejected"
    timestamp: String,
}
```

### Pattern 3: ChatMessage → ShareGPT Conversion

**What:** Walk `AgentResult.messages`, map each `ChatMessage` to a ShareGPT turn by role.

**When to use:** After every successful `AgentLoop::run()`.

```rust
// Source: AgentResult from crates/ironhermes-agent/src/agent_loop.rs
// ShareGPT format: {"from": "human"|"gpt"|"tool_call"|"tool_response", "value": "..."}
fn messages_to_sharegpt(messages: &[ChatMessage]) -> Vec<ShareGptTurn> {
    let mut turns = Vec::new();
    for msg in messages {
        match msg.role {
            Role::User => turns.push(ShareGptTurn { from: "human", value: msg.content_text().unwrap_or("") }),
            Role::Assistant => {
                if let Some(text) = msg.content_text() {
                    if !text.is_empty() {
                        turns.push(ShareGptTurn { from: "gpt", value: text });
                    }
                }
                if let Some(tool_calls) = &msg.tool_calls {
                    for tc in tool_calls {
                        turns.push(ShareGptTurn {
                            from: "tool_call",
                            value: serde_json::to_string(tc).unwrap_or_default(),
                        });
                    }
                }
            }
            Role::Tool => turns.push(ShareGptTurn { from: "tool_response", value: msg.content_text().unwrap_or("") }),
            Role::System => {} // system prompt excluded from trajectory turns
        }
    }
    turns
}
```

### Pattern 4: Quality Filter Pipeline

**What:** Pure functions over `AgentResult` + `ToolRegistry`. Run in sequence; collect all reasons; route to pass or reject file.

**When to use:** After every `AgentLoop::run()`, before writing trajectory.

```rust
// Rejection criteria types — all pure functions
fn filter_hallucinated_tools(result: &AgentResult, registry: &ToolRegistry) -> Option<String> {
    let known = registry.list_tools(); // returns Vec<&str>
    for msg in &result.messages {
        if let Some(tool_calls) = &msg.tool_calls {
            for tc in tool_calls {
                if !known.contains(&tc.function.name.as_str()) {
                    return Some(format!("hallucinated_tool:{}", tc.function.name));
                }
            }
        }
    }
    None
}

fn filter_no_reasoning(result: &AgentResult) -> Option<String> {
    let tool_call_count = result.messages.iter()
        .filter(|m| m.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty()))
        .count();
    let has_text = result.final_response.as_ref().is_some_and(|r| !r.is_empty());
    if tool_call_count == 0 && !has_text {
        return Some("no_reasoning_steps".to_string());
    }
    None
}

fn filter_error_only(result: &AgentResult) -> Option<String> {
    // Check all tool result messages — if ALL contain error strings, reject
    // ...
}

fn filter_secrets_in_output(result: &AgentResult) -> Option<String> {
    // Run SECRET_PATTERNS regex set against each tool result message content
    // Use a new LazyLock<RegexSet> for credential patterns (API keys, tokens, etc.)
    // ...
}
```

### Pattern 5: Graceful Cancel

**What:** An `Arc<AtomicBool>` cancel flag (same pattern as `TelegramAdapter`). `batch cancel` writes a cancel file. The main dispatch loop checks the flag before spawning new tasks.

**When to use:** Implementing D-03 graceful shutdown.

```rust
// Source: Arc<AtomicBool> pattern from crates/ironhermes-gateway/src/telegram.rs
use std::sync::atomic::{AtomicBool, Ordering};
let cancel_flag = Arc::new(AtomicBool::new(false));

// In cancel signal handler:
cancel_flag.store(true, Ordering::Relaxed);

// In dispatch loop, before each new spawn:
if cancel_flag.load(Ordering::Relaxed) {
    break; // stop dispatching, let in-flight workers finish
}
```

### Pattern 6: BatchConfig in Config struct

**What:** Add `batch: BatchConfig` to `Config` struct following `SubagentConfig` precedent.

```rust
// Source: crates/ironhermes-core/src/config.rs SubagentConfig pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchConfig {
    /// Default worker concurrency. Default: 4.
    pub workers: usize,
    /// Default max agent iterations per prompt. Default: 20.
    pub max_turns: usize,
    /// Default output directory (relative to cwd). Default: "batch_output".
    pub output_dir: String,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            workers: 4,
            max_turns: 20,
            output_dir: "batch_output".to_string(),
        }
    }
}
```

### Anti-Patterns to Avoid

- **Concurrent JSONL writes without a channel:** Multiple tasks writing directly to the same file produces corrupt JSONL. Always route all writes through a single mpsc channel to a single writer task. This is explicitly called out in PITFALLS.md (referenced by CONTEXT.md).
- **Spawning unlimited tasks:** Spawning one task per entry before the semaphore causes memory exhaustion on large JSONL files. Acquire the semaphore permit BEFORE spawning, or gate spawning on available permits.
- **Mutable checkpoint under concurrent writes:** The checkpoint file must only be updated by the single writer task (same goroutine that writes trajectories), not by individual workers. Workers send completed trajectories; the writer updates both files.
- **Using `scan_context_content` for secrets detection:** `context_scanner.rs` detects prompt injection in loaded files — not secrets in tool output. BATCH-04 criterion 4 needs a separate secrets-pattern regex set targeting API key formats, bearer tokens, etc.
- **Blocking tokio thread for file I/O:** `std::fs` blocks the tokio thread. Use `tokio::fs` for the output JSONL writer task.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Content hashing | Custom hash | `sha2::Sha256` from workspace | Already in workspace deps (ironhermes-hooks). Correct, tested, constant-time. |
| Concurrent task limit | Manual counter + Mutex | `tokio::sync::Semaphore` | Proven pattern from Phase 9 delegate_task.rs and runner.rs. |
| Parallel task tracking | Vec<JoinHandle> | `tokio::task::JoinSet` | Already used in runner.rs. Handles panics, cancellation, and result collection. |
| JSONL serialisation | Manual string building | `serde_json::to_string` + `writeln!` | Proven pattern from log_writer.rs. Handles escaping, Unicode, etc. |
| Secrets regex patterns | Manual string scanning | `regex::RegexSet` | Same approach as `THREAT_PATTERNS` in context_scanner.rs. |
| SHA-256 hex encoding | Custom hex | `format!("{:x}", hasher.finalize())` | Standard approach, same as webhook.rs HMAC encoding. |

**Key insight:** Every concurrency primitive, serialisation path, and security pattern in this phase has been built and proven in earlier phases. This phase is integration work, not new infrastructure.

## Common Pitfalls

### Pitfall 1: Memory Exhaustion on Large Input Files

**What goes wrong:** Reading the entire JSONL input into memory before processing causes OOM on files with thousands of entries and large system prompts.

**Why it happens:** Naive `std::fs::read_to_string` + `serde_json::from_str` on each line in a Vec.

**How to avoid:** Stream the input file line by line using `tokio::io::BufReader::lines()`. Load one line at a time, hash it, check checkpoint, then gate on semaphore before spawning. Never hold all entries in memory simultaneously.

**Warning signs:** RSS growing proportionally to file size at startup before any workers run.

### Pitfall 2: Checkpoint File Corruption on Panic

**What goes wrong:** If the writer task panics mid-write to the checkpoint file, the JSON map becomes truncated and unreadable. Future runs fail to parse the checkpoint and re-run all entries.

**Why it happens:** Direct overwrite of checkpoint file without atomic rename.

**How to avoid:** Follow the cron atomic-save pattern: write to `{checkpoint}.tmp`, then `std::fs::rename()`. Rename is atomic on Linux/macOS for same-filesystem moves.

**Warning signs:** `serde_json::from_str` errors on checkpoint load in a resumed run.

### Pitfall 3: Tool Call Detection Missing Nested JSON

**What goes wrong:** Hallucinated tool name detection misses cases where the model emits a tool call with valid JSON arguments but an unregistered tool name.

**Why it happens:** Checking only `msg.content_text()` instead of `msg.tool_calls`.

**How to avoid:** Inspect `msg.tool_calls` in every `ChatMessage` with role `Assistant`. Cross-reference `tc.function.name` against `registry.list_tools()`.

**Warning signs:** Trajectories with clearly hallucinated tool names (e.g., `search_web` instead of `web_search`) passing the filter.

### Pitfall 4: `batch status` Races With Active Batch

**What goes wrong:** `batch status` reads the checkpoint file while the writer task is mid-write, observing an incomplete JSON map.

**Why it happens:** The checkpoint file is written by the batch runner process; `batch status` is a separate invocation reading the same file.

**How to avoid:** Status command reads the checkpoint file and handles parse errors gracefully (treats partial file as "in progress, N entries completed"). Do not require the checkpoint to be perfectly formed for status reads.

**Warning signs:** `batch status` panicking on JSON parse error.

### Pitfall 5: Cancel Signal Not Observed by Long-Running Worker

**What goes wrong:** `batch cancel` writes a cancel signal, but a worker in mid-AgentLoop (5+ minutes) keeps running.

**Why it happens:** Cancel only stops new task dispatch; it cannot interrupt a running AgentLoop (which has no cancellation mechanism).

**How to avoid:** This is expected and correct per D-03 ("in-flight workers finish their current entry"). Document clearly that cancel means "finish current work, stop starting new entries." Do not use `tokio::task::AbortHandle` on running tasks — that would produce incomplete trajectories.

**Warning signs:** User expecting immediate termination after `batch cancel`.

### Pitfall 6: ShareGPT Role Mismatch Breaks HuggingFace Viewer

**What goes wrong:** Using `"assistant"` instead of `"gpt"` as the from field, or `"user"` instead of `"human"`, causes the HuggingFace dataset viewer to not render the conversation correctly.

**Why it happens:** Confusing OpenAI chat format roles with ShareGPT roles.

**How to avoid:** Per D-08: use exactly `human` / `gpt` / `tool_call` / `tool_response`. Never `user` or `assistant` in the `from` field.

**Warning signs:** HuggingFace dataset viewer shows raw JSON or flat text instead of a rendered conversation.

## Code Examples

### Trajectory JSONL Line Structure (ShareGPT + metadata)

```rust
// Source: D-07, D-08, D-09 from 10-CONTEXT.md; ShareGPT format from HuggingFace datasets
#[derive(Serialize, Deserialize)]
struct TrajectoryLine {
    id: String,                          // SHA-256 of input prompt (hex)
    model: String,
    timestamp: String,                   // RFC3339
    usage: UsageSummary,
    turns: usize,                        // AgentResult.turns_used
    quality: QualityResult,
    conversations: Vec<ShareGptTurn>,
}

#[derive(Serialize, Deserialize)]
struct ShareGptTurn {
    from: String,   // "human" | "gpt" | "tool_call" | "tool_response"
    value: String,
}

#[derive(Serialize, Deserialize)]
struct QualityResult {
    passed: bool,
    reasons: Vec<String>,   // empty if passed; rejection reasons if failed
}

#[derive(Serialize, Deserialize)]
struct UsageSummary {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
}
```

### Rejection-Only JSONL Line

```rust
// Written to {output%.jsonl}_rejected.jsonl per D-11
#[derive(Serialize, Deserialize)]
struct RejectedLine {
    // Same fields as TrajectoryLine
    id: String,
    model: String,
    timestamp: String,
    usage: UsageSummary,
    turns: usize,
    quality: QualityResult,        // quality.passed = false
    conversations: Vec<ShareGptTurn>,
    rejection_reason: String,      // human-readable, e.g. "hallucinated_tool:nonexistent_tool"
}
```

### Checkpoint File Format

```json
// {output_path}.checkpoint.json
{
  "format_version": 1,
  "completed": {
    "a3f2b1...": { "status": "completed", "timestamp": "2026-04-10T12:00:00Z" },
    "c9d4e5...": { "status": "rejected",  "timestamp": "2026-04-10T12:00:05Z" }
  }
}
```

### Batch Run History Entry (for `batch list`)

```rust
// Written to ~/.ironhermes/batch/runs.json (append-on-complete)
// Mirrors cron jobs.json pattern
#[derive(Serialize, Deserialize)]
struct BatchRunRecord {
    id: String,            // UUID
    input_path: String,
    output_path: String,
    started_at: String,
    finished_at: Option<String>,
    total_entries: usize,
    completed: usize,
    rejected: usize,
    status: String,        // "running" | "completed" | "cancelled" | "failed"
}
```

### CLI Module Structure (cron.rs precedent)

```rust
// crates/ironhermes-cli/src/batch.rs — mirrors cron.rs pattern
#[derive(Subcommand)]
pub enum BatchCommands {
    /// Run a batch job from a JSONL input file
    Run {
        /// Input JSONL file (one {"prompt": "..."} per line)
        input: PathBuf,
        /// Output JSONL file [default: {input%.jsonl}_output.jsonl]
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Maximum parallel workers [default: config.batch.workers]
        #[arg(long)]
        workers: Option<usize>,
        /// Model override [default: config.model.default]
        #[arg(long)]
        model: Option<String>,
    },
    /// Show progress of the current/last batch run
    Status,
    /// Cancel the current batch run gracefully
    Cancel,
    /// List past batch runs
    List,
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Single-threaded batch scripts | `JoinSet` + `Semaphore` bounded concurrency | Tokio 1.0+ | Workers complete in parallel, limited by `--workers` flag |
| ShareGPT `human`/`gpt` only | `tool_call`/`tool_response` turns added | 2024 (tool-use fine-tuning) | Trajectories with tool use are directly usable for tool-call fine-tuning |
| Full-file rewrites for checkpoint | Atomic rename (`tmp` → final) | Always best practice | Prevents corrupt checkpoint on crash |

**Deprecated/outdated:**
- `from: "system"` at conversation start: Per D-08 and standard ShareGPT practice, system prompts are excluded from the `conversations` array in trajectory output. They are implementation context, not training data.
- `from: "user"` / `from: "assistant"`: These are OpenAI chat format roles. ShareGPT uses `human` and `gpt` in the `from` field. [ASSUMED — training knowledge; confirmed consistent with D-08 locked decision]

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | ShareGPT `from` values `tool_call`/`tool_response` are accepted by the HuggingFace dataset viewer for tool-use trajectories | Standard Stack / Code Examples | Viewer may not render tool turns specially; data still valid for fine-tuning |
| A2 | `ironhermes-cli` Cargo.toml does not yet declare `sha2` as a direct dep (only `ironhermes-hooks` does) | Standard Stack | Linker resolves it transitively via workspace but best practice is explicit dep declaration |
| A3 | Default concurrency of 4 workers is reasonable for typical hardware and API rate limits | Architecture Patterns (BatchConfig) | May need tuning; value is in Claude's discretion per CONTEXT.md |
| A4 | The existing `ChatMessage` enum `Role` variants cover `User`, `Assistant`, `Tool`, `System` — covering all message types emitted by `AgentLoop` | Code Examples | If Role enum has additional variants, the match in messages_to_sharegpt needs an arm |

**If this table is empty:** Not empty — four low-risk assumptions logged above.

## Open Questions

1. **Secrets pattern set for BATCH-04 criterion 4**
   - What we know: `context_scanner.rs` has a `THREAT_PATTERNS` RegexSet for injection detection. `CONCERNS.md` mentions `redact_secrets: bool` in config is not yet implemented.
   - What's unclear: What credential patterns should the batch output scanner match? Common patterns (AWS keys `AKIA[0-9A-Z]{16}`, OpenAI keys `sk-[a-zA-Z0-9]{48}`, bearer tokens `Bearer [A-Za-z0-9-._~+/]+=*`) would be appropriate but the exact set needs a decision.
   - Recommendation: Create a new `SECRET_PATTERNS` `LazyLock<RegexSet>` in a new `batch_scanner.rs` (or inline in the quality filter module). Do not reuse `scan_context_content` — different purpose.

2. **`batch status` data source**
   - What we know: `batch status` (D-02) shows progress of current/last run. The running batch process is in a separate process invocation from the `batch status` subcommand.
   - What's unclear: How does `status` observe live progress? Options: (a) read checkpoint file count, (b) read a lightweight progress file (`{output}.progress.json`), (c) SQLite.
   - Recommendation: Write a progress sidecar file (`{output}.progress.json`) alongside the checkpoint. The batch runner updates it atomically after each completed entry. `batch status` reads it and pretty-prints. Simpler than SQLite, survives process restart.

3. **JSONL input validation**
   - What we know: Minimum input is `{"prompt": "..."}` per line (Claude's Discretion from CONTEXT.md). Additional fields possible.
   - What's unclear: How strictly to validate unknown fields (ignore vs. error) and how to handle empty prompts or malformed lines.
   - Recommendation: Skip malformed lines with a warning (logged to stderr). Treat empty prompt as a user error logged to stderr, skip the entry, do not create a trajectory for it.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain | Build | Yes (assumed) | 1.85+ (edition 2024) | — |
| `sha2` crate | Checkpointing | Yes (workspace dep) | workspace | — |
| `tokio::task::JoinSet` | Worker pool | Yes (tokio 1) | workspace | — |
| `ironhermes-agent::AgentLoop` | Per-prompt execution | Yes (Phase 9 complete) | — | — |
| `ironhermes-core::Config` | BatchConfig | Yes | — | — |

**Missing dependencies with no fallback:** None.

**Missing dependencies with fallback:** None.

**Step 2.6: No external CLI tools, services, or runtimes required.** Batch processing is pure Rust async; no external processes, databases, or services are needed beyond what runs `ironhermes` today.

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in test harness (`cargo test`) |
| Config file | None — workspace-level `cargo test` |
| Quick run command | `cargo test -p ironhermes-cli -- batch` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| BATCH-01 | Semaphore bounds parallel workers to `--workers` limit | unit | `cargo test -p ironhermes-cli batch::tests::test_worker_concurrency` | ❌ Wave 0 |
| BATCH-01 | JSONL input parsed line-by-line; each line becomes one AgentLoop run | unit | `cargo test -p ironhermes-cli batch::tests::test_jsonl_parsing` | ❌ Wave 0 |
| BATCH-02 | `ChatMessage` sequence maps correctly to ShareGPT turns | unit | `cargo test -p ironhermes-cli batch::tests::test_messages_to_sharegpt` | ❌ Wave 0 |
| BATCH-02 | Output JSONL has valid `conversations` array with correct `from` values | unit | `cargo test -p ironhermes-cli batch::tests::test_sharegpt_from_roles` | ❌ Wave 0 |
| BATCH-03 | SHA-256 hash of prompt produces consistent hex string | unit | `cargo test -p ironhermes-cli batch::tests::test_prompt_hash_stable` | ❌ Wave 0 |
| BATCH-03 | Resume skips entries whose hash appears in checkpoint | unit | `cargo test -p ironhermes-cli batch::tests::test_checkpoint_skip` | ❌ Wave 0 |
| BATCH-03 | Checkpoint file deleted on successful completion | unit | `cargo test -p ironhermes-cli batch::tests::test_checkpoint_cleanup` | ❌ Wave 0 |
| BATCH-04 | Hallucinated tool name triggers rejection | unit | `cargo test -p ironhermes-cli batch::tests::test_filter_hallucinated_tool` | ❌ Wave 0 |
| BATCH-04 | No-tool-call, no-text trajectory triggers rejection | unit | `cargo test -p ironhermes-cli batch::tests::test_filter_no_reasoning` | ❌ Wave 0 |
| BATCH-04 | Error-only tool calls trigger rejection | unit | `cargo test -p ironhermes-cli batch::tests::test_filter_error_only` | ❌ Wave 0 |
| BATCH-04 | Rejected trajectories written to `_rejected.jsonl` with `rejection_reason` | unit | `cargo test -p ironhermes-cli batch::tests::test_reject_file_output` | ❌ Wave 0 |

### Sampling Rate

- **Per task commit:** `cargo test -p ironhermes-cli -- batch`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** Full suite green before `/gsd-verify-work 10`

### Wave 0 Gaps

- [ ] `crates/ironhermes-cli/src/batch.rs` — new module with all batch tests (covers BATCH-01 through BATCH-04)
- [ ] No new test config needed — existing workspace test harness applies
- [ ] `crates/ironhermes-core/src/config.rs` — add `BatchConfig` struct and tests following `test_subagent_config_default` pattern

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | No | N/A — batch runs under same API key as CLI |
| V3 Session Management | No | Batch entries are stateless; no sessions |
| V4 Access Control | No | Single-operator tool; no multi-user access |
| V5 Input Validation | Yes | Validate JSONL input lines; skip/warn on malformed; reject empty prompts |
| V6 Cryptography | Partial | SHA-256 used for checkpointing (integrity, not secrecy) — `sha2` crate is correct |

### Known Threat Patterns for Batch Processing Stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Prompt injection via JSONL input | Tampering | Treat input prompts as untrusted user content (already handled by AgentLoop — model decides what to execute) |
| Secrets leaking into trajectory output | Information Disclosure | BATCH-04 criterion 4: scan tool result strings with SECRET_PATTERNS before writing to JSONL |
| Checkpoint file corruption causing re-execution | Tampering | Atomic rename pattern for checkpoint writes |
| JSONL output truncation from concurrent writes | Tampering | Single mpsc channel writer (D-10 locked decision) |
| API key exfiltration in batch output | Information Disclosure | Same SECRET_PATTERNS scan; reject trajectories containing credential patterns |

## Sources

### Primary (HIGH confidence)

- `crates/ironhermes-agent/src/agent_loop.rs` — `AgentResult`, `AggregatedUsage`, `ChatMessage` role handling verified in source
- `crates/ironhermes-tools/src/delegate_task.rs` — `Semaphore::acquire_owned()` + `JoinSet` pattern verified in source
- `crates/ironhermes-gateway/src/runner.rs` — `JoinSet` + `mpsc` channel pattern verified in source
- `crates/ironhermes-hooks/src/log_writer.rs` — JSONL write pattern (`serde_json::to_string` + `writeln!`) verified in source
- `crates/ironhermes-hooks/src/webhook.rs` — `sha2::Sha256` usage and hex encoding verified in source
- `crates/ironhermes-core/src/context_scanner.rs` — `LazyLock<RegexSet>` pattern for threat scanning verified in source
- `crates/ironhermes-core/src/config.rs` — `BatchConfig` will follow `SubagentConfig` pattern verified in source
- `/Users/twilson/code/ironhermes/Cargo.toml` — workspace deps (`sha2`, `tokio`, `serde_json`, `clap`, `regex`, `chrono`, `uuid`) verified

### Secondary (MEDIUM confidence)

- [ShareGPT format via HuggingFace datasets](https://huggingface.co/datasets/kingbri/PIPPA-shareGPT) — `conversations` array with `from`/`value` pairs confirmed by WebSearch; `human`/`gpt` roles standard
- [TRL ShareGPT support issue](https://github.com/huggingface/trl/issues/2083) — confirms `human`/`gpt` as canonical ShareGPT role names for fine-tuning toolchains

### Tertiary (LOW confidence)

- `tool_call`/`tool_response` as ShareGPT `from` values for tool-use trajectories — consistent with D-07/D-08 locked decisions but not independently verified against a canonical tool-use fine-tuning dataset spec [ASSUMED]

## Metadata

**Confidence breakdown:**

- Standard stack: HIGH — all libraries verified in workspace Cargo.toml and existing source files
- Architecture: HIGH — all patterns directly lifted from existing crates (delegate_task.rs, runner.rs, log_writer.rs, config.rs)
- ShareGPT format: MEDIUM — roles verified via web search; tool_call/tool_response roles assumed from D-07/D-08 locked decisions
- Pitfalls: HIGH — most sourced directly from CONTEXT.md specifics and CONCERNS.md observations
- Quality filter: HIGH — ToolRegistry.list_tools() and RegexSet pattern both verified in existing code

**Research date:** 2026-04-10
**Valid until:** 2026-05-10 (stable libraries, no fast-moving deps)
