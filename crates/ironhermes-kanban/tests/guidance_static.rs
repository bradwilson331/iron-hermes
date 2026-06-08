//! Phase 36.3.7 Plan 05 static invariants for `KANBAN_GUIDANCE`.
//!
//! These tests assert the cache-stability and content invariants for the
//! `KANBAN_GUIDANCE` constant.  They follow the `include_str!` pattern from
//! `invariants_27_1_4_1.rs` — no extra dev-deps required.

use ironhermes_kanban::KANBAN_GUIDANCE;

/// Compile-time check: KANBAN_GUIDANCE is assignable to a `const &str`,
/// proving it requires no runtime computation.
#[test]
fn guidance_is_static_str() {
    const _: &str = KANBAN_GUIDANCE;
    // If this compiles, the invariant holds.
}

/// The three worker-critical tool names must appear in the guidance.
/// `kanban_show` (orient), `kanban_complete` (success terminator),
/// and `kanban_block` (block terminator) are the mandatory lifecycle calls.
#[test]
fn guidance_mentions_three_critical_tools() {
    assert!(
        KANBAN_GUIDANCE.contains("kanban_show"),
        "KANBAN_GUIDANCE must mention kanban_show (ORIENT step)"
    );
    assert!(
        KANBAN_GUIDANCE.contains("kanban_complete"),
        "KANBAN_GUIDANCE must mention kanban_complete (COMPLETE step)"
    );
    assert!(
        KANBAN_GUIDANCE.contains("kanban_block"),
        "KANBAN_GUIDANCE must mention kanban_block (BLOCK step)"
    );
}

/// Six lifecycle step keywords must appear (case-insensitive check via
/// lower-casing the constant).
#[test]
fn guidance_mentions_six_lifecycle_steps() {
    let lower = KANBAN_GUIDANCE.to_lowercase();
    let keywords = [
        "orient",
        "work",
        "heartbeat",
        "block",
        "complete",
        "terminate",
    ];
    for kw in &keywords {
        assert!(
            lower.contains(kw),
            "KANBAN_GUIDANCE must mention lifecycle keyword '{kw}'"
        );
    }
}

/// All four anti-pattern rules must appear, keyed by distinctive substrings
/// from the plan spec (`delegate_task`, `outside`, `follow-up`,
/// "complete a task you d…").
#[test]
fn guidance_mentions_anti_patterns() {
    assert!(
        KANBAN_GUIDANCE.contains("delegate_task"),
        "KANBAN_GUIDANCE must include the delegate_task anti-pattern rule"
    );
    assert!(
        KANBAN_GUIDANCE.contains("outside"),
        "KANBAN_GUIDANCE must include the 'files outside workspace' anti-pattern rule"
    );
    assert!(
        KANBAN_GUIDANCE.contains("follow-up"),
        "KANBAN_GUIDANCE must include the 'follow-up to self' anti-pattern rule"
    );
    // "Never complete a task you did not actually finish."
    assert!(
        KANBAN_GUIDANCE.contains("complete a task you did not"),
        "KANBAN_GUIDANCE must include the 'complete a task you did not finish' anti-pattern rule"
    );
}

/// Length must be ≥ 800 and ≤ 4096 bytes (cache-friendly, non-trivial).
#[test]
fn guidance_length_bounded() {
    let len = KANBAN_GUIDANCE.len();
    assert!(
        len >= 800,
        "KANBAN_GUIDANCE.len() = {len} is too short (< 800); add missing content"
    );
    assert!(
        len <= 4096,
        "KANBAN_GUIDANCE.len() = {len} exceeds 4096; trim to stay cache-friendly"
    );
}
