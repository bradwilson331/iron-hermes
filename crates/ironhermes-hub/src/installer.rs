//! Skill install / update / uninstall pipeline (Phase 21.8, D-25).
//!
//! PIPELINE ORDER LOCKED (D-25): migrate -> fetch -> audit -> quarantine-write
//! -> scan (trust-gated) -> atomic rename -> post-install hash verify -> lock write.
//!
//! Any reordering is a D-25 violation (see 21.8-RESEARCH.md Pitfall 3).
//!
//! Failed installs leave no partial state in `skills/`; the quarantine `TempDir`
//! is cleaned up on drop.
//!
//! Security mitigations:
//! - D-11: quarantine isolation — bundle never written directly to final location.
//! - D-15: trust-gated scan enforcement via `enforce_trust_gate`.
//! - D-18: `sanitize::sanitize_subpath` runs BEFORE `validate_bundle_rel_path`
//!   in `write_bundle_to_dir` to reject server-originated path traversal.
//! - D-20: `sanitize::assert_temp_contained` gates every `remove_dir_all` on
//!   a quarantine path (symlink-swap guard).
//! - D-13/D-14: after atomic rename, `compute_folder_hash(final_path)` is observed
//!   against `bundle.snapshot_hash` as ADVISORY telemetry; mismatch logs a
//!   `tracing::warn!` and proceeds. Client-authoritative drift detection uses the
//!   stored `SkillLockEntry.computed_hash` on subsequent `update()` calls, NOT
//!   server/client parity (per D-14, `snapshotHash` is opaque).
//! - D-19: soft-fail audit endpoint call between fetch and quarantine (never blocks install).

use std::path::{Path, PathBuf};

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::error::{HubError, HubErrorKind};
use crate::lock::{SkillLock, SkillLockEntry, compute_folder_hash};
use crate::scanner::{SkillScanner, enforce_trust_gate};
use crate::source::{HubSource, SkillBundle};

// ── Install outcome ─────────────────────────────────────────────────────────

/// Result of a successful install.
#[derive(Debug)]
pub struct InstallOutcome {
    pub name: String,
    pub install_path: PathBuf,
    pub content_hash: String,
    pub scan_verdict: String,
    pub trust_level: ironhermes_core::SkillSource,
}

/// Result of a successful update.
#[derive(Debug)]
pub struct UpdateOutcome {
    pub name: String,
    pub install_path: PathBuf,
    pub old_hash: String,
    pub new_hash: String,
    pub scan_verdict: String,
}

/// Result of a successful uninstall.
#[derive(Debug)]
pub struct UninstallOutcome {
    pub name: String,
    pub removed_path: PathBuf,
}

// ── Content hash ────────────────────────────────────────────────────────────

/// Compute a deterministic SHA-256 hash over the bundle's files.
///
/// Matches Python `bundle_content_hash`: sort files by path, then feed
/// `path_bytes + 0x00 + content_bytes` for each file into the hasher.
///
/// NOTE: This is DISTINCT from `lock::compute_folder_hash` (which walks disk and
/// uses NO separators per D-13). `bundle_content_hash` hashes the in-memory bundle
/// and is used as the pre-21.8 provenance fingerprint. For 21.8 drift detection
/// against `SkillLockEntry::computed_hash`, use `bundle_folder_hash` which shares
/// the no-separator D-13 algorithm with `compute_folder_hash`.
pub fn bundle_content_hash(bundle: &SkillBundle) -> String {
    let mut hasher = Sha256::new();

    let mut sorted: Vec<_> = bundle.files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));

    for file in &sorted {
        hasher.update(file.path.as_bytes());
        hasher.update([0x00]);
        hasher.update(&file.bytes);
    }

    hex::encode(hasher.finalize())
}

/// D-13-compatible in-memory hash over a bundle's files.
///
/// Mirrors `lock::compute_folder_hash` semantics (sorted by forward-slash-normalized
/// path, NO separator between path and content, NO separator between files) so the
/// result compares byte-for-byte against `compute_folder_hash` applied to the
/// on-disk install at `install_path`.
///
/// Crate-private: used by `update()` for drift detection without a second disk walk.
fn bundle_folder_hash(bundle: &SkillBundle) -> String {
    let mut files: Vec<(String, &[u8])> = bundle
        .files
        .iter()
        .map(|f| (f.path.replace('\\', "/"), f.bytes.as_slice()))
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (path, content) in &files {
        hasher.update(path.as_bytes());
        hasher.update(content);
    }
    hex::encode(hasher.finalize())
}

// ── Skill name / category parsing ───────────────────────────────────────────

/// Parse category and skill name from the SKILL.md frontmatter or identifier.
///
/// If the SKILL.md frontmatter has `metadata.hermes.category`, use that.
/// Otherwise default to `"general"`.
fn parse_skill_identity(bundle: &SkillBundle) -> (String, String) {
    let category = extract_category_from_frontmatter(&bundle.skill_md)
        .unwrap_or_else(|| "general".to_string());
    (category, bundle.name.clone())
}

/// Extract `metadata.hermes.category` from SKILL.md frontmatter YAML.
fn extract_category_from_frontmatter(skill_md: &str) -> Option<String> {
    let trimmed = skill_md.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_start = &trimmed[3..];
    let end = after_start.find("\n---")?;
    let yaml_block = &after_start[..end];

    let doc: serde_yaml::Value = serde_yaml::from_str(yaml_block).ok()?;
    doc.get("metadata")?
        .get("hermes")?
        .get("category")?
        .as_str()
        .map(|s| s.to_string())
}

/// Extract `<owner>/<repo>` from a bundle identifier for audit lookups.
///
/// Supports the formats produced by the current `HubSource` impls:
/// - GitHub: `owner/repo[/path...]`                   -> `owner/repo`
/// - skills-sh: `skills-sh:owner/repo/slug`           -> `owner/repo`
/// - well-known URLs: `https://host/path` / `well-known:host/path` -> host/path or empty
///
/// Returns an empty string when no sensible owner/repo can be extracted —
/// the caller must treat that as "do not audit".
fn extract_owner_repo(bundle: &SkillBundle) -> String {
    let ident = bundle
        .identifier
        .strip_prefix("skills-sh:")
        .unwrap_or(&bundle.identifier);

    // well-known / HTTPS identifiers don't have owner/repo semantics; skip audit.
    if ident.starts_with("http://")
        || ident.starts_with("https://")
        || ident.starts_with("well-known:")
    {
        return String::new();
    }

    let parts: Vec<&str> = ident.splitn(3, '/').collect();
    if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        format!("{}/{}", parts[0], parts[1])
    } else {
        String::new()
    }
}

/// Derive the `repo_path` field of a `SkillLockEntry` from a bundle.
///
/// Prefers the first file path (usually `<skill-dir>/SKILL.md`); falls back
/// to empty if the bundle carries no files.
fn extract_repo_path(bundle: &SkillBundle) -> String {
    bundle
        .files
        .first()
        .map(|f| f.path.clone())
        .unwrap_or_default()
}

fn typed(kind: HubErrorKind, msg: impl Into<String>) -> HubError {
    HubError::Typed {
        kind,
        message: msg.into(),
        suggestion: None,
        retry_after_s: None,
    }
}

// ── Install pipeline ────────────────────────────────────────────────────────

/// PIPELINE ORDER LOCKED (D-25) — any reordering is a D-25 violation.
///
/// 1. **Migrate** — best-effort idempotent 19.1 `.hub/lock.json` → 21.8 `skills-lock.json`.
/// 2. **Fetch** — `source.fetch(identifier)` returns the `SkillBundle` including
///    the server-returned `bundle.snapshot_hash` (None for github/well-known).
/// 3. **Audit** — if `skip_audit=false`, call `audit::fetch_audit` with a 3 s
///    timeout; soft-fail to `None` on any error (D-19). Never blocks install.
/// 4. **Quarantine** — write bundle to a tempdir under `.hub/quarantine/` via
///    `write_bundle_to_dir`, which now runs `sanitize_subpath` BEFORE
///    `validate_bundle_rel_path` (D-18).
/// 5. **Scan** — run the skill scanner and apply D-15 trust enforcement. On
///    failure, cleanup quarantine via `cleanup_quarantine_safely` (D-20 gate).
/// 6. **Atomic rename** — `rename` (or copy+remove) from quarantine to final path.
/// 7. **Post-install hash observation (D-13/D-14, advisory)** — `compute_folder_hash`
///    the final path; if `bundle.snapshot_hash` is `Some(non_empty)` and differs,
///    emit `tracing::warn!` and proceed. The install dir is NOT cleaned and no
///    error is returned — mismatch is telemetry only (D-14 opaque contract).
/// 8. **Lock file write** — `SkillLock::load_or_default` → `add_or_replace` →
///    `save_atomic` (replaces the legacy 19.1 manifest write).
pub async fn install(
    source: &dyn HubSource,
    identifier: &str,
    scanner: &dyn SkillScanner,
    skills_root: &Path,
    skip_audit: bool,
) -> Result<InstallOutcome, HubError> {
    // ── Step 1: Migrate 19.1 -> 21.8 (idempotent; safe on every call). ─────
    let _ = crate::lock::migrate_from_hub_manifest()
        .map_err(|e| typed(HubErrorKind::Io, format!("migration failed: {e}")))?;

    // ── Step 2: Fetch ──────────────────────────────────────────────────────
    let bundle = source.fetch(identifier).await?;
    let content_hash = bundle_content_hash(&bundle);
    // Read the snapshot hash directly off the bundle (plan 02 contract): no
    // side-channel, no trait extension, no alternative plumbing.
    let server_snapshot_hash: Option<String> = bundle.snapshot_hash.clone();

    // ── Step 3: Audit (D-19 soft-fail; skipped if --skip-audit) ────────────
    if !skip_audit {
        let owner_repo = extract_owner_repo(&bundle);
        if !owner_repo.is_empty() {
            if let Ok(client) = reqwest::Client::builder()
                .user_agent(concat!("ironhermes-hub/", env!("CARGO_PKG_VERSION")))
                .build()
            {
                let slug = crate::sanitize::to_skill_slug(&bundle.name);
                if let Some(audit) = crate::audit::fetch_audit(&client, &owner_repo, &[&slug]).await
                {
                    for (s, a) in &audit {
                        tracing::info!(skill = %s, risk = %a.risk, alerts = a.alerts, "audit result");
                    }
                }
            }
        }
    }

    // ── Step 4: Quarantine ─────────────────────────────────────────────────
    let quarantine_root = crate::paths::quarantine_dir()?;
    std::fs::create_dir_all(&quarantine_root)?;
    let quarantine = tempfile::tempdir_in(&quarantine_root)?;
    write_bundle_to_dir(quarantine.path(), &bundle)?;

    // ── Step 5: Scan (D-15 trust-gated enforcement) ────────────────────────
    let trust = source.trust_level_for(identifier);
    let verdict = scanner.scan_bundle(&bundle.files);
    if let Err(e) = enforce_trust_gate(trust, &verdict) {
        // D-20 gated cleanup on scan failure.
        cleanup_quarantine_safely(quarantine.path());
        return Err(e);
    }

    // ── Step 6: Atomic rename ──────────────────────────────────────────────
    let (category, name) = parse_skill_identity(&bundle);
    let final_path = skills_root.join(&category).join(&name);

    if final_path.exists() {
        cleanup_quarantine_safely(quarantine.path());
        return Err(HubError::Typed {
            kind: HubErrorKind::AlreadyInstalled,
            message: format!(
                "skill '{}' is already installed at {}",
                name,
                final_path.display()
            ),
            suggestion: Some(format!(
                "Run 'hermes skills update {}' to update, or 'hermes skills uninstall {}' first.",
                name, name
            )),
            retry_after_s: None,
        });
    }

    std::fs::create_dir_all(final_path.parent().unwrap_or(skills_root))?;
    atomic_move(quarantine.path(), &final_path)?;
    // Consume the TempDir without running its destructor (the dir was moved).
    let _ = quarantine.keep();

    // ── Step 7: Post-install hash observation (D-13/D-14 — advisory) ────
    //
    // D-14 declares `snapshotHash` OPAQUE: the server may hash the skill
    // bundle with any algorithm it chooses, and the client MUST NOT
    // recompute it or enforce byte-for-byte equality. We keep the
    // local D-13 `compute_folder_hash` for two reasons:
    //   1. It is the value that flows into `SkillLockEntry.computed_hash`
    //      (the client-authoritative drift-detection hash used by
    //      `update()` and `list`-side tamper checks).
    //   2. When it disagrees with the server-returned snapshot, that is
    //      ONLY telemetry — the install has already succeeded on disk
    //      and the opaque server value still round-trips verbatim into
    //      `SkillLockEntry.snapshot_hash` at Step 8.
    //
    // UAT blocker (21.8-06): skills.sh's production hash algorithm is not
    // our D-13 no-separator SHA-256, so 100% of live installs tripped the
    // previous strict equality check. Strict-mode gating is deferred
    // (not built in this plan); see decisions G-01/G-02 in 21.8-06-PLAN.md.
    let computed = compute_folder_hash(&final_path)?;
    if let Some(server_hash) = &server_snapshot_hash {
        if !server_hash.is_empty() && &computed != server_hash {
            tracing::warn!(
                computed_hash = %computed,
                server_snapshot_hash = %server_hash,
                skill = %name,
                "server snapshotHash differs from local folder hash — advisory only, install proceeding (D-14 opaque contract)"
            );
        }
    }

    // ── Step 8: SkillLock write (replaces legacy 19.1 manifest write) ──────
    let scan_summary = verdict.summary();
    let mut lock = SkillLock::load_or_default()
        .map_err(|e| typed(HubErrorKind::Io, format!("load skills-lock.json: {e}")))?;
    lock.add_or_replace(SkillLockEntry {
        name: name.clone(),
        source: source.source_id().to_string(),
        identifier: identifier.to_string(),
        repo_path: extract_repo_path(&bundle),
        snapshot_hash: server_snapshot_hash.unwrap_or_default(),
        computed_hash: computed,
        installed_at: Utc::now(),
        extras: Default::default(),
    });
    lock.save_atomic()
        .map_err(|e| typed(HubErrorKind::Io, format!("save skills-lock.json: {e}")))?;

    Ok(InstallOutcome {
        name,
        install_path: final_path,
        content_hash,
        scan_verdict: scan_summary,
        trust_level: trust,
    })
}

// ── Update pipeline ─────────────────────────────────────────────────────────

/// Update a previously installed skill.
///
/// 1. Look up the existing lock entry in `skills-lock.json`.
/// 2. Fetch the latest bundle from the same source.
/// 3. Compare the server `snapshot_hash` against the stored `snapshot_hash`
///    when available; otherwise fall back to `bundle_content_hash` parity.
/// 4. On drift: quarantine → scan → atomic replace → post-install verify → lock write.
pub async fn update(
    source: &dyn HubSource,
    skill_name: &str,
    scanner: &dyn SkillScanner,
    skills_root: &Path,
    skip_audit: bool,
) -> Result<UpdateOutcome, HubError> {
    // Idempotent 19.1 -> 21.8 migration (so `hermes skills update` on a pre-21.8
    // machine picks up the prior install set before lookup).
    let _ = crate::lock::migrate_from_hub_manifest()
        .map_err(|e| typed(HubErrorKind::Io, format!("migration failed: {e}")))?;

    let mut lock = SkillLock::load_or_default()
        .map_err(|e| typed(HubErrorKind::Io, format!("load skills-lock.json: {e}")))?;
    let entry = lock
        .get(skill_name)
        .cloned()
        .ok_or_else(|| HubError::Typed {
            kind: HubErrorKind::NotFound,
            message: format!("skill '{}' is not installed", skill_name),
            suggestion: Some("Run 'hermes skills list' to see installed skills.".to_string()),
            retry_after_s: None,
        })?;

    let identifier = entry.identifier.clone();
    let old_hash = entry.computed_hash.clone();
    let install_path = skills_root
        .join(
            // best-effort category resolution: look one level up from repo_path or default
            // general. We recompute category after the fresh fetch below.
            "general",
        )
        .join(&entry.name);

    // Fetch latest bundle.
    let bundle = source.fetch(&identifier).await?;
    let server_snapshot_hash: Option<String> = bundle.snapshot_hash.clone();

    // Drift detection (algorithmically consistent with SkillLockEntry.computed_hash):
    //  - If both old + new snapshot_hash are known AND equal -> no-op (fast path).
    //  - Else compare D-13 folder hashes: bundle_folder_hash vs entry.computed_hash.
    //    bundle_folder_hash uses the same no-separator algorithm as compute_folder_hash
    //    so the comparison is apples-to-apples.
    let fresh_folder_hash = bundle_folder_hash(&bundle);
    let drift_detected = match (&server_snapshot_hash, entry.snapshot_hash.is_empty()) {
        (Some(new), false) if new == &entry.snapshot_hash => false, // same snapshot
        (Some(new), _) if !new.is_empty() => true,                  // different snapshot
        _ => old_hash != fresh_folder_hash,
    };
    if !drift_detected {
        return Err(HubError::Typed {
            kind: HubErrorKind::AlreadyInstalled,
            message: format!(
                "skill '{}' is already up to date (hash: {})",
                skill_name,
                old_hash.get(..12).unwrap_or(&old_hash)
            ),
            suggestion: None,
            retry_after_s: None,
        });
    }

    // Audit (soft-fail, same as install).
    if !skip_audit {
        let owner_repo = extract_owner_repo(&bundle);
        if !owner_repo.is_empty() {
            if let Ok(client) = reqwest::Client::builder()
                .user_agent(concat!("ironhermes-hub/", env!("CARGO_PKG_VERSION")))
                .build()
            {
                let slug = crate::sanitize::to_skill_slug(&bundle.name);
                if let Some(audit) = crate::audit::fetch_audit(&client, &owner_repo, &[&slug]).await
                {
                    for (s, a) in &audit {
                        tracing::info!(skill = %s, risk = %a.risk, alerts = a.alerts, "audit result");
                    }
                }
            }
        }
    }

    // Quarantine new version.
    let quarantine_root = crate::paths::quarantine_dir()?;
    std::fs::create_dir_all(&quarantine_root)?;
    let quarantine = tempfile::tempdir_in(&quarantine_root)?;
    write_bundle_to_dir(quarantine.path(), &bundle)?;

    // Re-scan.
    let trust = source.trust_level_for(&identifier);
    let verdict = scanner.scan_bundle(&bundle.files);
    if let Err(e) = enforce_trust_gate(trust, &verdict) {
        cleanup_quarantine_safely(quarantine.path());
        return Err(e);
    }

    // Atomic replace: remove old dir (D-20 gated only for quarantine paths —
    // the old install_path is under skills_root so we use is_path_safe).
    let (category, name) = parse_skill_identity(&bundle);
    let resolved_final = skills_root.join(&category).join(&name);
    // Clean old install location if it exists (path may differ from resolved_final
    // if the category moved between versions).
    for candidate in [&install_path, &resolved_final] {
        if candidate.exists() {
            cleanup_final_path_safely(candidate);
        }
    }

    std::fs::create_dir_all(resolved_final.parent().unwrap_or(skills_root))?;
    atomic_move(quarantine.path(), &resolved_final)?;
    let _ = quarantine.keep();

    // Post-install hash observation (D-13/D-14 — advisory; see install() Step 7 for rationale).
    let computed = compute_folder_hash(&resolved_final)?;
    if let Some(server_hash) = &server_snapshot_hash {
        if !server_hash.is_empty() && &computed != server_hash {
            tracing::warn!(
                computed_hash = %computed,
                server_snapshot_hash = %server_hash,
                skill = %skill_name,
                "server snapshotHash differs from local folder hash on update — advisory only, replacement proceeding (D-14 opaque contract)"
            );
        }
    }

    // Update lock entry.
    let scan_summary = verdict.summary();
    lock.add_or_replace(SkillLockEntry {
        name: name.clone(),
        source: source.source_id().to_string(),
        identifier: identifier.clone(),
        repo_path: extract_repo_path(&bundle),
        snapshot_hash: server_snapshot_hash.unwrap_or_default(),
        computed_hash: computed.clone(),
        installed_at: Utc::now(),
        extras: entry.extras.clone(),
    });
    lock.save_atomic()
        .map_err(|e| typed(HubErrorKind::Io, format!("save skills-lock.json: {e}")))?;

    Ok(UpdateOutcome {
        name: skill_name.to_string(),
        install_path: resolved_final,
        old_hash,
        new_hash: computed,
        scan_verdict: scan_summary,
    })
}

// ── Uninstall ───────────────────────────────────────────────────────────────

/// Remove an installed skill: delete directory + lock entry atomically.
///
/// De-registers the skill from `skills-lock.json` first so that if the directory
/// removal fails, the skill is at least de-registered (orphan cleanup can
/// handle the dir later). Cleanup of the skill directory is gated by
/// `sanitize::is_path_safe(skills_root, skill_dir)` to prevent arbitrary path
/// deletion if the lock file is ever tampered with.
pub fn uninstall(skill_name: &str) -> Result<UninstallOutcome, HubError> {
    // Idempotent 19.1 -> 21.8 migration.
    let _ = crate::lock::migrate_from_hub_manifest()
        .map_err(|e| typed(HubErrorKind::Io, format!("migration failed: {e}")))?;

    let mut lock = SkillLock::load_or_default()
        .map_err(|e| typed(HubErrorKind::Io, format!("load skills-lock.json: {e}")))?;
    let entry = lock.remove(skill_name).ok_or_else(|| HubError::Typed {
        kind: HubErrorKind::NotFound,
        message: format!("skill '{}' is not installed", skill_name),
        suggestion: Some("Run 'hermes skills list' to see installed skills.".to_string()),
        retry_after_s: None,
    })?;

    let skills_root = crate::paths::skills_root()
        .map_err(|e| typed(HubErrorKind::Io, format!("skills_root: {e}")))?;
    // Best-effort resolution: <skills_root>/<category>/<name>. The lock entry
    // stores repo_path (first file) which by convention starts with the skill
    // dir path, but the category is parsed fresh from the on-disk SKILL.md when
    // possible. For uninstall we search both the general/ default and any
    // existing subdir of skills_root that contains the skill dir.
    let install_path = find_install_path(&skills_root, &entry.name)
        .unwrap_or_else(|| skills_root.join("general").join(&entry.name));

    // Save lock first (de-register before dir removal).
    lock.save_atomic()
        .map_err(|e| typed(HubErrorKind::Io, format!("save skills-lock.json: {e}")))?;

    // Remove the skill directory (D-18 gate: must resolve under skills_root).
    if install_path.exists() {
        match crate::sanitize::is_path_safe(&skills_root, &install_path) {
            Ok(true) => {
                std::fs::remove_dir_all(&install_path).map_err(|e| HubError::Typed {
                    kind: HubErrorKind::Io,
                    message: format!(
                        "failed to remove skill directory {}: {}",
                        install_path.display(),
                        e
                    ),
                    suggestion: Some(format!(
                        "Skill '{}' has been de-registered from the lock file. \
                         Manually remove {} if needed.",
                        skill_name,
                        install_path.display()
                    )),
                    retry_after_s: None,
                })?;
            }
            _ => {
                tracing::warn!(
                    path = %install_path.display(),
                    "refusing to remove_dir_all — not under skills_root"
                );
            }
        }
    }

    // Clean up empty parent category dir.
    if let Some(parent) = install_path.parent() {
        if parent.exists() {
            if let Ok(mut entries) = std::fs::read_dir(parent) {
                if entries.next().is_none() {
                    let _ = std::fs::remove_dir(parent);
                }
            }
        }
    }

    Ok(UninstallOutcome {
        name: skill_name.to_string(),
        removed_path: install_path,
    })
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Find `<skills_root>/<category>/<name>` for an arbitrary category. Prefers
/// `general/` for default installs but scans every top-level subdir of
/// `skills_root` for a matching skill directory (except `.hub`).
fn find_install_path(skills_root: &Path, name: &str) -> Option<PathBuf> {
    let default = skills_root.join("general").join(name);
    if default.exists() {
        return Some(default);
    }
    for entry in std::fs::read_dir(skills_root).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() && p.file_name().and_then(|s| s.to_str()) != Some(".hub") {
            let candidate = p.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Write all files from a bundle into a directory.
///
/// D-18: `sanitize::sanitize_subpath` is invoked BEFORE the existing
/// `tarball::validate_bundle_rel_path` so server-supplied path traversal is
/// rejected with `HubErrorKind::PathTraversal` before the tar-centric guard.
fn write_bundle_to_dir(dir: &Path, bundle: &SkillBundle) -> Result<(), HubError> {
    for file in &bundle.files {
        // D-18: sanitize server-supplied path FIRST (rejects .., absolute, NUL, drive).
        let _safe = crate::sanitize::sanitize_subpath(&file.path)?;
        // Then the tar-centric belt-and-suspenders check.
        let _ = crate::tarball::validate_bundle_rel_path(&file.path)?;
        let dest = dir.join(&file.path);
        if !dest.starts_with(dir) {
            return Err(HubError::Typed {
                kind: HubErrorKind::PathTraversal,
                message: format!("path escapes target directory: {}", file.path),
                suggestion: None,
                retry_after_s: None,
            });
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, &file.bytes)?;
    }
    Ok(())
}

/// D-20: never call `remove_dir_all` on a path not contained in a known-safe
/// root. Two roots are accepted:
///   - `env::temp_dir()` (traditional tmp-based quarantine)
///   - `<skills_root>/.hub/quarantine/` (our actual quarantine location; see
///     `paths::quarantine_dir`). `tempfile::tempdir_in(quarantine_root)`
///     places the tmp dir here, which is NOT under `env::temp_dir()` on
///     macOS, so gating on temp_dir alone refused every legitimate cleanup.
/// Symlink-swap defense is preserved: we still canonicalize and verify
/// containment before any `remove_dir_all`.
fn cleanup_quarantine_safely(p: &Path) {
    if is_in_accepted_cleanup_root(p) {
        let _ = std::fs::remove_dir_all(p);
    } else {
        tracing::warn!(
            path = %p.display(),
            "refusing to remove_dir_all — path not contained in temp_dir or quarantine_dir (D-20)"
        );
    }
}

/// True iff `p` canonicalizes under `env::temp_dir()` OR under the configured
/// `.hub/quarantine/` root. Either containment satisfies the D-20 symlink-swap
/// guard because both roots are install-controlled.
fn is_in_accepted_cleanup_root(p: &Path) -> bool {
    if crate::sanitize::assert_temp_contained(p).is_ok() {
        return true;
    }
    if let Ok(qroot) = crate::paths::quarantine_dir() {
        if let Ok(true) = crate::sanitize::is_path_safe(&qroot, p) {
            return true;
        }
    }
    false
}

/// Cleanup of the final install directory on post-rename failure (ShaMismatch).
///
/// NOT gated by `assert_temp_contained` because this path is explicitly under
/// `skills_root` — use `is_path_safe(skills_root, final_path)` as the guard.
fn cleanup_final_path_safely(final_path: &Path) {
    if let Ok(root) = crate::paths::skills_root() {
        match crate::sanitize::is_path_safe(&root, final_path) {
            Ok(true) => {
                let _ = std::fs::remove_dir_all(final_path);
            }
            _ => {
                tracing::warn!(
                    path = %final_path.display(),
                    "refusing to remove_dir_all — not under skills_root"
                );
            }
        }
    }
}

/// Atomic move: try `rename` first, fall back to recursive copy + remove.
///
/// `rename` only works within the same filesystem.  The quarantine lives under
/// `.hub/quarantine/` which is the same FS as the skills root, so `rename`
/// should succeed in normal operation.  The fallback handles edge cases (e.g.
/// bind-mounted `/tmp` in containers).
fn atomic_move(src: &Path, dst: &Path) -> Result<(), HubError> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_rename_err) => {
            copy_dir_recursive(src, dst)?;
            // We copied out of the quarantine tmp path; the D-20 gate applies.
            cleanup_quarantine_safely(src);
            Ok(())
        }
    }
}

/// Recursively copy a directory tree.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), HubError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::BundleFile;

    fn mk_bundle(files: Vec<BundleFile>) -> SkillBundle {
        SkillBundle {
            name: "test".to_string(),
            identifier: "owner/repo/test".to_string(),
            source_id: "github".to_string(),
            files,
            skill_md: "---\nname: test\n---\nbody".to_string(),
            metadata: serde_json::json!({}),
            snapshot_hash: None,
        }
    }

    #[test]
    fn test_bundle_content_hash_deterministic() {
        let bundle = mk_bundle(vec![
            BundleFile {
                path: "SKILL.md".to_string(),
                bytes: b"---\nname: test\n---\nbody".to_vec(),
            },
            BundleFile {
                path: "handler.py".to_string(),
                bytes: b"# code".to_vec(),
            },
        ]);
        let h1 = bundle_content_hash(&bundle);
        let h2 = bundle_content_hash(&bundle);
        assert_eq!(h1, h2, "hash must be deterministic");
        assert_eq!(h1.len(), 64, "SHA-256 hex digest is 64 chars");
    }

    #[test]
    fn test_bundle_content_hash_sorted_by_path() {
        let files_a = vec![
            BundleFile {
                path: "a.txt".to_string(),
                bytes: b"aaa".to_vec(),
            },
            BundleFile {
                path: "b.txt".to_string(),
                bytes: b"bbb".to_vec(),
            },
        ];
        let files_b = vec![
            BundleFile {
                path: "b.txt".to_string(),
                bytes: b"bbb".to_vec(),
            },
            BundleFile {
                path: "a.txt".to_string(),
                bytes: b"aaa".to_vec(),
            },
        ];
        assert_eq!(
            bundle_content_hash(&mk_bundle(files_a)),
            bundle_content_hash(&mk_bundle(files_b))
        );
    }

    #[test]
    fn test_bundle_content_hash_differs_on_content_change() {
        let mk = |data: &[u8]| {
            mk_bundle(vec![BundleFile {
                path: "f.txt".into(),
                bytes: data.to_vec(),
            }])
        };
        assert_ne!(
            bundle_content_hash(&mk(b"hello")),
            bundle_content_hash(&mk(b"world"))
        );
    }

    #[test]
    fn test_extract_category_from_frontmatter() {
        let md = "---\nname: test\nmetadata:\n  hermes:\n    category: automation\n---\nbody";
        assert_eq!(
            extract_category_from_frontmatter(md),
            Some("automation".to_string())
        );
    }

    #[test]
    fn test_extract_category_missing_defaults_to_none() {
        let md = "---\nname: test\n---\nbody";
        assert_eq!(extract_category_from_frontmatter(md), None);
    }

    #[test]
    fn test_parse_skill_identity_with_category() {
        let mut bundle = mk_bundle(vec![]);
        bundle.name = "my-skill".into();
        bundle.skill_md =
            "---\nname: my-skill\nmetadata:\n  hermes:\n    category: devops\n---\n".into();
        let (cat, name) = parse_skill_identity(&bundle);
        assert_eq!(cat, "devops");
        assert_eq!(name, "my-skill");
    }

    #[test]
    fn test_parse_skill_identity_defaults_to_general() {
        let mut bundle = mk_bundle(vec![]);
        bundle.name = "my-skill".into();
        bundle.skill_md = "---\nname: my-skill\n---\n".into();
        let (cat, name) = parse_skill_identity(&bundle);
        assert_eq!(cat, "general");
        assert_eq!(name, "my-skill");
    }

    #[test]
    fn test_extract_owner_repo_from_github_identifier() {
        let mut bundle = mk_bundle(vec![]);
        bundle.identifier = "anthropics/skills/tenor-gif".into();
        assert_eq!(extract_owner_repo(&bundle), "anthropics/skills");
    }

    #[test]
    fn test_extract_owner_repo_strips_skills_sh_prefix() {
        let mut bundle = mk_bundle(vec![]);
        bundle.identifier = "skills-sh:o/r/slug".into();
        assert_eq!(extract_owner_repo(&bundle), "o/r");
    }

    #[test]
    fn test_extract_owner_repo_returns_empty_for_https_identifier() {
        let mut bundle = mk_bundle(vec![]);
        bundle.identifier = "https://example.com/path".into();
        assert_eq!(extract_owner_repo(&bundle), "");
    }

    #[test]
    fn test_extract_owner_repo_returns_empty_for_well_known() {
        let mut bundle = mk_bundle(vec![]);
        bundle.identifier = "well-known:example.com".into();
        assert_eq!(extract_owner_repo(&bundle), "");
    }

    #[test]
    fn test_extract_repo_path_prefers_first_file() {
        let bundle = mk_bundle(vec![
            BundleFile {
                path: "my-skill/SKILL.md".into(),
                bytes: b"x".to_vec(),
            },
            BundleFile {
                path: "my-skill/handler.py".into(),
                bytes: b"y".to_vec(),
            },
        ]);
        assert_eq!(extract_repo_path(&bundle), "my-skill/SKILL.md");
    }

    #[test]
    fn test_extract_repo_path_empty_for_no_files() {
        let bundle = mk_bundle(vec![]);
        assert_eq!(extract_repo_path(&bundle), "");
    }

    #[test]
    fn test_write_bundle_to_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = mk_bundle(vec![
            BundleFile {
                path: "SKILL.md".into(),
                bytes: b"# skill".to_vec(),
            },
            BundleFile {
                path: "sub/handler.py".into(),
                bytes: b"# code".to_vec(),
            },
        ]);
        write_bundle_to_dir(tmp.path(), &bundle).unwrap();
        assert!(tmp.path().join("SKILL.md").exists());
        assert!(tmp.path().join("sub/handler.py").exists());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("SKILL.md")).unwrap(),
            "# skill"
        );
    }

    #[test]
    fn test_write_bundle_rejects_sanitize_subpath_violations() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = mk_bundle(vec![BundleFile {
            path: "../evil.md".into(),
            bytes: b"bad".to_vec(),
        }]);
        let err = write_bundle_to_dir(tmp.path(), &bundle).expect_err("should reject traversal");
        match err {
            HubError::Typed { kind, .. } => assert_eq!(kind, HubErrorKind::PathTraversal),
            other => panic!("expected PathTraversal, got {other:?}"),
        }
    }

    #[test]
    fn test_cleanup_quarantine_safely_skips_non_temp_paths() {
        // Build a dir OUTSIDE env::temp_dir and assert cleanup refuses to
        // remove it.
        let home = dirs::home_dir().unwrap();
        let outside = home.join(".ironhermes-test-cleanup-should-not-exist");
        if outside.exists() {
            let _ = std::fs::remove_dir_all(&outside);
        }
        std::fs::create_dir_all(&outside).unwrap();
        cleanup_quarantine_safely(&outside);
        // The gate MUST have refused removal — the dir still exists.
        assert!(
            outside.exists(),
            "cleanup_quarantine_safely must refuse to remove non-temp paths"
        );
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn test_cleanup_quarantine_safely_removes_temp_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_path_buf();
        assert!(path.exists());
        // Consume the TempDir so the Drop doesn't fight us.
        let _ = tmp.keep();
        cleanup_quarantine_safely(&path);
        assert!(!path.exists(), "temp-contained path should be removed");
    }

    #[test]
    fn test_cleanup_quarantine_safely_removes_under_quarantine_root() {
        // Regression test for the macOS false-positive where HERMES_HOME lives
        // outside env::temp_dir(). The fix accepts paths under
        // `<skills_root>/.hub/quarantine/` as a second safe root.
        let fake_home = tempfile::tempdir().unwrap();
        let prev = std::env::var("HERMES_HOME").ok();
        unsafe {
            std::env::set_var("HERMES_HOME", fake_home.path());
        }

        let qroot = crate::paths::quarantine_dir().unwrap();
        std::fs::create_dir_all(&qroot).unwrap();
        let quarantine_tmp = tempfile::tempdir_in(&qroot).unwrap();
        let path = quarantine_tmp.path().to_path_buf();
        let _ = quarantine_tmp.keep();

        assert!(path.exists());
        cleanup_quarantine_safely(&path);
        assert!(
            !path.exists(),
            "path under quarantine_dir() must be cleaned by D-20 gate"
        );

        unsafe {
            match prev {
                Some(v) => std::env::set_var("HERMES_HOME", v),
                None => std::env::remove_var("HERMES_HOME"),
            }
        }
    }

    #[test]
    fn test_atomic_move_same_fs() {
        let parent = tempfile::tempdir().unwrap();
        let src = parent.path().join("src_dir");
        let dst = parent.path().join("dst_dir");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("file.txt"), "hello").unwrap();

        atomic_move(&src, &dst).unwrap();

        assert!(!src.exists(), "source should be gone after move");
        assert!(dst.join("file.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dst.join("file.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn test_copy_dir_recursive() {
        let parent = tempfile::tempdir().unwrap();
        let src = parent.path().join("src");
        let dst = parent.path().join("dst");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "aaa").unwrap();
        std::fs::write(src.join("sub/b.txt"), "bbb").unwrap();

        copy_dir_recursive(&src, &dst).unwrap();

        assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "aaa");
        assert_eq!(
            std::fs::read_to_string(dst.join("sub/b.txt")).unwrap(),
            "bbb"
        );
    }

    // ── Advisory-path unit tests (21.8-06) ────────────────────────────────────

    /// Minimal in-process HubSource test double for advisory-path unit tests.
    /// Returns a fixed SkillBundle with a caller-supplied snapshot_hash.
    struct FixedBundleSource {
        bundle: crate::source::SkillBundle,
    }

    #[async_trait::async_trait]
    impl crate::source::HubSource for FixedBundleSource {
        fn source_id(&self) -> &str {
            "test-fixed"
        }

        fn trust_level_for(&self, _identifier: &str) -> ironhermes_core::SkillSource {
            ironhermes_core::SkillSource::Official
        }

        async fn search(
            &self,
            _query: &str,
            _limit: usize,
        ) -> Result<Vec<crate::source::SkillMeta>, crate::HubError> {
            Ok(vec![])
        }

        async fn fetch(
            &self,
            _identifier: &str,
        ) -> Result<crate::source::SkillBundle, crate::HubError> {
            Ok(crate::source::SkillBundle {
                name: self.bundle.name.clone(),
                identifier: self.bundle.identifier.clone(),
                source_id: self.bundle.source_id.clone(),
                files: self.bundle.files.clone(),
                skill_md: self.bundle.skill_md.clone(),
                metadata: self.bundle.metadata.clone(),
                snapshot_hash: self.bundle.snapshot_hash.clone(),
            })
        }
    }

    fn make_advisory_bundle(snapshot_hash: Option<String>) -> crate::source::SkillBundle {
        crate::source::SkillBundle {
            name: "advisory-skill".to_string(),
            identifier: "test/repo/advisory-skill".to_string(),
            source_id: "test-fixed".to_string(),
            files: vec![
                BundleFile {
                    path: "SKILL.md".to_string(),
                    bytes: b"---\nname: advisory-skill\n---\nbody".to_vec(),
                },
                BundleFile {
                    path: "helper.py".to_string(),
                    bytes: b"print('advisory')\n".to_vec(),
                },
            ],
            skill_md: "---\nname: advisory-skill\n---\nbody".to_string(),
            metadata: serde_json::json!({}),
            snapshot_hash,
        }
    }

    /// install() must return Ok even when the server-returned snapshot_hash does
    /// not match the locally-computed D-13 folder hash. The advisory branch emits
    /// a tracing::warn but MUST NOT cleanup the install dir or return ShaMismatch.
    #[tokio::test]
    async fn post_install_compare_is_advisory_when_hashes_diverge() {
        let hermes_home = tempfile::tempdir().unwrap();
        let prev_home = std::env::var("HERMES_HOME").ok();
        unsafe {
            std::env::set_var("HERMES_HOME", hermes_home.path());
        }

        let skills_root = hermes_home.path().join("skills");
        std::fs::create_dir_all(&skills_root).unwrap();

        let server_hash = "opaque-server-hash-that-does-not-match".to_string();
        let bundle = make_advisory_bundle(Some(server_hash.clone()));
        let src = FixedBundleSource { bundle };
        let scanner = crate::scanner::AlwaysCleanScanner;

        let outcome = install(&src, "test/repo/advisory-skill", &scanner, &skills_root, true)
            .await
            .expect("install() MUST return Ok when server hash diverges (advisory posture, UAT gap 21.8-06)");

        // Advisory posture: install dir must survive the mismatch branch.
        assert!(
            outcome.install_path.exists(),
            "advisory branch MUST NOT cleanup the final install path on divergence"
        );

        // D-14: server hash round-trips verbatim.
        let lock = crate::lock::SkillLock::load_or_default().expect("lock load");
        let entry = lock.get("advisory-skill").expect("lock entry present");
        assert_eq!(
            entry.snapshot_hash, server_hash,
            "D-14 opaque contract: server snapshotHash MUST round-trip verbatim"
        );

        // D-13: computed_hash matches on-disk folder.
        let disk_hash = compute_folder_hash(&outcome.install_path)
            .expect("compute_folder_hash on install_path");
        assert_eq!(
            disk_hash, entry.computed_hash,
            "D-13: SkillLockEntry.computed_hash MUST equal compute_folder_hash(install_path)"
        );

        // The hashes must actually differ (test precondition — otherwise the test is vacuous).
        assert_ne!(
            disk_hash, server_hash,
            "test precondition: server hash MUST differ from D-13 folder hash"
        );

        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HERMES_HOME", v),
                None => std::env::remove_var("HERMES_HOME"),
            }
        }
    }

    /// update() must return Ok even when the fresh server snapshot_hash does not
    /// match the locally-computed D-13 folder hash. The advisory branch emits
    /// a tracing::warn but MUST NOT cleanup resolved_final or return ShaMismatch.
    #[tokio::test]
    async fn update_post_rename_compare_is_advisory_when_hashes_diverge() {
        let hermes_home = tempfile::tempdir().unwrap();
        let prev_home = std::env::var("HERMES_HOME").ok();
        unsafe {
            std::env::set_var("HERMES_HOME", hermes_home.path());
        }

        let skills_root = hermes_home.path().join("skills");
        std::fs::create_dir_all(&skills_root).unwrap();

        // Step 1 — prime: install with the D-13 folder hash as the server hash so
        // the initial lock entry is clean (no advisory trigger on first install).
        // We derive the expected hash after the fact by computing it from the bundle.
        let bundle_v1 = make_advisory_bundle(None); // None -> server_snapshot_hash stays empty
        let src_v1 = FixedBundleSource { bundle: bundle_v1 };
        let scanner = crate::scanner::AlwaysCleanScanner;

        install(
            &src_v1,
            "test/repo/advisory-skill",
            &scanner,
            &skills_root,
            true,
        )
        .await
        .expect("prime install");

        // Step 2 — remount with a divergent server hash.
        // Use a new bundle with slightly different content so drift is also detected.
        let server_hash_v2 = "divergent-update-server-hash-advisory-unit-test".to_string();
        let mut bundle_v2 = make_advisory_bundle(Some(server_hash_v2.clone()));
        // Mutate one file to ensure bundle_folder_hash differs from v1 -> drift detected.
        bundle_v2.files[1].bytes = b"print('updated advisory')\n".to_vec();
        let src_v2 = FixedBundleSource { bundle: bundle_v2 };

        // Step 3 — update with divergent server hash.
        let outcome = update(&src_v2, "advisory-skill", &scanner, &skills_root, true)
            .await
            .expect("update() MUST return Ok when server hash diverges (advisory posture, UAT gap 21.8-06)");

        // Advisory: resolved_final must survive.
        assert!(
            outcome.install_path.exists(),
            "advisory branch MUST NOT cleanup resolved_final on divergence"
        );

        // D-14: round-trip on update path.
        let lock = crate::lock::SkillLock::load_or_default().expect("lock load");
        let entry = lock.get("advisory-skill").expect("updated entry present");
        assert_eq!(
            entry.snapshot_hash, server_hash_v2,
            "update() MUST round-trip the new server snapshotHash verbatim even on divergence"
        );

        // D-13: refreshed computed_hash on update path.
        let disk_hash = compute_folder_hash(&outcome.install_path)
            .expect("compute_folder_hash on install_path");
        assert_eq!(
            disk_hash, entry.computed_hash,
            "update() MUST refresh entry.computed_hash to match compute_folder_hash(resolved_final)"
        );

        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HERMES_HOME", v),
                None => std::env::remove_var("HERMES_HOME"),
            }
        }
    }
}
