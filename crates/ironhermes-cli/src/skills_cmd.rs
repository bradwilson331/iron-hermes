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
    install as hub_install, uninstall as hub_uninstall, update as hub_update, CoreSkillScanner,
    GitHubAuth, GitHubSource, GitHubTap, HubManifest, HubSource, SkillsShSource,
    WellKnownSkillSource,
};
use std::sync::Arc;

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
    /// Uninstall a skill by name.
    Uninstall { name: String },
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
    let auth = GitHubAuth::resolve(
        cfg.skills.hub.github_token_env.as_deref(),
    ).await;
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
    let sh = SkillsShSource::new(gh.clone());

    vec![
        Box::new(SharedGitHubSource(gh)),
        Box::new(wk),
        Box::new(sh),
    ]
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
/// (`GitHubSource`, `WellKnownSkillSource`, `SkillsShSource`).
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
///   "skills-sh:..."    → SkillsShSource
///   otherwise          → GitHubSource (owner/repo/... format)
pub async fn cmd_install(cfg: &Config, identifier: &str) -> anyhow::Result<i32> {
    let skills_root = ironhermes_hub::paths::skills_root()
        .map_err(|e| anyhow::anyhow!("cannot resolve skills root: {}", e))?;
    std::fs::create_dir_all(&skills_root)?;

    let sources = build_sources(cfg).await;
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
    } else {
        // GitHub: must look like owner/repo/...
        if !identifier.contains('/') {
            eprintln!(
                "error: unknown identifier format '{}'. Expected owner/repo/path, well-known:..., or skills-sh:...",
                identifier
            );
            return Ok(1);
        }
        sources
            .iter()
            .find(|s| s.source_id() == "github")
            .map(|s| s.as_ref())
            .ok_or_else(|| anyhow::anyhow!("github source not available"))?
    };

    let scanner = CoreSkillScanner;
    match hub_install(source, identifier, &scanner, &skills_root).await {
        Ok(outcome) => {
            println!(
                "installed '{}' ({}) — trust: {} — hash: {}",
                outcome.name,
                outcome.install_path.display(),
                trust_level_str(outcome.trust_level),
                outcome.content_hash.get(..12).unwrap_or(&outcome.content_hash),
            );
            Ok(0)
        }
        Err(e) => {
            eprintln!("error: {}", e);
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
                tracing::warn!(source = source.source_id(), "search error: {}", e);
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
pub async fn cmd_update(cfg: &Config, name: Option<&str>) -> anyhow::Result<i32> {
    let skills_root = ironhermes_hub::paths::skills_root()
        .map_err(|e| anyhow::anyhow!("cannot resolve skills root: {}", e))?;
    let sources = build_sources(cfg).await;
    let scanner = CoreSkillScanner;

    let names_to_update: Vec<String> = if let Some(n) = name {
        vec![n.to_string()]
    } else {
        // update all: read manifest
        match HubManifest::load_or_default() {
            Ok(manifest) => manifest.installed.keys().cloned().collect(),
            Err(e) => {
                eprintln!("error reading manifest: {}", e);
                return Ok(1);
            }
        }
    };

    let mut exit_code = 0i32;
    for skill_name in &names_to_update {
        // Find source that has this skill in its manifest entry
        let manifest = match HubManifest::load_or_default() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("error reading manifest: {}", e);
                exit_code = 1;
                continue;
            }
        };
        let entry = match manifest.installed.get(skill_name.as_str()) {
            Some(e) => e.clone(),
            None => {
                eprintln!("error: skill '{}' is not installed", skill_name);
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
                    entry.source, skill_name
                );
                exit_code = 1;
                continue;
            }
        };

        match hub_update(source, skill_name, &scanner, &skills_root).await {
            Ok(outcome) => {
                println!(
                    "updated '{}': {} → {} ({})",
                    outcome.name,
                    outcome.old_hash.get(..12).unwrap_or(&outcome.old_hash),
                    outcome.new_hash.get(..12).unwrap_or(&outcome.new_hash),
                    outcome.scan_verdict,
                );
            }
            Err(e) => {
                eprintln!("error updating '{}': {}", skill_name, e);
                exit_code = 1;
            }
        }
    }

    Ok(exit_code)
}

/// Uninstall a skill by name.
pub fn cmd_uninstall(name: &str) -> anyhow::Result<i32> {
    match hub_uninstall(name) {
        Ok(outcome) => {
            println!(
                "uninstalled '{}' (removed {})",
                outcome.name,
                outcome.removed_path.display()
            );
            Ok(0)
        }
        Err(e) => {
            eprintln!("error: {}", e);
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
pub fn cmd_list_impl(cfg: &Config, format: Format) -> String {
    let manifest = match HubManifest::load_or_default() {
        Ok(m) => m,
        Err(_) => HubManifest::default(),
    };

    let trusted_set = cfg.skills.hub.trusted_repos_set();

    let items: Vec<serde_json::Value> = manifest
        .installed
        .values()
        .map(|e| {
            // Recompute trust per D-08 (never frozen in manifest)
            let trust_level = recompute_trust_str(&e.source, &e.identifier, &trusted_set);
            serde_json::json!({
                "name": e.name,
                "source": e.source,
                "identifier": e.identifier,
                "trust_level": trust_level,
                "install_path": e.install_path.display().to_string(),
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
                    format!(
                        "{} [{}] ({}) @ {}",
                        r["name"].as_str().unwrap_or(""),
                        r["trust_level"].as_str().unwrap_or(""),
                        r["source"].as_str().unwrap_or(""),
                        r["install_path"].as_str().unwrap_or(""),
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
        Format::Json => {
            serde_json::to_string_pretty(&sorted).unwrap_or_else(|_| "[]".to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level dispatch (called from main.rs)
// ---------------------------------------------------------------------------

pub async fn dispatch(config_path: &std::path::Path, action: SkillsAction) -> anyhow::Result<i32> {
    let mut cfg = load_config(config_path)?;
    match action {
        SkillsAction::Install { identifier, yes: _ } => cmd_install(&cfg, &identifier).await,
        SkillsAction::Search {
            query,
            source,
            format,
            limit,
        } => cmd_search(&cfg, &query, source, format, limit).await,
        SkillsAction::Update { name } => cmd_update(&cfg, name.as_deref()).await,
        SkillsAction::Uninstall { name } => {
            let code = cmd_uninstall(&name)?;
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
