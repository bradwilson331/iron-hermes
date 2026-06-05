---
phase: 37
slug: rustsec-2026-0104-reachable-panic
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-06-05
---

# Phase 37 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.
> Source: `37-RESEARCH.md` → Validation Architecture. This phase is a Cargo
> dependency-patch + workspace version bump — verification is entirely via
> `cargo` commands and greps; no new test files are required.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo-nextest (migrated in phase 36.17.7) |
| **Config file** | `.config/nextest.toml` |
| **Quick run command** | `cargo tree -i rustls-webpki` (resolved-graph check, no build) |
| **Full suite command** | `cargo build --workspace && cargo nextest run --workspace && cargo test --doc` |
| **Estimated runtime** | ~quick: <5s · full: several minutes (cold build) |

---

## Sampling Rate

- **After every task commit:** Run `cargo tree -i rustls-webpki` (graph check) and, for build-affecting tasks, `cargo build --workspace`
- **After every plan wave:** Run `cargo build --workspace && cargo nextest run --workspace`
- **Before `/gsd:verify-work`:** Full suite must be green AND `cargo tree -i rustls-webpki` must show `0.103.13` (not `0.103.10`)
- **Max feedback latency:** ~5s for graph check; full build several minutes

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 37-01-xx | 01 | 1 | SEC-01 | RUSTSEC-2026-0104 | `rustls-webpki 0.103.10` no longer in resolved graph | grep | `cargo tree -i rustls-webpki \| grep -q 0.103.10 && exit 1 \|\| exit 0` | ✅ | ⬜ pending |
| 37-01-xx | 01 | 1 | SEC-02 | RUSTSEC-2026-0104 | `rustls-webpki 0.103.13` present (patched) | grep | `cargo tree -i rustls-webpki \| grep -q 0.103.13` | ✅ | ⬜ pending |
| 37-01-xx | 01 | 1 | SEC-03 | RUSTSEC-2026-0104 | Workspace builds after patch | build | `cargo build --workspace` | ✅ | ⬜ pending |
| 37-01-xx | 01 | 1 | SEC-04 | RUSTSEC-2026-0104 | Full test suite passes (no TLS regressions) | test | `cargo nextest run --workspace` | ✅ | ⬜ pending |
| 37-01-xx | 01 | 1 | SEC-05 | RUSTSEC-2026-0104 | Chain 1 (`0.102.8`) exemption documented (no upstream patch) | grep | `grep -q "RUSTSEC-2026-0104" Cargo.toml` (or `audit.toml`) | ✅ | ⬜ pending |
| 37-02-xx | 02 | 2 | VER-01 | — | Workspace version = `0.2.0` in root manifest | grep | `grep '^version = "0.2.0"' Cargo.toml` | ✅ | ⬜ pending |
| 37-02-xx | 02 | 2 | VER-02 | — | `iron_hermes_ui` version = `0.2.0` | grep | `grep '^version = "0.2.0"' crates/iron_hermes_ui/Cargo.toml` | ✅ | ⬜ pending |
| 37-02-xx | 02 | 2 | VER-03 | — | `ironhermes-exec` version = `0.2.0` | grep | `grep '^version = "0.2.0"' crates/ironhermes-exec/Cargo.toml` | ✅ | ⬜ pending |
| 37-02-xx | 02 | 2 | VER-04 | — | CLI `--version` outputs `0.2.0` | run | `cargo run -p ironhermes-cli -- --version \| grep -q 0.2.0` | ✅ | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*
*Task IDs are placeholders (`xx`) — finalized by the planner against PLAN.md task numbering.*

---

## Wave 0 Requirements

- [ ] (Optional) `cargo install cargo-audit` — enables authoritative RUSTSEC scanning beyond `cargo tree`. Not blocking: `cargo tree -i rustls-webpki` is the primary, install-free verification gate.
- [ ] (Optional) Create `audit.toml` / root-`Cargo.toml` comment documenting the `RUSTSEC-2026-0104` exemption for `rustls-webpki 0.102.8` (serenity 0.12.5 chain — no upstream fix).

*All SEC/VER verification uses built-in `cargo` commands — no test framework install or stub files required.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| `cargo audit` reports `0.102.8` as acknowledged/exempted (not an unhandled failure) | SEC-05 | Requires `cargo-audit` install (optional Wave 0); exemption is a documented risk-acceptance, not a code fix | After `cargo install cargo-audit`, run `cargo audit`; confirm `RUSTSEC-2026-0104` for `0.103.x` is gone and `0.102.8` is the only remaining hit, matching the documented exemption rationale |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references (none required — cargo built-ins)
- [ ] No watch-mode flags
- [ ] Feedback latency < 5s (graph check)
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
