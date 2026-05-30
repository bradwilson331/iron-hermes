# IronHermes — Project Learnings

Cross-phase patterns, meta-rules, and decisions worth keeping. Each entry cites the phase(s) it came from. New entries go at the top; entries are not deleted, only marked superseded.

---

## 2026-05-30 — Bilateral-tracing-by-construction validated at scale

**Source:** Phase 36.3.7.5 (gateway notifier — auto-subscribe + polling loop + 3 notify-* CLI verbs)

**Pattern observed:** First at-scale application of the 2026-05-29 bilateral-tracing rule. Phase 36.3.7.5 was structured "by construction" — every BUG row in the phase plan paired a producer site with a named consumer test, and plan execution shipped both ends in the same commit set. 4 plans, 22 commits, 25 new tests, **zero receiver-end bugs surfaced by gsd-verifier or any of the 13 phase-level gates**. Compare to Phases 36.3.7 + 36.3.7.0 where producer-only verification left 5 live UAT bugs.

**What the planner did differently:**
- Frontmatter `must_haves.key_links` listed every producer→consumer linkage with a grep-able `pattern` field.
- The phase PLAN.md "Gate 9: Bilateral-tracing self-audit" table enumerated all 7 BUGs with both endpoints named *before* execution started, so each plan's SUMMARY just had to fill in evidence rows.
- The verifier prompt used the literal Gate 9 table as a template — no room for "I checked the producer" pass-throughs.

**Reusable structural patterns surfaced (worth lifting to future phases):**

1. **Store-arc lift refactor** (Plan 03): when two spawn blocks need the same `Arc<TokioMutex<KanbanStore>>`, hoist `KanbanStore::open_default()` ABOVE both gating checks rather than duplicating the open. The dispatcher branch and the notifier branch share the same Arc via `.clone()`. Semantics-preserving (cargo test -p ironhermes-kanban delta = 0); structural lift only. Lift+share pattern applies whenever a runner gains a second consumer of a shared resource.

2. **Pure-function gate extracted to its own module** (Plan 03): `notifier_gating.rs` contains only `pub fn compute_notifier_gate(notification_sources, enabled_platforms) -> NotifierGate`. Pure, testable, reachable from integration tests without `#[cfg(test)]` re-exports. The "Option 1" pattern in the Plan 03 task description — extract any pure decision function with a small enum return type into a sibling module, then `use` it from the runner. Beats both `pub(crate)` (untestable from integration crate) and `pub` at lib.rs root (untyped surface area).

3. **Trait-object closure as cross-crate boundary** (Plan 02 + Plan 03): `SendFn = Arc<dyn Fn(&str, &str, Option<&str>, &str) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>`. The kanban crate publishes the alias; the gateway crate constructs the closure at spawn time, capturing its `Arc<dyn PlatformAdapter>` set. **Zero compile-time dep on the gateway crate from kanban** (`grep -E '^ironhermes-gateway\s*=' crates/ironhermes-kanban/Cargo.toml` == 0). Same shape mirrors the dispatcher's existing `spawn_fn` injection — when adding a new spawn-time injection point, look for the nearest existing one and copy its shape.

4. **NEW write trait sibling to existing read trait** (Plan 04): `KanbanStoreReader` already existed as a read-only abstraction. Plan 04 introduced `KanbanStoreWriter` as its sibling — additive, forward-compatible for future `/kanban comment`, `/kanban complete` arms without touching the reader. When extending command surface, sibling-trait extension beats expanding the existing trait (no churn for unaffected callers).

5. **Conditional-gate task** (Plan 01 was the model, Plan 02 reused): a "Task N" in a plan can be a read-only verification step whose outcome turns "Task N+1" into a no-op if pre-existing code already satisfies the contract. Phase 36.3.7.4 used this to skip its producer fix; Phase 36.3.7.5 used variants for several "extend existing" tasks. Pattern: read-first, fix-only-if-needed, document either way.

**Forward note flagged during Plan 03 verification:** `build_notifier_send_fn` only retains the Telegram `Arc<dyn PlatformAdapter>` at runner scope — Discord and Slack adapters live inside their own spawned tasks (Discord wraps a Serenity Context post-handshake; Slack constructs inside its socket-mode runner). Subscriptions naming `discord` or `slack` fall through to the closure's `"platform X not enabled in gateway"` branch, which matches the locked `D-log-and-drop-on-fail` policy. This is a **documented forward-compat gap**, NOT a verification failure. Future phase can hoist adapter constructions out of their tasks OR add a delivery-dispatch indirection — both forward-compatible refactors. Capture this as the kickoff context for whichever phase first needs cross-platform notifier delivery.

**Counter-pattern to avoid (from this phase's anomaly log):** when crafting verifier prompts, do NOT hardcode counter deltas (`total_plans 78→82`) — let the verifier compute live deltas from STATE.md frontmatter. Pre-computed deltas in the prompt drift relative to ground truth (verifier had to override mine; the real delta was phases-only `17→18` because plans were already individually closed). Rule: prompts encode policy + structure, not numbers.

**Operational takeaway:** when a phase is structured for bilateral-tracing-by-construction (every BUG paired with a named consumer test before plan execution starts), the verification phase is fast and gates rarely surprise. The expensive work is at PLAN time, not VERIFY time. Future phases that touch wire-ups (gateway/notifier/dispatcher/handler/CLI surface) SHOULD adopt this structure.

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
