---
phase: 34b-context-system-parity
plan: 03
type: execute
wave: 3
depends_on: [34b-02]
files_modified:
  - crates/ironhermes-agent/src/context_refs.rs
  - crates/ironhermes-agent/src/agent_loop.rs
  - crates/ironhermes-agent/src/agent_runtime.rs
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-gateway/src/handler.rs
  - crates/iron_hermes_ui/src/server/state.rs
  - crates/ironhermes-agent/tests/invariants_34b.rs
autonomous: true
requirements: [CTX-REF-02, CTX-REF-03]
must_haves:
  truths:
    - "WR-01 closed: each of the three production surfaces (CLI run_single + run_chat_turn, gateway run_agent, web run_web_turn) reads `result.context_warnings` and renders the `--- Context Warnings ---` block out-of-band (separate from the model response text)"
    - "No double-render: the `--- Context Warnings ---` block is NOT also embedded into the message text sent to the model — `preprocess_context_references_async` stops appending the warnings block to `final_msg` (the `--- Attached Context ---` block stays in-message; only the warnings block moves out-of-band)"
    - "The `--- Attached Context ---` injected-context block continues to be embedded in the message text (the model still needs it) — only the warnings channel changed"
    - "Doc comments at agent_loop.rs context_warnings field and agent_runtime.rs run_turn no longer promise behavior that does not exist — they describe the actual out-of-band-surface-rendering path that this plan wires"
    - "A source-guard test in invariants_34b proves each surface file references `context_warnings`, and a context_refs unit test proves the warnings block is no longer in the returned `message` while warnings remain in the `warnings` Vec"
  artifacts:
    - path: crates/ironhermes-agent/src/context_refs.rs
      provides: "preprocess_context_references_async no longer embeds the --- Context Warnings --- block into final_msg; warnings remain on ContextReferenceResult.warnings"
      contains: "--- Attached Context ---"
    - path: crates/ironhermes-cli/src/main.rs
      provides: "out-of-band Context Warnings rendering at both run_single and run_chat_turn after run_turn returns"
      contains: "context_warnings"
    - path: crates/ironhermes-gateway/src/handler.rs
      provides: "out-of-band Context Warnings rendering in run_agent after run_turn returns"
      contains: "context_warnings"
    - path: crates/iron_hermes_ui/src/server/state.rs
      provides: "run_web_turn surfaces context_warnings out-of-band (streamed annotation / log) before returning AgentResult"
      contains: "context_warnings"
    - path: crates/ironhermes-agent/tests/invariants_34b.rs
      provides: "source-guard asserting all three surface files reference context_warnings"
      contains: "context_warnings"
  key_links:
    - from: crates/ironhermes-cli/src/main.rs
      to: crates/ironhermes-agent/src/agent_loop.rs
      via: "result.context_warnings read after runtime.run_turn returns, rendered out-of-band"
      pattern: "context_warnings"
    - from: crates/ironhermes-gateway/src/handler.rs
      to: crates/ironhermes-agent/src/agent_loop.rs
      via: "result.context_warnings read in run_agent Ok arm, sent via adapter.send_message out-of-band"
      pattern: "context_warnings"
    - from: crates/iron_hermes_ui/src/server/state.rs
      to: crates/ironhermes-agent/src/agent_loop.rs
      via: "result.context_warnings read in run_web_turn before returning"
      pattern: "context_warnings"
---

<objective>
Close WR-01: wire each of the three production surfaces to consume
`AgentResult.context_warnings` and render the `--- Context Warnings ---` block
**out-of-band** (separate from the agent's response text), so the field
populated by `run_turn` (agent_runtime.rs:379) is actually used as the
architecture intended. Today the field is dead — `grep context_warnings`
returns 0 matches in cli/main.rs, gateway/handler.rs, and ui/server/state.rs.

DUPLICATION DECISION (option a — stop in-message embedding, rely on out-of-band):
Investigation of `preprocess_context_references_async`
(context_refs.rs:876-889) confirms warnings are currently appended into
`final_msg` — the message **text that is sent to the model as input**, not the
user-facing output. This is the wrong channel: it spends prompt tokens on
operational metadata and does NOT reliably reach the user (the model decides
whether to echo it). This plan REMOVES the in-message warnings append (the
`--- Context Warnings ---` push_str at context_refs.rs:880-884) and routes
warnings exclusively through the out-of-band `context_warnings` field that each
surface now renders after `run_turn` returns. The legitimate
`--- Attached Context ---` injected-context block STAYS in-message (the model
needs that content) — only the warnings channel moves. This eliminates
double-render by construction: warnings live in exactly one place
(`AgentResult.context_warnings`), surfaces render them once.

Also corrects the misleading doc comments at agent_loop.rs:71-76 and
agent_runtime.rs:369-370, which currently promise out-of-band surface rendering
that did not exist until this plan.

Output: warnings-block removed from in-message embedding; CLI/gateway/web each
read and render `context_warnings` out-of-band; doc comments corrected;
source-guard + unit tests proving the new contract.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/STATE.md
@.planning/phases/34b-context-system-parity/34B-CONTEXT.md
@.planning/phases/34b-context-system-parity/34b-VERIFICATION.md
@.planning/phases/34b-context-system-parity/34b-HUMAN-UAT.md
@.planning/phases/34b-context-system-parity/34b-REVIEW.md

<interfaces>
From crates/ironhermes-agent/src/agent_loop.rs — the carrier field (line 76):
```rust
/// Phase 34b Plan 01 (D-11): warnings produced by `@`-reference expansion ...
pub context_warnings: Vec<String>,
```
Initialized to `Vec::new()` in `budget_exhausted` (line 95) and all 4
`Ok(AgentResult{..})` construction sites (lines 910, 940, 1059, 1193). The doc
comment (lines 71-76) currently claims surfaces render it "without per-surface
preprocessing" — that promise is what this plan makes true.

From crates/ironhermes-agent/src/agent_runtime.rs — run_turn populates the field
at line 379 (`out.context_warnings = context_warnings;`) after building
`context_warnings` from the preprocess step (line 221) and after `agent.run`
(line 371). The doc comment at lines 369-370 describes the intended surfacing.

From crates/ironhermes-agent/src/context_refs.rs — `preprocess_context_references_async`
(line 791) builds `final_msg`. Today it appends BOTH blocks:
- warnings block at lines 880-884 (`final_msg.push_str("\n\n--- Context Warnings ---\n")` + the per-warning lines) — THIS is what gets removed.
- attached-context block at lines 886-889 (`--- Attached Context ---`) — STAYS.
`ContextReferenceResult.warnings: Vec<String>` (line 171) is the data that
flows up to `AgentResult.context_warnings`; it is unaffected by removing the
in-message embedding (warnings still populate the Vec, just not the message text).

Surface result-consumption loci (read result; render warnings after these):
- CLI: crates/ironhermes-cli/src/main.rs — `let result = runtime.run_turn(request).await?;`
  at line 850 (run_single / one-shot send path) AND line 2320 (run_chat_turn /
  interactive REPL path). run_chat_turn returns `Ok(result.final_response)` at
  line 2358; streamed tokens land via `write_into_scroll_region` (imported line 4).
  run_single flushes via `print!`/stdout.
- Gateway: crates/ironhermes-gateway/src/handler.rs — `rt.run_turn(request).await`
  at line 1041; the `Ok(result)` arm starts at line 1062-1063; user-facing text is
  delivered via `adapter.send_message(&event.chat_id, ..., None)` (the same call
  used throughout, e.g. lines 502, 548, 1151) and streamed via the StreamConsumer.
- Web: crates/iron_hermes_ui/src/server/state.rs — `let result = self.runtime.run_turn(request).await?;`
  at line 238 inside `run_web_turn` (returns `Result<AgentResult>` per signature
  at line 215); `stream_callback` is the StreamCallback passed in at line 212.

invariants idiom: crates/ironhermes-agent/tests/invariants_34b.rs uses
`include_str!` on each surface file (RUNTIME_SOURCE, HANDLER_SOURCE, STATE_SOURCE,
MAIN_SOURCE at lines 15-26) and asserts `.contains(token)` / byte-offset ordering.
Extend with a guard that each surface source contains "context_warnings".
</interfaces>
</context>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Stop in-message warnings embedding + correct doc comments</name>
  <files>crates/ironhermes-agent/src/context_refs.rs, crates/ironhermes-agent/src/agent_loop.rs, crates/ironhermes-agent/src/agent_runtime.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/context_refs.rs (preprocess_context_references_async ~:791; the warnings push_str at :880-884; the attached-context push_str at :886-889; the `expanded` computation at :891; the early-return ContextReferenceResult sites at :800-808 and :858-866; the existing tests at :1055-1270 — especially test_soft_limit_warning ~:1218 and test_hard_limit_blocked ~:1190 which assert on `result.warnings`)
    - crates/ironhermes-agent/src/agent_loop.rs (context_warnings field + doc comment :71-76)
    - crates/ironhermes-agent/src/agent_runtime.rs (run_turn warning population :221, :369-379; doc comment :369-370)
  </read_first>
  <behavior>
    - Test warnings no longer in message text: build a `@`-reference input that produces a soft-limit warning (reuse the existing soft-limit test fixture path), call `preprocess_context_references_async`, assert `result.warnings` is non-empty AND `result.message` does NOT contain "--- Context Warnings ---".
    - Test attached-context still embedded: a successful `@file:` expansion still yields `result.message` containing "--- Attached Context ---" (existing test_expand_file-style assertion stays green).
    - Test warnings preserved on the result: the same warning strings that were previously embedded are still present on `result.warnings` (so the out-of-band path has the data).
  </behavior>
  <action>
    In `preprocess_context_references_async` (context_refs.rs), DELETE the
    in-message warnings embedding: remove the `if !warnings.is_empty() { ... }`
    block at lines 880-884 that does `final_msg.push_str("\n\n--- Context
    Warnings ---\n")` plus the per-warning `warning_lines.join`. Keep the
    `--- Attached Context ---` block (lines 886-889) intact. Keep populating the
    `warnings` Vec and returning it on `ContextReferenceResult.warnings`
    unchanged (the early-return blocked/soft paths already push to `warnings` —
    leave those pushes; only the message-text embedding is removed). Re-check the
    `expanded` flag at line 891 (`!blocks.is_empty() || !warnings.is_empty()`):
    keep it as-is so a warnings-only result still reports `expanded: true` (the
    surface still needs to render the out-of-band warnings). Update any existing
    in-file test that asserted the warnings substring appears in
    `result.message` to instead assert it appears in `result.warnings` (search
    the test mod for `--- Context Warnings ---` and for `.message` assertions
    near the soft/hard-limit tests). Then correct the doc comments: in
    agent_loop.rs (:71-76) reword the `context_warnings` doc to state plainly
    that the field is the OUT-OF-BAND warnings carrier, rendered by each surface
    after `run_turn` returns (no longer embedded in the message text); in
    agent_runtime.rs (:369-370) reword the inline comment to describe that the
    field is populated for surface-side out-of-band rendering (drop any wording
    implying the surfacing already happens automatically). Add the new
    context_refs unit test from <behavior> (warnings-not-in-message +
    warnings-preserved-on-result).
  </action>
  <verify>
    <automated>cargo test -p ironhermes-agent --lib context_refs::tests 2>&1 | tail -20</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build -p ironhermes-agent` succeeds.
    - `grep -c 'final_msg.push_str("\\n\\n--- Context Warnings ---' crates/ironhermes-agent/src/context_refs.rs` returns 0 (in-message warnings embedding removed).
    - `grep -c '\-\-\- Attached Context \-\-\-' crates/ironhermes-agent/src/context_refs.rs` returns >= 1 (attached-context embedding retained, excluding test-only matches is acceptable as long as the production push_str remains).
    - The new unit test asserts `result.message` does NOT contain "--- Context Warnings ---" while `result.warnings` is non-empty; it passes.
    - `cargo test -p ironhermes-agent --lib context_refs::tests` passes (all 17 prior tests + the new one; any test that previously asserted the warnings substring in `result.message` is updated to assert on `result.warnings`).
    - The agent_loop.rs context_warnings doc comment no longer contains the phrase "without per-surface preprocessing"; `grep -c 'without per-surface preprocessing' crates/ironhermes-agent/src/agent_loop.rs` returns 0.
  </acceptance_criteria>
  <done>Warnings block removed from in-message embedding; attached-context retained; warnings still on result.warnings; doc comments corrected; context_refs tests green.</done>
</task>

<task type="auto">
  <name>Task 2: Render context_warnings out-of-band at all three surfaces</name>
  <files>crates/ironhermes-cli/src/main.rs, crates/ironhermes-gateway/src/handler.rs, crates/iron_hermes_ui/src/server/state.rs</files>
  <read_first>
    - crates/ironhermes-cli/src/main.rs (run_single result consume :850; scrubber tail flush :852-858; run_chat_turn result consume :2320; final_response return :2358; write_into_scroll_region import :4 and usage :2325; the println!/print! idioms around :836,:856,:922)
    - crates/ironhermes-gateway/src/handler.rs (run_turn call :1041; Ok(result) arm :1062-1145; adapter.send_message signature usage :502,:548,:1151; the Err arm error_suffix send :1147-1153 as the out-of-band-append idiom to mirror)
    - crates/iron_hermes_ui/src/server/state.rs (run_web_turn :209-238; stream_callback param :212; result returned after appended persistence ~:241-246; the function returns Result<AgentResult>)
    - crates/ironhermes-agent/src/agent_loop.rs (AgentResult.context_warnings field :76 — the data to read)
  </read_first>
  <action>
    Render `result.context_warnings` out-of-band at each surface, AFTER
    `run_turn` returns, only when the Vec is non-empty, formatted as a
    `--- Context Warnings ---` header followed by one `- {warning}` line per
    entry (mirror the prior in-message format so users see the same text, just
    in a distinct channel). CLI run_single (main.rs ~:850): after the scrubber
    tail flush (~:858), if `!result.context_warnings.is_empty()`, build the
    block and `print!`/`println!` it to stdout (one-shot path, plain stdout is
    the out-of-band channel). CLI run_chat_turn (main.rs ~:2320): after the
    scrubber tail flush (~:2326) and before `Ok(result.final_response)` (:2358),
    if `!result.context_warnings.is_empty()`, render the block via
    `write_into_scroll_region(block.as_bytes(), tui.reserved_row_count())` so it
    lands in the scroll region like streamed output (do NOT fold it into
    final_response — keep it a separate write so it is visibly out-of-band).
    Gateway run_agent (handler.rs Ok arm ~:1062): after the response handling in
    the Ok arm, if `!result.context_warnings.is_empty()`, send the formatted
    block as a SEPARATE message via
    `adapter.send_message(&event.chat_id, &warnings_block, None)` (mirror the Err
    arm's `error_suffix` send idiom at :1147-1153 — a distinct message, not
    appended to the streamed response). Web run_web_turn (state.rs ~:238): after
    persisting `result.appended` (~:243) and before returning, if
    `!result.context_warnings.is_empty()`, emit the block out-of-band — invoke
    the `stream_callback` with the formatted warnings block (so the web client
    receives it as a distinct streamed annotation) AND `tracing::warn!` it for
    server-side visibility; the web client has no separate annotation channel
    yet, so streaming it after the response with the `--- Context Warnings ---`
    header is the accepted out-of-band rendering for this surface. Do NOT alter
    `run_turn` or re-add any in-message embedding. Read `context_warnings` only;
    do not mutate it.
  </action>
  <verify>
    <automated>cargo build --workspace 2>&1 | tail -20</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build --workspace` succeeds.
    - `grep -c context_warnings crates/ironhermes-cli/src/main.rs` returns >= 2 (run_single + run_chat_turn both consume it).
    - `grep -c context_warnings crates/ironhermes-gateway/src/handler.rs` returns >= 1.
    - `grep -c context_warnings crates/iron_hermes_ui/src/server/state.rs` returns >= 1.
    - Each surface renders the block only when `context_warnings` is non-empty (guarded by an `is_empty()` check — `grep -c 'context_warnings.is_empty()' ` across the three files returns >= 3 total, or an equivalent non-empty guard is present in each).
    - The rendered block uses the `--- Context Warnings ---` header in each surface (`grep -c 'Context Warnings' ` returns >= 1 in each of the three surface files).
  </acceptance_criteria>
  <done>All three surfaces read result.context_warnings and render the --- Context Warnings --- block out-of-band, guarded by a non-empty check; workspace builds.</done>
</task>

<task type="auto">
  <name>Task 3: Source-guard test proving each surface consumes context_warnings</name>
  <files>crates/ironhermes-agent/tests/invariants_34b.rs</files>
  <read_first>
    - crates/ironhermes-agent/tests/invariants_34b.rs (include_str! consts :15-26 — RUNTIME_SOURCE, HANDLER_SOURCE, STATE_SOURCE, MAIN_SOURCE; the existing .contains-based guards :52-71 and byte-offset guards :86-127 — the idiom to extend)
    - crates/ironhermes-agent/src/context_refs.rs (confirm the in-message warnings embedding is gone — the guard below asserts production source no longer pushes the warnings header into final_msg)
  </read_first>
  <action>
    Add source-guard tests to invariants_34b.rs following the existing
    `include_str!` + `.contains` idiom. (1) Add a const
    `CONTEXT_REFS_SOURCE = include_str!("../src/context_refs.rs")`. (2) Add a test
    `surfaces_consume_context_warnings` asserting `MAIN_SOURCE`, `HANDLER_SOURCE`,
    and `STATE_SOURCE` each `.contains("context_warnings")` (WR-01 closure: every
    surface reads the field). (3) Add a test
    `warnings_not_embedded_in_message_text` asserting `CONTEXT_REFS_SOURCE` does
    NOT contain the in-message embedding marker
    `final_msg.push_str("\n\n--- Context Warnings ---` (proving the warnings
    block is no longer pushed into the model-bound message text) while it DOES
    still contain `--- Attached Context ---` (proving attached context embedding
    is retained). Use clear assertion messages naming WR-01 and the
    no-double-render contract. Do not weaken the existing five guards.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-agent --test invariants_34b 2>&1 | tail -16</automated>
  </verify>
  <acceptance_criteria>
    - `cargo test -p ironhermes-agent --test invariants_34b` passes with the new tests included (>= 7 tests total: the original 5 plus the 2 new ones).
    - `surfaces_consume_context_warnings` asserts all three surface sources contain "context_warnings" and passes.
    - `warnings_not_embedded_in_message_text` asserts context_refs.rs no longer pushes the warnings header into final_msg and still embeds attached context; it passes.
    - The original 5 invariants_34b tests still pass (no regression).
  </acceptance_criteria>
  <done>invariants_34b proves each surface reads context_warnings and that the warnings block is no longer embedded in the message text; full suite green.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| `@`-ref warnings → model prompt | Today operational warnings are injected into the model's input message text, spending prompt tokens on metadata and exposing internal blocklist/budget detail inside the reasoning context. Moving them out-of-band removes that input-side surface. |
| `context_warnings` → user-facing surface | Warnings cross from agent result into CLI stdout / gateway chat message / web stream. The text is agent-generated operational metadata (blocklist hits, budget violations), not untrusted external input, so rendering as plain text is safe. |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-34b-03-DUP | Tampering | warnings rendering channel | mitigate | Single source of truth: warnings live only on `AgentResult.context_warnings`; the in-message embedding is removed (Task 1) so no surface can double-render. invariants_34b `warnings_not_embedded_in_message_text` pins this. |
| T-34b-03-DOC | Repudiation | doc/behavior mismatch (WR-01) | mitigate | Doc comments at agent_loop.rs:71-76 and agent_runtime.rs:369-370 corrected to describe the actual out-of-band path; source-guard `surfaces_consume_context_warnings` proves the behavior matches the docs. |
| T-34b-03-INJ | Information Disclosure | warnings text content | accept | Warning strings are agent-generated (blocklist/budget messages from context_refs.rs), contain only the offending `@`-token and limit math, no secret file contents (blocked files are never read). Rendering as plain text is low risk. |
| T-34b-03-SC | Tampering | no new package installs | accept | This plan adds no dependencies and runs no package-manager installs (no `## Package Legitimacy Audit` needed). |
</threat_model>

<verification>
```bash
# Task 1: in-message embedding removed, attached-context retained, warnings on result
cargo test -p ironhermes-agent --lib context_refs::tests 2>&1 | tail -20
grep -c 'final_msg.push_str("\n\n--- Context Warnings ---' crates/ironhermes-agent/src/context_refs.rs   # expect 0
grep -c 'without per-surface preprocessing' crates/ironhermes-agent/src/agent_loop.rs                     # expect 0

# Task 2: each surface consumes context_warnings out-of-band
cargo build --workspace 2>&1 | tail -20
grep -c context_warnings crates/ironhermes-cli/src/main.rs        # expect >= 2
grep -c context_warnings crates/ironhermes-gateway/src/handler.rs # expect >= 1
grep -c context_warnings crates/iron_hermes_ui/src/server/state.rs # expect >= 1

# Task 3: source-guard suite
cargo test -p ironhermes-agent --test invariants_34b 2>&1 | tail -16

# Regression gates (must stay green — per CONTEXT canonical_refs):
cargo test -p ironhermes-agent --lib nudge::tests memory_context::tests streaming_scrubber::tests
cargo test -p ironhermes-agent --test invariants_33
cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load
```
</verification>

<success_criteria>
- The `--- Context Warnings ---` block is no longer embedded into the model-bound message text; the `--- Attached Context ---` block remains embedded.
- CLI (run_single + run_chat_turn), gateway (run_agent), and web (run_web_turn) each read `result.context_warnings` and render the warnings block out-of-band, guarded by a non-empty check.
- Doc comments at agent_loop.rs:71-76 and agent_runtime.rs:369-370 describe the actual out-of-band surface-rendering path (WR-01 doc/behavior mismatch resolved).
- invariants_34b proves all three surfaces reference `context_warnings` and that the warnings block is no longer in the message text (no double-render).
- All cross-phase regression gates stay green.
</success_criteria>

<output>
Create `.planning/phases/34b-context-system-parity/34B-03-SUMMARY.md` when done.
</output>
