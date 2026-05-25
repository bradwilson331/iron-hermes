---
phase: 36
slug: gateway-running-agent-guard-wiring-completes-gw-05
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-05-24
---

# Phase 36 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.
> Derived from `36-RESEARCH.md` "Validation Architecture" section. Tests cover all 11 sub-behaviors of GW-05 plus the Pitfall-1 free-text path the researcher flagged.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust `#[cfg(test)]` modules + `cargo test` |
| **Config file** | `Cargo.toml` (workspace root) |
| **Quick run command** | `cargo test -p ironhermes-gateway running_agent` |
| **Full suite command** | `cargo test -p ironhermes-gateway` |
| **Estimated runtime** | ~30 seconds (full gateway suite) |

---

## Sampling Rate

- **After every task commit:** `cargo test -p ironhermes-gateway running_agent`
- **After every plan wave:** `cargo test -p ironhermes-gateway`
- **Before `/gsd:verify-work`:** `cargo test --workspace` must be green
- **Max feedback latency:** ~30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 36-XX-XX | TBD | 0 | GW-05 (scaffold) | — | Test file exists; harness compiles | unit | `cargo test -p ironhermes-gateway running_agent_guard_tests --no-run` | ❌ W0 | ⬜ pending |
| 36-XX-XX | TBD | 1 | GW-05-1 | T-21.1-05 | Per-session isolation — session A Running does not block session B | unit | `cargo test -p ironhermes-gateway running_agent_guard::test_session_isolation` | ❌ W0 | ⬜ pending |
| 36-XX-XX | TBD | 1 | GW-05-2 | T-21.1-09 (TOCTOU) | Guard rejects `/model` during active turn with D-02 error message | unit | `cargo test -p ironhermes-gateway running_agent_guard::test_model_rejected_when_running` | ❌ W0 | ⬜ pending |
| 36-XX-XX | TBD | 1 | GW-05-3 | T-21.1-05 | Bypass: `/stop` dispatches even when flag is true | unit | `cargo test -p ironhermes-gateway running_agent_guard::test_stop_bypasses_guard` | ❌ W0 | ⬜ pending |
| 36-XX-XX | TBD | 1 | GW-05-4 | T-21.1-05 | Bypass: `/new` dispatches even when flag is true | unit | `cargo test -p ironhermes-gateway running_agent_guard::test_new_bypasses_guard` | ❌ W0 | ⬜ pending |
| 36-XX-XX | TBD | 1 | GW-05-5 | T-21.1-05 | Bypass: `/status` dispatches even when flag is true | unit | `cargo test -p ironhermes-gateway running_agent_guard::test_status_bypasses_guard` | ❌ W0 | ⬜ pending |
| 36-XX-XX | TBD | 1 | GW-05-6 | T-21.1-05 | Bypass: `/queue` dispatches even when flag is true | unit | `cargo test -p ironhermes-gateway running_agent_guard::test_queue_bypasses_guard` | ❌ W0 | ⬜ pending |
| 36-XX-XX | TBD | 1 | GW-05-7 | T-21.1-05 | Flag clears on `run_agent` returning `Ok(...)` (RAII guard discipline) | unit | `cargo test -p ironhermes-gateway running_agent_guard::test_guard_clears_on_success` | ❌ W0 | ⬜ pending |
| 36-XX-XX | TBD | 1 | GW-05-8 | T-21.1-05 | Flag clears on `run_agent` returning `Err(...)` (RAII guard discipline) | unit | `cargo test -p ironhermes-gateway running_agent_guard::test_guard_clears_on_error` | ❌ W0 | ⬜ pending |
| 36-XX-XX | TBD | 1 | GW-05-9 | T-21.1-05 | Alias `/reset` (resolves to `new`) bypasses guard (check post-resolution name) | unit | `cargo test -p ironhermes-gateway running_agent_guard::test_alias_bypasses_guard` | ❌ W0 | ⬜ pending |
| 36-XX-XX | TBD | 1 | GW-05-10 | T-21.1-08 | Non-slash free-text during active turn is rejected (Pitfall 1 — guard must cover non-slash path too) | unit | `cargo test -p ironhermes-gateway running_agent_guard::test_freetext_rejected_when_running` | ❌ W0 | ⬜ pending |
| 36-XX-XX | TBD | 1 | GW-05-11 | T-21.1-05 | `cmd_stop` on gateway reads a non-false `agent_running` (closes the "/stop always says no agent" bug) | integration | `cargo test -p ironhermes-gateway running_agent_guard::test_stop_reads_real_flag` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

*Task IDs left as `36-XX-XX` for the planner to fill once plan numbering is decided.*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-gateway/tests/running_agent_guard_tests.rs` — create test file with all 11 `#[ignore]` stubs (each named per the table above)
- [ ] Mock or stub for `SessionStore` that returns controllable `Arc<AtomicBool>` values — likely a `fn make_test_store() -> Arc<RwLock<SessionStore>>` helper inside the test module
- [ ] Mock `PlatformAdapter` that records outgoing `send_message` calls so tests can assert the D-02 error string was sent

*Existing infrastructure (the `ironhermes-gateway` crate's test harness) covers everything else.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Real-Telegram UAT: send `/model` during a long agent turn → bot replies with D-02 error; agent turn continues uninterrupted | GW-05-2 | Telegram round-trip with real LLM call cannot be deterministically unit-tested | (1) Configure a TG bot + chat. (2) Send a prompt that triggers a long agent turn (e.g., a deep web-search workflow). (3) While the turn is mid-flight, send `/model anthropic:claude-opus-4-7`. (4) Verify bot replies with the D-02 rejection message. (5) Verify the original turn completes normally. |
| Real-Telegram UAT: `/stop` during an active subagent run reports "Stopped N background process(es)." (existing behavior, regression check) | GW-05-3 | Subagent process_registry interaction with real OS processes | (1) Send a prompt that triggers subagent delegation. (2) Send `/stop` while subagents are running. (3) Verify response shows non-zero stop count. (4) Compare to behavior on `develop` branch pre-fix — should be unchanged. |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies (planner fills task IDs)
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references (the test file scaffold + mocks)
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter (after Wave 0 tests committed)

**Approval:** pending
