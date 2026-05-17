---
phase: 34
slug: webchat-and-multi-platform-gateway
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-05-17
---

# Phase 34 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | `cargo test` (built-in Rust test runner; integration crates use `tokio::test`) |
| **Config file** | `Cargo.toml` (workspace-level) |
| **Quick run command** | `cargo test -p ironhermes-gateway -p iron_hermes_ui` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~120 seconds for quick; ~300 seconds full |

---

## Sampling Rate

- **After every task commit:** Run quick command for the touched crate (`cargo test -p <crate>`)
- **After every plan wave:** Run `cargo test -p ironhermes-gateway -p iron_hermes_ui`
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** ~120 seconds

---

## Per-Task Verification Map

> Filled by planner. Each plan's tasks land here with their automated command + Wave 0 dependency status.

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 34-01-01 | 01 | 0 | LEARN-01..05 | — | INV-33-07 confirms `AppState::new` calls `build_app_runtime_bundle` | unit | `cargo test -p ironhermes-gateway --test invariants_33 inv_33_07` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-gateway/tests/invariants_33.rs` — extend with `inv_33_07_app_state_calls_build_app_runtime_bundle` (include_str! against `iron_hermes_ui/src/server/state.rs`)
- [ ] Discord/Slack adapter test scaffolds — `crates/ironhermes-gateway/tests/discord_adapter_routes_through_handler.rs`, `slack_adapter_routes_through_handler.rs`
- [ ] Unified session store test — `crates/iron_hermes_ui/tests/session_store_shared_with_gateway.rs`

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Discord bot receives a real message and emits an agent reply | LEARN-01..05 (Discord parity) | Requires external Discord Developer Portal app + Message Content Intent enabled; cannot be CI-automated without bot account secrets | Owner runs `cargo run --bin ironhermes-gateway -- --discord-token $DISCORD_BOT_TOKEN`; mentions bot in test channel; confirms reply within 5s |
| Slack bot receives a real message via Socket Mode and emits an agent reply | LEARN-01..05 (Slack parity) | Requires Slack app with `xapp-` (Socket Mode) + `xoxb-` (bot) tokens; cannot run in CI without secrets | Owner runs gateway with `SLACK_APP_TOKEN` + `SLACK_BOT_TOKEN`; DMs bot in test workspace; confirms reply within 5s |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 120s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
