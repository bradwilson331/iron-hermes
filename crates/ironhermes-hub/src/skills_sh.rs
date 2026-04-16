//! skills.sh source adapter.
//!
//! Wraps `GitHubSource` — uses the skills.sh registry to map short names to
//! `owner/repo/path` identifiers, then delegates fetching to `GitHubSource`.
//!
//! Preserves `"skills-sh"` as the `source_id` in all returned `SkillBundle`s
//! so the manifest records the correct provenance (D-06/D-09).

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::SkillSource;

use crate::{GitHubSource, HubError, HubErrorKind, HubSource, SkillBundle, SkillMeta};

/// Registry entry shape returned by `https://skills.sh/registry.json`.
#[derive(Debug, serde::Deserialize)]
struct RegistryEntry {
    repo: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

/// Adapter that discovers skills via the skills.sh registry and delegates
/// fetching to the wrapped `GitHubSource`.
pub struct SkillsShSource {
    http: reqwest::Client,
    github: Arc<GitHubSource>,
    registry_url: String,
}

impl SkillsShSource {
    /// Create a new `SkillsShSource` wrapping the provided `GitHubSource`.
    pub fn new(github: Arc<GitHubSource>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("ironhermes-hub/", env!("CARGO_PKG_VERSION")))
                .https_only(true)
                .build()
                .expect("reqwest client"),
            github,
            registry_url: "https://skills.sh/registry.json".to_string(),
        }
    }

    /// Override the registry URL (for tests using wiremock).
    ///
    /// If the URL uses HTTP (wiremock), rebuilds the client without https_only.
    /// Not part of the public API — exposed for integration tests only.
    #[doc(hidden)]
    pub fn with_registry_url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        if url.starts_with("http://") {
            // Rebuild client without https_only so wiremock (HTTP) works
            self.http = reqwest::Client::builder()
                .user_agent(concat!("ironhermes-hub/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client");
        }
        self.registry_url = url;
        self
    }

    /// Fetch the skills.sh registry JSON.
    async fn fetch_registry(&self) -> Result<serde_json::Value, HubError> {
        let resp = self.http.get(&self.registry_url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(HubError::Typed {
                kind: HubErrorKind::Network,
                message: format!("registry fetch returned {status}"),
                suggestion: None,
                retry_after_s: None,
            });
        }
        Ok(resp.json().await?)
    }

    /// Resolve a `skills-sh:<name>` identifier to a GitHub `owner/repo/path` identifier.
    async fn resolve_to_github_identifier(&self, skills_sh_ident: &str) -> Result<String, HubError> {
        let name = skills_sh_ident
            .strip_prefix("skills-sh:")
            .unwrap_or(skills_sh_ident);

        let reg = self.fetch_registry().await?;
        let entry: RegistryEntry = serde_json::from_value(
            reg.get(name)
                .ok_or_else(|| HubError::Typed {
                    kind: HubErrorKind::NotFound,
                    message: format!("skills.sh: '{name}' not found in registry"),
                    suggestion: Some(format!("Run: hermes skills search {name}")),
                    retry_after_s: None,
                })?
                .clone(),
        )
        .map_err(|e| HubError::Typed {
            kind: HubErrorKind::Parse,
            message: format!("failed to parse registry entry for '{name}': {e}"),
            suggestion: None,
            retry_after_s: None,
        })?;

        let path = entry.path.unwrap_or_else(|| name.to_string());
        Ok(format!("{}/{}", entry.repo, path))
    }
}

#[async_trait]
impl HubSource for SkillsShSource {
    fn source_id(&self) -> &str {
        "skills-sh"
    }

    /// Trust is deferred to the install pipeline (D-06/D-08).
    ///
    /// The synchronous trait method cannot hit the registry, so we return
    /// `Community` by default. The install pipeline re-resolves via the async
    /// path and consults `GitHubSource.trust_level_for` on the resolved identifier.
    fn trust_level_for(&self, _identifier: &str) -> SkillSource {
        SkillSource::Community
    }

    /// Search the skills.sh registry for skills matching `query`.
    #[tracing::instrument(skip(self), fields(query, limit))]
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillMeta>, HubError> {
        let reg = self.fetch_registry().await?;
        let map = reg.as_object().ok_or_else(|| HubError::Typed {
            kind: HubErrorKind::Parse,
            message: "skills.sh registry is not a JSON object".to_string(),
            suggestion: None,
            retry_after_s: None,
        })?;

        let q = query.to_lowercase();
        let mut out: Vec<SkillMeta> = Vec::new();

        for (name, raw_entry) in map {
            let entry: RegistryEntry = match serde_json::from_value(raw_entry.clone()) {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Substring match on name + description
            let haystack = format!(
                "{} {}",
                name.to_lowercase(),
                entry.description.as_deref().unwrap_or("").to_lowercase()
            );
            if !q.is_empty() && !haystack.contains(&q) {
                continue;
            }

            out.push(SkillMeta {
                name: name.clone(),
                identifier: format!("skills-sh:{name}"),
                source_id: "skills-sh".to_string(),
                description: entry.description,
                version: entry.version,
            });

            if out.len() >= limit {
                break;
            }
        }

        Ok(out)
    }

    /// Fetch a skill via the skills.sh registry, delegating to `GitHubSource`.
    ///
    /// Re-stamps `source_id = "skills-sh"` and `identifier` in the returned
    /// `SkillBundle` to preserve provenance for the manifest (D-06/D-09).
    #[tracing::instrument(skip(self), fields(identifier))]
    async fn fetch(&self, identifier: &str) -> Result<SkillBundle, HubError> {
        let gh_id = self.resolve_to_github_identifier(identifier).await?;
        let mut bundle = HubSource::fetch(self.github.as_ref(), &gh_id).await?;

        // Re-stamp provenance (D-06/D-09) so the manifest records "skills-sh",
        // not "github".
        bundle.source_id = "skills-sh".to_string();
        bundle.identifier = identifier.to_string();

        Ok(bundle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_is_skills_sh() {
        let github = Arc::new(GitHubSource::new(
            crate::GitHubAuth::anonymous(),
            std::collections::HashSet::new(),
            vec![],
        ));
        let src = SkillsShSource::new(github);
        assert_eq!(src.source_id(), "skills-sh");
    }

    #[test]
    fn trust_level_for_always_community() {
        let github = Arc::new(GitHubSource::new(
            crate::GitHubAuth::anonymous(),
            std::collections::HashSet::new(),
            vec![],
        ));
        let src = SkillsShSource::new(github);
        assert_eq!(src.trust_level_for("skills-sh:tenor-gif"), SkillSource::Community);
    }
}
