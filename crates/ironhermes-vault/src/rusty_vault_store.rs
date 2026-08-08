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
//! `Core`, calling exactly the API shape Plan 02's spike test (`tests/rusty_vault_spike.rs`)
//! proved against the real `=0.2.1` crate: `Core` has no `Core::new`, `"secret/"` is a DEFAULT
//! `kv` mount (no manual `mount()` call), and every request needs `client_token` set to the
//! `init()` root token.
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
//!
//! # `Core::handle_request` bridged via `spawn_blocking` (not held across `.await`)
//!
//! `Core::handle_request` is `async fn(&self, ...)`, but its own internal implementation
//! holds a `std::sync::RwLockReadGuard<Vec<Arc<dyn Handler>>>` (`self.handlers.read()?`)
//! across its own internal await points — this makes the future it returns **not `Send`**,
//! independent of anything this module does at the call site. Since [`crate::SecretStore`] is
//! `Send + Sync` (`async_trait` boxes each method as `Pin<Box<dyn Future<Output = ..> + Send>>`
//! by default), directly `.await`-ing `handle_request` from a `SecretStore` method would fail
//! to compile. Every CRUD method therefore runs the whole synchronous-in-practice request
//! (lock + build + `handle_request`) inside `tokio::task::spawn_blocking` +
//! `tokio::runtime::Handle::block_on` (see [`run_request`]) — this isolates the non-`Send`
//! inner future and the `std::sync::RwLockReadGuard` entirely within a dedicated blocking-pool
//! thread, so no lock is ever held across an `.await` point in *this* module's async fns (no
//! `#[allow(clippy::await_holding_lock)]` needed). Plan 02's spike confirmed no genuine
//! executor yield occurs inside `handle_request` for the `file` backend (synchronous I/O
//! only), so blocking on it this way is safe and does not starve the async runtime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use rusty_vault::core::{Core, SealConfig};
use rusty_vault::errors::RvError;
use rusty_vault::logical::{Operation, Request};
use rusty_vault::storage::{self, barrier_aes_gcm};
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
    core: Arc<RwLock<Core>>,
    /// Kept as a `SecretString` even though it never crosses the public trait surface — it is
    /// as sensitive as the unseal key itself (D-08 defense-in-depth).
    root_token: SecretString,
}

/// Construct a fresh `Core` wired to a `file`-backed physical storage rooted at `data_dir`,
/// with an AES-GCM encryption barrier over it.
///
/// CORRECTED vs RESEARCH's Code-Examples sketch (Plan 02 spike, reused verbatim here):
/// `Core` has no `Core::new(...)` constructor — build the physical backend + barrier by hand,
/// then construct `Core { physical, barrier, ..Default::default() }`.
fn new_core(data_dir: &Path) -> Result<Arc<RwLock<Core>>, VaultError> {
    let mut conf: HashMap<String, JsonValue> = HashMap::new();
    conf.insert(
        "path".to_string(),
        JsonValue::String(data_dir.to_string_lossy().into_owned()),
    );
    let backend = storage::new_backend("file", &conf)
        .map_err(|e| VaultError::Backend(format!("construct file physical backend: {e}")))?;
    let barrier = barrier_aes_gcm::AESGCMBarrier::new(Arc::clone(&backend));
    Ok(Arc::new(RwLock::new(Core {
        physical: backend,
        barrier: Arc::new(barrier),
        ..Default::default()
    })))
}

/// Map a `rusty_vault` error to our [`VaultError`], preserving the D-07 hard-error posture:
/// a sealed barrier is always surfaced as [`VaultError::Sealed`], never coerced into
/// `Ok(None)`/a generic backend error. `RvError`'s `Display` impl (via `thiserror`) only ever
/// emits static, code-shaped messages — never secret material (D-08/D-15 safe to format).
fn map_rv_error(e: RvError) -> VaultError {
    match e {
        RvError::ErrBarrierSealed => VaultError::Sealed,
        RvError::ErrBarrierNotInit => VaultError::NotInitialized,
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

fn read_request(path: &str, root_token: &str) -> Request {
    let mut req = Request::new(path);
    req.operation = Operation::Read;
    req.client_token = root_token.to_string();
    req
}

fn write_request(path: &str, value: &str, root_token: &str) -> Request {
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

fn delete_request(path: &str, root_token: &str) -> Request {
    let mut req = Request::new(path);
    req.operation = Operation::Delete;
    req.client_token = root_token.to_string();
    req
}

fn list_request(path: &str, root_token: &str) -> Request {
    let mut req = Request::new(path);
    req.operation = Operation::List;
    req.client_token = root_token.to_string();
    req
}

/// Run one `Core::handle_request` call on a blocking-pool thread (see the module-level "not
/// held across `.await`" doc section for why) and return the response's `data` map (`None`
/// when the backend returned no `Response` at all, e.g. `get_secret` on a missing key).
async fn run_request(
    core: Arc<RwLock<Core>>,
    build: impl FnOnce() -> Request + Send + 'static,
) -> Result<Option<serde_json::Map<String, JsonValue>>, VaultError> {
    tokio::task::spawn_blocking(move || {
        let c = core
            .read()
            .map_err(|_| VaultError::Backend("core read lock poisoned".to_string()))?;
        let mut req = build();
        let resp = tokio::runtime::Handle::current()
            .block_on(c.handle_request(&mut req))
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
    /// Does **not** mount `"secret/"` explicitly and does **not** unseal — Plan 02's spike
    /// confirmed `"secret/"` is a DEFAULT `kv` mount seeded by `DEFAULT_CORE_MOUNTS` the
    /// first time `post_unseal()` runs (i.e. on the first `unseal()`, not on `init()`), and a
    /// manual `core.mount()` call for that path fails with `ErrMountPathExist`. Leaving the
    /// store sealed after `init()` also keeps D-05's tiered-unseal semantics consistent
    /// regardless of which `unseal_mode` a later `open()` uses.
    pub fn init(config: &RustyVaultConfig) -> Result<(), VaultError> {
        ensure_dir_0700(&config.data_dir)?;

        let core = new_core(&config.data_dir)?;
        let mut c = core
            .write()
            .map_err(|_| VaultError::Backend("core write lock poisoned".to_string()))?;

        if c.inited().map_err(map_rv_error)? {
            return Err(VaultError::Backend(format!(
                "vault already initialized at {}",
                config.data_dir.display()
            )));
        }

        c.config(Arc::clone(&core), None)
            .map_err(|e| VaultError::Backend(format!("core.config(): {e}")))?;

        let seal_config = SealConfig {
            secret_shares: 1,
            secret_threshold: 1,
        };
        let init_result = c.init(&seal_config).map_err(map_rv_error)?;
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
        {
            let mut c = core
                .write()
                .map_err(|_| VaultError::Backend("core write lock poisoned".to_string()))?;

            if !c.inited().map_err(map_rv_error)? {
                return Err(VaultError::NotInitialized);
            }

            c.config(Arc::clone(&core), None)
                .map_err(|e| VaultError::Backend(format!("core.config(): {e}")))?;

            match config.unseal_mode.as_str() {
                "keyfile" => {
                    let key = from_hex(&material.unseal_key_hex)?;
                    let unsealed = c.unseal(&key).map_err(map_rv_error)?;
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
        }

        Ok(Self {
            core,
            root_token: SecretString::from(material.root_token),
        })
    }

    /// WR-01 (46.8-gap): report the underlying `Core`'s ACTUAL seal state, rather than
    /// inferring it from whether [`RustyVaultStore::open`] returned `Ok`/`Err`. `open()`
    /// returns `Ok` in `unseal_mode == "passphrase"` while deliberately leaving the store
    /// sealed (see the module-level "Tiered unseal" doc section), so callers that need to
    /// know the real seal state (e.g. `ironhermes doctor`) must call this instead of
    /// treating a successful `open()` as proof of "unsealed".
    pub fn is_sealed(&self) -> Result<bool, VaultError> {
        let c = self
            .core
            .read()
            .map_err(|_| VaultError::Backend("core read lock poisoned".to_string()))?;
        Ok(c.sealed())
    }

    /// Submit the single unseal key (hex-encoded, matching [`RustyVaultStore::init`]'s
    /// keyfile format) to fully unseal a store opened in `unseal_mode == "passphrase"`.
    pub fn unlock(&self, passphrase_or_key: &str) -> Result<(), VaultError> {
        let key = from_hex(passphrase_or_key)?;
        let mut c = self
            .core
            .write()
            .map_err(|_| VaultError::Backend("core write lock poisoned".to_string()))?;
        let unsealed = c.unseal(&key).map_err(map_rv_error)?;
        if !unsealed {
            return Err(VaultError::Backend(
                "unseal() did not complete on the first key submission (expected immediate \
                 Ok(true) for 1-of-1 Shamir)"
                    .to_string(),
            ));
        }
        Ok(())
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
