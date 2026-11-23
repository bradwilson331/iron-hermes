//! Cross-process write safety for `AuthStore` (phase 54.8, plan 16, D-14 / D-15).
//!
//! Two `AuthStore` instances opened on the same `auth.json` model two processes
//! (the gateway and the web server in the container, or the CLI plus the gateway
//! on a host): each holds its own in-memory snapshot, and the `flock` taken on
//! `auth.json.lock` conflicts across separate opens exactly as it does across
//! processes. A save from a stale snapshot must never delete or revert a key
//! another writer put on disk (T-54.8-47).

use ironhermes_core::auth::{AuthStore, DcrEntry, TokenEntry};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

fn make_token(access: &str) -> TokenEntry {
    TokenEntry {
        access_token: access.to_string(),
        refresh_token: Some("refresh-test-548".to_string()),
        expires_at: Some(UNIX_EPOCH + Duration::from_secs(9_999_999_999)),
        scopes: vec!["account:read".to_string()],
    }
}

fn make_dcr(client_id: &str) -> DcrEntry {
    DcrEntry {
        client_id: client_id.to_string(),
        client_secret: Some("secret-test-548".to_string()),
        registration_access_token: None,
    }
}

const CLIENT_URL: &str = "https://mcp.example.test/mcp";

#[tokio::test]
async fn two_stores_on_one_file_keep_both_writes() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("auth.json");

    let a = AuthStore::open(path.clone()).await.unwrap();
    let b = AuthStore::open(path.clone()).await.unwrap();

    a.put_token("ns_a", make_token("token-a-548"))
        .await
        .unwrap();
    b.put_token("ns_b", make_token("token-b-548"))
        .await
        .unwrap();

    let c = AuthStore::open(path).await.unwrap();
    assert_eq!(
        c.get_token("ns_a")
            .await
            .expect("ns_a must survive")
            .access_token,
        "token-a-548"
    );
    assert_eq!(
        c.get_token("ns_b")
            .await
            .expect("ns_b must survive")
            .access_token,
        "token-b-548"
    );
}

#[tokio::test]
async fn stale_snapshot_save_keeps_other_writers_token() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("auth.json");

    {
        let seed = AuthStore::open(path.clone()).await.unwrap();
        seed.put_token("ns_x", make_token("token-v1-548"))
            .await
            .unwrap();
    }

    let a = AuthStore::open(path.clone()).await.unwrap();
    let b = AuthStore::open(path.clone()).await.unwrap();

    a.put_token("ns_x", make_token("token-v2-548"))
        .await
        .unwrap();
    // B's snapshot still holds v1 for ns_x; saving its own client entry must not revert it.
    b.put_client(CLIENT_URL, make_dcr("client-548"))
        .await
        .unwrap();

    let c = AuthStore::open(path).await.unwrap();
    assert_eq!(
        c.get_token("ns_x")
            .await
            .expect("ns_x must survive")
            .access_token,
        "token-v2-548",
        "a stale snapshot save reverted another writer's newer token"
    );
    assert_eq!(
        c.get_client(CLIENT_URL)
            .await
            .expect("client must be saved")
            .client_id,
        "client-548"
    );
}

#[tokio::test]
async fn same_key_last_writer_wins() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("auth.json");

    let a = AuthStore::open(path.clone()).await.unwrap();
    let b = AuthStore::open(path.clone()).await.unwrap();

    a.put_token("ns_y", make_token("token-v1-548"))
        .await
        .unwrap();
    b.put_token("ns_y", make_token("token-v2-548"))
        .await
        .unwrap();

    let c = AuthStore::open(path).await.unwrap();
    assert_eq!(
        c.get_token("ns_y").await.unwrap().access_token,
        "token-v2-548"
    );
}

#[tokio::test]
async fn merge_adopts_other_writers_keys_in_memory() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("auth.json");

    {
        let seed = AuthStore::open(path.clone()).await.unwrap();
        seed.put_token("ns_x", make_token("token-v1-548"))
            .await
            .unwrap();
    }

    let a = AuthStore::open(path.clone()).await.unwrap();
    let b = AuthStore::open(path).await.unwrap();

    a.put_token("ns_x", make_token("token-v2-548"))
        .await
        .unwrap();
    b.put_client(CLIENT_URL, make_dcr("client-548"))
        .await
        .unwrap();

    assert_eq!(
        b.get_token("ns_x")
            .await
            .expect("merge must be adopted into memory")
            .access_token,
        "token-v2-548",
        "B must see A's newer token without reopening"
    );
}

#[tokio::test]
async fn save_times_out_when_lock_is_held() {
    use fs2::FileExt as _;

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("auth.json");
    let lock_path = dir.path().join("auth.json.lock");

    let store = AuthStore::open(path.clone()).await.unwrap();
    store.set_lock_timeout_for_tests(Duration::from_millis(200));

    // Hold the cross-process lock from outside the store.
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .unwrap();
    holder.lock_exclusive().unwrap();

    let started = std::time::Instant::now();
    let err = store
        .put_token("ns_z", make_token("token-z-548"))
        .await
        .expect_err("a save under a held lock must fail, not hang or succeed");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "lock wait must be bounded; took {:?}",
        started.elapsed()
    );
    assert!(
        format!("{err:#}").contains("auth.json.lock"),
        "error must name the lock file; got: {err:#}"
    );

    holder.unlock().unwrap();
    drop(holder);

    // The failed save's key stayed pending, so the next successful save carries it.
    store
        .put_token("ns_w", make_token("token-w-548"))
        .await
        .unwrap();

    let reopened = AuthStore::open(path).await.unwrap();
    assert!(
        reopened.get_token("ns_z").await.is_some(),
        "ns_z must be retried"
    );
    assert!(
        reopened.get_token("ns_w").await.is_some(),
        "ns_w must be saved"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn lock_and_auth_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("auth.json");
    let lock_path = dir.path().join("auth.json.lock");

    let store = AuthStore::open(path.clone()).await.unwrap();
    store
        .put_token("ns_m", make_token("token-m-548"))
        .await
        .unwrap();

    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600, "auth.json must be 0600");
    assert!(lock_path.exists(), "auth.json.lock must exist after a save");
    assert_eq!(mode(&lock_path), 0o600, "auth.json.lock must be 0600");
    assert_eq!(mode(dir.path()), 0o700, "auth directory must be 0700");
}
