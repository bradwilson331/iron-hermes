//! Wizard section-coverage test scaffold for new wizard.rs functions.
//! REQ-37.1-04: The wizard must cover every Config section via apply_*_answer helpers.
//! No rustyline, no I/O, no async.
//!
//! INTENTIONALLY RED (Wave 0 scaffolding):
//! `apply_kanban_section_answer` and `write_defaults_if_absent` do NOT exist yet in wizard.rs.
//! These tests FAIL TO COMPILE until Plan 05 adds the two functions.
//! Turns GREEN in Plan 05 (wizard extension).

use ironhermes_core::config::Config;
use ironhermes_core::wizard::{apply_kanban_section_answer, write_defaults_if_absent};

/// REQ-37.1-04: apply_kanban_section_answer must return a serde_yaml::Mapping containing
/// all 17 KanbanConfig fields (from RESEARCH.md §Q2 / PATTERNS.md kanban section).
/// WAVE-0 RED: FAILS TO COMPILE — apply_kanban_section_answer is not yet defined.
/// Turns GREEN in Plan 05.
#[test]
fn apply_kanban_section_answer_emits_all_17_fields() {
    let mut config = Config::default();
    let block = apply_kanban_section_answer(&mut config, "");

    // All 17 KanbanConfig fields must be present as keys in the returned Mapping.
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
            "apply_kanban_section_answer must emit field `{field}` in the returned Mapping \
             (REQ-37.1-04)"
        );
    }
}

/// REQ-37.1-04: write_defaults_if_absent must be idempotent —
/// the second call on a key that already exists must not overwrite the existing value.
/// WAVE-0 RED: FAILS TO COMPILE — write_defaults_if_absent is not yet defined.
/// Turns GREEN in Plan 05.
#[test]
fn write_defaults_if_absent_is_idempotent() {
    let mut yaml: serde_yaml::Value = serde_yaml::from_str("{}").unwrap();

    let first_block = "value: original";
    let second_block = "value: clobbered";

    write_defaults_if_absent(&mut yaml, "test_section", first_block);

    // Capture state after first call.
    let after_first = yaml.clone();

    // Second call with different content must NOT overwrite.
    write_defaults_if_absent(&mut yaml, "test_section", second_block);

    assert_eq!(
        yaml, after_first,
        "write_defaults_if_absent must not clobber an existing section on a second call \
         (REQ-37.1-04)"
    );
}

/// REQ-37.1-04: write_defaults_if_absent must insert the section when it is absent.
/// WAVE-0 RED: FAILS TO COMPILE — write_defaults_if_absent is not yet defined.
/// Turns GREEN in Plan 05.
#[test]
fn write_defaults_if_absent_inserts_when_missing() {
    let mut yaml: serde_yaml::Value = serde_yaml::from_str("{}").unwrap();

    let default_block = "enabled: false";

    write_defaults_if_absent(&mut yaml, "new_section", default_block);

    let map = yaml.as_mapping().expect("yaml must remain a mapping");
    assert!(
        map.contains_key(serde_yaml::Value::String("new_section".to_string())),
        "write_defaults_if_absent must insert the key when it is absent (REQ-37.1-04)"
    );
}
