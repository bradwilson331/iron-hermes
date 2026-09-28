//! `hermes provider <subcommand>` — Phase 26, D-14 operator control surface.
//!
//! Structural model: `toolset_cmd.rs` (Phase 25, D-04).
//! Slug validation reuses `ironhermes_core::profile::validate_profile_name` per T-26-03.
//! Cache-break banner emitted on stderr for state-changing commands per D-16.
//! D-15: `cmd_provider_test` NEVER includes the api_key VALUE in any output string.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use ironhermes_core::{
    Config, ProviderResolver,
    commands::provider_display::{
        ProviderRow, render_provider_list, render_provider_list_json, render_provider_show,
    },
    config_setter,
};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Subcommand, Debug, Clone)]
pub enum ProviderSubcommand {
    /// List all providers with status (Phase 26 D-14).
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show detail for one provider.
    Show { name: String },
    /// Live-ping a provider API endpoint (D-15: never prints key value).
    Test { name: String },
    /// Enable a provider — persists to active profile config.yaml.
    Enable { name: String },
    /// Disable a provider — persists to active profile config.yaml.
    Disable { name: String },
}

/// Maximum provider-name length, matching the profile validator's bound.
const PROVIDER_NAME_MAX_LEN: usize = 64;

/// T-26-03: validate a provider name. This is the FIRST call in every
/// state-changing path — traversal and injection are rejected BEFORE any
/// config write.
///
/// This used to delegate to `profile::validate_profile_name`, which enforces
/// `[a-z0-9][a-z0-9-]*` and therefore forbids dots. But a provider name is a
/// `providers` MAP KEY, not a profile slug, and dotted keys are legitimate:
/// `venice.ai` could not be passed to `provider show/test/enable/disable` at
/// all, even while being the configured `model.provider`. So the rule is now
/// `[a-z0-9][a-z0-9.-]*` — the profile rule plus `.`.
///
/// The security properties that mattered are kept explicitly rather than
/// inherited:
/// - `/` and `\` are still excluded by the character allow-list, so a name can
///   never introduce a path separator.
/// - `..` is rejected outright. Redundant while separators are excluded, but a
///   bare `..` is still the parent-directory token if a future caller ever puts
///   a provider name in a path, and this is the chokepoint where that is cheap
///   to deny.
/// - A trailing `.` or `-` is rejected, so a name cannot produce an empty or
///   dot-only YAML key segment.
pub fn validate_provider_name(name: &str) -> Result<String> {
    let reject = |msg: &str| anyhow::anyhow!("invalid provider name '{}': {}", name, msg);

    if name.is_empty() {
        return Err(reject("name is empty"));
    }
    if name.len() > PROVIDER_NAME_MAX_LEN {
        return Err(reject(&format!(
            "exceeds {PROVIDER_NAME_MAX_LEN} characters"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
    {
        return Err(reject(
            "must match [a-z0-9][a-z0-9.-]* (lowercase alphanumeric, dashes, dots)",
        ));
    }
    let first = name.chars().next().expect("non-empty checked above");
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(reject("must start with a lowercase letter or digit"));
    }
    if name.ends_with('.') || name.ends_with('-') {
        return Err(reject("must not end with '.' or '-'"));
    }
    if name.contains("..") {
        return Err(reject("must not contain '..'"));
    }
    Ok(name.to_string())
}

pub async fn handle_provider_command(cmd: ProviderSubcommand, _profile_name: &str) -> Result<()> {
    let hermes_home = ironhermes_core::constants::get_hermes_home();
    match cmd {
        ProviderSubcommand::List { json } => cmd_provider_list(&hermes_home, json).await,
        ProviderSubcommand::Show { name } => cmd_provider_show(&hermes_home, &name).await,
        ProviderSubcommand::Test { name } => cmd_provider_test(&hermes_home, &name).await,
        ProviderSubcommand::Enable { name } => cmd_provider_enable(&hermes_home, &name).await,
        ProviderSubcommand::Disable { name } => cmd_provider_disable(&hermes_home, &name).await,
    }
}

fn load_full_config(hermes_home: &Path) -> Config {
    let path = hermes_home.join("config.yaml");
    if !path.exists() {
        return Config::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(t) => serde_yaml::from_str(&t).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

/// Build a ProviderRow from resolver introspection.
///
/// **T-26-01 by construction:** this function NEVER reads `endpoint.api_key` VALUE.
/// It only calls `endpoint.api_key.is_some()` to determine ✓/✗, and reads
/// `config.providers[name].api_key_env` for the env var NAME. The key value
/// itself never appears in the returned ProviderRow.
fn build_provider_row(name: &str, config: &Config, resolver: &ProviderResolver) -> ProviderRow {
    let endpoint = resolver.resolve(name);
    let provider_cfg = config.providers.get(name);
    let env_name = provider_cfg.and_then(|p| p.api_key_env.as_ref());
    let key_resolved = endpoint.map(|e| e.api_key.is_some()).unwrap_or(false);

    // api_key_status shows env var NAME only — never the VALUE (T-26-01).
    let api_key_status = match (env_name, key_resolved) {
        (Some(var), true) => format!("\u{2713} ${}", var),
        (Some(var), false) => format!("\u{2717} missing ${}", var),
        (None, true) => "\u{2713} (legacy/inline)".to_string(),
        (None, false) => "\u{2717} no key".to_string(),
    };

    let role = if name == config.model.provider {
        "main".to_string()
    } else if config.auxiliary.is_set() && config.auxiliary.provider == name {
        "aux".to_string()
    } else {
        "\u{2014}".to_string()
    };

    let fallbacks = endpoint
        .map(|e| {
            if e.fallback_providers.is_empty() {
                "\u{2014}".to_string()
            } else {
                e.fallback_providers.join(", ")
            }
        })
        .unwrap_or_else(|| "\u{2014}".to_string());

    ProviderRow {
        name: name.to_string(),
        base_url: endpoint.map(|e| e.base_url.clone()).unwrap_or_default(),
        api_key_status,
        default_model: endpoint
            .map(|e| e.default_model.clone())
            .unwrap_or_default(),
        role,
        fallbacks,
        disabled: provider_cfg.and_then(|p| p.disabled).unwrap_or(false),
    }
}

pub async fn cmd_provider_list(hermes_home: &Path, json: bool) -> Result<()> {
    let config = load_full_config(hermes_home);
    let resolver = ProviderResolver::build(&config).context("failed to build provider resolver")?;

    let mut rows: Vec<ProviderRow> = config
        .providers
        .keys()
        .map(|name| build_provider_row(name, &config, &resolver))
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    if json {
        println!("{}", render_provider_list_json(&rows)?);
    } else {
        print!("{}", render_provider_list(rows));
    }
    Ok(())
}

pub async fn cmd_provider_show(hermes_home: &Path, name: &str) -> Result<()> {
    let validated = validate_provider_name(name)?;
    let config = load_full_config(hermes_home);
    let resolver = ProviderResolver::build(&config)?;
    if !config.providers.contains_key(&validated) && resolver.resolve(&validated).is_none() {
        anyhow::bail!(
            "unknown provider '{}' — see `hermes provider list`",
            validated
        );
    }
    let row = build_provider_row(&validated, &config, &resolver);
    print!("{}", render_provider_show(&row));
    Ok(())
}

pub async fn cmd_provider_test(hermes_home: &Path, name: &str) -> Result<()> {
    let validated = validate_provider_name(name)?;
    let config = load_full_config(hermes_home);
    let resolver = ProviderResolver::build(&config)?;
    let endpoint = resolver
        .resolve(&validated)
        .ok_or_else(|| anyhow::anyhow!("unknown provider '{}'", validated))?;

    // D-15: Report the env var NAME, not the value.
    let env_name = config
        .providers
        .get(&validated)
        .and_then(|p| p.api_key_env.as_ref())
        .map(|s| format!("${}", s))
        .unwrap_or_else(|| "(no env var configured)".to_string());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    // Try GET /models first; fall back to POST /chat/completions on 404.
    let base = endpoint.base_url.trim_end_matches('/');
    let models_url = format!("{}/models", base);

    let mut req = client.get(&models_url);
    // D-15: api_key value flows into bearer_auth ONLY — never into any string
    // used for output.
    if let Some(ref key) = endpoint.api_key {
        req = req.bearer_auth(key);
    }

    let start = Instant::now();
    match req.send().await {
        Ok(resp) if resp.status().as_u16() == 404 => {
            // Fall back to a minimal POST chat/completions probe.
            let chat_url = format!("{}/chat/completions", base);
            let mut chat_req = client.post(&chat_url).json(&serde_json::json!({
                "model": endpoint.default_model,
                "messages": [{"role": "user", "content": "ping"}],
                "max_tokens": 1
            }));
            if let Some(ref key) = endpoint.api_key {
                chat_req = chat_req.bearer_auth(key);
            }
            let chat_start = Instant::now();
            match chat_req.send().await {
                Ok(chat_resp) => {
                    let elapsed = chat_start.elapsed();
                    // D-15: output contains HTTP status + latency + env var NAME only.
                    println!(
                        "[provider:{}] HTTP {} (latency {}ms) — key from {}",
                        validated,
                        chat_resp.status().as_u16(),
                        elapsed.as_millis(),
                        env_name
                    );
                }
                Err(e) => {
                    // D-15: .without_url() strips potential key-bearing URL from error message.
                    anyhow::bail!(
                        "[provider:{}] network error (POST /chat/completions): {} (key from {})",
                        validated,
                        e.without_url(),
                        env_name
                    );
                }
            }
        }
        Ok(resp) => {
            let elapsed = start.elapsed();
            // D-15: output contains HTTP status + latency + env var NAME only.
            println!(
                "[provider:{}] HTTP {} (latency {}ms) — key from {}",
                validated,
                resp.status().as_u16(),
                elapsed.as_millis(),
                env_name
            );
        }
        Err(e) => {
            anyhow::bail!(
                "[provider:{}] network error (GET /models): {} (key from {})",
                validated,
                e.without_url(),
                env_name
            );
        }
    }
    Ok(())
}

pub async fn cmd_provider_enable(hermes_home: &Path, name: &str) -> Result<()> {
    // T-26-03: validate BEFORE any config write.
    let validated = validate_provider_name(name)?;
    // Segment API, not the dotted one: a dotted provider name such as
    // `venice.ai` would make `providers.venice.ai.disabled` split into four
    // segments and silently write the WRONG nesting.
    config_setter::config_set_segments(
        hermes_home,
        &["providers", validated.as_str(), "disabled"],
        "false",
    )
    .with_context(|| format!("failed to enable provider {}", validated))?;
    // D-16: cache-break banner on stderr (not stdout).
    eprintln!(
        "{} [provider: {}] config changed \u{2014} schema cache will rebuild on next LLM call",
        "\u{26a0}".yellow(),
        validated,
    );
    Ok(())
}

pub async fn cmd_provider_disable(hermes_home: &Path, name: &str) -> Result<()> {
    // T-26-03: validate BEFORE any config write.
    let validated = validate_provider_name(name)?;
    // Segment API, not the dotted one: a dotted provider name such as
    // `venice.ai` would make `providers.venice.ai.disabled` split into four
    // segments and silently write the WRONG nesting.
    config_setter::config_set_segments(
        hermes_home,
        &["providers", validated.as_str(), "disabled"],
        "true",
    )
    .with_context(|| format!("failed to disable provider {}", validated))?;
    // D-16: cache-break banner on stderr (not stdout).
    eprintln!(
        "{} [provider: {}] config changed \u{2014} schema cache will rebuild on next LLM call",
        "\u{26a0}".yellow(),
        validated,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Test 1: ProviderSubcommand enum variant existence (compile gate).
    #[test]
    fn provider_subcommand_enum_has_5_variants() {
        // Construct each variant to ensure they compile.
        let _list = ProviderSubcommand::List { json: false };
        let _list_json = ProviderSubcommand::List { json: true };
        let _show = ProviderSubcommand::Show {
            name: "openai".to_string(),
        };
        let _test = ProviderSubcommand::Test {
            name: "openai".to_string(),
        };
        let _enable = ProviderSubcommand::Enable {
            name: "openai".to_string(),
        };
        let _disable = ProviderSubcommand::Disable {
            name: "openai".to_string(),
        };
    }

    // Test 2: validate_provider_name — happy path + injection rejection.
    #[test]
    fn validate_provider_name_rejects_slug_injection() {
        // Path traversal
        assert!(
            validate_provider_name("../etc/passwd").is_err(),
            "path traversal must be rejected"
        );
        // Shell metacharacters
        assert!(
            validate_provider_name("foo;rm -rf").is_err(),
            "shell metachar must be rejected"
        );
        // Uppercase
        assert!(
            validate_provider_name("UPPER").is_err(),
            "uppercase must be rejected"
        );
        // Whitespace
        assert!(
            validate_provider_name("with space").is_err(),
            "whitespace must be rejected"
        );
        // Valid slugs
        assert!(
            validate_provider_name("my-local-llm").is_ok(),
            "my-local-llm should be valid"
        );
        assert!(
            validate_provider_name("openai").is_ok(),
            "openai should be valid"
        );
        assert!(
            validate_provider_name("anthropic").is_ok(),
            "anthropic should be valid"
        );
    }

    // Test 3: cmd_provider_enable writes disabled=false to config.yaml.
    #[tokio::test]
    async fn cmd_provider_enable_writes_disabled_false() {
        let tmp = tempfile::TempDir::new().unwrap();
        let result = cmd_provider_enable(tmp.path(), "openai").await;
        assert!(result.is_ok(), "enable should succeed: {:?}", result);

        let yaml = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();
        assert!(
            yaml.contains("disabled: false") || yaml.contains("false"),
            "expected disabled: false in config; got: {}",
            yaml
        );
    }

    // Test 4: cmd_provider_disable writes disabled=true to config.yaml.
    #[tokio::test]
    async fn cmd_provider_disable_writes_disabled_true() {
        let tmp = tempfile::TempDir::new().unwrap();
        let result = cmd_provider_disable(tmp.path(), "openai").await;
        assert!(result.is_ok(), "disable should succeed: {:?}", result);

        let yaml = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();
        assert!(
            yaml.contains("disabled: true"),
            "expected disabled: true in config; got: {}",
            yaml
        );
    }

    // Dotted provider names (venice-401 follow-up).
    //
    // `venice.ai` is a legitimate `providers` map key and was rejected because
    // this validator delegated to the PROFILE slug rule. These pin the widened
    // rule AND the traversal properties that must survive widening.

    #[test]
    fn validate_provider_name_accepts_dotted_names() {
        for name in ["venice.ai", "api.example.co", "z-ai.v2"] {
            assert!(
                validate_provider_name(name).is_ok(),
                "dotted provider name '{name}' must be accepted — it is a config \
                 map key, not a profile slug"
            );
        }
    }

    #[test]
    fn validate_provider_name_still_rejects_traversal_after_allowing_dots() {
        // Allowing '.' must not admit the parent-directory token.
        for bad in ["..", "../etc/passwd", "a..b", "..venice", "venice..ai"] {
            assert!(
                validate_provider_name(bad).is_err(),
                "'{bad}' contains '..' and must be rejected"
            );
        }
        // Separators stay excluded by the character allow-list.
        for bad in ["a/b", "a\\b"] {
            assert!(
                validate_provider_name(bad).is_err(),
                "'{bad}' contains a path separator and must be rejected"
            );
        }
        // A name may not start with '.' nor end with '.' or '-', so it can
        // never yield an empty or dot-only YAML key segment.
        for bad in [".venice", "venice.", "venice-"] {
            assert!(
                validate_provider_name(bad).is_err(),
                "'{bad}' must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn enable_writes_a_dotted_provider_under_one_key_not_nested() {
        // The regression the segment API exists to prevent: via the dotted
        // config_set, `providers.venice.ai.disabled` would split four ways and
        // create providers -> venice -> ai -> disabled.
        let tmp = tempfile::TempDir::new().unwrap();
        cmd_provider_enable(tmp.path(), "venice.ai")
            .await
            .expect("enable venice.ai");
        let yaml = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();

        let doc: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let providers = doc
            .get("providers")
            .and_then(|p| p.as_mapping())
            .expect("providers mapping exists");
        assert!(
            providers.contains_key(serde_yaml::Value::String("venice.ai".into())),
            "the provider must live under the single key 'venice.ai': {yaml}"
        );
        assert!(
            !providers.contains_key(serde_yaml::Value::String("venice".into())),
            "must NOT have split into a nested 'venice' key: {yaml}"
        );
    }

    // Test 5: validate_provider_name accepts known-good provider names.
    #[test]
    fn validate_provider_name_accepts_valid_names() {
        let valid = [
            "openai",
            "anthropic",
            "openrouter",
            "my-local-llm",
            "llm-v2",
            "a1b2",
        ];
        for name in &valid {
            assert!(
                validate_provider_name(name).is_ok(),
                "should accept '{}', got error",
                name
            );
        }
    }

    // Test 6: cmd_provider_list with empty config renders the header row.
    #[tokio::test]
    async fn cmd_provider_list_renders_header() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Write a minimal config with two providers.
        let config_yaml = r#"
model:
  provider: openai
  default: gpt-4o
providers:
  openai:
    base_url: https://api.openai.com/v1
    api_key_env: OPENAI_API_KEY
"#;
        std::fs::write(tmp.path().join("config.yaml"), config_yaml).unwrap();

        // Capture stdout output from cmd_provider_list.
        // We test it doesn't panic and produces output via the render function.
        let config = load_full_config(tmp.path());
        let resolver = ProviderResolver::build(&config).unwrap();
        let rows: Vec<ProviderRow> = config
            .providers
            .keys()
            .map(|name| build_provider_row(name, &config, &resolver))
            .collect();
        let rendered = ironhermes_core::commands::provider_display::render_provider_list(rows);
        assert!(
            rendered.starts_with("NAME              "),
            "header must start with NAME padded to 18 chars; got: {:?}",
            rendered
        );
        assert!(
            rendered.contains("openai"),
            "rendered list must contain 'openai': {}",
            rendered
        );
        // T-26-01: no key value in rendered output.
        assert!(
            !rendered.contains("sk-"),
            "rendered output must not contain sk- prefix: {}",
            rendered
        );
    }
}
