---
phase: 36-gateway-running-agent-guard-wiring-completes-gw-05
plan: "01"
subsystem: gateway
tags:
  - gateway
  - testing
  - scaffolding
  - gw-05

dependency_graph:
  requires: []
  provides:
    - "GW-05 Wave-0 test scaffold (11 ignored stubs + helpers)"
    - "RecordingPlatformAdapter for Plan 36-02 integration tests"
    - "D-02 locked error string in d02_error_message() helper"
  affects:
    - "crates/ironhermes-gateway/tests/running_agent_guard_tests.rs"

tech_stack:
  added: []
  patterns:
    - "Integration test file in tests/ directory (Cargo conventions) — links against public lib surface"
    - "RecordingPlatformAdapter with TokioMutex<Vec<(String, String)>> capture log"
    - "build_test_session_store mirrors handler.rs:1261 in-memory StateStore pattern"

key_files:
  created:
    - "crates/ironhermes-gateway/tests/running_agent_guard_tests.rs"
  modified: []

decisions:
  - "make_handler() at handler.rs:1226 is inside #[cfg(test)] mod tests{} — not reachable from integration tests. Replicated construction logic in build_test_handler() with inline comment pointing at source pattern."
  - "Cargo.toml dev-dependencies unchanged — tokio, async-trait already present as workspace deps in [dependencies]; all required types available."
  - "test_session_key uses Platform::Telegram with user 'u1' per plan spec."
  - "All 11 stubs use todo!() not panic!() so accidental un-ignore produces an actionable diagnostic."

metrics:
  duration: "4 min"
  completed: "2026-05-24"
  tasks_completed: 1
  files_modified: 1
---

# Phase 36 Plan 01: Wave-0 Test Scaffold for GW-05 Running-Agent Guard — Summary

Wave-0 integration test scaffold for the GW-05 running-agent guard: 11 named `#[ignore]` stubs covering all sub-behaviors from 36-VALIDATION.md, plus compile-ready helpers that Plan 36-02 will need to make the tests pass.

## What Was Built

Created `crates/ironhermes-gateway/tests/running_agent_guard_tests.rs` containing:

**Helpers (`mod helpers`):**
- `build_test_session_store()` — in-memory `SessionStore` wrapped in `Arc<RwLock<...>>`, mirroring `handler.rs:1261`
- `RecordingPlatformAdapter` — captures `(chat_id, text)` on every `send_message`; all other `PlatformAdapter` methods are no-ops returning `Ok(Default::default())`
- `build_test_handler(store)` — replicates `make_handler()` construction (see below for discovery)
- `test_session_key(chat_id)` — `SessionKey::new(Telegram, chat_id).with_user("u1")`
- `d02_error_message()` — returns exact locked D-02 string byte-for-byte

**11 `#[ignore]`'d test stubs:**

| # | Test Name | GW-05 Sub-behavior | D-ref |
|---|-----------|-------------------|-------|
| 1 | `test_session_isolation` | Session A Running does not block Session B | HIGH-2 |
| 2 | `test_model_rejected_when_running` | `/model` rejected during active turn | D-04, D-02 |
| 3 | `test_stop_bypasses_guard` | `/stop` dispatches when running | D-01 |
| 4 | `test_new_bypasses_guard` | `/new` dispatches when running | D-01 |
| 5 | `test_status_bypasses_guard` | `/status` dispatches when running | D-01 |
| 6 | `test_queue_bypasses_guard` | `/queue` dispatches when running | D-01 |
| 7 | `test_guard_clears_on_success` | Flag clears after `run_agent` Ok | D-06 |
| 8 | `test_guard_clears_on_error` | Flag clears after `run_agent` Err | D-06 |
| 9 | `test_alias_bypasses_guard` | `/reset` (alias→`new`) bypasses guard | D-01 |
| 10 | `test_freetext_rejected_when_running` | Free-text rejected when running | Pitfall 1 |
| 11 | `test_stop_reads_real_flag` | `cmd_stop` reads real per-session flag | GW-05-11 |

## Discovery Notes (from read_first pass)

**`make_handler()` reachability:** The function at `handler.rs:1226` lives inside `#[cfg(test)] mod tests {}` and has no explicit `pub` visibility modifier beyond `fn`. It is NOT reachable from `tests/` integration test files. Per plan instructions, construction logic was replicated inline in `build_test_handler()` with a comment pointing to `handler.rs:1226` as the source pattern. Production `make_handler` was NOT modified.

**Cargo.toml dev-dependencies:** Inspected existing `[dev-dependencies]` — the block was empty (a single comment line). However, all required types (`tokio`, `async-trait`, `anyhow`) are in `[dependencies]` as workspace deps, making them available to test binaries without explicit dev-dep addition. No additions needed.

**`GatewaySession.model` field:** Current struct at `session.rs:48` has `pub model: String` (not `Option<String>` as the plan's interfaces section suggested). The test helper uses `"model"` as the literal model string — no impact on scaffold.

**`PlatformAdapter::send_message` return type:** Returns `Result<MessageResponse>` (not `SentMessage`). `MessageResponse` has fields `message_id: String`, `chat_id: String`, `platform: Platform`. `RecordingPlatformAdapter::send_message` constructs a stub `MessageResponse` with `message_id = "stub-msg-id"`.

## Verification Results

```
running 11 tests
test test_alias_bypasses_guard ... ignored, Wave 0 scaffold ...
test test_freetext_rejected_when_running ... ignored, Wave 0 scaffold ...
test test_guard_clears_on_error ... ignored, Wave 0 scaffold ...
test test_guard_clears_on_success ... ignored, Wave 0 scaffold ...
test test_model_rejected_when_running ... ignored, Wave 0 scaffold ...
test test_new_bypasses_guard ... ignored, Wave 0 scaffold ...
test test_queue_bypasses_guard ... ignored, Wave 0 scaffold ...
test test_session_isolation ... ignored, Wave 0 scaffold ...
test test_status_bypasses_guard ... ignored, Wave 0 scaffold ...
test test_stop_bypasses_guard ... ignored, Wave 0 scaffold ...
test test_stop_reads_real_flag ... ignored, Wave 0 scaffold ...

test result: ok. 0 passed; 0 failed; 11 ignored; 0 measured; 0 filtered out
```

D-02 string grep: exactly 1 hit (in `d02_error_message()` helper only).

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

This is intentionally a scaffold plan — all 11 test bodies are `todo!()`. This is by design (Wave 0). Plan 36-02 implements the production guard behavior; Plan 36-03 removes `#[ignore]` attributes. No stub represents missing plan-goal functionality for THIS plan — the plan's goal is the scaffold itself, which is complete.

## Threat Surface Scan

No new network endpoints, auth paths, file access patterns, or schema changes introduced. Test-only code in `tests/` directory; `RecordingPlatformAdapter` does not ship in the production binary (T-36-SC: accepted per threat model).

## Self-Check: PASSED

- `crates/ironhermes-gateway/tests/running_agent_guard_tests.rs` exists: FOUND
- Commit f0e10a65 exists: VERIFIED
- 11 `#[tokio::test]` annotations: VERIFIED (grep confirms)
- 11 `#[ignore` attributes: VERIFIED
- D-02 string present: VERIFIED (1 hit)
- `RecordingPlatformAdapter` and `impl PlatformAdapter for RecordingPlatformAdapter`: VERIFIED
- `build_test_session_store` and `build_test_handler`: VERIFIED
- No production source modified: VERIFIED (git diff shows only new file)
- `cargo test` result: 0 passed; 0 failed; 11 ignored: VERIFIED
