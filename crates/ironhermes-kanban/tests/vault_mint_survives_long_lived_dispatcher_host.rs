//! Phase 51 Plan 10 gap-closure: proves (or disproves) that the REAL, shipped kanban
//! dispatcher path — a `DispatcherContext` whose credential endpoint host opens ONCE at
//! construction and stays alive across the whole dispatcher's lifetime — actually hands a
//! spawned worker a credential that WORKS when read back over the socket.
//!
//! # Why this file exists (the coverage gap it closes)
//!
//! Three existing tests each prove one layer, and NONE of them proves the layer this file
//! proves:
//!
//! - `dispatcher.rs`'s own `vault_mint_tests` module asserts the ledger records the
//!   accessor and that the injected `spawn_fn` received `vault.is_some() == true` — it
//!   never reads the minted token back over any socket.
//! - `worker_spawn_vault_env.rs` asserts `build_kanban_worker_env`'s returned `Vec`
//!   contains the two vault env vars — a pure, in-memory assertion with no vault, no
//!   socket, and no dispatcher involved at all.
//! - `vault_bootstrap_reaches_worker_process_env.rs` (the closest precedent, and this
//!   file's template) proves the two vault env vars reach a REAL spawned OS process's
//!   environment — but it never seeds a real profile secret and never reads the token
//!   back through the socket, so it cannot tell a WORKING credential from a
//!   token-shaped string the endpoint would refuse.
//!
//! `ironhermes-core`'s own `spawn_credential_decision.rs` (Plan 10 Task 2) DOES read back
//! over a real socket — but against a hand-constructed `ProfileCredentialHost`, not
//! `DispatcherContext`'s real, disk-config-driven construction path
//! (`host_profile_credentials_from_disk`), and it found that a host opened BEFORE a mint
//! cannot validate that mint's token. The open question this file answers: does the REAL
//! dispatcher — which unavoidably opens its host once at `DispatcherContext::new` and
//! mints on every subsequent tick — reproduce that same failure, or does something about
//! its real construction path differ?
//!
//! This test changes NOTHING about the mint/host/gate implementation. It is diagnostic
//! only, per Plan 10's Task 2 checkpoint escalation.

use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use ironhermes_core::config::{Config, ProviderConfig};
use ironhermes_kanban::dispatcher::DispatchGateFn;
use ironhermes_kanban::{CreateTaskOptions, DispatcherContext, KanbanConfig, KanbanStore, run_dispatch_tick};
use secrecy::{ExposeSecret, SecretString};
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

const SLUG: &str = "vaultlonglived";
const PROVIDER: &str = "vaultlonglivedprovider";
const SECRET_VALUE: &str = "sk-long-lived-dispatcher-host-value";

/// RAII guard sandboxing an env var — mirrors `dispatcher.rs`'s own
/// `vault_mint_tests::ScopedEnv` / `vault_bootstrap_reaches_worker_process_env.rs`'s local
/// copy exactly (kept local since the crate's own is `#[cfg(test)]`-private).
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

fn write_vault_config(home: &Path, vault_data_dir: &Path) -> Config {
    let mut config = Config::default();
    config.vault.enabled = true;
    config.vault.backend = "rusty-vault".to_string();
    config.vault.rusty_vault.data_dir = vault_data_dir.to_path_buf();
    config.vault.rusty_vault.unseal_mode = "keyfile".to_string();
    config.model.provider = PROVIDER.to_string();
    config.providers.insert(
        PROVIDER.to_string(),
        ProviderConfig {
            api_key_env: Some("VAULTLONGLIVED_API_KEY".to_string()),
            ..Default::default()
        },
    );
    config
        .save_to(&home.join("config.yaml"))
        .expect("save_to config.yaml");
    config
}

/// Init the vault AND seed `secret/profiles/{SLUG}/{PROVIDER}` — explicitly dropping every
/// seeding handle before returning, mirroring `ironhermes-cli/tests/worker_vault_bootstrap.rs`'s
/// `set_up_vault_and_mint` exactly: never hold more than one live open against the same
/// on-disk data_dir from this process at once. This is the ONE open that happens before
/// `DispatcherContext::new` opens the long-lived host below.
async fn init_vault_with_seeded_secret(data_dir: &Path) {
    let rv_config = ironhermes_vault::RustyVaultConfig {
        data_dir: data_dir.to_path_buf(),
        unseal_mode: "keyfile".to_string(),
    };
    ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");
    let store = ironhermes_vault::RustyVaultStore::open(&rv_config).expect("vault open");
    let profile_store = ironhermes_vault::ProfileSecretStore::from_rusty_vault_store(&store);
    profile_store
        .put_profile_secret(SLUG, PROVIDER, SecretString::from(SECRET_VALUE.to_string()))
        .await
        .expect("write profile secret");
    drop(profile_store);
    drop(store);
}

fn always_allow_from_vault() -> DispatchGateFn {
    std::sync::Arc::new(|_assignee: &str| {
        Box::pin(async { ironhermes_core::dispatch_gate::DispatchDecision::AllowFromVault })
    })
}

/// Write an executable shell script at `script_path` that dumps its own process
/// environment (post `env_clear()` + `.envs(...)`, i.e. exactly what a real worker
/// subprocess would see) to `dump_path`, ignoring all argv. Mirrors
/// `vault_bootstrap_reaches_worker_process_env.rs`'s identical helper.
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

/// Poll for `path` to exist and be non-empty (the dumped env file), up to 5s — the child
/// is a real, detached subprocess this test does not `.wait()` on.
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

/// Extract `KEY=value` from a dumped `env` text blob — the value runs to end of line
/// (values here are a UUID-shaped socket path or a base64/opaque token, neither of which
/// contains a newline).
fn extract_env_var<'a>(dumped_env: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    dumped_env
        .lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
}

/// THE QUESTION THIS FILE ANSWERS. Real vault, real seeded secret, the REAL
/// `DispatcherContext::new` construction path (host opens ONCE, before any mint, and
/// stays alive — genuinely long-lived, not hand-rolled), a real dispatch tick that mints
/// against that already-open host, a real spawned OS process, and then a REAL
/// `read_profile_credential` socket read using the token+socket the real child process
/// actually received. Only the gate is overridden (`always_allow_from_vault`) — matching
/// every other test in this suite's pattern, since the gate's own correctness is proven
/// elsewhere and is not what this file is checking.
///
/// Was `#[ignore]`d, DOCUMENTED, HONEST — same treatment as
/// `ironhermes-core/tests/spawn_credential_decision.rs`'s
/// `bot_credential_is_readable_over_the_socket`; un-ignored by Phase 51 Plan 11 (CR-06
/// fix), with NO edit to any file under `crates/ironhermes-kanban/src/` — the fix landed
/// entirely in the shared `mint_worker_credential` seam this dispatcher already called
/// through. When it passes, it proves the shipped kanban dispatcher path (the real
/// construction shape production uses: one long-lived host opened at
/// `DispatcherContext::new`, minting on every subsequent tick) hands a spawned worker a
/// credential the vault genuinely recognizes.
///
/// **Why it used to fail, and why it now passes with no dispatcher-side change (Phase 51
/// Plan 11):** `rusty_vault`'s `TokenStore::new()` performs an unsynchronized
/// check-then-write on a process-scoped token salt on every `Core` boot, so two `Core`s
/// that disagree on that salt disagree on every token lookup between them. The failing
/// configuration this test built was an in-process mint (`run_dispatch_tick`'s call to
/// `ironhermes_core::profile_credentials::mint_worker_credential`) against a SECOND
/// `Core` opened via `mint_profile_token_via_config` — a different `Core` than the
/// long-lived host's own. `mint_worker_credential`'s implementation (in
/// `ironhermes-core`, not this crate) now calls `mint_profile_token_for_host`, which
/// mints through the host's own `Core` — the exact `Core` this test's long-lived
/// `ctx.profile_credentials` already holds. This dispatcher's per-tick mint therefore
/// now goes through the long-lived host's own `Core`, so the salt disagreement that
/// produced `TokenInvalid` cannot arise: one `Core`, one `TokenStore`, one salt. This
/// file stays separate from the core-level repro deliberately — it exists to separate
/// crate-wiring faults from upstream faults, and it now demonstrates the fix was
/// inherited through the shared seam rather than needing its own patch. See
/// `51-11-SUMMARY.md` for the fix record; `51-10-SUMMARY.md` for the original
/// investigation arc this superseded.
#[tokio::test(flavor = "multi_thread")]
async fn shipped_dispatcher_credential_is_readable_after_a_long_lived_host() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let vault_dir = tmp.path().join("vault");
    init_vault_with_seeded_secret(&vault_dir).await;
    write_vault_config(&home, &vault_dir);
    let _home_env = ScopedEnv::set("IRONHERMES_HOME", &home.to_string_lossy());

    let dump_path = tmp.path().join("worker_env_dump.txt");
    let script_path = tmp.path().join("fake_worker.sh");
    write_env_dump_script(&script_path, &dump_path);
    let _worker_bin_env = ScopedEnv::set("IRONHERMES_WORKER_BIN", &script_path.to_string_lossy());

    let store_dir = TempDir::new().unwrap();
    let mut store = KanbanStore::open(store_dir.path().join("kanban.db")).expect("open kanban store");
    store
        .create_task("vault long-lived host test", SLUG, CreateTaskOptions::default())
        .unwrap();
    let store_arc = std::sync::Arc::new(TokioMutex::new(store));

    // The REAL default `DispatcherContext::new` — real `host_profile_credentials_from_disk`
    // (reads the config.yaml just written, vault.enabled=true), opening the host HERE,
    // BEFORE any mint. This handle is held alive for the REST of this test — genuinely
    // long-lived, exactly like `runner.rs`'s single `Arc<DispatcherContext>` held across
    // every `run_dispatch_loop` tick. Real `spawn_fn` (`worker_spawn::spawn_worker_for_board`,
    // no injection).
    let mut ctx = DispatcherContext::new(store_arc, KanbanConfig::default());
    ctx = ctx.with_gate_fn(always_allow_from_vault());
    assert!(
        ctx.profile_credentials.is_some(),
        "host_profile_credentials_from_disk should have hosted a real endpoint from the \
         vault.enabled=true config.yaml this test just wrote — if this is None, the config \
         write or IRONHERMES_HOME scoping above is broken, not the thing under test"
    );

    struct NoopAudit;
    impl ironhermes_core::profile_credentials::ProfileTokenAudit for NoopAudit {
        fn record_mint(&self, _slug: &str, _accessor: &str, _ttl: Duration) -> anyhow::Result<()> {
            Ok(())
        }
    }
    ctx = ctx.with_token_audit(std::sync::Arc::new(NoopAudit));

    // The mint happens INSIDE this call, against the host opened above — which has been
    // alive since before this line ran, exactly as it would be on tick N of a real,
    // long-running dispatcher.
    run_dispatch_tick(&ctx).await.expect("run_dispatch_tick");

    let dumped_env = wait_for_dump(&dump_path).await;

    let token_str = extract_env_var(&dumped_env, "IRONHERMES_KANBAN_VAULT_TOKEN").unwrap_or_else(|| {
        panic!(
            "the real spawned child process env is missing IRONHERMES_KANBAN_VAULT_TOKEN. \
             Dumped env:\n{dumped_env}"
        )
    });
    let socket_str = extract_env_var(&dumped_env, "IRONHERMES_KANBAN_VAULT_SOCKET").unwrap_or_else(|| {
        panic!(
            "the real spawned child process env is missing IRONHERMES_KANBAN_VAULT_SOCKET. \
             Dumped env:\n{dumped_env}"
        )
    });

    let token = SecretString::from(token_str.to_string());
    let socket_path = Path::new(socket_str);

    // THE ANSWER. Read the credential back over the real socket using EXACTLY what the
    // real spawned child process received — the same call `bootstrap_worker_credential`
    // makes in production.
    let read_back = ironhermes_vault::read_profile_credential(socket_path, &token, SLUG, PROVIDER)
        .await
        .expect(
            "reading the minted credential back over the real socket must succeed — if this \
             fails with TokenInvalid, the shipped kanban dispatcher path (long-lived host, \
             per-tick mint) reproduces Plan 10 Task 2's core-level finding: a token minted \
             after the host opened is not recognized by that already-open host",
        );

    assert_eq!(
        read_back.expose_secret(),
        SECRET_VALUE,
        "the credential read back over the socket must equal the seeded secret"
    );
}
