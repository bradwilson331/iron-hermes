# UAT-09-A Re-Run #6 — Operator Runbook

**Operator:** Brad Wilson (you)
**Orchestrator role:** runbook + interpretation only (this run, you drive)
**Date queued:** 2026-05-29
**Inherits run history:** Runs #1–#5 (see `36.3.7.0-04-UAT-EVIDENCE.md`)
**Goal:** Close the bilateral consumer loop on the entire 36.3.7.x cascade. First live kanban worker round-trip with ALL the following fixes landed:

| Source phase | Fix landed |
|---|---|
| 36.3.7.0 Plan 01 | `--skills` argv dropped from worker spawn (BUG-36.3.7-01) |
| 36.3.7.0 Plan 02 | `cmd_kanban` handler + 24 deferred-subverb routing (BUG-36.3.7-02) |
| 36.3.7.0 Plan 03 | `apply_circuit_breaker` wired into `detect_crashed_workers` (BUG-36.3.7-03) |
| 36.3.7.0 Plan 05 | `chat -q` preflight-gate exclusion (BUG-36.3.7-04) |
| 36.3.7.1 Plan 01 | `apply_circuit_breaker` wired into `reclaim_stale_claims` (BUG-36.3.7.1-01) |
| 36.3.7.1 Plan 02 | `apply_circuit_breaker` wired into `enforce_max_runtime` (BUG-36.3.7.1-02) |
| 36.3.7.2 Plan 01 | top-level `oneOf` dropped from `delegate_task` schema (BUG-IRONHERMES-TOOLS-SCHEMA-COMPAT-01) |
| 36.3.7.2 Plan 02 | system-level receiver-end lock test against future top-level combinators (BUG-IRONHERMES-TOOLS-SCHEMA-COMPAT-03) |
| 36.3.7.3 | CFG-03 marker restored + `phase_amendment_doc_comment_present` test hardened to anchor on `async fn main` scope |

**Expected outcome:** UAT-09-A stages 1-10 ALL green. Stage 7 (the BLOCKED stage in Run #5) is the headline check — Anthropic should no longer reject the tool registration with `400: input_schema does not support oneOf, allOf, or anyOf at the top level`. The worker should reach an LLM tool-call round-trip, call `kanban_complete`, and the dispatcher should observe a `completed` event.

---

## Stage 0 — Pre-flight (10 minutes, no LLM tokens)

You're checking that the binary you're about to spawn actually has the fixes. Skip none of these.

### 0.1 — Confirm the develop branch is at the latest 36.3.7.3 commit

```bash
cd /Users/twilson/code/ironhermes
git status -b --short                       # branch == develop, clean (or only OMC noise + .DS_Store)
git log --oneline -10                       # top 5 commits should reference 36.3.7.3 + 36.3.7.1
```

**PASS signal:** topmost commit is `085f05b6 docs(36.3.7.3): phase Complete — STATE + ROADMAP rationalized` (or a later HEAD if you've made more commits).
**FAIL signal:** anything older than `3c02c645 docs(36.3.7.3): SUMMARY` is on top → halt, paste `git log --oneline -5` back to me.

### 0.2 — Rebuild release binary

The kanban dispatcher spawns workers via `Command::new("ironhermes")`, which PATH-resolves to `~/.local/bin/ironhermes` (a symlink to the release binary, per Run #2's discovery — see LEARNINGS "PATH-resolved binaries are a deployment seam"). **You MUST rebuild release before this UAT.**

```bash
cd /Users/twilson/code/ironhermes
cargo build --release 2>&1 | tail -5
```

**PASS signal:** `Finished \`release\` profile [optimized] target(s) in <N>s`. Exit 0.
**FAIL signal:** any `error[E...]` line → halt, paste the error.

### 0.3 — Confirm PATH resolution + binary freshness

```bash
which ironhermes                                           # MUST resolve to ~/.local/bin/ironhermes (or wherever your symlink lives)
ls -la "$(which ironhermes)"                               # symlink target should be /Users/twilson/code/ironhermes/target/release/ironhermes
ls -la /Users/twilson/code/ironhermes/target/release/ironhermes
ironhermes --version 2>&1 | head -1                        # any version string is fine; the goal is "binary is runnable"
```

**PASS signal:** symlink target is the release binary AND the release binary's mtime is from THIS session (today, after the 36.3.7.3 commits).
**FAIL signal:** symlink points at debug, OR the release binary mtime is from before today → halt + paste the `ls -la` lines.

### 0.4 — Confirm the 4 fixes are in the release binary

Five static-grep checks across the binary's source (the source is what got built, so these confirm what's in the binary):

```bash
# Fix 1: --skills NOT in worker spawn argv (36.3.7.0 Plan 01)
grep -n '"--skills"' /Users/twilson/code/ironhermes/crates/ironhermes-kanban/src/worker_spawn.rs && echo "FAIL: --skills still present" || echo "PASS: --skills dropped"

# Fix 2: kanban dispatch arm in handlers.rs (36.3.7.0 Plan 02)
grep -n '"kanban" =>' /Users/twilson/code/ironhermes/crates/ironhermes-core/src/handlers.rs && echo "PASS: kanban dispatch arm present" || echo "FAIL: missing kanban arm"

# Fix 3: breaker called from all 4 failure paths (36.3.7.0 Plan 03 + 36.3.7.1 Plans 01 + 02 + original 36.3.7)
grep -nE 'apply_circuit_breaker\(' /Users/twilson/code/ironhermes/crates/ironhermes-kanban/src/dispatcher.rs | head -10
# EXPECTED: 5 hits at lines 305, 481, 604, 923, 996 (4 call sites + 1 def)

# Fix 4: chat -q preflight exclusion (36.3.7.0 Plan 05)
grep -n '!chat_has_query' /Users/twilson/code/ironhermes/crates/ironhermes-cli/src/main.rs | head -5
# EXPECTED: 2 hits (run_preflight gate + is_interactive_repl gate)

# Fix 5: delegate_task schema has no top-level oneOf (36.3.7.2)
grep -n '"oneOf":' /Users/twilson/code/ironhermes/crates/ironhermes-tools/src/delegate_task.rs && echo "FAIL: oneOf still present" || echo "PASS: oneOf dropped"
```

**PASS signal:** 4 PASS lines + 5 hits in the apply_circuit_breaker grep + 2 hits in the !chat_has_query grep.
**FAIL signal:** any FAIL line OR wrong hit count → halt + paste the grep outputs.

### 0.5 — Confirm profile env is still symlinked from Run #5

```bash
ls -la ~/.ironhermes/profiles/testbanner/.env
# EXPECTED: symlink to ~/.ironhermes/.env (per Run #5 operator action)
```

**PASS signal:** symlink target is the global `.env`.
**FAIL signal:** symlink missing OR target wrong → re-create:
```bash
ln -sf ~/.ironhermes/.env ~/.ironhermes/profiles/testbanner/.env
```
Then re-check.

---

## Stage 1 — Set the env-scrub sentinel (10 seconds, no tokens)

The sentinel proves D-16 env_scrub still works (which Run #5 confirmed). Set a unique marker in your shell BEFORE the dispatch:

```bash
export OPENAI_API_KEY=marker_$(date +%s)_uat09a_rerun6
echo "sentinel: $OPENAI_API_KEY"     # save this string — you'll grep for it in Stage 8
```

Write the sentinel string into a file for later (saves you needing to remember it):

```bash
echo "$OPENAI_API_KEY" > /tmp/uat09a-rerun6-sentinel.txt
```

---

## Stage 2 — Create a kanban task (1 minute, no tokens)

The binary's actual `kanban create` shape (per `--help`):
- `--assignee <NAME>` is REQUIRED (this is the profile the dispatcher will spawn the worker under)
- `<TITLE>` is the positional, must be SHORT (one line)
- `--body <BODY>` carries the long-form prompt the worker actually sees

```bash
cd /Users/twilson/code/ironhermes

ironhermes kanban --profile testbanner create \
    --assignee testbanner \
    --workspace scratch \
    --tenant t-test \
    --body "Briefly acknowledge this task with one sentence, then call the kanban_complete tool with summary='re-run 6 acknowledged' and any small metadata payload like {\"check\":\"schema-fix\"}." \
    --json \
    "UAT-09-A re-run #6 — verify Anthropic schema fix end-to-end"
```

**PASS signal:** stdout is a JSON object containing `"task_id":"t_<16-hex>"` and `"status":"ready"`. Save it:
```bash
export TASK_ID=$(ironhermes kanban --profile testbanner list --json 2>/dev/null | grep -o 't_[a-f0-9]\{16\}' | tail -1)
# OR just paste the task_id from the JSON output:
# export TASK_ID=t_<paste-the-id>
echo "$TASK_ID" > /tmp/uat09a-rerun6-task-id.txt
echo "Captured: $TASK_ID"
```

**FAIL signal:** any error → halt + paste stdout/stderr.

---

## Stage 3 — Single dispatcher tick (30 seconds, no tokens until worker spawns)

This claims the task, spawns the worker. The worker will then go out to Anthropic-via-OpenRouter — that's the first paid step.

The dispatch subcommand is one-shot by default (no `--once` flag). Use `--max <N>` to cap how many tasks are claimed per tick.

```bash
ironhermes kanban --profile testbanner dispatch --max 1 2>&1 | tee /tmp/uat09a-rerun6-dispatcher.log
```

**PASS signals to look for (in the dispatcher.log):**
- `event="claimed" task_id=t_<your-id> run_id=r_<16-hex> profile=testbanner` → **Stage 1+2 PASS** (atomic claim + spawn)
- `event="spawned" pid=<N> workspace="~/.ironhermes/profiles/testbanner/kanban/workspaces/t_<id>"` → **Stage 2 PASS** (real subprocess)
- NO `error: unexpected argument` line → **Stage 3 PASS** (argparse — was Run #1's blocker)
- NO `EOF on stdin` line → **Stage 4 PASS** (preflight gate — was Run #3's blocker)

Save the run_id:
```bash
export RUN_ID=r_<paste-from-dispatcher.log>
```

The tick returns once the dispatch loop completes one pass — the worker keeps running in the background. Don't kill it yet.

---

## Stage 4 — Watch the worker (1–3 minutes, this is where Run #5 died)

The worker is doing its LLM round-trip now. Tail its stderr:

```bash
tail -f ~/.ironhermes/profiles/testbanner/logs/kanban/${TASK_ID}.stderr.log
```

**PASS signals to look for (in stderr) — the headline check for THIS run:**
- `[profile: testbanner] HERMES_HOME=~/.ironhermes/profiles/testbanner` → Stage 5 PASS (profile activation)
- The deprecated `OPENROUTER_API_KEY` warning is FINE — that's a separate config-tidy issue, not a UAT blocker
- `Starting agent loop` (or any tracing indicating the agent is iterating) → Stage 6 PASS (worker reached the LLM call layer)
- NO `400 Bad Request` about `oneOf` / `allOf` / `anyOf` → **Stage 7 PASS — THIS IS THE NEW THING that the 36.3.7.2 fix unblocks**
- A tool-call log entry referencing `kanban_complete` followed by a successful response → Stage 7+ PASS (the worker actually completed)
- Eventually the worker process exits cleanly (PID gone from `ps`)

**FAIL signal — the one we want to NOT see:**
```
400 Bad Request: ... tools.<N>.custom.input_schema: input_schema does not support
                    oneOf, allOf, or anyOf at the top level ...
```
If you see this → the 36.3.7.2 fix didn't ship in the release binary. Halt, paste the stderr block, re-check 0.2 + 0.4.

**Press Ctrl-C on the `tail -f` once the worker exits.** You can confirm exit via:
```bash
ps -p <PID-from-dispatcher.log>     # should report "No such process" once the worker is done
```

---

## Stage 5 — Confirm the dispatcher saw the completion (10 seconds, no tokens)

Run another single tick so the dispatcher picks up the worker's `kanban_complete` call and marks the task done:

```bash
ironhermes kanban --profile testbanner dispatch --max 1 2>&1 | tee /tmp/uat09a-rerun6-dispatcher-tick2.log
```

Then verify task state:

```bash
ironhermes kanban --profile testbanner show "$TASK_ID" 2>&1 | tee /tmp/uat09a-rerun6-task-show.log
```

**PASS signals:**
- Task status: `done`
- Latest `task_runs` entry: `outcome='completed'`
- Latest `task_events`: a `completed` event with the worker's summary
- A `claimed` event AND a `completed` event both reference the same `run_id` → **INV-36.3.7-01 + INV-36.3.7-02 runtime PASS** (the canonical 10-invariant check)

**FAIL signals:**
- Task status still `running`: the worker didn't call `kanban_complete`. Check the worker's stdout log (`~/.ironhermes/profiles/testbanner/logs/kanban/${TASK_ID}.stdout.log`) for what it actually did. Paste the last 50 lines back to me.
- Task status `blocked` with a `gave_up` event: the breaker fired. This SHOULD only happen if something else (LLM auth, schema, etc.) made the worker fail and the failure_limit (2) was exceeded. Paste the `gave_up` event payload back to me — that's the breaker working as designed; we just need to understand what tripped it.

---

## Stage 6 — Env-scrub sentinel check (Stage 8 of the original UAT-09-A — 10 seconds, no tokens)

```bash
SENTINEL=$(cat /tmp/uat09a-rerun6-sentinel.txt)
grep -r "$SENTINEL" ~/.ironhermes/logs ~/.ironhermes/kanban 2>&1 | head -20
```

**PASS signal:** zero hits (the env_clear + 9-var contract correctly scrubbed your parent shell secrets even though you set them before dispatch). This is INV-36.3.7-07 runtime PASS.
**FAIL signal:** any hit shows the sentinel leaked into worker logs → D-16 env-scrub regression. Paste the hit lines back.

---

## Stage 7 — Workspace + log path checks (Stages 9 + 10 of original UAT-09-A — 10 seconds, no tokens)

```bash
ls -la ~/.ironhermes/profiles/testbanner/kanban/workspaces/${TASK_ID}/ 2>&1
ls -la ~/.ironhermes/profiles/testbanner/logs/kanban/${TASK_ID}.stdout.log ~/.ironhermes/profiles/testbanner/logs/kanban/${TASK_ID}.stderr.log 2>&1
```

**PASS signal:** workspace dir exists + both log files exist.
**FAIL signal:** any "No such file or directory" → halt + paste the ls output.

---

## Stage 8 — Stage matrix + verdict (3 minutes — fill out the table)

Copy this table into your reply along with any FAIL evidence. I'll interpret + decide whether to file a follow-up phase or close UAT-09-A clean.

```
| # | Stage | Verdict | Evidence (1-line) |
|---|---|---|---|
| 1 | Dispatcher `claimed` event with unique `run_id` | ?  |  |
| 2 | Dispatcher `spawned` event with real OS PID | ?  |  |
| 3 | Worker subprocess passes argparse (no `--skills` error) | ?  |  |
| 4 | Worker passes preflight gate (no `EOF on stdin`) | ?  |  |
| 5 | Profile activation + .env load (no `401 Missing Auth header`) | ?  |  |
| 6 | Worker reaches `Starting agent loop` tracing | ?  |  |
| 7 | Worker completes an LLM round-trip + calls `kanban_complete` (no `400 oneOf` error) | ?  |  |
| 8 | D-16 env-scrub sentinel does NOT leak into worker logs | ?  |  |
| 9 | D-31 workspace dir created per task | ?  |  |
| 10 | Log paths match `paths::kanban_log_*` contract | ?  |  |
```

**Headline question:** did Stage 7 flip from BLOCKED (Run #5) to PASS (Run #6)? That's what this entire 36.3.7.x cascade was designed to unblock.

---

## Optional Stage 9 — Breaker behavior verification (extends 36.3.7.1's coverage)

If you want to also exercise the two NEW breaker paths from 36.3.7.1 live (NOT required for UAT-09-A PASS; this is a bonus end-to-end check of the dispatcher follow-up):

**For `reclaim_stale_claims` (BUG-36.3.7.1-01):**
1. Create a task that NEVER spawns successfully (use an invalid `--toolset` arg or a too-restrictive system prompt). Watch consecutive_failures climb past `failure_limit` (default 2). Expected: `gave_up` event fires on the same tick the second reclaim trips the threshold.

**For `enforce_max_runtime` (BUG-36.3.7.1-02):**
1. Create a task with `--max-runtime-seconds 5` and a system prompt instructing the agent to do something LONG (e.g., "for each of these 50 topics, write 200 words"). Expected: SIGTERM + 5s grace + SIGKILL, `timed_out` event, breaker call on the same tick.

These are unit-tested already (see `crates/ironhermes-kanban/tests/dispatcher_logic.rs` lines 908, 1029, 1127, 1243 — added by Plan 36.3.7.1-01/02). The live exercise is optional belt-and-suspenders.

---

## Cleanup

Once Stages 1-10 are filled out:

```bash
# Save the run artifacts for the evidence log
cp /tmp/uat09a-rerun6-dispatcher.log ~/.ironhermes/uat-evidence/
cp /tmp/uat09a-rerun6-task-show.log ~/.ironhermes/uat-evidence/
mkdir -p ~/.ironhermes/uat-evidence
# (mkdir before cp if the dir doesn't exist yet)

# Clean up the sentinel from your shell
unset OPENAI_API_KEY
```

If Stage 7 PASSED: paste the filled-out matrix back to me. I'll create `36.3.7.0-04-UAT-EVIDENCE.md` Run #6 section and mark Task #18 complete + close the 36.3.7.x cascade narratively in STATE.md.

If Stage 7 FAILED with a NEW error (not the old `oneOf` one): paste the matrix + the new error block. I'll triage + scope a follow-up phase.

If you hit any other stage failure earlier: paste the matrix + the FAIL evidence for that stage. I'll diagnose + dispatch a fix path.

---

## Budget guidance

- LLM tokens: 1 full Anthropic round-trip via OpenRouter (probably $0.01-$0.05 with Sonnet 4.6, depending on the system prompt + tool surface). The kanban worker only does 1 LLM call here because we ask for a 1-sentence acknowledge + `kanban_complete`.
- Time: ~15-20 minutes elapsed, of which ~12 minutes is the pre-flight + ~3 minutes is the live dispatcher tick + worker round-trip.
- Risk of partial billing: if Stage 4 hits the old `400 oneOf` error again, you've already burned the network round-trip + the request validation cost on Anthropic's side, even though no completion was generated. OpenRouter typically passes that cost through. Cost upper bound for the failure case: ~$0.01.

---

## Closure criteria

UAT-09-A re-run #6 closes the 36.3.7.x cascade IFF:
- Stages 1-10 ALL green
- Stage 7 explicitly different from Run #5 (no `oneOf` error in worker stderr)

If those conditions hold, the bilateral consumer signal that the LEARNINGS rule requires for this cascade is fully delivered. Task #18 in the orchestrator's task list flips to completed. Next-queue selection resumes from a clean state.
