---
phase: 34b-context-system-parity
plan: 02
type: execute
wave: 2
depends_on: [34b-01]
files_modified:
  - crates/ironhermes-agent/src/context_engine.rs
  - crates/ironhermes-agent/src/context_compressor.rs
  - crates/ironhermes-agent/src/summarizing_engine.rs
  - crates/ironhermes-agent/src/agent_runtime.rs
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-gateway/src/handler.rs
  - crates/iron_hermes_ui/src/server/state.rs
  - crates/ironhermes-agent/tests/invariants_34b.rs
autonomous: true
requirements: [CTX-ENG-01, CTX-ENG-02, CTX-ENG-03, CTX-ENG-04]
must_haves:
  truths:
    - "D-06: the 5 lifecycle hooks (on_session_start, on_session_reset, update_from_response, update_model, has_content_to_compress) are ADDITIVE default-no-op impls on the EXISTING ContextEngine trait (Task 1); existing implementors (LocalPruningEngine, SummarizingEngine) inherit the no-ops and compile unchanged"
    - "ContextCompressor::on_session_reset zeroes compression_count and all token counters; a unit test proves it"
    - "The compaction history-segment header contains the memory-authority reminder (MEMORY.md + ALWAYS authoritative); a unit test proves it"
    - "Per-turn hooks (update_from_response AND update_model) are invoked ONCE centrally in run_turn — not per-surface (D-09); update_model is wired definitely this phase (D-07), not conditionally"
    - "Per-session reset is wired at the surfaces where the durable per-session counter lives: CLI /new (ClearSession arm) resets compression_count; gateway /new (NewSession arm); web reset_web_session documented stub (D-09/D-10)"
    - "D-08: no-op for 34b — the is_recall_context / recall-message compression interaction is already handled by Phase 34a compressor step 0 (messages.retain(|m| !m.is_recall_context)); this phase adds NO additional recall-stripping work"
  artifacts:
    - path: crates/ironhermes-agent/src/context_engine.rs
      provides: "5 default-no-op lifecycle hooks on the ContextEngine trait"
      contains: "fn on_session_reset"
    - path: crates/ironhermes-agent/src/context_compressor.rs
      provides: "on_session_reset override clearing counters + memory-authority reminder in compaction header"
      contains: "fn on_session_reset"
    - path: crates/ironhermes-agent/src/summarizing_engine.rs
      provides: "memory-authority reminder embedded in the pinned history-segment header"
      contains: "ALWAYS authoritative"
    - path: crates/ironhermes-agent/src/agent_runtime.rs
      provides: "central per-turn update_from_response + update_model invocation in run_turn"
      contains: "update_from_response"
  key_links:
    - from: crates/ironhermes-agent/src/agent_runtime.rs
      to: crates/ironhermes-agent/src/context_engine.rs
      via: "engine.update_from_response(&usage) + engine.update_model(...) called once in run_turn"
      pattern: "update_from_response|update_model"
    - from: crates/ironhermes-cli/src/main.rs
      to: crates/ironhermes-agent/src/context_engine.rs
      via: "on_session_reset at /new (ClearSession arm) + compression_count reset"
      pattern: "on_session_reset|compression_count"
---

<objective>
Port `../hermes-agent/agent/context_engine.py` lifecycle hooks +
`context_compressor.py` counter reset + the SUMMARY_PREFIX memory-authority
reminder, and wire them per the post-28.1 architecture (D-09/D-10).

D-10 RESOLUTION (stated explicitly): `run_turn` rebuilds the ContextEngine FRESH
every turn (`attach_context_engine`), so per-session counter state is NOT durable
inside the engine. We take the HYBRID of options (a) and (b):
- The parity-shaped state-threading already exists for `compression_count`
  (TurnRequest.compression_count → run_turn → AgentResult.compression_count_after,
  CLI persists it in an Arc<AtomicUsize>). We do NOT add new per-turn token-counter
  threading — post-turn token totals are already available on AgentResult.total_usage.
- We RE-SCOPE the hooks to what is meaningful under a per-turn engine:
  * `update_from_response`/`update_model` are default-no-op trait hooks invoked
    ONCE centrally in run_turn (D-09 per-turn locus) so any engine that DOES hold
    durable state can react; the shipped engines keep them as no-ops (their
    durable counter is the surface-owned compression_count, not engine state).
  * `on_session_reset` is wired at the SURFACES where the durable per-session
    state lives: CLI resets its Arc<AtomicUsize> compression_count at /new; the
    ContextCompressor::on_session_reset override still zeroes its own fields so a
    long-lived compressor instance (and the parity unit test) is correct.
This avoids reproducing the broken old 34b-02 (engine holds counters, hook clears
them every-turn-fresh) while preserving the Python trait shape.

Output: 5 trait hooks, compressor reset override, memory-authority reminder,
central per-turn hook call in run_turn, surface-level session-reset wiring.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/STATE.md
@.planning/phases/34b-context-system-parity/34B-CONTEXT.md
@.planning/phases/34b-context-system-parity/34B-PATTERNS.md

<interfaces>
From crates/ironhermes-agent/src/context_engine.rs — the trait to extend (add hooks alongside check_pressure):
```rust
#[async_trait]
pub trait ContextEngine: Send + Sync + 'static {
    async fn compress(&self, messages: &mut Vec<ChatMessage>, stats: ContextStats) -> Result<CompressionOutcome, ContextError>;
    fn threshold(&self) -> f32;
    fn mode(&self) -> CompressionMode;
    async fn check_pressure(&self, _stats: &ContextStats) -> bool { false }   // existing default-no-op template
}
```
New hooks follow the `check_pressure` default-no-op idiom. `&self` only — any
clearing uses interior mutability (Mutex/AtomicUsize), per CONTEXT code_context.

From crates/ironhermes-agent/src/context_compressor.rs:
- `pub struct ContextCompressor { compression_count: usize, ... }` (~:39-44); `compression_count += 1` at ~:101 (so the field is mutated via &mut self today — the reset override needs interior mutability OR the override clears via &mut on a Mutex-wrapped instance; confirm the engine-trait `&self` constraint and use AtomicUsize/Mutex for the resettable fields).
- Python parity fields to clear: compression_count, last_prompt_tokens, last_completion_tokens, last_total_tokens (+ any internal ineffective-compression counter).

From crates/ironhermes-agent/src/summarizing_engine.rs:
- `make_history_message(summary_body)` (~:54) builds the pinned `[CONTEXT HISTORY]\n<summary>` system block — this is the compaction header the model sees. The memory-authority reminder belongs here (or in the prompt preamble at ~:521-537), so the pinned summary carries it.

From ../hermes-agent/agent/context_compressor.py — SUMMARY_PREFIX (~:37-46), the exact reminder text (line 45-46):
"IMPORTANT: Your persistent memory (MEMORY.md, USER.md) in the system prompt is ALWAYS authoritative and active — never ignore or deprioritize memory content due to this compaction note."

From crates/ironhermes-agent/src/agent_runtime.rs — run_turn (~:205): the model
identity for the turn is fully resolvable here from
`self.resolver.resolve_for_main()` (the SAME accessor run_turn already uses at
~:209 for `context_length`). It returns a `&ResolvedEndpoint`
(crates/ironhermes-core/src/provider.rs:43) exposing `pub default_model: String`,
`pub base_url: String`, and `.context_length() -> usize`. So
`engine.update_model(endpoint.default_model.as_str(), context_length,
Some(endpoint.base_url.as_str()))` is wireable with NO hedge (resolves the
RESEARCH Open Question on the update_model call site). After
`agent.run(req.messages).await` returns the AgentResult with `total_usage`, call
`engine.update_from_response(&result.total_usage)`. The engine is attached at
~:249 (attach_context_engine returns the AgentLoop holding the
Arc<dyn ContextEngine>); add a minimal AgentLoop accessor to reach the engine for
the hook calls if one is not already present.

NOTE (resolves RESEARCH Open Question on PressureTracker): `PressureTracker`
(crates/ironhermes-agent/src/pressure_warning.rs) has NO `reset_session`/`reset`
method — only `new`, `take_transient`, `was_warned`, `warn_count`. The compressor
reset therefore zeroes its OWN fields directly; it does not delegate to a
PressureTracker reset that does not exist.

Surface session-reset loci (D-09 — per-session hooks STAY at surfaces):
- CLI: crates/ironhermes-cli/src/main.rs — `compression_count: Arc<AtomicUsize>` created ~:1166; `CommandResult::ClearSession(output)` arm ~:1573 (`messages.truncate(1)`) is the /new reset point.
- Gateway: crates/ironhermes-gateway/src/handler.rs — `CoreCommandResult::NewSession` arm ~:506 (session_store.remove).
- Web: crates/iron_hermes_ui/src/server/state.rs — `ensure_web_session` ~:174; no new-chat reset trigger exists yet → documented `reset_web_session` stub (CONTEXT Open Question 1 / VALIDATION manual note). This stub is the accepted scope for this phase.
</interfaces>
</context>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Add 5 lifecycle hooks to ContextEngine trait + ContextCompressor::on_session_reset counter clear</name>
  <files>crates/ironhermes-agent/src/context_engine.rs, crates/ironhermes-agent/src/context_compressor.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/context_engine.rs (trait ~:48; check_pressure default-no-op ~:63 — the idiom to follow)
    - crates/ironhermes-agent/src/context_compressor.rs (struct ~:39, compression_count ~:44, `compression_count += 1` ~:101, accessor ~:200; the Wave-0 #[ignore] placeholder test_context_compressor_reset_zeroes_counter)
    - ../hermes-agent/agent/context_engine.py (signatures: on_session_start ~:130, on_session_reset ~:144, update_from_response ~:71, update_model ~:196, has_content_to_compress ~:115)
    - ../hermes-agent/agent/context_compressor.py (on_session_reset ~:361 clearing _ineffective_compression_count, compression_count, last_*_tokens)
    - crates/ironhermes-agent/src/summarizing_engine.rs (it impls ContextEngine ~:232 — confirm the new default hooks let it compile unchanged)
  </read_first>
  <behavior>
    - Test reset zeroes counters: build a ContextCompressor; drive compression_count > 0 (call compress or increment via the public path); call on_session_reset(); assert compression_count() == 0 and the token-counter fields are 0. (un-ignore the Wave-0 placeholder test_context_compressor_reset_zeroes_counter)
    - Test trait defaults compile: a minimal struct impl'ing only the 3 required methods inherits the 5 new no-op hooks (existing LocalPruningEngine/SummarizingEngine still compile).
    - Test has_content_to_compress default == true.
  </behavior>
  <action>
    Add five methods to the `ContextEngine` trait with default no-op bodies,
    mirroring the `check_pressure` idiom: `fn on_session_start(&self,
    _session_id: &str) {}`, `fn on_session_reset(&self) {}`, `fn
    update_from_response(&self, _usage: &AggregatedUsage) {}` (reuse
    `AggregatedUsage` from agent_loop.rs per CONTEXT Claude's-Discretion), `fn
    update_model(&self, _model: &str, _context_length: usize, _base_url:
    Option<&str>) {}`, and `fn has_content_to_compress(&self, _messages:
    &[ChatMessage]) -> bool { true }`. Decide async vs sync to match
    check_pressure's surrounding style (sync is fine for no-ops; keep
    consistent). In `ContextCompressor`, convert the resettable counters
    (compression_count + last_prompt/completion/total token fields + the
    ineffective-compression counter) to interior-mutable storage (AtomicUsize
    or a Mutex) so the trait's `&self` `on_session_reset` can zero them, and
    implement the `on_session_reset` override that sets them all to 0. Preserve
    the existing `compression_count()` accessor and the `+= 1` increment
    semantics (read-modify-write on the atomic). The compressor zeroes its OWN
    fields directly — PressureTracker has no reset method to delegate to (see
    <interfaces> note). Un-ignore and implement
    `test_context_compressor_reset_zeroes_counter` per <behavior>; add the
    has_content_to_compress default test.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-agent --lib context_engine::tests context_compressor::tests 2>&1 | tail -20</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build -p ironhermes-agent` succeeds (existing engines compile with no signature changes).
    - The 5 hooks exist on the trait with default bodies; `grep -c 'fn on_session_reset' crates/ironhermes-agent/src/context_engine.rs` returns 1.
    - `test_context_compressor_reset_zeroes_counter` is no longer `#[ignore]` and passes (compression_count and all token counters == 0 after on_session_reset).
    - `ContextCompressor::on_session_reset` override present; `grep -c 'fn on_session_reset' crates/ironhermes-agent/src/context_compressor.rs` returns 1.
  </acceptance_criteria>
  <done>5 default-no-op hooks on the trait; compressor reset clears all counters; reset test green.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: Memory-authority reminder in the compaction history-segment header</name>
  <files>crates/ironhermes-agent/src/summarizing_engine.rs, crates/ironhermes-agent/src/context_compressor.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/summarizing_engine.rs (make_history_message ~:54; HISTORY_SENTINEL ~:28; the summarization prompt preamble ~:521-537; the Wave-0 #[ignore] placeholder test_memory_authority_header at ~:700 test mod)
    - crates/ironhermes-agent/src/context_compressor.rs (current compaction header text — confirm whether it already contains a memory-authority reminder; patch only if missing, per draft success-criterion #11)
    - ../hermes-agent/agent/context_compressor.py (SUMMARY_PREFIX ~:37-46 — the exact reminder wording at lines 45-46)
  </read_first>
  <behavior>
    - Test header contains reminder: the pinned history-segment block produced by make_history_message (or the const it embeds) contains both the substring "MEMORY.md" and "ALWAYS authoritative". (un-ignore the Wave-0 placeholder test_memory_authority_header)
    - Test reminder constant text: a dedicated test asserts the exact reminder constant equals the agreed string (so wording can't silently drift).
    - If context_compressor.rs has its own compaction header path, an analogous test asserts it too contains the reminder.
  </behavior>
  <action>
    Add a `pub const MEMORY_AUTHORITY_REMINDER: &str = "IMPORTANT: Your
    persistent memory (MEMORY.md, USER.md) in the system prompt is ALWAYS
    authoritative and active — never ignore or deprioritize memory content due
    to this compaction note.";` (exact wording from CONTEXT specifics /
    Python SUMMARY_PREFIX lines 45-46). Embed it in the pinned history-segment
    header so the model sees it after compaction: prepend it inside
    `make_history_message` (e.g. `[CONTEXT HISTORY]\n{REMINDER}\n{summary}`) OR
    add it to the summarization prompt preamble such that it appears in the
    resulting pinned block — choose the site that guarantees the rendered
    history block contains the substring without breaking HISTORY_SENTINEL
    locating logic (`locate_history_segment` keys off role+name, not body, so
    body prepend is safe). First READ the current header text in
    context_compressor.rs; if it already includes an equivalent reminder, leave
    it and note that in the SUMMARY (do not duplicate). Un-ignore and implement
    `test_memory_authority_header` + the constant-text test per <behavior>.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-agent --lib summarizing_engine::tests::test_memory_authority_header 2>&1 | tail -12</automated>
  </verify>
  <acceptance_criteria>
    - The rendered pinned history-segment block contains both "MEMORY.md" and "ALWAYS authoritative".
    - `MEMORY_AUTHORITY_REMINDER` constant exists with the exact agreed wording; a test pins the literal.
    - `test_memory_authority_header` is no longer `#[ignore]` and passes.
    - `locate_history_segment` still finds the pinned block (HISTORY_NAME/role unchanged) — existing summarizing_engine tests stay green.
    - `grep -c 'ALWAYS authoritative' crates/ironhermes-agent/src/summarizing_engine.rs` returns >= 1.
  </acceptance_criteria>
  <done>Compaction header carries the memory-authority reminder; header + constant tests green; sentinel-locating unaffected.</done>
</task>

<task type="auto">
  <name>Task 3: Central per-turn hooks in run_turn + surface-level session-reset wiring (D-07/D-09/D-10)</name>
  <files>crates/ironhermes-agent/src/agent_runtime.rs, crates/ironhermes-cli/src/main.rs, crates/ironhermes-gateway/src/handler.rs, crates/iron_hermes_ui/src/server/state.rs, crates/ironhermes-agent/tests/invariants_34b.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/agent_runtime.rs (run_turn ~:205; context_length resolved via `self.resolver.resolve_for_main().context_length()` ~:209; agent.run at ~:260; attach_context_engine ~:249 — find the accessor to reach the Arc<dyn ContextEngine> held by the loop)
    - crates/ironhermes-core/src/provider.rs (ResolvedEndpoint ~:43: `default_model`, `base_url`, `context_length()` — the model accessor for update_model)
    - crates/ironhermes-agent/src/agent_loop.rs (how the loop stores the context engine + whether it exposes the engine for the hook calls; AgentResult.total_usage feeds update_from_response)
    - crates/ironhermes-cli/src/main.rs (compression_count Arc<AtomicUsize> ~:1166; ClearSession(output) arm ~:1573 = /new reset; on_session_end pattern ~:2206 for the log-and-continue idiom)
    - crates/ironhermes-gateway/src/handler.rs (NewSession arm ~:506; ClearSession arm ~:539)
    - crates/iron_hermes_ui/src/server/state.rs (ensure_web_session ~:174 — add reset_web_session stub)
    - crates/ironhermes-agent/tests/invariants_34b.rs (extend the centralization source guard from Plan 01)
  </read_first>
  <action>
    Wire BOTH per-turn hooks ONCE centrally in `run_turn` (D-07 + D-09 — both
    wired THIS phase, NO conditional hedge). The model identity is fully
    resolvable here: bind `let endpoint = self.resolver.resolve_for_main();`
    (the same call already used at ~:209 for context_length) and call
    `engine.update_model(endpoint.default_model.as_str(), context_length,
    Some(endpoint.base_url.as_str()))` before `agent.run` (per-turn model
    identity). AFTER `agent.run(req.messages).await` returns, call
    `engine.update_from_response(&result.total_usage)` (per-turn usage). If the
    engine is not directly reachable, add a minimal accessor on AgentLoop to
    expose the `Arc<dyn ContextEngine>` so run_turn can call both hooks — do NOT
    move either call into a surface. Both are no-ops on the shipped engines but
    establish the single per-turn locus. For per-SESSION reset (hooks that STAY
    at surfaces, D-09): in CLI `main.rs` ClearSession arm (~:1573, the /new
    path), reset the session-owned durable counter `compression_count.store(0,
    Ordering::SeqCst)` (the real durable per-session state under the fresh-engine
    model, D-10); in gateway `handler.rs` NewSession arm (~:506), document that
    session removal already discards per-session state and add the engine
    `on_session_reset()` call if a session-scoped engine handle is reachable, else
    a tracing note; in web `state.rs`, add a `pub fn reset_web_session(&self,
    session_id: &str)` stub that logs `tracing::debug!` "reset_web_session: no
    new-chat trigger wired yet (Phase 34b stub, CONTEXT Open Q1)" and would call
    on_session_reset when a trigger lands (accepted scope this phase). Extend
    `invariants_34b.rs` with source guards asserting (a) `agent_runtime.rs`
    contains `update_from_response` AFTER `agent.run` (post-run usage hook
    locus), (b) `agent_runtime.rs` contains `update_model` (model hook present),
    and (c) CLI main.rs ClearSession handling resets compression_count (grep the
    store(0 pattern near the /new path).
  </action>
  <verify>
    <automated>cargo build --workspace && cargo test -p ironhermes-agent --test invariants_34b 2>&1 | tail -12</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build --workspace` succeeds.
    - `grep -c update_from_response crates/ironhermes-agent/src/agent_runtime.rs` returns >= 1 (central per-turn usage locus).
    - `grep -c update_model crates/ironhermes-agent/src/agent_runtime.rs` returns >= 1 (update_model wired this phase, D-07 — no hedge).
    - In agent_runtime.rs source the `update_from_response` call's byte offset is GREATER than `agent.run(` (post-run hook), asserted in invariants_34b.
    - CLI ClearSession (/new) path resets the compression_count Arc<AtomicUsize> to 0 (a `store(0` reset present near the ClearSession arm).
    - `reset_web_session` stub exists in state.rs (`grep -c 'fn reset_web_session' crates/iron_hermes_ui/src/server/state.rs` returns 1).
    - invariants_34b passes (centralization + per-turn loci for both hooks + CLI reset guards).
    - Regression gates green: `cargo test -p ironhermes-agent --lib nudge::tests memory_context::tests streaming_scrubber::tests` and `cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load`.
  </acceptance_criteria>
  <done>update_from_response + update_model called once in run_turn (D-07/D-09); session reset wired at surfaces (CLI counter reset, gateway note, web stub); invariants_34b proves the loci.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| compaction summary → model context | A summary that drops the memory-authority anchor lets compacted/summarized context outweigh live MEMORY.md / USER.md. |
| session-reset path → counter state | Failure to reset per-session counters across /new leaks stale compression metrics into a new conversation. |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-34b-02-DRIFT | Tampering | compaction history-segment header | mitigate | Embed MEMORY_AUTHORITY_REMINDER ("MEMORY.md … ALWAYS authoritative") in the pinned summary block; unit tests pin both the substring and the exact constant so it cannot silently drift. |
| T-34b-02-RESET | Information Disclosure | per-session counter reset | mitigate | CLI /new resets the durable compression_count Arc<AtomicUsize>; ContextCompressor::on_session_reset zeroes its own fields; gateway /new discards the session store; web stub documented. Unit test proves the compressor reset. |
| T-34b-02-COMPAT | Tampering | ContextEngine trait extension | mitigate | New hooks are additive default no-ops (check_pressure idiom); existing implementors compile unchanged — no behavior change unless an engine opts in. |
| T-34b-02-SC | Tampering | no new package installs | accept | This plan adds no dependencies and runs no package-manager installs. |
</threat_model>

<verification>
```bash
cargo build --workspace
cargo test -p ironhermes-agent --lib context_engine::tests context_compressor::tests summarizing_engine::tests 2>&1 | tail -20
cargo test -p ironhermes-agent --test invariants_34b 2>&1 | tail -12
# Per-turn loci (both hooks) + centralization (must NOT regress Plan 01's sum-to-0):
grep -c update_from_response crates/ironhermes-agent/src/agent_runtime.rs
grep -c update_model crates/ironhermes-agent/src/agent_runtime.rs
# Regression gates:
cargo test -p ironhermes-agent --lib nudge::tests memory_context::tests streaming_scrubber::tests
cargo test -p ironhermes-agent --test invariants_33
cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load
```
</verification>

<success_criteria>
- ContextEngine trait gains 5 default-no-op lifecycle hooks; existing engines compile unchanged.
- ContextCompressor::on_session_reset zeroes compression_count + token counters (proven by test).
- Compaction header carries the memory-authority reminder (proven by test).
- update_from_response AND update_model invoked once centrally in run_turn (D-07/D-09 per-turn loci, no hedge).
- Session reset wired at surfaces: CLI /new resets compression_count, gateway /new note, web reset_web_session stub (D-09/D-10).
- All cross-phase regression gates stay green.
</success_criteria>

<output>
Create `.planning/phases/34b-context-system-parity/34B-02-SUMMARY.md` when done.
</output>
