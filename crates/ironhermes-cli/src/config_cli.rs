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

pub async fn handle_config_command(cmd: ConfigSubcommand, profile_name: &str) -> Result<()> {
    let hermes_home = ironhermes_core::constants::get_hermes_home();
    match cmd {
        ConfigSubcommand::Set { key, value } => cmd_config_set(&hermes_home, &key, &value).await,
        ConfigSubcommand::Get { key } => cmd_config_get(&hermes_home, &key).await,
        ConfigSubcommand::Show => cmd_config_show(&hermes_home, profile_name).await,
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

async fn cmd_config_show(hermes_home: &Path, profile_name: &str) -> Result<()> {
    // Phase 24 D-15: always-on Profile header, ABOVE Phase 23's Learning Loop banner.
    println!("Profile: {}", profile_name);
    println!();

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

    // D-17: Learning Loop banner (Phase 23). Sits BELOW the Phase 24 Profile: line.
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

async fn cmd_config_migrate(hermes_home: &Path) -> Result<()> {
    use ironhermes_core::SkillRegistry;

    // Load installed skills and collect their config/env declarations.
    let skills_dir = hermes_home.join("skills");
    if !skills_dir.exists() {
        println!(
            "No configuration gaps detected. (No skills directory at {})",
            skills_dir.display()
        );
        return Ok(());
    }

    let registry = SkillRegistry::load_with_paths(&[skills_dir.clone()]);

    let mut config_gaps: Vec<(String, String)> = Vec::new(); // (skill_name, dotted_key)
    let mut env_gaps: Vec<(String, String)> = Vec::new(); // (skill_name, env_var)

    // Walk each skill's hermes_metadata for config / required_environment_variables.
    for skill in registry.list() {
        if let Some(hm) = &skill.hermes_metadata {
            for cfg_field in &hm.config {
                if config_setter::config_get(hermes_home, &cfg_field.key)?.is_none() {
                    config_gaps.push((skill.name.clone(), cfg_field.key.clone()));
                }
            }
            for env_entry in &hm.required_environment_variables {
                if !env_var_exists_in_dotenv(&hermes_home.join(".env"), &env_entry.name)? {
                    env_gaps.push((skill.name.clone(), env_entry.name.clone()));
                }
            }
        }
    }

    if config_gaps.is_empty() && env_gaps.is_empty() {
        println!("No configuration gaps detected.");
        return Ok(());
    }

    // Print gap table.
    println!("Skill configuration gaps:");
    println!();
    if !config_gaps.is_empty() {
        println!("  config.yaml gaps:");
        for (skill, key) in &config_gaps {
            println!("    [{}] missing: {}", skill, key);
        }
        println!();
    }
    if !env_gaps.is_empty() {
        println!("  .env gaps:");
        for (skill, var) in &env_gaps {
            println!("    [{}] missing: {}", skill, var);
        }
        println!();
    }

    // Per-gap prompts with skip/skip-all.
    let mut rl = rustyline::DefaultEditor::new()?;
    let mut skip_all = false;
    for (skill, key) in &config_gaps {
        if skip_all {
            break;
        }
        println!("[{}] Set {} now? [y / skip / skip all]", skill, key);
        let answer = rl.readline("> ").unwrap_or_default();
        match answer.trim() {
            "skip all" => {
                skip_all = true;
            }
            "skip" | "n" | "no" => continue,
            _ => {
                let value = rl.readline(&format!("Value for {}: ", key))?;
                config_setter::config_set(hermes_home, key, value.trim())?;
            }
        }
    }
    // Env gaps left to manual (.env editing) — print path hint.
    if !env_gaps.is_empty() {
        println!(
            "Edit env vars in {} (use `hermes config env-path`).",
            hermes_home.join(".env").display()
        );
    }

    Ok(())
}

fn env_var_exists_in_dotenv(env_path: &Path, var: &str) -> Result<bool> {
    if !env_path.exists() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(env_path)?;
    Ok(text.lines().any(|l| {
        let trimmed = l.trim_start();
        trimmed.starts_with(&format!("{}=", var))
    }))
}
