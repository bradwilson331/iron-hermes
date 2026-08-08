//! On-disk OAuth token store for IronHermes.
//!
//! Schema (D-06): `{ "tokens": { "<ns>": TokenEntry }, "clients": { "<url>": DcrEntry } }`
//!
//! Security properties:
//! - `auth.json` is written at mode 0600; parent directory enforced at 0700 on every
//!   write, not only on first creation (T-41-01, M-03).
//! - `TokenEntry` and `DcrEntry` never derive `Debug` — manual impl emits `[REDACTED]` for all
//!   secret fields so tracing/panic output cannot leak credentials (D-08, PITFALLS §A-1).
//! - Atomic write: serialize → `.json.tmp` → fsync → rename → chmod 0600 (T-41-04).
//! - wasm32: all I/O paths are no-ops; the store operates in-memory only (D-09).
//! - Thundering-herd guard (Plan 02, T-41-05): `refresh()` uses `Arc<Mutex>` + `Arc<Notify>`
//!   to ensure at most one concurrent token exchange per namespace. The Mutex is NEVER held
//!   across the exchange call (PITFALLS §A-2).

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify};

// ─────────────────────────────────────────────────────────────────────────────
// RefreshFn — injectable token-exchange seam (Phase 43 wires the real OAuth flow)
// ─────────────────────────────────────────────────────────────────────────────

/// Async function that performs a token exchange for a given namespace.
///
/// The seam is injected at construction time:
/// - `AuthStore::open` installs a stub that returns an error (live flow is Phase 43, D-08).
/// - `AuthStore::open_with_refresh` accepts a custom implementation for tests and Phase 43.
///
/// Signature: given a namespace string, return a fresh `TokenEntry` or an error.
pub type RefreshFn = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = anyhow::Result<TokenEntry>> + Send>>
        + Send
        + Sync,
>;

// ─────────────────────────────────────────────────────────────────────────────
// SystemTime ↔ JSON serde (seconds since UNIX_EPOCH, no extra deps)
// ─────────────────────────────────────────────────────────────────────────────

mod opt_system_time_serde {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(v: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match v {
            None => s.serialize_none(),
            Some(t) => {
                let secs = t
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or(Duration::ZERO)
                    .as_secs();
                s.serialize_some(&secs)
            }
        }
    }

    pub fn deserialize<'de, D>(d: D) -> Result<Option<SystemTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt = Option::<u64>::deserialize(d)?;
        Ok(opt.map(|secs| UNIX_EPOCH + Duration::from_secs(secs)))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public token types
// IMPORTANT: Never #[derive(Debug)] on these — manual impls emit [REDACTED]
// for every secret field (D-08, PITFALLS §A-1).
// ─────────────────────────────────────────────────────────────────────────────

/// OAuth access/refresh token for a single provider namespace.
///
/// Stored under `tokens.<ns>` in `auth.json` (D-06).
/// `expires_at` serializes as seconds-since-epoch (u64) so no chrono dep is needed.
#[derive(Clone, Serialize, Deserialize)]
pub struct TokenEntry {
    pub access_token: String,
    pub refresh_token: Option<String>,
    #[serde(
        with = "opt_system_time_serde",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub expires_at: Option<std::time::SystemTime>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Manual Debug — never derives to avoid leaking secrets into tracing/panic output.
impl std::fmt::Debug for TokenEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenEntry")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// Dynamic Client Registration entry keyed by server URL.
///
/// Stored under `clients.<url>` in `auth.json` (D-06, PITFALLS §B-3).
#[derive(Clone, Serialize, Deserialize)]
pub struct DcrEntry {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub registration_access_token: Option<String>,
}

/// Manual Debug — never derives to avoid leaking secrets into tracing/panic output.
impl std::fmt::Debug for DcrEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DcrEntry")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("registration_access_token", &"[REDACTED]")
            .finish()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Private on-disk shape — Debug intentionally omitted (holds secrets)
// Only compiled on native; wasm never touches disk.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Serialize, Deserialize, Default)]
struct AuthFile {
    #[serde(default)]
    tokens: HashMap<String, TokenEntry>,
    #[serde(default)]
    clients: HashMap<String, DcrEntry>,
}

/// Monotonic counter making each atomic-write temp path unique (H-03).
///
/// A unique `auth.json.tmp.<pid>.<n>` name (rather than a fixed `auth.json.tmp`)
/// lets `create_new(true)` stay strict — it refuses a pre-planted tmp/symlink —
/// without two concurrent `save_to_disk` calls colliding on the same temp file.
#[cfg(not(target_arch = "wasm32"))]
static TMP_WRITE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// In-memory cache
// ─────────────────────────────────────────────────────────────────────────────

/// Shared per-namespace refresh coordination slot (H-01/H-02/M-01).
///
/// Created by the leader for a single `ns`, then cloned (`Arc`) by every
/// latecomer for that same `ns`:
/// - `notify` wakes latecomers once the leader finishes (success *or* failure).
/// - `outcome` carries the leader's `Result` so latecomers observe the real
///   outcome (M-01) instead of silently returning a stale cached token when the
///   leader's exchange actually failed.
///
/// The error is stored as a `String` (the formatted `{e:#}` chain) because
/// `anyhow::Error` is not `Clone` and each latecomer needs its own copy.
struct RefreshSlot {
    notify: Notify,
    /// `None` until the leader records its result; then `Some(Ok)` / `Some(Err)`.
    /// Guarded by a `std::sync::Mutex` that is only ever held for the duration of
    /// a clone/assignment — never across an `.await`.
    outcome: std::sync::Mutex<Option<std::result::Result<TokenEntry, String>>>,
}

/// In-memory token cache.
///
/// `refresh_in_flight` is the **per-namespace** thundering-herd guard (H-01,
/// PITFALLS §A-2, T-41-05). It is a `HashMap` keyed by `ns` so that concurrent
/// refreshes of *different* providers proceed independently — only refreshes of
/// the *same* `ns` collapse onto a single exchange:
/// - The first caller for an `ns` becomes the leader and inserts a `RefreshSlot`.
/// - Latecomers for that same `ns` clone the `Arc<RefreshSlot>`, register on its
///   `Notify` *before* dropping the Mutex guard (H-02 lost-wakeup fix), then wait.
/// - On completion (success or error), the leader records the outcome, removes
///   its entry under `ns`, and calls `notify_waiters()` so no waiter hangs.
#[derive(Default)]
struct AuthState {
    tokens: HashMap<String, TokenEntry>,
    clients: HashMap<String, DcrEntry>,
    /// Per-namespace thundering-herd guard: keyed by `ns`. Each entry is created
    /// by that namespace's leader and removed on completion (success or error).
    refresh_in_flight: HashMap<String, Arc<RefreshSlot>>,
}

// ─────────────────────────────────────────────────────────────────────────────
// AuthStore
// ─────────────────────────────────────────────────────────────────────────────

/// Thread-safe, file-backed OAuth token store.
///
/// - Tokens are namespaced by provider/server key (`tokens.<ns>`).
/// - DCR entries are namespaced by server URL (`clients.<url>`).
/// - `auth.json` is written atomically at mode 0600 (T-41-01).
/// - On wasm32 the store is in-memory only; all I/O operations are no-ops (D-09).
/// - `refresh()` implements a thundering-herd guard: at most one concurrent
///   token exchange per namespace (T-41-05, PITFALLS §A-2).
pub struct AuthStore {
    state: Arc<Mutex<AuthState>>,
    auth_json_path: PathBuf,
    /// Injected token-exchange seam. `open` installs a stub (D-08); Phase 43 wires
    /// the real OAuth flow via `open_with_refresh`.
    refresh_fn: RefreshFn,
}

/// Manual Debug — hides `state` and `refresh_fn` entirely because they hold secret data.
impl std::fmt::Debug for AuthStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthStore")
            .field("auth_json_path", &self.auth_json_path)
            .finish_non_exhaustive()
    }
}

impl AuthStore {
    // ─────────────────────────────────────────────────────────────────────────
    // Construction
    // ─────────────────────────────────────────────────────────────────────────

    /// Open (or create) the store at `path` with a stub `RefreshFn`.
    ///
    /// The stub returns `Err("live OAuth token exchange is not implemented until Phase 43")`
    /// for every call, satisfying D-08 (no live flow this phase) while keeping the
    /// thundering-herd guard machinery real and testable.
    ///
    /// On native: loads any existing data from disk (missing file → empty store).
    /// On wasm32: always returns an empty in-memory store.
    pub async fn open(path: PathBuf) -> Result<Arc<Self>> {
        let stub: RefreshFn = Arc::new(|_ns: String| {
            Box::pin(async {
                Err(anyhow::anyhow!(
                    "live OAuth token exchange is not implemented until Phase 43"
                ))
            })
        });
        Self::open_with_refresh(path, stub).await
    }

    /// Open (or create) the store at `path` with an injected `RefreshFn`.
    ///
    /// Use this constructor in tests (mock exchange) and in Phase 43 (real OAuth flow).
    pub async fn open_with_refresh(path: PathBuf, refresh_fn: RefreshFn) -> Result<Arc<Self>> {
        let state = Self::load_from_disk(&path).await?;
        Ok(Arc::new(Self {
            state: Arc::new(Mutex::new(state)),
            auth_json_path: path,
            refresh_fn,
        }))
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Token CRUD
    // ─────────────────────────────────────────────────────────────────────────

    /// Return the `TokenEntry` for namespace `ns`, if present.
    pub async fn get_token(&self, ns: &str) -> Option<TokenEntry> {
        let st = self.state.lock().await;
        st.tokens.get(ns).cloned()
    }

    /// Store a `TokenEntry` under namespace `ns` and persist to disk.
    pub async fn put_token(&self, ns: &str, entry: TokenEntry) -> Result<()> {
        {
            let mut st = self.state.lock().await;
            st.tokens.insert(ns.to_string(), entry);
        }
        self.save_to_disk().await
    }

    /// Refresh the token for namespace `ns` using the injected `RefreshFn`.
    ///
    /// # Thundering-herd guard (T-41-05, PITFALLS §A-2)
    ///
    /// The guard is keyed **per namespace** (H-01): concurrent refreshes of
    /// *different* providers run independently; only concurrent refreshes of the
    /// *same* `ns` collapse onto a single exchange.
    ///
    /// When multiple tasks call `refresh(ns)` concurrently for the same `ns`:
    /// - The first caller becomes the **leader**: it inserts a `RefreshSlot` under
    ///   `ns`, releases the Mutex, and calls the `RefreshFn` without holding the lock.
    /// - All subsequent callers (**latecomers**) clone the `Arc<RefreshSlot>`,
    ///   register on its `Notify` *before* dropping the Mutex guard (H-02
    ///   lost-wakeup fix), then wait. They never issue their own exchange.
    /// - On completion (success or error), the leader records its outcome in the
    ///   slot, removes the `ns` entry, and calls `notify_waiters()` so no latecomer
    ///   hangs. Latecomers then observe the leader's real result (M-01) rather than
    ///   masking a failed refresh with a stale cached token.
    /// - Defense-in-depth (H-02): the latecomer wait is bounded by a timeout so a
    ///   missed wakeup degrades to a cache re-check instead of a permanent hang.
    pub async fn refresh(&self, ns: &str) -> anyhow::Result<TokenEntry> {
        // Step 1: Check the in-flight state under the lock, keyed by `ns`.
        // Either subscribe as a latecomer or become the leader for this `ns`.
        let slot = {
            let mut st = self.state.lock().await;
            if let Some(existing) = st.refresh_in_flight.get(ns) {
                // Another refresh for THIS ns is in progress — subscribe and wait.
                let existing = Arc::clone(existing);

                // H-02: register the waiter BEFORE releasing the lock to close the
                // lost-wakeup window. `notify_waiters()` only wakes waiters that are
                // already registered; the leader cannot call it until it re-acquires
                // this same state lock, which we still hold here. `enable()` performs
                // the registration without first polling the future.
                let notified = existing.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                drop(st); // MUST release the lock before awaiting (never hold across await).

                // H-02 defense-in-depth: bound the wait so a missed wakeup degrades to
                // a cache re-check (below) rather than a permanent hang.
                let _ = tokio::time::timeout(std::time::Duration::from_secs(30), notified).await;

                // M-01: propagate the leader's real outcome to latecomers.
                let outcome = existing
                    .outcome
                    .lock()
                    .expect("refresh outcome mutex poisoned")
                    .clone();
                return match outcome {
                    Some(Ok(token)) => Ok(token),
                    Some(Err(msg)) => {
                        Err(anyhow::anyhow!("refresh failed upstream for '{ns}': {msg}"))
                    }
                    None => {
                        // Woke (or timed out) before the leader recorded an outcome:
                        // best-effort retry by re-checking cached state.
                        let st = self.state.lock().await;
                        st.tokens.get(ns).cloned().ok_or_else(|| {
                            anyhow::anyhow!(
                                "token for '{ns}' not refreshed (leader produced no outcome)"
                            )
                        })
                    }
                };
            }
            // We are the refresh leader for this `ns` — install the per-ns slot.
            let slot = Arc::new(RefreshSlot {
                notify: Notify::new(),
                outcome: std::sync::Mutex::new(None),
            });
            st.refresh_in_flight
                .insert(ns.to_string(), Arc::clone(&slot));
            slot
            // `st` (MutexGuard) is dropped here — lock released before the exchange.
        };

        // Step 2: Call the exchange seam WITHOUT holding the Mutex.
        // The lock is not held at this point.
        let result = (self.refresh_fn)(ns.to_string()).await;

        // Step 3: Update state, record the per-ns outcome, and notify waiters —
        // even on the error path.
        match result {
            Ok(new_token) => {
                {
                    let mut st = self.state.lock().await;
                    st.tokens.insert(ns.to_string(), new_token.clone());
                    st.refresh_in_flight.remove(ns);
                    // lock released here before save_to_disk and notify_waiters
                }
                // Record the outcome BEFORE waking latecomers so they read a fresh value.
                *slot.outcome.lock().expect("refresh outcome mutex poisoned") =
                    Some(Ok(new_token.clone()));
                // Persist outside the lock (best-effort; failures are non-fatal here).
                let _ = self.save_to_disk().await;
                // Wake all latecomers AFTER the cache + outcome are updated.
                slot.notify.notify_waiters();
                Ok(new_token)
            }
            Err(e) => {
                // Error path: clear the per-ns flag, record the failure for latecomers
                // (M-01), and wake waiters so they never hang.
                {
                    let mut st = self.state.lock().await;
                    st.refresh_in_flight.remove(ns);
                }
                *slot.outcome.lock().expect("refresh outcome mutex poisoned") =
                    Some(Err(format!("{e:#}")));
                slot.notify.notify_waiters();
                Err(e)
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Client (DCR) CRUD
    // ─────────────────────────────────────────────────────────────────────────

    /// Return the `DcrEntry` for server `url`, if present.
    pub async fn get_client(&self, url: &str) -> Option<DcrEntry> {
        let st = self.state.lock().await;
        st.clients.get(url).cloned()
    }

    /// Store a `DcrEntry` under server `url` and persist to disk.
    pub async fn put_client(&self, url: &str, entry: DcrEntry) -> Result<()> {
        {
            let mut st = self.state.lock().await;
            st.clients.insert(url.to_string(), entry);
        }
        self.save_to_disk().await
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Background refresh task — native only (wasm32 has no token expiry to schedule)
    // ─────────────────────────────────────────────────────────────────────────

    /// Spawn a background task that proactively refreshes `ns` before its token expires.
    ///
    /// Schedule (D-07, PITFALLS §A-6):
    /// - Fires ~60 seconds before expiry.
    /// - Applies a ±30-second clock-skew tolerance: treats the token as due for refresh
    ///   if `now >= expires_at - 60s - 30s` (i.e., within 90s of expiry).
    /// - If no expiry is known, polls at a 300-second default interval.
    ///
    /// Failure handling (D-03, 46.1-03): a loop-local `consecutive_failures` counter
    /// tracks the current failure streak.
    /// - The FIRST failure in a streak always `warn!`s (never suppressed).
    /// - Repeat failures in the same streak are throttled to `debug!`.
    /// - A success after a streak `warn!`s a recovery message and resets the streak.
    /// - The failure-retry interval escalates from 30s to a ~5-minute cap
    ///   (`backoff_after_failures`), then resets to the 30s base on recovery.
    /// - The success-side "fire ~60s before expiry" schedule above is UNCHANGED by
    ///   this — backoff applies only to the failure-retry path (D-03 constraint).
    ///
    /// Only compiled on native targets; wasm32 has no background tasks.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn_refresh_task(store: Arc<Self>, ns: String) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // D-03: loop-local failure streak tracker — plain `let mut`, never a
            // Mutex, never held across an `.await` (thundering-herd invariant).
            let mut consecutive_failures: u32 = 0;
            loop {
                // Compute sleep duration under the lock; release before sleeping.
                // UNCHANGED (D-03 constraint): success-path fire_at timing.
                let sleep_dur = {
                    let st = store.state.lock().await;
                    match st.tokens.get(&ns).and_then(|t| t.expires_at) {
                        None => {
                            // No expiry known — use a default poll interval.
                            std::time::Duration::from_secs(300)
                        }
                        Some(exp) => {
                            // Target: fire 60s before expiry.
                            // Skew tolerance: treat as due if now >= expires_at - 60s - 30s
                            // (PITFALLS §A-6, D-07). Both the 60s window and 30s tolerance
                            // are combined into the fire_at computation below.
                            let now = std::time::SystemTime::now();
                            let fire_at = exp
                                .checked_sub(std::time::Duration::from_secs(60 + 30))
                                .unwrap_or(exp);
                            if now >= fire_at {
                                std::time::Duration::ZERO
                            } else {
                                fire_at
                                    .duration_since(now)
                                    .unwrap_or(std::time::Duration::ZERO)
                            }
                        }
                    }
                    // MutexGuard dropped here — lock released before tokio::time::sleep.
                };

                tokio::time::sleep(sleep_dur).await;

                match store.refresh(&ns).await {
                    Ok(_) => {
                        // D-03: warn on recovery so a resolved outage stays visible,
                        // then reset the streak (and therefore the backoff) to base.
                        if consecutive_failures > 0 {
                            tracing::warn!("auth background refresh recovered for ns={ns}");
                        }
                        consecutive_failures = 0;
                    }
                    Err(e) => {
                        // Log ns + error ONLY — never log a token value; redacting Debug
                        // on TokenEntry/DcrEntry covers accidental struct formatting
                        // (PITFALLS §A-1). D-03: warn once per streak, throttle repeats
                        // to debug so a persistently-failing namespace cannot spam.
                        if should_warn_failure(consecutive_failures) {
                            tracing::warn!("auth background refresh failed for ns={ns}: {e:#}");
                        } else {
                            tracing::debug!(
                                "auth background refresh still failing for ns={ns} \
                                 (streak={consecutive_failures}): {e:#}"
                            );
                        }
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        // D-03: escalating backoff, failure-retry path only.
                        tokio::time::sleep(backoff_after_failures(consecutive_failures)).await;
                    }
                }
            }
        })
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Persistence — native arm (wasm32 no-op stubs follow)
    // ─────────────────────────────────────────────────────────────────────────

    /// Load state from disk. Returns empty `AuthState` when the file is absent.
    #[cfg(not(target_arch = "wasm32"))]
    async fn load_from_disk(path: &Path) -> Result<AuthState> {
        if !path.exists() {
            return Ok(AuthState::default());
        }
        let content = std::fs::read_to_string(path)?;
        // M-02: a present-but-unparseable file is corruption/truncation/schema-skew,
        // NOT "empty". Silently defaulting here would let the next save_to_disk
        // overwrite the recoverable file with an empty one, destroying every stored
        // credential. Preserve the bad file (rename to auth.json.corrupt-<ts>) and warn
        // before starting empty; if preservation fails, refuse rather than clobber.
        let file: AuthFile = match serde_json::from_str(&content) {
            Ok(parsed) => parsed,
            Err(e) => {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let corrupt_path = path.with_extension(format!("json.corrupt-{ts}"));
                match std::fs::rename(path, &corrupt_path) {
                    Ok(()) => {
                        tracing::warn!(
                            "auth.json at {} is present but unparseable ({e}); preserved it as \
                             {} and starting with an empty store",
                            path.display(),
                            corrupt_path.display(),
                        );
                        AuthFile::default()
                    }
                    Err(rename_err) => {
                        return Err(anyhow::anyhow!(
                            "auth.json at {} is present but unparseable ({e}), and preserving it \
                             failed ({rename_err}); refusing to start empty to avoid overwriting \
                             stored credentials",
                            path.display(),
                        ));
                    }
                }
            }
        };
        Ok(AuthState {
            tokens: file.tokens,
            clients: file.clients,
            refresh_in_flight: HashMap::new(),
        })
    }

    /// wasm32 stub — always returns an empty in-memory state (D-09).
    #[cfg(target_arch = "wasm32")]
    async fn load_from_disk(_path: &Path) -> Result<AuthState> {
        Ok(AuthState::default())
    }

    /// Atomically serialize in-memory state to `auth_json_path` at mode 0600.
    ///
    /// Write sequence (T-41-04):
    /// 1. Snapshot state under lock, then release before I/O.
    /// 2. Ensure parent directory exists; enforce 0700 unconditionally (M-03, T-41-01).
    /// 3. Create a unique temp file at mode 0600 *before* writing (H-03), fsync,
    ///    rename to final path.
    /// 4. `chmod 0600` on final path — redundant safety net (T-41-01, PITFALLS §A-1).
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn save_to_disk(&self) -> Result<()> {
        use std::io::Write as _;

        // 1. Snapshot under lock; release before blocking I/O.
        let file_data = {
            let st = self.state.lock().await;
            AuthFile {
                tokens: st.tokens.clone(),
                clients: st.clients.clone(),
            }
        };
        let json = serde_json::to_string_pretty(&file_data)?;

        // 2. Ensure parent directory exists and enforce 0700 UNCONDITIONALLY (M-03).
        //    A pre-existing ~/.ironhermes (commonly created 0755 by other subsystems)
        //    must be tightened too, not just a dir we create here — otherwise the 0700
        //    invariant silently never takes effect. set_permissions is idempotent.
        if let Some(parent) = self
            .auth_json_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }

        // 3. Atomic write: temp file (created 0600 BEFORE any content) → fsync → rename.
        //    H-03: the secret must never sit in a world-readable temp file. Create the
        //    temp file with mode 0600 and O_CREAT|O_EXCL (`create_new`) so the bytes are
        //    protected from the instant the file exists, and a pre-planted tmp/symlink is
        //    refused. A unique per-write name avoids collisions between concurrent saves.
        let counter = TMP_WRITE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp_path = self.auth_json_path.with_extension(format!(
            "json.tmp.{}.{}",
            std::process::id(),
            counter
        ));
        let write_result = (|| -> std::io::Result<()> {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp_path)?;
            f.write_all(json.as_bytes())?;
            f.flush()?;
            f.sync_all()?;
            Ok(())
        })();
        if let Err(e) = write_result {
            // Never leave a half-written temp file behind on failure.
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e.into());
        }
        std::fs::rename(&tmp_path, &self.auth_json_path)?;

        // 4. chmod 0600 on final path — redundant safety net now that the temp file is
        //    already 0600 (rename does not change mode), but kept to guarantee the
        //    invariant on the non-unix fallback and against any future write-path change.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&self.auth_json_path, std::fs::Permissions::from_mode(0o600))?;
        }

        Ok(())
    }

    /// wasm32 stub — no-op; store is in-memory only (D-09).
    #[cfg(target_arch = "wasm32")]
    pub async fn save_to_disk(&self) -> Result<()> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// D-03: failure-streak log-once + escalating backoff — pure helpers used only by
// `spawn_refresh_task`'s failure (`Err`) arm. The success-path timing above is
// untouched by these; see `spawn_refresh_task`'s doc comment.
// ─────────────────────────────────────────────────────────────────────────────

/// Base backoff between failed refresh attempts (D-03) — matches the prior fixed
/// 30-second retry so a single, isolated failure behaves exactly as before.
#[cfg(not(target_arch = "wasm32"))]
const REFRESH_FAILURE_BACKOFF_BASE: std::time::Duration = std::time::Duration::from_secs(30);

/// Escalating-backoff cap (D-03): repeated failures never wait longer than ~5 minutes.
#[cfg(not(target_arch = "wasm32"))]
const REFRESH_FAILURE_BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(300);

/// Number of 30s steps climbed before the cap takes over (10 * 30s == the 300s cap).
#[cfg(not(target_arch = "wasm32"))]
const REFRESH_FAILURE_BACKOFF_STEPS: u32 = 10;

/// Returns `true` only for the first failure in a streak (D-03: log-once-per-streak).
///
/// `consecutive_failures` is the count of failures BEFORE the current one is
/// recorded — so `0` means "this is the first failure ever seen in this streak".
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn should_warn_failure(consecutive_failures: u32) -> bool {
    consecutive_failures == 0
}

/// Escalating backoff after `consecutive_failures` failed refresh attempts (D-03).
///
/// Linear ramp from the 30s base, capped at ~5 minutes; never decreasing in
/// `consecutive_failures` and never exceeding the cap.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn backoff_after_failures(consecutive_failures: u32) -> std::time::Duration {
    let steps = consecutive_failures.clamp(1, REFRESH_FAILURE_BACKOFF_STEPS);
    (REFRESH_FAILURE_BACKOFF_BASE * steps).min(REFRESH_FAILURE_BACKOFF_CAP)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod failure_backoff_tests {
    use super::{backoff_after_failures, should_warn_failure};
    use std::time::Duration;

    #[test]
    fn should_warn_failure_only_on_first_failure_in_streak() {
        assert!(should_warn_failure(0), "first failure must always warn");
        assert!(
            !should_warn_failure(1),
            "second failure in the same streak must not warn"
        );
        assert!(
            !should_warn_failure(5),
            "repeat failures deep in a streak must not warn"
        );
    }

    #[test]
    fn backoff_after_failures_base_is_30s() {
        assert_eq!(
            backoff_after_failures(1),
            Duration::from_secs(30),
            "the base backoff (first failure) must match the prior fixed 30s retry"
        );
    }

    #[test]
    fn backoff_after_failures_is_non_decreasing_and_capped() {
        let mut prev = Duration::from_secs(0);
        for n in 1..=50u32 {
            let d = backoff_after_failures(n);
            assert!(
                d >= prev,
                "backoff must be non-decreasing in consecutive_failures (n={n}: {d:?} < {prev:?})"
            );
            assert!(
                d <= Duration::from_secs(300),
                "backoff must never exceed the ~5min cap (n={n}: {d:?})"
            );
            prev = d;
        }
        // The cap must actually be reached, not just approached.
        assert_eq!(
            backoff_after_failures(50),
            Duration::from_secs(300),
            "backoff must reach the 300s cap for a long failure streak"
        );
    }

    #[test]
    fn backoff_after_failures_zero_is_treated_as_first_failure() {
        // consecutive_failures == 0 should never be passed post-increment in
        // practice, but the helper must not panic or divide-by-zero if it is.
        assert_eq!(backoff_after_failures(0), Duration::from_secs(30));
    }
}
