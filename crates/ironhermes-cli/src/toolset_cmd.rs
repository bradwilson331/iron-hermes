//! `hermes toolset <subcommand>` — Phase 25, D-04 operator control surface.
//!
//! Structural model: `config_cli.rs::ConfigSubcommand` (subcommand enum + dispatcher).
//! Slug validation reuses `ironhermes_core::profile::validate_profile_name` per D-02 / T-25-01.
//! Cache-break banner emitted on stderr for state-changing commands per T-25-03.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use ironhermes_core::{DEFAULT_TOOLSETS, ToolsConfig, config_setter, profile};
use ironhermes_tools::ToolRegistry;
use std::path::Path;

/// D-01/D-04: The seven concrete toolsets shipped in v2.1 (browser added in Phase 25.1).
const KNOWN_TOOLSETS: &[&str] = &[
    "web", "code", "memory", "agent", "skills", "session", "browser",
];

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

pub async fn handle_toolset_command(cmd: ToolsetSubcommand, _profile_name: &str) -> Result<()> {
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
    profile::validate_profile_name(name).map_err(|e| anyhow::anyhow!("invalid toolset name: {}", e))
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

    let (row, members) = build_toolset_show_view(&cfg, &registry, &validated);

    let rendered = ironhermes_core::commands::toolset_display::render_toolset_show(&row, &members);
    print!("{}", rendered);
    Ok(())
}

/// Phase 25.1 GAP-6 closure: pure logic for `toolset show` extracted so the
/// chromium-availability contract can be unit-tested without spawning a
/// subprocess or capturing stdout. Both `cmd_toolset_show` (the CLI entry
/// point) and the `browser_show_reports_*` regression tests call this.
///
/// Returns `(ToolsetRow, members)` where `members` is the per-tool tuple
/// `(name, is_available, prereq_label)` consumed by `render_toolset_show`.
pub fn build_toolset_show_view(
    cfg: &ToolsConfig,
    registry: &ToolRegistry,
    validated: &str,
) -> (
    ironhermes_core::commands::toolset_display::ToolsetRow,
    Vec<(String, bool, String)>,
) {
    let members_map = toolset_members_map();
    let member_names = members_map.get(validated).copied().unwrap_or(&[]);

    let enabled = cfg.is_toolset_enabled(validated);
    let unavailable = registry.list_unavailable();
    let unavailable_names: std::collections::HashSet<&str> =
        unavailable.iter().map(|(name, _)| name.as_str()).collect();

    // GAP-6: special-case the browser toolset because register_defaults() does
    // not register browser_* tools (they require Arc<Config> from plan 14 and
    // are registered separately via register_browser_tools_with_vision).
    // See `browser_chromium_unavailable()` for the rationale + single-source-of-truth.
    let chromium_missing = validated == "browser" && browser_chromium_unavailable();

    let members: Vec<(String, bool, String)> = member_names
        .iter()
        .map(|&tool_name| {
            let avail = !chromium_missing && !unavailable_names.contains(tool_name);
            let prereq_str = if avail {
                String::new()
            } else if chromium_missing {
                "chromium".to_string()
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
        name: validated.to_string(),
        enabled,
        member_count: member_names.len(),
        // available_count derives from per-member `avail`, which already
        // respects `chromium_missing` — no second branch needed here.
        available_count: members.iter().filter(|(_, avail, _)| *avail).count(),
        member_summary: String::new(), // not used for show
    };

    (row, members)
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
    m.insert("web", &["web_search", "web_read", "web_extract"]);
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
    m.insert(
        "browser",
        &[
            "browser_back",
            "browser_click",
            "browser_close",
            "browser_console",
            "browser_get_images",
            "browser_navigate",
            "browser_press",
            "browser_scroll",
            "browser_snapshot",
            "browser_type",
            "browser_vision",
        ],
    );
    m
}

/// Build display rows for toolset list output.
pub fn build_toolset_rows(
    cfg: &ToolsConfig,
    registry: &ToolRegistry,
    members_map: &std::collections::HashMap<&'static str, &'static [&'static str]>,
) -> Vec<ironhermes_core::commands::toolset_display::ToolsetRow> {
    let unavailable = registry.list_unavailable();
    let unavailable_names: std::collections::HashSet<&str> =
        unavailable.iter().map(|(name, _)| name.as_str()).collect();

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

            // GAP-6: special-case the browser toolset because register_defaults()
            // does not register browser_* tools — see `browser_chromium_unavailable()`
            // for the rationale + single-source-of-truth contract with `cmd_toolset_show`.
            let chromium_missing = ts_name == "browser" && browser_chromium_unavailable();

            let available_count = if chromium_missing {
                0
            } else {
                member_names
                    .iter()
                    .filter(|&&n| !unavailable_names.contains(n))
                    .count()
            };

            // Build summary string: "web_search ✓, web_read ✗ FIRECRAWL_API_KEY"
            let member_summary = if member_names.is_empty() {
                String::new()
            } else {
                member_names
                    .iter()
                    .map(|&n| {
                        let unavailable_now = chromium_missing || unavailable_names.contains(n);
                        if !unavailable_now {
                            return format!("{} \u{2713}", n);
                        }
                        let prereq_str = if chromium_missing {
                            "chromium".to_string()
                        } else {
                            unavailable
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
                                .unwrap_or_default()
                        };
                        if prereq_str.is_empty() {
                            format!("{} \u{2717}", n)
                        } else {
                            format!("{} \u{2717} {}", n, prereq_str)
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
// Phase 25.1 GAP-6 closure helper
// ---------------------------------------------------------------------------

/// Phase 25.1 GAP-6: registry-level prereq machinery does not know about browser
/// tools because `register_defaults()` does not register them (browser_* tools
/// require `Arc<Config>` from plan 14 and are registered separately via
/// `register_browser_tools_with_vision`). Both `toolset list` and `toolset show`
/// build their `ToolRegistry` via `register_defaults()`, so without an explicit
/// chromium-availability check the browser row would always render as `11/11 ✓`
/// even when `BROWSER_PATH` points at a non-existent file.
///
/// To keep the two subcommands consistent (D-05 chromium discovery surfaces in
/// BOTH `toolset list` AND `toolset show` — never just one), this helper
/// centralises the chromium-availability check used by `build_toolset_rows`
/// and `build_toolset_show_view`. A future drift between list and show is
/// impossible without deleting the helper.
///
/// Returns `true` when chromium is NOT discoverable per `find_chromium_binary`
/// (i.e. `BROWSER_PATH` / `CHROMIUM_PATH` set to a non-existent file, or no
/// system chromium found on `PATH` and platform paths).
fn browser_chromium_unavailable() -> bool {
    ironhermes_tools::browser_session::find_chromium_binary(None).is_none()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::ToolsetEntry;
    use std::sync::OnceLock;

    /// Process-wide ENV_LOCK — mirrors the pattern used in
    /// `tests/toolset_integration.rs` lines 18-21. Required because `cargo test`
    /// runs tests in the same process on multiple threads by default; any test
    /// that mutates `BROWSER_PATH` / `CHROMIUM_PATH` must hold this lock to avoid
    /// cross-test bleed (also collides with `find_chromium_binary` env reads in
    /// other crates' tests when the same binary is invoked).
    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

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
        assert!(msg.contains("invalid toolset name"), "got: {}", msg);
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

    /// GAP-1 regression: `browser` must appear in KNOWN_TOOLSETS so that
    /// `hermes toolset enable browser` does not return "unknown toolset 'browser'".
    #[test]
    fn browser_in_known_set() {
        assert!(
            KNOWN_TOOLSETS.contains(&"browser"),
            "GAP-1: 'browser' must be in KNOWN_TOOLSETS (got: {:?})",
            KNOWN_TOOLSETS
        );
        assert_eq!(
            KNOWN_TOOLSETS.len(),
            7,
            "KNOWN_TOOLSETS must have exactly 7 entries after Phase 25.1 extension"
        );
    }

    /// GAP-1 regression: `hermes toolset enable browser` must succeed (not error with
    /// "unknown toolset 'browser'"). Verifies the full enable path accepts the browser name.
    #[test]
    fn cmd_toolset_enable_accepts_browser() {
        let tmp = tempfile::TempDir::new().unwrap();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(cmd_toolset_enable(tmp.path(), "browser"));
        assert!(
            result.is_ok(),
            "GAP-1: cmd_toolset_enable must accept 'browser', got error: {:?}",
            result.err()
        );
    }

    // ---------------------------------------------------------------------
    // Phase 25.1 GAP-6 regression — list AND show must agree on chromium
    // availability. The `find_chromium_binary` discovery uses `path.is_file()`
    // ONLY (no magic-byte check, no executable-bit check), so the green-arm
    // test below using `current_exe()` remains safe even if a future tightening
    // adds a magic-byte check (the test binary itself is a real ELF/Mach-O).
    // ---------------------------------------------------------------------

    /// GAP-6 LIST path: when BROWSER_PATH points at a non-existent path, the
    /// browser row must show 0/11 with every member marked `✗ chromium`.
    #[test]
    fn browser_row_reports_zero_when_chromium_unavailable() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

        // SAFETY: env mutation is process-wide; serialised by env_lock.
        unsafe {
            std::env::remove_var("CHROMIUM_PATH");
            std::env::set_var("BROWSER_PATH", "/dev/null/nonexistent-chromium-for-test");
        }

        let mut cfg = ToolsConfig::default();
        cfg.toolsets
            .insert("browser".to_string(), ToolsetEntry { enabled: true });

        let mut registry = ToolRegistry::new();
        registry.register_defaults();

        let members_map = toolset_members_map();
        let rows = build_toolset_rows(&cfg, &registry, &members_map);

        let row = rows
            .iter()
            .find(|r| r.name == "browser")
            .expect("browser row must exist");

        assert_eq!(
            row.available_count, 0,
            "GAP-6 LIST: browser available_count must be 0 when chromium missing, got {} (summary: {})",
            row.available_count, row.member_summary,
        );
        assert_eq!(
            row.member_count, 11,
            "browser member_count must be 11 (the 11 browser_* tools)"
        );
        assert!(
            row.member_summary.contains("chromium"),
            "GAP-6 LIST: member_summary must surface 'chromium' prereq label, got: {}",
            row.member_summary,
        );
        assert!(
            row.member_summary.contains("browser_navigate"),
            "browser_navigate must still appear in member_summary, got: {}",
            row.member_summary,
        );

        // Tear down so we do not leak BROWSER_PATH into other tests.
        unsafe {
            std::env::remove_var("BROWSER_PATH");
        }
    }

    /// GAP-6 SHOW path: when BROWSER_PATH points at a non-existent path, the
    /// show view must report 0/11 with EVERY member marked unavailable + prereq=chromium.
    /// This is the regression guard against list/show divergence flagged by the planner.
    #[test]
    fn browser_show_reports_zero_when_chromium_unavailable() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

        unsafe {
            std::env::remove_var("CHROMIUM_PATH");
            std::env::set_var("BROWSER_PATH", "/dev/null/nonexistent-chromium-for-test");
        }

        let mut cfg = ToolsConfig::default();
        cfg.toolsets
            .insert("browser".to_string(), ToolsetEntry { enabled: true });

        let mut registry = ToolRegistry::new();
        registry.register_defaults();

        let (row, members) = build_toolset_show_view(&cfg, &registry, "browser");

        assert_eq!(
            row.available_count, 0,
            "GAP-6 SHOW: ToolsetRow.available_count must be 0 when chromium missing, got {}",
            row.available_count,
        );
        assert_eq!(row.member_count, 11, "browser member_count must be 11");
        assert_eq!(
            members.len(),
            11,
            "build_toolset_show_view must return 11 member tuples for browser, got {}",
            members.len(),
        );
        for (name, avail, prereq) in &members {
            assert!(
                !*avail,
                "GAP-6 SHOW: member {} must be unavailable when chromium missing",
                name,
            );
            assert!(
                prereq.contains("chromium"),
                "GAP-6 SHOW: member {} prereq must contain 'chromium', got: {:?}",
                name,
                prereq,
            );
        }

        unsafe {
            std::env::remove_var("BROWSER_PATH");
        }
    }

    /// GAP-6 GREEN arm: when chromium IS discoverable (CHROMIUM_PATH points at a
    /// real file), the browser row must report 11/11 with every member marked
    /// `✓` and no `chromium` prereq label — i.e. the pre-fix behaviour preserved.
    /// We use `std::env::current_exe()` because `find_chromium_binary` uses
    /// `path.is_file()` only (no magic-byte / no executable-bit check), which the
    /// test binary always satisfies. If a future tightening ever adds a magic-byte
    /// check, the test still passes (the test binary is a real Mach-O/ELF).
    #[test]
    fn browser_row_reports_full_when_chromium_available() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

        let real_path = std::env::current_exe()
            .expect("test binary must have a current_exe path")
            .to_string_lossy()
            .into_owned();

        unsafe {
            std::env::remove_var("BROWSER_PATH");
            std::env::set_var("CHROMIUM_PATH", &real_path);
        }

        let mut cfg = ToolsConfig::default();
        cfg.toolsets
            .insert("browser".to_string(), ToolsetEntry { enabled: true });

        let mut registry = ToolRegistry::new();
        registry.register_defaults();

        let members_map = toolset_members_map();
        let rows = build_toolset_rows(&cfg, &registry, &members_map);

        let row = rows
            .iter()
            .find(|r| r.name == "browser")
            .expect("browser row must exist");

        assert_eq!(
            row.available_count, 11,
            "GAP-6 GREEN: chromium IS available — available_count must be 11, got {} (summary: {})",
            row.available_count, row.member_summary,
        );
        assert!(
            row.member_summary.contains("\u{2713}"),
            "GAP-6 GREEN: member_summary must contain the check-mark, got: {}",
            row.member_summary,
        );
        assert!(
            !row.member_summary.contains("chromium"),
            "GAP-6 GREEN: member_summary must NOT contain 'chromium' prereq when chromium is available, got: {}",
            row.member_summary,
        );

        unsafe {
            std::env::remove_var("CHROMIUM_PATH");
        }
    }

    /// Phase 25.2 Plan 15 (UAT Issue 2 side-bug): the static toolset_members_map
    /// MUST include web_extract in the web toolset. Phase 25.2 added the tool but
    /// this map was not updated until Plan 15. Lock against future drift.
    #[test]
    fn toolset_members_map_web_includes_web_extract() {
        let m = toolset_members_map();
        let web = m
            .get("web")
            .copied()
            .expect("web toolset must exist in members map");
        assert!(
            web.contains(&"web_extract"),
            "web toolset must list web_extract; got {:?}",
            web
        );
        assert!(web.contains(&"web_search"));
        assert!(web.contains(&"web_read"));
        assert_eq!(
            web.len(),
            3,
            "web toolset must have exactly 3 members (search/read/extract); got {:?}",
            web
        );
    }

    /// Phase 25.2 Plan 15 cross-crate parity: the CLI subcommand's static map
    /// MUST agree with the slash UI's RegistryToolsetSession internal map.
    /// Both surfaces show the same data; drift would surface as different
    /// member counts depending on entry point.
    #[test]
    fn toolset_members_map_agrees_with_registry_toolset_session() {
        // Plan 15 Task 1 invariant: the slash-UI map keeps "web" at 3 members
        // including web_extract. CLI map (this file) must match.
        let cli_map = toolset_members_map();
        let cli_web = cli_map.get("web").copied().unwrap_or(&[]);
        assert_eq!(cli_web.len(), 3, "CLI map web entry: {:?}", cli_web);
    }
}
