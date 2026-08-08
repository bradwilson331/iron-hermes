//! Pre-flight check (D-05/D-07): runs after Cli::parse() and before
//! dispatch. Detects missing config or validation failures and launches
//! fix-mode wizard before falling through to the original command.
//!
//! Phase 25 D-17: after config validates, probe for unsatisfied required tool
//! prerequisites and emit a stderr banner. NO auto-wizard launch — operator
//! runs `hermes toolset setup` to fix. Phase 23 gate location preserved.
//!
//! Phase 35.1 D-07/D-08: after config validates, check whether a runnable LLM
//! is configured. If not, auto-launch FirstRun wizard before proceeding.
//!
//! GAP-7 (D-06/D-10, Phase 46.9 gap-closure round 2): the gateway is a
//! long-running daemon — an interactive stdin wizard prompt is always wrong
//! for it. `run_preflight_check` now takes an `interactive: bool`. When
//! true, behavior is byte-for-byte the original (all four wizard branches
//! below, unchanged). When false (the gateway's `--non-interactive` flag, or
//! any non-TTY stdin caller): a missing/invalid config fails fast with an
//! actionable error instead of launching the wizard; a present+valid config
//! with no runnable LLM proceeds after printing an actionable stderr notice
//! (the gateway can still run cron/telegram and will surface auth errors at
//! request time) instead of blocking on stdin.

use anyhow::Result;
use ironhermes_core::config::Config;
use ironhermes_tools::Prerequisite;

use crate::Cli;

/// GAP-7 (D-06): pure decision helper for the preflight gate. Given the
/// three environment signals (config present / config valid / an LLM is
/// runnable) plus whether this is an interactive entry point, decides the
/// outcome without touching stdin or the filesystem — kept separate from
/// `run_preflight_check` so it's unit-testable in isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreflightAction {
    /// Launch the interactive (stdin-blocking) setup wizard. The caller
    /// picks FirstRun vs. FixMode from the same input signals — this enum
    /// only captures WHETHER to launch it.
    LaunchWizard,
    /// Proceed to the tool-prereq banner + `Ok(())`. Covers both a fully
    /// runnable config, and (non-interactive only) a valid-but-not-runnable
    /// config — the caller emits an actionable stderr notice in the latter
    /// case, but never blocks on stdin.
    Proceed,
    /// Non-interactive only: config missing or invalid. The caller returns
    /// an actionable `Err` instead of launching a stdin-blocking wizard.
    FailFast,
}

fn preflight_action(
    config_present: bool,
    config_valid: bool,
    runnable: bool,
    interactive: bool,
) -> PreflightAction {
    if interactive {
        if !config_present || !config_valid || !runnable {
            PreflightAction::LaunchWizard
        } else {
            PreflightAction::Proceed
        }
    } else if !config_present || !config_valid {
        PreflightAction::FailFast
    } else {
        // Present + valid: proceed whether or not it's runnable. A
        // not-runnable config gets a stderr notice from the caller, but
        // non-interactive mode NEVER blocks on stdin.
        PreflightAction::Proceed
    }
}

/// GAP-7 (D-06): actionable stderr notice for non-interactive mode when the
/// config is present+valid but `has_runnable_llm` returned false. Mirrors
/// the interactive wizard's concern (no usable provider key) without
/// dropping into a stdin prompt.
fn emit_non_interactive_llm_notice(out: &mut dyn std::io::Write) {
    let _ = writeln!(
        out,
        "\u{26a0} No runnable LLM provider detected (checked OPENROUTER_API_KEY, \
ANTHROPIC_API_KEY, OPENAI_API_KEY, a local config.model.base_url, and config.model.api_key) \
\u{2014} proceeding non-interactively. Set one of those env vars (or run `hermes setup` \
interactively) to fix this; requests will fail with an auth error until then."
    );
}

pub async fn run_preflight_check(_cli: &Cli, interactive: bool) -> Result<()> {
    let cfg_path = Config::config_path();
    let config_present = cfg_path.exists();
    if !config_present {
        return match preflight_action(false, false, false, interactive) {
            PreflightAction::LaunchWizard => {
                crate::setup::run_setup(None, ironhermes_core::wizard::WizardMode::FirstRun).await
            }
            PreflightAction::FailFast => Err(anyhow::anyhow!(
                "No config at {}; run `hermes setup` interactively first",
                cfg_path.display()
            )),
            PreflightAction::Proceed => {
                unreachable!("preflight_action never returns Proceed when config_present=false")
            }
        };
    }
    match Config::load() {
        Err(_) => match preflight_action(true, false, false, interactive) {
            PreflightAction::LaunchWizard => {
                crate::setup::run_setup(None, ironhermes_core::wizard::WizardMode::FixMode).await
            }
            PreflightAction::FailFast => Err(anyhow::anyhow!(
                "Config at {} failed to load; run `hermes setup` interactively to fix it",
                cfg_path.display()
            )),
            PreflightAction::Proceed => {
                unreachable!("preflight_action never returns Proceed when config_valid=false")
            }
        },
        Ok(config) => {
            let config_valid = config.validate().is_empty();
            if !config_valid {
                return match preflight_action(true, false, false, interactive) {
                    PreflightAction::LaunchWizard => {
                        crate::setup::run_setup(None, ironhermes_core::wizard::WizardMode::FixMode)
                            .await
                    }
                    PreflightAction::FailFast => Err(anyhow::anyhow!(
                        "Config at {} is invalid; run `hermes setup` interactively to fix it",
                        cfg_path.display()
                    )),
                    PreflightAction::Proceed => unreachable!(
                        "preflight_action never returns Proceed when config_valid=false"
                    ),
                };
            }
            // Phase 35.1 D-07/D-08: check for a runnable LLM after config
            // validates. A valid config skeleton with no usable provider key
            // would give a cryptic "unauthorized" error on first turn — route
            // through setup instead. MUST run AFTER dotenvy loads in main.rs
            // (line ~275) so std::env::var() reflects the merged .env state.
            let hermes_home = ironhermes_core::constants::get_hermes_home();
            let runnable = has_runnable_llm(&config, &hermes_home);
            match preflight_action(true, true, runnable, interactive) {
                PreflightAction::LaunchWizard => {
                    return crate::setup::run_setup(
                        None,
                        ironhermes_core::wizard::WizardMode::FirstRun,
                    )
                    .await;
                }
                PreflightAction::FailFast => unreachable!(
                    "preflight_action never returns FailFast when config_present && config_valid"
                ),
                PreflightAction::Proceed => {
                    if !runnable {
                        // Non-interactive + not-runnable: notice instead of wizard.
                        emit_non_interactive_llm_notice(&mut std::io::stderr());
                    }
                }
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

/// D-08: Determine whether the current environment has a usable LLM provider.
///
/// Three ordered checks (RESEARCH Pitfall 6 — env vars FIRST because
/// dotenvy::from_path already ran in main.rs before preflight):
///
/// 1. Post-dotenvy process env vars — reads the merged state (highest signal).
/// 2. Raw .env file scan — belt-and-suspenders for edge cases where the env
///    var was not loaded into the process (e.g. sub-process launch contexts).
/// 3. Local endpoint in config.model.base_url — Ollama users with a localhost
///    base_url are NEVER prompted; this is the D-08 escape hatch.
///
/// T-35.1-01 mitigation: `l.len() > key.len()` in the raw .env scan rejects
/// lines like `OPENROUTER_API_KEY=` (empty value) that would otherwise bypass
/// detection and silently let a "bad" state through.
fn has_runnable_llm(config: &Config, hermes_home: &std::path::Path) -> bool {
    // Check 1: post-dotenvy env vars (primary — reads AFTER dotenvy::from_path
    // at main.rs ~line 275, so this reflects the merged .env + process env state).
    for var in &["OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"] {
        if std::env::var(var).map(|v| !v.is_empty()).unwrap_or(false) {
            return true;
        }
    }
    // Check 2: raw .env file scan (belt-and-suspenders).
    let env_path = hermes_home.join(".env");
    if env_path.exists()
        && let Ok(text) = std::fs::read_to_string(&env_path)
    {
        for key in &[
            "OPENROUTER_API_KEY=",
            "ANTHROPIC_API_KEY=",
            "OPENAI_API_KEY=",
        ] {
            // l.len() > key.len() rejects empty-value lines (T-35.1-01).
            if text
                .lines()
                .any(|l| l.starts_with(key) && l.len() > key.len())
            {
                return true;
            }
        }
    }
    // Check 3: local endpoint (Ollama escape hatch — D-08 must NOT prompt
    // users who have configured a local base_url in config.yaml).
    if let Some(ref base_url) = config.model.base_url {
        let lower = base_url.to_lowercase();
        if lower.contains("localhost") || lower.contains("127.0.0.1") {
            return true;
        }
    }
    // Check 4: deprecated model.api_key in config.yaml (still accepted by
    // validate()). Users on the old config format are runnable — do NOT
    // prompt them with the setup wizard.
    if config
        .model
        .api_key
        .as_deref()
        .map(|k| !k.is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    false
}

/// Writer-injection seam for testability (D-17). Emits the tool-prereq banner
/// to the provided writer. `std::io::stderr()` is the production caller.
fn emit_prereq_banner(active: &[(String, Vec<Prerequisite>)], out: &mut dyn std::io::Write) {
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
// Unit tests (Task 2 TDD + Phase 35.1 D-08 has_runnable_llm tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::config::Config;
    use ironhermes_tools::Prerequisite;
    use tempfile::TempDir;

    // Serialise all env-mutating tests. MUST be the crate-wide lock
    // (`crate::test_env_lock`), not a module-local mutex: a per-module mutex
    // serialises this module against ITSELF but not against other modules that
    // mutate the SAME process-global vars — `setup.rs` also set/removes
    // `OPENROUTER_API_KEY`, so two separate locks meant no mutual exclusion at all
    // and `setup::tests::backfill_uses_process_env_when_dotenv_absent` flaked
    // (it read the var after this module's `remove_var` had wiped it).

    fn make_prereq(name: &str) -> Prerequisite {
        Prerequisite {
            kind: "env_var".to_string(),
            name: name.to_string(),
            description: "test prereq".to_string(),
            required: true,
            group: None,
        }
    }

    // -----------------------------------------------------------------------
    // preflight_action unit tests (GAP-7 D-06, Phase 46.9 gap-closure round 2)
    // -----------------------------------------------------------------------

    #[test]
    fn preflight_action_interactive_missing_config_launches_wizard() {
        assert_eq!(
            preflight_action(false, false, false, true),
            PreflightAction::LaunchWizard
        );
    }

    #[test]
    fn preflight_action_interactive_invalid_config_launches_wizard() {
        assert_eq!(
            preflight_action(true, false, false, true),
            PreflightAction::LaunchWizard
        );
    }

    #[test]
    fn preflight_action_interactive_not_runnable_launches_wizard() {
        assert_eq!(
            preflight_action(true, true, false, true),
            PreflightAction::LaunchWizard
        );
    }

    #[test]
    fn preflight_action_interactive_runnable_proceeds() {
        assert_eq!(
            preflight_action(true, true, true, true),
            PreflightAction::Proceed
        );
    }

    #[test]
    fn preflight_action_non_interactive_present_valid_not_runnable_proceeds() {
        assert_eq!(
            preflight_action(true, true, false, false),
            PreflightAction::Proceed
        );
    }

    #[test]
    fn preflight_action_non_interactive_present_valid_runnable_proceeds() {
        assert_eq!(
            preflight_action(true, true, true, false),
            PreflightAction::Proceed
        );
    }

    #[test]
    fn preflight_action_non_interactive_missing_config_fails_fast() {
        assert_eq!(
            preflight_action(false, false, false, false),
            PreflightAction::FailFast
        );
    }

    #[test]
    fn preflight_action_non_interactive_invalid_config_fails_fast() {
        assert_eq!(
            preflight_action(true, false, false, false),
            PreflightAction::FailFast
        );
    }

    // -----------------------------------------------------------------------
    // has_runnable_llm unit tests (Phase 35.1 D-08)
    // -----------------------------------------------------------------------

    #[test]
    fn has_runnable_llm_returns_true_when_openrouter_api_key_set_in_env() {
        let _g = crate::test_env_lock();
        // SAFETY: test-only env mutation; serialised by env_lock.
        unsafe { std::env::set_var("OPENROUTER_API_KEY", "sk-abc") };
        let config = Config::default();
        let tmp = TempDir::new().unwrap();
        let result = has_runnable_llm(&config, tmp.path());
        // SAFETY: restore env.
        unsafe { std::env::remove_var("OPENROUTER_API_KEY") };
        assert!(
            result,
            "expected true when OPENROUTER_API_KEY is set in env"
        );
    }

    #[test]
    fn has_runnable_llm_returns_true_when_local_base_url_configured() {
        let _g = crate::test_env_lock();
        // SAFETY: clear API key env vars so only the base_url check fires.
        unsafe {
            std::env::remove_var("OPENROUTER_API_KEY");
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
        }
        let tmp = TempDir::new().unwrap();
        // localhost case
        let mut config = Config::default();
        config.model.base_url = Some("http://localhost:11434".to_string());
        assert!(
            has_runnable_llm(&config, tmp.path()),
            "expected true for localhost base_url"
        );
        // 127.0.0.1 case
        let mut config2 = Config::default();
        config2.model.base_url = Some("http://127.0.0.1:8000".to_string());
        assert!(
            has_runnable_llm(&config2, tmp.path()),
            "expected true for 127.0.0.1 base_url"
        );
    }

    #[test]
    fn has_runnable_llm_returns_false_when_no_signal() {
        let _g = crate::test_env_lock();
        // SAFETY: clear all relevant env vars.
        unsafe {
            std::env::remove_var("OPENROUTER_API_KEY");
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
        }
        let config = Config::default(); // base_url is None
        let tmp = TempDir::new().unwrap();
        // No .env file in tmp
        assert!(
            !has_runnable_llm(&config, tmp.path()),
            "expected false when no env var, no .env file, and no local base_url"
        );
    }

    #[test]
    fn has_runnable_llm_returns_false_when_dotenv_has_empty_key_value() {
        let _g = crate::test_env_lock();
        // SAFETY: clear env vars.
        unsafe {
            std::env::remove_var("OPENROUTER_API_KEY");
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
        }
        let tmp = TempDir::new().unwrap();
        // Write .env with an empty-value key — T-35.1-01 bypass attempt.
        std::fs::write(tmp.path().join(".env"), "OPENROUTER_API_KEY=\n").unwrap();
        let config = Config::default();
        assert!(
            !has_runnable_llm(&config, tmp.path()),
            "expected false when .env key has empty value (T-35.1-01 mitigation)"
        );
    }

    #[test]
    fn has_runnable_llm_returns_true_when_dotenv_has_nonempty_key_value() {
        let _g = crate::test_env_lock();
        // SAFETY: clear env vars so only the .env file scan fires.
        unsafe {
            std::env::remove_var("OPENROUTER_API_KEY");
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
        }
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".env"), "OPENROUTER_API_KEY=sk-real\n").unwrap();
        let config = Config::default();
        assert!(
            has_runnable_llm(&config, tmp.path()),
            "expected true when .env contains a non-empty OPENROUTER_API_KEY"
        );
    }

    // -----------------------------------------------------------------------
    // Original emit_prereq_banner tests
    // -----------------------------------------------------------------------

    #[test]
    fn preflight_emits_banner_when_required_prereq_missing() {
        let active = vec![(
            "web_search".to_string(),
            vec![make_prereq("FIRECRAWL_API_KEY")],
        )];
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
        let all_unavailable = vec![(
            "web_search".to_string(),
            vec![make_prereq("FIRECRAWL_API_KEY")],
        )];
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
