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

/// Apply kanban section wizard answer (D-10, Plan 05).
///
/// Returns a `serde_yaml::Mapping` with all 17 KanbanConfig fields and their
/// default values. The caller splices this under the `kanban:` key in config.yaml
/// via the raw-YAML load-mutate-save path (same pattern as apply_learning_loop_answer).
///
/// The `_enable` parameter is reserved for future use (e.g. a wizard yes/no prompt);
/// defaults are written regardless for now so every field is self-documenting.
pub fn apply_kanban_section_answer(config: &mut Config, _enable: &str) -> serde_yaml::Mapping {
    // Config does not have a typed kanban field (it is stored as serde_yaml::Value).
    // We only touch the returned Mapping; config is accepted for API symmetry with
    // the other apply_*_answer helpers.
    let _ = config;

    let mut block = serde_yaml::Mapping::new();

    macro_rules! kv_bool {
        ($k:expr, $v:expr) => {
            block.insert(
                serde_yaml::Value::String($k.into()),
                serde_yaml::Value::Bool($v),
            );
        };
    }
    macro_rules! kv_u64 {
        ($k:expr, $v:expr) => {
            block.insert(
                serde_yaml::Value::String($k.into()),
                serde_yaml::Value::Number(($v as u64).into()),
            );
        };
    }
    macro_rules! kv_u32 {
        ($k:expr, $v:expr) => {
            block.insert(
                serde_yaml::Value::String($k.into()),
                serde_yaml::Value::Number(($v as u64).into()),
            );
        };
    }
    macro_rules! kv_null {
        ($k:expr) => {
            block.insert(
                serde_yaml::Value::String($k.into()),
                serde_yaml::Value::Null,
            );
        };
    }
    macro_rules! kv_str {
        ($k:expr, $v:expr) => {
            block.insert(
                serde_yaml::Value::String($k.into()),
                serde_yaml::Value::String($v.into()),
            );
        };
    }

    // 17 KanbanConfig fields in declaration order (config.rs).
    kv_bool!("dispatch_in_gateway", true);
    kv_u64!("dispatch_interval_seconds", 60u64);
    kv_u64!("max_in_progress", 8u64);
    kv_u32!("failure_limit", 2u32);
    kv_u64!("stranded_threshold_seconds", 1800u64);
    kv_u64!("dispatch_stale_timeout_seconds", 14400u64);
    kv_null!("default_workdir");
    kv_null!("notification_sources");
    kv_u64!("notifier_poll_seconds", 3u64);
    kv_bool!("auto_decompose", false);
    kv_u32!("auto_decompose_per_tick", 3u32);
    kv_str!("orchestrator_profile", "");
    kv_str!("default_assignee", "");
    kv_str!("decomposer_model", "");
    kv_bool!("auto_promote_children", true);
    kv_str!("judge_model", "");
    kv_u32!("goal_inner_max_iterations", 10u32);

    block
}

/// Additive-merge helper: insert `section_key` into `yaml_value` only when absent.
///
/// Contract (D-11, never-clobber rule + G1 null-is-absent rule):
/// - If `yaml_value` is a Mapping and does NOT contain `section_key`, parse
///   `default_block` via `serde_yaml::from_str` and insert it under `section_key`.
/// - If `section_key` is present with a NON-NULL value, this is a no-op — the
///   existing value is never overwritten (D-11 idempotent / never-clobber).
/// - If `section_key` is present with `Value::Null`, it is treated as ABSENT and
///   populated with `default_block` (G1 + D-11 null-is-absent rule): install.sh
///   seeds every all-commented section as a bare key with no sub-fields, which
///   `serde_yaml` parses as `Null`. That Null is the absence-of-value sentinel,
///   NOT a value the user set — populating it does not violate never-clobber.
/// - If `yaml_value` is not a Mapping (e.g. Null on an empty file), it is
///   promoted to a Mapping before inserting, so a fresh-install always works.
///
/// The caller is responsible for the load-mutate-save cycle.
pub fn write_defaults_if_absent(
    yaml_value: &mut serde_yaml::Value,
    section_key: &str,
    default_block: &str,
) {
    // Promote Null to empty Mapping (fresh install / empty file).
    if yaml_value.is_null() {
        *yaml_value = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }

    if let serde_yaml::Value::Mapping(map) = yaml_value {
        let key = serde_yaml::Value::String(section_key.to_string());
        match map.get(&key) {
            Some(serde_yaml::Value::Null) => {
                // G1 + D-11 null-is-absent rule: present-but-null is the install.sh
                // seed sentinel (all sub-fields are commented out, so the section key
                // parses as Null). Treat it as absent and fall through to insert.
                // Remove the null entry first so the insert below works cleanly.
                map.remove(&key);
            }
            Some(_) => {
                // Key is present with a real non-null value — never clobber (D-11).
                return;
            }
            None => {
                // Key is absent — fall through to insert.
            }
        }
        // Parse the default_block YAML into a Value, then insert under the key.
        // If parsing fails (malformed constant), skip silently — don't crash setup.
        if let Ok(parsed) = serde_yaml::from_str::<serde_yaml::Value>(default_block) {
            map.insert(key, parsed);
        }
    }
}

// ---------------------------------------------------------------------------
// Default-block YAML strings for the 13 missing config sections (D-10).
// These mirror cli-config.yaml.example and are used by setup.rs via
// write_defaults_if_absent for a fresh install or on update/reinstall.
// All values are COMMENTED-OUT so user edits survive (additive merge only).
// ---------------------------------------------------------------------------

/// Return the default commented-block YAML string for a config section.
/// Returns `None` for unknown section names.
pub fn default_block_for(section: &str) -> Option<&'static str> {
    match section {
        "agent" => Some(AGENT_DEFAULT_BLOCK),
        "terminal" => Some(TERMINAL_DEFAULT_BLOCK),
        "web" => Some(WEB_DEFAULT_BLOCK),
        "exec" => Some(EXEC_DEFAULT_BLOCK),
        "cron" => Some(CRON_DEFAULT_BLOCK),
        "compression" => Some(COMPRESSION_DEFAULT_BLOCK),
        "skills" => Some(SKILLS_DEFAULT_BLOCK),
        "delegation" => Some(DELEGATION_DEFAULT_BLOCK),
        "rate_limit" => Some(RATE_LIMIT_DEFAULT_BLOCK),
        "batch" => Some(BATCH_DEFAULT_BLOCK),
        "security" => Some(SECURITY_DEFAULT_BLOCK),
        "custom_providers" => Some(CUSTOM_PROVIDERS_DEFAULT_BLOCK),
        "kanban" => Some(KANBAN_DEFAULT_BLOCK),
        "gateway" => Some(GATEWAY_DEFAULT_BLOCK),
        "tools" => Some(TOOLS_DEFAULT_BLOCK),
        _ => None,
    }
}

/// Agent section default block (mirrors cli-config.yaml.example).
/// The `{}` base ensures serde_yaml::from_str parses to an empty Mapping rather than Null,
/// so the section key is inserted with a valid value on a fresh install.
pub const AGENT_DEFAULT_BLOCK: &str = "{}";

/// Terminal section default block.
pub const TERMINAL_DEFAULT_BLOCK: &str = "{}";

/// Web section default block.
pub const WEB_DEFAULT_BLOCK: &str = "{}";

/// Exec section default block.
pub const EXEC_DEFAULT_BLOCK: &str = "{}";

/// Cron section default block.
pub const CRON_DEFAULT_BLOCK: &str = "{}";

/// Compression section default block.
pub const COMPRESSION_DEFAULT_BLOCK: &str = "{}";

/// Skills section default block.
pub const SKILLS_DEFAULT_BLOCK: &str = "{}";

/// Delegation section default block.
pub const DELEGATION_DEFAULT_BLOCK: &str = "{}";

/// Rate limit section default block.
pub const RATE_LIMIT_DEFAULT_BLOCK: &str = "{}";

/// Batch section default block.
pub const BATCH_DEFAULT_BLOCK: &str = "{}";

/// Security section default block.
pub const SECURITY_DEFAULT_BLOCK: &str = "{}";

/// Custom providers section default block (empty list placeholder).
pub const CUSTOM_PROVIDERS_DEFAULT_BLOCK: &str = "[]";

/// Kanban section default block (all 17 fields, commented).
/// Actual values come from apply_kanban_section_answer which builds a typed Mapping;
/// this constant is a fallback for the write_defaults_if_absent path.
pub const KANBAN_DEFAULT_BLOCK: &str = "{}";

/// Gateway section default block (empty platforms map).
pub const GATEWAY_DEFAULT_BLOCK: &str = "platforms: {}";

/// Tools section default block.
pub const TOOLS_DEFAULT_BLOCK: &str = "{}";

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

    // -----------------------------------------------------------------------
    // Plan 05 inline tests: apply_kanban_section_answer + write_defaults_if_absent
    // -----------------------------------------------------------------------

    #[test]
    fn apply_kanban_section_answer_has_17_fields() {
        let mut config = Config::default();
        let block = apply_kanban_section_answer(&mut config, "");
        let expected_fields = &[
            "dispatch_in_gateway",
            "dispatch_interval_seconds",
            "max_in_progress",
            "failure_limit",
            "stranded_threshold_seconds",
            "dispatch_stale_timeout_seconds",
            "default_workdir",
            "notification_sources",
            "notifier_poll_seconds",
            "auto_decompose",
            "auto_decompose_per_tick",
            "orchestrator_profile",
            "default_assignee",
            "decomposer_model",
            "auto_promote_children",
            "judge_model",
            "goal_inner_max_iterations",
        ];
        for field in expected_fields {
            assert!(
                block.contains_key(serde_yaml::Value::String(field.to_string())),
                "apply_kanban_section_answer must emit field `{field}`"
            );
        }
        assert_eq!(block.len(), 17, "block must contain exactly 17 entries");
    }

    #[test]
    fn write_defaults_if_absent_inserts_when_absent() {
        let mut yaml: serde_yaml::Value = serde_yaml::from_str("{}").unwrap();
        write_defaults_if_absent(&mut yaml, "new_section", "enabled: false");
        let map = yaml.as_mapping().expect("must remain a mapping");
        assert!(
            map.contains_key(serde_yaml::Value::String("new_section".to_string())),
            "write_defaults_if_absent must insert the key when absent"
        );
    }

    #[test]
    fn write_defaults_if_absent_is_idempotent_no_clobber() {
        let mut yaml: serde_yaml::Value = serde_yaml::from_str("{}").unwrap();
        write_defaults_if_absent(&mut yaml, "test_section", "value: original");
        let after_first = yaml.clone();
        write_defaults_if_absent(&mut yaml, "test_section", "value: clobbered");
        assert_eq!(
            yaml, after_first,
            "write_defaults_if_absent must not clobber an existing section on second call (D-11)"
        );
    }

    // -----------------------------------------------------------------------
    // Plan 07 Task 1: null-aware write_defaults_if_absent tests (G1 fix)
    // -----------------------------------------------------------------------

    #[test]
    fn write_defaults_if_absent_replaces_present_but_null() {
        // G1 root cause: install.sh seeds `kanban:` with no sub-fields, which
        // parses as `kanban: null` (present-but-null). The old code treated this
        // as "present" and returned early without populating. Per D-11, a
        // null value is the absence-of-value sentinel, NOT a user-set value.
        let mut yaml: serde_yaml::Value = serde_yaml::from_str("kanban:\n").unwrap();
        {
            let map = yaml.as_mapping().expect("must be a mapping");
            let key = serde_yaml::Value::String("kanban".to_string());
            assert!(
                map.contains_key(&key),
                "test precondition: kanban key must be present"
            );
            assert_eq!(
                map.get(&key),
                Some(&serde_yaml::Value::Null),
                "test precondition: kanban value must be Null"
            );
        }
        write_defaults_if_absent(&mut yaml, "kanban", "foo: 1");
        let map = yaml.as_mapping().expect("must remain a mapping");
        let val = map
            .get(serde_yaml::Value::String("kanban".to_string()))
            .expect("kanban key must still be present");
        assert_ne!(
            val,
            &serde_yaml::Value::Null,
            "write_defaults_if_absent must replace present-but-null with the default block (G1 + D-11 null-is-absent rule)"
        );
    }

    #[test]
    fn write_defaults_if_absent_preserves_present_non_null() {
        // D-11 never-clobber: a present non-null mapping must survive a second call.
        let mut yaml: serde_yaml::Value = serde_yaml::from_str("{}").unwrap();
        write_defaults_if_absent(&mut yaml, "kanban", "max_in_progress: 5");
        let before = yaml.clone();
        // Second call with a different block must be a no-op.
        write_defaults_if_absent(&mut yaml, "kanban", "max_in_progress: 99");
        assert_eq!(
            yaml, before,
            "write_defaults_if_absent must NOT clobber a present non-null value (D-11)"
        );
        // Confirm the value is still 5, not 99.
        let map = yaml.as_mapping().unwrap();
        let kanban = map
            .get(serde_yaml::Value::String("kanban".to_string()))
            .expect("kanban must be present");
        let inner = kanban.as_mapping().expect("kanban must be a mapping");
        let mip = inner
            .get(serde_yaml::Value::String("max_in_progress".to_string()))
            .expect("max_in_progress must be present");
        assert_eq!(
            mip.as_u64(),
            Some(5),
            "max_in_progress must remain 5 (D-11 never-clobber)"
        );
    }

    // -----------------------------------------------------------------------
    // Existing tests for apply_auxiliary_answer
    // -----------------------------------------------------------------------

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
        let mut config = Config {
            auxiliary: AuxiliaryConfig {
                provider: "old-provider".to_string(),
                model: "old-model".to_string(),
            },
            ..Default::default()
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
