//! `hermes config <subcommand>` — six configuration subcommands (Phase 23, D-08..D-11).
//!
//! Structural model: `cron.rs::CronCommands` (Subcommand enum + handle_* dispatcher).
//! All pure logic delegated to `ironhermes_core::{config_schema, config_setter}`.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use ironhermes_core::{config_schema, config_setter};
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

async fn cmd_config_set(hermes_home: &Path, key: &str, value: &str) -> Result<()> {
    let schema = config_schema::schema();
    if config_setter::is_cache_breaking(key, &schema) {
        // D-10: warn-and-persist. Warning to stderr; persistence message to stdout.
        eprintln!(
            "{} Changing {} invalidates the prompt cache. Active sessions will pay full cache-miss cost on next turn.",
            "⚠".yellow(),
            key
        );
    }
    let _old = config_setter::config_set(hermes_home, key, value)
        .with_context(|| format!("failed to set {}", key))?;
    println!("Persisted: {} = {}", key, value);
    Ok(())
}

async fn cmd_config_get(hermes_home: &Path, key: &str) -> Result<()> {
    match config_setter::config_get(hermes_home, key)? {
        Some(v) => println!("{}", v),
        None => {} // missing key: silent + exit 0 (idiomatic for shell scripting)
    }
    Ok(())
}

/// Mask a secret value with prefix preservation (D-09).
/// Format: first 4–6 chars + "***".
fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let prefix_len = value.len().min(6).max(4).min(value.len());
    format!("{}***", &value[..prefix_len])
}

/// Walk a serde_yaml::Value, masking the leaf at every dotted-path that
/// matches a SCHEMA entry with `secret: true`.
fn redact_secrets(doc: &mut serde_yaml::Value, schema: &[config_schema::ConfigField]) {
    for field in schema.iter().filter(|f| f.secret) {
        let keys: Vec<&str> = field.key.split('.').collect();
        redact_at(doc, &keys);
    }
}

fn redact_at(doc: &mut serde_yaml::Value, keys: &[&str]) {
    let mut node = doc;
    for (i, k) in keys.iter().enumerate() {
        let key_v = serde_yaml::Value::String((*k).to_string());
        let map = match node.as_mapping_mut() {
            Some(m) => m,
            None => return,
        };
        if i == keys.len() - 1 {
            if let Some(v) = map.get_mut(&key_v) {
                if let Some(s) = v.as_str() {
                    *v = serde_yaml::Value::String(mask_secret(s));
                }
            }
            return;
        }
        match map.get_mut(&key_v) {
            Some(next) => node = next,
            None => return,
        }
    }
}

async fn cmd_config_show(hermes_home: &Path) -> Result<()> {
    let cfg_path = hermes_home.join("config.yaml");
    if !cfg_path.exists() {
        println!("No config.yaml found at {}.", cfg_path.display());
        println!("Run `hermes setup` to create one.");
        return Ok(());
    }

    let text = std::fs::read_to_string(&cfg_path)
        .with_context(|| format!("reading {}", cfg_path.display()))?;
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&text)
        .unwrap_or(serde_yaml::Value::Mapping(Default::default()));

    // D-17: Learning Loop banner first.
    let memory_enabled = config_setter::config_get(hermes_home, "memory.memory_enabled")
        .ok()
        .flatten()
        .as_deref()
        == Some("true");
    let skill_gen = config_setter::config_get(hermes_home, "learning.skill_generation_enabled")
        .ok()
        .flatten()
        .as_deref()
        == Some("true");
    if memory_enabled && skill_gen {
        println!("🧠 Learning Loop: enabled (memory + skill generation)");
    } else {
        println!("⚠ Learning Loop: disabled — IronHermes is operating as a single-session agent. Run `hermes setup memory` to enable.");
    }
    println!();

    // D-09: redact secrets in-place.
    let schema = config_schema::schema();
    redact_secrets(&mut doc, &schema);

    let masked_yaml = serde_yaml::to_string(&doc)?;
    print!("{}", masked_yaml);
    Ok(())
}

async fn cmd_config_migrate(_home: &Path) -> Result<()> {
    anyhow::bail!("config migrate not implemented (Task 6)")
}
