//! Polling-based hot-reload watcher for hooks.toml.
//!
//! Polls the config file every 5 seconds for modification time changes.
//! When a change is detected, the config is reloaded and the shared state updated.
//! This avoids the `notify` crate dependency (simpler, per D-12).

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::config::HooksConfig;

/// Spawn a background task that polls hooks.toml for changes every 5 seconds.
/// When the file's modified time changes, reload the config and update the shared state.
/// Returns the shared config handle for readers.
pub fn spawn_config_watcher(
    config_path: PathBuf,
    cancel: tokio_util::sync::CancellationToken,
) -> Arc<RwLock<HooksConfig>> {
    let initial_config = HooksConfig::load_from(&config_path).unwrap_or_default();
    let shared_config = Arc::new(RwLock::new(initial_config));
    let config_handle = shared_config.clone();

    tokio::spawn(async move {
        let mut last_modified = std::fs::metadata(&config_path)
            .and_then(|m| m.modified())
            .ok();

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {
                    let current_modified = std::fs::metadata(&config_path)
                        .and_then(|m| m.modified())
                        .ok();
                    if current_modified != last_modified {
                        match HooksConfig::load_from(&config_path) {
                            Ok(new_config) => {
                                tracing::info!("hooks.toml reloaded");
                                *shared_config.write().await = new_config;
                                last_modified = current_modified;
                            }
                            Err(e) => {
                                tracing::warn!("Failed to reload hooks.toml: {}", e);
                            }
                        }
                    }
                }
            }
        }
    });

    config_handle
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn test_spawn_config_watcher_loads_initial() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("hooks.toml");

        // Write a hooks.toml with known values
        let toml_content = r#"
blocked_tools = ["terminal"]

[event_log]
enabled = false
"#;
        let mut f = std::fs::File::create(&config_path).expect("create");
        f.write_all(toml_content.as_bytes()).expect("write");

        let cancel = CancellationToken::new();
        let shared = spawn_config_watcher(config_path, cancel.clone());

        // Read initial config — should reflect the file contents
        let config = shared.read().await;
        assert!(!config.event_log.enabled);
        assert_eq!(config.blocked_tools, vec!["terminal"]);

        cancel.cancel();
    }
}
