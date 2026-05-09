//! Phase 21.8.2 Plan 02: static invariant enforcing the with_skill_registry
//! wiring in the gateway CommandContext chain. Plan 03 builds on this.

#[test]
fn with_skill_registry_present_in_gateway_handler() {
    let src = include_str!("../src/handler.rs");
    assert!(
        src.contains(".with_skill_registry("),
        "Phase 21.8.2 Plan 02: handler.rs handle_slash_command must call .with_skill_registry() in CommandContext chain"
    );
}
