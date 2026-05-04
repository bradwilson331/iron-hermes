//! Three-hop skills.sh blob API adapter (replaces the deleted `skills_sh.rs`).
//!
//! Flow per D-06 (CORRECTED — see RESEARCH.md §Summary):
//!   1. GitHub Trees API (`api.github.com/repos/<owner>/<repo>/git/trees/<ref>?recursive=1`)
//!   2. Raw frontmatter (`raw.githubusercontent.com/<owner>/<repo>/<ref>/<path>`)
//!   3. skills.sh blob download (`<DOWNLOAD_BASE_URL>/api/download/<owner>/<repo>/<slug>`
//!      where `<slug> = to_skill_slug(frontmatter_name)` — PATH-BASED, NOT query string.
//!
//! Response from hop 3 is JSON: `{files: [{path, contents}], hash}` (plain strings, no tarball).
//!
//! Security: every server-originated string runs through `sanitize::sanitize_metadata`;
//! frontmatter runs through `sanitize::strict_yaml_delimiter`; every file path runs
//! through `sanitize::sanitize_subpath` before any filesystem write (installer.rs does this).
//!
//! Retry: each HTTP call is wrapped in `with_one_retry` — exactly one retry on transient
//! errors (5xx, timeout, connect). No retry on 404, PathTraversal, ScanHit (D-24).

use crate::github::GitHubSource;
use crate::sanitize;
use crate::source::{BundleFile, HubSource, SkillBundle, SkillMeta};
use crate::{HubError, HubErrorKind};
use async_trait::async_trait;
use ironhermes_core::SkillSource;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

// SP-1: per-module typed().
fn typed(kind: HubErrorKind, msg: impl Into<String>) -> HubError {
    HubError::Typed {
        kind,
        message: msg.into(),
        suggestion: None,
        retry_after_s: None,
    }
}

// ============================================================================
// Constants (D-07 override point)
// ============================================================================

const DEFAULT_DOWNLOAD_BASE_URL: &str = "https://skills.sh";
const FETCH_TIMEOUT: Duration = Duration::from_secs(10); // D-08

/// D-22: User-Agent "ironhermes/<ver> (via openclaw)" — tags ironhermes as riding on the
/// openclaw agent identity until upstream adds an `ironhermes` AgentType. Use this string
/// for the blob adapter ONLY; `github.rs` and `well_known.rs` retain "ironhermes-hub/<ver>".
fn user_agent_string() -> String {
    format!("ironhermes/{} (via openclaw)", env!("CARGO_PKG_VERSION"))
}

/// D-07: resolves base URL from env; test harnesses set SKILLS_DOWNLOAD_URL to wiremock origin.
fn resolve_download_base_url() -> String {
    std::env::var("SKILLS_DOWNLOAD_URL").unwrap_or_else(|_| DEFAULT_DOWNLOAD_BASE_URL.to_string())
}

/// Optional test overrides for the GitHub-facing hops so CLI subprocess
/// integration tests can re-target ALL THREE hops (Trees API + raw +
/// /api/download) at a single wiremock origin without using the in-process
/// `new_http_for_tests` + `with_upstream_bases` fluent builder.
///
/// Prod defaults are applied when the env var is unset. These are read at
/// construction time only; a restart of the process is required to change
/// them at runtime (matches the skills.sh mirror-override convention).
fn resolve_github_api_base() -> String {
    std::env::var("GITHUB_API_BASE").unwrap_or_else(|_| "https://api.github.com".to_string())
}

fn resolve_raw_content_base() -> String {
    std::env::var("GITHUB_RAW_CONTENT_BASE")
        .unwrap_or_else(|_| "https://raw.githubusercontent.com".to_string())
}

/// If any of the three hop bases were overridden to an `http://` URL
/// (typical for wiremock / local mirrors), relax the HTTPS-only constraint.
/// Without this, `SkillsShBlobSource::new` rejects the wiremock hop at the
/// reqwest layer because `https_only(true)` refuses plain-HTTP URIs.
/// Test mirrors are responsible for operating on loopback / private networks;
/// SSRF defense remains intact for production (default) URIs.
fn any_override_is_http() -> bool {
    let is_http = |s: &str| s.starts_with("http://");
    is_http(&resolve_download_base_url())
        || is_http(&resolve_github_api_base())
        || is_http(&resolve_raw_content_base())
}

// ============================================================================
// Types (RESEARCH.md §Type Definitions — verbatim)
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub sha: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct RepoTree {
    pub sha: String,
    pub branch: String,
    pub tree: Vec<TreeEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillSnapshotFile {
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillDownloadResponse {
    pub files: Vec<SkillSnapshotFile>,
    pub hash: String, // maps to snapshotHash in the lock file (D-14)
}

/// Single hit returned from `/api/search?q=<name>`. `id` is `"owner/repo/slug"`.
/// `skill_id` and `name` are used to prefer exact-name matches over fuzzy ones
/// when the user types a bare skill name.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillSearchHit {
    pub id: String,
    #[serde(rename = "skillId", default)]
    pub skill_id: String,
    #[serde(default)]
    pub name: String,
}

/// Response shape for `/api/search?q=<name>` (skills.sh fuzzy search).
#[derive(Debug, Clone, Deserialize)]
pub struct SkillSearchResponse {
    #[serde(default)]
    pub skills: Vec<SkillSearchHit>,
}

#[derive(Debug, Clone)]
pub struct BlobSkill {
    pub name: String,
    pub description: String,
    pub raw_content: String,
    pub metadata: Option<serde_json::Value>,
    pub files: Vec<SkillSnapshotFile>,
    pub snapshot_hash: String,
    pub repo_path: String,
}

// ============================================================================
// Adapter struct
// ============================================================================

pub struct SkillsShBlobSource {
    http: Client,
    github: Arc<GitHubSource>, // reuse auth machinery per D-05
    download_base_url: String, // override point for tests and SKILLS_DOWNLOAD_URL
    #[doc(hidden)]
    pub test_mode: bool,
    /// Optional override for api.github.com — set by tests that mock the Trees API.
    #[doc(hidden)]
    pub github_api_base: String,
    /// Optional override for raw.githubusercontent.com — set by tests.
    #[doc(hidden)]
    pub raw_content_base: String,
}

impl SkillsShBlobSource {
    /// Production constructor — HTTPS-only reqwest client with D-22 User-Agent and D-08 timeout.
    ///
    /// Env-driven URL overrides for all three hops are honored (D-07 +
    /// GITHUB_API_BASE / GITHUB_RAW_CONTENT_BASE). If any override points to
    /// an `http://` URL (typical for wiremock / local mirrors), `https_only`
    /// is relaxed so the subprocess-based integration tests can drive the
    /// full pipeline against a local mock server; for production (default)
    /// URIs the HTTPS-only constraint remains active.
    pub fn new(github: Arc<GitHubSource>) -> Self {
        let mut builder = Client::builder()
            .user_agent(user_agent_string())
            .timeout(FETCH_TIMEOUT);
        if !any_override_is_http() {
            builder = builder.https_only(true);
        }
        let http = builder.build().expect("reqwest client");
        Self {
            http,
            github,
            download_base_url: resolve_download_base_url(),
            test_mode: false,
            github_api_base: resolve_github_api_base(),
            raw_content_base: resolve_raw_content_base(),
        }
    }

    /// Test constructor — allows HTTP (wiremock), overrideable base URL for the `/api/download` hop.
    #[doc(hidden)]
    pub fn new_http_for_tests(github: Arc<GitHubSource>, download_base_url: String) -> Self {
        let http = Client::builder()
            .user_agent(user_agent_string())
            .timeout(FETCH_TIMEOUT)
            .build()
            .expect("reqwest client");
        Self {
            http,
            github,
            download_base_url,
            test_mode: true,
            github_api_base: "https://api.github.com".to_string(),
            raw_content_base: "https://raw.githubusercontent.com".to_string(),
        }
    }

    /// Plumb test overrides for the two upstream hops (Trees API + raw.githubusercontent).
    #[doc(hidden)]
    pub fn with_upstream_bases(
        mut self,
        github_api_base: impl Into<String>,
        raw_content_base: impl Into<String>,
    ) -> Self {
        self.github_api_base = github_api_base.into();
        self.raw_content_base = raw_content_base.into();
        self
    }

    /// D-06 corrected: path-based URL for the /api/download hop.
    pub(crate) fn build_download_url(&self, owner: &str, repo: &str, slug: &str) -> String {
        format!(
            "{}/api/download/{}/{}/{}",
            self.download_base_url.trim_end_matches('/'),
            owner,
            repo,
            slug
        )
    }

    // Full three-hop orchestration: resolve identifier → fetch tree → pick SKILL.md → fetch
    // frontmatter → extract `name` → compute slug → fetch /api/download → sanitize → return.
    #[tracing::instrument(skip(self), fields(identifier))]
    async fn fetch_blob_skill(&self, identifier: &str) -> Result<BlobSkill, HubError> {
        // Bare-name resolution: if identifier has no '/', look it up via skills.sh
        // /api/search?q=<name> and take the top hit's `id` (owner/repo/slug).
        // This gives `hermes skills install ascii-art` parity with the Python tool.
        let resolved = if identifier.contains('/') {
            identifier.to_string()
        } else {
            self.resolve_bare_name(identifier).await?
        };

        // Parse identifier: "owner/repo/skill_path" (at least 3 components).
        let parts: Vec<&str> = resolved.splitn(3, '/').collect();
        if parts.len() < 3 {
            return Err(typed(
                HubErrorKind::InvalidIdentifier,
                format!("identifier must be 'owner/repo/skill_path', got: {resolved}"),
            ));
        }
        let owner = parts[0];
        let repo = parts[1];
        let skill_path = parts[2];
        let owner_repo = format!("{owner}/{repo}");

        // Hop 1: GitHub Trees API
        let tree = self.fetch_repo_tree(&owner_repo, None).await?;

        // Locate the SKILL.md file within the requested skill path. The
        // skills.sh `id` shape `owner/repo/slug` doesn't always map to a
        // directory: single-skill repos keep `SKILL.md` at root, multi-skill
        // repos nest under arbitrary prefixes (e.g.
        // `optional-skills/creative/ascii-video/`, `cli-tool/components/skills/<cat>/<slug>/`).
        // Walk the tree and pick the best match by priority:
        //   1. exact `<folder>/SKILL.md`
        //   2. shallowest `*/<folder>/SKILL.md`
        //   3. root `SKILL.md`
        let folder_norm = skill_path.trim_end_matches('/').replace('\\', "/");
        let skill_md_entry = find_skill_md_in_tree(&tree.tree, &folder_norm).ok_or_else(|| {
            typed(
                HubErrorKind::NotFound,
                format!("SKILL.md not found for slug '{folder_norm}' in {owner_repo}"),
            )
        })?;

        // Hop 2: raw.githubusercontent
        let raw_content = self
            .fetch_skill_md_content(&owner_repo, &tree.branch, &skill_md_entry.path)
            .await?;

        // Enforce YAML-only frontmatter (D-17) then parse to get `name` + `description`.
        sanitize::strict_yaml_delimiter(&raw_content)?;
        let (name_raw, description_raw, metadata_value) = parse_frontmatter_fields(&raw_content)?;
        let safe_name = sanitize::sanitize_metadata(&name_raw);
        let safe_description = sanitize::sanitize_metadata(&description_raw);
        if safe_name.is_empty() {
            return Err(typed(
                HubErrorKind::Parse,
                "frontmatter missing required 'name' field",
            ));
        }

        // Compute slug from sanitized name.
        let slug = sanitize::to_skill_slug(&safe_name);
        if slug.is_empty() {
            return Err(typed(
                HubErrorKind::Parse,
                format!("frontmatter 'name' has no URL-safe characters: {safe_name}"),
            ));
        }

        // Hop 3: skills.sh /api/download
        let download = self.fetch_skill_download(owner, repo, &slug).await?;

        // Sanitize every file path server returned. PathTraversal short-circuits (D-24).
        let mut safe_files: Vec<SkillSnapshotFile> = Vec::with_capacity(download.files.len());
        for f in download.files {
            let safe_path = sanitize::sanitize_subpath(&f.path)?;
            safe_files.push(SkillSnapshotFile {
                path: safe_path,
                contents: f.contents,
            });
        }

        Ok(BlobSkill {
            name: safe_name,
            description: safe_description,
            raw_content,
            metadata: metadata_value,
            files: safe_files,
            snapshot_hash: download.hash,
            repo_path: skill_md_entry.path.clone(),
        })
    }

    /// Hop 1: GitHub Trees API. Returns `(sha, branch, tree[])` for first branch that succeeds.
    async fn fetch_repo_tree(
        &self,
        owner_repo: &str,
        git_ref: Option<&str>,
    ) -> Result<RepoTree, HubError> {
        let branches: Vec<String> = match git_ref {
            Some(r) => vec![r.to_string()],
            None => vec!["HEAD".to_string(), "main".to_string(), "master".to_string()],
        };

        let auth_header = self.github.auth().authorization_header();
        let mut last_err: Option<HubError> = None;

        for branch in &branches {
            let branch_ref = branch.clone();
            let url = format!(
                "{}/repos/{}/git/trees/{}",
                self.github_api_base.trim_end_matches('/'),
                owner_repo,
                urlencoding(&branch_ref),
            );
            let auth_hdr = auth_header.clone();
            let http = self.http.clone();

            let op = || {
                let url = url.clone();
                let auth = auth_hdr.clone();
                let http = http.clone();
                async move {
                    let mut req = http
                        .get(&url)
                        .query(&[("recursive", "1")])
                        .header("Accept", "application/vnd.github.v3+json");
                    if let Some(h) = &auth {
                        req = req.header("Authorization", h.clone());
                    }
                    let resp = req.send().await.map_err(HubError::Reqwest)?;
                    let status = resp.status();
                    if status == 404 {
                        return Err(typed(
                            HubErrorKind::NotFound,
                            format!("tree not found: {url}"),
                        ));
                    }
                    if status == 403 {
                        return Err(typed(
                            HubErrorKind::RateLimited,
                            format!("GitHub API 403 (rate limited): {url}"),
                        ));
                    }
                    if !status.is_success() {
                        return Err(typed(
                            HubErrorKind::Network,
                            format!("GET {url} returned {status}"),
                        ));
                    }
                    let body: serde_json::Value = resp.json().await.map_err(HubError::Reqwest)?;
                    Ok::<serde_json::Value, HubError>(body)
                }
            };

            match with_one_retry(op).await {
                Ok(body) => {
                    let sha = body
                        .get("sha")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let tree_arr = body
                        .get("tree")
                        .and_then(|v| v.as_array())
                        .ok_or_else(|| {
                            typed(
                                HubErrorKind::Parse,
                                "git tree response missing 'tree' array",
                            )
                        })?
                        .clone();
                    let entries: Vec<TreeEntry> = tree_arr
                        .into_iter()
                        .filter_map(|e| serde_json::from_value::<TreeEntry>(e).ok())
                        .collect();
                    return Ok(RepoTree {
                        sha,
                        branch: branch_ref,
                        tree: entries,
                    });
                }
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            typed(
                HubErrorKind::NotFound,
                format!("no branches resolved for {owner_repo}"),
            )
        }))
    }

    /// Hop 2: raw.githubusercontent. Returns the raw SKILL.md body as UTF-8.
    async fn fetch_skill_md_content(
        &self,
        owner_repo: &str,
        git_ref: &str,
        path: &str,
    ) -> Result<String, HubError> {
        let url = format!(
            "{}/{}/{}/{}",
            self.raw_content_base.trim_end_matches('/'),
            owner_repo,
            git_ref,
            path,
        );
        let http = self.http.clone();

        let op = || {
            let url = url.clone();
            let http = http.clone();
            async move {
                let resp = http.get(&url).send().await.map_err(HubError::Reqwest)?;
                let status = resp.status();
                if status == 404 {
                    return Err(typed(
                        HubErrorKind::NotFound,
                        format!("SKILL.md not found: {url}"),
                    ));
                }
                if !status.is_success() {
                    return Err(typed(
                        HubErrorKind::Network,
                        format!("GET {url} returned {status}"),
                    ));
                }
                resp.text().await.map_err(HubError::Reqwest)
            }
        };

        with_one_retry(op).await
    }

    /// Build the `/api/search?q=<query>` URL against the configured download base.
    pub(crate) fn build_search_url(&self, query: &str) -> String {
        format!(
            "{}/api/search?q={}",
            self.download_base_url.trim_end_matches('/'),
            urlencoding(query),
        )
    }

    /// Resolve a bare skill name (e.g. `ascii-art`) to a canonical
    /// `owner/repo/slug` identifier via skills.sh `/api/search?q=<name>`.
    ///
    /// Returns the top hit's `id`. Errors with `NotFound` if no hits.
    /// The returned id is validated to be exactly three slash-separated
    /// segments of URL-safe characters — defense-in-depth so a compromised
    /// registry can't inject a traversal-shaped identifier downstream.
    async fn resolve_bare_name(&self, query: &str) -> Result<String, HubError> {
        if query.is_empty() {
            return Err(typed(
                HubErrorKind::InvalidIdentifier,
                "bare skill name must be non-empty",
            ));
        }
        let url = self.build_search_url(query);
        let http = self.http.clone();

        let op = || {
            let url = url.clone();
            let http = http.clone();
            async move {
                let resp = http.get(&url).send().await.map_err(HubError::Reqwest)?;
                let status = resp.status();
                if !status.is_success() {
                    return Err(typed(
                        HubErrorKind::Network,
                        format!("GET {url} returned {status}"),
                    ));
                }
                resp.json::<SkillSearchResponse>()
                    .await
                    .map_err(HubError::Reqwest)
            }
        };

        let body = with_one_retry(op).await?;
        // Prefer a hit whose skillId or name matches the query exactly over the
        // first fuzzy hit. skills.sh ranks by install count, so a popular hit
        // for a *different* skill can outrank the exact name the user typed.
        let id = body
            .skills
            .iter()
            .find(|h| h.skill_id == query || h.name == query)
            .or_else(|| body.skills.first())
            .map(|h| h.id.clone())
            .ok_or_else(|| {
                typed(
                    HubErrorKind::NotFound,
                    format!("no skill matched bare name '{query}' on skills.sh"),
                )
            })?;

        let segments: Vec<&str> = id.split('/').collect();
        let safe = segments.len() == 3
            && segments.iter().all(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            });
        if !safe {
            return Err(typed(
                HubErrorKind::InvalidIdentifier,
                format!("search returned unsafe id '{id}' for '{query}'"),
            ));
        }
        Ok(id)
    }

    /// Hop 3: skills.sh /api/download path-based URL.
    async fn fetch_skill_download(
        &self,
        owner: &str,
        repo: &str,
        slug: &str,
    ) -> Result<SkillDownloadResponse, HubError> {
        let url = self.build_download_url(owner, repo, slug);
        let http = self.http.clone();

        let op = || {
            let url = url.clone();
            let http = http.clone();
            async move {
                let resp = http.get(&url).send().await.map_err(HubError::Reqwest)?;
                let status = resp.status();
                if status == 404 {
                    return Err(typed(
                        HubErrorKind::NotFound,
                        format!("skill not found: {url}"),
                    ));
                }
                if !status.is_success() {
                    return Err(typed(
                        HubErrorKind::Network,
                        format!("GET {url} returned {status}"),
                    ));
                }
                resp.json::<SkillDownloadResponse>()
                    .await
                    .map_err(HubError::Reqwest)
            }
        };

        with_one_retry(op).await
    }
}

/// Minimal URL path-segment encoder for GitHub refs (branches, slash-free strings here).
fn urlencoding(s: &str) -> String {
    // GitHub refs can contain `/` — but the Trees API endpoint expects the ref as a single
    // URL path segment. We percent-encode `/` here. Branches like `main`/`HEAD`/`master` are
    // untouched. Use a tiny hand-rolled encoder to avoid dragging in a new crate.
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

/// Locate the best SKILL.md entry in a GitHub tree for the given skill slug.
///
/// Matching priority (first hit wins):
///   1. `<folder>/SKILL.md` direct
///   2. Shallowest `*/<folder>/SKILL.md` — handles deep nesting like
///      `optional-skills/creative/ascii-video/SKILL.md` or
///      `cli-tool/components/skills/ai-research/excalidraw/SKILL.md`
///   3. Root `SKILL.md` — single-skill repos like `neethanwu/ascii-art`
///
/// Returns `None` if the tree contains no SKILL.md at all.
pub(crate) fn find_skill_md_in_tree<'a>(
    tree: &'a [TreeEntry],
    folder: &str,
) -> Option<&'a TreeEntry> {
    let blobs = || tree.iter().filter(|e| e.entry_type == "blob");

    if !folder.is_empty() {
        let direct = format!("{folder}/SKILL.md");
        if let Some(e) = blobs().find(|e| e.path == direct) {
            return Some(e);
        }

        let needle = format!("/{folder}/SKILL.md");
        let mut nested: Vec<&TreeEntry> = blobs().filter(|e| e.path.ends_with(&needle)).collect();
        if !nested.is_empty() {
            nested.sort_by_key(|e| e.path.matches('/').count());
            return nested.first().copied();
        }
    }

    blobs().find(|e| e.path == "SKILL.md")
}

/// Parse SKILL.md frontmatter; return `(name, description, metadata)` or error if missing.
///
/// Uses serde_yaml on the delimited block. `strict_yaml_delimiter` must be run BEFORE this.
fn parse_frontmatter_fields(
    content: &str,
) -> Result<(String, String, Option<serde_json::Value>), HubError> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(typed(
            HubErrorKind::Parse,
            "frontmatter missing leading '---' delimiter",
        ));
    }
    let after_start = &trimmed[3..];
    // strip leading newline (strict_yaml_delimiter already accepted ---\n or ---\r\n).
    let after_start = after_start
        .strip_prefix("\r\n")
        .unwrap_or_else(|| after_start.strip_prefix('\n').unwrap_or(after_start));
    // Find the closing "---" delimiter at the start of a line.
    let end = after_start.find("\n---").ok_or_else(|| {
        typed(
            HubErrorKind::Parse,
            "frontmatter missing closing '---' delimiter",
        )
    })?;
    let yaml_block = &after_start[..end];

    let value: serde_yaml::Value = serde_yaml::from_str(yaml_block).map_err(HubError::Yaml)?;
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let description = value
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let metadata = value
        .get("metadata")
        .and_then(|v| serde_json::to_value(v).ok());
    Ok((name, description, metadata))
}

// ============================================================================
// HubSource impl (contract from skills_sh.rs:112-189 — keep source_id + trust_level,
// rewrite internals for blob pipeline)
// ============================================================================

#[async_trait]
impl HubSource for SkillsShBlobSource {
    fn source_id(&self) -> &str {
        "skills-sh"
    }

    /// All skills.sh skills are Community-trust (D-15 from Phase 19 CONTEXT.md).
    fn trust_level_for(&self, _identifier: &str) -> SkillSource {
        SkillSource::Community
    }

    #[tracing::instrument(skip(self), fields(query))]
    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SkillMeta>, HubError> {
        // RESEARCH.md O-6: no search endpoint; return empty Vec. Defer real impl.
        Ok(Vec::new())
    }

    #[tracing::instrument(skip(self), fields(identifier))]
    async fn fetch(&self, identifier: &str) -> Result<SkillBundle, HubError> {
        let blob = self.fetch_blob_skill(identifier).await?;

        // Synthesize a SkillBundle. source_id MUST be "skills-sh" (matches deleted skills_sh.rs:184).
        // Each SkillSnapshotFile becomes a BundleFile; sanitize_subpath already run in fetch_blob_skill.
        let files: Vec<BundleFile> = blob
            .files
            .into_iter()
            .map(|f| BundleFile {
                path: f.path,
                bytes: f.contents.into_bytes(),
            })
            .collect();

        Ok(SkillBundle {
            name: sanitize::sanitize_metadata(&blob.name),
            identifier: identifier.to_string(),
            source_id: "skills-sh".to_string(),
            files,
            skill_md: blob.raw_content,
            metadata: blob.metadata.unwrap_or(serde_json::Value::Null),
            snapshot_hash: Some(blob.snapshot_hash.clone()),
        })
    }
}

// ============================================================================
// Retry wrapper (RESEARCH.md §Pattern 1)
// ============================================================================

pub(crate) async fn with_one_retry<F, Fut, T>(op: F) -> Result<T, HubError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, HubError>>,
{
    match op().await {
        Ok(v) => Ok(v),
        Err(e) if is_transient(&e) => {
            tracing::warn!("transient error, retrying once: {e}");
            op().await
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn is_transient(e: &HubError) -> bool {
    match e {
        HubError::Reqwest(re) => {
            re.is_timeout() || re.is_connect() || re.status().is_some_and(|s| s.is_server_error())
        }
        HubError::Typed {
            kind: HubErrorKind::Network,
            ..
        } => true,
        _ => false,
    }
}

// ============================================================================
// Tests (unit-level — integration with wiremock lives in plan 05)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fake_github() -> Arc<GitHubSource> {
        Arc::new(GitHubSource::new(
            crate::auth::GitHubAuth::anonymous(),
            Default::default(),
            Vec::new(),
        ))
    }

    #[test]
    fn source_id_is_skills_sh() {
        let s = SkillsShBlobSource::new(fake_github());
        assert_eq!(s.source_id(), "skills-sh");
    }

    #[test]
    fn trust_level_always_community() {
        let s = SkillsShBlobSource::new(fake_github());
        match s.trust_level_for("anything") {
            SkillSource::Community => {}
            other => panic!("expected Community, got {other:?}"),
        }
    }

    #[test]
    fn build_download_url_is_path_based() {
        let s = SkillsShBlobSource::new(fake_github());
        let url = s.build_download_url("foo", "bar", "ascii-art");
        assert!(
            url.ends_with("/api/download/foo/bar/ascii-art"),
            "got {url}"
        );
        assert!(
            !url.contains('?'),
            "must be path-based (D-06 corrected), not query-string: {url}"
        );
    }

    #[test]
    fn skills_download_url_env_override() {
        let prev = std::env::var("SKILLS_DOWNLOAD_URL").ok();
        unsafe {
            std::env::set_var("SKILLS_DOWNLOAD_URL", "http://127.0.0.1:9999");
        }
        // Construct AFTER setting env so resolve_download_base_url picks it up.
        let s = SkillsShBlobSource::new(fake_github());
        assert!(
            s.build_download_url("o", "r", "s")
                .starts_with("http://127.0.0.1:9999/api/download/")
        );
        unsafe {
            match prev {
                Some(v) => std::env::set_var("SKILLS_DOWNLOAD_URL", v),
                None => std::env::remove_var("SKILLS_DOWNLOAD_URL"),
            }
        }
    }

    #[test]
    fn user_agent_string_is_openclaw_ride() {
        let ua = user_agent_string();
        assert!(ua.starts_with("ironhermes/"), "got {ua}");
        assert!(
            ua.contains("(via openclaw)"),
            "D-22 requires openclaw ride: {ua}"
        );
    }

    #[test]
    fn build_search_url_is_query_string_based() {
        let s = SkillsShBlobSource::new(fake_github());
        let url = s.build_search_url("ascii-art");
        assert!(url.ends_with("/api/search?q=ascii-art"), "got {url}");
    }

    #[test]
    fn build_search_url_encodes_unsafe_chars() {
        let s = SkillsShBlobSource::new(fake_github());
        let url = s.build_search_url("foo bar/baz");
        // space -> %20, slash -> %2F
        assert!(url.ends_with("/api/search?q=foo%20bar%2Fbaz"), "got {url}");
    }

    #[test]
    fn skill_search_response_deserializes() {
        let j = r#"{"query":"ascii-art","skills":[{"id":"neethanwu/ascii-art/ascii-art","skillId":"ascii-art","name":"ascii-art","installs":31,"source":"neethanwu/ascii-art"}],"count":1}"#;
        let r: SkillSearchResponse = serde_json::from_str(j).unwrap();
        assert_eq!(r.skills.len(), 1);
        assert_eq!(r.skills[0].id, "neethanwu/ascii-art/ascii-art");
        assert_eq!(r.skills[0].skill_id, "ascii-art");
        assert_eq!(r.skills[0].name, "ascii-art");
    }

    #[test]
    fn skill_search_response_defaults_missing_name_fields() {
        // Degraded response: only `id` present — skill_id/name default to empty.
        let j = r#"{"query":"x","skills":[{"id":"a/b/c"}],"count":1}"#;
        let r: SkillSearchResponse = serde_json::from_str(j).unwrap();
        assert_eq!(r.skills[0].id, "a/b/c");
        assert_eq!(r.skills[0].skill_id, "");
        assert_eq!(r.skills[0].name, "");
    }

    #[test]
    fn skill_search_response_deserializes_empty() {
        let j = r#"{"query":"nothing","skills":[],"count":0}"#;
        let r: SkillSearchResponse = serde_json::from_str(j).unwrap();
        assert!(r.skills.is_empty());
    }

    #[test]
    fn skill_search_response_tolerates_missing_skills_field() {
        // Defensive: if skills.sh ever omits the field (degraded response),
        // `#[serde(default)]` gives us an empty Vec rather than a parse error.
        let j = r#"{"query":"x","count":0}"#;
        let r: SkillSearchResponse = serde_json::from_str(j).unwrap();
        assert!(r.skills.is_empty());
    }

    #[test]
    fn skill_download_response_deserializes() {
        let j = r#"{"files":[{"path":"SKILL.md","contents":"---\nname:x\n---\n"}],"hash":"abc"}"#;
        let r: SkillDownloadResponse = serde_json::from_str(j).unwrap();
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.files[0].path, "SKILL.md");
        assert_eq!(r.hash, "abc");
    }

    #[test]
    fn is_transient_classification() {
        assert!(is_transient(&HubError::Typed {
            kind: HubErrorKind::Network,
            message: "".into(),
            suggestion: None,
            retry_after_s: None
        }));
        assert!(!is_transient(&HubError::Typed {
            kind: HubErrorKind::NotFound,
            message: "".into(),
            suggestion: None,
            retry_after_s: None
        }));
        assert!(!is_transient(&HubError::Typed {
            kind: HubErrorKind::PathTraversal,
            message: "".into(),
            suggestion: None,
            retry_after_s: None
        }));
        assert!(!is_transient(&HubError::Typed {
            kind: HubErrorKind::ScanHit,
            message: "".into(),
            suggestion: None,
            retry_after_s: None
        }));
        assert!(!is_transient(&HubError::Typed {
            kind: HubErrorKind::Parse,
            message: "".into(),
            suggestion: None,
            retry_after_s: None
        }));
    }

    #[tokio::test]
    async fn with_one_retry_retries_once_on_transient() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = calls.clone();
        let result = with_one_retry(|| {
            let c = calls_ref.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err::<i32, _>(HubError::Typed {
                        kind: HubErrorKind::Network,
                        message: "transient".into(),
                        suggestion: None,
                        retry_after_s: None,
                    })
                } else {
                    Ok(42)
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "exactly 2 calls: one fail, one retry"
        );
    }

    #[tokio::test]
    async fn with_one_retry_does_not_retry_not_found() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = calls.clone();
        let result = with_one_retry(|| {
            let c = calls_ref.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(HubError::Typed {
                    kind: HubErrorKind::NotFound,
                    message: "gone".into(),
                    suggestion: None,
                    retry_after_s: None,
                })
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry on NotFound");
    }

    #[tokio::test]
    async fn with_one_retry_does_not_retry_path_traversal() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = calls.clone();
        let _ = with_one_retry(|| {
            let c = calls_ref.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(HubError::Typed {
                    kind: HubErrorKind::PathTraversal,
                    message: "".into(),
                    suggestion: None,
                    retry_after_s: None,
                })
            }
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry on PathTraversal");
    }

    #[tokio::test]
    async fn search_returns_empty_vec() {
        let s = SkillsShBlobSource::new(fake_github());
        let result = s.search("anything", 10).await.unwrap();
        assert!(
            result.is_empty(),
            "search intentionally stubbed per O-6; defer real impl"
        );
    }

    #[test]
    fn parse_frontmatter_fields_extracts_name_desc() {
        let md = "---\nname: my-skill\ndescription: does things\nmetadata:\n  hermes:\n    category: x\n---\nbody\n";
        let (name, desc, meta) = parse_frontmatter_fields(md).unwrap();
        assert_eq!(name, "my-skill");
        assert_eq!(desc, "does things");
        assert!(meta.is_some());
    }

    #[test]
    fn parse_frontmatter_fields_missing_closing_delim_errors() {
        let md = "---\nname: x\n";
        assert!(parse_frontmatter_fields(md).is_err());
    }

    fn blob(path: &str) -> TreeEntry {
        TreeEntry {
            path: path.to_string(),
            entry_type: "blob".to_string(),
            sha: "0".to_string(),
            size: Some(0),
        }
    }

    #[test]
    fn find_skill_md_direct_match() {
        let tree = vec![
            blob("README.md"),
            blob("ascii-art/SKILL.md"),
            blob("ascii-art/helper.py"),
        ];
        let got = find_skill_md_in_tree(&tree, "ascii-art").unwrap();
        assert_eq!(got.path, "ascii-art/SKILL.md");
    }

    #[test]
    fn find_skill_md_deeply_nested() {
        // Matches the real nousresearch/hermes-agent layout.
        let tree = vec![
            blob("README.md"),
            blob("optional-skills/creative/ascii-video/SKILL.md"),
            blob("optional-skills/creative/ascii-video/helper.py"),
        ];
        let got = find_skill_md_in_tree(&tree, "ascii-video").unwrap();
        assert_eq!(got.path, "optional-skills/creative/ascii-video/SKILL.md");
    }

    #[test]
    fn find_skill_md_root_fallback_for_single_skill_repo() {
        // Matches the real neethanwu/ascii-art layout (SKILL.md at root, slug
        // matches repo name — no `<slug>/` subdir).
        let tree = vec![blob("SKILL.md"), blob("scripts/convert.py")];
        let got = find_skill_md_in_tree(&tree, "ascii-art").unwrap();
        assert_eq!(got.path, "SKILL.md");
    }

    #[test]
    fn find_skill_md_prefers_shallowest_nested_match() {
        let tree = vec![
            blob("SKILL.md"),
            blob("a/b/c/d/ascii-video/SKILL.md"),
            blob("x/ascii-video/SKILL.md"),
        ];
        let got = find_skill_md_in_tree(&tree, "ascii-video").unwrap();
        assert_eq!(
            got.path, "x/ascii-video/SKILL.md",
            "shallowest nested match wins over deeper"
        );
    }

    #[test]
    fn find_skill_md_returns_none_when_absent() {
        let tree = vec![blob("README.md"), blob("src/main.rs")];
        assert!(find_skill_md_in_tree(&tree, "ascii-art").is_none());
    }

    #[test]
    fn find_skill_md_direct_beats_nested() {
        let tree = vec![blob("ascii-art/SKILL.md"), blob("other/ascii-art/SKILL.md")];
        let got = find_skill_md_in_tree(&tree, "ascii-art").unwrap();
        assert_eq!(got.path, "ascii-art/SKILL.md", "direct match must win");
    }

    #[test]
    fn find_skill_md_ignores_tree_entries() {
        // Directory entries (type = "tree") with the right path must not match —
        // only blobs count. The real GitHub trees API returns both.
        let tree = vec![
            TreeEntry {
                path: "ascii-art/SKILL.md".to_string(),
                entry_type: "tree".to_string(),
                sha: "0".to_string(),
                size: None,
            },
            blob("SKILL.md"),
        ];
        let got = find_skill_md_in_tree(&tree, "ascii-art").unwrap();
        assert_eq!(got.path, "SKILL.md", "tree-type entries are skipped");
    }

    #[test]
    fn urlencoding_preserves_unreserved_chars() {
        assert_eq!(urlencoding("main"), "main");
        assert_eq!(urlencoding("v1.2.3"), "v1.2.3");
        assert_eq!(urlencoding("feature/foo"), "feature%2Ffoo");
    }
}
