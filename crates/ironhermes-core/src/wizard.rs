//! `wizard.rs` — pure-function wizard helpers (apply_*_answer).
//!
//! No I/O dependency. All rustyline / stdin interaction lives in
//! `ironhermes-cli/src/setup.rs` (Plan 23-02). Import and call these
//! functions from the rustyline-driven wizard runner.

use crate::config::Config;
use serde::{Deserialize, Serialize};

/// Wizard mode discriminator. Cross-crate plain-data type per D-12.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WizardMode {
    /// Explicit `hermes setup` invocation — full minimum-viable flow.
    Explicit,
    /// First-run auto-launch (no config.yaml present) — same flow.
    FirstRun,
    /// Auto-launch with existing-but-invalid config — preserves valid sections, repairs broken (D-05).
    FixMode,
}

/// Verbatim Learning Loop framing paragraph (D-16).
/// Locked by regression test in tests/wizard_flow.rs — do not silently edit.
pub const LEARNING_LOOP_FRAMING: &str = "\
IronHermes can curate its own memory and write its own skills as you use it — \
this is the \"Learning Loop\" that makes the agent grow with you instead of \
starting fresh every session. We strongly recommend enabling it now (you can \
always disable later with `hermes config set memory.enabled false`).";

/// Apply a wizard answer for `model.default`. Empty input accepts default.
pub fn apply_model_answer(config: &mut Config, raw_input: &str, default: &str) {
    let val = if raw_input.trim().is_empty() { default } else { raw_input.trim() };
    config.model.default = val.to_string();
}

/// Apply a wizard answer for `model.provider`. Empty input keeps the existing/default value.
pub fn apply_provider_answer(config: &mut Config, raw_input: &str, default: &str) {
    let trimmed = raw_input.trim();
    if !trimmed.is_empty() {
        config.model.provider = trimmed.to_string();
    } else if config.model.provider.is_empty() {
        config.model.provider = default.to_string();
    }
}

/// Apply a wizard answer for `model.api_key`. Empty input does NOT clear an existing key —
/// it would silently break the user's setup. The wizard re-prompts on empty when required.
pub fn apply_api_key_answer(config: &mut Config, raw_input: &str) {
    let trimmed = raw_input.trim();
    if !trimmed.is_empty() {
        config.model.api_key = Some(trimmed.to_string());
    }
    // Empty: leave config.model.api_key untouched.
}

/// Apply Learning Loop opt-in answer (D-14, D-16).
///
/// Empty input or "y"/"Y"/"yes"/"YES" → enabled (default YES per D-14).
/// "n"/"N"/"no"/"NO" → explicit disabled (never absent — D-14 final paragraph).
///
/// Returns a `serde_yaml::Mapping` representing the `learning:` block to be
/// merged into config.yaml. The caller (Plan 23-02 setup.rs) is responsible
/// for splicing this into the live config.yaml using `serde_yaml::Value`
/// load-mutate-save (so unknown keys survive — D-15).
pub fn apply_learning_loop_answer(
    config: &mut Config,
    raw_input: &str,
) -> serde_yaml::Mapping {
    let trimmed = raw_input.trim();
    let enabled = trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("y")
        || trimmed.eq_ignore_ascii_case("yes");

    // memory.* — typed Config fields.
    config.memory.memory_enabled = enabled;
    config.memory.user_profile_enabled = enabled;

    // learning.* — serde_yaml::Mapping for unknown-key survival (D-15).
    let mut block = serde_yaml::Mapping::new();
    block.insert(
        serde_yaml::Value::String("skill_generation_enabled".into()),
        serde_yaml::Value::Bool(enabled),
    );
    block.insert(
        serde_yaml::Value::String("periodic_nudge_interval_seconds".into()),
        serde_yaml::Value::Number(300u64.into()),
    );
    block.insert(
        serde_yaml::Value::String("reflection_depth".into()),
        serde_yaml::Value::String("standard".into()),
    );
    block.insert(
        serde_yaml::Value::String("skill_eval".into()),
        serde_yaml::Value::Bool(enabled),
    );
    block.insert(
        serde_yaml::Value::String("max_skills".into()),
        serde_yaml::Value::Number(500u64.into()),
    );
    block
}

/// Apply memory backend choice. Returns Err for unknown providers.
pub fn apply_memory_provider_answer(
    config: &mut Config,
    raw_input: &str,
    default: &str,
) -> anyhow::Result<()> {
    let val = raw_input.trim();
    let chosen = if val.is_empty() { default } else { val };
    const VALID: &[&str] = &["file", "sqlite", "grafeo", "duckdb"];
    if !VALID.contains(&chosen) {
        anyhow::bail!("unknown memory provider: {} (valid: {})", chosen, VALID.join(", "));
    }
    config.memory.provider = chosen.to_string();
    Ok(())
}

/// Resolve a HERMES_HOME path answer. Empty input returns the default.
/// Path normalization (~ expansion, abs-path) is the caller's job.
pub fn apply_hermes_home_answer(raw_input: &str, default: &str) -> String {
    let trimmed = raw_input.trim();
    if trimmed.is_empty() { default.to_string() } else { trimmed.to_string() }
}

/// Apply gateway section answer (stub — Phase 25/26 hooks plug in later).
pub fn apply_gateway_section_answer(_config: &mut Config, _enable_telegram: &str) -> anyhow::Result<()> {
    Ok(())
}

/// Apply tools section answer (stub — Phase 25/26 hooks plug in later).
pub fn apply_tools_section_answer(_config: &mut Config, _selection: &str) -> anyhow::Result<()> {
    Ok(())
}
