//! Severity x DEFCON policy (Phase 57, D-06).
//!
//! The table is applied to the RAW finding severities the sidecar reports. tirith's
//! own `action` is never consulted (research Pitfall 4): the sidecar does not even
//! send it, and this module has no way to read it.

use ironhermes_core::config::DefconLevel;
use ironhermes_security_proto::{SubjectKind, WireSeverity};

use crate::client::UnavailableReason;

pub use ironhermes_core::content_screen::ScreenVerdict as Verdict;

/// tirith's five severities. `Ord`: `Info < Low < Medium < High < Critical`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl From<WireSeverity> for Severity {
    fn from(s: WireSeverity) -> Self {
        match s {
            WireSeverity::Info => Severity::Info,
            WireSeverity::Low => Severity::Low,
            WireSeverity::Medium => Severity::Medium,
            WireSeverity::High => Severity::High,
            WireSeverity::Critical => Severity::Critical,
        }
    }
}

impl From<Severity> for WireSeverity {
    fn from(s: Severity) -> Self {
        match s {
            Severity::Info => WireSeverity::Info,
            Severity::Low => WireSeverity::Low,
            Severity::Medium => WireSeverity::Medium,
            Severity::High => WireSeverity::High,
            Severity::Critical => WireSeverity::Critical,
        }
    }
}

impl Severity {
    /// Upper-case label used in outcomes and logs.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "INFO",
            Severity::Low => "LOW",
            Severity::Medium => "MEDIUM",
            Severity::High => "HIGH",
            Severity::Critical => "CRITICAL",
        }
    }
}

/// The 25-cell (DEFCON level, severity) -> verdict table (D-06).
///
/// Applied to the raw severities the sidecar reports, never to tirith's own
/// `action` (research Pitfall 4). One arm per cell, no arithmetic shortcut, so
/// every cell is visible and a regression in one is a one-line diff:
///
/// | level | Critical | High  | Medium | Low   | Info  |
/// |-------|----------|-------|--------|-------|-------|
/// | 5     | Block    | Warn  | Allow  | Allow | Allow |
/// | 4     | Block    | Warn  | Warn   | Allow | Allow |
/// | 3     | Block    | Block | Warn   | Allow | Allow |
/// | 2     | Block    | Block | Warn   | Warn  | Allow |
/// | 1     | Block    | Block | Block  | Warn  | Allow |
///
/// Every cell at level n is at least as strict as the same cell at level n+1.
/// `DefconLevel` is always 1..=5; the level-1 row is also the fail-closed arm.
pub fn table_verdict(level: DefconLevel, sev: Severity) -> Verdict {
    use Severity::{Critical, High, Info, Low, Medium};
    use Verdict::{Allow, Block, Warn};
    match (level.level(), sev) {
        (5, Critical) => Block,
        (5, High) => Warn,
        (5, Medium) => Allow,
        (5, Low) => Allow,
        (5, Info) => Allow,

        (4, Critical) => Block,
        (4, High) => Warn,
        (4, Medium) => Warn,
        (4, Low) => Allow,
        (4, Info) => Allow,

        (3, Critical) => Block,
        (3, High) => Block,
        (3, Medium) => Warn,
        (3, Low) => Allow,
        (3, Info) => Allow,

        (2, Critical) => Block,
        (2, High) => Block,
        (2, Medium) => Warn,
        (2, Low) => Warn,
        (2, Info) => Allow,

        // Level 1, and any out-of-range value (unreachable by construction):
        // the strictest row, so a bad level can only tighten.
        (_, Critical) => Block,
        (_, High) => Block,
        (_, Medium) => Block,
        (_, Low) => Warn,
        (_, Info) => Allow,
    }
}

/// Classes of ironhermes-side floor findings (D-24).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloorClass {
    None,
    FloorWarn,
    NeedsApproval,
    Critical,
}

/// Verdict for a floor class at a level (D-24).
///
/// - `FloorWarn` (regex Tier-1 and Python-table hits): Warn at levels 2-5, Block
///   only at level 1. Never a hard Block at level 3.
/// - `NeedsApproval`: Warn at every level, and never bypassable.
/// - `Critical` (Tier-2 floor class): Block at every level.
pub fn floor_verdict(level: DefconLevel, class: FloorClass) -> Verdict {
    match class {
        FloorClass::None => Verdict::Allow,
        FloorClass::FloorWarn => {
            if level.level() <= 1 {
                Verdict::Block
            } else {
                Verdict::Warn
            }
        }
        FloorClass::NeedsApproval => Verdict::Warn,
        FloorClass::Critical => Verdict::Block,
    }
}

/// Whether a yolo bypass may override this floor class. `NeedsApproval` and
/// `Critical` are never bypassable.
pub fn bypassable(class: FloorClass) -> bool {
    matches!(class, FloorClass::None | FloorClass::FloorWarn)
}

/// Verdict when the sidecar gave no answer for a subject (D-12, CR-01).
///
/// Two classes of reason (see [`UnavailableReason::is_outage`]): an outage means the
/// sidecar is down; every other reason means this subject was not screened, and the
/// text's author can cause that (a `%PDF-` header, an oversize body, command-dense
/// text, or a subject that hangs the analysis until the client kills the child for
/// repeated timeouts, `KilledForTimeouts`). Rules, first match wins:
///
/// | subject                                   | level 4-5 | level 1-3 |
/// |-------------------------------------------|-----------|-----------|
/// | `warn_class`, any reason                  | Warn      | Block     |
/// | outage (`Missing`, `Crash`), other        | Allow     | Allow     |
/// | context file, reason `Unsupported`        | Block     | Block     |
/// | not screened (any other reason, including a kill after repeated timeouts, which the author of a hanging subject can cause), other | Warn | Block |
///
/// An outage keeps the caller's regex floor (the sidecar side is Allow). A context
/// file the sidecar refuses to read as text is never loaded (D-04 c). The
/// unavailability itself is carried in the [`crate::service::SecurityVerdict`],
/// never reported as clean. Every row is at least as strict as the next looser level.
pub fn unavailable_verdict(
    level: DefconLevel,
    warn_class: bool,
    kind: SubjectKind,
    reason: UnavailableReason,
) -> Verdict {
    let relaxed = if level.level() >= 4 {
        Verdict::Warn
    } else {
        Verdict::Block
    };
    if warn_class {
        relaxed
    } else if reason.is_outage() {
        Verdict::Allow
    } else if kind == SubjectKind::File && reason == UnavailableReason::Unsupported {
        Verdict::Block
    } else {
        relaxed
    }
}

/// Strictest-wins merge (D-03): Allow < Warn < Block. A tirith Allow never lowers
/// a floor Warn or Block. Empty input is Allow.
pub fn combine(parts: &[Verdict]) -> Verdict {
    parts.iter().copied().max().unwrap_or(Verdict::Allow)
}

/// Tools whose verdict degrades to Warn/Block (not Allow) when the sidecar is
/// unavailable. One table for every surface.
///
/// `write_file` and `patch` are the registered file-write tools
/// (`crates/ironhermes-tools/src/file_tools.rs`). `patch_file` is not a registered
/// name today; it stays listed as an alias so a future rename cannot silently drop
/// the tool out of the Warn class.
pub const WARN_CLASS_TOOLS: &[&str] = &[
    "terminal",
    "execute_code",
    "delegate_task",
    "cronjob",
    "write_file",
    "patch",
    "patch_file",
];

/// Whether `name` is a Warn-class tool. `is_mcp_mutation` is supplied by the caller
/// (the MCP mutation-verb guardrail decides it from the prefixed tool name).
pub fn is_warn_class_tool(name: &str, is_mcp_mutation: bool) -> bool {
    is_mcp_mutation || WARN_CLASS_TOOLS.contains(&name)
}
