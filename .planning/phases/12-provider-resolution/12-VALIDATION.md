---
phase: 12
slug: provider-resolution
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-11
---

# Phase 12 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | Cargo.toml workspace |
| **Quick run command** | `cargo test -p ironhermes-core` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-core`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 12-01-01 | 01 | 1 | PROV-01 | T-12-01 / — | Resolver returns correct (api_mode, key, url) tuple | unit | `cargo test resolver` | ❌ W0 | ⬜ pending |
| 12-01-02 | 01 | 1 | PROV-02 | — | All three API modes reachable by config | unit | `cargo test api_mode` | ❌ W0 | ⬜ pending |
| 12-01-03 | 01 | 1 | PROV-03 | T-12-02 | API keys scoped per provider, not leaked cross-provider | unit | `cargo test provider_key` | ❌ W0 | ⬜ pending |
| 12-02-01 | 02 | 1 | PROV-04 | — | Fallback chain triggers on 429/5xx/401 | unit | `cargo test fallback` | ❌ W0 | ⬜ pending |
| 12-02-02 | 02 | 1 | PROV-05 | — | Iteration budget enforced at 70%/90%/100% thresholds | unit | `cargo test budget` | ❌ W0 | ⬜ pending |
| 12-03-01 | 03 | 2 | PROV-06 | — | Custom named providers route correctly | integration | `cargo test custom_provider` | ❌ W0 | ⬜ pending |
| 12-03-02 | 03 | 2 | PROV-07 | T-12-03 | Credential refresh handles OAuth token expiry | integration | `cargo test credential` | ❌ W0 | ⬜ pending |
| 12-04-01 | 04 | 2 | PROV-08 | — | All call sites use shared resolver | integration | `cargo test call_site` | ❌ W0 | ⬜ pending |
| 12-04-02 | 04 | 2 | PROV-09 | — | Anthropic adapter format conversion correct | unit | `cargo test anthropic_adapter` | ❌ W0 | ⬜ pending |
| 12-04-03 | 04 | 2 | PROV-10 | — | Budget shared between parent and child agents | integration | `cargo test shared_budget` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-core/src/resolver/tests.rs` — unit test stubs for PROV-01 through PROV-03
- [ ] `crates/ironhermes-core/src/resolver/fallback_tests.rs` — fallback chain test stubs for PROV-04
- [ ] `crates/ironhermes-core/src/budget/tests.rs` — budget enforcement test stubs for PROV-05

*Existing cargo test infrastructure covers framework needs.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| OAuth credential refresh with live Anthropic API | PROV-07 | Requires real API credentials and token expiry | 1. Set expired token in credentials.json 2. Run CLI command 3. Verify token refreshed and request succeeded |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
