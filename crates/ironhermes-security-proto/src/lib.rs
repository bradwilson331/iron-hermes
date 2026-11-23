//! Wire types for the `ironhermes-tirith` sidecar.
//!
//! This crate is the arm's-length boundary between the MIT-licensed IronHermes
//! binaries and the AGPL-3.0-only `ironhermes-tirith` sidecar (Phase 57, D-01 /
//! D-02). The contract is deliberately "strings in, findings out": a request is
//! a subject string plus a kind, a response is a list of
//! `{rule, severity, title}` triples. No internal tirith type crosses it, which
//! is what keeps the two programs separate works that communicate over a pipe
//! rather than one combined work.
//!
//! Framing: one JSON object per line, UTF-8, `\n` terminated, over the child's
//! stdin (requests) and stdout (responses). Responses are keyed by the request
//! `id`.
//!
//! The dependency direction is one-way: the AGPL crate depends on this crate and
//! never the reverse. This crate depends on `serde` only so the sidecar binary
//! does not have to compile `ironhermes-core`.

use serde::{Deserialize, Serialize};

/// Protocol revision. Bumped on any incompatible change to [`Request`] or
/// [`Response`].
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest subject (`text`) the sidecar will analyse, in bytes. Anything larger
/// is answered with [`ERR_TOO_LARGE`] and must be treated by the caller as
/// "unavailable", never as "clean".
pub const MAX_SUBJECT_BYTES: usize = 1_048_576;

/// The request line was not valid JSON for a [`Request`].
pub const ERR_PROTOCOL: &str = "protocol_error";
/// The subject exceeded [`MAX_SUBJECT_BYTES`].
pub const ERR_TOO_LARGE: &str = "too_large";
/// A `file` subject named something the sidecar refuses to hand to tirith
/// (a path, or a PDF / archive / binary suffix).
pub const ERR_UNSUPPORTED_SUBJECT: &str = "unsupported_subject";
/// The analysis itself failed (including a caught panic).
pub const ERR_INTERNAL: &str = "internal";

/// Which operation a [`Request`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Analyse a subject; needs `kind` and `text` (and `name` for files).
    Check,
    /// Liveness probe.
    Ping,
    /// Report sidecar / tirith-core / protocol versions.
    Version,
}

/// What kind of text a `check` subject is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    /// A terminal command string (tool-call argument text).
    Exec,
    /// Inbound / prompt text (runs the byte scan: zero-width, bidi, ANSI).
    Paste,
    /// The text content of a named context file (`SOUL.md`, `mcp.json`, ...).
    File,
}

/// One request line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub op: Op,
    #[serde(default)]
    pub kind: Option<SubjectKind>,
    #[serde(default)]
    pub text: Option<String>,
    /// Bare basename for `kind = file`; tirith's config rules key on it.
    #[serde(default)]
    pub name: Option<String>,
}

/// tirith's five severities, raw. The sidecar never maps these to an action;
/// the caller applies its own severity x DEFCON table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum WireSeverity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

/// One finding: rule id, raw severity, short title. Never evidence text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireFinding {
    pub rule: String,
    pub sev: WireSeverity,
    pub title: String,
}

/// Answer to `op = version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarVersion {
    pub sidecar: String,
    pub tirith_core: String,
    pub protocol: u32,
    /// `"absent"` in this phase: the tirith threat database is not shipped.
    pub threat_db: String,
}

/// One response line, keyed by the request `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(default)]
    pub findings: Vec<WireFinding>,
    #[serde(default)]
    pub tier: Option<u8>,
    #[serde(default)]
    pub ms: Option<f64>,
    #[serde(default)]
    pub version: Option<SidecarVersion>,
    #[serde(default)]
    pub err: Option<String>,
}

impl Response {
    /// A bare success (ping).
    pub fn ok(id: u64) -> Self {
        Self {
            id,
            ok: true,
            findings: Vec::new(),
            tier: None,
            ms: None,
            version: None,
            err: None,
        }
    }

    /// A failure carrying one of the `ERR_*` codes.
    pub fn err(id: u64, code: &str) -> Self {
        Self {
            id,
            ok: false,
            findings: Vec::new(),
            tier: None,
            ms: None,
            version: None,
            err: Some(code.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_with_snake_case_enums() {
        let line = r#"{"id":7,"op":"check","kind":"exec","text":"echo hi"}"#;
        let req: Request = serde_json::from_str(line).unwrap();
        assert_eq!(req.id, 7);
        assert_eq!(req.op, Op::Check);
        assert_eq!(req.kind, Some(SubjectKind::Exec));
        assert_eq!(req.name, None);
    }

    #[test]
    fn severity_serialises_uppercase_and_orders() {
        assert_eq!(
            serde_json::to_string(&WireSeverity::High).unwrap(),
            "\"HIGH\""
        );
        assert!(WireSeverity::Critical > WireSeverity::High);
        assert!(WireSeverity::Info < WireSeverity::Low);
    }

    #[test]
    fn err_response_omits_nothing_the_client_needs() {
        let r = Response::err(3, ERR_TOO_LARGE);
        let back: Response = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
        assert!(!back.ok);
    }
}
