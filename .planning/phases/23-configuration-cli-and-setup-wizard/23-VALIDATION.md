---
phase: 23
slug: configuration-cli-and-setup-wizard
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-27
---

# Phase 23 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.
> Derived from `23-RESEARCH.md` §"Validation Architecture". Refined as plans land.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in `#[test]` + `insta` (workspace-pinned `"1"`) for snapshot tests where helpful |
| **Config file** | None — uses `cargo test` |
| **Quick run command** | `cargo test -p ironhermes-core --lib config -- --test-threads=4` |
| **Full suite command** | `cargo test -p ironhermes-core -p ironhermes-cli` |
| **Estimated runtime** | Quick: ~5s. Full suite: ~30–45s. |

---

## Sampling Rate

- **After every task commit:** Run quick command (`cargo test -p ironhermes-core --lib config -- --test-threads=4`)
- **After every plan wave:** Run full suite (`cargo test -p ironhermes-core -p ironhermes-cli`)
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 5 seconds (quick) / 45 seconds (full)

---

## Per-Task Verification Map

> Filled in by the planner per Phase 23 plan (`23-NN-PLAN.md` files). Below are the **requirement-level** anchors derived from RESEARCH.md §"Phase Requirements → Test Map" — each plan task should reference one or more.

| Req ID | Behavior | Test Type | Automated Command | File Exists | Status |
|--------|----------|-----------|-------------------|-------------|--------|
| CFG-01 | `hermes setup` writes valid config.yaml on first run (temp HOME) | integration | `cargo test -p ironhermes-cli --test setup_wizard` | ❌ W0 | ⬜ pending |
| CFG-01 | Wizard question-flow: input string → config mutation (pure fn) | unit | `cargo test -p ironhermes-core --lib wizard_flow` | ❌ W0 | ⬜ pending |
| CFG-01 | `readline_with_initial` default accepted on empty input (pure fn) | unit | `cargo test -p ironhermes-core --lib wizard_empty_input_uses_default` | ❌ W0 | ⬜ pending |
| CFG-01 | Learning Loop opt-in (D-14) writes the full `memory.*` + `learning.*` key block | unit | `cargo test -p ironhermes-core --lib wizard_learning_loop_default_on` | ❌ W0 | ⬜ pending |
| CFG-01 | First-run auto-launch fix-mode (D-05) preserves valid sections | unit | `cargo test -p ironhermes-core --lib wizard_fix_mode_preserves_valid_sections` | ❌ W0 | ⬜ pending |
| CFG-02 | `config set` dotted-path round-trip (load → mutate → save → reload) | unit | `cargo test -p ironhermes-core --lib config_set_roundtrip` | ❌ W0 | ⬜ pending |
| CFG-02 | `config show` masks fields tagged `secret: true` with prefix preservation (`sk-abc***`) | unit | `cargo test -p ironhermes-core --lib config_show_redaction` | ❌ W0 | ⬜ pending |
| CFG-02 | `config set` for `cache_breaking: true` field emits stderr warning | unit | `cargo test -p ironhermes-core --lib cache_break_warning` | ❌ W0 | ⬜ pending |
| CFG-02 | `config get` returns raw scalar for nested keys | unit | `cargo test -p ironhermes-core --lib config_get_dotted_path` | ❌ W0 | ⬜ pending |
| CFG-02 | `hermes config show` Learning Loop status banner (D-17) reflects `memory.enabled` + `learning.skill_generation_enabled` | unit | `cargo test -p ironhermes-core --lib config_show_learning_loop_banner` | ❌ W0 | ⬜ pending |
| CFG-03 | `Config::validate()` returns structured errors for missing required fields | property | `cargo test -p ironhermes-core --lib config_validate` | ❌ W0 | ⬜ pending |
| CFG-03 | `Config::validate()` returns empty vec for all-defaults config (post-wizard) | unit | `cargo test -p ironhermes-core --lib config_validate_defaults_ok` | ❌ W0 | ⬜ pending |
| CFG-03 | `hermes config migrate` discovers skill `requires_config` / `requires_env` gaps | integration | `cargo test -p ironhermes-cli --test config_migrate_discovery` | ❌ W0 | ⬜ pending |
| CFG-03 | YAML round-trip property: serialize → deserialize → validate is identity for the all-defaults config | property | `cargo test -p ironhermes-core --lib config_roundtrip_validate` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

Wave 0 (the first plan in the phase) MUST land these test infrastructure stubs so that subsequent task commits can sample the test suite for feedback:

- [ ] `crates/ironhermes-core/src/config_validate.rs` — `Config::validate()` + `ConfigValidationError` types stub returning empty vec (all tests fail until implementation lands; expected red)
- [ ] `crates/ironhermes-core/src/wizard.rs` — pure-function wizard helpers (`apply_model_answer`, `apply_memory_answer`, etc.) signatures stubbed; full impl follows in plan tasks
- [ ] `crates/ironhermes-core/tests/wizard_flow.rs` — wizard pure-function unit test scaffolding
- [ ] `crates/ironhermes-cli/tests/setup_wizard.rs` — integration test scaffolding using `tempfile` + temp HOME for `hermes setup` end-to-end
- [ ] `crates/ironhermes-cli/tests/config_migrate_discovery.rs` — integration scaffolding for `hermes config migrate`
- [ ] Add `tempfile` to `[dev-dependencies]` of `ironhermes-cli` and `ironhermes-core` if not already present (verify before adding — Phase 21.6 may have added it)

*Existing infrastructure provides:* serde + serde_yaml round-trip is already proven by `Config::load()` round-trip tests in `config.rs::tests`. The `ConfigField` schema in `config_schema.rs` provides the introspection backbone for `secret`/`cache_breaking` flags — Wave 0 extends, does not replace.

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| First-run wizard UX feels natural (D-16 framing paragraph reads well, no awkward pauses, defaults visible) | CFG-01 | Subjective UX assessment cannot be automated; rustyline I/O in tests is brittle | After Phase 23 lands: in a fresh shell with no `~/.ironhermes/config.yaml`, run `hermes` and walk the wizard. Verify: (1) the Learning Loop framing paragraph appears with the verbatim wording from D-16, (2) Enter accepts the default at every prompt, (3) the wizard exits transparently into chat. |
| `hermes config show` Learning Loop banner is glanceable | CFG-02 / D-17 | Visual format check (color, position, length) | After enabling Learning Loop via wizard: run `hermes config show` and confirm the first line is `🧠 Learning Loop: enabled (memory + skill generation)`. Disable via `hermes config set memory.enabled false`; rerun and confirm the warning banner replaces it. |

*All other phase behaviors have automated verification.*

### What NOT to test (anti-patterns)

- ❌ **Direct rustyline terminal-interaction tests** — pty-driven tests of "type X, expect Y" are flaky and slow. Decompose wizard into pure functions (per RESEARCH.md §Q2) and test the pure functions; the rustyline I/O layer is verified by manual UAT only.
- ❌ **Live `cargo run --bin hermes setup`** in CI — auto-launch behavior is verified via temp-HOME integration tests, not by spawning the real binary.
- ❌ **YAML byte-equality round-trip** — serde_yaml does NOT preserve comments (RESEARCH.md §Q1). Round-trip tests assert structural equality of the deserialized `Config`, not byte-equality of the YAML file.
- ❌ **Property tests over arbitrary `Config` mutations** — the config schema is rich enough that `proptest`-style mutations would mostly hit invalid configurations. Stick to targeted property tests on a few well-defined invariants (round-trip identity, defaults validate, missing required fields surface).

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies — populated by planner
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references (the 5 stub files above)
- [ ] No watch-mode flags (`cargo test`, not `cargo watch`)
- [ ] Feedback latency < 5s (quick) / 45s (full)
- [ ] `nyquist_compliant: true` set in frontmatter once Wave 0 commits AND all per-task entries have an automated command (planner step)

**Approval:** pending — finalized after planner produces `23-NN-PLAN.md` files and per-task verification map is fully populated.
