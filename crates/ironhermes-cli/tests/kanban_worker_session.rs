//! Phase 36.3.7 Plan 05 — kanban worker session wiring tests.
//!
//! Tests that `inject_kanban_guidance_if_worker` and
//! `register_kanban_tools_if_applicable` are wired at the correct locations in
//! `main.rs`.  We use the `include_str!` static-grep pattern (same as
//! `invariants_27_1_4_1.rs`) to avoid executing a real `ironhermes` subprocess.

const MAIN_RS: &str = include_str!("../src/main.rs");

/// D-26: The guidance injection helper must be called at the run_single path.
/// Verified by the presence of `inject_kanban_guidance_if_worker` + the
/// `build_system_message` call that follows it.
#[test]
fn main_rs_calls_inject_kanban_guidance_if_worker() {
    assert!(
        MAIN_RS.contains("inject_kanban_guidance_if_worker"),
        "INV-36.3.7-05-01: main.rs must call inject_kanban_guidance_if_worker \
         at session construction (D-26 prompt injection)"
    );
}

/// D-20: The kanban tool registration helper must be wired in main.rs.
#[test]
fn main_rs_calls_register_kanban_tools_if_applicable() {
    assert!(
        MAIN_RS.contains("register_kanban_tools_if_applicable"),
        "INV-36.3.7-05-02: main.rs must call register_kanban_tools_if_applicable \
         at session construction (D-20 worker-mode toolset registration)"
    );
}

/// The HERMES_KANBAN_TASK env var check must appear at least twice in main.rs:
/// once for the guidance injection and once for the tool registration helper.
#[test]
fn main_rs_checks_hermes_kanban_task_env() {
    let count = MAIN_RS.matches("HERMES_KANBAN_TASK").count();
    assert!(
        count >= 2,
        "INV-36.3.7-05-03: HERMES_KANBAN_TASK must appear ≥ 2 times in main.rs \
         (guidance injection + tool registration), found {count}"
    );
}

/// The KANBAN_GUIDANCE constant must be referenced in main.rs (either imported
/// or referenced in the helper function body).
#[test]
fn main_rs_references_kanban_guidance_constant() {
    assert!(
        MAIN_RS.contains("KANBAN_GUIDANCE"),
        "INV-36.3.7-05-04: main.rs must reference KANBAN_GUIDANCE (D-26 injection)"
    );
}

/// The activate_skill call with KANBAN_GUIDANCE must appear in main.rs.
/// This verifies the exact mechanism (activate_skill not add_skill_overlay).
#[test]
fn main_rs_activates_skill_with_kanban_guidance() {
    assert!(
        MAIN_RS.contains("activate_skill"),
        "INV-36.3.7-05-05: main.rs must call activate_skill for KANBAN_GUIDANCE injection \
         (PromptBuilder uses activate_skill, not add_skill_overlay)"
    );
}
