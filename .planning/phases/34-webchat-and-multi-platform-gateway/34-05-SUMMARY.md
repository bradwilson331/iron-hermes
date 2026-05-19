---
phase: 34-webchat-and-multi-platform-gateway
plan: "05"
subsystem: ironhermes-gateway
tags: [multi-platform, discord, slack, telegram, gateway-runner, wiring, invariants]
dependency_graph:
  requires: [34-03, 34-04]
  provides: [multi-platform-runner-wiring]
  affects: [crates/ironhermes-gateway/src/runner.rs, crates/ironhermes-gateway/tests/invariants_34.rs]
tech_stack:
  added: []
  patterns:
    - "resolve_token_with_env() helper — per-platform env-var fallback, avoids TELEGRAM_BOT_TOKEN cross-contamination"
    - "JoinSet multi-platform spawn — Discord + Slack added alongside Telegram poll loop"
    - "Silent-skip pattern — unwrap_or_default() + resolve_token_with_env() returns None when config absent"
    - "include_str! static grep invariants — RUNNER_SOURCE const locks spawn call sites at compile time"
key_files:
  modified:
    - crates/ironhermes-gateway/src/runner.rs
    - crates/ironhermes-gateway/tests/invariants_34.rs
decisions:
  - "resolve_token_with_env() added instead of modifying resolve_token() — avoids changing the existing Telegram fallback behavior while giving Discord/Slack their own env-var fallbacks (DISCORD_BOT_TOKEN, SLACK_APP_TOKEN, SLACK_BOT_TOKEN)"
  - "Empty-whitelist is propagated to adapters unchanged (not a spawn gate) — mirrors Telegram's existing spawn-then-deny-in-adapter semantics (D-12). Consistent cross-platform operator UX: all three platforms log 'adapter spawning' then 'Whitelist is empty — denying all messages (D-12)' per message"
  - "Slack whitelist converts Vec<i64> to Vec<String> via to_string() — Slack-native user IDs are alphanumeric strings (e.g. U012AB3CD); operators currently configure i64 in PlatformGatewayConfig. Migrating whitelist field to Vec<String> is a deferred config-schema improvement"
  - "Token strings never appear in tracing macros — only whitelist_len and binary spawning/skipped status logged (T-34-01 mitigation)"
metrics:
  duration_minutes: 15
  completed_date: "2026-05-19"
  tasks_completed: 2
  files_changed: 2
---

# Phase 34 Plan 05: Multi-Platform Runner Wiring Summary

Wave 3 final wiring — GatewayRunner::start() now conditionally spawns DiscordAdapter and SlackAdapter alongside the existing Telegram poll loop, using the same JoinSet and CancellationToken shutdown machinery, with INV-34-03/04 locking the call sites.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Add optional Discord + Slack spawns to GatewayRunner::start() | 3be1848a | crates/ironhermes-gateway/src/runner.rs |
| 2 | Add INV-34-03/04 to invariants_34.rs (lock runner wiring) | 9967eb36 | crates/ironhermes-gateway/tests/invariants_34.rs |

## What Was Built

**Task 1 — runner.rs multi-platform wiring:**

Two new conditional spawn blocks were inserted after the Telegram poll loop spawn (section 7b/7c) and before the dispatch loop (section 8):

- **Discord block (section 7b):** Looks up `platforms.get("discord").unwrap_or_default()`, resolves token via `resolve_token_with_env(&discord_config.token, "DISCORD_BOT_TOKEN")`. If token resolves: converts `Vec<i64>` whitelist to `Vec<u64>`, logs `whitelist_len`, spawns `run_discord_adapter(...)` into the JoinSet. If token absent: `tracing::debug!("Discord adapter skipped...")`.

- **Slack block (section 7c):** Same shape but gates on `if let (Some(slack_app), Some(slack_bot))` — both app_token and bot_token must resolve (T-34-PITFALL-2 mitigation). Converts `Vec<i64>` to `Vec<String>` for Slack's whitelist parameter.

- **resolve_token_with_env():** New private helper with the same `${ENV_VAR}` interpolation logic as `resolve_token()` but with a caller-specified fallback env var. Prevents cross-contamination where a Telegram-only deployment's `TELEGRAM_BOT_TOKEN` would be incorrectly picked up as the Discord or Slack token when those config sections are absent.

**Task 2 — invariants_34.rs locks:**

- Added `const RUNNER_SOURCE: &str = include_str!("../src/runner.rs");`
- `inv_34_03_runner_spawns_discord`: asserts `RUNNER_SOURCE.matches("run_discord_adapter").count() >= 1`
- `inv_34_04_runner_spawns_slack`: asserts `RUNNER_SOURCE.matches("run_slack_adapter").count() >= 1`
- Extended file-header `//!` doc with INV-34-03/04 explanations (D-10/D-11/D-14/D-15 trilogy lock)

## Verification Results

```
cargo build -p ironhermes-gateway       → Finished (0 errors, 4 pre-existing warnings)
cargo test -p ironhermes-gateway --lib  → 75 passed; 0 failed
cargo test -p ironhermes-gateway --test invariants_34 → 4 passed; 0 failed
  inv_34_01_discord_routes_through_handle_with_multimodal ... ok
  inv_34_02_slack_routes_through_handle_with_multimodal ... ok
  inv_34_03_runner_spawns_discord ... ok
  inv_34_04_runner_spawns_slack ... ok
```

## Telegram-Only Deployment Compatibility

Existing Telegram-only deployments (no `gateway.platforms.discord` or `gateway.platforms.slack` sections in config.yaml) are unaffected:

1. `platforms.get("discord").cloned().unwrap_or_default()` returns a default `PlatformGatewayConfig` with `token: None`
2. `resolve_token_with_env(&None, "DISCORD_BOT_TOKEN")` returns `None` when `DISCORD_BOT_TOKEN` is not set
3. The `if let Some(discord_token)` branch is skipped; `tracing::debug!("Discord adapter skipped...")` fires
4. Same pattern for Slack
5. The existing Telegram `resolve_token()` path is completely unchanged — no modifications to the Telegram startup sequence

This matches the plan's "Missing config = silent skip (NOT an error)" requirement.

## Deviations from Plan

**1. [Rule 2 - Missing Critical Functionality] Added resolve_token_with_env() helper**

- **Found during:** Task 1 implementation
- **Issue:** The existing `resolve_token()` falls back to `TELEGRAM_BOT_TOKEN` when the token field is `None`. If the plan's action were followed literally (using `resolve_token(&discord_config.token)`), a Telegram-only deployment with `TELEGRAM_BOT_TOKEN` set but no Discord config section would accidentally attempt to spawn a Discord adapter using the Telegram token, which would fail at Discord API authentication but still cause a confusing error log.
- **Fix:** Added `resolve_token_with_env(token: &Option<String>, env_var: &str)` with a caller-specified fallback. Discord/Slack spawn blocks use this with `"DISCORD_BOT_TOKEN"`, `"SLACK_APP_TOKEN"`, `"SLACK_BOT_TOKEN"` respectively. The original `resolve_token()` is preserved unchanged for the Telegram path.
- **Files modified:** `crates/ironhermes-gateway/src/runner.rs`
- **Commit:** 3be1848a

## Known Stubs

**Slack whitelist type mismatch (documented deferral):**

`PlatformGatewayConfig.whitelist` is `Vec<i64>` but `run_slack_adapter` expects `Vec<String>`. The runner converts via `.map(|v| v.to_string())`. Slack-native user IDs are alphanumeric (e.g. `U012AB3CD`); operators configuring numeric IDs will get string comparisons against Slack's actual IDs which will never match. This is an existing config-schema limitation — the correct fix is adding a `Vec<String>` whitelist variant or a separate `slack_whitelist` field in `PlatformGatewayConfig`. Tracked as a deferred config-schema improvement.

## Threat Surface Scan

No new network endpoints, auth paths, or trust boundaries introduced beyond what is documented in the plan's `<threat_model>`:

- T-34-01 (token logging): Mitigated — `discord_token`, `slack_app`, `slack_bot` never appear as arguments to tracing macros (grep-asserted in acceptance criteria)
- T-34-02 (whitelist propagation): Mitigated — runner passes whitelist by value; adapters enforce deny-all
- T-34-PITFALL-2 (Slack two-token): Mitigated — `if let (Some(slack_app), Some(slack_bot))` pattern
- T-34-PHASE-SC (future refactor removes calls): Mitigated — INV-34-03/04 lock call sites

## Self-Check: PASSED

- [x] `crates/ironhermes-gateway/src/runner.rs` — exists and contains `run_discord_adapter`, `run_slack_adapter`, `resolve_token_with_env`
- [x] `crates/ironhermes-gateway/tests/invariants_34.rs` — exists and contains `RUNNER_SOURCE`, `inv_34_03_runner_spawns_discord`, `inv_34_04_runner_spawns_slack`
- [x] Commit `3be1848a` — exists (Task 1)
- [x] Commit `9967eb36` — exists (Task 2)
- [x] 4/4 invariants green
- [x] Build clean
