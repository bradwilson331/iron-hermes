---
phase: 12
slug: provider-resolution
status: draft
nyquist_compliant: true
wave_0_complete: true
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
| 12-02-01 | 02 | 2 | PROV-05 | T-12-06 | Anthropic adapter translates messages; credential discovery works | unit | `cargo test anthropic` | ❌ W0 | ⬜ pending |
| 12-02-02 | 02 | 2 | PROV-02 | T-12-07 | AnyClient enum dispatches to correct client by ApiMode | unit | `cargo test any_client` | ❌ W0 | ⬜ pending |
| 12-03-01 | 03 | 2 | PROV-09 | T-12-08 | Iteration budget enforced at 70%/90%/100% thresholds | unit | `cargo test budget` | ❌ W0 | ⬜ pending |
| 12-03-02 | 03 | 2 | PROV-07 | T-12-10 | Fallback chain triggers on 429/5xx/401 one-shot swap | unit | `cargo test fallback` | ❌ W0 | ⬜ pending |
| 12-04-01 | 04 | 3 | PROV-01 | T-12-11 | All call sites use shared resolver via build_main_client | integration | `cargo build --workspace` | ❌ W0 | ⬜ pending |
| 12-04-02 | 04 | 3 | PROV-03 | T-12-12 | Old resolve_base_url/resolve_api_key removed; zero callers remain | integration | `cargo test --workspace` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-core/src/provider.rs` — unit tests for PROV-01, PROV-02, PROV-03, PROV-04, PROV-06, PROV-08 (Plan 01 Task 1 creates tests inline)
- [ ] `crates/ironhermes-agent/src/anthropic_client.rs` — unit tests for PROV-05 adapter + credential discovery (Plan 02 Task 1 creates tests inline)
- [ ] `crates/ironhermes-agent/src/agent_loop.rs` — unit tests for PROV-07, PROV-09, PROV-10 budget + fallback (Plan 03 creates tests inline)

*Existing cargo test infrastructure covers framework needs.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Anthropic credential discovery with live API | PROV-05 | Requires real credentials.json or ANTHROPIC_API_KEY | 1. Set ANTHROPIC_API_KEY env var 2. Run CLI with `provider: anthropic` 3. Verify API call succeeds |

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify
- [x] Wave 0 covers all MISSING references
- [x] No watch-mode flags
- [x] Feedback latency < 30s
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** approved
