//! IronHermes Skills Hub (Phase 19.1).
//!
//! Publish/install skills from GitHub, skills.sh, and well-known HTTPS origins.
//! Publish flows are deferred per D-12; this crate covers install / search /
//! update / uninstall / trust-management only.

pub mod auth;
pub mod error;
pub mod github;
pub mod installer;
pub mod manifest;
pub mod paths;
pub mod sanitize;
pub mod scanner;
pub mod skills_sh;
pub mod source;
pub mod tarball;
pub mod well_known;

pub use auth::GitHubAuth;
pub use error::{HubError, HubErrorKind};
pub use github::{GitHubSource, GitHubTap};
pub use installer::{
    bundle_content_hash, install, uninstall, update, InstallOutcome, UninstallOutcome,
    UpdateOutcome,
};
pub use manifest::{HubManifest, ManifestEntry};
pub use scanner::{
    enforce_trust_gate, AlwaysBlockedScanner, AlwaysCleanScanner, CoreSkillScanner, ScanVerdict,
    SkillScanner,
};
pub use skills_sh::SkillsShSource;
pub use source::{BundleFile, HubSource, SkillBundle, SkillMeta};
pub use well_known::WellKnownSkillSource;
