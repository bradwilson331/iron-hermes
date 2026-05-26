# Phase 24: Profile Isolation - Pattern Map

**Mapped:** 2026-04-28
**Files analyzed:** 11 (4 new, 7 modified)
**Analogs found:** 11 / 11

---

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `crates/ironhermes-core/src/profile.rs` | validator / newtype | transform | `crates/ironhermes-core/src/skills.rs:39-53` (`validate_skill_name`) | exact |
| `crates/ironhermes-gateway/src/pid.rs` | atomic file I/O + liveness probe | file-I/O + request-response | `crates/ironhermes-core/src/memory_store.rs:377-391` (`write_target_atomic`) | role-match |
| `crates/ironhermes-cli/tests/profile_isolation.rs` | integration test | CRUD + env isolation | `crates/ironhermes-cli/tests/setup_wizard.rs` (env_lock + apply_minimum_viable_answers seam) | exact |
| `crates/ironhermes-cli/tests/gateway_pid.rs` | integration test | file-I/O | `crates/ironhermes-cli/tests/cron_default_deliver.rs` (tempdir + direct helper invocation pattern) | role-match |
| `crates/ironhermes-core/src/constants.rs` (add `PROFILES_SUBDIR`) | constant | — | same file lines 22-27 (existing block of `pub const` declarations) | exact |
| `crates/ironhermes-core/src/lib.rs` (add `pub mod profile;`) | module declaration | — | same file lines 1-18 (existing `pub mod` block) | exact |
| `crates/ironhermes-cli/src/main.rs:213-223` (pivot block before preflight gate) | CLI dispatch / env mutation | request-response | same file lines 199-223 (existing `main()` setup sequence) | exact |
| `crates/ironhermes-cli/src/main.rs:385-401` (`ensure_home_dirs` call site) | directory scaffolding | file-I/O | same function lines 385-401 (no change — pattern reference only) | exact |
| `crates/ironhermes-cli/src/main.rs:409-446` (`cmd_doctor` add liveness check) | CLI check command | request-response | same function lines 409-446 (existing `print_check` pattern) | exact |
| `crates/ironhermes-cli/src/config_cli.rs:113` (`cmd_config_show` prepend) | CLI display command | request-response | same function lines 113-151 (existing `println!` prepend pattern at D-17 Learning Loop banner) | exact |
| `crates/ironhermes-cli/src/status_cmd.rs` (Profile section + JSON field) | status collector / report struct | CRUD | same file lines 42-149 (existing `StatusReport` struct + `GatewayStatus` sub-struct pattern) | exact |
| `crates/ironhermes-cli/src/main.rs` (add `--profile` to `Cli` struct) | CLI flag | request-response | same file lines 47-91 (existing `Cli` struct with `--yolo` flag) | exact |
| `crates/ironhermes-gateway/src/lib.rs` (add `pub mod pid;`) | module declaration | — | same file lines 1-10 (existing `pub mod` block) | exact |
| `crates/ironhermes-gateway/src/runner.rs` (acquire_pid_lock before startup) | startup / Drop guard | request-response | same file lines 241-291 (`start()` numbered-step pattern) | exact |
| `crates/ironhermes-gateway/Cargo.toml` (promote tempfile) | build config | — | same file lines 33-35 (`[dev-dependencies]` block) | exact |

---

## Pattern Assignments

### `crates/ironhermes-core/src/profile.rs` (validator / newtype, transform)

**Analog:** `crates/ironhermes-core/src/skills.rs` lines 35-53

**Imports pattern** — follow existing `skills.rs` top-of-file style (no `regex` dep — hand-rolled):
```rust
// no extern regex dep; pure char iteration, same as skills.rs validate_skill_name
```

**Core validation pattern** (`skills.rs` lines 35-53):
```rust
/// Strict rules (reject on failure):
/// - Length 1..=64
/// - Must match `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`
/// - Must not contain consecutive hyphens (`--`)
fn validate_skill_name(name: &str) -> Result<(), &'static str> {
    if name.len() < SKILL_NAME_MIN_LEN {
        return Err("name is empty");
    }
    if name.len() > SKILL_NAME_MAX_LEN {
        return Err("name exceeds 64 characters");
    }
    if name.contains("--") {
        return Err("name contains consecutive hyphens");
    }
    if !SKILL_NAME_RE.is_match(name) {
        return Err("name does not match ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$");
    }
    Ok(())
}
```

**Divergences for `profile.rs`:**
- Use `pub fn validate_profile_name(name: &str) -> Result<String, ProfileNameError>` returning the validated name as `String` (D-17 plain-String cross-crate convention), not `Result<(), &'static str>`.
- Add reserved-word check before the regex check: reject `"default"`, `"current"`, `"none"`, or any name starting with `'_'`.
- Replace the `regex` dep with a pure char iteration: `name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')` (skills.rs uses a `Lazy<Regex>` — profile.rs must NOT, per RESEARCH.md "no-new-deps discipline").
- Return `Ok(name.to_string())` on success.
- Define a `ProfileNameError` enum with variants: `Empty`, `LeadingUnderscore`, `Reserved(String)`, `InvalidChars`, `TooLong`.
- Export from `lib.rs` via `pub mod profile;` (see lib.rs pattern below).

**Unit test pattern** (`skills.rs` lines 1512-1536, same file):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_skill_name_valid() { ... }

    #[test]
    fn test_validate_skill_name_invalid_regex() { ... }

    #[test]
    fn test_validate_skill_name_consecutive_hyphens() { ... }

    #[test]
    fn test_validate_skill_name_length_boundaries() { ... }
}
```
Follow this exact in-module `#[cfg(test)]` block structure. Add cases for: `"work"` (valid), `"default"` (reserved), `"_priv"` (leading underscore), `"foo/bar"` (invalid chars), `""` (empty).

---

### `crates/ironhermes-gateway/src/pid.rs` (atomic file I/O + liveness probe, file-I/O)

**Primary analog:** `crates/ironhermes-core/src/memory_store.rs` lines 376-391

**Atomic write pattern** (`memory_store.rs` lines 376-391):
```rust
/// Joins entries with ENTRY_DELIMITER, writes to temp file, fsync, rename (D-08).
fn write_target_atomic(&self, target: MemoryTarget) -> anyhow::Result<()> {
    let path = self.memory_dir.join(target.filename());
    let entries = self.entries.get(&target).map(|v| v.as_slice()).unwrap_or(&[]);
    let content = entries.join(ENTRY_DELIMITER);

    let tmp_path = path.with_extension("md.tmp");
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(content.as_bytes())?;
        f.flush()?;
        f.sync_all()?; // fsync before rename for durability
    }
    std::fs::rename(&tmp_path, &path)?;
    Ok(())
}
```

**Divergences for `pid.rs` `write_gateway_pid`:**
- Use `tempfile::NamedTempFile::new_in(home)?` + `.persist(&pid_path)` instead of the hand-rolled `path.with_extension(".tmp")` + `std::fs::rename` pattern. `NamedTempFile::persist()` handles cleanup on failure automatically (per RESEARCH.md Pattern 3). This is the preferred pattern for new code.
- `tempfile` must be moved from `[dev-dependencies]` to `[dependencies]` in `Cargo.toml` (see Cargo.toml section below).
- Content is the 3-line hand-rolled YAML: `"pid: {}\nstarted_at: {}\nprofile: {}\n"`.

**Secondary analog for liveness probe:** no existing codebase analog — use RESEARCH.md Pattern 4 directly:
```rust
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
    panic!("Gateway PID liveness check is not supported on this platform (v2.1 is Unix-only).");
}
```

**`acquire_pid_lock` control flow** — no analog; derive from D-11/D-12 decisions:
1. Read `home.join("gateway.pid")` — if absent, proceed to write.
2. Parse via `GatewayPidRecord::from_yaml`.
3. Call `is_pid_alive(record.pid)`.
4. `Stale` → `std::fs::remove_file(pid_path)?` then proceed.
5. `Live | LiveOtherUser` → return `Err(anyhow!("Gateway already running..."))`.
6. Write new `GatewayPidRecord` via `write_gateway_pid(home, &record)`.
7. Return `Ok(PidLockGuard { home: home.to_path_buf() })`.

**`PidLockGuard` Drop pattern** — follow `CancellationToken` RAII convention already in `runner.rs`; implement `Drop` to call `std::fs::remove_file(self.home.join("gateway.pid")).ok()`.

---

### `crates/ironhermes-cli/tests/profile_isolation.rs` (integration test, env isolation)

**Primary analog:** `crates/ironhermes-cli/tests/setup_wizard.rs` lines 1-60

**Env-lock + tempdir pattern** (`setup_wizard.rs` lines 10-35):
```rust
fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[test]
fn minimum_viable_answers_seed_full_config() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    unsafe {
        std::env::set_var("IRONHERMES_HOME", tmp.path());
    }

    let mut config = Config::default();
    let block = ironhermes_cli::setup::apply_minimum_viable_answers(
        &mut config,
        "openrouter",
        "sk-test",
        "openai/gpt-4o-mini",
        "y",
    );
    config.save_to(&tmp.path().join("config.yaml")).unwrap();
    ...
}
```

**Divergences for `profile_isolation.rs`:**
- Test name: `profile_isolation_smoke` (not `minimum_viable_answers_seed_full_config`).
- Allocate TWO `TempDir`s (`dir_a`, `dir_b`) — do not use `env_lock` + `set_var` for the isolation assertion itself. Per RESEARCH.md Pitfall 6: pass temp dir paths directly to helper functions rather than routing through `set_var` to avoid race under parallel test execution.
- Call `apply_minimum_viable_answers` against each dir separately by setting `IRONHERMES_HOME` before each call within the env_lock guard.
- Write a memory entry under `dir_a/memories/MEMORY.md`, assert it is absent from `dir_b/memories/MEMORY.md`.
- Assert `dir_a/state.db` and `dir_b/state.db` are distinct paths (both exist independently).
- Use `use tempfile::TempDir;` (already a dep in `ironhermes-cli`).

---

### `crates/ironhermes-cli/tests/gateway_pid.rs` (integration test, file-I/O)

**Primary analog:** `crates/ironhermes-cli/tests/cron_default_deliver.rs` lines 1-55

**Direct-helper-call pattern** (`cron_default_deliver.rs` lines 41-55):
```rust
#[test]
fn tg_enabled_single_chat_routes_to_origin() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_test_config(&tmp, true, &[12345]);
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    let config = Config::load().expect("config must load");
    let decision = config.telegram_default_origin();
    ...
}
```

**Divergences for `gateway_pid.rs`:**
- Test name: `gateway_pid_concurrent_refuse`.
- No `env_lock` or `set_var` needed — pass `home: &Path` directly to `ironhermes_gateway::pid::acquire_pid_lock(home)`.
- Write a `gateway.pid` file with `pid = std::process::id()` (current process — guaranteed alive) before calling `acquire_pid_lock`.
- Use `chrono::Utc::now().to_rfc3339()` for `started_at` field.
- Assert `acquire_pid_lock` returns `Err`.
- Assert `home.join("gateway.pid")` still exists and content is unchanged (not deleted on live conflict).
- Assert the error string contains `"Stop it first"`.
- Imports: `use tempfile::TempDir; use ironhermes_gateway::pid::{write_gateway_pid, acquire_pid_lock, GatewayPidRecord};`

---

### `crates/ironhermes-core/src/constants.rs` (add `PROFILES_SUBDIR` constant)

**Analog:** Same file, lines 22-27 (existing subsystem constant block):
```rust
/// Memory subsystem constants (D-05, D-06)
pub const ENTRY_DELIMITER: &str = "\n\u{00a7}\n";
pub const MEMORY_CHAR_LIMIT: usize = 2_200;
pub const USER_CHAR_LIMIT: usize = 1_375;
pub const MEMORY_FILENAME: &str = "MEMORY.md";
pub const USER_FILENAME: &str = "USER.md";
pub const MEMORIES_DIR: &str = "memories";
```

**Edit:** Insert immediately after the memory constants block (before `get_hermes_home()`):
```rust
/// Profile isolation constants (D-04, Phase 24)
pub const PROFILES_SUBDIR: &str = "profiles";
```
Do NOT touch `get_hermes_home()` or `display_hermes_home()` — per D-01 absolute constraint.

---

### `crates/ironhermes-core/src/lib.rs` (add `pub mod profile;`)

**Analog:** Same file, lines 1-18 (existing `pub mod` block):
```rust
pub mod commands;
pub mod config;
pub mod config_schema;
pub mod config_setter;
pub mod config_validate;
pub mod constants;
pub mod wizard;
pub mod context_scanner;
pub mod error;
pub mod memory_provider;
pub mod memory_store;
pub mod model_metadata;
pub mod models_cache;
pub mod provider;
pub mod skills;
pub mod token_estimator;
pub mod ssrf;
pub mod types;
```

**Edit:** Insert `pub mod profile;` into this block (alphabetically after `pub mod provider;`). Add `pub use profile::validate_profile_name;` to the `pub use` section at the appropriate position.

---

### `crates/ironhermes-cli/src/main.rs` — `--profile` global flag on `Cli` struct

**Analog:** Same file, lines 47-92 (`Cli` struct with `--yolo` as the most recent flag addition):
```rust
#[derive(Parser)]
#[command(
    name = "ironhermes",
    about = "IronHermes — The self-improving AI agent, rewritten in Rust",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    // ... existing flags ...

    /// Phase 21.7 Plan 08 (D-11 / D-12): enable autonomous (yolo) mode.
    /// Only honored on the batch (`-e`) and `chat` entry points — the
    /// `gateway` subcommand deliberately does NOT expose this flag
    /// (INV-21.7-05 / D-12). Top-level + Chat-subcommand flags OR together.
    #[arg(long, global = false)]
    yolo: bool,

    /// Use the classic (crossterm+rustyline) REPL instead of the Phase 22.4
    /// ratatui-backed REPL.
    #[arg(long = "classic-tui")]
    classic_tui: bool,
}
```

**Edit:** Add after `classic_tui` (or after `yolo` — maintain field ordering by phase):
```rust
    /// Activate a named profile (isolated HERMES_HOME under ~/.ironhermes/profiles/<name>/).
    /// Wins over any pre-set IRONHERMES_HOME env var silently (D-02).
    /// Available on every subcommand including `gateway run` (D-07).
    #[arg(long, global = true, value_name = "NAME")]
    profile: Option<String>,
```

**Key divergence from `--yolo`:** `global = true` (yolo uses `global = false` to exclude gateway — profile must work on ALL subcommands per D-07).

---

### `crates/ironhermes-cli/src/main.rs:213-223` — pivot block before preflight gate

**Analog:** Same file, lines 199-223 (the existing `main()` setup sequence):
```rust
#[tokio::main]
async fn main() -> Result<()> {
    let env_path = Config::env_path();
    if env_path.exists() {
        dotenvy::from_path(&env_path).ok();
    }

    // D-21: Create ~/.ironhermes/ subdirectories on first run (belt-and-suspenders)
    ensure_home_dirs().context("Failed to initialize IronHermes home directory")?;

    let cli = Cli::parse();

    // Phase 23 D-05: pre-flight check fires ONLY on interactive entry points
    let run_preflight = matches!(cli.command, Some(Commands::Chat { .. }) | None)
        && cli.execute.is_none();
    if run_preflight {
        preflight::run_preflight_check(&cli).await?;
    }
```

**Edit — required reordering and insertion.** The new sequence must be:
1. `dotenvy` load (unchanged — already before Cli::parse)
2. `let cli = Cli::parse();` — move BEFORE `ensure_home_dirs()`
3. **NEW:** `let active_profile = resolve_and_set_profile(&cli)?;` — validates name, sets `IRONHERMES_HOME`
4. **NEW:** emit D-08 banner if `active_profile.is_some()` (stderr, reuse `display_hermes_home()`)
5. `ensure_home_dirs()` — now runs against profile-scoped path
6. Phase 23 preflight gate — unchanged condition

**New helper function** (place near `ensure_home_dirs`):
```rust
/// Validates --profile name, computes profile path, sets IRONHERMES_HOME.
/// Returns Some(slug) when --profile was provided, None for bare hermes (D-01/D-04).
fn resolve_and_set_profile(cli: &Cli) -> Result<Option<String>> {
    let Some(ref name) = cli.profile else { return Ok(None) };
    let name = ironhermes_core::profile::validate_profile_name(name)
        .map_err(|e| anyhow::anyhow!("Invalid profile name '{}': {}", name, e))?;
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let profile_path = home
        .join(".ironhermes")
        .join(ironhermes_core::PROFILES_SUBDIR)
        .join(&name);
    // SAFETY: called once at process start, before any threads read IRONHERMES_HOME.
    // Same unsafe pattern established by Phase 21.6 test isolation (cron_default_deliver.rs).
    unsafe { std::env::set_var("IRONHERMES_HOME", &profile_path) };
    Ok(Some(name))
}
```

---

### `crates/ironhermes-cli/src/main.rs:409-446` — `cmd_doctor` add gateway.pid liveness check

**Analog:** Same function, lines 409-446 (existing `print_check` call pattern):
```rust
fn cmd_doctor() -> Result<()> {
    println!("{}", "IronHermes Doctor".bold().cyan());
    println!("{}", "─".repeat(40));

    let home = ironhermes_core::get_hermes_home();
    print_check("Home directory", home.exists());

    let config_path = Config::config_path();
    print_check("Config file", config_path.exists());

    let env_path = Config::env_path();
    print_check(".env file", env_path.exists());

    print_check("OpenRouter API key", std::env::var("OPENROUTER_API_KEY").is_ok());
    print_check("Anthropic API key", std::env::var("ANTHROPIC_API_KEY").is_ok());

    let db_path = home.join("state.db");
    print_check("State database", db_path.exists());

    println!();
    println!("{}", "Run `ironhermes status` for more details.".dimmed());
    Ok(())
}
```

**Edit:** Add after the `state.db` check and before the closing `println!()`:
```rust
    // D-16: gateway.pid liveness check (Phase 24)
    let pid_path = home.join("gateway.pid");
    if pid_path.exists() {
        let pid_ok = ironhermes_gateway::pid::read_gateway_pid(&home)
            .ok()
            .flatten()
            .map(|r| matches!(
                ironhermes_gateway::pid::is_pid_alive(r.pid),
                ironhermes_gateway::pid::PidLiveness::Live | ironhermes_gateway::pid::PidLiveness::LiveOtherUser
            ))
            .unwrap_or(false);
        print_check("Gateway PID (gateway.pid → live process)", pid_ok);
    } else {
        print_check("Gateway PID (not running)", true); // absent = healthy, not a failure
    }
```

Also add `profile_name: &str` parameter and print active profile at top:
```rust
fn cmd_doctor(profile_name: &str) -> Result<()> {
    println!("{}", "IronHermes Doctor".bold().cyan());
    println!("Profile: {}", profile_name);
    println!("{}", "─".repeat(40));
    // ... rest unchanged
```

---

### `crates/ironhermes-cli/src/config_cli.rs:113` — `cmd_config_show` prepend `Profile:` line

**Analog:** Same function, lines 113-142 (existing `println!` prepend at Phase 23 D-17 Learning Loop banner):
```rust
async fn cmd_config_show(hermes_home: &Path) -> Result<()> {
    let cfg_path = hermes_home.join("config.yaml");
    if !cfg_path.exists() {
        println!("No config.yaml found at {}.", cfg_path.display());
        println!("Run `hermes setup` to create one.");
        return Ok(());
    }
    ...
    // D-17: Learning Loop banner first.
    if memory_enabled && skill_gen {
        println!("🧠 Learning Loop: enabled (memory + skill generation)");
    } else {
        println!("⚠ Learning Loop: disabled ...");
    }
    println!();
```

**Edit:** Add `profile_name: &str` parameter to the signature. Insert the Profile line as the FIRST output (above the Learning Loop banner):
```rust
async fn cmd_config_show(hermes_home: &Path, profile_name: &str) -> Result<()> {
    // D-15 (Phase 24): always-on profile header, above Learning Loop banner.
    println!("Profile: {}", profile_name);
    println!();
    // ... existing early-return for missing config.yaml
    // ... existing D-17 Learning Loop banner
```

Thread `profile_name` from the call site: `cli.profile.as_deref().unwrap_or("default")` — available at `main()` dispatch time after the pivot (no filesystem reverse-lookup needed, per RESEARCH.md Pitfall 4).

---

### `crates/ironhermes-cli/src/status_cmd.rs` — Profile section + JSON field

**Analog:** Same file, lines 42-149 (existing `StatusReport` struct and subsection structs like `GatewayStatus`):
```rust
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct StatusReport {
    pub provider: ProviderStatus,
    pub memory: MemoryStatus,
    pub gateway: GatewayStatus,
    // ...
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct GatewayStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub platforms: Vec<String>,
    pub allowlist_count: usize,
    pub telegram_authed: bool,
}
```

**Edit:** Add `profiles` field to `StatusReport`:
```rust
pub struct StatusReport {
    pub provider: ProviderStatus,
    pub memory: MemoryStatus,
    pub gateway: GatewayStatus,
    pub subagents: SubagentStatus,
    pub processes: ProcessesStatus,
    pub mcp: McpStatus,
    pub yolo: YoloStatus,
    // D-14 (Phase 24): additive field; skip_serializing_if keeps v1 JSON schema non-breaking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profiles: Option<Vec<ProfileSummary>>,
}
```

**New subsection struct** (follow `GatewayStatus` field pattern):
```rust
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ProfileSummary {
    pub name: String,
    pub active: bool,  // true if this profile matches the current invocation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_pid: Option<u32>,
    pub gateway_live: bool,
    /// RFC3339 last-modified of the profile dir's config.yaml, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}
```

**New helper function** (add near `StatusReport::collect`):
```rust
pub fn enumerate_profiles(ironhermes_root: &Path, active_profile: Option<&str>) -> Vec<ProfileSummary> {
    // Walk ironhermes_root/profiles/*/config.yaml
    // Return one ProfileSummary per subdir that contains config.yaml
    // Mark active = (subdir_name == active_profile.unwrap_or(""))
    // Read gateway.pid via ironhermes_gateway::pid::read_gateway_pid if present
    // ...
}
```

**`fixture()` update** (lines 151-200): add `profiles: None` to the fixture to keep insta snapshots non-breaking.

---

### `crates/ironhermes-gateway/src/lib.rs` (add `pub mod pid;`)

**Analog:** Same file, lines 1-10 (existing `pub mod` block):
```rust
pub mod adapter;
pub mod backoff;
pub mod handler;
pub mod multimodal;
pub mod rate_limiter;
pub mod session;
pub mod stream_consumer;
pub mod telegram;
pub mod runner;
pub mod user_queue;
```

**Edit:** Insert `pub mod pid;` (alphabetically, after `pub mod multimodal;`). Add re-exports:
```rust
pub use pid::{
    read_gateway_pid, write_gateway_pid, acquire_pid_lock,
    GatewayPidRecord, PidLiveness, PidLockGuard,
};
```

---

### `crates/ironhermes-gateway/src/runner.rs` — insert `acquire_pid_lock` before startup

**Analog:** Same file, lines 241-291 (`start()` numbered-step pattern):
```rust
pub async fn start(&self) -> Result<()> {
    // --- 1. Resolve Telegram token ---
    let tg_config = self.config.gateway.platforms.get("telegram")...;
    let token = resolve_token(&tg_config.token).context("...")?;

    // --- 2. Create adapter ---
    let adapter: Arc<TelegramAdapter> = Arc::new(TelegramAdapter::new(&token));

    // --- 3. Verify token via getMe ---
    let bot_info = adapter.get_me().await.context("...")?;
    ...

    // --- 4. Register slash commands (D-17) ---
    ...
```

**Edit:** Insert as the FIRST numbered step (before token resolution), following the exact same `// --- N. Description ---` comment style:
```rust
pub async fn start(&self) -> Result<()> {
    // --- 0. Acquire PID lock (D-09/D-12, Phase 24) ---
    let home = ironhermes_core::get_hermes_home();
    let _pid_guard = crate::pid::acquire_pid_lock(&home)
        .context("Gateway startup refused: PID lock conflict")?;
    // _pid_guard's Drop impl removes gateway.pid on graceful shutdown.

    // --- 1. Resolve Telegram token --- (renumber existing steps if needed)
    ...
```

The `_pid_guard: PidLockGuard` variable is kept alive for the duration of `start()` by Rust's drop order — it is dropped (and the PID file removed) when `start()` returns, either normally or on error propagation.

---

### `crates/ironhermes-gateway/Cargo.toml` (promote `tempfile` to `[dependencies]`)

**Analog:** Same file, lines 33-35 (`[dev-dependencies]` block):
```toml
[dev-dependencies]
tempfile = "3"
```

**Edit:** Move `tempfile = "3"` from `[dev-dependencies]` to the `[dependencies]` block (after `base64`):
```toml
[dependencies]
...
base64 = { workspace = true }
tempfile = "3"

[dev-dependencies]
# tempfile moved to [dependencies] — needed at runtime for write_gateway_pid atomic write
```

---

## Shared Patterns

### Env-Lock for IRONHERMES_HOME mutation in tests

**Source:** `crates/ironhermes-cli/tests/setup_wizard.rs` lines 10-13, `crates/ironhermes-cli/tests/cron_default_deliver.rs` lines 14-20
**Apply to:** `profile_isolation.rs` (when set_var is needed within a guard), any other test that mutates `IRONHERMES_HOME`

```rust
fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
// Usage: let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
```

**Note:** `gateway_pid.rs` does NOT need this lock — it passes `&Path` directly to helpers, avoiding `set_var` entirely (per RESEARCH.md Pitfall 6).

### `print_check` display pattern

**Source:** `crates/ironhermes-cli/src/main.rs` lines 448-455
**Apply to:** `cmd_doctor` additions

```rust
fn print_check(name: &str, ok: bool) {
    let icon = if ok { "OK".green() } else { "MISSING".yellow() };
    println!("  [{icon}] {name}");
}
```

### `display_hermes_home()` for banner formatting

**Source:** `crates/ironhermes-core/src/constants.rs` lines 39-48
**Apply to:** D-08 banner in `main()` pivot block; do NOT call `profile_path.display()` directly

```rust
pub fn display_hermes_home() -> String {
    let home = get_hermes_home();
    if let Some(user_home) = dirs::home_dir()
        && let Ok(relative) = home.strip_prefix(&user_home)
    {
        return format!("~/{}", relative.display());
    }
    home.display().to_string()
}
// Usage after set_var: eprintln!("[profile: {}] HERMES_HOME={}", name, ironhermes_core::display_hermes_home());
```

### `#[serde(skip_serializing_if = "Option::is_none")]` additive JSON field

**Source:** `crates/ironhermes-cli/src/status_cmd.rs` lines 59-61, 67-68, 121-122
**Apply to:** `profiles: Option<Vec<ProfileSummary>>` field on `StatusReport`; keeps v1 JSON schema non-breaking

### `assert_cmd::Command` + `.env()` subprocess test pattern

**Source:** `crates/ironhermes-cli/tests/setup_wizard.rs` lines 128-136
**Apply to:** Any subprocess-level assertions in `profile_isolation.rs` (e.g., testing D-08 banner on stderr)

```rust
Command::cargo_bin("ironhermes")
    .unwrap()
    .env("IRONHERMES_HOME", tmp.path())
    .args(["setup", "gateway"])
    .assert()
    .success()
    .stdout(predicate::str::contains("..."));
```

---

## No Analog Found

All files have a close analog or self-analog within the codebase. The `pid.rs` liveness probe (`nix::sys::signal::kill`) has no existing codebase usage — use RESEARCH.md Pattern 4 verbatim.

| File | Role | Data Flow | Reason |
|------|------|-----------|--------|
| `ironhermes-gateway/src/pid.rs` — liveness probe section only | utility | request-response | No existing `nix::sys::signal` usage in codebase; `nix` is a workspace dep but unused in gateway today |

---

## Metadata

**Analog search scope:** `crates/ironhermes-core/src/`, `crates/ironhermes-gateway/src/`, `crates/ironhermes-cli/src/`, `crates/ironhermes-cli/tests/`
**Files scanned:** 15 source files read, 8 grep searches
**Pattern extraction date:** 2026-04-28

**Critical ordering constraint (RESEARCH.md Pitfall 1):** The only file where insertion order is load-bearing is `main.rs`. Required sequence:

```
dotenvy load → Cli::parse() → resolve_and_set_profile() → emit D-08 banner → ensure_home_dirs() → Phase 23 preflight gate → subcommand dispatch
```

`ensure_home_dirs()` currently runs BEFORE `Cli::parse()` in the existing code. The Phase 24 edit must move `Cli::parse()` above `ensure_home_dirs()` so the profile pivot can happen in between.
