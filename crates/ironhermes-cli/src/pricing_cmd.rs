// Phase 36.2 Plan 09 — `hermes pricing` CLI subcommand.
//
// Mirrors `models_cmd.rs:1-36` shape. `cmd_list` merges the bundled
// pricing.toml with the on-disk cache and renders a table. `cmd_refresh`
// fetches from models.dev (compile-time-hardcoded URL — threat T-36.2-09-URL
// mitigation), validates the response is non-empty (unless --force), and
// writes `$HERMES_HOME/pricing-cache.json` only on success.

use anyhow::Result;
use clap::Subcommand;
use colored::Colorize;
use std::fmt::Write as FmtWrite;
use std::path::Path;

use ironhermes_core::pricing_cache::{
    PricingCache, PricingCacheEntry, fetch_from_url,
};
use ironhermes_core::{PricingEntry, PricingRegistry};

// ---------------------------------------------------------------------------
// PricingSubcommand
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug, Clone)]
pub enum PricingSubcommand {
    /// Show current pricing table (static table + cache overlay merged).
    List,
    /// Fetch latest pricing from models.dev; write $HERMES_HOME/pricing-cache.json.
    Refresh {
        /// Overwrite cache even if the source returns a partial response.
        /// Without --force, partial responses (zero parsed entries) are rejected.
        #[arg(long)]
        force: bool,
    },
}

// ---------------------------------------------------------------------------
// handle_pricing_command — top-level routing
// ---------------------------------------------------------------------------

pub async fn handle_pricing_command(cmd: PricingSubcommand) -> Result<()> {
    match cmd {
        PricingSubcommand::List => cmd_list().await,
        PricingSubcommand::Refresh { force } => cmd_refresh(force).await,
    }
}

// ---------------------------------------------------------------------------
// cmd_list — print the merged pricing table
// ---------------------------------------------------------------------------

async fn cmd_list() -> Result<()> {
    let path = PricingCache::cache_path();
    let output = cmd_list_to_string(&path);
    print!("{output}");
    Ok(())
}

/// Pure-formatter variant used by integration tests. Reads the bundled
/// static table, merges in any disk-cache entries from `path`, and returns
/// the full table as a `String`. Never touches stdout.
pub fn cmd_list_to_string(cache_path: &Path) -> String {
    let mut registry = PricingRegistry::new();
    let cache = PricingCache::load_from(cache_path);
    let cache_status = if cache.entries.is_empty() {
        "not loaded".to_string()
    } else {
        let latest = cache
            .entries
            .values()
            .map(|e| e.fetched_at)
            .max()
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        format!("fetched {latest}")
    };
    registry.merge_cache(cache.into_pricing_map());
    let models = registry.all_models();
    render_pricing_list(&models, &cache_status)
}

/// Render the pricing table as a String — testable without stdout capture.
fn render_pricing_list(models: &[(&str, &PricingEntry)], cache_status: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", "Pricing Table".bold().cyan());
    let _ = writeln!(out, "{}", "─".repeat(96));

    if models.is_empty() {
        let _ = writeln!(out, "  {}", "No pricing entries loaded.".dimmed());
        return out;
    }

    let _ = writeln!(
        out,
        "  {:<32} {:<14} {:>10} {:>10} {:>12} {:>14}",
        "MODEL".bold(),
        "PROVIDER".bold(),
        "IN/1M".bold(),
        "OUT/1M".bold(),
        "CACHE_R/1M".bold(),
        "CACHE_CRE/1M".bold(),
    );

    for (model, entry) in models {
        let _ = writeln!(
            out,
            "  {:<32} {:<14} {:>10} {:>10} {:>12} {:>14}",
            model.yellow(),
            entry.provider,
            format_micros_as_dollars(entry.input_per_1m_micros),
            format_micros_as_dollars(entry.output_per_1m_micros),
            format_micros_as_dollars(entry.cache_read_per_1m_micros),
            format_micros_as_dollars(entry.cache_creation_per_1m_micros),
        );
    }

    let _ = writeln!(out, "{}", "─".repeat(96));
    let _ = writeln!(
        out,
        "  {}",
        format!("{} model(s) total · Cache: {}", models.len(), cache_status).dimmed()
    );
    out
}

/// Format an i64 micro-USD value as `$X.XX`. Display-time only — micro-USD
/// integers stay the canonical storage form (RESEARCH.md Pitfall 5).
fn format_micros_as_dollars(micros: i64) -> String {
    let dollars = micros / 1_000_000;
    let cents = (micros % 1_000_000) / 10_000; // two-decimal display
    format!("${dollars}.{cents:02}")
}

// ---------------------------------------------------------------------------
// cmd_refresh — fetch and persist
// ---------------------------------------------------------------------------

/// Exit code when the HTTP fetch itself fails (network / DNS / non-2xx).
const EXIT_FETCH_FAILED: i32 = 2;
/// Exit code when the response was reachable but the parsed body is empty
/// AND `--force` was not supplied.
const EXIT_EMPTY_REJECTED: i32 = 3;

async fn cmd_refresh(force: bool) -> Result<()> {
    let cache_path = PricingCache::cache_path();
    match cmd_refresh_from_url_with_path(
        ironhermes_core::pricing_cache::MODELS_DEV_URL,
        force,
        &cache_path,
    )
    .await
    {
        Ok(count) => {
            println!(
                "hermes pricing refresh: wrote {count} entries to {}",
                cache_path.display()
            );
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            // Surface the distinction between fetch-failed and empty-rejected
            // so operators can tell at a glance which path tripped.
            if msg.contains("EMPTY-REJECTED:") {
                eprintln!(
                    "hermes pricing refresh: source returned 0 entries (suspicious schema)."
                );
                eprintln!("Re-run with --force to overwrite cache anyway.");
                std::process::exit(EXIT_EMPTY_REJECTED);
            } else {
                eprintln!("hermes pricing refresh: fetch failed — {msg}");
                eprintln!(
                    "Existing {} was NOT overwritten.",
                    cache_path.display()
                );
                std::process::exit(EXIT_FETCH_FAILED);
            }
        }
    }
}

/// Test-injection seam: drive the full refresh path against an arbitrary URL
/// and write to a caller-supplied cache file. Used by `tests/pricing_cli.rs`
/// to wiremock the upstream API and confirm:
/// - empty-without-force preserves the existing file byte-for-byte
/// - --force overwrites with whatever was returned
/// - network failure preserves the file byte-for-byte
///
/// Production code calls this via [`cmd_refresh`] with the hardcoded
/// `MODELS_DEV_URL` and `PricingCache::cache_path()`.
///
/// Returns the number of entries written on success. Returns `Err` (without
/// exiting) on either fetch failure or empty-without-force, so tests can
/// observe the error without process-exit semantics.
pub async fn cmd_refresh_from_url_with_path(
    url: &str,
    force: bool,
    cache_path: &Path,
) -> Result<usize> {
    let fetched = fetch_from_url(url).await?;
    if fetched.is_empty() && !force {
        anyhow::bail!(
            "EMPTY-REJECTED: models.dev returned 0 entries; re-run with --force to overwrite"
        );
    }
    let count = fetched.len();
    let cache = build_cache(fetched);
    cache.save_to(cache_path)?;
    Ok(count)
}

fn build_cache(entries: std::collections::HashMap<String, PricingCacheEntry>) -> PricingCache {
    PricingCache { entries }
}

// ---------------------------------------------------------------------------
// Tests (unit — full integration coverage lives in tests/pricing_cli.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_micros_as_dollars_basic() {
        assert_eq!(format_micros_as_dollars(5_000_000), "$5.00");
        assert_eq!(format_micros_as_dollars(15_000_000), "$15.00");
        assert_eq!(format_micros_as_dollars(500_000), "$0.50");
        assert_eq!(format_micros_as_dollars(0), "$0.00");
    }

    #[test]
    fn render_pricing_list_empty() {
        let out = render_pricing_list(&[], "not loaded");
        assert!(out.contains("No pricing entries loaded."));
    }

    #[test]
    fn render_pricing_list_has_header_and_row() {
        let entry = PricingEntry {
            provider: "anthropic".to_string(),
            input_per_1m_micros: 5_000_000,
            output_per_1m_micros: 25_000_000,
            cache_read_per_1m_micros: 500_000,
            cache_creation_per_1m_micros: 6_250_000,
        };
        let rows = vec![("claude-opus-4-7", &entry)];
        let out = render_pricing_list(&rows, "fetched 2026-05-25");
        assert!(out.contains("Pricing Table"));
        assert!(out.contains("claude-opus-4-7"));
        assert!(out.contains("anthropic"));
        assert!(out.contains("$5.00"));
        assert!(out.contains("$25.00"));
        assert!(out.contains("1 model(s) total"));
        assert!(out.contains("fetched 2026-05-25"));
    }
}
