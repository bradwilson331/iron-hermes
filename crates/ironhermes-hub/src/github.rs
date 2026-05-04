//! GitHub adapter for the IronHermes Skills Hub.
//!
//! Implements `HubSource` by searching GitHub repository trees (Trees API)
//! and downloading skills as gzipped tarballs.
//!
//! Security mitigations:
//! - T-19.1-02-01: tarball path-traversal rejected via `tarball::validate_bundle_rel_path`
//! - T-19.1-02-02: size/entry caps via `tarball::MAX_EXTRACTED_BYTES` / `MAX_ENTRIES`
//! - T-19.1-02-04: trust gate uses only `owner/repo` prefix (first 2 path segments)
//! - T-19.1-02-06: 403 + X-RateLimit-Remaining=0 → RateLimited with retry_after_s
//! - T-19.1-02-07: auth header never logged (tracing::instrument skips auth param)

use std::collections::HashSet;

use async_trait::async_trait;
use ironhermes_core::SkillSource;
use tracing::warn;

use crate::{
    BundleFile, GitHubAuth, HubError, HubErrorKind, HubSource, SkillBundle, SkillMeta,
    tarball::extract_tarball_prefix,
};

// ── Types ────────────────────────────────────────────────────────────────────

/// A GitHub repository tap (owner/repo + optional subdirectory path).
#[derive(Debug, Clone)]
pub struct GitHubTap {
    /// `"owner/repo"` format.
    pub repo: String,
    /// Optional subdirectory prefix within the repo (e.g. `"skills/"`).
    pub path_prefix: Option<String>,
}

/// GitHub source adapter.
///
/// Searches `DEFAULT_TAPS` + any `extra_taps` from config, and downloads
/// skill bundles via the GitHub tarball endpoint.
pub struct GitHubSource {
    http: reqwest::Client,
    auth: GitHubAuth,
    trusted_repos: HashSet<String>,
    taps: Vec<GitHubTap>,
    /// API base URL — overridable in tests via `with_api_base`.
    api_base: String,
}

impl GitHubSource {
    /// Default taps: ports hermes-agent `DEFAULT_TAPS` exactly (D-02).
    pub const DEFAULT_TAPS: &'static [(&'static str, Option<&'static str>)] = &[
        ("openai/skills", None),
        ("anthropics/skills", None),
        ("VoltAgent/awesome-agent-skills", None),
        ("garrytan/gstack", None),
    ];

    /// Create a new `GitHubSource`.
    ///
    /// `trusted_repos` should come from `config.hub.trusted_repos_set()` (D-04/D-08).
    /// `extra_taps` are appended after `DEFAULT_TAPS` (D-02).
    pub fn new(
        auth: GitHubAuth,
        trusted_repos: HashSet<String>,
        extra_taps: Vec<GitHubTap>,
    ) -> Self {
        let mut taps: Vec<GitHubTap> = Self::DEFAULT_TAPS
            .iter()
            .map(|(repo, p)| GitHubTap {
                repo: (*repo).to_string(),
                path_prefix: p.map(|s| s.to_string()),
            })
            .collect();
        taps.extend(extra_taps);

        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("ironhermes-hub/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client"),
            auth,
            trusted_repos,
            taps,
            api_base: "https://api.github.com".to_string(),
        }
    }

    /// Override the API base URL (for tests using wiremock).
    ///
    /// Not part of the public API — exposed for integration tests only.
    #[doc(hidden)]
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }

    /// Accessor for the resolved GitHub auth. Exposed so sibling adapters
    /// (e.g. `SkillsShBlobSource`) can reuse the Phase 19.1 token-resolution
    /// precedence without re-probing env vars or shelling out to `gh`.
    pub fn auth(&self) -> &GitHubAuth {
        &self.auth
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    /// Resolve the default branch for a repo via `GET /repos/{repo}`.
    async fn default_branch(&self, repo: &str) -> Result<String, HubError> {
        let url = format!("{}/repos/{}", self.api_base, repo);
        let mut req = self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github.v3+json");
        if let Some(h) = self.auth.authorization_header() {
            req = req.header("Authorization", h);
        }
        let resp = req.send().await?;
        let status = resp.status();

        if status == 401 {
            return Err(HubError::Typed {
                kind: HubErrorKind::AuthRequired,
                message: format!("GitHub auth required for {repo}"),
                suggestion: Some("Set HERMES_GITHUB_TOKEN or run: gh auth login".to_string()),
                retry_after_s: None,
            });
        }
        if status == 404 {
            return Err(HubError::Typed {
                kind: HubErrorKind::NotFound,
                message: format!("repo not found: {repo}"),
                suggestion: None,
                retry_after_s: None,
            });
        }
        if status == 403 {
            return Err(self.rate_limit_error(resp).await);
        }
        if !status.is_success() {
            return Err(HubError::Typed {
                kind: HubErrorKind::Network,
                message: format!("GET /repos/{repo} returned {status}"),
                suggestion: None,
                retry_after_s: None,
            });
        }

        let body: serde_json::Value = resp.json().await?;
        Ok(body
            .get("default_branch")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .to_string())
    }

    /// Fetch the recursive git tree for a repo at `sha` (branch name or SHA).
    async fn git_tree(&self, repo: &str, sha: &str) -> Result<serde_json::Value, HubError> {
        let url = format!("{}/repos/{}/git/trees/{}", self.api_base, repo, sha);
        let mut req = self
            .http
            .get(&url)
            .query(&[("recursive", "1")])
            .header("Accept", "application/vnd.github.v3+json");
        if let Some(h) = self.auth.authorization_header() {
            req = req.header("Authorization", h);
        }
        let resp = req.send().await?;
        let status = resp.status();

        if status == 403 {
            return Err(self.rate_limit_error(resp).await);
        }
        if status == 404 {
            return Err(HubError::Typed {
                kind: HubErrorKind::NotFound,
                message: format!("tree not found: {repo}/{sha}"),
                suggestion: None,
                retry_after_s: None,
            });
        }
        if !status.is_success() {
            return Err(HubError::Typed {
                kind: HubErrorKind::Network,
                message: format!("GET /repos/{repo}/git/trees/{sha} returned {status}"),
                suggestion: None,
                retry_after_s: None,
            });
        }

        Ok(resp.json().await?)
    }

    /// Build a `RateLimited` error from a 403 response, parsing `X-RateLimit-Reset`.
    async fn rate_limit_error(&self, resp: reqwest::Response) -> HubError {
        let remaining = resp
            .headers()
            .get("X-RateLimit-Remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let retry_after_s = if remaining == 0 {
            resp.headers()
                .get("X-RateLimit-Reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<i64>().ok())
                .map(|reset_ts| {
                    let now = chrono::Utc::now().timestamp();
                    (reset_ts - now).max(0) as u64
                })
        } else {
            None
        };

        HubError::Typed {
            kind: HubErrorKind::RateLimited,
            message: "GitHub API rate limit exceeded".to_string(),
            suggestion: Some(
                "Set HERMES_GITHUB_TOKEN or wait for rate-limit window to reset".to_string(),
            ),
            retry_after_s,
        }
    }

    /// List skills in a tap by searching the recursive tree for `SKILL.md` paths.
    async fn search_tap(&self, tap: &GitHubTap, query: &str) -> Result<Vec<SkillMeta>, HubError> {
        let branch = self.default_branch(&tap.repo).await?;
        let tree = self.git_tree(&tap.repo, &branch).await?;

        let entries =
            tree.get("tree")
                .and_then(|t| t.as_array())
                .ok_or_else(|| HubError::Typed {
                    kind: HubErrorKind::Parse,
                    message: "git tree response missing 'tree' array".to_string(),
                    suggestion: None,
                    retry_after_s: None,
                })?;

        let q = query.to_lowercase();
        let mut skills: Vec<SkillMeta> = Vec::new();
        let prefix = tap.path_prefix.as_deref().unwrap_or("");

        for entry in entries {
            let entry_path = match entry.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => continue,
            };

            // Only look at SKILL.md files
            if !entry_path.ends_with("/SKILL.md") && entry_path != "SKILL.md" {
                continue;
            }
            // Apply path_prefix filter
            if !prefix.is_empty() && !entry_path.starts_with(prefix) {
                continue;
            }

            // skill_dir = path without trailing /SKILL.md (or bare "SKILL.md" at root)
            let skill_dir = entry_path
                .strip_suffix("/SKILL.md")
                .unwrap_or_else(|| entry_path.strip_suffix("SKILL.md").unwrap_or(entry_path));
            // skill_name = last component (or empty-string guard for root SKILL.md)
            let skill_name = if skill_dir.is_empty() {
                tap.repo.rsplit('/').next().unwrap_or(&tap.repo)
            } else {
                skill_dir.rsplit('/').next().unwrap_or(skill_dir)
            };

            // Substring match against query
            if !q.is_empty() && !skill_name.to_lowercase().contains(&q) {
                continue;
            }

            let identifier = if skill_dir.is_empty() {
                tap.repo.clone()
            } else {
                format!("{}/{}", tap.repo, skill_dir)
            };
            skills.push(SkillMeta {
                name: skill_name.to_string(),
                identifier,
                source_id: "github".to_string(),
                description: None,
                version: None,
            });
        }

        Ok(skills)
    }

    /// Download the tarball for `owner/repo` at HEAD.
    async fn download_tarball(&self, owner: &str, repo: &str) -> Result<Vec<u8>, HubError> {
        let url = format!("{}/repos/{}/{}/tarball/HEAD", self.api_base, owner, repo);
        let mut req = self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github.v3+json");
        if let Some(h) = self.auth.authorization_header() {
            req = req.header("Authorization", h);
        }
        let resp = req.send().await?;
        let status = resp.status();

        if status == 403 {
            return Err(self.rate_limit_error(resp).await);
        }
        if status == 404 {
            return Err(HubError::Typed {
                kind: HubErrorKind::NotFound,
                message: format!("tarball not found for {owner}/{repo}"),
                suggestion: None,
                retry_after_s: None,
            });
        }
        if !status.is_success() {
            return Err(HubError::Typed {
                kind: HubErrorKind::Network,
                message: format!("tarball download returned {status}"),
                suggestion: None,
                retry_after_s: None,
            });
        }

        Ok(resp.bytes().await?.to_vec())
    }

    /// Discover the GitHub-style top-level prefix dir in a tarball.
    ///
    /// GitHub tarballs wrap everything in `{owner}-{repo}-{sha}/`.
    /// We find the first directory component to use as the prefix.
    fn detect_tarball_prefix(bytes: &[u8]) -> Option<String> {
        use flate2::read::GzDecoder;
        use tar::Archive;

        let gz = GzDecoder::new(bytes);
        let mut ar = Archive::new(gz);

        for entry in ar.entries().ok()? {
            let entry = entry.ok()?;
            let raw = entry.path().ok()?.to_string_lossy().into_owned();
            // The top-level dir is the first component of any path.
            if let Some(slash_pos) = raw.find('/') {
                let prefix = &raw[..slash_pos + 1]; // e.g. "anthropics-skills-abc123/"
                return Some(prefix.to_string());
            }
        }
        None
    }

    /// Parse SKILL.md frontmatter from the bundle files. Returns the raw SKILL.md content.
    fn find_skill_md(files: &[BundleFile]) -> Option<&BundleFile> {
        files.iter().find(|f| f.path == "SKILL.md")
    }

    /// Parse a simple `name: <value>` field from frontmatter YAML.
    fn parse_frontmatter_name(content: &str) -> Option<String> {
        if !content.trim_start().starts_with("---") {
            return None;
        }
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("name:") {
                let name = rest.trim().trim_matches('"').trim_matches('\'');
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
        None
    }
}

#[async_trait]
impl HubSource for GitHubSource {
    fn source_id(&self) -> &str {
        "github"
    }

    /// Determine trust level from `owner/repo` prefix of identifier (D-06/T-19.1-02-04).
    ///
    /// Only the first two path segments (`owner/repo`) are used — path components
    /// are ignored so a sub-path can't inflate trust.
    fn trust_level_for(&self, identifier: &str) -> SkillSource {
        let parts: Vec<&str> = identifier.splitn(3, '/').collect();
        if parts.len() < 2 {
            return SkillSource::Community;
        }
        let repo = format!("{}/{}", parts[0], parts[1]);
        if self.trusted_repos.contains(&repo) {
            SkillSource::Trusted
        } else {
            SkillSource::Community
        }
    }

    /// Search all taps for skills matching `query`.
    ///
    /// Per-tap errors are logged and skipped; an empty result is returned only
    /// if all taps fail. A `RateLimited` error from any tap is propagated
    /// immediately (fast-fail).
    #[tracing::instrument(skip(self), fields(query, limit))]
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillMeta>, HubError> {
        let mut out: Vec<SkillMeta> = Vec::new();
        let mut last_rate_limit: Option<HubError> = None;

        for tap in &self.taps {
            match self.search_tap(tap, query).await {
                Ok(mut results) => {
                    out.append(&mut results);
                    if out.len() >= limit {
                        break;
                    }
                }
                Err(
                    e @ HubError::Typed {
                        kind: HubErrorKind::RateLimited,
                        ..
                    },
                ) => {
                    last_rate_limit = Some(e);
                    // Propagate rate-limit immediately — no point trying other taps.
                    break;
                }
                Err(e) => {
                    warn!(tap = %tap.repo, err = %e, "skipping tap due to error");
                }
            }
        }

        // If we got nothing and hit a rate-limit, surface that error.
        if out.is_empty() {
            if let Some(e) = last_rate_limit {
                return Err(e);
            }
        }

        out.truncate(limit);
        Ok(out)
    }

    /// Download and extract a skill bundle from GitHub.
    ///
    /// `identifier` format: `"owner/repo/skill_path"` (at least 2 slashes).
    #[tracing::instrument(skip(self), fields(identifier))]
    async fn fetch(&self, identifier: &str) -> Result<SkillBundle, HubError> {
        // Parse identifier: must be "owner/repo/skill_path"
        let parts: Vec<&str> = identifier.splitn(3, '/').collect();
        if parts.len() < 3 {
            return Err(HubError::Typed {
                kind: HubErrorKind::InvalidIdentifier,
                message: format!("identifier must be 'owner/repo/skill_path', got: {identifier}"),
                suggestion: Some("Example: anthropics/skills/tenor-gif".to_string()),
                retry_after_s: None,
            });
        }
        let owner = parts[0];
        let repo = parts[1];
        let skill_path = parts[2];

        let tarball_bytes = self.download_tarball(owner, repo).await?;

        // Detect the GitHub-style top-level prefix (e.g. "anthropics-skills-abc123/").
        let top_prefix = Self::detect_tarball_prefix(&tarball_bytes).unwrap_or_default();

        // keep_prefix = top-level dir + skill_path + "/"
        let keep_prefix = if top_prefix.is_empty() {
            format!("{skill_path}/")
        } else {
            format!("{top_prefix}{skill_path}/")
        };

        let files = extract_tarball_prefix(&tarball_bytes, &keep_prefix)?;

        if files.is_empty() {
            return Err(HubError::Typed {
                kind: HubErrorKind::NotFound,
                message: format!("no files found under '{keep_prefix}' in tarball"),
                suggestion: Some(format!(
                    "Check that the skill path '{skill_path}' exists in {owner}/{repo}"
                )),
                retry_after_s: None,
            });
        }

        // Confirm SKILL.md is present.
        let skill_md_file = Self::find_skill_md(&files).ok_or_else(|| HubError::Typed {
            kind: HubErrorKind::Parse,
            message: format!("SKILL.md not found in bundle for {identifier}"),
            suggestion: None,
            retry_after_s: None,
        })?;

        let skill_md = String::from_utf8_lossy(&skill_md_file.bytes).into_owned();
        let skill_name = Self::parse_frontmatter_name(&skill_md).unwrap_or_else(|| {
            skill_path
                .rsplit('/')
                .next()
                .unwrap_or(skill_path)
                .to_string()
        });

        Ok(SkillBundle {
            name: skill_name,
            identifier: identifier.to_string(),
            source_id: "github".to_string(),
            files,
            skill_md,
            metadata: serde_json::json!({
                "owner": owner,
                "repo": repo,
                "skill_path": skill_path,
            }),
            snapshot_hash: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_taps_count_and_order() {
        assert_eq!(GitHubSource::DEFAULT_TAPS.len(), 4);
        assert_eq!(GitHubSource::DEFAULT_TAPS[0].0, "openai/skills");
        assert_eq!(GitHubSource::DEFAULT_TAPS[1].0, "anthropics/skills");
        assert_eq!(
            GitHubSource::DEFAULT_TAPS[2].0,
            "VoltAgent/awesome-agent-skills"
        );
        assert_eq!(GitHubSource::DEFAULT_TAPS[3].0, "garrytan/gstack");
    }

    #[test]
    fn trust_level_uses_only_owner_repo() {
        let trusted: HashSet<String> = ["owner/repo"].iter().map(|s| s.to_string()).collect();
        let src = GitHubSource::new(GitHubAuth::anonymous(), trusted, vec![]);
        // Exact match
        assert_eq!(
            src.trust_level_for("owner/repo/skill"),
            SkillSource::Trusted
        );
        // Sub-paths don't escape trust boundary
        assert_eq!(
            src.trust_level_for("owner/repo/a/b/c"),
            SkillSource::Trusted
        );
        // Different repo
        assert_eq!(
            src.trust_level_for("owner/other/skill"),
            SkillSource::Community
        );
        // Too short
        assert_eq!(src.trust_level_for("owner"), SkillSource::Community);
    }
}
