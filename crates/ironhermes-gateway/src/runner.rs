use anyhow::Result;
use ironhermes_core::Config;
use tracing::info;

/// Runs the multi-platform messaging gateway.
pub struct GatewayRunner {
    #[allow(dead_code)]
    config: Config,
}

impl GatewayRunner {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Start the gateway. Full implementation arrives in plan 03.
    pub async fn start(&self) -> Result<()> {
        info!("Gateway runner placeholder — full implementation in plan 03");
        Ok(())
    }
}
