//! Pre-flight check (D-05/D-07): runs after Cli::parse() and before
//! dispatch. Detects missing config or validation failures and launches
//! fix-mode wizard before falling through to the original command.

use anyhow::Result;
use ironhermes_core::config::Config;

use crate::Cli;

pub async fn run_preflight_check(_cli: &Cli) -> Result<()> {
    let cfg_path = Config::config_path();
    if !cfg_path.exists() {
        return crate::setup::run_setup(None, ironhermes_core::wizard::WizardMode::FirstRun).await;
    }
    match Config::load() {
        Err(_) => {
            crate::setup::run_setup(None, ironhermes_core::wizard::WizardMode::FixMode).await
        }
        Ok(config) => {
            if !config.validate().is_empty() {
                crate::setup::run_setup(None, ironhermes_core::wizard::WizardMode::FixMode).await
            } else {
                Ok(())
            }
        }
    }
}
