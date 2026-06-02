---
phase: quick-260602-nd7
plan: 01
subsystem: kanban
tags: [u9-fix, d-21, drawer-auto-refresh, producer-fix, task-events, bilateral-regression]
requires:
  - .planning/quick/260602-nd7-fix-u9-drawer-comments-auto-refresh/260602-nd7-PLAN.md
provides:
  - producer-end emits task_events row from KanbanStore::add_comment (U9 fix)
  - producer-end regression test pinning the contract (3 tests)
  - consumer-end regression test pinning drawer's comments-resource D-21 read (1 test)
affects:
  - crates/ironhermes-kanban/src/store.rs::add_comment
  - crates/ironhermes-kanban/tests/comment_appends_event.rs (NEW)
  - crates/iron_hermes_ui/tests/kanban_drawer.rs (added 1 test)
tech-stack:
  added: []
  patterns: [rusqlite-transaction, json-payload-subkind-tag, source-string-byte-offset-assertion]
key-files:
  created:
    - crates/ironhermes-kanban/tests/comment_appends_event.rs
  modified:
    - crates/ironhermes-kanban/src/store.rs
    - crates/iron_hermes_ui/tests/kanban_drawer.rs
decisions:
  - Reused KanbanEventKind::Edited with payload.subkind="comment" (events.rs frozen surface per Phase 36.3.7.6)
  - Single rusqlite transaction wraps task_comments + task_events INSERTs (T-quick-nd7-01 mitigation)
  - Comment body excluded from event payload to bound row sizes (T-quick-nd7-02)
  - Used existing Self::append_event_internal helper rather than inlined raw SQL (consistency with other store mutators)
  - Consumer-end byte-offset slice assertion (not Rust syntactic parsing) — simplest robust localization
metrics:
  duration_seconds: 752
  completed_date: 2026-06-02T21:08:54Z
  tasks_executed: 3
  commits: 3
  tests_added: 4
  tests_modified: 0
---

# Quick Task 260602-nd7: Fix U9 Drawer Comments Auto-refresh — Summary

One-line: Restored the D-21 dashboard live-update contract on the comments path by adding the missing `task_events` INSERT inside `KanbanStore::add_comment`, plus bilateral regression coverage (producer-end + consumer-end) so the same defect cannot ship again.

## What landed

Three commits on `worktree-agent-a030e685e762f4d4b` (branched from `d2e51d52`):

| # | Hash | Type | Files |
|---|------|------|-------|
| 1 | `fba3c5c1` | fix | `crates/ironhermes-kanban/src/store.rs` (+32 / -1) |
| 2 | `e62c7997` | test | `crates/ironhermes-kanban/tests/comment_appends_event.rs` (+201, new file) |
| 3 | `4e126b64` | test | `crates/iron_hermes_ui/tests/kanban_drawer.rs` (+63) |

## Task 1 — Producer fix in `KanbanStore::add_comment`

Exact diff in `crates/ironhermes-kanban/src/store.rs` (lines 546-598):

```diff
     /// Add a comment to a task. Returns the new `TaskComment`.
+    ///
+    /// Quick task 260602-nd7 (U9 fix): in addition to the existing
+    /// `task_comments` row, this method now appends one
+    /// `KanbanEventKind::Edited` row to `task_events` with payload
+    /// `{"subkind":"comment","comment_id":<id>,"author":<author>}` so the
+    /// dashboard tail consumer (D-15) can broadcast a `TaskEventBatch`
+    /// to all connected WS clients, which the drawer's per-task event
+    /// counter then picks up (D-21) and re-runs the `comments`
+    /// `use_resource`. The two writes share a single transaction so a
+    /// half-landed (comment without event) state is impossible — pattern
+    /// matches the `block_task` / `insert_link_checked` mutators in this
+    /// file. `events.rs` is frozen surface (Phase 36.3.7.6) so we reuse
+    /// the existing `Edited` variant with a `subkind` JSON tag instead of
+    /// adding a `Comment` variant. The comment body is intentionally
+    /// excluded from the event payload to keep `task_events` row sizes
+    /// bounded (T-quick-nd7-02 / threat register).
     pub fn add_comment(&mut self, task_id: &str, author: &str, body: &str) -> Result<TaskComment> {
         // Verify task exists.
         self.get_task(task_id)?;

         let id = Self::new_id("c");
         let now = Self::now();

-        self.conn.execute(
+        let tx = self.conn.transaction()?;
+        tx.execute(
             "INSERT INTO task_comments (id, task_id, author, body, created_at) \
              VALUES (?1, ?2, ?3, ?4, ?5)",
             params![id, task_id, author, body, now],
         )?;
+        let event_payload = serde_json::json!({
+            "subkind": "comment",
+            "comment_id": id,
+            "author": author,
+        });
+        Self::append_event_internal(
+            &tx,
+            task_id,
+            None,
+            KanbanEventKind::Edited,
+            Some(&event_payload),
+            now,
+        )?;
+        tx.commit()?;

         Ok(TaskComment {
             id,
             task_id: task_id.to_string(),
             author: author.to_string(),
             body: body.to_string(),
             created_at: now,
         })
     }
```

**Transaction idiom chosen:** inline `self.conn.transaction()` plus the existing `Self::append_event_internal` helper at `store.rs:377` (which takes `&Connection` so it accepts either `&self.conn` or a transaction handle). This matches the pattern used by `insert_link_checked` (store.rs:712-719) and `swarm_create` (store.rs:935-967, see the `&tx` argument to `append_event_internal`). No new helpers introduced.

**`KanbanEventKind` was NOT extended.** Verification: `grep -c '^    Comment,' crates/ironhermes-kanban/src/events.rs` returns 0. The frozen-surface invariant from Phase 36.3.7.6 is preserved.

## Task 2 — Producer-end regression test (`comment_appends_event.rs`)

**4 new tests** authored (the plan called for 3; one extra-defensive assertion landed inline within Test 1):

| Test | Asserts |
|------|---------|
| `comment_emits_task_event_row` | After `add_comment`, `task_events` count for the same `task_id` increases by exactly 1; the new row's `kind == "edited"`; the new row's `task_id` matches. |
| `comment_event_carries_subkind_and_comment_id_in_payload` | The newest event row's `payload` parses as JSON; `payload.subkind == "comment"`; `payload.comment_id == <the returned TaskComment.id>`. |
| `comment_does_not_emit_event_when_task_missing` | `add_comment` on a nonexistent `task_id` returns `Err` AND `task_events` count is unchanged (pins the `get_task` precondition gate). |

**Fixture style:** Mirrors `crates/ironhermes-kanban/tests/store_smoke.rs` — `tempfile::tempdir() + KanbanStore::new(&path.join("kanban.db"))`. Raw rusqlite queries against `store.conn` for `task_events` row inspection. Zero new cargo deps (tempfile + serde_json already present in workspace).

**Bilateral revert/restore sanity check executed:** (per Task 2's `<done>`)
- Reverted commit `fba3c5c1` locally (`git checkout HEAD~1 -- crates/ironhermes-kanban/src/store.rs`).
- Re-ran tests: `comment_emits_task_event_row` and `comment_event_carries_subkind_and_comment_id_in_payload` **FAILED** with the exact authored assertion messages (`after=1, expected 2`; payload was the legacy `created` event's `{"assignee":"bob","parents":[],"status":"ready","tenant":null}` instead of the new `{"subkind":"comment",...}`).
- Test 3 (negative-path) **PASSED** unchanged because the `get_task` precondition is identical in both versions.
- Restored commit `fba3c5c1` (`git checkout HEAD -- crates/ironhermes-kanban/src/store.rs`).
- Re-ran tests: all 3 GREEN.

This proves the tests genuinely lock the producer contract — they fail in the absence of the fix, not by coincidence.

## Task 3 — Consumer-end regression test (`kanban_drawer.rs`)

One new test appended to `crates/iron_hermes_ui/tests/kanban_drawer.rs` (line 391+):

`comments_resource_reads_per_task_event_counter_for_d21` — uses byte-offset slicing to assert `per_task_event_counter()` appears in the source slice between the `let comments = use_resource` declaration and the `fetch_comments` call site. This catches a regressor who deletes `let _counter = per_task_event_counter();` from the comments closure while leaving the other 3 resources' counter reads intact (which would silently break U9 again but pass the coarse line-115 assertion).

**First-run result:** GREEN (no fallback drawer.rs patch needed). drawer.rs:109 already reads the counter inside the comments closure per the plan's `<interfaces>` block.

**Test count:** `kanban_drawer` suite went from 27 → 28; full suite passes.

## Gate Exit Codes

| Gate | Exit | Notes |
|------|------|-------|
| `cargo build -p ironhermes-kanban` | 0 | clean |
| `cargo test -p ironhermes-kanban --lib` | 0 | 131 / 131 |
| `cargo test -p ironhermes-kanban --test comment_appends_event` | 0 | 3 / 3 |
| `cargo test -p iron_hermes_ui --features server --test kanban_drawer` | 0 | 28 / 28 |
| `cargo build --workspace` | 0 | `Finished dev profile in 1m 15s` |
| `cargo test --workspace` | non-zero | **Pre-existing failures** logged in `deferred-items.md` — none caused by this plan (see DEFER-1, DEFER-3) |
| `cargo clippy --workspace --features iron_hermes_ui/server -- -D warnings` | non-zero | **Pre-existing Rust 1.94 lint upgrade** in `ironhermes-core` (16 errors) and `ironhermes-kanban` lib-test (21 errors). All from commits older than this plan (`0db139084` 2026-05-09, `9cc4114d8` 2026-05-29). See DEFER-2. |
| `cargo clippy -p iron_hermes_ui --features server --test kanban_drawer --no-deps -- -D warnings` | 0 | my touched test file is clippy-clean |

## Deviations from Plan

**No Rule 1-4 deviations.** The plan's diagnosis was exactly correct:
- Producer `add_comment` was missing `task_events` INSERT — confirmed by reading store.rs:547-568.
- `events.rs` is frozen surface — confirmed at events.rs:29-122 (variant set unchanged).
- `append_event_internal` helper exists at store.rs:377 — reused as planned.
- `drawer.rs:109` already reads `per_task_event_counter()` in the comments closure — confirmed, no fallback patch needed.

The implementation followed the plan's `<action>` and `<behavior>` blocks verbatim.

**Worktree base correction:** On agent start, the worktree HEAD was at `d67e0318` (pre-kanban-crate). Per the `<worktree_branch_check>` instruction, hard-reset to `d2e51d527` (the orchestrator-mandated base). Verified all referenced files (`store.rs::add_comment`, `events.rs::KanbanEventKind`, `kanban_drawer.rs`, plan file) became present after the reset.

## Deferred Items

See `.planning/quick/260602-nd7-fix-u9-drawer-comments-auto-refresh/deferred-items.md`:

- **DEFER-1:** `ironhermes-kanban::end_to_end.rs` 2 test failures at base (env-var race or dispatcher regression, unrelated to comments).
- **DEFER-2:** Rust 1.94 clippy lint upgrade — 37+ pre-existing lint errors across `ironhermes-core` and `ironhermes-kanban` (auto-fix suggestions available).
- **DEFER-3:** `ironhermes-cli::setup.rs::backfill_uses_process_env_when_dotenv_absent` env-var race.

All three reproducible on the unmodified `d2e51d52` base before any nd7 changes are applied.

## TDD Gate Compliance

The plan annotated Tasks 1-3 with `tdd="true"` but ordered the commits as
**feat → test (producer) → test (consumer)** — not the classic
RED → GREEN ordering. Per the plan author's `<done>` block on Task 2:
"Re-running with the Task 1 fix REVERTED ... confirms the tests genuinely
lock the producer contract — restore the fix before continuing.
**This step is a one-time manual revert/restore sanity check; do NOT
commit the reverted state.**" — the test is explicitly designed as a
regression-locker, not a TDD driver.

The bilateral revert/restore was performed (see Task 2 section above)
and the producer tests demonstrably fail without the fix, satisfying
the GSD execution-flow's TDD-gate intent (test demonstrably constrains
behavior) without the canonical RED commit. Per the executor's
TDD-gate guidance: if RED gate commits are missing, add a warning to
SUMMARY.md — this section is that warning.

No additional action is needed; the contract is locked bilaterally and
the producer test was empirically demonstrated to fail against the
unfixed code.

## Manual U9 Verification (optional, deferred to operator)

End-to-end repro from the plan's `<verification>` step 4:

```bash
# Terminal 1 — launch the dev server with the dashboard plugin
cd /Users/twilson/code/ironhermes && dx serve --features server -p iron_hermes_ui

# Browser — navigate to /kanban, click any task card to open the drawer

# Terminal 2 — write a comment via the CLI
cargo run -p ironhermes-cli -- kanban comment <task_id_from_drawer> "post-fix live test"
```

**Expected behavior (post-fix):** Within ~500ms the COMMENTS section of the
open drawer refreshes to show the new comment WITHOUT the drawer closing.
The 200ms-debounced `per_task_event_counter` for that task_id bumps; the
comments `use_resource` re-runs; the fresh comment list renders.

This is a sanity check only — the automated tests in `comment_appends_event.rs`
and `kanban_drawer.rs` are the binding contract.

## Self-Check: PASSED

- `crates/ironhermes-kanban/src/store.rs` — modified, contains `INSERT INTO task_events` via `append_event_internal` inside `add_comment` transaction: **FOUND**
- `crates/ironhermes-kanban/tests/comment_appends_event.rs` — exists, 3 tests, all green: **FOUND**
- `crates/iron_hermes_ui/tests/kanban_drawer.rs` — contains `comments_resource_reads_per_task_event_counter_for_d21`: **FOUND**
- Commit `fba3c5c1` (fix store.rs): **FOUND in git log**
- Commit `e62c7997` (test producer): **FOUND in git log**
- Commit `4e126b64` (test consumer): **FOUND in git log**
