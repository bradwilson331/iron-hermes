use anyhow::Result;
use ironhermes_core::Config;
use tracing::{info, warn};

use crate::adapter::{MessageHandler, PlatformAdapter};
use crate::session::SessionStore;
use crate::telegram::TelegramAdapter;

/// Runs the multi-platform messaging gateway.
pub struct GatewayRunner {
    config: Config,
    adapters: Vec<Box<dyn PlatformAdapter>>,
    session_store: SessionStore,
}

impl GatewayRunner {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            adapters: Vec::new(),
            session_store: SessionStore::new(),
        }
    }

    /// Initialize and start all configured platform adapters.
    pub async fn start(&mut self, _handler: Box<dyn MessageHandler>) -> Result<()> {
        info!("Starting gateway");

        // Check for Telegram
        if let Some(platform_config) = self.config.gateway.platforms.get("telegram") {
            if platform_config.enabled {
                if let Some(ref token) = platform_config.token {
                    let resolved_token = resolve_env_var(token);
                    let adapter = TelegramAdapter::new(resolved_token);
                    // Clone the handler for each adapter
                    // In a real implementation, we'd use Arc<dyn MessageHandler>
                    info!("Telegram adapter configured");
                    self.adapters.push(Box::new(adapter));
                } else {
                    warn!("Telegram enabled but no token configured");
                }
            }
        }

        info!(
            adapters = self.adapters.len(),
            "Gateway started with {} platform(s)",
            self.adapters.len()
        );

        // Keep running until interrupted
        tokio::signal::ctrl_c().await?;
        self.stop().await?;

        Ok(())
    }

    /// Stop all adapters gracefully.
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping gateway");
        for adapter in &mut self.adapters {
            if let Err(e) = adapter.stop().await {
                warn!(
                    platform = %adapter.platform(),
                    error = %e,
                    "Failed to stop adapter"
                );
            }
        }
        Ok(())
    }

    pub fn session_store(&self) -> &SessionStore {
        &self.session_store
    }

    pub fn session_store_mut(&mut self) -> &mut SessionStore {
        &mut self.session_store
    }
}

/// Resolve environment variable references in config values (e.g., "${TELEGRAM_BOT_TOKEN}").
fn resolve_env_var(value: &str) -> String {
    if value.starts_with("${") && value.ends_with('}') {
        let var_name = &value[2..value.len() - 1];
        std::env::var(var_name).unwrap_or_else(|_| value.to_string())
    } else {
        value.to_string()
    }
}
