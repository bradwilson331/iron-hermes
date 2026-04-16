//! IronHermes Skills Hub (Phase 19.1).
//!
//! Publish/install skills from GitHub, skills.sh, and well-known HTTPS origins.
//! Publish flows are deferred per D-12; this crate covers install / search /
//! update / uninstall / trust-management only.

pub mod auth;
pub mod error;
pub mod manifest;
pub mod paths;
pub mod source;

pub use auth::GitHubAuth;
pub use error::{HubError, HubErrorKind};
pub use manifest::{HubManifest, ManifestEntry};
pub use source::{BundleFile, HubSource, SkillBundle, SkillMeta};
