//! Approval store — session and persistent approval records for exec guardrails.
//!
//! - **Session approvals** (`session`): in-process memory only; cleared on restart.
//! - **Always approvals** (`always`): persisted to `get_hermes_home()/approvals.json` at
//!   mode 0600 via the atomic auth.rs write posture
//!   (tmp `create_new` 0600 → `fsync` → `rename` → redundant `chmod` 0600).
//!
//! # Key-Kind namespaces (D-02)
//!
//! - [`KeyKind::Name`]: Quick Command name (by_name) — keyed on the declared command name.
//! - [`KeyKind::Command`]: exact normalized ad-hoc command string (by_command) — keyed on
//!   `normalize_command(raw)`.
//!
//! The two namespaces are strictly independent: approving `"curl example.com"` by_command
//! does NOT approve a Quick Command named `"curl"` by_name, and vice versa.
//!
//! # Normalization (D-02)
//!
//! `normalize_command(raw)` trims leading/trailing whitespace and collapses all internal
//! whitespace runs to a single space. Paths are case-sensitive: the function does NOT
//! lowercase the result.
//!
//! # Threat surface
//!
//! `approvals.json` is protected by the 0600 write posture (T-42-11). A corrupt/truncated
//! file triggers a rename-and-warn fallback (mirrors `AuthStore::load_from_disk`). Never
//! hardcode `~/.ironhermes` — always call `get_hermes_home()` which reads `IRONHERMES_HOME`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::constants::get_hermes_home;

static TMP_WRITE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const APPROVALS_FILENAME: &str = "approvals.json";

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

/// Which namespace a cache key belongs to.
///
/// - `Name`: Quick Command name (by_name) — D-02.
/// - `Command`: exact normalized ad-hoc command string (by_command) — D-02.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// Quick Command name (by_name namespace).
    Name,
    /// Exact normalized ad-hoc command string (by_command namespace).
    Command,
}

/// Per-entry metadata stored in `approvals.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalEntry {
    /// Unix timestamp (seconds) when this approval was granted.
    pub approved_at: i64,
}

/// The `always` sub-object with separate `by_name` and `by_command` maps.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlwaysApprovals {
    /// Keys: Quick Command names (D-02 by_name).
    #[serde(default)]
    pub by_name: HashMap<String, ApprovalEntry>,
    /// Keys: exact normalized command strings (D-02 by_command).
    #[serde(default)]
    pub by_command: HashMap<String, ApprovalEntry>,
}

/// On-disk schema for `approvals.json` (version 1).
///
/// ```json
/// { "version": 1, "always": { "by_name": {}, "by_command": {} } }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalsState {
    /// Schema version. Currently always `1`.
    pub version: u32,
    /// Persisted always-approvals split by namespace.
    #[serde(default)]
    pub always: AlwaysApprovals,
}

impl Default for ApprovalsState {
    fn default() -> Self {
        Self {
            version: 1,
            always: AlwaysApprovals::default(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ApprovalsStore
// ─────────────────────────────────────────────────────────────────────────────

/// In-process + persistent approval store.
///
/// Session approvals survive only for the current process lifetime. Always approvals
/// are serialised to `get_hermes_home()/approvals.json` by [`save_to_disk`] using
/// the atomic 0600 write posture from `auth.rs`.
///
/// [`save_to_disk`]: ApprovalsStore::save_to_disk
pub struct ApprovalsStore {
    /// Path to `approvals.json` (resolved via `get_hermes_home()` or a test override).
    path: PathBuf,
    /// Persisted always-approval state (in memory until `save_to_disk` is called).
    state: Arc<Mutex<ApprovalsState>>,
    /// In-process session approvals — never written to disk.
    session: Arc<Mutex<HashSet<String>>>,
}

/// Manual Debug — never expose approval keys, path, or session contents in derived output.
///
/// Mirrors `AuthStore`'s discipline: only the path is shown; everything else is opaque.
impl std::fmt::Debug for ApprovalsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalsStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ApprovalsStore {
    // ─────────────────────────────────────────────────────────────────────────
    // Construction
    // ─────────────────────────────────────────────────────────────────────────

    /// Create a new, empty in-memory store at the canonical path.
    ///
    /// Resolves the path via [`get_hermes_home()`] (reads `IRONHERMES_HOME`; never
    /// hardcodes `~/.ironhermes`). No disk I/O; call [`load`] to restore persisted state.
    ///
    /// [`get_hermes_home()`]: crate::constants::get_hermes_home
    /// [`load`]: ApprovalsStore::load
    pub fn new() -> Self {
        Self::with_path(get_hermes_home().join(APPROVALS_FILENAME))
    }

    /// Create a new, empty in-memory store at an explicit path.
    ///
    /// Primarily for tests that want to point at a temp directory.
    pub fn with_path(path: PathBuf) -> Self {
        Self {
            path,
            state: Arc::new(Mutex::new(ApprovalsState::default())),
            session: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Load the store from the canonical path (`get_hermes_home()/approvals.json`),
    /// restoring persisted `always` approvals.
    ///
    /// A missing file returns an empty store (first-run normal).
    /// A corrupt/unparseable file is renamed to `approvals.json.corrupt-<ts>`, a warning
    /// is logged, and the store starts empty — mirrors `AuthStore::load_from_disk`.
    pub async fn load() -> Self {
        Self::load_from_path(get_hermes_home().join(APPROVALS_FILENAME)).await
    }

    /// Load from a specific path — used in tests pointing at a temp directory.
    pub async fn load_from_path(path: PathBuf) -> Self {
        let state = Self::load_state_from_disk(&path).await;
        Self {
            path,
            state: Arc::new(Mutex::new(state)),
            session: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Command normalization (D-02)
    // ─────────────────────────────────────────────────────────────────────────

    /// Produce a stable `by_command` cache key from a raw command string.
    ///
    /// Rules (D-02):
    /// - trim leading/trailing ASCII whitespace
    /// - collapse all internal whitespace runs to a single space
    /// - NOT lowercased (paths are case-sensitive)
    pub fn normalize_command(raw: &str) -> String {
        raw.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Session (in-memory, not persisted — D-01)
    // ─────────────────────────────────────────────────────────────────────────

    /// `true` if `cache_key` was approved for this session (cleared on process exit).
    pub async fn is_session_approved(&self, cache_key: &str) -> bool {
        self.session.lock().await.contains(cache_key)
    }

    /// Record a session approval for `cache_key`. Not written to disk.
    pub async fn approve_session(&self, cache_key: &str) {
        self.session.lock().await.insert(cache_key.to_owned());
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Always (persisted — D-01)
    // ─────────────────────────────────────────────────────────────────────────

    /// `true` if `cache_key` is in the `always` store for the given `kind`.
    pub async fn is_always_approved(&self, cache_key: &str, kind: KeyKind) -> bool {
        let st = self.state.lock().await;
        match kind {
            KeyKind::Name => st.always.by_name.contains_key(cache_key),
            KeyKind::Command => st.always.by_command.contains_key(cache_key),
        }
    }

    /// Record a persistent approval for `cache_key` in the in-memory state.
    ///
    /// **Call [`save_to_disk`] afterwards** to persist across restarts.
    ///
    /// [`save_to_disk`]: ApprovalsStore::save_to_disk
    pub async fn approve_always(&self, cache_key: &str, kind: KeyKind) {
        let approved_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let entry = ApprovalEntry { approved_at };
        let mut st = self.state.lock().await;
        match kind {
            KeyKind::Name => {
                st.always.by_name.insert(cache_key.to_owned(), entry);
            }
            KeyKind::Command => {
                st.always.by_command.insert(cache_key.to_owned(), entry);
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Persistence — native arm (wasm32 no-op stubs follow)
    // ─────────────────────────────────────────────────────────────────────────

    /// Load state from disk. Returns [`ApprovalsState::default`] when the file is absent.
    ///
    /// On corrupt/unparseable files: renames the bad file to `approvals.json.corrupt-<ts>`
    /// and logs a warning before returning an empty state. Mirrors `AuthStore::load_from_disk`.
    #[cfg(not(target_arch = "wasm32"))]
    async fn load_state_from_disk(path: &Path) -> ApprovalsState {
        if !path.exists() {
            return ApprovalsState::default();
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "approvals: failed to read {} ({e}); starting with empty store",
                    path.display()
                );
                return ApprovalsState::default();
            }
        };
        match serde_json::from_str::<ApprovalsState>(&content) {
            Ok(state) => state,
            Err(e) => {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let corrupt_path = path.with_extension(format!("json.corrupt-{ts}"));
                match std::fs::rename(path, &corrupt_path) {
                    Ok(()) => {
                        tracing::warn!(
                            "approvals.json at {} is present but unparseable ({e}); preserved as \
                             {} and starting with empty store",
                            path.display(),
                            corrupt_path.display(),
                        );
                    }
                    Err(rename_err) => {
                        tracing::warn!(
                            "approvals.json at {} is unparseable ({e}) and could not be preserved \
                             ({rename_err}); starting with empty store",
                            path.display(),
                        );
                    }
                }
                ApprovalsState::default()
            }
        }
    }

    /// wasm32 stub — always returns empty state; store is in-memory only.
    #[cfg(target_arch = "wasm32")]
    async fn load_state_from_disk(_path: &Path) -> ApprovalsState {
        ApprovalsState::default()
    }

    /// Atomically write the in-memory `always` state to `approvals.json` at mode 0600.
    ///
    /// Mirrors `AuthStore::save_to_disk()` exactly (T-42-11, H-03):
    /// 1. Snapshot state under lock; release lock before I/O.
    /// 2. Ensure parent directory exists; enforce 0700 unconditionally (M-03).
    /// 3. Atomic write: unique tmp file at 0600 (`create_new`) → write → flush → `sync_all`
    ///    → `rename` to final path.
    /// 4. Redundant `chmod 0600` on the final path (safety net against rename mode reset).
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn save_to_disk(&self) -> Result<()> {
        use std::io::Write as _;

        // 1. Snapshot under lock; release before blocking I/O.
        let snapshot = {
            let st = self.state.lock().await;
            st.clone()
        };
        let json = serde_json::to_string_pretty(&snapshot)?;

        // 2. Ensure parent directory exists and enforce 0700 unconditionally (M-03).
        //    Pre-existing directories created 0755 by other subsystems must also be
        //    tightened — set_permissions is idempotent.
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }

        // 3. Atomic write: unique tmp at 0600 BEFORE writing → fsync → rename (H-03).
        //    create_new + mode 0600 ensures the secret never sits in a world-readable
        //    temp file and rejects a pre-planted tmp/symlink.
        let counter = TMP_WRITE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp_path =
            self.path
                .with_extension(format!("json.tmp.{}.{}", std::process::id(), counter));
        // WR-02: track whether *this* code path created the tmp file.
        // If open() fails with EEXIST (another writer owns the path), we must
        // NOT call remove_file — we don't own that file. The `created` flag is
        // set to `true` only AFTER a successful open(), so cleanup is gated on
        // actual ownership.
        let mut created = false;
        let write_result = (|| -> std::io::Result<()> {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp_path)?;
            created = true; // only set AFTER open() succeeds — we own the file now
            f.write_all(json.as_bytes())?;
            f.flush()?;
            f.sync_all()?;
            Ok(())
        })();
        if let Err(e) = write_result {
            // Never leave a half-written temp file behind on failure —
            // but only remove if we created it (WR-02: guard against EEXIST path).
            if created {
                let _ = std::fs::remove_file(&tmp_path);
            }
            return Err(e.into());
        }
        std::fs::rename(&tmp_path, &self.path)?;

        // 4. Redundant chmod 0600 on final path — safety net (T-42-11, PITFALLS §A-1).
        //    rename(2) preserves the source permissions (0600 from step 3) but this
        //    redundant call guards against any future change to the write path.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }

        Ok(())
    }

    /// wasm32 stub — no-op; store is in-memory only.
    #[cfg(target_arch = "wasm32")]
    pub async fn save_to_disk(&self) -> Result<()> {
        Ok(())
    }
}

impl Default for ApprovalsStore {
    fn default() -> Self {
        Self::new()
    }
}
