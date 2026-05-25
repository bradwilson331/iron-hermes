# Phase 32 Plan 01 — Deferred Items

## Out-of-scope test failures discovered during workspace regression

### `ironhermes-cli::chat_memory_persistence::run_chat_and_run_single_both_wire_memory_manager`

- **Status:** Pre-existing failure on the worktree base commit (`3acd8f9a`,
  before any phase 32 plan 01 edits) — not introduced by this plan.
- **Verified:**
  `git show HEAD~2:crates/ironhermes-cli/src/main.rs | grep -c register_memory_tool`
  returns `2` (baseline). Current HEAD also returns `2`. The test asserts
  `register_count >= 3` (expects gateway rpc + gateway main + chat + single).
- **Symptom:** `expected >=3 register_memory_tool calls (gateway rpc +
  gateway main + chat + single); got 2`.
- **Scope-boundary disposition:** This is a static-grep regression test for
  earlier-phase memory wiring (Plan 20-03 GAP-4 / Phase 21.4). Plan 32-01's
  only `main.rs` edit is the nudge fire site + counter declarations; the
  test predicate is orthogonal to nudge wiring. Surfacing for a future
  memory-wiring follow-up — the assistant must not retro-add a
  `register_memory_tool` call inside an in-scope nudge plan.

### `iron_hermes_ui::server_runtime_parity::api_sessions_and_tools_are_backed_by_real_state`

- **Status:** Pre-existing failure on the worktree base commit (`3acd8f9a`, before any
  phase 32 plan 01 edits) — not introduced by this plan.
- **Verified:** the test is a static-grep assertion against
  `crates/iron_hermes_ui/src/server/api.rs`, which is completely outside this plan's
  modified-file set (`ironhermes-core/config.rs`, `ironhermes-core/wizard.rs`,
  `ironhermes-agent/lib.rs`, `ironhermes-agent/nudge.rs`,
  `ironhermes-cli/src/main.rs`).
- **Symptom:** `list_sessions must query StateStore for Platform::Web sessions` —
  the static grep expects `api.rs` to contain `list_sessions(` AND
  `Platform::Web.to_string()`. Counting via grep returns 3 occurrences of
  `list_sessions|Platform::Web` across api.rs but the dual-pattern AND check fails
  (likely a refactor that split the two strings across helpers).
- **Scope-boundary disposition:** Out of scope per the executor's scope rule.
  No phase 32 file touches `iron_hermes_ui`. Surfacing for a future
  `26.x` / `iron_hermes_ui` follow-up plan.
