//! `/blueprint save`'s CLI-only write path (Phase 49.6 Plan 03, D-08/D-14).
//!
//! `BlueprintSaverImpl` implements `ironhermes_core::commands::context::BlueprintSaver`
//! — the trait declared in `ironhermes-core` for the same cycle-break reason
//! `CronJobWriter` exists: `ironhermes-core` cannot depend on
//! `ironhermes-cron` (which reads the job) or `ironhermes-hub` (which writes
//! the installed skill) without a dependency cycle.
//!
//! Lives in `ironhermes-cli`, NOT `ironhermes-cron` (where the read-side
//! sibling `CronJobWriterImpl` lives) — this impl ALSO needs
//! `ironhermes_hub::install`, and `ironhermes-cron` does not (and must not)
//! depend on `ironhermes-hub`. `ironhermes-cli` already depends on both.
//!
//! Opens `ironhermes_cron::JobStore::new()` — the process's own home, which
//! on the CLI IS the profile (D-06: one targeting mechanism, no `--profile`
//! flag here) — per call. No shared mutable state at the impl layer, so the
//! trait object is safe to clone into multiple contexts, mirroring
//! `CronJobWriterImpl`'s own "fresh store per call" precedent.

use ironhermes_core::commands::context::{BlueprintSaveRequest, BlueprintSaver};
use ironhermes_core::skills::{BlueprintMetadata, compose_blueprint_skill_md, sanitize_blueprint_name};
use ironhermes_cron::{CronJob, JobStore};

/// Maps a `CronJob`'s D-12 portable fields onto `BlueprintMetadata`. Reads
/// EXACTLY these seven fields and no others — `script`, `workdir`,
/// `base_url`, `skills`, `context_from`, and `continuity` all exist on the
/// same `CronJob` struct and must never be read here. Mirrors
/// `iron_hermes_ui::server::skills_import_api::blueprint_metadata_from_job`
/// field-for-field: that function is the UI save path's own copy of this
/// exact mapping, private to a `#[cfg(feature = "server")]`-gated module
/// this crate cannot reach, so this is a second, independently-maintained
/// copy of the SAME D-12 rule rather than a shared import.
fn blueprint_metadata_from_job(job: &CronJob) -> BlueprintMetadata {
    // `origin` resolves to the job's LIVE originating chat at delivery time —
    // a per-installation concept that means nothing once the job is exported
    // as a shareable artifact, so it is the one deliver value that must not
    // survive the round trip. Any other value is carried through verbatim
    // (RESEARCH.md Open Question 1, resolved: the sentinel is `"origin"`).
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

    BlueprintMetadata {
        schedule: job.schedule_display.clone(),
        deliver,
        prompt,
        no_agent: job.no_agent,
        model: job.model.clone(),
        provider: job.provider.clone(),
        enabled_toolsets,
    }
}

/// Install an already-composed blueprint `SKILL.md` through the real hub
/// installer pipeline — the CLI-side sibling of
/// `iron_hermes_ui::server::skills_import_api::install_composed_blueprint`,
/// modeled on it line for line. Never writes the file directly: bypassing
/// the installer would skip the security scan and leave no lock-file
/// provenance record (D-08).
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
        &ironhermes_hub::PastedSkillSource,
        skill_md,
        &ironhermes_hub::CoreSkillScanner,
        skills_root,
        true,
    )
    .await
    .map_err(|e| format!("{e}"))?;

    Ok(outcome.name)
}

/// Production `BlueprintSaver` impl for the CLI/TUI's `/blueprint save`.
/// Wired ONLY on the CLI/TUI context builder (`tui_rata/commands.rs`), never
/// on the gateway's — `cmd_blueprint_save`'s unconditional platform gate is
/// the control; this handle's absence everywhere else is the backstop.
pub struct BlueprintSaverImpl;

impl BlueprintSaverImpl {
    pub fn new() -> Self {
        Self
    }

    /// Testable core: takes an already-opened store and skills root so tests
    /// can drive it against a `tempfile::TempDir` without going through the
    /// `BlueprintSaver` trait wrapper (which has no async-friendly shape).
    fn save_in_store(
        store: &JobStore,
        req: &BlueprintSaveRequest,
        skills_root: &std::path::Path,
    ) -> Result<String, String> {
        let job = store
            .find_job(&req.job_id_or_name)
            .ok_or_else(|| format!("job not found: {}", req.job_id_or_name))?;

        let bp = blueprint_metadata_from_job(job);
        let raw_name = match &req.blueprint_name {
            Some(name) if !name.trim().is_empty() => name.as_str(),
            _ => job.name.as_str(),
        };
        // Sanitized HERE (not left to `compose_blueprint_skill_md`'s own
        // internal sanitization) because `install_composed_blueprint`'s
        // `to_skill_slug` guard has no `shared-blueprint` fallback — an
        // unsanitized name that sanitizes to something valid (e.g.
        // all-punctuation input) would otherwise be wrongly rejected before
        // the composer ever ran. Mirrors `save_job_as_blueprint_in_store`'s
        // identical precedent in the UI save path.
        let name = sanitize_blueprint_name(raw_name);
        let skill_md = compose_blueprint_skill_md(&name, &req.body, &bp);

        // The trait method is deliberately synchronous (mirrors
        // `CronJobWriter::create_job_from_blueprint`), but the installer is
        // async — bridge via the crate's LocalSet-safe async->sync bridge
        // rather than `block_in_place`, which panics on a current-thread
        // runtime (`ironhermes_core::async_bridge` doc comment).
        ironhermes_core::async_bridge::block_on_sync(install_composed_blueprint(
            &name,
            &skill_md,
            skills_root,
        ))
    }
}

impl Default for BlueprintSaverImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl BlueprintSaver for BlueprintSaverImpl {
    fn save_job_as_blueprint(&self, req: BlueprintSaveRequest) -> Result<String, String> {
        let store = JobStore::new().map_err(|e| format!("open cron store: {e}"))?;
        let skills_root = ironhermes_hub::paths::skills_root()
            .map_err(|e| format!("resolve skills root: {e}"))?;
        Self::save_in_store(&store, &req, &skills_root)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod blueprint_saver_impl_tests {
    use super::*;
    use ironhermes_cron::{NewJobSpec, ScheduleParsed};
    use tempfile::TempDir;

    fn make_store_with_one_job(store_dir: &std::path::Path) -> (JobStore, String) {
        let mut store = JobStore::open(store_dir.to_path_buf()).expect("open store");
        let schedule = ScheduleParsed::Interval {
            display: "every 30m".to_string(),
            minutes: 30,
        };
        let spec = NewJobSpec::new(
            "morning digest".to_string(),
            "Summarize overnight activity".to_string(),
            schedule,
            "every 30m".to_string(),
            "local".to_string(),
        );
        let job = store.add_job_spec(spec).expect("add job");
        (store, job.id)
    }

    #[test]
    fn saves_a_known_job_and_installs_one_skill_md_that_reparses_with_a_blueprint_block() {
        let cron_dir = TempDir::new().expect("cron tempdir");
        let skills_dir = TempDir::new().expect("skills tempdir");
        let (store, job_id) = make_store_with_one_job(cron_dir.path());

        let req = BlueprintSaveRequest {
            job_id_or_name: job_id,
            blueprint_name: Some("my-morning-digest".to_string()),
            body: "A shared morning digest automation.".to_string(),
        };

        let name = BlueprintSaverImpl::save_in_store(&store, &req, skills_dir.path())
            .expect("save must succeed");
        assert_eq!(name, "my-morning-digest");

        let skill_md_path = skills_dir
            .path()
            .join("general")
            .join("my-morning-digest")
            .join("SKILL.md");
        assert!(skill_md_path.is_file(), "SKILL.md must be installed on disk");

        let content = std::fs::read_to_string(&skill_md_path).expect("read installed SKILL.md");
        let (frontmatter, _body) =
            ironhermes_core::skills::parse_skill_md(&content).expect("must re-parse");
        let hermes_meta = ironhermes_core::skills::extract_hermes_metadata(&frontmatter.metadata)
            .expect("hermes metadata must be present");
        let bp = hermes_meta.blueprint.expect("blueprint block must be present");
        assert_eq!(bp.schedule, "every 30m");
    }

    #[test]
    fn unknown_job_id_errors_and_installs_nothing() {
        let cron_dir = TempDir::new().expect("cron tempdir");
        let skills_dir = TempDir::new().expect("skills tempdir");
        let store = JobStore::open(cron_dir.path().to_path_buf()).expect("open store");

        let req = BlueprintSaveRequest {
            job_id_or_name: "not-a-real-job".to_string(),
            blueprint_name: None,
            body: "irrelevant".to_string(),
        };

        let err = BlueprintSaverImpl::save_in_store(&store, &req, skills_dir.path())
            .expect_err("unknown job must error");
        assert!(
            err.contains("not-a-real-job"),
            "error must name the missing job: {err}"
        );

        let entries = std::fs::read_dir(skills_dir.path())
            .map(|d| d.count())
            .unwrap_or(0);
        assert_eq!(entries, 0, "nothing must be installed on error");
    }

    #[test]
    fn falls_back_to_the_jobs_own_name_when_no_blueprint_name_given() {
        let cron_dir = TempDir::new().expect("cron tempdir");
        let skills_dir = TempDir::new().expect("skills tempdir");
        let (store, job_id) = make_store_with_one_job(cron_dir.path());

        let req = BlueprintSaveRequest {
            job_id_or_name: job_id,
            blueprint_name: None,
            body: "A shared morning digest automation.".to_string(),
        };

        let name = BlueprintSaverImpl::save_in_store(&store, &req, skills_dir.path())
            .expect("save must succeed");
        assert_eq!(name, "morning-digest", "sanitized job name is the fallback");
    }
}
