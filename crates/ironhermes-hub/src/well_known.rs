//! Well-known skills source adapter.
//!
//! Fetches skill indices from `/.well-known/skills/index.json` over HTTPS only.
//!
//! Security mitigations:
//! - T-19.1-02-03: HTTPS-only reqwest client; SSRF guard rejects loopback/private IPs
//! - T-19.1-02-05: `trust_level_for` is hardcoded to `SkillSource::Community` (D-07)

use async_trait::async_trait;
use ironhermes_core::SkillSource;
use url::Url;

use crate::{
    tarball::extract_tarball_prefix, HubError, HubErrorKind, HubSource, SkillBundle, SkillMeta,
};

// ── Index JSON shape ─────────────────────────────────────────────────────────

/// An entry in the `/.well-known/skills/index.json` array.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct IndexEntry {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    identifier: Option<String>,
    /// Direct download URL for the skill tarball.
    tarball_url: Option<String>,
}

// ── Adapter ──────────────────────────────────────────────────────────────────

/// Adapter that fetches skills from `/.well-known/skills/index.json` endpoints.
///
/// Always returns `SkillSource::Community` for all identifiers (D-07).
pub struct WellKnownSkillSource {
    /// HTTPS-only reqwest client (T-19.1-02-03).
    http: reqwest::Client,
    /// Allowed origin hostnames (lowercase). Empty = allow any HTTPS origin.
    allowed_origins: Vec<String>,
    /// When true, skip HTTPS-only and SSRF checks (integration tests with wiremock).
    #[doc(hidden)]
    pub test_mode: bool,
}

impl WellKnownSkillSource {
    /// Create a new `WellKnownSkillSource`.
    ///
    /// `allowed_origins` should come from `config.hub.well_known_origins`.
    /// Pass an empty vec to allow any HTTPS origin.
    pub fn new(allowed_origins: Vec<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("ironhermes-hub/", env!("CARGO_PKG_VERSION")))
                .https_only(true)
                .build()
                .expect("reqwest client"),
            allowed_origins: allowed_origins.into_iter().map(|s| s.to_lowercase()).collect(),
            test_mode: false,
        }
    }

    /// Create a non-HTTPS-only instance for integration tests using wiremock (HTTP).
    ///
    /// Skips HTTPS-only enforcement and SSRF guard so wiremock (HTTP + loopback) works.
    /// Not part of the public API.
    #[doc(hidden)]
    pub fn new_http_for_tests(allowed_origins: Vec<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("ironhermes-hub/", env!("CARGO_PKG_VERSION")))
                // No https_only — wiremock listens on HTTP
                .build()
                .expect("reqwest client"),
            allowed_origins: allowed_origins.into_iter().map(|s| s.to_lowercase()).collect(),
            test_mode: true,
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Parse and validate a well-known identifier.
    ///
    /// Accepts:
    /// - `well-known:<host>[/<path>]`
    /// - `https://<host>[/<path>]`
    ///
    /// Rejects HTTP URLs, loopback/private-IP hosts (SSRF guard), and hosts
    /// not in the allowlist (if configured).
    fn parse_identifier(&self, ident: &str) -> Result<Url, HubError> {
        let trimmed = ident.strip_prefix("well-known:").unwrap_or(ident);

        let url_str = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed.to_string()
        } else if self.test_mode {
            // In test mode, wiremock listens on HTTP
            format!("http://{trimmed}")
        } else {
            format!("https://{trimmed}")
        };

        let url = Url::parse(&url_str).map_err(|e| typed(HubErrorKind::InvalidIdentifier, format!("invalid URL: {e}")))?;

        // In test mode (wiremock uses HTTP + loopback), skip HTTPS and SSRF checks.
        if !self.test_mode {
            if url.scheme() != "https" {
                return Err(typed(
                    HubErrorKind::InvalidIdentifier,
                    "HTTPS required for well-known origins; HTTP is not allowed",
                ));
            }

            if let Some(host) = url.host_str() {
                // SSRF guard: reject loopback + private ranges (T-19.1-02-03)
                if is_private_host(host) {
                    return Err(typed(
                        HubErrorKind::InvalidIdentifier,
                        format!("private/loopback host rejected (SSRF guard): {host}"),
                    ));
                }
            }
        }

        if let Some(host) = url.host_str() {
            // Allowlist check (applied in all modes).
            // Compare against both "host" and "host:port" so tests using
            // wiremock (which passes "127.0.0.1:PORT" as origin) work correctly.
            if !self.allowed_origins.is_empty() {
                let host_lower = host.to_lowercase();
                let authority = match url.port() {
                    Some(p) => format!("{host_lower}:{p}"),
                    None => host_lower.clone(),
                };
                if !self.allowed_origins.contains(&host_lower)
                    && !self.allowed_origins.contains(&authority)
                {
                    return Err(typed(
                        HubErrorKind::NotFound,
                        format!("host not in well_known_origins allowlist: {host}"),
                    ));
                }
            }
        }

        Ok(url)
    }

    /// Fetch and parse the index.json from the base URL.
    ///
    /// `base_url` should be like `https://example.com` (no trailing slash).
    async fn fetch_index(&self, base_url: &str) -> Result<Vec<IndexEntry>, HubError> {
        let index_url = format!("{}/.well-known/skills/index.json", base_url.trim_end_matches('/'));
        let resp = self.http.get(&index_url).send().await?;
        let status = resp.status();

        if status == 404 {
            return Err(typed(HubErrorKind::NotFound, format!("index.json not found at {index_url}")));
        }
        if !status.is_success() {
            return Err(typed(HubErrorKind::Network, format!("GET {index_url} returned {status}")));
        }

        let entries: Vec<IndexEntry> = resp.json().await.map_err(|e| typed(HubErrorKind::Parse, format!("failed to parse index.json: {e}")))?;
        Ok(entries)
    }

    /// Fetch the tarball at `tarball_url` and extract its contents.
    ///
    /// Well-known tarballs may or may not have a top-level wrapper dir.
    /// We try to detect a prefix and fall back to no prefix.
    async fn fetch_tarball_bundle(
        &self,
        tarball_url: &str,
        skill_name: &str,
    ) -> Result<Vec<crate::BundleFile>, HubError> {
        let resp = self.http.get(tarball_url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(typed(HubErrorKind::Network, format!("tarball download returned {status}")));
        }
        let bytes = resp.bytes().await?.to_vec();

        // Try with skill_name/ prefix first (common for well-known tarballs that include the dir)
        let prefixed = extract_tarball_prefix(&bytes, &format!("{skill_name}/"));
        if let Ok(files) = prefixed {
            if !files.is_empty() {
                return Ok(files);
            }
        }

        // Fallback: no prefix (root-level files)
        extract_tarball_prefix(&bytes, "")
    }

    /// Build the base URL from an identifier URL (strip the skill-name path segment).
    fn base_url_from_identifier_url(url: &Url) -> String {
        // origin = scheme + host + optional port
        let mut base = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));
        if let Some(port) = url.port() {
            base.push_str(&format!(":{port}"));
        }
        base
    }

    /// Extract the skill name from the last path segment of the identifier URL.
    fn skill_name_from_identifier_url(url: &Url) -> Option<String> {
        url.path_segments()?.last().map(|s| s.to_string()).filter(|s| !s.is_empty())
    }
}

/// Returns `true` if `host` is a loopback, RFC-1918, or IPv6 private/link-local address.
fn is_private_host(host: &str) -> bool {
    // Strip brackets from IPv6 addresses in URLs (e.g. "[::1]" -> "::1")
    let host = host.trim_start_matches('[').trim_end_matches(']');

    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        return true;
    }
    // IPv6 link-local (fe80::) and unique-local (fc00::/fd00::)
    let lower = host.to_lowercase();
    if lower.starts_with("fe80:") || lower.starts_with("fc00:") || lower.starts_with("fd00:") {
        return true;
    }
    // RFC-1918 ranges
    if host.starts_with("10.") || host.starts_with("192.168.") {
        return true;
    }
    // 172.16.0.0/12 → 172.16.x.x through 172.31.x.x
    if let Some(rest) = host.strip_prefix("172.") {
        if let Some(octet_str) = rest.split('.').next() {
            if let Ok(octet) = octet_str.parse::<u8>() {
                if (16..=31).contains(&octet) {
                    return true;
                }
            }
        }
    }
    false
}

fn typed(kind: HubErrorKind, msg: impl Into<String>) -> HubError {
    HubError::Typed {
        kind,
        message: msg.into(),
        suggestion: None,
        retry_after_s: None,
    }
}

// ── HubSource impl ───────────────────────────────────────────────────────────

#[async_trait]
impl HubSource for WellKnownSkillSource {
    fn source_id(&self) -> &str {
        "well-known"
    }

    /// D-07: all well-known origins always evaluate as Community.
    fn trust_level_for(&self, _identifier: &str) -> SkillSource {
        SkillSource::Community
    }

    /// Search a well-known origin for skills.
    ///
    /// The identifier/query must contain or be a host (e.g. `well-known:example.com`
    /// or just `example.com`). If `allowed_origins` is non-empty, each is searched.
    #[tracing::instrument(skip(self), fields(query, limit))]
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillMeta>, HubError> {
        // Determine which origins to search.
        let origins: Vec<String> = if !self.allowed_origins.is_empty() {
            self.allowed_origins.clone()
        } else {
            // No explicit origins configured — we can't search without a host.
            return Ok(vec![]);
        };

        let q = query.to_lowercase();
        let mut out: Vec<SkillMeta> = Vec::new();

        for origin in &origins {
            let scheme = if self.test_mode { "http" } else { "https" };
            let base_url = format!("{scheme}://{origin}");
            let entries = match self.fetch_index(&base_url).await {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries {
                // Substring match on name + description
                let haystack = format!(
                    "{} {}",
                    entry.name.to_lowercase(),
                    entry.description.as_deref().unwrap_or("").to_lowercase()
                );
                if !q.is_empty() && !haystack.contains(&q) {
                    continue;
                }

                let identifier = entry
                    .identifier
                    .clone()
                    .unwrap_or_else(|| format!("well-known:{origin}/{}", entry.name));

                out.push(SkillMeta {
                    name: entry.name,
                    identifier,
                    source_id: "well-known".to_string(),
                    description: entry.description,
                    version: entry.version,
                });

                if out.len() >= limit {
                    break;
                }
            }
            if out.len() >= limit {
                break;
            }
        }

        Ok(out)
    }

    /// Fetch a skill bundle from a well-known origin.
    ///
    /// `identifier` format: `"well-known:<host>/<skill-name>"`
    /// (or raw `"https://<host>/..."` if the user passed a URL directly).
    #[tracing::instrument(skip(self), fields(identifier))]
    async fn fetch(&self, identifier: &str) -> Result<SkillBundle, HubError> {
        let url = self.parse_identifier(identifier)?;
        let base_url = Self::base_url_from_identifier_url(&url);
        let skill_name = Self::skill_name_from_identifier_url(&url).ok_or_else(|| {
            typed(HubErrorKind::InvalidIdentifier, format!("cannot extract skill name from: {identifier}"))
        })?;

        // Fetch the index to get the tarball_url.
        let entries = self.fetch_index(&base_url).await?;
        let entry = entries
            .into_iter()
            .find(|e| e.name == skill_name)
            .ok_or_else(|| typed(HubErrorKind::NotFound, format!("skill '{skill_name}' not found in index")))?;

        let tarball_url = entry.tarball_url.ok_or_else(|| {
            typed(HubErrorKind::Parse, format!("index entry for '{skill_name}' has no tarball_url"))
        })?;

        // Validate tarball_url against the same SSRF guards applied to identifiers.
        // A malicious well-known server could set tarball_url to an internal endpoint.
        let tarball_parsed = Url::parse(&tarball_url).map_err(|e| {
            typed(HubErrorKind::InvalidIdentifier, format!("invalid tarball_url: {e}"))
        })?;
        if !self.test_mode {
            if tarball_parsed.scheme() != "https" {
                return Err(typed(HubErrorKind::InvalidIdentifier, "tarball_url must use HTTPS"));
            }
            if let Some(host) = tarball_parsed.host_str() {
                if is_private_host(host) {
                    return Err(typed(
                        HubErrorKind::InvalidIdentifier,
                        format!("tarball_url has private/loopback host (SSRF guard): {host}"),
                    ));
                }
            }
        }

        // Download and extract the tarball
        let files = self.fetch_tarball_bundle(&tarball_url, &skill_name).await?;

        if files.is_empty() {
            return Err(typed(HubErrorKind::NotFound, format!("no files extracted from tarball for '{skill_name}'")));
        }

        let skill_md_file = files.iter().find(|f| f.path == "SKILL.md").ok_or_else(|| {
            typed(HubErrorKind::Parse, format!("SKILL.md not found in bundle for '{skill_name}'"))
        })?;

        let skill_md = String::from_utf8_lossy(&skill_md_file.bytes).into_owned();

        Ok(SkillBundle {
            name: skill_name.clone(),
            identifier: identifier.to_string(),
            source_id: "well-known".to_string(),
            files,
            skill_md,
            metadata: serde_json::json!({
                "base_url": base_url,
                "skill_name": skill_name,
            }),
            snapshot_hash: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_private_host_rejects_loopback() {
        assert!(is_private_host("localhost"));
        assert!(is_private_host("127.0.0.1"));
        assert!(is_private_host("::1"));
    }

    #[test]
    fn is_private_host_rejects_rfc1918() {
        assert!(is_private_host("10.0.0.1"));
        assert!(is_private_host("10.255.255.255"));
        assert!(is_private_host("192.168.1.1"));
        assert!(is_private_host("172.16.0.1"));
        assert!(is_private_host("172.31.255.255"));
    }

    #[test]
    fn is_private_host_rejects_ipv6_private() {
        assert!(is_private_host("fe80::1"));         // link-local
        assert!(is_private_host("FE80::1"));         // case-insensitive
        assert!(is_private_host("fc00::1"));         // unique-local
        assert!(is_private_host("fd00::abcd"));      // unique-local
        assert!(is_private_host("[::1]"));            // bracket-wrapped loopback
        assert!(is_private_host("[fe80::1]"));        // bracket-wrapped link-local
        assert!(is_private_host("[fd00::1]"));        // bracket-wrapped unique-local
    }

    #[test]
    fn is_private_host_allows_public() {
        assert!(!is_private_host("example.com"));
        assert!(!is_private_host("github.com"));
        assert!(!is_private_host("172.32.0.1")); // outside 172.16-31 range
        assert!(!is_private_host("2001:db8::1")); // documentation address, not private
    }

    #[test]
    fn trust_level_always_community() {
        let src = WellKnownSkillSource::new(vec![]);
        assert_eq!(src.trust_level_for("anything"), SkillSource::Community);
        assert_eq!(src.trust_level_for("well-known:example.com/foo"), SkillSource::Community);
    }

    #[test]
    fn parse_identifier_rejects_http() {
        let src = WellKnownSkillSource::new(vec![]);
        let err = src.parse_identifier("http://example.com/foo").unwrap_err();
        match err {
            HubError::Typed { kind: HubErrorKind::InvalidIdentifier, message, .. } => {
                assert!(message.contains("HTTPS required"), "message: {message}");
            }
            other => panic!("expected InvalidIdentifier, got {other:?}"),
        }
    }

    #[test]
    fn parse_identifier_rejects_loopback() {
        let src = WellKnownSkillSource::new(vec![]);
        let err = src.parse_identifier("https://127.0.0.1/foo").unwrap_err();
        match err {
            HubError::Typed { kind: HubErrorKind::InvalidIdentifier, .. } => {}
            other => panic!("expected InvalidIdentifier, got {other:?}"),
        }
    }
}
