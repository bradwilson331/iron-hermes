---
phase: 25
slug: toolset-management
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-29
---

# Phase 25 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in test harness + `tokio::test` for async |
| **Config file** | Per-crate `Cargo.toml`; no separate test config |
| **Quick run command** | `cargo test --workspace --lib` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~5s lib-only, ~30–60s full suite |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --workspace --lib`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 60 seconds

---

## Per-Task Verification Map

> Filled by gsd-planner once tasks are emitted. Each task row references its plan/wave, the
> requirement(s) it covers, threat ref (if any), expected secure behavior, test type, the exact
> automated command, and whether the test file exists or is a Wave 0 requirement.

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 25-01-* | 01 | 1 | TOOL-01 | — | Trait additions compile workspace-wide | unit | `cargo test -p ironhermes-tools` | ❌ W0 | ⬜ pending |
| 25-02-* | 02 | 2 | TOOL-03, TOOL-04 | — | Registry exposes intercept API; D-15 panic guards | unit | `cargo test -p ironhermes-tools intercept` | ❌ W0 | ⬜ pending |
| 25-03-* | 03 | 3 | TOOL-02, TOOL-04 | — | get_definitions() filters by toolset+prereq; agent_loop migrates intercept call site | unit | `cargo test -p ironhermes-tools toolset` / `cargo test -p ironhermes-agent` | ❌ W0 | ⬜ pending |
| 25-04-* | 04 | 4 | TOOL-02 | — | `hermes toolset list/enable/disable/show` persists per-profile; slash command parity | integration | `cargo test -p ironhermes-cli toolset_enable_disable_persists` | ❌ W0 | ⬜ pending |
| 25-05-* | 05 | 5 | TOOL-05 | — | `hermes toolset setup` walks missing prereqs; preflight banner on required-missing | integration | `cargo test -p ironhermes-cli toolset_setup` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Critical Test Surfaces

### Mandatory (D-26 in CONTEXT.md)

**Test 1 — `toolset_enable_disable_persists`** *(integration, Plan 04)*
- Spawn binary with fresh tempdir as `IRONHERMES_HOME`
- Run `hermes toolset enable web` → assert `[toolset: web] enabled` banner on stderr
- Assert `~/.ironhermes/config.yaml` now contains `tools.toolsets.web.enabled: true`
- Run `hermes toolset list` → assert `web` row shows `enabled`
- Restart binary (new `Command::new(bin)` invocation, same `IRONHERMES_HOME`)
- Run `hermes toolset list` again → assert `web` STILL `enabled`
- Location: `crates/ironhermes-cli/tests/toolset_integration.rs` (new file, mirrors `profile_isolation.rs` subprocess pattern from Phase 24)

**Test 2 — `tool_excluded_when_prereq_missing`** *(integration, Plan 03)*
- Hold env_lock (REQUIRED — env var mutation is shared global state)
- Ensure `FIRECRAWL_API_KEY` is unset
- Build `ToolRegistry` with defaults (web toolset enabled in config)
- Call `registry.get_definitions(None)`
- Assert `web_search` schema NOT present
- Set `FIRECRAWL_API_KEY=test_key`
- Call `registry.get_definitions(None)` again → assert `web_search` schema IS present
- Unset `FIRECRAWL_API_KEY`, release env_lock
- Location: `crates/ironhermes-tools/tests/toolset_prereq.rs` (new file)

**Test 3 — `intercepted_tool_no_schema_duplicate`** *(unit, Plan 02 with re-verify in Plan 03)*
- Build full registry with `register_intercepted()` for all D-13 tools
- Call `registry.get_definitions(None)`; collect schema names
- For each of `memory`, `session_search`, `delegate_task`, `todo_write`, `todo_read`, `cronjob`: assert schema name appears **exactly once** across all sources combined
- Location: `crates/ironhermes-tools/src/registry.rs` `#[cfg(test)]`

### Supporting Unit Tests

| Test | Covers | Location |
|------|--------|----------|
| `prerequisite_default_impl_returns_empty` | `Tool::prerequisites()` default | `registry.rs` tests |
| `is_available_default_walks_prerequisites` | Default `is_available()` walks required prereqs | `registry.rs` tests |
| `register_intercepted_panics_on_duplicate_with_tools` | D-15 panic guard | `registry.rs` tests |
| `register_tools_panics_on_duplicate_with_intercepts` | D-15 reverse guard | `registry.rs` tests |
| `list_unavailable_returns_missing_required_prereqs` | `list_unavailable()` correctness | `registry.rs` tests |
| `list_toolsets_returns_unique_set` | `list_toolsets()` deduplication | `registry.rs` tests |
| `toolset_disabled_excludes_all_member_tools` | toolset-level filter resolution order | `registry.rs` tests |
| `dispatch_intercepts_returns_some_for_known` | intercept routing | `registry.rs` tests |
| `dispatch_intercepts_returns_none_for_unknown` | fall-through to dispatch | `registry.rs` tests |
| `tools_config_default_has_correct_enabled_set` | D-20 default toolsets `[memory, session, agent, skills]` | `config.rs` tests |
| `toolset_name_slug_validation` | D-02 regex `[a-z0-9][a-z0-9-]*` | `toolset_cmd.rs` tests |

---

## Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command |
|--------|----------|-----------|-------------------|
| TOOL-01 | `is_available()` excludes tools with missing prereqs | unit | `cargo test -p ironhermes-tools is_available` |
| TOOL-01 | `prerequisites()` returns structured prereq list | unit | `cargo test -p ironhermes-tools prerequisite` |
| TOOL-02 | `hermes toolset enable` persists to active-profile config | integration | `cargo test -p ironhermes-cli toolset_enable_disable_persists` |
| TOOL-02 | Toolset-level filter applied in `get_definitions()` | unit | `cargo test -p ironhermes-tools toolset_disabled` |
| TOOL-03 | Registry expansion is the only registration call site | unit | `cargo test -p ironhermes-tools register_intercepted` |
| TOOL-04 | No schema duplication for intercepted tools | integration | `cargo test -p ironhermes-tools intercepted_tool_no_schema_duplicate` |
| TOOL-05 | `hermes toolset setup` walks missing prereqs | integration | `cargo test -p ironhermes-cli toolset_setup` |

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-cli/tests/toolset_integration.rs` — covers D-26 tests 1 + setup integration
- [ ] `crates/ironhermes-tools/tests/toolset_prereq.rs` — covers D-26 test 2 + env_lock pattern reuse
- [ ] `crates/ironhermes-tools/src/registry.rs` `#[cfg(test)]` block — covers D-26 test 3 + supporting unit tests
- [ ] No new framework install needed — standard cargo test harness already in workspace

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Stderr banner phrasing on `hermes toolset enable` | TOOL-02 | Banner is human-readable text; confirm visual layout | Run `hermes toolset enable web` and inspect stderr for `[toolset: web] enabled — schema cache will rebuild on next LLM call` |
| `hermes toolset list` aligned-columns rendering | TOOL-02 | Visual alignment / pluralization; Unicode column widths | Run `hermes toolset list` and inspect column alignment + ✓/✗ availability glyphs |
| `hermes toolset setup` rustyline UX | TOOL-05 | Interactive prompt loop; per-prereq accept/skip flow | Run `hermes toolset setup` with at least one missing prereq; verify each step; verify the `tools.skip_prompts` write path |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references (3 new test files)
- [ ] No watch-mode flags
- [ ] Feedback latency < 60s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
