//! Phase 36.3.7 static-grep invariants.
//!
//! Protocol-correctness properties that must survive refactors. Tests for
//! source files that do not yet exist (plan 02's `cas.rs`, plan 03's
//! `worker_spawn.rs`/`dispatcher.rs`, plan 06's `running_agent.rs` edit)
//! are `#[ignore]`-gated with a `// PLAN 0X unblocks this` comment.
//!
//! Plan 09 removes all `#[ignore]` attributes once the source files exist
//! and the assertions hold.
//!
//! Note on `include_str!`: the macro requires a real file at the path. To
//! let this test file compile in Wave 0 (before plans 02/03/06 land), the
//! placeholder constants below point at `src/error.rs` — an existing file
//! that trivially fails the downstream `.contains("…")` check. The
//! `#[ignore]` attribute keeps the failing assertion off the CI green
//! path; the un-ignored body still runs when an operator opts in with
//! `cargo test -- --ignored`, surfacing the placeholder mismatch as the
//! "this plan has not yet replaced the include_str! path" signal.
//!
//! Why static-grep over runtime tests: protocol invariants like "atomic
//! claim uses `TransactionBehavior::Immediate`" cannot be exercised
//! reliably in CI without flaky concurrency setups. A static-grep
//! regression gate prevents accidental removal of the literal string
//! during refactors and is precedent in the codebase
//! (`invariants_27_1_4_1.rs`).

// ---------------------------------------------------------------------------
// Source constants
// ---------------------------------------------------------------------------

// PLAN 02 will swap these placeholders for `include_str!("../src/cas.rs")`
// and the actual cas.rs source once it lands.
const CAS_SOURCE_PLACEHOLDER: &str = include_str!("../src/error.rs");

// PLAN 03 will swap these for `include_str!("../src/dispatcher.rs")` and
// `include_str!("../src/worker_spawn.rs")` and add at least one test that
// exercises DISPATCHER_SOURCE_PLACEHOLDER. Allow the unused constant in
// the interim — removing it would force plan 03 to re-add it.
#[allow(dead_code)]
const DISPATCHER_SOURCE_PLACEHOLDER: &str = include_str!("../src/error.rs");
const WORKER_SPAWN_SOURCE_PLACEHOLDER: &str = include_str!("../src/error.rs");

// PLAN 06 will swap this for
// `include_str!("../../ironhermes-core/src/commands/running_agent.rs")`
// once `kanban` is added to `is_bypass()`.
const RUNNING_AGENT_SOURCE_PLACEHOLDER: &str = include_str!("../src/error.rs");

// PID-liveness lives in this crate already (plan 01 Task 0 + Task 2). The
// invariant is non-ignored because the file exists.
const PID_SOURCE: &str = include_str!("../src/pid.rs");

// ---------------------------------------------------------------------------
// Active tests
// ---------------------------------------------------------------------------

/// Sanity: the test module loads and links — Wave 0 readiness gate from
/// VALIDATION.md. Without this, the orchestrator can't tell whether the
/// invariants file exists at all vs. silently failed to compile.
#[test]
fn invariants_module_loads() {
    // Intentional trivial assert. The act of compiling + running this
    // file is the gate.
    assert!(true);
}

/// INV-PID-LIVENESS: `is_pid_alive` uses the Errno-discriminating
/// `nix::sys::signal::kill` form approved at Task 0 — EPERM must map to
/// "alive" rather than the naive `is_ok()` check that would report
/// "not alive" for foreign-uid live processes.
///
/// (No `[ignore]` — the file exists and the literal must already hold
/// from Task 2.)
#[test]
fn pid_liveness_handles_eperm() {
    assert!(
        PID_SOURCE.contains("Errno::EPERM"),
        "INV-PID-LIVENESS: is_pid_alive must discriminate Errno::EPERM as \
         alive (process exists but owned by another uid). Naive \
         `kill(...).is_ok()` would report 'dead' for foreign-uid live \
         processes — Task 0 human decision."
    );
}

// ---------------------------------------------------------------------------
// Plan 02 invariants (cas.rs)
// ---------------------------------------------------------------------------

/// INV-36.3.7-01: atomic CAS claim must use
/// `TransactionBehavior::Immediate`. Default DEFERRED would let two
/// dispatcher instances both read `status='ready'` before either locks
/// and both succeed the UPDATE (Pitfall 1 / D-40).
#[test]
#[ignore = "PLAN 02 unblocks this — cas.rs does not yet exist"]
fn atomic_claim_uses_begin_immediate() {
    assert!(
        CAS_SOURCE_PLACEHOLDER.contains("Immediate"),
        "INV-36.3.7-01: atomic_claim must use TransactionBehavior::Immediate \
         (DEFERRED allows claim races). See Pitfall 1 in RESEARCH.md."
    );
}

/// INV-36.3.7-04: dispatcher inserts the `task_runs` row in the same
/// transaction as the CAS UPDATE so pointer + run row are coherent by
/// construction (D-40).
#[test]
#[ignore = "PLAN 02 unblocks this — cas.rs does not yet exist"]
fn cas_inserts_task_run_in_same_transaction() {
    assert!(
        CAS_SOURCE_PLACEHOLDER.contains("task_runs"),
        "INV-36.3.7-04: atomic_claim must INSERT INTO task_runs in the same \
         transaction as the CAS UPDATE (D-40)."
    );
}

// ---------------------------------------------------------------------------
// Plan 03 invariants (dispatcher.rs / worker_spawn.rs)
// ---------------------------------------------------------------------------

/// INV-36.3.7-02: worker spawn must call `build_kanban_worker_env` before
/// `exec` — env scrub policy (D-18).
#[test]
#[ignore = "PLAN 03 unblocks this — worker_spawn.rs does not yet exist"]
fn dispatcher_calls_build_kanban_worker_env() {
    assert!(
        WORKER_SPAWN_SOURCE_PLACEHOLDER.contains("build_kanban_worker_env"),
        "INV-36.3.7-02: worker spawn must call build_kanban_worker_env before \
         exec (env scrub policy D-18). Inheriting the dispatcher's env leaks \
         shell secrets into the worker."
    );
}

/// INV-36.3.7-05: worker spawn must call `env_clear` so only the
/// allowlist-built env reaches the subprocess (D-18 / Pitfall env-leak).
#[test]
#[ignore = "PLAN 03 unblocks this — worker_spawn.rs does not yet exist"]
fn worker_spawn_calls_env_clear() {
    assert!(
        WORKER_SPAWN_SOURCE_PLACEHOLDER.contains("env_clear"),
        "INV-36.3.7-05: tokio::process::Command must call env_clear() before \
         envs(build_kanban_worker_env(…)) so only the allowlist reaches the \
         worker (D-18)."
    );
}

// ---------------------------------------------------------------------------
// Plan 06 invariants (ironhermes-core running_agent.rs edit)
// ---------------------------------------------------------------------------

/// INV-36.3.7-03: `"kanban"` must appear in `is_bypass()` so the
/// `/kanban` slash command bypasses the running-agent guard (D-36).
#[test]
#[ignore = "PLAN 06 unblocks this — is_bypass() does not yet contain \"kanban\""]
fn kanban_is_in_bypass_list() {
    assert!(
        RUNNING_AGENT_SOURCE_PLACEHOLDER.contains("\"kanban\""),
        "INV-36.3.7-03: 'kanban' must appear in is_bypass() (D-36 mid-run \
         safety). The /kanban slash command must bypass the running-agent \
         guard for all subcommands."
    );
}
