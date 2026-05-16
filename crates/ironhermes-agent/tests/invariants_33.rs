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
