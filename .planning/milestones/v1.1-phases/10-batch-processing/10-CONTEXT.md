# Phase 10: Batch Processing - Context

**Gathered:** 2026-04-10
**Status:** Ready for planning

<domain>
## Phase Boundary

This phase delivers the `ironhermes batch` subcommand group: a parallel batch prompt executor that reads JSONL input, runs each prompt through an AgentLoop with bounded concurrency, outputs ShareGPT-format trajectory data with metadata, supports automatic checkpointing/resume, and applies quality filtering with reject separation.

**Out of scope:**
- Distributed batch execution across multiple machines
- Custom model routing per-prompt (all prompts use same model)
- Real-time streaming progress to external systems (webhooks)
- Post-processing pipelines (HuggingFace upload, dataset card generation)

</domain>

<decisions>
## Implementation Decisions

### CLI Invocation Design
- **D-01:** Subcommand group pattern — `ironhermes batch run|status|cancel|list`. Follows cron subcommand precedent. `run` takes positional input path, `-o` for output, `--workers` for concurrency, `--model` for override.
- **D-02:** `batch status` shows progress of the current/last batch run (entries completed/total, workers active, elapsed time, ETA).
- **D-03:** `batch cancel` sends graceful shutdown signal — in-flight workers finish their current entry, no new entries start. Checkpoint is saved.
- **D-04:** `batch list` shows past batch runs with summary (input file, entries, pass/reject counts, duration).

### Checkpointing & Resume
- **D-05:** Always checkpoint — every completed entry is persisted immediately. No `--resume` flag needed. Content hash (SHA-256 of input prompt) identifies completed entries. Rerunning the same command automatically skips completed entries.
- **D-06:** Checkpoint file stored alongside output — `{output_path}.checkpoint.json` containing hash→status mapping. Deleted when batch completes successfully with all entries.

### ShareGPT Output Format
- **D-07:** Tool calls as separate conversation turns — `tool_call` and `tool_response` as distinct `from` values. Maps directly from AgentLoop's ChatMessage sequence.
- **D-08:** Conversations use `human`/`gpt`/`tool_call`/`tool_response` role names. Standard ShareGPT `from` field convention.
- **D-09:** Each trajectory line includes metadata: `id` (content hash), `model`, `timestamp`, `usage` (prompt/completion tokens), `turns` count, `quality` object, plus `conversations` array.
- **D-10:** Output is JSONL — one trajectory per line. Channel-based serialization to avoid concurrent write corruption (per PITFALLS.md warning).

### Quality Filtering
- **D-11:** Separate reject file — filtered trajectories written to `{output_path%.jsonl}_rejected.jsonl` with `rejection_reason` field. Nothing silently discarded.
- **D-12:** Four built-in rejection criteria (all enabled by default):
  1. **Hallucinated tool names** — agent called a tool not in the registry
  2. **No reasoning steps** — final response with zero tool calls and no chain-of-thought
  3. **Error-only trajectories** — every tool call errored, no successful actions
  4. **Secrets in output** — API keys, tokens, or credentials detected in tool results (reuses existing security scanning from SELF-03)
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

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` — Lines 104-107 define BATCH-01 through BATCH-04. Lines 250-253 in traceability table map all four to Phase 10.
- `.planning/ROADMAP.md` — Phase 10 section has the 4 success criteria and 2-plan estimate.

### Existing patterns (templates for this phase)
- `crates/ironhermes-cli/src/main.rs` — CLI subcommand structure (clap derive), semaphore creation pattern from Phase 9.
- `crates/ironhermes-agent/src/agent_loop.rs` — AgentLoop::new() + run() returning AgentResult with messages, usage, final_response.
- `crates/ironhermes-cron/src/tick.rs` — Job checkpointing pattern (file-based state tracking, reload for external writes).
- `crates/ironhermes-hooks/src/log_writer.rs` — JSONL write pattern with serde_json + writeln.
- `crates/ironhermes-tools/src/delegate_task.rs` — Semaphore-based concurrency limiting (Phase 9).

### Architecture reference
- `.planning/codebase/ARCH.md` — Crate dependency graph, concurrency model, shared state patterns.
- `.planning/codebase/PITFALLS.md` — Batch-specific warnings: memory exhaustion, secrets in output, concurrent JSONL writes.
- `.planning/codebase/FEATURES.md` — ShareGPT format specification, batch runner feature mapping.

### Prior phase context
- `.planning/phases/09-subagent-delegation/09-CONTEXT.md` — Semaphore patterns, AgentLoop child spawning, SubagentConfig. Direct precedent for bounded parallel execution.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `AgentLoop::new(client, registry, max_iterations).run(messages)` — core execution unit for each batch entry
- `AgentResult { messages, turns_used, finished_naturally, final_response, total_usage }` — maps directly to ShareGPT output
- `ChatMessage` enum with role/content — source data for ShareGPT conversation turns
- `tokio::sync::Semaphore` — proven concurrency pattern from Phase 9 subagent delegation
- `serde_json::to_string` + `writeln!` — JSONL serialization pattern from hooks log writer
- Security scanning from `ironhermes-core` — reusable for secrets detection in output filtering

### Established Patterns
- CLI subcommands via clap derive with nested enum (CronCommands precedent)
- Config sections with `Default` impl — add `BatchConfig` with workers, output path defaults
- File-based state persistence (cron jobs.json pattern)
- Channel-based serialization for concurrent writes (recommended in PITFALLS.md)

### Integration Points
- `Config` struct — new `batch: BatchConfig` section
- `main.rs` — new `Batch(BatchCommands)` variant in Commands enum
- `ToolRegistry` — shared across batch workers (already Arc-wrapped)

</code_context>

<specifics>
## Specific Ideas

1. **Worker pool**: Use `tokio::task::JoinSet` with a semaphore to bound concurrency. Each worker takes a prompt, builds messages, runs AgentLoop, serializes output. Workers send completed trajectories through an `mpsc` channel to a single writer task.

2. **Checkpoint structure**: `{output}.checkpoint.json` mapping content hash → `{status: "completed"|"rejected", line_number: N}`. On resume, load checkpoint, hash each input line, skip if present.

3. **Quality filter pipeline**: After AgentLoop returns, run each filter function in sequence. If any returns a rejection reason, route to reject file. Filters are pure functions: `fn(agent_result: &AgentResult, registry: &ToolRegistry) -> Option<RejectionReason>`.

4. **Secrets scanning**: Reuse the existing `SecurityScanner` from context file loading (SELF-03). Run it on each tool result string in the conversation. Flag if any match.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 10-batch-processing*
*Context gathered: 2026-04-10*
