//! Phase 33: static-grep regression gates for autonomous skill creation wiring.
//!
//! These tests lock the four cross-crate wiring surfaces touched by Phase 33
//! (Plans 33-01, 33-02, 33-03) so a future refactor cannot silently remove any
//! of them. They use the same `include_str!`-at-compile-time pattern as
//! `invariants_27_1_4_1_1.rs` and `invariants_22_4.rs` — no dev-deps, no I/O at
//! test time, no runtime path resolution required.
//!
//! Why each invariant exists:
//!
//! INV-33-01 — Plan 33-03 wires `register_skill_manage_tool` into the runtime
//!   factory. If a future cleanup removes the call site, every CLI/gateway/TUI
//!   session loses the `skill_manage` tool and the 'learning' toolset becomes
//!   dead config. The toolset_config filter (`set_toolset_config`) hides
//!   absent tools silently, so the breakage would surface only as an
//!   "unknown tool" runtime error from the LLM.
//!
//! INV-33-02 — Plan 33-01 injects skill-creation trigger guidance into the
//!   ToolGuidance slot of the system prompt when `skill_manage` is active.
//!   Without `skill_creation_guidance`, the agent has no behavioral instruction
//!   to call `skill_manage` after task completion — autonomous skill creation
//!   regresses to manual.
//!
//! INV-33-03 — Plan 33-01 adds `SkillSource::SelfCreated` to the SkillSource
//!   enum and updates every exhaustive match arm. The variant name appears at
//!   the declaration, the WARN-BUT-LOAD scan-enforcement arm, the exhaustive
//!   test, and (sometimes) doc comments. The ≥3 floor catches accidental
//!   removal of the variant (which would cascade compile errors across crates).
//!
//! INV-33-04 — Plan 33-01 promotes `validate_skill_name` from private to public
//!   so Plan 02's `skill_manage` tool can validate slugs cross-crate. Reverting
//!   the visibility would silently break the create/patch/edit slug guard.
//!
//! INV-33-05 — Plan 33-02 declares `pub mod skill_manage` in the
//!   `ironhermes-tools` crate. Without the module declaration, the tool type
//!   is unreachable from `register_skill_manage_tool`.
//!
//! INV-33-06 — Plan 33-03 adds "learning" to KNOWN_TOOLSETS (CLI validator)
//!   and to the CLI members_map. ≥2 occurrences catches a refactor that
//!   leaves the array entry but drops the map insert (or vice versa).
//!
//! INV-33-07 — Phase 34 D-05: AppState::init in iron_hermes_ui calls
//!   `build_app_runtime_bundle` so web turns inherit the full skill_manage
//!   toolset (Learning Loop) through the shared runtime bundle. Removing the
//!   call site would silently disconnect every web session from the tool
//!   registry, causing "unknown tool" errors at runtime with no compile-time
//!   signal. Locks the Phase 33 Plan 03 carry-over decision.
//!
//! INV-33-08 — Phase 32 Plan 03 / Phase 34 Success Criterion 1: run_web_turn
//!   in iron_hermes_ui fires `spawn_nudge_review` every `nudge_interval`
//!   successful web turns and maintains a `nudge_turns` counter for keying.
//!   Without both symbols, the web path has no Learning Loop nudge and web
//!   sessions never trigger autonomous memory consolidation.

const APP_RUNTIME_FACTORY_SOURCE: &str = include_str!("../src/app_runtime_factory.rs");
const PROMPT_BUILDER_SOURCE: &str = include_str!("../src/prompt_builder.rs");
const SKILLS_RS_SOURCE: &str = include_str!("../../ironhermes-core/src/skills.rs");
const TOOLS_LIB_SOURCE: &str = include_str!("../../ironhermes-tools/src/lib.rs");
const TOOLSET_CMD_SOURCE: &str = include_str!("../../ironhermes-cli/src/toolset_cmd.rs");

/// INV-33-01: `register_skill_manage_tool` is called in `app_runtime_factory.rs`
/// so every entry point (CLI, gateway, TUI) wires the learning-toolset tool.
#[test]
fn inv_33_01_register_skill_manage_tool_in_app_runtime_factory() {
    let count = APP_RUNTIME_FACTORY_SOURCE
        .matches("register_skill_manage_tool")
        .count();
    assert!(
        count >= 1,
        "INV-33-01: crates/ironhermes-agent/src/app_runtime_factory.rs must \
         call register_skill_manage_tool() so the 'learning' toolset's \
         skill_manage tool is wired into every runtime bundle. Found {count} \
         occurrences (expected >= 1). See Phase 33 Plan 03."
    );
}

/// INV-33-02: `skill_creation_guidance` field/setter is wired in prompt_builder
/// so the system prompt carries the Phase 33 trigger guidance block.
#[test]
fn inv_33_02_skill_creation_guidance_in_prompt_builder() {
    let count = PROMPT_BUILDER_SOURCE
        .matches("skill_creation_guidance")
        .count();
    assert!(
        count >= 1,
        "INV-33-02: crates/ironhermes-agent/src/prompt_builder.rs must \
         reference 'skill_creation_guidance' (field, setter, or const block) \
         so the Phase 33 trigger guidance is injected into the ToolGuidance \
         slot when skill_manage is active. Found {count} occurrences \
         (expected >= 1). See Phase 33 Plan 01."
    );
}

/// INV-33-03: `SkillSource::SelfCreated` variant exists in skills.rs
/// (declaration + exhaustive-match sites; floor ≥3 covers removal).
#[test]
fn inv_33_03_self_created_variant_in_skills_rs() {
    let count = SKILLS_RS_SOURCE.matches("SelfCreated").count();
    assert!(
        count >= 3,
        "INV-33-03: crates/ironhermes-core/src/skills.rs must contain the \
         SkillSource::SelfCreated variant at the declaration, the \
         WARN-BUT-LOAD scan-enforcement arm, and the exhaustive variants \
         test (>= 3 occurrences). Found {count}. See Phase 33 Plan 01."
    );
}

/// INV-33-04: `validate_skill_name` is `pub` in skills.rs so the
/// `ironhermes-tools` crate can call it from `SkillManageTool`.
#[test]
fn inv_33_04_validate_skill_name_is_pub() {
    let count = SKILLS_RS_SOURCE.matches("pub fn validate_skill_name").count();
    assert!(
        count >= 1,
        "INV-33-04: crates/ironhermes-core/src/skills.rs must expose \
         validate_skill_name as `pub fn validate_skill_name` so Plan 02's \
         skill_manage tool can validate slugs cross-crate. Found {count} \
         occurrences (expected >= 1). See Phase 33 Plan 01."
    );
}

/// INV-33-05: `skill_manage` module is declared in the ironhermes-tools crate.
#[test]
fn inv_33_05_skill_manage_module_in_tools_lib() {
    let count = TOOLS_LIB_SOURCE.matches("mod skill_manage").count();
    assert!(
        count >= 1,
        "INV-33-05: crates/ironhermes-tools/src/lib.rs must declare \
         `pub mod skill_manage;` so the SkillManageTool type is reachable \
         from ToolRegistry::register_skill_manage_tool. Found {count} \
         occurrences (expected >= 1). See Phase 33 Plan 02."
    );
}

/// INV-33-06: "learning" appears in toolset_cmd.rs (KNOWN_TOOLSETS entry +
/// members_map insert). The ≥2 floor catches partial-revert refactors.
#[test]
fn inv_33_06_learning_in_known_toolsets() {
    let count = TOOLSET_CMD_SOURCE.matches("\"learning\"").count();
    assert!(
        count >= 2,
        "INV-33-06: crates/ironhermes-cli/src/toolset_cmd.rs must contain \
         \"learning\" in both KNOWN_TOOLSETS and toolset_members_map \
         (>= 2 string-literal occurrences). Found {count}. Without both \
         entries, `hermes toolset enable learning` fails with \
         'unknown toolset'. See Phase 33 Plan 03."
    );
}

const WEB_STATE_SOURCE: &str = include_str!("../../iron_hermes_ui/src/server/state.rs");
const AGENT_RUNTIME_SOURCE: &str = include_str!("../src/agent_runtime.rs");

/// INV-33-07: web UI sessions wire the full skill_manage toolset through the shared
/// runtime bundle.
///
/// Phase 28.1-03: AppState no longer calls build_app_runtime_bundle directly — it
/// constructs the shared runtime via AgentRuntime::from_config, which builds the
/// AppRuntimeBundle internally. The guarantee (web sessions get the full bundle, and
/// thus the Learning Loop skill_manage toolset) is preserved across the two files.
#[test]
fn inv_33_07_appstate_calls_build_app_runtime_bundle() {
    assert!(
        WEB_STATE_SOURCE.matches("AgentRuntime::from_config").count() >= 1,
        "INV-33-07: crates/iron_hermes_ui/src/server/state.rs must build the shared \
         runtime via AgentRuntime::from_config so web UI sessions wire the full \
         AppRuntimeBundle (incl. the Learning Loop skill_manage toolset). \
         See Phase 33 Plan 03 / Phase 34 D-05 / Phase 28.1-03."
    );
    assert!(
        AGENT_RUNTIME_SOURCE.matches("build_app_runtime_bundle").count() >= 1,
        "INV-33-07: AgentRuntime::from_config must call build_app_runtime_bundle so the \
         shared runtime used by the web UI assembles the full AppRuntimeBundle."
    );
}

/// INV-33-08: `spawn_nudge_review` and `nudge_turns` are both present in
/// iron_hermes_ui/src/server/state.rs, locking the web nudge fire site
/// shipped by Phase 32 Plan 03 (Success Criterion 1).
#[test]
fn inv_33_08_web_nudge_fire_site_exists() {
    assert!(
        WEB_STATE_SOURCE.contains("spawn_nudge_review"),
        "INV-33-08: crates/iron_hermes_ui/src/server/state.rs must call \
         spawn_nudge_review from run_web_turn so web sessions trigger the \
         Learning Loop memory nudge. See Phase 32 Plan 03 / Phase 34 Success Criterion 1."
    );
    assert!(
        WEB_STATE_SOURCE.contains("nudge_turns"),
        "INV-33-08: crates/iron_hermes_ui/src/server/state.rs must maintain \
         nudge_turns counter for per-session nudge interval tracking. \
         See Phase 32 Plan 03 / Phase 34 Success Criterion 1."
    );
}
