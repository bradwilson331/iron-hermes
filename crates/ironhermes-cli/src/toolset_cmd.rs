//! `hermes toolset <subcommand>` — Phase 25, D-04 operator control surface.
//!
//! Structural model: `config_cli.rs::ConfigSubcommand` (subcommand enum + dispatcher).
//! Slug validation reuses `ironhermes_core::profile::validate_profile_name` per D-02 / T-25-01.
//! Cache-break banner emitted on stderr for state-changing commands per T-25-03.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use ironhermes_core::{config_setter, profile, ToolsConfig, DEFAULT_TOOLSETS};
use ironhermes_tools::ToolRegistry;
use std::path::Path;

/// D-01: The six concrete toolsets shipped in v2.1.
const KNOWN_TOOLSETS: &[&str] = &["web", "code", "memory", "agent", "skills", "session"];

#[derive(Subcommand)]
pub enum ToolsetSubcommand {
    /// List all toolsets with status and availability
    List,
    /// Enable a toolset (persists to active profile config.yaml)
    Enable { name: String },
    /// Disable a toolset (persists to active profile config.yaml)
    Disable { name: String },
    /// Show detail for one toolset (members + schemas + prerequisites)
    Show { name: String },
    /// Walk every unsatisfied required tool prerequisite, prompting for env var or
    /// config field values. Persistent: writes env vars to .env (0600 mode), config
    /// fields via dotted-path setter. Phase 25 D-18 / TOOL-05.
    Setup,
}

pub async fn handle_toolset_command(
    cmd: ToolsetSubcommand,
    _profile_name: &str,
) -> Result<()> {
    let hermes_home = ironhermes_core::constants::get_hermes_home();
    match cmd {
        ToolsetSubcommand::List => cmd_toolset_list(&hermes_home).await,
        ToolsetSubcommand::Enable { name } => cmd_toolset_enable(&hermes_home, &name).await,
        ToolsetSubcommand::Disable { name } => cmd_toolset_disable(&hermes_home, &name).await,
        ToolsetSubcommand::Show { name } => cmd_toolset_show(&hermes_home, &name).await,
        ToolsetSubcommand::Setup => cmd_toolset_setup(&hermes_home).await,
    }
}

/// Phase 25 D-18: walk every unsatisfied required prerequisite via rustyline wizard.
async fn cmd_toolset_setup(hermes_home: &Path) -> Result<()> {
    let mut rl = crate::setup::make_wizard_editor()?;
    crate::setup::run_tools_section(&mut rl, hermes_home).await
}

/// T-25-01 mitigation: slug-validate a toolset name using the Phase 24 D-03 pattern.
///
/// Reuses `profile::validate_profile_name` which enforces `[a-z0-9][a-z0-9-]*`.
/// This is the first call in every state-changing path — any path traversal or
/// invalid character is rejected BEFORE any config write or registry mutation.
pub fn validate_toolset_name(name: &str) -> Result<String> {
    profile::validate_profile_name(name)
        .map_err(|e| anyhow::anyhow!("invalid toolset name: {}", e))
}

/// Check that the validated name is one of the six D-01 built-in toolsets.
fn check_known_toolset(validated: &str) -> Result<()> {
    if !KNOWN_TOOLSETS.contains(&validated) {
        let mut known: Vec<&str> = KNOWN_TOOLSETS.to_vec();
        known.sort_unstable();
        anyhow::bail!(
            "unknown toolset '{}' — known toolsets: {}",
            validated,
            known.join(", ")
        );
    }
    Ok(())
}

pub async fn cmd_toolset_enable(hermes_home: &Path, name: &str) -> Result<()> {
    // T-25-01: validate BEFORE any config write.
    let validated = validate_toolset_name(name)?;
    check_known_toolset(&validated)?;
    config_setter::config_set(
        hermes_home,
        &format!("tools.toolsets.{}.enabled", validated),
        "true",
    )
    .with_context(|| format!("failed to enable toolset {}", validated))?;
    // T-25-03: cache-break banner on stderr (not stdout).
    eprintln!(
        "{} [toolset: {}] enabled \u{2014} schema cache will rebuild on next LLM call",
        "\u{26a0}".yellow(),
        validated,
    );
    Ok(())
}

pub async fn cmd_toolset_disable(hermes_home: &Path, name: &str) -> Result<()> {
    // T-25-01: validate BEFORE any config write.
    let validated = validate_toolset_name(name)?;
    check_known_toolset(&validated)?;
    config_setter::config_set(
        hermes_home,
        &format!("tools.toolsets.{}.enabled", validated),
        "false",
    )
    .with_context(|| format!("failed to disable toolset {}", validated))?;
    // T-25-03: cache-break banner on stderr (not stdout).
    eprintln!(
        "{} [toolset: {}] disabled \u{2014} schema cache will rebuild on next LLM call",
        "\u{26a0}".yellow(),
        validated,
    );
    Ok(())
}

async fn cmd_toolset_list(hermes_home: &Path) -> Result<()> {
    // Load config to get toolset enable/disable state.
    let cfg = load_tools_config(hermes_home);

    // Build a registry to get is_available info for known tool members.
    let mut registry = ToolRegistry::new();
    registry.register_defaults();

    // D-01 member map: toolset -> member tool names.
    let members_map = toolset_members_map();

    // Build display rows.
    let rows = build_toolset_rows(&cfg, &registry, &members_map);

    // Render and print.
    let rendered = ironhermes_core::commands::toolset_display::render_toolset_list(rows);
    print!("{}", rendered);
    Ok(())
}

async fn cmd_toolset_show(hermes_home: &Path, name: &str) -> Result<()> {
    // T-25-01: validate BEFORE any read.
    let validated = validate_toolset_name(name)?;
    check_known_toolset(&validated)?;

    let cfg = load_tools_config(hermes_home);
    let mut registry = ToolRegistry::new();
    registry.register_defaults();

    let members_map = toolset_members_map();
    let member_names = members_map.get(validated.as_str()).copied().unwrap_or(&[]);

    let enabled = cfg.is_toolset_enabled(&validated);
    let unavailable = registry.list_unavailable();
    let unavailable_names: std::collections::HashSet<&str> = unavailable
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();

    let members: Vec<(String, bool, String)> = member_names
        .iter()
        .map(|&tool_name| {
            let avail = !unavailable_names.contains(tool_name);
            let prereq_str = if avail {
                String::new()
            } else {
                unavailable
                    .iter()
                    .find(|(n, _)| n == tool_name)
                    .map(|(_, prereqs)| {
                        prereqs
                            .iter()
                            .filter(|p| p.required)
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default()
            };
            (tool_name.to_string(), avail, prereq_str)
        })
        .collect();

    let row = ironhermes_core::commands::toolset_display::ToolsetRow {
        name: validated.clone(),
        enabled,
        member_count: member_names.len(),
        available_count: members.iter().filter(|(_, avail, _)| *avail).count(),
        member_summary: String::new(), // not used for show
    };

    let rendered = ironhermes_core::commands::toolset_display::render_toolset_show(&row, &members);
    print!("{}", rendered);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_tools_config(hermes_home: &Path) -> ToolsConfig {
    let cfg_path = hermes_home.join("config.yaml");
    if !cfg_path.exists() {
        return ToolsConfig::default();
    }
    let text = match std::fs::read_to_string(&cfg_path) {
        Ok(t) => t,
        Err(_) => return ToolsConfig::default(),
    };
    let config: ironhermes_core::Config = serde_yaml::from_str(&text).unwrap_or_default();
    config.tools
}

/// D-01: Static membership map (toolset name -> member tool names).
fn toolset_members_map() -> std::collections::HashMap<&'static str, &'static [&'static str]> {
    let mut m: std::collections::HashMap<&'static str, &'static [&'static str]> =
        std::collections::HashMap::new();
    m.insert("web", &["web_search", "web_read"]);
    m.insert(
        "code",
        &[
            "execute_code",
            "terminal",
            "read_file",
            "write_file",
            "list_dir",
            "grep_files",
        ],
    );
    m.insert("memory", &["memory"]);
    m.insert("agent", &["delegate_task", "cronjob"]);
    m.insert("skills", &["skills"]);
    m.insert("session", &["session_search"]);
    m
}

/// Build display rows for toolset list output.
pub fn build_toolset_rows(
    cfg: &ToolsConfig,
    registry: &ToolRegistry,
    members_map: &std::collections::HashMap<&'static str, &'static [&'static str]>,
) -> Vec<ironhermes_core::commands::toolset_display::ToolsetRow> {
    let unavailable = registry.list_unavailable();
    let unavailable_names: std::collections::HashSet<&str> = unavailable
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();

    // Use DEFAULT_TOOLSETS order + remaining toolsets.
    let mut ordered: Vec<&str> = DEFAULT_TOOLSETS.to_vec();
    for &ts in KNOWN_TOOLSETS {
        if !ordered.contains(&ts) {
            ordered.push(ts);
        }
    }

    ordered
        .into_iter()
        .map(|ts_name| {
            let member_names = members_map.get(ts_name).copied().unwrap_or(&[]);
            let enabled = cfg.is_toolset_enabled(ts_name);
            let available_count = member_names
                .iter()
                .filter(|&&n| !unavailable_names.contains(n))
                .count();

            // Build summary string: "web_search ✓, web_read ✗ FIRECRAWL_API_KEY"
            let member_summary = if member_names.is_empty() {
                String::new()
            } else {
                member_names
                    .iter()
                    .map(|&n| {
                        if unavailable_names.contains(n) {
                            let prereq_str = unavailable
                                .iter()
                                .find(|(nm, _)| nm == n)
                                .map(|(_, prereqs)| {
                                    prereqs
                                        .iter()
                                        .filter(|p| p.required)
                                        .map(|p| p.name.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                })
                                .unwrap_or_default();
                            if prereq_str.is_empty() {
                                format!("{} \u{2717}", n)
                            } else {
                                format!("{} \u{2717} {}", n, prereq_str)
                            }
                        } else {
                            format!("{} \u{2713}", n)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };

            ironhermes_core::commands::toolset_display::ToolsetRow {
                name: ts_name.to_string(),
                enabled,
                member_count: member_names.len(),
                available_count,
                member_summary,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_toolset_name_rejects_path_traversal() {
        let result = validate_toolset_name("../etc/passwd");
        assert!(
            result.is_err(),
            "path-traversal name must be rejected, got Ok"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("invalid toolset name"),
            "error message should contain 'invalid toolset name', got: {}",
            msg
        );
    }

    #[test]
    fn validate_toolset_name_rejects_empty() {
        let result = validate_toolset_name("");
        assert!(result.is_err(), "empty name must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("invalid toolset name"),
            "error message should contain 'invalid toolset name', got: {}",
            msg
        );
    }

    #[test]
    fn validate_toolset_name_accepts_known_d01_names() {
        for name in KNOWN_TOOLSETS {
            let result = validate_toolset_name(name);
            assert!(
                result.is_ok(),
                "D-01 name '{}' should be valid, got: {:?}",
                name,
                result
            );
            assert_eq!(result.unwrap(), *name);
        }
    }

    #[test]
    fn validate_toolset_name_rejects_uppercase() {
        let result = validate_toolset_name("WEB");
        assert!(result.is_err(), "uppercase name 'WEB' must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("invalid toolset name"),
            "got: {}",
            msg
        );
    }

    #[test]
    fn cmd_toolset_enable_rejects_unknown_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(cmd_toolset_enable(tmp.path(), "not_a_real_toolset"));
        assert!(result.is_err(), "unknown toolset name must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("unknown toolset") || msg.contains("invalid"),
            "error should mention 'unknown toolset' or 'invalid', got: {}",
            msg
        );
    }
}
