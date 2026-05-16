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
    let val = if raw_input.trim().is_empty() {
        default
    } else {
        raw_input.trim()
    };
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
pub fn apply_learning_loop_answer(config: &mut Config, raw_input: &str) -> serde_yaml::Mapping {
    let trimmed = raw_input.trim();
    let enabled = trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("y")
        || trimmed.eq_ignore_ascii_case("yes");

    // memory.* — typed Config fields.
    config.memory.memory_enabled = enabled;
    config.memory.user_profile_enabled = enabled;

    // Phase 32 LEARN-01: companion write of the typed runtime field
    // `memory.nudge_interval` (turn count, default 10). The wizard surfaces
    // this as "every N turns" in its prompt text; the runtime is turn-based,
    // not time-based, so the typed `MemoryConfig.nudge_interval` field
    // (Phase 32 Plan 01 Task 1) is the canonical source of truth. The
    // legacy `learning.periodic_nudge_interval_seconds` key continues to be
    // written below for ROADMAP Phase 32 Success Criterion 4 back-compat.
    config.memory.nudge_interval = 10;

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
        anyhow::bail!(
            "unknown memory provider: {} (valid: {})",
            chosen,
            VALID.join(", ")
        );
    }
    config.memory.provider = chosen.to_string();
    Ok(())
}

/// Resolve a HERMES_HOME path answer. Empty input returns the default.
/// Path normalization (~ expansion, abs-path) is the caller's job.
pub fn apply_hermes_home_answer(raw_input: &str, default: &str) -> String {
    let trimmed = raw_input.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Apply gateway section answer (stub — Phase 25/26 hooks plug in later).
pub fn apply_gateway_section_answer(
    _config: &mut Config,
    _enable_telegram: &str,
) -> anyhow::Result<()> {
    Ok(())
}

/// Apply tools section answer (stub — Phase 25/26 hooks plug in later).
pub fn apply_tools_section_answer(_config: &mut Config, _selection: &str) -> anyhow::Result<()> {
    Ok(())
}

/// Phase 26 D-05/D-19: write Config.auxiliary from wizard input.
/// Pure mutation — no I/O. Skip semantics match D-06 default-None: if `provider`
/// is empty/whitespace, leave config.auxiliary unchanged (operator opted out).
/// Mirrors apply_provider_answer pattern (lines 37-44).
pub fn apply_auxiliary_answer(config: &mut Config, provider: &str, model: &str) {
    use crate::config::AuxiliaryConfig;
    let p = provider.trim();
    let m = model.trim();
    if p.is_empty() {
        // D-06: auxiliary is optional; empty input means "don't enable".
        return;
    }
    config.auxiliary = AuxiliaryConfig {
        provider: p.to_string(),
        model: m.to_string(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn apply_auxiliary_answer_writes_when_provider_nonempty() {
        let mut config = Config::default();
        apply_auxiliary_answer(&mut config, "openai", "gpt-4o-mini");
        assert!(
            config.auxiliary.is_set(),
            "auxiliary must be set after non-empty provider"
        );
        assert_eq!(config.auxiliary.provider, "openai");
        assert_eq!(config.auxiliary.model, "gpt-4o-mini");
    }

    #[test]
    fn apply_auxiliary_answer_skips_when_provider_empty() {
        let mut config = Config::default();
        // D-06: empty provider = skip, auxiliary stays unset.
        apply_auxiliary_answer(&mut config, "", "gpt-4o-mini");
        assert!(
            !config.auxiliary.is_set(),
            "auxiliary MUST remain unset when provider is empty (D-06)"
        );
    }

    #[test]
    fn apply_auxiliary_answer_trims_whitespace() {
        let mut config = Config::default();
        apply_auxiliary_answer(&mut config, "  openai  ", "  gpt-4o-mini  ");
        assert_eq!(
            config.auxiliary.provider, "openai",
            "provider must be trimmed"
        );
        assert_eq!(
            config.auxiliary.model, "gpt-4o-mini",
            "model must be trimmed"
        );
    }

    #[test]
    fn apply_auxiliary_answer_overwrites_existing() {
        use crate::config::AuxiliaryConfig;
        let mut config = Config::default();
        config.auxiliary = AuxiliaryConfig {
            provider: "old-provider".to_string(),
            model: "old-model".to_string(),
        };
        apply_auxiliary_answer(&mut config, "openai", "gpt-4o-mini");
        assert_eq!(
            config.auxiliary.provider, "openai",
            "must overwrite existing provider"
        );
        assert_eq!(
            config.auxiliary.model, "gpt-4o-mini",
            "must overwrite existing model"
        );
    }
}
