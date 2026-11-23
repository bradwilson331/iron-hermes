//! `report_publish` tool + per-profile report store (Phase 54.10 D-09..D-12).
//!
//! A report is a generic, named, versioned data source (red-team first; audits
//! and evals later). Each publish writes ONE file,
//! `<home>/reports/<name>/<unix_ms>-<uuid>.json`, so concurrent producers (a
//! kanban worker, a chat turn) never read-modify-write a shared file.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::{AuditLog, ToolSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::registry::Tool;

/// On-disk format version carried by every version file.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeverityCounts {
    pub critical: u32,
    pub high: u32,
    pub medium: u32,
    pub low: u32,
    pub info: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportVersion {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub title: String,
    pub summary: String,
    pub counts: SeverityCounts,
    pub findings: Vec<Finding>,
    pub body: String,
    pub published_at_ms: i64,
}

/// `<home>/reports`.
pub fn reports_root(home: &Path) -> PathBuf {
    home.join("reports")
}

/// `^[a-z0-9][a-z0-9._-]{0,63}$` with no `..` — checked before any path join.
pub fn valid_report_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    first_ok
        && name.len() <= 64
        && !name.contains("..")
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// Versions kept per report; older ones are pruned after a successful write (D-11).
pub const REPORT_VERSIONS_MAX: usize = 20;
/// Report names per profile; a new name beyond this is refused.
pub const REPORTS_MAX: usize = 100;
pub const REPORT_BODY_MAX_BYTES: usize = 64 * 1024;
pub const REPORT_FINDINGS_MAX: usize = 200;
pub const REPORT_TITLE_MAX_CHARS: usize = 120;
pub const REPORT_SUMMARY_MAX_CHARS: usize = 1000;
pub const FINDING_TITLE_MAX_CHARS: usize = 200;
pub const REPORT_COUNT_MAX: u32 = 1_000_000;

fn validate_version(v: &ReportVersion) -> Result<(), String> {
    if !valid_report_name(&v.name) {
        return Err(format!("invalid report name '{}'", v.name));
    }
    let title_chars = v.title.chars().count();
    if title_chars == 0 || title_chars > REPORT_TITLE_MAX_CHARS {
        return Err(format!("title must be 1..={REPORT_TITLE_MAX_CHARS} chars"));
    }
    if v.summary.chars().count() > REPORT_SUMMARY_MAX_CHARS {
        return Err(format!("summary exceeds {REPORT_SUMMARY_MAX_CHARS} chars"));
    }
    if v.body.len() > REPORT_BODY_MAX_BYTES {
        return Err(format!("body exceeds {REPORT_BODY_MAX_BYTES} bytes"));
    }
    if v.findings.len() > REPORT_FINDINGS_MAX {
        return Err(format!("more than {REPORT_FINDINGS_MAX} findings"));
    }
    if v.findings
        .iter()
        .any(|f| f.title.chars().count() > FINDING_TITLE_MAX_CHARS)
    {
        return Err(format!(
            "finding title exceeds {FINDING_TITLE_MAX_CHARS} chars"
        ));
    }
    let c = &v.counts;
    if [c.critical, c.high, c.medium, c.low, c.info]
        .iter()
        .any(|n| *n > REPORT_COUNT_MAX)
    {
        return Err(format!("a severity count exceeds {REPORT_COUNT_MAX}"));
    }
    Ok(())
}

/// Version files in `dir` as `(published_ms_from_filename, path)`, oldest first.
fn version_files(dir: &Path) -> Vec<(i64, PathBuf)> {
    let mut files: Vec<(i64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let fname = e.file_name().to_string_lossy().into_owned();
            let stem = fname.strip_suffix(".json")?;
            // Parse the numeric prefix rather than trusting lexical order.
            let ms = stem.split('-').next()?.parse::<i64>().ok()?;
            Some((ms, e.path()))
        })
        .collect();
    files.sort();
    files
}

/// Valid report names (directories) under `root`, sorted.
pub fn list_report_names_at(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| valid_report_name(n))
        .collect();
    names.sort();
    names
}

/// Versions of `name`, newest first. Unreadable files and files whose `name`
/// differs from the directory are skipped.
pub fn list_versions_at(root: &Path, name: &str) -> Vec<ReportVersion> {
    if !valid_report_name(name) {
        return Vec::new();
    }
    let mut versions: Vec<ReportVersion> = version_files(&root.join(name))
        .into_iter()
        .filter_map(|(_, p)| serde_json::from_slice::<ReportVersion>(&std::fs::read(p).ok()?).ok())
        .filter(|v| v.name == name)
        .collect();
    versions.sort_by(|a, b| {
        b.published_at_ms
            .cmp(&a.published_at_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    versions
}

/// Write one version file under `root/<name>/`, prune beyond
/// [`REPORT_VERSIONS_MAX`], and return the stored version count.
pub fn publish_at(root: &Path, version: ReportVersion) -> Result<usize, String> {
    validate_version(&version)?;
    let dir = root.join(&version.name);
    if !dir.exists() && list_report_names_at(root).len() >= REPORTS_MAX {
        return Err(format!("report limit reached ({REPORTS_MAX} names)"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("create report dir: {e}"))?;
    let bytes = serde_json::to_vec_pretty(&version).map_err(|e| format!("serialize: {e}"))?;
    let tmp = dir.join(format!(".{}.tmp", version.id));
    std::fs::write(&tmp, bytes).map_err(|e| format!("write report: {e}"))?;
    let dest = dir.join(format!("{}-{}.json", version.published_at_ms, version.id));
    std::fs::rename(&tmp, &dest).map_err(|e| format!("commit report: {e}"))?;
    let mut files = version_files(&dir);
    while files.len() > REPORT_VERSIONS_MAX {
        let (_, oldest) = files.remove(0);
        let _ = std::fs::remove_file(oldest);
    }
    Ok(files.len())
}

/// Publishes a named structured report (D-09) and audits it (D-12).
pub struct ReportPublishTool {
    audit: Arc<AuditLog>,
    /// `None` = the producing process's home, resolved inside `execute`.
    root: Option<PathBuf>,
}

impl ReportPublishTool {
    pub fn new(audit: Arc<AuditLog>) -> Self {
        Self { audit, root: None }
    }

    #[cfg(test)]
    fn with_root(audit: Arc<AuditLog>, root: PathBuf) -> Self {
        Self {
            audit,
            root: Some(root),
        }
    }
}

fn derive_counts(findings: &[Finding]) -> SeverityCounts {
    let mut c = SeverityCounts::default();
    for f in findings {
        match f.severity {
            Severity::Critical => c.critical += 1,
            Severity::High => c.high += 1,
            Severity::Medium => c.medium += 1,
            Severity::Low => c.low += 1,
            Severity::Info => c.info += 1,
        }
    }
    c
}

fn str_arg(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

#[async_trait]
impl Tool for ReportPublishTool {
    fn name(&self) -> &str {
        "report_publish"
    }

    fn toolset(&self) -> &str {
        // Reuses the 46.6 `artifacts` group (RESEARCH A1): no new ALL_TOOLSETS entry.
        "artifacts"
    }

    fn description(&self) -> &str {
        "Publish or update a named structured report (for example red-team, audit \
         or eval results). Calling again with the same name adds a new version; \
         the Dashboard Reports tile shows the latest with a delta. Do not include \
         secrets."
    }

    fn schema(&self) -> ToolSchema {
        let sev = json!(["critical", "high", "medium", "low", "info"]);
        ToolSchema::new(
            "report_publish",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Report name: lowercase letters, digits, '.', '_' or '-' (1-64 chars, starts with a letter or digit)."
                    },
                    "title": { "type": "string", "description": "Human title (1-120 chars)." },
                    "summary": { "type": "string", "description": "Short summary (max 1000 chars)." },
                    "counts": {
                        "type": "object",
                        "description": "Severity counts. Derived from findings when omitted.",
                        "properties": {
                            "critical": { "type": "integer" },
                            "high": { "type": "integer" },
                            "medium": { "type": "integer" },
                            "low": { "type": "integer" },
                            "info": { "type": "integer" }
                        }
                    },
                    "findings": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "severity": { "type": "string", "enum": sev },
                                "title": { "type": "string" }
                            },
                            "required": ["severity", "title"]
                        }
                    },
                    "body": { "type": "string", "description": "Full report text (max 64 KiB)." }
                },
                "required": ["name", "title"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let name = str_arg(&args, "name")
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: name"))?;
        let title = str_arg(&args, "title")
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: title"))?;
        let findings: Vec<Finding> = match args.get("findings") {
            Some(v) if !v.is_null() => serde_json::from_value(v.clone())
                .map_err(|e| anyhow::anyhow!("Invalid findings: {e}"))?,
            _ => Vec::new(),
        };
        let counts = match args.get("counts") {
            Some(v) if !v.is_null() => serde_json::from_value(v.clone())
                .map_err(|e| anyhow::anyhow!("Invalid counts: {e}"))?,
            _ => derive_counts(&findings),
        };
        let version = ReportVersion {
            schema_version: REPORT_SCHEMA_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            name: name.clone(),
            title,
            summary: str_arg(&args, "summary").unwrap_or_default(),
            counts,
            findings,
            body: str_arg(&args, "body").unwrap_or_default(),
            published_at_ms: chrono::Utc::now().timestamp_millis(),
        };
        let id = version.id.clone();
        let root = self
            .root
            .clone()
            .unwrap_or_else(|| reports_root(&ironhermes_core::get_hermes_home()));
        let n = publish_at(&root, version)
            .map_err(|e| anyhow::anyhow!("report publish failed: {e}"))?;

        // D-12: audit after the write; a failed append never fails the publish.
        let (producer_key, surface) =
            crate::artifact::resolve_producer(std::env::var("IRONHERMES_KANBAN_TASK").ok(), None);
        let entry = self.audit.make_entry(
            "report_publish",
            name.clone(),
            None,
            producer_key.clone().unwrap_or_else(|| "chat".to_string()),
            surface,
            producer_key.unwrap_or_default(),
            "report publish",
            "approved",
            "auto",
            &args,
        );
        if let Err(e) = self.audit.append(&entry).await {
            tracing::warn!(
                target: "ironhermes_tools::report",
                report = %name,
                error = %e,
                "report published but audit append failed (publish NOT rolled back)"
            );
        }

        Ok(json!({ "name": name, "version": n, "id": id }).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::AuditConfig;

    fn make_tool(dir: &tempfile::TempDir) -> ReportPublishTool {
        let audit = Arc::new(AuditLog::with_path(
            dir.path().join("audit.jsonl"),
            AuditConfig::default(),
        ));
        ReportPublishTool::with_root(audit, dir.path().join("reports"))
    }

    fn read_audit_lines(dir: &tempfile::TempDir) -> Vec<serde_json::Value> {
        std::fs::read_to_string(dir.path().join("audit.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn report_publish_writes_a_version_and_an_audit_line() {
        let dir = tempfile::tempdir().unwrap();
        let tool = make_tool(&dir);
        let out = tool
            .execute(json!({
                "name": "redteam", "title": "Red team", "summary": "s",
                "findings": [{"severity": "high", "title": "x"}]
            }))
            .await
            .expect("publish must succeed");
        assert!(out.contains("\"version\":1"), "got: {out}");
        let files: Vec<_> = std::fs::read_dir(dir.path().join("reports/redteam"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
            .collect();
        assert_eq!(files.len(), 1);
        let entries = read_audit_lines(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["kind"], "report_publish");
    }

    fn ver(name: &str, ms: i64) -> ReportVersion {
        ReportVersion {
            schema_version: REPORT_SCHEMA_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            title: "t".to_string(),
            summary: String::new(),
            counts: SeverityCounts::default(),
            findings: Vec::new(),
            body: String::new(),
            published_at_ms: ms,
        }
    }

    fn json_files(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                    .count()
            })
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn report_publish_appends_a_version_on_second_call() {
        let dir = tempfile::tempdir().unwrap();
        let tool = make_tool(&dir);
        tool.execute(json!({"name": "redteam", "title": "a"}))
            .await
            .unwrap();
        // Distinct ms so newest-first ordering is deterministic.
        std::thread::sleep(std::time::Duration::from_millis(3));
        let out = tool
            .execute(json!({"name": "redteam", "title": "b"}))
            .await
            .unwrap();
        assert!(out.contains("\"version\":2"), "got: {out}");
        let root = dir.path().join("reports");
        assert_eq!(json_files(&root.join("redteam")), 2);
        let versions = list_versions_at(&root, "redteam");
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].title, "b");
        assert_eq!(versions[1].title, "a");
    }

    #[test]
    fn report_history_is_pruned_to_cap() {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=(REPORT_VERSIONS_MAX as i64 + 1) {
            publish_at(dir.path(), ver("redteam", i)).unwrap();
        }
        assert_eq!(json_files(&dir.path().join("redteam")), REPORT_VERSIONS_MAX);
        let versions = list_versions_at(dir.path(), "redteam");
        assert_eq!(versions.len(), REPORT_VERSIONS_MAX);
        assert!(
            versions.iter().all(|v| v.published_at_ms != 1),
            "oldest must be pruned"
        );
        assert_eq!(versions[0].published_at_ms, REPORT_VERSIONS_MAX as i64 + 1);
    }

    #[test]
    fn report_name_rejects_traversal_and_bad_charset() {
        let dir = tempfile::tempdir().unwrap();
        let long = "a".repeat(65);
        for bad in ["../x", "a/b", "A", "-x", "", "x..y", long.as_str()] {
            assert!(!valid_report_name(bad), "{bad:?} must be invalid");
            assert!(publish_at(dir.path(), ver(bad, 1)).is_err(), "{bad:?}");
        }
        assert!(valid_report_name("red-team_1.v2"));
        assert!(valid_report_name(&"a".repeat(64)));
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "no directory may be created for a refused name"
        );
    }

    #[test]
    fn report_caps_reject_oversize_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let mut cases: Vec<ReportVersion> = Vec::new();
        let mut v = ver("r", 1);
        v.body = "x".repeat(REPORT_BODY_MAX_BYTES + 1);
        cases.push(v);
        let mut v = ver("r", 1);
        v.findings = (0..=REPORT_FINDINGS_MAX)
            .map(|_| Finding {
                severity: Severity::Low,
                title: "f".into(),
            })
            .collect();
        cases.push(v);
        let mut v = ver("r", 1);
        v.title = "t".repeat(REPORT_TITLE_MAX_CHARS + 1);
        cases.push(v);
        let mut v = ver("r", 1);
        v.summary = "s".repeat(REPORT_SUMMARY_MAX_CHARS + 1);
        cases.push(v);
        let mut v = ver("r", 1);
        v.counts.high = REPORT_COUNT_MAX + 1;
        cases.push(v);
        let mut v = ver("r", 1);
        v.findings = vec![Finding {
            severity: Severity::Low,
            title: "f".repeat(FINDING_TITLE_MAX_CHARS + 1),
        }];
        cases.push(v);
        for (i, v) in cases.into_iter().enumerate() {
            assert!(
                publish_at(dir.path(), v).is_err(),
                "case {i} must be refused"
            );
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn report_counts_derive_from_findings_when_omitted() {
        let dir = tempfile::tempdir().unwrap();
        let tool = make_tool(&dir);
        tool.execute(json!({
            "name": "derived", "title": "t",
            "findings": [
                {"severity": "high", "title": "a"},
                {"severity": "high", "title": "b"},
                {"severity": "low", "title": "c"}
            ]
        }))
        .await
        .unwrap();
        tool.execute(json!({
            "name": "explicit", "title": "t", "counts": {"critical": 7},
            "findings": [{"severity": "high", "title": "a"}]
        }))
        .await
        .unwrap();
        let root = dir.path().join("reports");
        let c = &list_versions_at(&root, "derived")[0].counts;
        assert_eq!((c.high, c.low, c.critical), (2, 1, 0));
        let c = &list_versions_at(&root, "explicit")[0].counts;
        assert_eq!((c.critical, c.high), (7, 0));
    }

    #[test]
    fn reports_cap_refuses_a_new_name() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..REPORTS_MAX {
            std::fs::create_dir(dir.path().join(format!("r{i}"))).unwrap();
        }
        assert!(publish_at(dir.path(), ver("brand-new", 1)).is_err());
        assert!(!dir.path().join("brand-new").exists());
        assert!(publish_at(dir.path(), ver("r0", 1)).is_ok());
    }

    #[tokio::test]
    async fn report_audit_failure_does_not_fail_publish() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "not a dir").unwrap();
        let audit = Arc::new(AuditLog::with_path(
            blocker.join("audit.jsonl"),
            ironhermes_core::AuditConfig::default(),
        ));
        let root = dir.path().join("reports");
        let tool = ReportPublishTool::with_root(audit, root.clone());
        tool.execute(json!({"name": "redteam", "title": "t"}))
            .await
            .expect("an audit failure must not fail the publish");
        assert_eq!(json_files(&root.join("redteam")), 1);
    }

    #[test]
    fn list_report_names_and_versions_skip_garbage() {
        let dir = tempfile::tempdir().unwrap();
        publish_at(dir.path(), ver("beta", 1)).unwrap();
        publish_at(dir.path(), ver("alpha", 2)).unwrap();
        std::fs::create_dir(dir.path().join("Bad Name")).unwrap();
        std::fs::write(dir.path().join("stray.json"), "{}").unwrap();
        assert_eq!(list_report_names_at(dir.path()), vec!["alpha", "beta"]);

        let alpha = dir.path().join("alpha");
        std::fs::write(alpha.join("5-garbage.json"), "not json").unwrap();
        let other = serde_json::to_vec(&ver("beta", 9)).unwrap();
        std::fs::write(alpha.join("9-wrongname.json"), other).unwrap();
        let versions = list_versions_at(dir.path(), "alpha");
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].published_at_ms, 2);
        assert!(list_versions_at(dir.path(), "../alpha").is_empty());
    }

    #[test]
    fn report_tool_name_toolset_and_schema() {
        let dir = tempfile::tempdir().unwrap();
        let tool = make_tool(&dir);
        assert_eq!(tool.name(), "report_publish");
        assert_eq!(tool.toolset(), "artifacts");
        let schema = tool.schema();
        let required: Vec<&str> = schema.function.parameters["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"name") && required.contains(&"title"));
    }
}
