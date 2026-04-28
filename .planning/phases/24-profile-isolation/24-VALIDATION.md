---
phase: 24
slug: profile-isolation
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-28
---

# Phase 24 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in `#[test]` + `cargo test` workspace runner |
| **Config file** | none — workspace `Cargo.toml` only |
| **Quick run command** | `cargo test -p ironhermes-core -- profile && cargo test -p ironhermes-gateway -- pid` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~45 seconds (workspace) / ~3 seconds (quick) |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-core -- profile && cargo test -p ironhermes-gateway -- pid`
- **After every plan wave:** Run `cargo test -p ironhermes-cli --test profile_isolation && cargo test -p ironhermes-cli --test gateway_pid`
- **Before `/gsd-verify-work`:** Full suite must be green (`cargo test --workspace`)
- **Max feedback latency:** 5 seconds for quick run; 60 seconds for full suite

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 24-01-W0 | 01 | 0 | CFG-04 | T-24-01 | Reject path traversal `../`, uppercase, special chars in profile names | unit | `cargo test -p ironhermes-core profile::tests::validate_profile_name_rejects` | ❌ W0 | ⬜ pending |
| 24-01-01 | 01 | 1 | CFG-04 | T-24-01 | Validate profile name slug; reject `default`/`current`/`none`/`_*` | unit | `cargo test -p ironhermes-core profile::tests` | ❌ W0 | ⬜ pending |
| 24-02-W0 | 02 | 0 | CFG-04 | — | Atomic PID file write (no torn reads) | unit | `cargo test -p ironhermes-gateway pid::tests::pid_write_is_atomic` | ❌ W0 | ⬜ pending |
| 24-02-01 | 02 | 1 | CFG-04 | T-24-02 | Stale PID auto-deleted on ESRCH; concurrent PID refused with exit 2 | unit | `cargo test -p ironhermes-gateway pid::tests` | ❌ W0 | ⬜ pending |
| 24-03-01 | 03 | 1 | CFG-04 | — | `--profile` flag wins over pre-set env var; `IRONHERMES_HOME` set before scaffold | integration | `cargo test -p ironhermes-cli --test profile_isolation profile_env_var_set_before_scaffold` | ❌ W0 | ⬜ pending |
| 24-03-02 | 03 | 1 | CFG-04 | — | D-08 banner on stderr only (stdout untouched) | integration | `cargo test -p ironhermes-cli --test profile_isolation profile_banner_printed_to_stderr` | ❌ W0 | ⬜ pending |
| 24-04-01 | 04 | 2 | CFG-04 | — | Two profiles' memory/state.db do not bleed (D-19 test 1) | integration smoke | `cargo test -p ironhermes-cli --test profile_isolation profile_isolation_smoke` | ❌ W0 | ⬜ pending |
| 24-04-02 | 04 | 2 | CFG-04 | T-24-02 | Second `gateway run` refuses with D-12 error (D-19 test 2) | integration | `cargo test -p ironhermes-cli --test gateway_pid gateway_pid_concurrent_refuse` | ❌ W0 | ⬜ pending |
| 24-05-01 | 05 | 2 | CFG-04 | — | `hermes status` Profile section enumerates `profiles/*/` with config.yaml | integration | `cargo test -p ironhermes-cli --test status_cmd_integration profile_section` | ❌ W0 | ⬜ pending |
| 24-05-02 | 05 | 2 | CFG-04 | — | `hermes config show` prepends `Profile: <name>` line | integration | `cargo test -p ironhermes-cli --test config_show_integration profile_line` | ❌ W0 | ⬜ pending |
| 24-05-03 | 05 | 2 | CFG-04 | — | `hermes doctor` includes gateway.pid liveness check on active profile | integration | `cargo test -p ironhermes-cli --test doctor_integration profile_doctor` | ❌ W0 | ⬜ pending |
| 24-06-01 | 06 | 2 | CFG-04 | — | First `--profile NEW chat` auto-scaffolds + auto-launches Phase 23 wizard | integration | `cargo test -p ironhermes-cli --test profile_isolation first_use_scaffolds_and_runs_wizard` | ❌ W0 | ⬜ pending |
| 24-07-01 | 07 | 2 | CFG-04 | — | Subagent transcripts under `--profile X` land in `profiles/X/subagent-transcripts/` | integration | `cargo test -p ironhermes-cli --test profile_isolation subagent_transcript_isolation` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

*Plan/Task IDs above are PROVISIONAL — gsd-planner may renumber when producing PLAN.md files. The Task → Test mapping (right side) is the contract; planner must keep test commands intact and reassign Task IDs to match its final wave/plan layout.*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-core/src/profile.rs` — new module with `validate_profile_name(&str) -> Result<ProfileName, ProfileError>` + unit tests for slug validation, reserved tokens, and edge cases
- [ ] `crates/ironhermes-gateway/src/pid.rs` — new module with `acquire_pid_lock`, `read_gateway_pid`, `is_pid_alive`, atomic write helpers + unit tests
- [ ] `crates/ironhermes-cli/tests/profile_isolation.rs` — integration test file containing `profile_isolation_smoke` (D-19 test 1) and helpers
- [ ] `crates/ironhermes-cli/tests/gateway_pid.rs` — integration test file containing `gateway_pid_concurrent_refuse` (D-19 test 2)
- [ ] `crates/ironhermes-gateway/Cargo.toml` — promote `tempfile = "3"` from `[dev-dependencies]` to `[dependencies]` (D-10 atomic write needs it in production code)

*All Wave 0 items are NEW files / additive Cargo.toml edits — no destructive changes.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Stderr banner format readable in real terminal (not just captured stream) | CFG-04 D-08 | Visual UX check; automated test asserts presence and content but cannot assess "readability" | Run `hermes --profile work chat` in a real terminal; verify `[profile: work] HERMES_HOME=~/.ironhermes/profiles/work/` appears once before any other output and does NOT bleed into `hermes -e "prompt" \| jq` style pipes |
| Cross-profile workflow does not surprise operator (run two real `hermes` instances side-by-side) | CFG-04 success criteria #2 | Behavioral validation across two terminal sessions | Open terminal A: `hermes --profile work chat` → write a memory; Open terminal B: `hermes --profile personal chat` → assert work memory is not present |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references (5 new files + 1 Cargo.toml edit)
- [ ] No watch-mode flags (no `cargo watch`)
- [ ] Feedback latency < 5s for quick run, < 60s for full suite
- [ ] `nyquist_compliant: true` set in frontmatter after planner produces PLAN.md files

**Approval:** pending
