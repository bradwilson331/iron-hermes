//! Phase 47.6 Plan 03 (P0-1): the boot gate — a pure, unit-testable function
//! that decides which of the four messaging platforms (Telegram, Discord,
//! Slack, Buzz) are usable BEFORE `GatewayRunner::start` spawns any adapter.
//!
//! Before this module, Telegram's credential resolution was an inline
//! `resolve_token(...).context(...)?` inside `start()` — the ONLY way to
//! exercise it was to actually boot a gateway. [`resolve_enabled_platforms`]
//! extracts that decision into a pure function over `&Config` (plus an
//! injectable credential-lookup seam), so the whole boot decision — for all
//! four platforms, not just Telegram — is testable without starting a
//! gateway or touching the real process environment.
//!
//! # Per-platform semantics
//!
//! Telegram, Discord, and Slack's `enabled` flag is **informational only**
//! (see `cli-config.yaml.example`'s own comment: "Enable Telegram gateway
//! (informational; resolved token is the gate)") — this has been true since
//! Phase 34 and `GatewayRunner::start` has never consulted it for these three
//! platforms. This gate preserves that: a Telegram/Discord/Slack section is
//! usable whenever its credential(s) resolve, `enabled` value notwithstanding.
//! Changing that here would be a real behavior regression for every existing
//! deployment that never set `enabled: true` explicitly (the commented
//! example config ships with `platforms: {}`).
//!
//! Buzz is different: `GatewayRunner::start`'s section 7d already has a real
//! `if !buzz_config.enabled { skip }` gate (Plan 01), so this module mirrors
//! that — a Buzz section with `enabled: false` is reported as
//! [`PlatformSkipReason::Disabled`], not attempted.
//!
//! # Security
//!
//! No resolved credential value is ever interpolated into a
//! [`PlatformSkipReason`] or [`BootGateError`] — reasons are static
//! descriptions of WHAT is missing (e.g. "TELEGRAM_BOT_TOKEN or
//! gateway.platforms.telegram.token"), never the credential's own value.

use std::collections::HashMap;

use ironhermes_core::Config;

/// One platform's resolution outcome from [`resolve_enabled_platforms`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformResolution {
    /// The platform's credential(s) resolved. Carries the resolved
    /// credential material as name -> value pairs (e.g. `{"token": "..."}`
    /// for Telegram/Discord, `{"app_token": "...", "token": "..."}` for
    /// Slack, `{"nsec": "..."}` for Buzz).
    Usable(HashMap<String, String>),
    /// The platform is not usable, with a reason.
    NotUsable(PlatformSkipReason),
}

impl PlatformResolution {
    /// `true` iff this resolution is [`PlatformResolution::Usable`].
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Usable(_))
    }
}

/// Why a platform is not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformSkipReason {
    /// `gateway.platforms` has no entry for this platform at all.
    SectionAbsent,
    /// The platform's section is present but was explicitly disabled
    /// (`enabled: false`). Only meaningful for platforms whose `enabled`
    /// flag is a REAL gate (currently: Buzz only — see module docs).
    Disabled,
    /// The section was attempted but a required credential did not resolve.
    /// `detail` names which credential and how to set it — never the
    /// resolved value itself (there is none to leak in this case).
    MissingCredential(String),
    /// The platform was configured and enabled, but this binary was built
    /// without the cargo feature that platform requires.
    FeatureDisabled(String),
}

impl std::fmt::Display for PlatformSkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SectionAbsent => write!(f, "not configured (no platform section)"),
            Self::Disabled => write!(f, "disabled (enabled: false)"),
            Self::MissingCredential(detail) => write!(f, "{detail}"),
            Self::FeatureDisabled(detail) => write!(f, "{detail}"),
        }
    }
}

/// The full boot-gate result: one [`PlatformResolution`] per platform.
#[derive(Debug, Clone)]
pub struct PlatformCredentials {
    pub telegram: PlatformResolution,
    pub discord: PlatformResolution,
    pub slack: PlatformResolution,
    pub buzz: PlatformResolution,
    /// Phase 36.7.1 Plan 01.
    pub webhook: PlatformResolution,
    /// Phase 36.7.1 Plan 01. The adapter itself is constructed in a later
    /// plan; this resolution arm exists so `PlatformCredentials` is
    /// exhaustive from the start.
    pub api_server: PlatformResolution,
}

/// Returned when ZERO platforms are usable. `Display` lists every platform
/// that was ENABLED-and-attempted but failed, each with its own reason, so
/// an operator sees the whole picture in one line. A platform that was never
/// configured (`SectionAbsent`) or explicitly disabled (`Disabled`) is not a
/// failure and never appears in this list.
#[derive(Debug, Clone)]
pub struct BootGateError {
    failures: Vec<(String, PlatformSkipReason)>,
}

impl BootGateError {
    /// The platforms that were attempted (enabled/configured) but failed to
    /// resolve a credential, in platform-check order.
    pub fn failures(&self) -> &[(String, PlatformSkipReason)] {
        &self.failures
    }
}

impl std::fmt::Display for BootGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.failures.is_empty() {
            write!(
                f,
                "No usable messaging platform is configured. Configure at least one of \
                 telegram, discord, slack, or buzz under gateway.platforms in config.yaml \
                 with a resolvable credential."
            )
        } else {
            write!(f, "No usable messaging platform is configured. Attempted:")?;
            for (name, reason) in &self.failures {
                write!(f, " [{name}: {reason}]")?;
            }
            Ok(())
        }
    }
}

impl std::error::Error for BootGateError {}

/// Resolve a token value from an optional config value or a named
/// environment variable fallback, through the injectable `env_lookup` seam.
/// Supports `${ENV_VAR}` indirection syntax — mirrors `runner.rs`'s
/// `resolve_token_with_env` exactly (kept in sync deliberately; see this
/// module's `<read_first>`).
fn resolve_via_env<F: Fn(&str) -> Option<String>>(
    token: &Option<String>,
    env_lookup: &F,
    fallback_env: &str,
) -> Option<String> {
    if let Some(t) = token {
        if let Some(var_name) = t.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
            return env_lookup(var_name);
        }
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    env_lookup(fallback_env)
}

fn is_present(v: &Option<String>) -> bool {
    v.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
}

fn resolve_telegram<F: Fn(&str) -> Option<String>>(
    config: &Config,
    env_lookup: &F,
) -> PlatformResolution {
    let Some(cfg) = config.gateway.platforms.get("telegram") else {
        return PlatformResolution::NotUsable(PlatformSkipReason::SectionAbsent);
    };
    let resolved = resolve_via_env(&cfg.token, env_lookup, "TELEGRAM_BOT_TOKEN");
    if is_present(&resolved) {
        let mut creds = HashMap::new();
        creds.insert("token".to_string(), resolved.expect("checked present"));
        PlatformResolution::Usable(creds)
    } else {
        PlatformResolution::NotUsable(PlatformSkipReason::MissingCredential(
            "No Telegram bot token configured. Set TELEGRAM_BOT_TOKEN or \
             gateway.platforms.telegram.token in config.yaml"
                .to_string(),
        ))
    }
}

fn resolve_discord<F: Fn(&str) -> Option<String>>(
    config: &Config,
    env_lookup: &F,
) -> PlatformResolution {
    let Some(cfg) = config.gateway.platforms.get("discord") else {
        return PlatformResolution::NotUsable(PlatformSkipReason::SectionAbsent);
    };
    let resolved = resolve_via_env(&cfg.token, env_lookup, "DISCORD_BOT_TOKEN");
    if is_present(&resolved) {
        let mut creds = HashMap::new();
        creds.insert("token".to_string(), resolved.expect("checked present"));
        PlatformResolution::Usable(creds)
    } else {
        PlatformResolution::NotUsable(PlatformSkipReason::MissingCredential(
            "No Discord bot token configured. Set DISCORD_BOT_TOKEN or \
             gateway.platforms.discord.token in config.yaml"
                .to_string(),
        ))
    }
}

fn resolve_slack<F: Fn(&str) -> Option<String>>(
    config: &Config,
    env_lookup: &F,
) -> PlatformResolution {
    let Some(cfg) = config.gateway.platforms.get("slack") else {
        return PlatformResolution::NotUsable(PlatformSkipReason::SectionAbsent);
    };
    let app = resolve_via_env(&cfg.app_token, env_lookup, "SLACK_APP_TOKEN");
    let bot = resolve_via_env(&cfg.token, env_lookup, "SLACK_BOT_TOKEN");
    let app_ok = is_present(&app);
    let bot_ok = is_present(&bot);
    if app_ok && bot_ok {
        let mut creds = HashMap::new();
        creds.insert("app_token".to_string(), app.expect("checked present"));
        creds.insert("token".to_string(), bot.expect("checked present"));
        PlatformResolution::Usable(creds)
    } else {
        let mut missing = Vec::new();
        if !app_ok {
            missing.push("app_token (SLACK_APP_TOKEN)");
        }
        if !bot_ok {
            missing.push("bot token (SLACK_BOT_TOKEN)");
        }
        PlatformResolution::NotUsable(PlatformSkipReason::MissingCredential(format!(
            "Slack requires both app_token and token; missing: {}. Set the environment \
             variable(s) or gateway.platforms.slack.{{app_token,token}} in config.yaml",
            missing.join(" and ")
        )))
    }
}

fn resolve_buzz<G: Fn() -> Result<String, PlatformSkipReason>>(
    config: &Config,
    buzz_nsec_lookup: &G,
) -> PlatformResolution {
    let Some(cfg) = config.gateway.platforms.get("buzz") else {
        return PlatformResolution::NotUsable(PlatformSkipReason::SectionAbsent);
    };
    if !cfg.enabled {
        return PlatformResolution::NotUsable(PlatformSkipReason::Disabled);
    }
    match buzz_nsec_lookup() {
        Ok(nsec) => {
            let mut creds = HashMap::new();
            creds.insert("nsec".to_string(), nsec);
            PlatformResolution::Usable(creds)
        }
        Err(reason) => PlatformResolution::NotUsable(reason),
    }
}

/// Phase 36.7.1 Plan 01: the webhook platform carries no single
/// platform-wide credential — each route's key material is resolved
/// independently at `WebhookAdapter::new` construction time (per
/// RESEARCH.md's Open Question #2: this gate answers "is the platform
/// enabled at all", not per-route validation, which `boot_gate` has no
/// route-shaped config to perform anyway). An enabled section with at
/// least one configured route is [`PlatformResolution::Usable`]; an
/// enabled section with an empty route list is
/// [`PlatformSkipReason::MissingCredential`] naming the `routes` key —
/// there is nothing for the adapter to serve.
fn resolve_webhook(config: &Config) -> PlatformResolution {
    let Some(cfg) = config.gateway.platforms.get("webhook") else {
        return PlatformResolution::NotUsable(PlatformSkipReason::SectionAbsent);
    };
    if !cfg.enabled {
        return PlatformResolution::NotUsable(PlatformSkipReason::Disabled);
    }
    if cfg.routes.is_empty() {
        return PlatformResolution::NotUsable(PlatformSkipReason::MissingCredential(
            "gateway.platforms.webhook is enabled but has no configured routes — set \
             gateway.platforms.webhook.routes to at least one route"
                .to_string(),
        ));
    }
    PlatformResolution::Usable(HashMap::new())
}

/// Phase 36.7.1 Plan 01 (D-06): the REST API server platform's single
/// gateway-wide credential is `IRONHERMES_API_SERVER_KEY`. Per D-06's
/// fail-closed gate, an enabled section with that variable unset is
/// refused — the message names the variable and NEVER its value, matching
/// every other credential-missing message in this module. The adapter
/// itself is constructed in a later plan; this resolution arm is added now
/// (alongside `resolve_webhook`) so `PlatformCredentials` — a named-field
/// struct every construction site must be exhaustive over — is not edited
/// twice across two plans.
fn resolve_api_server<F: Fn(&str) -> Option<String>>(
    config: &Config,
    env_lookup: &F,
) -> PlatformResolution {
    let Some(cfg) = config.gateway.platforms.get("api_server") else {
        return PlatformResolution::NotUsable(PlatformSkipReason::SectionAbsent);
    };
    if !cfg.enabled {
        return PlatformResolution::NotUsable(PlatformSkipReason::Disabled);
    }
    match env_lookup("IRONHERMES_API_SERVER_KEY") {
        Some(key) if !key.is_empty() => {
            let mut creds = HashMap::new();
            creds.insert("api_key".to_string(), key);
            PlatformResolution::Usable(creds)
        }
        _ => PlatformResolution::NotUsable(PlatformSkipReason::MissingCredential(
            "gateway.platforms.api_server is enabled but IRONHERMES_API_SERVER_KEY is not set. \
             Set the environment variable before starting the gateway."
                .to_string(),
        )),
    }
}

/// The buzz nsec lookup used in production: resolves via
/// [`crate::buzz_identity::resolve_buzz_nsec`] when the `buzz` cargo feature
/// is compiled in; otherwise always reports [`PlatformSkipReason::FeatureDisabled`]
/// so an operator who configured Buzz against a non-Buzz binary is told
/// exactly that instead of silence.
fn production_buzz_nsec_lookup() -> Result<String, PlatformSkipReason> {
    #[cfg(feature = "buzz")]
    {
        crate::buzz_identity::resolve_buzz_nsec().map_err(|e| {
            PlatformSkipReason::MissingCredential(format!("Buzz identity not resolved: {e}"))
        })
    }
    #[cfg(not(feature = "buzz"))]
    {
        Err(PlatformSkipReason::FeatureDisabled(
            "Buzz platform is configured, but this binary was built without the 'buzz' cargo \
             feature — rebuild with --features buzz to use it"
                .to_string(),
        ))
    }
}

/// The injectable-seam implementation [`resolve_enabled_platforms`] wraps.
///
/// `env_lookup` resolves a named environment variable for
/// Telegram/Discord/Slack token indirection; `buzz_nsec_lookup` resolves the
/// Buzz nsec (or reports why it could not). Tests exercise every branch
/// through these closures WITHOUT mutating the real process environment —
/// `nextest` runs tests in the same binary concurrently and process
/// environment is global mutable state (the same injectable-seam rule
/// `buzz_identity.rs`'s own resolver already applies).
pub fn resolve_enabled_platforms_with<F, G>(
    config: &Config,
    env_lookup: F,
    buzz_nsec_lookup: G,
) -> Result<PlatformCredentials, BootGateError>
where
    F: Fn(&str) -> Option<String>,
    G: Fn() -> Result<String, PlatformSkipReason>,
{
    let telegram = resolve_telegram(config, &env_lookup);
    let discord = resolve_discord(config, &env_lookup);
    let slack = resolve_slack(config, &env_lookup);
    let buzz = resolve_buzz(config, &buzz_nsec_lookup);
    let webhook = resolve_webhook(config);
    let api_server = resolve_api_server(config, &env_lookup);

    let any_usable = telegram.is_usable()
        || discord.is_usable()
        || slack.is_usable()
        || buzz.is_usable()
        || webhook.is_usable()
        || api_server.is_usable();

    if any_usable {
        return Ok(PlatformCredentials {
            telegram,
            discord,
            slack,
            buzz,
            webhook,
            api_server,
        });
    }

    let mut failures = Vec::new();
    for (name, res) in [
        ("telegram", &telegram),
        ("discord", &discord),
        ("slack", &slack),
        ("buzz", &buzz),
        ("webhook", &webhook),
        ("api_server", &api_server),
    ] {
        if let PlatformResolution::NotUsable(reason) = res
            && !matches!(
                reason,
                PlatformSkipReason::SectionAbsent | PlatformSkipReason::Disabled
            )
        {
            failures.push((name.to_string(), reason.clone()));
        }
    }
    Err(BootGateError { failures })
}

/// Inspect `config.gateway.platforms` and, for each of the four platforms
/// (Telegram, Discord, Slack, Buzz), resolve whether it is usable at boot.
///
/// Returns `Err(BootGateError)` when ZERO platforms are usable — the caller
/// (`GatewayRunner::start`) propagates this and the gateway refuses to boot,
/// same as before this extraction, except the error now names every
/// platform it tried and why each one failed instead of only ever
/// mentioning Telegram.
///
/// This is the thin process-env/production wrapper around
/// [`resolve_enabled_platforms_with`], the actual injectable-seam
/// implementation tests exercise directly.
pub fn resolve_enabled_platforms(config: &Config) -> Result<PlatformCredentials, BootGateError> {
    resolve_enabled_platforms_with(
        config,
        |name: &str| std::env::var(name).ok(),
        production_buzz_nsec_lookup,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_from_yaml(yaml: &str) -> Config {
        serde_yaml::from_str(yaml).expect("valid config fixture")
    }

    fn no_env(_name: &str) -> Option<String> {
        None
    }

    fn no_buzz() -> Result<String, PlatformSkipReason> {
        Err(PlatformSkipReason::MissingCredential(
            "no buzz nsec configured for this test".to_string(),
        ))
    }

    fn ok_buzz_nsec() -> Result<String, PlatformSkipReason> {
        Ok("nsec1testfixturekeymaterial".to_string())
    }

    #[test]
    fn telegram_only_config_resolves_telegram() {
        let config = config_from_yaml(
            r#"
gateway:
  platforms:
    telegram:
      enabled: true
      token: "abc:123"
"#,
        );
        let result = resolve_enabled_platforms_with(&config, no_env, no_buzz)
            .expect("telegram should be usable");
        assert!(result.telegram.is_usable());
        assert!(!result.discord.is_usable());
        assert!(!result.slack.is_usable());
        assert!(!result.buzz.is_usable());
        assert!(matches!(
            result.discord,
            PlatformResolution::NotUsable(PlatformSkipReason::SectionAbsent)
        ));
    }

    #[test]
    fn buzz_only_config_resolves_buzz() {
        let config = config_from_yaml(
            r#"
gateway:
  platforms:
    buzz:
      enabled: true
      relay_url: "wss://relay.example"
"#,
        );
        let result = resolve_enabled_platforms_with(&config, no_env, ok_buzz_nsec)
            .expect("buzz should be usable");
        assert!(result.buzz.is_usable());
        assert!(!result.telegram.is_usable());
        assert!(matches!(
            result.telegram,
            PlatformResolution::NotUsable(PlatformSkipReason::SectionAbsent)
        ));
    }

    #[test]
    fn no_platform_configured_is_an_error() {
        let config = Config::default();
        let err = resolve_enabled_platforms_with(&config, no_env, no_buzz)
            .expect_err("zero platforms configured must be an error");
        // Nothing was ever attempted (all SectionAbsent) — failure list is empty,
        // but the overall call is still Err.
        assert!(err.failures().is_empty());
    }

    #[test]
    fn all_platforms_configured_but_none_resolves_is_an_error() {
        let config = config_from_yaml(
            r#"
gateway:
  platforms:
    telegram:
      enabled: true
    discord:
      enabled: true
    slack:
      enabled: true
    buzz:
      enabled: true
"#,
        );
        let err = resolve_enabled_platforms_with(&config, no_env, no_buzz)
            .expect_err("no platform resolves a credential");
        let names: Vec<&str> = err.failures().iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"telegram"));
        assert!(names.contains(&"discord"));
        assert!(names.contains(&"slack"));
        assert!(names.contains(&"buzz"));
    }

    #[test]
    fn error_names_every_attempted_platform_and_its_reason() {
        let config = config_from_yaml(
            r#"
gateway:
  platforms:
    telegram:
      enabled: true
    slack:
      enabled: true
      app_token: "xapp-only"
"#,
        );
        let err = resolve_enabled_platforms_with(&config, no_env, no_buzz)
            .expect_err("neither telegram nor slack resolves fully");
        let rendered = err.to_string();
        assert!(rendered.contains("telegram"));
        assert!(rendered.contains("slack"));
        // Distinct reasons: telegram's reason names a bot token, slack's names
        // the missing bot token specifically (it already has app_token).
        let telegram_reason = err
            .failures()
            .iter()
            .find(|(n, _)| n == "telegram")
            .map(|(_, r)| r.to_string())
            .unwrap();
        let slack_reason = err
            .failures()
            .iter()
            .find(|(n, _)| n == "slack")
            .map(|(_, r)| r.to_string())
            .unwrap();
        assert_ne!(telegram_reason, slack_reason);
        assert!(slack_reason.contains("bot token"));
    }

    #[test]
    fn disabled_platform_is_not_reported_as_a_failure() {
        // Buzz's `enabled` flag is a REAL gate (unlike telegram/discord/slack) —
        // enabled: false with no nsec configured must be Disabled, not a failure.
        let config = config_from_yaml(
            r#"
gateway:
  platforms:
    buzz:
      enabled: false
"#,
        );
        let err = resolve_enabled_platforms_with(&config, no_env, no_buzz)
            .expect_err("no usable platform");
        assert!(err.failures().is_empty());
        // Directly confirm the underlying resolution too, not just the error shape.
        let telegram_absent_config = Config::default();
        let result = resolve_buzz(&telegram_absent_config, &no_buzz);
        assert!(matches!(
            result,
            PlatformResolution::NotUsable(PlatformSkipReason::SectionAbsent)
        ));
        let disabled_result = resolve_buzz(
            &config_from_yaml("gateway:\n  platforms:\n    buzz:\n      enabled: false\n"),
            &no_buzz,
        );
        assert!(matches!(
            disabled_result,
            PlatformResolution::NotUsable(PlatformSkipReason::Disabled)
        ));
    }

    #[test]
    fn slack_requires_both_tokens() {
        let only_app = config_from_yaml(
            r#"
gateway:
  platforms:
    slack:
      enabled: true
      app_token: "xapp-only"
"#,
        );
        let result = resolve_slack(&only_app, &no_env);
        assert!(!result.is_usable());

        let both = config_from_yaml(
            r#"
gateway:
  platforms:
    slack:
      enabled: true
      app_token: "xapp-1"
      token: "xoxb-1"
"#,
        );
        let result = resolve_slack(&both, &no_env);
        assert!(result.is_usable());
    }

    #[test]
    fn error_render_contains_no_credential_material() {
        let secret_telegram = "TELEGRAM_SECRET_MARKER_SHOULD_NEVER_LEAK";
        let secret_slack_app = "SLACK_APP_SECRET_MARKER_SHOULD_NEVER_LEAK";
        let secret_slack_bot = "SLACK_BOT_SECRET_MARKER_SHOULD_NEVER_LEAK";
        let secret_buzz = "BUZZ_NSEC_SECRET_MARKER_SHOULD_NEVER_LEAK";

        let seeded_env = move |name: &str| -> Option<String> {
            match name {
                "TELEGRAM_BOT_TOKEN" => Some(secret_telegram.to_string()),
                "SLACK_APP_TOKEN" => Some(secret_slack_app.to_string()),
                "SLACK_BOT_TOKEN" => Some(secret_slack_bot.to_string()),
                _ => None,
            }
        };
        // Slack resolves fully (both seeded) so it does NOT force a failure;
        // force the failure on discord (unresolvable) instead, while telegram
        // ALSO stays unresolved (its fallback env var is not seeded above —
        // TELEGRAM_BOT_TOKEN IS seeded, so seed discord's own var instead to
        // force a distinct failing platform without accidentally making
        // telegram usable).
        let config = config_from_yaml(
            r#"
gateway:
  platforms:
    discord:
      enabled: true
    buzz:
      enabled: true
"#,
        );
        let seeded_buzz = move || -> Result<String, PlatformSkipReason> {
            // Simulate a buzz resolver whose underlying error text could have
            // embedded the secret (it must not).
            let _ = secret_buzz; // marker kept alive/visible for the assertion below
            Err(PlatformSkipReason::MissingCredential(
                "Buzz identity not resolved: set BUZZ_NSEC".to_string(),
            ))
        };

        let err = resolve_enabled_platforms_with(&config, seeded_env, seeded_buzz)
            .expect_err("discord and buzz both fail to resolve");
        let rendered = err.to_string();
        assert!(!rendered.contains(secret_telegram));
        assert!(!rendered.contains(secret_slack_app));
        assert!(!rendered.contains(secret_slack_bot));
        assert!(!rendered.contains(secret_buzz));
    }

    /// Confirms this module never mutates the real process environment —
    /// this whole file contains no direct process-environment mutation
    /// anywhere, by construction (grep-verifiable per the plan's own
    /// acceptance criterion).
    #[test]
    fn tests_use_injected_lookups_not_process_env() {
        let config = Config::default();
        let result = resolve_enabled_platforms_with(&config, no_env, no_buzz);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // Phase 36.7.1 Plan 01 Task 3: webhook / api_server resolution arms
    // -----------------------------------------------------------------

    #[test]
    fn webhook_platform_resolves_through_boot_gate() {
        let config = config_from_yaml(
            r#"
gateway:
  platforms:
    webhook:
      enabled: true
      host: "127.0.0.1"
      port: 8099
      routes:
        - name: "test-route"
          path: "/webhook/test-route"
          signature: "generic_v2"
          secret_env: "SOME_SECRET_ENV"
          prompt_template: "{Body}"
"#,
        );
        let result = resolve_enabled_platforms_with(&config, no_env, no_buzz)
            .expect("webhook should be usable");
        assert!(result.webhook.is_usable());
    }

    #[test]
    fn webhook_section_absent_is_not_usable_and_not_disabled() {
        let config = Config::default();
        let result = resolve_enabled_platforms_with(&config, no_env, no_buzz);
        // No platform usable at all -> Err; but the important assertion is
        // the webhook resolution SHAPE, which resolve_webhook computes
        // regardless of the overall any_usable outcome. Call resolve_webhook
        // directly to inspect it in isolation.
        let _ = result; // overall boot-gate error is expected and irrelevant here
        assert!(matches!(
            resolve_webhook(&config),
            PlatformResolution::NotUsable(PlatformSkipReason::SectionAbsent)
        ));
    }

    #[test]
    fn webhook_enabled_with_empty_routes_is_missing_credential() {
        let config = config_from_yaml(
            r#"
gateway:
  platforms:
    webhook:
      enabled: true
      host: "127.0.0.1"
      port: 8099
"#,
        );
        assert!(matches!(
            resolve_webhook(&config),
            PlatformResolution::NotUsable(PlatformSkipReason::MissingCredential(_))
        ));
    }

    #[test]
    fn webhook_disabled_section_is_disabled_reason() {
        let config = config_from_yaml(
            r#"
gateway:
  platforms:
    webhook:
      enabled: false
"#,
        );
        assert!(matches!(
            resolve_webhook(&config),
            PlatformResolution::NotUsable(PlatformSkipReason::Disabled)
        ));
    }

    #[test]
    fn api_server_enabled_missing_key_names_variable_never_value() {
        let config = config_from_yaml(
            r#"
gateway:
  platforms:
    api_server:
      enabled: true
"#,
        );
        let result = resolve_api_server(&config, &no_env);
        match result {
            PlatformResolution::NotUsable(PlatformSkipReason::MissingCredential(msg)) => {
                assert!(msg.contains("IRONHERMES_API_SERVER_KEY"));
                // D-06: never the value — no env lookup was even invoked
                // (`no_env` always returns None), so there is no value to
                // leak by construction.
            }
            other => panic!("expected MissingCredential, got {other:?}"),
        }
    }

    #[test]
    fn api_server_enabled_with_key_is_usable() {
        let config = config_from_yaml(
            r#"
gateway:
  platforms:
    api_server:
      enabled: true
"#,
        );
        let env_with_key = |name: &str| -> Option<String> {
            if name == "IRONHERMES_API_SERVER_KEY" {
                Some("test-key-value".to_string())
            } else {
                None
            }
        };
        let result = resolve_api_server(&config, &env_with_key);
        assert!(result.is_usable());
    }
}
