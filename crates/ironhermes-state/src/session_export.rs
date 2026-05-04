//! Hybrid flat-file session export (Phase 25.3 D-F-1).
//!
//! SQLite remains the canonical store; this module produces the
//! `~/.ironhermes/sessions/<id>/{messages.json, metadata.json, context.json, trajectories.jsonl}`
//! 4-file layout for tooling / RL pipelines / Phase 25.4 Curator's offline consumers.
//!
//! Used by Plan 11's `/export-session` slash command and `hermes session export` CLI.
//! Bulk export (`hermes session export-all --since`) iterates session IDs and calls
//! this for each one.

use crate::SessionExport;
use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

/// 4-file directory writer for a single session export (D-F-1).
///
/// Construct with the session_id + output_dir, then call `write` with the data
/// fetched from StateStore + (optionally) the compressor output + the trajectory
/// source path.
pub struct SessionDirectoryExport {
    pub session_id: String,
    pub output_dir: PathBuf,
}

impl SessionDirectoryExport {
    pub fn new(session_id: impl Into<String>, output_dir: impl Into<PathBuf>) -> Self {
        Self {
            session_id: session_id.into(),
            output_dir: output_dir.into(),
        }
    }

    /// Write the 4-file export (D-F-1).
    ///
    /// `export` is the SQLite session+messages export from StateStore.
    /// `context_json` is the optional Phase 18 compressor output (None => "{}").
    /// `trajectory_source` is the path to the existing trajectories.jsonl from the
    /// trajectory crate's primary location (None or non-existent path => skip; the
    /// trajectories.jsonl in the export will be missing or empty, which is operator-tolerant).
    pub fn write(
        &self,
        export: &SessionExport,
        context_json: Option<&str>,
        trajectory_source: Option<&Path>,
    ) -> Result<()> {
        std::fs::create_dir_all(&self.output_dir)
            .with_context(|| format!("create export dir {}", self.output_dir.display()))?;

        // 1. messages.json — array of StoredMessage
        let messages_json =
            serde_json::to_string_pretty(&export.messages).context("serialize messages")?;
        std::fs::write(self.output_dir.join("messages.json"), messages_json)
            .with_context(|| format!("write messages.json to {}", self.output_dir.display()))?;

        // 2. metadata.json — Session struct (includes workspace_root from Plan 0)
        let metadata_json =
            serde_json::to_string_pretty(&export.session).context("serialize session metadata")?;
        std::fs::write(self.output_dir.join("metadata.json"), metadata_json)
            .with_context(|| format!("write metadata.json to {}", self.output_dir.display()))?;

        // 3. context.json — Phase 18 compressor output (empty object if None)
        let context_payload = context_json.unwrap_or("{}");
        std::fs::write(self.output_dir.join("context.json"), context_payload)
            .with_context(|| format!("write context.json to {}", self.output_dir.display()))?;

        // 4. trajectories.jsonl — copy from the trajectory crate's primary location.
        // Operator-tolerant: missing source => no trajectories.jsonl in the export
        // (matches D-T-4 "no automatic eviction" semantic; absence is informative).
        if let Some(src) = trajectory_source {
            if src.exists() {
                let dst = self.output_dir.join("trajectories.jsonl");
                std::fs::copy(src, &dst).with_context(|| {
                    format!(
                        "copy trajectories.jsonl from {} to {}",
                        src.display(),
                        dst.display()
                    )
                })?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, StoredMessage};
    use tempfile::tempdir;

    fn sample_export() -> SessionExport {
        SessionExport {
            session: Session {
                id: "sess-x".to_string(),
                source: "cli".to_string(),
                user_id: None,
                model: Some("test-model".to_string()),
                system_prompt: None,
                parent_session_id: None,
                started_at: 1.0,
                ended_at: None,
                end_reason: None,
                message_count: 1,
                tool_call_count: 0,
                input_tokens: 0,
                output_tokens: 0,
                title: Some("Test session".to_string()),
                workspace_root: Some("/tmp/myrepo".to_string()),
            },
            messages: vec![StoredMessage {
                id: 1,
                session_id: "sess-x".to_string(),
                role: "user".to_string(),
                content: Some("hi".to_string()),
                tool_call_id: None,
                tool_calls: None,
                tool_name: None,
                timestamp: 1.0,
                token_count: None,
                finish_reason: None,
            }],
        }
    }

    #[test]
    fn write_creates_4_files() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("sess-x");
        let exporter = SessionDirectoryExport::new("sess-x", &out);
        exporter
            .write(&sample_export(), Some(r#"{"compressed": false}"#), None)
            .unwrap();
        assert!(out.join("messages.json").exists());
        assert!(out.join("metadata.json").exists());
        assert!(out.join("context.json").exists());
        // trajectories.jsonl absent because trajectory_source = None
        assert!(!out.join("trajectories.jsonl").exists());
    }

    #[test]
    fn metadata_preserves_workspace_root() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("sess-x");
        let exporter = SessionDirectoryExport::new("sess-x", &out);
        exporter.write(&sample_export(), None, None).unwrap();
        let raw = std::fs::read_to_string(out.join("metadata.json")).unwrap();
        let parsed: Session = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.workspace_root.as_deref(), Some("/tmp/myrepo"));
        assert_eq!(parsed.id, "sess-x");
    }

    #[test]
    fn messages_roundtrip() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("sess-x");
        let exporter = SessionDirectoryExport::new("sess-x", &out);
        exporter.write(&sample_export(), None, None).unwrap();
        let raw = std::fs::read_to_string(out.join("messages.json")).unwrap();
        let parsed: Vec<StoredMessage> = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].content.as_deref(), Some("hi"));
    }

    #[test]
    fn context_json_defaults_to_empty_object_when_none() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("sess-x");
        let exporter = SessionDirectoryExport::new("sess-x", &out);
        exporter.write(&sample_export(), None, None).unwrap();
        let raw = std::fs::read_to_string(out.join("context.json")).unwrap();
        assert_eq!(raw, "{}");
    }

    #[test]
    fn trajectories_copied_when_source_exists() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src-trajectories.jsonl");
        std::fs::write(&src, b"{\"name\":\"x\"}\n").unwrap();
        let out = dir.path().join("sess-x");
        let exporter = SessionDirectoryExport::new("sess-x", &out);
        exporter.write(&sample_export(), None, Some(&src)).unwrap();
        let copied = std::fs::read_to_string(out.join("trajectories.jsonl")).unwrap();
        assert_eq!(copied, "{\"name\":\"x\"}\n");
    }

    #[test]
    fn trajectories_absent_when_source_missing() {
        let dir = tempdir().unwrap();
        let nonexistent = dir.path().join("does-not-exist.jsonl");
        let out = dir.path().join("sess-x");
        let exporter = SessionDirectoryExport::new("sess-x", &out);
        exporter
            .write(&sample_export(), None, Some(&nonexistent))
            .unwrap();
        // Operator-tolerant: missing source => no trajectories.jsonl in output
        assert!(!out.join("trajectories.jsonl").exists());
    }

    #[test]
    fn create_export_dir_if_missing() {
        let dir = tempdir().unwrap();
        // Multi-level path that does not exist
        let out = dir.path().join("a").join("b").join("c").join("sess-x");
        let exporter = SessionDirectoryExport::new("sess-x", &out);
        exporter.write(&sample_export(), None, None).unwrap();
        assert!(out.join("messages.json").exists());
    }
}
