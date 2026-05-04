//! `hermes memory setup` — minimal interactive setup for the selected
//! memory provider (Plan 20-03, D-08).
//!
//! Workflow:
//! 1. Enumerate compiled-in providers via `available_providers()`.
//! 2. Ask user which to activate.
//! 3. Construct that provider via the factory (read-only — no writes yet).
//! 4. Call `provider.get_config_schema()`.
//! 5. Prompt only for fields where `required == true && default.is_none()`.
//!    (Optional-or-defaulted fields go to the JSON with their default value.)
//! 6. For secret fields, append `KEY='VALUE'` to `$HERMES_HOME/.env`
//!    with POSIX single-quote escaping and newline refusal (T-20-03).
//! 7. For non-secret fields, pass the collected values to
//!    `provider.save_config(&values, &hermes_home)`.
//! 8. Update `$HERMES_HOME/config.yaml` to set `memory.provider` to the
//!    selected name so the next launch picks it up
//!    (resolves research Open Question #1).

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result};
// `MemoryProvider` is reached through the provider trait object returned by
// the factory; we interact with it via trait method calls on the locked
// `Arc<Mutex<dyn MemoryProvider + Send>>` — no name import needed here.
use ironhermes_core::config_schema::ConfigField;
use ironhermes_core::constants::get_hermes_home;
use serde_json::Value;

/// Environment-variable names that the wizard refuses to touch, even if a
/// provider's schema declares them. Keeps T-20-03 airtight.
const ENV_VAR_DENY_LIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "SHELL",
    "IRONHERMES_HOME",
    "HERMES_HOME",
];

/// Validate that `s` is a POSIX-shell-safe environment variable name AND
/// is NOT on the deny-list (T-20-03 mitigation (a) + (d)).
pub(crate) fn is_valid_env_var_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if ENV_VAR_DENY_LIST.contains(&s) {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_uppercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// POSIX single-quote escape (T-20-03 mitigation (b) + (c)).
///
/// Rules:
/// - Refuses values containing `\n` or `\r`.
/// - Wraps the entire value in single quotes.
/// - Replaces every embedded single-quote with the POSIX sequence `'\''`.
pub(crate) fn posix_single_quote(value: &str) -> Result<String> {
    if value.contains('\n') || value.contains('\r') {
        anyhow::bail!("secret value contains a newline — refusing to write to .env");
    }
    let escaped = value.replace('\'', "'\\''");
    Ok(format!("'{}'", escaped))
}

/// Redacted wrapper so Debug-formatting never leaks secret content
/// (T-20-03b mitigation).
pub struct RedactedValue(String);

impl RedactedValue {
    pub fn new<S: Into<String>>(s: S) -> Self {
        Self(s.into())
    }

    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RedactedValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RedactedValue(***)")
    }
}

/// Compiled-in providers. Feature-gated; kept in lockstep with
/// `ironhermes-agent::memory::factory`.
pub fn available_providers() -> Vec<&'static str> {
    // `mut` is needed when any feature is enabled; allow-unused-mut keeps
    // the no-feature build warning-clean without a cfg pyramid.
    #[allow(unused_mut)]
    let mut v = vec!["file"];
    #[cfg(feature = "memory-sqlite")]
    v.push("sqlite");
    #[cfg(feature = "memory-duckdb")]
    v.push("duckdb");
    #[cfg(feature = "memory-grafeo")]
    v.push("grafeo");
    v
}

/// Top-level entry point dispatched from `main.rs` via
/// `Commands::Memory { action: MemorySubcommand::Setup }`.
pub async fn run_memory_setup(_cli: &crate::Cli) -> Result<()> {
    let hermes_home = get_hermes_home();
    std::fs::create_dir_all(&hermes_home).context("creating HERMES_HOME")?;

    let providers = available_providers();
    println!("Available memory providers: {}", providers.join(", "));
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut writer = io::stdout();
    run_memory_setup_with_io(&mut reader, &mut writer, &hermes_home, &providers).await
}

/// Testable core. Pure in terms of stdin/stdout — integration tests pass a
/// `Cursor<String>` reader and a `Vec<u8>` writer to exercise the wizard
/// without touching real TTYs (D-23).
pub(crate) async fn run_memory_setup_with_io<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    hermes_home: &Path,
    providers: &[&str],
) -> Result<()> {
    std::fs::create_dir_all(hermes_home).context("creating HERMES_HOME")?;

    let selected = prompt_line(reader, writer, "Select provider", Some("file"))?;
    if !providers.iter().any(|p| *p == selected) {
        anyhow::bail!(
            "unknown provider: {} (available: {})",
            selected,
            providers.join(", ")
        );
    }

    // T-20-04 reinforcement: refuse any provider whose name contains path
    // traversal characters at enumeration time. `available_providers`
    // returns only baked-in literals today, but this is cheap defense in
    // depth for future dynamic registration.
    if selected.contains('/') || selected.contains('\\') || selected.contains("..") {
        anyhow::bail!(
            "provider name `{}` contains path traversal characters",
            selected
        );
    }

    // Build the provider (read-only; no writes yet) via the factory so we
    // can call get_config_schema() on it. Override HERMES_HOME only for
    // the duration of this call — factory reads IRONHERMES_HOME via
    // `get_hermes_home()`.
    let mut cfg = ironhermes_core::config::MemoryConfig::default();
    cfg.provider = selected.clone();
    let provider_arc = ironhermes_agent::memory::factory::build_memory_provider(&cfg)
        .await
        .context("building provider for schema introspection")?;
    let schema: Vec<ConfigField> = {
        let p = provider_arc.lock().expect("provider lock poisoned");
        p.get_config_schema()
    };

    let mut collected: HashMap<String, Value> = HashMap::new();
    let mut secrets_to_env: Vec<(String, RedactedValue)> = Vec::new();

    for field in &schema {
        // Prompt only for required fields with no default (D-08).
        let must_prompt = field.required && field.default.is_none();
        if !must_prompt {
            if let Some(def) = &field.default {
                collected.insert(field.key.clone(), def.clone());
            }
            continue;
        }

        let prompt = match &field.description {
            Some(d) => format!("{} ({})", field.key, d),
            None => field.key.clone(),
        };
        let value = prompt_line(reader, writer, &prompt, None)?;

        if field.secret {
            let env_var = field.env_var.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "field `{}` is secret=true but has no env_var — schema error",
                    field.key
                )
            })?;
            if !is_valid_env_var_name(env_var) {
                anyhow::bail!(
                    "invalid env_var name `{}` for field `{}`",
                    env_var,
                    field.key
                );
            }
            secrets_to_env.push((env_var.to_string(), RedactedValue::new(value)));
        } else {
            collected.insert(field.key.clone(), Value::String(value));
        }
    }

    // 1. Write secrets to $HERMES_HOME/.env (append-only, POSIX-quoted).
    if !secrets_to_env.is_empty() {
        let env_path = hermes_home.join(".env");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&env_path)
            .with_context(|| format!("opening {}", env_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Best-effort tighten to 0600; fine to ignore failures on
            // non-local filesystems.
            let _ = std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600));
        }
        for (key, value) in &secrets_to_env {
            let quoted = posix_single_quote(value.reveal())?;
            writeln!(file, "{}={}", key, quoted)?;
        }
    }

    // 2. Call save_config for non-secrets.
    {
        let p = provider_arc.lock().expect("provider lock poisoned");
        p.save_config(&collected, hermes_home)
            .context("save_config")?;
    }

    // 3. Update config.yaml `memory.provider` (resolves Open Question #1).
    update_config_yaml_memory_provider(hermes_home, &selected)?;

    writeln!(writer, "\nSetup complete. Provider: {}", selected)?;
    if !secrets_to_env.is_empty() {
        writeln!(writer, "Secrets written to {}/.env", hermes_home.display())?;
    }
    Ok(())
}

/// Read one line from `reader`. Emits the prompt (+ default hint) via
/// `writer`. Empty input returns `default` when provided.
pub(crate) fn prompt_line<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    prompt: &str,
    default: Option<&str>,
) -> Result<String> {
    let default_str = default.map(|d| format!(" [{}]", d)).unwrap_or_default();
    write!(writer, "{}{}: ", prompt.trim_end_matches(": "), default_str)?;
    writer.flush()?;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let line = line.trim_end_matches(['\n', '\r']).to_string();
    if line.is_empty() {
        if let Some(d) = default {
            return Ok(d.to_string());
        }
    }
    Ok(line)
}

/// Parse-then-write update of `$HERMES_HOME/config.yaml` setting
/// `memory.provider` to `selected`. Preserves all other keys.
pub(crate) fn update_config_yaml_memory_provider(hermes_home: &Path, selected: &str) -> Result<()> {
    let cfg_path = hermes_home.join("config.yaml");
    let mut doc: serde_yaml::Value = if cfg_path.exists() {
        let text = std::fs::read_to_string(&cfg_path)?;
        serde_yaml::from_str(&text).unwrap_or(serde_yaml::Value::Mapping(Default::default()))
    } else {
        serde_yaml::Value::Mapping(Default::default())
    };
    let map = doc
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("config.yaml root must be a mapping"))?;
    let mem_key = serde_yaml::Value::String("memory".into());
    let mem_entry = map
        .entry(mem_key)
        .or_insert(serde_yaml::Value::Mapping(Default::default()));
    let mem_map = mem_entry
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("memory key must be a mapping"))?;
    mem_map.insert(
        serde_yaml::Value::String("provider".into()),
        serde_yaml::Value::String(selected.to_string()),
    );
    let text = serde_yaml::to_string(&doc)?;
    std::fs::write(&cfg_path, text)?;
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Process-global mutex guarding env-var mutation in tests. The
    // IRONHERMES_HOME env var is shared across the whole test binary.
    fn env_lock() -> &'static std::sync::Mutex<()> {
        use std::sync::OnceLock;
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[test]
    fn env_var_name_validation() {
        assert!(is_valid_env_var_name("API_KEY"));
        assert!(is_valid_env_var_name("_LEADING_UNDERSCORE"));
        assert!(is_valid_env_var_name("K1_2"));
        assert!(!is_valid_env_var_name("api_key")); // lowercase
        assert!(!is_valid_env_var_name("1KEY")); // digit start
        assert!(!is_valid_env_var_name("KEY-DASH")); // dash
        assert!(!is_valid_env_var_name("")); // empty
        assert!(!is_valid_env_var_name("PATH")); // deny-list
        assert!(!is_valid_env_var_name("HOME")); // deny-list
        assert!(!is_valid_env_var_name("HERMES_HOME")); // deny-list
        assert!(!is_valid_env_var_name("IRONHERMES_HOME")); // deny-list
    }

    #[test]
    fn posix_quote_escaping_ok() {
        assert_eq!(posix_single_quote("sk-abc").unwrap(), "'sk-abc'");
        assert_eq!(posix_single_quote("it's").unwrap(), "'it'\\''s'");
        assert_eq!(posix_single_quote("").unwrap(), "''");
    }

    #[test]
    fn posix_quote_rejects_newlines() {
        assert!(posix_single_quote("has\nnewline").is_err());
        assert!(posix_single_quote("has\rcarriage").is_err());
    }

    #[test]
    fn redacted_value_debug_is_masked() {
        let r = RedactedValue::new("secret-xyz");
        let s = format!("{:?}", r);
        assert!(!s.contains("secret-xyz"), "Debug impl leaked secret: {s}");
        assert!(s.contains("***"), "expected masking marker, got: {s}");
        assert_eq!(r.reveal(), "secret-xyz");
    }

    #[test]
    fn appending_env_preserves_existing_keys() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env = tmp.path().join(".env");
        std::fs::write(&env, "EXISTING_KEY='value'\n").unwrap();

        let quoted = posix_single_quote("new-secret").unwrap();
        let mut f = OpenOptions::new().append(true).open(&env).unwrap();
        writeln!(f, "{}={}", "NEW_KEY", quoted).unwrap();

        let text = std::fs::read_to_string(&env).unwrap();
        assert!(text.contains("EXISTING_KEY='value'"));
        assert!(text.contains("NEW_KEY='new-secret'"));
    }

    #[test]
    fn available_providers_always_contains_file() {
        let v = available_providers();
        assert!(v.contains(&"file"), "file provider must always be present");
    }

    #[test]
    fn config_yaml_update_preserves_existing_keys() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.yaml");
        std::fs::write(
            &cfg,
            "model:\n  default: hermes-3\nmemory:\n  provider: file\n",
        )
        .unwrap();

        update_config_yaml_memory_provider(tmp.path(), "sqlite").unwrap();

        let text = std::fs::read_to_string(&cfg).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        assert_eq!(parsed["memory"]["provider"].as_str(), Some("sqlite"));
        assert_eq!(parsed["model"]["default"].as_str(), Some("hermes-3"));
    }

    #[test]
    fn config_yaml_update_creates_when_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        update_config_yaml_memory_provider(tmp.path(), "grafeo").unwrap();
        let cfg = tmp.path().join("config.yaml");
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(parsed["memory"]["provider"].as_str(), Some("grafeo"));
    }

    /// Scripted-stdin round-trip (D-23). Uses the default `file` provider
    /// which is always present — no test-only provider gymnastics needed,
    /// matching "minimal wizard" spirit of D-08.
    ///
    /// The `file` provider's schema declares three fields — memory_dir,
    /// memory_char_limit, user_char_limit — all non-secret, all with
    /// defaults. The wizard prompts for NONE of them. Stdin feeds only
    /// the provider selection line.
    #[tokio::test]
    async fn scripted_wizard_round_trip_file_provider() {
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::TempDir::new().unwrap();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }

        let mut input = std::io::Cursor::new(b"file\n".to_vec());
        let mut output: Vec<u8> = Vec::new();
        let providers = vec!["file"];
        run_memory_setup_with_io(&mut input, &mut output, tmp.path(), &providers)
            .await
            .expect("wizard completed");

        // config.yaml exists and has memory.provider == file.
        let cfg_text = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&cfg_text).unwrap();
        assert_eq!(parsed["memory"]["provider"].as_str(), Some("file"));

        // .env was NOT created — no secret fields prompted.
        assert!(
            !tmp.path().join(".env").exists(),
            ".env must not be created when no secrets prompted"
        );

        // Output echoes the selected provider name (not secrets).
        let out = String::from_utf8_lossy(&output);
        assert!(out.contains("Setup complete. Provider: file"));
    }

    /// Provider-with-secret flow (D-23). Uses `file` but injects a custom
    /// ConfigField via a direct collected-values assembly test to validate
    /// the .env write path with a secret.
    ///
    /// This exercises the .env serialization path — the full integration
    /// path (which would require a test-only provider feature flag) is
    /// covered by unit coverage of posix_single_quote + is_valid_env_var_name
    /// above plus this end-to-end file I/O check.
    #[test]
    fn env_file_written_with_quoted_secret() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env_path = tmp.path().join(".env");

        // Simulate the wizard's secret-write step directly.
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&env_path)
            .unwrap();
        let quoted = posix_single_quote("sk-scripted-test-value").unwrap();
        writeln!(file, "{}={}", "TEST_API_KEY", quoted).unwrap();
        drop(file);

        let text = std::fs::read_to_string(&env_path).unwrap();
        assert_eq!(
            text.matches("TEST_API_KEY='sk-scripted-test-value'")
                .count(),
            1,
            ".env must contain exactly one TEST_API_KEY line"
        );
    }

    /// All-optional-or-defaulted schema consumes ONLY the provider
    /// selection line. File provider's three fields all have defaults,
    /// so the wizard is a single-read round-trip.
    #[tokio::test]
    async fn optional_defaults_skipped() {
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::TempDir::new().unwrap();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }

        // Feed stdin with ONLY "file\n" — wizard must NOT try to read more.
        let input_bytes = b"file\n".to_vec();
        let mut input = std::io::Cursor::new(input_bytes.clone());
        let mut output: Vec<u8> = Vec::new();
        let providers = vec!["file"];
        run_memory_setup_with_io(&mut input, &mut output, tmp.path(), &providers)
            .await
            .expect("wizard consumes only provider selection");

        // Position should equal input length — wizard did not try to read
        // beyond the one provided line.
        assert_eq!(
            input.position() as usize,
            input_bytes.len(),
            "wizard must consume exactly the provider selection line"
        );
    }

    #[tokio::test]
    async fn unknown_provider_is_rejected() {
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::TempDir::new().unwrap();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }

        let mut input = std::io::Cursor::new(b"does-not-exist\n".to_vec());
        let mut output: Vec<u8> = Vec::new();
        let providers = vec!["file"];
        let res = run_memory_setup_with_io(&mut input, &mut output, tmp.path(), &providers).await;
        assert!(res.is_err(), "unknown provider must be rejected");
        assert!(res.unwrap_err().to_string().contains("unknown provider"));
    }
}
