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
use ironhermes_core::provider::main_provider_key_env_name;
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
///
/// Quick task 260820-5fu (T-5FU-01): names the env var actually consulted for
/// the configured main provider, resolved config-only via
/// [`main_provider_key_env_name`] (`crates/ironhermes-core/src/provider.rs`,
/// which performs no `std::env` access) — never the variable's VALUE. Before
/// this fix the message always named the three canonical vars regardless of
/// the configured provider, which was actively wrong advice for an operator
/// on e.g. `model.provider: groq`. When no name resolves (an unrecognized
/// custom provider with no `providers.<name>.api_key_env`), falls back to the
/// original three-name list unchanged. The leading sentence through the word
/// "detected" and the word "provider" are preserved byte-identically —
/// `doctor_integration.rs`'s loose substring assertion depends on it.
fn emit_non_interactive_llm_notice(config: &Config, out: &mut dyn std::io::Write) {
    let checked = match main_provider_key_env_name(config) {
        Some(name) => format!("{name}, a local config.model.base_url, and config.model.api_key"),
        None => "OPENROUTER_API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY, a local \
config.model.base_url, and config.model.api_key"
            .to_string(),
    };
    let _ = writeln!(
        out,
        "\u{26a0} No runnable LLM provider detected (checked {checked}) \
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
                        emit_non_interactive_llm_notice(&config, &mut std::io::stderr());
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
/// Four checks (RESEARCH Pitfall 6 — env vars FIRST because dotenvy::from_path
/// already ran in main.rs before preflight):
///
/// 0. The MAIN provider's configured key env var, resolved via
///    [`main_provider_key_env_name`] (`crates/ironhermes-core/src/provider.rs`)
///    — covers non-canonical providers (groq, mistral, deepseek, ...)
///    declared through `providers.<name>.api_key_env`, checked against both
///    the process env and the raw `.env` file. `None` falls straight through
///    to check 1 (quick task 260820-5fu).
/// 1. Post-dotenvy process env vars for the three canonical names — reads
///    the merged state (highest signal).
/// 2. Raw .env file scan for the same three canonical names —
///    belt-and-suspenders for edge cases where the env var was not loaded
///    into the process (e.g. sub-process launch contexts).
/// 3. Local endpoint in config.model.base_url — Ollama users with a localhost
///    base_url are NEVER prompted; this is the D-08 escape hatch.
/// 4. Deprecated `config.model.api_key` inline literal — still accepted by
///    `validate()`.
///
/// T-35.1-01 mitigation: `l.len() > key.len()` in the raw .env scan (check 2)
/// rejects lines like `OPENROUTER_API_KEY=` (empty value) that would
/// otherwise bypass detection and silently let a "bad" state through. Check 0
/// applies an equivalent trim-based emptiness test on the resolved variable.
fn has_runnable_llm(config: &Config, hermes_home: &std::path::Path) -> bool {
    // Check 0 (quick task 260820-5fu): the main provider's configured key env
    // var, resolved config-only via `main_provider_key_env_name`. Additive —
    // `None` (unrecognized custom provider with no api_key_env) falls
    // straight through to the existing checks below; never panics, never
    // short-circuits to `false`.
    if let Some(name) = main_provider_key_env_name(config) {
        if std::env::var(&name)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
        {
            return true;
        }
        let prefix = format!("{name}=");
        let env_path = hermes_home.join(".env");
        if env_path.exists()
            && let Ok(text) = std::fs::read_to_string(&env_path)
            && text
                .lines()
                .any(|l| l.starts_with(prefix.as_str()) && !l[prefix.len()..].trim().is_empty())
        {
            return true;
        }
    }
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
    // Quick task 260820-5fu: Task 1 — non-canonical provider via
    // providers.<main>.api_key_env, resolved through main_provider_key_env_name.
    // -----------------------------------------------------------------------

    #[test]
    fn has_runnable_llm_returns_true_for_non_canonical_provider_api_key_env_in_process_env() {
        let _g = crate::test_env_lock();
        // SAFETY: clear the canonical vars and the test-specific var so an
        // inherited developer key cannot make this pass for the wrong reason.
        unsafe {
            std::env::remove_var("OPENROUTER_API_KEY");
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
            std::env::set_var("GROQ_API_KEY", "gsk-test-value");
        }
        let mut config = Config::default();
        config.model.provider = "groq".to_string();
        config.providers.insert(
            "groq".to_string(),
            ironhermes_core::config::ProviderConfig {
                api_key_env: Some("GROQ_API_KEY".to_string()),
                ..Default::default()
            },
        );
        let tmp = TempDir::new().unwrap();
        let result = has_runnable_llm(&config, tmp.path());
        // SAFETY: restore env.
        unsafe { std::env::remove_var("GROQ_API_KEY") };
        assert!(
            result,
            "expected true when the main provider's providers.<name>.api_key_env \
             variable is exported non-empty, even though it is not one of the \
             three canonical names"
        );
    }

    // -----------------------------------------------------------------------
    // Quick task 260820-5fu: Task 2 — pin the edges (empty/whitespace values,
    // None fallback, the additive guarantee, and untouched escape hatches).
    // -----------------------------------------------------------------------

    fn groq_config_with_api_key_env() -> Config {
        let mut config = Config::default();
        config.model.provider = "groq".to_string();
        config.providers.insert(
            "groq".to_string(),
            ironhermes_core::config::ProviderConfig {
                api_key_env: Some("GROQ_API_KEY".to_string()),
                ..Default::default()
            },
        );
        config
    }

    fn clear_canonical_and_groq_env() {
        // SAFETY: test-only env mutation; serialised by env_lock (caller holds it).
        unsafe {
            std::env::remove_var("OPENROUTER_API_KEY");
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("GROQ_API_KEY");
        }
    }

    #[test]
    fn has_runnable_llm_returns_true_when_dotenv_has_provider_api_key_env_value() {
        let _g = crate::test_env_lock();
        clear_canonical_and_groq_env();
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".env"), "GROQ_API_KEY=gsk-real\n").unwrap();
        let config = groq_config_with_api_key_env();
        assert!(
            has_runnable_llm(&config, tmp.path()),
            "expected true when providers.groq.api_key_env's variable is present \
             non-empty only in .env, symmetric with the process-env case"
        );
    }

    #[test]
    fn has_runnable_llm_returns_false_when_provider_api_key_env_value_is_empty() {
        let _g = crate::test_env_lock();
        clear_canonical_and_groq_env();
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".env"), "GROQ_API_KEY=\n").unwrap();
        let config = groq_config_with_api_key_env();
        assert!(
            !has_runnable_llm(&config, tmp.path()),
            "expected false when the .env value for the resolved provider key is empty"
        );
    }

    #[test]
    fn has_runnable_llm_returns_false_when_provider_api_key_env_value_is_whitespace_only() {
        let _g = crate::test_env_lock();
        clear_canonical_and_groq_env();
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".env"), "GROQ_API_KEY=   \n").unwrap();
        let config = groq_config_with_api_key_env();
        assert!(
            !has_runnable_llm(&config, tmp.path()),
            "expected false when the .env value for the resolved provider key is \
             whitespace-only (stricter trim-based test than the existing \
             length-comparison check, applied only on the new path)"
        );
    }

    #[test]
    fn has_runnable_llm_returns_false_when_provider_api_key_env_process_value_is_whitespace_only() {
        let _g = crate::test_env_lock();
        clear_canonical_and_groq_env();
        // SAFETY: test-only env mutation; serialised by env_lock.
        unsafe { std::env::set_var("GROQ_API_KEY", " ") };
        let config = groq_config_with_api_key_env();
        let tmp = TempDir::new().unwrap();
        let result = has_runnable_llm(&config, tmp.path());
        // SAFETY: restore env.
        unsafe { std::env::remove_var("GROQ_API_KEY") };
        assert!(
            !result,
            "expected false when the exported provider key value is whitespace-only"
        );
    }

    #[test]
    fn has_runnable_llm_returns_false_when_main_provider_has_no_resolvable_key_name() {
        let _g = crate::test_env_lock();
        clear_canonical_and_groq_env();
        let mut config = Config::default();
        config.model.provider = "totally_unknown".to_string();
        // No `providers` entry for "totally_unknown" and it is not one of the
        // three canonical names, so main_provider_key_env_name returns None.
        let tmp = TempDir::new().unwrap();
        // No .env file, no base_url, no model.api_key.
        assert!(
            !has_runnable_llm(&config, tmp.path()),
            "expected false (no panic) when main_provider_key_env_name resolves to None"
        );
    }

    #[test]
    fn has_runnable_llm_returns_true_for_exported_canonical_key_without_providers_entry() {
        let _g = crate::test_env_lock();
        clear_canonical_and_groq_env();
        // SAFETY: test-only env mutation; serialised by env_lock.
        unsafe { std::env::set_var("OPENROUTER_API_KEY", "sk-abc") };
        // Default config: provider = "openrouter", providers map EMPTY — the
        // additive guarantee: today's behaviour must survive unchanged. Both
        // the new check 0 (via the canonical fallback in
        // main_provider_key_env_name) and the existing check 1 resolve to the
        // same OPENROUTER_API_KEY name here, so they agree.
        let config = Config::default();
        assert!(config.providers.is_empty());
        let tmp = TempDir::new().unwrap();
        let result = has_runnable_llm(&config, tmp.path());
        // SAFETY: restore env.
        unsafe { std::env::remove_var("OPENROUTER_API_KEY") };
        assert!(
            result,
            "expected true for an exported canonical key with an empty providers map \
             (today's behaviour, must survive additively)"
        );
    }

    #[test]
    fn has_runnable_llm_returns_true_for_untouched_local_base_url_escape_hatch_with_no_api_key_env()
    {
        let _g = crate::test_env_lock();
        clear_canonical_and_groq_env();
        // groq main provider with NO api_key_env anywhere — main_provider_key_env_name
        // resolves to None (groq is not canonical and has no providers entry), so
        // check 0 must fall straight through to check 3 (local base_url).
        let mut config = Config::default();
        config.model.provider = "groq".to_string();
        config.model.base_url = Some("http://localhost:11434".to_string());
        let tmp = TempDir::new().unwrap();
        assert!(
            has_runnable_llm(&config, tmp.path()),
            "expected true via the untouched local base_url escape hatch, unaffected \
             by check 0 resolving to None"
        );
    }

    #[test]
    fn has_runnable_llm_returns_true_for_untouched_deprecated_model_api_key_with_no_api_key_env() {
        let _g = crate::test_env_lock();
        clear_canonical_and_groq_env();
        // Same as above, but via the deprecated config.model.api_key escape hatch.
        let mut config = Config::default();
        config.model.provider = "groq".to_string();
        config.model.api_key = Some("sk-legacy".to_string());
        let tmp = TempDir::new().unwrap();
        assert!(
            has_runnable_llm(&config, tmp.path()),
            "expected true via the untouched deprecated model.api_key escape hatch, \
             unaffected by check 0 resolving to None"
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

    // -----------------------------------------------------------------------
    // Quick task 260820-5fu: Task 3 — emit_non_interactive_llm_notice names
    // the variable it actually checked, and never a value (T-5FU-01).
    // -----------------------------------------------------------------------

    #[test]
    fn emit_non_interactive_llm_notice_names_resolved_provider_variable() {
        let config = groq_config_with_api_key_env();
        let mut buf: Vec<u8> = Vec::new();
        emit_non_interactive_llm_notice(&config, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.contains("GROQ_API_KEY"),
            "notice must name the resolved provider variable, got: {output}"
        );
    }

    #[test]
    fn emit_non_interactive_llm_notice_still_names_openrouter_for_default_config() {
        let config = Config::default();
        let mut buf: Vec<u8> = Vec::new();
        emit_non_interactive_llm_notice(&config, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.contains("OPENROUTER_API_KEY"),
            "notice must still name OPENROUTER_API_KEY for the default config, got: {output}"
        );
    }

    #[test]
    fn emit_non_interactive_llm_notice_falls_back_and_does_not_panic_for_unresolvable_provider() {
        let mut config = Config::default();
        config.model.provider = "totally_unknown".to_string();
        let mut buf: Vec<u8> = Vec::new();
        emit_non_interactive_llm_notice(&config, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.contains("No runnable LLM provider detected"),
            "notice must still render for an unrecognized custom provider, got: {output}"
        );
        assert!(
            !output.contains("totally_unknown"),
            "notice must omit any per-provider variable name when none resolves, got: {output}"
        );
    }

    #[test]
    fn emit_non_interactive_llm_notice_never_contains_a_key_value() {
        // The function is config-only and never reads std::env — this test
        // documents that guarantee even though there is no value to leak from
        // a Config alone. A dummy secret is asserted absent as a belt-and-
        // suspenders check against future regressions.
        let config = groq_config_with_api_key_env();
        let mut buf: Vec<u8> = Vec::new();
        emit_non_interactive_llm_notice(&config, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        const DUMMY_SECRET: &str = "gsk-dummy-secret-value-should-never-appear";
        assert!(
            output.contains("GROQ_API_KEY"),
            "notice must contain the variable NAME, got: {output}"
        );
        assert!(
            !output.contains(DUMMY_SECRET),
            "notice must never contain a key VALUE, got: {output}"
        );
    }

    #[test]
    fn emit_non_interactive_llm_notice_preserves_leading_sentence_and_provider_word() {
        let config = groq_config_with_api_key_env();
        let mut buf: Vec<u8> = Vec::new();
        emit_non_interactive_llm_notice(&config, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.starts_with("\u{26a0} No runnable LLM provider detected"),
            "leading sentence through 'detected' must be byte-identical \
             (doctor_integration.rs depends on the substring), got: {output}"
        );
        assert!(
            output.contains("provider"),
            "the word 'provider' must still appear, got: {output}"
        );
    }
}
