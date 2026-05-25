//! Phase 36.2 Plan 03 static-grep regression gates.
//!
//! Locks the public surface of the new `error_classifier` module and the
//! facade-shape of the legacy `classify_llm_error` method in `agent_loop.rs`.
//! Follows the `include_str!` pattern from `invariants_27_1_4_1_1.rs`. No
//! dev-deps.
//!
//! Per Plan 03 D-ERR-03:
//! - `error_classifier.rs` MUST contain the typed `ProviderError` enum, the
//!   `classify_llm_error_typed` function, the `From<ProviderError> for (bool, bool)`
//!   facade impl, and the `variant_name(&self) -> &'static str` method.
//! - `agent_loop.rs` MUST still contain the legacy helpers `extract_http_status`
//!   and `is_transport_failure` (the `invariants_27_1_4_1_1.rs` precondition is
//!   preserved by leaving them in place and only widening visibility to
//!   `pub(crate)`).
//! - `agent_loop.rs` MUST mention `classify_llm_error_typed` (proves the facade
//!   delegates rather than duplicates the classifier).

const AGENT_LOOP_SOURCE: &str = include_str!("../src/agent_loop.rs");
const ERROR_CLASSIFIER_SOURCE: &str = include_str!("../src/error_classifier.rs");

#[test]
fn error_classifier_has_provider_error_enum() {
    assert!(
        ERROR_CLASSIFIER_SOURCE.contains("pub enum ProviderError"),
        "Phase 36.2-03: crates/ironhermes-agent/src/error_classifier.rs must \
         contain a public `pub enum ProviderError` (D-ERR-01)."
    );
}

#[test]
fn error_classifier_has_classify_llm_error_typed() {
    assert!(
        ERROR_CLASSIFIER_SOURCE.contains("pub fn classify_llm_error_typed("),
        "Phase 36.2-03: error_classifier.rs must expose the new canonical \
         entry point `pub fn classify_llm_error_typed(` (D-ERR-03)."
    );
}

#[test]
fn error_classifier_has_from_impl_for_tuple() {
    assert!(
        ERROR_CLASSIFIER_SOURCE.contains("impl From<ProviderError> for (bool, bool)"),
        "Phase 36.2-03: error_classifier.rs must provide \
         `impl From<ProviderError> for (bool, bool)` so the legacy facade can \
         delegate via `.into()` (D-ERR-03)."
    );
}

#[test]
fn error_classifier_has_variant_name() {
    assert!(
        ERROR_CLASSIFIER_SOURCE.contains("pub fn variant_name("),
        "Phase 36.2-03: error_classifier.rs must expose \
         `pub fn variant_name(&self) -> &'static str` to support Plan 07's \
         `usage_events.error_kind` bounded-string storage."
    );
}

#[test]
fn error_classifier_has_should_retry_and_should_fallback() {
    assert!(
        ERROR_CLASSIFIER_SOURCE.contains("pub fn should_retry("),
        "Phase 36.2-03: error_classifier.rs must expose `should_retry`."
    );
    assert!(
        ERROR_CLASSIFIER_SOURCE.contains("pub fn should_fallback("),
        "Phase 36.2-03: error_classifier.rs must expose `should_fallback`."
    );
}

#[test]
fn agent_loop_classify_llm_error_is_facade() {
    // The legacy `classify_llm_error` MUST delegate to `classify_llm_error_typed`.
    // This catches anyone who tries to re-fork the truth table.
    assert!(
        AGENT_LOOP_SOURCE.contains("classify_llm_error_typed"),
        "Phase 36.2-03: agent_loop.rs must call `classify_llm_error_typed` from \
         within `classify_llm_error` (D-ERR-03 facade pattern)."
    );
}

#[test]
fn agent_loop_preserves_is_transport_failure() {
    // Required for invariants_27_1_4_1_1.rs's `agent_loop_has_transport_failure_helper_prov07`
    // precondition. The helper stays in `agent_loop.rs`; only its visibility is widened.
    assert!(
        AGENT_LOOP_SOURCE.contains("fn is_transport_failure("),
        "Phase 36.2-03: agent_loop.rs must STILL contain \
         `fn is_transport_failure(` so the PROV-07 invariant in \
         invariants_27_1_4_1_1.rs continues to pass."
    );
}

#[test]
fn agent_loop_preserves_extract_http_status() {
    assert!(
        AGENT_LOOP_SOURCE.contains("fn extract_http_status("),
        "Phase 36.2-03: agent_loop.rs must STILL contain \
         `fn extract_http_status(` — error_classifier.rs reuses this helper \
         in-place; deleting it would silently break classification (D-ERR-03)."
    );
}
