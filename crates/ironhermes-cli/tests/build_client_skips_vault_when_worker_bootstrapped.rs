//! Phase 51 WR-08 source-text invariant, re-keyed in Phase 51-13 Task 2:
//! `build_client`'s vault-fallback block (the actual kanban-worker code path,
//! reached via a worker's `chat -q "..."` spawn shape through `run_single` ->
//! `build_client`) must check `resolver.has_keyless_endpoint()` BEFORE ever
//! calling `ironhermes_vault::open_store`, so a worker whose endpoints are all
//! already keyed never opens a vault store at all — not merely declines to use
//! one after opening it.
//!
//! # Why this changed from the original F-04 guard
//!
//! The original guard (Phase 51 UAT F-04 fix, commit 2) keyed the SAME
//! optimization off `ironhermes_core::provider::worker_bootstrapped_over_socket()`
//! — "did this process bootstrap ANY provider over the socket". That was wrong
//! for the same reason `apply_vault_fallback`'s own process-global early return
//! (Phase 51-13 Task 1) was wrong: a worker that bootstrapped ONE provider can
//! still legitimately need a SECOND vault-backed provider (`roles.vision`,
//! `roles.kanban_judge`, a fallback model), and the old guard never even let
//! that worker reach the (now-correct) per-provider loop — it skipped opening
//! the store at all. The guard is now keyed on the question that actually
//! matters: are there any endpoints still without a key at all? If none, the
//! vault has nothing to fill and skipping the open is a pure optimization
//! (and keeps F-04's never-touch-the-vault-file property for the common
//! single-provider worker). If some remain, the store opens and
//! `apply_vault_fallback`'s per-provider loop decides which ones it can fill.
//!
//! # Source-text invariant, not a runtime proof
//!
//! Matching this crate's own established convention for this exact class of
//! property (`rusty_vault_feature_reaches_dispatch_gate.rs`,
//! `ironhermes-core/tests/dispatch_gate_vault_backed.rs`'s
//! `no_production_path_calls_the_dotenv_only_gate`, `invariants_41_3_credentials.rs`
//! in `ironhermes-agent`): a runtime end-to-end subprocess proof of "opened
//! zero files" would need either a live network call past the vault stage
//! (flaky, slow, an external dependency this test suite otherwise avoids) or
//! an early-exit test hook that — by construction — returns before
//! `build_client` is ever reached.
//!
//! The suppression mechanism ITSELF (`apply_vault_fallback`'s per-provider
//! `continue`) is proven at runtime, RED-then-GREEN, by
//! `ironhermes-core/tests/vault_fallback_suppressed_after_worker_bootstrap.rs`.
//! This test proves the one remaining fact that mechanism alone cannot: that
//! the REAL worker call site actually consults the keyless-endpoint predicate
//! before ever touching the vault file, so `open_store` (which reads
//! `vault.key`) is skipped entirely when there is nothing left to fill — not
//! just its result discarded.

#[test]
fn build_client_checks_keyless_endpoints_before_opening_the_vault_store() {
    let src = include_str!("../src/main.rs");

    let fn_start = src
        .find("async fn build_client(cli: &Cli)")
        .expect("build_client must exist in main.rs");
    // Bound the search to build_client's own body — up to the next top-level
    // `async fn` after it — so this can never accidentally match an unrelated
    // later call site in the same file.
    let after_fn_start = &src[fn_start..];
    let fn_end = after_fn_start[10..]
        .find("\nasync fn ")
        .map(|i| i + 10)
        .unwrap_or(after_fn_start.len());
    let body = &after_fn_start[..fn_end];

    let guard_idx = body.find("resolver.has_keyless_endpoint()").expect(
        "build_client must call resolver.has_keyless_endpoint() — the guard that decides \
         whether there is anything left for the vault to fill (Phase 51-13 WR-08 re-key)",
    );
    let open_store_idx = body
        .find("ironhermes_vault::open_store(&ironhermes_core::resolve_vault_config(&config))")
        .expect("build_client must call open_store via the shared resolve_vault_config seam");

    assert!(
        guard_idx < open_store_idx,
        "has_keyless_endpoint() must be checked BEFORE open_store is called — a worker \
         with no keyless endpoints must never open the vault store at all, not merely \
         skip using it after opening it"
    );

    // And the guard must actually gate the SAME `if` that guards open_store —
    // not just appear earlier in the function for an unrelated reason.
    assert!(
        body.contains("if config.vault.enabled && resolver.has_keyless_endpoint()"),
        "the guard must be part of the SAME condition that gates open_store, not a \
         separate, disconnected check"
    );

    // Negative assertion: the OLD process-global guard expression must be
    // gone from this function — its presence here would mean the re-key
    // (Task 2) was only partially applied, leaving the whole-process
    // optimization alongside the new one instead of replacing it.
    assert!(
        !body.contains("worker_bootstrapped_over_socket()"),
        "build_client's guard must no longer key off worker_bootstrapped_over_socket() — \
         that check regressed a worker's ability to open the store for a SECOND \
         vault-backed provider after bootstrapping a different one (T17/WR-08)"
    );
}
