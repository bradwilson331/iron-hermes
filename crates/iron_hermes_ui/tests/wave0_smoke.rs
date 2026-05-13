//! Wave-0 smoke test: asserts the integration-test binary links cleanly
//! against the iron_hermes_ui crate. Real verification commands live in
//! .planning/phases/26.2.1-new-web-ui-with-wheel-menu/26.2.1-VALIDATION.md.

#[test]
fn default_build_compiles() {
    // If this binary links and runs, the default-features build linked
    // the iron_hermes_ui lib crate. That is the assertion.
}
