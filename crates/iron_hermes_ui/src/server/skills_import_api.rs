//! Phase 49.4 Plan 05 (D-05..D-09): the Skills IMPORT and NEW SKILL server
//! surface.
//!
//! This module is a thin, gated wrapper over the already-shipping (via
//! `ironhermes` CLI) `ironhermes-hub` install pipeline. No fetch,
//! quarantine, trust-gated scan, atomic-rename, hash-verify, or lock-file
//! logic lives here — all of that is implemented and reviewed in
//! `ironhermes-hub::installer::install`. This module only:
//!
//!   1. classifies an operator-supplied import source string
//!      (`classify_import_source`/`normalize_github_identifier` — pure,
//!      ungated so both targets and the client can reuse them);
//!   2. implements the two `HubSource` adapters the hub crate does not ship
//!      — a raw/zip URL fetch (`UrlSkillSource`) and a pasted-content
//!      source (`PastedSkillSource`) — closing D-05's URL/paste gaps;
//!   3. exposes four gated `#[server]` entry points
//!      (`preview_skill_import`/`install_previewed_skill`/`create_skill`/
//!      `fork_bundled_skill`) that drive the hub `install()` pipeline.
//!
//! Every write entry point follows this crate's four-step gated-write
//! protocol (`profile_api.rs::delete_profile` is the template): validate
//! input, load a fresh `Config`, fail closed on
//! `check_skills_write_gate`, then perform the mutation. `preview_skill_import`
//! writes nothing to disk, so it does not run the write gate — but it still
//! performs an outbound fetch for URL/GitHub sources, so the SSRF guard
//! (`ironhermes_tools::web_local::validate_url_async`, called inside
//! `UrlSkillSource::fetch`) is the load-bearing control there.

use dioxus::prelude::*;

// ============================================================================
// Pure classification (ungated — compiles on wasm32 too; ImportSourceKind
// carries no native-only types)
// ============================================================================

/// The five shapes an operator-supplied import source string can take
/// (D-05: URL covers GitHub repo / raw SKILL.md / zip; plus local path and
/// pasted content).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSourceKind {
    GitHubRepo,
    RawUrl,
    ZipUrl,
    LocalPath,
    Pasted,
}

/// Classify an operator-supplied import source string into one of the five
/// [`ImportSourceKind`] shapes.
///
/// A URL with a scheme other than `http`/`https` is a hard error — never
/// silently reinterpreted as a local path (matches D-05's "explicit sources
/// only" posture).
pub fn classify_import_source(input: &str) -> Result<ImportSourceKind, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("import source is empty".to_string());
    }

    // SSH-form GitHub remote: `git@github.com:owner/repo.git`.
    if trimmed.starts_with("git@github.com:") {
        return Ok(ImportSourceKind::GitHubRepo);
    }

    if let Ok(url) = url::Url::parse(trimmed) {
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(format!("unsupported URL scheme: {scheme}"));
        }
        let host = url.host_str().unwrap_or_default();
        if host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com")
        {
            return Ok(ImportSourceKind::GitHubRepo);
        }
        if url.path().to_ascii_lowercase().ends_with(".zip") {
            return Ok(ImportSourceKind::ZipUrl);
        }
        return Ok(ImportSourceKind::RawUrl);
    }

    // Not a URL: a local filesystem path (leading `/` or `~`).
    if trimmed.starts_with('/') || trimmed.starts_with('~') {
        return Ok(ImportSourceKind::LocalPath);
    }

    // Pasted content: a YAML frontmatter delimiter or a markdown heading,
    // with no scheme and no leading path separator (already ruled out
    // above).
    let looks_pasted =
        trimmed.starts_with("---") || trimmed.lines().any(|l| l.trim_start().starts_with('#'));
    if looks_pasted {
        return Ok(ImportSourceKind::Pasted);
    }

    Err(format!("could not classify import source: {trimmed:?}"))
}

/// Normalize a GitHub URL or SSH remote into the `owner/repo[/subpath]`
/// identifier form `ironhermes_hub::GitHubSource::fetch` expects (the
/// subpath, when present, is preserved verbatim as the third+ segments).
///
/// A bare repo URL (no subpath) normalizes to `owner/repo` — the caller is
/// responsible for adapting this to `GitHubSource::fetch`'s exact 3-segment
/// contract (see `to_github_fetch_identifier`, `#[cfg(feature = "server")]`
/// below) since that adaptation needs no test coverage of its own beyond
/// what the fetch-identifier unit tests already assert.
pub fn normalize_github_identifier(url: &str) -> Result<String, String> {
    let trimmed = url.trim();

    if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let mut parts = rest.splitn(2, '/');
        let owner = parts.next().filter(|s| !s.is_empty());
        let repo = parts.next().filter(|s| !s.is_empty());
        return match (owner, repo) {
            (Some(o), Some(r)) => Ok(format!("{o}/{r}")),
            _ => Err(format!("could not parse owner/repo from {trimmed:?}")),
        };
    }

    let parsed =
        url::Url::parse(trimmed).map_err(|e| format!("invalid GitHub URL {trimmed:?}: {e}"))?;
    let host = parsed.host_str().unwrap_or_default();
    if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
        return Err(format!("not a github.com URL: {trimmed:?}"));
    }

    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|s| s.filter(|seg| !seg.is_empty()).collect())
        .unwrap_or_default();
    if segments.len() < 2 {
        return Err(format!("URL is missing owner/repo: {trimmed:?}"));
    }
    let owner = segments[0];
    let repo = segments[1].strip_suffix(".git").unwrap_or(segments[1]);

    // https://github.com/owner/repo/tree/<ref>/<subpath...> or /blob/<ref>/<path>
    // -> subpath preserved (joined back with '/').
    if segments.len() > 4 && (segments[2] == "tree" || segments[2] == "blob") {
        let subpath = segments[4..].join("/");
        if !subpath.is_empty() {
            return Ok(format!("{owner}/{repo}/{subpath}"));
        }
    }

    Ok(format!("{owner}/{repo}"))
}

#[cfg(test)]
mod classify_import_source_tests {
    use super::*;

    #[test]
    fn github_repo_url_classifies_as_github() {
        assert_eq!(
            classify_import_source("https://github.com/owner/repo").unwrap(),
            ImportSourceKind::GitHubRepo
        );
    }

    #[test]
    fn github_repo_subpath_url_classifies_as_github() {
        assert_eq!(
            classify_import_source("https://github.com/owner/repo/tree/main/skills/thing")
                .unwrap(),
            ImportSourceKind::GitHubRepo
        );
    }

    #[test]
    fn github_ssh_remote_classifies_as_github() {
        assert_eq!(
            classify_import_source("git@github.com:owner/repo.git").unwrap(),
            ImportSourceKind::GitHubRepo
        );
    }

    #[test]
    fn raw_skill_md_url_classifies_as_raw_url() {
        assert_eq!(
            classify_import_source("https://example.com/path/SKILL.md").unwrap(),
            ImportSourceKind::RawUrl
        );
    }

    #[test]
    fn zip_url_classifies_as_zip_url() {
        assert_eq!(
            classify_import_source("https://example.com/bundle.zip").unwrap(),
            ImportSourceKind::ZipUrl
        );
    }

    #[test]
    fn absolute_local_path_classifies_as_local_path() {
        assert_eq!(
            classify_import_source("/srv/skills/my-skill").unwrap(),
            ImportSourceKind::LocalPath
        );
    }

    #[test]
    fn tilde_local_path_classifies_as_local_path() {
        assert_eq!(
            classify_import_source("~/skills/my-skill").unwrap(),
            ImportSourceKind::LocalPath
        );
    }

    #[test]
    fn frontmatter_pasted_content_classifies_as_pasted() {
        assert_eq!(
            classify_import_source("---\nname: x\ndescription: y\n---\nbody").unwrap(),
            ImportSourceKind::Pasted
        );
    }

    #[test]
    fn markdown_heading_pasted_content_classifies_as_pasted() {
        assert_eq!(
            classify_import_source("# My Skill\nDoes things.").unwrap(),
            ImportSourceKind::Pasted
        );
    }

    #[test]
    fn unsupported_scheme_is_rejected_not_treated_as_path() {
        let err = classify_import_source("ftp://example.com/file").unwrap_err();
        assert!(err.contains("unsupported URL scheme"), "got: {err}");
    }

    #[test]
    fn empty_input_is_rejected() {
        assert!(classify_import_source("").is_err());
        assert!(classify_import_source("   ").is_err());
    }
}

#[cfg(test)]
mod normalize_github_identifier_tests {
    use super::*;

    #[test]
    fn bare_repo_url_normalizes_to_owner_repo() {
        assert_eq!(
            normalize_github_identifier("https://github.com/owner/repo").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn repo_with_trailing_slash_normalizes_to_owner_repo() {
        assert_eq!(
            normalize_github_identifier("https://github.com/owner/repo/").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn tree_subpath_url_preserves_subpath() {
        assert_eq!(
            normalize_github_identifier("https://github.com/owner/repo/tree/main/skills/thing")
                .unwrap(),
            "owner/repo/skills/thing"
        );
    }

    #[test]
    fn blob_subpath_url_preserves_subpath() {
        assert_eq!(
            normalize_github_identifier(
                "https://github.com/owner/repo/blob/main/skills/thing/SKILL.md"
            )
            .unwrap(),
            "owner/repo/skills/thing/SKILL.md"
        );
    }

    #[test]
    fn ssh_remote_normalizes_to_owner_repo() {
        assert_eq!(
            normalize_github_identifier("git@github.com:owner/repo.git").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn ssh_remote_without_git_suffix_normalizes_to_owner_repo() {
        assert_eq!(
            normalize_github_identifier("git@github.com:owner/repo").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn non_github_host_is_rejected() {
        assert!(normalize_github_identifier("https://gitlab.com/owner/repo").is_err());
    }

    #[test]
    fn missing_repo_segment_is_rejected() {
        assert!(normalize_github_identifier("https://github.com/owner-only").is_err());
    }
}

// ============================================================================
// Write gate (D-07/threat model T-49.4-05-04) — feature="server"-gated: it
// takes `ironhermes_core::config::Config`, a native-only (non-wasm target
// table) type.
// ============================================================================

/// Phase 49.4 Plan 05: fail-closed write gate — mirrors
/// `profile_api::check_profile_write_gate` exactly, naming the skills
/// surface in the error.
#[cfg(feature = "server")]
pub(crate) fn check_skills_write_gate(
    config: &ironhermes_core::config::Config,
) -> Result<(), String> {
    if !config.security.web_config_write_enabled {
        return Err("Config writes are disabled for the skills surface".to_string());
    }
    Ok(())
}

#[cfg(all(test, feature = "server"))]
mod write_gate_tests {
    use super::*;

    #[test]
    fn write_gate_reads_web_config_write_enabled() {
        let mut config = ironhermes_core::config::Config::default();
        config.security.web_config_write_enabled = false;
        let err = check_skills_write_gate(&config).unwrap_err();
        assert!(err.contains("disabled"), "got: {err}");

        config.security.web_config_write_enabled = true;
        assert!(check_skills_write_gate(&config).is_ok());
    }
}

// ============================================================================
// Two new `HubSource` adapters closing D-05's URL/paste gaps
// (feature="server"-gated: they implement `ironhermes_hub::HubSource`,
// which depends on `reqwest`/`tokio`, neither available to the wasm client).
// ============================================================================

/// Bounded response size for a URL import fetch (T-49.4-05-06 DoS mitigation).
/// 20 MB comfortably covers a raw SKILL.md and most zip skill bundles while
/// bounding memory for a hostile or misconfigured server.
#[cfg(feature = "server")]
const MAX_URL_RESPONSE_BYTES: u64 = 20 * 1024 * 1024;

/// Redirect-chain bound for `UrlSkillSource::fetch` — each hop is re-run
/// through the canonical SSRF guard before it is fetched.
#[cfg(feature = "server")]
const MAX_REDIRECT_HOPS: usize = 5;

/// Resolve a `Location` header value against the URL that produced it.
/// Handles both absolute and relative Location values; rejects anything the
/// `url` crate cannot represent as an absolute URL.
#[cfg(feature = "server")]
fn resolve_redirect_target(base: &str, location: &str) -> Result<String, String> {
    let base_url = url::Url::parse(base).map_err(|e| format!("base url unparseable: {e}"))?;
    let next = base_url
        .join(location)
        .map_err(|e| format!("location unresolvable: {e}"))?;
    match next.scheme() {
        "http" | "https" => Ok(next.into()),
        s => Err(format!("redirect to unsupported scheme '{s}'")),
    }
}

/// WR-01: close the DNS-rebinding TOCTOU between `validate_url_async`
/// (which validates the URL *string* — it resolves a host and checks that
/// resolution, then returns) and the HTTP client, which performs its own,
/// independent DNS resolution moments later. A short-TTL attacker-controlled
/// DNS name can resolve to a public IP during validation and to a private
/// address by the time the client connects.
///
/// This resolves `url`'s host itself, re-validates each candidate IP using
/// `ironhermes_core::is_safe_url`'s exact semantics, and returns an HTTP
/// client whose DNS resolution for that host is pinned (via
/// `ClientBuilder::resolve`) to the one IP just validated — so the
/// connection cannot land anywhere but the address this function checked.
///
/// Self-contained to this module: `is_safe_url` takes a URL string and does
/// its OWN DNS resolution internally, so passing it a URL whose host is an
/// IP literal (rather than the original hostname) re-runs its full
/// private/loopback/CGNAT/metadata-IP checks against a resolution *we*
/// performed — `(ip_literal, port).to_socket_addrs()` returns the literal
/// address without any further DNS lookup, so no rebinding window remains
/// between this check and the pin. No new symbol needs to be exported from
/// `ironhermes-core`/`ironhermes-tools` for this.
///
/// Must be called immediately before EVERY connection attempt this adapter
/// makes (the initial URL and every redirect hop), since each hop can name a
/// different host.
#[cfg(feature = "server")]
async fn resolve_and_pin(url: &str) -> Result<reqwest::Client, ironhermes_hub::HubError> {
    let parsed = url::Url::parse(url)
        .map_err(|e| typed(ironhermes_hub::HubErrorKind::Network, format!("bad URL: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| typed(ironhermes_hub::HubErrorKind::Network, "URL has no host".to_string()))?
        .to_string();
    let port = parsed.port_or_known_default().unwrap_or(80);

    // Mirrors `validate_url_async`'s own `IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK`
    // escape hatch (`ironhermes_tools::web_local`) so this module's loopback
    // test-server fetches keep working — the bypass only ever widens
    // acceptance for loopback IPs, and only under that env var.
    let test_allow_loopback = std::env::var("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK").is_ok();

    let resolved = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| {
            typed(
                ironhermes_hub::HubErrorKind::Network,
                format!("DNS resolution failed for {host}: {e}"),
            )
        })?;

    let mut pinned: Option<std::net::SocketAddr> = None;
    for addr in resolved {
        let safe = (test_allow_loopback && addr.ip().is_loopback())
            || ironhermes_core::is_safe_url(&format!("http://{addr}/"));
        if safe {
            pinned = Some(addr);
            break;
        }
    }
    let pinned = pinned.ok_or_else(|| {
        typed(
            ironhermes_hub::HubErrorKind::Network,
            format!("all addresses resolved for {host} were blocked by the SSRF guard"),
        )
    })?;

    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, pinned)
        .build()
        .map_err(|e| typed(ironhermes_hub::HubErrorKind::Network, format!("build http client: {e}")))
}

/// Per-entry decompressed size cap inside a fetched zip (decompression-bomb
/// guard) — mirrors `ironhermes_hub::tarball::MAX_EXTRACTED_BYTES`'s intent
/// at the per-file granularity zip's central directory gives us up front.
#[cfg(feature = "server")]
const MAX_ZIP_ENTRY_BYTES: u64 = 20 * 1024 * 1024;

/// Cap on the SUM of decompressed bytes across every entry in a fetched zip
/// (CR-02) — the per-entry cap above bounds each individual entry, but not
/// the aggregate, so up to `MAX_ZIP_ENTRIES` highly-compressible entries
/// each near the per-entry cap could otherwise inflate to tens of GB in
/// memory. Twice the per-entry cap: generous enough for a legitimate
/// multi-file skill bundle, small enough to fail closed well before the
/// zip-bomb amplification becomes a real DoS.
#[cfg(feature = "server")]
const MAX_TOTAL_EXTRACTED_BYTES: u64 = 40 * 1024 * 1024;

#[cfg(feature = "server")]
fn typed(kind: ironhermes_hub::HubErrorKind, msg: impl Into<String>) -> ironhermes_hub::HubError {
    ironhermes_hub::HubError::Typed {
        kind,
        message: msg.into(),
        suggestion: None,
        retry_after_s: None,
    }
}

/// Extract the `name:` field from SKILL.md YAML frontmatter. Mirrors
/// `ironhermes_hub::local_dir`'s private helper of the same shape — kept as
/// its own small copy here (the upstream one is private to that module) and
/// run through `ironhermes_hub::sanitize_name` for the same normalization
/// every other adapter applies.
#[cfg(feature = "server")]
fn parse_frontmatter_name(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()? != "---" {
        return None;
    }
    for line in lines {
        if line == "---" {
            return None;
        }
        if let Some(rest) = line.strip_prefix("name:") {
            let name = rest.trim().trim_matches('"').trim_matches('\'');
            if !name.is_empty() {
                return Some(ironhermes_hub::sanitize_name(name));
            }
        }
    }
    None
}

// ── Minimal ZIP reader (no external zip crate — see 49.4-05-PLAN.md threat
// model T-49.4-05-SC: this phase adds no external registry package) ────────
//
// Parses the End-Of-Central-Directory record + Central Directory (fixed-size,
// fully bounds-checked) rather than scanning local file headers sequentially,
// so a zip using the streaming/data-descriptor flag (which zeroes local
// header sizes) still parses correctly. Only "stored" (method 0) and
// "deflated" (method 8, via the already-vendored `flate2` crate's raw
// DeflateDecoder) entries are supported; any other compression method is a
// hard error rather than a silent skip.

#[cfg(feature = "server")]
fn zip_read_u16_le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

#[cfg(feature = "server")]
fn zip_read_u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Locate the End-Of-Central-Directory record. The EOCD signature can be
/// followed by up to 65535 bytes of zip comment, so we search backward
/// within that window rather than assuming it is the last 22 bytes.
#[cfg(feature = "server")]
fn find_zip_eocd(data: &[u8]) -> Result<usize, ironhermes_hub::HubError> {
    const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    if data.len() < 22 {
        return Err(typed(
            ironhermes_hub::HubErrorKind::Parse,
            "zip archive too small to contain an End-Of-Central-Directory record",
        ));
    }
    let search_window = data.len().min(22 + 65535);
    let earliest = data.len() - search_window;
    let mut i = data.len() - 22;
    loop {
        if data[i..i + 4] == EOCD_SIG {
            return Ok(i);
        }
        if i == earliest {
            break;
        }
        i -= 1;
    }
    Err(typed(
        ironhermes_hub::HubErrorKind::Parse,
        "End-Of-Central-Directory record not found — not a valid zip archive",
    ))
}

/// Parse a zip archive's central directory and return each entry's raw
/// (unsanitized) name plus its decompressed bytes. Directory entries (names
/// ending in `/`) are skipped. Callers MUST run every returned name through
/// `ironhermes_hub::sanitize_subpath` before using it as a filesystem path
/// (this fn does not do so itself — see `build_bundle_from_zip`).
#[cfg(feature = "server")]
fn parse_zip_entries(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>, ironhermes_hub::HubError> {
    use ironhermes_hub::HubErrorKind;
    use std::io::Read;

    let eocd = find_zip_eocd(data)?;
    if eocd + 22 > data.len() {
        return Err(typed(HubErrorKind::Parse, "truncated EOCD record"));
    }
    let total_entries = zip_read_u16_le(&data[eocd + 10..eocd + 12]) as usize;
    let cd_size = zip_read_u32_le(&data[eocd + 12..eocd + 16]) as usize;
    let cd_offset = zip_read_u32_le(&data[eocd + 16..eocd + 20]) as usize;

    let cd_end = cd_offset
        .checked_add(cd_size)
        .ok_or_else(|| typed(HubErrorKind::Parse, "central directory size overflow"))?;
    if cd_end > data.len() {
        return Err(typed(
            HubErrorKind::Parse,
            "central directory extends past end of archive",
        ));
    }

    let mut out = Vec::with_capacity(total_entries.min(MAX_ZIP_ENTRIES));
    let mut pos = cd_offset;
    // CR-02: the per-entry cap alone does not stop a zip-bomb — up to
    // MAX_ZIP_ENTRIES entries can each individually decompress to just under
    // MAX_ZIP_ENTRY_BYTES, ballooning to tens of GB in memory in aggregate.
    // Track the running sum of decompressed bytes across all entries and
    // fail closed once it crosses MAX_TOTAL_EXTRACTED_BYTES.
    let mut total_extracted: u64 = 0;
    for _ in 0..total_entries {
        if out.len() >= MAX_ZIP_ENTRIES {
            return Err(typed(HubErrorKind::Parse, "zip archive has too many entries"));
        }
        if pos + 46 > data.len() || data[pos..pos + 4] != [0x50, 0x4b, 0x01, 0x02] {
            return Err(typed(
                HubErrorKind::Parse,
                "malformed central directory entry",
            ));
        }
        let compression_method = zip_read_u16_le(&data[pos + 10..pos + 12]);
        let compressed_size = zip_read_u32_le(&data[pos + 20..pos + 24]) as usize;
        let uncompressed_size = zip_read_u32_le(&data[pos + 24..pos + 28]) as u64;
        let name_len = zip_read_u16_le(&data[pos + 28..pos + 30]) as usize;
        let extra_len = zip_read_u16_le(&data[pos + 30..pos + 32]) as usize;
        let comment_len = zip_read_u16_le(&data[pos + 32..pos + 34]) as usize;
        let local_header_offset = zip_read_u32_le(&data[pos + 42..pos + 46]) as usize;

        let name_start = pos + 46;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or_else(|| typed(HubErrorKind::Parse, "filename length overflow"))?;
        if name_end > data.len() {
            return Err(typed(HubErrorKind::Parse, "filename extends past archive"));
        }
        let name = String::from_utf8_lossy(&data[name_start..name_end]).into_owned();

        if uncompressed_size > MAX_ZIP_ENTRY_BYTES {
            return Err(typed(
                HubErrorKind::Parse,
                format!("zip entry {name:?} exceeds the maximum extracted size"),
            ));
        }

        if !name.ends_with('/') {
            if local_header_offset + 30 > data.len()
                || data[local_header_offset..local_header_offset + 4] != [0x50, 0x4b, 0x03, 0x04]
            {
                return Err(typed(
                    HubErrorKind::Parse,
                    format!("malformed local file header for {name:?}"),
                ));
            }
            let lfh_name_len =
                zip_read_u16_le(&data[local_header_offset + 26..local_header_offset + 28])
                    as usize;
            let lfh_extra_len =
                zip_read_u16_le(&data[local_header_offset + 28..local_header_offset + 30])
                    as usize;
            let data_start = local_header_offset + 30 + lfh_name_len + lfh_extra_len;
            let data_end = data_start
                .checked_add(compressed_size)
                .ok_or_else(|| typed(HubErrorKind::Parse, "entry data length overflow"))?;
            if data_end > data.len() {
                return Err(typed(
                    HubErrorKind::Parse,
                    format!("entry data for {name:?} extends past archive"),
                ));
            }
            let raw = &data[data_start..data_end];

            let content = match compression_method {
                0 => raw.to_vec(),
                8 => {
                    let mut decoder = flate2::read::DeflateDecoder::new(raw);
                    let mut buf = Vec::with_capacity(
                        (uncompressed_size as usize).min(MAX_ZIP_ENTRY_BYTES as usize),
                    );
                    (&mut decoder)
                        .take(MAX_ZIP_ENTRY_BYTES)
                        .read_to_end(&mut buf)
                        .map_err(|e| typed(HubErrorKind::Parse, format!("inflate {name:?}: {e}")))?;
                    buf
                }
                other => {
                    return Err(typed(
                        HubErrorKind::Parse,
                        format!("unsupported zip compression method {other} for {name:?}"),
                    ));
                }
            };

            total_extracted = total_extracted.saturating_add(content.len() as u64);
            if total_extracted > MAX_TOTAL_EXTRACTED_BYTES {
                return Err(typed(
                    HubErrorKind::Parse,
                    "zip archive's total extracted size is too large",
                ));
            }

            out.push((name, content));
        }

        pos = name_end
            .checked_add(extra_len)
            .and_then(|p| p.checked_add(comment_len))
            .ok_or_else(|| typed(HubErrorKind::Parse, "central directory entry length overflow"))?;
    }
    Ok(out)
}

/// Max entry count per zip (mirrors `ironhermes_hub::tarball::MAX_ENTRIES`).
#[cfg(feature = "server")]
const MAX_ZIP_ENTRIES: usize = 1000;

/// Parse `data` as a zip archive and sanitize every entry path through
/// `ironhermes_hub::sanitize_subpath` before returning — rejects the whole
/// bundle (not just the offending entry) if any entry fails, per D-18/T-49.4-05-02.
#[cfg(feature = "server")]
fn build_bundle_files_from_zip(
    data: &[u8],
) -> Result<Vec<ironhermes_hub::BundleFile>, ironhermes_hub::HubError> {
    let raw_entries = parse_zip_entries(data)?;
    let mut files = Vec::with_capacity(raw_entries.len());
    for (raw_name, bytes) in raw_entries {
        let safe_path = ironhermes_hub::sanitize_subpath(&raw_name)?;
        files.push(ironhermes_hub::BundleFile {
            path: safe_path,
            bytes,
        });
    }
    Ok(files)
}

/// URL-fetch adapter closing D-05's "raw SKILL.md URL" and "zip link" gaps.
/// Native-only, fetch-only (search always empty). Every fetch is preceded by
/// the workspace's canonical async SSRF guard — the load-bearing control,
/// since this adapter is the only one in this module that reaches an
/// operator-chosen network address.
#[cfg(feature = "server")]
pub struct UrlSkillSource;

#[cfg(feature = "server")]
#[async_trait::async_trait]
impl ironhermes_hub::HubSource for UrlSkillSource {
    fn source_id(&self) -> &str {
        "url-import"
    }

    /// Neither a URL nor pasted text has verifiable provenance (unlike a
    /// local path, which is user-authored content from the operator's own
    /// filesystem — see `LocalDirSource::trust_level_for`'s doc comment).
    /// `Community` is the hub's lowest trust tier: the only one
    /// `enforce_trust_gate` hard-rejects on a scanner hit rather than
    /// warn-but-load.
    fn trust_level_for(&self, _identifier: &str) -> ironhermes_core::SkillSource {
        ironhermes_core::SkillSource::Community
    }

    async fn search(
        &self,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<ironhermes_hub::SkillMeta>, ironhermes_hub::HubError> {
        Ok(vec![])
    }

    async fn fetch(&self, identifier: &str) -> Result<ironhermes_hub::SkillBundle, ironhermes_hub::HubError> {
        ironhermes_tools::web_local::validate_url_async(identifier)
            .await
            .map_err(|e| {
                typed(
                    ironhermes_hub::HubErrorKind::Network,
                    format!("URL blocked by SSRF guard: {e}"),
                )
            })?;

        // Redirects are followed manually so the SSRF guard above stays
        // load-bearing on EVERY hop: reqwest's default policy would follow a
        // 3xx from an approved external host straight to a link-local or
        // loopback address the guard never saw (redirect-bypass SSRF).
        let mut current_url = identifier.to_string();
        let mut hops = 0usize;
        let resp = loop {
            // WR-01: build a fresh client per hop, pinned to the exact IP
            // `resolve_and_pin` just re-validated for THIS hop's host — the
            // `validate_url_async` call above (and per-hop below) only
            // validates the URL string; without this, reqwest would
            // independently re-resolve DNS moments later and could connect
            // to a different (rebound) address than the one validated.
            let client = resolve_and_pin(&current_url).await?;
            let r = client.get(&current_url).send().await?;
            if r.status().is_redirection() {
                hops += 1;
                if hops > MAX_REDIRECT_HOPS {
                    return Err(typed(
                        ironhermes_hub::HubErrorKind::Network,
                        format!("GET {identifier} exceeded {MAX_REDIRECT_HOPS} redirects"),
                    ));
                }
                let location = r
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        typed(
                            ironhermes_hub::HubErrorKind::Network,
                            format!("GET {current_url} returned a redirect without a Location header"),
                        )
                    })?;
                let next = resolve_redirect_target(&current_url, location).map_err(|e| {
                    typed(ironhermes_hub::HubErrorKind::Network, format!("bad redirect target: {e}"))
                })?;
                ironhermes_tools::web_local::validate_url_async(&next)
                    .await
                    .map_err(|e| {
                        typed(
                            ironhermes_hub::HubErrorKind::Network,
                            format!("redirect target blocked by SSRF guard: {e}"),
                        )
                    })?;
                current_url = next;
                continue;
            }
            break r;
        };
        if !resp.status().is_success() {
            return Err(typed(
                ironhermes_hub::HubErrorKind::Network,
                format!("GET {identifier} returned {}", resp.status()),
            ));
        }
        // Cheap fast-path only: a well-behaved server that reports an
        // over-cap Content-Length is rejected before any bytes are read.
        // Not load-bearing by itself — a server can omit this header
        // (chunked transfer-encoding) or simply lie about it, so the real
        // enforcement is the streaming accumulation below (CR-01).
        if let Some(len) = resp.content_length() {
            if len > MAX_URL_RESPONSE_BYTES {
                return Err(typed(
                    ironhermes_hub::HubErrorKind::Network,
                    format!("response too large: {len} bytes"),
                ));
            }
        }

        let is_zip = identifier.to_ascii_lowercase().ends_with(".zip");
        // CR-01: stream the body and enforce MAX_URL_RESPONSE_BYTES as bytes
        // arrive, rather than buffering the whole response via `resp.bytes()`
        // first — a server with no (or a lying) Content-Length header could
        // otherwise push an unbounded amount of data into memory before the
        // old post-hoc length check ever ran.
        use tokio_stream::StreamExt as _;
        let mut body: Vec<u8> = Vec::new();
        let mut chunks = resp.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|e| {
                typed(ironhermes_hub::HubErrorKind::Network, format!("read body: {e}"))
            })?;
            if body.len() as u64 + chunk.len() as u64 > MAX_URL_RESPONSE_BYTES {
                return Err(typed(
                    ironhermes_hub::HubErrorKind::Network,
                    "response exceeded the maximum bundle size",
                ));
            }
            body.extend_from_slice(&chunk);
        }

        if is_zip {
            let files = build_bundle_files_from_zip(&body)?;
            let skill_md_content = files
                .iter()
                .find(|f| f.path == "SKILL.md" || f.path.ends_with("/SKILL.md"))
                .map(|f| String::from_utf8_lossy(&f.bytes).into_owned())
                .ok_or_else(|| {
                    typed(
                        ironhermes_hub::HubErrorKind::Parse,
                        "no SKILL.md found in zip archive",
                    )
                })?;
            let name = parse_frontmatter_name(&skill_md_content).unwrap_or_else(|| "unnamed-skill".to_string());
            Ok(ironhermes_hub::SkillBundle {
                name,
                identifier: identifier.to_string(),
                source_id: "url-import".to_string(),
                files,
                skill_md: skill_md_content,
                metadata: serde_json::json!({}),
                snapshot_hash: None,
            })
        } else {
            let skill_md_content = String::from_utf8_lossy(&body).into_owned();
            let name = parse_frontmatter_name(&skill_md_content).unwrap_or_else(|| "unnamed-skill".to_string());
            Ok(ironhermes_hub::SkillBundle {
                name,
                identifier: identifier.to_string(),
                source_id: "url-import".to_string(),
                files: vec![ironhermes_hub::BundleFile {
                    path: "SKILL.md".to_string(),
                    bytes: body.to_vec(),
                }],
                skill_md: skill_md_content,
                metadata: serde_json::json!({}),
                snapshot_hash: None,
            })
        }
    }
}

/// Pasted-content adapter closing D-05's "pasted SKILL.md content" gap.
///
/// Promoted to `ironhermes_hub::pasted::PastedSkillSource` in Phase 49.6
/// Plan 03 (D-08, T-49.6-03-05) so both this UI's save path and the CLI's
/// `/blueprint save` path derive an installed artifact's name identically —
/// promoting rather than duplicating means the two callers cannot disagree
/// about how a composed blueprint's name is derived. Every existing
/// reference in this file (`create_or_install_composed`,
/// `install_composed_blueprint`, the import-preview path, and the tests
/// below) keeps compiling and behaving identically against the re-exported
/// name.
#[cfg(feature = "server")]
pub use ironhermes_hub::PastedSkillSource;

#[cfg(all(test, feature = "server"))]
mod url_and_pasted_source_tests {
    use super::*;
    use ironhermes_hub::HubSource;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// RAII env-var guard, copied verbatim from `profile_api.rs`'s own
    /// `ScopedEnv` (this crate's established per-module-copy convention for
    /// this tiny helper).
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context (--test-threads=1); no
            // concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: single-threaded test context; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    /// Serve `body` exactly once on a loopback TCP listener and return the
    /// URL to fetch it. Avoids pulling in a new dev-dependency (e.g.
    /// wiremock) just for this module's two happy-path fetch tests.
    async fn serve_once(body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(&body).await;
                let _ = stream.shutdown().await;
            }
        });
        format!("http://{addr}/SKILL.md")
    }

    /// Like `serve_once`, but deliberately omits the `Content-Length` header
    /// (relying on `Connection: close` + EOF to delimit the body instead) —
    /// exercises the CR-01 streaming cap, which must not depend on the
    /// remote server ever reporting a length at all.
    async fn serve_once_no_content_length(body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let response =
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(&body).await;
                let _ = stream.shutdown().await;
            }
        });
        format!("http://{addr}/SKILL.md")
    }

    /// Minimal stored-only (method 0) zip writer — the symmetric counterpart
    /// to `parse_zip_entries` above, used only by this test module.
    fn build_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut offsets = Vec::new();

        for (name, data) in entries {
            offsets.push(buf.len() as u32);
            buf.extend_from_slice(&0x04034b50u32.to_le_bytes());
            buf.extend_from_slice(&20u16.to_le_bytes());
            buf.extend_from_slice(&0u16.to_le_bytes());
            buf.extend_from_slice(&0u16.to_le_bytes()); // method = store
            buf.extend_from_slice(&0u16.to_le_bytes());
            buf.extend_from_slice(&0u16.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes());
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
            buf.extend_from_slice(&0u16.to_le_bytes());
            buf.extend_from_slice(name.as_bytes());
            buf.extend_from_slice(data);
        }

        let cd_start = buf.len() as u32;
        let mut central = Vec::new();
        for ((name, data), offset) in entries.iter().zip(offsets.iter()) {
            central.extend_from_slice(&0x02014b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // method = store
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_size = central.len() as u32;
        buf.extend_from_slice(&central);

        buf.extend_from_slice(&0x06054b50u32.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        buf.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        buf.extend_from_slice(&cd_size.to_le_bytes());
        buf.extend_from_slice(&cd_start.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());

        buf
    }

    #[test]
    fn zip_parser_extracts_stored_entries() {
        let zip = build_test_zip(&[
            (
                "SKILL.md",
                b"---\nname: z\ndescription: d\n---\nbody" as &[u8],
            ),
            ("helper.sh", b"#!/bin/sh\necho hi" as &[u8]),
        ]);
        let entries = parse_zip_entries(&zip).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "SKILL.md");
        assert_eq!(entries[1].0, "helper.sh");
    }

    #[test]
    fn zip_parser_rejects_traversal_entry() {
        let zip = build_test_zip(&[("../../etc/passwd", b"evil" as &[u8])]);
        let raw = parse_zip_entries(&zip).unwrap();
        let err = build_bundle_files_from_zip(&zip).unwrap_err();
        assert!(!raw.is_empty(), "sanity: reader itself must still parse");
        match err {
            ironhermes_hub::HubError::Typed { kind, .. } => {
                assert_eq!(kind, ironhermes_hub::HubErrorKind::PathTraversal)
            }
            other => panic!("expected PathTraversal, got {other:?}"),
        }
    }

    #[test]
    fn zip_parser_rejects_total_extracted_size_over_aggregate_cap() {
        // Three entries, each individually under MAX_ZIP_ENTRY_BYTES (20MB),
        // but summing to 45MB -- over MAX_TOTAL_EXTRACTED_BYTES (40MB). The
        // per-entry cap alone would let every one of these through; CR-02's
        // running total must reject the archive once the sum crosses the cap.
        let entry_size = 15 * 1024 * 1024;
        let a = vec![0u8; entry_size];
        let b = vec![0u8; entry_size];
        let c = vec![0u8; entry_size];
        let zip = build_test_zip(&[
            ("a.bin", a.as_slice()),
            ("b.bin", b.as_slice()),
            ("c.bin", c.as_slice()),
        ]);

        let err = parse_zip_entries(&zip).unwrap_err();
        match err {
            ironhermes_hub::HubError::Typed { kind, message, .. } => {
                assert_eq!(kind, ironhermes_hub::HubErrorKind::Parse);
                assert!(
                    message.contains("total extracted size"),
                    "expected aggregate-cap rejection, got: {message}"
                );
            }
            other => panic!("expected Typed Parse rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn raw_skill_md_url_fetch_produces_single_file_bundle() {
        let _guard = ScopedEnv::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");
        let body = b"---\nname: my-skill\ndescription: does things\n---\nBody here.\n".to_vec();
        let url = serve_once(body.clone()).await;

        let bundle = UrlSkillSource.fetch(&url).await.unwrap();
        assert_eq!(bundle.files.len(), 1);
        assert_eq!(bundle.files[0].path, "SKILL.md");
        assert_eq!(bundle.skill_md, String::from_utf8_lossy(&body));
        assert_eq!(bundle.name, "my-skill");
    }

    #[tokio::test]
    async fn url_fetch_without_content_length_rejects_oversized_body() {
        // No Content-Length header at all -- the pre-CR-01 code path relied
        // entirely on `resp.content_length()` for its early check, which a
        // server can simply omit. The streaming accumulator must still catch
        // this via the running per-chunk total.
        let _guard = ScopedEnv::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");
        let oversized = vec![0u8; MAX_URL_RESPONSE_BYTES as usize + 1024];
        let url = serve_once_no_content_length(oversized).await;

        let err = UrlSkillSource.fetch(&url).await.unwrap_err();
        match err {
            ironhermes_hub::HubError::Typed { message, .. } => {
                assert!(
                    message.contains("exceeded the maximum bundle size"),
                    "expected the streaming cap to reject the body, got: {message}"
                );
            }
            other => panic!("expected Typed Network rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn zip_url_fetch_produces_bundle_from_archive() {
        let _guard = ScopedEnv::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");
        let zip = build_test_zip(&[
            (
                "SKILL.md",
                b"---\nname: zipped-skill\ndescription: d\n---\nbody" as &[u8],
            ),
            ("helper.sh", b"#!/bin/sh\necho hi" as &[u8]),
        ]);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let zip_clone = zip.clone();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    zip_clone.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(&zip_clone).await;
                let _ = stream.shutdown().await;
            }
        });
        let url = format!("http://{addr}/bundle.zip");

        let bundle = UrlSkillSource.fetch(&url).await.unwrap();
        assert_eq!(bundle.files.len(), 2);
        assert_eq!(bundle.name, "zipped-skill");
    }

    #[tokio::test]
    async fn pasted_content_fetch_produces_single_file_bundle() {
        let text = "---\nname: pasted-one\ndescription: d\n---\nbody".to_string();
        let bundle = PastedSkillSource.fetch(&text).await.unwrap();
        assert_eq!(bundle.files.len(), 1);
        assert_eq!(bundle.files[0].path, "SKILL.md");
        assert_eq!(bundle.files[0].bytes, text.as_bytes());
        assert_eq!(bundle.name, "pasted-one");
    }

    #[tokio::test]
    async fn pasted_content_without_name_returns_error() {
        let text = "no frontmatter here, just prose".to_string();
        let err = PastedSkillSource.fetch(&text).await.unwrap_err();
        match err {
            ironhermes_hub::HubError::Typed { kind, .. } => {
                assert_eq!(kind, ironhermes_hub::HubErrorKind::Parse)
            }
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn url_source_id_and_search_are_stable() {
        let src = UrlSkillSource;
        assert_eq!(src.source_id(), "url-import");
    }

    #[tokio::test]
    async fn url_source_search_returns_empty() {
        let results = UrlSkillSource.search("anything", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn pasted_source_id_is_stable() {
        let src = PastedSkillSource;
        assert_eq!(src.source_id(), "pasted-skill");
    }

    #[tokio::test]
    async fn pasted_source_search_returns_empty() {
        let results = PastedSkillSource.search("anything", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn both_adapters_use_the_same_lowest_trust_tier() {
        assert_eq!(
            UrlSkillSource.trust_level_for("anything"),
            ironhermes_core::SkillSource::Community
        );
        assert_eq!(
            PastedSkillSource.trust_level_for("anything"),
            ironhermes_core::SkillSource::Community
        );
    }

    #[tokio::test]
    async fn loopback_url_rejected_before_any_request() {
        // No listener on this port — if the SSRF guard did NOT fire first,
        // the error would be a connection error, not the guard's own
        // "blocked by SSRF guard" message.
        let err = UrlSkillSource
            .fetch("http://127.0.0.1:1/SKILL.md")
            .await
            .unwrap_err();
        match err {
            ironhermes_hub::HubError::Typed { message, .. } => {
                assert!(
                    message.contains("SSRF guard"),
                    "expected SSRF-guard rejection before any connection attempt, got: {message}"
                );
            }
            other => panic!("expected Typed SSRF rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ipv6_loopback_url_rejected_before_any_request() {
        let err = UrlSkillSource
            .fetch("http://[::1]:1/SKILL.md")
            .await
            .unwrap_err();
        match err {
            ironhermes_hub::HubError::Typed { message, .. } => {
                assert!(message.contains("SSRF guard"), "got: {message}");
            }
            other => panic!("expected Typed SSRF rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ipv4_mapped_private_url_rejected_before_any_request() {
        let err = UrlSkillSource
            .fetch("http://[::ffff:10.0.0.1]:1/SKILL.md")
            .await
            .unwrap_err();
        match err {
            ironhermes_hub::HubError::Typed { message, .. } => {
                assert!(message.contains("SSRF guard"), "got: {message}");
            }
            other => panic!("expected Typed SSRF rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_and_pin_independently_rejects_loopback() {
        // No IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK set here -- proves WR-01's
        // own resolve-then-validate step is independently load-bearing, not
        // merely redundant with the `validate_url_async` call that already
        // ran before it in `fetch`.
        let err = resolve_and_pin("http://127.0.0.1:1/x").await.unwrap_err();
        match err {
            ironhermes_hub::HubError::Typed { message, .. } => {
                assert!(
                    message.contains("SSRF guard"),
                    "expected resolve_and_pin to reject the resolved loopback IP, got: {message}"
                );
            }
            other => panic!("expected Typed SSRF rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_and_pin_test_bypass_allows_loopback() {
        // With the bypass set, resolve_and_pin must still succeed in
        // building a client pinned to the loopback address -- otherwise this
        // module's other loopback-server fetch tests (which rely on this
        // exact bypass) would break.
        let _guard = ScopedEnv::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");
        let client = resolve_and_pin("http://127.0.0.1:1/x").await;
        assert!(
            client.is_ok(),
            "expected the test bypass to allow pinning a loopback address"
        );
    }
}

// ============================================================================
// Four gated `#[server]` entry points (D-06/D-07/D-08/D-09) — every write fn
// follows the crate's four-step gated-write protocol (validate -> fresh
// Config::load() -> check_skills_write_gate -> mutate), mirroring
// `profile_api::archive_profile` verbatim. `preview_skill_import` performs no
// write, so it is exempt from the gate — but it still reaches an operator-
// chosen network address for URL/GitHub sources, so `UrlSkillSource::fetch`'s
// SSRF guard is the load-bearing control there (see its own doc comment).
//
// Each `#[server]` fn is a thin wrapper around a private `_impl` async fn so
// tests call the impl directly (mirrors this crate's own convention —
// `profile_api.rs`'s tests call `create_profile_impl`/`archive_profile_impl`,
// never the `#[server]`-macro'd wrapper).
// ============================================================================

/// Client-visible preview of an import source, returned by
/// `preview_skill_import` before anything is written to disk (D-06).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillImportPreview {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub tags: Vec<String>,
    pub command_block: Option<String>,
    pub source_label: String,
    pub trust_tier: String,
}

/// Copywriting Contract row ("Skill import fetch/parse failure") — used
/// uniformly for both fetch failures (network/SSRF/parse) and SKILL.md
/// parse failures, since the contract frames both as one inline error.
#[cfg(feature = "server")]
const IMPORT_READ_ERROR: &str = "Couldn't read a SKILL.md from this source. Check the URL, path, or pasted content and try again.";

/// Adapt a GitHub identifier normalized by `normalize_github_identifier`
/// (which may be a bare `owner/repo` with no subpath) to the exact 3+
/// segment form `ironhermes_hub::GitHubSource::fetch` requires (it rejects
/// anything with fewer than 2 `/`-separators). A bare repo gets an empty
/// trailing skill_path segment; a URL that already carried a subpath is
/// passed through unchanged.
#[cfg(feature = "server")]
fn to_github_fetch_identifier(normalized: &str) -> String {
    if normalized.matches('/').count() < 2 {
        format!("{normalized}/")
    } else {
        normalized.to_string()
    }
}

/// Expand a leading `~` in an operator-supplied local path. `LocalDirSource`
/// deliberately does no canonicalization itself (the caller's
/// responsibility, per its own doc comment) — this is that caller-side step.
#[cfg(feature = "server")]
fn expand_local_path(input: &str) -> Result<String, String> {
    if let Some(rest) = input.strip_prefix("~/") {
        let home =
            std::env::var("HOME").map_err(|_| "HOME environment variable not set".to_string())?;
        return Ok(format!("{home}/{rest}"));
    }
    if input == "~" {
        return std::env::var("HOME").map_err(|_| "HOME environment variable not set".to_string());
    }
    Ok(input.to_string())
}

/// Classify `source_input` and construct the matching `HubSource` adapter
/// plus the identifier to pass to its `fetch`. Re-run by both
/// `preview_skill_import` and `install_previewed_skill` — no adapter or
/// fetch result is cached between the two calls (D-06: a genuinely separate
/// preview and install, not a preview that quietly reuses a fetched bundle).
#[cfg(feature = "server")]
async fn build_source(
    source_input: &str,
) -> Result<(Box<dyn ironhermes_hub::HubSource + Send + Sync>, String), String> {
    let kind = classify_import_source(source_input)?;
    match kind {
        ImportSourceKind::GitHubRepo => {
            let normalized = normalize_github_identifier(source_input)?;
            let identifier = to_github_fetch_identifier(&normalized);
            let config = ironhermes_core::config::Config::load()
                .map_err(|e| format!("Config load failed: {e}"))?;
            let auth = ironhermes_hub::GitHubAuth::resolve(
                config.skills.hub.github_token_env.as_deref(),
            )
            .await;
            let trusted = config.skills.hub.trusted_repos_set();
            let source: Box<dyn ironhermes_hub::HubSource + Send + Sync> =
                Box::new(ironhermes_hub::GitHubSource::new(auth, trusted, vec![]));
            Ok((source, identifier))
        }
        ImportSourceKind::RawUrl | ImportSourceKind::ZipUrl => {
            let source: Box<dyn ironhermes_hub::HubSource + Send + Sync> =
                Box::new(UrlSkillSource);
            Ok((source, source_input.trim().to_string()))
        }
        ImportSourceKind::LocalPath => {
            let expanded = expand_local_path(source_input.trim())?;
            let source: Box<dyn ironhermes_hub::HubSource + Send + Sync> =
                Box::new(ironhermes_hub::LocalDirSource);
            Ok((source, expanded))
        }
        ImportSourceKind::Pasted => {
            let source: Box<dyn ironhermes_hub::HubSource + Send + Sync> =
                Box::new(PastedSkillSource);
            Ok((source, source_input.to_string()))
        }
    }
}

/// Extract `metadata.hermes.tags` from a parsed `SkillFrontmatter`'s opaque
/// `metadata` blob. Returns an empty vec when absent — tags are optional.
#[cfg(feature = "server")]
fn extract_tags_from_frontmatter(fm: &ironhermes_core::skills::SkillFrontmatter) -> Vec<String> {
    fm.metadata
        .as_ref()
        .and_then(|m| m.get("hermes"))
        .and_then(|h| h.get("tags"))
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Extract the first fenced code block (` ``` `-delimited) from a SKILL.md
/// body — this module's definition of the "runnable command block" a
/// preview surfaces (no prior convention exists elsewhere in the codebase;
/// this is a new, minimal, defensible choice recorded here and in the
/// SUMMARY).
#[cfg(feature = "server")]
fn extract_first_code_block(body: &str) -> Option<String> {
    let start = body.find("```")?;
    let after_fence_line_end = body[start..].find('\n')? + start + 1;
    let rest = &body[after_fence_line_end..];
    let end = rest.find("```")?;
    let block = rest[..end].trim_end_matches('\n');
    if block.is_empty() {
        None
    } else {
        Some(block.to_string())
    }
}

/// Quote a scalar for inclusion in composed YAML frontmatter. Always
/// double-quotes and escapes backslash/quote so a description containing a
/// colon or other YAML-significant character round-trips safely.
#[cfg(feature = "server")]
fn yaml_escape_scalar(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Compose a SKILL.md document from its four constituent fields — the
/// D-08/D-09 write path's input to the SAME installer pipeline every other
/// source uses (never a raw filesystem write; see `create_or_install_composed`).
#[cfg(feature = "server")]
fn compose_skill_md(name: &str, description: &str, tags: &[String], body: &str) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {}\n", yaml_escape_scalar(name)));
    out.push_str(&format!(
        "description: {}\n",
        yaml_escape_scalar(description)
    ));
    if !tags.is_empty() {
        out.push_str("metadata:\n  hermes:\n    tags:\n");
        for t in tags {
            out.push_str(&format!("      - {}\n", yaml_escape_scalar(t)));
        }
    }
    out.push_str("---\n");
    out.push_str(body.trim_end());
    out.push('\n');
    out
}

/// Read-only recursive search for a skill's `SKILL.md` under `skills_root`
/// by its (already kebab-cased) frontmatter name, returning its description
/// and tags. Used by the D-09 fork path to recover the ORIGINAL skill's
/// metadata from `original_name` alone (the `fork_bundled_skill` signature
/// carries no description/tags argument) — deliberately independent of the
/// in-process `SkillRegistry` singleton so this stays unit-testable without
/// standing up an `AgentRuntime`, and never opens anything under the
/// original skill's directory for writing (D-09): this function only calls
/// `std::fs::read_to_string`.
#[cfg(feature = "server")]
fn find_skill_description_and_tags(
    skills_root: &std::path::Path,
    name: &str,
) -> Result<(String, Vec<String>), String> {
    fn walk(dir: &std::path::Path, name: &str) -> Option<(String, Vec<String>)> {
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = entry.file_type().ok()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if let Some(found) = walk(&path, name) {
                    return Some(found);
                }
            } else if file_type.is_file() && entry.file_name() == "SKILL.md" {
                let content = std::fs::read_to_string(&path).ok()?;
                if let Some((fm, _body)) = ironhermes_core::skills::parse_skill_md(&content) {
                    if fm.name == name {
                        let tags = extract_tags_from_frontmatter(&fm);
                        return Some((fm.description, tags));
                    }
                }
            }
        }
        None
    }

    walk(skills_root, name).ok_or_else(|| format!("skill '{name}' not found"))
}

/// Shared write path for `create_skill` and `fork_bundled_skill` (D-08/D-09):
/// compose a SKILL.md, then route it through the SAME installer pipeline
/// every import source uses via `PastedSkillSource` — chosen over driving
/// `LocalDirSource` over a temp directory because it needs no filesystem
/// staging at all (the composed text goes straight into the in-memory
/// bundle) and, being routed through the lowest trust tier, is scanned no
/// more leniently than operator-pasted content (see `PastedSkillSource`'s
/// own doc comment) — a deliberately conservative choice for self-authored
/// content, not just a convenience one.
#[cfg(feature = "server")]
async fn create_or_install_composed(
    name: &str,
    description: &str,
    tags: &[String],
    body: &str,
) -> Result<String, String> {
    let slug = ironhermes_hub::to_skill_slug(name);
    if slug.is_empty() {
        return Err("skill name must contain at least one letter or number".to_string());
    }

    let skill_md = compose_skill_md(name, description, tags, body);
    let skills_root =
        ironhermes_hub::paths::skills_root().map_err(|e| format!("resolve skills root: {e}"))?;

    let outcome = ironhermes_hub::install(
        &PastedSkillSource,
        &skill_md,
        &ironhermes_hub::CoreSkillScanner,
        &skills_root,
        true,
    )
    .await
    .map_err(|e| format!("{e}"))?;

    Ok(outcome.name)
}

/// Blueprint-save write path (Phase 49.6 Plan 01, D-08/D-12): install an
/// already-composed blueprint `SKILL.md` (see
/// `ironhermes_core::skills::compose_blueprint_skill_md`) through the SAME
/// installer pipeline `create_or_install_composed` uses, modeled on it line
/// for line. Takes `skills_root` as a PARAMETER rather than resolving
/// `ironhermes_hub::paths::skills_root()` internally: that resolver reads the
/// process-global `IRONHERMES_HOME` env var, and driving this function from a
/// test would otherwise require mutating that variable — unsafe under Rust
/// 2024 and racy against every other thread. Never widen `compose_skill_md`
/// or `create_or_install_composed` for this — `create_skill`'s existing
/// behavior must stay untouched (RESEARCH.md Pitfall 3).
#[cfg(feature = "server")]
async fn install_composed_blueprint(
    name: &str,
    skill_md: &str,
    skills_root: &std::path::Path,
) -> Result<String, String> {
    let slug = ironhermes_hub::to_skill_slug(name);
    if slug.is_empty() {
        return Err("skill name must contain at least one letter or number".to_string());
    }

    let outcome = ironhermes_hub::install(
        &PastedSkillSource,
        skill_md,
        &ironhermes_hub::CoreSkillScanner,
        skills_root,
        true,
    )
    .await
    .map_err(|e| format!("{e}"))?;

    Ok(outcome.name)
}

/// The D-09 fork-on-save impl: recover the original skill's description/tags
/// read-only, then write the edited body under `new_name` through the same
/// installer path `create_skill` uses. Never opens anything under the
/// original skill's directory for writing (see
/// `find_skill_description_and_tags`'s doc comment).
#[cfg(feature = "server")]
async fn fork_bundled_skill_impl(
    original_name: &str,
    body: &str,
    new_name: &str,
) -> Result<String, String> {
    let slug = ironhermes_hub::to_skill_slug(new_name);
    if slug.is_empty() {
        return Err("new skill name must contain at least one letter or number".to_string());
    }

    let skills_root =
        ironhermes_hub::paths::skills_root().map_err(|e| format!("resolve skills root: {e}"))?;
    let (description, tags) = find_skill_description_and_tags(&skills_root, original_name)?;

    create_or_install_composed(new_name, &description, &tags, body).await
}

/// D-06 preview impl: classify + fetch + parse, writing nothing. Because it
/// performs no write, it does not run `check_skills_write_gate` — but it is
/// still reachable only through this crate's authentication wrapper (every
/// `#[server]` fn is wrapped by `require_auth`, `main.rs`), and it performs
/// an outbound fetch for URL/GitHub sources, so `UrlSkillSource::fetch`'s
/// SSRF guard is the load-bearing control here.
#[cfg(feature = "server")]
async fn preview_skill_import_impl(source_input: &str) -> Result<SkillImportPreview, String> {
    let (source, identifier) = build_source(source_input)
        .await
        .map_err(|_| IMPORT_READ_ERROR.to_string())?;

    let bundle = source
        .fetch(&identifier)
        .await
        .map_err(|_| IMPORT_READ_ERROR.to_string())?;

    // The SKILL.md was FOUND and read — only its content is at fault here, so
    // report the actual parse reason (which names the offending field) instead
    // of the generic "couldn't read a SKILL.md" used for fetch failures above.
    // A valid-looking skill that silently failed to import with no explanation
    // is exactly what made `allowed-tools: a, b` so hard to diagnose.
    let (frontmatter, body) = ironhermes_core::skills::parse_skill_md_verbose(&bundle.skill_md)
        .map_err(|e| format!("This SKILL.md could not be imported — {e}"))?;

    let tags = extract_tags_from_frontmatter(&frontmatter);
    let command_block = extract_first_code_block(&body);
    let trust = source.trust_level_for(&identifier);

    Ok(SkillImportPreview {
        name: frontmatter.name,
        description: frontmatter.description,
        version: frontmatter.version,
        tags,
        command_block,
        source_label: identifier,
        trust_tier: format!("{trust:?}"),
    })
}

/// D-07 install impl: re-classify, re-construct the adapter, and drive the
/// hub installer with the core scanner. Never adds an enable step — the
/// skill lands disabled; per-profile enabling is the separate, pre-existing
/// `toggle_skill` gate.
#[cfg(feature = "server")]
async fn install_previewed_skill_impl(source_input: &str) -> Result<String, String> {
    let (source, identifier) = build_source(source_input)
        .await
        .map_err(|_| IMPORT_READ_ERROR.to_string())?;

    let skills_root =
        ironhermes_hub::paths::skills_root().map_err(|e| format!("resolve skills root: {e}"))?;

    let outcome = ironhermes_hub::install(
        source.as_ref(),
        &identifier,
        &ironhermes_hub::CoreSkillScanner,
        &skills_root,
        false,
    )
    .await
    .map_err(|e| format!("{e}"))?;

    Ok(outcome.name)
}

/// Preview a skill import from a URL, local path, or pasted SKILL.md
/// content — parses and returns the skill's metadata WITHOUT writing
/// anything to disk (D-06).
#[server]
pub async fn preview_skill_import(
    source_input: String,
) -> Result<SkillImportPreview, ServerFnError> {
    #[cfg(feature = "server")]
    {
        preview_skill_import_impl(&source_input)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = source_input;
        Err(ServerFnError::new(
            "preview_skill_import unavailable without `server` feature",
        ))
    }
}

/// Phase 49.4: swap the running skill catalog in after a write.
///
/// The process serves its catalog from `AppRuntimeBundle::skill_registry`, which
/// was a boot-time constant until this call site existed: a skill written to
/// disk stayed absent from `list_skills` (so the Skills screen showed the import
/// as a no-op) AND from the agent's own `skills` tool, until the server was
/// restarted. Reloading here is what makes an install visible and usable in the
/// same process.
///
/// A reload failure must never fail the write — the skill is already on disk and
/// the lock file already records it — so this reports and returns.
#[cfg(feature = "server")]
async fn reload_skill_catalog(config: &ironhermes_core::config::Config) {
    let outcome = crate::server::state::global_app_state()
        .runtime
        .reload_skill_registry(&config.skills)
        .await;
    tracing::info!(
        added = ?outcome.added,
        removed = ?outcome.removed,
        total = outcome.total,
        "skill registry reloaded after skill write"
    );
}

/// Install a previously previewed skill import — re-fetches from the same
/// source and drives the reviewed hub install pipeline. The skill lands
/// installed and disabled (D-07); it is never enabled for any profile here.
#[server]
pub async fn install_previewed_skill(source_input: String) -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_skills_write_gate(&config).map_err(ServerFnError::new)?;

        let name = install_previewed_skill_impl(&source_input)
            .await
            .map_err(ServerFnError::new)?;
        reload_skill_catalog(&config).await;
        Ok(name)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = source_input;
        Err(ServerFnError::new(
            "install_previewed_skill unavailable without `server` feature",
        ))
    }
}

/// Create a new self-authored skill from a name, description, tags and body
/// (D-08). Routes through the same installer pipeline as import so the new
/// skill classifies as installed, not bundled.
#[server]
pub async fn create_skill(
    name: String,
    description: String,
    tags: Vec<String>,
    body: String,
) -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_skills_write_gate(&config).map_err(ServerFnError::new)?;

        let installed = create_or_install_composed(&name, &description, &tags, &body)
            .await
            .map_err(ServerFnError::new)?;
        reload_skill_catalog(&config).await;
        Ok(installed)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (name, description, tags, body);
        Err(ServerFnError::new(
            "create_skill unavailable without `server` feature",
        ))
    }
}

/// Fork-on-save for editing a bundled skill (D-09): writes the edited body
/// under `new_name` and never modifies the bundled original on disk.
#[server]
pub async fn fork_bundled_skill(
    original_name: String,
    body: String,
    new_name: String,
) -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_skills_write_gate(&config).map_err(ServerFnError::new)?;

        let installed = fork_bundled_skill_impl(&original_name, &body, &new_name)
            .await
            .map_err(ServerFnError::new)?;
        reload_skill_catalog(&config).await;
        Ok(installed)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (original_name, body, new_name);
        Err(ServerFnError::new(
            "fork_bundled_skill unavailable without `server` feature",
        ))
    }
}

/// Maps a `CronJob`'s D-12 portable fields onto `BlueprintMetadata`. Reads
/// EXACTLY these seven fields and no others — `script`, `workdir`,
/// `base_url`, `skills`, `context_from`, and `continuity` all exist on the
/// same `CronJob` struct and must never be read here. Because
/// `BlueprintMetadata` has no member able to hold a shell command, a
/// filesystem path, or an inference endpoint, this mapper is the ONLY place
/// an excluded field could leak into a blueprint — and only by someone
/// adding a line here.
#[cfg(feature = "server")]
fn blueprint_metadata_from_job(
    job: &ironhermes_cron::CronJob,
) -> ironhermes_core::skills::BlueprintMetadata {
    // `origin` resolves to the job's LIVE originating chat at delivery time
    // (`ironhermes-cron/src/delivery.rs::expand_routing_token`) — a
    // per-installation concept that means nothing once the job is exported
    // as a shareable artifact, so it is the one deliver value that must not
    // survive the round trip. Any other value — including this crate's own
    // blank-input default, `"local"` (`normalize_deliver`,
    // `schedules_api.rs:831`) — is carried through verbatim (RESEARCH.md
    // Open Question 1, resolved: the sentinel is `"origin"`, not `"local"`).
    let deliver = if job.deliver.eq_ignore_ascii_case("origin") {
        None
    } else {
        Some(job.deliver.clone())
    };
    let prompt = if job.prompt.trim().is_empty() {
        None
    } else {
        Some(job.prompt.clone())
    };
    let enabled_toolsets = job
        .enabled_toolsets
        .clone()
        .filter(|toolsets| !toolsets.is_empty());

    ironhermes_core::skills::BlueprintMetadata {
        schedule: job.schedule_display.clone(),
        deliver,
        prompt,
        no_agent: job.no_agent,
        model: job.model.clone(),
        provider: job.provider.clone(),
        enabled_toolsets,
    }
}

/// Blueprint-save impl (Phase 49.6 Plan 01, D-12/D-15): look up `job_id` in
/// the caller-selected `store`, map it through `blueprint_metadata_from_job`,
/// resolve the artifact name (sanitized `blueprint_name`, falling back to
/// the job's own name when blank), and install the composed `SKILL.md` via
/// Task 1's `install_composed_blueprint`. The name is sanitized HERE (not
/// left to `compose_blueprint_skill_md`'s own internal sanitization) because
/// `install_composed_blueprint`'s `to_skill_slug` guard has no
/// `shared-blueprint` fallback — an unsanitized name that sanitizes to
/// something valid (e.g. all-punctuation input) would otherwise be rejected
/// by that guard before ever reaching the composer.
#[cfg(feature = "server")]
async fn save_job_as_blueprint_in_store(
    store: &ironhermes_cron::JobStore,
    job_id: &str,
    blueprint_name: &str,
    body: &str,
    skills_root: &std::path::Path,
) -> Result<String, String> {
    let job = store
        .get_job(job_id)
        .ok_or_else(|| format!("job not found: {job_id}"))?;

    let bp = blueprint_metadata_from_job(job);
    let raw_name = if blueprint_name.trim().is_empty() {
        job.name.as_str()
    } else {
        blueprint_name
    };
    let name = ironhermes_core::skills::sanitize_blueprint_name(raw_name);
    let skill_md = ironhermes_core::skills::compose_blueprint_skill_md(&name, body, &bp);

    install_composed_blueprint(&name, &skill_md, skills_root).await
}

/// Save a cron job as a blueprint `SKILL.md` (D-08/D-12/D-15) — reachable
/// from both the Jobs tab's per-row action and the job edit panel's entry
/// point (one dialog, two callers, no duplicated write logic).
///
/// `profile` selects which `JobStore` `job_id` is read from (widened
/// `open_job_store`, Task 2). The DESTINATION is always the process's own
/// skills root regardless of that profile — the skills tree is a
/// single-process concept with no profile dimension
/// (`ironhermes_hub::paths::skills_root()` derives solely from this
/// process's own `IRONHERMES_HOME`, RESEARCH.md Pitfall 5). UI copy
/// describing this save action must not imply per-profile skill isolation
/// that does not exist.
///
/// Follows `create_skill`'s exact four-step gated-write shape:
/// `Config::load()` -> `check_skills_write_gate` (FIRST statement, before any
/// store is opened or any path resolved) -> mutation -> `reload_skill_catalog`.
#[server]
pub async fn save_job_as_blueprint(
    job_id: String,
    profile: Option<String>,
    blueprint_name: String,
    body: String,
) -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_skills_write_gate(&config).map_err(ServerFnError::new)?;

        let store = crate::server::schedules_api::open_job_store(profile.as_deref())
            .map_err(ServerFnError::new)?;
        let skills_root = ironhermes_hub::paths::skills_root()
            .map_err(|e| ServerFnError::new(format!("resolve skills root: {e}")))?;

        let installed = save_job_as_blueprint_in_store(
            &store,
            &job_id,
            &blueprint_name,
            &body,
            &skills_root,
        )
        .await
        .map_err(ServerFnError::new)?;
        reload_skill_catalog(&config).await;
        Ok(installed)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (job_id, profile, blueprint_name, body);
        Err(ServerFnError::new(
            "save_job_as_blueprint unavailable without `server` feature",
        ))
    }
}

/// Phase 49.4 Plan 07 (D-09) deviation: none of plan 05's four entry points
/// above is a read — the SKILL.md editor still needs to fetch a skill's
/// CURRENT body before it can render anything. Read-only recursive search
/// under `skills_root` by frontmatter name, returning only the body (the
/// same frontmatter/body split `find_skill_description_and_tags` already
/// performs) — the editor's buffer is body-only, mirroring the SOUL editor's
/// own body-only buffer. Never opens anything for writing.
#[cfg(feature = "server")]
fn find_skill_body(skills_root: &std::path::Path, name: &str) -> Result<String, String> {
    fn walk(dir: &std::path::Path, name: &str) -> Option<String> {
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = entry.file_type().ok()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if let Some(found) = walk(&path, name) {
                    return Some(found);
                }
            } else if file_type.is_file() && entry.file_name() == "SKILL.md" {
                let content = std::fs::read_to_string(&path).ok()?;
                if let Some((fm, body)) = ironhermes_core::skills::parse_skill_md(&content) {
                    if fm.name == name {
                        return Some(body);
                    }
                }
            }
        }
        None
    }

    walk(skills_root, name).ok_or_else(|| format!("skill '{name}' not found"))
}

#[cfg(feature = "server")]
async fn fetch_skill_body_impl(name: &str) -> Result<String, String> {
    let skills_root =
        ironhermes_hub::paths::skills_root().map_err(|e| format!("resolve skills root: {e}"))?;
    find_skill_body(&skills_root, name)
}

/// Fetch a skill's current SKILL.md body for the editor (D-09) — read-only,
/// so it runs no write gate (mirrors `preview_skill_import`'s own no-write
/// exemption).
#[server]
pub async fn fetch_skill_body(name: String) -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        fetch_skill_body_impl(&name).await.map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = name;
        Err(ServerFnError::new(
            "fetch_skill_body unavailable without `server` feature",
        ))
    }
}

/// Phase 49.4: accept a skill uploaded from the operator's OWN machine (a
/// browser file picker) and STAGE it server-side, returning the staged
/// directory path. The caller then runs the normal
/// `preview_skill_import` → `install_previewed_skill` flow on that path, so an
/// upload converges onto the exact same preview/trust/install pipeline as every
/// other source — this fn adds a staging step, never a second install path.
///
/// Accepts either a `.zip` bundle or a single markdown file (`SKILL.md`).
///
/// Safety properties, in order:
/// - Gated behind `check_skills_write_gate` like every other write entry point.
/// - `bytes` is capped at `MAX_URL_RESPONSE_BYTES`, the same ceiling the URL
///   fetch enforces, so an upload cannot be a larger DoS than a fetch.
/// - The client-supplied `filename` is used ONLY to choose zip-vs-markdown; it
///   is NEVER joined into a path. Every staged file lands under a fresh
///   directory whose name this fn generates, so a hostile filename (`../`, an
///   absolute path, a NUL) cannot influence where anything is written.
/// - Zip entries go through `build_bundle_files_from_zip`, which applies the
///   per-entry and aggregate decompression caps AND `sanitize_subpath`'s
///   traversal rejection — the same reader the zip-URL path uses.
/// - Staging lives under the hub's quarantine dir, the location already
///   designated for not-yet-trusted content.
#[server]
pub async fn stage_uploaded_skill(
    filename: String,
    bytes: Vec<u8>,
) -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_skills_write_gate(&config).map_err(ServerFnError::new)?;

        if bytes.is_empty() {
            return Err(ServerFnError::new("uploaded file is empty"));
        }
        if bytes.len() as u64 > MAX_URL_RESPONSE_BYTES {
            return Err(ServerFnError::new(
                "uploaded file exceeds the maximum bundle size",
            ));
        }
        // Only ever read as a type hint — never as a path component.
        let is_zip = filename.to_ascii_lowercase().ends_with(".zip");

        tokio::task::spawn_blocking(move || stage_uploaded_skill_impl(is_zip, &bytes))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (filename, bytes);
        Err(ServerFnError::new(
            "stage_uploaded_skill unavailable without `server` feature",
        ))
    }
}

/// Blocking half of [`stage_uploaded_skill`]. Writes the upload into a fresh
/// directory under the hub quarantine dir and returns that directory's path.
/// The staged layout is exactly what `LocalDirSource` expects (a directory
/// containing `SKILL.md`), so the existing local-path import flow reads it
/// without special-casing an upload.
#[cfg(feature = "server")]
fn stage_uploaded_skill_impl(is_zip: bool, bytes: &[u8]) -> Result<String, String> {
    use std::io::Write as _;

    let quarantine =
        ironhermes_hub::paths::quarantine_dir().map_err(|e| format!("resolve staging dir: {e}"))?;
    // Directory name is generated here, never taken from the client. Process id
    // + nanosecond clock keeps concurrent uploads from colliding without
    // needing a rng dependency.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let staged = quarantine.join(format!("upload-{}-{}", std::process::id(), stamp));
    std::fs::create_dir_all(&staged).map_err(|e| format!("create staging dir: {e}"))?;

    let write_file = |path: &std::path::Path, data: &[u8]| -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create staging subdir: {e}"))?;
        }
        let mut f = std::fs::File::create(path).map_err(|e| format!("write staged file: {e}"))?;
        f.write_all(data)
            .map_err(|e| format!("write staged file: {e}"))?;
        Ok(())
    };

    if is_zip {
        // Reuses the hardened reader: per-entry + aggregate decompression caps
        // and `sanitize_subpath` traversal rejection.
        let files = build_bundle_files_from_zip(bytes)
            .map_err(|e| format!("read uploaded zip: {e}"))?;
        if !files
            .iter()
            .any(|f| f.path == "SKILL.md" || f.path.ends_with("/SKILL.md"))
        {
            let _ = std::fs::remove_dir_all(&staged);
            return Err("no SKILL.md found in the uploaded zip archive".to_string());
        }
        for file in &files {
            // `file.path` already passed `sanitize_subpath`, so it is a
            // relative, traversal-free path.
            write_file(&staged.join(&file.path), &file.bytes)?;
        }
        // A zip that wraps everything in a single top-level folder stages as
        // `<staged>/<folder>/SKILL.md`; point the caller at that folder so
        // `LocalDirSource` finds SKILL.md where it expects it.
        let root = files
            .iter()
            .find(|f| f.path == "SKILL.md")
            .map(|_| staged.clone())
            .or_else(|| {
                files
                    .iter()
                    .find(|f| f.path.ends_with("/SKILL.md"))
                    .and_then(|f| f.path.rsplit_once("/SKILL.md").map(|(dir, _)| dir.to_string()))
                    .map(|dir| staged.join(dir))
            })
            .unwrap_or_else(|| staged.clone());
        root.to_str()
            .map(str::to_string)
            .ok_or_else(|| "staged path is not valid UTF-8".to_string())
    } else {
        // A single uploaded markdown file becomes the bundle's SKILL.md.
        write_file(&staged.join("SKILL.md"), bytes)?;
        staged
            .to_str()
            .map(str::to_string)
            .ok_or_else(|| "staged path is not valid UTF-8".to_string())
    }
}

/// Phase 49.4: the known local skill root directories that currently exist on
/// disk, for the import wizard's Local Path "quick-pick" — lets the operator
/// prefill a real base path instead of typing the long prefix. Read-only;
/// returns absolute paths, most-canonical first, silently skipping any that
/// do not exist. Never errors on a missing dir — an empty list just means the
/// quick-pick offers nothing and the operator types the path as before.
#[server]
pub async fn list_known_skill_dirs() -> Result<Vec<String>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(root) = ironhermes_hub::paths::skills_root() {
            candidates.push(root);
        }
        let home = ironhermes_core::constants::get_hermes_home();
        candidates.push(home.join("optional-skills"));
        candidates.push(home.join(".agents").join("skills"));
        if let Ok(user_home) = std::env::var("HOME") {
            candidates.push(std::path::PathBuf::from(user_home).join("skills"));
        }
        let mut seen = std::collections::BTreeSet::new();
        let dirs: Vec<String> = candidates
            .into_iter()
            .filter(|p| p.is_dir())
            .filter_map(|p| p.to_str().map(str::to_string))
            .filter(|s| seen.insert(s.clone()))
            .collect();
        Ok(dirs)
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "list_known_skill_dirs unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod fetch_skill_body_tests {
    use super::*;

    #[test]
    fn find_skill_body_reads_body_after_frontmatter() {
        let dir = std::env::temp_dir().join(format!(
            "skills-import-test-{}",
            std::process::id()
        ));
        let skill_dir = dir.join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: d\n---\nThe body text.\n",
        )
        .unwrap();

        let result = find_skill_body(&dir, "my-skill");
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(result.unwrap().trim(), "The body text.");
    }

    #[test]
    fn find_skill_body_missing_skill_returns_error() {
        let dir = std::env::temp_dir().join(format!(
            "skills-import-test-missing-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let result = find_skill_body(&dir, "does-not-exist");
        std::fs::remove_dir_all(&dir).ok();

        assert!(result.is_err());
    }
}

#[cfg(all(test, feature = "server"))]
mod entry_point_tests {
    use super::*;

    /// RAII env-var guard, copied verbatim from `profile_api.rs`'s own
    /// `ScopedEnv` (this crate's established per-module-copy convention).
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context (--test-threads=1); no
            // concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: single-threaded test context; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn list_dir_names(dir: &std::path::Path) -> Vec<String> {
        match std::fs::read_dir(dir) {
            Ok(entries) => {
                let mut names: Vec<String> = entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                names
            }
            Err(_) => vec![],
        }
    }

    #[tokio::test]
    async fn preview_skill_import_pasted_returns_parsed_fields() {
        let text = "---\nname: My Cool Skill\ndescription: does cool things\nversion: \"1.2.3\"\nmetadata:\n  hermes:\n    tags:\n      - devops\n      - ci\n---\nSome prose.\n\n```\nhermes run my-cool-skill\n```\n\nMore prose.".to_string();
        let preview = preview_skill_import_impl(&text).await.unwrap();
        assert_eq!(preview.name, "my-cool-skill");
        assert_eq!(preview.description, "does cool things");
        assert_eq!(preview.version.as_deref(), Some("1.2.3"));
        assert_eq!(preview.tags, vec!["devops".to_string(), "ci".to_string()]);
        assert_eq!(
            preview.command_block.as_deref(),
            Some("hermes run my-cool-skill")
        );
    }

    #[tokio::test]
    async fn preview_skill_import_unparseable_input_returns_error() {
        // Valid enough to classify as Pasted (leading `---`) and to extract a
        // name (PastedSkillSource's lenient line-scan), but invalid YAML
        // overall — the STRICT parse_skill_md must reject it.
        let text = "---\nname: [this is not valid yaml\ndescription: d\n---\nbody".to_string();
        let err = preview_skill_import_impl(&text).await.unwrap_err();
        // Phase 49.4: a SKILL.md that was READ but failed to PARSE now reports
        // the specific reason instead of the generic IMPORT_READ_ERROR (which
        // is still used for fetch/read failures, where naming the internal
        // cause would leak probe detail). The old assertion pinned exactly the
        // behaviour that made a valid-looking skill fail with no explanation.
        assert_ne!(
            err, IMPORT_READ_ERROR,
            "a parse failure must not collapse to the generic read error"
        );
        assert!(
            err.contains("could not be imported"),
            "must be phrased for an operator, got: {err}"
        );
        assert!(
            err.contains("name"),
            "must name the offending field so the operator can fix it, got: {err}"
        );
    }

    #[tokio::test]
    async fn preview_skill_import_missing_optional_frontmatter_succeeds() {
        let text = "---\nname: minimal-skill\ndescription: d\n---\nbody".to_string();
        let preview = preview_skill_import_impl(&text).await.unwrap();
        assert_eq!(preview.name, "minimal-skill");
        assert_eq!(preview.version, None);
        assert!(preview.tags.is_empty());
    }

    #[tokio::test]
    async fn preview_skill_import_writes_nothing_to_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = ScopedEnv::set("IRONHERMES_HOME", tmp.path().to_str().unwrap());
        let skills_dir = tmp.path().join("skills");

        let before = list_dir_names(&skills_dir);
        let text = "---\nname: preview-me\ndescription: d\n---\nbody".to_string();
        let preview = preview_skill_import_impl(&text).await.unwrap();
        let after = list_dir_names(&skills_dir);

        assert_eq!(before, after, "preview must not write to the skills root");
        assert_eq!(preview.name, "preview-me");
    }

    #[tokio::test]
    async fn install_previewed_skill_writes_and_classifies_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = ScopedEnv::set("IRONHERMES_HOME", tmp.path().to_str().unwrap());
        let text = "---\nname: installed-one\ndescription: d\n---\nbody".to_string();

        let name = install_previewed_skill_impl(&text).await.unwrap();
        assert_eq!(name, "installed-one");

        let installed_path = tmp
            .path()
            .join("skills")
            .join("general")
            .join("installed-one")
            .join("SKILL.md");
        assert!(installed_path.is_file(), "skill must be written to disk");

        let lock = ironhermes_hub::SkillLock::load_or_default().unwrap();
        let entry = lock.get("installed-one").expect("lock entry must exist");
        assert_eq!(
            entry.source, "pasted-skill",
            "PastedSkillSource routed this install"
        );
    }

    #[test]
    fn create_skill_writes_skill_md_with_body() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = ScopedEnv::set("IRONHERMES_HOME", tmp.path().to_str().unwrap());
        let rt = tokio::runtime::Runtime::new().unwrap();

        let name = rt
            .block_on(create_or_install_composed(
                "create-me",
                "a created skill",
                &["one".to_string()],
                "the body of the skill",
            ))
            .unwrap();
        assert_eq!(name, "create-me");

        let content = std::fs::read_to_string(
            tmp.path()
                .join("skills")
                .join("general")
                .join("create-me")
                .join("SKILL.md"),
        )
        .unwrap();
        assert!(content.contains("the body of the skill"));
        assert!(content.contains("one"));
    }

    #[tokio::test]
    async fn create_skill_classifies_as_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = ScopedEnv::set("IRONHERMES_HOME", tmp.path().to_str().unwrap());

        create_or_install_composed("classify-me", "d", &[], "body")
            .await
            .unwrap();

        let lock = ironhermes_hub::SkillLock::load_or_default().unwrap();
        assert!(lock.get("classify-me").is_some());
    }

    #[tokio::test]
    async fn create_skill_duplicate_name_returns_error_and_leaves_existing_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = ScopedEnv::set("IRONHERMES_HOME", tmp.path().to_str().unwrap());

        create_or_install_composed("dup-skill", "d", &[], "first body")
            .await
            .unwrap();
        let path = tmp
            .path()
            .join("skills")
            .join("general")
            .join("dup-skill")
            .join("SKILL.md");
        let before = std::fs::read(&path).unwrap();

        let err = create_or_install_composed("dup-skill", "d", &[], "second body")
            .await
            .unwrap_err();
        assert!(!err.is_empty());

        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after, "existing skill must be untouched");
    }

    #[tokio::test]
    async fn create_skill_empty_slug_returns_error_before_touching_filesystem() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = ScopedEnv::set("IRONHERMES_HOME", tmp.path().to_str().unwrap());
        let skills_dir = tmp.path().join("skills");

        let err = create_or_install_composed("!!!", "d", &[], "body")
            .await
            .unwrap_err();
        assert!(err.contains("at least one letter or number"), "got: {err}");
        assert!(
            !skills_dir.exists() || list_dir_names(&skills_dir).is_empty(),
            "no filesystem write must occur for an empty-slug name"
        );
    }

    #[tokio::test]
    async fn fork_bundled_skill_writes_new_installed_skill_and_leaves_original_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = ScopedEnv::set("IRONHERMES_HOME", tmp.path().to_str().unwrap());

        // Seed a "bundled" skill directly on disk (mirrors how a real
        // bundled skill ships — no lock entry, never installed via the hub).
        let bundled_dir = tmp.path().join("skills").join("general").join("polymarket");
        std::fs::create_dir_all(&bundled_dir).unwrap();
        std::fs::write(
            bundled_dir.join("SKILL.md"),
            b"---\nname: polymarket\ndescription: trades on polymarket\n---\noriginal body\n",
        )
        .unwrap();
        let before = std::fs::read(bundled_dir.join("SKILL.md")).unwrap();

        let installed_name =
            fork_bundled_skill_impl("polymarket", "edited body", "polymarket-custom")
                .await
                .unwrap();
        assert_eq!(installed_name, "polymarket-custom");

        let after = std::fs::read(bundled_dir.join("SKILL.md")).unwrap();
        assert_eq!(
            before, after,
            "the bundled skill's directory must be byte-identical"
        );

        let lock = ironhermes_hub::SkillLock::load_or_default().unwrap();
        assert!(
            lock.get("polymarket-custom").is_some(),
            "forked skill must classify as installed"
        );

        let forked_content = std::fs::read_to_string(
            tmp.path()
                .join("skills")
                .join("general")
                .join("polymarket-custom")
                .join("SKILL.md"),
        )
        .unwrap();
        assert!(forked_content.contains("edited body"));
        assert!(forked_content.contains("trades on polymarket"));
    }

    #[tokio::test]
    async fn install_previewed_skill_returns_gate_error_when_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = ScopedEnv::set("IRONHERMES_HOME", tmp.path().to_str().unwrap());
        // A fresh IRONHERMES_HOME with no config.yaml resolves
        // `web_config_write_enabled` to its fail-closed default (`false`).
        let text = "---\nname: gated\ndescription: d\n---\nbody".to_string();

        let err = install_previewed_skill(text).await.unwrap_err();
        assert!(
            format!("{err}").contains("disabled"),
            "expected gate refusal, got: {err}"
        );
    }

    #[test]
    fn compose_skill_md_includes_tags_and_body() {
        let md = compose_skill_md(
            "my skill",
            "a description",
            &["a".to_string(), "b".to_string()],
            "the body",
        );
        assert!(md.starts_with("---\n"));
        assert!(md.contains("name: \"my skill\""));
        assert!(md.contains("- \"a\""));
        assert!(md.contains("- \"b\""));
        assert!(md.ends_with("the body\n"));
    }

    #[test]
    fn extract_first_code_block_extracts_first_fenced_block() {
        let body = "prose\n\n```\nhermes run x\n```\n\nmore prose\n\n```\nignored\n```\n";
        assert_eq!(
            extract_first_code_block(body).as_deref(),
            Some("hermes run x")
        );
    }

    #[test]
    fn extract_first_code_block_none_when_absent() {
        assert_eq!(extract_first_code_block("just prose, no fences"), None);
    }

    #[test]
    fn to_github_fetch_identifier_appends_slash_for_bare_repo() {
        assert_eq!(to_github_fetch_identifier("owner/repo"), "owner/repo/");
        assert_eq!(
            to_github_fetch_identifier("owner/repo/subpath"),
            "owner/repo/subpath"
        );
    }
}

/// Phase 49.6 Plan 01 (D-08 tracer): proves the whole "a blueprint is an
/// ordinary skill" contract end to end — compose -> real `ironhermes_hub`
/// install -> disk -> parse back -> equal. `install_composed_blueprint`
/// takes `skills_root` as a parameter, so these tests never touch
/// `IRONHERMES_HOME` (no `ScopedEnv` needed, unlike `entry_point_tests`
/// above).
#[cfg(all(test, feature = "server"))]
mod blueprint_save_roundtrip_tests {
    use super::*;
    use ironhermes_core::skills::{
        BlueprintMetadata, blueprint_description_from_body, compose_blueprint_skill_md,
        extract_hermes_metadata, parse_skill_md, sanitize_blueprint_name,
    };

    fn full_blueprint() -> BlueprintMetadata {
        BlueprintMetadata {
            schedule: "every 30m".to_string(),
            deliver: Some("telegram".to_string()),
            prompt: Some("Summarize the inbox".to_string()),
            no_agent: true,
            model: Some("gpt-5".to_string()),
            provider: Some("openrouter".to_string()),
            enabled_toolsets: Some(vec!["email".to_string(), "calendar".to_string()]),
        }
    }

    /// Composer never sets `metadata.hermes.category`, so the installer's
    /// `parse_skill_identity` falls back to `"general"`
    /// (`ironhermes-hub/src/installer.rs:123`) — the same layout
    /// `entry_point_tests::install_previewed_skill_writes_and_classifies_installed`
    /// asserts above.
    fn installed_skill_md_path(skills_root: &std::path::Path, installed_name: &str) -> std::path::PathBuf {
        skills_root
            .join("general")
            .join(installed_name)
            .join("SKILL.md")
    }

    #[tokio::test]
    async fn blueprint_metadata_round_trips_through_real_install() {
        let skills_root = tempfile::tempdir().expect("tempdir");
        let bp = full_blueprint();
        let skill_md =
            compose_blueprint_skill_md("inbox-digest", "Digest my inbox every 30 minutes.", &bp);

        let installed_name =
            install_composed_blueprint("inbox-digest", &skill_md, skills_root.path())
                .await
                .expect("install composed blueprint");

        let content = std::fs::read_to_string(installed_skill_md_path(
            skills_root.path(),
            &installed_name,
        ))
        .expect("read installed SKILL.md");
        let (frontmatter, _body) =
            parse_skill_md(&content).expect("installed SKILL.md must parse");
        let hermes_metadata = extract_hermes_metadata(&frontmatter.metadata)
            .expect("hermes metadata block must be present");

        assert_eq!(hermes_metadata.blueprint, Some(bp));
    }

    #[tokio::test]
    async fn blueprint_with_only_schedule_omits_optional_keys_and_round_trips() {
        let skills_root = tempfile::tempdir().expect("tempdir");
        let bp = BlueprintMetadata {
            schedule: "0 9 * * *".to_string(),
            ..Default::default()
        };
        let skill_md = compose_blueprint_skill_md("minimal", "", &bp);
        assert!(!skill_md.contains("deliver:"), "got: {skill_md}");
        assert!(!skill_md.contains("prompt:"), "got: {skill_md}");
        assert!(!skill_md.contains("no_agent:"), "got: {skill_md}");
        assert!(!skill_md.contains("model:"), "got: {skill_md}");
        assert!(!skill_md.contains("provider:"), "got: {skill_md}");
        assert!(!skill_md.contains("enabled_toolsets:"), "got: {skill_md}");

        let installed_name = install_composed_blueprint("minimal", &skill_md, skills_root.path())
            .await
            .expect("install composed blueprint");
        let content = std::fs::read_to_string(installed_skill_md_path(
            skills_root.path(),
            &installed_name,
        ))
        .expect("read installed SKILL.md");
        let (frontmatter, _body) =
            parse_skill_md(&content).expect("installed SKILL.md must parse");
        let hermes_metadata = extract_hermes_metadata(&frontmatter.metadata)
            .expect("hermes metadata block must be present");
        assert_eq!(hermes_metadata.blueprint, Some(bp));
    }

    #[test]
    fn blueprint_absent_leaves_requires_toolsets_intact() {
        let content = "---\nname: plain-skill\ndescription: d\nmetadata:\n  hermes:\n    tags:\n      - devops\n    requires_toolsets:\n      - filesystem\n---\nBody";
        let (frontmatter, _body) = parse_skill_md(content).expect("must parse");
        let hermes_metadata =
            extract_hermes_metadata(&frontmatter.metadata).expect("hermes block present");
        assert_eq!(hermes_metadata.blueprint, None);
        assert_eq!(
            hermes_metadata.requires_toolsets,
            vec!["filesystem".to_string()]
        );
    }

    #[test]
    fn malformed_blueprint_block_degrades_in_isolation() {
        // `blueprint:` is a bare scalar here, not a mapping — the exact
        // Pitfall 4 shape.
        let content = "---\nname: broken-blueprint\ndescription: d\nmetadata:\n  hermes:\n    requires_toolsets:\n      - filesystem\n    blueprint: not-a-mapping\n---\nBody";
        let (frontmatter, _body) = parse_skill_md(content).expect("must parse");
        let hermes_metadata =
            extract_hermes_metadata(&frontmatter.metadata).expect("hermes block present");
        assert_eq!(hermes_metadata.blueprint, None);
        assert_eq!(
            hermes_metadata.requires_toolsets,
            vec!["filesystem".to_string()]
        );
    }

    #[test]
    fn sanitize_blueprint_name_lowercases_dashes_and_falls_back() {
        assert_eq!(
            sanitize_blueprint_name("My Cool Blueprint!"),
            "my-cool-blueprint"
        );
        assert_eq!(sanitize_blueprint_name("   "), "shared-blueprint");
        assert_eq!(sanitize_blueprint_name("!!!"), "shared-blueprint");
        assert_eq!(sanitize_blueprint_name(""), "shared-blueprint");
    }

    #[test]
    fn blueprint_description_from_body_truncates_and_falls_back() {
        assert_eq!(
            blueprint_description_from_body(""),
            "Shared automation blueprint."
        );
        assert_eq!(
            blueprint_description_from_body("   \n\n  "),
            "Shared automation blueprint."
        );
        assert_eq!(
            blueprint_description_from_body("\n\nHello world\nmore prose"),
            "Hello world"
        );
        let long_line = "a".repeat(250);
        let truncated = blueprint_description_from_body(&long_line);
        assert_eq!(truncated.chars().count(), 200);
    }
}

/// Phase 49.6 Plan 01 (Task 3): `blueprint_metadata_from_job` field mapping
/// (including the D-12 exclusion set) and `save_job_as_blueprint_in_store`'s
/// end-to-end write.
#[cfg(all(test, feature = "server"))]
mod blueprint_save_export_tests {
    use super::*;
    use ironhermes_cron::{JobStore, JobUpdate};

    /// A job carrying values in every D-12-excluded field (`script`,
    /// `workdir`, `base_url`, `context_from`) plus `skills` from `add_job`
    /// itself, alongside the seven portable fields this task DOES map.
    fn tmp_store_with_kitchen_sink_job() -> (tempfile::TempDir, JobStore, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = JobStore::open(dir.path().join("cron")).expect("open store");
        let parsed = ironhermes_cron::parse_schedule("every 30m").expect("parse schedule");
        let job = store
            .add_job(
                "kitchen-sink",
                "Digest my inbox",
                parsed,
                "every 30m",
                "local",
                vec!["email".to_string()],
                None,
            )
            .expect("add_job");
        let id = job.id.clone();
        store
            .update_job(
                &id,
                JobUpdate {
                    model: Some("gpt-5".to_string()),
                    provider: Some("openrouter".to_string()),
                    base_url: Some("https://internal.example/v1".to_string()),
                    script: Some("rm -rf /".to_string()),
                    workdir: Some("/srv/secret".to_string()),
                    context_from: Some(vec!["ctx-a".to_string()]),
                    enabled_toolsets: Some(vec!["email".to_string(), "calendar".to_string()]),
                    no_agent: Some(true),
                    continuity: Some(true),
                    ..Default::default()
                },
            )
            .expect("update_job");
        (dir, store, id)
    }

    #[test]
    fn blueprint_metadata_from_job_maps_seven_fields_and_composer_excludes_the_rest() {
        let (_dir, store, id) = tmp_store_with_kitchen_sink_job();
        let job = store.get_job(&id).expect("job present");
        let bp = blueprint_metadata_from_job(job);

        assert_eq!(bp.schedule, "every 30m");
        assert_eq!(bp.deliver.as_deref(), Some("local"));
        assert_eq!(bp.prompt.as_deref(), Some("Digest my inbox"));
        assert!(bp.no_agent);
        assert_eq!(bp.model.as_deref(), Some("gpt-5"));
        assert_eq!(bp.provider.as_deref(), Some("openrouter"));
        assert_eq!(
            bp.enabled_toolsets,
            Some(vec!["email".to_string(), "calendar".to_string()])
        );

        let skill_md =
            ironhermes_core::skills::compose_blueprint_skill_md("kitchen-sink", "Digest body", &bp);
        for excluded in [
            "rm -rf /",
            "/srv/secret",
            "https://internal.example/v1",
            "ctx-a",
        ] {
            assert!(
                !skill_md.contains(excluded),
                "composed SKILL.md must not contain excluded value {excluded:?}, got: {skill_md}"
            );
        }
    }

    #[test]
    fn deliver_origin_sentinel_omits_other_values_carry_through() {
        let (_dir, mut store, id) = tmp_store_with_kitchen_sink_job();

        for (deliver, expected) in [
            ("origin", None),
            ("Origin", None), // case-insensitive
            ("telegram", Some("telegram".to_string())),
            ("local", Some("local".to_string())),
        ] {
            store
                .update_job(
                    &id,
                    JobUpdate {
                        deliver: Some(deliver.to_string()),
                        ..Default::default()
                    },
                )
                .unwrap_or_else(|e| panic!("update deliver to {deliver:?}: {e}"));
            let job = store.get_job(&id).expect("job present");
            assert_eq!(
                blueprint_metadata_from_job(job).deliver,
                expected,
                "deliver={deliver:?}"
            );
        }
    }

    #[tokio::test]
    async fn save_job_as_blueprint_in_store_writes_exactly_one_skill_md() {
        let (_dir, store, id) = tmp_store_with_kitchen_sink_job();
        let skills_root = tempfile::tempdir().expect("skills tempdir");

        let installed_name = save_job_as_blueprint_in_store(
            &store,
            &id,
            "",
            "Digest my inbox on a schedule.",
            skills_root.path(),
        )
        .await
        .expect("save as blueprint");

        let general_dir = skills_root.path().join("general");
        let entries: Vec<_> = std::fs::read_dir(&general_dir)
            .expect("read general dir")
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1, "exactly one SKILL.md must be written");

        let content = std::fs::read_to_string(general_dir.join(&installed_name).join("SKILL.md"))
            .expect("read installed SKILL.md");
        let (frontmatter, _body) =
            ironhermes_core::skills::parse_skill_md(&content).expect("installed SKILL.md must parse");
        let hermes_metadata = ironhermes_core::skills::extract_hermes_metadata(&frontmatter.metadata)
            .expect("hermes metadata block must be present");
        assert!(hermes_metadata.blueprint.is_some());
    }

    #[tokio::test]
    async fn save_job_as_blueprint_in_store_unknown_job_id_errors_and_writes_nothing() {
        let (_dir, store, _id) = tmp_store_with_kitchen_sink_job();
        let skills_root = tempfile::tempdir().expect("skills tempdir");

        let result =
            save_job_as_blueprint_in_store(&store, "does-not-exist", "", "body", skills_root.path())
                .await;
        assert!(result.is_err());

        let general_dir = skills_root.path().join("general");
        assert!(
            !general_dir.exists()
                || std::fs::read_dir(&general_dir)
                    .expect("read general dir")
                    .next()
                    .is_none(),
            "no SKILL.md may be written for an unknown job id"
        );
    }
}

#[cfg(all(test, feature = "server"))]
mod resolve_redirect_target_tests {
    use super::resolve_redirect_target;

    #[test]
    fn absolute_location_is_used_verbatim() {
        assert_eq!(
            resolve_redirect_target("https://example.com/a", "https://cdn.example.net/b").unwrap(),
            "https://cdn.example.net/b"
        );
    }

    #[test]
    fn relative_location_resolves_against_base() {
        assert_eq!(
            resolve_redirect_target("https://example.com/skills/a.zip", "/dl/a.zip").unwrap(),
            "https://example.com/dl/a.zip"
        );
    }

    #[test]
    fn non_http_scheme_is_rejected() {
        assert!(resolve_redirect_target("https://example.com/a", "file:///etc/passwd").is_err());
        assert!(resolve_redirect_target("https://example.com/a", "gopher://internal/").is_err());
    }

    #[test]
    fn unparseable_location_is_rejected() {
        assert!(resolve_redirect_target("https://example.com/a", "https://exa mple.com").is_err());
    }
}

/// Every skill WRITE must publish its result to the running process.
///
/// A skill written to disk is invisible to `list_skills` and to the agent's own
/// `skills` tool until `AgentRuntime::reload_skill_registry` swaps the new
/// catalog in — that gap is what made a successful import look like a no-op in
/// the Skills screen. The invariant is derived, not hardcoded: find every helper
/// that drives `ironhermes_hub::install`, then require each `#[server]` fn
/// calling one of them to reload. A new install path added without a reload
/// fails here, which no behavioral test on today's three entry points would.
#[cfg(all(test, feature = "server"))]
mod write_paths_reload_the_catalog {
    const SRC: &str = include_str!("skills_import_api.rs");

    /// A top-level item's body ends at the first `}` in column 0.
    fn item_body(block: &str) -> &str {
        match block.find("\n}\n") {
            Some(end) => &block[..end],
            None => block,
        }
    }

    fn fn_name(body: &str) -> String {
        body.lines()
            .find(|l| l.contains("fn "))
            .and_then(|l| l.split("fn ").nth(1))
            .and_then(|rest| rest.split(['(', '<']).next())
            .unwrap_or("<unknown>")
            .trim()
            .to_string()
    }

    /// Helpers that change what is on disk — the ones whose callers owe the
    /// running process a reload.
    ///
    /// Seeded with whatever calls `ironhermes_hub::install` directly, then closed
    /// transitively: `fork_bundled_skill_impl` installs only by way of
    /// `create_or_install_composed`, and a single direct-call check would miss it.
    fn install_drivers() -> Vec<String> {
        let helpers: Vec<(String, &str)> = SRC
            .split("\nasync fn ")
            .skip(1)
            .map(|block| {
                let body = item_body(block);
                (fn_name(&format!("fn {block}")), body)
            })
            .collect();

        let mut drivers: Vec<String> = helpers
            .iter()
            .filter(|(_, body)| body.contains("ironhermes_hub::install("))
            .map(|(name, _)| name.clone())
            .collect();

        loop {
            let mut grew = false;
            for (name, body) in &helpers {
                if drivers.contains(name) {
                    continue;
                }
                if drivers.iter().any(|d| body.contains(&format!("{d}("))) {
                    drivers.push(name.clone());
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        drivers
    }

    #[test]
    fn install_drivers_are_discoverable() {
        let drivers = install_drivers();
        assert!(
            drivers.len() >= 2,
            "expected to find the helpers that call ironhermes_hub::install, found {drivers:?}"
        );
    }

    #[test]
    fn every_server_fn_that_installs_also_reloads_the_catalog() {
        let drivers = install_drivers();
        let mut checked = 0;
        for block in SRC.split("\n#[server]\n").skip(1) {
            let body = item_body(block);
            let calls_installer = drivers.iter().any(|d| body.contains(&format!("{d}(")));
            if !calls_installer {
                continue;
            }
            assert!(
                body.contains("reload_skill_catalog"),
                "`{}` writes a skill but never reloads the catalog, so its result \
                 stays invisible to list_skills and to the agent until restart",
                fn_name(body)
            );
            checked += 1;
        }
        assert_eq!(
            checked, 4,
            "expected exactly the install/create/fork/save-as-blueprint entry points to install \
             (Phase 49.6 Plan 01 added save_job_as_blueprint as the 4th)"
        );
    }
}
