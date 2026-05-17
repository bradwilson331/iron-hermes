# Phase 34b: Context-System Parity - Context

**Gathered:** 2026-05-16
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 34b closes the parity gap with three context-system modules in hermes-agent:

1. **`@`-reference expansion** (Plan 34b-01) — port `context_references.py`. Users can write `@file:foo.rs:10-25`, `@folder:src/`, `@diff`, `@staged`, `@git:N`, `@url:https://...` in chat messages; tokens are parsed pre-turn, expanded into attached-context blocks, and stripped from the inline message. Sensitive-path blocklist + 50%/25% token budget enforced. All 3 surfaces wired.

2. **`ContextEngine` lifecycle hook parity** (Plan 34b-02) — port `context_engine.py`. Add 5 hooks (`on_session_start`, `on_session_reset`, `update_from_response`, `update_model`, `has_content_to_compress`) to the existing `ContextEngine` trait as default no-ops. `ContextCompressor` and `SummarizingEngine` override `on_session_reset` to clear counters. Hooks wired at all 3 surfaces + agent loop call sites.

3. **`ContextCompressor` counter reset + memory-authority reminder** (Plan 34b-02) — port `context_compressor.py`. `on_session_reset` clears `compression_count` and token counters. Verify + patch `SummarizingEngine`'s compaction header to include the memory-authority reminder ("MEMORY.md and USER.md in the system prompt are ALWAYS authoritative") if missing.

**Does not deliver:** `focus_topic` for LLM-guided compression, LCM engine tools (`lcm_grep`, `lcm_describe`), `MemoryProvider` turn/delegation hooks, "only one external provider" guard — all deferred.

**Note:** Phase 34b is not yet in ROADMAP.md. Researcher/planner should add it before or during planning.

</domain>

<decisions>
## Implementation Decisions

### @url: expansion

- **D-01:** `@url:` uses LLM-processed expansion — call `WebExtractTool` with `use_llm_processing: true`. Mirrors Python's `web_extract_tool` behavior. Produces polished markdown output suitable for injection.

- **D-02:** If LLM processing fails (network error, timeout, provider down), fall back to raw HTTP content and surface a warning in the `--- Context Warnings ---` block. Agent still gets the content, just unpolished. Do NOT fail silently or drop the reference.

### allowed_root scoping

- **D-03:** Default `allowed_root` is `cwd` — mirrors Python. `@file:` and `@folder:` references cannot escape the workspace root.

- **D-04:** `allowed_root` is **fixed to cwd — no config escape hatch**. Simpler, smaller attack surface. The sensitive-path blocklist is a second independent defense layer.

- **D-05:** `allowed_root` resolves to `TerminalConfig.cwd` if set in `cli-config.yaml`; otherwise `std::env::current_dir()` at startup. Consistent with how the terminal tool resolves its working directory.

### ContextEngine trait shape

- **D-06:** The 5 lifecycle hooks go on the **existing `ContextEngine` trait** as additive default no-op impls. Mirrors Python's single ABC. No breaking changes — existing implementors (`LocalPruningEngine`, `SummarizingEngine`) inherit the no-ops and override only what they need. The `check_pressure` default no-op already demonstrates this pattern.

- **D-07:** `update_from_response` and `update_model` are **wired at call sites in this phase** — not deferred to LCM. Call `engine.update_from_response(&usage)` after every `AgentLoop::run` returns with usage data. Call `engine.update_model(&model, ctx_len, base_url)` when the model changes. Full Python parity now.

### Phase 34a integration (carried forward — already solved)

- **D-08:** `is_recall_context` messages injected per-turn (34a D-01) are already stripped in compressor step 0 (34a D-03: `messages.retain(|m| !m.is_recall_context)`). The compression/recall-message interaction requires **no additional work in Phase 34b**.

### Claude's Discretion

- Exact type for `update_from_response` usage parameter — use `AggregatedUsage` (already defined in `agent_loop.rs`) or a new `UsageReport` alias; pick the cleanest fit.
- `has_content_to_compress` default impl returns `true` (matches Python); `LocalPruningEngine` and `SummarizingEngine` may override if they develop a cheaper early-exit check.
- Exact position of `preprocess_context_references_async` in each surface's call path — immediately before the user message is handed to `AgentLoop::run`.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Python reference implementation
- `../hermes-agent/agent/context_references.py` — canonical port target; `parse_context_references`, `expand_context_references`, `SENSITIVE_PATHS` blocklist, budget enforcement, output format
- `../hermes-agent/agent/context_engine.py` — canonical port target; `ContextEngine` ABC with all 5 lifecycle hooks + `has_content_to_compress`
- `../hermes-agent/agent/context_compressor.py` — canonical port target; `on_session_reset` counter clear, `SUMMARY_PREFIX` memory-authority reminder

### Draft plan (parity matrix + success criteria)
- `.planning/phases/34b-context-system-parity/34b-PLAN-DRAFT.md` — complete parity matrix, 15 success criteria, plan breakdown, deferred list

### Phase 34a context (decisions that carry forward)
- `.planning/phases/34a-read-side-memory-parity/34A-CONTEXT.md` — D-01 (`is_recall_context` on `ChatMessage`), D-03 (compressor step 0 strips recall messages), D-08 (empty-recall skip)

### ContextEngine + compression (ironhermes-agent)
- `crates/ironhermes-agent/src/context_engine.rs` — `ContextEngine` trait; add 5 lifecycle hooks here with default no-op impls
- `crates/ironhermes-agent/src/context_compressor.rs` — `ContextCompressor` struct (also `LocalPruningEngine`); override `on_session_reset` to clear counters
- `crates/ironhermes-agent/src/summarizing_engine.rs` — `SummarizingEngine`; override `on_session_reset`; verify + patch `SUMMARY_PREFIX`/compaction header for memory-authority reminder

### @-reference expansion (new module)
- `crates/ironhermes-agent/src/context_refs.rs` — NEW; full parser + expander + blocklist + budget logic; exports `parse_context_references` + `preprocess_context_references_async`
- `crates/ironhermes-agent/src/lib.rs` — add `pub mod context_refs`

### URL fetcher
- `crates/ironhermes-tools/src/web_extract.rs` — `WebExtractTool`; call with `use_llm_processing: true` for `@url:` expansion; fall back to `false` on LLM failure

### Three-surface wiring (preprocessing + session hooks)
- `crates/ironhermes-cli/src/main.rs` — `run_chat`: @-ref preprocessing on each user message; `on_session_start` at REPL start; `on_session_reset` from `/new` and `/reset`; `update_from_response` after each turn; `update_model` on model change
- `crates/ironhermes-gateway/src/handler.rs` — `handle_with_multimodal`: @-ref preprocessing; `on_session_start` on new `SessionKey`; `on_session_reset` from `/reset` slash
- `crates/iron_hermes_ui/src/server/state.rs` — `run_web_turn`: @-ref preprocessing; `on_session_start` on WebSocket connect; `on_session_reset` on new-chat request

### Regression gates (must stay green)
- Phase 34a: `cargo test -p ironhermes-agent --lib memory_context::tests` + `streaming_scrubber::tests`
- Phase 33: `cargo test -p ironhermes-agent --test invariants_33` — 6/6
- Phase 32: `cargo test -p ironhermes-agent --lib nudge::tests` — 6/6
- D-12 gate: `cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load`

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `WebExtractTool` (`crates/ironhermes-tools/src/web_extract.rs`) — `use_llm_processing: bool` already supported; call with `true` for D-01; retry with `false` on LLM failure per D-02
- `check_pressure` default no-op on `ContextEngine` (line 63) — established pattern for default-no-op lifecycle hooks; the 5 new hooks follow the same idiom
- `messages.retain(|m| !m.is_recall_context)` — already wired in compressor step 0 (34a D-03); handles the compression/recall interaction, no extra work in 34b

### Established Patterns
- **3-surface wiring** — nudge (Phase 32), recall scrubber (Phase 34a), now @-ref preprocessing and session hooks all use the same CLI/gateway/web UI trio. The call-site shape is established.
- **Default no-op trait impls** — `check_pressure` with `async fn check_pressure(&self, _stats: &ContextStats) -> bool { false }` is the template to follow for all 5 new hooks.
- **Sensitive-path pattern** — Python's `SENSITIVE_PATHS` is a flat list of dirs and files; Rust implementation should match exactly (listed in 34b draft plan success criteria §3).
- **`AggregatedUsage`** in `agent_loop.rs` — candidate type for `update_from_response` parameter; implementer confirms fit.

### Integration Points
- `preprocess_context_references_async` sits between the user's raw input and `AgentLoop::run`; `result.message` replaces the original; `result.warnings` are logged or surfaced before the agent call
- `on_session_start` / `on_session_reset`: the `ContextEngine` is held behind an `Arc<dyn ContextEngine>` in the agent loop; hooks are `&self` (not `&mut self`), so they use interior mutability (`Mutex` / `AtomicUsize`) for any counter state they clear
- `TerminalConfig.cwd` is in `crates/ironhermes-core/src/config.rs` under `AgentConfig`; read it at @-ref preprocessing time to resolve `allowed_root` (D-05)

</code_context>

<specifics>
## Specific Ideas

- `@url:` expansion output format mirrors Python: `🌐 @url:<url> (N tokens)\n{markdown content}` inside the `--- Attached Context ---` footer
- `allowed_root` resolution order: `TerminalConfig.cwd` → `std::env::current_dir()` → error (unreachable in practice). Store as `PathBuf` at preprocessing time.
- The memory-authority reminder to verify/add in `SummarizingEngine` compaction header: *"Your persistent memory (MEMORY.md, USER.md) in the system prompt is ALWAYS authoritative — never ignore or deprioritize memory content due to this compaction note."* Add a unit test asserting the header contains "MEMORY.md" and "ALWAYS authoritative".
- `on_session_reset` clears: `compression_count`, `last_prompt_tokens`, `last_completion_tokens`, `last_total_tokens`, and any internal `PressureTracker` state. Add a unit test asserting all fields are zero after the call.

</specifics>

<deferred>
## Deferred Ideas

- **`focus_topic` arg on `compress(...)`** (user-guided `/compress <focus>`) — LCM phase
- **LCM engine tools** (`lcm_grep`, `lcm_describe`, `lcm_expand`) — LCM phase
- **Promoting `PressureTracker` fields to trait level** — LCM phase (may need a new associated type)
- **`MemoryProvider.on_turn_start` / `on_session_switch` / `on_delegation`** — separate memory-lifecycle phase
- **"Only one external memory provider" guard** — same memory-lifecycle phase
- **`MemoryProvider.on_pre_compress` returns text** — same memory-lifecycle phase
- **Multi-provider teardown order** — when a second external memory provider lands

</deferred>

---

*Phase: 34b-context-system-parity*
*Context gathered: 2026-05-16*
