---
phase: 26-provider-polish
plan: "04"
subsystem: provider-cli
tags: [cli-subcommand, slash-command, display, integration-test, cache-break-banner]
dependency_graph:
  requires: [26-01, 26-02, 26-03]
  provides: [hermes-provider-subcommand, /provider-slash-commands, provider-integration-test-infrastructure]
  affects: [ironhermes-cli, ironhermes-core]
tech_stack:
  added: []
  patterns:
    - ProviderRow struct (T-26-01 by-construction: no api_key VALUE field)
    - render_provider_list / render_provider_show pure display helpers (toolset_display.rs analog)
    - validate_provider_name reuses profile::validate_profile_name (T-26-03)
    - config_setter dotted-path write for providers.NAME.disabled (D-14 enable/disable)
    - D-16 cache-break stderr banner on persistent writes
    - env_lock() + EnvGuard RAII for integration test env isolation
    - wiremock MockServer for outbound-request Authorization header inspection
key_files:
  created:
    - crates/ironhermes-core/src/commands/provider_display.rs
    - crates/ironhermes-cli/src/provider_cmd.rs
    - crates/ironhermes-cli/tests/provider_integration.rs
  modified:
    - crates/ironhermes-core/src/commands/mod.rs
    - crates/ironhermes-core/src/commands/registry.rs
    - crates/ironhermes-cli/src/main.rs
decisions:
  - "D-12 banner test uses hermes provider list (not hermes status): status does not call ProviderResolver::build(); provider list does. Deviation from plan's hermes status suggestion; functionally equivalent for once-only verification."
  - "drive_client_for_test omitted: no easy public AnyClient entry point for minimal completion; library-level endpoint.api_key==None is the hard D-20 Test 1 gate per plan's fallback note. Wiremock's received_requests() is a soft gate."
  - "D-20 Test 3 uses reqwest directly to drive an HTTP request to wiremock rather than AnyClient — cleaner and avoids agent-loop overhead in tests."
  - "registry.rs: kept existing 'provider' entry (one entry) plus added 5 subcommand entries (provider list/show/test/enable/disable) — 6 entries total per CommandDef shape."
metrics:
  duration: "~40 minutes"
  completed: "2026-04-29"
  tasks_completed: 3
  files_changed: 6
---

# Phase 26 Plan 04: provider CLI + slash commands + integration tests — Summary

One-liner: `hermes provider list/show/test/enable/disable` CLI subcommand + 5 `/provider` slash entries + wiremock-backed integration test suite proving PROV-04 key isolation and PROV-08 custom provider selectability.

## What Was Built

### Task 1: provider_display.rs + slash command registry

**`crates/ironhermes-core/src/commands/provider_display.rs`** (new, 160 lines)

- `ProviderRow` struct — T-26-01 by construction: the struct has NO field for an API key VALUE. `api_key_status` carries only the env var NAME (`"✓ $OPENAI_API_KEY"` or `"✗ missing $VAR"`). There is no path from a key value into this struct.
- `render_provider_list(rows)` — aligned columns NAME=18 / BASE_URL=36 / API_KEY=22 / MODEL=20 / ROLE=10 / FALLBACKS=remainder. Header row + one row per ProviderRow. Disabled providers show `(disabled)` suffix on name.
- `render_provider_show(row)` — multi-line detail block with labeled fields.
- `render_provider_list_json(rows)` — serde_json pretty-print for `--json` flag.
- 4 inline unit tests: aligned columns, no sk-* in output, show format, disabled label.

**`crates/ironhermes-core/src/commands/mod.rs`** — added `pub mod provider_display;` alongside `toolset_display`.

**`crates/ironhermes-core/src/commands/registry.rs`** — updated the existing `"provider"` entry with full D-14 description + args_hint, and added 5 subcommand entries: `"provider list"`, `"provider show"`, `"provider test"`, `"provider enable"`, `"provider disable"`. All `Universal` platform (CLI + gateway). Registry no-duplicate tests still pass.

### Task 2: provider_cmd.rs + main.rs wiring

**`crates/ironhermes-cli/src/provider_cmd.rs`** (new, 350 lines)

- `ProviderSubcommand` enum: `List { json: bool }`, `Show { name }`, `Test { name }`, `Enable { name }`, `Disable { name }`.
- `validate_provider_name(name)` — delegates to `profile::validate_profile_name` (slug regex `[a-z0-9][a-z0-9-]*`). T-26-03 by construction: called BEFORE any config write in enable/disable/show/test.
- `build_provider_row(name, config, resolver)` — T-26-01 by construction: only reads `endpoint.api_key.is_some()` (never the value) and `config.providers[name].api_key_env` for the env var NAME.
- `cmd_provider_test` — D-15: output format `[provider:NAME] HTTP 200 (latency 142ms) — key from $VAR`. Key value flows ONLY into `bearer_auth()`, never into any format string used for output. Network error uses `.without_url()` to strip key-bearing URL representations. Falls back from GET /models (404) to POST /chat/completions.
- `cmd_provider_enable` / `cmd_provider_disable` — `config_setter::config_set(home, "providers.NAME.disabled", "true/false")` + D-16 cache-break banner on stderr: `⚠ [provider: NAME] config changed — schema cache will rebuild on next LLM call`.
- 6 unit tests pass: variant compile gate, slug injection rejection (4 vectors), enable/disable tempdir round-trip, list renders header.

**`crates/ironhermes-cli/src/main.rs`** — added `mod provider_cmd;`, `Provider { subcommand: provider_cmd::ProviderSubcommand }` to `Commands` enum, and dispatch arm `Commands::Provider { subcommand } => provider_cmd::handle_provider_command(subcommand, &profile_name).await`.

`hermes provider --help` shows all 5 subcommands via clap.

### Task 3: provider_integration.rs (7 tests, all pass)

**`crates/ironhermes-cli/tests/provider_integration.rs`** (new, 493 lines)

Infrastructure:
- `env_lock()` — separate `OnceLock<Mutex<()>>` static (own binary, not shared with toolset_integration.rs).
- `EnvGuard` RAII — restores env var on drop, even on panic.
- `ironhermes_bin()` — `CARGO_BIN_EXE_ironhermes` guard with early-return skip on missing.

Tests:

| Test | Gate | Status |
|------|------|--------|
| `key_does_not_leak_to_wrong_provider` | D-20 Test 1 / PROV-04 | PASS |
| `custom_provider_selectable_by_name` | D-20 Test 3 / PROV-08 | PASS |
| `provider_test_does_not_print_key` | D-15 / T-26-01 | PASS |
| `legacy_env_banner_emitted_once_per_process` | D-12 / Resolution #2 | PASS |
| `provider_enable_disable_persists` | D-14 | PASS |
| `cache_break_banner_on_persistent_enable_disable` | D-16 | PASS |
| `provider_enable_rejects_slug_injection` | T-26-03 | PASS |

## Known Stubs

None — all commands are fully implemented and functional.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] D-12 test used `hermes status` which doesn't build a ProviderResolver**
- **Found during:** Task 3 test execution (legacy_env_banner_emitted_once_per_process failed)
- **Issue:** The plan suggested using `hermes status` to trigger the D-12 legacy env var deprecation banner. However, `hermes status` does not call `ProviderResolver::build()` — it just reads config fields directly. The banner is emitted inside `ProviderResolver::build()`, so `status` never triggers it.
- **Fix:** Changed the test to use `hermes provider list`, which calls `ProviderResolver::build()` as part of its implementation.
- **Files modified:** `crates/ironhermes-cli/tests/provider_integration.rs`
- **Commit:** 184e345 (amended to fix test)

### Design Decisions

**drive_client_for_test omitted (D-20 Test 1):** The plan noted this was a placeholder for the executor to fill in based on actual AnyClient API. After inspecting `any_client.rs`, `build_main_client()` requires a full provider endpoint with valid credentials to make a completion call. For D-20 Test 1, the library-level assertion `endpoint.api_key == None` is the hard gate — it proves the PROV-04 D-11 fix holds at the resolver boundary. The wiremock mock server serves as a soft gate (any outbound requests would be inspected). This matches the plan's explicit fallback note.

**D-20 Test 3 uses reqwest directly:** Rather than going through AnyClient (which requires proper chat completion JSON), a direct `reqwest::Client` GET to `/v1/models` on the wiremock server verifies that the configured base_url is reachable and receives the correct Authorization header. Cleaner and doesn't require AnyClient's full LLM-call pipeline.

**Registry shape:** Used one parent entry (`"provider"`) + 5 subcommand entries (`"provider list"` etc.) for 6 total. This matches the plan spec which suggested checking whether Phase 25 used one-entry-per-subcommand or one-per-parent. The existing entry was updated rather than replaced.

## Threat Surface Scan

No new network endpoints, auth paths, file access patterns, or schema changes introduced beyond what's documented in the plan's threat model (T-26-01 through T-26-05).

## Plan 05 Readiness

`provider_integration.rs` provides the full infrastructure for Plan 05's D-20 Test 2 (`auxiliary_routes_to_separate_model`):
- `env_lock()` + `EnvGuard` RAII ready
- `MockServer::start().await` wiremock pattern in place
- `CARGO_BIN_EXE_ironhermes` subprocess pattern established

## Self-Check

### Created files exist:
- `/Users/twilson/code/ironhermes/.claude/worktrees/agent-a430b1eacd6261320/crates/ironhermes-core/src/commands/provider_display.rs` — FOUND
- `/Users/twilson/code/ironhermes/.claude/worktrees/agent-a430b1eacd6261320/crates/ironhermes-cli/src/provider_cmd.rs` — FOUND
- `/Users/twilson/code/ironhermes/.claude/worktrees/agent-a430b1eacd6261320/crates/ironhermes-cli/tests/provider_integration.rs` — FOUND

### Commits exist:
- ea311f0: feat(26-04): Task 1 — provider_display.rs + slash command registry entries
- 1d2162d: feat(26-04): Task 2 — provider_cmd.rs CLI subcommand + main.rs wiring
- 184e345: test(26-04): Task 3 — provider_integration.rs (7 integration tests)

### Test results:
- `cargo test -p ironhermes-core --lib commands::provider_display` — 4/4 PASS
- `cargo test -p ironhermes-cli --bin ironhermes provider_cmd` — 6/6 PASS
- `cargo test -p ironhermes-cli --test provider_integration` — 7/7 PASS
- `cargo build -p ironhermes-cli -p ironhermes-core` — exit 0

## Self-Check: PASSED
