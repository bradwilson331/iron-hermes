#![cfg(feature = "rusty-vault")]
//! Real-subprocess contract test for the one-shot dispatcher's credential-endpoint lifetime
//! (Phase 51 Plan 19, G-51-6).
//!
//! # The bug this file proves is fixed
//!
//! Under `vault.enabled: true`, every worker spawned by the one-shot `ironhermes kanban
//! dispatch` verb died immediately with a `connect ... No such file or directory` — because
//! `cmd_dispatch` drops its `DispatcherContext` (and with it the PID-keyed
//! `ProfileCredentialEndpointHandle`, whose `Drop` unlinks the socket) the instant
//! `run_dispatch_tick` returns, while the detached workers it just spawned are still starting.
//! This is deterministic, not a race the path sometimes wins.
//!
//! # A NEW file, not an addition to `worker_vault_bootstrap.rs`
//!
//! That file's subject is the worker-side bootstrap; this file's subject is the DISPATCHER-side
//! host lifetime. Reuses its conventions exactly — the `#![cfg(feature = "rusty-vault")]` gate,
//! the `CARGO_BIN_EXE_ironhermes` lookup with a skip when absent, the real-vault fixture shape
//! (`RustyVaultConfig { unseal_mode: "keyfile", .. }`, `RustyVaultStore::init`/`open`,
//! `ProfileSecretStore::from_rusty_vault_store`), and the documented discipline of dropping
//! every open vault handle before anything else opens the same `data_dir`.
//!
//! # Why a real subprocess, not an in-process function call
//!
//! An in-process stand-in would prove the STUB was honored, not that the lifetime bug is fixed
//! — exactly the trap `worker_vault_bootstrap.rs`'s own module doc names. Every test here drives
//! the REAL compiled `ironhermes` binary three times (`kanban create`, `kanban dispatch`,
//! `kanban show`) against a real vault and a real stub worker executable.
//!
//! # Threading (T-51 project trap #1)
//!
//! `ironhermes-cli` races on env `set_var` under multi-threaded `cargo test`. This file's own
//! test process never mutates its own `HOME`/`IRONHERMES_HOME` (every home-scoped operation
//! goes through the real binary as a subprocess, exactly as this plan's `<action>` directs) —
//! but this file still follows the plan's mandated `--test-threads=1` invocation, matching every
//! other test file in this plan.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use secrecy::SecretString;
use tempfile::TempDir;

const PROFILE: &str = "onezhotvault";
const PROVIDER: &str = "onezhotprovider";
const SECRET_VALUE: &str = "sk-one-shot-dispatch-fixture-secret";

fn cargo_bin() -> Option<String> {
    match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => Some(p),
        Err(_) => {
            eprintln!("Skipping: CARGO_BIN_EXE_ironhermes not set");
            None
        }
    }
}

/// Seed a real rusty-vault at `vault_dir` with `PROFILE`'s `PROVIDER` secret, mirroring
/// `worker_vault_bootstrap.rs::real_vault::set_up_vault_and_mint`'s drop discipline: every open
/// handle from THIS setup is dropped before the dispatch subprocess (which opens the same
/// `data_dir` itself) ever runs.
async fn seed_vault(vault_dir: &Path) {
    let rv_config = ironhermes_vault::RustyVaultConfig {
        data_dir: vault_dir.to_path_buf(),
        unseal_mode: "keyfile".to_string(),
    };
    ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");
    let store = ironhermes_vault::RustyVaultStore::open(&rv_config).expect("vault open");
    let profile_store = ironhermes_vault::ProfileSecretStore::from_rusty_vault_store(&store);
    profile_store
        .put_profile_secret(
            PROFILE,
            PROVIDER,
            SecretString::from(SECRET_VALUE.to_string()),
        )
        .await
        .expect("write profile secret");
    drop(profile_store);
    drop(store);
}

/// Write `<home>/.ironhermes/config.yaml` — `vault.enabled: true`, `vault.backend:
/// rusty-vault`, `vault.rusty_vault.data_dir` pointed at `vault_dir` so the test's in-process
/// seeding and the dispatch subprocess agree on one path, `unseal_mode: keyfile`.
fn write_root_config(home: &Path, vault_dir: &Path) {
    let ironhermes_dir = home.join(".ironhermes");
    std::fs::create_dir_all(&ironhermes_dir).unwrap();
    std::fs::write(
        ironhermes_dir.join("config.yaml"),
        format!(
            "vault:\n  enabled: true\n  backend: rusty-vault\n  rusty_vault:\n    data_dir: {}\n    unseal_mode: keyfile\n",
            vault_dir.display()
        ),
    )
    .unwrap();
}

/// Write `<home>/.ironhermes/profiles/<PROFILE>/config.yaml` naming `PROVIDER` with an
/// `api_key_env`, and — deliberately — NO `.env` file for that profile, which is what makes the
/// dispatch gate return `AllowFromVault` rather than `Allow`.
///
/// The gate loads the PROFILE's OWN `config.yaml` — not the root one — to decide whether to try
/// the vault branch at all (`evaluate_profile_dispatch_core` reads `config.vault` from the
/// `Config` it parses at `profiles_root/<slug>/config.yaml`), so the profile config must carry
/// the SAME `vault:` block as the root config, pointed at the SAME `data_dir`.
fn write_vault_backed_profile(home: &Path, vault_dir: &Path) -> PathBuf {
    let profile_dir = home.join(".ironhermes").join("profiles").join(PROFILE);
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("config.yaml"),
        format!(
            "model:\n  provider: {PROVIDER}\n  default: {PROVIDER}/some-model\nproviders:\n  {PROVIDER}:\n    api_key_env: ONEZHOT_API_KEY\nvault:\n  enabled: true\n  backend: rusty-vault\n  rusty_vault:\n    data_dir: {}\n    unseal_mode: keyfile\n",
            vault_dir.display()
        ),
    )
    .unwrap();
    profile_dir
}

/// Write `<home>/.ironhermes/profiles/<name>/config.yaml` PLUS a plaintext `.env` carrying the
/// provider's key — so the dispatch gate resolves the plain `Allow` decision, not the vault one
/// (Task 2's scope-control fixture, `one_shot_dispatch_still_dispatches_a_non_vault_backed_task`).
fn write_dotenv_backed_profile(home: &Path, name: &str, provider: &str, env_var: &str) -> PathBuf {
    let profile_dir = home.join(".ironhermes").join("profiles").join(name);
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("config.yaml"),
        format!(
            "model:\n  provider: {provider}\n  default: {provider}/some-model\nproviders:\n  {provider}:\n    api_key_env: {env_var}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        profile_dir.join(".env"),
        format!("{env_var}=sk-dotenv-fixture-value\n"),
    )
    .unwrap();
    profile_dir
}

/// Write a stub "worker" executable at `path` that, on start:
///   1. reads `IRONHERMES_KANBAN_VAULT_SOCKET` from its own environment
///   2. checks whether that path exists AT THIS INSTANT
///   3. writes both observations to `marker_path` (baked into the script's own text — the
///      dispatcher's `SAFE_SYSTEM_VARS` is a closed 7-entry allowlist, so a marker path cannot
///      ride an arbitrary forwarded env var)
///   4. exits immediately
///
/// Never reads or records `IRONHERMES_KANBAN_VAULT_TOKEN` (D-15) — the marker file is written
/// inside a `TempDir` removed at test end regardless, but the stub itself must never even look
/// at the token.
fn write_stub_worker(path: &Path, marker_path: &Path) {
    let script = format!(
        "#!/bin/sh\nSOCKET=\"$IRONHERMES_KANBAN_VAULT_SOCKET\"\nif [ -n \"$SOCKET\" ] && [ -e \"$SOCKET\" ]; then\n  EXISTS=true\nelse\n  EXISTS=false\nfi\n{{\n  echo \"socket=$SOCKET\"\n  echo \"exists=$EXISTS\"\n}} > \"{marker}\"\nexit 0\n",
        marker = marker_path.display()
    );
    std::fs::write(path, script).expect("write stub worker script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
}

/// Parsed observation from the stub worker's marker file.
struct StubObservation {
    socket: String,
    exists: bool,
}

fn read_marker(marker_path: &Path) -> Option<StubObservation> {
    let contents = std::fs::read_to_string(marker_path).ok()?;
    let mut socket = String::new();
    let mut exists = false;
    for line in contents.lines() {
        if let Some(v) = line.strip_prefix("socket=") {
            socket = v.to_string();
        } else if let Some(v) = line.strip_prefix("exists=") {
            exists = v == "true";
        }
    }
    Some(StubObservation { socket, exists })
}

/// Base env every subprocess in this file needs: `PATH` (to resolve dynamic linking / shell)
/// plus `HOME` pointed at the fixture tempdir (`get_hermes_home()` falls back to
/// `dirs::home_dir().join(".ironhermes")` when `IRONHERMES_HOME` is unset — matching
/// `worker_vault_bootstrap.rs`'s own convention). This test process itself never sets `HOME` or
/// `IRONHERMES_HOME` on ITS OWN environment — only on the subprocess `Command`s below.
fn home_env(cmd: &mut Command, home: &Path) {
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

/// `ironhermes kanban create <title> --assignee <slug> --json` — returns the created task id.
fn create_task(bin: &str, home: &Path, assignee: &str, title: &str) -> String {
    let mut cmd = Command::new(bin);
    home_env(&mut cmd, home);
    cmd.args(["kanban", "create", title, "--assignee", assignee, "--json"]);
    let output = cmd.output().expect("run kanban create");
    assert!(
        output.status.success(),
        "kanban create failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("parse create JSON: {e}; stdout={stdout}"));
    parsed["task_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no task_id in create output: {stdout}"))
        .to_string()
}

/// `ironhermes kanban dispatch --json` with `IRONHERMES_WORKER_BIN` pointed at the stub. Returns
/// the subprocess's combined stdout+stderr (for the D-15 secret-absence assertion).
fn run_dispatch(bin: &str, home: &Path, worker_bin: &Path) -> (std::process::ExitStatus, String) {
    let mut cmd = Command::new(bin);
    home_env(&mut cmd, home);
    cmd.env("IRONHERMES_WORKER_BIN", worker_bin)
        .args(["kanban", "dispatch", "--json"]);
    let output = cmd.output().expect("run kanban dispatch");
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push('\n');
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status, combined)
}

/// `ironhermes kanban show <id> --json` — returns the parsed task JSON.
fn show_task(bin: &str, home: &Path, id: &str) -> serde_json::Value {
    let mut cmd = Command::new(bin);
    home_env(&mut cmd, home);
    cmd.args(["kanban", "show", id, "--json"]);
    let output = cmd.output().expect("run kanban show");
    assert!(
        output.status.success(),
        "kanban show failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("parse show JSON: {e}; stdout={stdout}"))
}

/// True when `task_json`'s events array contains a `blocked` event whose payload reason
/// contains `needle`. `format_task_json` (`ironhermes-cli/src/kanban/format.rs`) emits
/// `events[].payload` as the RAW STRING stored in the DB (already-serialized JSON text, e.g.
/// `"{\"reason\":\"...\"}"`) — not a nested JSON object — so this checks substring containment
/// on that string rather than indexing into it as an object (which would silently see `null`).
fn blocked_reason_contains(task_json: &serde_json::Value, needle: &str) -> bool {
    task_json["events"]
        .as_array()
        .map(|events| {
            events.iter().any(|e| {
                e["kind"].as_str() == Some("blocked")
                    && e["payload"]
                        .as_str()
                        .map(|r| r.contains(needle))
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// Task 1 (tracer): the ordering contract, as a disjunction over the general property.
// ─────────────────────────────────────────────────────────────────────────────────────────

/// THE regression test. EITHER no vault-backed worker was spawned (the stub's marker file is
/// absent) AND the task ended `blocked` with a reason containing the one-shot refusal marker;
/// OR a worker WAS spawned and the socket path it recorded existed at the instant it started.
///
/// At HEAD (before this plan's fix) the SECOND disjunct is taken — the stub IS spawned — and it
/// FAILS on the existence check: `cmd_dispatch` drops its `DispatcherContext` (unlinking the
/// PID-keyed socket) the instant `run_dispatch_tick` returns, racing (and, deterministically,
/// losing to) the detached worker's own startup. After the fix the FIRST disjunct is taken: the
/// one-shot host declines to host at all, so the task is refused before any worker spawns.
#[tokio::test(flavor = "multi_thread")]
async fn one_shot_dispatch_never_hands_a_worker_a_socket_that_is_already_gone() {
    let Some(bin) = cargo_bin() else { return };

    let home_tmp = TempDir::new().expect("home tempdir");
    let home = home_tmp.path();
    let vault_tmp = TempDir::new().expect("vault tempdir");
    let vault_dir = vault_tmp.path().join("vault");

    seed_vault(&vault_dir).await;
    write_root_config(home, &vault_dir);
    write_vault_backed_profile(home, &vault_dir);

    let worker_tmp = TempDir::new().expect("worker tempdir");
    let stub_path = worker_tmp.path().join("stub-worker");
    let marker_path = worker_tmp.path().join("marker.txt");
    write_stub_worker(&stub_path, &marker_path);

    let task_id = create_task(&bin, home, PROFILE, "one-shot vault dispatch regression");

    let (status, combined_output) = run_dispatch(&bin, home, &stub_path);
    assert!(
        status.success(),
        "kanban dispatch exited non-zero: {combined_output}"
    );

    // D-15: the fixture's secret value must never appear in the dispatch subprocess's output,
    // on either disjunct's path.
    assert!(
        !combined_output.contains(SECRET_VALUE),
        "the fixture's secret value leaked into the dispatch subprocess's combined output: \
         {combined_output}"
    );

    let task_json = show_task(&bin, home, &task_id);
    let status_str = task_json["status"].as_str().unwrap_or("<missing>");
    let marker = read_marker(&marker_path);

    match marker {
        None => {
            // Disjunct 1: no worker was spawned at all.
            assert_eq!(
                status_str, "blocked",
                "no worker was spawned (no marker file), but the task's status is {status_str:?} \
                 rather than \"blocked\" — task JSON: {task_json}"
            );
            assert!(
                blocked_reason_contains(
                    &task_json,
                    "one-shot dispatcher cannot host a vault credential endpoint"
                ),
                "no worker was spawned, and the task is blocked, but its blocked-event reason \
                 does not contain the one-shot refusal marker — task JSON: {task_json}"
            );
        }
        Some(obs) => {
            // Disjunct 2: a worker WAS spawned — the socket it recorded must have existed.
            assert!(
                obs.exists,
                "a worker WAS spawned and recorded socket path {:?} — but that path did NOT \
                 exist at the instant the worker started (the ordering bug this test proves is \
                 fixed: the one-shot dispatcher unlinked its socket before the worker could \
                 connect). Recorded observation: socket={:?} exists={}",
                obs.socket, obs.socket, obs.exists
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// Task 2: scope control — a non-vault-backed task on the same board still dispatches.
// ─────────────────────────────────────────────────────────────────────────────────────────

/// A profile WITH a plaintext `.env` carrying its provider's key (so the gate returns the plain
/// `Allow` decision, not the vault one) is dispatched by the same one-shot `ironhermes kanban
/// dispatch` run. Its stub worker IS invoked and the task does NOT end `blocked` — proving the
/// refusal is scoped to vault-backed tasks rather than a blanket kill on the verb.
#[tokio::test(flavor = "multi_thread")]
async fn one_shot_dispatch_still_dispatches_a_non_vault_backed_task() {
    let Some(bin) = cargo_bin() else { return };

    let home_tmp = TempDir::new().expect("home tempdir");
    let home = home_tmp.path();

    // No vault config at all for this fixture — the point is a dotenv-backed profile,
    // proving the one-shot verb keeps working for the common (non-vault) case.
    const NAME: &str = "onezhotdotenv";
    const PROVIDER_NAME: &str = "onezhotdotenvprovider";
    const ENV_VAR: &str = "ONEZHOTDOTENV_API_KEY";
    write_dotenv_backed_profile(home, NAME, PROVIDER_NAME, ENV_VAR);

    let worker_tmp = TempDir::new().expect("worker tempdir");
    let stub_path = worker_tmp.path().join("stub-worker");
    let marker_path = worker_tmp.path().join("marker.txt");
    write_stub_worker(&stub_path, &marker_path);

    let task_id = create_task(&bin, home, NAME, "non-vault dispatch scope control");

    let (status, combined_output) = run_dispatch(&bin, home, &stub_path);
    assert!(
        status.success(),
        "kanban dispatch exited non-zero: {combined_output}"
    );

    let task_json = show_task(&bin, home, &task_id);
    let status_str = task_json["status"].as_str().unwrap_or("<missing>");

    assert!(
        read_marker(&marker_path).is_some(),
        "the stub worker's marker file does not exist — the non-vault-backed task was never \
         spawned at all; task JSON: {task_json}"
    );
    assert_ne!(
        status_str, "blocked",
        "a non-vault-backed task must not be refused by the one-shot host's vault guard; task \
         JSON: {task_json}"
    );
    assert!(
        !blocked_reason_contains(&task_json, "dispatch gate: "),
        "a non-vault-backed task must not carry a dispatch-gate blocked reason; task JSON: \
         {task_json}"
    );
}
