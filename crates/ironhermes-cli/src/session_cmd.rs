//! `hermes session` subcommand handlers (Phase 25.3 D-F-1 / D-F-2).
//!
//! Mirrors the pattern of `crates/ironhermes-cli/src/toolset_cmd.rs` (Phase 25):
//! a top-level Subcommand enum + a top-level handle function that dispatches to
//! per-subcommand impls. Each impl uses ironhermes_state primitives directly
//! (`StateStore::open_default` + `SessionDirectoryExport` from Plan 10).
//!
//! Two subcommands:
//!   - `Export { session_id, output }` — D-F-1 single-session export
//!   - `ExportAll { since }` — D-F-2 bulk export with optional date filter
//!
//! Both produce the canonical `~/.ironhermes/sessions/<id>/{messages.json,
//! metadata.json, context.json, trajectories.jsonl}` 4-file layout per session.

use anyhow::{Context as _, Result};
use clap::Subcommand;
use ironhermes_core::workspace;
use ironhermes_state::{SessionDirectoryExport, StateStore};
use std::path::PathBuf;

/// Subcommands for `hermes session`.
#[derive(Subcommand, Debug, Clone)]
pub enum SessionSubcommand {
    /// Export a single session to flat-file layout (D-F-1).
    Export {
        /// Session ID (full UUID or unique prefix).
        session_id: String,
        /// Output directory. Default: <hermes_home>/sessions/<session-id>/
        #[arg(long)]
        output: Option<String>,
    },
    /// Export all sessions to flat-file layout (D-F-2).
    ExportAll {
        /// Filter to sessions started on or after this date (YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
    },
}

/// Top-level dispatch for `hermes session <subcommand>`.
pub async fn handle_session_command(subcommand: SessionSubcommand) -> Result<()> {
    match subcommand {
        SessionSubcommand::Export { session_id, output } => {
            run_export_one(&session_id, output.as_deref()).await
        }
        SessionSubcommand::ExportAll { since } => run_export_all(since.as_deref()).await,
    }
}

/// Resolve the output directory for `<session-id>`, given an optional override.
///
/// Default: `<hermes_home>/sessions/<session-id>/` (where hermes_home respects
/// `--profile` pivoting via Phase 24).
fn resolve_output_dir(session_id: &str, output_override: Option<&str>) -> PathBuf {
    match output_override {
        Some(s) => PathBuf::from(s),
        None => ironhermes_core::constants::get_hermes_home()
            .join("sessions")
            .join(session_id),
    }
}

/// Resolve the trajectory source path for `<session-id>` (D-F-1: copy into the export).
///
/// Workspace-aware: matches the same path resolution Plan 8 uses when opening the writer.
/// Walks cwd to detect a workspace; falls back to global hermes_home.
fn resolve_trajectory_source(session_id: &str) -> PathBuf {
    let cwd = std::env::current_dir().ok();
    let traj_root = match cwd.as_ref().and_then(|c| workspace::resolve_from_cwd(c)) {
        Some(ws) => ws.root.join(".ironhermes"),
        None => ironhermes_core::constants::get_hermes_home(),
    };
    traj_root
        .join("sessions")
        .join(session_id)
        .join("trajectories.jsonl")
}

async fn run_export_one(session_id: &str, output_override: Option<&str>) -> Result<()> {
    // Open StateStore, fetch SessionExport, write 4-file layout.
    let store = StateStore::open_default()
        .with_context(|| "open default state store for session export")?;
    let export = store
        .export_session(session_id)
        .with_context(|| format!("fetch session {session_id} for export"))?;
    let output_dir = resolve_output_dir(session_id, output_override);
    let traj_src = resolve_trajectory_source(session_id);
    let exporter = SessionDirectoryExport::new(session_id, &output_dir);
    exporter.write(
        &export,
        None, // context_json: Phase 18 compressor output not threaded here yet
        Some(traj_src.as_path()), // trajectory_source: SessionDirectoryExport tolerates non-existent
    )?;
    eprintln!("Session {session_id} exported to {}", output_dir.display());
    Ok(())
}

async fn run_export_all(since: Option<&str>) -> Result<()> {
    let store =
        StateStore::open_default().with_context(|| "open default state store for export-all")?;

    // Resolve `since` (YYYY-MM-DD) into a unix timestamp.
    let since_unix: Option<f64> = match since {
        None => None,
        Some(s) => {
            let parsed = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .with_context(|| format!("--since must be YYYY-MM-DD; got '{s}'"))?
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| anyhow::anyhow!("invalid date components in {s}"))?
                .and_utc();
            Some(parsed.timestamp() as f64)
        }
    };

    // List ALL sessions; filter by since_unix in Rust (existing list_sessions
    // doesn't expose a started_at WHERE clause; the operator-on-demand
    // semantic of D-F-2 means we can iterate all and filter — bulk export
    // across thousands of rows is still tractable since each export is a
    // 4-file write).
    let sessions = store
        .list_sessions(None, usize::MAX)
        .with_context(|| "list sessions for export-all")?;

    let mut exported = 0usize;
    for session in sessions {
        if let Some(threshold) = since_unix {
            if session.started_at < threshold {
                continue;
            }
        }
        let output_dir = resolve_output_dir(&session.id, None);
        let traj_src = resolve_trajectory_source(&session.id);
        // Re-fetch full SessionExport for each session (messages too)
        let export = match store.export_session(&session.id) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("Warning: failed to export session {}: {e}", session.id);
                continue;
            }
        };
        let exporter = SessionDirectoryExport::new(&session.id, &output_dir);
        if let Err(e) = exporter.write(&export, None, Some(traj_src.as_path())) {
            eprintln!("Warning: failed to export session {}: {e}", session.id);
            continue;
        }
        exported += 1;
    }
    eprintln!("Exported {exported} session(s).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_parse_accepts_yyyy_mm_dd() {
        let res = chrono::NaiveDate::parse_from_str("2026-01-01", "%Y-%m-%d");
        assert!(res.is_ok());
    }

    #[test]
    fn since_parse_rejects_garbage() {
        let res = chrono::NaiveDate::parse_from_str("not-a-date", "%Y-%m-%d");
        assert!(res.is_err());
    }

    #[test]
    fn resolve_output_dir_uses_override_when_set() {
        let p = resolve_output_dir("sess-x", Some("/tmp/explicit"));
        assert_eq!(p, PathBuf::from("/tmp/explicit"));
    }

    #[test]
    fn resolve_output_dir_defaults_to_hermes_home_sessions() {
        let p = resolve_output_dir("sess-x", None);
        // Tail must be sessions/sess-x
        assert!(p.ends_with("sessions/sess-x"), "got: {p:?}");
    }
}
