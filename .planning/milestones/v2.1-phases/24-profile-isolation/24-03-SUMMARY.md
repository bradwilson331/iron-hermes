---
phase: 24
plan: 03
subsystem: ironhermes-cli
tags: [phase-24, profile-isolation, cli, env-pivot, banner, integration-test]
dependency_graph:
  requires:
    - 24-01 (validate_profile_name, PROFILES_SUBDIR)
  provides:
    - --profile global clap flag on Cli struct
    - resolve_and_set_profile helper in main.rs
    - D-08 stderr banner emission
    - reordered main() pivot sequence (canonical order locked)
    - profile_isolation.rs Wave 0 test scaffolding
  affects:
    - Plans 06, 07 (wizard auto-launch, smoke test appends to profile_isolation.rs)
tech_stack:
  added:
    - dirs = { workspace = true } added to ironhermes-cli [dependencies]
  patterns:
    - unsafe set_var at process start (Rust 2024 edition, single-threaded startup)
    - OnceLock<Mutex<()>> env_lock for test isolation (mirrors setup_wizard.rs:10-13)
    - CARGO_BIN_EXE_ironhermes subprocess test pattern (Phase 21.8 P05 STATE)
    - skip-if-binary-missing early return for env-sensitive subprocess tests
key_files:
  created:
    - crates/ironhermes-cli/tests/profile_isolation.rs
  modified:
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-cli/Cargo.toml
decisions:
  - "dotenvy moved AFTER resolve_and_set_profile so Config::env_path() reads the pivoted IRONHERMES_HOME (Pitfall 1 fix — existing code had dotenvy before Cli::parse)"
  - "dirs added as runtime dep (not dev-dep) since resolve_and_set_profile calls dirs::home_dir() at process start"
  - "global = true on --profile flag per D-07; diverges from --yolo which uses global = false"
  - "Phase 23 preflight gate condition byte-for-byte unchanged per 23-VERIFICATION.md lock"
metrics:
  duration_seconds: 264
  completed_date: "2026-04-29"
  tasks_completed: 3
  files_created: 1
  files_modified: 2
---

# Phase 24 Plan 03: --profile Flag + Main() Pivot + D-08 Banner Summary

**One-liner:** `--profile NAME` global clap flag with `resolve_and_set_profile()` pivot (sets `IRONHERMES_HOME` early via `unsafe set_var`) plus D-08 stderr banner and 3-test integration suite — every existing consumer gets profile isolation for free.

## What Was Built

### `--profile NAME` Global Clap Flag

Added to `Cli` struct in `crates/ironhermes-cli/src/main.rs` with `#[arg(long, global = true, value_name = "NAME")]`. The `global = true` attribute (D-07) makes it available on every subcommand including `gateway run`, diverging from `--yolo` which uses `global = false`.

### `resolve_and_set_profile()` Helper Function

Private helper placed near `ensure_home_dirs()` in `main.rs`. Validates the profile name via `ironhermes_core::profile::validate_profile_name` (Plan 01's T-24-01 security gate), constructs `~/.ironhermes/profiles/<name>/` via `dirs::home_dir()` + `ironhermes_core::PROFILES_SUBDIR`, then pivots `IRONHERMES_HOME` via `unsafe { std::env::set_var(...) }`. Returns `Some(slug)` when active, `None` for bare `hermes`.

### Canonical main() Ordering (Pitfall 1 fix)

The existing code had `dotenvy → ensure_home_dirs → Cli::parse → preflight`. This was incorrect because `Config::env_path()` (called by dotenvy) routes through `get_hermes_home()` which reads `IRONHERMES_HOME` — so dotenvy was loading from the wrong home before the profile pivot. The corrected canonical order is:

```
Cli::parse() → resolve_and_set_profile → D-08 banner → dotenvy → ensure_home_dirs → preflight gate → dispatch
```

**Ordering evidence** (line numbers within `fn main` body):
```
line 10:  let cli = Cli::parse();
line 14:  let active_profile = resolve_and_set_profile(&cli)?;
line 36:  ensure_home_dirs().context(...)?;
line 49:  preflight::run_preflight_check(&cli).await?;
```
Strictly ascending: 10 < 14 < 36 < 49. Verified via `awk '/fn main/,/^}$/' | grep -n`.

### D-08 Stderr Banner

```rust
if let Some(ref name) = active_profile {
    eprintln!("[profile: {}] HERMES_HOME={}", name, ironhermes_core::display_hermes_home());
}
```

Uses `display_hermes_home()` for `~/`-relative path rendering. Emitted after `resolve_and_set_profile` sets the env var, so `display_hermes_home()` reads the pivoted value. Stdout untouched — pipes stay clean.

### Phase 23 Preflight Gate (UNCHANGED)

Verified byte-for-byte preserved:
```rust
let run_preflight = matches!(cli.command, Some(Commands::Chat { .. }) | None)
    && cli.execute.is_none();
```
`grep -c 'matches!(cli.command, Some(Commands::Chat'` returns 2 (preflight condition + `is_interactive_repl` — both unchanged from Phase 23).

### dotenvy Reorder (Deviation: Pitfall 1 auto-fix)

The existing code ran `dotenvy::from_path(Config::env_path())` BEFORE `Cli::parse()`. Since `Config::env_path()` calls `get_hermes_home()` (which reads `IRONHERMES_HOME`), dotenvy was loading from the wrong profile-less home. The reorder moves dotenvy AFTER `resolve_and_set_profile` so it reads from the now-pivoted path. This is a Rule 1 auto-fix per Pitfall 1 in the RESEARCH.md.

### Integration Test Suite — `profile_isolation.rs`

Wave 0 scaffolding with 3 tests, all passing:

| Test | ID | Result |
|------|----|--------|
| `profile_env_var_set_before_scaffold` | 24-03-01 | PASS |
| `profile_banner_printed_to_stderr` | 24-03-02 | PASS |
| `no_banner_when_profile_absent` | 24-03-02 neg | PASS |

Key implementation details:
- `env_lock()` via `OnceLock<Mutex<()>>` — mirrors `setup_wizard.rs:10-13`
- Subprocess tests use `CARGO_BIN_EXE_ironhermes` (not `_hermes`) per Phase 21.8 P05 STATE
- Skip-if-binary-missing fallback guards subprocess tests against non-bin test compilations
- File is structured for Plan 07 to append `profile_isolation_smoke` and subagent transcript tests

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug / Pitfall 1] dotenvy reordered after resolve_and_set_profile**
- **Found during:** Task 1, step 5 (plan explicitly flags: "check if dotenvy reads Config::env_path()")
- **Issue:** Existing main() ran `dotenvy::from_path(Config::env_path())` before `Cli::parse()`. `Config::env_path()` calls `get_hermes_home()` which reads `IRONHERMES_HOME`. With the profile pivot happening after parse, dotenvy would load `.env` from the wrong (non-profile) home.
- **Fix:** Moved dotenvy block to after `resolve_and_set_profile` and the D-08 banner, before `ensure_home_dirs`. This is the canonical Pitfall 1 order from RESEARCH.md.
- **Files modified:** `crates/ironhermes-cli/src/main.rs`
- **Commit:** 9f2e6e2

## Known Stubs

None. All implemented functionality is fully wired: `--profile` flag accepted by clap, `resolve_and_set_profile` validates and pivots env var, banner emits correctly, tests verify behavior end-to-end.

## Threat Surface Scan

No new network endpoints, auth paths, or schema changes introduced. The `resolve_and_set_profile` call site is the T-24-01 mitigator: `validate_profile_name` rejects path traversal (`../`, `/`, `\`), uppercase, reserved names (`default`, `current`, `none`), and leading underscores before any `home.join()` path construction. T-24-01 disposition: mitigated per plan.

## Self-Check: PASSED

| Item | Status |
|------|--------|
| crates/ironhermes-cli/src/main.rs | FOUND |
| crates/ironhermes-cli/tests/profile_isolation.rs | FOUND |
| .planning/phases/24-profile-isolation/24-03-SUMMARY.md | FOUND |
| commit 9f2e6e2 (Task 1) | FOUND |
| commit 94b7a39 (Task 2) | FOUND |
| commit 8aa71c6 (Task 3) | FOUND |
