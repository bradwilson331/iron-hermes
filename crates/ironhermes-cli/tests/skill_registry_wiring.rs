//! Phase 21.8.2 Plan 02: static invariants enforcing the with_skill_registry
//! wiring at all 4 CommandContext construction sites. Plan 03 builds on this.

#[test]
fn with_skill_registry_present_in_main_rs() {
    let src = include_str!("../src/main.rs");
    // Both run_chat and run_single CommandContext builds go through the shared
    // build_cmd_ctx helper (Phase 21.7-11 extraction), which contains the single
    // .with_skill_registry( call that covers both dispatch sites.
    let count = src.matches("with_skill_registry(").count();
    assert!(
        count >= 1,
        "Phase 21.8.2 Plan 02: main.rs must call .with_skill_registry( in build_cmd_ctx (covers run_chat + run_single); found {count}"
    );
}

#[test]
fn with_skill_registry_present_in_tui_rata_commands() {
    let src = include_str!("../src/tui_rata/commands.rs");
    assert!(
        src.contains(".with_skill_registry("),
        "Phase 21.8.2 Plan 02: tui_rata/commands.rs build_command_context must call .with_skill_registry()"
    );
}

#[test]
fn skill_registry_field_present_in_tui_rata_app() {
    let src = include_str!("../src/tui_rata/app.rs");
    assert!(
        src.contains("pub skill_registry: Option<Arc<ironhermes_core::SkillRegistry>>")
            || src.contains("pub skill_registry: Option<Arc<SkillRegistry>>"),
        "Phase 21.8.2 Plan 02: tui_rata/app.rs App struct must have a skill_registry field"
    );
}

#[test]
fn run_chat_skill_registry_is_mut() {
    let src = include_str!("../src/main.rs");
    assert!(
        src.contains("let mut skill_registry = runtime_bundle.skill_registry.clone()"),
        "Phase 21.8.2 Plan 02: run_chat skill_registry local must be `let mut` so Plan 03 reload arm can reassign"
    );
}
