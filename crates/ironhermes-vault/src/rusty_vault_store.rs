//! [`RustyVaultStore`] — the encrypted `SecretStore` backend over an in-process embedded
//! `rusty_vault::core::Core` (Phase 46.8 D-05/D-08/D-09/D-10/D-11/D-12).
//!
//! Entirely `#[cfg(feature = "rusty-vault")]` (gated at the `mod` declaration in `lib.rs`) —
//! default builds never compile this file or the `rusty_vault`/openssl dependency tree (D-10).
//!
//! # No hand-rolled crypto (D-11/D-12, RESEARCH Don't Hand-Roll)
//!
//! Shamir splitting, the AES-GCM encryption barrier, and on-disk physical storage are all
//! delegated entirely to `rusty_vault` — this module is a thin adapter over the embedded
//! `Core`.
//!
//! # `rusty_vault` 0.3.1 `Core` construction (Phase 51 Plan 01 — D-01/D-02/D-03)
//!
//! At the pinned rev (`0922e6d5e6fbe6fd2d909863eca6d583623f4ad7`) `Core` has **no external
//! lock** — every mutable field is `ArcSwap`/`ArcSwapOption` internally, so the 0.2.1 adapter's
//! `Arc<RwLock<Core>>` wrapper is gone; this module holds a plain `Arc<Core>`. There is also no
//! `Core::config(...)` call anymore (it does not exist anywhere in the 0.3.1 source — RESEARCH
//! Pitfall 1).
//!
//! **Observed construction sequence (Assumption A1 — settled by running it, not inferred):**
//! `Core::new(backend).wrap()` alone is NOT sufficient — it builds the barrier/router/mounts
//! router but leaves `Core::module_manager` completely empty, so the very first `init()` fails
//! with `ErrCoreLogicalBackendNoExist` (there is no `"kv"` logical-backend factory registered
//! yet to satisfy the default `"secret/"` mount). The missing step `Core::config(...)` used to
//! perform is `ModuleManager::set_default_modules` plus registering `AuthModule` (the crate's
//! own top-level `rusty_vault::RustyVault::new(backend, config)` — `src/lib.rs:92-147` at the
//! pinned rev — does exactly this: `set_default_modules` for Kv+System, then adds `AuthModule`
//! plus the crate's other built-in feature modules, in the crate's own canonical order. Rather
//! than hand-reimplementing that wiring (and risking silently omitting a module a future
//! `rusty_vault` bump adds), [`new_core`] calls `rusty_vault::RustyVault::new(backend, None)`
//! directly — the crate's own public, already-correct constructor — and extracts its `Arc<Core>` via
//! `rv.core.load_full()`. `RustyVault::new` internally does `Core::new(backend)` then `.wrap()`
//! itself, so `Core::self_ptr` is wired the same way either path would wire it; that
//! self-reference is load-bearing regardless — `Core::post_unseal` (called from inside
//! `unseal()`) does `self.self_ptr.upgrade().unwrap()` to hand the mounts router a strong
//! `Arc<Core>`, so a `Core` that never went through `.wrap()` would panic there.
//! `"secret/"` is still seeded as a DEFAULT `kv` mount on the *first* successful `unseal()`
//! (not on `init()`), confirmed against `secret/providers/<key>` put/get/delete/list in
//! `tests/rusty_vault_spike.rs`. The rest of `RustyVault`'s state (its `token: ArcSwap<String>`
//! field) is unused by this adapter — only the `Arc<Core>` is kept.
//!
//! # Sync-to-async bridge (Assumption A2 — Send-ness / bridging strategy)
//!
//! `Core::{inited, init, unseal}` are `async fn` upstream now (0.2.1 had them sync); `sealed()`
//! stays a plain sync `fn`. This module's own public surface (`init`, `open`, `is_sealed`,
//! `unlock`) must stay sync `pub fn` (RESEARCH/CONTEXT contract — ~15 production call sites and
//! two invariant test files assume it). [`block_on_admin_call`] bridges the three admin calls:
//! when already inside a tokio runtime it runs the actual `.block_on()` on a *separate*
//! blocking-pool thread (via `tokio::task::spawn_blocking`) and the calling thread waits on a
//! plain `std::sync::mpsc` channel — an ordinary blocking receive, not a tokio API, so it never
//! trips tokio's "Cannot start a runtime from within a runtime" panic that a direct
//! `Handle::current().block_on(...)` on the calling thread would risk if that thread happens to
//! be a runtime worker. When there is no runtime at all (`Handle::try_current()` is `Err` — a
//! plain non-async `#[test]` fn, or a call before `#[tokio::main]` starts), it builds a private
//! current-thread runtime for that one call instead of panicking (covered by
//! `init_from_outside_any_runtime` in `tests/rusty_vault_spike.rs`).
//!
//! **`Core::handle_request`'s future (used by [`run_request`], the CRUD path) — CHOSEN
//! STRATEGY: retained `spawn_blocking` + `Handle::block_on` bridge, not a direct `.await`.**
//! `tests/rusty_vault_spike.rs`'s `handle_request_future_send_outcome` applies
//! `fn assert_send<T: Send>(_: T) {}` to the future `Core::handle_request` returns and records
//! the compiler's literal answer. [`run_request`] keeps the exact same blocking-pool bridge
//! `run_request` used against 0.2.1 (isolating the call inside a dedicated blocking thread so
//! `Handle::current().block_on(...)` is always safe to call there, regardless of whether the
//! future is `Send`) — this is a correct superset either way and this migration plan is
//! explicitly scoped to prove existing behavior only, not to introduce a new async shape.
//!
//! Tokio's "run this closure in place on the current worker, promoted out of the async
//! scheduler" primitive is never used anywhere in this module (plan prohibition — the
//! `iron_hermes_ui` server polls server functions inside a per-connection `LocalSet`, where
//! that primitive panics despite a multi-thread runtime — always use `spawn_blocking` instead).
//!
//! # Tiered unseal (D-05)
//!
//! `init` performs a 1-of-1 Shamir init (`secret_shares: 1, secret_threshold: 1` —
//! RESEARCH Pitfall 6: no multi-share submission loop needed) and persists the single unseal
//! key **and** the root token together in a `0600` keyfile beside the `0700` data dir
//! (mirrors `ironhermes_core::audit::AuditLog::append`'s `OpenOptions::mode(0o600)` +
//! redundant post-write `chmod` defense-in-depth pattern). `open` with `unseal_mode ==
//! "keyfile"` reads that file and auto-unseals immediately (daemon-friendly default);
//! `unseal_mode == "passphrase"` deliberately leaves the store sealed — every CRUD call
//! naturally hard-errors via `Core::handle_request`'s own `self.sealed` check until an
//! operator calls [`RustyVaultStore::unlock`] (`ironhermes vault unlock`). Honest framing:
//! the keyfile protects against casual/backup reads and satisfies D-05's single-operator
//! posture — it does not defend against an attacker with full read access to this process's
//! filesystem while it is running (T-46.8-06).
//!
//! # Secret hygiene (D-08/D-15)
//!
//! Every value that crosses [`crate::SecretStore`] stays inside `secrecy::SecretString`; the
//! root token is *also* kept in a `SecretString` in memory (it is not part of the four-method
//! trait surface, but it is just as sensitive as the unseal key) and is only
//! `.expose_secret()`-ed at the one point building `Request::client_token`/`Request::body` —
//! never inside a `tracing`/`format!`/error string. [`KeyMaterial`] (the keyfile's on-disk
//! shape) intentionally derives neither `Debug` nor `Display`.
//!
//! # KV v1, no rotation (D-09)
//!
//! Keys live at `secret/providers/<name>` — flat KV v1, with **no** `/data/` infix anywhere
//! in the path (that KV v2 convention does not exist in this crate — RESEARCH Pitfall 1).
//! `put_secret` is an overwrite (no version history); `delete_secret`
//! on an absent key is a no-op `Ok(())` (the crate's own physical `file` backend already
//! treats a missing file as `Ok(())` on delete — no extra idempotency code needed here);
//! `list_secrets` returns sorted names (the crate's own `AESGCMBarrier::list` already sorts).

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusty_vault::RustyVault;
use rusty_vault::core::{Core, SealConfig};
use rusty_vault::errors::RvError;
use rusty_vault::logical::{Operation, Request};
use rusty_vault::storage;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use crate::SecretStore;
use crate::config::RustyVaultConfig;
use crate::error::VaultError;

/// Fixed KV v1 mount path for provider secrets (D-09: no `/data/` infix, Pitfall 1).
const SECRET_MOUNT_PREFIX: &str = "secret/providers/";

/// On-disk key material persisted in the `0600` keyfile beside the data dir (D-05).
///
/// D-15: intentionally derives neither `Debug` nor `Display` — the only two places this is
/// constructed/read ([`RustyVaultStore::init`]/[`RustyVaultStore::open`]) convert straight
/// into an in-memory `unseal()` call or a [`SecretString`], never into a log line.
#[derive(Serialize, Deserialize)]
struct KeyMaterial {
    /// Hex-encoded single Shamir key share (`secret_shares: 1, secret_threshold: 1`).
    unseal_key_hex: String,
    /// The `init()` root token, required as `Request::client_token` on every subsequent
    /// `Core::handle_request` call (RESEARCH Pitfall — `TokenStore` rejects an empty token).
    root_token: String,
}

/// Encrypted, embedded `SecretStore` backend over `rusty_vault::core::Core` (D-05/D-10).
pub struct RustyVaultStore {
    core: Arc<Core>,
    /// Kept as a `SecretString` even though it never crosses the public trait surface — it is
    /// as sensitive as the unseal key itself (D-08 defense-in-depth).
    root_token: SecretString,
}

/// Construct a fresh `Core` wired to a `file`-backed physical storage rooted at `data_dir`.
///
/// See the module doc's "Observed construction sequence" section for why this goes through
/// `rusty_vault::RustyVault::new` rather than a hand-rolled `Core::new(backend).wrap()` —
/// bare `Core::new(...).wrap()` leaves `module_manager` empty and the first `init()` fails
/// with `ErrCoreLogicalBackendNoExist`. `RustyVault::new` is the crate's own top-level
/// constructor and does the full, correct module wiring; only its `Arc<Core>` is kept here.
fn new_core(data_dir: &Path) -> Result<Arc<Core>, VaultError> {
    let mut conf: HashMap<String, JsonValue> = HashMap::new();
    conf.insert(
        "path".to_string(),
        JsonValue::String(data_dir.to_string_lossy().into_owned()),
    );
    let backend = storage::new_backend("file", &conf)
        .map_err(|e| VaultError::Backend(format!("construct file physical backend: {e}")))?;
    let rv = RustyVault::new(backend, None)
        .map_err(|e| VaultError::Backend(format!("construct rusty_vault core: {e}")))?;
    Ok(rv.core.load_full())
}

/// Map a `rusty_vault` error to our [`VaultError`], preserving the D-07 hard-error posture:
/// a sealed barrier is always surfaced as [`VaultError::Sealed`], never coerced into
/// `Ok(None)`/a generic backend error. `RvError`'s `Display` impl (via `thiserror`) only ever
/// emits static, code-shaped messages — never secret material (D-08/D-15 safe to format).
///
/// `pub(crate)`: Phase 51 Plan 09's `profile_store.rs` reuses this mapping verbatim through
/// [`run_request`] rather than duplicating it — see that module for the `ErrPermissionDenied`
/// arm's role in D-08 layer 2's "denied" outcome (T-51-51/T-51-56).
pub(crate) fn map_rv_error(e: RvError) -> VaultError {
    match e {
        RvError::ErrBarrierSealed => VaultError::Sealed,
        RvError::ErrBarrierNotInit => VaultError::NotInitialized,
        // Phase 51 Task 3 (D-08 layer 2): both an unrecognized token (TokenStore::check_token)
        // and an ACL refusal for a recognized token (PolicyStore::post_auth) surface as this
        // identical RvError variant at the pinned rev — see profile_store.rs's module doc for
        // the two call sites confirmed by direct read.
        RvError::ErrPermissionDenied => VaultError::Denied,
        other => VaultError::Backend(other.to_string()),
    }
}

/// The `0600` keyfile lives beside (a sibling of) the `0700` data dir, e.g. `.../vault` (dir)
/// and `.../vault.key` (file) — never inside the data dir itself.
fn keyfile_path(data_dir: &Path) -> PathBuf {
    let file_name = data_dir
        .file_name()
        .map(|n| format!("{}.key", n.to_string_lossy()))
        .unwrap_or_else(|| "vault.key".to_string());
    match data_dir.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(file_name),
        _ => PathBuf::from(file_name),
    }
}

/// Create (if needed) and lock down the vault data dir to `0700` — mirrors
/// `ironhermes_core::audit::AuditLog::append`'s parent-dir permission pattern.
fn ensure_dir_0700(dir: &Path) -> Result<(), VaultError> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write the keyfile with `0600` perms at creation time, plus a redundant post-write `chmod`
/// — the exact defense-in-depth idiom `ironhermes_core::audit::AuditLog::append` uses for
/// secret-adjacent files (V12).
fn write_keyfile_0600(path: &Path, material: &KeyMaterial) -> Result<(), VaultError> {
    use std::io::Write as _;

    let json = serde_json::to_string(material)
        .map_err(|e| VaultError::Backend(format!("serialize vault key material: {e}")))?;

    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(json.as_bytes())?;
    f.flush()?;

    // Redundant chmod 0600 — safety net mirroring audit.rs's own defense-in-depth pattern.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

fn read_keyfile(path: &Path) -> Result<KeyMaterial, VaultError> {
    if !path.exists() {
        return Err(VaultError::NotInitialized);
    }
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| VaultError::Backend(format!("corrupt vault key material file: {e}")))
}

/// Minimal hex codec (not cryptography — just a textual encoding for the raw key-share
/// bytes returned by `Core::init`/consumed by `Core::unseal`; no new dependency needed).
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, VaultError> {
    if !s.len().is_multiple_of(2) {
        return Err(VaultError::Backend(
            "corrupt vault key material: odd-length hex string".to_string(),
        ));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| {
                VaultError::Backend("corrupt vault key material: invalid hex digit".to_string())
            })
        })
        .collect()
}

/// T-46.8-16 (46.8-gap): reject key names that could escape the fixed
/// `secret/providers/` mount prefix when interpolated into a `rusty_vault`
/// logical-storage path — `secret_path` does no filtering of its own, so a
/// key containing `/` or `..` would otherwise reach arbitrary paths under
/// the KV mount (or, with a sufficiently pathological key, outside it).
/// Practical risk is low (D-14: operator-only surface, no chat/agent path
/// reaches `SecretStore` — T-46.8-15), but this closes the gap defensively
/// rather than relying on that boundary alone. Rejects empty keys, keys
/// containing `/`, keys containing `..`, and keys containing any control
/// character — returns a `VaultError`, never panics.
fn validate_key(key: &str) -> Result<(), VaultError> {
    if key.is_empty() {
        return Err(VaultError::InvalidKey("key must not be empty".to_string()));
    }
    if key.contains('/') {
        return Err(VaultError::InvalidKey(
            "key must not contain '/' (path traversal)".to_string(),
        ));
    }
    if key.contains("..") {
        return Err(VaultError::InvalidKey(
            "key must not contain '..' (path traversal)".to_string(),
        ));
    }
    if key.chars().any(|c| c.is_control()) {
        return Err(VaultError::InvalidKey(
            "key must not contain control characters".to_string(),
        ));
    }
    Ok(())
}

fn secret_path(key: &str) -> String {
    format!("{SECRET_MOUNT_PREFIX}{key}")
}

/// `pub(crate)`: already generalized over both the path AND the token (the parameter is named
/// `root_token` because every call site in THIS file passes the root token, but the function
/// itself has no opinion about whose token it is) — Phase 51 Plan 09's `profile_store.rs`
/// reuses this directly for both its root-authorized and token-authorized reads rather than
/// declaring a second, drift-prone builder.
pub(crate) fn read_request(path: &str, root_token: &str) -> Request {
    let mut req = Request::new(path);
    req.operation = Operation::Read;
    req.client_token = root_token.to_string();
    req
}

/// `pub(crate)` — see [`read_request`]'s doc for why this is reused as-is by
/// `profile_store.rs` rather than copied.
pub(crate) fn write_request(path: &str, value: &str, root_token: &str) -> Request {
    let mut req = Request::new(path);
    req.operation = Operation::Write;
    req.client_token = root_token.to_string();
    req.body = Some(
        json!({ "value": value })
            .as_object()
            .expect("json object literal is always a map")
            .clone(),
    );
    req
}

/// `pub(crate)` — see [`read_request`]'s doc for why this is reused as-is by
/// `profile_store.rs` rather than copied.
pub(crate) fn delete_request(path: &str, root_token: &str) -> Request {
    let mut req = Request::new(path);
    req.operation = Operation::Delete;
    req.client_token = root_token.to_string();
    req
}

/// `pub(crate)` — see [`read_request`]'s doc for why this is reused as-is by
/// `profile_store.rs` rather than copied. Phase 51 Plan 09 issues a FRESH request through this
/// builder rooted at [`crate::profile_paths::profile_secret_prefix`] — never through
/// [`RustyVaultStore`]'s own [`SecretStore::list_secrets`], whose list root is the fixed
/// [`SECRET_MOUNT_PREFIX`] constant and cannot reach `secret/profiles/` at any `path` argument.
pub(crate) fn list_request(path: &str, root_token: &str) -> Request {
    let mut req = Request::new(path);
    req.operation = Operation::List;
    req.client_token = root_token.to_string();
    req
}

/// Bridge one of `Core`'s async admin methods (`inited`/`init`/`unseal`) into this module's
/// sync public API. See the module-level "Sync-to-async bridge" doc section for the full
/// rationale — this never calls `Handle::block_on` on the thread that called it (which may
/// itself already be a tokio runtime worker), and never promotes that closure out of the
/// async scheduler in place (the `LocalSet`-unsafe alternative this module deliberately
/// avoids — always `spawn_blocking` instead).
fn block_on_admin_call<F, Fut, T>(f: F) -> Result<T, VaultError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, RvError>> + Send + 'static,
    T: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            let (tx, rx) = std::sync::mpsc::channel();
            handle.spawn_blocking(move || {
                let result = tokio::runtime::Handle::current().block_on(f());
                let _ = tx.send(result);
            });
            rx.recv()
                .map_err(|_| {
                    VaultError::Backend(
                        "vault admin task ended without sending a result".to_string(),
                    )
                })?
                .map_err(map_rv_error)
        }
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| VaultError::Backend(format!("build private tokio runtime: {e}")))?;
            rt.block_on(f()).map_err(map_rv_error)
        }
    }
}

/// Run one `Core::handle_request` call on a blocking-pool thread and return the response's
/// `data` map (`None` when the backend returned no `Response` at all, e.g. `get_secret` on a
/// missing key). See the module-level doc section for why this bridge is kept rather than
/// replaced with a direct `.await` (Assumption A2's recorded outcome).
///
/// `pub(crate)`: Phase 51 Plan 09's `profile_store.rs` reuses this exact bridge for the
/// profile-scoped address space rather than inventing a second one — see [`read_request`]'s
/// doc for the shared-builder rationale.
pub(crate) async fn run_request(
    core: Arc<Core>,
    build: impl FnOnce() -> Request + Send + 'static,
) -> Result<Option<serde_json::Map<String, JsonValue>>, VaultError> {
    tokio::task::spawn_blocking(move || {
        let mut req = build();
        let resp = tokio::runtime::Handle::current()
            .block_on(core.handle_request(&mut req))
            .map_err(map_rv_error)?;
        Ok(resp.and_then(|r| r.data))
    })
    .await
    .map_err(|e| VaultError::Backend(format!("blocking vault task panicked: {e}")))?
}

impl RustyVaultStore {
    /// Create a brand-new vault at `config.data_dir`: `0700` data dir, 1-of-1 Shamir init
    /// (D-05), `0600` keyfile holding the unseal key + root token. Refuses to re-init an
    /// already-initialized data dir (`Core::inited()` — the crate's own check, not a
    /// file-existence heuristic we'd have to keep in sync).
    ///
    /// Does **not** mount `"secret/"` explicitly and does **not** unseal — the "secret/" KV
    /// mount is a DEFAULT mount seeded by `post_unseal()` the first time `unseal()` runs (i.e.
    /// on the first `unseal()`, not on `init()`). Leaving the store sealed after `init()` also
    /// keeps D-05's tiered-unseal semantics consistent regardless of which `unseal_mode` a
    /// later `open()` uses.
    pub fn init(config: &RustyVaultConfig) -> Result<(), VaultError> {
        ensure_dir_0700(&config.data_dir)?;

        let core = new_core(&config.data_dir)?;

        let already_inited = {
            let core = Arc::clone(&core);
            block_on_admin_call(move || async move { core.inited().await })?
        };
        if already_inited {
            return Err(VaultError::Backend(format!(
                "vault already initialized at {}",
                config.data_dir.display()
            )));
        }

        let seal_config = SealConfig {
            secret_shares: 1,
            secret_threshold: 1,
        };
        let init_result = {
            let core = Arc::clone(&core);
            let seal_config = seal_config.clone();
            block_on_admin_call(move || async move { core.init(&seal_config).await })?
        };
        assert_eq!(
            init_result.secret_shares.len(),
            1,
            "secret_shares:1/secret_threshold:1 must yield exactly one key share"
        );

        let material = KeyMaterial {
            unseal_key_hex: to_hex(&init_result.secret_shares[0]),
            root_token: init_result.root_token.clone(),
        };
        write_keyfile_0600(&keyfile_path(&config.data_dir), &material)?;

        Ok(())
    }

    /// Load an existing vault at `config.data_dir`. `unseal_mode == "keyfile"` (default)
    /// reads the `0600` keyfile and auto-unseals immediately (single `unseal(key)` call,
    /// `Ok(true)` on the first submission — RESEARCH Pitfall 6, 1-of-1 Shamir). `unseal_mode
    /// == "passphrase"` deliberately does not auto-unseal; the returned store is sealed and
    /// every `SecretStore` CRUD call will hard-error with `VaultError::Sealed` until
    /// [`RustyVaultStore::unlock`] is called.
    pub fn open(config: &RustyVaultConfig) -> Result<Self, VaultError> {
        let material = read_keyfile(&keyfile_path(&config.data_dir))?;

        let core = new_core(&config.data_dir)?;

        let inited = {
            let core = Arc::clone(&core);
            block_on_admin_call(move || async move { core.inited().await })?
        };
        if !inited {
            return Err(VaultError::NotInitialized);
        }

        match config.unseal_mode.as_str() {
            "keyfile" => {
                let key = from_hex(&material.unseal_key_hex)?;
                let core = Arc::clone(&core);
                let unsealed = block_on_admin_call(move || async move { core.unseal(&key).await })?;
                if !unsealed {
                    return Err(VaultError::Backend(
                        "unseal() did not complete on the first key submission (expected \
                         immediate Ok(true) for 1-of-1 Shamir)"
                            .to_string(),
                    ));
                }
            }
            "passphrase" => {
                // Leave sealed — D-05 tiered unseal: the operator must call `unlock()`
                // (`ironhermes vault unlock`) before any CRUD call will succeed.
            }
            other => {
                return Err(VaultError::Backend(format!(
                    "unknown vault.rusty_vault.unseal_mode {other:?} — expected \
                     \"keyfile\" or \"passphrase\""
                )));
            }
        }

        Ok(Self {
            core,
            root_token: SecretString::from(material.root_token),
        })
    }

    /// Async-safe counterpart to [`RustyVaultStore::open`] (Phase 51 Plan 17, CR-05 /
    /// T-51-67): runs the ENTIRE synchronous `open` — including
    /// [`block_on_admin_call`]'s blocking `rx.recv()` bridge — on a blocking-pool thread via
    /// [`tokio::task::spawn_blocking`], so an async caller's OWN thread (a runtime worker, or
    /// worse, one of `iron_hermes_ui`'s per-connection `LocalSet` threads) never blocks on it
    /// directly. Two pieces of in-tree evidence record why this matters:
    /// `rusty_vault_feature_reaches_bot_spawn.rs`'s doc, which needs `flavor =
    /// "multi_thread"` specifically because the default current-thread flavor deadlocks on
    /// this bridge, and `cli_handoff.rs::bot_credential_holder`'s own `spawn_blocking` wrap
    /// around `host_profile_credentials`, added after reproducing 240-second hangs. That
    /// mitigation was hand-applied to one call site; this makes the property true BY
    /// CONSTRUCTION for every `.await`ed caller instead.
    ///
    /// Returns the identical `Result` [`RustyVaultStore::open`] would for the same config on
    /// every outcome — success, sealed (`unseal_mode == "passphrase"`), uninitialized, or an
    /// opaque backend error. A panic inside the blocking task (a `JoinError`) is mapped to
    /// [`VaultError::Backend`] rather than unwrapped or propagated as a join panic — the same
    /// mapping [`crate::profile_token::run_auth_request`] already uses for the identical
    /// class of failure.
    ///
    /// **Async callers must use this, not [`RustyVaultStore::open`].** The synchronous `open`
    /// stays exactly as-is and is still required: the CLI's `vault` subcommands, the profile
    /// migration's preflight, and `doctor`'s diagnostics are plain non-async call sites (or
    /// one-shot commands where briefly blocking their own single task is not a correctness
    /// hazard the way it is for a long-lived server), and [`block_on_admin_call`]'s "outside
    /// any runtime" branch (`init_from_outside_any_runtime` pins this) depends on `open`
    /// being callable with no tokio runtime present at all — `open_async` cannot substitute
    /// there, since `spawn_blocking` itself requires a runtime to schedule onto.
    pub async fn open_async(config: &RustyVaultConfig) -> Result<Self, VaultError> {
        let config = config.clone();
        tokio::task::spawn_blocking(move || Self::open(&config))
            .await
            .map_err(|e| VaultError::Backend(format!("vault open task panicked: {e}")))?
    }

    /// WR-01 (46.8-gap): report the underlying `Core`'s ACTUAL seal state, rather than
    /// inferring it from whether [`RustyVaultStore::open`] returned `Ok`/`Err`. `open()`
    /// returns `Ok` in `unseal_mode == "passphrase"` while deliberately leaving the store
    /// sealed (see the module-level "Tiered unseal" doc section), so callers that need to
    /// know the real seal state (e.g. `ironhermes doctor`) must call this instead of
    /// treating a successful `open()` as proof of "unsealed". `Core::sealed()` is a plain
    /// sync `fn` at 0.3.1 too — no bridge needed.
    pub fn is_sealed(&self) -> Result<bool, VaultError> {
        Ok(self.core.sealed())
    }

    /// Submit the single unseal key (hex-encoded, matching [`RustyVaultStore::init`]'s
    /// keyfile format) to fully unseal a store opened in `unseal_mode == "passphrase"`.
    pub fn unlock(&self, passphrase_or_key: &str) -> Result<(), VaultError> {
        let key = from_hex(passphrase_or_key)?;
        let core = Arc::clone(&self.core);
        let unsealed = block_on_admin_call(move || async move { core.unseal(&key).await })?;
        if !unsealed {
            return Err(VaultError::Backend(
                "unseal() did not complete on the first key submission (expected immediate \
                 Ok(true) for 1-of-1 Shamir)"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Crate-visible accessor for [`crate::profile_store::ProfileSecretStore::from_rusty_vault_store`]
    /// — a caller that already opened this store should not open a SECOND `Core` against the
    /// same data dir. `pub(crate)` rather than a widening of the public surface Plan 01 holds
    /// fixed (D-06): this is not part of the ~15-call-site `open_store`/`RustyVaultStore`
    /// contract, only an internal seam between this module and `profile_store.rs`.
    pub(crate) fn core_handle(&self) -> Arc<Core> {
        Arc::clone(&self.core)
    }

    /// Crate-visible accessor for the same reason as [`RustyVaultStore::core_handle`] — hands
    /// back a fresh [`SecretString`] copy of the root token (not the live field) so the two
    /// stores never share a reference to the same secret-string allocation.
    pub(crate) fn root_token_secret(&self) -> SecretString {
        SecretString::from(self.root_token.expose_secret().to_string())
    }
}

#[async_trait::async_trait]
impl SecretStore for RustyVaultStore {
    async fn get_secret(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
        validate_key(key)?;
        let path = secret_path(key);
        let root_token = self.root_token.expose_secret().to_string();
        let core = Arc::clone(&self.core);

        let data = run_request(core, move || read_request(&path, &root_token)).await?;
        let Some(data) = data else {
            return Ok(None);
        };
        let value = data
            .get("value")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| VaultError::Backend("kv entry missing \"value\" field".to_string()))?;
        Ok(Some(SecretString::from(value.to_string())))
    }

    async fn put_secret(&self, key: &str, value: SecretString) -> anyhow::Result<()> {
        validate_key(key)?;
        let path = secret_path(key);
        let root_token = self.root_token.expose_secret().to_string();
        // D-08 boundary: expose only to build the request body handed to the vault backend —
        // never into a log/Debug/error string.
        let raw_value = value.expose_secret().to_string();
        let core = Arc::clone(&self.core);

        run_request(core, move || write_request(&path, &raw_value, &root_token)).await?;
        Ok(())
    }

    async fn delete_secret(&self, key: &str) -> anyhow::Result<()> {
        validate_key(key)?;
        let path = secret_path(key);
        let root_token = self.root_token.expose_secret().to_string();
        let core = Arc::clone(&self.core);

        // The physical `file` backend already treats deleting an absent key as `Ok(())`
        // (D-09 empty edge) — no extra idempotency handling needed here.
        run_request(core, move || delete_request(&path, &root_token)).await?;
        Ok(())
    }

    async fn list_secrets(&self, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        let root_token = self.root_token.expose_secret().to_string();
        let core = Arc::clone(&self.core);

        let data =
            run_request(core, move || list_request(SECRET_MOUNT_PREFIX, &root_token)).await?;
        let Some(data) = data else {
            return Ok(Vec::new());
        };
        let keys = data
            .get("keys")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| VaultError::Backend("kv list missing \"keys\" field".to_string()))?;

        // D-09 ordering: the crate's own `AESGCMBarrier::list` already sorts, but sort again
        // explicitly after prefix-filtering so this is never accidentally dependent on that
        // upstream implementation detail.
        let mut names: Vec<String> = keys
            .iter()
            .filter_map(JsonValue::as_str)
            .map(str::to_string)
            .filter(|name| match prefix {
                Some(p) => name.starts_with(p),
                None => true,
            })
            .collect();
        names.sort();
        Ok(names)
    }
}
