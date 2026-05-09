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

// ── Phase 21.8.2 Plan 03 static-grep invariants ────────────────────────────

#[test]
fn main_rs_skillsreload_arm_calls_load_with_config() {
    let src = include_str!("../src/main.rs");
    assert!(src.contains("CommandResult::SkillsReload =>"));
    assert!(
        src.contains("SkillRegistry::load_with_config"),
        "Phase 21.8.2 Plan 03: SkillsReload arm must call SkillRegistry::load_with_config"
    );
}

#[test]
fn main_rs_skillactivated_arm_calls_activate_skill() {
    let src = include_str!("../src/main.rs");
    assert!(src.contains("CommandResult::SkillActivated"));
    assert!(
        src.contains("activate_skill(&name"),
        "Phase 21.8.2 Plan 03: SkillActivated arm must call prompt_builder.activate_skill(&name, ...)"
    );
}

#[test]
fn main_rs_skill13_fallback_present() {
    // The SKILL-13 fallback for the classic-TUI REPL lives in tui/commands.rs
    // dispatch_command() NotFound branch, which the run_chat REPL loop calls.
    // The SkillActivated variant is referenced in both main.rs (consumer arm)
    // and tui/commands.rs (NotFound producer).
    let main_src = include_str!("../src/main.rs");
    assert!(main_src.contains("CommandResult::SkillActivated"));
    let tui_src = include_str!("../src/tui/commands.rs");
    assert!(
        tui_src.contains("registry.find("),
        "Phase 21.8.2 Plan 03: tui/commands.rs dispatch_command NotFound branch must include SKILL-13 fallback"
    );
}

#[test]
fn main_rs_no_unreachable_for_skills_variants() {
    let src = include_str!("../src/main.rs");
    assert!(
        !src.contains("Plan 03 lands the SkillsReload"),
        "Plan 02 placeholder must be replaced by Plan 03"
    );
    assert!(
        !src.contains("Plan 03 lands the SkillActivated"),
        "Plan 02 placeholder must be replaced by Plan 03"
    );
}

#[test]
fn main_rs_skillsreload_arm_reports_invalid_skipped() {
    let src = include_str!("../src/main.rs");
    assert!(
        src.contains("invalid skipped"),
        "Phase 21.8.2 D-05: SkillsReload arm must report invalid_skipped count in diff output"
    );
    assert!(
        src.contains("count_invalid_skipped"),
        "Phase 21.8.2 D-Plan03-06: helper count_invalid_skipped must be called from SkillsReload arm"
    );
}

#[test]
fn tui_rata_skillsreload_handled() {
    let src = include_str!("../src/tui_rata/commands.rs");
    assert!(src.contains("SlashOutcome::SkillsReload"));
    assert!(
        !src.contains("Plan 03 lands TUI skill reload integration"),
        "Plan 02 placeholder must be replaced by Plan 03"
    );
}

#[test]
fn tui_rata_skill13_fallback_at_notfound() {
    let src = include_str!("../src/tui_rata/commands.rs");
    assert!(src.contains("SlashOutcome::SkillActivated"));
    assert!(
        src.contains("registry.find(cmd_token)") || src.contains("registry.find("),
        "Phase 21.8.2 Plan 03: tui_rata NotFound branch must include SKILL-13 fallback"
    );
}

#[test]
fn tui_rata_app_has_skills_config() {
    let src = include_str!("../src/tui_rata/app.rs");
    let count = src.matches("pub skills_config:").count();
    assert!(
        count >= 2,
        "Phase 21.8.2 Plan 03: tui_rata/app.rs must have pub skills_config: on AppDeps AND App (found {count})"
    );
}

#[test]
fn tui_rata_app_has_pending_skill_overlays() {
    let src = include_str!("../src/tui_rata/app.rs");
    assert!(
        src.contains("pub pending_skill_overlays: Vec<(String, String)>"),
        "Phase 21.8.2 Plan 03 D-07 (TUI): pending_skill_overlays buffer must exist on App"
    );
}

#[test]
fn tui_rata_apply_slash_outcome_uses_history_push_for_skills() {
    let src = include_str!("../src/tui_rata/app.rs");
    assert!(
        src.contains("SlashOutcome::SkillsReload")
            && src.contains("Role::System")
            && src.contains("self.history.push("),
        "Phase 21.8.2 Plan 03: TUI SkillsReload arm must use Role::System + self.history.push (verified app.rs:524-528 pattern)"
    );
}
