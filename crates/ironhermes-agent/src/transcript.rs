//! Fire-and-forget JSONL transcript writer (D-05).
//!
//! Pitfall 3 / E-08: the transcript writer path MUST NOT panic. Errors from
//! serialization, directory creation, file open, or write all resolve to a
//! tracing::warn and are otherwise swallowed. Writes must never stall the
//! agent turn, so every append runs under tokio::spawn.
//!
//! Path convention (D-05):
//!   $HERMES_HOME/subagent-transcripts/<session_id>/<subagent_id>.jsonl

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TranscriptLine {
    ToolCall {
        at: DateTime<Utc>,
        tool: String,
        args_preview: String,
    },
    ToolResult {
        at: DateTime<Utc>,
        tool: String,
        ok: bool,
        content_preview: String,
    },
    StreamDelta {
        at: DateTime<Utc>,
        delta: String,
    },
    Done {
        at: DateTime<Utc>,
        final_response_preview: String,
    },
    Cancelled {
        at: DateTime<Utc>,
        reason: String,
    },
}

impl TranscriptLine {
    pub fn now_tool_call(tool: impl Into<String>, args_preview: impl Into<String>) -> Self {
        Self::ToolCall {
            at: Utc::now(),
            tool: tool.into(),
            args_preview: args_preview.into(),
        }
    }

    pub fn now_tool_result(
        tool: impl Into<String>,
        ok: bool,
        content_preview: impl Into<String>,
    ) -> Self {
        Self::ToolResult {
            at: Utc::now(),
            tool: tool.into(),
            ok,
            content_preview: content_preview.into(),
        }
    }

    pub fn now_stream_delta(delta: impl Into<String>) -> Self {
        Self::StreamDelta {
            at: Utc::now(),
            delta: delta.into(),
        }
    }

    pub fn now_done(final_response_preview: impl Into<String>) -> Self {
        Self::Done {
            at: Utc::now(),
            final_response_preview: final_response_preview.into(),
        }
    }

    pub fn now_cancelled(reason: impl Into<String>) -> Self {
        Self::Cancelled {
            at: Utc::now(),
            reason: reason.into(),
        }
    }
}

/// Compose the per-session transcripts directory path (D-05).
/// `$HERMES_HOME/subagent-transcripts/<session_id>`
pub fn transcripts_dir_for_session(hermes_home: &Path, session_id: &str) -> PathBuf {
    hermes_home.join("subagent-transcripts").join(session_id)
}

/// Compose the full per-subagent transcript file path (D-05).
/// `$HERMES_HOME/subagent-transcripts/<session_id>/<subagent_id>.jsonl`
pub fn transcript_path_for(hermes_home: &Path, session_id: &str, subagent_id: &str) -> PathBuf {
    transcripts_dir_for_session(hermes_home, session_id).join(format!("{}.jsonl", subagent_id))
}

#[derive(Clone)]
pub struct TranscriptWriter {
    path: PathBuf,
}

impl TranscriptWriter {
    /// Prepare the transcript file path. Creates the parent directory
    /// best-effort — failure to create logs at `tracing::warn` and writes
    /// will subsequently fail silently (Pitfall 3 / E-08).
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    target: "ironhermes_agent::transcript",
                    path = ?parent,
                    error = ?e,
                    "failed to create transcripts dir; writes will fail silently"
                );
            }
        }
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Fire-and-forget append. Never panics, never bubbles. Serialization,
    /// open, and write errors all resolve to `tracing::warn` and are
    /// otherwise swallowed (Pitfall 3 / E-08).
    pub fn append(&self, line: TranscriptLine) {
        let path = self.path.clone();
        tokio::spawn(async move {
            let serialized = match serde_json::to_string(&line) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        target: "ironhermes_agent::transcript",
                        error = ?e,
                        "transcript serialization failed; dropping line"
                    );
                    return;
                }
            };
            let body = format!("{}\n", serialized);

            let open_result: std::io::Result<tokio::fs::File> = {
                let mut opts = tokio::fs::OpenOptions::new();
                opts.append(true).create(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    opts.mode(0o600);
                }
                opts.open(&path).await
            };

            let mut file = match open_result {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(
                        target: "ironhermes_agent::transcript",
                        path = ?path,
                        error = ?e,
                        "transcript open failed; dropping line"
                    );
                    return;
                }
            };

            use tokio::io::AsyncWriteExt;
            if let Err(e) = file.write_all(body.as_bytes()).await {
                tracing::warn!(
                    target: "ironhermes_agent::transcript",
                    path = ?path,
                    error = ?e,
                    "transcript write failed; dropping line"
                );
            }
        });
    }
}
