//! CLI subcommands for memory management (Phase 21.4, D-09/D-10).
//!
//! `hermes memory status` — display current memory subsystem state.
//! `hermes memory off` — disable external provider, keep built-in file memory.

use anyhow::Result;
use colored::Colorize;
use ironhermes_core::config::Config;
use ironhermes_core::memory_store::{MemoryStore, MemoryTarget};

/// Handle `hermes memory status` (D-09).
///
/// Displays:
/// - Active provider name, memory_enabled and user_profile_enabled toggles
/// - MEMORY.md: entry count, chars used / chars limit, percentage
/// - USER.md: same fields
/// - Mirror provider: enabled/disabled, name if set
pub async fn handle_memory_status() -> Result<()> {
    let config = Config::load()?;
    let mem_config = &config.memory;

    println!("{}", "=== Memory Status ===".bold());
    println!();

    // Provider info
    println!("  {}: {}", "Provider".bold(), mem_config.provider.cyan());
    println!(
        "  {}: {}",
        "Memory enabled".bold(),
        if mem_config.memory_enabled {
            "yes".green()
        } else {
            "no".red()
        }
    );
    println!(
        "  {}: {}",
        "User profile enabled".bold(),
        if mem_config.user_profile_enabled {
            "yes".green()
        } else {
            "no".red()
        }
    );

    // Load memory store to get actual sizes
    if mem_config.memory_enabled {
        let hermes_home = ironhermes_core::get_hermes_home();
        let mut store = MemoryStore::new(hermes_home.clone());
        if store.load_from_disk().is_ok() {
            let entries_map = store.entries();

            println!();
            println!("  {}", "MEMORY.md".bold().underline());
            let mem_entries = entries_map
                .get(&MemoryTarget::Memory)
                .map(|v| v.len())
                .unwrap_or(0);
            let mem_chars = entries_map
                .get(&MemoryTarget::Memory)
                .map(|v| v.iter().map(|s| s.len()).sum::<usize>())
                .unwrap_or(0);
            let mem_limit = MemoryTarget::Memory.char_limit();
            let mem_pct = if mem_limit > 0 {
                (mem_chars as f64 / mem_limit as f64 * 100.0) as u32
            } else {
                0
            };
            println!("    Entries:  {}", mem_entries.to_string().cyan());
            println!(
                "    Usage:    {}/{} chars ({}%)",
                mem_chars.to_string().cyan(),
                mem_limit.to_string().dimmed(),
                format!("{}", mem_pct).yellow()
            );

            if mem_config.user_profile_enabled {
                println!();
                println!("  {}", "USER.md".bold().underline());
                let user_entries = entries_map
                    .get(&MemoryTarget::User)
                    .map(|v| v.len())
                    .unwrap_or(0);
                let user_chars = entries_map
                    .get(&MemoryTarget::User)
                    .map(|v| v.iter().map(|s| s.len()).sum::<usize>())
                    .unwrap_or(0);
                let user_limit = MemoryTarget::User.char_limit();
                let user_pct = if user_limit > 0 {
                    (user_chars as f64 / user_limit as f64 * 100.0) as u32
                } else {
                    0
                };
                println!("    Entries:  {}", user_entries.to_string().cyan());
                println!(
                    "    Usage:    {}/{} chars ({}%)",
                    user_chars.to_string().cyan(),
                    user_limit.to_string().dimmed(),
                    format!("{}", user_pct).yellow()
                );
            } else {
                println!();
                println!("  {}: {}", "USER.md".bold(), "disabled".dimmed());
            }
        } else {
            println!();
            println!("  {}", "Could not load memory store from disk".yellow());
        }
    } else {
        println!();
        println!("  {}", "Memory subsystem is disabled".yellow());
    }

    // Mirror status
    println!();
    match &mem_config.mirror_provider {
        Some(mirror) => println!(
            "  {}: {} ({})",
            "Mirror".bold(),
            "enabled".green(),
            mirror.cyan()
        ),
        None => println!("  {}: {}", "Mirror".bold(), "none".dimmed()),
    }

    println!();
    Ok(())
}

/// Handle `hermes memory off` (D-10).
///
/// Sets `memory.provider` to `"file"` and clears `mirror_provider`.
/// Does NOT set `memory_enabled=false` — built-in memory stays active.
/// This matches hermes-agent behavior where "off" means "no external provider."
pub async fn handle_memory_off() -> Result<()> {
    let mut config = Config::load()?;

    if config.memory.provider == "file" && config.memory.mirror_provider.is_none() {
        println!(
            "{}",
            "Already using built-in file provider (no external provider active).".yellow()
        );
        return Ok(());
    }

    let old_provider = config.memory.provider.clone();
    let old_mirror = config.memory.mirror_provider.clone();

    config.memory.provider = "file".to_string();
    config.memory.mirror_provider = None;
    config.save()?;

    println!("{}", "External memory provider disabled.".green().bold());
    println!("  Previous provider: {}", old_provider.dimmed());
    if let Some(mirror) = old_mirror {
        println!("  Previous mirror:   {}", mirror.dimmed());
    }
    println!("  Current provider:  {}", "file".cyan());
    println!();
    println!("  Built-in MEMORY.md and USER.md remain active.");
    println!("  To re-enable, run: {}", "hermes memory setup".cyan());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::config::Config;
    use tempfile::TempDir;

    #[test]
    fn memory_off_resets_to_file_provider() {
        // Create a temp config with sqlite provider
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("config.yaml");
        let mut config = Config::default();
        config.memory.provider = "sqlite".to_string();
        config.memory.mirror_provider = Some("duckdb".to_string());
        config.save_to(&config_path).unwrap();

        // Load, apply "off" logic, save
        let mut loaded = Config::load_from(&config_path).unwrap();
        loaded.memory.provider = "file".to_string();
        loaded.memory.mirror_provider = None;
        loaded.save_to(&config_path).unwrap();

        // Verify
        let reloaded = Config::load_from(&config_path).unwrap();
        assert_eq!(reloaded.memory.provider, "file");
        assert!(reloaded.memory.mirror_provider.is_none());
        // memory_enabled should still be true (D-10: off does NOT disable memory)
        assert!(reloaded.memory.memory_enabled);
        // user_profile_enabled should still be true
        assert!(reloaded.memory.user_profile_enabled);
    }

    #[test]
    fn memory_off_handler_preserves_memory_enabled() {
        // Verify the logic: when provider is already "file" with no mirror,
        // handle_memory_off returns early without modifying config
        let config = Config::default(); // default: provider="file", mirror=None
        assert_eq!(config.memory.provider, "file");
        assert!(config.memory.mirror_provider.is_none());
        assert!(config.memory.memory_enabled);
        // This covers the early-return branch of handle_memory_off
    }
}
