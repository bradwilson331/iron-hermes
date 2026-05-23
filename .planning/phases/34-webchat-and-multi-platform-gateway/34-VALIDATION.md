---
phase: 34
slug: webchat-and-multi-platform-gateway
status: ready
nyquist_compliant: true
wave_0_complete: false
created: 2026-05-17
revised: 2026-05-17
---

# Phase 34 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | `cargo test` (built-in Rust test runner; integration crates use `tokio::test`) |
| **Config file** | `Cargo.toml` (workspace-level) |
| **Quick run command** | `cargo test -p ironhermes-gateway -p iron_hermes_ui -p ironhermes-agent` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~120 seconds for quick; ~300 seconds full |

---

## Sampling Rate

- **After every task commit:** Run quick command for the touched crate (`cargo test -p <crate>`)
- **After every plan wave:** Run `cargo test -p ironhermes-gateway -p iron_hermes_ui -p ironhermes-agent`
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** ~120 seconds

---

## Per-Task Verification Map

> Each task across Plans 34-01..05. File-Exists column flags Wave 0 dependency: ❌ W0 = file does not exist yet at planning time and must be produced by an earlier wave; ✅ = file already exists and will be modified in place.

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-----------------|-------------|--------|
| 34-01-01 | 01 | 0 | LEARN-01..05 | T-34-PHASE-SC | INV-33-07 grep locks AppState→build_app_runtime_bundle; INV-33-08 grep locks web nudge fire site (spawn_nudge_review + nudge_turns) | grep-static | `cargo test -p ironhermes-agent --test invariants_33 inv_33_07_appstate_calls_build_app_runtime_bundle && cargo test -p ironhermes-agent --test invariants_33 inv_33_08_web_nudge_fire_site_exists` | ✅ | ⬜ pending |
| 34-01-02 | 01 | 0 | LEARN-01..05 | T-34-PHASE-SC | INV-34-01/02 scaffold gates Wave 3 — RED until discord.rs/slack.rs exist | grep-static (compile-RED gate) | `test -f crates/ironhermes-gateway/tests/invariants_34.rs && grep -q "inv_34_01_discord_routes_through_handle_with_multimodal" crates/ironhermes-gateway/tests/invariants_34.rs && grep -q "inv_34_02_slack_routes_through_handle_with_multimodal" crates/ironhermes-gateway/tests/invariants_34.rs && grep -q 'include_str!.*discord\.rs' crates/ironhermes-gateway/tests/invariants_34.rs && grep -q 'include_str!.*slack\.rs' crates/ironhermes-gateway/tests/invariants_34.rs` | ❌ W0 (creates scaffold) | ⬜ pending |
| 34-01-03 | 01 | 0 | LEARN-01..05 | T-34-PHASE-SC | web_session_keyed_by_platform_web locks Platform::Web in state.rs | grep-static | `cargo test -p iron_hermes_ui --test session_store_shared_with_gateway web_session_keyed_by_platform_web` | ❌ W0 (creates test file) | ⬜ pending |
| 34-02-01 | 02 | 1 | LEARN-01..05 | T-34-SC | Human-verify legitimacy of serenity + slack-morphism crates before install | human-checkpoint | `<human-check>` (blocking-human) | ✅ | ⬜ pending |
| 34-02-02 | 02 | 1 | LEARN-01..05 | T-34-SC, T-34-01 | serenity + slack-morphism deps land; PlatformGatewayConfig.app_token field for Slack two-token shape | build + lib test | `cargo build -p ironhermes-gateway && cargo build -p ironhermes-core && cargo test -p ironhermes-core --lib && grep -n 'pub app_token: Option<String>' crates/ironhermes-core/src/config.rs` | ✅ | ⬜ pending |
| 34-02-03 | 02 | 1 | LEARN-01..05 | T-34-05 | list_sessions queries StateStore filtered by Platform::Web (structural test + behavioral tokio::test round-trip — BLOCKER 6 closure) | integration (tokio::test) | `cargo test -p iron_hermes_ui --test server_runtime_parity api_sessions_and_tools_are_backed_by_real_state && cargo test -p iron_hermes_ui --test list_sessions_returns_platform_web list_sessions_returns_inserted_platform_web_session` | ❌ W0 (creates behavioral test file) | ⬜ pending |
| 34-03-01 | 03 | 2 | LEARN-01..05 | T-34-01, T-34-05 | DiscordAdapter implements PlatformAdapter; discord_message_to_event sets Platform::Discord | unit (lib) | `cargo test -p ironhermes-gateway --lib discord` | ❌ W0 (creates discord.rs) | ⬜ pending |
| 34-03-02 | 03 | 2 | LEARN-01..05 | T-34-01, T-34-02, T-34-03, T-34-PITFALL-3, T-34-PITFALL-5 | run_discord_adapter wires MESSAGE_CONTENT intent + canonical empty-whitelist deny-all + shard shutdown + token-redaction | build + clippy await-holding-lock gate + grep | `cargo build -p ironhermes-gateway && cargo clippy -p ironhermes-gateway --lib -- -D clippy::await_holding_lock && grep -q 'denying all messages (D-12)' crates/ironhermes-gateway/src/discord.rs` | ❌ W0 (extends discord.rs) | ⬜ pending |
| 34-03-03 | 03 | 2 | LEARN-01..05 | T-34-PHASE-SC | INV-34-01 flips RED → GREEN; pub mod discord; re-exported | integration (invariants test) | `cargo test -p ironhermes-gateway --test invariants_34 inv_34_01_discord_routes_through_handle_with_multimodal` | ✅ | ⬜ pending |
| 34-04-01 | 04 | 2 | LEARN-01..05 | T-34-01, T-34-05 | SlackAdapter implements PlatformAdapter; slack_event_to_message_event sets Platform::Slack; classify_slack_channel_type unit-locked | unit (lib) | `cargo test -p ironhermes-gateway --lib slack` | ❌ W0 (creates slack.rs) | ⬜ pending |
| 34-04-02 | 04 | 2 | LEARN-01..05 | T-34-01, T-34-02, T-34-04, T-34-PITFALL-2, T-34-PITFALL-3, T-34-SUPPLY | run_slack_adapter wires Socket Mode + canonical empty-whitelist deny-all + tokio::spawn(async move) for <3s ACK + token-redaction | build + clippy await-holding-lock gate + grep | `cargo build -p ironhermes-gateway && cargo clippy -p ironhermes-gateway --lib -- -D clippy::await_holding_lock && grep -q 'denying all messages (D-12)' crates/ironhermes-gateway/src/slack.rs && grep -q 'async move' crates/ironhermes-gateway/src/slack.rs` | ❌ W0 (extends slack.rs) | ⬜ pending |
| 34-04-03 | 04 | 2 | LEARN-01..05 | T-34-PHASE-SC | INV-34-02 flips RED → GREEN; pub mod slack; re-exported | integration (invariants test) | `cargo test -p ironhermes-gateway --test invariants_34 inv_34_02_slack_routes_through_handle_with_multimodal` | ✅ | ⬜ pending |
| 34-05-01 | 05 | 3 | LEARN-01..05 | T-34-01, T-34-02, T-34-PITFALL-2, T-34-PHASE-SC | runner.rs conditionally spawns Discord + Slack via JoinSet; missing config = silent skip; tokens never logged | build + workspace compile gate + lib test | `cargo build -p ironhermes-gateway && cargo test -p ironhermes-gateway --lib && cargo test --workspace --no-run` | ✅ | ⬜ pending |
| 34-05-02 | 05 | 3 | LEARN-01..05 | T-34-PHASE-SC | INV-34-03/04 lock runner spawn call sites for Discord + Slack | integration (invariants test) | `cargo test -p ironhermes-gateway --test invariants_34` | ❌ W0 (extends invariants_34.rs) | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

**Total: 14 tasks across 5 plans (Waves 0..3).** Every task has an `<automated>` command. The W0 entries are explicit Wave 0 dependency relationships: those test files do not exist until the noted plan creates them — the orchestrator must execute the creating plan before running the listed command. No task uses watch-mode flags.

---

## Wave 0 Requirements

- [x] `crates/ironhermes-agent/tests/invariants_33.rs` — extended with `inv_33_07_appstate_calls_build_app_runtime_bundle` (include_str! against `iron_hermes_ui/src/server/state.rs`) AND `inv_33_08_web_nudge_fire_site_exists` (locks `spawn_nudge_review` + `nudge_turns` per Phase 34 Success Criterion 1) — Plan 34-01 Task 1
- [x] `crates/ironhermes-gateway/tests/invariants_34.rs` — created with INV-34-01/02 scaffolding (RED until Wave 2/3 lands discord.rs/slack.rs) — Plan 34-01 Task 2
- [x] `crates/iron_hermes_ui/tests/session_store_shared_with_gateway.rs` — created with web_session_keyed_by_platform_web — Plan 34-01 Task 3
- [x] `crates/iron_hermes_ui/tests/list_sessions_returns_platform_web.rs` — created with behavioral tokio::test for D-07/D-08 list_sessions round-trip (BLOCKER 6 closure) — Plan 34-02 Task 3

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Discord bot receives a real message and emits an agent reply | LEARN-01..05 (Discord parity) | Requires external Discord Developer Portal app + Message Content Intent enabled; cannot be CI-automated without bot account secrets | Owner runs `cargo run --bin ironhermes-gateway -- --discord-token $DISCORD_BOT_TOKEN`; mentions bot in test channel; confirms reply within 5s |
| Slack bot receives a real message via Socket Mode and emits an agent reply | LEARN-01..05 (Slack parity) | Requires Slack app with `xapp-` (Socket Mode) + `xoxb-` (bot) tokens; cannot run in CI without secrets | Owner runs gateway with `SLACK_APP_TOKEN` + `SLACK_BOT_TOKEN`; DMs bot in test workspace; confirms reply within 5s |

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify (every task has a command)
- [x] Wave 0 covers all MISSING references (4 new/extended test files mapped in Wave 0 Requirements)
- [x] No watch-mode flags
- [x] Feedback latency < 120s
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** ready
