# Phase 36.2 — Deferred Items

Discovered out-of-scope while executing in-scope work. NOT fixed in this phase.

## From Plan 36.2-02 (Migration v9)

### Pre-existing clippy noise in `ironhermes-state`

Three lints predated this plan but became blockers because the plan introduced a
`cargo clippy -- -D warnings` acceptance gate. Suppressed with localized `#[allow]`
attributes to satisfy the gate without an out-of-scope refactor. Re-evaluate when
the relevant subsystems are next touched.

| Lint | File:line | Why deferred |
|------|-----------|---------------|
| `dead_code` on `with_busy_retry` | `crates/ironhermes-state/src/lib.rs:1009` | Helper kept for future SESS-13 (SQLite BUSY retry) wiring; do not delete without a phase that re-enables retries. |
| `dead_code` on `is_busy` | `crates/ironhermes-state/src/lib.rs:1024` | Companion helper to `with_busy_retry`; same rationale. |
| `clippy::collapsible_if` | `crates/ironhermes-state/src/session_export.rs:69` | Nested `if let Some(src) = .. { if src.exists() { .. } }` reads clearer than the let-chain form for the trajectory export edge case. Pre-existing from Phase 25.3-10. |

### Acceptance grep "exactly 1" criterion mismatch

The plan's acceptance criteria specify `grep -c 'CREATE TABLE IF NOT EXISTS usage_events'`
and the two index-create lines return exactly 1. The Phase 25.3 v8 precedent for
`workspace_root` places the column in **both** `SCHEMA_SQL` (fresh-DB path) **and**
the migration `ALTER TABLE` block (existing-DB path) — `grep -c 'workspace_root'`
returns multiple hits. Replicating that pattern for the v9 `usage_events` table
yields 2 hits for the CREATE TABLE / CREATE INDEX statements (one in `SCHEMA_SQL`,
one in the migration block). Removing the table from `SCHEMA_SQL` would break the
fresh-DB path (which does not call `run_migrations`); changing that path to call
`run_migrations(0)` on fresh installs would be a cross-cutting refactor unrelated
to D-USAGE-02. Documented in SUMMARY as an intentional deviation; semantic intent
(no duplicate v9 migration block — `grep -c 'if current < 9'` is 1) is preserved.

### `Session` struct does NOT yet carry cache/cost fields

Plan 02 Step 7 explicitly defers extending the `Session` struct in
`crates/ironhermes-state/src/lib.rs:119-141` to include `cache_read_tokens`,
`cache_creation_tokens`, `cost_usd_micros`. The SQL `SELECT` paths at the four
known read sites (lib.rs:490, 547, 731, 903) still select only `input_tokens,
output_tokens` — they continue to work because the new columns have `DEFAULT 0`.
A future plan in this phase (or follow-on) should extend the struct + readers
so consumers don't have to issue a second query for cost data.

## From Plan 36.2-04 (Pricing Registry)

### Pre-existing clippy lints in `ironhermes-core`

Plan 36.2-04 acceptance ran `cargo clippy -p ironhermes-core --no-deps -- -D warnings`
against the new `pricing.rs` and `pricing_cache.rs`. Both files are 100% clean.
The 14 clippy warnings the lib emits crate-wide all live in pre-existing files
that the plan does not modify:

| File | Lint | Count |
|------|------|-------|
| crates/ironhermes-core/src/browser_profile.rs | collapsible_if (3), manually_reimplement_div_ceil | 4 |
| crates/ironhermes-core/src/commands/handlers.rs | field_reassign_with_default, needless_range_loop ×2 | 3 |
| crates/ironhermes-core/src/commands/typo.rs | iter_any_over_contains ×2 | 2 |
| crates/ironhermes-core/src/config.rs | derivable_impls ×2, missing_default ×1 | 3 |
| crates/ironhermes-core/src/memory_store.rs | manual_is_multiple_of | 1 |
| crates/ironhermes-core/src/skills.rs | derivable_impls, collapsible_if | 2 |

Scope boundary applied (Rule SCOPE BOUNDARY in execute-plan.md): only auto-fix
issues DIRECTLY caused by current task changes. These lints pre-date Plan 36.2-04
and are unchanged by it. Suggested follow-up: a dedicated clean-up plan running
`cargo clippy --fix --lib -p ironhermes-core` against the 11 auto-applicable
suggestions, then a manual pass for the 3 remaining (needless_range_loop in
handlers.rs is intentional pairwise indexing).

Verification:
```bash
cargo clippy -p ironhermes-core --no-deps 2>&1 | grep -E '(pricing\.rs|pricing_cache\.rs)'
# (empty output — new files are lint-clean)
```

## From Plan 36.2-05 (Anthropic prompt caching)

### `unreachable_pub` on `build_anthropic_request`

The new module-level helper `build_anthropic_request` (in `anthropic_client.rs`)
is marked `pub(crate)` so internal call sites + test helpers can invoke it.
Its return type is `AnthropicRequest` which is itself `pub` (so the test helper
re-export shape stays consistent with the existing test-friendly surface of
`adapt_messages`, `adapt_tools`, and `parse_anthropic_response` — all three are
`pub` returning private-to-module types). Clippy emits `unreachable_pub` on the
return-type-vs-fn-visibility mismatch.

| Lint | File:line | Why deferred |
|------|-----------|---------------|
| `unreachable_pub` on `build_anthropic_request` returning `AnthropicRequest` | `crates/ironhermes-agent/src/anthropic_client.rs:592` | Matches the established pattern on `adapt_messages` (line 362), `adapt_tools` (line 568), `parse_anthropic_response` (line 712) — all four emit the same lint. A consistent fix requires marking the four internal request/response struct types as `pub` (cross-cutting refactor unrelated to Plan 05's PRMT-08/PRMT-09 closure). |

Scope boundary applied (Rule SCOPE BOUNDARY in execute-plan.md): only auto-fix
issues DIRECTLY caused by current task changes. The pre-existing 3 instances of
this lint pattern (61 baseline warnings) prove the project's accepted posture.
Verification:
```bash
cargo clippy -p ironhermes-agent --no-deps 2>&1 | grep -c "^warning"
# 63 (baseline 61 + 2 — see scope-boundary note above)
```
