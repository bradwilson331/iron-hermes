//! Integration tests for AuthStore persistence (Plan 41-01).
//!
//! Covers success-criteria #1 (0600 mode) and #4 (two-provider namespacing) plus the
//! Debug redaction invariant (PITFALLS §A-1, D-08).

use ironhermes_core::auth::{AuthStore, DcrEntry, TokenEntry};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn make_token(access: &str) -> TokenEntry {
    TokenEntry {
        access_token: access.to_string(),
        refresh_token: Some("refresh-abc".to_string()),
        expires_at: Some(UNIX_EPOCH + Duration::from_secs(9_999_999_999)),
        scopes: vec!["account:read".to_string()],
    }
}

fn make_dcr(client_id: &str) -> DcrEntry {
    DcrEntry {
        client_id: client_id.to_string(),
        client_secret: Some("secret-xyz".to_string()),
        registration_access_token: Some("rat-abc".to_string()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: auth.json written at mode 0600 (success-criterion #1, T-41-01)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(unix)]
#[tokio::test]
async fn auth_json_written_at_mode_0600() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = TempDir::new().unwrap();
    let auth_path = dir.path().join("auth.json");

    let store = AuthStore::open(auth_path.clone()).await.unwrap();
    store
        .put_token("cloudflare_mcp_api", make_token("tok-secret-0600"))
        .await
        .unwrap();

    // auth.json must exist and have mode 0o600.
    assert!(auth_path.exists(), "auth.json must be created by put_token");
    let mode = std::fs::metadata(&auth_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "auth.json must be mode 0600, got {mode:04o}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: two providers namespaced, no collision (success-criterion #4, D-06)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn two_providers_namespaced_no_collision() {
    let dir = TempDir::new().unwrap();
    let auth_path = dir.path().join("auth.json");

    // Write two distinct tokens and a DCR entry.
    {
        let store = AuthStore::open(auth_path.clone()).await.unwrap();
        store
            .put_token("cloudflare_mcp_api", make_token("tok-cloudflare"))
            .await
            .unwrap();
        store
            .put_token("provider_minimax", make_token("tok-minimax"))
            .await
            .unwrap();
        store
            .put_client("https://api.example.com/mcp", make_dcr("client-42"))
            .await
            .unwrap();
    }

    // Re-open from disk and verify all three entries are intact.
    let store2 = AuthStore::open(auth_path).await.unwrap();

    let cf = store2
        .get_token("cloudflare_mcp_api")
        .await
        .expect("cloudflare token must survive disk round-trip");
    assert_eq!(cf.access_token, "tok-cloudflare");

    let mm = store2
        .get_token("provider_minimax")
        .await
        .expect("minimax token must survive disk round-trip");
    assert_eq!(mm.access_token, "tok-minimax");

    // Tokens must not have overwritten each other.
    assert_ne!(
        cf.access_token, mm.access_token,
        "two namespaces must not collide"
    );

    // DCR entry must be in clients, not tokens.
    let dcr = store2
        .get_client("https://api.example.com/mcp")
        .await
        .expect("DCR entry must survive disk round-trip");
    assert_eq!(dcr.client_id, "client-42");

    // The clients namespace must not bleed into tokens.
    assert!(
        store2
            .get_token("https://api.example.com/mcp")
            .await
            .is_none(),
        "DCR entries must not appear in the tokens map"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: TokenEntry Debug output is redacted (PITFALLS §A-1, D-08)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn token_entry_debug_is_redacted() {
    let entry = make_token("SUPER_SECRET_ACCESS_TOKEN_DO_NOT_LOG");

    let debug_str = format!("{entry:?}");

    assert!(
        debug_str.contains("[REDACTED]"),
        "Debug output must contain the [REDACTED] marker; got: {debug_str}"
    );
    assert!(
        !debug_str.contains("SUPER_SECRET_ACCESS_TOKEN_DO_NOT_LOG"),
        "Debug output must NOT contain the raw access_token; got: {debug_str}"
    );
    // refresh_token is also a secret — must be redacted.
    assert!(
        !debug_str.contains("refresh-abc"),
        "Debug output must NOT contain the raw refresh_token; got: {debug_str}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Bonus: DcrEntry Debug is also redacted
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn dcr_entry_debug_is_redacted() {
    let entry = make_dcr("public-client-id");
    let entry = DcrEntry {
        client_secret: Some("SECRET_CLIENT_SECRET".to_string()),
        registration_access_token: Some("SECRET_RAT".to_string()),
        ..entry
    };

    let debug_str = format!("{entry:?}");

    assert!(
        debug_str.contains("[REDACTED]"),
        "DcrEntry Debug must contain [REDACTED]; got: {debug_str}"
    );
    assert!(
        debug_str.contains("public-client-id"),
        "DcrEntry Debug should show client_id (non-secret); got: {debug_str}"
    );
    assert!(
        !debug_str.contains("SECRET_CLIENT_SECRET"),
        "DcrEntry Debug must not expose client_secret; got: {debug_str}"
    );
    assert!(
        !debug_str.contains("SECRET_RAT"),
        "DcrEntry Debug must not expose registration_access_token; got: {debug_str}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Bonus: SystemTime round-trips correctly through serialization
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn expires_at_round_trips() {
    let dir = TempDir::new().unwrap();
    let auth_path = dir.path().join("auth.json");

    let expected_secs = 1_800_000_000u64;
    let entry = TokenEntry {
        access_token: "tok".to_string(),
        refresh_token: None,
        expires_at: Some(UNIX_EPOCH + Duration::from_secs(expected_secs)),
        scopes: vec![],
    };

    let store = AuthStore::open(auth_path.clone()).await.unwrap();
    store.put_token("test_ns", entry).await.unwrap();

    let store2 = AuthStore::open(auth_path).await.unwrap();
    let loaded = store2.get_token("test_ns").await.unwrap();

    let loaded_secs = loaded
        .expires_at
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert_eq!(
        loaded_secs, expected_secs,
        "expires_at must round-trip through disk serialization"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Bonus: missing file returns empty store (no crash)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn open_nonexistent_path_returns_empty_store() {
    let dir = TempDir::new().unwrap();
    let auth_path = dir.path().join("does_not_exist.json");

    let store = AuthStore::open(auth_path).await.unwrap();
    assert!(store.get_token("any").await.is_none());
    assert!(store.get_client("any").await.is_none());
}
