//! Phase 54.13: the raw `config.yaml` editor's server surface.
//!
//! D-01 read + salted digest, D-03 non-exiting validation, D-04 default-route
//! server fns (the global `require_auth` layer covers them), D-05 read-only
//! gate (the ROOT `security.web_config_write_enabled`), D-05a path
//! confinement, D-07/D-08/D-09/D-09a secrets never reach the browser.
//!
//! # Test reachability
//!
//! The in-file tests never call `global_app_state()` (G-50.2-2b precedent):
//! every helper here is a plain fn over a scope and the filesystem.
//!
//! # Target gating
//!
//! The DTOs compile on both targets and carry no `ironhermes_core` types
//! (`ironhermes-core` is native-only). Every native helper is
//! `#[cfg(feature = "server")]`, and the server-fn bodies use the
//! `list_profiles` split.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::server::tools_config_api::ConfigScope;

/// A successfully loaded (masked) config file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConfigYamlRead {
    /// The file text with secrets hidden (D-08).
    pub text: String,
    /// Salted sha256 of the raw on-disk bytes (D-01).
    pub sha256: String,
    /// The ROOT `security.web_config_write_enabled` gate (D-05).
    pub write_enabled: bool,
    /// Home-relative display path; never an absolute host path.
    pub display_path: String,
}

/// `get_config_yaml` outcome.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ConfigYamlLoad {
    Loaded(ConfigYamlRead),
    Unavailable { reason: String },
}

/// A validation problem with a 1-based position when the parser gave one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConfigYamlIssue {
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// `check_config_yaml` outcome (D-06/D-12).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum CheckOutcome {
    /// Valid; `unknown_keys` are warnings, the full sorted list (the UI truncates).
    Valid {
        unknown_keys: Vec<String>,
    },
    Invalid(ConfigYamlIssue),
    /// A placeholder with no on-disk secret to restore (D-08).
    UnmatchedPlaceholder {
        key_path: String,
    },
    Unavailable {
        reason: String,
    },
}

/// One picker entry (D-05b): a scope plus whether its `config.yaml` exists.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConfigTarget {
    pub scope: ConfigScope,
    pub exists: bool,
}

/// `save_config_yaml` outcome (D-02/D-05/D-06/D-08/D-15).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum SaveOutcome {
    /// Written. `unknown_keys` are warnings; `locked` is true when the save
    /// turned the root write gate off (acknowledged, D-15).
    Saved {
        sha256: String,
        unknown_keys: Vec<String>,
        locked: bool,
    },
    Invalid(ConfigYamlIssue),
    /// A placeholder with no on-disk secret to restore (D-08).
    UnmatchedPlaceholder {
        key_path: String,
    },
    /// The file changed after the editor loaded it (D-02 step 3).
    ChangedOnDisk,
    /// The ROOT write gate is off (D-05).
    GateClosed,
    /// The save turns the root gate off and carried no acknowledgement (D-15).
    LockoutAckRequired {
        removes_key: bool,
    },
    /// An I/O failure; the cause goes to the log, never the browser.
    Failed,
}

#[cfg(feature = "server")]
const MAX_CONFIG_BYTES: usize = 512 * 1024;

#[cfg(feature = "server")]
struct ResolvedConfig {
    path: std::path::PathBuf,
    display_path: String,
}

/// D-05a: map a scope to its `config.yaml`, confined under the home. Errors
/// never contain an absolute host path.
#[cfg(feature = "server")]
fn resolve_config_path(scope: &ConfigScope) -> Result<ResolvedConfig, String> {
    let (dir, display_path) = match scope {
        ConfigScope::Root => (
            ironhermes_core::get_hermes_home(),
            "config.yaml".to_string(),
        ),
        ConfigScope::Profile(name) => {
            let validated = ironhermes_core::profile::validate_profile_name(name)
                .map_err(|_| "invalid profile name".to_string())?;
            let dir = crate::server::profile_api::profile_dir_for(&validated);
            let meta = std::fs::symlink_metadata(&dir)
                .map_err(|_| format!("profile '{validated}' does not exist"))?;
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(format!("profile '{validated}' is not a regular directory"));
            }
            let profiles_root = ironhermes_core::get_hermes_home()
                .join(ironhermes_core::PROFILES_SUBDIR)
                .canonicalize()
                .map_err(|_| "profiles directory is unavailable".to_string())?;
            let canonical = dir
                .canonicalize()
                .map_err(|_| format!("profile '{validated}' is unavailable"))?;
            if !canonical.starts_with(&profiles_root) {
                return Err(format!(
                    "profile '{validated}' resolves outside the profiles directory"
                ));
            }
            (dir, format!("profiles/{validated}/config.yaml"))
        }
    };
    let path = dir.join("config.yaml");
    let is_symlink = std::fs::symlink_metadata(&path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if is_symlink {
        return Err("config.yaml is a symlink".to_string());
    }
    Ok(ResolvedConfig { path, display_path })
}

/// Raw bytes plus text. The failure strings finish the sentence
/// `{display_path} {problem}`.
#[cfg(feature = "server")]
fn read_config_text(path: &std::path::Path) -> Result<(Vec<u8>, String), String> {
    let bytes = std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "does not exist".to_string()
        } else {
            "could not be read".to_string()
        }
    })?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err("is larger than 512 KiB".to_string());
    }
    if bytes.contains(&0) {
        return Err("contains a NUL byte".to_string());
    }
    let text = String::from_utf8(bytes.clone()).map_err(|_| "is not UTF-8".to_string())?;
    Ok((bytes, text))
}

/// sha256 over a per-process random salt, then the raw bytes. Salted so the
/// browser-held digest is not an offline guessing oracle for short inline
/// passwords. A restart only makes an open editor's next save report
/// "changed on disk".
#[cfg(feature = "server")]
fn config_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    static SALT: std::sync::OnceLock<[u8; 16]> = std::sync::OnceLock::new();
    let salt = SALT.get_or_init(|| uuid::Uuid::new_v4().into_bytes());
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(feature = "server")]
fn get_config_yaml_impl(scope: &ConfigScope, write_enabled: bool) -> ConfigYamlLoad {
    use ironhermes_core::config_yaml_edit::{self, MaskError};
    let resolved = match resolve_config_path(scope) {
        Ok(r) => r,
        Err(reason) => return ConfigYamlLoad::Unavailable { reason },
    };
    let unavailable = |problem: String| ConfigYamlLoad::Unavailable {
        reason: format!("{} {problem}", resolved.display_path),
    };
    let (bytes, text) = match read_config_text(&resolved.path) {
        Ok(v) => v,
        Err(problem) => return unavailable(problem),
    };
    match config_yaml_edit::mask(&text) {
        Ok(masked) => ConfigYamlLoad::Loaded(ConfigYamlRead {
            text: masked,
            sha256: config_digest(&bytes),
            write_enabled,
            display_path: resolved.display_path.clone(),
        }),
        Err(MaskError::Unsupported { key_path }) => unavailable(format!(
            "cannot be shown: the secret at {key_path} uses a layout the editor cannot hide"
        )),
        Err(MaskError::NotYaml) => {
            unavailable("is not valid YAML, so its secrets cannot be verified hidden".to_string())
        }
    }
}

#[cfg(feature = "server")]
fn load_for_scope(scope: &ConfigScope) -> ConfigYamlLoad {
    get_config_yaml_impl(scope, crate::server::tools_config_api::root_gate_open())
}

/// A regular (non-symlink) file at `path`.
#[cfg(feature = "server")]
fn is_regular_file(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
}

/// D-05b: the picker list. Same on-disk scan as `list_profiles`, but no
/// profile `config.yaml` is ever opened or parsed (the loader exits the
/// process on the legacy key and a malformed file would read as missing).
#[cfg(feature = "server")]
fn list_config_targets_impl() -> Result<Vec<ConfigTarget>, String> {
    let home = ironhermes_core::get_hermes_home();
    let mut out = vec![ConfigTarget {
        scope: ConfigScope::Root,
        exists: is_regular_file(&home.join("config.yaml")),
    }];
    let root = home.join(ironhermes_core::PROFILES_SUBDIR);
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(_) => return Err("could not list the profiles directory".to_string()),
    };
    let mut profiles: Vec<(String, bool)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| "could not read a profiles entry".to_string())?;
        // `file_type()` does not follow symlinks: symlinked dirs drop out.
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_dir
            || name.starts_with('.')
            || ironhermes_core::profile::validate_profile_name(&name).is_err()
        {
            continue;
        }
        let exists = is_regular_file(&entry.path().join("config.yaml"));
        profiles.push((name, exists));
    }
    profiles.sort();
    out.extend(profiles.into_iter().map(|(name, exists)| ConfigTarget {
        scope: ConfigScope::Profile(name),
        exists,
    }));
    Ok(out)
}

#[server]
pub async fn list_config_targets() -> Result<Vec<ConfigTarget>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tokio::task::spawn_blocking(list_config_targets_impl)
            .await
            .map_err(|e| ServerFnError::new(format!("list_config_targets task failed: {e}")))?
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "list_config_targets unavailable without `server` feature",
        ))
    }
}

/// Size / NUL guard on submitted text. Plan 04 reuses it for save.
#[cfg(feature = "server")]
fn submitted_text_issue(text: &str) -> Option<ConfigYamlIssue> {
    let message = if text.len() > MAX_CONFIG_BYTES {
        "The text is larger than 512 KiB."
    } else if text.contains('\0') {
        "The text contains a NUL character."
    } else {
        return None;
    };
    Some(ConfigYamlIssue {
        message: message.to_string(),
        line: None,
        column: None,
    })
}

/// Validate submitted text against the real `Config`. Never writes (D-12).
#[cfg(feature = "server")]
fn check_config_yaml_impl(scope: &ConfigScope, text: &str) -> CheckOutcome {
    use ironhermes_core::config_yaml_edit::{self, PrepareError};
    if let Some(issue) = submitted_text_issue(text) {
        return CheckOutcome::Invalid(issue);
    }
    let resolved = match resolve_config_path(scope) {
        Ok(r) => r,
        Err(reason) => return CheckOutcome::Unavailable { reason },
    };
    let on_disk = match read_config_text(&resolved.path) {
        Ok((_, on_disk)) => on_disk,
        Err(problem) => {
            return CheckOutcome::Unavailable {
                reason: format!("{} {problem}", resolved.display_path),
            };
        }
    };
    match config_yaml_edit::prepare(text, &on_disk) {
        Ok(prepared) => CheckOutcome::Valid {
            unknown_keys: prepared.unknown_keys,
        },
        Err(PrepareError::Unmatched { key_path }) => {
            CheckOutcome::UnmatchedPlaceholder { key_path }
        }
        Err(PrepareError::Invalid(issue)) => CheckOutcome::Invalid(ConfigYamlIssue {
            message: issue.message,
            line: issue.line.map(|l| l as u32),
            column: issue.column.map(|c| c as u32),
        }),
    }
}

#[cfg(feature = "server")]
const BACKUP_PREFIX: &str = "config.yaml.bak-";

/// `YYYYMMDDTHHMMSSmmmZ` (UTC, 19 chars). Fixed width, so name order is
/// chronological order.
#[cfg(feature = "server")]
fn backup_id_now() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string()
}

/// Strict backup id: 8 digits, `T`, 9 digits, `Z`, then optionally `-` and 1
/// to 3 digits. Nothing else passes, so no path text can reach a file name.
#[cfg(feature = "server")]
fn is_backup_id(s: &str) -> bool {
    let (base, suffix) = match s.split_once('-') {
        Some((b, x)) => (b.as_bytes(), Some(x)),
        None => (s.as_bytes(), None),
    };
    let base_ok = base.len() == 19
        && base[..8].iter().all(u8::is_ascii_digit)
        && base[8] == b'T'
        && base[9..18].iter().all(u8::is_ascii_digit)
        && base[18] == b'Z';
    let suffix_ok = match suffix {
        None => true,
        Some(x) => (1..=3).contains(&x.len()) && x.bytes().all(|c| c.is_ascii_digit()),
    };
    base_ok && suffix_ok
}

/// Write `original` as `config.yaml.bak-{id}` (0600) in `dir`; on a name
/// collision use `{id}-1` .. `{id}-999`. Returns the id used.
#[cfg(feature = "server")]
fn write_backup_with_id(
    dir: &std::path::Path,
    original: &str,
    id: &str,
) -> std::io::Result<String> {
    let mut used = id.to_string();
    let mut n = 0;
    while dir.join(format!("{BACKUP_PREFIX}{used}")).exists() {
        n += 1;
        if n > 999 {
            return Err(std::io::Error::other("backup id collisions exhausted"));
        }
        used = format!("{id}-{n}");
    }
    crate::server::profile_api::write_env_atomic_0600(
        &dir.join(format!("{BACKUP_PREFIX}{used}")),
        original,
    )?;
    Ok(used)
}

/// D-14: newest backups kept per file.
#[cfg(feature = "server")]
const BACKUP_KEEP: usize = 10;

/// Ids of the regular-file backups in `dir` (name = `BACKUP_PREFIX` + a strict
/// id), sorted ascending, which is chronological. Anything else is ignored.
#[cfg(feature = "server")]
fn list_backup_ids(dir: &std::path::Path) -> std::io::Result<Vec<String>> {
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_prefix(BACKUP_PREFIX) else {
            continue;
        };
        let is_file = std::fs::symlink_metadata(entry.path())
            .map(|m| m.file_type().is_file())
            .unwrap_or(false);
        if is_backup_id(id) && is_file {
            ids.push(id.to_string());
        }
    }
    ids.sort();
    Ok(ids)
}

/// The oldest `len - keep` ids of an ascending list.
#[cfg(feature = "server")]
fn ids_to_prune(ids: &[String], keep: usize) -> Vec<String> {
    ids[..ids.len().saturating_sub(keep)].to_vec()
}

/// Delete all but the newest `keep` strict-name backups. Never fails the save.
#[cfg(feature = "server")]
fn prune_backups(dir: &std::path::Path, keep: usize) {
    let ids = match list_backup_ids(dir) {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(target: "iron_hermes_ui::config_yaml", error = ?e.kind(), "backup prune: listing failed");
            return;
        }
    };
    for id in ids_to_prune(&ids, keep) {
        if let Err(e) = std::fs::remove_file(dir.join(format!("{BACKUP_PREFIX}{id}"))) {
            tracing::warn!(target: "iron_hermes_ui::config_yaml", error = ?e.kind(), "backup prune: remove failed");
        }
    }
}

/// One listed backup (D-14). The id is a strict timestamp, never a file name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConfigBackup {
    pub id: String,
    pub size_bytes: u64,
}

/// `get_config_backup` outcome. Deliberately no digest: the editor keeps the
/// CURRENT file's digest so the save compare still guards the live file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum BackupText {
    /// The backup text with secrets hidden (same `mask` as live files).
    Loaded {
        text: String,
    },
    Unavailable {
        reason: String,
    },
}

/// D-14: the selected file's backups, newest first.
#[cfg(feature = "server")]
fn list_config_backups_impl(scope: &ConfigScope) -> Result<Vec<ConfigBackup>, String> {
    let resolved = resolve_config_path(scope)?;
    let dir = resolved
        .path
        .parent()
        .ok_or_else(|| "config directory is unavailable".to_string())?;
    let ids = list_backup_ids(dir).map_err(|_| "could not list backups".to_string())?;
    Ok(ids
        .into_iter()
        .rev()
        .map(|id| {
            let size_bytes = std::fs::symlink_metadata(dir.join(format!("{BACKUP_PREFIX}{id}")))
                .map(|m| m.len())
                .unwrap_or(0);
            ConfigBackup { id, size_bytes }
        })
        .collect())
}

/// D-14: one backup, masked. `id` is client text: only a strict id ever
/// reaches a path, and the path is built here.
#[cfg(feature = "server")]
fn get_config_backup_impl(scope: &ConfigScope, id: &str) -> BackupText {
    use ironhermes_core::config_yaml_edit::{self, MaskError};
    let unavailable = |reason: String| BackupText::Unavailable { reason };
    if !is_backup_id(id) {
        return unavailable("invalid backup id".to_string());
    }
    let resolved = match resolve_config_path(scope) {
        Ok(r) => r,
        Err(reason) => return unavailable(reason),
    };
    let Some(dir) = resolved.path.parent() else {
        return unavailable("config directory is unavailable".to_string());
    };
    let path = dir.join(format!("{BACKUP_PREFIX}{id}"));
    if !is_regular_file(&path) {
        return unavailable("backup does not exist".to_string());
    }
    let (_, text) = match read_config_text(&path) {
        Ok(v) => v,
        Err(problem) => return unavailable(format!("backup {problem}")),
    };
    match config_yaml_edit::mask(&text) {
        Ok(masked) => BackupText::Loaded { text: masked },
        Err(MaskError::Unsupported { key_path }) => unavailable(format!(
            "backup cannot be shown: the secret at {key_path} uses a layout the editor cannot hide"
        )),
        Err(MaskError::NotYaml) => unavailable(
            "backup is not valid YAML, so its secrets cannot be verified hidden".to_string(),
        ),
    }
}

#[server]
pub async fn list_config_backups(scope: ConfigScope) -> Result<Vec<ConfigBackup>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tokio::task::spawn_blocking(move || list_config_backups_impl(&scope))
            .await
            .map_err(|e| ServerFnError::new(format!("list_config_backups task failed: {e}")))?
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = scope;
        Err(ServerFnError::new(
            "list_config_backups unavailable without `server` feature",
        ))
    }
}

#[server]
pub async fn get_config_backup(
    scope: ConfigScope,
    id: String,
) -> Result<BackupText, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tokio::task::spawn_blocking(move || get_config_backup_impl(&scope, &id))
            .await
            .map_err(|e| ServerFnError::new(format!("get_config_backup task failed: {e}")))
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (scope, id);
        Err(ServerFnError::new(
            "get_config_backup unavailable without `server` feature",
        ))
    }
}

#[cfg(feature = "server")]
enum LockoutKind {
    SetFalse,
    KeyRemoved,
}

/// D-15: `Some` when the new text turns the root gate from on to off. An
/// unparseable new text counts as off (fail closed); validation already ran.
#[cfg(feature = "server")]
fn lockout_kind(old_text: &str, new_text: &str) -> Option<LockoutKind> {
    use ironhermes_core::config::Config;
    let old: Config = serde_yaml::from_str(old_text).ok()?;
    let new_on = serde_yaml::from_str::<Config>(new_text)
        .map(|c| c.security.web_config_write_enabled)
        .unwrap_or(false);
    if !old.security.web_config_write_enabled || new_on {
        return None;
    }
    let has_key = serde_yaml::from_str::<serde_yaml::Value>(new_text)
        .ok()
        .and_then(|v| {
            v.get("security")?
                .get("web_config_write_enabled")
                .map(|_| ())
        })
        .is_some();
    Some(if has_key {
        LockoutKind::SetFalse
    } else {
        LockoutKind::KeyRemoved
    })
}

/// `root` / `profile:{name}`.
#[cfg(feature = "server")]
fn scope_label(scope: &ConfigScope) -> String {
    match scope {
        ConfigScope::Root => "root".to_string(),
        ConfigScope::Profile(name) => format!("profile:{name}"),
    }
}

/// One `config_write` audit row per attempt: scope, outcome, locked flag,
/// unknown-key count and the new digest only. Never file text, never a value.
#[cfg(feature = "server")]
fn audit_save(scope: &ConfigScope, outcome: &SaveOutcome) {
    let (name, decision) = match outcome {
        SaveOutcome::Saved { .. } => ("Saved", "approved"),
        SaveOutcome::Invalid(_) => ("Invalid", "denied"),
        SaveOutcome::UnmatchedPlaceholder { .. } => ("UnmatchedPlaceholder", "denied"),
        SaveOutcome::ChangedOnDisk => ("ChangedOnDisk", "denied"),
        SaveOutcome::GateClosed => ("GateClosed", "denied"),
        SaveOutcome::LockoutAckRequired { .. } => ("LockoutAckRequired", "denied"),
        SaveOutcome::Failed => ("Failed", "denied"),
    };
    let label = scope_label(scope);
    let (locked, unknown, new_sha) = match outcome {
        SaveOutcome::Saved {
            sha256,
            unknown_keys,
            locked,
        } => (*locked, unknown_keys.len(), Some(sha256.as_str())),
        _ => (false, 0, None),
    };
    let args = serde_json::json!({
        "scope": label,
        "outcome": name,
        "locked": locked,
        "unknown_keys": unknown,
        "new_sha256": new_sha,
    });
    let audit = ironhermes_core::AuditLog::load(
        ironhermes_core::config::Config::load()
            .map(|c| c.audit)
            .unwrap_or_default(),
    );
    let entry = audit.make_entry(
        "config_write",
        "config.yaml.save",
        None,
        label.clone(),
        "web",
        label,
        name,
        decision,
        "operator",
        &args,
    );
    if let Err(e) = audit.append_sync(&entry) {
        tracing::warn!(target: "iron_hermes_ui::config_yaml", error = ?e.kind(), "failed to append config_write audit entry");
    }
}

#[cfg(feature = "server")]
fn write_backup(dir: &std::path::Path, original: &str) -> std::io::Result<String> {
    write_backup_with_id(dir, original, &backup_id_now())
}

/// Log a failure cause (error kind only: no path, no text) and report `Failed`.
#[cfg(feature = "server")]
fn failed(stage: &str, kind: impl std::fmt::Debug) -> SaveOutcome {
    tracing::warn!(target: "iron_hermes_ui::config_yaml", stage, error = ?kind, "config.yaml save failed");
    SaveOutcome::Failed
}

/// The D-02 pipeline as one synchronous fn (the std lock is never held across
/// an await): gate, lock, digest re-check, restore + validate, backup, atomic
/// 0600 raw write.
#[cfg(feature = "server")]
fn save_attempt(
    scope: &ConfigScope,
    text: &str,
    expected_sha256: &str,
    ack_lockout: bool,
) -> SaveOutcome {
    use ironhermes_core::config_yaml_edit::{self, PrepareError};
    if crate::server::tools_config_api::check_tools_write_gate().is_err() {
        return SaveOutcome::GateClosed;
    }
    let _guard = match scope {
        ConfigScope::Root => crate::server::profile_activation_api::lock_root_config_write(),
        ConfigScope::Profile(_) => crate::server::profile_api::lock_profile_config_write(),
    };
    let resolved = match resolve_config_path(scope) {
        Ok(r) => r,
        Err(reason) => return failed("resolve", reason),
    };
    let (bytes, on_disk) = match read_config_text(&resolved.path) {
        Ok(v) => v,
        Err(problem) => return failed("read", problem),
    };
    if config_digest(&bytes) != expected_sha256 {
        return SaveOutcome::ChangedOnDisk;
    }
    if let Some(issue) = submitted_text_issue(text) {
        return SaveOutcome::Invalid(issue);
    }
    let prepared = match config_yaml_edit::prepare(text, &on_disk) {
        Ok(p) => p,
        Err(PrepareError::Unmatched { key_path }) => {
            return SaveOutcome::UnmatchedPlaceholder { key_path };
        }
        Err(PrepareError::Invalid(issue)) => {
            return SaveOutcome::Invalid(ConfigYamlIssue {
                message: issue.message,
                line: issue.line.map(|l| l as u32),
                column: issue.column.map(|c| c as u32),
            });
        }
    };
    // D-15: only a ROOT save can turn the root gate off; refuse before any write.
    let mut locked = false;
    let lockout = match scope {
        ConfigScope::Root => lockout_kind(&on_disk, &prepared.text),
        ConfigScope::Profile(_) => None,
    };
    if let Some(kind) = lockout {
        if !ack_lockout {
            return SaveOutcome::LockoutAckRequired {
                removes_key: matches!(kind, LockoutKind::KeyRemoved),
            };
        }
        locked = true;
    }
    let Some(dir) = resolved.path.parent() else {
        return failed("parent", "no parent dir");
    };
    if let Err(e) = write_backup(dir, &on_disk) {
        return failed("backup", e.kind());
    }
    prune_backups(dir, BACKUP_KEEP);
    if let Err(e) =
        crate::server::profile_api::write_env_atomic_0600(&resolved.path, &prepared.text)
    {
        return failed("write", e.kind());
    }
    SaveOutcome::Saved {
        sha256: config_digest(prepared.text.as_bytes()),
        unknown_keys: prepared.unknown_keys,
        locked,
    }
}

/// One attempt, then exactly one audit row (after the lock guard dropped).
#[cfg(feature = "server")]
fn save_config_yaml_impl(
    scope: &ConfigScope,
    text: &str,
    expected_sha256: &str,
    ack_lockout: bool,
) -> SaveOutcome {
    let outcome = save_attempt(scope, text, expected_sha256, ack_lockout);
    audit_save(scope, &outcome);
    outcome
}

/// Save the editor text (D-02). Runs the whole synchronous pipeline on a
/// blocking thread.
#[server]
pub async fn save_config_yaml(
    scope: ConfigScope,
    text: String,
    expected_sha256: String,
    ack_lockout: bool,
) -> Result<SaveOutcome, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tokio::task::spawn_blocking(move || {
            save_config_yaml_impl(&scope, &text, &expected_sha256, ack_lockout)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("save_config_yaml task failed: {e}")))
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (scope, text, expected_sha256, ack_lockout);
        Err(ServerFnError::new(
            "save_config_yaml unavailable without `server` feature",
        ))
    }
}

/// Validate the editor text without saving (D-12). Ungated server-side: it
/// writes nothing; the UI disables the button when read-only.
#[server]
pub async fn check_config_yaml(
    scope: ConfigScope,
    text: String,
) -> Result<CheckOutcome, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tokio::task::spawn_blocking(move || check_config_yaml_impl(&scope, &text))
            .await
            .map_err(|e| ServerFnError::new(format!("check_config_yaml task failed: {e}")))
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (scope, text);
        Err(ServerFnError::new(
            "check_config_yaml unavailable without `server` feature",
        ))
    }
}

/// Read `scope`'s `config.yaml`, masked, with its digest (D-01).
#[server]
pub async fn get_config_yaml(scope: ConfigScope) -> Result<ConfigYamlLoad, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tokio::task::spawn_blocking(move || load_for_scope(&scope))
            .await
            .map_err(|e| ServerFnError::new(format!("get_config_yaml task failed: {e}")))
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = scope;
        Err(ServerFnError::new(
            "get_config_yaml unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::path::Path;

    /// Run `f` with `IRONHERMES_HOME` pointing at a fresh temp dir.
    fn with_home<R>(f: impl FnOnce(&Path) -> R) -> R {
        let _g = crate::server::test_support::env_lock();
        let home = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home.path()) };
        let out = f(home.path());
        unsafe { std::env::remove_var("IRONHERMES_HOME") };
        out
    }

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn get_root_returns_text_and_salted_digest_of_disk_bytes() {
        with_home(|home| {
            let body = "# hi\nmodel:\n  default: gpt-5\n";
            write(&home.join("config.yaml"), body);
            let a = load_for_scope(&ConfigScope::Root);
            let b = load_for_scope(&ConfigScope::Root);
            let ConfigYamlLoad::Loaded(r) = a.clone() else {
                panic!("expected Loaded, got {a:?}")
            };
            assert_eq!(r.text, body);
            assert_eq!(r.display_path, "config.yaml");
            assert_eq!(r.sha256, config_digest(body.as_bytes()));
            assert_eq!(a, b, "digest must be stable across calls");
            let plain = format!("{:x}", Sha256::digest(body.as_bytes()));
            assert_ne!(r.sha256, plain, "digest must be salted");
        });
    }

    #[test]
    fn get_reports_write_enabled_from_root_gate() {
        with_home(|home| {
            write(
                &home.join("config.yaml"),
                "security:\n  web_config_write_enabled: true\n",
            );
            let ConfigYamlLoad::Loaded(r) = load_for_scope(&ConfigScope::Root) else {
                panic!("expected Loaded")
            };
            assert!(r.write_enabled);
            write(&home.join("config.yaml"), "model:\n  default: x\n");
            let ConfigYamlLoad::Loaded(r) = load_for_scope(&ConfigScope::Root) else {
                panic!("expected Loaded")
            };
            assert!(!r.write_enabled);
        });
    }

    #[test]
    fn get_refuses_unsupported_secret_layout_without_leaking() {
        with_home(|home| {
            write(
                &home.join("config.yaml"),
                "providers: {openai: {api_key: sk-leak-me}}\n",
            );
            let ConfigYamlLoad::Unavailable { reason } = load_for_scope(&ConfigScope::Root) else {
                panic!("expected Unavailable")
            };
            assert!(reason.contains("providers.openai.api_key"), "{reason}");
            assert!(!reason.contains("sk-leak-me"), "{reason}");
            assert!(
                !reason.contains(&home.display().to_string()),
                "absolute path leaked: {reason}"
            );
        });
    }

    #[test]
    fn get_profile_reads_profiles_name_config_yaml() {
        with_home(|home| {
            write(
                &home.join("profiles/alpha/config.yaml"),
                "model:\n  default: a\n",
            );
            let ConfigYamlLoad::Loaded(r) =
                load_for_scope(&ConfigScope::Profile("alpha".to_string()))
            else {
                panic!("expected Loaded")
            };
            assert_eq!(r.display_path, "profiles/alpha/config.yaml");
        });
    }

    #[test]
    fn resolve_refuses_invalid_profile_names() {
        with_home(|home| {
            // Real directories, so only the name validation can refuse them.
            for name in ["Alpha", "default", "current", "none"] {
                write(
                    &home.join("profiles").join(name).join("config.yaml"),
                    "a: 1\n",
                );
                assert!(
                    resolve_config_path(&ConfigScope::Profile(name.to_string())).is_err(),
                    "{name} must be refused by name validation"
                );
            }
            for name in ["../x", "a/b"] {
                assert!(resolve_config_path(&ConfigScope::Profile(name.to_string())).is_err());
            }
        });
    }

    #[cfg(unix)]
    #[test]
    fn resolve_refuses_symlinked_profile_dir_and_config() {
        with_home(|home| {
            let outside = tempfile::tempdir().unwrap();
            write(&outside.path().join("config.yaml"), "a: 1\n");
            std::fs::create_dir_all(home.join("profiles")).unwrap();
            std::os::unix::fs::symlink(outside.path(), home.join("profiles/linked")).unwrap();
            assert!(resolve_config_path(&ConfigScope::Profile("linked".to_string())).is_err());

            std::fs::create_dir_all(home.join("profiles/real")).unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("config.yaml"),
                home.join("profiles/real/config.yaml"),
            )
            .unwrap();
            assert!(resolve_config_path(&ConfigScope::Profile("real".to_string())).is_err());
        });
    }

    #[test]
    fn get_refuses_oversized_and_nul_files() {
        with_home(|home| {
            write(&home.join("config.yaml"), &"#".repeat(MAX_CONFIG_BYTES + 1));
            assert!(matches!(
                load_for_scope(&ConfigScope::Root),
                ConfigYamlLoad::Unavailable { .. }
            ));
            std::fs::write(home.join("config.yaml"), b"a: 1\0\n").unwrap();
            assert!(matches!(
                load_for_scope(&ConfigScope::Root),
                ConfigYamlLoad::Unavailable { .. }
            ));
        });
    }

    #[test]
    fn get_missing_file_is_unavailable() {
        with_home(|home| {
            std::fs::create_dir_all(home.join("profiles/beta")).unwrap();
            assert!(matches!(
                load_for_scope(&ConfigScope::Profile("beta".to_string())),
                ConfigYamlLoad::Unavailable { .. }
            ));
        });
    }

    #[test]
    fn check_valid_root_returns_unknown_keys() {
        with_home(|home| {
            write(&home.join("config.yaml"), "agent:\n  max_iterations: 3\n");
            assert_eq!(
                check_config_yaml_impl(&ConfigScope::Root, "bogus: 1\n"),
                CheckOutcome::Valid {
                    unknown_keys: vec!["bogus".to_string()]
                }
            );
        });
    }

    #[test]
    fn check_invalid_returns_issue_with_line_and_column() {
        with_home(|home| {
            write(&home.join("config.yaml"), "agent:\n  max_iterations: 3\n");
            let out = check_config_yaml_impl(
                &ConfigScope::Root,
                "agent:\n  max_iterations: notanumber\n",
            );
            match out {
                CheckOutcome::Invalid(ConfigYamlIssue {
                    line: Some(2),
                    column: Some(_),
                    ..
                }) => {}
                other => panic!("expected positioned Invalid, got {other:?}"),
            }
        });
    }

    #[test]
    fn check_subagent_key_is_invalid_and_server_survives() {
        with_home(|home| {
            write(&home.join("config.yaml"), "agent:\n  max_iterations: 3\n");
            let out = check_config_yaml_impl(&ConfigScope::Root, "a: 1\nb: 2\nsubagent:\n  x: 1\n");
            assert!(
                matches!(
                    out,
                    CheckOutcome::Invalid(ConfigYamlIssue { line: Some(3), .. })
                ),
                "{out:?}"
            );
        });
    }

    #[test]
    fn check_leftover_placeholder_is_unmatched_with_key_path() {
        with_home(|home| {
            write(&home.join("config.yaml"), "agent:\n  max_iterations: 3\n");
            let text = format!(
                "notes: {}\n",
                ironhermes_core::config_yaml_edit::PLACEHOLDER
            );
            assert_eq!(
                check_config_yaml_impl(&ConfigScope::Root, &text),
                CheckOutcome::UnmatchedPlaceholder {
                    key_path: "notes".to_string()
                }
            );
        });
    }

    #[test]
    fn check_never_writes_the_file() {
        with_home(|home| {
            let path = home.join("config.yaml");
            let body = "agent:\n  max_iterations: 3\n";
            write(&path, body);
            let _ = check_config_yaml_impl(&ConfigScope::Root, "bogus: 1\n");
            let _ = check_config_yaml_impl(&ConfigScope::Root, "agent:\n  max_iterations: x\n");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        });
    }

    fn names(v: &[ConfigTarget]) -> Vec<String> {
        v.iter()
            .map(|t| match &t.scope {
                ConfigScope::Root => "Root".to_string(),
                ConfigScope::Profile(n) => format!("Profile({n})"),
            })
            .collect()
    }

    #[test]
    fn list_config_targets_puts_root_first_then_sorted_profiles() {
        with_home(|home| {
            write(&home.join("profiles/zeta/config.yaml"), "a: 1\n");
            write(&home.join("profiles/alpha/config.yaml"), "a: 1\n");
            let t = list_config_targets_impl().unwrap();
            assert_eq!(names(&t), ["Root", "Profile(alpha)", "Profile(zeta)"]);
        });
    }

    #[test]
    fn list_config_targets_skips_dotfiles_files_and_invalid_names() {
        with_home(|home| {
            write(&home.join("profiles/ok/config.yaml"), "a: 1\n");
            std::fs::create_dir_all(home.join("profiles/.hidden")).unwrap();
            std::fs::write(home.join("profiles/notes"), "x").unwrap();
            std::fs::create_dir_all(home.join("profiles/Bad_Name")).unwrap();
            #[cfg(unix)]
            {
                let outside = tempfile::tempdir().unwrap();
                std::os::unix::fs::symlink(outside.path(), home.join("profiles/linked")).unwrap();
            }
            let t = list_config_targets_impl().unwrap();
            assert_eq!(names(&t), ["Root", "Profile(ok)"]);
        });
    }

    #[test]
    fn list_config_targets_marks_missing_config_yaml() {
        with_home(|home| {
            write(&home.join("profiles/alpha/config.yaml"), "a: 1\n");
            std::fs::create_dir_all(home.join("profiles/beta")).unwrap();
            let t = list_config_targets_impl().unwrap();
            assert_eq!(names(&t), ["Root", "Profile(alpha)", "Profile(beta)"]);
            assert!(!t[0].exists, "no root config.yaml written");
            assert!(t[1].exists);
            assert!(!t[2].exists);
        });
    }

    #[test]
    fn list_config_targets_does_not_parse_profile_configs() {
        with_home(|home| {
            write(
                &home.join("profiles/legacy/config.yaml"),
                "a: 1\nsubagent:\n  x: 1\n",
            );
            let t = list_config_targets_impl().unwrap();
            assert_eq!(names(&t), ["Root", "Profile(legacy)"]);
            assert!(t[1].exists);
        });
    }

    #[test]
    fn list_config_targets_without_profiles_dir_is_root_only() {
        with_home(|_| {
            let t = list_config_targets_impl().unwrap();
            assert_eq!(names(&t), ["Root"]);
        });
    }

    // ---- Plan 04 Task 1: save pipeline ----

    const GATE_ON: &str = "security:\n  web_config_write_enabled: true\n";
    const MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022} (kept)";

    fn open(scope: &ConfigScope) -> ConfigYamlRead {
        match load_for_scope(scope) {
            ConfigYamlLoad::Loaded(r) => r,
            other => panic!("expected Loaded, got {other:?}"),
        }
    }

    fn backups(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("config.yaml.bak-"))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn save_root_writes_raw_bytes_and_keeps_comments() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let text = "# top comment\n\nmodel:\n  default: x   # trailing\n\nsecurity:\n  web_config_write_enabled: true\n";
            let out = save_config_yaml_impl(&ConfigScope::Root, text, &r.sha256, false);
            assert!(matches!(out, SaveOutcome::Saved { .. }), "{out:?}");
            assert_eq!(
                std::fs::read_to_string(home.join("config.yaml")).unwrap(),
                text
            );
        });
    }

    #[test]
    fn save_restores_placeholder_secret_byte_identical() {
        with_home(|home| {
            let body = format!(
                "{GATE_ON}providers:\n  openai:\n    api_key: \"sk-keep-1\"\nagent:\n  max_iterations: 3\n"
            );
            write(&home.join("config.yaml"), &body);
            let r = open(&ConfigScope::Root);
            assert!(r.text.contains(MASK), "{}", r.text);
            let edited = r.text.replace("max_iterations: 3", "max_iterations: 4");
            let out = save_config_yaml_impl(&ConfigScope::Root, &edited, &r.sha256, false);
            assert!(matches!(out, SaveOutcome::Saved { .. }), "{out:?}");
            let on_disk = std::fs::read_to_string(home.join("config.yaml")).unwrap();
            assert!(
                on_disk.contains("    api_key: \"sk-keep-1\"\n"),
                "{on_disk}"
            );
            assert!(!on_disk.contains("(kept)"), "{on_disk}");
            assert!(on_disk.contains("max_iterations: 4"));
        });
    }

    #[test]
    fn save_writes_backup_of_previous_bytes() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let text = format!("{GATE_ON}agent:\n  max_iterations: 2\n");
            let out = save_config_yaml_impl(&ConfigScope::Root, &text, &r.sha256, false);
            assert!(matches!(out, SaveOutcome::Saved { .. }), "{out:?}");
            let b = backups(home);
            assert_eq!(b.len(), 1, "{b:?}");
            let id = b[0].strip_prefix("config.yaml.bak-").unwrap();
            assert!(is_backup_id(id), "{id}");
            assert_eq!(id.len(), 19, "{id}");
            assert_eq!(std::fs::read_to_string(home.join(&b[0])).unwrap(), GATE_ON);
        });
    }

    #[cfg(unix)]
    #[test]
    fn save_writes_mode_0600_for_config_and_backup() {
        use std::os::unix::fs::PermissionsExt;
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let text = format!("{GATE_ON}agent:\n  max_iterations: 2\n");
            let out = save_config_yaml_impl(&ConfigScope::Root, &text, &r.sha256, false);
            assert!(matches!(out, SaveOutcome::Saved { .. }), "{out:?}");
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&home.join("config.yaml")), 0o600);
            let b = backups(home);
            assert_eq!(b.len(), 1);
            assert_eq!(mode(&home.join(&b[0])), 0o600);
        });
    }

    #[test]
    fn save_returns_digest_matching_next_get() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let text = format!("{GATE_ON}agent:\n  max_iterations: 2\n");
            let SaveOutcome::Saved { sha256, .. } =
                save_config_yaml_impl(&ConfigScope::Root, &text, &r.sha256, false)
            else {
                panic!("expected Saved")
            };
            assert_eq!(sha256, open(&ConfigScope::Root).sha256);
        });
    }

    #[test]
    fn save_profile_writes_profile_file_only() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            write(
                &home.join("profiles/alpha/config.yaml"),
                "agent:\n  max_iterations: 3\n",
            );
            let scope = ConfigScope::Profile("alpha".to_string());
            let r = open(&scope);
            let text = "# profile\nagent:\n  max_iterations: 9\n";
            let out = save_config_yaml_impl(&scope, text, &r.sha256, false);
            assert!(matches!(out, SaveOutcome::Saved { .. }), "{out:?}");
            assert_eq!(
                std::fs::read_to_string(home.join("profiles/alpha/config.yaml")).unwrap(),
                text
            );
            assert_eq!(
                std::fs::read_to_string(home.join("config.yaml")).unwrap(),
                GATE_ON
            );
            assert_eq!(backups(&home.join("profiles/alpha")).len(), 1);
            assert!(backups(home).is_empty(), "root dir must get no backup");
        });
    }

    #[test]
    fn get_serves_masked_secret_never_plaintext() {
        with_home(|home| {
            write(
                &home.join("config.yaml"),
                "providers:\n  openai:\n    api_key: sk-plain-77\n",
            );
            let r = open(&ConfigScope::Root);
            assert!(r.text.contains(MASK), "{}", r.text);
            assert!(!r.text.contains("sk-plain-77"), "{}", r.text);
        });
    }

    // ---- Plan 04 Task 2: every control has a test that fails without it ----

    const GATE_OFF: &str = "model:\n  default: x\n";

    fn save(scope: &ConfigScope, text: &str, sha: &str) -> SaveOutcome {
        save_config_yaml_impl(scope, text, sha, false)
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn save_refuses_when_root_gate_closed_and_leaves_file() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_OFF);
            let r = open(&ConfigScope::Root);
            let out = save(&ConfigScope::Root, "model:\n  default: y\n", &r.sha256);
            assert_eq!(out, SaveOutcome::GateClosed);
            assert_eq!(read(&home.join("config.yaml")), GATE_OFF);
            assert!(backups(home).is_empty());
        });
    }

    #[test]
    fn save_reads_gate_fresh_on_every_call() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let text = format!("{GATE_ON}agent:\n  max_iterations: 2\n");
            assert!(matches!(
                save(&ConfigScope::Root, &text, &r.sha256),
                SaveOutcome::Saved { .. }
            ));
            write(&home.join("config.yaml"), GATE_OFF);
            let r = open(&ConfigScope::Root);
            assert_eq!(
                save(&ConfigScope::Root, &text, &r.sha256),
                SaveOutcome::GateClosed
            );
        });
    }

    #[test]
    fn save_refuses_stale_digest_and_leaves_file() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let changed = format!("{GATE_ON}agent:\n  max_iterations: 7\n");
            write(&home.join("config.yaml"), &changed);
            let out = save(&ConfigScope::Root, GATE_ON, &r.sha256);
            assert_eq!(out, SaveOutcome::ChangedOnDisk);
            assert_eq!(read(&home.join("config.yaml")), changed);
            assert!(backups(home).is_empty());
        });
    }

    #[test]
    fn save_profile_is_gated_by_root_flag_only() {
        with_home(|home| {
            let scope = ConfigScope::Profile("alpha".to_string());
            // Root gate off, the profile's own flag on: refused.
            write(&home.join("config.yaml"), GATE_OFF);
            write(&home.join("profiles/alpha/config.yaml"), GATE_ON);
            let r = open(&scope);
            assert_eq!(
                save(&scope, "agent:\n  max_iterations: 2\n", &r.sha256),
                SaveOutcome::GateClosed
            );
            // Root gate on, the profile's own flag off: saved.
            write(&home.join("config.yaml"), GATE_ON);
            write(&home.join("profiles/alpha/config.yaml"), GATE_OFF);
            let r = open(&scope);
            assert!(matches!(
                save(&scope, "agent:\n  max_iterations: 2\n", &r.sha256),
                SaveOutcome::Saved { .. }
            ));
        });
    }

    #[test]
    fn save_root_lockout_requires_ack_when_set_false() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let off = "security:\n  web_config_write_enabled: false\n";
            let out = save(&ConfigScope::Root, off, &r.sha256);
            assert_eq!(out, SaveOutcome::LockoutAckRequired { removes_key: false });
            assert_eq!(read(&home.join("config.yaml")), GATE_ON);
            assert!(backups(home).is_empty());
        });
    }

    #[test]
    fn save_root_lockout_requires_ack_when_key_removed() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let out = save(
                &ConfigScope::Root,
                "agent:\n  max_iterations: 2\n",
                &r.sha256,
            );
            assert_eq!(out, SaveOutcome::LockoutAckRequired { removes_key: true });
            assert_eq!(read(&home.join("config.yaml")), GATE_ON);
            assert!(backups(home).is_empty());
        });
    }

    #[test]
    fn save_root_lockout_with_ack_writes_and_reports_locked() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let off = "security:\n  web_config_write_enabled: false\n";
            let out = save_config_yaml_impl(&ConfigScope::Root, off, &r.sha256, true);
            assert!(
                matches!(out, SaveOutcome::Saved { locked: true, .. }),
                "{out:?}"
            );
            assert_eq!(read(&home.join("config.yaml")), off);
        });
    }

    #[test]
    fn save_profile_never_triggers_lockout() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            write(&home.join("profiles/alpha/config.yaml"), GATE_ON);
            let scope = ConfigScope::Profile("alpha".to_string());
            let r = open(&scope);
            let off = "security:\n  web_config_write_enabled: false\n";
            let out = save(&scope, off, &r.sha256);
            assert!(
                matches!(out, SaveOutcome::Saved { locked: false, .. }),
                "{out:?}"
            );
        });
    }

    fn valid_backups(dir: &Path) -> Vec<String> {
        backups(dir)
            .into_iter()
            .filter(|n| is_backup_id(n.strip_prefix(BACKUP_PREFIX).unwrap_or("")))
            .collect()
    }

    #[test]
    fn save_prunes_backups_to_ten_and_spares_other_files() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            for d in 1..=10 {
                let id = format!("202601{d:02}T000000000Z");
                write(&home.join(format!("config.yaml.bak-{id}")), "old");
            }
            write(&home.join("config.yaml.bak-evil"), "keep me");
            write(&home.join("notes.txt"), "keep me too");
            let r = open(&ConfigScope::Root);
            let text = format!("{GATE_ON}agent:\n  max_iterations: 2\n");
            assert!(matches!(
                save(&ConfigScope::Root, &text, &r.sha256),
                SaveOutcome::Saved { .. }
            ));
            let valid = valid_backups(home);
            assert_eq!(valid.len(), 10, "{valid:?}");
            assert!(
                !valid.contains(&"config.yaml.bak-20260101T000000000Z".to_string()),
                "oldest must be pruned: {valid:?}"
            );
            assert!(
                valid.contains(&"config.yaml.bak-20260110T000000000Z".to_string()),
                "{valid:?}"
            );
            assert_eq!(read(&home.join("config.yaml.bak-evil")), "keep me");
            assert_eq!(read(&home.join("notes.txt")), "keep me too");
        });
    }

    #[test]
    fn backup_id_collision_gets_numeric_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let id = "20261116T120000123Z";
        assert_eq!(write_backup_with_id(dir.path(), "a", id).unwrap(), id);
        assert_eq!(
            write_backup_with_id(dir.path(), "b", id).unwrap(),
            format!("{id}-1")
        );
    }

    #[test]
    fn is_backup_id_accepts_only_strict_timestamps() {
        assert!(is_backup_id("20261116T120000123Z"));
        assert!(is_backup_id("20261116T120000123Z-7"));
        for bad in [
            "",
            "..",
            "../config.yaml",
            "a/b",
            "20261116T120000123Z/x",
            "20261116T12000012Z",
            "20261116T120000123Z-1234",
        ] {
            assert!(!is_backup_id(bad), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn ids_to_prune_keeps_newest_ten() {
        let ids: Vec<String> = (1..=12)
            .map(|d| format!("202601{d:02}T000000000Z"))
            .collect();
        assert_eq!(ids_to_prune(&ids, 10), ids[..2].to_vec());
        assert!(ids_to_prune(&ids[..10], 10).is_empty());
    }

    #[test]
    fn save_appends_one_audit_row_per_attempt_without_contents() {
        with_home(|home| {
            let body = format!(
                "# very-secret-comment\n{GATE_ON}providers:\n  openai:\n    api_key: \"sk-audit-secret\"\n"
            );
            write(&home.join("config.yaml"), &body);
            let r = open(&ConfigScope::Root);
            let edited = r
                .text
                .replace("# very-secret-comment", "# very-secret-comment2");
            assert!(matches!(
                save(&ConfigScope::Root, &edited, &r.sha256),
                SaveOutcome::Saved { .. }
            ));
            write(&home.join("config.yaml"), GATE_OFF);
            let r = open(&ConfigScope::Root);
            assert_eq!(
                save(&ConfigScope::Root, &edited, &r.sha256),
                SaveOutcome::GateClosed
            );
            let log = read(&home.join("logs/audit.jsonl"));
            let lines: Vec<&str> = log.lines().collect();
            assert_eq!(lines.len(), 2, "{log}");
            for l in &lines {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                assert_eq!(v["kind"], "config_write", "{l}");
            }
            assert!(!log.contains("sk-audit-secret"), "{log}");
            assert!(!log.contains("very-secret-comment"), "{log}");
        });
    }

    #[test]
    fn save_refuses_oversized_and_nul_text() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let big = "#".repeat(MAX_CONFIG_BYTES + 1);
            assert!(matches!(
                save(&ConfigScope::Root, &big, &r.sha256),
                SaveOutcome::Invalid(_)
            ));
            let nul = format!("{GATE_ON}a: 1\0\n");
            assert!(matches!(
                save(&ConfigScope::Root, &nul, &r.sha256),
                SaveOutcome::Invalid(_)
            ));
            assert_eq!(read(&home.join("config.yaml")), GATE_ON);
            assert!(backups(home).is_empty());
        });
    }

    #[test]
    fn save_refuses_legacy_subagent_key_and_server_survives() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let text = format!("{GATE_ON}subagent:\n  x: 1\n");
            let out = save(&ConfigScope::Root, &text, &r.sha256);
            assert!(
                matches!(
                    out,
                    SaveOutcome::Invalid(ConfigYamlIssue { line: Some(_), .. })
                ),
                "{out:?}"
            );
            assert_eq!(read(&home.join("config.yaml")), GATE_ON);
        });
    }

    #[test]
    fn save_returns_unknown_keys_and_still_writes() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let text = format!("{GATE_ON}bogus: 1\n");
            let out = save(&ConfigScope::Root, &text, &r.sha256);
            let SaveOutcome::Saved { unknown_keys, .. } = out else {
                panic!("expected Saved, got {out:?}")
            };
            assert_eq!(unknown_keys, vec!["bogus".to_string()]);
            assert!(read(&home.join("config.yaml")).contains("bogus: 1"));
        });
    }

    #[test]
    fn save_unmatched_placeholder_writes_nothing() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let r = open(&ConfigScope::Root);
            let text = format!("{GATE_ON}providers:\n  openai:\n    api_key: {MASK}\n");
            let out = save(&ConfigScope::Root, &text, &r.sha256);
            assert_eq!(
                out,
                SaveOutcome::UnmatchedPlaceholder {
                    key_path: "providers.openai.api_key".to_string()
                }
            );
            assert_eq!(read(&home.join("config.yaml")), GATE_ON);
            assert!(backups(home).is_empty());
        });
    }

    #[test]
    fn save_after_deleting_list_entry_refuses_and_writes_nothing() {
        with_home(|home| {
            let body = format!(
                "{GATE_ON}custom_providers:\n  - name: a\n    base_url: https://a\n    api_key: sk-a-1111\n  - name: b\n    base_url: https://b\n    api_key: sk-b-2222\n"
            );
            write(&home.join("config.yaml"), &body);
            let r = open(&ConfigScope::Root);
            assert!(!r.text.contains("sk-a-1111"), "{}", r.text);
            // Delete entry `a`: `b` moves to index 0 and keeps its placeholder.
            let start = r.text.find("  - name: a\n").unwrap();
            let end = r.text.find("  - name: b\n").unwrap();
            let edited = format!("{}{}", &r.text[..start], &r.text[end..]);
            let out = save(&ConfigScope::Root, &edited, &r.sha256);
            assert_eq!(
                out,
                SaveOutcome::UnmatchedPlaceholder {
                    key_path: "custom_providers[0].api_key".to_string()
                }
            );
            assert_eq!(read(&home.join("config.yaml")), body);
            assert!(backups(home).is_empty());
        });
    }

    // ---- Plan 05 Task 1: backups list + load ----

    #[test]
    fn list_backups_newest_first_and_ignores_non_matching() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            write(&home.join("config.yaml.bak-20261116T120000100Z"), "a");
            write(&home.join("config.yaml.bak-20261116T120000200Z"), "bb");
            write(&home.join("config.yaml.bak-20261116T120000200Z-1"), "ccc");
            write(&home.join("config.yaml.bak-evil"), "x");
            write(&home.join("config.yaml.tmp.1.2"), "x");
            let got = list_config_backups_impl(&ConfigScope::Root).unwrap();
            let want = [
                ("20261116T120000200Z-1", 3),
                ("20261116T120000200Z", 2),
                ("20261116T120000100Z", 1),
            ];
            assert_eq!(got.len(), want.len(), "{got:?}");
            for (b, (id, size)) in got.iter().zip(want) {
                assert_eq!((b.id.as_str(), b.size_bytes), (id, size));
            }
        });
    }

    #[test]
    fn list_backups_for_profile_reads_profile_dir() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            write(&home.join("config.yaml.bak-20261116T120000100Z"), "root");
            write(&home.join("profiles/alpha/config.yaml"), GATE_OFF);
            write(
                &home.join("profiles/alpha/config.yaml.bak-20261116T120000900Z"),
                "p",
            );
            let got = list_config_backups_impl(&ConfigScope::Profile("alpha".to_string())).unwrap();
            assert_eq!(got.len(), 1, "{got:?}");
            assert_eq!(got[0].id, "20261116T120000900Z");
        });
    }

    #[test]
    fn get_backup_refuses_malformed_ids() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            write(&home.join("secret.txt"), "top-secret");
            for id in [
                "",
                "..",
                "../config.yaml",
                "a/b",
                "20261116T120000123Z/../x",
            ] {
                assert!(
                    matches!(
                        get_config_backup_impl(&ConfigScope::Root, id),
                        BackupText::Unavailable { .. }
                    ),
                    "{id:?} must be refused"
                );
            }
        });
    }

    #[test]
    fn get_backup_missing_id_is_unavailable() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            assert!(matches!(
                get_config_backup_impl(&ConfigScope::Root, "20261116T120000123Z"),
                BackupText::Unavailable { .. }
            ));
        });
    }

    #[cfg(unix)]
    #[test]
    fn get_backup_refuses_symlinked_backup() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            let outside = tempfile::tempdir().unwrap();
            write(&outside.path().join("x.yaml"), "a: 1\n");
            std::os::unix::fs::symlink(
                outside.path().join("x.yaml"),
                home.join("config.yaml.bak-20261116T120000123Z"),
            )
            .unwrap();
            assert!(matches!(
                get_config_backup_impl(&ConfigScope::Root, "20261116T120000123Z"),
                BackupText::Unavailable { .. }
            ));
        });
    }

    #[test]
    fn get_backup_serves_masked_text() {
        with_home(|home| {
            write(&home.join("config.yaml"), GATE_ON);
            write(
                &home.join("config.yaml.bak-20261116T120000123Z"),
                "providers:\n  openai:\n    api_key: sk-bak-42\n",
            );
            let BackupText::Loaded { text } =
                get_config_backup_impl(&ConfigScope::Root, "20261116T120000123Z")
            else {
                panic!("expected Loaded")
            };
            assert!(text.contains(MASK), "{text}");
            assert!(!text.contains("sk-bak-42"), "{text}");
        });
    }

    #[test]
    fn restore_backup_via_ordinary_save_round_trips() {
        with_home(|home| {
            let v1 = "# v1\nsecurity:\n  web_config_write_enabled: true\nproviders:\n  openai:\n    api_key: sk-same-1\n";
            write(&home.join("config.yaml"), v1);
            let r = open(&ConfigScope::Root);
            let v2 = r.text.replace("# v1", "# v2");
            let out = save(&ConfigScope::Root, &v2, &r.sha256);
            assert!(matches!(out, SaveOutcome::Saved { .. }), "{out:?}");
            let ids = list_config_backups_impl(&ConfigScope::Root).unwrap();
            assert_eq!(ids.len(), 1);
            let BackupText::Loaded { text } =
                get_config_backup_impl(&ConfigScope::Root, &ids[0].id)
            else {
                panic!("expected Loaded")
            };
            let cur = open(&ConfigScope::Root);
            let out = save(&ConfigScope::Root, &text, &cur.sha256);
            assert!(matches!(out, SaveOutcome::Saved { .. }), "{out:?}");
            assert_eq!(read(&home.join("config.yaml")), v1);
        });
    }
}
