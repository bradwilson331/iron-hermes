//! Decisive end-to-end test (Phase 51 Plan 07 UAT gap-closure round 2): proves the
//! two vault spawn variables (`IRONHERMES_KANBAN_VAULT_TOKEN` /
//! `IRONHERMES_KANBAN_VAULT_SOCKET`) actually reach the REAL OS-level child
//! process a vault-backed (`DispatchDecision::AllowFromVault`) dispatch spawns —
//! not merely that `build_kanban_worker_env`'s returned `Vec` contains them
//! (`worker_spawn_vault_env.rs`), and not merely that the dispatcher's mint logic
//! PASSES an `Option<WorkerVaultBootstrap>` to an INJECTED `spawn_fn`
//! (`dispatcher.rs`'s own `vault_mint_tests` module, `with_spawn_fn`).
//!
//! Both of those existing tests stop one layer short of the real
//! `Command::env_clear()` + `.envs(...)` + `.spawn()` boundary — exactly the same
//! class of gap the operator's live UAT caught for the `rusty-vault` cargo
//! feature forward (B-01): every individual layer was unit-tested in isolation,
//! and nothing drove the REAL default `spawn_fn` (`DispatcherContext::new`'s
//! `worker_spawn::spawn_worker_for_board`) through an actual subprocess spawn
//! and inspected what environment the child actually received.
//!
//! This test drives `run_dispatch_tick` with `DispatcherContext::new` (the real,
//! non-injected spawn function) end-to-end against a real vault, a real
//! `AllowFromVault` gate decision, and a real `Command::spawn()` — redirecting
//! `IRONHERMES_WORKER_BIN` to a throwaway shell script that dumps its own
//! process environment to a file, so the test can assert on exactly what the
//! child process saw, not what any intermediate Rust value claims it will see.

use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use ironhermes_core::config::{Config, ProviderConfig};
use ironhermes_kanban::dispatcher::DispatchGateFn;
use ironhermes_kanban::{CreateTaskOptions, DispatcherContext, KanbanConfig, KanbanStore, run_dispatch_tick};
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

const SLUG: &str = "vaultworkerenv";
const PROVIDER: &str = "vaultworkerenvprovider";

/// RAII guard sandboxing an env var — mirrors `dispatcher.rs`'s own
/// `vault_mint_tests::ScopedEnv` precedent exactly (kept local since that one is
/// `#[cfg(test)]`-private to the crate and unreachable from an integration test).
struct ScopedEnv {
    key: String,
    prev: Option<String>,
}

impl ScopedEnv {
    fn set(key: &str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: test-only, guarded by --test-threads=1 (this plan's mandated
        // invocation for any run touching this crate).
        unsafe { std::env::set_var(key, value) };
        Self {
            key: key.to_string(),
            prev,
        }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        // SAFETY: see `set` above.
        match &self.prev {
            Some(v) => unsafe { std::env::set_var(&self.key, v) },
            None => unsafe { std::env::remove_var(&self.key) },
        }
    }
}

fn write_vault_config(home: &Path, vault_data_dir: &Path) {
    let mut config = Config::default();
    config.vault.enabled = true;
    config.vault.backend = "rusty-vault".to_string();
    config.vault.rusty_vault.data_dir = vault_data_dir.to_path_buf();
    config.vault.rusty_vault.unseal_mode = "keyfile".to_string();
    config.model.provider = PROVIDER.to_string();
    config.providers.insert(
        PROVIDER.to_string(),
        ProviderConfig {
            api_key_env: Some("VAULTWORKERENV_API_KEY".to_string()),
            ..Default::default()
        },
    );
    config
        .save_to(&home.join("config.yaml"))
        .expect("save_to config.yaml");
}

fn init_vault(data_dir: &Path) {
    let rv_config = ironhermes_vault::RustyVaultConfig {
        data_dir: data_dir.to_path_buf(),
        unseal_mode: "keyfile".to_string(),
    };
    ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");
}

fn always_allow_from_vault() -> DispatchGateFn {
    std::sync::Arc::new(|_assignee: &str| {
        Box::pin(async { ironhermes_core::dispatch_gate::DispatchDecision::AllowFromVault })
    })
}

/// Write an executable shell script at `script_path` that dumps its own
/// process environment (post `env_clear()` + `.envs(...)`, i.e. exactly what a
/// real worker subprocess would see) to `dump_path`, ignoring all argv.
fn write_env_dump_script(script_path: &Path, dump_path: &Path) {
    let mut f = std::fs::File::create(script_path).expect("create diagnostic script");
    writeln!(f, "#!/bin/sh").unwrap();
    writeln!(f, "env > \"{}\"", dump_path.display()).unwrap();
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(script_path, std::fs::Permissions::from_mode(0o700))
            .expect("chmod diagnostic script");
    }
}

/// Poll for `path` to exist and be non-empty (the dumped env file), up to 5s —
/// the child is a real, detached subprocess this test does not `.wait()` on.
async fn wait_for_dump(path: &Path) -> String {
    for _ in 0..50 {
        if let Ok(contents) = std::fs::read_to_string(path)
            && !contents.is_empty()
        {
            return contents;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "diagnostic worker env dump never appeared at {} within 5s — the real \
         spawn_worker_for_board path never invoked (or never completed invoking) \
         the substituted IRONHERMES_WORKER_BIN script",
        path.display()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_from_vault_dispatch_delivers_both_vault_vars_to_the_real_spawned_process() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let vault_dir = tmp.path().join("vault");
    init_vault(&vault_dir);
    write_vault_config(&home, &vault_dir);
    let _home_env = ScopedEnv::set("IRONHERMES_HOME", &home.to_string_lossy());

    let dump_path = tmp.path().join("worker_env_dump.txt");
    let script_path = tmp.path().join("fake_worker.sh");
    write_env_dump_script(&script_path, &dump_path);
    let _worker_bin_env = ScopedEnv::set("IRONHERMES_WORKER_BIN", &script_path.to_string_lossy());

    let store_dir = TempDir::new().unwrap();
    let mut store = KanbanStore::open(store_dir.path().join("kanban.db")).expect("open kanban store");
    store
        .create_task("vault worker env test", SLUG, CreateTaskOptions::default())
        .unwrap();
    let store_arc = std::sync::Arc::new(TokioMutex::new(store));

    // The REAL default `DispatcherContext::new` — real `host_profile_credentials_from_disk`
    // (reads the `config.yaml` just written, vault.enabled=true), real
    // `spawn_fn` (`worker_spawn::spawn_worker_for_board`, no injection). Only the
    // gate is overridden, matching every other test in this suite's pattern —
    // the gate is not what this test is proving.
    let mut ctx = DispatcherContext::new(store_arc, KanbanConfig::default());
    ctx = ctx.with_gate_fn(always_allow_from_vault());
    assert!(
        ctx.profile_credentials.is_some(),
        "host_profile_credentials_from_disk should have hosted a real endpoint from the \
         vault.enabled=true config.yaml this test just wrote — if this is None, the \
         config write or IRONHERMES_HOME scoping above is broken, not the thing under test"
    );

    struct NoopAudit;
    impl ironhermes_core::profile_credentials::ProfileTokenAudit for NoopAudit {
        fn record_mint(&self, _slug: &str, _accessor: &str, _ttl: Duration) -> anyhow::Result<()> {
            Ok(())
        }
    }
    ctx = ctx.with_token_audit(std::sync::Arc::new(NoopAudit));

    run_dispatch_tick(&ctx).await.expect("run_dispatch_tick");

    let dumped_env = wait_for_dump(&dump_path).await;

    assert!(
        dumped_env.contains("IRONHERMES_KANBAN_VAULT_TOKEN="),
        "the real spawned child process env is missing IRONHERMES_KANBAN_VAULT_TOKEN — \
         this is the exact defect the operator's live UAT retry surfaced (mint + ledger \
         succeed, but the worker never receives the credential). Dumped env:\n{dumped_env}"
    );
    assert!(
        dumped_env.contains("IRONHERMES_KANBAN_VAULT_SOCKET="),
        "the real spawned child process env is missing IRONHERMES_KANBAN_VAULT_SOCKET. \
         Dumped env:\n{dumped_env}"
    );
}
