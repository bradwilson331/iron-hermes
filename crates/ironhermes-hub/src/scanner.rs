//! Skill scanner abstraction for the install pipeline.
//!
//! The `SkillScanner` trait wraps `scan_skill_content` from `ironhermes-core`
//! so that the install pipeline can be tested with injected scan behaviour
//! (e.g. always-clean, always-blocked) without needing real threat-pattern matching.
//!
//! Production code uses `CoreSkillScanner` which delegates to the real scanner.

use crate::{BundleFile, HubError, HubErrorKind};

// ── Scan verdict ────────────────────────────────────────────────────────────

/// Result of scanning a skill bundle's files.
#[derive(Debug, Clone)]
pub struct ScanVerdict {
    /// Per-file results: `(relative_path, blocked_reason_or_empty)`.
    pub file_results: Vec<(String, String)>,
}

impl ScanVerdict {
    /// True if any file was blocked by the scanner.
    pub fn has_blocks(&self) -> bool {
        self.file_results
            .iter()
            .any(|(_, r)| r.starts_with("[BLOCKED:"))
    }

    /// Human-readable one-line summary for manifest `scan_verdict` field.
    pub fn summary(&self) -> String {
        let blocked: Vec<&str> = self
            .file_results
            .iter()
            .filter(|(_, r)| r.starts_with("[BLOCKED:"))
            .map(|(p, _)| p.as_str())
            .collect();
        if blocked.is_empty() {
            "clean".to_string()
        } else {
            format!("blocked: {}", blocked.join(", "))
        }
    }
}

// ── Trait ────────────────────────────────────────────────────────────────────

/// Abstraction over skill content scanning, allowing test injection.
///
/// The install pipeline calls `scan_bundle` between quarantine write and
/// atomic move.  Production uses `CoreSkillScanner`; tests inject
/// `AlwaysCleanScanner` or `AlwaysBlockedScanner`.
pub trait SkillScanner: Send + Sync {
    /// Scan all files in a bundle.  Returns a verdict with per-file results.
    fn scan_bundle(&self, files: &[BundleFile]) -> ScanVerdict;
}

// ── Production implementation ───────────────────────────────────────────────

/// Delegates to `ironhermes_core::context_scanner::scan_skill_content`.
pub struct CoreSkillScanner;

impl SkillScanner for CoreSkillScanner {
    fn scan_bundle(&self, files: &[BundleFile]) -> ScanVerdict {
        let mut results = Vec::with_capacity(files.len());
        for file in files {
            let content = String::from_utf8_lossy(&file.bytes);
            let result = ironhermes_core::context_scanner::scan_skill_content(&content, &file.path);
            results.push((file.path.clone(), result));
        }
        ScanVerdict {
            file_results: results,
        }
    }
}

// ── Test helpers ────────────────────────────────────────────────────────────

/// Scanner that always returns clean verdicts (for happy-path tests).
pub struct AlwaysCleanScanner;

impl SkillScanner for AlwaysCleanScanner {
    fn scan_bundle(&self, files: &[BundleFile]) -> ScanVerdict {
        ScanVerdict {
            file_results: files
                .iter()
                .map(|f| {
                    (
                        f.path.clone(),
                        String::from_utf8_lossy(&f.bytes).into_owned(),
                    )
                })
                .collect(),
        }
    }
}

/// Scanner that always blocks with a fixed reason (for rejection tests).
pub struct AlwaysBlockedScanner {
    pub reason: String,
}

impl AlwaysBlockedScanner {
    pub fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
        }
    }
}

impl SkillScanner for AlwaysBlockedScanner {
    fn scan_bundle(&self, files: &[BundleFile]) -> ScanVerdict {
        ScanVerdict {
            file_results: files
                .iter()
                .map(|f| (f.path.clone(), format!("[BLOCKED: {}]", self.reason)))
                .collect(),
        }
    }
}

// ── Trust enforcement (D-15) ────────────────────────────────────────────────

/// Apply D-15 trust-gated enforcement to a scan verdict.
///
/// - Community + scan-hit = hard-reject (`Err`)
/// - Builtin/Official/Trusted + scan-hit = WARN-BUT-LOAD (returns `Ok` with warning logged)
/// - No scan hits = always `Ok`
pub fn enforce_trust_gate(
    trust: ironhermes_core::SkillSource,
    verdict: &ScanVerdict,
) -> Result<(), HubError> {
    if !verdict.has_blocks() {
        return Ok(());
    }

    let blocked_files: Vec<&str> = verdict
        .file_results
        .iter()
        .filter(|(_, r)| r.starts_with("[BLOCKED:"))
        .map(|(p, _)| p.as_str())
        .collect();

    match trust {
        ironhermes_core::SkillSource::Community => Err(HubError::Typed {
            kind: HubErrorKind::ScanBlocked,
            message: format!(
                "Community skill blocked by security scan: {}",
                blocked_files.join(", ")
            ),
            suggestion: Some(
                "Community skills with scan hits are hard-rejected (D-15). \
                 To allow, add the source repo to hub.trusted_repos in config.yaml."
                    .to_string(),
            ),
            retry_after_s: None,
        }),
        _ => {
            // WARN-BUT-LOAD for Builtin/Official/Trusted
            tracing::warn!(
                blocked_files = ?blocked_files,
                trust_level = ?trust,
                "Scan hit on trusted skill; proceeding with WARN-BUT-LOAD (D-15)"
            );
            Ok(())
        }
    }
}
