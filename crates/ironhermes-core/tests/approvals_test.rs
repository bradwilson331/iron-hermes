//! Integration tests for ApprovalsStore (EXEC-04 / D-01 / D-02).
//!
//! RED phase — references `ApprovalsStore` and `KeyKind` which do not exist yet in
//! `ironhermes_core`, so `cargo test -p ironhermes-core -- approvals` must fail to compile.

use ironhermes_core::{ApprovalsStore, KeyKind};
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// normalize_command (D-02)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn normalize_command_collapses_whitespace() {
    let result = ApprovalsStore::normalize_command("  rm   -rf  /tmp/x  ");
    assert_eq!(result, "rm -rf /tmp/x");
}

#[tokio::test]
async fn normalize_command_not_lowercased() {
    // Paths are case-sensitive — must NOT lowercase (D-02)
    let result = ApprovalsStore::normalize_command("GREP -r Pattern /Some/PATH");
    assert_eq!(result, "GREP -r Pattern /Some/PATH");
}

#[tokio::test]
async fn normalize_command_trims_only() {
    // Single-space-separated tokens: no collapse needed, just trim
    let result = ApprovalsStore::normalize_command("  echo hello  ");
    assert_eq!(result, "echo hello");
}

// ─────────────────────────────────────────────────────────────────────────────
// Session scope (in-memory, not persisted — D-01)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn session_scope_in_memory() {
    let tmp = TempDir::new().unwrap();
    let store = ApprovalsStore::with_path(tmp.path().join("approvals.json"));

    let key = "test-cache-key";

    // Nothing approved yet
    assert!(!store.is_session_approved(key).await);

    // Approve for session
    store.approve_session(key).await;
    assert!(store.is_session_approved(key).await);

    // A fresh store at the same path has no session memory (simulates restart)
    let fresh = ApprovalsStore::with_path(tmp.path().join("approvals.json"));
    assert!(
        !fresh.is_session_approved(key).await,
        "fresh store must not inherit session memory from previous instance"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// by_name / by_command are distinct namespaces (D-02 anti-binary-keying)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn by_name_and_command_are_distinct_namespaces() {
    let tmp = TempDir::new().unwrap();
    let store = ApprovalsStore::with_path(tmp.path().join("approvals.json"));

    // Approve "curl example.com" by_command
    store
        .approve_always("curl example.com", KeyKind::Command)
        .await;

    // by_name "curl example.com" must NOT be approved (different namespace)
    assert!(
        !store
            .is_always_approved("curl example.com", KeyKind::Name)
            .await,
        "by_command approval must not bleed into by_name"
    );
    assert!(
        store
            .is_always_approved("curl example.com", KeyKind::Command)
            .await,
        "by_command approval must be visible in the command namespace"
    );
}

#[tokio::test]
async fn by_name_approval_does_not_bleed_into_by_command() {
    let tmp = TempDir::new().unwrap();
    let store = ApprovalsStore::with_path(tmp.path().join("approvals.json"));

    store.approve_always("wipe-cache", KeyKind::Name).await;

    assert!(
        !store
            .is_always_approved("wipe-cache", KeyKind::Command)
            .await,
        "by_name approval must not bleed into by_command"
    );
    assert!(
        store.is_always_approved("wipe-cache", KeyKind::Name).await,
        "by_name approval must be visible in the name namespace"
    );
}

#[tokio::test]
async fn different_command_strings_are_independent() {
    // D-02 anti-binary-keying: approving "rm -rf /tmp/foo" must NOT approve "rm -rf /tmp/bar"
    let tmp = TempDir::new().unwrap();
    let store = ApprovalsStore::with_path(tmp.path().join("approvals.json"));

    store
        .approve_always("rm -rf /tmp/foo", KeyKind::Command)
        .await;

    assert!(
        !store
            .is_always_approved("rm -rf /tmp/bar", KeyKind::Command)
            .await,
        "approving one command must not approve a different command string"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Persistence (D-01 / T-42-11)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_always_persists() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("approvals.json");
    let store = ApprovalsStore::with_path(path.clone());

    store.approve_always("wipe-cache", KeyKind::Name).await;
    store.save_to_disk().await.unwrap();

    // Load a fresh store from the same path — must see the persisted approval
    let loaded = ApprovalsStore::load_from_path(path).await;
    assert!(
        loaded.is_always_approved("wipe-cache", KeyKind::Name).await,
        "always approval must survive a store reload (restart)"
    );

    // Session memory is NOT persisted
    assert!(
        !loaded.is_session_approved("wipe-cache").await,
        "session memory must not be persisted to disk"
    );
}

#[tokio::test]
async fn approvals_json_permissions() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("approvals.json");
    let store = ApprovalsStore::with_path(path.clone());

    store
        .approve_always("redis-cli flushall", KeyKind::Command)
        .await;
    store.save_to_disk().await.unwrap();

    assert!(
        path.exists(),
        "approvals.json must be created on save_to_disk"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&path).unwrap();
        let mode = meta.permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "approvals.json must have mode 0600 (got {:o})",
            mode & 0o777
        );
    }
}
