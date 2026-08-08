//! Phase 21.8.2 Plan 02: static invariant enforcing the with_skill_registry
//! wiring in the gateway CommandContext chain. Plan 03 builds on this.

#[test]
fn with_skill_registry_present_in_gateway_handler() {
    let src = include_str!("../src/handler.rs");
    // Phase 41.3 Plan 04 (D-11/D-12): handle_slash_command no longer calls
    // `.with_skill_registry(...)` directly — it populates
    // `CoreContextHandles.skill_registry` from a snapshot read of
    // `self.skill_registry`, and the shared `build_core_context` factory
    // (ironhermes-core) applies the builder internally.
    assert!(
        src.contains("skill_registry: skill_registry_snapshot"),
        "Phase 21.8.2 Plan 02 (revised Phase 41.3 Plan 04): handler.rs handle_slash_command \
         must populate CoreContextHandles.skill_registry from its skill_registry_snapshot \
         read via the D-11 shared factory"
    );
}
