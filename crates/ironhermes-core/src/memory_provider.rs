use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::memory_store::{MemoryResult, MemoryTarget};

// =============================================================================
// MemoryEntries wrapper type
// =============================================================================

/// Wrapper around per-target entry vectors, used by lifecycle hooks.
#[derive(Debug, Clone, Default)]
pub struct MemoryEntries {
    pub entries: HashMap<MemoryTarget, Vec<String>>,
}

// =============================================================================
// MemoryProviderConfig
// =============================================================================

/// Configuration passed to a memory provider during initialization (D-03).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryProviderConfig {
    pub provider: String,
    pub memory_dir: PathBuf,
    pub memory_char_limit: usize,
    pub user_char_limit: usize,
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

// =============================================================================
// MemoryProvider trait
// =============================================================================

/// Pluggable memory backend abstraction (D-01 through D-07).
///
/// The trait has two sections:
/// - **Lifecycle hooks** (async): initialize, prefetch, sync_turn, on_session_end, shutdown
/// - **Operational methods** (sync): load_from_disk, add, replace, remove, format_for_system_prompt, to_memory_entries
///
/// The operational methods mirror MemoryStore's public API so that `dyn MemoryProvider`
/// is a full replacement for concrete `MemoryStore` at all call sites.
#[async_trait]
pub trait MemoryProvider: Send + Sync + 'static {
    // === Lifecycle hooks (D-01 through D-07) ===

    /// Set up provider resources. Fatal on error (D-11).
    async fn initialize(&mut self, config: &MemoryProviderConfig) -> anyhow::Result<()>;
    /// Load provider state for session. On error, log warning and return empty (D-12).
    async fn prefetch(&self, session_id: &str) -> anyhow::Result<MemoryEntries>;
    /// Sync current entries after mutation. On error, log warning and skip (D-12).
    async fn sync_turn(&self, session_id: &str, entries: &MemoryEntries) -> anyhow::Result<()>;
    /// Persist/flush on session end. On error, log warning and skip (D-12).
    async fn on_session_end(
        &self,
        session_id: &str,
        entries: &MemoryEntries,
    ) -> anyhow::Result<()>;
    /// Clean teardown. Fatal on error (D-11).
    async fn shutdown(&mut self) -> anyhow::Result<()>;

    // === Operational methods (sync -- used by MemoryTool and PromptBuilder) ===

    /// Load entries from backing store. Called at startup before wrapping in Arc<Mutex<>>.
    fn load_from_disk(&mut self) -> anyhow::Result<()>;
    /// Add a new memory entry for the given target.
    fn add(&mut self, target: MemoryTarget, content: &str) -> MemoryResult;
    /// Replace an entry identified by substring match.
    fn replace(
        &mut self,
        target: MemoryTarget,
        old_text: &str,
        new_content: &str,
    ) -> MemoryResult;
    /// Remove an entry identified by substring match.
    fn remove(&mut self, target: MemoryTarget, old_text: &str) -> MemoryResult;
    /// Return formatted snapshot for system prompt injection (D-12: frozen snapshot).
    fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String>;
    /// Return current entries as a MemoryEntries snapshot.
    fn to_memory_entries(&self) -> MemoryEntries;
}

// =============================================================================
// format_entries_for_prompt standalone function
// =============================================================================

/// Format entries for a given target into a prompt-ready string.
/// Returns None if no entries exist for the target.
pub fn format_entries_for_prompt(entries: &MemoryEntries, target: MemoryTarget) -> Option<String> {
    let entry_list = entries.entries.get(&target)?;
    if entry_list.is_empty() {
        return None;
    }
    let header = match target {
        MemoryTarget::Memory => "## Memory",
        MemoryTarget::User => "## User Profile",
    };
    Some(format!("{}\n\n{}", header, entry_list.join("\n")))
}

// =============================================================================
// build_memory_provider factory (D-09, D-10)
// =============================================================================

use crate::config::MemoryConfig;
use crate::memory_store::MemoryStore;

const MEMORIES_DIR: &str = "memories";

/// Build a memory provider from configuration.
///
/// Currently only "file" is available. Future providers (sqlite, grafeo, duckdb)
/// require feature flags. Unknown provider names cause a hard error at startup (D-09).
pub fn build_memory_provider(
    config: &MemoryConfig,
) -> anyhow::Result<Box<dyn MemoryProvider + Send>> {
    match config.provider.as_str() {
        "file" => {
            let memory_dir = crate::constants::get_hermes_home().join(MEMORIES_DIR);
            Ok(Box::new(MemoryStore::new(memory_dir)))
        }
        "sqlite" | "grafeo" | "duckdb" => {
            anyhow::bail!(
                "Memory provider '{}' is not available in this build. \
                 It requires the '{}' feature flag. Available providers: file.",
                config.provider,
                config.provider
            )
        }
        other => {
            anyhow::bail!(
                "Unknown memory provider '{}'. Available providers: file. \
                 Future providers (sqlite, grafeo, duckdb) require feature flags.",
                other
            )
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- MemoryEntries tests ----

    #[test]
    fn test_memory_entries_default_is_empty() {
        let entries = MemoryEntries::default();
        assert!(entries.entries.is_empty());
    }

    #[test]
    fn test_memory_entries_insert_and_retrieve() {
        let mut entries = MemoryEntries::default();
        entries
            .entries
            .insert(MemoryTarget::Memory, vec!["fact one".into()]);
        entries
            .entries
            .insert(MemoryTarget::User, vec!["user info".into()]);

        assert_eq!(entries.entries[&MemoryTarget::Memory], vec!["fact one"]);
        assert_eq!(entries.entries[&MemoryTarget::User], vec!["user info"]);
    }

    // ---- MemoryProviderConfig tests ----

    #[test]
    fn test_provider_config_serde_round_trip() {
        let config = MemoryProviderConfig {
            provider: "file".to_string(),
            memory_dir: PathBuf::from("/tmp/test"),
            memory_char_limit: 2200,
            user_char_limit: 1375,
            extra: HashMap::new(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: MemoryProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.provider, "file");
        assert_eq!(parsed.memory_dir, PathBuf::from("/tmp/test"));
        assert_eq!(parsed.memory_char_limit, 2200);
        assert_eq!(parsed.user_char_limit, 1375);
    }

    #[test]
    fn test_provider_config_extra_defaults_to_empty() {
        let json = r#"{
            "provider": "file",
            "memory_dir": "/tmp",
            "memory_char_limit": 2200,
            "user_char_limit": 1375
        }"#;
        let config: MemoryProviderConfig = serde_json::from_str(json).unwrap();
        assert!(config.extra.is_empty());
    }

    // ---- format_entries_for_prompt tests ----

    #[test]
    fn test_format_entries_for_prompt_empty_returns_none() {
        let entries = MemoryEntries::default();
        assert!(format_entries_for_prompt(&entries, MemoryTarget::Memory).is_none());
    }

    #[test]
    fn test_format_entries_for_prompt_empty_vec_returns_none() {
        let mut entries = MemoryEntries::default();
        entries.entries.insert(MemoryTarget::Memory, vec![]);
        assert!(format_entries_for_prompt(&entries, MemoryTarget::Memory).is_none());
    }

    #[test]
    fn test_format_entries_for_prompt_memory_target() {
        let mut entries = MemoryEntries::default();
        entries.entries.insert(
            MemoryTarget::Memory,
            vec!["fact one".into(), "fact two".into()],
        );
        let result = format_entries_for_prompt(&entries, MemoryTarget::Memory).unwrap();
        assert_eq!(result, "## Memory\n\nfact one\nfact two");
    }

    #[test]
    fn test_format_entries_for_prompt_user_target() {
        let mut entries = MemoryEntries::default();
        entries
            .entries
            .insert(MemoryTarget::User, vec!["user info".into()]);
        let result = format_entries_for_prompt(&entries, MemoryTarget::User).unwrap();
        assert_eq!(result, "## User Profile\n\nuser info");
    }

    // ---- Mock MemoryProvider (proves dyn-compatibility) ----

    struct MockProvider {
        entries: MemoryEntries,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                entries: MemoryEntries::default(),
            }
        }
    }

    #[async_trait]
    impl MemoryProvider for MockProvider {
        async fn initialize(&mut self, _config: &MemoryProviderConfig) -> anyhow::Result<()> {
            Ok(())
        }
        async fn prefetch(&self, _session_id: &str) -> anyhow::Result<MemoryEntries> {
            Ok(self.entries.clone())
        }
        async fn sync_turn(
            &self,
            _session_id: &str,
            _entries: &MemoryEntries,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn on_session_end(
            &self,
            _session_id: &str,
            _entries: &MemoryEntries,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn shutdown(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn load_from_disk(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn add(&mut self, target: MemoryTarget, content: &str) -> MemoryResult {
            self.entries
                .entries
                .entry(target)
                .or_default()
                .push(content.to_string());
            Ok(format!("added: {}", content))
        }
        fn replace(
            &mut self,
            _target: MemoryTarget,
            _old_text: &str,
            _new_content: &str,
        ) -> MemoryResult {
            Ok("replaced".to_string())
        }
        fn remove(&mut self, _target: MemoryTarget, _old_text: &str) -> MemoryResult {
            Ok("removed".to_string())
        }
        fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String> {
            format_entries_for_prompt(&self.entries, target)
        }
        fn to_memory_entries(&self) -> MemoryEntries {
            self.entries.clone()
        }
    }

    #[test]
    fn test_mock_provider_as_boxed_dyn_send() {
        // Proves the trait is dyn-compatible with Send bound
        let provider: Box<dyn MemoryProvider + Send> = Box::new(MockProvider::new());
        // Just proving construction compiles is the test
        assert!(provider.format_for_system_prompt(MemoryTarget::Memory).is_none());
    }

    // ---- build_memory_provider factory tests ----

    #[test]
    fn test_build_memory_provider_file_returns_ok() {
        let config = crate::config::MemoryConfig {
            provider: "file".to_string(),
        };
        let result = super::build_memory_provider(&config);
        assert!(result.is_ok(), "file provider should build: {:?}", result.err());
    }

    #[test]
    fn test_build_memory_provider_unknown_returns_error() {
        let config = crate::config::MemoryConfig {
            provider: "unknown".to_string(),
        };
        let result = super::build_memory_provider(&config);
        assert!(result.is_err());
        let err = format!("{}", result.err().unwrap());
        assert!(
            err.contains("Unknown memory provider 'unknown'"),
            "Error should mention unknown: {}",
            err
        );
        assert!(err.contains("Available providers: file"), "Error should list providers: {}", err);
    }

    #[test]
    fn test_build_memory_provider_sqlite_returns_feature_flag_error() {
        let config = crate::config::MemoryConfig {
            provider: "sqlite".to_string(),
        };
        let result = super::build_memory_provider(&config);
        assert!(result.is_err());
        let err = format!("{}", result.err().unwrap());
        assert!(err.contains("feature flag"), "Error should mention feature flag: {}", err);
    }

    // ---- MemoryStore as MemoryProvider tests ----

    #[test]
    fn test_memory_store_implements_memory_provider() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join("memories");
        let mut store = crate::memory_store::MemoryStore::new(mem_dir);

        // Use trait methods
        let provider: &mut dyn MemoryProvider = &mut store;
        provider.load_from_disk().unwrap();
        let result = provider.add(MemoryTarget::Memory, "test fact");
        assert!(result.is_ok());

        let entries = provider.to_memory_entries();
        assert!(entries.entries.contains_key(&MemoryTarget::Memory));
        assert_eq!(entries.entries[&MemoryTarget::Memory], vec!["test fact"]);
    }

    #[test]
    fn test_arc_mutex_coercion_and_operations() {
        use std::sync::{Arc, Mutex};

        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join("memories");
        let store = crate::memory_store::MemoryStore::new(mem_dir);

        // Coerce Arc<Mutex<MemoryStore>> to Arc<Mutex<dyn MemoryProvider + Send>>
        let provider: Arc<Mutex<dyn MemoryProvider + Send>> = Arc::new(Mutex::new(store));

        // Operate through trait object behind Arc<Mutex<>>
        {
            let mut p = provider.lock().unwrap();
            p.load_from_disk().unwrap();
            let result = p.add(MemoryTarget::Memory, "arc test");
            assert!(result.is_ok());
        }
        {
            let mut p = provider.lock().unwrap();
            let result = p.replace(MemoryTarget::Memory, "arc test", "arc updated");
            assert!(result.is_ok());
        }
        {
            let mut p = provider.lock().unwrap();
            let result = p.remove(MemoryTarget::Memory, "arc updated");
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_mock_provider_operational_methods_through_trait_object() {
        let mut provider: Box<dyn MemoryProvider + Send> = Box::new(MockProvider::new());

        // add through trait object
        let result = provider.add(MemoryTarget::Memory, "test entry");
        assert!(result.is_ok());

        // replace through trait object
        let result = provider.replace(MemoryTarget::Memory, "test", "new");
        assert!(result.is_ok());

        // remove through trait object
        let result = provider.remove(MemoryTarget::Memory, "test");
        assert!(result.is_ok());

        // to_memory_entries through trait object
        let entries = provider.to_memory_entries();
        assert!(entries.entries.contains_key(&MemoryTarget::Memory));
    }
}
