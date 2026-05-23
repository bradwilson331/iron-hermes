//! IronHermes Skills Hub (Phase 19.1).
//!
//! Publish/install skills from GitHub, skills.sh, and well-known HTTPS origins.
//! Publish flows are deferred per D-12; this crate covers install / search /
//! update / uninstall / trust-management only.

pub mod audit;
pub mod auth;
pub mod blob;
pub mod error;
pub mod github;
pub mod installer;
pub mod local_dir;
pub mod lock;
pub mod manifest;
pub mod paths;
pub mod sanitize;
pub mod scanner;
pub mod source;
pub mod tarball;
pub mod well_known;

/// Process-global serialization lock for tests that mutate the shared
/// `HERMES_HOME` env var. Every `HERMES_HOME`-touching test across all modules
/// MUST hold this single lock — independent per-module mutexes do NOT serialize
/// against each other, so concurrent tests in different modules previously
/// stomped each other's `HERMES_HOME` and produced flaky cross-module failures.
#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub use audit::{AuditData, PartnerAudit, fetch_audit};
pub use auth::GitHubAuth;
pub use blob::{
    BlobSkill, RepoTree, SkillDownloadResponse, SkillSnapshotFile, SkillsShBlobSource, TreeEntry,
};
pub use error::{HubError, HubErrorKind};
pub use github::{GitHubSource, GitHubTap};
pub use installer::{
    InstallOutcome, UninstallOutcome, UpdateOutcome, bundle_content_hash, install, uninstall,
    update,
};
pub use local_dir::LocalDirSource;
pub use lock::{
    MigrationOutcome, SkillLock, SkillLockEntry, compute_folder_hash, migrate_from_hub_manifest,
};
pub use manifest::{HubManifest, ManifestEntry};
pub use sanitize::{
    assert_temp_contained, is_contained_in, is_path_safe, sanitize_metadata, sanitize_name,
    sanitize_subpath, strict_yaml_delimiter, strip_terminal_escapes, to_skill_slug,
};
pub use scanner::{
    AlwaysBlockedScanner, AlwaysCleanScanner, CoreSkillScanner, ScanVerdict, SkillScanner,
    enforce_trust_gate,
};
pub use source::{BundleFile, HubSource, SkillBundle, SkillMeta};
pub use well_known::WellKnownSkillSource;
