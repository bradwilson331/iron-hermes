# Phase 32: Periodic Nudge & Memory Curation — Validation

**Generated from:** 32-RESEARCH.md §Validation Architecture
**Phase requirements:** LEARN-01, LEARN-02

## Test Framework

| Property | Value |
|----------|-------|
| Framework | cargo test (built-in) |
| Config file | none — inline test modules |
| Quick run (nudge only) | `cargo test -p ironhermes-agent nudge` |
| Quick run (config only) | `cargo test -p ironhermes-core config_nudge_interval` |
| Full suite | `cargo test --workspace` |

## Requirements → Test Map

| Req ID | Behavior | Test Name | Crate / File | Plan Wave |
|--------|----------|-----------|--------------|-----------|
| LEARN-01 | nudge_interval field deserializes with default 10 | `config_nudge_interval_default` | ironhermes-core / config.rs | Wave 1 (32-01) |
| LEARN-01 | nudge_interval=0 disables nudge at config layer | `config_nudge_interval_zero_disabled` | ironhermes-core / config.rs | Wave 1 (32-01) |
| LEARN-01 | nudge_interval deserializes from YAML | `config_nudge_interval_deserialize` | ironhermes-core / config.rs | Wave 1 (32-01) |
| LEARN-01 | missing nudge_interval key uses default 10 | `config_nudge_interval_missing_uses_default` | ironhermes-core / config.rs | Wave 1 (32-01) |
| LEARN-01 | Nudge fires at configured interval | `fires_at_interval` | ironhermes-agent / nudge.rs | Wave 2 (32-02) |
| LEARN-01 | Nudge disabled when interval=0 | `disabled_when_zero` | ironhermes-agent / nudge.rs | Wave 2 (32-02) |
| LEARN-01 | Counter resets after nudge fires | `counter_resets_after_fire` | ironhermes-agent / nudge.rs | Wave 2 (32-02) |
| LEARN-02 | Prompt contains two-tier judgment text | `prompt_contains_tier_guidance` | ironhermes-agent / nudge.rs | Wave 1 (32-01) |
| LEARN-02 | Prompt contains memory cap (3,575 chars) | `prompt_contains_cap_info` | ironhermes-agent / nudge.rs | Wave 1 (32-01) |
| LEARN-02 | Prompt contains "Nothing to save" signal | `prompt_contains_nothing_to_save_signal` | ironhermes-agent / nudge.rs | Wave 1 (32-01) |
| LEARN-02 | Memory cap honored (existing tests) | `memory_manager` existing suite | ironhermes-agent / memory/ | Pre-existing |
| LEARN-01 | Nudge does not fire mid-stream | (manual UAT) | N/A | N/A |

## Static-Grep Acceptance Criteria

These are verified by plan acceptance criteria (not `cargo test`):

| Check | Command | Expected |
|-------|---------|----------|
| nudge_interval field exists | `grep "pub nudge_interval: u32" crates/ironhermes-core/src/config.rs` | 1 match |
| default_nudge_interval fn | `grep "fn default_nudge_interval" crates/ironhermes-core/src/config.rs` | 1 match |
| nudge module declared | `grep "pub mod nudge" crates/ironhermes-agent/src/lib.rs` | 1 match |
| session_search in prompt only | `grep "session_search" crates/ironhermes-agent/src/nudge.rs` | match in MEMORY_REVIEW_PROMPT string only; no `register` call with session_search |
| turns_since_nudge in run_chat | `grep -c "turns_since_nudge" crates/ironhermes-cli/src/main.rs` | >= 3 |
| spawn_nudge_review in run_chat | `grep "ironhermes_agent::nudge::spawn_nudge_review" crates/ironhermes-cli/src/main.rs` | 1 match |
| nudge_turns in gateway handler | `grep -c "nudge_turns" crates/ironhermes-gateway/src/handler.rs` | >= 4 |
| spawn_nudge_review in gateway | `grep "ironhermes_agent::nudge::spawn_nudge_review" crates/ironhermes-gateway/src/handler.rs` | 1 match |
| wizard writes memory.nudge_interval | `grep "memory.nudge_interval\|memory_nudge_interval" crates/ironhermes-core/src/wizard.rs` | 1 match |
| wizard preserves learning key | `grep "periodic_nudge_interval_seconds" crates/ironhermes-core/src/wizard.rs` | 1 match |

## Sampling Rate

- **Per task commit:** run the crate-scoped test command listed in `<verify>` for that task
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** full suite green before `/gsd:verify-work`

## UAT Checklist (Manual)

- [ ] `hermes chat` — respond 10 times; on turn 10 no visible lag; `tracing::info!` log appears
- [ ] `hermes chat` with `memory.nudge_interval: 0` in config — nudge never fires across 20 turns
- [ ] Telegram gateway — 10 messages; nudge fires silently; session continues normally
- [ ] `hermes setup` → learning loop wizard → enter `5` → config file contains both `learning.periodic_nudge_interval_seconds: 5` AND `memory.nudge_interval: 5`
