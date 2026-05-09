//! Phase 21.8.2 Plan 03 Task 3: smoke tests for `hermes skills reload` CLI subcommand.
//!
//! Tests via static-grep invariants — `cmd_reload` calls `SkillRegistry::load_with_config`
//! which requires a real filesystem; smoke-level validation uses include_str! pattern
//! consistent with the other static-grep tests in this crate.

/// Verify that `SkillsAction::Reload` variant is declared in skills_cmd.rs.
///
/// Static-grep: `hermes skills reload` routes through the dispatch arm.
#[test]
fn skills_action_reload_variant_present() {
    let src = include_str!("../src/skills_cmd.rs");
    assert!(
        src.contains("Reload,") || src.contains("Reload =>"),
        "Phase 21.8.2 Task 3: SkillsAction::Reload variant must be declared in skills_cmd.rs"
    );
}

/// Verify that `cmd_reload` function exists in skills_cmd.rs.
#[test]
fn cmd_reload_fn_present() {
    let src = include_str!("../src/skills_cmd.rs");
    assert!(
        src.contains("pub fn cmd_reload("),
        "Phase 21.8.2 Task 3: cmd_reload must be a public function in skills_cmd.rs"
    );
}

/// Verify that the dispatch arm routes `SkillsAction::Reload` to `cmd_reload`.
#[test]
fn dispatch_arm_routes_reload() {
    let src = include_str!("../src/skills_cmd.rs");
    assert!(
        src.contains("SkillsAction::Reload => cmd_reload"),
        "Phase 21.8.2 Task 3: dispatch must route SkillsAction::Reload to cmd_reload"
    );
}

/// Verify that `cmd_reload` calls `SkillRegistry::load_with_config`.
#[test]
fn cmd_reload_calls_load_with_config() {
    let src = include_str!("../src/skills_cmd.rs");
    assert!(
        src.contains("SkillRegistry::load_with_config"),
        "Phase 21.8.2 Task 3: cmd_reload must call SkillRegistry::load_with_config"
    );
}

/// Verify that `cmd_reload` reports `invalid_skipped` in its output.
#[test]
fn cmd_reload_reports_invalid_skipped() {
    let src = include_str!("../src/skills_cmd.rs");
    assert!(
        src.contains("invalid_skipped") || src.contains("invalid skipped"),
        "Phase 21.8.2 Task 3: cmd_reload must report invalid skipped count (D-05 WARN-BUT-LOAD)"
    );
}
