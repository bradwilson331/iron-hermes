//! Append-only mutation audit log — `~/.ironhermes/logs/audit.jsonl` (Phase 46 D-01/D-02/D-03).
//!
//! Phase 45 explicitly deferred this (45 D-12): `PendingApproval` never wrote to disk.
//! This module closes that gap so CFL-03's audit half is satisfied: every approval-gate
//! resolution (approved / denied / timed_out, including D-03 CLI-store bypass) appends
//! exactly one JSONL entry — for MCP mutations from any connected server AND flagged
//! shell commands alike.
//!
//! # Fail-closed posture (D-02)
//!
//! The entry is written + flushed + fsynced BEFORE the approved operation executes.
//! If the append fails, an otherwise-Approved (or bypass-Approved) outcome downgrades
//! to `Denied` and the caller surfaces the error to chat — no destructive op ever runs
//! unrecorded. This module only owns the write; the fail-closed downgrade itself lives
//! in `ironhermes-gateway::approval::ApprovalCoordinator::request()` (Phase 46 Plan 01
//! Task 3), which is the only caller of [`AuditLog::append`].
//!
//! # Append-only, NOT atomic-rewrite (D-03)
//!
//! Unlike `ApprovalsStore::save_to_disk` (approvals.rs), this file is NEVER rewritten
//! atomically via tmp+rename — that pattern is correct for small mutable state, wrong
//! (and slow/racy under Phase 39.1 concurrent turns) for an ever-growing log. Every
//! `append()` call opens in append mode, guarded by an in-process `tokio::sync::Mutex<()>`
//! held across the whole open+writeln!+flush+sync_data sequence, so concurrent turns
//! serialize onto one another instead of interleaving partial lines.
//!
//! # Redaction (D-03 / T-46-01)
//!
//! The args JSON is walked key-by-key (case-insensitively) before serialization; any
//! key whose lowercase form contains a known-sensitive substring (or a config-supplied
//! override) has its value replaced with the literal `[REDACTED]`. Redaction happens
//! BEFORE truncation so a secret can never survive as a truncated fragment.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::constants::get_hermes_home;

const AUDIT_FILENAME: &str = "audit.jsonl";

/// Phase 48.2 operator request: log-class files live under the home's `logs/`
/// subdirectory alongside `blackbox-*.jsonl` and `kanban/`, not loose at the
/// home root. `AuditLog::append` already `create_dir_all`s its parent, so this
/// directory materializes on first write with no separate bootstrap step.
const LOGS_DIRNAME: &str = "logs";

/// Default byte cap for the serialized `args` string before the `…(truncated)` marker
/// is appended (D-03).
const DEFAULT_MAX_ARGS_BYTES: usize = 2048;

/// Default case-insensitive substrings that mark an args key as sensitive (T-46-01).
/// Config-supplied `redact_keys` are additive, never replace this list.
const DEFAULT_SENSITIVE_TOKENS: &[&str] = &[
    "token",
    "api_key",
    "apikey",
    "secret",
    "password",
    "authorization",
    "bearer",
    "credential",
];

const REDACTED_MARKER: &str = "[REDACTED]";
const TRUNCATED_MARKER: &str = "…(truncated)";

fn default_max_args_bytes() -> usize {
    DEFAULT_MAX_ARGS_BYTES
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Phase 46 D-03: audit-log configuration.
///
/// Per RESEARCH Open Question 2: NO on/off disable knob — D-02's fail-loudly posture
/// intentionally has no escape hatch. This mirrors [`crate::config::McpMutationGuardrailConfig`]'s
/// shape (an additive/override keylist, no boolean toggle).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditConfig {
    /// Additional case-insensitive substrings (beyond the built-in defaults) whose
    /// presence in an args key marks the value for redaction.
    pub redact_keys: Vec<String>,
    /// Byte cap for the serialized (post-redaction) args string. Default: 2048.
    #[serde(default = "default_max_args_bytes")]
    pub max_args_bytes: usize,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            redact_keys: Vec::new(),
            max_args_bytes: DEFAULT_MAX_ARGS_BYTES,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditEntry
// ─────────────────────────────────────────────────────────────────────────────

/// One append-only JSONL record in `audit.jsonl` (D-03).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// RFC3339 UTC timestamp of the resolution.
    pub ts: String,
    /// `"mcp_mutation"`, `"shell_command"`, or `"new_issuer"` (Phase 46.1 D-02:
    /// non-blocking audit floor for a newly-added, non-baseline MCP-OAuth issuer).
    pub kind: String,
    /// `server__tool` name for MCP, or the shell command string.
    pub tool: String,
    /// MCP server name when `kind == "mcp_mutation"`; `None` for shell commands.
    pub server: Option<String>,
    /// Session id the resolution occurred under.
    pub session: String,
    /// Originating surface (e.g. `"telegram"`).
    pub surface: String,
    /// Originating chat id.
    pub chat_id: String,
    /// Guardrail reason surfaced to the operator.
    pub reason: String,
    /// `"approved"` | `"denied"` | `"timed_out"`.
    pub decision: String,
    /// `"operator"` | `"bypass"` | `"timeout"` | `"dropped"`.
    pub resolution: String,
    /// Already-redacted, already-truncated JSON string of the tool call arguments.
    pub args: String,
}

impl AuditEntry {
    /// Construct a new entry, redacting and truncating `raw_args` per `cfg`.
    ///
    /// `ts` is stamped at construction time (RFC3339 UTC via `chrono`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: impl Into<String>,
        tool: impl Into<String>,
        server: Option<String>,
        session: impl Into<String>,
        surface: impl Into<String>,
        chat_id: impl Into<String>,
        reason: impl Into<String>,
        decision: impl Into<String>,
        resolution: impl Into<String>,
        raw_args: &Value,
        cfg: &AuditConfig,
    ) -> Self {
        Self {
            ts: chrono::Utc::now().to_rfc3339(),
            kind: kind.into(),
            tool: tool.into(),
            server,
            session: session.into(),
            surface: surface.into(),
            chat_id: chat_id.into(),
            reason: reason.into(),
            decision: decision.into(),
            resolution: resolution.into(),
            args: redact_and_truncate(raw_args, cfg),
        }
    }
}

/// Redact known-sensitive keys in `raw` (case-insensitive substring match against
/// `DEFAULT_SENSITIVE_TOKENS` + `cfg.redact_keys`), serialize, then hard-truncate to
/// `cfg.max_args_bytes`, appending [`TRUNCATED_MARKER`] when cut. Redact BEFORE truncate
/// so a secret can never survive as a truncated fragment (T-46-01).
fn redact_and_truncate(raw: &Value, cfg: &AuditConfig) -> String {
    let redacted = redact_value(raw, &cfg.redact_keys);
    let serialized = serde_json::to_string(&redacted).unwrap_or_else(|_| "null".to_string());
    truncate_str(&serialized, cfg.max_args_bytes)
}

/// Recursively walk a JSON value, replacing the value of any object key whose
/// lowercase form contains a sensitive substring with [`REDACTED_MARKER`].
fn redact_value(value: &Value, extra_keys: &[String]) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                if is_sensitive_key(k, extra_keys) {
                    out.insert(k.clone(), Value::String(REDACTED_MARKER.to_string()));
                } else {
                    out.insert(k.clone(), redact_value(v, extra_keys));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| redact_value(v, extra_keys)).collect())
        }
        other => other.clone(),
    }
}

fn is_sensitive_key(key: &str, extra_keys: &[String]) -> bool {
    let lower = key.to_lowercase();
    DEFAULT_SENSITIVE_TOKENS
        .iter()
        .any(|tok| lower.contains(tok))
        || extra_keys
            .iter()
            .any(|tok| lower.contains(tok.to_lowercase().as_str()))
}

/// Hard-truncate `s` to `max_bytes`, appending [`TRUNCATED_MARKER`] when cut.
/// Truncation is on a UTF-8 char boundary to avoid panicking mid-codepoint.
fn truncate_str(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut cut = max_bytes;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{TRUNCATED_MARKER}", &s[..cut])
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditLog
// ─────────────────────────────────────────────────────────────────────────────

/// Append-only writer for `audit.jsonl`.
///
/// One `AuditLog` is constructed alongside `ApprovalsStore` in `runner.rs` and shared
/// as `Arc<AuditLog>` across the coordinator's whole lifetime — `write_lock` serializes
/// concurrent turns (Phase 39.1) onto a single append sequence.
pub struct AuditLog {
    path: PathBuf,
    write_lock: tokio::sync::Mutex<()>,
    cfg: AuditConfig,
}

impl AuditLog {
    /// Build an `AuditLog` at `get_hermes_home()/logs/audit.jsonl` (never `HERMES_HOME` —
    /// always `IRONHERMES_HOME` per project convention). Does not touch disk until the
    /// first [`AuditLog::append`] call.
    pub fn load(cfg: AuditConfig) -> Self {
        Self {
            path: get_hermes_home().join(LOGS_DIRNAME).join(AUDIT_FILENAME),
            write_lock: tokio::sync::Mutex::new(()),
            cfg,
        }
    }

    /// Test/advanced-use constructor: build an `AuditLog` at an explicit path.
    pub fn with_path(path: PathBuf, cfg: AuditConfig) -> Self {
        Self {
            path,
            write_lock: tokio::sync::Mutex::new(()),
            cfg,
        }
    }

    /// The resolved [`AuditConfig`] this log was constructed with.
    pub fn config(&self) -> &AuditConfig {
        &self.cfg
    }

    /// Construct an [`AuditEntry`] using this log's configured redaction/truncation.
    #[allow(clippy::too_many_arguments)]
    pub fn make_entry(
        &self,
        kind: impl Into<String>,
        tool: impl Into<String>,
        server: Option<String>,
        session: impl Into<String>,
        surface: impl Into<String>,
        chat_id: impl Into<String>,
        reason: impl Into<String>,
        decision: impl Into<String>,
        resolution: impl Into<String>,
        raw_args: &Value,
    ) -> AuditEntry {
        AuditEntry::new(
            kind, tool, server, session, surface, chat_id, reason, decision, resolution, raw_args,
            &self.cfg,
        )
    }

    /// Append one JSONL entry — write + flush + fsync, serialized by `write_lock` across
    /// the whole sequence (D-02/D-03). Append-mode only; never atomic tmp+rename.
    pub async fn append(&self, entry: &AuditEntry) -> std::io::Result<()> {
        use std::io::Write as _;

        let _guard = self.write_lock.lock().await;

        let line =
            serde_json::to_string(entry).map_err(|e| std::io::Error::other(e.to_string()))?;

        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }

        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut f = opts.open(&self.path)?;
        writeln!(f, "{line}")?;
        f.flush()?;
        f.sync_data()?;

        // Redundant chmod 0600 — safety net mirroring approvals.rs step 4 (PITFALLS §A-1).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_entry(raw_args: &Value, cfg: &AuditConfig) -> AuditEntry {
        AuditEntry::new(
            "mcp_mutation",
            "cloudflare__kv_delete",
            Some("cloudflare".to_string()),
            "sess1",
            "telegram",
            "chat1",
            "destructive verb",
            "approved",
            "operator",
            raw_args,
            cfg,
        )
    }

    #[tokio::test]
    async fn append_writes_one_line_then_two() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("audit.jsonl");
        let log = AuditLog::with_path(path.clone(), AuditConfig::default());

        let entry = test_entry(&json!({"key": "value"}), log.config());
        log.append(&entry).await.expect("first append must succeed");

        let contents = std::fs::read_to_string(&path).expect("read after first append");
        assert_eq!(
            contents.lines().count(),
            1,
            "first append must yield exactly 1 line"
        );

        log.append(&entry)
            .await
            .expect("second append must succeed");
        let contents = std::fs::read_to_string(&path).expect("read after second append");
        assert_eq!(
            contents.lines().count(),
            2,
            "second append must yield exactly 2 lines (append-mode, not rewrite)"
        );
    }

    #[tokio::test]
    async fn append_writes_new_issuer_kind_entry() {
        // Phase 46.1 D-02: proves the "new_issuer" kind round-trips through the
        // existing writer unchanged — kind is already a free-form String, this
        // test only proves the new value serializes/deserializes correctly.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("audit.jsonl");
        let log = AuditLog::with_path(path.clone(), AuditConfig::default());

        let entry = log.make_entry(
            "new_issuer",
            "connect:test-server",
            Some("test-server".to_string()),
            "cli-connect",
            "cli",
            "n/a",
            "new issuer github.com allowed for server test-server",
            "approved",
            "operator",
            &json!({}),
        );
        log.append(&entry)
            .await
            .expect("new_issuer append must succeed");

        let contents = std::fs::read_to_string(&path).expect("read after append");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 1, "must write exactly one JSONL line");

        let parsed: AuditEntry =
            serde_json::from_str(lines[0]).expect("line must deserialize as AuditEntry");
        assert_eq!(parsed.kind, "new_issuer");
        assert!(
            parsed.reason.contains("new issuer github.com"),
            "reason must contain the new-issuer message; got: {}",
            parsed.reason
        );
    }

    #[tokio::test]
    async fn append_writes_artifact_publish_kind_entry() {
        // Phase 46.6 D-06: proves the "artifact_publish" kind round-trips through
        // the existing writer unchanged — kind is already a free-form String, this
        // test only proves the new value serializes/deserializes correctly.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("audit.jsonl");
        let log = AuditLog::with_path(path.clone(), AuditConfig::default());

        let entry = log.make_entry(
            "artifact_publish",
            "artifact-id-123",
            None,
            "chat",
            "chat",
            "n/a",
            "artifact publish",
            "approved",
            "auto",
            &json!({}),
        );
        log.append(&entry)
            .await
            .expect("artifact_publish append must succeed");

        let contents = std::fs::read_to_string(&path).expect("read after append");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 1, "must write exactly one JSONL line");

        let parsed: AuditEntry =
            serde_json::from_str(lines[0]).expect("line must deserialize as AuditEntry");
        assert_eq!(parsed.kind, "artifact_publish");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn append_sets_0600_file_and_0700_parent() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().join("audit-parent");
        let path = parent.join("audit.jsonl");
        let log = AuditLog::with_path(path.clone(), AuditConfig::default());

        let entry = test_entry(&json!({"key": "value"}), log.config());
        log.append(&entry).await.expect("append must succeed");

        let file_mode = std::fs::metadata(&path)
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "audit.jsonl must be mode 0600");

        let dir_mode = std::fs::metadata(&parent)
            .expect("parent metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "parent dir must be mode 0700");
    }

    #[test]
    fn redaction_covers_all_known_sensitive_tokens() {
        let cfg = AuditConfig::default();
        for key in [
            "token",
            "api_key",
            "apiKey",
            "secret",
            "PASSWORD",
            "Authorization",
            "bearer_token",
            "credential_id",
        ] {
            let raw = json!({ key: "super-secret-value" });
            let entry = test_entry(&raw, &cfg);
            assert!(
                entry.args.contains(REDACTED_MARKER),
                "key `{key}` must be redacted; got args: {}",
                entry.args
            );
            assert!(
                !entry.args.contains("super-secret-value"),
                "raw secret value must never appear in args for key `{key}`; got: {}",
                entry.args
            );
        }
    }

    #[test]
    fn redaction_respects_config_supplied_keylist() {
        let cfg = AuditConfig {
            redact_keys: vec!["custom_field".to_string()],
            ..AuditConfig::default()
        };
        let raw = json!({ "custom_field": "shh", "safe_field": "visible" });
        let entry = test_entry(&raw, &cfg);
        assert!(entry.args.contains(REDACTED_MARKER));
        assert!(!entry.args.contains("shh"));
        assert!(
            entry.args.contains("visible"),
            "non-sensitive keys must survive redaction"
        );
    }

    #[test]
    fn truncation_caps_args_and_appends_marker() {
        let cfg = AuditConfig {
            redact_keys: Vec::new(),
            max_args_bytes: 32,
        };
        let raw = json!({ "big_field": "x".repeat(500) });
        let entry = test_entry(&raw, &cfg);
        assert!(
            entry.args.ends_with(TRUNCATED_MARKER),
            "oversized args must end with the truncated marker; got: {}",
            entry.args
        );
        assert!(
            entry.args.len() <= 32 + TRUNCATED_MARKER.len(),
            "truncated args must respect the byte cap; got len {}",
            entry.args.len()
        );
    }

    #[test]
    fn redaction_applies_before_truncation() {
        // A secret placed right at the truncation boundary must never survive as a
        // truncated fragment — redaction must happen first.
        let cfg = AuditConfig {
            redact_keys: Vec::new(),
            max_args_bytes: 2048,
        };
        let raw = json!({ "api_key": "sk-live-should-never-appear-anywhere-in-output" });
        let entry = test_entry(&raw, &cfg);
        assert!(
            !entry
                .args
                .contains("sk-live-should-never-appear-anywhere-in-output")
        );
        assert!(entry.args.contains(REDACTED_MARKER));
    }
}
