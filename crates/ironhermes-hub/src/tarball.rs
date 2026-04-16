//! Shared tarball extraction helpers for GitHub and skills.sh adapters.
//!
//! Ports `~/code/hermes-agent/tools/skills_hub.py::_validate_bundle_rel_path` (lines 121-148).
//! Security mitigations: T-19.1-02-01 (path traversal), T-19.1-02-02 (size/entry bomb).

use std::path::{Component, Path};

use crate::{BundleFile, HubError, HubErrorKind};

/// Max total extracted bytes per skill (50 MB).
pub const MAX_EXTRACTED_BYTES: u64 = 50 * 1024 * 1024;

/// Max entry count per tarball (1000 entries).
pub const MAX_ENTRIES: usize = 1000;

fn typed(kind: HubErrorKind, msg: impl Into<String>) -> HubError {
    HubError::Typed {
        kind,
        message: msg.into(),
        suggestion: None,
        retry_after_s: None,
    }
}

/// Ports `_validate_bundle_rel_path` from the Python reference (lines 121-148).
///
/// Rejects:
/// - empty paths
/// - NUL bytes
/// - absolute paths
/// - parent-directory components (`..`)
/// - Windows drive-letter prefixes
/// - root-directory components
///
/// Returns the cleaned, forward-slash-joined relative path on success.
pub fn validate_bundle_rel_path(rel: &str) -> Result<String, HubError> {
    if rel.is_empty() {
        return Err(typed(HubErrorKind::Parse, "empty tar entry path"));
    }
    if rel.contains('\0') {
        return Err(typed(HubErrorKind::Parse, "NUL byte in tar entry path"));
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err(typed(
            HubErrorKind::Parse,
            format!("absolute tar entry path rejected: {rel}"),
        ));
    }
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                return Err(typed(
                    HubErrorKind::Parse,
                    format!(".. component rejected: {rel}"),
                ))
            }
            Component::Prefix(_) => {
                return Err(typed(
                    HubErrorKind::Parse,
                    format!("drive-letter/prefix rejected: {rel}"),
                ))
            }
            Component::RootDir => {
                return Err(typed(
                    HubErrorKind::Parse,
                    format!("root component rejected: {rel}"),
                ))
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    let cleaned = p
        .components()
        .filter_map(|c| {
            if let Component::Normal(s) = c {
                Some(s.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    if cleaned.is_empty() {
        return Err(typed(HubErrorKind::Parse, "path reduced to empty after cleaning"));
    }
    Ok(cleaned)
}

/// Extract a gzipped tarball body into a `Vec<BundleFile>`, keeping only entries
/// under `keep_prefix` (e.g., `"anthropics-skills-abc123/tenor-gif/"`).
///
/// Symlinks and hardlinks are skipped outright (T-19.1-02-01).
/// Total extracted bytes are capped at [`MAX_EXTRACTED_BYTES`] (T-19.1-02-02).
/// Entry count is capped at [`MAX_ENTRIES`] (T-19.1-02-02).
pub fn extract_tarball_prefix(bytes: &[u8], keep_prefix: &str) -> Result<Vec<BundleFile>, HubError> {
    use flate2::read::GzDecoder;
    use std::io::Read as _;
    use tar::{Archive, EntryType};

    let mut total = 0u64;
    let mut count = 0usize;
    let mut out: Vec<BundleFile> = Vec::new();

    let gz = GzDecoder::new(bytes);
    let mut ar = Archive::new(gz);

    for entry in ar.entries()? {
        let mut entry = entry?;
        count += 1;
        if count > MAX_ENTRIES {
            return Err(typed(HubErrorKind::Parse, "tar entry count exceeds MAX_ENTRIES"));
        }

        // Skip anything that isn't a regular file (reject symlinks/hardlinks — T-19.1-02-01).
        match entry.header().entry_type() {
            EntryType::Regular | EntryType::GNUSparse => {}
            EntryType::Directory => continue,
            _ => continue, // skip symlinks, hard links, etc.
        }

        let raw_path = entry.path()?.to_string_lossy().into_owned();

        if !keep_prefix.is_empty() && !raw_path.starts_with(keep_prefix) {
            continue;
        }

        let rel = if keep_prefix.is_empty() {
            raw_path.as_str()
        } else {
            &raw_path[keep_prefix.len()..]
        };

        if rel.is_empty() {
            continue;
        }

        let safe = validate_bundle_rel_path(rel)?;

        let size = entry.header().size().unwrap_or(0);
        total = total.saturating_add(size);
        if total > MAX_EXTRACTED_BYTES {
            return Err(typed(
                HubErrorKind::Parse,
                "tar extracted bytes exceeds MAX_EXTRACTED_BYTES",
            ));
        }

        let mut buf = Vec::with_capacity(size as usize);
        entry.read_to_end(&mut buf)?;
        out.push(BundleFile { path: safe, bytes: buf });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_simple_path() {
        assert_eq!(validate_bundle_rel_path("handler.py").unwrap(), "handler.py");
    }

    #[test]
    fn valid_nested_path() {
        assert_eq!(
            validate_bundle_rel_path("sub/dir/file.txt").unwrap(),
            "sub/dir/file.txt"
        );
    }

    #[test]
    fn rejects_empty() {
        assert!(validate_bundle_rel_path("").is_err());
    }

    #[test]
    fn rejects_absolute() {
        assert!(validate_bundle_rel_path("/etc/passwd").is_err());
    }

    #[test]
    fn rejects_parent_dir() {
        assert!(validate_bundle_rel_path("../../etc/passwd").is_err());
        assert!(validate_bundle_rel_path("foo/../../../etc/passwd").is_err());
    }

    #[test]
    fn rejects_nul_byte() {
        assert!(validate_bundle_rel_path("foo\0bar").is_err());
    }

    #[test]
    fn strips_cur_dir_components() {
        // ./SKILL.md → "SKILL.md"
        let result = validate_bundle_rel_path("./SKILL.md").unwrap();
        assert_eq!(result, "SKILL.md");
    }
}
