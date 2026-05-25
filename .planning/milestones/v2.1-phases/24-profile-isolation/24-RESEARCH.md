# Phase 24: Profile Isolation - Research

**Researched:** 2026-04-28
**Domain:** Rust CLI profile isolation — env-var pivoting, PID file management, clap global flags, directory scaffolding
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** CLI sets `IRONHERMES_HOME` early in `main.rs` before any consumer. Zero changes to `constants.rs`.
- **D-02:** `--profile` wins over pre-set `IRONHERMES_HOME` silently.
- **D-03:** Profile name validation slug-style `[a-z0-9][a-z0-9-]*`. Reserved: `default`, `current`, `none`, any name beginning with `_`.
- **D-04:** Profile dirs always under `~/.ironhermes/profiles/<name>/`. `pub const PROFILES_SUBDIR: &str = "profiles";` in `ironhermes-core::constants`.
- **D-05:** Bare `hermes` keeps using `~/.ironhermes/` exactly as before. Zero migration.
- **D-06:** First `hermes --profile NEW chat` auto-scaffolds AND auto-launches Phase 23 setup wizard.
- **D-07:** `--profile <name>` is a global clap flag on the top-level `Cli` struct.
- **D-08:** One-line stderr banner when `--profile` active: `[profile: work] HERMES_HOME=~/.ironhermes/profiles/work/`.
- **D-09:** PID file at `$HERMES_HOME/gateway.pid` (root, not run/).
- **D-10:** PID file YAML 3 fields: `pid`, `started_at`, `profile`. Atomic write via `tempfile::NamedTempFile::persist()`. Hand-rolled YAML.
- **D-11:** Staleness via `kill(pid, 0)` probe. `#[cfg(unix)]` only; Windows panics with clear message.
- **D-12:** Second `gateway run` in same profile refuses with explicit error (exit code 2).
- **D-13:** Strict minimum CLI surface — only `--profile` global flag. No `hermes profile` subcommand.
- **D-14:** `hermes status` Profile section enumerates `~/.ironhermes/profiles/*/` with `config.yaml`. Active profile marked `*`. JSON adds `profiles[]`.
- **D-15:** `hermes config show` prepends `Profile: <name>` line (always-on; `default` for bare).
- **D-16:** `hermes doctor` runs active-profile checks only + new `gateway.pid` liveness check.
- **D-17:** New types in `ironhermes-core` crossing crate boundaries use plain Strings (e.g., `ProfileName` newtype wrapping `String`).
- **D-18:** PID file write/read helpers live in `ironhermes-gateway`. `pub fn read_gateway_pid(home: &Path) -> Result<Option<GatewayPidRecord>>` callable from CLI crates.
- **D-19:** Two integration tests mandatory: `profile_isolation_smoke` and `gateway_pid_concurrent_refuse`.

### Claude's Discretion

- Exact wording of D-08 banner, D-12 conflict error message, D-14 Profile section header.
- Whether to use `nix` crate or `libc` for `kill(pid, 0)` — check existing dep tree.
- Whether `ProfileName` is a tuple-struct newtype or plain validator function returning `Result<String, _>`.
- Whether D-10 atomic-write uses `tempfile::NamedTempFile::persist()` or hand-rolled write-then-rename.
- Whether D-11 `EPERM` branch logs differently than `ESRCH`.

### Deferred Ideas (OUT OF SCOPE)

- `hermes profile list/create/show/current/use/delete/rename/alias/export/import` (PROF-01..N, v2.2)
- `hermes gateway run --force`
- `hermes doctor --all` cross-profile sweep
- `IRONHERMES_PROFILE` env var fallback
- Gateway start TUI marker / persistent prompt prefix
- Profile templates / copy-from-existing
- Windows `OpenProcess`-based PID liveness
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| CFG-04 | Profile isolation: each profile gets own HERMES_HOME, config, memory, sessions, gateway PID | Covered end-to-end: env-var pivot (D-01), directory scaffolding (D-04/D-06), PID file (D-09..D-12), status/doctor/config-show surfaces (D-14..D-16), integration tests (D-19) |
</phase_requirements>

---

## Summary

Phase 24 adds per-profile directory isolation to IronHermes by inserting a single env-var mutation point at the top of `main()`. Because every consumer — memory factory, state.db, skills loader, config, `.env`, prompt builder — already reads `IRONHERMES_HOME` through `get_hermes_home()` in `ironhermes-core::constants`, setting the env var before any consumer runs gives all isolation for free without touching any consumer code. The phase is deliberately narrow: one new global clap flag (`--profile`), one new constant (`PROFILES_SUBDIR`), one new crate module (`ironhermes-gateway::pid`), small additive edits to three existing functions (`cmd_doctor`, `cmd_config_show`, `run_status`), and two mandatory integration tests.

The PID file mechanism is new greenfield work in `ironhermes-gateway` — no existing PID logic exists there. The hand-rolled 3-line YAML format avoids adding `serde_yaml` to that crate. The `nix` crate (already a workspace dep with `features = ["process"]`) is the correct choice for `kill(pid, 0)` liveness probing. The `tempfile` crate is already a regular dep in all four relevant crates (`ironhermes-core`, `ironhermes-cli`, `ironhermes-gateway`, and the workspace root), so `NamedTempFile::persist()` is the right atomic-write strategy.

**Primary recommendation:** Implement in execution order: (1) `PROFILES_SUBDIR` constant + `ProfileName` validator in `ironhermes-core`, (2) `--profile` global flag + env-var pivot in `main.rs`, (3) PID file module in `ironhermes-gateway`, (4) surface edits to `cmd_doctor` / `cmd_config_show` / `run_status`, (5) integration tests.

---

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Profile path resolution | CLI (`main.rs`) | Core (constants) | `main()` calls `set_var` early; `get_hermes_home()` in core reads it |
| Profile name validation | Core (`ironhermes-core`) | — | Shared by CLI and any future crate that constructs a profile name |
| Directory scaffolding | CLI (`main.rs`) | — | `ensure_home_dirs()` already lives here; called unchanged against profile path |
| PID file write/read | Gateway (`ironhermes-gateway`) | — | Only the gateway writes it; CLI reads via exported helper (D-18) |
| PID liveness probe | Gateway (`ironhermes-gateway::pid`) | — | `kill(pid, 0)` logic co-located with PID file; reused by CLI status/doctor |
| Profile discovery (status) | CLI (`status_cmd.rs`) | — | `enumerate_profiles()` walks filesystem; gateway only writes PID file |
| Config show profile header | CLI (`config_cli.rs`) | — | D-15 single-line prepend to existing `cmd_config_show` |
| Doctor gateway.pid check | CLI (`main.rs::cmd_doctor`) | — | D-16 reuses PID liveness probe from gateway crate |

---

## Standard Stack

### Core (already in workspace — no new deps needed)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `nix` | 0.29 (workspace) | `kill(pid, 0)` liveness probe | Already workspace dep with `features = ["process"]` [VERIFIED: Cargo.toml grep] |
| `tempfile` | 3 (all crates) | `NamedTempFile::persist()` for atomic PID write | Already dep in `ironhermes-gateway` (dev), `ironhermes-cli`, `ironhermes-core`, workspace root [VERIFIED: Cargo.toml grep] |
| `chrono` | workspace | ISO8601 `started_at` timestamp in PID YAML | Already a gateway dep [VERIFIED: ironhermes-gateway/Cargo.toml] |
| `clap` (derive) | workspace | Global `--profile` flag on `Cli` struct | Already the CLI framework; `global = true` on arg |
| `dirs` | workspace | `home_dir()` for profile path resolution | Already used in `get_hermes_home()` |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `anyhow` | workspace | Error context on PID file I/O | Gateway already uses it |
| `serde` | workspace | `GatewayPidRecord` struct (Serialize/Deserialize for JSON output in `hermes status --json`) | Gateway already uses it |

**Note on `tempfile` in `ironhermes-gateway`:** Currently only a `dev-dependency`. Phase 24 uses it at runtime for the atomic PID write. The gateway `[dependencies]` section needs `tempfile = "3"` added (not just dev-deps). [VERIFIED: ironhermes-gateway/Cargo.toml shows `tempfile = "3"` only under `[dev-dependencies]`]

**Note on `libc`:** `libc` is NOT a direct workspace dep; `nix` 0.29 with `features = ["process"]` provides `nix::sys::signal::kill` and `nix::unistd::Pid` — the correct choice (D-11 discretion resolved: use `nix`). [VERIFIED: workspace Cargo.toml]

---

## Architecture Patterns

### System Architecture Diagram

```
hermes --profile work chat
         │
         ▼
    main() — Cli::parse()
         │
         ├─ cli.profile = Some("work")
         │
         ▼
    validate_profile_name("work")    ← ironhermes-core::profile::validate (D-03)
         │
         ▼
    resolve_profile_path("work")     → ~/.ironhermes/profiles/work/
         │
         ▼
    set_var("IRONHERMES_HOME", path) ← unsafe; D-01 pivot point
         │
         ▼
    emit D-08 banner → stderr        ← display_hermes_home() from constants
         │
         ▼
    ensure_home_dirs()               ← called once; now operates on profile path
         │
         ▼
    Phase 23 preflight gate          ← fires AFTER pivot; sees profile config.yaml
    (run_preflight_check)            ← D-06: missing config → wizard for new profile
         │
         ▼
    subcommand dispatch
    ├─ Chat → run_chat()             ← get_hermes_home() returns profile path ✓
    ├─ Gateway → run_gateway()
    │    └─ acquire_pid_lock(home)   ← ironhermes-gateway::pid::acquire_pid_lock
    │         ├─ read existing gateway.pid
    │         ├─ kill(pid, 0) probe  ← nix::sys::signal::kill
    │         │   ├─ Ok → refuse (D-12 error, exit 2)
    │         │   ├─ ESRCH → stale, delete + proceed
    │         │   └─ EPERM → treat as live, refuse (D-12 + ownership note)
    │         └─ write gateway.pid   ← NamedTempFile::persist() atomic
    ├─ Status → run_status()         ← adds Profile section (D-14)
    ├─ Doctor → cmd_doctor()         ← adds gateway.pid liveness check (D-16)
    └─ Config Show → cmd_config_show ← prepends "Profile: work" (D-15)
```

### Recommended Project Structure

New files introduced by Phase 24:

```
crates/ironhermes-core/src/
└── profile.rs                   # validate_profile_name(), ProfileName newtype, PROFILES_SUBDIR const

crates/ironhermes-gateway/src/
└── pid.rs                       # GatewayPidRecord, write_gateway_pid(), read_gateway_pid(), acquire_pid_lock()

crates/ironhermes-cli/tests/
├── profile_isolation.rs         # D-19 test 1: profile_isolation_smoke
└── gateway_pid.rs               # D-19 test 2: gateway_pid_concurrent_refuse
```

Modifications to existing files:

```
crates/ironhermes-core/src/
├── constants.rs    # + pub const PROFILES_SUBDIR: &str = "profiles";
└── lib.rs          # + pub mod profile;

crates/ironhermes-gateway/src/
├── lib.rs          # + pub mod pid; + pub use pid::{read_gateway_pid, GatewayPidRecord};
└── Cargo.toml      # tempfile moved from dev-deps to [dependencies]

crates/ironhermes-cli/src/
├── main.rs         # --profile flag on Cli; env-var pivot; D-08 banner; D-16 doctor check
├── config_cli.rs   # D-15: prepend "Profile: <name>" in cmd_config_show
└── status_cmd.rs   # D-14: ProfileSummary struct + enumerate_profiles() + Profile section
```

### Pattern 1: Env-Var Pivot (D-01)

**What:** Set `IRONHERMES_HOME` before any consumer runs, using `unsafe { std::env::set_var }`.
**When to use:** Immediately after `Cli::parse()` returns, before `ensure_home_dirs()` and before the Phase 23 preflight gate.
**Example:**
```rust
// Source: Phase 24 D-01 design + Rust 2024 edition unsafe requirement
// (Phase 21.6 decision: "Rust 2024 edition requires unsafe blocks for env var mutation")
fn resolve_and_set_profile(cli: &Cli) -> Result<Option<String>> {
    let Some(ref name) = cli.profile else { return Ok(None) };
    let name = ironhermes_core::profile::validate_profile_name(name)?;
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let profile_path = home
        .join(".ironhermes")
        .join(ironhermes_core::PROFILES_SUBDIR)
        .join(&name);
    // SAFETY: called once at process start, before any threads are spawned
    // that read IRONHERMES_HOME. Same pattern as Phase 21.6 test isolation.
    unsafe { std::env::set_var("IRONHERMES_HOME", &profile_path) };
    Ok(Some(name))
}
```

### Pattern 2: Hand-Rolled PID YAML (D-10)

**What:** Write and parse exactly 3 YAML lines without `serde_yaml`.
**When to use:** `ironhermes-gateway::pid` module only.
**Example:**
```rust
// Source: Phase 24 D-10 design
pub struct GatewayPidRecord {
    pub pid: u32,
    pub started_at: String,  // ISO8601 UTC
    pub profile: String,     // slug or "default"
}

impl GatewayPidRecord {
    pub fn to_yaml(&self) -> String {
        format!(
            "pid: {}\nstarted_at: {}\nprofile: {}\n",
            self.pid, self.started_at, self.profile
        )
    }

    pub fn from_yaml(s: &str) -> Result<Self> {
        let mut pid = None;
        let mut started_at = None;
        let mut profile = None;
        for line in s.lines() {
            if let Some(v) = line.strip_prefix("pid: ") { pid = Some(v.trim().parse::<u32>()?); }
            if let Some(v) = line.strip_prefix("started_at: ") { started_at = Some(v.trim().to_string()); }
            if let Some(v) = line.strip_prefix("profile: ") { profile = Some(v.trim().to_string()); }
        }
        Ok(Self { pid: pid.context("missing pid")?, started_at: started_at.context("missing started_at")?, profile: profile.context("missing profile")? })
    }
}
```

### Pattern 3: Atomic PID File Write (D-10)

**What:** Write PID file via tempfile + rename for atomic POSIX semantics.
**Example:**
```rust
// Source: Phase 24 D-10; tempfile::NamedTempFile::persist() pattern
// Same pattern used in Phase 21.5 memory persistence and Phase 21.8 skill installer lock
use tempfile::NamedTempFile;
use std::io::Write;

pub fn write_gateway_pid(home: &Path, record: &GatewayPidRecord) -> Result<()> {
    let pid_path = home.join("gateway.pid");
    let mut tmp = NamedTempFile::new_in(home)?;
    tmp.write_all(record.to_yaml().as_bytes())?;
    tmp.flush()?;
    tmp.persist(&pid_path)
        .with_context(|| format!("atomic rename to {}", pid_path.display()))?;
    Ok(())
}
```

### Pattern 4: PID Liveness Probe (D-11)

**What:** Use `nix::sys::signal::kill(Pid, None)` (signal 0) to probe process existence.
**Example:**
```rust
// Source: Phase 24 D-11 design; nix 0.29 workspace dep with features = ["process"]
#[cfg(unix)]
pub fn is_pid_alive(pid: u32) -> PidLiveness {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    use nix::errno::Errno;
    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => PidLiveness::Live,
        Err(Errno::ESRCH) => PidLiveness::Stale,
        Err(Errno::EPERM) => PidLiveness::LiveOtherUser,
        Err(_) => PidLiveness::Stale,
    }
}

#[cfg(not(unix))]
pub fn is_pid_alive(_pid: u32) -> PidLiveness {
    panic!("Gateway PID liveness check is not supported on this platform (v2.1 is Unix-only). See Phase 30 for Windows support.");
}

pub enum PidLiveness { Live, Stale, LiveOtherUser }
```

### Pattern 5: Clap Global Flag (D-07)

**What:** Add `--profile` as a global flag on the top-level `Cli` struct, not on individual subcommands.
**Example:**
```rust
// Source: clap derive global flag pattern; mirrors --yolo flag which uses global = false
// D-07: "global" here means available on every subcommand
#[derive(Parser)]
#[command(name = "hermes", ...)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Activate a named profile (isolated HERMES_HOME under ~/.ironhermes/profiles/<name>/)
    #[arg(long, global = true, value_name = "NAME")]
    pub profile: Option<String>,

    // ... existing flags (execute, yolo) ...
}
```

**Note:** The existing `--yolo` flag uses `global = false` (per Phase 21.7 D-12 which intentionally excludes it from the `gateway` subcommand). The `--profile` flag uses `global = true` because it must work on every subcommand including `gateway run`.

### Anti-Patterns to Avoid

- **Modifying `get_hermes_home()` or `constants.rs` beyond adding `PROFILES_SUBDIR`:** D-01 is absolute. The env-var pivot at the call site is the entire mechanism. Any change to the function itself risks breaking every other consumer.
- **Calling `ensure_home_dirs()` before `set_var`:** The scaffold must happen AFTER the pivot, or it creates subdirs in the wrong location.
- **Calling `ensure_home_dirs()` before the Phase 23 preflight gate:** The gate must fire after `ensure_home_dirs()` runs (the wizard needs the home to exist). Order: parse → validate → set_var → banner → ensure_home_dirs → preflight.
- **Widening the Phase 23 preflight gate:** The gate at `main.rs:219-223` is `matches!(cli.command, Some(Commands::Chat {..}) | None) && cli.execute.is_none()`. Phase 24's `--profile` flag must NOT cause the gate to trigger on non-interactive subcommands. The gate condition must remain unchanged.
- **Using `serde_yaml` in `ironhermes-gateway` for the PID file:** Adds a heavy dep for 3 YAML lines. Hand-roll the parser.
- **Writing `gateway.pid` in a `run/` subdirectory:** D-09 explicitly locks it to the root of `HERMES_HOME`. Do not create a `run/` directory.
- **Creating `~/.ironhermes/profiles/default/`:** D-05 forbids this. Bare `hermes` must never touch the `profiles/` tree.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Atomic file write | Manual `write` + `rename` with temp path construction | `tempfile::NamedTempFile::persist()` | Already in all relevant crates; handles temp file cleanup on failure; POSIX-atomic |
| Process signal 0 | `libc::kill` directly | `nix::sys::signal::kill(Pid, None)` | `nix` already workspace dep with `features = ["process"]`; type-safe `Pid` wrapper; ergonomic error handling with `nix::errno::Errno` variants |
| Home dir detection | Env var fallback chain | `dirs::home_dir()` | Already used in `get_hermes_home()`; cross-platform |
| Directory slug validation | Ad-hoc regex | 10-line hand-rolled validator in `ironhermes-core::profile` | Phase 21.8 set the precedent: `to_skill_slug` is hand-rolled in `ironhermes-core::sanitize` (no `regex` dep in core); profile validator follows same pattern |

**Key insight:** The no-new-deps discipline is a project invariant (Phase 21.8 D-18 precedent). Every tool needed for Phase 24 already exists in the workspace dep tree.

---

## Common Pitfalls

### Pitfall 1: Profile Path Set Too Late

**What goes wrong:** If `set_var("IRONHERMES_HOME", ...)` fires after `ensure_home_dirs()` or the preflight gate, scaffolding happens in the wrong location and the wizard reads the wrong (or absent) config.
**Why it happens:** Temptation to set the env var inside the subcommand dispatch arm rather than at the top of `main()`.
**How to avoid:** The pivot must happen immediately after `Cli::parse()` returns, before any other setup. The required order is: `parse → validate_profile_name → set_var → emit_banner → ensure_home_dirs → preflight_gate → dispatch`.
**Warning signs:** Integration test sees profile dirs being created in `~/.ironhermes/` instead of `~/.ironhermes/profiles/work/`.

### Pitfall 2: `tempfile` as Dev-Only Dep in Gateway

**What goes wrong:** `ironhermes-gateway/Cargo.toml` currently has `tempfile = "3"` only under `[dev-dependencies]`. The `write_gateway_pid()` runtime function uses `NamedTempFile` — this will fail to compile in release builds.
**Why it happens:** `tempfile` was added as a dev dep only (for tests). Phase 24 makes it a runtime dep.
**How to avoid:** Move `tempfile = "3"` from `[dev-dependencies]` to `[dependencies]` in `crates/ironhermes-gateway/Cargo.toml`.
**Warning signs:** `cargo build --release` fails with "use of undeclared crate or module `tempfile`".

### Pitfall 3: Banner Uses Wrong Path Display

**What goes wrong:** The D-08 banner hard-codes an absolute path, showing `/Users/operator/.ironhermes/profiles/work/` instead of `~/.ironhermes/profiles/work/`.
**Why it happens:** Calling `profile_path.display()` directly instead of reusing `display_hermes_home()`.
**How to avoid:** The banner is emitted AFTER `set_var`, so `display_hermes_home()` (which calls `get_hermes_home()`) already returns the profile path with `~/` prefix. Call it, don't reformat the path manually.

### Pitfall 4: `current_profile()` Helper Over-Engineering

**What goes wrong:** Reverse-computing the active profile name by comparing `IRONHERMES_HOME` against all entries in `~/.ironhermes/profiles/*/` on every call to `cmd_config_show`.
**Why it happens:** Not storing the resolved profile name where it's needed.
**How to avoid:** The profile name (or `"default"`) is known at the top of `main()` immediately after the pivot. Pass it down to `cmd_config_show` as a parameter (or thread it through `cli.profile.as_deref().unwrap_or("default")`). No filesystem walk required.

### Pitfall 5: `gateway.pid` Cross-Profile Confusion

**What goes wrong:** `hermes status` reads `gateway.pid` from `~/.ironhermes/gateway.pid` (the default root) instead of the active profile's home, showing a stale or wrong gateway entry.
**Why it happens:** Hardcoded path in the status collector instead of using `get_hermes_home()`.
**How to avoid:** All PID file paths must derive from `ironhermes_core::get_hermes_home()`, which returns the correct path after the env-var pivot.

### Pitfall 6: Integration Test Isolation

**What goes wrong:** Tests that call `std::env::set_var` race with each other under parallel test execution.
**Why it happens:** Rust test runner runs tests in the same process on multiple threads by default.
**How to avoid:** Phase 21.6 established the pattern: `OnceLock<Mutex<()>>` as `ENV_LOCK` for tests that mutate env vars. For Phase 24's D-19 integration tests, use `tempfile::tempdir()` to create isolated `IRONHERMES_HOME` paths and pass them directly rather than relying on the env var — the tests simulate the post-pivot state by passing the temp dir path directly to helper functions, not through `set_var`.

### Pitfall 7: Preflight Gate Widening

**What goes wrong:** Adding `--profile` resolution logic that causes non-interactive subcommands (e.g., `hermes --profile work doctor`) to hit the preflight wizard.
**Why it happens:** The gate at `main.rs:219` checks the command variant. If `--profile` resolution logic runs a `match` block that accidentally triggers setup, doctor and other subcommands start launching the wizard.
**How to avoid:** The preflight gate condition must stay exactly as-is: `matches!(cli.command, Some(Commands::Chat {..}) | None) && cli.execute.is_none()`. Profile resolution runs before this gate but must not call `run_preflight_check` directly — that call stays in the existing gate block only.

---

## Code Examples

### Profile Name Validation

```rust
// Source: Phase 24 D-03; follows Phase 21.8 sanitize.rs hand-rolled pattern
// (confirmed: ironhermes-core/src/sanitize.rs NOT found via grep — module may be in skills.rs)
// Pattern: hand-rolled validator, no regex dep

const RESERVED_NAMES: &[&str] = &["default", "current", "none"];

pub fn validate_profile_name(name: &str) -> Result<String, ProfileNameError> {
    if name.is_empty() {
        return Err(ProfileNameError::Empty);
    }
    if name.starts_with('_') {
        return Err(ProfileNameError::LeadingUnderscore);
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(ProfileNameError::Reserved(name.to_string()));
    }
    let valid = name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let starts_ok = name.chars().next().map(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).unwrap_or(false);
    if !valid || !starts_ok {
        return Err(ProfileNameError::InvalidChars);
    }
    Ok(name.to_string())
}
```

### D-08 Banner Emission

```rust
// Source: Phase 24 D-08; mirrors Phase 21.7 YOLO banner pattern (stderr, once per process)
if active_profile.is_some() {
    eprintln!(
        "[profile: {}] HERMES_HOME={}",
        active_profile.as_deref().unwrap_or("default"),
        ironhermes_core::display_hermes_home()  // already returns ~/... form
    );
}
```

### D-15 Config Show Header

```rust
// Source: Phase 24 D-15; single prepend to cmd_config_show
// profile_name: &str — "default" for bare hermes, slug for --profile invocations
async fn cmd_config_show(hermes_home: &Path, profile_name: &str) -> Result<()> {
    // NEW: D-15 profile header
    println!("Profile: {}", profile_name);
    // existing Learning Loop banner follows...
```

### D-14 Profile Section in Status

```rust
// Source: Phase 24 D-14 design
pub struct ProfileSummary {
    pub name: String,
    pub path: PathBuf,
    pub last_modified: Option<std::time::SystemTime>,
    pub gateway_pid: Option<GatewayPidRecord>,
    pub gateway_live: bool,
    pub active: bool,  // true if this profile matches the current invocation
}

pub fn enumerate_profiles(ironhermes_root: &Path) -> Vec<ProfileSummary> {
    let profiles_dir = ironhermes_root.join(ironhermes_core::PROFILES_SUBDIR);
    // walk profiles_dir/*/config.yaml; return one entry per subdir with config.yaml present
    // ...
}
```

### D-12 Conflict Error

```rust
// Source: Phase 24 D-12
Err(anyhow::anyhow!(
    "Gateway already running for profile '{}' (pid {}, started {}).\n   Stop it first: hermes --profile {} gateway stop",
    record.profile, record.pid, record.started_at, record.profile
))
// exit code: std::process::exit(2)
```

---

## Runtime State Inventory

> This phase is additive (new profile directories scaffolded on demand). Not a rename/migration phase.

| Category | Items Found | Action Required |
|----------|-------------|-----------------|
| Stored data | None — existing `~/.ironhermes/` data untouched per D-05 | None |
| Live service config | None — gateway PID file mechanism is new greenfield code | None |
| OS-registered state | None — no OS-level registrations involved | None |
| Secrets/env vars | `IRONHERMES_HOME` — code reads it via `get_hermes_home()`; Phase 24 writes it at startup | Code edit only (set_var in main.rs) |
| Build artifacts | None | None |

**Nothing found requiring data migration.** Phase 24 is purely additive.

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| `nix` crate | D-11 PID liveness | Yes | 0.29 (workspace) | — |
| `tempfile` crate | D-10 atomic PID write | Yes (dev-dep only in gateway) | 3 | Move to [dependencies] |
| `chrono` crate | D-10 `started_at` ISO8601 | Yes (gateway dep) | workspace | — |
| `dirs` crate | D-04 profile path resolution | Yes (used in get_hermes_home) | workspace | — |
| `cargo test` | D-19 integration tests | Yes | workspace | — |

**Missing dependencies with no fallback:** None.

**Missing dependencies with fallback:** `tempfile` must be moved from `[dev-dependencies]` to `[dependencies]` in `crates/ironhermes-gateway/Cargo.toml`. This is a one-line Cargo.toml edit.

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in test + cargo test integration |
| Config file | `Cargo.toml` workspace (no separate test config) |
| Quick run command | `cargo test -p ironhermes-cli --test profile_isolation && cargo test -p ironhermes-cli --test gateway_pid` |
| Full suite command | `cargo test --workspace 2>&1 \| grep -E "^(test|FAILED|error)"` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| CFG-04 | Profile dirs isolated — memory written under profile A not visible under profile B | Integration | `cargo test -p ironhermes-cli --test profile_isolation -- profile_isolation_smoke` | No — Wave 0 |
| CFG-04 | Second `gateway run` in same profile refuses with exit 2 | Integration | `cargo test -p ironhermes-cli --test gateway_pid -- gateway_pid_concurrent_refuse` | No — Wave 0 |
| CFG-04 | Profile name `foo_bar` rejected; `work` accepted; `default` rejected | Unit | `cargo test -p ironhermes-core -- profile::tests` | No — Wave 0 |
| CFG-04 | `hermes --profile work` sets env var before ensure_home_dirs | Unit / Invariant | `cargo test -p ironhermes-cli --test profile_isolation -- profile_env_var_set_before_scaffold` | No — Wave 0 |
| CFG-04 | Stale PID (ESRCH process) auto-deleted; gateway proceeds | Unit | `cargo test -p ironhermes-gateway -- pid::tests::stale_pid_deleted_on_esrch` | No — Wave 0 |
| CFG-04 | `gateway.pid` written atomically (file is complete or absent) | Unit | `cargo test -p ironhermes-gateway -- pid::tests::pid_write_is_atomic` | No — Wave 0 |
| CFG-04 | `hermes status` Profile section lists correct profiles | Integration | `cargo test -p ironhermes-cli --test status_cmd_integration -- profile_section` | No — Wave 0 |

### D-19 Test Specifications

**Test 1: `profile_isolation_smoke`**

```rust
// Location: crates/ironhermes-cli/tests/profile_isolation.rs
// Purpose: two independent IRONHERMES_HOME paths must not share state
#[tokio::test]
async fn profile_isolation_smoke() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    // 1. Apply minimum-viable answers (Phase 23 seam) against dir_a
    //    to scaffold config.yaml + memory dirs
    // 2. Apply minimum-viable answers against dir_b
    // 3. Write a memory entry under dir_a (write to dir_a/memories/MEMORY.md)
    // 4. Read dir_b/memories/MEMORY.md — assert it does NOT contain the entry
    // 5. Write a different entry under dir_b
    // 6. Assert dir_a/memories/MEMORY.md does NOT contain dir_b's entry
    // Bonus: assert dir_a/state.db and dir_b/state.db are distinct files
}
```

**Test 2: `gateway_pid_concurrent_refuse`**

```rust
// Location: crates/ironhermes-cli/tests/gateway_pid.rs
// Purpose: D-12 conflict detection using the *current* process's pid (which is genuinely alive)
#[test]
fn gateway_pid_concurrent_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // 1. Write a gateway.pid with pid = std::process::id() (current process — guaranteed alive)
    //    started_at = chrono::Utc::now().to_rfc3339()
    //    profile = "test"
    // 2. Call ironhermes_gateway::pid::acquire_pid_lock(home)
    // 3. Assert it returns Err (D-12 conflict error)
    // 4. Assert gateway.pid still exists and is unchanged (not deleted)
    // 5. Assert error message contains the pid and "Stop it first"
}
```

**Additional test ideas surfaced by research:**

- **`profile_banner_printed_to_stderr`** — assert that when `--profile` is active, the D-08 banner appears on stderr but stdout is clean (no banner bleed into piped output).
- **`subagent_transcript_isolation`** — assert a transcript written under `--profile work` lands in `profiles/work/subagent-transcripts/`, not `~/.ironhermes/subagent-transcripts/` (per the CONTEXT.md "Specific Ideas" callout).
- **`default_profile_no_profiles_dir_created`** — assert bare `hermes` (no `--profile`) does not create `~/.ironhermes/profiles/` directory.
- **`config_show_prepends_profile_line`** — assert `hermes --profile work config show` output starts with `Profile: work`; bare `hermes config show` starts with `Profile: default`.
- **`stale_pid_auto_cleared`** — write a PID file with `pid = 99999999` (virtually guaranteed ESRCH on any machine); assert `acquire_pid_lock` succeeds and the stale file is deleted.

### Cross-Platform Considerations

- **Unix (macOS, Linux):** `nix::sys::signal::kill(Pid, None)` works correctly. The `#[cfg(unix)]` path is the only v2.1 target.
- **Windows (not a v2.1 deliverable):** The `#[cfg(not(unix))]` path must `panic!()` with a clear message: "Gateway PID liveness check is not supported on this platform in IronHermes v2.1". This is per D-11 and is explicitly documented in the Deferred section.
- **Test isolation on macOS:** macOS runs tests in the same process; the `ENV_LOCK` pattern from Phase 21.6 applies for any test that calls `std::env::set_var`. The D-19 tests avoid `set_var` entirely by accepting `&Path` parameters directly — this is the preferred approach.
- **`tempfile::tempdir()` on macOS:** Returns paths under `/var/folders/...` which is a symlink to `/private/var/folders/...`. When comparing `IRONHERMES_HOME` paths in tests, use `canonicalize()` or compare path components, not string equality.

### Sampling Rate

- **Per task commit:** `cargo test -p ironhermes-core -- profile::tests && cargo test -p ironhermes-gateway -- pid::tests`
- **Per wave merge:** `cargo test -p ironhermes-cli --test profile_isolation && cargo test -p ironhermes-cli --test gateway_pid`
- **Phase gate:** `cargo test --workspace` full suite green before `/gsd-verify-work`

### Wave 0 Gaps

- [ ] `crates/ironhermes-cli/tests/profile_isolation.rs` — covers CFG-04 profile isolation smoke (D-19 test 1)
- [ ] `crates/ironhermes-cli/tests/gateway_pid.rs` — covers CFG-04 concurrent refuse (D-19 test 2)
- [ ] `crates/ironhermes-core/src/profile.rs` — `validate_profile_name` unit tests module
- [ ] `crates/ironhermes-gateway/src/pid.rs` — unit tests for `write_gateway_pid`, `read_gateway_pid`, `is_pid_alive`

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | — |
| V3 Session Management | no | — |
| V4 Access Control | partial | Profile dirs use default umask; no explicit chmod needed unless `.env` is created (Phase 21.6 established chmod 600 on `.env` in entrypoint) |
| V5 Input Validation | yes | `validate_profile_name()` prevents path traversal characters (`/`, `..`, `~`); slug regex `[a-z0-9][a-z0-9-]*` is the control |
| V6 Cryptography | no | — |

### Known Threat Patterns

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Path traversal via `--profile ../../../etc` | Tampering | `validate_profile_name` rejects `/`, `.`, uppercase, spaces — any such char fails validation |
| PID file symlink attack | Tampering | `NamedTempFile::persist()` uses `rename(2)` which replaces the target atomically; a symlink at `gateway.pid` would redirect the write to the symlink target — mitigated by creating temp file in the same directory (`new_in(home)`) so rename stays within the directory |
| Stale PID file from killed gateway (no cleanup) | Denial of Service | D-11 staleness check (`ESRCH`) auto-deletes and proceeds; no operator intervention needed for clean crashes |
| Cross-profile IRONHERMES_HOME leak via env inheritance | Information Disclosure | D-02: `--profile` always overwrites any pre-set `IRONHERMES_HOME`; child processes inherit the profile-scoped value, which is correct |

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Single global `~/.ironhermes/` | Per-profile `~/.ironhermes/profiles/<name>/` via env-var pivot | Phase 24 | Multi-persona use case (work/personal/client) without reinstalling or juggling HERMES_HOME manually |
| No PID file in gateway | `gateway.pid` at HERMES_HOME root | Phase 24 | Prevents duplicate gateway instances; enables `hermes status` to show live/stale gateway state |

**Deprecated/outdated:**
- Direct `IRONHERMES_HOME` export as the profile-switching mechanism: works but is being superseded by `--profile` flag. Documented in D-02 (flag beats env, silently). The env var remains the internal pivot; `--profile` is the new operator UX.

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `ironhermes-core/src/sanitize.rs` does not export reusable slug helpers (grep returned no results) — Phase 24 hand-rolls `validate_profile_name` in a new `profile.rs` module | Standard Stack / Architecture | LOW: if `sanitize.rs` does export compatible helpers, Phase 24 could reuse them; hand-rolling is safe either way |
| A2 | `PROFILES_SUBDIR` constant does not yet exist in `constants.rs` (grep returned no results) | Standard Stack | LOW: if it already exists, skip adding it; if absent, must be added |
| A3 | `ironhermes-gateway::runner.rs` has an existing graceful-shutdown flow where `gateway.pid` cleanup (Drop on `PidLockGuard`) should hook in | Architecture Patterns | MEDIUM: if the shutdown flow does not have a clean hook point, a separate explicit cleanup call may be needed; Plan should budget a task for confirming the shutdown path |

**If this table is empty after executor review:** All non-A claims were verified directly from codebase grep and file reads.

---

## Open Questions

1. **`cmd_config_show` signature: how is `profile_name` threaded in?**
   - What we know: `cmd_config_show` currently takes `hermes_home: &Path` only (config_cli.rs:113).
   - What's unclear: The resolved profile name (or "default") needs to reach this function. Either thread it as a parameter through `handle_config_command`, or derive it from `hermes_home` by comparing against known profile dirs.
   - Recommendation: Pass `profile_name: &str` as a parameter — it's available at the `main()` dispatch point after the pivot. Avoids filesystem reverse-lookup on every `config show` call.

2. **Does `run_gateway()` in `main.rs` receive `home` explicitly or call `get_hermes_home()` internally?**
   - What we know: `run_gateway` is at `main.rs:1904`. It calls `get_hermes_home()` at the call site to build the ProcessRegistry session ID.
   - What's unclear: Whether the gateway runner passes `home` down to the point where `acquire_pid_lock` needs to be inserted.
   - Recommendation: Call `ironhermes_core::get_hermes_home()` at the `acquire_pid_lock` call site — it correctly returns the profile-scoped path after the env-var pivot.

3. **`hermes status --json` `profiles[]` array: what schema fields are required?**
   - What we know: D-14 says JSON adds a top-level `profiles[]` array; `StatusReport` is the stable v1 schema.
   - What's unclear: Whether `profiles[]` is a breaking schema change requiring a schema version bump.
   - Recommendation: Add `profiles: Option<Vec<ProfileSummary>>` with `#[serde(skip_serializing_if = "Option::is_none")]` — only populated when profiles dir exists. This is additive and non-breaking.

---

## Sources

### Primary (HIGH confidence)

- `crates/ironhermes-core/src/constants.rs:30-48` — `get_hermes_home()` and `display_hermes_home()` exact signatures [VERIFIED: direct file read]
- `crates/ironhermes-cli/src/main.rs:213-223` — Phase 23 preflight gate exact code [VERIFIED: direct file read]
- `crates/ironhermes-cli/src/main.rs:385-401` — `ensure_home_dirs()` exact 8-subdir list [VERIFIED: direct file read]
- `crates/ironhermes-cli/src/config_cli.rs:113-151` — `cmd_config_show` exact signature and current structure [VERIFIED: direct file read]
- `Cargo.toml` (workspace root) — `nix = { version = "0.29", features = ["process"] }` present [VERIFIED: grep]
- `crates/ironhermes-gateway/Cargo.toml` — `tempfile = "3"` only in `[dev-dependencies]` [VERIFIED: grep + file read]
- `crates/ironhermes-gateway/src/` — no existing `pid.rs` or PID logic [VERIFIED: ls + grep]
- `.planning/phases/23-configuration-cli-and-setup-wizard/23-VERIFICATION.md` — preflight gate location locked at `main.rs:213-223`; `apply_minimum_viable_answers` seam at `setup.rs:250` [VERIFIED: direct file read]

### Secondary (MEDIUM confidence)

- Phase 24 CONTEXT.md D-01..D-19 — all decisions verified against codebase code sites section [CITED: .planning/phases/24-profile-isolation/24-CONTEXT.md]
- Phase 21.6 STATE.md entry — "Rust 2024 edition requires unsafe blocks for env var mutation in tests" and `chmod 600 on .env` [CITED: .planning/STATE.md]
- Phase 21.8 STATE.md entry — "Plan 01: sanitize_name preserves underscore while to_skill_slug strips it; hand-rolled" [CITED: .planning/STATE.md — confirms hand-rolled slug pattern in core, no regex dep]

### Tertiary (LOW confidence)

None — all claims were verified directly from codebase or CONTEXT.md.

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all deps verified directly from Cargo.toml files
- Architecture patterns: HIGH — all insertion points verified from source code reads
- Pitfalls: HIGH — derived from verified code structure and established project precedents
- Test specifications: HIGH — D-19 specs are locked in CONTEXT.md; test patterns derived from existing Phase 23 seam

**Research date:** 2026-04-28
**Valid until:** 2026-05-28 (stable Rust workspace; no fast-moving external deps)
