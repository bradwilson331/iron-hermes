//! Pre-spawn dispatch gate — **dispatcher-loop** coverage (Phase 47.4 GAP-1,
//! UAT inline fix).
//!
//! Why this file exists, when `ironhermes-cli/tests/dispatch_profile_gate.rs`
//! already covers the predicate:
//!
//! Plan 10 wired the gate into `cmd_dispatch` — the one-shot
//! `ironhermes kanban dispatch` CLI command — and tested it there. Its tests
//! were green and its predicate was correct. But the dispatcher that actually
//! spawns workers in production is `run_dispatch_loop`, spawned by the
//! **gateway** (`ironhermes-gateway/src/runner.rs`), which calls
//! `run_dispatch_tick` on an interval and never goes near the CLI command.
//!
//! The 47.4 UAT caught the consequence: a `bdev01` task spawned a worker that
//! died, the task stayed `status='ready'`, and every subsequent tick merely
//! logged `respawn_guarded / blocker_auth` — a guard that by construction can
//! only fire *after* a spawn has already failed. So these tests assert the
//! gate through `run_dispatch_tick`, not through the CLI.
//!
//! Two properties are load-bearing here and neither is checked by the CLI
//! suite:
//!   1. the refused task NEVER reaches the spawn function, and
//!   2. it lands in a terminal `blocked` state rather than staying `ready`
//!      and re-guarding forever.
//!
//! These tests deliberately exercise the REAL default gate — they do not
//! inject `with_gate_fn`. A stubbed decision here would only prove that a
//! stub was honoured, which is precisely the self-verifying-test failure mode
//! this project has shipped before.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use ironhermes_core::config::{Config, ProviderConfig};
use ironhermes_kanban::dispatcher::DispatcherContext;
use ironhermes_kanban::store::CreateTaskOptions;
use ironhermes_kanban::{KanbanConfig, KanbanStore, run_dispatch_tick};
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

/// RAII guard that sets an env var and restores the previous value on drop.
/// Sandboxes `IRONHERMES_HOME` so the real `evaluate_profile_dispatch` reads
/// this test's fixture profiles instead of the developer's own
/// `~/.ironhermes`. Requires `--test-threads=1` (this crate's suite already
/// runs that way) because process env is global.
struct ScopedEnv {
    key: String,
    prev: Option<String>,
}

impl ScopedEnv {
    fn set(key: &str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // Safety: test-only, single-threaded test runner.
        unsafe { std::env::set_var(key, value) };
        Self {
            key: key.to_string(),
            prev,
        }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        // Safety: see `set` above.
        match &self.prev {
            Some(v) => unsafe { std::env::set_var(&self.key, v) },
            None => unsafe { std::env::remove_var(&self.key) },
        }
    }
}

fn write_profile(root: &Path, name: &str, config: &Config, env_contents: Option<&str>) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).expect("mkdir profile dir");
    config
        .save_to(&dir.join("config.yaml"))
        .expect("save_to config.yaml");
    if let Some(contents) = env_contents {
        fs::write(dir.join(".env"), contents).expect("write .env");
    }
}

/// The exact UAT shape: main provider `moonshot`, and a `.env` carrying keys
/// for OTHER providers but not `MOONSHOT_API_KEY`.
fn bdev01_config() -> Config {
    let mut config = Config::default();
    config.model.provider = "moonshot".to_string();
    config.model.default = "k3-256k".to_string();
    config.providers.insert(
        "moonshot".to_string(),
        ProviderConfig {
            api_key_env: Some("MOONSHOT_API_KEY".to_string()),
            ..Default::default()
        },
    );
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            ..Default::default()
        },
    );
    config
}

/// A legitimately keyless local endpoint — the carve-out that must still
/// dispatch. Chosen over a key-bearing provider for the positive case so the
/// assertion cannot depend on the developer's ambient process env.
fn llama_config() -> Config {
    let mut config = Config::default();
    config.model.provider = "llama".to_string();
    config.model.default = "llama3.1:70b".to_string();
    config.providers.insert(
        "llama".to_string(),
        ProviderConfig {
            api_key_env: None,
            api_key: None,
            ..Default::default()
        },
    );
    config
}

/// Spawn fn that records every task id it was asked to spawn.
fn recording_spawn_fn(
    seen: Arc<std::sync::Mutex<Vec<String>>>,
) -> ironhermes_kanban::dispatcher::SpawnFn {
    Arc::new(
        move |task: ironhermes_kanban::types::Task,
              _run: ironhermes_kanban::types::TaskRun,
              _ws: String,
              _board_slug: String|
              -> std::pin::Pin<
            Box<dyn std::future::Future<Output = ironhermes_kanban::error::Result<u32>> + Send>,
        > {
            let seen = Arc::clone(&seen);
            Box::pin(async move {
                seen.lock().unwrap().push(task.id.clone());
                Ok(4242)
            })
        },
    )
}

/// THE regression: the `bdev01` shape must be refused by the dispatcher loop
/// itself — blocked before any spawn, and left terminal rather than `ready`.
#[tokio::test]
async fn bdev01_shape_is_blocked_by_the_dispatch_loop_before_any_spawn() {
    // NOTE: this test no longer asserts MOONSHOT_API_KEY is absent from the
    // process env. That precondition was how the FIRST version of this file
    // missed the real bug: it controlled away the exact hostile variable that
    // breaks production. The gate now resolves strictly against the profile's
    // own .env, so ambient process env is irrelevant here — and
    // `gate_ignores_process_env_because_the_worker_is_scrubbed` below proves it
    // by deliberately SETTING the key in the process env.

    let home = TempDir::new().unwrap();
    let _guard = ScopedEnv::set("IRONHERMES_HOME", home.path().to_str().unwrap());
    let profiles = home.path().join("profiles");
    write_profile(
        &profiles,
        "bdev01",
        &bdev01_config(),
        Some(
            "OPENROUTER_API_KEY=sk-fixture-openrouter-8f3a2c\n\
             ANTHROPIC_API_KEY=sk-ant-fixture-4d9e1b\n",
        ),
    );

    let db = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(
        KanbanStore::new(db.path().join("kanban.db")).expect("open store"),
    ));

    let task_id = {
        let mut store = store_arc.lock().await;
        store
            .create_task("undispatchable task", "bdev01", CreateTaskOptions::default())
            .unwrap()
            .id
    };

    let spawned = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    // NOTE: real default gate — no `with_gate_fn`.
    let ctx = DispatcherContext::with_spawn_fn(
        store_arc.clone(),
        KanbanConfig::default(),
        recording_spawn_fn(Arc::clone(&spawned)),
    );

    run_dispatch_tick(&ctx).await.expect("tick failed");

    assert!(
        spawned.lock().unwrap().is_empty(),
        "gate must refuse BEFORE spawn — spawn fn was called for {:?}",
        spawned.lock().unwrap()
    );

    let store = store_arc.lock().await;
    let task = store.get_task(&task_id).unwrap();
    assert_eq!(
        task.status, "blocked",
        "undispatchable task must end terminal-blocked, not stay 'ready' and \
         re-guard every tick (the 47.4 UAT symptom)"
    );
}

/// Guard against the gate degenerating into a blanket refusal: a profile that
/// legitimately resolves must still spawn through the same loop. Without this,
/// the test above would pass even if the gate blocked everything.
#[tokio::test]
async fn dispatchable_profile_still_spawns_through_the_loop() {
    let home = TempDir::new().unwrap();
    let _guard = ScopedEnv::set("IRONHERMES_HOME", home.path().to_str().unwrap());
    let profiles = home.path().join("profiles");
    write_profile(&profiles, "localdev", &llama_config(), None);

    let db = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(
        KanbanStore::new(db.path().join("kanban.db")).expect("open store"),
    ));

    let task_id = {
        let mut store = store_arc.lock().await;
        store
            .create_task("dispatchable task", "localdev", CreateTaskOptions::default())
            .unwrap()
            .id
    };

    let spawned = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let ctx = DispatcherContext::with_spawn_fn(
        store_arc.clone(),
        KanbanConfig::default(),
        recording_spawn_fn(Arc::clone(&spawned)),
    );

    run_dispatch_tick(&ctx).await.expect("tick failed");

    assert_eq!(
        spawned.lock().unwrap().as_slice(),
        std::slice::from_ref(&task_id),
        "a keyless-carve-out profile must still dispatch through the loop"
    );

    let store = store_arc.lock().await;
    let task = store.get_task(&task_id).unwrap();
    assert_ne!(
        task.status, "blocked",
        "a legitimately dispatchable profile must not be gate-blocked"
    );
}

// ---------------------------------------------------------------------------
// respawn_guard `blocker_auth` backoff (Phase 47.4 UAT follow-up)
//
// The guard used to return unconditionally on any auth-ish error in the newest
// closed run, with no time check — so a task whose profile was later repaired
// could never dispatch again. The 47.4 UAT hit exactly this: `bdev01` was fixed
// by adding MOONSHOT_API_KEY, the dispatch gate correctly began allowing it,
// and the guard still held the task on the strength of an hours-old 401.
// ---------------------------------------------------------------------------

use ironhermes_kanban::dispatcher::respawn_guard_reason;
use rusqlite::params;

/// Seed a closed, errored run so `respawn_guard_reason` has something to read.
fn seed_failed_run(store: &mut KanbanStore, task_id: &str, ended_at: f64, error: &str) {
    let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
    store
        .conn
        .execute(
            "INSERT INTO task_runs (id, task_id, claim_lock, started_at, ended_at, outcome, error) \
             VALUES (?1, ?2, 'lock:1:uuid', ?3, ?4, 'spawn_failed', ?5)",
            params![run_id, task_id, ended_at - 1.0, ended_at, error],
        )
        .unwrap();
}

/// The exact 47.4 UAT error text.
const UAT_401: &str = "Streaming chat completion failed (401 Unauthorized): \
                       {\"error\":\"Authentication failed\"}";

#[tokio::test]
async fn blocker_auth_still_guards_inside_the_backoff_window() {
    let db = TempDir::new().unwrap();
    let mut store = KanbanStore::new(db.path().join("kanban.db")).unwrap();
    let task = store
        .create_task("recent auth failure", "alice", CreateTaskOptions::default())
        .unwrap();
    let now = 1_000_000.0;
    // Failed 10 minutes ago; default backoff is 3600s.
    seed_failed_run(&mut store, &task.id, now - 600.0, UAT_401);

    let task = store.get_task(&task.id).unwrap();
    let reason = respawn_guard_reason(&store, &task, now, &KanbanConfig::default()).unwrap();
    assert_eq!(
        reason,
        Some("blocker_auth"),
        "a fresh auth failure must still be guarded"
    );
}

#[tokio::test]
async fn blocker_auth_expires_once_the_backoff_window_passes() {
    let db = TempDir::new().unwrap();
    let mut store = KanbanStore::new(db.path().join("kanban.db")).unwrap();
    let task = store
        .create_task("repaired profile", "alice", CreateTaskOptions::default())
        .unwrap();
    let now = 1_000_000.0;
    // Failed 2 hours ago; default backoff is 3600s -> cooled off.
    seed_failed_run(&mut store, &task.id, now - 7200.0, UAT_401);

    let task = store.get_task(&task.id).unwrap();
    let reason = respawn_guard_reason(&store, &task, now, &KanbanConfig::default()).unwrap();
    assert_eq!(
        reason, None,
        "after the backoff window the task must be dispatchable again — \
         otherwise repairing the profile can never unblock it (the 47.4 UAT trap)"
    );
}

#[tokio::test]
async fn blocker_auth_backoff_is_configurable_and_zero_means_forever() {
    let db = TempDir::new().unwrap();
    let mut store = KanbanStore::new(db.path().join("kanban.db")).unwrap();
    let task = store
        .create_task("old auth failure", "alice", CreateTaskOptions::default())
        .unwrap();
    let now = 1_000_000.0;
    seed_failed_run(&mut store, &task.id, now - 7200.0, UAT_401);
    let task = store.get_task(&task.id).unwrap();

    // A longer window keeps guarding the same run...
    let long = KanbanConfig {
        respawn_auth_backoff_seconds: 4 * 3600,
        ..KanbanConfig::default()
    };
    assert_eq!(
        respawn_guard_reason(&store, &task, now, &long).unwrap(),
        Some("blocker_auth"),
        "a 4h window must still guard a 2h-old failure"
    );

    // ...and 0 restores the legacy unbounded behaviour.
    let forever = KanbanConfig {
        respawn_auth_backoff_seconds: 0,
        ..KanbanConfig::default()
    };
    assert_eq!(
        respawn_guard_reason(&store, &task, now, &forever).unwrap(),
        Some("blocker_auth"),
        "0 must mean guard forever (legacy behaviour, opt-in)"
    );
}

/// THE third-root-cause regression (47.4 UAT round 3).
///
/// Production shape: the gateway has the key in its own environment (it loads
/// the ROOT ~/.ironhermes/.env), the target profile's .env does NOT, and the
/// worker is spawned with .env_clear() so it only ever sees the profile's .env.
/// A gate that consults the process env answers the wrong question and
/// false-ALLOWs — the worker then dies 401 ~1s after spawn.
///
/// This test deliberately SETS the key in the process environment. It must
/// still refuse.
#[tokio::test]
async fn gate_ignores_process_env_because_the_worker_is_scrubbed() {
    let home = TempDir::new().unwrap();
    let _home_guard = ScopedEnv::set("IRONHERMES_HOME", home.path().to_str().unwrap());
    // The hostile condition, made explicit rather than assumed away.
    let _key_guard = ScopedEnv::set("MOONSHOT_API_KEY", "sk-root-env-key-that-the-worker-never-sees");

    let profiles = home.path().join("profiles");
    // Profile carries a DIFFERENT provider's key, never MOONSHOT_API_KEY.
    write_profile(
        &profiles,
        "uatbad",
        &bdev01_config(),
        Some("OPENROUTER_API_KEY=sk-fixture-openrouter-8f3a2c\n"),
    );

    let db = TempDir::new().unwrap();
    let store_arc = Arc::new(TokioMutex::new(
        KanbanStore::new(db.path().join("kanban.db")).expect("open store"),
    ));
    let task_id = {
        let mut store = store_arc.lock().await;
        store
            .create_task("root has key, profile does not", "uatbad", CreateTaskOptions::default())
            .unwrap()
            .id
    };

    let spawned = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let ctx = DispatcherContext::with_spawn_fn(
        store_arc.clone(),
        KanbanConfig::default(),
        recording_spawn_fn(Arc::clone(&spawned)),
    );

    run_dispatch_tick(&ctx).await.expect("tick failed");

    assert!(
        spawned.lock().unwrap().is_empty(),
        "gate must ignore the process env — the spawned worker is env_clear()'d and \
         would NOT have MOONSHOT_API_KEY, so allowing this is a guaranteed 401"
    );
    let store = store_arc.lock().await;
    assert_eq!(
        store.get_task(&task_id).unwrap().status,
        "blocked",
        "profile whose own .env lacks the provider key must be blocked even when \
         the dispatching process happens to hold that key"
    );
}
