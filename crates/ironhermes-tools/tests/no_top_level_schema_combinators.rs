//! Receiver-end regression lock for Anthropic-compatible tool schemas.
//!
//! Phase 36.3.7.2 / BUG-IRONHERMES-TOOLS-SCHEMA-COMPAT-03.
//!
//! Anthropic's tool API rejects `input_schema` payloads that contain a
//! top-level `oneOf`, `allOf`, or `anyOf`. This test iterates every tool
//! registered via `ToolRegistry::register_defaults()` and asserts that
//! none of them emit a top-level boolean schema combinator. Failure
//! messages name the offending tool so future regressions are immediately
//! greppable.
//!
//! If a future tool legitimately needs a boolean combinator, it must
//! nest it INSIDE a property schema, not at the top level of the
//! `input_schema` value.
//!
//! Implementation path: uses `ToolRegistry::register_defaults()` (the
//! single production registration anchor) so future tools added there
//! are auto-covered without any update to this test.

use ironhermes_tools::ToolRegistry;

#[test]
fn no_default_tool_has_top_level_schema_combinator() {
    let mut registry = ToolRegistry::new();
    registry.register_defaults();

    let definitions = registry.get_definitions(None);
    assert!(
        !definitions.is_empty(),
        "register_defaults() registered zero tools — the test cannot exercise its invariant \
         (threat T-36.3.7.2-02-02: silent pass from empty registry)"
    );

    let forbidden_keys = ["oneOf", "allOf", "anyOf"];
    let mut offenders: Vec<String> = Vec::new();

    for schema in &definitions {
        let tool_name = &schema.function.name;
        let params = &schema.function.parameters;
        for key in &forbidden_keys {
            if params.get(*key).is_some() {
                offenders.push(format!(
                    "tool `{tool_name}` has top-level `{key}` in input_schema \
                     — see Phase 36.3.7.2 / BUG-IRONHERMES-TOOLS-SCHEMA-COMPAT-03"
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "BUG-IRONHERMES-TOOLS-SCHEMA-COMPAT-03: one or more tools emit a top-level boolean \
         schema combinator that Anthropic's tool API rejects at call time:\n  - {}",
        offenders.join("\n  - ")
    );
}
