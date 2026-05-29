# IronHermes — Project Learnings

Cross-phase patterns, meta-rules, and decisions worth keeping. Each entry cites the phase(s) it came from. New entries go at the top; entries are not deleted, only marked superseded.

---

## 2026-05-29 — Bilateral-tracing rule for wire-up verification

**Source:** Phase 36.3.7 + Phase 36.3.7.0

**Pattern observed:** Across two phases, the gsd-verifier returned 17/17 PASS verdicts, but live UAT then surfaced **5 receiver-end bugs** the verifier missed. Every one followed the same shape: the producer side of a wire-up was traced and confirmed; the consumer/dispatcher side was assumed wired and never traced.

| Bug | Producer (verifier ✓) | Consumer (verifier ✗ missed) |
|---|---|---|
| BUG-36.3.7-01 | `worker_spawn.rs` emits `--skills` argv | `Cli` struct has no `--skills` flag — worker crashes at argparse |
| BUG-36.3.7-02 | `CommandDef::new("kanban", ...)` registered + bypass list entry | `handlers.rs::dispatch` has no `"kanban" =>` arm — falls through to `todo_stub` |
| BUG-36.3.7-03 | `consecutive_failures` bump in `detect_crashed_workers` | `apply_circuit_breaker` only called from spawn-failure path — counter incremented but never acted on for crashed workers |
| BUG-36.3.7-04 | Plan 01 added `chat -q` flag + `run_single` short-circuit | Preflight gate at `main.rs:402` not updated — wizard tries to read stdin, dies with `EOF on stdin` |
| BUG-#5 (delegate_task schema) | Schema-level `oneOf` enforces mutual-exclusion at runtime | Anthropic's tool API rejects top-level `oneOf` — every Anthropic-routed worker crashes |

**Rule:** For every wire-up claim in a plan, CONTEXT, or verification check, both ends MUST be cited:

> **PRODUCER**: where does the value get emitted / registered / incremented / added?
> **CONSUMER**: who reads it / accepts it / dispatches on it / acts on it?

A producer-only verdict is NOT acceptable. Stop at the producer only when the consumer is **explicitly out-of-scope** (and document the reason).

**Concrete recipes** (use these when generating verifier prompts or plan-check prompts):

- "X argv is constructed" → ALSO require: "X argv is accepted by the binary's `Cli` parser"
- "X command is registered" → ALSO require: "X command has a dispatch arm in the handler match"
- "X event bumps Y counter" → ALSO require: "Y counter is read AND acted on by the consumer step in the same code path"
- "X flag added to enum" → ALSO require: "ALL sibling gates that filter on the same predicate set are updated"
- "X tool schema defined" → ALSO require: "EVERY provider in the active routing layer accepts this schema"

**Structural mitigation that worked (Phase 36.3.7.0 Plan 02):** the 24 deferred kanban subverbs route to a dedicated `deferred_subverb_message(name)` function, NOT the generic `todo_stub`. This makes the deferred state **greppable** for future verifiers. Apply the same pattern wherever a catch-all hides receiver-end gaps from grep-based verification.

---

## 2026-05-29 — UAT is where the verifier's blind spots become visible

**Source:** Phase 36.3.7 close-out + Phase 36.3.7.0 entirely

**Pattern:** Automated test surfaces + verifier scoring caught everything they were designed to catch. Live UAT — running the actual binary end-to-end — surfaced bugs the automated surface couldn't see by design:

- Unit tests mock the spawn function; UAT runs `Command::new(...)`.
- Integration tests build the binary once and reuse it; UAT discovers PATH-resolution mismatches (debug vs release vs `~/.local/bin/ironhermes` symlinks).
- Verifier traces source symbols; UAT exercises the actual runtime stack including provider-side validation (Anthropic schema rejection).

**Rule:** Every phase that introduces a runtime-observable behavior MUST have a live-UAT case alongside the automated tests. UAT cases should be runnable from a documented procedure with PASS/FAIL signals — not just descriptive. See Plan 09 of Phase 36.3.7 for the canonical UAT-09-A / UAT-09-B template.

**Operational note:** UAT can be expensive (real LLM tokens, real subprocess spawn, manual operator steps). Budget for it explicitly. The Phase 36.3.7 → 36.3.7.0 cascade burned ~3 hours of live UAT iteration to surface 4 bugs the automated surface missed; if those bugs had reached production they would have produced silently-broken kanban workers.

---

## 2026-05-29 — PATH-resolved binaries are a deployment seam

**Source:** Phase 36.3.7.0 UAT-09-A re-run #2

**Pattern observed:** The kanban dispatcher spawns workers via `Command::new("ironhermes")`. PATH lookup picked up `~/.local/bin/ironhermes` (a symlink to the **release** binary built before any of the 36.3.7.0 fixes). The newly-built debug binary at `target/debug/ironhermes` was invisible to PATH. The worker subprocess crashed with the pre-fix error message even though the orchestrator's own kanban CLI was the post-fix debug binary.

**Rule:** When the dispatcher / orchestrator and the spawned subprocess use the SAME binary name but resolve to different binaries (PATH vs explicit path), every "did you rebuild?" question has at least two answers. Either:
- (a) Build BOTH `--release` and `--debug` after every fix, OR
- (b) Make the worker spawn path explicit (`Command::new(env::current_exe())`), OR
- (c) Document the PATH-resolution behavior so operators know which binary they're testing.

For 36.3.7.0 we chose (a) — rebuild release before each UAT-09-A iteration. For future production deployments, (b) is the safer default — workers should always be the same binary that spawned them.

---

## 2026-05-29 — Receiver-end bugs come in batches per UAT round-trip

**Source:** Phase 36.3.7.0 UAT-09-A runs #1 through #5

**Pattern:** Each successful UAT-09-A run-trip revealed exactly ONE new bug — the next receiver-end gap in the chain — and stopped there. Once that bug was fixed, the next run-trip exposed the next gap. The chain was:

1. Run #1: `--skills` argparse → fix BUG-36.3.7-01 (Plan 01)
2. Run #2: PATH binary mismatch → rebuild release
3. Run #3: `chat -q` preflight gate → fix BUG-36.3.7-04 (Plan 05, inline)
4. Run #4: profile `.env` missing → operator symlink workaround
5. Run #5: Anthropic schema rejection → Bug #5 documented, out-of-scope

**Rule:** Don't budget for a single UAT-fix cycle. Receiver-end bugs hide each other — the earliest failure short-circuits all subsequent ones. Budget for **N round-trips** where N is at least the number of independent receiver-end gates in the runtime stack. For the kanban worker path that was 4-5.

**Operational mitigation:** When opening a UAT-driven phase, write the procedure FIRST (before the fix) and run it as the planning input. The CONTEXT.md for Phase 36.3.7.0 had 3 named bugs; UAT surfaced a 4th + a 5th. If we'd run UAT-09-A as a planning input, the phase scope would have included all 4 in-scope bugs from the start.
