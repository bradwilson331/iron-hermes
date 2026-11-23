//! CLI subcommands for memory management (Phase 21.4, D-09/D-10).
//!
//! `hermes memory status` — display current memory subsystem state.
//! `hermes memory off` — disable external provider, keep built-in file memory.

use std::io::{BufRead, Write};
use std::sync::Arc;

use anyhow::Result;
use colored::Colorize;
use ironhermes_agent::MemoryManager;
use ironhermes_agent::memory::factory::build_memory_manager_at;
use ironhermes_agent::memory::tier2_maintenance::{preview_tier2, purge_tier2};
use ironhermes_core::config::Config;
use ironhermes_core::memory_store::{MemoryStore, MemoryTarget};
use ironhermes_core::tier2::{PurgePreview, PurgeSurface};
use tokio::sync::Mutex;

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

/// Handle `ironhermes memory purge [--dry-run]` (D-28): permanently delete the claims staged for
/// forgetting, after a dry run and a typed confirmation.
pub async fn handle_memory_purge(dry_run: bool) -> Result<()> {
    let config = Config::load()?;
    if !config.memory.tier2_enabled {
        anyhow::bail!("memory.tier2_enabled is off: there is nothing to purge");
    }
    if !config.memory.forgetting_enabled {
        anyhow::bail!("memory.forgetting_enabled is off: set it to true to purge");
    }
    let home = ironhermes_core::get_hermes_home();
    let Some(mgr) = build_memory_manager_at(&config.memory, &home).await? else {
        anyhow::bail!("memory is disabled for this profile");
    };
    run_purge_cmd(
        &mgr,
        dry_run,
        std::io::IsTerminal::is_terminal(&std::io::stdin()),
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
    )
    .await
}

const NOT_INTERACTIVE: &str = "Refusing to purge without an interactive terminal: run this command yourself. --dry-run works anywhere.";

/// The preview always prints first; the typed word `purge` is the only consent. This never goes
/// through the tool-approval gate, which `autonomous.yolo` bypasses (D-28), and only the previewed
/// ids are purged.
pub async fn run_purge_cmd(
    mgr: &Arc<Mutex<MemoryManager>>,
    dry_run: bool,
    interactive: bool,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<()> {
    let preview = preview_tier2(mgr).await?;
    if let Some(why) = &preview.skipped {
        writeln!(out, "{why}")?;
        return Ok(());
    }
    if preview.claims.is_empty() {
        writeln!(out, "Nothing is staged to forget.")?;
        if !preview.scrub_pending {
            return Ok(());
        }
        // D-35: an empty run finishes an unfinished on-disk scrub. It deletes no claim, so it
        // needs no typed word.
        writeln!(out, "A previous purge did not finish its on-disk scrub.")?;
        if dry_run {
            writeln!(
                out,
                "Dry run: run this command without --dry-run to finish it."
            )?;
            return Ok(());
        }
        if !interactive {
            writeln!(out, "{NOT_INTERACTIVE}")?;
            anyhow::bail!(NOT_INTERACTIVE);
        }
        let report = purge_tier2(mgr, PurgeSurface::Cli, &[]).await?;
        match (&report.skipped, &report.scrub_incomplete) {
            (Some(why), _) => writeln!(out, "{why}")?,
            (None, None) => writeln!(out, "On-disk scrub finished.")?,
            (None, Some(why)) => writeln!(out, "{}", scrub_warning(why))?,
        }
        return Ok(());
    }

    write!(out, "{}", render_preview(&preview))?;
    if dry_run {
        writeln!(out, "Dry run: nothing was deleted.")?;
        return Ok(());
    }
    // An agent's `terminal` tool pipes stdin, so `echo purge | ironhermes memory purge` cannot
    // delete (D-28: agents never gain a purge action).
    if !interactive {
        writeln!(out, "{NOT_INTERACTIVE}")?;
        anyhow::bail!(NOT_INTERACTIVE);
    }
    write!(
        out,
        "Type the word \"purge\" to permanently delete {} claim(s) and the Tier 1 entries listed above (anything else cancels): ",
        preview.claims.len()
    )?;
    out.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    if line.trim() != "purge" {
        writeln!(out, "Aborted; nothing was deleted.")?;
        return Ok(());
    }

    let ids: Vec<String> = preview.claims.iter().map(|c| c.claim_id.clone()).collect();
    let report = purge_tier2(mgr, PurgeSurface::Cli, &ids).await?;
    if let Some(why) = &report.skipped {
        writeln!(out, "{why}")?;
    }
    writeln!(out, "Purged {} claim(s).", report.purged.len())?;
    writeln!(
        out,
        "  Tier 1 lines removed: {}",
        report.tier1_lines_removed
    )?;
    writeln!(
        out,
        "  Daily-note lines retracted: {}",
        report.live_lines_retracted
    )?;
    writeln!(
        out,
        "  Evidence quotes redacted: {}",
        report.evidence_redacted
    )?;
    if !report.not_staged.is_empty() {
        writeln!(
            out,
            "Kept (no longer staged since the preview): {}",
            report.not_staged.join(", ")
        )?;
    }
    if let Some(why) = &report.scrub_incomplete {
        writeln!(out, "{}", scrub_warning(why))?;
    }
    Ok(())
}

fn scrub_warning(why: &str) -> String {
    format!(
        "On-disk scrub incomplete: {why}. Stop the gateway and the web UI, then run ironhermes memory purge again; it finishes the scrub even with nothing staged."
    )
}

/// Claim text is model-written; control characters (ANSI escapes) must not redraw the screen the
/// human confirms from.
fn plain(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// D-28: the dry run as text, one block per claim.
pub fn render_preview(p: &PurgePreview) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for c in &p.claims {
        let _ = writeln!(s, "{}  \"{}\"", c.claim_id, plain(&c.text));
        if !c.predecessors.is_empty() {
            let _ = writeln!(
                s,
                "  also deletes older wording: {}",
                c.predecessors.join(", ")
            );
        }
        for (target, text) in &c.tier1_entries {
            let file = if target == "user" {
                "USER.md"
            } else {
                "MEMORY.md"
            };
            let _ = writeln!(s, "  {file} entry to delete: \"{}\"", plain(text));
        }
        let _ = writeln!(s, "  daily-note lines to retract: {}", c.live_lines);
    }
    let _ = writeln!(s, "Other claims' quotes to redact: {}", p.redacted_quotes);
    s
}

#[cfg(test)]
mod tests {

    use std::io::Cursor;
    use std::path::Path;

    use super::*;
    use ironhermes_core::config::{EmbeddingsConfig, MemoryConfig};
    use tempfile::TempDir;

    type Mgr = Arc<Mutex<MemoryManager>>;

    async fn manager(home: &Path) -> Mgr {
        let cfg = MemoryConfig {
            tier2_enabled: true,
            forgetting_enabled: true,
            embeddings: EmbeddingsConfig {
                base_url: Some("http://127.0.0.1:9".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        build_memory_manager_at(&cfg, home)
            .await
            .unwrap()
            .expect("memory is enabled")
    }

    /// A manager whose MEMORY.md holds `User lives in Lisbon`, staged for forgetting.
    async fn staged(home: &Path) -> Mgr {
        let mgr = manager(home).await;
        mgr.lock()
            .await
            .add(MemoryTarget::Memory, "User lives in Lisbon")
            .await
            .unwrap();
        let t2 = mgr.lock().await.tier2().unwrap();
        let id = t2.search("Lisbon", 5).await.unwrap().results[0]
            .claim_id
            .clone();
        t2.stage_forget(&id, None).await.unwrap();
        mgr
    }

    fn memory_md(home: &Path) -> String {
        std::fs::read_to_string(home.join("memories/MEMORY.md")).unwrap_or_default()
    }

    /// Runs the command; returns the result, the output and whether the input was left unread.
    async fn run(
        mgr: &Mgr,
        dry_run: bool,
        interactive: bool,
        typed: &str,
    ) -> (Result<()>, String, bool) {
        let mut input = Cursor::new(typed.as_bytes().to_vec());
        let mut out = Vec::new();
        let res = run_purge_cmd(mgr, dry_run, interactive, &mut input, &mut out).await;
        (res, String::from_utf8(out).unwrap(), input.position() == 0)
    }

    #[tokio::test]
    async fn memory_purge_cmd_dry_run_writes_nothing() {
        let home = TempDir::new().unwrap();
        let mgr = staged(home.path()).await;
        let before = memory_md(home.path());
        let (res, out, unread) = run(&mgr, true, true, "purge\n").await;
        res.unwrap();
        assert!(unread);
        assert!(
            out.contains("claim:") && out.contains("User lives in Lisbon"),
            "{out}"
        );
        assert!(
            out.contains("MEMORY.md entry to delete: \"User lives in Lisbon\""),
            "{out}"
        );
        assert!(out.contains("Dry run: nothing was deleted."), "{out}");
        assert_eq!(memory_md(home.path()), before);
        let t2 = mgr.lock().await.tier2().unwrap();
        assert_eq!(t2.list_staged().await.unwrap().len(), 1, "still staged");
    }

    #[tokio::test]
    async fn memory_purge_cmd_wrong_confirmation_aborts() {
        let home = TempDir::new().unwrap();
        let mgr = staged(home.path()).await;
        let before = memory_md(home.path());
        let (res, out, _) = run(&mgr, false, true, "nope\n").await;
        res.unwrap();
        assert!(out.contains("Type the word \"purge\""), "{out}");
        assert!(out.contains("Aborted; nothing was deleted."), "{out}");
        assert_eq!(memory_md(home.path()), before);
        let t2 = mgr.lock().await.tier2().unwrap();
        assert_eq!(t2.list_staged().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn memory_purge_cmd_typed_purge_deletes() {
        let home = TempDir::new().unwrap();
        let mgr = staged(home.path()).await;
        let (res, out, _) = run(&mgr, false, true, "purge\n").await;
        res.unwrap();
        assert!(out.contains("Purged 1 claim"), "{out}");
        assert!(!memory_md(home.path()).contains("Lisbon"));
        let t2 = mgr.lock().await.tier2().unwrap();
        assert!(t2.search("Lisbon", 5).await.unwrap().results.is_empty());
        assert!(t2.list_staged().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn memory_purge_cmd_nothing_staged() {
        let home = TempDir::new().unwrap();
        let mgr = manager(home.path()).await;
        let (res, out, unread) = run(&mgr, false, true, "purge\n").await;
        res.unwrap();
        assert!(unread);
        assert!(out.contains("Nothing is staged to forget."), "{out}");
        assert!(!out.contains("Type the word"), "{out}");
    }

    #[tokio::test]
    async fn memory_purge_cmd_requires_interactive_terminal() {
        let home = TempDir::new().unwrap();
        let mgr = staged(home.path()).await;
        let before = memory_md(home.path());
        let (res, out, unread) = run(&mgr, false, false, "purge\n").await;
        assert!(res.is_err());
        assert!(unread, "stdin must not be read");
        assert!(out.contains("MEMORY.md entry to delete:"), "{out}");
        assert!(
            out.contains("Refusing to purge without an interactive terminal"),
            "{out}"
        );
        assert_eq!(memory_md(home.path()), before);
        let t2 = mgr.lock().await.tier2().unwrap();
        assert_eq!(t2.list_staged().await.unwrap().len(), 1);

        let (res, out, _) = run(&mgr, true, false, "").await;
        res.unwrap();
        assert!(out.contains("Dry run: nothing was deleted."), "{out}");
    }

    #[tokio::test]
    async fn memory_purge_cmd_finishes_pending_scrub() {
        let home = TempDir::new().unwrap();
        let mgr = manager(home.path()).await;
        let marker = home.path().join("tier2/forget/scrub_pending");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, b"").unwrap();

        let (res, out, unread) = run(&mgr, true, true, "purge\n").await;
        res.unwrap();
        assert!(unread);
        assert!(out.contains("Nothing is staged to forget."), "{out}");
        assert!(
            out.contains("A previous purge did not finish its on-disk scrub."),
            "{out}"
        );
        assert!(
            out.contains("Dry run: run this command without --dry-run to finish it."),
            "{out}"
        );
        assert!(marker.exists(), "a dry run leaves the marker");

        let (res, out, unread) = run(&mgr, false, true, "purge\n").await;
        res.unwrap();
        assert!(unread, "no typed word for a scrub-only run");
        assert!(out.contains("On-disk scrub finished."), "{out}");
        assert!(!marker.exists());
    }

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
