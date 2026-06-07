//! Phase 36.3.7.5 Plan 03 (BUG-36.3.7.5-04 + receiver-end portion of
//! BUG-36.3.7.5-07).
//!
//! Receiver-end tests for the gateway notifier spawn-gating helper.
//!
//! These tests cover `compute_notifier_gate` in isolation — the pure-function
//! gating logic that decides whether the polling loop spawns at gateway
//! startup. Default-off semantics, case-insensitive platform matching, and
//! multi-source intersection are all locked in CONTEXT.md (D-gateway-gating)
//! and verified here. Per LEARNINGS 2026-05-29, the producer (runner.rs spawn
//! block + notifier_gating.rs helper) and the consumer (these tests) ship in
//! the same commit set.
//!
//! Plan 04's lifecycle test will exercise the full path end-to-end
//! (auto-subscribe → complete → notifier delivers → sub removed).

use ironhermes_gateway::notifier_gating::{NotifierGate, compute_notifier_gate};

// ---------------------------------------------------------------------------
// Test 1: default-off semantics (BUG-36.3.7.5-04 — gate fails closed)
// ---------------------------------------------------------------------------

/// Locks CONTEXT.md D-gateway-gating default-off: both `None` and `Some(empty)`
/// MUST collapse to `DisabledNoSources`. The gateway runner relies on this to
/// skip the notifier spawn entirely when the operator has not opted in.
#[test]
fn gate_returns_disabled_no_sources_when_none() {
    // Case A: notification_sources is None (fresh install — never written).
    let gate_a = compute_notifier_gate(None, &["telegram".to_string()]);
    assert_eq!(
        gate_a,
        NotifierGate::DisabledNoSources,
        "BUG-36.3.7.5-04: notification_sources=None MUST gate-fail closed \
         per CONTEXT.md D-gateway-gating (default-off semantics)."
    );

    // Case B: Some(empty) — operator typed `notification_sources: []` which
    // is semantically equivalent to "nothing opted in".
    let gate_b = compute_notifier_gate(Some(&[] as &[String]), &["telegram".to_string()]);
    assert_eq!(
        gate_b,
        NotifierGate::DisabledNoSources,
        "BUG-36.3.7.5-04: notification_sources=Some([]) MUST collapse to \
         DisabledNoSources — empty list is equivalent to no opt-in."
    );
}

// ---------------------------------------------------------------------------
// Test 2: configured platform missing from enabled set
// ---------------------------------------------------------------------------

/// Locks the wanted-vs-enabled intersection: if the operator names a platform
/// in `notification_sources` that the gateway hasn't enabled, the gate MUST
/// report `DisabledNoOverlap` so the runner logs the wanted/enabled mismatch
/// and skips the spawn. This is the operator-misconfiguration mitigation
/// (T-36.3.7.5-03-02 in PLAN's threat model).
#[test]
fn gate_returns_disabled_no_overlap_when_no_intersection() {
    // discord is wanted but telegram is the only enabled platform.
    let gate = compute_notifier_gate(
        Some(&["discord".to_string()]),
        &["telegram".to_string()],
    );
    assert_eq!(
        gate,
        NotifierGate::DisabledNoOverlap {
            wanted: vec!["discord".to_string()],
            enabled: vec!["telegram".to_string()],
        },
        "BUG-36.3.7.5-04: a wanted platform absent from enabled MUST yield \
         DisabledNoOverlap carrying both sets for the operator-facing log."
    );

    // Case-insensitive lookup MUST still reject when names truly do not match:
    // "Discord" (capitalized) still does not intersect with ["telegram"].
    let gate_case = compute_notifier_gate(
        Some(&["Discord".to_string()]),
        &["telegram".to_string()],
    );
    assert_eq!(
        gate_case,
        NotifierGate::DisabledNoOverlap {
            wanted: vec!["Discord".to_string()],
            enabled: vec!["telegram".to_string()],
        },
        "BUG-36.3.7.5-04: case-insensitive matching does NOT manufacture \
         intersection when names truly differ; Discord != telegram."
    );
}

// ---------------------------------------------------------------------------
// Test 3: enabled with intersection — case-insensitive + multi-source
// ---------------------------------------------------------------------------

/// Locks the happy-path gate-pass: when at least one configured source
/// matches an enabled platform (case-insensitive), the gate MUST return
/// `Enabled { sources }` where `sources` carries the overlap in the ORIGINAL
/// casing from `notification_sources`. The runner uses this list verbatim in
/// the "kanban notifier loop started" log line so the operator sees exactly
/// what they typed.
#[test]
fn gate_returns_enabled_with_overlap_when_intersection_exists() {
    // Case A: simple single-source match.
    let gate_a = compute_notifier_gate(
        Some(&["telegram".to_string()]),
        &["telegram".to_string(), "discord".to_string()],
    );
    assert_eq!(
        gate_a,
        NotifierGate::Enabled {
            sources: vec!["telegram".to_string()],
        },
        "BUG-36.3.7.5-04: 1 wanted ∩ 2 enabled MUST yield Enabled with the \
         single overlap."
    );

    // Case B: case-insensitive — `TELEGRAM` (caller's casing) matches
    // `telegram` (enabled set's casing). The returned `sources` preserves the
    // caller's `TELEGRAM` casing so the operator's notification_sources value
    // round-trips into the startup log.
    let gate_b = compute_notifier_gate(
        Some(&["TELEGRAM".to_string()]),
        &["telegram".to_string()],
    );
    assert_eq!(
        gate_b,
        NotifierGate::Enabled {
            sources: vec!["TELEGRAM".to_string()],
        },
        "BUG-36.3.7.5-04: case-insensitive matching MUST preserve the \
         caller's original casing in the returned overlap (operator sees \
         what they wrote in startup logs)."
    );

    // Case C: multi-source — only the overlap survives. Wanted has telegram
    // AND discord; enabled has telegram AND slack. Only telegram intersects.
    let gate_c = compute_notifier_gate(
        Some(&["telegram".to_string(), "discord".to_string()]),
        &["telegram".to_string(), "slack".to_string()],
    );
    assert_eq!(
        gate_c,
        NotifierGate::Enabled {
            sources: vec!["telegram".to_string()],
        },
        "BUG-36.3.7.5-04: multi-source intersection MUST drop the \
         non-overlapping `discord` entry and return only the surviving overlap."
    );

    // Case D: full-overlap multi-source — both wanted platforms enabled.
    let gate_d = compute_notifier_gate(
        Some(&["telegram".to_string(), "discord".to_string()]),
        &[
            "telegram".to_string(),
            "discord".to_string(),
            "slack".to_string(),
        ],
    );
    assert_eq!(
        gate_d,
        NotifierGate::Enabled {
            sources: vec!["telegram".to_string(), "discord".to_string()],
        },
        "BUG-36.3.7.5-04: full overlap MUST preserve insertion order of the \
         caller's notification_sources list."
    );
}
