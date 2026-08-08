//! WR-01 regression (Phase 36.3.12 Plan 10): the CLI/TUI `[s]ession` approval tier
//! must survive across multiple dispatches within the SAME process, and must NOT
//! be confused with the `[a]lways` tier's disk-persisted semantics.
//!
//! Before this plan, `build_gated_terminal_intercept` / `build_gated_execute_code_intercept`
//! called `ApprovalsStore::load()` fresh INSIDE the per-dispatch closure body. Because
//! `load()` always constructs a brand-new, empty `session: Arc<Mutex<HashSet<String>>>`
//! (`ApprovalsStore::load_from_path`), a session-tier grant recorded on dispatch N was
//! silently invisible on dispatch N+1 — choosing `[s]ession` behaved identically to
//! `[o]nce`. The fix hoists ONE `ApprovalsStore::load()` to a process-lifetime scope and
//! shares it (via `Arc`) across every dispatch.
//!
//! This test proves the underlying store semantics that the fix relies on, using the
//! SAME API the `'s'`/`'a'` prompt arms use (`approve_session` / `approve_always` in
//! `crates/ironhermes-cli/src/approval_gate.rs`'s `prompt_for_approval_with_reader`):
//!
//! - `session_tier_grant_persists_across_dispatches`: a grant recorded on a store is
//!   visible from a SECOND lookup against the SAME store instance (this is what sharing
//!   one `Arc<ApprovalsStore>` across dispatches buys you) — and, as the control proving
//!   the pre-fix defect was real, is NOT visible from a freshly `load_from_path`'d store
//!   at the same path (a per-dispatch reload starts a new empty `session` set).
//! - `always_tier_grant_survives_a_fresh_store_load`: the `[a]lways` tier is the OPPOSITE
//!   — it is disk-persisted, so it IS visible from a freshly loaded store. This is the
//!   control case proving the test distinguishes the two tiers rather than asserting a
//!   tautology that would pass even if both tiers behaved identically.

use ironhermes_core::{ApprovalsStore, KeyKind};

/// A session-tier grant recorded on one `ApprovalsStore` is visible from a second
/// lookup against that SAME instance, but is NOT visible from a freshly
/// `load_from_path`'d store at the same on-disk path — proving (a) sharing one store
/// across dispatches is what makes `[s]ession` persist, and (b) the pre-fix bug (a
/// fresh `ApprovalsStore::load()` per dispatch) really did lose the grant.
#[tokio::test]
async fn session_tier_grant_persists_across_dispatches() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("approvals.json");

    let cache_key = ApprovalsStore::normalize_command("ls -la /tmp");

    // ── Same store, two lookups (mirrors two dispatches sharing one Arc) ──────
    let store = ApprovalsStore::with_path(path.clone());
    assert!(
        !store.is_session_approved(&cache_key).await,
        "sanity: no grant recorded yet"
    );

    // The same API the `'s'` prompt arm calls (approval_gate.rs's
    // prompt_for_approval_with_reader, 's' | 'S' arm).
    store.approve_session(&cache_key).await;

    assert!(
        store.is_session_approved(&cache_key).await,
        "a session-tier grant must be visible on a SECOND lookup against the SAME store — \
         this is the persistence a shared Arc<ApprovalsStore> across dispatches relies on"
    );

    // ── Control: a fresh load at the same path does NOT see the grant ─────────
    // This demonstrates the pre-fix defect: `ApprovalsStore::load()` /
    // `load_from_path()` always constructs a brand-new, empty `session` set
    // (never restored from disk — session approvals are D-01 in-memory-only).
    // A per-dispatch reload is exactly this: a fresh, empty session set every time.
    let reloaded = ApprovalsStore::load_from_path(path).await;
    assert!(
        !reloaded.is_session_approved(&cache_key).await,
        "a FRESH store load must NOT see a prior instance's session grant — proving \
         that reloading per dispatch (the pre-fix bug) silently loses the [s]ession tier"
    );
}

/// Control case: an `[a]lways`-tier grant is the OPPOSITE of the session tier — it is
/// persisted to disk via `save_to_disk`, so it IS visible from a freshly loaded store
/// at the same path. Pins the tier boundary so `session_tier_grant_persists_across_dispatches`
/// is proven to distinguish the two tiers, not merely assert something trivially true of
/// any store.
#[tokio::test]
async fn always_tier_grant_survives_a_fresh_store_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("approvals.json");

    let cache_key = ApprovalsStore::normalize_command("curl https://example.com");

    // The same API the `'a'` prompt arm calls (approval_gate.rs's
    // prompt_for_approval_with_reader, 'a' | 'A' arm).
    let store = ApprovalsStore::with_path(path.clone());
    store.approve_always(&cache_key, KeyKind::Command).await;
    store
        .save_to_disk()
        .await
        .expect("save_to_disk must succeed for a fresh tempdir path");

    // A brand-new store, loaded fresh from the same path — the always-tier grant
    // round-trips through disk and IS visible, unlike the session tier above.
    let reloaded = ApprovalsStore::load_from_path(path).await;
    assert!(
        reloaded
            .is_always_approved(&cache_key, KeyKind::Command)
            .await,
        "an [a]lways-tier grant must survive a fresh store load — it is persisted to disk, \
         unlike the in-memory-only [s]ession tier exercised above"
    );

    // And the reloaded store's (freshly-constructed, empty) session set correctly
    // has NOT inherited anything from the always tier — the two namespaces stay
    // independent (D-02), which is also what keeps the WR-01 fix from accidentally
    // widening the session tier's trust window into the always tier's.
    assert!(
        !reloaded.is_session_approved(&cache_key).await,
        "the always-tier grant must not leak into the session-tier lookup"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// WR-06 (Phase 36.3.12 review round 2): drive the REAL builder, not just the
// store.
//
// The two tests above prove `ApprovalsStore`'s own session/always semantics —
// a layer BELOW the WR-01 regression. Neither one ever calls
// `build_gated_terminal_intercept`, `build_gated_execute_code_intercept`, or
// `CliApprovalGate::from_shared`. If a future edit reverted WR-01's fix (going
// back to `ApprovalsStore::load()` fresh inside the closure body instead of
// using the caller's shared `Arc<ApprovalsStore>`), neither test above would
// fail — this test closes that gap by driving `build_gated_terminal_intercept`
// itself, twice, with the SAME shared `Arc<ApprovalsStore>`.
//
// Requires the `test-support` feature (`io_gate::set_force_tty_for_test`) —
// see that function's doc for why: `CliApprovalGate::request_approval`'s D-12
// non-TTY check runs BEFORE the session-cache check it guards, and a
// `cargo test` process's stdin is never a real TTY, so without the override
// the session-cache branch is unreachable through the real closure at all,
// on every dispatch, regardless of the store's state.
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(feature = "test-support")]
#[tokio::test]
async fn shared_store_session_grant_persists_across_two_real_dispatches() {
    use ironhermes_cli::approval_gate::build_gated_terminal_intercept;
    use ironhermes_cli::io_gate::set_force_tty_for_test;
    use ironhermes_core::Config;
    use std::sync::Arc;

    // RAII guard: always clear the process-global TTY override on the way out
    // (including on panic/assertion failure) so it cannot leak into any other
    // test sharing this test binary process.
    struct TtyOverrideGuard;
    impl Drop for TtyOverrideGuard {
        fn drop(&mut self) {
            set_force_tty_for_test(None);
        }
    }
    let _guard = TtyOverrideGuard;

    // Force `can_prompt(false)` (== `is_terminal_stdin()`) to report a TTY so
    // `prompt_for_approval_with_reader`'s D-12 check does not short-circuit
    // before the session-cache check. Note: neither dispatch below ever
    // reaches the INTERACTIVE-READ branch (the session grant is recorded
    // before either closure is invoked, so both calls resolve from the
    // session cache) — real stdin is never touched, so this cannot hang
    // waiting for terminal input even if a human runs this test interactively.
    set_force_tty_for_test(Some(true));

    let dir = tempfile::tempdir().expect("tempdir");
    let approvals = Arc::new(ironhermes_core::ApprovalsStore::with_path(
        dir.path().join("approvals.json"),
    ));

    // "curl ..." is a network-access pattern the built-in DANGEROUS_PATTERNS
    // classifies as Tier-1 `Warn` (approval-required, not Block) — see WR-07's
    // finding, which names this exact string as Warn-classified.
    let command = "curl https://example.com/wr-06-shared-store-check";
    let cache_key = ironhermes_core::ApprovalsStore::normalize_command(command);
    let args = serde_json::json!({ "command": command });
    let config = Arc::new(Config::default());

    // Record the session-tier grant via the gate's own `approve_session` API —
    // the exact call `prompt_for_approval_with_reader`'s `'s'`/`'S'` prompt arm
    // makes — BEFORE building or invoking either closure. This is what
    // "dispatch 1 recorded a session grant" means at the store level; the
    // point under test is whether that grant, recorded on the shared Arc,
    // is visible to closures built from that SAME Arc.
    approvals.approve_session(&cache_key).await;

    // ── Dispatch 1: build the REAL closure via the REAL builder, with the
    // shared Arc, and invoke it. ──────────────────────────────────────────
    let intercept1 = build_gated_terminal_intercept(
        None, // no `terminal` tool registered — see outcome assertion below
        config.clone(),
        "sess-wr06".to_string(),
        "cli",
        "chat-wr06".to_string(),
        false, // yolo
        approvals.clone(),
    );
    let outcome1 = intercept1(args.clone())
        .await
        .expect("the closure itself must not return an Err — GatedOutcome captures failure");
    // `tool: None` means the underlying run closure always errors with
    // "terminal tool not registered on this runtime" — `Failed(...)` therefore
    // proves the approval gate let the run closure be INVOKED (Approved), as
    // distinct from `Denied(...)` (never invoked). See `GatedOutcome`'s doc
    // (WR-03 semantics) in `ironhermes-hooks/src/gated_exec.rs`.
    assert!(
        outcome1.starts_with("Failed("),
        "dispatch 1 must resolve via the session grant (Approved -> run invoked, \
         which errors because no tool is registered) — got {outcome1}"
    );

    // ── Dispatch 2: build a SECOND, INDEPENDENT closure — a fresh call to the
    // real builder — from the SAME shared Arc<ApprovalsStore>, simulating a
    // second LLM-issued terminal call in the same process. ────────────────
    let intercept2 = build_gated_terminal_intercept(
        None,
        config,
        "sess-wr06".to_string(),
        "cli",
        "chat-wr06".to_string(),
        false,
        approvals.clone(),
    );
    let outcome2 = intercept2(args)
        .await
        .expect("the closure itself must not return an Err");

    // If WR-01's fix holds, `build_gated_terminal_intercept` threads the
    // CALLER's shared `Arc<ApprovalsStore>` into `CliApprovalGate::from_shared`
    // — never a fresh `ApprovalsStore::load()` — so the session grant recorded
    // above is visible to this SECOND, independently-built closure too.
    assert!(
        outcome2.starts_with("Failed("),
        "dispatch 2 (a SECOND, independently-built closure from the SAME shared \
         Arc<ApprovalsStore>) must resolve via the SAME session grant recorded before \
         dispatch 1 — got {outcome2}. A `Denied(...)` here means the closure did NOT \
         consult the shared store's session cache — i.e. the WR-01 regression (a fresh \
         `ApprovalsStore::load()` inside the closure body, which always starts with an \
         empty in-memory session set — D-01) has reappeared."
    );
}
