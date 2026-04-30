---
phase: 26
slug: provider-polish
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-29
---

# Phase 26 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in test harness + `tokio::test` for async |
| **Mock HTTP** | `wiremock` 0.6 (already in `ironhermes-cli/Cargo.toml`) |
| **HTTP client for live ping** | `reqwest` 0.12 (already in workspace) |
| **Quick run command** | `cargo test -p ironhermes-core provider` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~5s lib-only, ~30–60s full suite |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-core provider`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 60 seconds

---

## Per-Task Verification Map

> Filled by gsd-planner once tasks are emitted. Each task references its plan/wave, the
> requirement(s) it covers, threat ref (if any), expected secure behavior, test type, exact
> automated command, and whether the test file exists or is a Wave 0 requirement.

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 26-01-* | 01 | 1 | PROV-04, PROV-08 | T-26-01 | Config schema additions; api_key_env validation | unit | `cargo test -p ironhermes-core api_key_env_validation` | ❌ W0 | ⬜ pending |
| 26-02-* | 02 | 2 | PROV-04 | T-26-01 | PROV-04 leak removed; deprecation banner once-only; resolve_role cascade | unit | `cargo test -p ironhermes-core legacy_openai_key_does_not_leak,resolve_role_cascade,legacy_env_banner_once_only` | ❌ W0 | ⬜ pending |
| 26-03-* | 03 | 3 | PROV-06 | — | engine_factory + summarizing_engine wire-through resolve_role | unit | `cargo test -p ironhermes-agent role_routing` | ❌ W0 | ⬜ pending |
| 26-04-* | 04 | 4 | PROV-04, PROV-06, PROV-08 | T-26-01 | hermes provider list/show/test/enable/disable + /provider slash + display helpers + D-20 integration tests | integration | `cargo test -p ironhermes-cli --test provider_integration` | ❌ W0 | ⬜ pending |
| 26-05-* | 05 | 5 | PROV-04, PROV-06 | — | hermes setup auxiliary stage (D-19); subprocess test for hermes provider test no-key-leak | integration | `cargo test -p ironhermes-cli provider_test_does_not_print_key` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Critical Test Surfaces

### Mandatory (D-20 in CONTEXT.md)

**Test 1 — `key_does_not_leak_to_wrong_provider`** *(integration, Plan 04, PROV-04)*
- Spawn binary with `OPENAI_API_KEY=sk-real` set in env (wrap in env_lock)
- Define `providers.my-local-llm` at `http://127.0.0.1:<wiremock-port>/v1` with NO `api_key_env`
- wiremock asserts no Authorization header containing `sk-real` reaches `my-local-llm`
- Location: `crates/ironhermes-cli/tests/provider_integration.rs`

**Test 2 — `auxiliary_routes_to_separate_model`** *(integration, Plan 04, PROV-06)*
- Set `auxiliary: { provider: openai, model: gpt-4o-mini }` with `model.provider: anthropic`
- wiremock listens for both endpoints; trigger a compression task
- Assert outbound request hits `api.openai.com` (mocked) with `model: gpt-4o-mini`, NOT `api.anthropic.com`
- Location: `crates/ironhermes-cli/tests/provider_integration.rs`

**Test 3 — `custom_provider_selectable_by_name`** *(integration, Plan 04, PROV-08)*
- Define `providers.my-local-llm` with custom `base_url` + `api_key_env: MY_LLM_KEY`
- Run `hermes --provider my-local-llm chat "ping"`
- wiremock asserts request hits the configured base_url with the resolved key
- Location: `crates/ironhermes-cli/tests/provider_integration.rs`

### D-21 Unit Test (PROV-04 lib-level confirmation)

**`legacy_openai_key_does_not_leak_to_unknown_provider`** *(unit, Plan 02)*
- Set `OPENAI_API_KEY=sk-leaked` (env_lock)
- Build resolver with `providers.my-local-llm` having `api_key_env: None`
- Assert `resolver.resolve("my-local-llm").unwrap().api_key == None`
- Location: `crates/ironhermes-core/src/provider.rs` `#[cfg(test)]`

### Supporting Unit Tests

| Test | Covers | Location |
|------|--------|----------|
| `api_key_env_validation_rejects_invalid` | D-04 regex `[A-Z][A-Z0-9_]*` | `provider.rs` or `config.rs` tests |
| `api_key_env_validation_accepts_valid` | D-04 happy path | same |
| `legacy_env_banner_once_only` | D-12 once-shot OnceLock guard | `provider.rs` tests (subprocess for assertion) |
| `model_api_key_legacy_banner_once_only` | D-13 once-shot for `config.model.api_key` | same |
| `custom_providers_migration_warning` | D-02 migration stderr emission | `config.rs` or `provider.rs` tests |
| `custom_providers_precedence_when_both_set` | D-02 collision behavior (`providers.foo` wins over `custom_providers: [foo]`) | same |
| `auxiliary_provider_unknown_name_fails_build` | Config validation: `auxiliary.provider` must reference a known name | `provider.rs` tests |
| `resolve_role_per_task_override_wins` | D-05 cascade level 1 | `provider.rs` tests |
| `resolve_role_falls_through_to_auxiliary` | D-05 cascade level 2 | `provider.rs` tests |
| `resolve_role_returns_none_when_no_role_set` | D-05 cascade level 3 (caller falls through to main) | `provider.rs` tests |
| `provider_test_does_not_print_key` | D-15 / T-26-01 — subprocess captures stdout/stderr, asserts key value absent | `provider_integration.rs` |
| `cache_break_banner_on_persistent_set_only` | D-16/D-17 — banner on `hermes config set providers.*`, NOT on `--provider` flag | `provider_integration.rs` |
| `provider_enable_disable_persists` | D-14 enable/disable round-trip via subprocess + restart | `provider_integration.rs` |

---

## Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command |
|--------|----------|-----------|-------------------|
| PROV-04 | Key does not leak to wrong base URL (unit) | unit | `cargo test -p ironhermes-core legacy_openai_key_does_not_leak_to_unknown_provider` |
| PROV-04 | Key does not appear in outbound HTTP header (integration) | integration | `cargo test -p ironhermes-cli key_does_not_leak_to_wrong_provider` |
| PROV-06 | resolve_role cascade per-task → aux → None | unit | `cargo test -p ironhermes-core resolve_role` |
| PROV-06 | Aux routes to separate model/provider end-to-end | integration | `cargo test -p ironhermes-cli auxiliary_routes_to_separate_model` |
| PROV-08 | Custom provider selectable by --provider flag | integration | `cargo test -p ironhermes-cli custom_provider_selectable_by_name` |
| D-02 | custom_providers migration emits warning | unit | `cargo test -p ironhermes-core custom_providers_migration_warning` |
| D-04 | api_key_env validation | unit | `cargo test -p ironhermes-core api_key_env_validation` |
| D-12 | Legacy env-var banner once-only | unit/subprocess | `cargo test -p ironhermes-core legacy_env_banner_once_only` |
| D-15 | provider test never prints key value | integration | `cargo test -p ironhermes-cli provider_test_does_not_print_key` |
| D-16/D-17 | Cache-break banner on persistent writes only | integration | `cargo test -p ironhermes-cli cache_break_banner` |

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-cli/tests/provider_integration.rs` — covers D-20 (3 integration tests) + provider_test_does_not_print_key + cache_break_banner + provider_enable_disable_persists
- [ ] `crates/ironhermes-core/src/provider.rs` `#[cfg(test)]` block — D-21 unit + cascade tests + validation + banner once-only
- [ ] `crates/ironhermes-core/src/commands/provider_display.rs` — `render_provider_list()`, `render_provider_show()` helpers (+ unit tests for column alignment / json output)
- [ ] `crates/ironhermes-cli/src/provider_cmd.rs` — CLI subcommand implementation (+ unit tests for slug validation / config_setter routing)
- [ ] No new framework install — `wiremock` and `reqwest` already in workspace

---

## Sampling Strategy for env-var Sensitive Tests

All tests that call `std::env::set_var` / `remove_var` (Rust 2024 edition: requires `unsafe {}` block) MUST hold the process-wide `env_lock()` for the duration. Pattern from `toolset_integration.rs`:

```rust
fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

// In each test:
let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
unsafe { std::env::set_var("OPENAI_API_KEY", "sk-test"); }
// ... test body ...
unsafe { std::env::remove_var("OPENAI_API_KEY"); }
```

**The `env_lock()` in `provider_integration.rs` is SEPARATE from the one in `toolset_integration.rs`** — they are different test binary compilations. Each `tests/*.rs` file gets its own process-wide static (Phase 25 pattern preserved).

`OnceLock<bool>` for the deprecation banner CANNOT be reset between unit tests in the same process — for the once-only assertion, use a subprocess test that exec's the binary twice and counts banner occurrences in captured stderr.

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| `hermes provider list` aligned-columns rendering with Unicode glyphs (✓/✗) | PROV-04 | Visual alignment; Unicode column widths | Run `hermes provider list` with at least 2 providers configured; inspect ✓/✗ glyph alignment + truncation behavior on narrow terminals |
| `hermes provider test <name>` 4xx vs 5xx vs network-error message phrasing | D-14 | Operator-facing copy needs human review | Run against unreachable URL, against URL that returns 401, against URL that returns 500; verify each error message is helpful |
| `hermes setup` D-19 auxiliary stage prompt UX | D-19 | Interactive rustyline flow; Enter-to-skip behavior | Run `hermes setup` after Plan 25's prereq stage; verify "Configure auxiliary model? [y/N]" defaults correctly + skips cleanly |
| Stderr deprecation banner readability | D-12, D-13 | One-line banner copy needs human review | Set `OPENAI_API_KEY` only (no `providers.openai.api_key_env`); run `hermes status`; verify banner appears once + is readable |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references (4 new test files / source files)
- [ ] No watch-mode flags
- [ ] Feedback latency < 60s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
