---
phase: 20
plan: 03
type: execute
wave: 3
depends_on: [20-02]
files_modified:
  - crates/ironhermes-cli/src/memory_setup.rs
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-cli/src/lib.rs
  - crates/ironhermes-cli/Cargo.toml
  - crates/ironhermes-agent/src/memory/factory.rs
autonomous: true
requirements: [MEM-07]
must_haves:
  truths:
    - "`hermes memory setup` subcommand exists, enumerates compiled-in providers, prompts only for required+no-default ConfigFields"
    - "Wizard appends secrets to `$HERMES_HOME/.env` with POSIX single-quote escaping and refuses newline values"
    - "Wizard calls `provider.save_config(values, hermes_home)` for non-secret fields"
    - "Wizard updates `config.yaml`'s `memory.provider` to the selected provider so the next launch picks it up"
    - "Scripted-stdin integration test round-trips a fake provider with one secret + one required-with-default + one optional field (D-23)"
    - "`run_chat` and `run_single` build a MemoryManager, register the memory tool, and call `prompt_builder.set_memory_manager` — Fix 2 of the pending todo"
    - "Chat-mode memory persists across invocations at the same HERMES_HOME"
  artifacts:
    - path: "crates/ironhermes-cli/src/memory_setup.rs"
      provides: "Setup wizard + scripted-stdin integration tests"
      contains: "pub async fn run_memory_setup"
    - path: "crates/ironhermes-cli/src/main.rs"
      provides: "Updated run_chat and run_single constructing MemoryManager (Fix 2)"
      contains: "build_memory_manager"
  key_links:
    - from: "crates/ironhermes-cli/src/memory_setup.rs"
      to: "MemoryProvider::get_config_schema + save_config"
      via: "wizard reads schema from selected provider, writes non-secrets via save_config, appends secrets to .env"
      pattern: "get_config_schema\\|save_config"
    - from: "crates/ironhermes-cli/src/main.rs"
      to: "build_memory_manager + register_memory_tool + prompt_builder.set_memory_manager"
      via: "three wiring calls inside run_chat and run_single (Fix 2)"
      pattern: "build_memory_manager"
---

<objective>
Deliver the minimal `hermes memory setup` CLI subcommand (D-08) and close Fix 2 of the pending todo by wiring `MemoryManager` into `run_chat` and `run_single`. The wizard enumerates compiled-in providers (via a small `available_providers()` helper gated by cargo features), asks which to activate, calls `get_config_schema()` on the selected provider, prompts only for fields that are `required=true` AND have no `default`, writes secrets to `$HERMES_HOME/.env` (append-only with POSIX single-quote escaping), calls `save_config()` for non-secrets, and updates `config.yaml`'s `memory.provider` so the choice is persistent across launches (resolves research open question #1 — silent otherwise).

Fix 2: `run_chat` and `run_single` currently have no memory wiring. This plan adds three calls at each entry point: `build_memory_manager(&cfg.memory).await?`, `register_memory_tool(&mut registry, manager.clone())`, and `prompt_builder.set_memory_manager(manager.clone())`. A regression test verifies memory persists across two `hermes chat` invocations at the same `HERMES_HOME`.

Purpose: Makes the Plan 20-01 `ConfigField` surface actually usable from the CLI and brings CLI modes to gateway parity for memory. Without this plan, setup is manual (edit YAML + JSON by hand) and `hermes chat` silently drops memory between runs.
Output: New `memory_setup.rs` module (or inline under `main.rs` per D-08 discretion); three edits to `run_chat`/`run_single`; integration tests with scripted stdin.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/PROJECT.md
@.planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md
@.planning/phases/20-memory-provider-plugin-contract/20-RESEARCH.md
@.planning/phases/20-memory-provider-plugin-contract/20-VALIDATION.md
@.planning/phases/20-memory-provider-plugin-contract/20-01-trait-enrichment-and-factory-fix-PLAN.md
@.planning/phases/20-memory-provider-plugin-contract/20-02-memory-manager-and-wiring-PLAN.md

<interfaces>
<!-- Post-Plan-20-01: ConfigField + MemoryProvider trait. -->
From `crates/ironhermes-core/src/config_schema.rs`:
```rust
pub struct ConfigField {
    pub key: String,
    pub description: Option<String>,
    pub secret: bool,
    pub required: bool,
    pub default: Option<serde_json::Value>,
    pub choices: Option<Vec<String>>,
    pub env_var: Option<String>,
    pub url: Option<String>,
}
```

<!-- Post-Plan-20-02: MemoryManager + factory helper. -->
From `crates/ironhermes-agent/src/memory/factory.rs`:
```rust
pub async fn build_memory_provider(config: &MemoryConfig) -> anyhow::Result<Arc<Mutex<dyn MemoryProvider + Send>>>;
pub async fn build_memory_manager(config: &MemoryConfig) -> anyhow::Result<Arc<Mutex<MemoryManager>>>;
```

<!-- Current clap shape (main.rs:54). -->
```rust
#[derive(Subcommand)]
enum Commands {
    Chat { #[arg(short, long)] message: Option<String> },
    Gateway { #[arg(short, long)] token: Option<String> },
    // ... other subcommands ...
    // Phase 20 adds:
    Memory { #[command(subcommand)] action: MemorySubcommand },
}

#[derive(Subcommand)]
enum MemorySubcommand {
    Setup,
}
```
</interfaces>
</context>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| User stdin -> wizard | Interactive secret input; wizard must handle newline-bearing values safely. |
| Wizard -> `.env` file | Append-only write; new secrets MUST be quote-escaped to prevent shell/YAML metacharacter injection. |
| Wizard -> `config.yaml` | Writes `memory.provider` key; must preserve existing keys (parse-then-write, not naive append). |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-20-03 | Tampering | `.env` append path in `run_memory_setup` | mitigate (HIGH) | (a) Validate `ConfigField.env_var` and `ConfigField.key` against regex `^[A-Z_][A-Z0-9_]*$`; refuse otherwise. (b) Refuse values containing `\n` or `\r` (prompt user again or abort). (c) Serialize as `KEY='VALUE'` where single-quote inside VALUE is escaped per POSIX shell rule: `'` -> `'\''`. (d) Reject `env_var` matching the deny-list: `["PATH","HOME","USER","SHELL","IRONHERMES_HOME","HERMES_HOME"]`. A dedicated unit test must cover all four mitigations. |
| T-20-04 | Tampering | `save_config` path traversal via provider `name()` | mitigate | Default trait impl already has `debug_assert!(!name().contains(['/', '\\', '..']))` (Plan 20-01). Wizard additionally validates `name` at enumeration time — refuses any provider whose `name()` matches traversal characters. |
| T-20-03b | Information Disclosure | Wizard Debug-printing secrets in test failures or tracing | mitigate | The wizard's internal `HashMap<String, Value>` MUST NOT be `{:?}`-logged. Wrap secret values in a `RedactedValue` struct whose `Debug` impl prints `***`. Only the concrete `env_var` key name is loggable, never the value. Unit test asserts `format!("{:?}", redacted)` never contains the secret content. |
</threat_model>

<tasks>

<task type="auto" tdd="true">
  <name>Task 20-03-01: Implement the `hermes memory setup` wizard module with scripted-stdin integration tests and .env safety unit tests</name>
  <read_first>
    - crates/ironhermes-cli/src/main.rs (lines 40-110 — Commands enum + clap derive; lines 605-700 for the existing run_gateway call pattern)
    - crates/ironhermes-cli/Cargo.toml (confirm dependencies: clap, rustyline, anyhow, tempfile, dirs, dotenvy, serde_yaml)
    - crates/ironhermes-core/src/config_schema.rs (post-Plan-20-01 ConfigField + MemoryAction)
    - crates/ironhermes-core/src/memory_provider.rs (post-Plan-20-01 trait — `get_config_schema`, `save_config`)
    - crates/ironhermes-core/src/constants.rs (get_hermes_home)
    - crates/ironhermes-core/src/config.rs (MemoryConfig shape + how YAML is currently loaded/saved)
    - .planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md (D-06, D-07, D-08, D-23)
    - .planning/phases/20-memory-provider-plugin-contract/20-RESEARCH.md (Open Question #1, Don't Hand-Roll `.env`, Security Domain)
  </read_first>
  <files>
    crates/ironhermes-cli/src/memory_setup.rs (NEW),
    crates/ironhermes-cli/src/main.rs (add Memory subcommand + dispatch),
    crates/ironhermes-cli/src/lib.rs (if it exists — export memory_setup; else skip and keep module private to main)
  </files>
  <behavior>
    - Test: `available_providers_enumerates_features` — with no optional features, returns `["file"]`; with `memory-sqlite`, returns `["file", "sqlite"]`; etc.
    - Test: `env_var_name_validation` — `"API_KEY"` passes; `"api_key"` (lowercase) fails; `"1KEY"` (digit start) fails; `"KEY-WITH-DASH"` fails; `"PATH"` fails (deny-list); `"HERMES_HOME"` fails (deny-list).
    - Test: `posix_quote_escaping` — input `"sk-abc"` serializes to `KEY='sk-abc'`; input `"it's"` serializes to `KEY='it'\''s'`; input `"has\nnewline"` returns `Err(...)` (newlines refused).
    - Test: `redacted_value_debug_is_masked` — `format!("{:?}", RedactedValue::new("secret-xyz"))` must NOT contain `"secret-xyz"`; must contain `"***"` or equivalent.
    - Test: `scripted_wizard_round_trip` (D-23) — uses a fake `TestProvider` registered behind a test-only feature flag with `get_config_schema` returning three fields:
        - `{key: "API_KEY", required: true, secret: true, env_var: Some("TEST_API_KEY"), default: None}`
        - `{key: "db_path", required: true, secret: false, default: Some(Value::String("$HERMES_HOME/test.db".into()))}` (required but has a default, so wizard does NOT prompt)
        - `{key: "threads", required: false, secret: false, default: Some(Value::Number(1.into()))}` (optional with default, never prompted)
      Scripted stdin feeds only:
        - `"sqlite"` (provider selection — use sqlite if feature enabled, else the test provider; see below)
        - `"sk-scripted-test-value"` (the one prompted field — API_KEY)
      Assert post-run:
        - `$HERMES_HOME/.env` contains `TEST_API_KEY='sk-scripted-test-value'` exactly once.
        - `$HERMES_HOME/.env` does NOT contain `db_path` or `threads` (non-secret — goes to JSON).
        - `$HERMES_HOME/<provider>.json` exists and contains `"db_path"` and `"threads"` with their default values.
        - `$HERMES_HOME/config.yaml` (or the appropriate config path) has `memory.provider` set to the selected provider name.
    - Test: `optional_defaults_skipped` — a schema with all-optional fields should NEVER prompt (zero stdin reads); wizard exits successfully with only the provider selection consumed.
    - Test: `appending_env_preserves_existing_keys` — pre-populate `.env` with `EXISTING_KEY='value'`; run wizard; assert `EXISTING_KEY` line is still present AND the new secret line was appended.
  </behavior>
  <action>
    1. ADD a `Memory` subcommand to `crates/ironhermes-cli/src/main.rs` Commands enum:
       ```rust
       // Inside #[derive(Subcommand)] enum Commands { ... add:
       /// Memory provider management (setup wizard).
       Memory {
           #[command(subcommand)]
           action: MemorySubcommand,
       },
       ```
       And define the subcommand enum:
       ```rust
       #[derive(clap::Subcommand)]
       enum MemorySubcommand {
           /// Interactive setup for the currently-selected memory provider.
           Setup,
       }
       ```
       In the main dispatch match (around line 113):
       ```rust
       Some(Commands::Memory { action: MemorySubcommand::Setup }) => {
           crate::memory_setup::run_memory_setup(&cli).await
       }
       ```

    2. CREATE `crates/ironhermes-cli/src/memory_setup.rs` with the full wizard. Key shape:

    ```rust
    //! `hermes memory setup` — minimal interactive setup for the selected
    //! memory provider (D-08).
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
    use std::fs::{OpenOptions, Permissions};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use anyhow::{Context, Result};
    use ironhermes_core::config_schema::ConfigField;
    use ironhermes_core::constants::get_hermes_home;
    use ironhermes_core::MemoryProvider;
    use serde_json::Value;

    const ENV_VAR_DENY_LIST: &[&str] = &[
        "PATH", "HOME", "USER", "SHELL", "IRONHERMES_HOME", "HERMES_HOME",
    ];

    fn is_valid_env_var_name(s: &str) -> bool {
        if s.is_empty() { return false; }
        if ENV_VAR_DENY_LIST.contains(&s) { return false; }
        let mut chars = s.chars();
        let first = chars.next().unwrap();
        if !(first.is_ascii_uppercase() || first == '_') { return false; }
        chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    }

    /// POSIX single-quote escape. Refuses newline-bearing values.
    fn posix_single_quote(value: &str) -> Result<String> {
        if value.contains('\n') || value.contains('\r') {
            anyhow::bail!("secret value contains a newline — refusing to write to .env");
        }
        // Replace every single-quote with `'\''` and wrap the whole thing in '...'.
        let escaped = value.replace('\'', "'\\''");
        Ok(format!("'{}'", escaped))
    }

    /// Redacted wrapper so Debug-formatting never leaks secret content.
    pub struct RedactedValue(String);
    impl RedactedValue {
        pub fn new<S: Into<String>>(s: S) -> Self { Self(s.into()) }
        pub fn reveal(&self) -> &str { &self.0 }
    }
    impl std::fmt::Debug for RedactedValue {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "RedactedValue(***)")
        }
    }

    /// Compiled-in providers. Feature-gated; kept in lockstep with factory.rs.
    pub fn available_providers() -> Vec<&'static str> {
        let mut v = vec!["file"];
        #[cfg(feature = "memory-sqlite")] v.push("sqlite");
        #[cfg(feature = "memory-duckdb")] v.push("duckdb");
        #[cfg(feature = "memory-grafeo")] v.push("grafeo");
        v
    }

    pub async fn run_memory_setup(_cli: &crate::Cli) -> Result<()> {
        let hermes_home = get_hermes_home();
        std::fs::create_dir_all(&hermes_home).context("creating HERMES_HOME")?;

        let providers = available_providers();
        println!("Available memory providers: {}", providers.join(", "));
        let selected = prompt_line("Select provider", Some("file"))?;
        if !providers.contains(&selected.as_str()) {
            anyhow::bail!("unknown provider: {} (available: {})", selected, providers.join(", "));
        }

        // Build the provider (read-only; no writes yet) via the factory so
        // we can call get_config_schema() on it.
        let mut cfg = ironhermes_core::config::MemoryConfig::default();
        cfg.provider = selected.clone();
        let provider_arc = ironhermes_agent::memory::factory::build_memory_provider(&cfg)
            .await
            .context("building provider for schema introspection")?;
        let schema: Vec<ConfigField> = {
            let p = provider_arc.lock().await;
            p.get_config_schema()
        };

        let mut collected: HashMap<String, Value> = HashMap::new();
        let mut secrets_to_env: Vec<(String, RedactedValue)> = Vec::new();

        for field in &schema {
            // Prompt only for required fields with no default (D-08).
            if !(field.required && field.default.is_none()) {
                if let Some(def) = &field.default {
                    collected.insert(field.key.clone(), def.clone());
                }
                continue;
            }

            let prompt = match &field.description {
                Some(d) => format!("{} ({}): ", field.key, d),
                None => format!("{}: ", field.key),
            };
            let value = prompt_line(&prompt, None)?;

            if field.secret {
                let env_var = field.env_var.as_deref()
                    .ok_or_else(|| anyhow::anyhow!(
                        "field `{}` is secret=true but has no env_var — schema error",
                        field.key
                    ))?;
                if !is_valid_env_var_name(env_var) {
                    anyhow::bail!("invalid env_var name `{}` for field `{}`", env_var, field.key);
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
                .create(true).append(true).open(&env_path)
                .with_context(|| format!("opening {}", env_path.display()))?;
            // Ensure 0600 before first write (create path only).
            let _ = std::fs::set_permissions(&env_path, Permissions::from_mode(0o600));
            for (key, value) in &secrets_to_env {
                let quoted = posix_single_quote(value.reveal())?;
                writeln!(file, "{}={}", key, quoted)?;
            }
        }

        // 2. Call save_config for non-secrets.
        {
            let p = provider_arc.lock().await;
            p.save_config(&collected, &hermes_home).context("save_config")?;
        }

        // 3. Update config.yaml `memory.provider` (resolves Open Question #1).
        update_config_yaml_memory_provider(&hermes_home, &selected)?;

        println!("\nSetup complete. Provider: {}", selected);
        if !secrets_to_env.is_empty() {
            println!("Secrets written to {}/.env", hermes_home.display());
        }
        Ok(())
    }

    fn prompt_line(prompt: &str, default: Option<&str>) -> Result<String> {
        use std::io::{self, BufRead};
        let default_str = default.map(|d| format!(" [{}]", d)).unwrap_or_default();
        print!("{}{}: ", prompt.trim_end_matches(": "), default_str);
        io::stdout().flush()?;
        let stdin = io::stdin();
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let line = line.trim_end_matches(['\n', '\r']).to_string();
        if line.is_empty() {
            if let Some(d) = default { return Ok(d.to_string()); }
        }
        Ok(line)
    }

    fn update_config_yaml_memory_provider(hermes_home: &Path, selected: &str) -> Result<()> {
        let cfg_path = hermes_home.join("config.yaml");
        let mut doc: serde_yaml::Value = if cfg_path.exists() {
            let text = std::fs::read_to_string(&cfg_path)?;
            serde_yaml::from_str(&text).unwrap_or(serde_yaml::Value::Mapping(Default::default()))
        } else {
            serde_yaml::Value::Mapping(Default::default())
        };
        let map = doc.as_mapping_mut().ok_or_else(|| anyhow::anyhow!("config.yaml root must be a mapping"))?;
        let mem_key = serde_yaml::Value::String("memory".into());
        let mem_entry = map.entry(mem_key).or_insert(serde_yaml::Value::Mapping(Default::default()));
        let mem_map = mem_entry.as_mapping_mut().ok_or_else(|| anyhow::anyhow!("memory key must be a mapping"))?;
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

        #[test]
        fn env_var_name_validation() {
            assert!(is_valid_env_var_name("API_KEY"));
            assert!(is_valid_env_var_name("_LEADING_UNDERSCORE"));
            assert!(is_valid_env_var_name("K1_2"));
            assert!(!is_valid_env_var_name("api_key"));  // lowercase
            assert!(!is_valid_env_var_name("1KEY"));      // digit start
            assert!(!is_valid_env_var_name("KEY-DASH"));  // dash
            assert!(!is_valid_env_var_name(""));           // empty
            assert!(!is_valid_env_var_name("PATH"));       // deny-list
            assert!(!is_valid_env_var_name("HOME"));       // deny-list
            assert!(!is_valid_env_var_name("HERMES_HOME"));// deny-list
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
        }

        #[test]
        fn appending_env_preserves_existing_keys() {
            let tmp = tempfile::TempDir::new().unwrap();
            let env = tmp.path().join(".env");
            std::fs::write(&env, "EXISTING_KEY='value'\n").unwrap();

            let quoted = posix_single_quote("new-secret").unwrap();
            use std::io::Write;
            let mut f = OpenOptions::new().append(true).open(&env).unwrap();
            writeln!(f, "{}={}", "NEW_KEY", quoted).unwrap();

            let text = std::fs::read_to_string(&env).unwrap();
            assert!(text.contains("EXISTING_KEY='value'"));
            assert!(text.contains("NEW_KEY='new-secret'"));
        }

        // =========================================================================
        // Scripted-stdin integration test (D-23)
        // =========================================================================
        // NOTE for executor: to drive stdin in-process, refactor `prompt_line` into
        // `prompt_from<R: BufRead>(reader: &mut R, writer: &mut impl Write, ...)`.
        // Keep `run_memory_setup` taking a generic BufRead/Write so the integration
        // test can feed a `Cursor<String>` and inspect the captured output without
        // touching real stdin/stdout.
        //
        // Test flow:
        // 1. TempDir as HERMES_HOME; set env var.
        // 2. Construct a TestFakeProvider with the three-field schema above.
        //    Register behind a #[cfg(test)] branch in `available_providers()` OR
        //    call `run_memory_setup_with_provider(test_provider, stdin, stdout)`
        //    helper that bypasses enumeration.
        // 3. Feed stdin: "sqlite\nsk-scripted-test-value\n".
        // 4. Assert: .env contains exactly one `TEST_API_KEY='sk-scripted-test-value'` line.
        // 5. Assert: provider JSON contains db_path + threads with default values.
        // 6. Assert: config.yaml memory.provider == the selected name.
        // 7. Assert: stdout did NOT echo the secret.
        #[tokio::test]
        async fn scripted_wizard_round_trip() {
            // EXECUTOR: implement per the notes above using the testable refactor.
        }

        #[tokio::test]
        async fn optional_defaults_skipped() {
            // EXECUTOR: provider with all-optional fields; feed stdin with ONLY
            // the provider selection line ("file\n"); assert the wizard consumes
            // no further lines.
        }
    }
    ```

    3. ADD `pub mod memory_setup;` near the top of `crates/ironhermes-cli/src/main.rs` (or `src/lib.rs` if the crate is structured as a lib+bin; verify via `grep 'pub mod\\|mod memory_setup' crates/ironhermes-cli/src/*.rs`).

    4. RUN `cargo check -p ironhermes-cli --all-features`. Fix any unused-import warnings.

    5. Confirm the integration tests drive in-process stdin (no TTY required) — aligns with D-08's "minimal wizard" and the `autonomous: true` plan frontmatter.
  </action>
  <verify>
    <automated>
      cargo check -p ironhermes-cli --all-features &&
      cargo test -p ironhermes-cli memory_setup::tests::env_var_name_validation &&
      cargo test -p ironhermes-cli memory_setup::tests::posix_quote_escaping_ok &&
      cargo test -p ironhermes-cli memory_setup::tests::posix_quote_rejects_newlines &&
      cargo test -p ironhermes-cli memory_setup::tests::redacted_value_debug_is_masked &&
      cargo test -p ironhermes-cli memory_setup::tests::appending_env_preserves_existing_keys &&
      cargo test -p ironhermes-cli memory_setup::tests::scripted_wizard_round_trip &&
      cargo test -p ironhermes-cli memory_setup::tests::optional_defaults_skipped
    </automated>
  </verify>
  <acceptance_criteria>
    - `grep -q "pub async fn run_memory_setup" crates/ironhermes-cli/src/memory_setup.rs`.
    - `grep -q "ENV_VAR_DENY_LIST" crates/ironhermes-cli/src/memory_setup.rs` AND the list contains `"PATH"`, `"HOME"`, `"HERMES_HOME"`.
    - `grep -q "posix_single_quote" crates/ironhermes-cli/src/memory_setup.rs` AND the function rejects values containing `\n` or `\r` (T-20-03 mitigation implemented).
    - `grep -q "RedactedValue" crates/ironhermes-cli/src/memory_setup.rs` (T-20-03b mitigation).
    - `grep -q "update_config_yaml_memory_provider" crates/ironhermes-cli/src/memory_setup.rs` (Open Question #1 resolved).
    - `grep -q "MemorySubcommand::Setup" crates/ironhermes-cli/src/main.rs` OR (equivalent dispatch in main match).
    - All eight tests listed in `<verify>` exit 0.
  </acceptance_criteria>
  <done>
    `hermes memory setup` subcommand exists and passes scripted-stdin round-trip; `.env` writes are shell-safe and deny-list-protected; secret values never leak through Debug or stdout; config.yaml is updated so the user's selection sticks across launches.
  </done>
</task>

<task type="auto" tdd="true">
  <name>Task 20-03-02: Wire MemoryManager into run_chat and run_single (Fix 2 of pending todo) + cross-invocation persistence regression test</name>
  <read_first>
    - crates/ironhermes-cli/src/main.rs (lines 243-345 — run_single; lines 348-600 — run_chat; lines 605-760 — run_gateway for the reference wiring pattern)
    - crates/ironhermes-agent/src/memory/factory.rs (post-Plan-20-02 `build_memory_manager`)
    - crates/ironhermes-agent/src/prompt_builder.rs (post-Plan-20-02 `set_memory_manager` setter)
    - crates/ironhermes-tools/src/registry.rs (register_memory_tool signature post-Plan-20-02)
    - crates/ironhermes-agent/src/agent_loop.rs (how memory manager is received — for mirroring the wiring in CLI modes)
    - .planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md (Fix 2, D-08)
    - .planning/todos/pending/2026-04-16-chat-and-single-cli-modes-have-no-memory-wiring.md (if readable)
  </read_first>
  <files>
    crates/ironhermes-cli/src/main.rs,
    crates/ironhermes-cli/tests/chat_memory_persistence.rs (NEW — integration test)
  </files>
  <behavior>
    - Test: `run_chat_memory_wiring_compiles` — validated by `cargo check -p ironhermes-cli`. Not a runtime test.
    - Test: `memory_persists_across_invocations` (integration): spawn two sequential in-process `run_chat`-like sessions at the same `HERMES_HOME`, with a stub LLM that produces one assistant tool-call of `memory_add` and exits. After session 1 ends, session 2 observes the memory entry in its first-turn system prompt.
    - Test: `run_single_memory_wiring_compiles` — similar, asserted by grep + cargo check.
  </behavior>
  <action>
    1. EDIT `crates/ironhermes-cli/src/main.rs::run_chat` (approx line 348). Locate the block that constructs the agent's dependencies (LLM client, prompt builder, registry). After `run_gateway`'s existing pattern (line ~613 does `build_memory_provider` — post-Plan 20-02 that flipped to `build_memory_manager`), add these three calls BEFORE the main chat loop starts:

    ```rust
    // Plan 20-03 Fix 2: CLI chat mode now has full memory wiring.
    let memory_manager = ironhermes_agent::memory::factory::build_memory_manager(&config.memory)
        .await
        .context("building memory manager for chat mode")?;

    // Register the memory tool so the LLM can call memory_add/replace/remove.
    ironhermes_tools::registry::register_memory_tool(&mut tool_registry, memory_manager.clone());

    // Inject the manager into the prompt builder so the frozen-snapshot memory
    // block AND the system_prompt_block (Plan 20-02) render into slot 3.
    prompt_builder.set_memory_manager(memory_manager.clone());
    ```

    IMPORTANT: The exact variable names (`config`, `tool_registry`, `prompt_builder`) must match what's already in scope in `run_chat`. EXECUTOR: read the surrounding ~50 lines before inserting and adjust names accordingly.

    2. REPEAT the same three-call pattern in `run_single` (approx line 243). The only difference: `run_single` emits one turn then exits — memory still needs to be wired because the turn may call `memory_add`.

    3. ADD `use anyhow::Context;` at the top of main.rs if not already present.

    4. ADD an import for the MemoryManager type if needed at the call site scope.

    5. CREATE `crates/ironhermes-cli/tests/chat_memory_persistence.rs`:

    ```rust
    //! Integration regression: Fix 2 of the pending todo — chat-mode memory
    //! must persist across invocations at the same HERMES_HOME.

    use tempfile::TempDir;

    #[tokio::test]
    async fn memory_persists_across_invocations_with_file_provider() {
        let tmp = TempDir::new().unwrap();
        unsafe { std::env::set_var("HERMES_HOME", tmp.path()); }

        // EXECUTOR: the CLI crate's test harness for run_chat should already
        // exist (Phase 15/18). If not, build the minimum driver:
        //   let cfg = ironhermes_core::config::MemoryConfig { provider: "file".into(), ..default };
        //   let mgr = ironhermes_agent::memory::factory::build_memory_manager(&cfg).await.unwrap();
        //   let mut guard = mgr.lock().await;
        //   guard.add(MemoryTarget::Memory, "persisted-fact").await.unwrap();
        //   drop(guard); drop(mgr);
        //
        //   let mgr2 = ironhermes_agent::memory::factory::build_memory_manager(&cfg).await.unwrap();
        //   let guard2 = mgr2.lock().await;
        //   let block = guard2.format_for_system_prompt(MemoryTarget::Memory).await
        //       .expect("memory block should be populated");
        //   assert!(block.contains("persisted-fact"));

        // This test exercises the factory + manager path the CLI run_chat uses.
        // A higher-fidelity end-to-end test would drive run_chat with a stub
        // LLM client; if that harness exists in the crate, prefer it.
    }

    #[cfg(feature = "memory-sqlite")]
    #[tokio::test]
    async fn memory_persists_across_invocations_with_sqlite_provider() {
        let tmp = TempDir::new().unwrap();
        unsafe { std::env::set_var("HERMES_HOME", tmp.path()); }

        let mut cfg = ironhermes_core::config::MemoryConfig::default();
        cfg.provider = "sqlite".into();

        let mgr1 = ironhermes_agent::memory::factory::build_memory_manager(&cfg).await.unwrap();
        {
            let guard = mgr1.lock().await;
            guard.add(ironhermes_core::memory_store::MemoryTarget::Memory, "sqlite-cross-run-fact").await.unwrap();
        }
        drop(mgr1);

        let mgr2 = ironhermes_agent::memory::factory::build_memory_manager(&cfg).await.unwrap();
        let guard2 = mgr2.lock().await;
        let block = guard2.format_for_system_prompt(ironhermes_core::memory_store::MemoryTarget::Memory).await
            .expect("sqlite memory block should be populated on second run");
        assert!(block.contains("sqlite-cross-run-fact"),
            "Fix 2 regression: memory did not persist across invocations; block was: {block}");
    }
    ```

    6. VERIFY by running `cargo test -p ironhermes-cli --test chat_memory_persistence` and `cargo check -p ironhermes-cli --all-features`.

    7. AUDIT `run_gateway` to confirm it still works after the `run_chat`/`run_single` edits (they shouldn't interact, but regression-check): `cargo test -p ironhermes-cli run_gateway` (or whatever tests exercise gateway boot — keep them green).
  </action>
  <verify>
    <automated>
      cargo check -p ironhermes-cli --all-features &&
      cargo test -p ironhermes-cli --test chat_memory_persistence &&
      cargo test -p ironhermes-cli --features memory-sqlite --test chat_memory_persistence memory_persists_across_invocations_with_sqlite_provider &&
      cargo test -p ironhermes-cli run_chat run_single
    </automated>
  </verify>
  <acceptance_criteria>
    - `grep -q "build_memory_manager" crates/ironhermes-cli/src/main.rs` at least TWICE (once each in `run_chat` and `run_single`).
    - `grep -q "set_memory_manager" crates/ironhermes-cli/src/main.rs` at least TWICE.
    - `grep -q "register_memory_tool" crates/ironhermes-cli/src/main.rs` at least TWICE.
    - `cargo test -p ironhermes-cli --features memory-sqlite --test chat_memory_persistence memory_persists_across_invocations_with_sqlite_provider` exits 0.
    - `cargo test -p ironhermes-cli --test chat_memory_persistence memory_persists_across_invocations_with_file_provider` exits 0.
    - `cargo check -p ironhermes-cli --all-features` exits 0 (no warnings suppressed).
  </acceptance_criteria>
  <done>
    run_chat and run_single both construct a MemoryManager, register the memory tool, and set the manager on the prompt builder. Fix 2 of the pending todo (2026-04-16-chat-and-single-cli-modes-have-no-memory-wiring) is closed. Regression test proves memory persists across two `build_memory_manager` round-trips at the same HERMES_HOME for both file and sqlite providers.
  </done>
</task>

</tasks>

<verification>
**Full-plan automated verification:**

```bash
cargo check --workspace --all-features &&
cargo clippy --workspace --all-features -- -D warnings &&
cargo test -p ironhermes-cli memory_setup &&
cargo test -p ironhermes-cli --test chat_memory_persistence &&
cargo test -p ironhermes-cli --features memory-sqlite --test chat_memory_persistence
```

**Manual UAT checks** (documented in VALIDATION.md, not automated):
- `hermes memory setup` from a real terminal walks through a sqlite setup and produces a working `~/.ironhermes/memory.db`.
- `hermes chat` preserves memory between two consecutive invocations (already covered by the sqlite integration test above; a human run is still valuable for stdin ergonomics).
</verification>

<success_criteria>
- [ ] `hermes memory setup` subcommand exists and dispatches to `memory_setup::run_memory_setup` (D-08).
- [ ] Wizard prompts only for `required && default.is_none()` fields (D-08).
- [ ] `.env` writes are POSIX-single-quoted, refuse newline values, and reject reserved env-var names via deny-list (T-20-03).
- [ ] Secret values never leak through Debug formatting (T-20-03b).
- [ ] `config.yaml` is updated with `memory.provider` to the user's selection (resolves Open Question #1).
- [ ] `run_chat` and `run_single` construct a MemoryManager, register the memory tool, and set the manager on the prompt builder (Fix 2 of pending todo).
- [ ] Cross-invocation persistence regression test passes for both file and sqlite providers.
- [ ] Scripted-stdin integration test validates three-field schema round-trip (D-23).
</success_criteria>

<output>
After completion, create `.planning/phases/20-memory-provider-plugin-contract/20-03-SUMMARY.md` capturing:
- How the integration test mocked the provider schema (which test-only feature flag or helper function was used).
- The exact `.env` and `config.yaml` file layout after a successful wizard run.
- Any deviations from the plan + rationale.
- Confirmation that Fix 2 of the pending todo is closed (reference the todo file path).
</output>
