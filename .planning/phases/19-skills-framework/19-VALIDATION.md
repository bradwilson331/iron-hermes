---
phase: 19
slug: skills-framework
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-14
---

# Phase 19 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in `#[test]` + `#[tokio::test]` |
| **Config file** | none — workspace-level `cargo test` |
| **Quick run command** | `cargo test -p ironhermes-core skills 2>&1 \| tail -20` |
| **Full suite command** | `cargo test --workspace 2>&1 \| tail -20` |
| **Estimated runtime** | ~60 seconds (quick) / ~180 seconds (full) |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-core skills 2>&1 | tail -20`
- **After every plan wave:** Run `cargo test --workspace 2>&1 | tail -20`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 180 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 19-XX-WARN0a | XX | 0 | SKILL-01 | — | N/A | unit | `cargo test -p ironhermes-core test_hermes_metadata` | ❌ W0 | ⬜ pending |
| 19-XX-WARN0b | XX | 0 | SKILL-01 (D-18) | — | unknown fields preserved, skill loads | unit | `cargo test -p ironhermes-core test_warn_but_load_unknown_fields` | ❌ W0 | ⬜ pending |
| 19-XX-WARN0c | XX | 0 | SKILL-01 (D-18) | — | 07.2 compat | unit | `cargo test -p ironhermes-core test_07_2_compat_metadata` | ❌ W0 | ⬜ pending |
| 19-XX-FILT01 | XX | 1 | SKILL-03 | — | requires_toolsets filter hides skill when toolset absent | unit | `cargo test -p ironhermes-core test_filter_requires_toolsets` | ❌ W0 | ⬜ pending |
| 19-XX-FILT02 | XX | 1 | SKILL-03 | — | fallback_for_tools hides skill when primary present | unit | `cargo test -p ironhermes-core test_filter_fallback_for_tools` | ❌ W0 | ⬜ pending |
| 19-XX-ENV01 | XX | 1 | SKILL-04 | — | activate returns setup-error envelope when env var missing | unit | `cargo test -p ironhermes-tools test_activate_missing_env_var` | ❌ W0 | ⬜ pending |
| 19-XX-CFG01 | XX | 1 | SKILL-05 | — | config header injected into skill body on activate | unit | `cargo test -p ironhermes-tools test_activate_config_injection` | ❌ W0 | ⬜ pending |
| 19-XX-CRED01 | XX | 1 | SKILL-06 | — | activate returns setup-error envelope when credential file missing | unit | `cargo test -p ironhermes-tools test_activate_missing_credential` | ❌ W0 | ⬜ pending |
| 19-XX-SCAN01 | XX | 1 | SKILL-07 | T-19-scan-comm | community skill hard-rejected on scan hit at registry load | unit | `cargo test -p ironhermes-core test_community_skill_scan_reject` | ❌ W0 | ⬜ pending |
| 19-XX-SCAN02 | XX | 1 | SKILL-07 | T-19-scan-builtin | builtin skill WARN-BUT-LOAD on scan hit | unit | `cargo test -p ironhermes-core test_builtin_skill_scan_warn_load` | ❌ W0 | ⬜ pending |
| 19-XX-PLAT01 | XX | 1 | SKILL-10 | — | platform filter regression: macos skill hidden on linux | unit | `cargo test -p ironhermes-core test_platform_filter` | ✅ existing | ⬜ pending |
| 19-XX-PASS01 | XX | 2 | SKILL-11 | T-19-env-leak | skill-declared env var reaches sandboxed child | integration | `cargo test -p ironhermes-exec test_skill_env_passthrough` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

*Task IDs are placeholders (`19-XX-*`) — planner resolves `XX` to concrete plan numbers.*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-core/src/skills.rs` — unit tests for HermesMetadata parse, D-18 extras preservation, D-15 scan enforcement (SKILL-01, SKILL-07)
- [ ] `crates/ironhermes-tools/src/skills_tool.rs` — unit tests for setup-error envelope + config body injection (SKILL-04, SKILL-05, SKILL-06)
- [ ] `crates/ironhermes-core/src/context_scanner.rs` — unit tests for new skill-specific patterns (SKILL-07)
- [ ] `crates/ironhermes-exec/src/sandbox.rs` — integration test for skill env whitelist passthrough (SKILL-11)

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Modal remote sandbox credential sync (D-11) | SKILL-06 | No Modal CI credentials in dev envs | Defer to Phase 19.1 / follow-up; unit-test Docker bind-mount path only |
| Agent user-facing setup prompt phrasing (D-04) | SKILL-04 | Natural-language UX review | Human reads envelope `setup_note` text on a missing-env skill |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 180s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
