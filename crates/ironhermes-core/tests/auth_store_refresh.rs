//! Concurrency test for `AuthStore::refresh()` — thundering-herd guard.
//!
//! Success-criterion #2 (PITFALLS §A-2, T-41-05): 10 concurrent `refresh()` calls
//! on the same namespace must trigger exactly 1 invocation of the `RefreshFn` seam.
//! All 10 callers must receive the same refreshed `TokenEntry` produced by the single
//! leader — latecomers wait on `tokio::sync::Notify` instead of issuing their own
//! exchange.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, UNIX_EPOCH};

use ironhermes_core::auth::{AuthStore, RefreshFn, TokenEntry};
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn make_fresh_token() -> TokenEntry {
    TokenEntry {
        access_token: "refreshed-access-token".to_string(),
        refresh_token: Some("refreshed-refresh-token".to_string()),
        // Far-future expiry so tests don't re-trigger immediately.
        expires_at: Some(UNIX_EPOCH + Duration::from_secs(9_999_999_999)),
        scopes: vec!["account:read".to_string()],
    }
}

fn make_expired_token() -> TokenEntry {
    TokenEntry {
        access_token: "old-access-token".to_string(),
        refresh_token: Some("old-refresh-token".to_string()),
        // UNIX_EPOCH is definitively in the past — token is expired.
        expires_at: Some(UNIX_EPOCH),
        scopes: vec!["account:read".to_string()],
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: success-criterion #2 — 10 concurrent refreshes → exactly 1 exchange
// ─────────────────────────────────────────────────────────────────────────────

/// Success-criterion #2 (PITFALLS §A-2):
/// Spawning 10 concurrent `refresh("cloudflare_mcp_api")` calls must result in:
/// 1. Exactly one invocation of the `RefreshFn` seam (the thundering-herd guard works).
/// 2. All 10 callers receiving the same refreshed `TokenEntry` from the leader.
///
/// The mock `RefreshFn` sleeps 50 ms to widen the race window so all 10 tasks can
/// queue as waiters before the leader completes.
#[cfg(not(target_arch = "wasm32"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ten_concurrent_refreshes_invoke_exchange_exactly_once() {
    let dir = TempDir::new().unwrap();
    let auth_path = dir.path().join("auth.json");

    // Shared counter: every invocation of the RefreshFn increments this.
    let exchange_count = Arc::new(AtomicUsize::new(0));
    let fresh_token = make_fresh_token();

    // Build the counting RefreshFn.
    let count_clone = Arc::clone(&exchange_count);
    let token_clone = fresh_token.clone();
    let refresh_fn: RefreshFn = Arc::new(move |_ns: String| {
        let count = Arc::clone(&count_clone);
        let token = token_clone.clone();
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            // Sleep to widen the race window: all 10 tasks must be able to queue
            // as waiters before the leader wakes and calls notify_waiters().
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(token)
        })
    });

    // Open store with the counting RefreshFn.
    let store = AuthStore::open_with_refresh(auth_path, refresh_fn)
        .await
        .unwrap();

    // Seed an expired token so all callers can see stale state.
    // (refresh() unconditionally calls refresh_fn; the guard fires regardless of expiry.)
    store
        .put_token("cloudflare_mcp_api", make_expired_token())
        .await
        .unwrap();

    // Spawn 10 concurrent tasks, each calling refresh() on the same namespace.
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let s = Arc::clone(&store);
        set.spawn(async move { s.refresh("cloudflare_mcp_api").await });
    }

    // Collect all results — every task must succeed.
    let mut tokens: Vec<TokenEntry> = Vec::new();
    while let Some(join_result) = set.join_next().await {
        let token = join_result
            .expect("task panicked")
            .expect("refresh() returned Err — leader exchange must succeed");
        tokens.push(token);
    }

    // ── Assertion 1: exactly one exchange occurred ──────────────────────────
    assert_eq!(
        exchange_count.load(Ordering::SeqCst),
        1,
        "thundering-herd guard must limit concurrent token exchanges to exactly 1; \
         got {} exchange(s) instead",
        exchange_count.load(Ordering::SeqCst),
    );

    // ── Assertion 2: all 10 callers received the leader's token ────────────
    assert_eq!(
        tokens.len(),
        10,
        "all 10 spawned tasks must complete and return a token"
    );
    for (i, token) in tokens.iter().enumerate() {
        assert_eq!(
            token.access_token, fresh_token.access_token,
            "task {i}: all callers must share the leader's refreshed access_token"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: H-01 — the thundering-herd guard is PER-NAMESPACE, not store-global.
//
// 5 concurrent refresh("provider_a") + 5 concurrent refresh("provider_b") against
// per-namespace counting RefreshFns must yield EXACTLY 1 exchange for "provider_a"
// AND EXACTLY 1 for "provider_b". A store-global guard (the pre-fix bug) would let
// one provider's leader suppress the other provider's exchange, producing a total
// of 1 (one namespace never refreshed) instead of 2.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_namespace_guard_refreshes_each_namespace_exactly_once() {
    let dir = TempDir::new().unwrap();
    let auth_path = dir.path().join("auth.json");

    // Separate counters per namespace so we can assert each was exchanged once.
    let count_a = Arc::new(AtomicUsize::new(0));
    let count_b = Arc::new(AtomicUsize::new(0));

    let count_a_fn = Arc::clone(&count_a);
    let count_b_fn = Arc::clone(&count_b);
    let refresh_fn: RefreshFn = Arc::new(move |ns: String| {
        let count_a = Arc::clone(&count_a_fn);
        let count_b = Arc::clone(&count_b_fn);
        Box::pin(async move {
            match ns.as_str() {
                "provider_a" => {
                    count_a.fetch_add(1, Ordering::SeqCst);
                }
                "provider_b" => {
                    count_b.fetch_add(1, Ordering::SeqCst);
                }
                other => panic!("unexpected namespace in refresh_fn: {other}"),
            }
            // Sleep to widen the race window so all same-ns callers queue as waiters
            // before the leader wakes them.
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut tok = make_fresh_token();
            tok.access_token = format!("refreshed-{ns}");
            Ok(tok)
        })
    });

    let store = AuthStore::open_with_refresh(auth_path, refresh_fn)
        .await
        .unwrap();

    // Seed expired tokens for both namespaces.
    store
        .put_token("provider_a", make_expired_token())
        .await
        .unwrap();
    store
        .put_token("provider_b", make_expired_token())
        .await
        .unwrap();

    // Spawn 5 concurrent refreshes for each namespace (10 tasks total).
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..5 {
        let s = Arc::clone(&store);
        set.spawn(async move { (String::from("provider_a"), s.refresh("provider_a").await) });
    }
    for _ in 0..5 {
        let s = Arc::clone(&store);
        set.spawn(async move { (String::from("provider_b"), s.refresh("provider_b").await) });
    }

    let mut completed = 0usize;
    while let Some(join_result) = set.join_next().await {
        let (ns, refresh_result) = join_result.expect("task panicked");
        let token = refresh_result.unwrap_or_else(|e| panic!("refresh({ns}) returned Err: {e}"));
        assert_eq!(
            token.access_token,
            format!("refreshed-{ns}"),
            "each caller must receive its own namespace's refreshed token"
        );
        completed += 1;
    }
    assert_eq!(completed, 10, "all 10 spawned tasks must complete");

    // ── The core per-namespace assertions ───────────────────────────────────
    assert_eq!(
        count_a.load(Ordering::SeqCst),
        1,
        "provider_a must be exchanged EXACTLY once (got {})",
        count_a.load(Ordering::SeqCst),
    );
    assert_eq!(
        count_b.load(Ordering::SeqCst),
        1,
        "provider_b must be exchanged EXACTLY once — a store-global guard would \
         suppress this and yield 0 (got {})",
        count_b.load(Ordering::SeqCst),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: D-03/D-04 (46.1-03) — spawn_refresh_task retries after a failure and
// recovers via the RefreshFn seam.
//
// Seeds an already-expired token so `spawn_refresh_task`'s `fire_at` computation
// is immediate (Duration::ZERO), forcing the FIRST call into the loop right away.
// The mock RefreshFn fails on its first invocation, then succeeds on every
// subsequent call. Rather than waiting out the real ~30s D-03 failure backoff
// (see `store::failure_backoff_tests` for the pure-helper coverage of that curve),
// this test drives a second `refresh()` call directly once the spawned task's
// first (failing) attempt is observed — proving the RefreshFn is invoked more
// than once and that the store's cached token is eventually updated.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_refresh_task_retries_after_failure_and_recovers() {
    let dir = TempDir::new().unwrap();
    let auth_path = dir.path().join("auth.json");
    let ns = "cloudflare_mcp_bg";

    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_fn = Arc::clone(&attempts);
    let mut fresh_token = make_fresh_token();
    fresh_token.access_token = "recovered-access-token".to_string();
    let fresh_token_fn = fresh_token.clone();

    let refresh_fn: RefreshFn = Arc::new(move |_ns: String| {
        let attempts = Arc::clone(&attempts_fn);
        let token = fresh_token_fn.clone();
        Box::pin(async move {
            let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 1 {
                // First invocation always fails — exercises the D-03 warn-once
                // + backoff-scheduling path in spawn_refresh_task's Err arm.
                Err(anyhow::anyhow!("mock refresh failure (attempt {n})"))
            } else {
                Ok(token)
            }
        })
    });

    let store = AuthStore::open_with_refresh(auth_path, refresh_fn)
        .await
        .unwrap();
    store.put_token(ns, make_expired_token()).await.unwrap();

    // fire_at is immediate for an already-expired token — the spawned task's
    // first loop iteration calls the RefreshFn right away (no real sleep needed).
    let _handle = AuthStore::spawn_refresh_task(Arc::clone(&store), ns.to_string());

    // Bounded wait for the spawned task's first (failing) attempt — proves the
    // background loop actually drives the seam without relying on a fixed sleep.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if attempts.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect(
        "spawn_refresh_task must invoke the RefreshFn at least once for an \
         already-expired token within 5s",
    );

    // D-03 test-friendliness: instead of waiting out the real ~30s backoff for
    // the spawned task's own retry, drive a second refresh() directly — this
    // proves recovery (>1 invocation, token eventually updated) without a real
    // 30s sleep in the test suite. The escalating-backoff curve itself is
    // covered by the pure `backoff_after_failures`/`should_warn_failure` unit
    // tests in `store.rs`.
    let recovered = tokio::time::timeout(Duration::from_secs(5), store.refresh(ns))
        .await
        .expect("second refresh() must complete within 5s")
        .expect("refresh() must eventually succeed after the first failure");

    assert!(
        attempts.load(Ordering::SeqCst) >= 2,
        "RefreshFn must be invoked more than once (failed, then succeeded); got {}",
        attempts.load(Ordering::SeqCst),
    );
    assert_eq!(
        recovered.access_token, fresh_token.access_token,
        "the recovered refresh() call must return the fresh token"
    );

    let stored = store
        .get_token(ns)
        .await
        .expect("AuthStore must have a cached token for ns after recovery");
    assert_eq!(
        stored.access_token, fresh_token.access_token,
        "AuthStore's cached token must reflect the recovered refresh"
    );
}
