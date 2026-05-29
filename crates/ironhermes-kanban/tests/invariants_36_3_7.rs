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

// Plan 02 landed cas.rs — use the real source file.
const CAS_SOURCE_PLACEHOLDER: &str = include_str!("../src/cas.rs");

// Plan 03 landed dispatcher.rs and worker_spawn.rs — use the real source files.
const DISPATCHER_SOURCE: &str = include_str!("../src/dispatcher.rs");
const WORKER_SPAWN_SOURCE: &str = include_str!("../src/worker_spawn.rs");

// Plan 06 landed: `kanban` is now in `is_bypass()`.
const RUNNING_AGENT_SOURCE: &str = include_str!("../../ironhermes-core/src/commands/running_agent.rs");

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

/// INV-36.3.7-06: dispatcher must contain all 8 step helper functions
/// from D-10 — detect_crashed, extend claims, reclaim stale, max-runtime,
/// promote ready, claim_and_spawn, respawn_guard_reason, circuit breaker.
#[test]
fn dispatcher_has_all_eight_step_helpers() {
    for helper in &[
        "detect_crashed_workers",
        "extend_live_pid_claims",
        "reclaim_stale_claims",
        "enforce_max_runtime",
        "promote_ready",
        "claim_and_spawn",
        "respawn_guard_reason",
        "apply_circuit_breaker",
    ] {
        assert!(
            DISPATCHER_SOURCE.contains(helper),
            "INV-36.3.7-06: dispatcher.rs must contain helper function '{helper}' \
             (D-10 8-step tick loop)"
        );
    }
}

/// INV-36.3.7-02: worker spawn must call `build_kanban_worker_env` before
/// `exec` — env scrub policy (D-18).
#[test]
fn dispatcher_calls_build_kanban_worker_env() {
    assert!(
        WORKER_SPAWN_SOURCE.contains("build_kanban_worker_env"),
        "INV-36.3.7-02: worker spawn must call build_kanban_worker_env before \
         exec (env scrub policy D-18). Inheriting the dispatcher's env leaks \
         shell secrets into the worker."
    );
}

/// INV-36.3.7-05: worker spawn must call `env_clear` so only the
/// allowlist-built env reaches the subprocess (D-18 / Pitfall env-leak).
#[test]
fn worker_spawn_calls_env_clear() {
    assert!(
        WORKER_SPAWN_SOURCE.contains("env_clear"),
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
fn kanban_is_in_bypass_list() {
    assert!(
        RUNNING_AGENT_SOURCE.contains("\"kanban\""),
        "INV-36.3.7-03: 'kanban' must appear in is_bypass() (D-36 mid-run \
         safety). The /kanban slash command must bypass the running-agent \
         guard for all subcommands."
    );
}

// ---------------------------------------------------------------------------
// Plan 08 invariant (gateway runner embed)
// ---------------------------------------------------------------------------

/// Source constant for the gateway runner (plan 08 embed verification).
const GATEWAY_RUNNER_SOURCE: &str =
    include_str!("../../ironhermes-gateway/src/runner.rs");

/// INV-36.3.7-07: `ironhermes-gateway/src/runner.rs` must call
/// `run_dispatch_loop` (D-09 gateway-embedded dispatcher). Without this call
/// the gateway-embedded kanban dispatcher is not wired into the runtime.
#[test]
fn gateway_runner_embeds_kanban_dispatcher() {
    assert!(
        GATEWAY_RUNNER_SOURCE.contains("run_dispatch_loop"),
        "INV-36.3.7-07: runner.rs must call ironhermes_kanban::run_dispatch_loop \
         (D-09 gateway-embedded by default). If this fails, kanban dispatching \
         does not run inside the gateway process."
    );
}
