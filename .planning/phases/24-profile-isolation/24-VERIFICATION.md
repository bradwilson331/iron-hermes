---
phase: 24-profile-isolation
verified: 2026-04-28T00:00:00Z
status: passed
score: 4/4
overrides_applied: 0
---

# Phase 24 — Verification Report

**Date:** 2026-04-28
**Phase:** 24 — Profile Isolation
**Verdict:** PASS

## Success Criteria Coverage

| # | Truth | Code Site | Test | Verdict |
|---|-------|-----------|------|---------|
| 1 | `hermes --profile work chat` uses `~/.ironhermes/profiles/work/` as HERMES_HOME, separate from default | `main.rs:resolve_and_set_profile` (line 443–464) — validates slug, constructs `~/.ironhermes/profiles/<name>/`, calls `set_var("IRONHERMES_HOME", ...)` before any consumer | `profile_env_var_set_before_scaffold` — PASS | PASS |
| 2 | Memory stores and session history for `work` profile are completely isolated from `personal` profile | Each profile's HERMES_HOME is a distinct directory; `get_hermes_home()` (constants.rs:33) routes memory factory, state.db, config, .env through the env-var pivot; `profile_isolation_smoke` exercises two distinct tempdirs and asserts zero cross-bleed | `profile_isolation_smoke` — PASS | PASS |
| 3 | Gateway started under one profile does not interfere with gateway under another profile (separate PID files) | `pid.rs:acquire_pid_lock` (line 167) writes `$HERMES_HOME/gateway.pid` atomically; `runner.rs:start()` (line 255) calls `acquire_pid_lock(&home)` where `home = get_hermes_home()` (profile-scoped after main.rs pivot); live-conflict returns D-12 error; `PidLockGuard::drop` removes file on shutdown | `gateway_pid_concurrent_refuse` — PASS; `gateway_pid_stale_is_cleaned` — PASS | PASS |
| 4 | Profile directory is scaffolded automatically on first use with the same `ensure_home_dirs()` structure as default | `main.rs:ensure_home_dirs()` (line 467–485) creates 8-subdir tree; called at line 242 AFTER `resolve_and_set_profile` sets IRONHERMES_HOME; Phase 23 preflight gate then fires on missing `config.yaml` and runs wizard | `first_use_scaffolds_and_runs_wizard` — PASS; `bare_hermes_first_use_works_unchanged` — PASS | PASS |

**Score:** 4/4 truths verified

## D-Decision Coverage

| D-ID | Decision | Verification Method | Verdict |
|------|----------|---------------------|---------|
| D-01 | `get_hermes_home()` body unchanged; only `PROFILES_SUBDIR` const added | `git show d0c0924 -- constants.rs` shows only `+pub const PROFILES_SUBDIR: &str = "profiles";` added; function body at lines 33–40 is identical to pre-Phase-24 form | PASS |
| D-02 | `--profile` wins over pre-set `IRONHERMES_HOME` silently | `resolve_and_set_profile` always overwrites IRONHERMES_HOME when `cli.profile` is Some, regardless of existing env var. Locked by `profile_env_var_set_before_scaffold` test | PASS |
| D-03 | Profile name slug validation `[a-z0-9][a-z0-9-]*` + reserved token rejection | `crates/ironhermes-core/src/profile.rs` — full validator with 13 unit tests; path traversal (`foo/bar`, `../etc`), uppercase, spaces, reserved (`default`, `current`, `none`), leading `_` all rejected | PASS |
| D-04 | Profile dirs always under `~/.ironhermes/profiles/` regardless of IRONHERMES_HOME | `resolve_and_set_profile` constructs path from `dirs::home_dir().join(".ironhermes").join(PROFILES_SUBDIR).join(validated)` — no env-var leak into path construction | PASS |
| D-05 | Bare `hermes` keeps `~/.ironhermes/` exactly as before — zero migration | `resolve_and_set_profile` returns `Ok(None)` when `cli.profile` is None; existing behavior untouched. `bare_hermes_first_use_works_unchanged` confirms profiles/ subdir is NOT created | PASS |
| D-06 | First `hermes --profile NEW chat` auto-scaffolds and auto-launches Phase 23 wizard | `ensure_home_dirs()` runs at line 242 after pivot; Phase 23 preflight gate at line 267 fires when `config.yaml` absent; wizard seam tested via `first_use_scaffolds_and_runs_wizard` | PASS |
| D-07 | `--profile` is global clap flag on `Cli` struct, available on every subcommand | `main.rs` line 93–98: `profile: Option<String>` field on top-level `Cli` struct | PASS |
| D-08 | One-line stderr banner when `--profile` active; bare hermes prints nothing | Lines 222–230: `eprintln!("[profile: {}] HERMES_HOME={}", ...)` only when `active_profile.is_some()`. Locked by `profile_banner_printed_to_stderr` (subprocess test) and `no_banner_when_profile_absent` | PASS |
| D-09 | PID file lives at `$HERMES_HOME/gateway.pid` | `pid.rs:16`: `const PID_FILENAME: &str = "gateway.pid"`. Write/read helpers always use `home.join(PID_FILENAME)` | PASS |
| D-10 | `gateway.pid` is 3-line hand-rolled YAML (`pid`, `started_at`, `profile`); atomic write via `NamedTempFile::persist()` | `pid.rs:29–82` — `to_yaml()` produces exact 3-line format; `write_gateway_pid` uses `NamedTempFile::new_in(home)` + `persist()` (same-filesystem rename). `pid_write_is_atomic` test covers round-trip | PASS |
| D-11 | Staleness detection via `kill(pid, 0)` with auto-delete on stale | `pid.rs:112–125` — `is_pid_alive()` uses `nix::sys::signal::kill(Pid, None)`: `Ok` → Live, `ESRCH` → Stale (auto-delete + proceed), `EPERM` → LiveOtherUser. Windows path panics with clear message | PASS |
| D-12 | Second `gateway run` same profile refuses with D-12 error, exit non-zero | `acquire_pid_lock` line 171: returns `Err(anyhow!("Gateway already running... Stop it first: hermes --profile {} gateway stop", ...))`. `gateway_pid_concurrent_refuse` verifies error text and file preservation | PASS |
| D-13 | No `hermes profile` subcommand introduced | Grep of `crates/` for `Commands::Profile`, `hermes profile`, `profile list/create/delete` — zero matches. Namespace not opened | PASS |
| D-14 | `hermes status` Profile section enumerates `profiles/*/` | `status_cmd.rs`: `enumerate_profiles()` (line 72), `current_profile()` (line 50), `ProfileSummary` struct (line 28), populated into `snap.profiles` at line 566, rendered in text output at line 577. Tests at lines 849, 856 | PASS |
| D-15 | `hermes config show` prepends `Profile: <name>` always-on | `config_cli.rs:115`: `println!("Profile: {}", profile_name)` above Learning Loop banner | PASS |
| D-16 | `hermes doctor` runs active-profile checks only, including gateway.pid liveness | `main.rs:493–538`: `cmd_doctor` prints profile name, checks `gateway.pid` via `read_gateway_pid` + `is_pid_alive`. `doctor_integration` tests pass | PASS |
| D-17 | Cross-crate types use plain Strings, not embedded downstream enums | `validate_profile_name` returns `Result<String, ProfileNameError>`; `GatewayPidRecord.profile` is `String`. No newtype enum crossing crate boundaries | PASS |
| D-18 | PID file write/read helpers live in `ironhermes-gateway`, not `ironhermes-core` | `crates/ironhermes-gateway/src/pid.rs` owns all helpers; CLI reads via `ironhermes_gateway::pid::read_gateway_pid` import | PASS |
| D-19 | Two mandatory integration tests exist and pass | `profile_isolation_smoke`: PASS. `gateway_pid_concurrent_refuse`: PASS | PASS |

## Threat Coverage

| Threat | Description | Mitigation Code | Test | Verdict |
|--------|-------------|-----------------|------|---------|
| T-24-01 | Path traversal via `--profile ../etc` or `foo/bar` | `validate_profile_name` rejects any char not in `[a-z0-9-]` via `InvalidChars`; also rejects leading dash | `rejects_path_traversal_slash`, `rejects_path_traversal_dotdot` in `profile.rs` unit tests | PASS |
| T-24-02 | Concurrent PID conflict + stale PID DoS | `acquire_pid_lock` refuses live PIDs (preserves file), auto-deletes stale PIDs via `kill(pid, 0)` probe | `gateway_pid_concurrent_refuse` (integration), `acquire_refuses_live_pid_and_preserves_file` + `acquire_overwrites_stale_pid` (unit) | PASS |
| T-24-03 | Info disclosure via `profiles[]` in JSON status output | `StatusSnapshot.profiles: Option<Vec<ProfileSummary>>` — field absent (`None`) when no profiles dir exists; only present when profiles have been created. Test at `status_cmd.rs:936` confirms field absent for bare install; `status_cmd.rs:948` confirms it appears when profiles exist | PASS |

## Test Run Evidence

```
$ cargo test -p ironhermes-cli --test profile_isolation profile_isolation_smoke 2>&1 | tail -5
running 1 test
test profile_isolation_smoke ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.00s

$ cargo test -p ironhermes-cli --test gateway_pid gateway_pid_concurrent_refuse 2>&1 | tail -5
running 1 test
test gateway_pid_concurrent_refuse ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s

$ cargo test -p ironhermes-cli --test profile_isolation 2>&1 | tail -5
running 5 tests
test profile_env_var_set_before_scaffold ... ok
test profile_isolation_smoke ... ok
test subagent_transcript_isolation ... ok
test profile_banner_printed_to_stderr ... ok
test no_banner_when_profile_absent ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.86s

$ cargo test -p ironhermes-cli --test gateway_pid 2>&1 | tail -5
running 2 tests
test gateway_pid_concurrent_refuse ... ok
test gateway_pid_stale_is_cleaned ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ cargo test -p ironhermes-cli --test profile_first_use 2>&1 | tail -5
running 2 tests
test bare_hermes_first_use_works_unchanged ... ok
test first_use_scaffolds_and_runs_wizard ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ cargo test -p ironhermes-gateway 2>&1 | grep "test result"
test result: ok. 68 passed; 0 failed; 1 ignored; ...
test result: ok. 3 passed; 0 failed; ...
test result: ok. 2 passed; 0 failed; ...

$ cargo test -p ironhermes-cli --test doctor_integration 2>&1 | tail -5
running 2 tests
test doctor_no_pid_file_is_healthy ... ok
test profile_doctor ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.73s
```

## Pre-existing Failures (Unrelated to Phase 24)

Two tests fail in `ironhermes-core` and are pre-existing failures unrelated to profile isolation:
- `commands::handlers::tests::dispatch_all_todo_stubs_return_not_yet_available`
- `provider::tests::provider_resolver_loads_disk_cache_at_build`

Neither touches `profile.rs`, `constants.rs`, or any Phase 24 delivery surface.

## Outstanding Concerns

None. All Phase 24 deliverables verified in code and locked by passing tests.

## Conclusion

Phase 24 goal **ACHIEVED**. Every named profile gets its own isolated `HERMES_HOME` via `resolve_and_set_profile` in `main.rs`, which pivots `IRONHERMES_HOME` before any consumer runs. Memory stores and sessions are isolated by path alone (the existing `get_hermes_home()` single resolution point is untouched per D-01). Gateway PID isolation is implemented via `pid.rs:acquire_pid_lock` wired into `runner.rs:start()`, with atomic writes, staleness detection, and live-conflict refusal all verified. First-use scaffolding and wizard launch are automatic via the unmodified Phase 23 preflight gate. All 4 success criteria pass. Both D-19 mandatory integration tests pass. No `hermes profile` subcommand was introduced (D-13 clean). The Phase 23 preflight gate condition is unchanged (no `|| cli.profile.is_some()` widening). All three threat vectors are mitigated and locked by tests.

---

_Verified: 2026-04-28_
_Verifier: Claude (gsd-verifier)_
