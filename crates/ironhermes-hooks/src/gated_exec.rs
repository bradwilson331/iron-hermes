//! `execute_gated_command()` — the surface-agnostic guardrail→approval→audit chokepoint.
//!
//! Phase 36.3.12 (D-08/D-09/D-10/D-11): generalizes
//! `ironhermes-gateway::shell_exec::shell_exec()` so CLI, TUI, and gateway can share ONE
//! decision path for BOTH the `terminal` tool and `execute_code`. Two deltas over the
//! gateway-only helper it generalizes (RESEARCH.md §Common Pitfalls):
//!
//! 1. **Audits every resolution** — including `Allow` (Pitfall 3: today's Allow path is
//!    never audited on any surface).
//! 2. **Forces approval on remote / credential-forwarding runs** — even when the command
//!    classifies as `Allow` (D-08).
//!
//! The executor is a caller-supplied `run` closure so this module never depends on the
//! tool-execution or backend-exec crates — no crate-dependency cycle is introduced.
//! Callers (CLI/TUI/gateway `terminal`/`execute_code` call sites, wired in Plan 07) supply
//! the actual subprocess/sandbox invocation.
//!
//! # Decision pipeline (Phase 57 — DEFCON-driven)
//!
//! The strictness dial is the session's DEFCON level, read live from the
//! [`GatePolicy`] at the moment of the decision (D-09, D-21). Three independent judges
//! each return a verdict (`Allow < Warn < Block`) and the strictest wins (D-03):
//!
//! ```text
//! 1. floor     guard.check(..)  ──► FloorClass ─► floor_verdict(level, class)    (D-24)
//!                Tier-2 → Critical            Block at every level
//!                Tier-1 / metachar / Python table → FloorWarn   Warn at 2-5, Block at 1
//! 2. rule      ironhermes_rule_class(..)  (works with no sidecar at all)
//!                D-23 mutation verb (terminal, or execute_code argv reconstruction)
//!                                  → Critical   Block at every level
//!                D-13 Python reach marker → Critical at DEFCON 3 and stricter,
//!                                  FloorWarn at 4-5
//! 3. sidecar   policy.security.screen(subject, level, warn_class)                 (D-06)
//!                subject = the command (terminal) or `python3 -c <quoted source>`
//!                (execute_code); absent service ⇒ this step is skipped
//!
//! final = combine([floor, rule, sidecar])
//!
//! final = Block ──► Blocked — `run` is NEVER invoked. This arm is matched FIRST and
//!                   no bypass code exists on its path (D-07, T-57-24).
//! final = Warn  ──► bypass iff level.yolo_bypass_active() (DEFCON 4-5, D-21) AND every
//!                   floor/rule class is bypassable (NeedsApproval never is)
//!                     ├─ bypass ─► Ran, audited "allowed"/"bypass", recorded in the
//!                     │            per-turn `yolo_ledger`
//!                     └─ else  ─► request_approval(): Ran / Denied, audited per outcome
//!                                 (no gate wired ⇒ Denied "no_gate")
//! final = Allow ──► audited "allowed"; UNLESS is_remote_backend || forward_env_nonempty
//!                   (D-08 forced approval), which goes through the same approval branch
//!                   at DEFCON 1-3 and is bypassed at 4-5.
//! ```
//!
//! A `security_verdict` audit row (rule ids and the level, never the command or source
//! text) is written when an ironhermes rule or the sidecar shaped the outcome. A call the
//! regex floor alone decided keeps today's single `shell_command` row, so existing audit
//! consumers are unaffected (T-57-28).
//!
//! # What the bypass is, and is not (D-21)
//!
//! Bypass and level are separate concepts: at DEFCON 1-3 every Warn reaches a real
//! approval, so an install with no DEFCON keys (resolved level 3) never bypasses. At 4-5
//! a Warn runs without a prompt — still audited (`resolution = "bypass"`), and **no longer
//! silent** (D-04, Phase 54 Finding 1; aggregated per turn by Phase 54.3 D-03):
//! immediately after the audit record and before `run`, the bypass is recorded into the
//! turn-scoped [`crate::yolo_ledger`]. `AgentRuntime::run_turn` flushes that ledger ONCE
//! at the end of the turn — one rollup audit row (`kind="yolo_bypass_rollup"`), then ONE
//! `approval_gate.notify_yolo_bypass_turn(summary)`. The per-call
//! `notify_yolo_bypass(session_id, tool_name, reason)` is the fail-visible fallback for a
//! bypass reached outside any turn ledger.
//!
//! **A Block is never bypassed**, at any level: Tier-2, a D-23 Critical rule, a D-13
//! reach block at DEFCON ≤ 3, and a sidecar verdict that maps to Block are all returned
//! before any bypass check exists in the call graph. Yolo can relax the *approval* tier;
//! it can never relax a *block* (D-07).
//!
//! # Legacy callers
//!
//! `policy: impl Into<GatePolicy>` accepts a `bool` through the D-08 alias (`true` ⇒ level
//! 5, `false` ⇒ level 3, no sidecar). Surfaces migrate to `GatePolicy::for_session(..)`
//! one at a time.

use std::future::Future;

use ironhermes_core::config::DefconLevel;
use ironhermes_core::content_screen::{
    REGEX_FLOOR_RULE_ID, SIDECAR_OUTAGE_RULE_PREFIX, UNSCREENED_RULE_PREFIX,
};
use ironhermes_core::{ApprovalGate, ApprovalOutcome, AuditLog, SecurityEvent, SecurityEventKind};
use ironhermes_security::policy::{
    FloorClass, Verdict, bypassable, combine, floor_verdict, is_warn_class_tool,
};
use ironhermes_security::service::{ScreenSubject, SecurityVerdict};
use ironhermes_security::{SubjectKind, UnavailableReason};

use crate::guardrail::{
    DangerousCommandGuardrail, GuardrailDecision, GuardrailHook, RULE_MUTATION_COMMAND,
    RULE_PYTHON_REACH, floor_class, ironhermes_rule_class,
};
use crate::policy_gate::GatePolicy;
use crate::refusal::{approval_refusal_text, no_approval_channel_text, policy_block_text};

/// Audit `kind` of the row written when an ironhermes rule or the sidecar contributed to
/// a verdict (Phase 57). Carries rule ids and the level, never subject text.
pub const SECURITY_VERDICT_KIND: &str = "security_verdict";

/// Audit `kind` of the row written for every sidecar-unavailable event (Phase 57 D-12):
/// `args = {reason, tool, level, verdict}`. The per-event row is the source of truth; the
/// per-turn `sidecar_unavailable_rollup` row only summarizes.
pub const SIDECAR_UNAVAILABLE_KIND: &str = "sidecar_unavailable";

/// The outcome of a gated command execution.
///
/// Mirrors `ironhermes-gateway::shell_exec::ShellExecOutcome`'s shape so callers
/// migrating off that gateway-only type have a familiar surface.
#[derive(Debug, Clone, PartialEq)]
pub enum GatedOutcome {
    /// The `run` closure executed successfully; inner string is its output.
    Ran(String),
    /// The command was denied — no approval gate wired, operator denied, timed out, or
    /// another approval already pending for this session. The text is built by
    /// [`crate::refusal`]: who refused, that the call did not run, and the
    /// no-circumvention notice (G-1).
    Denied(String),
    /// Hard block — `run` was NEVER invoked. Tier-2 (D-03), a D-23 Critical rule, a D-13
    /// reach block, or a sidecar verdict that maps to Block. This is the one outcome that
    /// no bypass can turn into `Ran` (D-07). The text is built by [`crate::refusal`]; the
    /// audit row keeps the raw reason.
    Blocked(String),
    /// `run` was invoked but returned an error (WR-03 semantics — distinguishable from a
    /// command that never executed at all).
    Failed(String),
}

impl std::fmt::Display for GatedOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ran(s) => write!(f, "Ran({s})"),
            Self::Denied(s) => write!(f, "Denied({s})"),
            Self::Blocked(s) => write!(f, "Blocked({s})"),
            Self::Failed(s) => write!(f, "Failed({s})"),
        }
    }
}

/// Surface-agnostic guardrail→approval→audit chokepoint (D-08/D-09/D-10/D-11, Phase 57).
///
/// # Parameters
///
/// - `tool_name`: `"terminal"` or `"execute_code"`.
/// - `classify_arg`: the shell command string for `"terminal"`; the Python source for
///   `"execute_code"` (Phase 54 D-01 — the guardrail's Python branch, the D-23 argv
///   reconstruction and the D-13 reach markers all read it).
/// - `guard`: the compiled two-tier dangerous-command guardrail.
/// - `approval_gate`: `None` fails closed on any approval-required resolution (D-12).
/// - `audit_log`: appended to on EVERY resolution branch, `Allow` included (Pitfall 3).
/// - `session_id` / `surface` / `chat_id`: forwarded into the audit entry and the
///   approval-gate prompt.
/// - `policy`: the DEFCON level and sidecar this call is judged under. A `bool` converts
///   through the D-08 alias (`true` ⇒ level 5, `false` ⇒ level 3, no sidecar); production
///   callers pass [`GatePolicy::for_session`]. The bypass applies only at levels 4-5 and
///   NEVER to a Block (D-07, D-21).
/// - `is_remote_backend`: true for a non-local execution backend (e.g. ssh) — forces
///   approval even on an `Allow` classification (D-08) at DEFCON 1-3.
/// - `forward_env_nonempty`: true when credentials/env vars are being forwarded into the
///   remote/child environment — forces approval even on an `Allow` classification (D-08).
/// - `run`: the caller-supplied executor closure. NEVER invoked on a `Blocked` outcome.
#[allow(clippy::too_many_arguments)]
pub async fn execute_gated_command<F, Fut>(
    tool_name: &str,
    classify_arg: &str,
    guard: &DangerousCommandGuardrail,
    approval_gate: Option<&dyn ApprovalGate>,
    audit_log: &AuditLog,
    session_id: &str,
    surface: &str,
    chat_id: &str,
    policy: impl Into<GatePolicy>,
    is_remote_backend: bool,
    forward_env_nonempty: bool,
    run: F,
) -> GatedOutcome
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<String>>,
{
    let raw_args = serde_json::json!({ "command": classify_arg });
    let decision = guard.check(tool_name, &raw_args);
    execute_with_floor(
        decision,
        tool_name,
        classify_arg,
        approval_gate,
        audit_log,
        session_id,
        surface,
        chat_id,
        policy.into(),
        is_remote_backend,
        forward_env_nonempty,
        run,
    )
    .await
}

/// The verdict pipeline behind [`execute_gated_command`], taking the regex floor's
/// decision as input. Split out so a floor result the shipped guardrail never produces
/// (`NeedsApproval`) can be driven in tests.
#[allow(clippy::too_many_arguments)]
async fn execute_with_floor<F, Fut>(
    decision: GuardrailDecision,
    tool_name: &str,
    classify_arg: &str,
    approval_gate: Option<&dyn ApprovalGate>,
    audit_log: &AuditLog,
    session_id: &str,
    surface: &str,
    chat_id: &str,
    policy: GatePolicy,
    is_remote_backend: bool,
    forward_env_nonempty: bool,
    run: F,
) -> GatedOutcome
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<String>>,
{
    let level = policy.level.get();
    let raw_args = serde_json::json!({ "command": classify_arg });

    // ── 1. The regex floor (D-24) ────────────────────────────────────────
    let floor_cls = floor_class(&decision);
    let floor_v = floor_verdict(level, floor_cls);
    let floor_reason: Option<&str> = match &decision {
        GuardrailDecision::Block { reason }
        | GuardrailDecision::Warn { reason }
        | GuardrailDecision::NeedsApproval { reason } => Some(reason.as_str()),
        GuardrailDecision::Allow => None,
    };

    // ── 2. The ironhermes-side rule (D-23 / D-13) — no sidecar needed ────
    let (rule_cls, rule_id) = ironhermes_rule_class(tool_name, classify_arg, level);
    let rule_v = floor_verdict(level, rule_cls);

    // ── 3. The sidecar (D-03/D-06) ───────────────────────────────────────
    let sidecar: Option<SecurityVerdict> = match policy.security.as_ref() {
        Some(service) => {
            let subject = ScreenSubject {
                kind: SubjectKind::Exec,
                text: sidecar_subject(tool_name, classify_arg),
                name: None,
            };
            Some(
                service
                    .screen(&subject, level, is_warn_class_tool(tool_name, false))
                    .await,
            )
        }
        None => None,
    };
    let sidecar_v = sidecar.as_ref().map_or(Verdict::Allow, |v| v.verdict);

    // ── 4. Strictest wins (D-03) ─────────────────────────────────────────
    let final_v = combine(&[floor_v, rule_v, sidecar_v]);

    // Which judges shaped the outcome, for the `security_verdict` row. The floor alone is
    // today's behaviour and writes no extra row.
    let rule_contributed = rule_cls == FloorClass::Critical || rule_v > floor_v;
    let sidecar_contributed = sidecar
        .as_ref()
        .is_some_and(|v| v.unavailable.is_none() && v.verdict != Verdict::Allow);
    let mut verdict_rules: Vec<&str> = Vec::new();
    if rule_contributed && let Some(id) = rule_id {
        verdict_rules.push(id);
    }
    if sidecar_contributed && let Some(v) = sidecar.as_ref() {
        verdict_rules.extend(v.rules.iter().map(String::as_str));
    }
    let contributed = rule_contributed || sidecar_contributed;

    // A Warn may run without a prompt only at a bypass-active level (DEFCON 4-5, D-21) and
    // only when every floor/rule class is bypassable: `NeedsApproval` never is.
    let bypass_ok = level.yolo_bypass_active() && bypassable(floor_cls) && bypassable(rule_cls);

    // ── The sidecar's state, reported (D-12, D-16) ───────────────────────
    // An outage is never silent: one `sidecar_unavailable` audit row per event, a record
    // in the turn ledger, and a critical event to the surface. The `outcome` word is
    // computed from the FINAL verdict, so the surface can say what actually happened to
    // the call (UI-SPEC C2). Plan 09's tool screen reuses `record_sidecar_unavailable` and
    // this outcome vocabulary so both surfaces emit the same summary.
    if let Some(sc) = sidecar.as_ref()
        && let Some(why) = sc.unavailable
    {
        let outcome = match final_v {
            Verdict::Block => "blocked",
            Verdict::Warn if bypass_ok => "bypass",
            Verdict::Warn => "approval",
            Verdict::Allow => "allowed",
        };
        record_sidecar_unavailable(
            audit_log,
            approval_gate,
            tool_name,
            session_id,
            surface,
            chat_id,
            level.level(),
            why,
            sc.verdict,
            outcome,
        )
        .await;
    }
    // A credential-shaped finding is reported whatever the verdict (D-16).
    if let Some(sc) = sidecar.as_ref() {
        let credential_rules: Vec<&str> = sc
            .rules
            .iter()
            .map(String::as_str)
            .filter(|r| r.to_ascii_lowercase().contains("credential"))
            .collect();
        if !credential_rules.is_empty() {
            notify_event(
                approval_gate,
                session_id,
                SecurityEventKind::CredentialDetected,
                tool_name,
                &credential_rules,
                level.level(),
                format!(
                    "credential detected in {tool_name} input at DEFCON {}",
                    level.level()
                ),
            )
            .await;
        }
    }

    let reason = if final_v == Verdict::Allow {
        String::new()
    } else {
        verdict_reason(
            final_v,
            level,
            (floor_v, floor_cls, floor_reason),
            (rule_v, rule_cls, rule_id),
            sidecar.as_ref(),
        )
    };

    match final_v {
        // ── Block (D-03/D-07/D-13/D-23) ──────────────────────────────────
        // Matched FIRST. Nothing on this path reads the level's bypass state, and there
        // is no code path from here that invokes `run`.
        Verdict::Block => {
            audit(
                audit_log, tool_name, session_id, surface, chat_id, &reason, "blocked", "dropped",
                &raw_args,
            )
            .await;
            if contributed {
                audit_verdict(
                    audit_log,
                    tool_name,
                    session_id,
                    surface,
                    chat_id,
                    &reason,
                    "blocked",
                    &verdict_rules,
                    level.level(),
                )
                .await;
            }
            // The event names the cause when no named rule decided (G-2). The audit rows
            // above keep `verdict_rules` exactly.
            let cause_ids = if verdict_rules.is_empty() {
                block_cause_ids(floor_v == Verdict::Block, sidecar.as_ref())
            } else {
                Vec::new()
            };
            let event_rules: Vec<&str> = if verdict_rules.is_empty() {
                cause_ids.iter().map(String::as_str).collect()
            } else {
                verdict_rules.clone()
            };
            notify_event(
                approval_gate,
                session_id,
                SecurityEventKind::Blocked,
                tool_name,
                &event_rules,
                level.level(),
                format!("blocked at DEFCON {}", level.level()),
            )
            .await;
            GatedOutcome::Blocked(policy_block_text(tool_name, &reason))
        }

        // ── Warn — approval-required, bypassable only at DEFCON 4-5 (D-21) ─
        // `NeedsApproval` (and any other non-bypassable class) never bypasses: an
        // unrecognized "needs approval" signal always fails closed.
        Verdict::Warn => {
            if contributed {
                audit_verdict(
                    audit_log,
                    tool_name,
                    session_id,
                    surface,
                    chat_id,
                    &reason,
                    "warn",
                    &verdict_rules,
                    level.level(),
                )
                .await;
            }
            if bypass_ok {
                run_bypassed(
                    tool_name,
                    &reason,
                    &raw_args,
                    approval_gate,
                    audit_log,
                    session_id,
                    surface,
                    chat_id,
                    (&verdict_rules, level.level()),
                    run,
                )
                .await
            } else {
                let outcome = handle_approval_required(
                    tool_name,
                    &reason,
                    &raw_args,
                    approval_gate,
                    audit_log,
                    session_id,
                    surface,
                    chat_id,
                    run,
                )
                .await;
                // The operator approved and the Warn ran (D-16: amber, not critical).
                if matches!(outcome, GatedOutcome::Ran(_) | GatedOutcome::Failed(_)) {
                    notify_event(
                        approval_gate,
                        session_id,
                        SecurityEventKind::WarnRan,
                        tool_name,
                        &verdict_rules,
                        level.level(),
                        format!("ran after approval at DEFCON {}", level.level()),
                    )
                    .await;
                }
                outcome
            }
        }

        // ── Allow — audited unconditionally (Pitfall 3); forced through the
        // approval branch when remote/cred-forwarding (D-08), unless the level is
        // bypass-active (4-5, D-21). ──
        Verdict::Allow => {
            let needs_forced_approval = is_remote_backend || forward_env_nonempty;
            if needs_forced_approval && !level.yolo_bypass_active() {
                let reason = forced_approval_reason(is_remote_backend, forward_env_nonempty);
                handle_approval_required(
                    tool_name,
                    &reason,
                    &raw_args,
                    approval_gate,
                    audit_log,
                    session_id,
                    surface,
                    chat_id,
                    run,
                )
                .await
            } else {
                let resolution = if needs_forced_approval {
                    "bypass"
                } else {
                    "allowed"
                };
                audit(
                    audit_log,
                    tool_name,
                    session_id,
                    surface,
                    chat_id,
                    "command classified Allow by the dangerous-command guardrail",
                    "allowed",
                    resolution,
                    &raw_args,
                )
                .await;
                // D-12 (Phase 54.3): this forced-approval-bypass site writes a
                // `resolution="bypass"` audit row, so the per-turn ledger records here
                // too — that is what makes the rollup's `count` equal
                // `count(resolution=="bypass")` for the turn. No per-call fallback notify
                // here: if no ledger is in scope the behaviour stays exactly as before
                // (audited, unannounced) — the ledger is the only path by which it
                // becomes visible.
                if resolution == "bypass" {
                    let _ = crate::yolo_ledger::try_record(
                        tool_name,
                        &forced_approval_reason(is_remote_backend, forward_env_nonempty),
                        session_id,
                        surface,
                        chat_id,
                    );
                }
                run_and_wrap(run).await
            }
        }
    }
}

/// The cause markers for a Block that no named rule decided (G-2, D-03, D-12).
///
/// These travel only in `SecurityEvent::rule_ids`, never in an audit row. The regex floor
/// has no rule ids of its own, so a floor Block is `regex_floor`. A sidecar Block that is
/// an unavailability is `sidecar_outage:<reason>` for a real outage and
/// `unscreened:<reason>` for a subject the sidecar refused. Only the reason's `Display`
/// word is used, never the client's free-form detail.
pub(crate) fn block_cause_ids(
    floor_blocked: bool,
    sidecar: Option<&SecurityVerdict>,
) -> Vec<String> {
    let mut ids = Vec::new();
    if floor_blocked {
        ids.push(REGEX_FLOOR_RULE_ID.to_string());
    }
    if let Some(sc) = sidecar
        && sc.verdict == Verdict::Block
        && let Some(why) = sc.unavailable
    {
        let prefix = if why.is_outage() {
            SIDECAR_OUTAGE_RULE_PREFIX
        } else {
            UNSCREENED_RULE_PREFIX
        };
        ids.push(format!("{prefix}{why}"));
    }
    ids
}

/// The text the sidecar screens (D-03): the command itself for `terminal`; for
/// `execute_code` the source wrapped as `python3 -c <shell-quoted source>`, so tirith's
/// exec rules see it as the interpreter invocation it effectively is.
pub(crate) fn sidecar_subject(tool_name: &str, classify_arg: &str) -> String {
    if tool_name == "execute_code" {
        format!("python3 -c {}", shell_quote(classify_arg))
    } else {
        classify_arg.to_string()
    }
}

/// POSIX single-quote `s`: `'` becomes `'\''`.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Human-readable reason for the final verdict: the reason of every judge that returned
/// that verdict. A floor-only call keeps the floor's own text unchanged.
pub(crate) fn verdict_reason(
    final_v: Verdict,
    level: DefconLevel,
    floor: (Verdict, FloorClass, Option<&str>),
    rule: (Verdict, FloorClass, Option<&'static str>),
    sidecar: Option<&SecurityVerdict>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if floor.0 == final_v
        && let Some(text) = floor.2
    {
        if floor.1 == FloorClass::FloorWarn && floor.0 == Verdict::Block {
            parts.push(format!(
                "{text} — blocked at DEFCON {} (floor-warn classes block only at level 1, D-24)",
                level.level()
            ));
        } else {
            parts.push(text.to_string());
        }
    }
    if rule.0 == final_v
        && let Some(id) = rule.2
    {
        parts.push(rule_reason(id, rule.1));
    }
    if let Some(sc) = sidecar
        && sc.verdict == final_v
        && sc.verdict != Verdict::Allow
    {
        parts.push(sidecar_reason(sc, level));
    }
    if parts.is_empty() {
        "security policy verdict".to_string()
    } else {
        parts.join("; ")
    }
}

/// Reason text for an ironhermes-side rule. Names the rule id; never the subject text.
pub(crate) fn rule_reason(id: &str, class: FloorClass) -> String {
    match (id, class) {
        (RULE_MUTATION_COMMAND, _) => format!(
            "ironhermes rule '{id}' classifies this call Critical — hard-blocked at every \
             DEFCON level, not approvable and not bypassable (D-23)"
        ),
        (RULE_PYTHON_REACH, FloorClass::Critical) => format!(
            "ironhermes rule '{id}': Python source reaches subprocess/socket/ctypes/importlib/\
             __import__ — hard-blocked at DEFCON 3 and stricter (D-13)"
        ),
        (RULE_PYTHON_REACH, _) => format!(
            "ironhermes rule '{id}': Python source reaches subprocess/socket/ctypes/importlib/\
             __import__ — approval required (D-13)"
        ),
        _ => format!("ironhermes rule '{id}'"),
    }
}

/// Reason text for a sidecar verdict: severity and rule ids, or the unavailability. An
/// outage (`missing`, `crash`) says the sidecar is unavailable; any other reason says the
/// call could not be screened.
pub(crate) fn sidecar_reason(sc: &SecurityVerdict, level: DefconLevel) -> String {
    let action = match sc.verdict {
        Verdict::Block => "blocked",
        _ => "approval required",
    };
    if let Some(why) = sc.unavailable {
        // The reason word only. The client's free-form detail can carry local paths and OS
        // error text, and this string is returned to the model as the tool result.
        if !why.is_outage() {
            // The sidecar refused this subject; it did not go down (GC-WR-03). Saying "the
            // sidecar is unavailable" here would send the operator and the model after a
            // fault that does not exist.
            return format!(
                "this call could not be screened ({why}) — {action} at DEFCON {} (D-12)",
                level.level()
            );
        }
        return format!(
            "security sidecar unavailable ({why}) — {action} at DEFCON {} (D-12)",
            level.level()
        );
    }
    let severity = sc.severity.map_or("unrated", |s| s.label());
    format!(
        "security sidecar reports {severity} (rules: {}) — {action} at DEFCON {} (D-06)",
        sc.rules.join(", "),
        level.level()
    )
}

/// D-08 forced-approval reason text — used both in the audit entry and the
/// operator-facing approval prompt.
fn forced_approval_reason(is_remote_backend: bool, forward_env_nonempty: bool) -> String {
    if is_remote_backend && forward_env_nonempty {
        "remote backend AND non-empty forward_env — approval required regardless of \
         guardrail classification (D-08)"
            .to_string()
    } else if is_remote_backend {
        "remote backend (e.g. ssh) — approval required regardless of guardrail \
         classification (D-08)"
            .to_string()
    } else {
        "non-empty forward_env (credential forwarding) — approval required regardless of \
         guardrail classification (D-08)"
            .to_string()
    }
}

/// A Warn run at a bypass-active level (DEFCON 4-5): audited, recorded, then run.
///
/// Only reached from the `Warn` arm, and only when the level is bypass-active and every
/// floor/rule class is bypassable. A Block never gets here.
#[allow(clippy::too_many_arguments)]
async fn run_bypassed<F, Fut>(
    tool_name: &str,
    reason: &str,
    raw_args: &serde_json::Value,
    approval_gate: Option<&dyn ApprovalGate>,
    audit_log: &AuditLog,
    session_id: &str,
    surface: &str,
    chat_id: &str,
    event_context: (&[&str], u8),
    run: F,
) -> GatedOutcome
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<String>>,
{
    audit(
        audit_log, tool_name, session_id, surface, chat_id, reason, "allowed", "bypass", raw_args,
    )
    .await;
    // D-04 (Phase 54 Finding 1) + D-03 (Phase 54.3): the audit record above is silent to
    // the operator, so the bypass must still be announced. Phase 54.3 aggregates the
    // announcement PER TURN: record into the turn-scoped ledger (a synchronous push — the
    // gated command never awaits a notifier on this path); `AgentRuntime::run_turn`
    // flushes it once at the end of the turn, after writing the rollup audit row.
    //
    // Fail-visible fallback: when no ledger is in scope (a bypass reached outside
    // `run_turn`) fall back to today's per-call notice — noisy but never silent. Still
    // AFTER audit and BEFORE running, so a notifier that panics or hangs can never cost
    // the audit record. Best-effort: absent gate is a no-op, not a panic.
    let recorded = crate::yolo_ledger::try_record(tool_name, reason, session_id, surface, chat_id);
    if !recorded && let Some(gate) = approval_gate {
        gate.notify_yolo_bypass(session_id, tool_name, reason).await;
    }
    // D-16: the typed event (amber, not critical). After the audit row, before `run`.
    let (rule_ids, level) = event_context;
    notify_event(
        approval_gate,
        session_id,
        SecurityEventKind::BypassRan,
        tool_name,
        rule_ids,
        level,
        format!("ran without approval at DEFCON {level}"),
    )
    .await;
    run_and_wrap(run).await
}

/// How long a surface may take to accept one security event before the gated call moves
/// on. The audit row is already written by then; this only bounds a hung notifier.
const SECURITY_EVENT_NOTIFY_TIMEOUT_SECS: u64 = 2;

/// Report one security event to the surface's gate, if there is one (D-16).
///
/// Best-effort and bounded: it returns `()`, is never consulted to decide whether anything
/// runs, and a notifier that hangs costs at most [`SECURITY_EVENT_NOTIFY_TIMEOUT_SECS`].
/// `summary` must never carry command text, arguments or secret values.
pub(crate) async fn notify_event(
    gate: Option<&dyn ApprovalGate>,
    session_id: &str,
    kind: SecurityEventKind,
    tool_name: &str,
    rule_ids: &[&str],
    level: u8,
    summary: String,
) {
    let Some(gate) = gate else {
        return;
    };
    let event = SecurityEvent {
        kind,
        tool: tool_name.to_string(),
        rule_ids: rule_ids.iter().map(|r| (*r).to_string()).collect(),
        level,
        summary,
    };
    let bounded = tokio::time::timeout(
        std::time::Duration::from_secs(SECURITY_EVENT_NOTIFY_TIMEOUT_SECS),
        gate.notify_security_event(session_id, &event),
    )
    .await;
    if bounded.is_err() {
        tracing::warn!(
            target: "ironhermes::security::gated_exec",
            tool = %tool_name,
            kind = ?event.kind,
            timeout_secs = SECURITY_EVENT_NOTIFY_TIMEOUT_SECS,
            "security event notifier did not complete in time; the call continues"
        );
    }
}

/// Record one sidecar unavailability (D-12): the `sidecar_unavailable` audit row, the turn
/// ledger entry, and a critical `SidecarUnavailable` event to the surface.
///
/// The wording splits on [`UnavailableReason::is_outage`] (GC-WR-03). The audit row's text
/// says "security sidecar unavailable" for a real outage (`missing`, `crash`) and "subject
/// could not be screened" for every other reason, where the sidecar answered but refused
/// the subject. The row's args are the same for both. The event is kept for every reason
/// because D-12 requires a critical notice for each unavailability; its row copy in the
/// web UI makes the same split, so a refused subject no longer reads as "scanner down".
///
/// `sidecar_verdict` is the verdict the outage mapped to for this subject (`warn` for a
/// Warn-class tool at DEFCON 4-5, `block` at 1-3, `allow` for others). `outcome` is what
/// happened to the call after the merge: `blocked`, `approval`, `bypass` or `allowed`.
/// The event summary is exactly `<reason>; outcome <outcome>`, which plan 14's web row
/// states (UI-SPEC C2, level-aware copy); the tool screen of plan 09 reuses this function
/// so both emit the same text. The summary never carries the subject.
#[allow(clippy::too_many_arguments)]
pub async fn record_sidecar_unavailable(
    audit_log: &AuditLog,
    gate: Option<&dyn ApprovalGate>,
    tool_name: &str,
    session_id: &str,
    surface: &str,
    chat_id: &str,
    level: u8,
    reason: UnavailableReason,
    sidecar_verdict: Verdict,
    outcome: &str,
) {
    let verdict_word = match sidecar_verdict {
        Verdict::Allow => "allow",
        Verdict::Warn => "warn",
        Verdict::Block => "block",
    };
    let why_is_outage = reason.is_outage();
    let reason = reason.to_string();
    let reason = reason.as_str();
    let text = if why_is_outage {
        format!("security sidecar unavailable ({reason}); {verdict_word} at DEFCON {level}")
    } else {
        format!("subject could not be screened ({reason}); {verdict_word} at DEFCON {level}")
    };
    let args = serde_json::json!({
        "reason": reason,
        "tool": tool_name,
        "level": level,
        "verdict": verdict_word,
    });
    let entry = audit_log.make_entry(
        SIDECAR_UNAVAILABLE_KIND,
        tool_name,
        None,
        session_id,
        surface,
        chat_id,
        text,
        verdict_word,
        "sidecar_unavailable",
        &args,
    );
    if let Err(e) = audit_log.append(&entry).await {
        tracing::warn!(
            target: "ironhermes::security::gated_exec",
            error = %e,
            tool = %tool_name,
            "failed to append sidecar_unavailable audit entry"
        );
    }
    // The ledger is a task-local: outside a turn it records nothing and the row above is
    // the only trail (research Pitfall 10).
    let _ = crate::yolo_ledger::try_record_unavailable(
        tool_name,
        reason,
        level,
        verdict_word,
        session_id,
        surface,
        chat_id,
    );
    notify_event(
        gate,
        session_id,
        SecurityEventKind::SidecarUnavailable,
        tool_name,
        &[],
        level,
        format!("{reason}; outcome {outcome}"),
    )
    .await;
}

/// Shared approval-required handling for `Warn`, a non-bypassable floor result, and the
/// forced-approval-on-remote/`Allow` path. Every outcome is audited before returning.
/// There is deliberately no bypass parameter: a bypass is decided by the caller and takes
/// [`run_bypassed`] instead.
#[allow(clippy::too_many_arguments)]
async fn handle_approval_required<F, Fut>(
    tool_name: &str,
    reason: &str,
    raw_args: &serde_json::Value,
    approval_gate: Option<&dyn ApprovalGate>,
    audit_log: &AuditLog,
    session_id: &str,
    surface: &str,
    chat_id: &str,
    run: F,
) -> GatedOutcome
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<String>>,
{
    let Some(gate) = approval_gate else {
        // D-12: no gate wired — fail-closed.
        audit(
            audit_log, tool_name, session_id, surface, chat_id, reason, "denied", "no_gate",
            raw_args,
        )
        .await;
        return GatedOutcome::Denied(no_approval_channel_text(tool_name, reason));
    };

    let outcome = gate
        .request_approval(session_id, tool_name, reason, raw_args)
        .await;

    let (decision, resolution) = match outcome {
        ApprovalOutcome::Approved => ("approved", "operator"),
        ApprovalOutcome::Denied => ("denied", "operator"),
        ApprovalOutcome::TimedOut => ("denied", "timeout"),
        ApprovalOutcome::AlreadyPending => ("denied", "already_pending"),
        ApprovalOutcome::GateUnavailable => ("denied", "gate_unavailable"),
    };
    audit(
        audit_log, tool_name, session_id, surface, chat_id, reason, decision, resolution, raw_args,
    )
    .await;

    match outcome {
        ApprovalOutcome::Approved => run_and_wrap(run).await,
        // Approved is matched above, so the fallback is unreachable; it avoids a panic path.
        other => GatedOutcome::Denied(
            approval_refusal_text(tool_name, &other)
                .unwrap_or_else(|| no_approval_channel_text(tool_name, reason)),
        ),
    }
}

/// Append one audit entry, unconditionally (Pitfall 3). A write failure is logged but
/// does not change the caller's decision outcome — this module owns only the write;
/// a fail-closed *downgrade* on audit-append failure (mirroring
/// `ApprovalCoordinator::request()` in the gateway) is a caller-level concern for
/// surfaces that need that stronger guarantee.
#[allow(clippy::too_many_arguments)]
async fn audit(
    audit_log: &AuditLog,
    tool_name: &str,
    session_id: &str,
    surface: &str,
    chat_id: &str,
    reason: &str,
    decision: &str,
    resolution: &str,
    raw_args: &serde_json::Value,
) {
    let entry = audit_log.make_entry(
        "shell_command",
        tool_name,
        None,
        session_id,
        surface,
        chat_id,
        reason,
        decision,
        resolution,
        raw_args,
    );
    if let Err(e) = audit_log.append(&entry).await {
        tracing::warn!(
            target: "ironhermes::security::gated_exec",
            error = %e,
            tool = %tool_name,
            "failed to append audit entry for gated command resolution"
        );
    }
}

/// Append one `security_verdict` row: rule ids and the level only, never the command or
/// source text (T-57-27).
#[allow(clippy::too_many_arguments)]
async fn audit_verdict(
    audit_log: &AuditLog,
    tool_name: &str,
    session_id: &str,
    surface: &str,
    chat_id: &str,
    reason: &str,
    decision: &str,
    rules: &[&str],
    level: u8,
) {
    let args = serde_json::json!({ "rule": rules.join(","), "rules": rules, "level": level });
    let entry = audit_log.make_entry(
        SECURITY_VERDICT_KIND,
        tool_name,
        None,
        session_id,
        surface,
        chat_id,
        reason,
        decision,
        "verdict",
        &args,
    );
    if let Err(e) = audit_log.append(&entry).await {
        tracing::warn!(
            target: "ironhermes::security::gated_exec",
            error = %e,
            tool = %tool_name,
            "failed to append security_verdict audit entry"
        );
    }
}

async fn run_and_wrap<F, Fut>(run: F) -> GatedOutcome
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<String>>,
{
    match run().await {
        Ok(output) => GatedOutcome::Ran(output),
        // WR-03 semantics: an execution error is distinguishable from a command that
        // never ran (Denied/Blocked).
        Err(e) => GatedOutcome::Failed(format!("execution error: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Tests — exhaustive branch coverage + audit-floor assertions (Task 2, D-08/D-10/D-11)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::AuditConfig;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// One captured per-turn delivery: `(session_id, summary.count(), summary.by_tool())`.
    type TurnNotice = (String, u32, Vec<(String, u32)>);

    /// Fake `ApprovalGate` returning a preset outcome; records whether
    /// `request_approval` was invoked so tests can assert the gate was (or was not)
    /// consulted.
    ///
    /// Also captures `notify_yolo_bypass` calls (Task 2, D-04): each call's
    /// `(session_id, tool_name, reason)` is appended to `notifications` under a
    /// `Mutex`, so tests can assert on the captured message CONTENT — not merely a
    /// boolean flag, which would stay green if the call fired with an empty or wrong
    /// reason.
    ///
    /// Phase 54.3 D-03: ALSO captures per-turn `notify_yolo_bypass_turn` deliveries
    /// (`turn_notifications`). BOTH overrides are required — a fake that overrode only
    /// the per-turn method would make the legacy per-call assertions vacuous, and one
    /// that overrode only the per-call method would make the new scoped assertions
    /// vacuous.
    struct FakeGate {
        outcome: ApprovalOutcome,
        called: AtomicBool,
        notifications: std::sync::Mutex<Vec<(String, String, String)>>,
        /// `(session_id, summary.count(), summary.by_tool())` per per-turn delivery.
        turn_notifications: std::sync::Mutex<Vec<TurnNotice>>,
    }

    impl FakeGate {
        fn new(outcome: ApprovalOutcome) -> Self {
            Self {
                outcome,
                called: AtomicBool::new(false),
                notifications: std::sync::Mutex::new(Vec::new()),
                turn_notifications: std::sync::Mutex::new(Vec::new()),
            }
        }

        /// Snapshot of every per-call `notify_yolo_bypass` call captured so far (the
        /// unscoped-fallback delivery path).
        fn notifications(&self) -> Vec<(String, String, String)> {
            self.notifications.lock().unwrap().clone()
        }

        /// Snapshot of every per-turn `notify_yolo_bypass_turn` delivery captured so
        /// far (the scoped, aggregated delivery path).
        fn turn_notifications(&self) -> Vec<TurnNotice> {
            self.turn_notifications.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ApprovalGate for FakeGate {
        async fn request_approval(
            &self,
            _session_id: &str,
            _tool_name: &str,
            _reason: &str,
            _args: &serde_json::Value,
        ) -> ApprovalOutcome {
            self.called.store(true, Ordering::SeqCst);
            self.outcome
        }

        async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
            false
        }

        async fn notify_yolo_bypass(&self, session_id: &str, tool_name: &str, reason: &str) {
            self.notifications.lock().unwrap().push((
                session_id.to_string(),
                tool_name.to_string(),
                reason.to_string(),
            ));
        }

        async fn notify_yolo_bypass_turn(&self, summary: &ironhermes_core::YoloBypassSummary) {
            self.turn_notifications.lock().unwrap().push((
                summary.session_id.clone(),
                summary.count(),
                summary.by_tool(),
            ));
        }
    }

    fn test_guard() -> DangerousCommandGuardrail {
        DangerousCommandGuardrail::new()
    }

    /// Tempfile-backed `AuditLog` (Task 2 requirement) — returns the `TempDir` guard
    /// alongside the log so the directory isn't dropped before assertions read it back.
    fn test_log() -> (tempfile::TempDir, AuditLog) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("audit.jsonl");
        let log = AuditLog::with_path(path, AuditConfig::default());
        (dir, log)
    }

    fn read_entries(dir: &tempfile::TempDir) -> Vec<ironhermes_core::AuditEntry> {
        let path = dir.path().join("audit.jsonl");
        let contents = std::fs::read_to_string(&path).unwrap_or_default();
        contents
            .lines()
            .map(|l| serde_json::from_str(l).expect("valid AuditEntry JSON"))
            .collect()
    }

    /// D-03/D-13: a Tier-2 `Block` command NEVER invokes `run` (AtomicBool stays false),
    /// EVEN UNDER yolo=true, and is audited with decision "blocked".
    #[tokio::test]
    async fn block_never_invokes_run_even_under_yolo() {
        let guard = test_guard();
        let (dir, log) = test_log();
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = ran.clone();

        let outcome = execute_gated_command(
            "terminal",
            "rm -rf /",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            true, // yolo=true — must NOT bypass Tier-2 (D-13)
            false,
            false,
            || async move {
                ran_clone.store(true, Ordering::SeqCst);
                Ok("should never run".to_string())
            },
        )
        .await;

        assert!(
            matches!(outcome, GatedOutcome::Blocked(_)),
            "expected Blocked, got {outcome:?}"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "run must NEVER be invoked on a Tier-2 Block, even under yolo (D-13)"
        );

        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1, "Block must be audited exactly once");
        assert_eq!(entries[0].decision, "blocked");
        assert_eq!(entries[0].resolution, "dropped");
    }

    /// Warn + no approval gate wired → Denied + audited "denied"/"no_gate" (D-12).
    #[tokio::test]
    async fn warn_no_gate_denies_and_audits() {
        let guard = test_guard();
        let (dir, log) = test_log();

        let outcome = execute_gated_command(
            "terminal",
            "curl https://x",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Ok("unused".to_string()) },
        )
        .await;

        assert!(
            matches!(outcome, GatedOutcome::Denied(_)),
            "expected Denied, got {outcome:?}"
        );
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "denied");
        assert_eq!(entries[0].resolution, "no_gate");
    }

    /// Warn + gate returns Approved → run invoked + audited decision "approved".
    #[tokio::test]
    async fn warn_approved_runs_and_audits() {
        let guard = test_guard();
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);

        let outcome = execute_gated_command(
            "terminal",
            "curl https://x",
            &guard,
            Some(&gate),
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Ok("ran ok".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("ran ok".to_string()));
        assert!(gate.called.load(Ordering::SeqCst), "gate must be consulted");
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "approved");
        assert_eq!(entries[0].resolution, "operator");
    }

    /// Warn + gate returns Denied/TimedOut/AlreadyPending/GateUnavailable → Denied +
    /// audited decision "denied" for each outcome.
    #[tokio::test]
    async fn warn_gate_denied_variants_deny_and_audit() {
        for outcome_in in [
            ApprovalOutcome::Denied,
            ApprovalOutcome::TimedOut,
            ApprovalOutcome::AlreadyPending,
            ApprovalOutcome::GateUnavailable,
        ] {
            let guard = test_guard();
            let (dir, log) = test_log();
            let gate = FakeGate::new(outcome_in);

            let outcome = execute_gated_command(
                "terminal",
                "curl https://x",
                &guard,
                Some(&gate),
                &log,
                "sess1",
                "cli",
                "chat1",
                false,
                false,
                false,
                || async { Ok("should not run".to_string()) },
            )
            .await;

            assert!(
                matches!(outcome, GatedOutcome::Denied(_)),
                "outcome_in={outcome_in:?} expected Denied, got {outcome:?}"
            );
            let entries = read_entries(&dir);
            assert_eq!(entries.len(), 1, "outcome_in={outcome_in:?}");
            assert_eq!(entries[0].decision, "denied", "outcome_in={outcome_in:?}");
        }
    }

    /// G-1 / D-07: every refusal on the gated path carries the no-circumvention text built
    /// by `refusal`, and `run` is never invoked.
    #[tokio::test]
    async fn gated_refusals_forbid_circumvention() {
        use crate::refusal::{NO_CIRCUMVENTION_NOTICE, approval_refusal_text};

        for outcome_in in [
            ApprovalOutcome::Denied,
            ApprovalOutcome::TimedOut,
            ApprovalOutcome::AlreadyPending,
            ApprovalOutcome::GateUnavailable,
        ] {
            let guard = test_guard();
            let (_dir, log) = test_log();
            let gate = FakeGate::new(outcome_in);
            let ran = Arc::new(AtomicBool::new(false));
            let ran_clone = ran.clone();

            let outcome = execute_gated_command(
                "terminal",
                "curl http://example.com/x",
                &guard,
                Some(&gate),
                &log,
                "sess1",
                "cli",
                "chat1",
                false,
                false,
                false,
                || async move {
                    ran_clone.store(true, Ordering::SeqCst);
                    Ok("should not run".to_string())
                },
            )
            .await;

            let expected = approval_refusal_text("terminal", &outcome_in).unwrap();
            assert_eq!(
                outcome,
                GatedOutcome::Denied(expected),
                "outcome_in={outcome_in:?}"
            );
            assert!(!ran.load(Ordering::SeqCst), "run must not be invoked");
        }

        // No gate wired: security policy refused, with the same notice.
        let guard = test_guard();
        let (_dir, log) = test_log();
        let outcome = execute_gated_command(
            "terminal",
            "curl http://example.com/x",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Err(anyhow::anyhow!("run must not be invoked")) },
        )
        .await;
        let GatedOutcome::Denied(t) = outcome else {
            panic!("expected Denied, got {outcome:?}");
        };
        assert!(
            t.starts_with("This terminal call needs the operator's approval ("),
            "{t}"
        );
        assert!(t.ends_with(NO_CIRCUMVENTION_NOTICE), "{t}");

        // Tier-2 Block: policy refused, with the same notice.
        let guard = test_guard();
        let (_dir, log) = test_log();
        let outcome = execute_gated_command(
            "terminal",
            "rm -rf /",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Err(anyhow::anyhow!("run must not be invoked")) },
        )
        .await;
        let GatedOutcome::Blocked(t) = outcome else {
            panic!("expected Blocked, got {outcome:?}");
        };
        assert!(
            t.starts_with("Security policy blocked this terminal call ("),
            "{t}"
        );
        assert!(t.ends_with(NO_CIRCUMVENTION_NOTICE), "{t}");
    }

    /// Allow (not remote, forward_env empty) → run invoked AND audited exactly once
    /// with decision "allowed" — closes the routine-command audit gap (Pitfall 3).
    #[tokio::test]
    async fn allow_runs_and_audits_routine_command() {
        let guard = test_guard();
        let (dir, log) = test_log();

        let outcome = execute_gated_command(
            "terminal",
            "ls -la",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Ok("file listing".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("file listing".to_string()));
        let entries = read_entries(&dir);
        assert_eq!(
            entries.len(),
            1,
            "an Allow command must produce exactly one audit entry (Pitfall 3 closed)"
        );
        assert_eq!(entries[0].decision, "allowed");
        assert_eq!(entries[0].resolution, "allowed");
    }

    /// D-08 load-bearing test: `is_remote_backend=true` forces even an `Allow` command
    /// through the approval gate (asserts the fake gate's `request_approval` was
    /// invoked) AND is audited.
    #[tokio::test]
    async fn allow_forced_through_gate_when_remote_backend() {
        let guard = test_guard();
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);

        let outcome = execute_gated_command(
            "terminal",
            "ls -la",
            &guard,
            Some(&gate),
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            true, // is_remote_backend
            false,
            || async { Ok("remote ls".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("remote ls".to_string()));
        assert!(
            gate.called.load(Ordering::SeqCst),
            "is_remote_backend=true must route an Allow command through the approval gate (D-08)"
        );
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "approved");
    }

    /// D-08: `forward_env_nonempty=true` forces even an `Allow` command through the
    /// approval gate, and is audited.
    #[tokio::test]
    async fn allow_forced_through_gate_when_forward_env_nonempty() {
        let guard = test_guard();
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);

        let outcome = execute_gated_command(
            "terminal",
            "ls -la",
            &guard,
            Some(&gate),
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            true, // forward_env_nonempty
            || async { Ok("cred-forwarded ls".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("cred-forwarded ls".to_string()));
        assert!(
            gate.called.load(Ordering::SeqCst),
            "forward_env_nonempty=true must route an Allow command through the approval gate (D-08)"
        );
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "approved");
    }

    /// yolo=true bypasses Tier-1 approval (still audited) but a Tier-2 Block under yolo
    /// still returns Blocked.
    #[tokio::test]
    async fn yolo_bypasses_tier1_but_not_block() {
        let guard = test_guard();
        let (dir, log) = test_log();

        // Tier-1 under yolo: runs without consulting a gate, audited "allowed"/"bypass".
        let outcome = execute_gated_command(
            "terminal",
            "curl https://x",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            true,
            false,
            false,
            || async { Ok("ran under yolo".to_string()) },
        )
        .await;
        assert_eq!(outcome, GatedOutcome::Ran("ran under yolo".to_string()));

        // Tier-2 under yolo: still Blocked, run never invoked.
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = ran.clone();
        let outcome2 = execute_gated_command(
            "terminal",
            "rm -rf /",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            true,
            false,
            false,
            || async move {
                ran_clone.store(true, Ordering::SeqCst);
                Ok("never".to_string())
            },
        )
        .await;
        assert!(matches!(outcome2, GatedOutcome::Blocked(_)));
        assert!(!ran.load(Ordering::SeqCst));

        let entries = read_entries(&dir);
        assert_eq!(
            entries.len(),
            2,
            "both calls must each produce one audit entry"
        );
        assert_eq!(entries[0].decision, "allowed");
        assert_eq!(entries[0].resolution, "bypass");
        assert_eq!(entries[1].decision, "blocked");
    }

    // ---------------------------------------------------------------------------
    // Task 2 (D-04): notify_yolo_bypass — the operator must learn of a yolo bypass
    // on the surface that issued it, not only after the fact in audit.jsonl.
    // ---------------------------------------------------------------------------

    /// UNSCOPED-FALLBACK contract (Phase 54.3 D-03 — retargeted, not deleted): a `Warn`
    /// decision with `yolo=true` and a gate supplied, reached with NO `YOLO_LEDGER`
    /// scope installed, makes `try_record` return false, so the designed fail-visible
    /// degradation fires — exactly one per-call `notify_yolo_bypass` whose captured
    /// message is the guardrail `reason` — and the command still runs. The scoped,
    /// aggregated contract is covered by the `yolo_ledger_scoped_*` tests below.
    #[tokio::test]
    async fn yolo_notifies_operator_on_bypass() {
        let guard = test_guard();
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);

        let outcome = execute_gated_command(
            "terminal",
            "curl https://x",
            &guard,
            Some(&gate),
            &log,
            "sess1",
            "cli",
            "chat1",
            true, // yolo=true
            false,
            false,
            || async { Ok("ran under yolo".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("ran under yolo".to_string()));
        assert!(
            !gate.called.load(Ordering::SeqCst),
            "request_approval must NOT be consulted under yolo — notify_yolo_bypass \
             is called INSTEAD of it"
        );

        let notifications = gate.notifications();
        assert_eq!(
            notifications.len(),
            1,
            "expected exactly one notify_yolo_bypass call (unscoped fallback: no \
             YOLO_LEDGER scope is installed, so try_record returns false and the \
             per-call notice fires), got {notifications:?}"
        );
        assert!(
            gate.turn_notifications().is_empty(),
            "the unscoped path must NOT produce a per-turn delivery, got {:?}",
            gate.turn_notifications()
        );
        let (sid, tool, reason) = &notifications[0];
        assert_eq!(sid, "sess1");
        assert_eq!(tool, "terminal");
        assert!(
            !reason.is_empty(),
            "captured reason must contain the guardrail's reason string, got empty"
        );

        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "allowed");
        assert_eq!(entries[0].resolution, "bypass");
        assert_eq!(
            &entries[0].reason, reason,
            "the notified reason must be the SAME guardrail reason recorded in the \
             audit entry — the notifier is the operator-facing form of the audit \
             record, not an independent restatement"
        );
    }

    /// The same `Warn` decision with `yolo=false` results in zero
    /// `notify_yolo_bypass` calls and exactly one `request_approval` call. (Phase 54.3:
    /// the matching scoped variant, asserting the ledger also records nothing, is
    /// `yolo_ledger_non_yolo_records_nothing_under_a_scope`.)
    #[tokio::test]
    async fn non_yolo_does_not_notify() {
        let guard = test_guard();
        let (_dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);

        let outcome = execute_gated_command(
            "terminal",
            "curl https://x",
            &guard,
            Some(&gate),
            &log,
            "sess1",
            "cli",
            "chat1",
            false, // yolo=false
            false,
            false,
            || async { Ok("ran via approval".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("ran via approval".to_string()));
        assert!(
            gate.called.load(Ordering::SeqCst),
            "request_approval must be consulted when yolo=false"
        );
        assert!(
            gate.notifications().is_empty(),
            "notify_yolo_bypass must NEVER fire on the non-yolo approval path, got \
             {:?}",
            gate.notifications()
        );
    }

    /// A Tier-2 `Block` decision results in zero `notify_yolo_bypass` calls at any
    /// yolo setting — `Block` is matched and returned before the yolo branch exists
    /// in the call graph (D-13), and the notifier must not weaken that invariant.
    /// (Phase 54.3: the scoped variant asserting `recorded_count() == 0` is
    /// `yolo_ledger_tier2_block_records_nothing_under_a_scope` — no record site can be
    /// on a Block path, ASVS V4.)
    #[tokio::test]
    async fn tier2_block_never_notifies() {
        let guard = test_guard();
        let (_dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);

        let outcome = execute_gated_command(
            "terminal",
            "rm -rf /",
            &guard,
            Some(&gate),
            &log,
            "sess1",
            "cli",
            "chat1",
            true, // yolo=true — must NOT reach the notifier either (D-13)
            false,
            false,
            || async { Ok("should never run".to_string()) },
        )
        .await;

        assert!(matches!(outcome, GatedOutcome::Blocked(_)));
        assert!(
            gate.notifications().is_empty(),
            "a Tier-2 Block must never invoke notify_yolo_bypass, at any yolo \
             setting, got {:?}",
            gate.notifications()
        );
    }

    /// A yolo bypass with no approval gate supplied still runs, still audits, and
    /// does not panic — the notification is best-effort. (Phase 54.3: the scoped
    /// variant, where the ledger records the bypass with `approval_gate: None`, is
    /// `yolo_ledger_scoped_bypass_without_gate_records_and_does_not_panic`.)
    #[tokio::test]
    async fn yolo_bypass_without_gate_does_not_panic() {
        let guard = test_guard();
        let (dir, log) = test_log();

        let outcome = execute_gated_command(
            "terminal",
            "curl https://x",
            &guard,
            None, // no approval gate wired
            &log,
            "sess1",
            "cli",
            "chat1",
            true, // yolo=true
            false,
            false,
            || async { Ok("ran under yolo, no gate".to_string()) },
        )
        .await;

        assert_eq!(
            outcome,
            GatedOutcome::Ran("ran under yolo, no gate".to_string())
        );
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "allowed");
        assert_eq!(entries[0].resolution, "bypass");
    }

    /// A `run` error maps to `Failed`, distinguishable from Denied/Blocked (WR-03
    /// semantics) — proves a command that ran-but-errored is not confused with one that
    /// never ran at all.
    #[tokio::test]
    async fn run_error_maps_to_failed() {
        let guard = test_guard();
        let (_dir, log) = test_log();

        let outcome = execute_gated_command(
            "terminal",
            "ls -la",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Err(anyhow::anyhow!("spawn failed")) },
        )
        .await;

        assert!(
            matches!(outcome, GatedOutcome::Failed(_)),
            "expected Failed, got {outcome:?}"
        );
    }

    /// D-11: `execute_code` passes an EMPTY opaque `classify_arg` — raw Python source
    /// never reaches `DangerousCommandGuardrail`'s shell-syntax `DANGEROUS_PATTERNS`
    /// matching, so a benign script classifies Allow (and is still audited).
    #[tokio::test]
    async fn execute_code_empty_classify_arg_allows_and_audits() {
        let guard = test_guard();
        let (dir, log) = test_log();

        let outcome = execute_gated_command(
            "execute_code",
            "",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Ok("python output".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("python output".to_string()));
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tool, "execute_code");
        assert_eq!(entries[0].decision, "allowed");
    }

    /// A Warn command routes to the approval branch, not silent pass-through — the
    /// generic `Allow | Warn` fast-path anti-pattern (RESEARCH.md Pitfall 2 / this
    /// plan's `prohibitions` entry) must never be reachable from this helper. Proven by
    /// `warn_no_gate_denies_and_audits` (Denied, not Ran, with no gate) and
    /// `warn_approved_runs_and_audits` (only runs after an explicit Approved outcome).
    #[tokio::test]
    async fn warn_never_silently_passes_through_without_gate_consultation() {
        let guard = test_guard();
        let (_dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Denied);

        let outcome = execute_gated_command(
            "terminal",
            "curl https://x",
            &guard,
            Some(&gate),
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Ok("must not run".to_string()) },
        )
        .await;

        assert!(
            gate.called.load(Ordering::SeqCst),
            "Warn must always consult the gate"
        );
        assert!(
            matches!(outcome, GatedOutcome::Denied(_)),
            "Warn + explicit Denied must Deny, not silently pass through, got {outcome:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 54.3 D-03/D-04/D-05/D-12: the SCOPED per-turn contract. Every test below
    // runs `execute_gated_command(.., yolo = true, ..)` inside a `YOLO_LEDGER.scope(..)`
    // against a tempdir `AuditLog`, then calls `flush_yolo_ledger`.
    //
    // Audit read-back is always stream-parsed: `audit.jsonl` is NOT strictly
    // line-delimited (the live log has lines carrying several concatenated entries), so
    // never a shell line count, `grep -c` or a line-split here.
    // ---------------------------------------------------------------------------

    use crate::yolo_ledger::{
        SIDECAR_UNAVAILABLE_ROLLUP_KIND, YOLO_FLUSH_TIMEOUT_SECS, YOLO_LEDGER, YOLO_ROLLUP_KIND,
        YOLO_ROLLUP_RESOLUTION, YoloLedger, flush_yolo_ledger,
    };

    /// A command containing `|` — the D-09 metachar `Warn` that yolo overrides.
    const WARN_CMD: &str = "echo a | tr a b";

    /// Stream-safe read of every audit entry in the tempdir log.
    fn read_rows(dir: &tempfile::TempDir) -> Vec<serde_json::Value> {
        let contents = std::fs::read_to_string(dir.path().join("audit.jsonl")).unwrap_or_default();
        serde_json::Deserializer::from_str(&contents)
            .into_iter::<serde_json::Value>()
            .map(|r| r.expect("audit stream must parse"))
            .collect()
    }

    fn count_rows(rows: &[serde_json::Value], kind: &str, resolution: &str) -> usize {
        rows.iter()
            .filter(|r| r["kind"] == kind && r["resolution"] == resolution)
            .count()
    }

    /// Run one yolo `execute_gated_command` for `cmd`; `remote` forces the D-12 site.
    async fn yolo_call(
        gate: Option<&dyn ApprovalGate>,
        log: &AuditLog,
        cmd: &str,
        remote: bool,
    ) -> GatedOutcome {
        execute_gated_command(
            "terminal",
            cmd,
            &test_guard(),
            gate,
            log,
            "sess1",
            "cli",
            "chat1",
            true,
            remote,
            false,
            || async { Ok("ran".to_string()) },
        )
        .await
    }

    /// A gate whose notifiers never complete — proves nothing on the scoped path awaits
    /// a notifier.
    struct NeverCompletesGate;

    #[async_trait::async_trait]
    impl ApprovalGate for NeverCompletesGate {
        async fn request_approval(
            &self,
            _s: &str,
            _t: &str,
            _r: &str,
            _a: &serde_json::Value,
        ) -> ApprovalOutcome {
            ApprovalOutcome::Denied
        }
        async fn resolve(&self, _s: &str, _a: bool) -> bool {
            false
        }
        async fn notify_yolo_bypass(&self, _s: &str, _t: &str, _r: &str) {
            std::future::pending::<()>().await;
        }
        async fn notify_yolo_bypass_turn(&self, _summary: &ironhermes_core::YoloBypassSummary) {
            std::future::pending::<()>().await;
        }
    }

    /// V1: N=5 scoped yolo bypasses → ZERO per-call notifies, a ledger holding 5, and
    /// ONE `notify_yolo_bypass_turn` of `count() == 5` after the flush. The second half
    /// proves the scoped path never awaits a notifier: with a gate whose per-call
    /// notifier would block forever if awaited, three scoped bypasses still finish.
    #[tokio::test]
    async fn yolo_ledger_scoped_turn_aggregates_n_bypasses_into_one_summary() {
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Denied);
        let ledger = Arc::new(YoloLedger::default());

        YOLO_LEDGER
            .scope(ledger.clone(), async {
                for _ in 0..5 {
                    let o = yolo_call(Some(&gate), &log, WARN_CMD, false).await;
                    assert_eq!(o, GatedOutcome::Ran("ran".to_string()));
                }
            })
            .await;

        assert!(
            gate.notifications().is_empty(),
            "a scoped turn must produce ZERO per-call notify_yolo_bypass calls, got {:?}",
            gate.notifications()
        );
        assert_eq!(ledger.recorded_count(), 5);

        flush_yolo_ledger(&ledger, &log, Some(&gate), None).await;
        let turns = gate.turn_notifications();
        assert_eq!(turns.len(), 1, "exactly ONE per-turn delivery: {turns:?}");
        assert_eq!(turns[0].1, 5, "summary.count() == 5: {turns:?}");
        assert_eq!(turns[0].2, vec![("terminal".to_string(), 5)]);
        assert_eq!(count_rows(&read_rows(&dir), "shell_command", "bypass"), 5);

        // Non-blocking record: a per-call notifier that never completes must not stall
        // the scoped path. If `handle_approval_required` awaited it, this would time out.
        let (_dir2, log2) = test_log();
        let hanging = NeverCompletesGate;
        let ledger2 = Arc::new(YoloLedger::default());
        let finished = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            YOLO_LEDGER.scope(ledger2.clone(), async {
                for _ in 0..3 {
                    yolo_call(Some(&hanging), &log2, WARN_CMD, false).await;
                }
            }),
        )
        .await;
        assert!(
            finished.is_ok(),
            "the scoped record path must never await a notifier (V6)"
        );
        assert_eq!(ledger2.recorded_count(), 3);
    }

    /// V2: a scoped turn with zero bypasses flushes nothing — no notifier call, no
    /// rollup row. (A non-bypass `Allow` still writes its own `allowed` row.)
    #[tokio::test]
    async fn yolo_ledger_scoped_turn_with_no_bypasses_flushes_nothing() {
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);
        let ledger = Arc::new(YoloLedger::default());

        YOLO_LEDGER
            .scope(ledger.clone(), async {
                // `ls` classifies Allow and is not remote → resolution "allowed", no bypass.
                yolo_call(Some(&gate), &log, "ls", false).await;
            })
            .await;
        assert_eq!(ledger.recorded_count(), 0);

        flush_yolo_ledger(&ledger, &log, Some(&gate), None).await;

        assert!(gate.turn_notifications().is_empty());
        assert!(gate.notifications().is_empty());
        let rows = read_rows(&dir);
        assert_eq!(
            count_rows(&rows, YOLO_ROLLUP_KIND, YOLO_ROLLUP_RESOLUTION),
            0
        );
        assert_eq!(rows.len(), 1, "only the Allow row itself: {rows:?}");
    }

    /// V3: after a scoped 5-bypass run plus flush, the audit file holds exactly 5
    /// `resolution == "bypass"` rows (D-04: per-call rows unchanged) and exactly 1
    /// rollup row — `kind == yolo_bypass_rollup`, `resolution == "rollup"` (never
    /// `"bypass"`), `args.count == 5`.
    #[tokio::test]
    async fn yolo_ledger_rollup_entry_shape_and_per_call_rows_preserved() {
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Denied);
        let ledger = Arc::new(YoloLedger::default());

        YOLO_LEDGER
            .scope(ledger.clone(), async {
                for _ in 0..5 {
                    yolo_call(Some(&gate), &log, WARN_CMD, false).await;
                }
            })
            .await;
        flush_yolo_ledger(&ledger, &log, Some(&gate), None).await;

        let rows = read_rows(&dir);
        assert_eq!(
            count_rows(&rows, "shell_command", "bypass"),
            5,
            "per-call bypass rows are 1:1 with bypassed calls (D-04): {rows:?}"
        );
        let rollups: Vec<_> = rows
            .iter()
            .filter(|r| r["kind"] == YOLO_ROLLUP_KIND)
            .collect();
        assert_eq!(rollups.len(), 1, "exactly one rollup row (D-05): {rows:?}");
        assert_eq!(rollups[0]["resolution"], "rollup");
        assert_ne!(
            rollups[0]["resolution"], "bypass",
            "a rollup carrying resolution=\"bypass\" double-counts every bypass consumer"
        );
        assert_eq!(rollups[0]["decision"], "allowed");
        let args: serde_json::Value =
            serde_json::from_str(rollups[0]["args"].as_str().unwrap()).unwrap();
        assert_eq!(args["count"], 5);
        assert_eq!(args["tools"]["terminal"], 5);
    }

    /// V6 ordering: the rollup audit row is on disk BEFORE the notifier is invoked.
    #[tokio::test]
    async fn yolo_ledger_flush_appends_rollup_before_invoking_notifier() {
        struct ProbeGate {
            audit_path: std::path::PathBuf,
            rollup_present_at_notify: AtomicBool,
            notified: AtomicBool,
        }

        #[async_trait::async_trait]
        impl ApprovalGate for ProbeGate {
            async fn request_approval(
                &self,
                _s: &str,
                _t: &str,
                _r: &str,
                _a: &serde_json::Value,
            ) -> ApprovalOutcome {
                ApprovalOutcome::Denied
            }
            async fn resolve(&self, _s: &str, _a: bool) -> bool {
                false
            }
            async fn notify_yolo_bypass_turn(&self, _summary: &ironhermes_core::YoloBypassSummary) {
                let contents = std::fs::read_to_string(&self.audit_path).unwrap_or_default();
                let present = serde_json::Deserializer::from_str(&contents)
                    .into_iter::<serde_json::Value>()
                    .filter_map(Result::ok)
                    .any(|r| r["kind"] == YOLO_ROLLUP_KIND);
                self.rollup_present_at_notify
                    .store(present, Ordering::SeqCst);
                self.notified.store(true, Ordering::SeqCst);
            }
        }

        let (dir, log) = test_log();
        let probe = ProbeGate {
            audit_path: dir.path().join("audit.jsonl"),
            rollup_present_at_notify: AtomicBool::new(false),
            notified: AtomicBool::new(false),
        };
        let ledger = Arc::new(YoloLedger::default());
        YOLO_LEDGER
            .scope(ledger.clone(), async {
                yolo_call(Some(&probe), &log, WARN_CMD, false).await;
            })
            .await;

        flush_yolo_ledger(&ledger, &log, Some(&probe), None).await;

        assert!(
            probe.notified.load(Ordering::SeqCst),
            "notifier must be invoked"
        );
        assert!(
            probe.rollup_present_at_notify.load(Ordering::SeqCst),
            "the rollup audit row must already be on disk when the notifier runs, so a \
             hanging notifier can never cost the audit record"
        );
    }

    /// V6 bound: a notifier whose `notify_yolo_bypass_turn` never completes cannot hold
    /// `flush_yolo_ledger` past `YOLO_FLUSH_TIMEOUT_SECS` (virtual time), and the
    /// rollup row is still on disk.
    #[tokio::test(start_paused = true)]
    async fn yolo_ledger_flush_timeout_does_not_hang_on_a_never_completing_notifier() {
        let (dir, log) = test_log();
        let hanging = NeverCompletesGate;
        let ledger = Arc::new(YoloLedger::default());
        YOLO_LEDGER
            .scope(ledger.clone(), async {
                yolo_call(Some(&hanging), &log, WARN_CMD, false).await;
            })
            .await;

        let start = tokio::time::Instant::now();
        let bound = std::time::Duration::from_secs(YOLO_FLUSH_TIMEOUT_SECS);
        let outcome = tokio::time::timeout(bound * 6, async {
            tokio::join!(
                flush_yolo_ledger(&ledger, &log, Some(&hanging), None),
                async {
                    tokio::task::yield_now().await;
                    tokio::time::advance(bound + std::time::Duration::from_secs(1)).await;
                }
            )
        })
        .await;

        assert!(
            outcome.is_ok(),
            "flush must return once virtual time passes YOLO_FLUSH_TIMEOUT_SECS"
        );
        assert!(
            start.elapsed() >= bound,
            "flush returned because the bound elapsed, not because the notifier finished"
        );
        assert_eq!(
            count_rows(&read_rows(&dir), YOLO_ROLLUP_KIND, YOLO_ROLLUP_RESOLUTION),
            1,
            "the rollup row is appended before the notifier is awaited"
        );
    }

    /// D-12: the forced-approval-under-yolo `Allow` site (remote backend) records into
    /// the ledger too, so on a turn mixing both bypass sites `rollup.args.count` equals
    /// the number of `resolution == "bypass"` rows.
    #[tokio::test]
    async fn yolo_ledger_records_forced_approval_bypass_site() {
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Denied);
        let ledger = Arc::new(YoloLedger::default());

        YOLO_LEDGER
            .scope(ledger.clone(), async {
                // Site 1: the Warn branch (metachar).
                yolo_call(Some(&gate), &log, WARN_CMD, false).await;
                // Site 2: `ls` classifies Allow, but a remote backend forces approval,
                // which yolo bypasses (`needs_forced_approval && yolo`).
                yolo_call(Some(&gate), &log, "ls", true).await;
            })
            .await;
        assert_eq!(
            ledger.recorded_count(),
            2,
            "both bypass-audit sites must record (D-12)"
        );
        assert!(
            gate.notifications().is_empty(),
            "scoped: no per-call notify at either site"
        );

        flush_yolo_ledger(&ledger, &log, Some(&gate), None).await;

        let rows = read_rows(&dir);
        let bypass_rows = count_rows(&rows, "shell_command", "bypass");
        assert_eq!(bypass_rows, 2, "{rows:?}");
        let rollup = rows
            .iter()
            .find(|r| r["kind"] == YOLO_ROLLUP_KIND)
            .expect("a rollup row");
        let args: serde_json::Value =
            serde_json::from_str(rollup["args"].as_str().unwrap()).unwrap();
        assert_eq!(
            args["count"].as_u64().unwrap() as usize,
            bypass_rows,
            "rollup.args.count must equal count(resolution==\"bypass\") for the turn"
        );
        assert_eq!(gate.turn_notifications()[0].1, 2);
    }

    /// Scoped variant of `non_yolo_does_not_notify`: a non-yolo approval path records
    /// nothing even with a ledger in scope.
    #[tokio::test]
    async fn yolo_ledger_non_yolo_records_nothing_under_a_scope() {
        let (_dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);
        let ledger = Arc::new(YoloLedger::default());

        YOLO_LEDGER
            .scope(ledger.clone(), async {
                execute_gated_command(
                    "terminal",
                    "curl https://x",
                    &test_guard(),
                    Some(&gate),
                    &log,
                    "sess1",
                    "cli",
                    "chat1",
                    false,
                    false,
                    false,
                    || async { Ok("ran via approval".to_string()) },
                )
                .await;
            })
            .await;

        assert_eq!(ledger.recorded_count(), 0);
        assert!(gate.notifications().is_empty());
    }

    /// Scoped variant of `tier2_block_never_notifies`: `Block` is matched and returned
    /// before `handle_approval_required`, so no record site can be on a Block path
    /// (T-54.3-06, ASVS V4) — even under yolo, even with a ledger in scope.
    #[tokio::test]
    async fn yolo_ledger_tier2_block_records_nothing_under_a_scope() {
        let (_dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);
        let ledger = Arc::new(YoloLedger::default());

        let outcome = YOLO_LEDGER
            .scope(ledger.clone(), async {
                yolo_call(Some(&gate), &log, "rm -rf /", false).await
            })
            .await;

        assert!(matches!(outcome, GatedOutcome::Blocked(_)));
        assert_eq!(ledger.recorded_count(), 0);
        assert!(gate.notifications().is_empty());
    }

    /// Scoped variant of `yolo_bypass_without_gate_does_not_panic`: with
    /// `approval_gate: None` the ledger still records the bypass and nothing panics.
    #[tokio::test]
    async fn yolo_ledger_scoped_bypass_without_gate_records_and_does_not_panic() {
        let (dir, log) = test_log();
        let ledger = Arc::new(YoloLedger::default());

        let outcome = YOLO_LEDGER
            .scope(ledger.clone(), async {
                yolo_call(None, &log, WARN_CMD, false).await
            })
            .await;

        assert_eq!(outcome, GatedOutcome::Ran("ran".to_string()));
        assert_eq!(ledger.recorded_count(), 1);
        assert_eq!(count_rows(&read_rows(&dir), "shell_command", "bypass"), 1);
    }

    // ---------------------------------------------------------------------------
    // Phase 57 plan 07 Task 2: the level-driven verdict pipeline (D-03, D-06, D-07,
    // D-13, D-21, D-24). Every test pins a level with `GatePolicy::fixed`.
    // ---------------------------------------------------------------------------

    use ironhermes_core::config::DefconLevel;
    use ironhermes_security::policy::Severity;
    use ironhermes_security::service::SecurityService;
    use ironhermes_security::testing::FakeBackend;

    fn lvl(n: u8) -> DefconLevel {
        DefconLevel::try_from(n).expect("1..=5 is a valid DEFCON level")
    }

    /// A service over `fake`, keeping the caller's handle on the fake for `seen()`.
    fn service_over(fake: &Arc<FakeBackend>) -> Arc<SecurityService> {
        Arc::new(SecurityService::new(fake.clone()))
    }

    /// Drive one call at `level` and report the outcome and whether `run` was invoked.
    async fn drive(
        tool: &str,
        arg: &str,
        level: u8,
        security: Option<Arc<SecurityService>>,
        gate: Option<&dyn ApprovalGate>,
        remote: bool,
        log: &AuditLog,
    ) -> (GatedOutcome, bool) {
        let ran = Arc::new(AtomicBool::new(false));
        let ran_in = ran.clone();
        let outcome = execute_gated_command(
            tool,
            arg,
            &test_guard(),
            gate,
            log,
            "sess1",
            "cli",
            "chat1",
            GatePolicy::fixed(lvl(level), security),
            remote,
            false,
            || async move {
                ran_in.store(true, Ordering::SeqCst);
                Ok("ran".to_string())
            },
        )
        .await;
        (outcome, ran.load(Ordering::SeqCst))
    }

    /// D-24: a plain `curl` is a FloorWarn. At the default level 3 it asks, it is not
    /// hard-blocked.
    #[tokio::test]
    async fn tier1_curl_needs_approval_at_three_not_block() {
        let (_dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);
        let (outcome, ran) = drive(
            "terminal",
            "curl https://example.com",
            3,
            None,
            Some(&gate),
            false,
            &log,
        )
        .await;
        assert!(
            gate.called.load(Ordering::SeqCst),
            "level 3 must ask the operator"
        );
        assert!(
            !matches!(outcome, GatedOutcome::Blocked(_)),
            "D-24: not a hard block at level 3, got {outcome:?}"
        );
        assert!(ran);
    }

    /// D-24: a FloorWarn is Block only at level 1; level 2 still asks.
    #[tokio::test]
    async fn tier1_blocks_only_at_one() {
        let (_dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);
        let (outcome, ran) = drive(
            "terminal",
            "curl https://example.com",
            1,
            None,
            Some(&gate),
            false,
            &log,
        )
        .await;
        assert!(matches!(outcome, GatedOutcome::Blocked(_)), "{outcome:?}");
        assert!(!ran && !gate.called.load(Ordering::SeqCst));

        let (outcome2, ran2) = drive(
            "terminal",
            "curl https://example.com",
            2,
            None,
            Some(&gate),
            false,
            &log,
        )
        .await;
        assert!(
            gate.called.load(Ordering::SeqCst),
            "level 2 must ask: {outcome2:?}"
        );
        assert!(ran2);
    }

    /// D-21: the bypass exists only at levels 4 and 5, and is audited and recorded in the
    /// per-turn ledger. At level 3 the same call reaches the approval gate and never bypasses.
    #[tokio::test]
    async fn tier1_bypassed_only_at_four_and_five() {
        for n in [4u8, 5] {
            let (dir, log) = test_log();
            let gate = FakeGate::new(ApprovalOutcome::Denied);
            let ledger = Arc::new(YoloLedger::default());
            let (outcome, ran) = YOLO_LEDGER
                .scope(ledger.clone(), async {
                    drive(
                        "terminal",
                        "curl https://example.com",
                        n,
                        None,
                        Some(&gate),
                        false,
                        &log,
                    )
                    .await
                })
                .await;
            assert_eq!(outcome, GatedOutcome::Ran("ran".to_string()), "level {n}");
            assert!(ran);
            assert!(
                !gate.called.load(Ordering::SeqCst),
                "level {n}: bypass skips the gate"
            );
            assert_eq!(
                count_rows(&read_rows(&dir), "shell_command", "bypass"),
                1,
                "level {n}"
            );
            assert_eq!(ledger.recorded_count(), 1, "level {n}");
        }

        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);
        let ledger = Arc::new(YoloLedger::default());
        let (_outcome, _ran) = YOLO_LEDGER
            .scope(ledger.clone(), async {
                drive(
                    "terminal",
                    "curl https://example.com",
                    3,
                    None,
                    Some(&gate),
                    false,
                    &log,
                )
                .await
            })
            .await;
        assert!(
            gate.called.load(Ordering::SeqCst),
            "level 3 must reach the gate"
        );
        assert_eq!(count_rows(&read_rows(&dir), "shell_command", "bypass"), 0);
        assert_eq!(ledger.recorded_count(), 0, "level 3 never bypasses (D-21)");
    }

    /// D-06 / D-24: Tier-2 is Critical, Blocked at every level.
    #[tokio::test]
    async fn tier2_blocks_everywhere() {
        for n in 1..=5u8 {
            let (_dir, log) = test_log();
            let (outcome, ran) = drive("terminal", "rm -rf /", n, None, None, false, &log).await;
            assert!(
                matches!(outcome, GatedOutcome::Blocked(_)),
                "level {n}: {outcome:?}"
            );
            assert!(!ran, "level {n}");
        }
    }

    /// D-13: Python that reaches subprocess, socket, ctypes, importlib or `__import__` is
    /// Blocked at levels 3, 2 and 1, and a FloorWarn (bypassable) at 4 and 5.
    #[tokio::test]
    async fn d13_reach_blocked_at_three() {
        for source in [
            "import socket\nprint(1)",
            "import ctypes\nprint(1)",
            "x = __import__('os')",
            "import subprocess; subprocess.run(['ls'])",
        ] {
            for n in [3u8, 2, 1] {
                let (_dir, log) = test_log();
                let (outcome, ran) =
                    drive("execute_code", source, n, None, None, false, &log).await;
                assert!(
                    matches!(outcome, GatedOutcome::Blocked(_)),
                    "level {n}, {source:?}: {outcome:?}"
                );
                assert!(!ran, "level {n}, {source:?}");
            }
            for n in [4u8, 5] {
                let (dir, log) = test_log();
                let (outcome, ran) =
                    drive("execute_code", source, n, None, None, false, &log).await;
                assert_eq!(
                    outcome,
                    GatedOutcome::Ran("ran".to_string()),
                    "level {n}, {source:?}"
                );
                assert!(ran);
                assert_eq!(
                    count_rows(&read_rows(&dir), "shell_command", "bypass"),
                    1,
                    "level {n}, {source:?}"
                );
            }
        }
    }

    /// D-06 / D-03: a sidecar HIGH finding on a floor-Allow command is Block at level 3 and
    /// Warn (bypassed) at levels 4 and 5.
    #[tokio::test]
    async fn sidecar_high_blocks_at_three() {
        let fake = Arc::new(FakeBackend::new().warn_if_contains("CANARY-HIGH", Severity::High));
        let svc = service_over(&fake);

        let (_dir, log) = test_log();
        let (outcome, ran) = drive(
            "terminal",
            "echo CANARY-HIGH",
            3,
            Some(svc.clone()),
            None,
            false,
            &log,
        )
        .await;
        assert!(matches!(outcome, GatedOutcome::Blocked(_)), "{outcome:?}");
        assert!(!ran);

        for n in [4u8, 5] {
            let (dir, log) = test_log();
            let (outcome, ran) = drive(
                "terminal",
                "echo CANARY-HIGH",
                n,
                Some(svc.clone()),
                None,
                false,
                &log,
            )
            .await;
            assert_eq!(outcome, GatedOutcome::Ran("ran".to_string()), "level {n}");
            assert!(ran);
            assert_eq!(
                count_rows(&read_rows(&dir), "shell_command", "bypass"),
                1,
                "level {n}"
            );
        }
    }

    /// D-03: a sidecar Allow never lowers the floor.
    #[tokio::test]
    async fn sidecar_allow_never_lowers_floor() {
        let fake = Arc::new(FakeBackend::new());
        let svc = service_over(&fake);
        for n in 1..=5u8 {
            let (_dir, log) = test_log();
            let (outcome, ran) = drive(
                "terminal",
                "rm -rf /",
                n,
                Some(svc.clone()),
                None,
                false,
                &log,
            )
            .await;
            assert!(
                matches!(outcome, GatedOutcome::Blocked(_)),
                "level {n}: {outcome:?}"
            );
            assert!(!ran);
        }
        let (_dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Denied);
        let (outcome, _ran) = drive(
            "terminal",
            "curl https://example.com",
            3,
            Some(svc),
            Some(&gate),
            false,
            &log,
        )
        .await;
        assert!(
            gate.called.load(Ordering::SeqCst),
            "a floor Warn still asks when the sidecar says Allow: {outcome:?}"
        );
    }

    /// D-03: the terminal subject is the command; the execute_code subject is the source
    /// wrapped as `python3 -c <shell-quoted source>`, as an Exec subject.
    #[tokio::test]
    async fn execute_code_subject_is_wrapped() {
        let fake = Arc::new(FakeBackend::new());
        let svc = service_over(&fake);
        let (_dir, log) = test_log();

        drive(
            "execute_code",
            "print(\"x\")",
            5,
            Some(svc.clone()),
            None,
            false,
            &log,
        )
        .await;
        drive(
            "execute_code",
            "print('x')",
            5,
            Some(svc.clone()),
            None,
            false,
            &log,
        )
        .await;
        drive("terminal", "echo hi", 5, Some(svc), None, false, &log).await;

        assert_eq!(
            fake.seen(),
            vec![
                "python3 -c 'print(\"x\")'".to_string(),
                "python3 -c 'print('\\''x'\\'')'".to_string(),
                "echo hi".to_string(),
            ]
        );
    }

    /// D-07 / D-24: a NeedsApproval floor result is never bypassed, even at level 5.
    #[tokio::test]
    async fn needs_approval_never_bypassed() {
        for n in [4u8, 5] {
            let (dir, log) = test_log();
            let gate = FakeGate::new(ApprovalOutcome::Approved);
            let ran = Arc::new(AtomicBool::new(false));
            let ran_in = ran.clone();
            let outcome = execute_with_floor(
                GuardrailDecision::NeedsApproval {
                    reason: "needs a human".to_string(),
                },
                "terminal",
                "ls",
                Some(&gate),
                &log,
                "sess1",
                "cli",
                "chat1",
                GatePolicy::fixed(lvl(n), None),
                false,
                false,
                || async move {
                    ran_in.store(true, Ordering::SeqCst);
                    Ok("ran".to_string())
                },
            )
            .await;
            assert!(
                gate.called.load(Ordering::SeqCst),
                "level {n}: must ask, not bypass"
            );
            assert_eq!(outcome, GatedOutcome::Ran("ran".to_string()));
            assert_eq!(
                count_rows(&read_rows(&dir), "shell_command", "bypass"),
                0,
                "level {n}"
            );
        }
    }

    /// D-21: the forced approval on a remote backend is bypassed only at levels 4 and 5.
    #[tokio::test]
    async fn forced_remote_approval_bypass_only_at_four_five() {
        let (_dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);
        let (_outcome, ran) = drive("terminal", "ls", 3, None, Some(&gate), true, &log).await;
        assert!(
            gate.called.load(Ordering::SeqCst),
            "level 3 must ask for a remote backend"
        );
        assert!(ran);

        for n in [4u8, 5] {
            let (dir, log) = test_log();
            let gate = FakeGate::new(ApprovalOutcome::Denied);
            let (outcome, ran) = drive("terminal", "ls", n, None, Some(&gate), true, &log).await;
            assert_eq!(outcome, GatedOutcome::Ran("ran".to_string()), "level {n}");
            assert!(ran);
            assert!(!gate.called.load(Ordering::SeqCst), "level {n}");
            assert_eq!(
                count_rows(&read_rows(&dir), "shell_command", "bypass"),
                1,
                "level {n}"
            );
        }
    }

    /// D-07: across every level, every Block source and every gate/remote state, `run` is
    /// never invoked. The bypass-ordering mutation (plan 07 research item 2) fails this.
    #[tokio::test]
    async fn block_never_runs_on_level_bypass_grid() {
        let critical =
            Arc::new(FakeBackend::new().block_if_contains("CANARY-CRIT", Severity::Critical));
        let svc = service_over(&critical);
        // (tool, argument, applies at levels)
        let sources: [(&str, &str, std::ops::RangeInclusive<u8>); 5] = [
            ("terminal", "rm -rf /", 1..=5),
            ("terminal", "wrangler delete hello-world-worker", 1..=5),
            ("terminal", "echo CANARY-CRIT", 1..=5),
            (
                "execute_code",
                "import subprocess\nsubprocess.run(['wrangler','delete','x'])",
                1..=5,
            ),
            ("execute_code", "import socket\nprint(1)", 1..=3),
        ];
        for (tool, arg, levels) in sources {
            for n in levels {
                for remote in [false, true] {
                    for with_gate in [false, true] {
                        let (_dir, log) = test_log();
                        let gate = FakeGate::new(ApprovalOutcome::Approved);
                        let gate_ref: Option<&dyn ApprovalGate> =
                            if with_gate { Some(&gate) } else { None };
                        let (outcome, ran) =
                            drive(tool, arg, n, Some(svc.clone()), gate_ref, remote, &log).await;
                        assert!(
                            matches!(outcome, GatedOutcome::Blocked(_)),
                            "{tool} {arg:?} level {n} remote {remote} gate {with_gate}: {outcome:?}"
                        );
                        assert!(
                            !ran,
                            "{tool} {arg:?} level {n} remote {remote} gate {with_gate}: run was invoked"
                        );
                        assert!(
                            !gate.called.load(Ordering::SeqCst),
                            "a Block must not even reach the gate"
                        );
                    }
                }
            }
        }
    }

    /// T-57-27 / T-57-28: a floor-only call keeps today's single `shell_command` row. A
    /// sidecar-contributed Block adds one `security_verdict` row carrying rule ids and the
    /// level, never the subject text.
    #[tokio::test]
    async fn security_verdict_rows_only_when_contributed() {
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);
        drive(
            "terminal",
            "curl https://example.com",
            3,
            None,
            Some(&gate),
            false,
            &log,
        )
        .await;
        let rows = read_rows(&dir);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["kind"], "shell_command");

        let fake =
            Arc::new(FakeBackend::new().block_if_contains("CANARY-CRIT", Severity::Critical));
        let (dir2, log2) = test_log();
        let (outcome, _ran) = drive(
            "terminal",
            "echo CANARY-CRIT",
            3,
            Some(service_over(&fake)),
            None,
            false,
            &log2,
        )
        .await;
        assert!(matches!(outcome, GatedOutcome::Blocked(_)));
        let rows2 = read_rows(&dir2);
        let kinds: Vec<&str> = rows2.iter().filter_map(|r| r["kind"].as_str()).collect();
        assert_eq!(
            kinds,
            vec!["shell_command", "security_verdict"],
            "{rows2:?}"
        );
        let verdict = rows2[1].to_string();
        assert!(verdict.contains("FakeBlock"), "rule id recorded: {verdict}");
        assert!(
            !verdict.contains("CANARY-CRIT"),
            "the subject text must not appear in a security_verdict row: {verdict}"
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 57 plan 07 Task 3: a dead sidecar fails by level and leaves a trail
    // (D-12), and surfaces hear about security events (D-16).
    // ---------------------------------------------------------------------------

    use ironhermes_core::{SecurityEvent, SecurityEventKind};
    use ironhermes_security::UnavailableReason;

    /// A gate that records every security event it is told about.
    struct EventGate {
        outcome: ApprovalOutcome,
        events: std::sync::Mutex<Vec<(String, SecurityEvent)>>,
    }

    impl EventGate {
        fn new(outcome: ApprovalOutcome) -> Self {
            Self {
                outcome,
                events: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn events(&self) -> Vec<SecurityEvent> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .map(|(_, e)| e.clone())
                .collect()
        }

        fn of_kind(&self, kind: SecurityEventKind) -> Vec<SecurityEvent> {
            self.events()
                .into_iter()
                .filter(|e| e.kind == kind)
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl ApprovalGate for EventGate {
        async fn request_approval(
            &self,
            _session_id: &str,
            _tool_name: &str,
            _reason: &str,
            _args: &serde_json::Value,
        ) -> ApprovalOutcome {
            self.outcome
        }

        async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
            false
        }

        async fn notify_security_event(&self, session_id: &str, event: &SecurityEvent) {
            self.events
                .lock()
                .unwrap()
                .push((session_id.to_string(), event.clone()));
        }
    }

    fn unavailable_service(reason: UnavailableReason) -> Arc<SecurityService> {
        Arc::new(SecurityService::new(Arc::new(FakeBackend::unavailable(
            reason,
        ))))
    }

    fn unavailable_rows(rows: &[serde_json::Value]) -> Vec<serde_json::Value> {
        rows.iter()
            .filter(|r| r["kind"] == SIDECAR_UNAVAILABLE_KIND)
            .map(|r| serde_json::from_str(r["args"].as_str().unwrap()).unwrap())
            .collect()
    }

    /// D-12: a Warn-class call with a dead sidecar is Warn at 4-5 (bypassed) and Block at
    /// 1-3. Each event writes one `sidecar_unavailable` row and one SidecarUnavailable
    /// event whose summary is `<reason>; outcome <blocked|approval|bypass|allowed>`.
    #[tokio::test]
    async fn unavailable_warn_class_by_level() {
        let svc = unavailable_service(UnavailableReason::Crash);

        let (dir5, log5) = test_log();
        let gate5 = EventGate::new(ApprovalOutcome::Denied);
        let (outcome, ran) = drive(
            "terminal",
            "ls",
            5,
            Some(svc.clone()),
            Some(&gate5),
            false,
            &log5,
        )
        .await;
        assert_eq!(outcome, GatedOutcome::Ran("ran".to_string()));
        assert!(ran);
        assert_eq!(
            unavailable_rows(&read_rows(&dir5)),
            vec![serde_json::json!({
                "reason": "crash", "tool": "terminal", "level": 5, "verdict": "warn"
            })]
        );
        let ev5 = gate5.of_kind(SecurityEventKind::SidecarUnavailable);
        assert_eq!(ev5.len(), 1, "{:?}", gate5.events());
        assert_eq!(ev5[0].summary, "crash; outcome bypass");
        assert_eq!(ev5[0].tool, "terminal");
        assert_eq!(ev5[0].level, 5);
        assert!(ev5[0].kind.is_critical());

        let (dir3, log3) = test_log();
        let gate3 = EventGate::new(ApprovalOutcome::Approved);
        let (outcome3, ran3) = drive(
            "terminal",
            "ls",
            3,
            Some(svc.clone()),
            Some(&gate3),
            false,
            &log3,
        )
        .await;
        assert!(matches!(outcome3, GatedOutcome::Blocked(_)), "{outcome3:?}");
        // The model sees the reason word, never the client's free-form detail.
        if let GatedOutcome::Blocked(text) = &outcome3 {
            assert!(text.contains("unavailable (crash)"), "{text}");
            assert!(!text.contains("fake backend is down"), "{text}");
        }
        assert!(!ran3);
        assert_eq!(
            unavailable_rows(&read_rows(&dir3)),
            vec![serde_json::json!({
                "reason": "crash", "tool": "terminal", "level": 3, "verdict": "block"
            })]
        );
        let ev3 = gate3.of_kind(SecurityEventKind::SidecarUnavailable);
        assert_eq!(ev3.len(), 1);
        assert_eq!(ev3[0].summary, "crash; outcome blocked");

        // Level 4 is bypass-active too; the reason word follows the outage.
        let (_d, log) = test_log();
        let gate4 = EventGate::new(ApprovalOutcome::Approved);
        let timeout_svc = unavailable_service(UnavailableReason::Timeout);
        drive(
            "terminal",
            "ls",
            4,
            Some(timeout_svc),
            Some(&gate4),
            false,
            &log,
        )
        .await;
        assert_eq!(
            gate4.of_kind(SecurityEventKind::SidecarUnavailable)[0].summary,
            "timeout; outcome bypass"
        );
    }

    /// D-12: every unavailability reason is recorded under its own word.
    #[tokio::test]
    async fn unavailable_reasons_recorded() {
        for (reason, word) in [
            (UnavailableReason::Missing, "missing"),
            (UnavailableReason::Timeout, "timeout"),
            (UnavailableReason::TooLarge, "too_large"),
            (UnavailableReason::ProtocolError, "protocol_error"),
        ] {
            let (dir, log) = test_log();
            let (outcome, _ran) = drive(
                "terminal",
                "ls",
                3,
                Some(unavailable_service(reason)),
                None,
                false,
                &log,
            )
            .await;
            assert!(matches!(outcome, GatedOutcome::Blocked(_)), "{word}");
            let rows = unavailable_rows(&read_rows(&dir));
            assert_eq!(rows.len(), 1, "{word}: {rows:?}");
            assert_eq!(rows[0]["reason"], word);
        }
    }

    /// D-12: inside a turn ledger, two outages leave two per-event rows and ONE
    /// `sidecar_unavailable_rollup` row with count 2. The per-event rows stay the source of
    /// truth.
    #[tokio::test]
    async fn unavailable_in_ledger_and_rollup() {
        let svc = unavailable_service(UnavailableReason::Crash);
        let (dir, log) = test_log();
        let ledger = Arc::new(YoloLedger::default());
        YOLO_LEDGER
            .scope(ledger.clone(), async {
                for _ in 0..2 {
                    drive("terminal", "ls", 3, Some(svc.clone()), None, false, &log).await;
                }
            })
            .await;

        flush_yolo_ledger(&ledger, &log, None, None).await;

        let rows = read_rows(&dir);
        assert_eq!(
            rows.iter()
                .filter(|r| r["kind"] == SIDECAR_UNAVAILABLE_KIND)
                .count(),
            2,
            "{rows:?}"
        );
        let rollups: Vec<_> = rows
            .iter()
            .filter(|r| r["kind"] == SIDECAR_UNAVAILABLE_ROLLUP_KIND)
            .collect();
        assert_eq!(rollups.len(), 1, "{rows:?}");
        assert_eq!(rollups[0]["resolution"], "rollup");
        let args: serde_json::Value =
            serde_json::from_str(rollups[0]["args"].as_str().unwrap()).unwrap();
        assert_eq!(args["count"], 2);

        // A second flush has nothing left to say.
        flush_yolo_ledger(&ledger, &log, None, None).await;
        assert_eq!(
            read_rows(&dir)
                .iter()
                .filter(|r| r["kind"] == SIDECAR_UNAVAILABLE_ROLLUP_KIND)
                .count(),
            1
        );
    }

    /// Pitfall 10: task-locals do not cross `tokio::spawn`. With no ledger in scope the
    /// per-event row is still written.
    #[tokio::test]
    async fn unavailable_outside_ledger_still_audited() {
        let (dir, log) = test_log();
        let (outcome, _ran) = drive(
            "terminal",
            "ls",
            3,
            Some(unavailable_service(UnavailableReason::Crash)),
            None,
            false,
            &log,
        )
        .await;
        assert!(matches!(outcome, GatedOutcome::Blocked(_)));
        assert_eq!(unavailable_rows(&read_rows(&dir)).len(), 1);
        assert!(
            !crate::yolo_ledger::try_record_unavailable(
                "terminal", "crash", 3, "block", "s", "cli", "c"
            ),
            "no ledger is in scope here"
        );
    }

    /// G-2: the one Blocked event carries the cause of the Block. A named rule keeps its own
    /// id; otherwise the event holds exactly one cause marker (D-03, D-12, D-16).
    #[tokio::test]
    async fn blocked_event_carries_its_cause() {
        let cases = [
            (
                "ls",
                Some(unavailable_service(UnavailableReason::Crash)),
                "crash",
                vec!["sidecar_outage:crash"],
            ),
            (
                "ls",
                Some(unavailable_service(UnavailableReason::TooLarge)),
                "too_large",
                vec!["unscreened:too_large"],
            ),
            ("rm -rf /", None, "floor", vec!["regex_floor"]),
            (
                "wrangler delete hello-world-worker",
                None,
                "rule",
                vec!["mutation_command"],
            ),
        ];
        for (cmd, svc, label, want) in cases {
            let gate = EventGate::new(ApprovalOutcome::Approved);
            let (_d, log) = test_log();
            let (outcome, ran) = drive("terminal", cmd, 3, svc, Some(&gate), false, &log).await;
            assert!(matches!(outcome, GatedOutcome::Blocked(_)), "{label}");
            assert!(!ran, "{label}");
            let blocked = gate.of_kind(SecurityEventKind::Blocked);
            assert_eq!(blocked.len(), 1, "{label}: {:?}", gate.events());
            assert_eq!(
                blocked[0].rule_ids,
                want.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "{label}"
            );
        }
    }

    /// GC-WR-03: a subject the sidecar refused reads as "could not be screened", never as
    /// "the sidecar is unavailable". A real outage keeps the outage wording. D-12 still
    /// writes the row and raises the critical event for both.
    #[tokio::test]
    async fn unscreened_tool_call_names_the_reason() {
        let gate = EventGate::new(ApprovalOutcome::Approved);
        let (dir, log) = test_log();
        let (outcome, ran) = drive(
            "terminal",
            "ls",
            3,
            Some(unavailable_service(UnavailableReason::TooLarge)),
            Some(&gate),
            false,
            &log,
        )
        .await;
        assert!(!ran);
        let GatedOutcome::Blocked(text) = &outcome else {
            panic!("expected Blocked, got {outcome:?}");
        };
        assert!(text.contains("could not be screened (too_large)"), "{text}");
        assert!(!text.contains("sidecar unavailable"), "{text}");
        let rows = read_rows(&dir);
        assert_eq!(
            unavailable_rows(&rows),
            vec![serde_json::json!({
                "reason": "too_large", "tool": "terminal", "level": 3, "verdict": "block"
            })]
        );
        let row = rows
            .iter()
            .find(|r| r["kind"] == SIDECAR_UNAVAILABLE_KIND)
            .unwrap();
        let row_reason = row["reason"].as_str().unwrap();
        assert!(
            row_reason.contains("could not be screened (too_large)"),
            "{row_reason}"
        );
        let unavailable = gate.of_kind(SecurityEventKind::SidecarUnavailable);
        assert_eq!(unavailable.len(), 1, "{:?}", gate.events());
        assert_eq!(unavailable[0].summary, "too_large; outcome blocked");
        let blocked = gate.of_kind(SecurityEventKind::Blocked);
        assert_eq!(blocked.len(), 1);
        assert_eq!(
            blocked[0].rule_ids,
            vec!["unscreened:too_large".to_string()]
        );

        // Contrast: a real outage keeps the outage wording.
        let gate = EventGate::new(ApprovalOutcome::Approved);
        let (dir, log) = test_log();
        let (outcome, _ran) = drive(
            "terminal",
            "ls",
            3,
            Some(unavailable_service(UnavailableReason::Crash)),
            Some(&gate),
            false,
            &log,
        )
        .await;
        let GatedOutcome::Blocked(text) = &outcome else {
            panic!("expected Blocked, got {outcome:?}");
        };
        assert!(text.contains("unavailable (crash)"), "{text}");
        let rows = read_rows(&dir);
        let row = rows
            .iter()
            .find(|r| r["kind"] == SIDECAR_UNAVAILABLE_KIND)
            .unwrap();
        let row_reason = row["reason"].as_str().unwrap();
        assert!(
            row_reason.contains("security sidecar unavailable (crash)"),
            "{row_reason}"
        );
    }

    /// A gate that does not override `notify_security_event` is unaffected: the default is
    /// a no-op, and the gated call behaves exactly as before.
    #[tokio::test]
    async fn notify_security_event_default_is_noop() {
        let gate = FakeGate::new(ApprovalOutcome::Approved);
        gate.notify_security_event(
            "s",
            &SecurityEvent {
                kind: SecurityEventKind::Blocked,
                tool: "terminal".to_string(),
                rule_ids: Vec::new(),
                level: 3,
                summary: "x".to_string(),
            },
        )
        .await;
        let (_dir, log) = test_log();
        let (outcome, _ran) =
            drive("terminal", "rm -rf /", 3, None, Some(&gate), false, &log).await;
        assert!(matches!(outcome, GatedOutcome::Blocked(_)));
    }

    /// D-16: Blocked, CredentialDetected, BypassRan, WarnRan and SidecarUnavailable reach
    /// the gate with the tool, rule ids and level, and never the command text.
    #[tokio::test]
    async fn security_event_reaches_gate() {
        // Blocked by the floor (the `regex_floor` cause marker) and by an ironhermes rule
        // (one rule id).
        let gate = EventGate::new(ApprovalOutcome::Approved);
        let (_d, log) = test_log();
        drive("terminal", "rm -rf /", 3, None, Some(&gate), false, &log).await;
        drive(
            "terminal",
            "wrangler delete hello-world-worker",
            3,
            None,
            Some(&gate),
            false,
            &log,
        )
        .await;
        let blocked = gate.of_kind(SecurityEventKind::Blocked);
        assert_eq!(blocked.len(), 2, "{:?}", gate.events());
        assert_eq!(
            blocked[0].rule_ids,
            vec![ironhermes_core::content_screen::REGEX_FLOOR_RULE_ID.to_string()]
        );
        assert_eq!(blocked[1].rule_ids, vec!["mutation_command".to_string()]);
        assert!(blocked.iter().all(|e| e.tool == "terminal" && e.level == 3));

        // A credential finding is reported even though the verdict is Allow.
        let fake = Arc::new(FakeBackend::new().finding_if_contains(
            "CANARY-SECRET",
            "CredentialInText",
            Severity::Low,
        ));
        let cred_gate = EventGate::new(ApprovalOutcome::Approved);
        let (_d, log) = test_log();
        let (outcome, _ran) = drive(
            "terminal",
            "echo CANARY-SECRET",
            5,
            Some(service_over(&fake)),
            Some(&cred_gate),
            false,
            &log,
        )
        .await;
        assert_eq!(outcome, GatedOutcome::Ran("ran".to_string()));
        let cred = cred_gate.of_kind(SecurityEventKind::CredentialDetected);
        assert_eq!(cred.len(), 1, "{:?}", cred_gate.events());
        assert_eq!(cred[0].rule_ids, vec!["CredentialInText".to_string()]);
        assert_eq!(cred[0].level, 5);

        // A bypass at level 5 and an approved Warn at level 3.
        let bypass_gate = EventGate::new(ApprovalOutcome::Denied);
        let (_d, log) = test_log();
        drive(
            "terminal",
            "curl https://example.com",
            5,
            None,
            Some(&bypass_gate),
            false,
            &log,
        )
        .await;
        let bypass = bypass_gate.of_kind(SecurityEventKind::BypassRan);
        assert_eq!(bypass.len(), 1, "{:?}", bypass_gate.events());
        assert!(!bypass[0].kind.is_critical());

        let warn_gate = EventGate::new(ApprovalOutcome::Approved);
        let (_d, log) = test_log();
        drive(
            "terminal",
            "curl https://example.com",
            3,
            None,
            Some(&warn_gate),
            false,
            &log,
        )
        .await;
        assert_eq!(warn_gate.of_kind(SecurityEventKind::WarnRan).len(), 1);
        assert!(warn_gate.of_kind(SecurityEventKind::BypassRan).is_empty());

        // A dead sidecar.
        let down_gate = EventGate::new(ApprovalOutcome::Approved);
        let (_d, log) = test_log();
        drive(
            "terminal",
            "ls",
            3,
            Some(unavailable_service(UnavailableReason::Missing)),
            Some(&down_gate),
            false,
            &log,
        )
        .await;
        assert_eq!(
            down_gate
                .of_kind(SecurityEventKind::SidecarUnavailable)
                .len(),
            1
        );

        // No event ever carries the command or the canary text.
        for g in [&gate, &cred_gate, &bypass_gate, &warn_gate, &down_gate] {
            for ev in g.events() {
                let json = serde_json::to_string(&ev).unwrap();
                for forbidden in ["rm -rf", "hello-world-worker", "CANARY", "curl https"] {
                    assert!(
                        !json.contains(forbidden),
                        "event leaks command text {forbidden:?}: {json}"
                    );
                }
            }
        }
    }
}
