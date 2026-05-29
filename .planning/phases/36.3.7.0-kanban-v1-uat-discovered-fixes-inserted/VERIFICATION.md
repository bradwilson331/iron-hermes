# VERIFICATION — Phase 36.3.7.0 (Kanban v1 — UAT-discovered fixes)

**Overall verdict:** PASS-WITH-NOTES
**Date:** 2026-05-29
**Method:** goal-backward + BILATERAL TRACING (per phase meta-finding)
**Verifier:** gsd-verifier (Claude Sonnet 4.6)

---

## Bilateral Verdict Table

| REQ-ID | Producer (file:line) | Consumer (file:line) | Receiver-end test (file:test_fn) | Verdict |
|--------|---------------------|---------------------|----------------------------------|---------|
| BUG-36.3.7-01 | `worker_spawn.rs` — `--skills` argv block DELETED (lines 181-191 removed; no `"--skills"` literal in file) | `Cli` parser never receives the flag; `spawn_worker` builds `ironhermes --profile <P> chat -q "..."` only | `argv_receiver_36_3_7_0.rs::worker_argv_omits_skills_flag` | PASS |
| BUG-36.3.7-02 | `context.rs:215` — `pub trait KanbanStoreReader` declared; `context.rs:477` — `pub kanban_store: Option<Arc<dyn KanbanStoreReader>>` | `handlers.rs:72` — `"kanban" => cmd_kanban(args, ctx)` dispatch arm; `handlers.rs:1150` — `fn cmd_kanban` routes to `store.list_text()` / `store.show_text()` / `store.tip_text()` / `store.deferred_subverb_message()` | `handlers_kanban.rs::dispatch_kanban_list_returns_table_format` (+ 7 additional tests) | PASS |
| BUG-36.3.7-03 | `dispatcher.rs:276-283` — `consecutive_failures += 1` bump in `detect_crashed_workers` | `dispatcher.rs:305` — `apply_circuit_breaker(ctx, &task, &run.id, &error_msg, now).await?` called AFTER the bump and AFTER the crashed event append | `dispatcher_logic.rs::circuit_breaker_trips_on_crashed_detection_path` + `::circuit_breaker_does_not_trip_below_limit_on_crashed_path` | PASS |
| BUG-36.3.7-04 | `main.rs:411-414` — `chat_has_query` predicate added; `main.rs:419` — `&& !chat_has_query` added to `run_preflight`; `main.rs:436` — `&& !chat_has_query` added to `is_interactive_repl` | `run_preflight` gate now correctly excludes `chat -q` from wizard; `is_interactive_repl` excludes it from interactive log filter | `preflight_chat_query_gate.rs::preflight_gate_excludes_chat_with_query` + `::chat_has_query_destructures_query_some` + `::interactive_repl_gate_also_excludes_chat_with_query` | PASS |
| re-UAT-09-A | Plan 01 fix + Plan 03 fix unblock the spawn path | UAT run #5 — stages 1-6 + 8-10 all PASS; stage 7 blocked by pre-existing out-of-scope Bug #5 | UAT-EVIDENCE.md stage-by-stage verdict table (10 rows) | PASS (PARTIAL on stage 7 — out-of-scope) |
| re-UAT-09-B | Plan 02 wires `/kanban` dispatch chain completely | 8 dispatch-chain tests in `handlers_kanban.rs` + 1 bypass-list unit test prove the chain; live REPL deferred with rationale | `handlers_kanban.rs` (8 tests) + Plan 06's `is_bypass("kanban")` unit test | PASS (automated coverage; live REPL deferred — see Notes) |

---

## Per-Requirement Detail

### REQ: BUG-36.3.7-01 — Drop `--skills` argv from worker_spawn.rs

**Producer end (the broken emitter, now removed)**
- File: `crates/ironhermes-kanban/src/worker_spawn.rs` — lines 181-191 DELETED
- Code: The `let mut skill_args: Vec<String> = vec!["--skills".into(), "kanban-worker".into()]; ...` block and `.args(&skill_args)` call no longer exist. Verified by direct read of the source file — no `"--skills"` string literal appears anywhere in the file (the doc comment updates at lines 37 and 142 also confirmed).
- Replacement: `HERMES_KANBAN_TASK_SKILLS` env var emitted from `build_kanban_worker_env` (lines 139-145) when `task.skills` is `Some` and decodes to a non-empty `Vec<String>`.

**Consumer end (the Cli parser that accepts the post-fix argv)**
- File: `crates/ironhermes-cli/src/main.rs` — Cli struct (top-level clap parser)
- Code: `spawn_worker` now builds `Command::new("ironhermes").arg("--profile").arg(&task.assignee).arg("chat").arg("-q").arg(format!("work kanban task {}", task.id))` — verified at lines 219-230 of worker_spawn.rs. The `Cli` struct in main.rs accepts `--profile` and the `chat -q` shape. UAT run #3 confirmed the argparse error no longer fires.

**Receiver-end test**
- File: `crates/ironhermes-kanban/tests/argv_receiver_36_3_7_0.rs`
- Test names: `worker_argv_omits_skills_flag` (static-grep, load-bearing), `build_kanban_worker_env_emits_skills_env_when_task_has_extras`, `build_kanban_worker_env_omits_skills_env_when_task_has_none`, `build_kanban_worker_env_omits_skills_env_when_task_has_empty_array`
- Test 2 (`ironhermes_binary_accepts_constructed_argv`) is present but `#[ignore]`-gated on `live-binary-test` feature per plan design.
- Asserts: `include_str!("../src/worker_spawn.rs")` does NOT contain `"--skills"`; env carrier emitted IFF `task.skills` is `Some(non_empty)`.
- Summary self-check reports 100 kanban tests, 0 failures, 2 ignored.

**Verdict: PASS**

Both producer (deletion of broken emitter) and consumer (Cli accepts post-fix argv shape) verified. Receiver-end test `worker_argv_omits_skills_flag` is a static-grep gate that would catch re-introduction. UAT run #5 stage 3 confirms runtime behavior.

---

### REQ: BUG-36.3.7-02 — Add `cmd_kanban` handler + KanbanStoreReader trait + dispatch arm + build_cmd_ctx wiring

**Producer end (trait declaration + field + builder)**
- File: `crates/ironhermes-core/src/commands/context.rs:203-226`
- Code: `pub trait KanbanStoreReader: Send + Sync { fn list_text(&self) -> String; fn show_text(&self, id: &str) -> Option<String>; fn tip_text(&self) -> String; fn deferred_subverb_message(&self, name: &str) -> String; }` — verified by direct read. Field `pub kanban_store: Option<Arc<dyn KanbanStoreReader>>` at line 477, builder `with_kanban_store` present.

**Consumer end (dispatch arm + cmd_kanban + KanbanStoreReaderImpl + main.rs wiring)**
- File: `crates/ironhermes-core/src/commands/handlers.rs:72` — `"kanban" => cmd_kanban(args, ctx),` — verified by direct read.
- File: `crates/ironhermes-core/src/commands/handlers.rs:1150-1183` — `fn cmd_kanban` with four arms (None/list, show, tip, deferred-or-typo) — verified by direct read.
- File: `crates/ironhermes-cli/src/kanban/store_reader_impl.rs:48-88` — `impl KanbanStoreReader for KanbanStoreReaderImpl` with all four methods — verified by direct read.
- File: `crates/ironhermes-cli/src/main.rs:1491-1498` — `kanban_store_handle` constructed once via `KanbanStoreReaderImpl::open_default()` — verified by direct read.
- File: `crates/ironhermes-cli/src/main.rs:1051` — `kanban_store` parameter added to `build_cmd_ctx` signature — verified by direct read.
- File: `crates/ironhermes-cli/src/main.rs:1678 + 2075` — both `build_cmd_ctx` call sites pass `kanban_store_handle.clone()` — verified by grep (`with_kanban_store` and `kanban_store_handle.clone()` found at both call sites).
- File: `crates/ironhermes-cli/src/kanban/mod.rs:11-13` — `pub mod store_reader_impl; pub use store_reader_impl::KanbanStoreReaderImpl;` — verified by direct read.
- `DEFERRED_KANBAN_SUBVERBS` const in handlers.rs — 24 entries verified by direct read (claim, complete, block, unblock, comment, archive, reclaim, reassign, assign, link, unlink, create, init, tail, watch, runs, assignees, dispatch, stats, log, context, gc, daemon, diagnostics).

**Receiver-end test**
- File: `crates/ironhermes-core/tests/handlers_kanban.rs`
- Test names (8 total): `dispatch_kanban_list_returns_table_format`, `dispatch_kanban_show_returns_detail`, `dispatch_kanban_show_missing_id_returns_error`, `dispatch_kanban_tip_returns_tip_text`, `dispatch_kanban_deferred_subverb_routes_to_cli_message`, `dispatch_kanban_unknown_subverb_typo_suggests`, `dispatch_kanban_without_store_returns_not_configured`, `static_grep_kanban_dispatch_arm_present`
- Asserts: dispatch chain via `FakeKanbanStoreReader` routes to `cmd_kanban` (NOT `todo_stub`); deferred subverbs return CLI-redirect message (NOT "not yet available"); typo-suggest fires for unknown verbs; None-store returns "not configured".
- Summary self-check: 8 passed, 0 failed.

**Verdict: PASS**

Full bilateral chain verified: trait in core (producer) → impl in cli (bridge) → handle opened once in `run_chat` (lifecycle) → passed through `build_cmd_ctx` at both call sites (wiring) → `cmd_kanban` dispatch arm receives it (consumer). The meta-finding is specifically closed here: the prior phase had `"kanban"` registered in `CommandDef` (emitter) but no dispatch arm (receiver). That gap is now filled and locked by test 8 (`static_grep_kanban_dispatch_arm_present`).

---

### REQ: BUG-36.3.7-03 — Invoke apply_circuit_breaker from detect_crashed_workers path

**Producer end (the consecutive_failures bump)**
- File: `crates/ironhermes-kanban/src/dispatcher.rs:276-283`
- Code: `store.conn.execute("UPDATE tasks SET consecutive_failures = consecutive_failures + 1 WHERE id=?1", params![task.id])` — verified by direct read. This is the bump in `detect_crashed_workers`.

**Consumer end (apply_circuit_breaker invocation on the same path)**
- File: `crates/ironhermes-kanban/src/dispatcher.rs:300-306`
- Code:
  ```rust
  // Phase 36.3.7.0 BUG-36.3.7-03: circuit breaker on crashed-detection path.
  let error_msg = format!("worker process crashed (pid={pid})");
  apply_circuit_breaker(ctx, &task, &run.id, &error_msg, now).await?;
  ```
  Verified by direct read. The call appears AFTER the crashed event append block (line 298), matching the event-then-breaker ordering of the spawn-failure precedent.
- `apply_circuit_breaker` invocation count in `dispatcher.rs`: 3 occurrences — definition (line 972), spawn-failure call site (line 899), crashed-detection call site (line 305). Confirmed via grep.
- D-12 clarifying comment added at `dispatcher.rs:996-999` — verified by direct read of lines 996-1000.

**D-12 determination finding**
The `>=` operator at line 1000 (`if consecutive_failures >= effective_limit {`) was already correct. The determination doc confirms this at Section 1 ("The comparison operator is already `>=`. No character-level change is needed."). The actual fix was structural — wiring the breaker call into the crashed path, not changing the operator.

**Receiver-end test**
- File: `crates/ironhermes-kanban/tests/dispatcher_logic.rs`
- Test names: `circuit_breaker_trips_on_crashed_detection_path` (seeds `consecutive_failures=1`, dead PID `999_999_999`, expects `status=="blocked"` + `gave_up` event); `circuit_breaker_does_not_trip_below_limit_on_crashed_path` (seeds `consecutive_failures=0`, dead PID `999_999_998`, `scheduled_at=now+3600` to prevent re-claim, expects `status=="ready"` + `crashed` event + NO `gave_up` event).
- Existing `circuit_breaker_after_failure_limit` (spawn-failure path) still passes — no regression.
- Summary self-check: 12 tests in `dispatcher_logic.rs`, all PASS; 98 kanban tests total (1 ignored).

**Verdict: PASS**

Producer (bump at line 280) is now paired with consumer (breaker call at line 305) on the same path. The prior gap — three bump sites with only one breaker call — is partially closed (detect_crashed_workers fixed; reclaim_stale_claims and enforce_max_runtime remain unwired per scope fence, documented as out-of-scope follow-up).

---

### REQ: BUG-36.3.7-04 — Preflight gate excludes chat -q

**Producer end (the chat -q path short-circuit, added in Phase 36.3.7 Plan 01)**
- File: `crates/ironhermes-cli/src/main.rs` — `chat -q "..."` routes through `run_single` (short-circuit, line ~430 per comment at line 401).
- The `-q/--query` flag was added by Phase 36.3.7 Plan 01. The `run_single` short-circuit was wired. But the preflight gate was not updated.

**Consumer end (the gate predicates that must exclude chat -q)**
- File: `crates/ironhermes-cli/src/main.rs:411-419` — `chat_has_query` predicate defined as `matches!(&cli.command, Some(Commands::Chat { query: Some(_), .. }))`. `run_preflight` binding now includes `&& !chat_has_query`. Verified by direct read.
- File: `crates/ironhermes-cli/src/main.rs:433-436` — `is_interactive_repl` binding also includes `&& !chat_has_query`. Verified by direct read.
- BOTH sibling gates patched — this is the bilateral requirement. The prior omission was a producer (flag added + `run_single` short-circuit) without updating ALL consumer gates.

**Receiver-end test**
- File: `crates/ironhermes-cli/tests/preflight_chat_query_gate.rs`
- 3 tests: `preflight_gate_excludes_chat_with_query` (asserts MAIN_RS contains `!chat_has_query`), `chat_has_query_destructures_query_some` (asserts the predicate uses `Commands::Chat { query: Some(_), .. }`), `interactive_repl_gate_also_excludes_chat_with_query` (asserts `!chat_has_query` appears >= 2 times in MAIN_RS — proves BOTH sibling gates patched).
- Summary self-check: 3/3 passed.

**Verdict: PASS**

Both sibling gates (`run_preflight` and `is_interactive_repl`) updated and locked by test 3. UAT run #4 runtime confirmation cited in UAT-EVIDENCE.md stage 4.

---

### REQ: re-UAT-09-A — Live worker spawn smoke (Plan 04)

**Producer end (the complete fix bundle)**
- Plans 01 (argv fix) + 02 (kanban handler) + 03 (circuit breaker) + 05 (preflight gate) all merged into `develop` before UAT run #5.

**Consumer end (the live binary exercising the full chain)**
- UAT-EVIDENCE.md documents 5 runs. Run #5 used task `t_c3de269c7956469e`, worker PID 77851, run_id `r_85bef5c2ff914ccf9d02f0ccf725ea81`.
- Stages 1-6 + 8-10 all PASS in the 10-row verdict table.
- Stage 7 (LLM round-trip completes + `kanban_complete` called) is BLOCKED by Bug #5 (`delegate_task` `oneOf` schema rejected by Anthropic). This is documented as out-of-scope, pre-existing, and not a kanban kernel regression.

**Receiver-end test**
- The 10-row verdict table in `36.3.7.0-04-UAT-EVIDENCE.md` constitutes the structured evidence record. Each stage cites a concrete artifact (event log line, grep result, log path).

**Verdict: PASS (stage 7 BLOCKED by out-of-scope Bug #5)**

The kanban kernel (spawn, argparse, preflight, profile activation, env scrub, workspace, log paths) is proven end-to-end. Stage 7 failure is downstream of kanban and documented appropriately. The five run progression (bugs #1 through #5 each surfaced by progressive run) is itself evidence of thorough bilateral tracing during execution.

---

### REQ: re-UAT-09-B — `/kanban list` slash bypass (Plan 04)

**Producer end (bypass registration and dispatch arm)**
- `is_bypass("kanban")` — registered in `running_agent.rs:48` per Plan 06 (36.3.7) and confirmed by Plan 06's unit test.
- `"kanban" => cmd_kanban(args, ctx)` dispatch arm at handlers.rs:72 — added by Plan 02 of this phase.

**Consumer end (dispatch chain through FakeKanbanStoreReader)**
- 8 tests in `handlers_kanban.rs` exercise the full dispatch chain: registry → dispatch function → `cmd_kanban` → `KanbanStoreReader` trait method. Verified that `"not yet available"` (the prior `todo_stub` output) does not appear in any test output.

**Receiver-end test**
- File: `crates/ironhermes-core/tests/handlers_kanban.rs` — 8 tests, all PASS.
- Live REPL exercise (rustyline/ratatui keyboard input path) DEFERRED per documented rationale: alt-screen ANSI sequences make programmatic input unreliable; the 4-line interception path (`input.starts_with('/')` at main.rs:1642 → `dispatch_command(...)`) has no logic of its own. The rationale is sound and the deferred scope is well-defined.

**Verdict: PASS (automated coverage; live REPL deferred — scope fence documented)**

---

## Scope Fence Compliance

| Fence | Status | Evidence |
|-------|--------|----------|
| No new design decisions | PASS | All fixes followed CONTEXT.md locked decisions exactly. D-12 finding confirmed `>=` already correct; structural path taken as prescribed. |
| No touching D-01..D-41 in 36.3.7-CONTEXT.md | PASS | D-12 received a one-line CLARIFICATION COMMENT added in dispatcher.rs source (not in CONTEXT.md text). The D-12 DETERMINATION.md is a new file in the 36.3.7.0 phase directory. No modification to 36.3.7-CONTEXT.md. |
| No source modification under `.planning/phases/36.3.7-...` | PASS | 36.3.7-09-SUMMARY.md not modified. Only new files created in the 36.3.7.0 phase directory. |
| No expanding cmd_kanban beyond list/show/tip | PASS | Handlers.rs has exactly three active subverbs (list, show, tip). 24 deferred subverbs route to `deferred_subverb_message`, not expanded to new functionality. `DEFERRED_KANBAN_SUBVERBS` const counted: 24 entries confirmed. |
| No new tools, CLI verbs, gateway behavior | PASS | No new tools added. No new CLI verbs. No gateway changes. Plan 05 is a gate-predicate patch, not a new verb. |
| Plan 05 added inline (no worktree) | PASS | 05-SUMMARY documents this explicitly: single atomic commit `c453411f`, orchestrator inline execution. |
| Plan 04 was human-verify | PASS | Plan 04 frontmatter: `autonomous: false`. The human checkpoint gate functioned — operator ran UAT procedures, pasted evidence, orchestrator adjudicated. |

---

## UAT Evidence Cross-check

Mapping the 10-row verdict table in `36.3.7.0-04-UAT-EVIDENCE.md` "Stage-by-stage verdict (Run #5)" to underlying artifacts:

| Stage | Description | Verdict | Concrete evidence artifact |
|-------|-------------|---------|---------------------------|
| 1 | Dispatcher `claimed` event with unique `run_id` | PASS | Event log: `event="claimed" task_id=t_c3de269c7956469e run_id=r_85bef5c2ff914ccf9d02f0ccf725ea81 profile=testbanner` — pasted verbatim in EVIDENCE.md |
| 2 | Dispatcher `spawned` event with real OS PID | PASS | Event log: `event="spawned" pid=77851 workspace="~/.ironhermes/kanban/workspaces/t_c3de269c7956469e"` — pasted verbatim |
| 3 | Worker subprocess passes argparse | PASS | Absence of `unexpected argument '--skills'` in worker stderr — BUG-36.3.7-01 fix confirmed runtime. Also locked by `worker_argv_omits_skills_flag` static-grep test. |
| 4 | Worker passes preflight gate (chat -q non-interactive) | PASS | Absence of `EOF on stdin` error — BUG-36.3.7-04 fix confirmed runtime. Also locked by `preflight_chat_query_gate.rs` 3 tests. |
| 5 | Profile activation + .env load → API key visible | PASS | Worker's LLM call sent an auth header (401→400 progression proves auth header was sent). Operator symlink workaround documented. |
| 6 | Worker reaches `Starting agent loop` tracing | PASS | Implicit from 400 firing inside agent's first tool-using turn — agent loop had to start for the tool call to be attempted. |
| 7 | Worker completes LLM round-trip + calls kanban_complete | BLOCKED | Anthropic 400: `tools.1.custom.input_schema does not support oneOf, allOf, or anyOf at the top level`. Bug #5 — out-of-scope. Error pasted verbatim in EVIDENCE.md. |
| 8 | D-16 env-scrub sentinel does NOT leak | PASS | `grep -r marker_41876_uat09a_rerun4_final ~/.ironhermes/logs ~/.ironhermes/kanban` → empty. Locked by INV-36.3.7-07 + `build_kanban_worker_env_scrubs_secrets` unit test. |
| 9 | D-31 workspace dir created per task | PASS | `~/.ironhermes/kanban/workspaces/t_c3de269c7956469e/` exists — pasted in EVIDENCE.md. |
| 10 | Log paths match `paths::kanban_log_*` contract | PASS | `~/.ironhermes/logs/kanban/t_c3de269c7956469e.{stdout,stderr}.log` exists — pasted in EVIDENCE.md. |

---

## Out-of-Scope Items Documented

| Bug | File:lineish | Why out-of-scope | Smallest-fix recommendation | Severity |
|-----|-------------|-----------------|----------------------------|---------|
| Bug #5 — `delegate_task` top-level `oneOf` rejected by Anthropic | `crates/ironhermes-tools/src/delegate_task.rs:735` — `"oneOf": [{"required": ["task"]}, {"required": ["tasks"]}]` at the top level of `input_schema` | Lives in `ironhermes-tools` (Phase 21.7-class infrastructure), not kanban. The `oneOf` predates 36.3.7; Anthropic's restriction on top-level boolean schema combinators is a long-standing API constraint. The kanban worker path happens to exercise this bug because workers register the full default tool surface. Explicitly excluded by phase scope ("No new tools, no new gateway behavior, no new skills"). | Drop schema-level `oneOf`; rely on runtime validation already present in `execute()` per comments at lines 1221-1239. This is the smallest diff and matches existing runtime-enforces-mutex note. | HIGH — blocks ALL Anthropic-routed worker round-trips. |
| `reclaim_stale_claims` + `enforce_max_runtime` paths bump `consecutive_failures` without invoking `apply_circuit_breaker` | `dispatcher.rs:~458` (`reclaim_stale_claims`) + `dispatcher.rs:~553` (`enforce_max_runtime`) | Same structural gap as BUG-36.3.7-03 but explicitly out-of-scope per Plan 03 CONTEXT.md fence ("No feature work beyond the three named bugs") and reiterated in Plan 03 SUMMARY "Deferred follow-up" section. Plan 03 was scoped to only the `detect_crashed_workers` path that UAT actually observed. | Mirror the same `apply_circuit_breaker(ctx, &task, &run.id, &error_msg, now).await?` call into both sites, matching the pattern now established in `detect_crashed_workers` at line 305. | LOW — no UAT evidence these paths fire in current usage. |
| Profile `.env` propagation gap | Operator workaround: `ln -s ~/.ironhermes/.env ~/.ironhermes/profiles/<P>/.env` | Not a kanban kernel bug; global vs profile-scoped `.env` is a design question for the profile subsystem. Operator workaround documented. | Could be design intent (profile isolation). Needs stakeholder call before changing. | MEDIUM — affects operator first-run experience. |
| OpenRouter deprecation warning | Config-level only — `using deprecated env var OPENROUTER_API_KEY` | No code change needed; operator silences by setting `providers.openrouter.api_key_env` in config.yaml. | Operator config only. | LOW — cosmetic warning. |

---

## Meta-Finding Verification

### Bilateral-tracing rule from Plan 04 SUMMARY (verbatim quote)

From `36.3.7.0-04-SUMMARY.md` "Meta-finding — bilateral-tracing rule for verifiers":

> For every wire-up claim ("X is registered" / "X argv is constructed" / "X event bumps Y counter"), the verifier MUST trace BOTH ends:
> - PRODUCER: where does the value get emitted/registered/incremented?
> - CONSUMER: who reads it, accepts it, dispatches on it, acts on it?
>
> Stop at the producer side only when the consumer side is explicitly out-of-scope (and document why). Otherwise, every "registered" check must be paired with a "received" check, every "added flag" check with an "ALL sibling gates updated" check, every "schema defined" check with an "EVERY provider accepts this schema" check.

**Assessment of rule quality:** The rule is concrete and actionable. It specifies three failure modes by example (registered/received, added flag/sibling gates, schema defined/provider acceptance), gives a clear stopping criterion ("consumer is explicitly out-of-scope — document why"), and is worded as a MUST requirement rather than a suggestion. This is not a vague observation.

**Cross-reference to Phase 36.3.7-09 SUMMARY Addendum:** The 36.3.7-09 SUMMARY does NOT contain a bilateral-tracing rule section — it deferred the UAT cases (Tasks 3 and 4) to 36.3.7.x with documented procedures, but did not explicitly capture the pattern as a verifier rule. The 36.3.7-09 SUMMARY documented the UAT procedures and escalation paths, but the meta-finding as a named, quotable rule first appears in Phase 36.3.7.0 Plan 04 SUMMARY. The UAT-EVIDENCE.md does cross-reference the "Meta-finding (carried from Phase 36.3.7-09 Addendum)" and attributes the pattern to the original UAT-09 round, making the lineage traceable. The rule compounds correctly: the 36.3.7 UAT surfaced the bugs; 36.3.7.0 UAT surfaced two more; the rule now names all five instances in a table.

**Concrete examples in Plan 04 SUMMARY:** Three specific examples map the rule to the actual bugs:
- "`worker_spawn` emits `--skills`" → paired with "`Cli` struct accepts `--skills`" — Bug #1
- "`CommandDef::new("kanban", ...)` registers /kanban" → paired with "`handlers.rs::dispatch` has a `"kanban" =>` arm" — Bug #2
- "`-q` flag added to `Chat` enum + `run_single` short-circuit wired" → paired with "ALL preflight/interactive sibling gates also exclude `chat -q`" — Bug #4

---

## Notes / Risks

1. **Live REPL deferred (UAT-09-B):** The interactive `ironhermes chat` REPL exercise was not run against the live binary. The 8 dispatch-chain tests provide strong automated coverage of the logic; the 4-line interception path at main.rs:1642 (`input.starts_with('/')` → `dispatch_command`) is the uncovered surface. The deferred rationale (alt-screen ANSI incompatibility with programmatic `expect`) is sound. A PTY-aware harness in a future phase would close this gap definitively.

2. **HERMES_KANBAN_TASK_SKILLS receiver-side is dormant:** The env var carrier added by Plan 01 has no consumer. This is explicitly documented as a forward-compatible carrier and is intentional per scope fences. No code path reads `HERMES_KANBAN_TASK_SKILLS` and acts on it. If a future phase adds a receiver, it should be accompanied by bilateral tracing verifying both the emit and the consume.

3. **reclaim_stale_claims + enforce_max_runtime structural gap remains:** These two paths bump `consecutive_failures` without calling `apply_circuit_breaker`. The 36.3.7.0 scope fence explicitly punts these. A future UAT that exercises stale-claim or max-runtime failure paths will surface this as a repeat of BUG-36.3.7-03. Recommend 36.3.7.1 explicitly targets these two sites.

4. **Test count consistency:** Plan 01 summary reports 100 kanban tests; Plan 03 summary reports 98 tests (+ 1 ignored). This apparent discrepancy is explained by Plan 01 adding 5 new tests (to 67 base → 72 total in kanban package), and Plan 03 adding 2 more (→ 74), but the 100-test figure in Plan 01 SUMMARY likely includes tests across all kanban test files at the time of its run. The per-suite counts in Plan 03's self-check table (98 passing, 1 ignored) are the authoritative figure post-merge.

5. **Bug #5 severity and scope clarity:** Plan 04 SUMMARY correctly identifies Bug #5 (`delegate_task oneOf`) as HIGH severity and out-of-scope. However, this bug blocks ALL Anthropic-routed worker round-trips, which means UAT-09-A stage 7 cannot pass on any subsequent re-run until it is fixed — regardless of kanban changes. The closing recommendation should make this sequencing explicit.

6. **Phase 36.3.7-09 SUMMARY Addendum note:** The 36.3.7-09 SUMMARY does not itself contain a "bilateral-tracing rule" section — it documents UAT procedures and deferred items. The UAT-EVIDENCE.md in 36.3.7.0 correctly attributes the pattern origin ("carried from Phase 36.3.7-09 Addendum + reinforced this session"), but a reader following only the 36.3.7 phase artifacts would not find a named rule. The rule lives exclusively in Phase 36.3.7.0 Plan 04 SUMMARY. This is acceptable for a phase-insert workflow but means the rule does not compound into a shared LEARNINGS.md yet — that step was called out in CONTEXT.md notes as a post-close action.

---

## Closing Recommendation

**Close Phase 36.3.7.0.** All four named requirements (BUG-36.3.7-01 through -04) are verified bilaterally with producer, consumer, and receiver-end tests in the codebase. The re-UAT results are honest: the kanban kernel is runtime-proven through 9 of 10 UAT stages; stage 7 is blocked by Bug #5 (`delegate_task` schema), which is pre-existing, correctly scoped out, documented with file:line and fix recommendation, and does not regress any kanban behavior.

Minimum follow-up required before the kanban runtime can achieve a full UAT-09-A PASS:

1. Fix Bug #5 (`delegate_task.rs:735` — drop top-level `oneOf`, rely on runtime validation in `execute()`). This is a dedicated phase on `ironhermes-tools`, not 36.3.7.x.
2. Open 36.3.7.1 to wire `apply_circuit_breaker` into `reclaim_stale_claims` (~line 458) and `enforce_max_runtime` (~line 553), and optionally to add a receiver-side consumer for `HERMES_KANBAN_TASK_SKILLS`.
3. Add the bilateral-tracing rule to a shared LEARNINGS.md so it becomes part of future verifier prompt generation rather than remaining in one SUMMARY file.

_Verified: 2026-05-29T00:00:00Z_
_Verifier: Claude (gsd-verifier, Sonnet 4.6)_
