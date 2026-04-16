use std::sync::{Arc, Mutex};

use ironhermes_core::MemoryProvider;
use ironhermes_core::memory_store::MemoryStore;
use ironhermes_core::constants::get_hermes_home;

/// Build a memory provider from config. Returns Arc<Mutex<...>> for direct use with MemoryTool.
/// Relocated from ironhermes-core per D-11. Feature-gated providers per D-12.
/// For the file-backed provider, loads existing memories from disk (warn-on-error).
/// External backends (sqlite/grafeo/duckdb) persist natively — no explicit load needed.
pub fn build_memory_provider(
    config: &ironhermes_core::config::MemoryConfig,
) -> anyhow::Result<Arc<Mutex<dyn MemoryProvider + Send>>> {
    match config.provider.as_str() {
        "file" => {
            let memory_dir = get_hermes_home().join("memories");
            let mut store = MemoryStore::new(memory_dir);
            if let Err(e) = store.load_from_disk() {
                tracing::warn!("Failed to load memory from disk: {}", e);
            }
            Ok(Arc::new(Mutex::new(store)))
        }
        #[cfg(feature = "memory-sqlite")]
        "sqlite" => {
            let db_path = get_hermes_home().join("memory.db");
            let provider = memory_sqlite::SqliteMemoryProvider::new(&db_path)?;
            Ok(Arc::new(Mutex::new(provider)))
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
            let db_path = get_hermes_home().join("memory_duckdb.db");
            let provider = memory_duckdb::DuckDbMemoryProvider::new(&db_path)?;
            Ok(Arc::new(Mutex::new(provider)))
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
            let db_path = get_hermes_home().join("memory_graph");
            let provider = memory_grafeo::GrafeoMemoryProvider::new(&db_path)?;
            Ok(Arc::new(Mutex::new(provider)))
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::config::MemoryConfig;

    fn cfg(provider: &str) -> MemoryConfig {
        let mut c = MemoryConfig::default();
        c.provider = provider.to_string();
        c
    }

    #[test]
    fn file_provider_returns_ok() {
        // Also exercises the load_from_disk path added in Task 1 — a missing
        // memories directory must not cause a bail (warn-on-error behavior).
        let result = build_memory_provider(&cfg("file"));
        assert!(result.is_ok(), "file provider should build, got {:?}", result.err());
    }

    #[test]
    fn unknown_provider_returns_err_with_message() {
        let result = build_memory_provider(&cfg("totally-unknown"));
        assert!(result.is_err(), "unknown provider must error");
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("Unknown memory provider 'totally-unknown'"),
            "got: {msg}",
        );
        assert!(msg.contains("file"), "available providers must include 'file', got: {msg}");
    }

    #[cfg(feature = "memory-sqlite")]
    #[test]
    fn sqlite_provider_with_feature_returns_ok() {
        // UAT Test 2 regression guard — this test must pass under
        // `cargo test -p ironhermes-agent --features memory-sqlite`.
        // Verifies that run_gateway no longer bails with "requires a feature flag
        // that is not enabled" when built with --features memory-sqlite.
        let _tmp = tempfile::TempDir::new().expect("tempdir");
        // SAFETY: test-only env mutation; acceptable since tests run single-threaded
        // per cargo's default for lib tests.
        unsafe { std::env::set_var("HERMES_HOME", _tmp.path()); }
        let result = build_memory_provider(&cfg("sqlite"));
        assert!(
            result.is_ok(),
            "sqlite provider must build when memory-sqlite feature is enabled, got {:?}",
            result.err(),
        );
    }

    #[cfg(not(feature = "memory-sqlite"))]
    #[test]
    fn sqlite_provider_without_feature_returns_err_naming_feature() {
        let result = build_memory_provider(&cfg("sqlite"));
        assert!(result.is_err(), "sqlite provider without feature must error");
        let msg = result.err().unwrap().to_string();
        assert!(msg.contains("memory-sqlite"), "error must name the feature, got: {msg}");
        assert!(
            msg.contains("cargo build --features memory-sqlite"),
            "error must include the rebuild instruction, got: {msg}",
        );
    }
}
