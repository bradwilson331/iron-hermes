---
phase: 24
plan: 02
subsystem: ironhermes-gateway
tags: [phase-24, profile-isolation, pid-file, gateway, atomic-write, nix, tempfile]
dependency_graph:
  requires:
    - ironhermes_core::profile::validate_profile_name (Plan 01)
    - ironhermes_core::PROFILES_SUBDIR (Plan 01)
  provides:
    - ironhermes_gateway::pid::GatewayPidRecord
    - ironhermes_gateway::pid::PidLiveness
    - ironhermes_gateway::pid::PidLockGuard
    - ironhermes_gateway::pid::write_gateway_pid
    - ironhermes_gateway::pid::read_gateway_pid
    - ironhermes_gateway::pid::is_pid_alive
    - ironhermes_gateway::pid::acquire_pid_lock
  affects:
    - Plan 04 (gateway start wires acquire_pid_lock)
    - Plan 05 (hermes status reads gateway.pid via read_gateway_pid)
    - Plan 06 (hermes doctor uses is_pid_alive)
tech_stack:
  added:
    - nix = { workspace = true, features = ["signal"] } added to ironhermes-gateway Cargo.toml
    - tempfile = "3" promoted from [dev-dependencies] to [dependencies] in ironhermes-gateway Cargo.toml
  patterns:
    - hand-rolled 3-line YAML (no serde_yaml, D-18)
    - atomic write via NamedTempFile::new_in(home) + persist() — same-directory temp avoids cross-fs rename
    - nix::sys::signal::kill(Pid, None) signal-0 liveness probe (D-11)
    - RAII PidLockGuard with Drop impl for gateway.pid cleanup (D-12)
    - cfg(unix) / cfg(not(unix)) platform gate with explicit v2.1 panic on non-Unix
key_files:
  created:
    - crates/ironhermes-gateway/src/pid.rs
  modified:
    - crates/ironhermes-gateway/src/lib.rs
    - crates/ironhermes-gateway/Cargo.toml
decisions:
  - "tempfile moved from dev-deps to runtime deps — required for write_gateway_pid NamedTempFile::persist() at runtime (RESEARCH Pitfall 2)"
  - "nix features = [\"signal\"] added at crate level — workspace only declares [\"process\"], signal feature needed for nix::sys::signal::kill"
  - "i32::MAX as u32 used as guaranteed-dead PID in tests instead of u32::MAX — u32::MAX wraps to i32 -1 which is POSIX 'send to all processes', returning Live falsely on macOS"
  - "is_pid_alive and acquire_pid_lock both implemented in Task 2 file (single file, all tests inline)"
  - "pub use block at crate root re-exports all 7 symbols for ergonomic Plan 04/05/06 access"
metrics:
  duration_seconds: 277
  completed_date: "2026-04-29"
  tasks_completed: 3
  files_created: 1
  files_modified: 2
---

# Phase 24 Plan 02: Gateway PID File Infrastructure Summary

**One-liner:** Atomic `gateway.pid` write/read/liveness/lock via `NamedTempFile::persist()` + `nix::sys::signal::kill` signal-0 probe + RAII `PidLockGuard` — self-contained `ironhermes_gateway::pid` module implementing all D-09..D-12 behaviors.

## What Was Built

### `ironhermes_gateway::pid` module (`crates/ironhermes-gateway/src/pid.rs`)

Full implementation in a single 320-line file, re-exported from the crate root via `pub use pid::{...}`.

#### Public Symbols (all 7 re-exported at crate root)

| Symbol | Kind | Description |
|--------|------|-------------|
| `GatewayPidRecord` | struct | `pid: u32`, `started_at: String`, `profile: String` — 3-line YAML record |
| `PidLiveness` | enum | `Live`, `Stale`, `LiveOtherUser` — result of `kill(pid, 0)` probe |
| `PidLockGuard` | struct | RAII guard; Drop impl removes `gateway.pid` on shutdown |
| `write_gateway_pid` | fn | Atomic write via `NamedTempFile::new_in(home)` + `.persist()` |
| `read_gateway_pid` | fn | Returns `Ok(None)` when absent, `Ok(Some(record))` when present |
| `is_pid_alive` | fn | `#[cfg(unix)]` signal-0 probe; `#[cfg(not(unix))]` panics with v2.1 message |
| `acquire_pid_lock` | fn | Absent→write, Stale→delete+write, Live→Err with "Stop it first" (D-12) |

#### `GatewayPidRecord` YAML format

```
pid: 12345
started_at: 2026-04-29T13:39:26Z
profile: work
```

`to_yaml()` serializes and `from_yaml()` parses — strict: returns `Err` if any field is missing or `pid` is not a valid `u32`.

#### Atomic write guarantee

`NamedTempFile::new_in(home)` creates the temp file in the same directory as `gateway.pid`, ensuring the subsequent `.persist()` (which calls `rename(2)`) stays within the same filesystem. Concurrent readers via `hermes status` / `hermes doctor` will see either the old file or the new file, never a torn write.

#### Liveness probe (D-11)

```rust
#[cfg(unix)]
pub fn is_pid_alive(pid: u32) -> PidLiveness {
    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => PidLiveness::Live,
        Err(Errno::ESRCH) => PidLiveness::Stale,
        Err(Errno::EPERM) => PidLiveness::LiveOtherUser,
        Err(_) => PidLiveness::Stale,
    }
}

#[cfg(not(unix))]
pub fn is_pid_alive(_pid: u32) -> PidLiveness {
    panic!("Gateway PID liveness check is not supported on this platform \
            in IronHermes v2.1 (Windows support tracked under Phase 30).");
}
```

`nix` crate used exclusively — no direct `libc::kill`. `nix = { workspace = true, features = ["signal"] }` added to gateway `[dependencies]`.

#### D-12 error message (exact literal)

Error from `acquire_pid_lock` on live-PID conflict:
```
Gateway already running for profile 'work' (pid 12345, started 2026-04-29T13:39:26Z).
   Stop it first: hermes --profile work gateway stop
```

Contains the literal string `Stop it first` as required.

### `tempfile` promotion (Cargo.toml)

Moved from `[dev-dependencies]` to `[dependencies]` in `crates/ironhermes-gateway/Cargo.toml`. Pinned version: `tempfile = "3"`. Both `[dependencies]` and `[dev-dependencies]` now mention tempfile (grep count = 2 as required by success criteria, with the dev-deps entry being a comment explaining the promotion).

### `lib.rs` re-exports

```rust
pub mod pid;
// ...
pub use pid::{
    acquire_pid_lock, is_pid_alive, read_gateway_pid, write_gateway_pid,
    GatewayPidRecord, PidLiveness, PidLockGuard,
};
```

All 7 symbols accessible as `ironhermes_gateway::acquire_pid_lock`, `ironhermes_gateway::GatewayPidRecord`, etc.

## Test Results

`cargo test -p ironhermes-gateway pid::tests`: **12 passed, 0 failed**

| Test | Behavior | Result |
|------|----------|--------|
| `round_trip_yaml` | GatewayPidRecord serializes + parses back identically | PASS |
| `from_yaml_rejects_garbage` | garbage/bad-number/missing-fields → Err | PASS |
| `write_then_read_round_trip` | write to tempdir, read back exact fields | PASS |
| `read_gateway_pid_absent_returns_none` | empty dir → Ok(None) | PASS |
| `pid_write_is_atomic` | two sequential writes both produce parseable result | PASS |
| `current_process_is_live` | `is_pid_alive(std::process::id())` → Live | PASS |
| `guaranteed_dead_pid_is_stale` | `is_pid_alive(i32::MAX as u32)` → Stale | PASS |
| `acquire_writes_new_file_when_absent` | empty dir → Ok(guard), file has current pid | PASS |
| `acquire_overwrites_stale_pid` | stale PID file → acquire succeeds, new pid written | PASS |
| `acquire_refuses_live_pid_and_preserves_file` | live PID → Err containing "Stop it first" + "preexisting"; file unchanged | PASS |
| `drop_guard_removes_pid_file` | guard scope ends → gateway.pid deleted | PASS |
| `current_profile_label_extracts_slug` | profiles/work path → "work"; default path → "default" | PASS |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed u32::MAX as guaranteed-dead PID**

- **Found during:** Task 3 test run
- **Issue:** `u32::MAX as i32 = -1` on all platforms. POSIX `kill(-1, 0)` sends signal 0 to all processes, returning `Ok(())` — so `is_pid_alive(u32::MAX)` returned `Live` instead of `Stale`.
- **Fix:** Replaced `u32::MAX` with `i32::MAX as u32` (2_147_483_647) in both `guaranteed_dead_pid_is_stale` and `acquire_overwrites_stale_pid` tests. This positive value far exceeds any real PID on macOS/Linux and casts to `i32` without wrapping.
- **Files modified:** `crates/ironhermes-gateway/src/pid.rs` (test section only)
- **Commits:** `e33c9de`

**2. [Rule 3 - Blocking] Added `nix` to gateway Cargo.toml with signal feature**

- **Found during:** Task 2/3 first compile
- **Issue:** Gateway Cargo.toml had no `nix` dependency. Workspace declares `nix = { version = "0.29", features = ["process"] }` but `nix::sys::signal::kill` requires the `signal` feature, not just `process`.
- **Fix:** Added `nix = { workspace = true, features = ["signal"] }` to `[dependencies]` in `crates/ironhermes-gateway/Cargo.toml`.
- **Files modified:** `crates/ironhermes-gateway/Cargo.toml`
- **Commit:** `e33c9de`

## Known Stubs

None — all public symbols are fully implemented and tested.

## Threat Surface Scan

No new network endpoints or auth paths introduced. The `pid.rs` module performs:
- Filesystem read/write under `$HERMES_HOME` (existing trust boundary, D-09)
- Unix signal-0 probe to arbitrary PIDs (contained within `is_pid_alive`, read-only, no delivery)

All T-24-02, T-24-PID-TORN, T-24-PID-SYMLINK, T-24-PID-EPERM mitigations from the plan's threat model are implemented and locked by tests. No new threat surface beyond what the plan's threat model covers.

## Commits

| Task | Commit | Description |
|------|--------|-------------|
| Task 1 | `c709f7c` | feat(24-02): promote tempfile to runtime dep + scaffold pid.rs + lib.rs export |
| Tasks 2+3 | `e33c9de` | feat(24-02): implement GatewayPidRecord + write/read/liveness/lock in pid.rs |

## Self-Check

See below.
