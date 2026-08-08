//! `hermes doctor` — configuration and dependency health check.
//!
//! Extracted from main.rs `cmd_doctor` (Phase 35.1 D-03).
//!
//! Called from `setup::run_setup` at wizard exit and from `main` dispatch.

use anyhow::Result;
use colored::Colorize;
use ironhermes_core::config::{
    Config, WEB_ANSWER_PROVIDERS, WEB_EXTRACT_PROVIDERS, WEB_SEARCH_PROVIDERS,
};
use ironhermes_tools::credentials::ToolCredentials;

/// Run the IronHermes doctor preflight check.
///
/// Prints a formatted health summary covering home directory, config file,
/// .env file, API keys, state database, and gateway PID liveness. Always
/// returns `Ok(())` — issues are reported as `MISSING` lines, not errors.
///
/// Phase 41.3 Plan 09 (D-19): `async` because reporting web-tool provider
/// coverage requires resolving the same env -> config -> vault credential
/// snapshot the runtime resolves at startup (`ToolCredentials::resolve` is
/// itself `async` — the vault's `SecretStore::get_secret` is async). Both
/// call sites (`main.rs`, `setup.rs`) already run inside an async fn.
pub async fn run_doctor_check() -> Result<()> {
    println!("{}", "IronHermes Doctor".bold().cyan());
    // Phase 24 D-16: show which profile this doctor run is inspecting.
    println!("Profile: {}", ironhermes_cli::status_cmd::current_profile());
    println!("{}", "─".repeat(40));

    // Check home directory
    let home = ironhermes_core::get_hermes_home();
    print_check("Home directory", home.exists());

    // Check config
    let config_path = Config::config_path();
    print_check("Config file", config_path.exists());

    // Check .env
    let env_path = Config::env_path();
    print_check(".env file", env_path.exists());

    // Check API keys
    print_check(
        "OpenRouter API key",
        std::env::var("OPENROUTER_API_KEY").is_ok(),
    );
    print_check(
        "Anthropic API key",
        std::env::var("ANTHROPIC_API_KEY").is_ok(),
    );

    // Phase 41.3 Plan 09 (D-08/D-19): web-tool provider coverage — resolved
    // from the SAME env -> config -> vault snapshot the runtime resolves at
    // startup (never a direct `std::env::var` read here), so this section
    // cannot disagree with what the model actually sees. A sealed/unreachable
    // vault is reported as a failed check, never a panic or a propagated
    // `Err` — `doctor` is the one place an operator can diagnose that
    // condition without booting the agent (D-19).
    let doctor_config = Config::load().unwrap_or_default();
    let (tool_credentials, vault_failed_checks) = resolve_doctor_credentials(&doctor_config).await;
    for msg in &vault_failed_checks {
        print_check(msg, false);
    }
    let tool_credentials = std::sync::Arc::new(tool_credentials);
    // `register_defaults()` builds `WebSearchTool`/`WebAnswerTool` from
    // whatever snapshot is installed on the registry at call time —
    // `with_credentials` MUST run before `register_defaults()`, or the
    // constructed tools would carry the placeholder env-only default instead.
    let mut provider_registry = ironhermes_tools::ToolRegistry::new();
    provider_registry.with_credentials(tool_credentials.clone());
    provider_registry.register_defaults();
    println!();
    println!("{}", "Web-tool provider coverage:".bold());
    for line in render_tool_provider_status(&provider_registry, &tool_credentials) {
        println!("  {line}");
    }

    // Phase 41.3 Plan 09 (D-08): a misconfigured chain (unknown provider name,
    // empty chain) is a doctor-reported configuration error, so a typo is
    // caught before a turn silently falls through the whole chain.
    for msg in render_chain_validation(&doctor_config) {
        print_check(&msg, false);
    }

    // Phase 46.8 Plan 05 (D-03): vault subsystem status block.
    let vault_config = &doctor_config;
    print_vault_status(vault_config);

    // Phase 46.5 D-07: vision-capable model check (kanban image tasks).
    // Doctor-only visibility per research approach (a) — `model.vision_model`
    // stays unwired dead code; this reports on the effective MAIN model, which
    // is already vision-capable by default (anthropic/claude-sonnet-4).
    let vision_model_ok = Config::load()
        .map(|c| is_vision_capable_model(&c.model.default))
        .unwrap_or(false);
    print_check("Vision-capable model (kanban image tasks)", vision_model_ok);

    // Check state database
    let db_path = home.join("state.db");
    print_check("State database", db_path.exists());

    // Phase 24 D-16: gateway.pid liveness check (active profile only — no
    // cross-profile sweep per the deferred-ideas list).
    let pid_path = home.join("gateway.pid");
    if pid_path.exists() {
        let pid_ok = ironhermes_gateway::pid::read_gateway_pid(&home)
            .ok()
            .flatten()
            .map(|r| {
                matches!(
                    ironhermes_gateway::pid::is_pid_alive(r.pid),
                    ironhermes_gateway::pid::PidLiveness::Live
                        | ironhermes_gateway::pid::PidLiveness::LiveOtherUser
                )
            })
            .unwrap_or(false);
        print_check("Gateway PID (gateway.pid → live process)", pid_ok);
    } else {
        // Absent file = healthy (no gateway running). Use the "OK" branch.
        print_check("Gateway PID (not running)", true);
    }

    // WR-03 (phase 47.4): flag profiles that may have been exfiltrated during the
    // CR-03 window, before plan 20 closed the `${...}` substitution path. A
    // pre-patch exploit persisted root's plaintext credential into the victim
    // profile's `.env`, and nothing else surfaces that. Detection only — a
    // disclosed credential is remediated by ROTATION, never by editing the file.
    print_profile_exposure_audit(&home);

    println!();
    println!("{}", "Run `ironhermes status` for more details.".dimmed());

    Ok(())
}

/// Report CR-03-window credential exposure across all profiles (WR-03).
///
/// Prints key NAMES only — never a value. See `profile_audit` for the detection
/// rules and for why same-name inheritance is deliberately not flagged.
fn print_profile_exposure_audit(home: &std::path::Path) {
    let profiles_root = home.join(ironhermes_core::PROFILES_SUBDIR);
    let findings =
        ironhermes_cli::profile_audit::audit_profiles_at(&profiles_root, &home.join(".env"));

    if findings.is_empty() {
        print_check("Profile credential exposure (CR-03 window)", true);
        return;
    }

    // NOT print_check's `false` branch: that renders "MISSING", which an operator
    // reads as "this check could not run" — the opposite of what a hit means here.
    println!(
        "  [{}] Profile credential exposure (CR-03 window)",
        "EXPOSED".red().bold()
    );
    for line in ironhermes_cli::profile_audit::render_findings(&findings) {
        if line.is_empty() {
            println!();
        } else if line.starts_with("    ") {
            println!("  {}", line.yellow());
        } else if line.starts_with("ROTATE") {
            println!("  {}", line.red().bold());
        } else {
            println!("  {line}");
        }
    }
}

fn print_check(name: &str, ok: bool) {
    let icon = if ok { "OK".green() } else { "MISSING".yellow() };
    println!("  [{icon}] {name}");
}

// -----------------------------------------------------------------------
// Phase 41.3 Plan 09 (D-08/D-19): web-tool provider coverage + chain
// validation. Pure render functions return lines rather than printing
// directly, so tests can assert on rendered content without capturing
// stdout — `run_doctor_check` above is the only `println!` call site.
// -----------------------------------------------------------------------

/// D-19: resolve the exact env -> config -> vault credential snapshot the
/// runtime resolves at `build_app_runtime_bundle` — only opening the vault
/// store when the operator enabled it (T-41.3-53: an operator who never
/// turned the vault on can never be stopped by it, and `doctor` must not
/// diverge from that on the open side either). Unlike startup, a resolution
/// failure here is never allowed to escape as an `Err` (D-19: doctor must
/// survive the exact condition it exists to diagnose) — it is instead
/// rendered as one failed-check line naming the configured backend, and the
/// returned snapshot degrades to env/config-tier-only so the rest of this
/// section still reports something real rather than nothing.
async fn resolve_doctor_credentials(config: &Config) -> (ToolCredentials, Vec<String>) {
    if !config.vault.enabled {
        // No store to open — `resolve(config, None)` never reaches the vault
        // arm, so this branch cannot itself produce a sealed-vault failure.
        let creds = ToolCredentials::resolve(config, None)
            .await
            .unwrap_or_default();
        return (creds, Vec::new());
    }

    match ironhermes_vault::open_store(&crate::vault_cmd::resolve_vault_config(config)) {
        Ok(store) => resolve_doctor_credentials_with_store(config, Some(store.as_ref())).await,
        Err(_e) => {
            // `open_store` itself failed (unknown backend, feature not
            // compiled in, etc.) — never format `_e` verbatim here; the
            // backend name alone identifies the problem without risking any
            // wrapped context string that a future error variant might add.
            let creds = ToolCredentials::resolve(config, None)
                .await
                .unwrap_or_default();
            (
                creds,
                vec![format!(
                    "Vault ({}): failed to open the configured backend — tool credentials \
                     resolved from env/config tiers only",
                    config.vault.backend
                )],
            )
        }
    }
}

/// D-19: the store-taking half of [`resolve_doctor_credentials`], split out
/// so tests can drive it with a fake `SecretStore` (sealed, corrupt, or
/// healthy) without going through real backend selection. Mirrors
/// `build_app_runtime_bundle`'s hard-error posture at the boundary — but
/// converts the `Err` into one failed-check line instead of propagating it,
/// per this file's own doc comment: doctor never errors.
async fn resolve_doctor_credentials_with_store(
    config: &Config,
    store: Option<&dyn ironhermes_vault::SecretStore>,
) -> (ToolCredentials, Vec<String>) {
    match ToolCredentials::resolve(config, store).await {
        Ok(creds) => (creds, Vec::new()),
        Err(_e) => {
            // T-41.3-62: never format `_e` — `sealed_vault_error_carries_no_secret_material`
            // (Plan 10) already proves the underlying error string is safe, but this
            // file's own discipline is to name only the backend, never any wrapped
            // context. Degrade to an env/config-tier-only snapshot (no store) so the
            // rest of this section still reports something real.
            let creds = ToolCredentials::resolve(config, None)
                .await
                .unwrap_or_default();
            (
                creds,
                vec![format!(
                    "Vault ({}): tool-credential resolution failed (sealed, locked, or \
                     unreachable) — reporting env/config tiers only",
                    config.vault.backend
                )],
            )
        }
    }
}

/// D-08: `tool_name` -> its legal provider chain in `ironhermes_core::config`
/// — the SAME const arrays `ToolsConfig::validate_chains()` reads. Doctor
/// reads the array's length only (never restates its contents) to catch a
/// future drift class the group-count rendering alone cannot: an operator
/// adding a 5th legal provider to one of these arrays without also adding a
/// matching `Prerequisite::grouped_env_var` would otherwise silently make
/// that provider unconfigurable through the wizard/doctor surface. Returns
/// `None` for a tool name this phase does not know about (no cross-check).
fn legal_provider_chain_len(tool_name: &str) -> Option<usize> {
    match tool_name {
        "web_search" => Some(WEB_SEARCH_PROVIDERS.len()),
        "web_answer" => Some(WEB_ANSWER_PROVIDERS.len()),
        "web_extract" => Some(WEB_EXTRACT_PROVIDERS.len()),
        _ => None,
    }
}

/// D-08/D-19: one rendered line per provider group across every registered
/// tool — `<tool>: <satisfied>/<total> providers configured`, using
/// `group_members`/`satisfied_group_members_with_credentials` (Plan 06/09) so
/// this count agrees with what `is_available()` would answer for the same
/// credential state by construction, not by coincidence. Never formats a
/// credential value — only env-var **names** and the **tier** that satisfied
/// each (`source_of`), matching this file's existing `Vault sealed-state`
/// idiom of reporting shape/provenance, never content (T-41.3-44).
fn render_tool_provider_status(
    registry: &ironhermes_tools::ToolRegistry,
    credentials: &ToolCredentials,
) -> Vec<String> {
    use ironhermes_tools::registry::{group_members, satisfied_group_members_with_credentials};

    let mut lines = Vec::new();
    for name in registry.list_tools() {
        let Some(tool) = registry.get(name) else {
            continue;
        };
        let prereqs = tool.prerequisites();
        let mut groups: Vec<&str> = prereqs.iter().filter_map(|p| p.group.as_deref()).collect();
        groups.sort_unstable();
        groups.dedup();

        for group in groups {
            let members = group_members(&prereqs, group);
            let satisfied =
                satisfied_group_members_with_credentials(&prereqs, group, Some(credentials));

            let mut configured = Vec::new();
            let mut missing = Vec::new();
            for m in &members {
                if credentials.has_credential(&m.name) {
                    let tier = credentials
                        .source_of(&m.name)
                        .map(|s| format!("{s:?}"))
                        .unwrap_or_else(|| "unknown".to_string());
                    configured.push(format!("{} ({tier})", m.name));
                } else {
                    missing.push(m.name.as_str());
                }
            }

            let mut line = format!("{name}: {satisfied}/{} providers configured", members.len());
            if !configured.is_empty() {
                line.push_str(&format!(" — configured: {}", configured.join(", ")));
            }
            if !missing.is_empty() {
                line.push_str(&format!(" — set to add: {}", missing.join(", ")));
            }
            if satisfied == 0 && tool.is_available() {
                line.push_str(" (tool remains available via its keyless default)");
            }
            // D-08 drift check: a keyed group member count that falls short
            // of the tool's own legal provider chain length (config.rs's
            // WEB_*_PROVIDERS array, minus exactly one keyless terminal per
            // D-09/D-10) means a provider was added to the chain's legal
            // values without a matching wizard/doctor-visible prerequisite —
            // it would silently be unconfigurable through this surface.
            if let Some(legal_len) = legal_provider_chain_len(name)
                && members.len() + 1 < legal_len
            {
                line.push_str(&format!(
                    " [drift: only {} of {} legal chain providers have a prerequisite entry]",
                    members.len(),
                    legal_len
                ));
            }
            lines.push(line);
        }
    }
    lines
}

/// D-08: one failed-check line per `ToolsConfig::validate_chains()` problem —
/// each message already names both the offending provider value and the
/// dotted key path (`unknown provider '<value>' in tools.<tool>.chain`), so a
/// typo is findable without bisecting. A valid config renders nothing.
fn render_chain_validation(config: &Config) -> Vec<String> {
    match config.tools.validate_chains() {
        Ok(()) => Vec::new(),
        Err(problems) => problems,
    }
}

/// Phase 46.8 Plan 05 (D-03): report vault enabled / backend / data-dir /
/// sealed-state via `print_check` — informational only, never a secret value
/// (D-15). Always returns (no `Result`) — any store-open failure while
/// deriving `sealed-state` is caught and printed as a status string, never
/// propagated (doctor never errors, per this file's own doc comment).
fn print_vault_status(config: &Config) {
    let vault_cfg = &config.vault;

    print_check("Vault enabled", vault_cfg.enabled);
    print_check(&format!("Vault backend: {}", vault_cfg.backend), true);

    let data_dir = crate::vault_cmd::resolve_vault_config(config)
        .rusty_vault
        .data_dir;
    print_check(
        &format!("Vault data-dir present ({})", data_dir.display()),
        data_dir.exists(),
    );

    let sealed_state = vault_sealed_state(vault_cfg, &data_dir);
    let sealed_ok = sealed_state == "unsealed" || sealed_state == "n/a";
    print_check(&format!("Vault sealed-state: {sealed_state}"), sealed_ok);
}

/// Derive the rusty-vault backend's sealed-state by attempting
/// `RustyVaultStore::open` at the resolved data dir, then querying the
/// opened store's ACTUAL seal state via `is_sealed()` (WR-01, 46.8-gap) —
/// NOT by inferring "unsealed" from `open()` returning `Ok`. `open()`
/// returns `Ok` even in `unseal_mode = "passphrase"`, where it deliberately
/// leaves the underlying `Core` sealed until an operator runs
/// `ironhermes vault unlock`; treating that `Ok` as "unsealed" was WR-01's
/// false-positive bug. An `open()` `Err` whose message names
/// "sealed"/"not initialized" still maps to that state for backward
/// compatibility with pre-init/corrupt-keyfile cases; anything else is
/// reported verbatim as an opaque backend error string (never a secret
/// value; `VaultError`'s `Display` impl is static/code-shaped by
/// construction, D-08/D-15 safe to format).
#[cfg(feature = "rusty-vault")]
fn vault_sealed_state(
    vault_cfg: &ironhermes_vault::VaultConfig,
    data_dir: &std::path::Path,
) -> String {
    if vault_cfg.backend != "rusty-vault" {
        return "n/a".to_string();
    }
    let mut rusty_vault_cfg = vault_cfg.rusty_vault.clone();
    rusty_vault_cfg.data_dir = data_dir.to_path_buf();

    match ironhermes_vault::RustyVaultStore::open(&rusty_vault_cfg) {
        Ok(store) => match store.is_sealed() {
            Ok(true) => "sealed".to_string(),
            Ok(false) => "unsealed".to_string(),
            Err(e) => format!("error: {e}"),
        },
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not initialized") {
                "not initialized".to_string()
            } else if msg.contains("sealed") {
                "sealed".to_string()
            } else {
                format!("error: {msg}")
            }
        }
    }
}

#[cfg(not(feature = "rusty-vault"))]
fn vault_sealed_state(
    vault_cfg: &ironhermes_vault::VaultConfig,
    _data_dir: &std::path::Path,
) -> String {
    if vault_cfg.backend == "rusty-vault" {
        "rusty-vault feature not compiled in".to_string()
    } else {
        "n/a".to_string()
    }
}

/// Phase 46.5 D-07: is `model` (an OpenRouter-style `provider/model-id`
/// string, or a bare model id) on the known vision-capable allowlist?
///
/// Doctor-only surfacing (research approach (a)) — this does NOT wire
/// `model.vision_model` (verified dead code, RESEARCH.md Pitfall 1). It
/// reports on the effective MAIN model resolved from `config.model.default`.
///
/// Matched by case-insensitive substring against the current (2026-07)
/// OpenRouter vision-model ecosystem: Claude 3/4.x family, GPT-4o/4.1/5,
/// Gemini, Grok vision variants, GLM-4V/4.6V, Pixtral, Qwen-VL.
fn is_vision_capable_model(model: &str) -> bool {
    let m = model.to_lowercase();
    const VISION_ALLOWLIST: &[&str] = &[
        "claude-3",
        "claude-4",
        "claude-sonnet",
        "claude-opus",
        "claude-haiku",
        "gpt-4o",
        "gpt-4.1",
        "gpt-5",
        "gemini",
        "grok-2-vision",
        "grok-4",
        "grok-vision",
        "glm-4v",
        "glm-4.6v",
        "pixtral",
        "qwen-vl",
    ];
    VISION_ALLOWLIST.iter().any(|needle| m.contains(needle))
        || (m.contains("qwen") && m.contains("vl"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, OnceLock};

    use ironhermes_core::config::Config;
    use secrecy::SecretString;

    #[test]
    fn is_vision_capable_model_true_cases() {
        assert!(is_vision_capable_model("anthropic/claude-sonnet-4"));
        assert!(is_vision_capable_model("openai/gpt-4o"));
        assert!(is_vision_capable_model("google/gemini-2.0-flash"));
    }

    #[test]
    fn is_vision_capable_model_false_cases() {
        assert!(!is_vision_capable_model("mistralai/mistral-7b-instruct"));
        assert!(!is_vision_capable_model("deepseek/deepseek-chat"));
    }

    // -----------------------------------------------------------------
    // Phase 41.3 Plan 09 (D-08/D-19): web-tool provider coverage tests.
    //
    // An async-aware `tokio::sync::Mutex`, not `std::sync::Mutex` — every
    // test here is `#[tokio::test]` and holds the guard across
    // `ToolCredentials::resolve(...).await`, which `clippy::await_holding_lock`
    // (denied under this workspace's `-D warnings` gate) forbids for a std
    // Mutex guard. Mirrors `tests/tool_credentials.rs`'s (Plan 10) identical
    // convention.
    // -----------------------------------------------------------------

    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    /// Remove every `TOOL_CREDENTIAL_KEYS` member from the process env so a
    /// developer's real `.env`/shell exports never leak into these tests —
    /// mirrors `tool_credentials.rs`'s `clear_all_credential_env_vars`.
    fn clear_all_web_provider_env_vars() {
        for key in ironhermes_tools::credentials::TOOL_CREDENTIAL_KEYS {
            // SAFETY: caller holds env_lock(); single-threaded test binary
            // (--test-threads=1, project-wide convention for env-mutating tests).
            unsafe { std::env::remove_var(key) };
        }
    }

    #[derive(Clone)]
    enum FakeVaultResponse {
        Found(String),
        Sealed,
    }

    /// Test-only `SecretStore` — mirrors `tests/tool_credentials.rs`'s
    /// `MockStore` shape, trimmed to what `resolve_doctor_credentials_with_store`
    /// actually calls (`get_secret` only).
    struct FakeVaultStore {
        responses: HashMap<String, FakeVaultResponse>,
    }

    #[async_trait::async_trait]
    impl ironhermes_vault::SecretStore for FakeVaultStore {
        async fn get_secret(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
            match self.responses.get(key) {
                Some(FakeVaultResponse::Found(v)) => Ok(Some(SecretString::from(v.clone()))),
                Some(FakeVaultResponse::Sealed) => Err(anyhow::anyhow!(
                    "vault is sealed — run `ironhermes vault unlock`"
                )),
                None => Ok(None),
            }
        }
        async fn put_secret(&self, _key: &str, _value: SecretString) -> anyhow::Result<()> {
            unimplemented!("doctor never writes secrets")
        }
        async fn delete_secret(&self, _key: &str) -> anyhow::Result<()> {
            unimplemented!("doctor never deletes secrets")
        }
        async fn list_secrets(&self, _prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
            unimplemented!("doctor never lists secrets")
        }
    }

    /// Build a registry with `credentials` installed and defaults registered
    /// — the exact order `run_doctor_check` itself follows (`with_credentials`
    /// before `register_defaults`, or `WebSearchTool`/`WebAnswerTool` would be
    /// constructed against the placeholder env-only snapshot instead).
    fn registry_with_credentials(
        credentials: Arc<ToolCredentials>,
    ) -> ironhermes_tools::ToolRegistry {
        let mut registry = ironhermes_tools::ToolRegistry::new();
        registry.with_credentials(credentials);
        registry.register_defaults();
        registry
    }

    #[tokio::test]
    async fn tool_provider_status_renders_a_group_count() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();
        // SAFETY: env_lock held, --test-threads=1.
        unsafe { std::env::set_var("EXA_API_KEY", "test-value") };

        let config = Config::default();
        let creds = Arc::new(
            ToolCredentials::resolve(&config, None)
                .await
                .expect("resolve must succeed with no store"),
        );
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);

        unsafe { std::env::remove_var("EXA_API_KEY") };

        let web_search_line = lines
            .iter()
            .find(|l| l.starts_with("web_search:"))
            .unwrap_or_else(|| panic!("expected a web_search line, got: {lines:?}"));
        assert!(
            web_search_line.contains("1/3"),
            "one of three web_search providers configured -> \"1/3\": {web_search_line}"
        );
    }

    #[tokio::test]
    async fn tool_provider_status_reports_zero_without_hard_failing() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();

        let config = Config::default();
        let creds = Arc::new(
            ToolCredentials::resolve(&config, None)
                .await
                .expect("resolve must succeed with no store"),
        );
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);

        let web_search_line = lines
            .iter()
            .find(|l| l.starts_with("web_search:"))
            .unwrap_or_else(|| panic!("expected a web_search line, got: {lines:?}"));
        assert!(
            web_search_line.contains("0/3"),
            "zero providers configured -> \"0/3\": {web_search_line}"
        );
        assert!(
            web_search_line.contains("keyless default"),
            "a 0/3 must note the tool stays available (D-09), not read as broken: {web_search_line}"
        );
    }

    #[test]
    fn chain_validation_reports_an_unknown_provider() {
        let mut config = Config::default();
        config.tools.web_search.chain = vec!["not_a_real_provider".to_string()];

        let lines = render_chain_validation(&config);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("not_a_real_provider") && l.contains("tools.web_search.chain")),
            "must name both the offending value and the key path: {lines:?}"
        );
    }

    #[test]
    fn chain_validation_is_silent_on_a_valid_config() {
        let config = Config::default();
        assert!(
            render_chain_validation(&config).is_empty(),
            "a default (valid) config must produce no chain-validation lines"
        );
    }

    #[tokio::test]
    async fn provider_status_counts_a_key_configured_only_in_the_config_tier() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();

        let mut config = Config::default();
        config
            .tools
            .credentials
            .insert("BRAVE_API_KEY".to_string(), "config-tier-value".to_string());

        let creds = Arc::new(
            ToolCredentials::resolve(&config, None)
                .await
                .expect("resolve must succeed with no store"),
        );
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);

        let web_search_line = lines
            .iter()
            .find(|l| l.starts_with("web_search:"))
            .unwrap_or_else(|| panic!("expected a web_search line, got: {lines:?}"));
        assert!(
            web_search_line.contains("1/3"),
            "a config-tier-only key must be counted, not treated as a false alarm (D-19): {web_search_line}"
        );
        assert!(
            web_search_line.contains("BRAVE_API_KEY (Config)"),
            "the satisfying tier must be named: {web_search_line}"
        );
    }

    #[tokio::test]
    async fn provider_status_counts_a_key_configured_only_in_the_vault() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();

        let config = Config::default();
        let mut responses = HashMap::new();
        responses.insert(
            "TAVILY_API_KEY".to_string(),
            FakeVaultResponse::Found("vault-tier-value".to_string()),
        );
        let store = FakeVaultStore { responses };

        let (creds, failed) = resolve_doctor_credentials_with_store(&config, Some(&store)).await;
        assert!(
            failed.is_empty(),
            "a healthy vault must not produce a failed check: {failed:?}"
        );

        let creds = Arc::new(creds);
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);

        let web_search_line = lines
            .iter()
            .find(|l| l.starts_with("web_search:"))
            .unwrap_or_else(|| panic!("expected a web_search line, got: {lines:?}"));
        assert!(
            web_search_line.contains("1/3"),
            "a vault-tier-only key must be counted (D-19): {web_search_line}"
        );
        assert!(
            web_search_line.contains("TAVILY_API_KEY (Vault)"),
            "the satisfying tier must be named: {web_search_line}"
        );
    }

    #[tokio::test]
    async fn provider_status_never_prints_a_configured_value() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();
        // SAFETY: env_lock held, --test-threads=1.
        unsafe { std::env::set_var("EXA_API_KEY", "sentinel-env-9f3a") };

        let mut config = Config::default();
        config.tools.credentials.insert(
            "BRAVE_API_KEY".to_string(),
            "sentinel-config-7b21".to_string(),
        );
        let mut responses = HashMap::new();
        responses.insert(
            "TAVILY_API_KEY".to_string(),
            FakeVaultResponse::Found("sentinel-vault-c405".to_string()),
        );
        let store = FakeVaultStore { responses };

        let (creds, _failed) = resolve_doctor_credentials_with_store(&config, Some(&store)).await;
        let creds = Arc::new(creds);
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);

        unsafe { std::env::remove_var("EXA_API_KEY") };

        let rendered = lines.join("\n");
        for sentinel in [
            "sentinel-env-9f3a",
            "sentinel-config-7b21",
            "sentinel-vault-c405",
        ] {
            assert!(
                !rendered.contains(sentinel),
                "rendered output must never contain a configured value; found {sentinel} in: {rendered}"
            );
        }
        // Positive control: prove all three tiers were actually exercised, so
        // the negative assertion above isn't vacuously true.
        let web_search_line = lines
            .iter()
            .find(|l| l.starts_with("web_search:"))
            .unwrap_or_else(|| panic!("expected a web_search line, got: {lines:?}"));
        assert!(
            web_search_line.contains("3/3"),
            "all three tiers (env/config/vault) must be counted: {web_search_line}"
        );
    }

    #[tokio::test]
    async fn a_sealed_vault_is_a_failed_check_not_a_panic() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();

        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();

        let mut responses = HashMap::new();
        responses.insert("EXA_API_KEY".to_string(), FakeVaultResponse::Sealed);
        let store = FakeVaultStore { responses };

        // Must not panic and must not propagate an Err — doctor never errors.
        let (creds, failed) = resolve_doctor_credentials_with_store(&config, Some(&store)).await;

        assert!(
            !failed.is_empty(),
            "a sealed vault must render a failed check, not silence"
        );
        assert!(
            failed.iter().any(|m| m.contains("rusty-vault")),
            "the failed check must name the configured backend: {failed:?}"
        );
        for msg in &failed {
            assert!(
                !msg.to_lowercase().contains("sentinel"),
                "a failed-check message must never carry secret material: {msg}"
            );
        }

        // Doctor keeps going with a degraded snapshot — never panics rendering it.
        let creds = Arc::new(creds);
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);
        assert!(
            !lines.is_empty(),
            "doctor must still render provider status after a sealed vault"
        );
    }
}
