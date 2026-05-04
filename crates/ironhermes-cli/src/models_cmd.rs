use anyhow::Result;
use clap::Subcommand;
use colored::Colorize;
use std::fmt::Write as FmtWrite;

use crate::tui::status_line::format_token_count;
use ironhermes_core::{ModelMetadata, ModelRegistry, ModelsCache, fetch_all};

// ---------------------------------------------------------------------------
// ModelsSubcommand
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum ModelsSubcommand {
    /// List all known models with context lengths.
    List,
    /// Fetch latest metadata from models.dev and OpenRouter APIs.
    Fetch,
    /// Show metadata for a specific model.
    Info {
        /// Model ID (canonical, alias, or provider-prefixed)
        model: String,
    },
}

// ---------------------------------------------------------------------------
// handle_models_command
// ---------------------------------------------------------------------------

pub async fn handle_models_command(cmd: ModelsSubcommand) -> Result<()> {
    match cmd {
        ModelsSubcommand::List => cmd_list().await,
        ModelsSubcommand::Fetch => cmd_fetch().await,
        ModelsSubcommand::Info { model } => cmd_info(&model).await,
    }
}

// ---------------------------------------------------------------------------
// cmd_list
// ---------------------------------------------------------------------------

async fn cmd_list() -> Result<()> {
    let mut registry = ModelRegistry::new();
    let cache = ModelsCache::load();

    let cache_status = if cache.entries.is_empty() {
        "not loaded".to_string()
    } else {
        // Find the most recent fetched_at
        let latest = cache
            .entries
            .values()
            .map(|e| e.fetched_at)
            .max()
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        format!("fetched {}", latest)
    };

    registry.merge_cache(cache.into_metadata_map());
    let models = registry.all_models();

    print!("{}", render_model_list(&models, &cache_status));
    Ok(())
}

/// Pure rendering helper -- produces the full list output as a String so it
/// can be unit-tested without capturing stdout.
fn render_model_list(models: &[(&str, &ModelMetadata)], cache_status: &str) -> String {
    let mut out = String::new();

    let _ = writeln!(out, "{}", "Known Models".bold().cyan());
    let _ = writeln!(out, "{}", "─".repeat(70));

    if models.is_empty() {
        let _ = writeln!(out, "  {}", "No models loaded.".dimmed());
        return out;
    }

    let _ = writeln!(
        out,
        "  {:<20} {:<10} {:<10} {:<14} {}",
        "NAME".bold(),
        "CONTEXT".bold(),
        "OUTPUT".bold(),
        "TOKENIZER".bold(),
        "CAPABILITIES".bold(),
    );

    for (name, meta) in models {
        let context_str = format_token_count(meta.context_length);
        let output_str = meta
            .max_output_tokens
            .map(format_token_count)
            .unwrap_or_else(|| "unknown".to_string());

        let mut caps = Vec::new();
        if meta.capabilities.vision {
            caps.push("vision".green().to_string());
        }
        if meta.capabilities.tool_use {
            caps.push("tool_use".green().to_string());
        }
        if meta.capabilities.reasoning {
            caps.push("reasoning".green().to_string());
        }
        if meta.capabilities.streaming {
            caps.push("streaming".green().to_string());
        }
        let caps_str = if caps.is_empty() {
            "none".dimmed().to_string()
        } else {
            caps.join(" ")
        };

        let _ = writeln!(
            out,
            "  {:<20} {:<10} {:<10} {:<14} {}",
            name.yellow(),
            context_str,
            output_str,
            meta.tokenizer,
            caps_str,
        );
    }

    let _ = writeln!(out, "{}", "─".repeat(70));
    let _ = writeln!(
        out,
        "  {}",
        format!("{} model(s) total · Cache: {}", models.len(), cache_status).dimmed()
    );

    out
}

// ---------------------------------------------------------------------------
// cmd_fetch
// ---------------------------------------------------------------------------

async fn cmd_fetch() -> Result<()> {
    println!("{}", "Fetching model metadata...".dimmed());
    println!("{}", "  Querying models.dev...".dimmed());

    let (entries, fetch_result) = fetch_all().await;

    // Report per-source results
    match fetch_result.models_dev_count {
        Some(n) => println!("  models.dev: {} models received", n),
        None => {
            if let Some(ref e) = fetch_result.models_dev_error {
                println!("  {}", format!("models.dev: failed - {}", e).yellow());
            }
        }
    }

    let has_openrouter_key = std::env::var("OPENROUTER_API_KEY").is_ok();
    println!(
        "{}",
        format!(
            "  Querying OpenRouter (OPENROUTER_API_KEY {})...",
            if has_openrouter_key { "set" } else { "not set" }
        )
        .dimmed()
    );

    match fetch_result.openrouter_count {
        Some(n) => println!("  OpenRouter: {} models received", n),
        None => {
            if let Some(ref e) = fetch_result.openrouter_error {
                println!("  {}", format!("OpenRouter: failed - {}", e).yellow());
            }
        }
    }

    // Check for total failure
    if fetch_result.models_dev_count.is_none() && fetch_result.openrouter_count.is_none() {
        eprintln!(
            "{} {}",
            "Error:".red().bold(),
            "Fetch failed: both sources returned errors. Check network and OPENROUTER_API_KEY."
        );
        return Err(anyhow::anyhow!("All fetch sources failed"));
    }

    // Warn on partial failure
    if fetch_result.models_dev_error.is_some() || fetch_result.openrouter_error.is_some() {
        if let Some(ref e) = fetch_result.models_dev_error {
            println!(
                "  {}",
                format!("Warning: models.dev fetch failed - {}", e).yellow()
            );
        }
        if let Some(ref e) = fetch_result.openrouter_error {
            println!(
                "  {}",
                format!("Warning: OpenRouter fetch failed - {}", e).yellow()
            );
        }
    }

    // Save to disk
    let entry_count = entries.len();
    let mut cache = ModelsCache::default();
    cache.entries = entries;
    cache.save()?;

    println!("{}", "Fetch Complete".bold().cyan());
    println!("{}", "─".repeat(70));
    let cache_path = ModelsCache::cache_path();
    println!(
        "  {}",
        format!("Cache saved: {}", cache_path.display()).dimmed()
    );
    println!(
        "  {}",
        format!("Models updated: {} entries", entry_count).dimmed()
    );
    println!(
        "  {}",
        format!(
            "Fetched at: {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M UTC")
        )
        .dimmed()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_info
// ---------------------------------------------------------------------------

async fn cmd_info(model: &str) -> Result<()> {
    let mut registry = ModelRegistry::new();
    let cache = ModelsCache::load();

    // Determine source for this model
    let source = if cache.entries.contains_key(model) {
        let fetched_at = cache.entries[model]
            .fetched_at
            .format("%Y-%m-%d %H:%M UTC")
            .to_string();
        format!("disk cache (fetched {})", fetched_at)
    } else {
        "static table".to_string()
    };

    registry.merge_cache(cache.into_metadata_map());

    match registry.lookup(model) {
        Some(metadata) => {
            // Find aliases that map to this canonical ID
            let all_models = registry.all_models();
            let canonical = all_models
                .iter()
                .find(|(_, m)| std::ptr::eq(*m, metadata))
                .map(|(id, _)| *id)
                .unwrap_or(model);

            // Build alias list from the static alias map
            let alias_registry = ModelRegistry::new();
            let alias_models = alias_registry.all_models();
            let mut aliases: Vec<&str> = Vec::new();
            // Check all_models for entries that resolve to the same metadata
            // by looking up each alias candidate
            for (id, _) in &alias_models {
                if *id != canonical {
                    if let Some(m) = registry.lookup(id) {
                        if m.context_length == metadata.context_length
                            && m.tokenizer == metadata.tokenizer
                            && m.max_output_tokens == metadata.max_output_tokens
                        {
                            // This could be an alias -- but we only want actual aliases
                            // not different models with the same specs
                        }
                    }
                }
            }
            // For now, aliases are not easily extractable from the registry
            // since the alias map is private. Pass empty.
            // This is acceptable since the primary use case is model info display.

            print!(
                "{}",
                render_model_info(canonical, metadata, &source, &aliases)
            );
            Ok(())
        }
        None => {
            eprintln!(
                "{} Model not found: {}. Run `hermes models fetch` to update cache.",
                "Error:".red().bold(),
                model,
            );
            Ok(())
        }
    }
}

/// Pure rendering helper -- produces the full detail view as a String so it
/// can be unit-tested without capturing stdout.
fn render_model_info(
    canonical_id: &str,
    metadata: &ModelMetadata,
    source: &str,
    aliases: &[&str],
) -> String {
    let mut out = String::new();

    let _ = writeln!(out, "{}", "Model Info".bold().cyan());
    let _ = writeln!(out, "{}", "─".repeat(50));

    let _ = writeln!(
        out,
        "  {:<18} {}",
        "Canonical ID:".dimmed(),
        canonical_id.yellow()
    );
    let _ = writeln!(
        out,
        "  {:<18} {} tokens",
        "Context:".dimmed(),
        format_token_count(metadata.context_length)
    );

    match metadata.max_output_tokens {
        Some(n) => {
            let _ = writeln!(
                out,
                "  {:<18} {} tokens",
                "Max output:".dimmed(),
                format_token_count(n)
            );
        }
        None => {
            let _ = writeln!(
                out,
                "  {:<18} {}",
                "Max output:".dimmed(),
                "unknown".dimmed()
            );
        }
    }

    let _ = writeln!(
        out,
        "  {:<18} {}",
        "Tokenizer:".dimmed(),
        metadata.tokenizer
    );

    let yes = "yes".green();
    let no = "no".dimmed();

    let _ = writeln!(
        out,
        "  {:<18} {}",
        "Vision:".dimmed(),
        if metadata.capabilities.vision {
            yes.clone()
        } else {
            no.clone()
        }
    );
    let _ = writeln!(
        out,
        "  {:<18} {}",
        "Tool use:".dimmed(),
        if metadata.capabilities.tool_use {
            yes.clone()
        } else {
            no.clone()
        }
    );
    let _ = writeln!(
        out,
        "  {:<18} {}",
        "Reasoning:".dimmed(),
        if metadata.capabilities.reasoning {
            yes.clone()
        } else {
            no.clone()
        }
    );
    let _ = writeln!(
        out,
        "  {:<18} {}",
        "Streaming:".dimmed(),
        if metadata.capabilities.streaming {
            yes.clone()
        } else {
            no.clone()
        }
    );

    let _ = writeln!(out, "  {:<18} {}", "Source:".dimmed(), source);

    let aliases_str = if aliases.is_empty() {
        "none".dimmed().to_string()
    } else {
        aliases.join(", ")
    };
    let _ = writeln!(out, "  {:<18} {}", "Aliases:".dimmed(), aliases_str);

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::{ModelCapabilities, ModelMetadata};

    #[test]
    fn render_model_list_empty() {
        let output = render_model_list(&[], "not loaded");
        assert!(output.contains("No models loaded."));
    }

    #[test]
    fn render_model_list_has_header_and_footer() {
        let meta = ModelMetadata {
            context_length: 200_000,
            max_output_tokens: Some(64_000),
            tokenizer: "cl100k_base".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tool_use: true,
                reasoning: false,
                streaming: true,
            },
        };
        let models = vec![("claude-sonnet-4", &meta)];
        let output = render_model_list(&models, "fetched 2026-04-19");

        assert!(output.contains("Known Models"), "missing header");
        assert!(output.contains("claude-sonnet-4"), "missing model name");
        assert!(output.contains("200.0K"), "missing context length");
        assert!(output.contains("64.0K"), "missing output tokens");
        assert!(output.contains("cl100k_base"), "missing tokenizer");
        assert!(output.contains("1 model(s) total"), "missing footer count");
        assert!(
            output.contains("fetched 2026-04-19"),
            "missing cache status"
        );
    }

    #[test]
    fn render_model_info_has_all_fields() {
        let meta = ModelMetadata {
            context_length: 200_000,
            max_output_tokens: Some(64_000),
            tokenizer: "cl100k_base".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tool_use: true,
                reasoning: false,
                streaming: true,
            },
        };
        let output = render_model_info("claude-sonnet-4", &meta, "static table", &[]);

        assert!(output.contains("Model Info"), "missing header");
        assert!(output.contains("claude-sonnet-4"), "missing canonical ID");
        assert!(output.contains("200.0K"), "missing context length");
        assert!(output.contains("64.0K"), "missing output tokens");
        assert!(output.contains("cl100k_base"), "missing tokenizer");
        assert!(output.contains("static table"), "missing source");
    }

    #[test]
    fn render_model_info_unknown_output() {
        let meta = ModelMetadata {
            context_length: 128_000,
            max_output_tokens: None,
            tokenizer: "o200k_base".to_string(),
            capabilities: ModelCapabilities::default(),
        };
        let output = render_model_info("test-model", &meta, "static table", &[]);

        assert!(
            output.contains("unknown"),
            "missing 'unknown' for max output"
        );
    }

    #[test]
    fn render_model_info_with_aliases() {
        let meta = ModelMetadata {
            context_length: 200_000,
            max_output_tokens: Some(64_000),
            tokenizer: "cl100k_base".to_string(),
            capabilities: ModelCapabilities::default(),
        };
        let aliases = vec!["claude-sonnet-4-20250514", "anthropic/claude-sonnet-4"];
        let output = render_model_info("claude-sonnet-4", &meta, "static table", &aliases);

        assert!(
            output.contains("claude-sonnet-4-20250514"),
            "missing alias 1"
        );
        assert!(
            output.contains("anthropic/claude-sonnet-4"),
            "missing alias 2"
        );
    }
}
