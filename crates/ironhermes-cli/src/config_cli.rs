//! `hermes config <subcommand>` — six configuration subcommands (Phase 23, D-08..D-11).
//!
//! Structural model: `cron.rs::CronCommands` (Subcommand enum + handle_* dispatcher).
//! All pure logic delegated to `ironhermes_core::{config_schema, config_setter}`.

use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::Path;

#[derive(Subcommand)]
pub enum ConfigSubcommand {
    /// Set a config value by dotted path (e.g., model.default openrouter/foo)
    Set { key: String, value: String },
    /// Get a config value by dotted path
    Get { key: String },
    /// Show full active config (secrets masked, Learning Loop banner first)
    Show,
    /// Scan installed skills and prompt for missing config/env gaps
    Migrate,
    /// Print path to config.yaml
    Path,
    /// Print path to .env
    #[command(name = "env-path")]
    EnvPath,
}

pub async fn handle_config_command(cmd: ConfigSubcommand) -> Result<()> {
    let hermes_home = ironhermes_core::constants::get_hermes_home();
    match cmd {
        ConfigSubcommand::Set { key, value } => cmd_config_set(&hermes_home, &key, &value).await,
        ConfigSubcommand::Get { key } => cmd_config_get(&hermes_home, &key).await,
        ConfigSubcommand::Show => cmd_config_show(&hermes_home).await,
        ConfigSubcommand::Migrate => cmd_config_migrate(&hermes_home).await,
        ConfigSubcommand::Path => {
            println!("{}", hermes_home.join("config.yaml").display());
            Ok(())
        }
        ConfigSubcommand::EnvPath => {
            println!("{}", hermes_home.join(".env").display());
            Ok(())
        }
    }
}

// Stubs — full impls in Tasks 4–6.
async fn cmd_config_set(_home: &Path, _k: &str, _v: &str) -> Result<()> {
    anyhow::bail!("config set not implemented (Task 4)")
}
async fn cmd_config_get(_home: &Path, _k: &str) -> Result<()> {
    anyhow::bail!("config get not implemented (Task 4)")
}
async fn cmd_config_show(_home: &Path) -> Result<()> {
    anyhow::bail!("config show not implemented (Task 5)")
}
async fn cmd_config_migrate(_home: &Path) -> Result<()> {
    anyhow::bail!("config migrate not implemented (Task 6)")
}
