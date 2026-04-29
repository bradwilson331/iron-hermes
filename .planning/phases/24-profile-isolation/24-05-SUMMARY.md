---
phase: 24
plan: "05"
subsystem: cli-surface
tags: [phase-24, profile-isolation, status, config-show, doctor, cli-surface]
dependency_graph:
  requires: ["24-02", "24-03", "24-04"]
  provides: ["profile-discovery-surface", "config-show-profile-prefix", "doctor-liveness-check"]
  affects: ["crates/ironhermes-cli/src/status_cmd.rs", "crates/ironhermes-cli/src/config_cli.rs", "crates/ironhermes-cli/src/main.rs"]
tech_stack:
  added: []
  patterns: ["skip_serializing_if = Option::is_none for additive JSON fields", "dirs::home_dir reverse-walk for profile root resolution"]
key_files:
  created:
    - crates/ironhermes-cli/tests/config_show_integration.rs
    - crates/ironhermes-cli/tests/doctor_integration.rs
  modified:
    - crates/ironhermes-cli/src/status_cmd.rs
    - crates/ironhermes-cli/src/config_cli.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-cli/tests/status_cmd_integration.rs
decisions:
  - "enumerate_profiles root resolution: bare mode uses hermes_home directly; profile mode walks two levels up from hermes_home to find ~/.ironhermes root"
  - "current_profile() invoked as reverse-walk fallback inside doctor (cli struct not in scope); cmd_config_show uses cli.profile.as_deref().unwrap_or('default') at dispatch site per RESEARCH Pitfall 4"
  - "StatusReport.profiles uses #[serde(skip_serializing_if = 'Option::is_none', default)] for non-breaking v1 JSON schema compatibility"
metrics:
  duration: "12 min"
  completed: "2026-04-29"
  tasks: 3
  files: 6
---

# Phase 24 Plan 05: CLI Surface (status Profile section, config show prefix, doctor liveness) Summary

Phase 24 Plan 05 wires the three operator-facing CLI surfaces — `hermes status`, `hermes config show`, and `hermes doctor` — to surface Phase 24 profile isolation state per D-14, D-15, D-16.

## One-liner

Additive Profile section in `hermes status` (text + JSON), always-on `Profile: <name>` prefix in `hermes config show`, and gateway.pid liveness check in `hermes doctor` — no new command namespaces, zero v1 JSON schema breakage.

## Tasks Completed

| Task | Name | Commit | Key files |
|------|------|--------|-----------|
| 1 | ProfileSummary + enumerate_profiles + current_profile + profiles field | c572770 | status_cmd.rs |
| 2 | Profile: prefix in cmd_config_show + gateway.pid liveness in cmd_doctor | d6488d6 | config_cli.rs, main.rs |
| 3 | Three integration test files | 674e5d5 | tests/status_cmd_integration.rs, tests/config_show_integration.rs, tests/doctor_integration.rs |

## What Was Built

### Task 1 — ProfileSummary + enumerate_profiles + current_profile (status_cmd.rs)

- `ProfileSummary` struct: `name`, `active`, `gateway_pid`, `gateway_live`, `last_modified`, `learning_loop` — metadata only per T-24-03 (no config values, no secrets)
- `current_profile()` public helper: reverse-walks IRONHERMES_HOME path components looking for `profiles/<slug>`, returns `"default"` for bare-hermes root
- `enumerate_profiles(root, active)`: walks `root/profiles/*/` subdirs containing `config.yaml`; per-profile gateway.pid liveness via Plan 02's `read_gateway_pid` + `is_pid_alive`; learning_loop field mirrors Phase 23 D-17 banner logic via `read_learning_loop_status`; alphabetically sorted
- `read_learning_loop_status()`: best-effort serde_yaml parse of `config.yaml`, returns `"enabled"`/`"disabled"`; caller defaults to `"unknown"` on failure
- `StatusReport.profiles: Option<Vec<ProfileSummary>>` with `#[serde(skip_serializing_if = "Option::is_none", default)]` — additive field, v1 JSON schema unchanged for bare-hermes installs
- `StatusReport::fixture()` and `collect()` set `profiles: None`
- `run_status` populates profiles after collect(); renders Profile section in text path if any entries
- Root resolution in `run_status`: bare mode → `hermes_home` directly; profile mode → `hermes_home.parent().parent()` (strips `profiles/<slug>`) — respects IRONHERMES_HOME overrides including test tempdirs
- 7 unit tests: enumerate_profiles empty/alphabetical/skips, current_profile default/slug, JSON field absent-when-None/present-when-Some

### Task 2 — Profile: prefix + gateway.pid liveness (config_cli.rs + main.rs)

- `cmd_config_show` signature extended to `async fn cmd_config_show(hermes_home: &Path, profile_name: &str)` (D-15)
- `Profile: {}` println is the **first** output in `cmd_config_show`, ABOVE the Phase 23 Learning Loop banner
- Call site in main.rs Config dispatch threads `cli.profile.as_deref().unwrap_or("default")` per RESEARCH Pitfall 4 (no filesystem reverse-walk at dispatch site)
- `cmd_doctor` prints active profile name at banner top using `ironhermes_cli::status_cmd::current_profile()`
- Gateway.pid liveness check added to `cmd_doctor`: if `gateway.pid` exists, probes with `is_pid_alive`; if absent, reports healthy `Gateway PID (not running)`

### Phase 23 Preflight Gate — UNCHANGED

The preflight gate at `main.rs:252-253`:
```rust
let run_preflight = matches!(cli.command, Some(Commands::Chat { .. }) | None)
    && cli.execute.is_none();
```
is **byte-for-byte preserved**. Phase 24's `--profile` resolution runs at line 220, well before this gate. `git diff` for lines 252-253 shows zero changes.

Verification:
- `grep -c 'matches!(cli.command, Some(Commands::Chat'` → 2 (unchanged)
- `grep -c '&& cli.execute.is_none()'` → 3 (unchanged)

### Task 3 — Integration Tests

| File | Test | What it verifies |
|------|------|-----------------|
| `tests/status_cmd_integration.rs` | `profile_section` (24-05-01) | `hermes status` prints "Profiles" header + both profile slugs when `IRONHERMES_HOME` contains seeded profiles |
| `tests/config_show_integration.rs` | `profile_line` (24-05-02) | First non-empty stdout line from `hermes config show` starts with `Profile:` and contains `default` for bare hermes |
| `tests/doctor_integration.rs` | `profile_doctor` (24-05-03) | `hermes doctor` stdout contains `Gateway PID` |
| `tests/doctor_integration.rs` | `doctor_no_pid_file_is_healthy` | Absent gateway.pid reports healthy branch |

All tests use `CARGO_BIN_EXE_ironhermes` with skip-if-missing fallback.

## Acceptance Criteria Verification

```
cargo build -p ironhermes-cli              → exit 0 ✓
grep -c 'pub fn enumerate_profiles' ...    → 1 ✓
grep -c 'Profile:' config_cli.rs           → 2 (format string + code comment) ✓
grep -c 'is_pid_alive' main.rs             → 1 ✓
grep -c 'skip_serializing_if.*Option::is_none' status_cmd.rs → 11 ✓
cargo test --test status_cmd_integration   → 5 passed ✓
cargo test --test config_show_integration  → 1 passed ✓
cargo test --test doctor_integration       → 2 passed ✓
cargo test status_cmd (unit tests)         → 7 passed ✓
```

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Wrong ironhermes_root in run_status for IRONHERMES_HOME test isolation**

- **Found during:** Task 3 (profile_section integration test failure)
- **Issue:** `run_status` computed `ironhermes_root = dirs::home_dir().join(".ironhermes")` — always the real home directory, ignoring `IRONHERMES_HOME`. The integration test sets `IRONHERMES_HOME=tmp_path` and puts profiles under `tmp_path/profiles/`, but enumeration looked in the real `~/.ironhermes/profiles/`.
- **Fix:** Use `hermes_home` (which already reflects `IRONHERMES_HOME`) as the base. Bare mode uses `hermes_home` directly; profile mode walks `hermes_home.parent().parent()` to find the root two levels above the profile slug.
- **Files modified:** `crates/ironhermes-cli/src/status_cmd.rs`
- **Commit:** 674e5d5

## Confirmation of cmd_config_show Signature Change

`cmd_config_show` now accepts `profile_name: &str`. The call site in main.rs threads the value directly from `cli.profile.as_deref().unwrap_or("default")` — no filesystem reverse-walk at the dispatch site, consistent with RESEARCH Pitfall 4.

## Confirmation of current_profile() in cmd_doctor

`cmd_doctor` calls `ironhermes_cli::status_cmd::current_profile()` (the reverse-walk fallback) to print the active profile at the doctor banner top, because the `cli` struct is not in scope inside `cmd_doctor`. This is the acceptable pattern documented in the plan's `<action>` step 4.

## JSON Schema Non-breaking Confirmation

`StatusReport.profiles` uses `#[serde(skip_serializing_if = "Option::is_none", default)]`. Bare-hermes installs with no `profiles/` directory produce `profiles: None` → field absent in JSON output → v1 consumers see byte-identical JSON. Verified by unit test `status_report_profiles_field_absent_when_none`.

## Known Stubs

None. All profile enumeration, learning loop status, and gateway liveness logic is fully wired.

## Threat Flags

No new threat surface beyond what the plan's `<threat_model>` covers. The `enumerate_profiles` path filter (UTF-8 dir name + `config.yaml` existence gate) mitigates T-24-05-PATH. The `is_pid_alive` signal-0 probe is non-disruptive per T-24-05-PROBE. `ProfileSummary` exposes only the metadata fields listed in T-24-03.

## Self-Check: PASSED

| Check | Result |
|-------|--------|
| crates/ironhermes-cli/src/status_cmd.rs | FOUND |
| crates/ironhermes-cli/src/config_cli.rs | FOUND |
| crates/ironhermes-cli/src/main.rs | FOUND |
| crates/ironhermes-cli/tests/status_cmd_integration.rs | FOUND |
| crates/ironhermes-cli/tests/config_show_integration.rs | FOUND |
| crates/ironhermes-cli/tests/doctor_integration.rs | FOUND |
| .planning/phases/24-profile-isolation/24-05-SUMMARY.md | FOUND |
| commit c572770 | FOUND |
| commit d6488d6 | FOUND |
| commit 674e5d5 | FOUND |
