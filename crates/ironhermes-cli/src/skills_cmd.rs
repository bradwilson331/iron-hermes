//! `hermes skills` subcommand tree — install / search / update / uninstall / list / trust.
//!
//! Exposes the Hub to users per D-14.  All mutation surfaces live here;
//! the agent gets only the read-only `hub_search` tool action (see ironhermes-tools).
//!
//! Security mitigations:
//! - T-19.1-05-01: save_config_atomic uses tmp + rename (never write-in-place)
//! - T-19.1-05-02: NO hub_install tool action; mutations are CLI-only (D-13)
//! - T-19.1-05-06: cmd_install picks adapter by identifier prefix; unknown formats
//!   return InvalidIdentifier early before any network call.

use clap::{Subcommand, ValueEnum};
use ironhermes_core::Config;
use ironhermes_hub::{
    CoreSkillScanner, GitHubAuth, GitHubSource, GitHubTap, HubSource, SkillLock,
    SkillsShBlobSource, WellKnownSkillSource, install as hub_install, migrate_from_hub_manifest,
    strip_terminal_escapes, uninstall as hub_uninstall, update as hub_update,
};
use std::sync::Arc;

/// Return a terminal-safe version of an error string for display at a print
/// boundary (stderr/stdout). Thin SP-10 wrapper around `strip_terminal_escapes`
/// exposed as a pub helper for unit-testing the D-16 contract without
/// capturing process stderr.
pub fn format_error_clean(msg: &str) -> String {
    strip_terminal_escapes(msg)
}

// ---------------------------------------------------------------------------
// Clap enums
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum SkillsAction {
    /// Install a skill from the Hub.
    Install {
        identifier: String,
        /// Skip confirmation prompts (reserved for future interactive paths).
        #[arg(long)]
        yes: bool,
        /// Skip the pre-install audit endpoint call (D-19). Useful for air-gapped
        /// environments or when the audit endpoint is degraded.
        #[arg(long)]
        skip_audit: bool,
    },
    /// Search configured adapters for matching skills.
    Search {
        query: String,
        #[arg(long)]
        source: Option<SourceFlag>,
        #[arg(long, default_value = "text")]
        format: Format,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Update one installed skill (or all if no name given).
    Update { name: Option<String> },
    /// Remove an installed skill by name. `uninstall` is accepted as an alias
    /// for one release cycle per D-04.
    #[command(alias = "uninstall")]
    Remove { name: String },
    /// List installed skills with source and trust level.
    List {
        #[arg(long, default_value = "text")]
        format: Format,
    },
    /// Manage the hub.trusted_repos allowlist.
    Trust {
        #[command(subcommand)]
        action: TrustAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum TrustAction {
    /// Add a repo to the trusted allowlist.
    Add { repo: String },
    /// Remove a repo from the trusted allowlist.
    Remove { repo: String },
    /// List the current trusted repos.
    List {
        #[arg(long, default_value = "text")]
        format: Format,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceFlag {
    Github,
    #[value(name = "well-known")]
    WellKnown,
    #[value(name = "skills-sh")]
    SkillsSh,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

// ---------------------------------------------------------------------------
// Config I/O helpers
// ---------------------------------------------------------------------------

pub fn load_config(path: &std::path::Path) -> anyhow::Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let s = std::fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&s)?)
}

// Public alias for tests
pub use load_config as load_config_for_test;

pub fn save_config_atomic(path: &std::path::Path, cfg: &Config) -> anyhow::Result<()> {
    let s = serde_yaml::to_string(cfg)?;
    let tmp = path.with_extension("yaml.tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tmp, s)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Source builder
// ---------------------------------------------------------------------------

async fn build_sources(cfg: &Config) -> Vec<Box<dyn HubSource + Send + Sync>> {
    let auth = GitHubAuth::resolve(cfg.skills.hub.github_token_env.as_deref()).await;
    let trusted = cfg.skills.hub.trusted_repos_set();
    let extra_taps: Vec<GitHubTap> = cfg
        .skills
        .hub
        .extra_taps
        .iter()
        .map(|t| GitHubTap {
            repo: t.repo.clone(),
            path_prefix: t.path.clone(),
        })
        .collect();

    let gh = Arc::new(GitHubSource::new(auth, trusted, extra_taps));
    let wk = WellKnownSkillSource::new(cfg.skills.hub.well_known_origins.clone());
    let sh = SkillsShBlobSource::new(gh.clone());

    vec![Box::new(SharedGitHubSource(gh)), Box::new(wk), Box::new(sh)]
}

// ---------------------------------------------------------------------------
// SharedGitHubSource — thin Arc wrapper so we can Box<dyn HubSource> without
// requiring Clone on GitHubSource (which holds a reqwest::Client).
// ---------------------------------------------------------------------------

struct SharedGitHubSource(Arc<GitHubSource>);

#[async_trait::async_trait]
impl HubSource for SharedGitHubSource {
    fn source_id(&self) -> &str {
        self.0.source_id()
    }

    fn trust_level_for(&self, identifier: &str) -> ironhermes_core::SkillSource {
        self.0.trust_level_for(identifier)
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ironhermes_hub::SkillMeta>, ironhermes_hub::HubError> {
        self.0.search(query, limit).await
    }

    async fn fetch(
        &self,
        identifier: &str,
    ) -> Result<ironhermes_hub::SkillBundle, ironhermes_hub::HubError> {
        self.0.fetch(identifier).await
    }
}

// ---------------------------------------------------------------------------
// Trust level string helper
// ---------------------------------------------------------------------------

pub fn trust_level_str(s: ironhermes_core::SkillSource) -> &'static str {
    match s {
        ironhermes_core::SkillSource::Builtin => "builtin",
        ironhermes_core::SkillSource::Official => "official",
        ironhermes_core::SkillSource::Trusted => "trusted",
        ironhermes_core::SkillSource::Community => "community",
    }
}

// ---------------------------------------------------------------------------
// Trust recomputation helper (D-08: never frozen in manifest)
// ---------------------------------------------------------------------------

/// Recompute the trust level string for a manifest entry from live config.
///
/// IMPORTANT: This must stay in sync with `resolve_source` in
/// `ironhermes-core/src/skills.rs` and the adapters' `trust_level_for` methods
/// (`GitHubSource`, `WellKnownSkillSource`, `SkillsShBlobSource`).
fn recompute_trust_str(
    source: &str,
    identifier: &str,
    trusted_set: &std::collections::HashSet<String>,
) -> &'static str {
    if source == "github" {
        let owner_repo = identifier
            .splitn(3, '/')
            .take(2)
            .collect::<Vec<_>>()
            .join("/");
        if trusted_set.contains(&owner_repo) {
            "trusted"
        } else {
            "community"
        }
    } else if source == "well-known" || source == "skills-sh" {
        "community"
    } else if source == "local-dir" {
        "trusted" // D-B2: local installs are always Trusted
    } else {
        "builtin"
    }
}

// ---------------------------------------------------------------------------
// Command handlers — public for lib-level (test) access
// ---------------------------------------------------------------------------

/// Install a skill by identifier.
///
/// Identifier routing:
///   "well-known:..."   → WellKnownSkillSource
///   "skills-sh:..."    → SkillsShBlobSource
///   otherwise          → GitHubSource (owner/repo/... format)
///
/// Emits the D-21 5-line progress shape on stdout for successful installs, and
/// the D-23 restart message on success. Every server-originated string printed
/// to the terminal passes through `strip_terminal_escapes` (D-16 / SP-10).
pub async fn cmd_install(cfg: &Config, identifier: &str, skip_audit: bool) -> anyhow::Result<i32> {
    // D-15: idempotent one-shot 19.1 -> 21.8 migration at first skills command
    // invocation. Swallow errors — migration is best-effort and must never block.
    let _ = migrate_from_hub_manifest();

    let skills_root = ironhermes_hub::paths::skills_root()
        .map_err(|e| anyhow::anyhow!("cannot resolve skills root: {}", e))?;
    std::fs::create_dir_all(&skills_root)?;

    // D-21 line 1: Resolving.
    println!(
        "Resolving skills.sh/{}...",
        strip_terminal_escapes(identifier)
    );

    let sources = build_sources(cfg).await;

    // D-D1 (Phase 21.8.1): pre-dispatch path-shape probe.
    // Fire ONLY when no recognized prefix matched. The hint never re-routes to
    // a source — it surfaces the user-facing fix and exits 1. False positive
    // for owner/repo identifiers that happen to also exist as a directory in
    // the user's CWD is accepted per CONTEXT.md D-D1.
    let has_known_prefix = identifier.starts_with("well-known:")
        || identifier.starts_with("skills-sh:")
        || identifier.starts_with("local:");
    if !has_known_prefix {
        let looks_like_path = looks_like_local_path_with_probe(identifier, |id| {
            std::fs::metadata(id).map(|m| m.is_dir()).unwrap_or(false)
        });
        if looks_like_path {
            // D-16 carry-forward: even our own strings traverse the print boundary
            // through strip_terminal_escapes so the contract is structural.
            eprintln!(
                "error: {} looks like a local path — did you mean 'hermes skills install local:{}'?",
                strip_terminal_escapes(identifier),
                strip_terminal_escapes(identifier),
            );
            return Ok(1);
        }
    }

    let source: &(dyn HubSource + Send + Sync) = if identifier.starts_with("well-known:") {
        // find the WellKnownSkillSource box
        sources
            .iter()
            .find(|s| s.source_id() == "well-known")
            .map(|s| s.as_ref())
            .ok_or_else(|| anyhow::anyhow!("well-known source not available"))?
    } else if identifier.starts_with("skills-sh:") {
        sources
            .iter()
            .find(|s| s.source_id() == "skills-sh")
            .map(|s| s.as_ref())
            .ok_or_else(|| anyhow::anyhow!("skills-sh source not available"))?
    } else if identifier.contains('/') {
        // GitHub: owner/repo/... format
        sources
            .iter()
            .find(|s| s.source_id() == "github")
            .map(|s| s.as_ref())
            .ok_or_else(|| anyhow::anyhow!("github source not available"))?
    } else {
        // Bare name (e.g. "ascii-art") — resolve via skills-sh registry
        sources
            .iter()
            .find(|s| s.source_id() == "skills-sh")
            .map(|s| s.as_ref())
            .ok_or_else(|| anyhow::anyhow!(
                "skills-sh source not available — try 'skills-sh:{identifier}' or 'owner/repo/path' format"
            ))?
    };

    // D-21 line 2: Discovering. For bare-name/skills-sh routing we don't know
    // the owner/repo upfront — the blob adapter resolves it inside fetch().
    // Emit a generic Discovering line so the 5-line shape is preserved; richer
    // owner/repo discovery is plumbed via the adapter's tracing::info span.
    println!(
        "Discovering skills in {}...",
        strip_terminal_escapes(identifier)
    );

    // D-21 line 3: Downloading. We don't have the byte count upfront without a
    // size-probe; emit a non-committal form so the shape stays consistent.
    println!("Downloading {size} bytes...", size = 0);

    // D-21 line 4: Scanning. The scan runs inside hub_install between
    // quarantine and atomic rename; print right before invoking it.
    println!("Scanning for threats...");

    let scanner = CoreSkillScanner;
    match hub_install(source, identifier, &scanner, &skills_root, skip_audit).await {
        Ok(outcome) => {
            // D-21 line 5: final install line (trust + 12-char hash prefix).
            let name = strip_terminal_escapes(&outcome.name);
            let short = outcome
                .content_hash
                .get(..12)
                .unwrap_or(&outcome.content_hash)
                .to_string();
            println!(
                "Installed '{name}' [{trust}] — hash: {short}",
                name = name,
                trust = trust_level_str(outcome.trust_level),
                short = short,
            );
            // D-23: restart message on separate stdout line.
            println!(
                "Installed. Restart the agent or start a new session to use {name}.",
                name = name,
            );
            Ok(0)
        }
        Err(e) => {
            // D-16: every server-originated error passes through strip_terminal_escapes.
            eprintln!("error: {}", strip_terminal_escapes(&e.to_string()));
            Ok(1)
        }
    }
}

/// Search all (or one) configured adapter(s) for skills matching `query`.
pub async fn cmd_search(
    cfg: &Config,
    query: &str,
    source: Option<SourceFlag>,
    format: Format,
    limit: usize,
) -> anyhow::Result<i32> {
    let output = cmd_search_impl(cfg, query, source, format, limit).await;
    println!("{}", output);
    Ok(0)
}

/// Inner search that returns a String (for testing without printing).
pub async fn cmd_search_impl(
    cfg: &Config,
    query: &str,
    source_filter: Option<SourceFlag>,
    format: Format,
    limit: usize,
) -> String {
    let sources = build_sources(cfg).await;
    const HARD_CAP: usize = 20;
    let effective_limit = limit.min(HARD_CAP);

    let mut results: Vec<serde_json::Value> = Vec::new();

    let source_ids: Vec<&str> = match source_filter {
        None => vec!["github", "well-known", "skills-sh"],
        Some(SourceFlag::Github) => vec!["github"],
        Some(SourceFlag::WellKnown) => vec!["well-known"],
        Some(SourceFlag::SkillsSh) => vec!["skills-sh"],
    };

    for source in &sources {
        if !source_ids.contains(&source.source_id()) {
            continue;
        }
        if results.len() >= HARD_CAP {
            break;
        }
        let per_source = effective_limit.saturating_sub(results.len()).max(1);
        match source.search(query, per_source).await {
            Ok(metas) => {
                for m in metas {
                    if results.len() >= HARD_CAP {
                        break;
                    }
                    let trust = source.trust_level_for(&m.identifier);
                    results.push(serde_json::json!({
                        "name": m.name,
                        "source": m.source_id,
                        "identifier": m.identifier,
                        "description": m.description,
                        "trust_level": trust_level_str(trust),
                    }));
                }
            }
            Err(e) => {
                tracing::warn!(
                    source = source.source_id(),
                    "search error: {}",
                    strip_terminal_escapes(&e.to_string())
                );
            }
        }
    }

    match format {
        Format::Json => serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string()),
        Format::Text => {
            if results.is_empty() {
                return "No results found.".to_string();
            }
            results
                .iter()
                .map(|r| {
                    format!(
                        "{} [{}] ({}) — {}",
                        r["name"].as_str().unwrap_or(""),
                        r["trust_level"].as_str().unwrap_or(""),
                        r["source"].as_str().unwrap_or(""),
                        r["identifier"].as_str().unwrap_or(""),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

/// Update one skill by name, or all installed skills if `name` is None.
///
/// cmd_update reads `SkillLock` (the 21.8 lock file) per D-10. Every
/// server-originated error passes through `strip_terminal_escapes` (D-16).
pub async fn cmd_update(cfg: &Config, name: Option<&str>, skip_audit: bool) -> anyhow::Result<i32> {
    // D-15: idempotent migration at first skills command invocation.
    let _ = migrate_from_hub_manifest();

    let skills_root = ironhermes_hub::paths::skills_root()
        .map_err(|e| anyhow::anyhow!("cannot resolve skills root: {}", e))?;
    let sources = build_sources(cfg).await;
    let scanner = CoreSkillScanner;

    let names_to_update: Vec<String> = if let Some(n) = name {
        vec![n.to_string()]
    } else {
        // update all: read lock
        match SkillLock::load_or_default() {
            Ok(lock) => lock.skills.iter().map(|e| e.name.clone()).collect(),
            Err(e) => {
                eprintln!(
                    "error reading skills-lock.json: {}",
                    strip_terminal_escapes(&e.to_string())
                );
                return Ok(1);
            }
        }
    };

    let mut exit_code = 0i32;
    for skill_name in &names_to_update {
        // Re-read the lock on each iteration so concurrent mutations are seen.
        let lock = match SkillLock::load_or_default() {
            Ok(l) => l,
            Err(e) => {
                eprintln!(
                    "error reading skills-lock.json: {}",
                    strip_terminal_escapes(&e.to_string())
                );
                exit_code = 1;
                continue;
            }
        };
        let entry = match lock.get(skill_name.as_str()) {
            Some(e) => e.clone(),
            None => {
                eprintln!(
                    "error: skill '{}' is not installed",
                    strip_terminal_escapes(skill_name)
                );
                exit_code = 1;
                continue;
            }
        };

        // Match source_id to our built sources
        let source = sources
            .iter()
            .find(|s| s.source_id() == entry.source.as_str());
        let source = match source {
            Some(s) => s.as_ref(),
            None => {
                eprintln!(
                    "error: unknown source '{}' for skill '{}'",
                    strip_terminal_escapes(&entry.source),
                    strip_terminal_escapes(skill_name),
                );
                exit_code = 1;
                continue;
            }
        };

        match hub_update(source, skill_name, &scanner, &skills_root, skip_audit).await {
            Ok(outcome) => {
                println!(
                    "updated '{}': {} → {} ({})",
                    strip_terminal_escapes(&outcome.name),
                    outcome.old_hash.get(..12).unwrap_or(&outcome.old_hash),
                    outcome.new_hash.get(..12).unwrap_or(&outcome.new_hash),
                    strip_terminal_escapes(&outcome.scan_verdict),
                );
            }
            Err(e) => {
                eprintln!(
                    "error updating '{}': {}",
                    strip_terminal_escapes(skill_name),
                    strip_terminal_escapes(&e.to_string())
                );
                exit_code = 1;
            }
        }
    }

    Ok(exit_code)
}

/// Remove an installed skill by name.
///
/// Canonical CLI verb per D-04 (`uninstall` is retained as a clap alias on the
/// `SkillsAction::Remove` variant). Delegates to the hub-level `hub_uninstall`
/// which handles the on-disk removal + SkillLock entry drop.
/// Every server-originated error passes through `strip_terminal_escapes` (D-16).
pub fn cmd_remove(name: &str) -> anyhow::Result<i32> {
    // D-15: idempotent migration at first skills command invocation.
    let _ = migrate_from_hub_manifest();

    match hub_uninstall(name) {
        Ok(outcome) => {
            println!(
                "removed '{}' (removed {})",
                strip_terminal_escapes(&outcome.name),
                outcome.removed_path.display()
            );
            Ok(0)
        }
        Err(e) => {
            eprintln!("error: {}", strip_terminal_escapes(&e.to_string()));
            Ok(1)
        }
    }
}

/// List installed skills with source, trust level and install path.
pub fn cmd_list(cfg: &Config, format: Format) -> anyhow::Result<i32> {
    let output = cmd_list_impl(cfg, format);
    println!("{}", output);
    Ok(0)
}

/// Inner list that returns a String (for testing without printing).
///
/// Reads `SkillLock` (the 21.8 lock file) per D-10. Trust is recomputed live
/// from config per D-08 (never frozen in the lock file).
pub fn cmd_list_impl(cfg: &Config, format: Format) -> String {
    // D-15: idempotent migration at first skills command invocation so a pre-21.8
    // `.hub/lock.json` is picked up for listing before the new reader hits
    // `skills-lock.json`.
    let _ = migrate_from_hub_manifest();

    let lock = SkillLock::load_or_default().unwrap_or_default();
    let trusted_set = cfg.skills.hub.trusted_repos_set();

    let items: Vec<serde_json::Value> = lock
        .skills
        .iter()
        .map(|e| {
            // Recompute trust per D-08 (never frozen in lock)
            let trust_level = recompute_trust_str(&e.source, &e.identifier, &trusted_set);
            serde_json::json!({
                "name": e.name,
                "source": e.source,
                "identifier": e.identifier,
                "trust_level": trust_level,
                "repo_path": e.repo_path,
                "snapshot_hash": e.snapshot_hash,
                "computed_hash": e.computed_hash,
                "installed_at": e.installed_at.to_rfc3339(),
            })
        })
        .collect();

    match format {
        Format::Json => serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string()),
        Format::Text => {
            if items.is_empty() {
                return "No skills installed.".to_string();
            }
            items
                .iter()
                .map(|r| {
                    let snap = r["snapshot_hash"].as_str().unwrap_or("");
                    let snap_short: &str = snap.get(..12).unwrap_or(snap);
                    format!(
                        "{} [{}] ({}) — hash: {}",
                        r["name"].as_str().unwrap_or(""),
                        r["trust_level"].as_str().unwrap_or(""),
                        r["source"].as_str().unwrap_or(""),
                        snap_short,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

// ---------------------------------------------------------------------------
// Trust management — public inner impls for testing
// ---------------------------------------------------------------------------

/// Add a repo to trusted_repos (idempotent).
pub fn cmd_trust_add_impl(
    cfg: &mut Config,
    config_path: &std::path::Path,
    repo: &str,
) -> anyhow::Result<()> {
    if !cfg.skills.hub.trusted_repos.contains(&repo.to_string()) {
        cfg.skills.hub.trusted_repos.push(repo.to_string());
        save_config_atomic(config_path, cfg)?;
    }
    Ok(())
}

/// Remove a repo from trusted_repos (no-op if absent).
pub fn cmd_trust_remove_impl(
    cfg: &mut Config,
    config_path: &std::path::Path,
    repo: &str,
) -> anyhow::Result<()> {
    let before = cfg.skills.hub.trusted_repos.len();
    cfg.skills.hub.trusted_repos.retain(|r| r != repo);
    if cfg.skills.hub.trusted_repos.len() != before {
        save_config_atomic(config_path, cfg)?;
    }
    Ok(())
}

/// Return the trust list as a formatted string.
pub fn cmd_trust_list_impl(cfg: &Config, format: Format) -> String {
    let mut sorted = cfg.skills.hub.trusted_repos.clone();
    sorted.sort();
    match format {
        Format::Text => {
            if sorted.is_empty() {
                "No trusted repos configured.".to_string()
            } else {
                sorted.join("\n")
            }
        }
        Format::Json => serde_json::to_string_pretty(&sorted).unwrap_or_else(|_| "[]".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Top-level dispatch (called from main.rs)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// D-D1 path-shape probe helper (Phase 21.8.1)
// ---------------------------------------------------------------------------

/// Determine whether an identifier looks like a local filesystem path.
///
/// Uses a `dir_probe` closure instead of calling `fs::metadata` directly so
/// the function is pure-testable without env mutation. The production call
/// site in `cmd_install` passes `|id| fs::metadata(id).map(|m| m.is_dir()).unwrap_or(false)`.
///
/// The five D-D1 trigger shapes are: leading `/`, `./`, `../`, `~/`, or
/// the identifier resolving to an existing directory on disk (the metadata probe).
fn looks_like_local_path_with_probe<F>(identifier: &str, dir_probe: F) -> bool
where
    F: Fn(&str) -> bool,
{
    identifier.starts_with('/')
        || identifier.starts_with("./")
        || identifier.starts_with("../")
        || identifier.starts_with("~/")
        || dir_probe(identifier)
}

pub async fn dispatch(config_path: &std::path::Path, action: SkillsAction) -> anyhow::Result<i32> {
    let mut cfg = load_config(config_path)?;
    match action {
        SkillsAction::Install {
            identifier,
            yes: _,
            skip_audit,
        } => cmd_install(&cfg, &identifier, skip_audit).await,
        SkillsAction::Search {
            query,
            source,
            format,
            limit,
        } => cmd_search(&cfg, &query, source, format, limit).await,
        // skip_audit defaults to false on update; a future revision can surface a
        // flag on the Update variant if operators need to bypass the audit on update.
        SkillsAction::Update { name } => cmd_update(&cfg, name.as_deref(), false).await,
        SkillsAction::Remove { name } => {
            let code = cmd_remove(&name)?;
            Ok(code)
        }
        SkillsAction::List { format } => cmd_list(&cfg, format),
        SkillsAction::Trust { action } => match action {
            TrustAction::Add { repo } => {
                cmd_trust_add_impl(&mut cfg, config_path, &repo)?;
                Ok(0)
            }
            TrustAction::Remove { repo } => {
                cmd_trust_remove_impl(&mut cfg, config_path, &repo)?;
                Ok(0)
            }
            TrustAction::List { format } => {
                println!("{}", cmd_trust_list_impl(&cfg, format));
                Ok(0)
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Unit tests (Phase 21.8.1)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Test 7 from plan: recompute_trust_str returns "trusted" for "local-dir"
    // and does not regress the "github" → "community" arm.
    #[test]
    fn recompute_trust_str_returns_trusted_for_local_dir() {
        let empty_trusted: std::collections::HashSet<String> = std::collections::HashSet::new();
        assert_eq!(
            recompute_trust_str("local-dir", "/some/path", &empty_trusted),
            "trusted",
            "local-dir source must map to trusted (D-B2)"
        );
        // Regression guard: existing github arm still returns "community" for untrusted repos.
        assert_eq!(
            recompute_trust_str("github", "owner/repo", &empty_trusted),
            "community",
            "github arm regression: untrusted repo must still return community"
        );
    }

    // D-D1 path-shape tests using the pure helper (no env mutation).

    #[test]
    fn local_dir_hint_absolute_path() {
        assert!(
            looks_like_local_path_with_probe("/tmp/foo", |_| false),
            "absolute path starting with / must trigger hint"
        );
    }

    #[test]
    fn local_dir_hint_relative_dot_slash() {
        assert!(
            looks_like_local_path_with_probe("./skill", |_| false),
            "./ relative path must trigger hint"
        );
    }

    #[test]
    fn local_dir_hint_dotdot() {
        assert!(
            looks_like_local_path_with_probe("../skill", |_| false),
            "../ relative path must trigger hint"
        );
    }

    #[test]
    fn local_dir_hint_tilde() {
        assert!(
            looks_like_local_path_with_probe("~/skill", |_| false),
            "~/ tilde path must trigger hint"
        );
    }

    #[test]
    fn local_dir_hint_existing_dir_via_probe() {
        // Simulate the metadata probe returning true (as if a dir with that name exists in CWD).
        assert!(
            looks_like_local_path_with_probe("bradwilson/download/ascii-art/", |_| true),
            "metadata probe returning true must trigger hint"
        );
    }

    #[test]
    fn local_dir_hint_no_false_positive_github_id() {
        // When the dir does NOT exist in CWD, a bare owner/repo identifier must NOT trigger.
        assert!(
            !looks_like_local_path_with_probe("anthropic-ai/something-not-a-dir", |_| false),
            "non-path owner/repo with no matching dir must NOT trigger hint"
        );
    }
}
