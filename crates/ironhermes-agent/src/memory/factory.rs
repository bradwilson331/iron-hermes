use std::sync::{Arc, Mutex};

use ironhermes_core::MemoryProvider;
use ironhermes_core::constants::get_hermes_home;
use ironhermes_core::memory_store::MemoryStore;

/// Build a memory provider from config. Returns `Arc<std::sync::Mutex<...>>`
/// for direct use with `MemoryTool`, `MemoryManager` (Plan 20-02) and the
/// existing gateway/agent consumers.
///
/// Phase 20 (D-10, D-16, D-17): async factory that
/// 1. constructs the provider via `Provider::new(db_path)`,
/// 2. calls `provider.initialize(session_id, hermes_home, provider_config).await`,
/// 3. calls `provider.load_from_disk()` unconditionally (Fix 1 for the pending
///    todo "gateway memory does not persist across restart"),
/// 4. if `is_available()` returns false, logs `tracing::warn!` with
///    `unavailable_reason()` and falls back to the file-based provider.
///
/// Feature-gated per D-16 — external providers require their respective
/// cargo feature. PROJECT.md:52 — compile-time plugin selection only.
///
/// **Mutex flavor note (Plan 20-01 deviation — documented):** The plan's
/// open question #2 specified `tokio::sync::Mutex` for the factory so that
/// guards could cross `.await`. However, every existing downstream consumer
/// (memory_tool, prompt_builder, gateway runner, delegate_task, registry,
/// cronjob) holds an `Arc<std::sync::Mutex<dyn MemoryProvider + Send>>`
/// and calls `.lock().unwrap()` synchronously. Migrating the factory alone
/// would force a workspace-wide type-level migration that the plan itself
/// defers to Plan 20-02 ("The previous Arc<std::sync::Mutex<...>> usage at
/// memory_tool.rs:10 and memory_tool.rs:212 will migrate to
/// tokio::sync::Mutex in Plan 20-02"). To keep Plan 20-01 atomic and the
/// workspace compiling, the factory stays on `std::sync::Mutex` here —
/// Plan 20-02 will migrate the factory return type, `MemoryManager`, the
/// tool, and every consumer in a single atomic wave. The default async
/// hooks (queue_prefetch, on_pre_compress, on_memory_write) that motivate
/// tokio::sync::Mutex are all defaulted no-ops in Plan 20-01 with no live
/// callers, so the await-under-guard hazard does not exist yet.
pub async fn build_memory_provider(
    config: &ironhermes_core::config::MemoryConfig,
) -> anyhow::Result<Arc<Mutex<dyn MemoryProvider + Send>>> {
    let hermes_home = get_hermes_home();
    let provider_config = serde_json::Value::Null; // Plan 20-03 will load
                                                   // `$HERMES_HOME/<name>.json`
                                                   // here; Phase 20-01 passes Null.

    let provider: Arc<Mutex<dyn MemoryProvider + Send>> = match config.provider.as_str() {
        "file" => build_file_provider(&hermes_home).await?,
        #[cfg(feature = "memory-sqlite")]
        "sqlite" => {
            let db_path = hermes_home.join("memory.db");
            let mut p = memory_sqlite::SqliteMemoryProvider::new(&db_path)?;
            p.initialize("factory-boot", &hermes_home, &provider_config).await?;
            p.load_from_disk()?;
            if !p.is_available() {
                let reason = p.unavailable_reason().unwrap_or_else(|| "unknown".into());
                tracing::warn!(
                    provider = "sqlite",
                    reason = %reason,
                    "memory provider reported is_available=false; falling back to file provider"
                );
                return build_file_provider(&hermes_home).await;
            }
            Arc::new(Mutex::new(p))
        }
        #[cfg(not(feature = "memory-sqlite"))]
        "sqlite" => {
            anyhow::bail!(
                "Memory provider 'sqlite' requires the 'memory-sqlite' feature. \
                 Rebuild with: cargo build --features memory-sqlite"
            );
        }
        #[cfg(feature = "memory-duckdb")]
        "duckdb" => {
            let db_path = hermes_home.join("memory_duckdb.db");
            let mut p = memory_duckdb::DuckDbMemoryProvider::new(&db_path)?;
            p.initialize("factory-boot", &hermes_home, &provider_config).await?;
            p.load_from_disk()?;
            if !p.is_available() {
                let reason = p.unavailable_reason().unwrap_or_else(|| "unknown".into());
                tracing::warn!(
                    provider = "duckdb",
                    reason = %reason,
                    "memory provider reported is_available=false; falling back to file provider"
                );
                return build_file_provider(&hermes_home).await;
            }
            Arc::new(Mutex::new(p))
        }
        #[cfg(not(feature = "memory-duckdb"))]
        "duckdb" => {
            anyhow::bail!(
                "Memory provider 'duckdb' requires the 'memory-duckdb' feature. \
                 Rebuild with: cargo build --features memory-duckdb"
            );
        }
        #[cfg(feature = "memory-grafeo")]
        "grafeo" => {
            // Grafeo convention: persistent stores use the `.grafeo` file/dir
            // extension (see memory-grafeo own `test_persistence_survives_reopen`).
            // Without the suffix the DB opens successfully but does not flush
            // new nodes to disk between process lifetimes.
            let db_path = hermes_home.join("memory_graph.grafeo");
            let mut p = memory_grafeo::GrafeoMemoryProvider::new(&db_path)?;
            p.initialize("factory-boot", &hermes_home, &provider_config).await?;
            p.load_from_disk()?;
            if !p.is_available() {
                let reason = p.unavailable_reason().unwrap_or_else(|| "unknown".into());
                tracing::warn!(
                    provider = "grafeo",
                    reason = %reason,
                    "memory provider reported is_available=false; falling back to file provider"
                );
                return build_file_provider(&hermes_home).await;
            }
            Arc::new(Mutex::new(p))
        }
        #[cfg(not(feature = "memory-grafeo"))]
        "grafeo" => {
            anyhow::bail!(
                "Memory provider 'grafeo' requires the 'memory-grafeo' feature. \
                 Rebuild with: cargo build --features memory-grafeo"
            );
        }
        other => {
            anyhow::bail!(
                "Unknown memory provider '{}'. Available providers: file{}{}{}",
                other,
                if cfg!(feature = "memory-sqlite") { ", sqlite" } else { "" },
                if cfg!(feature = "memory-grafeo") { ", grafeo" } else { "" },
                if cfg!(feature = "memory-duckdb") { ", duckdb" } else { "" }
            );
        }
    };

    Ok(provider)
}

async fn build_file_provider(
    hermes_home: &std::path::Path,
) -> anyhow::Result<Arc<Mutex<dyn MemoryProvider + Send>>> {
    let memory_dir = hermes_home.join("memories");
    let mut store = MemoryStore::new(memory_dir);
    // initialize is a no-op for file provider but we call it for symmetry.
    store
        .initialize("factory-boot", hermes_home, &serde_json::Value::Null)
        .await?;
    if let Err(e) = store.load_from_disk() {
        tracing::warn!("Failed to load memory from disk: {}", e);
    }
    Ok(Arc::new(Mutex::new(store)))
}

/// Build a `MemoryManager` wrapping a primary provider plus an optional
/// mirror provider selected via `MemoryConfig.mirror_provider`.
///
/// The returned handle is `Arc<tokio::sync::Mutex<MemoryManager>>` — the
/// canonical Plan-20-02 sharing shape. All consumers (MemoryTool,
/// prompt_builder, context_engine, memory_flush_handler, agent_loop,
/// gateway runner/handler, CLI) hold this handle and call `.lock().await`.
///
/// Mirror recursion guard: when a mirror provider is requested, its own
/// `mirror_provider` field is cleared before constructing the secondary
/// provider so a config like `{provider: sqlite, mirror_provider: Some("grafeo")}`
/// does not recursively build more mirrors.
///
/// Implementation note: `build_memory_provider` returns
/// `Arc<std::sync::Mutex<dyn MemoryProvider + Send>>` (legacy shape held by
/// the existing gateway/CLI consumers until Plan 20-02's Task 02 migrates
/// them). We CANNOT convert that to `tokio::sync::Mutex` because moving a
/// `dyn Trait` out of `std::sync::Mutex::into_inner` requires `Sized`, which
/// trait objects do not satisfy. Instead this function rebuilds the inner
/// provider(s) directly into `tokio::sync::Mutex` wrappers — duplicating the
/// provider-kind match once. When Task 02 migrates `build_memory_provider`
/// to `tokio::sync::Mutex`, this helper collapses back to a single path.
pub async fn build_memory_manager(
    config: &ironhermes_core::config::MemoryConfig,
) -> anyhow::Result<Arc<tokio::sync::Mutex<crate::memory::MemoryManager>>> {
    let primary = build_tokio_provider(config).await?;

    let mirror = if let Some(name) = &config.mirror_provider {
        let mut mirror_cfg = config.clone();
        mirror_cfg.provider = name.clone();
        mirror_cfg.mirror_provider = None; // prevent recursion
        Some(build_tokio_provider(&mirror_cfg).await?)
    } else {
        None
    };

    let mgr = crate::memory::MemoryManager::new(primary, mirror).await?;
    Ok(Arc::new(tokio::sync::Mutex::new(mgr)))
}

/// Build a provider wrapped in `Arc<tokio::sync::Mutex<...>>` directly.
/// Mirrors the provider-kind dispatch of `build_memory_provider` but emits
/// the Plan-20-02 tokio-mutex shape that `MemoryManager` requires.
async fn build_tokio_provider(
    config: &ironhermes_core::config::MemoryConfig,
) -> anyhow::Result<crate::memory::SharedProvider> {
    let hermes_home = get_hermes_home();
    let provider_config = serde_json::Value::Null;

    let provider: crate::memory::SharedProvider = match config.provider.as_str() {
        "file" => build_tokio_file_provider(&hermes_home).await?,
        #[cfg(feature = "memory-sqlite")]
        "sqlite" => {
            let db_path = hermes_home.join("memory.db");
            let mut p = memory_sqlite::SqliteMemoryProvider::new(&db_path)?;
            p.initialize("factory-boot", &hermes_home, &provider_config).await?;
            p.load_from_disk()?;
            if !p.is_available() {
                let reason = p.unavailable_reason().unwrap_or_else(|| "unknown".into());
                tracing::warn!(
                    provider = "sqlite",
                    reason = %reason,
                    "memory provider reported is_available=false; falling back to file provider"
                );
                return build_tokio_file_provider(&hermes_home).await;
            }
            Arc::new(tokio::sync::Mutex::new(p))
        }
        #[cfg(not(feature = "memory-sqlite"))]
        "sqlite" => {
            anyhow::bail!(
                "Memory provider 'sqlite' requires the 'memory-sqlite' feature. \
                 Rebuild with: cargo build --features memory-sqlite"
            );
        }
        #[cfg(feature = "memory-duckdb")]
        "duckdb" => {
            let db_path = hermes_home.join("memory_duckdb.db");
            let mut p = memory_duckdb::DuckDbMemoryProvider::new(&db_path)?;
            p.initialize("factory-boot", &hermes_home, &provider_config).await?;
            p.load_from_disk()?;
            if !p.is_available() {
                let reason = p.unavailable_reason().unwrap_or_else(|| "unknown".into());
                tracing::warn!(
                    provider = "duckdb",
                    reason = %reason,
                    "memory provider reported is_available=false; falling back to file provider"
                );
                return build_tokio_file_provider(&hermes_home).await;
            }
            Arc::new(tokio::sync::Mutex::new(p))
        }
        #[cfg(not(feature = "memory-duckdb"))]
        "duckdb" => {
            anyhow::bail!(
                "Memory provider 'duckdb' requires the 'memory-duckdb' feature. \
                 Rebuild with: cargo build --features memory-duckdb"
            );
        }
        #[cfg(feature = "memory-grafeo")]
        "grafeo" => {
            let db_path = hermes_home.join("memory_graph.grafeo");
            let mut p = memory_grafeo::GrafeoMemoryProvider::new(&db_path)?;
            p.initialize("factory-boot", &hermes_home, &provider_config).await?;
            p.load_from_disk()?;
            if !p.is_available() {
                let reason = p.unavailable_reason().unwrap_or_else(|| "unknown".into());
                tracing::warn!(
                    provider = "grafeo",
                    reason = %reason,
                    "memory provider reported is_available=false; falling back to file provider"
                );
                return build_tokio_file_provider(&hermes_home).await;
            }
            Arc::new(tokio::sync::Mutex::new(p))
        }
        #[cfg(not(feature = "memory-grafeo"))]
        "grafeo" => {
            anyhow::bail!(
                "Memory provider 'grafeo' requires the 'memory-grafeo' feature. \
                 Rebuild with: cargo build --features memory-grafeo"
            );
        }
        other => {
            anyhow::bail!(
                "Unknown memory provider '{}'. Available providers: file{}{}{}",
                other,
                if cfg!(feature = "memory-sqlite") { ", sqlite" } else { "" },
                if cfg!(feature = "memory-grafeo") { ", grafeo" } else { "" },
                if cfg!(feature = "memory-duckdb") { ", duckdb" } else { "" }
            );
        }
    };

    Ok(provider)
}

async fn build_tokio_file_provider(
    hermes_home: &std::path::Path,
) -> anyhow::Result<crate::memory::SharedProvider> {
    let memory_dir = hermes_home.join("memories");
    let mut store = MemoryStore::new(memory_dir);
    store
        .initialize("factory-boot", hermes_home, &serde_json::Value::Null)
        .await?;
    if let Err(e) = store.load_from_disk() {
        tracing::warn!("Failed to load memory from disk: {}", e);
    }
    Ok(Arc::new(tokio::sync::Mutex::new(store)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::config::MemoryConfig;
    #[cfg(any(feature = "memory-sqlite", feature = "memory-duckdb", feature = "memory-grafeo"))]
    use ironhermes_core::memory_store::MemoryTarget;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Serializes tests that mutate `IRONHERMES_HOME` via `std::env::set_var`.
    ///
    /// `cargo test` runs tests in parallel by default. `std::env::set_var` is
    /// process-global, so two env-mutating tests racing can cause a
    /// `build_memory_provider` call in test A to read test B's tempdir and
    /// observe/write foreign state (the root cause of the plan-20-01 round-trip
    /// test flakes — see 20-01-SUMMARY.md Deviations / test-isolation fix).
    ///
    /// Every test in this module that calls `set_var("IRONHERMES_HOME", ...)`
    /// MUST hold `env_lock()` for its entire duration. The returned
    /// `MutexGuard` outlives the tempdir and is dropped at test-function
    /// return, which is sufficient because `build_memory_provider` is awaited
    /// to completion before the guard is dropped.
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner()) // poison-tolerant for test-panic recovery
    }

    fn cfg(provider: &str) -> MemoryConfig {
        MemoryConfig {
            provider: provider.to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn file_provider_returns_ok() {
        // Also exercises the load_from_disk path — a missing memories
        // directory must not cause a bail (warn-on-error behavior).
        let _guard = env_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        // SAFETY: test-only env mutation; serialized by `env_lock` so no other
        // test in this module can race this thread's view of IRONHERMES_HOME.
        // NOTE: `get_hermes_home()` reads `IRONHERMES_HOME` (not `HERMES_HOME`).
        // Using the wrong name falls through to `~/.ironhermes`, which is
        // the user's real directory — tests must never write there.
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
        let result = build_memory_provider(&cfg("file")).await;
        assert!(result.is_ok(), "file provider should build, got {:?}", result.err());
    }

    #[tokio::test]
    async fn unknown_provider_returns_err_with_message() {
        // Does not mutate env, but still takes the lock because this test
        // reads IRONHERMES_HOME indirectly via `build_memory_provider` and a
        // concurrent env-mutating test could point it at a tempdir that is
        // deleted mid-call.
        let _guard = env_lock();
        let result = build_memory_provider(&cfg("totally-unknown")).await;
        assert!(result.is_err(), "unknown provider must error");
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("Unknown memory provider 'totally-unknown'"),
            "got: {msg}",
        );
        assert!(msg.contains("file"), "available providers must include 'file', got: {msg}");
    }

    #[cfg(feature = "memory-sqlite")]
    #[tokio::test]
    async fn sqlite_provider_with_feature_returns_ok() {
        // UAT Test 2 regression guard — this test must pass under
        // `cargo test -p ironhermes-agent --features memory-sqlite`.
        let _guard = env_lock();
        let _tmp = tempfile::TempDir::new().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", _tmp.path()); }
        let result = build_memory_provider(&cfg("sqlite")).await;
        assert!(
            result.is_ok(),
            "sqlite provider must build when memory-sqlite feature is enabled, got {:?}",
            result.err(),
        );
    }

    #[cfg(not(feature = "memory-sqlite"))]
    #[tokio::test]
    async fn sqlite_provider_without_feature_returns_err_naming_feature() {
        let result = build_memory_provider(&cfg("sqlite")).await;
        assert!(result.is_err(), "sqlite provider without feature must error");
        let msg = result.err().unwrap().to_string();
        assert!(msg.contains("memory-sqlite"), "error must name the feature, got: {msg}");
        assert!(
            msg.contains("cargo build --features memory-sqlite"),
            "error must include the rebuild instruction, got: {msg}",
        );
    }

    // =========================================================================
    // D-24 regression: pending todo Fix 1 — factory must call load_from_disk
    // for external providers so gateway/chat memory persists across restart.
    // =========================================================================

    #[cfg(feature = "memory-sqlite")]
    #[tokio::test]
    async fn sqlite_round_trip_via_factory() {
        // See `env_lock` docs for why this is held across the whole test.
        // We re-set IRONHERMES_HOME before every build_memory_provider call
        // because OTHER test modules in this binary (notably `prompt_builder`)
        // also mutate IRONHERMES_HOME and DO NOT take `env_lock`. Re-asserting
        // the var immediately before each factory call guarantees the provider
        // opens the correct tempdir even if a racing test clobbered it between
        // our two phases.
        let _guard = env_lock();
        let tmp = tempfile::TempDir::new().unwrap();

        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
        {
            let p = build_memory_provider(&cfg("sqlite")).await.unwrap();
            let mut guard = p.lock().unwrap();
            guard
                .add(MemoryTarget::Memory, "integration-fact-XYZ")
                .expect("add should succeed");
        } // drop provider

        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
        let p2 = build_memory_provider(&cfg("sqlite")).await.unwrap();
        let guard2 = p2.lock().unwrap();
        let block = guard2
            .format_for_system_prompt(MemoryTarget::Memory)
            .expect("memory block should be populated after reload");
        assert!(
            block.contains("integration-fact-XYZ"),
            "factory reload lost the entry; block was: {block}"
        );
    }

    #[cfg(feature = "memory-duckdb")]
    #[tokio::test]
    async fn duckdb_round_trip_via_factory() {
        // See sqlite_round_trip_via_factory for the double-set rationale.
        let _guard = env_lock();
        let tmp = tempfile::TempDir::new().unwrap();

        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
        {
            let p = build_memory_provider(&cfg("duckdb")).await.unwrap();
            let mut guard = p.lock().unwrap();
            guard
                .add(MemoryTarget::Memory, "duckdb-fact-XYZ")
                .expect("add should succeed");
        }

        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
        let p2 = build_memory_provider(&cfg("duckdb")).await.unwrap();
        let guard2 = p2.lock().unwrap();
        let block = guard2
            .format_for_system_prompt(MemoryTarget::Memory)
            .expect("duckdb reload should populate");
        assert!(block.contains("duckdb-fact-XYZ"), "got: {block}");
    }

    // =========================================================================
    // Plan 20-02: `build_memory_manager` should construct a no-mirror manager
    // for the default `file` provider without error.
    // =========================================================================

    #[tokio::test]
    async fn factory_builds_manager_with_no_mirror() {
        let _guard = env_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
        let mgr = build_memory_manager(&cfg("file")).await;
        assert!(
            mgr.is_ok(),
            "file-provider manager must build with no mirror, got {:?}",
            mgr.err()
        );
        // Smoke-test that we can lock + drop the tokio mutex.
        let handle = mgr.unwrap();
        let _guard = handle.lock().await;
    }

    #[cfg(feature = "memory-grafeo")]
    #[tokio::test]
    async fn grafeo_round_trip_via_factory() {
        // See sqlite_round_trip_via_factory for the double-set rationale.
        let _guard = env_lock();
        let tmp = tempfile::TempDir::new().unwrap();

        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
        {
            let p = build_memory_provider(&cfg("grafeo")).await.unwrap();
            let mut guard = p.lock().unwrap();
            guard
                .add(MemoryTarget::Memory, "grafeo-fact-XYZ")
                .expect("add should succeed");
        }

        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
        let p2 = build_memory_provider(&cfg("grafeo")).await.unwrap();
        let guard2 = p2.lock().unwrap();
        let block = guard2
            .format_for_system_prompt(MemoryTarget::Memory)
            .expect("grafeo reload should populate");
        assert!(block.contains("grafeo-fact-XYZ"), "got: {block}");
    }
}
