//! Pre-flight check (D-05/D-07): runs after Cli::parse() and before
//! dispatch. Detects missing config or validation failures and launches
//! fix-mode wizard before falling through to the original command.
//!
//! Phase 25 D-17: after config validates, probe for unsatisfied required tool
//! prerequisites and emit a stderr banner. NO auto-wizard launch — operator
//! runs `hermes toolset setup` to fix. Phase 23 gate location preserved.

use anyhow::Result;
use ironhermes_core::config::Config;
use ironhermes_tools::Prerequisite;

use crate::Cli;

pub async fn run_preflight_check(_cli: &Cli) -> Result<()> {
    let cfg_path = Config::config_path();
    if !cfg_path.exists() {
        return crate::setup::run_setup(None, ironhermes_core::wizard::WizardMode::FirstRun).await;
    }
    match Config::load() {
        Err(_) => {
            crate::setup::run_setup(None, ironhermes_core::wizard::WizardMode::FixMode).await
        }
        Ok(config) => {
            if !config.validate().is_empty() {
                return crate::setup::run_setup(None, ironhermes_core::wizard::WizardMode::FixMode).await;
            }
            // Phase 25 D-17: tool-prereq probe. Builds a registry, queries
            // list_unavailable(), filters by config.tools.skip_prompts, emits a
            // stderr banner for required-missing prereqs. NO auto-wizard launch
            // — operator runs `hermes toolset setup` themselves (D-17 contract).
            let registry = crate::setup::build_full_registry();
            let unavailable = registry.list_unavailable();
            let skip: std::collections::HashSet<&str> = config
                .tools
                .skip_prompts
                .iter()
                .map(|s| s.as_str())
                .collect();
            let active: Vec<_> = unavailable
                .into_iter()
                .filter(|(name, _)| !skip.contains(name.as_str()))
                .collect();
            emit_prereq_banner(&active, &mut std::io::stderr());
            Ok(())
        }
    }
}

/// Writer-injection seam for testability (D-17). Emits the tool-prereq banner
/// to the provided writer. `std::io::stderr()` is the production caller.
fn emit_prereq_banner(
    active: &[(String, Vec<Prerequisite>)],
    out: &mut dyn std::io::Write,
) {
    if active.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "\u{26a0} Tool prerequisites unsatisfied \u{2014} run `hermes toolset setup` to configure:"
    );
    for (tool, missing) in active {
        let prereq_names: Vec<_> = missing.iter().map(|p| p.name.as_str()).collect();
        let _ = writeln!(out, "  - {} ({})", tool, prereq_names.join(", "));
    }
}

// ---------------------------------------------------------------------------
// Unit tests (Task 2 TDD)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_tools::Prerequisite;

    fn make_prereq(name: &str) -> Prerequisite {
        Prerequisite {
            kind: "env_var".to_string(),
            name: name.to_string(),
            description: "test prereq".to_string(),
            required: true,
        }
    }

    #[test]
    fn preflight_emits_banner_when_required_prereq_missing() {
        let active = vec![
            ("web_search".to_string(), vec![make_prereq("FIRECRAWL_API_KEY")]),
        ];
        let mut buf: Vec<u8> = Vec::new();
        emit_prereq_banner(&active, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.contains("Tool prerequisites unsatisfied"),
            "banner must contain 'Tool prerequisites unsatisfied', got: {}",
            output
        );
        assert!(
            output.contains("hermes toolset setup"),
            "banner must mention 'hermes toolset setup', got: {}",
            output
        );
        assert!(
            output.contains("web_search"),
            "banner must name the tool, got: {}",
            output
        );
        assert!(
            output.contains("FIRECRAWL_API_KEY"),
            "banner must name the missing prereq, got: {}",
            output
        );
    }

    #[test]
    fn preflight_suppresses_banner_for_skip_prompts_tools() {
        // Simulate the skip filter: web_search is in skip_prompts so it is
        // excluded from the active list before emit_prereq_banner is called.
        let all_unavailable = vec![
            ("web_search".to_string(), vec![make_prereq("FIRECRAWL_API_KEY")]),
        ];
        let skip: std::collections::HashSet<&str> = ["web_search"].iter().copied().collect();
        let active: Vec<_> = all_unavailable
            .into_iter()
            .filter(|(name, _)| !skip.contains(name.as_str()))
            .collect();
        let mut buf: Vec<u8> = Vec::new();
        emit_prereq_banner(&active, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.is_empty(),
            "banner must be empty when all tools are in skip_prompts, got: {}",
            output
        );
    }

    #[test]
    fn preflight_no_banner_when_active_is_empty() {
        let active: Vec<(String, Vec<Prerequisite>)> = vec![];
        let mut buf: Vec<u8> = Vec::new();
        emit_prereq_banner(&active, &mut buf);
        assert!(buf.is_empty(), "no output when active list is empty");
    }
}
