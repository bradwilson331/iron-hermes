use std::sync::{Arc, Mutex};

use ironhermes_core::MemoryProvider;
use ironhermes_core::memory_store::MemoryStore;
use ironhermes_core::constants::get_hermes_home;

/// Build a memory provider from config. Returns Arc<Mutex<...>> for direct use with MemoryTool.
/// Relocated from ironhermes-core per D-11. Feature-gated providers per D-12.
pub fn build_memory_provider(
    config: &ironhermes_core::config::MemoryConfig,
) -> anyhow::Result<Arc<Mutex<dyn MemoryProvider + Send>>> {
    match config.provider.as_str() {
        "file" => {
            let memory_dir = get_hermes_home().join("memories");
            let store = MemoryStore::new(memory_dir);
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
        #[cfg(not(feature = "memory-duckdb"))]
        "duckdb" => {
            anyhow::bail!(
                "Memory provider 'duckdb' requires the 'memory-duckdb' feature. \
                 Rebuild with: cargo build --features memory-duckdb"
            );
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
                "Unknown memory provider '{}'. Available providers: file{}",
                other,
                if cfg!(feature = "memory-sqlite") { ", sqlite" } else { "" }
            );
        }
    }
}
