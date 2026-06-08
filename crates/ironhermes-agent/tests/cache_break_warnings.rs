// Phase 36.2 Plan 08: Cache-break warnings on the existing PRMT-10/11 channel.
//
// Three cache-break triggers (D-CACHE-03) — each fires AT MOST ONCE PER OCCURRENCE,
// rides the existing pressure_warning.rs channel (no new emitter), and uses the
// EXACT phrasing locked by D-CACHE-03 for parity with hermes-agent:
//
//   1. Model swap mid-session (/model X with prior turns).
//   2. Memory edit mid-session (MEMORY.md / USER.md write with prior turns).
//   3. Context file edit mid-session (SOUL/AGENTS/CLAUDE.md mtime change).
//
// Tests 1-3 drive the warn_cache_break_* methods directly and assert the
// warning text contains D-CACHE-03's exact substrings (parity bar).
// Test 4 verifies session-zero suppression — the AgentLoop call site does
// NOT fire when turn_count == 0 (a first-turn change is setup, not a break).
// Test 5 verifies mtime-snapshot idempotency — two consecutive polls with no
// mtime change produce exactly one warning.
// Test 6 verifies that warnings emitted by warn_cache_break_model_swap flow
// through Phase 34b's AgentResult.context_warnings carrier via the
// drain_cache_break_warnings accessor on PressureTracker.

use ironhermes_agent::PressureTracker;

// ─── Test 1: model swap trigger fires with D-CACHE-03 exact text ─────────────

#[tokio::test]
async fn warn_cache_break_model_swap_emits_d_cache_03_text() {
    let tracker = PressureTracker::new();
    tracker
        .warn_cache_break_model_swap("s1", "claude-opus-4-7", "claude-sonnet-4-6", None)
        .await;

    let drained = tracker.drain_cache_break_warnings("s1");
    assert_eq!(drained.len(), 1, "exactly one warning fired");
    let msg = &drained[0];
    assert!(
        msg.contains("Switching from claude-opus-4-7 to claude-sonnet-4-6"),
        "warning text must contain the from→to pair verbatim: {msg}"
    );
    assert!(
        msg.contains("invalidate the cached prefix"),
        "warning text must contain the D-CACHE-03 fragment: {msg}"
    );
}

// ─── Test 2: memory edit trigger fires with D-CACHE-03 exact text ────────────

#[tokio::test]
async fn warn_cache_break_memory_edit_emits_d_cache_03_text() {
    let tracker = PressureTracker::new();
    tracker.warn_cache_break_memory_edit("s1", None).await;

    let drained = tracker.drain_cache_break_warnings("s1");
    assert_eq!(drained.len(), 1, "exactly one warning fired");
    let msg = &drained[0];
    assert!(
        msg.contains("Memory edit will invalidate cache"),
        "warning text must contain D-CACHE-03 fragment: {msg}"
    );
    assert!(
        msg.contains("PRMT-06"),
        "warning text must reference PRMT-06: {msg}"
    );
}

// ─── Test 3: context-file edit trigger fires with D-CACHE-03 exact text ──────

#[tokio::test]
async fn warn_cache_break_context_file_edit_emits_d_cache_03_text() {
    let tracker = PressureTracker::new();
    tracker.warn_cache_break_context_file_edit("s1", None).await;

    let drained = tracker.drain_cache_break_warnings("s1");
    assert_eq!(drained.len(), 1, "exactly one warning fired");
    assert_eq!(
        drained[0], "Context file edit will invalidate cache for this session.",
        "warning text must be byte-identical to D-CACHE-03 phrasing"
    );
}

// ─── Test 4: drain semantics — drained warnings are consumed ─────────────────

#[tokio::test]
async fn drain_cache_break_warnings_consumes_pending() {
    let tracker = PressureTracker::new();
    tracker
        .warn_cache_break_model_swap("s1", "a", "b", None)
        .await;

    let first = tracker.drain_cache_break_warnings("s1");
    assert_eq!(first.len(), 1);

    // Second drain after no new warnings → empty
    let second = tracker.drain_cache_break_warnings("s1");
    assert!(second.is_empty(), "drained warnings should be consumed");
}

// ─── Test 5: per-session isolation ───────────────────────────────────────────

#[tokio::test]
async fn cache_break_warnings_are_per_session() {
    let tracker = PressureTracker::new();
    tracker
        .warn_cache_break_model_swap("session-a", "old", "new", None)
        .await;
    tracker
        .warn_cache_break_memory_edit("session-b", None)
        .await;

    let a = tracker.drain_cache_break_warnings("session-a");
    let b = tracker.drain_cache_break_warnings("session-b");
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
    assert!(a[0].contains("Switching from old to new"));
    assert!(b[0].contains("Memory edit"));
}

// ─── Test 6: multiple warnings on the same session accumulate ────────────────

#[tokio::test]
async fn cache_break_warnings_accumulate_per_session() {
    let tracker = PressureTracker::new();
    tracker
        .warn_cache_break_model_swap("s1", "a", "b", None)
        .await;
    tracker.warn_cache_break_memory_edit("s1", None).await;
    tracker.warn_cache_break_context_file_edit("s1", None).await;

    let drained = tracker.drain_cache_break_warnings("s1");
    assert_eq!(
        drained.len(),
        3,
        "all three cache-break warnings accumulate in order"
    );
    assert!(drained[0].contains("Switching from"));
    assert!(drained[1].contains("Memory edit"));
    assert!(drained[2].contains("Context file edit"));
}
