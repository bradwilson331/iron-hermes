---
phase: 34-webchat-and-multi-platform-gateway
plan: "01"
subsystem: invariant-tests
tags: [invariants, tdd, wave-0, red-gate, discord, slack, web-ui, platform]
dependency_graph:
  requires: []
  provides:
    - INV-33-07 (build_app_runtime_bundle lock in state.rs)
    - INV-33-08 (spawn_nudge_review + nudge_turns lock in state.rs)
    - INV-34-01 (Discord → handle_with_multimodal gate)
    - INV-34-02 (Slack → handle_with_multimodal gate)
    - web_session_keyed_by_platform_web (Platform::Web keying lock)
  affects:
    - crates/ironhermes-gateway/src/discord.rs (Wave 2/3 must satisfy INV-34-01)
    - crates/ironhermes-gateway/src/slack.rs (Wave 3 must satisfy INV-34-02)
tech_stack:
  added: []
  patterns:
    - include_str! compile-time static-grep invariant (established by invariants_33.rs)
key_files:
  modified:
    - crates/ironhermes-agent/tests/invariants_33.rs
  created:
    - crates/ironhermes-gateway/tests/invariants_34.rs
    - crates/iron_hermes_ui/tests/session_store_shared_with_gateway.rs
decisions:
  - "Wave 0 RED gate for invariants_34.rs is intentional — discord.rs/slack.rs do not exist yet; compile error is the gate contract"
  - "INV-33-08 uses .contains() (not .matches().count()) for clearer per-string failure messages per plan spec"
  - "WEB_STATE_SOURCE const shared between INV-33-07 and INV-33-08 to avoid duplicate include_str! declarations"
metrics:
  duration: "~12 minutes"
  completed: "2026-05-19"
  tasks: 3
  files: 3
requirements: [LEARN-01, LEARN-02, LEARN-03, LEARN-04, LEARN-05]
---

# Phase 34 Plan 01: Wave 0 Invariant Scaffolds Summary

**One-liner:** Five static-grep invariants locking web-path build_app_runtime_bundle, nudge fire site, Discord/Slack handle_with_multimodal routing, and Platform::Web session keying — Wave 0 RED gate established for Discord/Slack (compile error until Wave 3).

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Add INV-33-07 + INV-33-08 to invariants_33.rs | 703aa92e | crates/ironhermes-agent/tests/invariants_33.rs |
| 2 | Create invariants_34.rs (Wave 0 RED gate) | d8fda29a | crates/ironhermes-gateway/tests/invariants_34.rs |
| 3 | Create session_store_shared_with_gateway.rs | a0735422 | crates/iron_hermes_ui/tests/session_store_shared_with_gateway.rs |

## What Was Built

### Task 1 — INV-33-07 + INV-33-08 (invariants_33.rs extended)

Extended `crates/ironhermes-agent/tests/invariants_33.rs` with:

- Extended `//!` file-header doc with two new invariant purpose bullets (INV-33-07, INV-33-08).
- `const WEB_STATE_SOURCE: &str = include_str!("../../iron_hermes_ui/src/server/state.rs");` — shared const for both new tests.
- `inv_33_07_appstate_calls_build_app_runtime_bundle`: asserts `build_app_runtime_bundle` appears at least once in state.rs, locking Phase 34 D-05 (web turns inherit full skill_manage toolset via shared AppRuntimeBundle).
- `inv_33_08_web_nudge_fire_site_exists`: asserts both `spawn_nudge_review` AND `nudge_turns` are present in state.rs, locking Phase 32 Plan 03 / Phase 34 Success Criterion 1 (web turns trigger Learning Loop memory nudge).

All 8 invariants_33 tests pass (`cargo test -p ironhermes-agent --test invariants_33` → 8 passed).

### Task 2 — invariants_34.rs scaffold (Wave 0 RED gate)

Created `crates/ironhermes-gateway/tests/invariants_34.rs` containing:

- `//!` file-header block explaining the Wave 0 RED gate contract and why each invariant exists.
- `const DISCORD_SOURCE: &str = include_str!("../src/discord.rs");`
- `const SLACK_SOURCE: &str = include_str!("../src/slack.rs");`
- `inv_34_01_discord_routes_through_handle_with_multimodal`: asserts `DISCORD_SOURCE.matches("handle_with_multimodal").count() >= 1`, citing Phase 34 D-10.
- `inv_34_02_slack_routes_through_handle_with_multimodal`: asserts `SLACK_SOURCE.matches("handle_with_multimodal").count() >= 1`, citing Phase 34 D-11.

**Wave 0 RED gate confirmed:** `cargo test -p ironhermes-gateway --test invariants_34` fails with:

```
error: couldn't read `crates/ironhermes-gateway/tests/../src/discord.rs`: No such file or directory (os error 2)
error: couldn't read `crates/ironhermes-gateway/tests/../src/slack.rs`: No such file or directory (os error 2)
error: could not compile `ironhermes-gateway` (test "invariants_34") due to 2 previous errors
```

This compile failure is the intended gate. Wave 2 must create `discord.rs` with `handle_with_multimodal`, and Wave 3 must create `slack.rs` with `handle_with_multimodal` to flip both tests to GREEN. No `#[cfg(...)]` guard was used — the failure is the gate.

### Task 3 — session_store_shared_with_gateway.rs

Created `crates/iron_hermes_ui/tests/session_store_shared_with_gateway.rs` containing:

- `const STATE_SOURCE: &str = include_str!("../src/server/state.rs");`
- `web_session_keyed_by_platform_web`: asserts `Platform::Web` appears at least once in state.rs, locking Phase 34 D-07/D-08 (web sessions keyed by Platform::Web in StateStore).

Test passes immediately (`cargo test -p iron_hermes_ui --test session_store_shared_with_gateway web_session_keyed_by_platform_web` → 1 passed) because state.rs:199 already uses `&Platform::Web.to_string()` in `ensure_web_session`.

## Wave 0 RED Gate Record

**Status:** ESTABLISHED — Wave 3 must flip to GREEN.

| File | Gate | Expected Failure | Must Go GREEN In |
|------|------|-----------------|-----------------|
| `crates/ironhermes-gateway/tests/invariants_34.rs` | compile error: missing discord.rs + slack.rs | `couldn't read ../src/discord.rs: No such file or directory` | Wave 3 (when discord.rs + slack.rs land with `handle_with_multimodal`) |

Wave 2/3 executors must verify: after creating `src/discord.rs` and `src/slack.rs`, run `cargo test -p ironhermes-gateway --test invariants_34` — both INV-34-01 and INV-34-02 must pass.

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None — all tests are structural invariants with no stub data paths.

## Threat Flags

None — this plan adds test-only files that read production sources as read-only strings at compile time. No new network endpoints, auth paths, file access patterns, or schema changes at trust boundaries were introduced.

## Self-Check: PASSED

| Check | Result |
|-------|--------|
| crates/ironhermes-agent/tests/invariants_33.rs exists | FOUND |
| crates/ironhermes-gateway/tests/invariants_34.rs exists | FOUND |
| crates/iron_hermes_ui/tests/session_store_shared_with_gateway.rs exists | FOUND |
| .planning/phases/34-webchat-and-multi-platform-gateway/34-01-SUMMARY.md exists | FOUND |
| commit 703aa92e (Task 1) exists | FOUND |
| commit d8fda29a (Task 2) exists | FOUND |
| commit a0735422 (Task 3) exists | FOUND |
