//! Phase 41.3 Plan 11 (D-19) — source-text invariants locking the credential
//! resolution seam inside `build_app_runtime_bundle`.
//!
//! These are source-text invariant tests: they assert structural properties
//! of the factory file rather than runtime behaviour. A behavioral test would
//! require standing up a failing vault inside a full runtime bundle; the
//! property under test — "the sealed-vault error is not swallowed" — is a
//! source-shape property, not a runtime one. Follows the existing
//! `invariants_*.rs` convention in this crate (see `invariants_36_17_5.rs`).

/// D-19: credentials must resolve BEFORE any tool is registered — the
/// resolution call must appear, in source order, before the
/// `build_registry_with_process_registry` call it feeds.
#[test]
fn factory_resolves_credentials_before_registering_tools() {
    const SRC: &str = include_str!("../src/app_runtime_factory.rs");

    let resolve_idx = SRC
        .find("ironhermes_tools::credentials::ToolCredentials::resolve(")
        .expect("build_app_runtime_bundle must call ToolCredentials::resolve");
    let registry_build_idx = SRC
        .find("let mut registry = build_registry_with_process_registry(")
        .expect("build_app_runtime_bundle must call build_registry_with_process_registry");

    assert!(
        resolve_idx < registry_build_idx,
        "ToolCredentials::resolve must appear before build_registry_with_process_registry \
         in source order — credentials must be resolved before any tool is registered (D-19)"
    );
}

/// D-19: a sealed/locked/corrupt vault must surface as a loud startup `Err`,
/// never a `warn`-and-continue or a silently keyless default snapshot. Scoped
/// to the credential-resolution block (bounded by its own doc comment and the
/// next unrelated comment marker) so this does not false-positive on the
/// file's other, unrelated `.ok()`/`warn!` call sites.
#[test]
fn factory_propagates_a_sealed_vault_as_an_error() {
    const SRC: &str = include_str!("../src/app_runtime_factory.rs");

    let start = SRC
        .find("Phase 41.3 Plan 11 (D-19): resolve the tool-credential snapshot")
        .expect("the credential-resolution block's doc comment must be present");
    let end = start
        + SRC[start..]
            .find("Phase 36.3.12 GAP 1")
            .expect("the credential-resolution block must be followed by the terminal-config comment");
    let block = &SRC[start..end];

    assert!(
        block.contains("ToolCredentials::resolve("),
        "the scoped block must contain the resolve() call"
    );
    assert!(
        !block.contains("unwrap_or_default()"),
        "the resolver's Result must not be swallowed with unwrap_or_default(), got block:\n{block}"
    );
    assert!(
        !block.contains(".ok()"),
        "the resolver's Result must not be swallowed with .ok(), got block:\n{block}"
    );
    assert!(
        !block.contains("warn!"),
        "a sealed vault must not be softened to a warn-and-continue, got block:\n{block}"
    );
    assert!(
        block.contains(")?,"),
        "the resolver's Result must be propagated with a bare `?`, got block:\n{block}"
    );
}

/// T-41.3-53: an operator who never enabled the vault must never be blocked
/// from booting by it — the store is opened ONLY behind the `vault.enabled`
/// check, never unconditionally.
#[test]
fn factory_only_opens_the_store_when_the_vault_is_enabled() {
    const SRC: &str = include_str!("../src/app_runtime_factory.rs");

    let enabled_check_idx = SRC
        .find("input.config.vault.enabled")
        .expect("credential resolution must check input.config.vault.enabled");
    let open_store_idx = SRC
        .find("ironhermes_vault::open_store(")
        .expect("credential resolution must call ironhermes_vault::open_store");

    assert!(
        enabled_check_idx < open_store_idx,
        "the vault.enabled check must precede the open_store call, so an operator who \
         never enabled the vault can never be blocked from booting by it (T-41.3-53)"
    );
    assert!(
        SRC.contains("resolve_vault_config(") && SRC.contains("open_store(&ironhermes_core::resolve_vault_config("),
        "open_store must be called with resolve_vault_config's output, never the raw \
         input.config.vault (the unresolved data_dir sentinel is what \
         vault_resolve_integration.rs exists to catch)"
    );
    assert!(
        !SRC.contains("open_store(&input.config.vault)"),
        "open_store must never be called with the raw, unresolved input.config.vault"
    );
}

/// D-19: the RPC sandbox registry (`build_rpc_registry`, used by
/// `execute_code`'s nested calls) must receive the SAME resolved snapshot as
/// the default registry — not a fresh env-only default. This widens no
/// capability (the sandbox's hand-rolled tool list is unchanged); it only
/// gives its web_search a credential source.
#[test]
fn rpc_registry_receives_the_resolved_snapshot() {
    const SRC: &str = include_str!("../src/app_runtime_factory.rs");

    assert!(
        SRC.contains("build_rpc_registry(input.memory_manager.clone(), tool_credentials.clone())"),
        "build_rpc_registry must be called with the resolved tool_credentials snapshot, \
         not just the memory manager"
    );
    assert!(
        SRC.contains("fn build_rpc_registry(")
            && SRC.contains("credentials: Arc<ironhermes_tools::credentials::ToolCredentials>,"),
        "build_rpc_registry's signature must take the resolved snapshot as a parameter — \
         it is a sync fn and cannot resolve anything itself"
    );
}
