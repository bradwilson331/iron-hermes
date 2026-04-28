//! `hermes setup [section]` — interactive first-run wizard (D-01, D-02).
//!
//! Uses rustyline 15 readline_with_initial for inline-default prompts.
//! Production I/O lives here; pure mutation logic lives in
//! `ironhermes_core::wizard`. The `apply_minimum_viable_answers` function
//! is the testability seam — drives the wizard with pre-scripted strings
//! so integration tests don't need a real TTY.

use anyhow::{anyhow, Context, Result};
use ironhermes_core::config::Config;
use ironhermes_core::constants::get_hermes_home;
use ironhermes_core::wizard::{
    apply_api_key_answer, apply_learning_loop_answer, apply_memory_provider_answer,
    apply_model_answer, apply_provider_answer, WizardMode, LEARNING_LOOP_FRAMING,
};
use std::path::Path;

pub use ironhermes_core::wizard::WizardMode as ReExportedWizardMode;

/// Construct a fresh rustyline editor for wizard use.
/// NO history persistence (Anti-Pattern #3 — wizard answers must not bleed into chat history).
fn make_wizard_editor() -> Result<rustyline::DefaultEditor> {
    use rustyline::config::Configurer;
    let mut rl = rustyline::DefaultEditor::new().context("initializing rustyline for wizard")?;
    rl.set_history_ignore_dups(true).ok();
    // Do NOT call set_max_history_size, load_history, or save_history.
    Ok(rl)
}

/// Prompt with an inline pre-populated default. Empty submission accepts default.
fn prompt_with_default(
    rl: &mut rustyline::DefaultEditor,
    prompt: &str,
    default: &str,
) -> Result<String> {
    use rustyline::error::ReadlineError;
    let full = format!("{} [{}]: ", prompt, default);
    let raw = match rl.readline_with_initial(&full, (default, "")) {
        Ok(s) => s,
        Err(ReadlineError::Interrupted) => return Err(anyhow!("interrupted")),
        Err(ReadlineError::Eof) => return Err(anyhow!("EOF on stdin")),
        Err(e) => return Err(anyhow!("readline error: {}", e)),
    };
    let chosen = if raw.trim().is_empty() {
        default.to_string()
    } else {
        raw.trim().to_string()
    };
    Ok(chosen)
}

/// Like prompt_with_default but with no inline default — used for required fields like API keys.
fn prompt_required(rl: &mut rustyline::DefaultEditor, prompt: &str) -> Result<String> {
    use rustyline::error::ReadlineError;
    loop {
        let raw = match rl.readline(&format!("{}: ", prompt)) {
            Ok(s) => s,
            Err(ReadlineError::Interrupted) => return Err(anyhow!("interrupted")),
            Err(ReadlineError::Eof) => return Err(anyhow!("EOF on stdin")),
            Err(e) => return Err(anyhow!("readline error: {}", e)),
        };
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
        eprintln!("Required — please enter a value (Ctrl-C to abort).");
    }
}

/// Top-level entry: `hermes setup [section]`.
pub async fn run_setup(section: Option<&str>, mode: WizardMode) -> Result<()> {
    let hermes_home = get_hermes_home();
    std::fs::create_dir_all(&hermes_home).context("creating HERMES_HOME")?;

    let mut config = Config::load().unwrap_or_default();
    let mut rl = make_wizard_editor()?;

    match section {
        None => run_minimum_viable_flow(&mut config, &hermes_home, &mut rl, mode).await?,
        Some("model") => run_model_section(&mut config, &mut rl).await?,
        Some("memory") => run_memory_section(&mut config, &hermes_home, &mut rl).await?,
        Some("gateway") => run_gateway_section(&mut config, &mut rl).await?,
        Some("tools") => run_tools_section(&mut config, &mut rl).await?,
        Some("agent") => anyhow::bail!("section deferred to Phase 26"),
        Some("skills") => anyhow::bail!("section deferred to Phase 28"),
        Some(other) => anyhow::bail!(
            "unknown setup section: {} (valid: model, memory, gateway, tools)",
            other
        ),
    }

    // Persist the typed Config struct for section flows that don't save mid-stream.
    // (minimum_viable_flow and memory_section already save internally before the
    //  learning.* splice, so this is a safe idempotent belt-and-suspenders write.)
    config
        .save_to(&hermes_home.join("config.yaml"))
        .context("writing config.yaml")?;
    Ok(())
}

async fn run_minimum_viable_flow(
    config: &mut Config,
    hermes_home: &Path,
    rl: &mut rustyline::DefaultEditor,
    _mode: WizardMode,
) -> Result<()> {
    use ironhermes_core::config_setter;

    println!("\nWelcome to IronHermes. Let's get you configured.\n");

    // 1. Provider
    let provider = prompt_with_default(rl, "Provider", "openrouter")?;
    apply_provider_answer(config, &provider, "openrouter");

    // 2. API key
    let api_key = prompt_required(rl, &format!("API key for {}", provider))?;
    apply_api_key_answer(config, &api_key);

    // 3. Default model
    let default_model = "openrouter/qwen-2.5-coder-32b";
    let model = prompt_with_default(rl, "Default model", default_model)?;
    apply_model_answer(config, &model, default_model);

    // 4. Learning Loop opt-in (D-14, D-16) — verbatim framing first.
    println!("\n{}\n", LEARNING_LOOP_FRAMING);
    let learning_answer = prompt_with_default(rl, "Enable IronHermes' Learning Loop?", "Y")?;
    let learning_block = apply_learning_loop_answer(config, &learning_answer);

    // 5. Persist the learning.* block via config_setter (D-15 — preserves unknown keys).
    // Write the typed Config first via save_to, then splice learning.* keys.
    config
        .save_to(&hermes_home.join("config.yaml"))
        .context("writing config.yaml")?;
    for (key, value) in &learning_block {
        let key_str = key
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("non-string key in learning block"))?;
        let dotted = format!("learning.{}", key_str);
        let value_str = match value {
            serde_yaml::Value::Bool(b) => b.to_string(),
            serde_yaml::Value::Number(n) => n.to_string(),
            serde_yaml::Value::String(s) => s.clone(),
            other => serde_yaml::to_string(other)?.trim().to_string(),
        };
        config_setter::config_set(hermes_home, &dotted, &value_str)?;
    }

    println!(
        "\nSetup complete. Configuration written to {}.",
        hermes_home.join("config.yaml").display()
    );
    Ok(())
}

async fn run_model_section(
    config: &mut Config,
    rl: &mut rustyline::DefaultEditor,
) -> Result<()> {
    let provider = prompt_with_default(rl, "Provider", &config.model.provider)?;
    apply_provider_answer(config, &provider, "openrouter");
    let api_key = prompt_required(rl, &format!("API key for {}", provider))?;
    apply_api_key_answer(config, &api_key);
    let model = prompt_with_default(rl, "Default model", &config.model.default)?;
    apply_model_answer(config, &model, "openrouter/qwen-2.5-coder-32b");
    Ok(())
}

async fn run_memory_section(
    config: &mut Config,
    hermes_home: &Path,
    rl: &mut rustyline::DefaultEditor,
) -> Result<()> {
    use ironhermes_core::config_setter;

    // Reframe Learning Loop opt-in for re-configuration context.
    let current = if config.memory.memory_enabled {
        "enabled"
    } else {
        "disabled"
    };
    let action = if config.memory.memory_enabled {
        "Keep"
    } else {
        "Enable"
    };
    println!("\n{}\n", LEARNING_LOOP_FRAMING);
    let prompt = format!(
        "Learning Loop is currently {}. {} Learning Loop?",
        current, action
    );
    let learning_answer = prompt_with_default(rl, &prompt, "Y")?;
    let block = apply_learning_loop_answer(config, &learning_answer);

    // Memory backend choice.
    let backend = prompt_with_default(
        rl,
        "Memory backend (file/sqlite/grafeo/duckdb)",
        &config.memory.provider,
    )?;
    apply_memory_provider_answer(config, &backend, "file")?;

    // HERMES_HOME path (informational only — actual env var resolution is shell-time).
    let home_default = hermes_home.display().to_string();
    let home = prompt_with_default(rl, "HERMES_HOME path", &home_default)?;
    let _resolved = ironhermes_core::wizard::apply_hermes_home_answer(&home, &home_default);
    // Phase 24 (CFG-04) actually persists this — for now we just echo it back.

    // Splice learning.* block via config_setter (preserves unknown keys per D-15).
    config.save_to(&hermes_home.join("config.yaml"))?;
    for (k, v) in &block {
        let key_str = k
            .as_str()
            .ok_or_else(|| anyhow!("non-string key in learning block"))?;
        let dotted = format!("learning.{}", key_str);
        let value_str = match v {
            serde_yaml::Value::Bool(b) => b.to_string(),
            serde_yaml::Value::Number(n) => n.to_string(),
            serde_yaml::Value::String(s) => s.clone(),
            other => serde_yaml::to_string(other)?.trim().to_string(),
        };
        config_setter::config_set(hermes_home, &dotted, &value_str)?;
    }
    Ok(())
}

async fn run_gateway_section(
    _config: &mut Config,
    _rl: &mut rustyline::DefaultEditor,
) -> Result<()> {
    // Phase 23: dispatch surface only. Phase 25/26 plug in real questions.
    println!("Gateway setup will gain Telegram/Discord prompts in Phase 25 (TOOL-05).");
    Ok(())
}

async fn run_tools_section(
    _config: &mut Config,
    _rl: &mut rustyline::DefaultEditor,
) -> Result<()> {
    // Phase 23: dispatch surface only. Phase 25 (TOOL-05) plugs in real questions.
    println!("Tools setup will gain prerequisite-check prompts in Phase 25 (TOOL-05).");
    Ok(())
}

/// Testability seam — drives minimum-viable flow with pre-scripted strings.
/// Tests in `tests/setup_wizard.rs` call this directly to bypass rustyline.
pub fn apply_minimum_viable_answers(
    config: &mut Config,
    provider: &str,
    api_key: &str,
    model: &str,
    learning_loop: &str,
) -> serde_yaml::Mapping {
    apply_provider_answer(config, provider, "openrouter");
    apply_api_key_answer(config, api_key);
    apply_model_answer(config, model, ironhermes_core::constants::DEFAULT_MODEL);
    apply_learning_loop_answer(config, learning_loop)
}

/// Testability seam for the memory section.
pub fn apply_memory_section_answers(
    config: &mut Config,
    learning_loop: &str,
    backend: &str,
    _home_path: &str,
) -> Result<serde_yaml::Mapping> {
    let block = apply_learning_loop_answer(config, learning_loop);
    apply_memory_provider_answer(config, backend, "file")?;
    Ok(block)
}
