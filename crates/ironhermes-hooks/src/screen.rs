//! `ToolScreen`: the async, per-registry screen every tool call passes through
//! (Phase 57 Plan 09, SC-1 / D-15).
//!
//! The sync [`GuardrailHook`](crate::guardrail::GuardrailHook) chain stays I/O-free
//! (research Pitfall 13). Anything that needs the sidecar lives here instead, behind one
//! async seam that `ToolRegistry` carries and every derived registry (scoped copies, RPC
//! registries, delegate children) inherits.
//!
//! # What the screen decides
//!
//! * `terminal` / `execute_code` **with** a session-scoped gated intercept: [`ScreenDecision::Skip`].
//!   Plan 07's [`execute_gated_command`](crate::gated_exec::execute_gated_command) owns that
//!   call and screens it, so each call is screened exactly once.
//! * `terminal` / `execute_code` **without** one (the cron runner's scoped registry, delegate
//!   child registries, the RPC sandbox): the screen applies the same three judges plan 07
//!   does (regex floor, ironhermes rule, sidecar), strictest wins, reusing its helpers.
//! * Everything else: the subjects [`extract_subjects`] derives from the arguments go to the
//!   sidecar at the calling session's live DEFCON level.
//!
//! A `Block` never runs. A `Warn` runs only at DEFCON 4-5 as an audited bypass, otherwise the
//! caller asks the turn's approval gate; with no gate it fails closed.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use ironhermes_core::config::DefconLevel;
use ironhermes_core::defcon::session_defcon;
use ironhermes_core::{
    ApprovalGate, AuditConfig, AuditLog, SecurityEvent, SecurityEventKind, publish_security_event,
};
use ironhermes_security::policy::{
    FloorClass, Verdict, bypassable, combine, floor_verdict, is_warn_class_tool,
};
use ironhermes_security::service::{ScreenSubject, SecurityService, SecurityVerdict};
use ironhermes_security::{SubjectKind, UnavailableReason};

use crate::gated_exec::{
    SECURITY_VERDICT_KIND, block_cause_ids, notify_event, record_sidecar_unavailable, rule_reason,
    shell_quote, sidecar_reason, sidecar_subject, verdict_reason,
};
use crate::guardrail::{
    DangerousCommandGuardrail, GuardrailDecision, GuardrailHook, McpMutationGuardrail,
    RULE_MUTATION_COMMAND, floor_class, ironhermes_rule_class, matches_mutation_command,
};

/// What the screen decided about one tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenDecision {
    /// Screened and clean.
    Allow,
    /// Needs approval; may run without one only at a bypass-active level (DEFCON 4-5) and
    /// only when `bypassable` is true.
    Warn {
        reason: String,
        rules: Vec<String>,
        bypassable: bool,
    },
    /// Never runs, at any level.
    Block { reason: String, rules: Vec<String> },
    /// Not screened here: no screen is installed, or the call is owned by a gated
    /// intercept that screens it itself (each call is screened exactly once).
    Skip,
}

/// One tool call, as the screen sees it.
#[derive(Clone, Copy)]
pub struct ToolCallSubject<'a> {
    pub tool_name: &'a str,
    pub args: &'a serde_json::Value,
    pub session_id: &'a str,
    /// True when a session-scoped gated intercept owns `terminal` / `execute_code` for this
    /// session (plan 07's path): the screen then skips the call.
    pub gated_intercept_present: bool,
    /// The turn's approval gate, when the caller has one, so events reach the surface that
    /// issued the call. `None` publishes them on the process-local bus instead.
    pub gate: Option<&'a dyn ApprovalGate>,
}

/// The async screen a `ToolRegistry` carries.
#[async_trait]
pub trait ToolScreen: Send + Sync {
    /// Decide one call.
    async fn screen(&self, call: &ToolCallSubject<'_>) -> ScreenDecision;

    /// Record how a `Warn` was resolved by the caller: `decision` is `allowed`, `approved` or
    /// `denied`; `resolution` is `bypass`, `operator`, `no_gate`, ...
    async fn record_resolution(
        &self,
        call: &ToolCallSubject<'_>,
        decision: &str,
        resolution: &str,
        reason: &str,
    );
}

/// The DEFCON level `session_id` is judged at, read live at the moment of the decision.
///
/// A session-less loop's id is a per-turn unique `__no_session__<run>` key that nothing can
/// ever set a level for, so it reads the process default instead of registering one more
/// entry per turn.
///
/// A `__delegate_unscoped__` id (see [`UNSCOPED_DELEGATE_SESSION_PREFIX`]) is judged at the
/// strictest level, DEFCON 1, whatever the process default is.
pub fn session_level(session_id: &str) -> DefconLevel {
    if session_id.starts_with(UNSCOPED_DELEGATE_SESSION_PREFIX) {
        // Fail closed (Phase 57 CR-02): a delegate child that has no delegating session in
        // scope belongs to a chat whose level is unknown, and the process default may be 4-5
        // (bypass). Resolved here, before the registry lookup, so the id is never registered
        // (no per-child entry accumulates) and a `/yolo` sent with this id cannot loosen it.
        return DefconLevel::ONE;
    }
    if session_id.starts_with("__no_session__") {
        return session_defcon().process_default();
    }
    session_defcon().handle_for(session_id).get()
}

/// Id prefix of a delegate child that started with no delegating session in scope.
/// [`session_level`] judges such an id at DEFCON 1 (the strictest level).
pub const UNSCOPED_DELEGATE_SESSION_PREFIX: &str = "__delegate_unscoped__";

tokio::task_local! {
    /// The session whose live DEFCON level governs any `delegate_task` child started inside
    /// this scope (Phase 57 CR-02 / D-09 / D-15). Installed by `AgentLoop::run` around the
    /// whole run; read by the subagent runner when it builds a child loop.
    ///
    /// It travels as a task-local, not a `SubagentRunner::run_child` parameter, so the trait
    /// (13+ test impls) stays unchanged. A task-local does NOT cross `tokio::spawn`: a caller
    /// that spawns a child (batch delegation) must capture [`delegating_session`] first and
    /// re-scope it inside the spawned future, exactly as `yolo_ledger::YOLO_LEDGER` requires.
    static DELEGATING_SESSION: String;
}

/// The delegating session in scope, or `None` outside any [`with_delegating_session`] scope.
///
/// Uses `try_with` (NOT `with`, which panics when the task-local is unset).
pub fn delegating_session() -> Option<String> {
    DELEGATING_SESSION.try_with(Clone::clone).ok()
}

/// Run `fut` with `session_id` as the delegating session. Nested scopes shadow the outer one
/// and restore it on exit.
pub async fn with_delegating_session<F: std::future::Future>(
    session_id: String,
    fut: F,
) -> F::Output {
    DELEGATING_SESSION.scope(session_id, fut).await
}

/// Where a [`SecurityToolScreen`] gets its sidecar service.
enum ServiceSource {
    /// The process-global service, looked up at call time so a `boot()` that runs after the
    /// screen was built still takes effect.
    Installed,
    /// A fixed service (tests, or a host that owns its own).
    Fixed(Arc<SecurityService>),
}

/// The web-fetch tools (Phase 57 D-25): `web_read` and `web_extract`. A clean URL to either is
/// a Warn at DEFCON 1 through [`WEB_FETCH_APPROVAL_MAX_LEVEL`], so the operator decides.
pub const WEB_FETCH_TOOLS: &[&str] = &["web_read", "web_extract"];

/// The highest DEFCON level at which a clean web fetch needs the operator's approval
/// (Phase 57 D-25, the operator's `b-strict-levels` choice).
///
/// At 1-2 D-07 already makes every Warn a human decision and no bypass exists (D-21), so the
/// strict levels also gate web fetches. At 3-5 a clean URL runs: the default level and the
/// Sliplane VPS are unchanged. A Critical URL is a Block at every level, whatever this says.
///
/// Consequence: `execute_code`'s RPC bridge has no approval gate, so a Python script that
/// calls `web_read` or `web_extract` fails closed at these levels.
///
/// Reversible: changing the decision is this one constant plus its tests.
pub const WEB_FETCH_APPROVAL_MAX_LEVEL: u8 = 2;

/// Rule id of the web-fetch Warn (D-25).
pub const RULE_WEB_FETCH_APPROVAL: &str = "web_fetch_needs_approval";

/// Surface label on every audit row the screen writes.
const SCREEN_SURFACE: &str = "tool_screen";

/// The regex floor for `terminal` / `execute_code` calls the screen judges itself. Built once;
/// it is the same type and table plan 07's gated path compiles.
static COMMAND_FLOOR: LazyLock<DangerousCommandGuardrail> =
    LazyLock::new(DangerousCommandGuardrail::new);

/// The production [`ToolScreen`]: sidecar plus the ironhermes-side rules, merged
/// strictest-wins, at the calling session's live DEFCON level.
pub struct SecurityToolScreen {
    source: ServiceSource,
    audit: AuditLog,
}

impl SecurityToolScreen {
    /// A screen over whatever service `ironhermes_security::boot` installed, if any.
    pub fn from_installed(audit_cfg: AuditConfig) -> Self {
        Self {
            source: ServiceSource::Installed,
            audit: AuditLog::load(audit_cfg),
        }
    }

    /// A screen over a fixed service.
    pub fn with_service(service: Arc<SecurityService>, audit_cfg: AuditConfig) -> Self {
        Self {
            source: ServiceSource::Fixed(service),
            audit: AuditLog::load(audit_cfg),
        }
    }

    /// Write audit rows to `audit` instead of the default `logs/audit.jsonl`.
    #[must_use]
    pub fn with_audit_log(mut self, audit: AuditLog) -> Self {
        self.audit = audit;
        self
    }

    fn service(&self) -> Option<Arc<SecurityService>> {
        match &self.source {
            ServiceSource::Installed => ironhermes_security::service::installed(),
            ServiceSource::Fixed(s) => Some(s.clone()),
        }
    }

    /// Append one `security_verdict` row: rule ids and the level only, never the arguments.
    async fn write_row(
        &self,
        call: &ToolCallSubject<'_>,
        level: u8,
        rules: &[String],
        reason: &str,
        decision: &str,
        resolution: &str,
    ) {
        let args = serde_json::json!({ "rule": rules.join(","), "rules": rules, "level": level });
        let entry = self.audit.make_entry(
            SECURITY_VERDICT_KIND,
            call.tool_name,
            None,
            call.session_id,
            SCREEN_SURFACE,
            "",
            reason,
            decision,
            resolution,
            &args,
        );
        if let Err(e) = self.audit.append(&entry).await {
            tracing::warn!(
                target: "ironhermes::security::tool_screen",
                error = %e,
                tool = %call.tool_name,
                "failed to append security_verdict audit entry"
            );
        }
    }

    /// Report one event: to the turn's gate when the caller has one, else on the
    /// process-local bus, so a Block raised outside a turn is still visible.
    async fn emit(
        &self,
        call: &ToolCallSubject<'_>,
        kind: SecurityEventKind,
        rules: &[String],
        level: u8,
        summary: String,
    ) {
        match call.gate {
            Some(gate) => {
                let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
                notify_event(
                    Some(gate),
                    call.session_id,
                    kind,
                    call.tool_name,
                    &rule_refs,
                    level,
                    summary,
                )
                .await;
            }
            None => publish_security_event(&SecurityEvent {
                kind,
                tool: call.tool_name.to_string(),
                rule_ids: rules.to_vec(),
                level,
                summary,
            }),
        }
    }

    /// Turn a merged verdict into the decision the caller acts on, writing the Block row and
    /// event. A `Warn` writes nothing here: its outcome is recorded by
    /// [`ToolScreen::record_resolution`] once the caller has resolved it.
    ///
    /// `rules` are the rule ids that go into the audit row and the decision. `cause_ids`
    /// are the cause markers (`regex_floor`, `unscreened:<reason>`, `sidecar_outage:<reason>`)
    /// that stand in for them on the Blocked event only when `rules` is empty, so the web
    /// card can name why a Block happened (G-2). They are never written to a row.
    #[allow(clippy::too_many_arguments)]
    async fn finish(
        &self,
        call: &ToolCallSubject<'_>,
        level: DefconLevel,
        final_v: Verdict,
        reason: String,
        rules: Vec<String>,
        cause_ids: Vec<String>,
        bypassable: bool,
    ) -> ScreenDecision {
        match final_v {
            Verdict::Allow => ScreenDecision::Allow,
            Verdict::Block => {
                self.write_row(call, level.level(), &rules, &reason, "blocked", "dropped")
                    .await;
                // The event names the cause when no rule did (G-2); the row keeps `rules`.
                let event_rules = if rules.is_empty() { &cause_ids } else { &rules };
                self.emit(
                    call,
                    SecurityEventKind::Blocked,
                    event_rules,
                    level.level(),
                    format!("blocked at DEFCON {}", level.level()),
                )
                .await;
                ScreenDecision::Block { reason, rules }
            }
            Verdict::Warn => ScreenDecision::Warn {
                reason,
                rules,
                bypassable,
            },
        }
    }

    /// `terminal` / `execute_code` with no gated intercept: the three judges plan 07's
    /// `execute_gated_command` uses, merged strictest-wins. Reuses its helpers (floor class,
    /// ironhermes rule, sidecar subject, reason text); no table is duplicated here.
    async fn screen_command(
        &self,
        call: &ToolCallSubject<'_>,
        level: DefconLevel,
    ) -> ScreenDecision {
        let classify_arg = command_text(call.tool_name, call.args);
        let raw_args = serde_json::json!({ "command": classify_arg });

        // 1. The regex floor (D-24).
        let decision = COMMAND_FLOOR.check(call.tool_name, &raw_args);
        let floor_cls = floor_class(&decision);
        let floor_v = floor_verdict(level, floor_cls);
        let floor_reason = match &decision {
            GuardrailDecision::Block { reason }
            | GuardrailDecision::Warn { reason }
            | GuardrailDecision::NeedsApproval { reason } => Some(reason.as_str()),
            GuardrailDecision::Allow => None,
        };
        // 2. The ironhermes-side rule (D-23 / D-13), which needs no sidecar.
        let (rule_cls, rule_id) = ironhermes_rule_class(call.tool_name, classify_arg, level);
        let rule_v = floor_verdict(level, rule_cls);
        // 3. The sidecar (D-03 / D-06).
        let sidecar: Option<SecurityVerdict> = match self.service() {
            Some(service) => {
                let subject = ScreenSubject {
                    kind: SubjectKind::Exec,
                    text: sidecar_subject(call.tool_name, classify_arg),
                    name: None,
                };
                Some(
                    service
                        .screen(&subject, level, is_warn_class_tool(call.tool_name, false))
                        .await,
                )
            }
            None => None,
        };
        let sidecar_v = sidecar.as_ref().map_or(Verdict::Allow, |v| v.verdict);

        let final_v = combine(&[floor_v, rule_v, sidecar_v]);
        let may_bypass = bypassable(floor_cls) && bypassable(rule_cls);

        // An outage is never silent (D-12), whatever the merged verdict turns out to be.
        if let Some(sc) = sidecar.as_ref()
            && let Some(why) = sc.unavailable
        {
            let outcome = match final_v {
                Verdict::Block => "blocked",
                Verdict::Warn if level.yolo_bypass_active() && may_bypass => "bypass",
                Verdict::Warn => "approval",
                Verdict::Allow => "allowed",
            };
            self.record_outage(call, level, why, sc.verdict, outcome)
                .await;
        }

        if final_v == Verdict::Allow {
            return ScreenDecision::Allow;
        }
        let reason = verdict_reason(
            final_v,
            level,
            (floor_v, floor_cls, floor_reason),
            (rule_v, rule_cls, rule_id),
            sidecar.as_ref(),
        );
        let mut rules: Vec<String> = Vec::new();
        if (rule_cls == FloorClass::Critical || rule_v > floor_v)
            && let Some(id) = rule_id
        {
            rules.push(id.to_string());
        }
        if let Some(sc) = sidecar.as_ref()
            && sc.unavailable.is_none()
        {
            for r in &sc.rules {
                if !rules.contains(r) {
                    rules.push(r.clone());
                }
            }
        }
        let cause_ids = block_cause_ids(floor_v == Verdict::Block, sidecar.as_ref());
        self.finish(call, level, final_v, reason, rules, cause_ids, may_bypass)
            .await
    }

    /// Every other tool: screen the subjects its arguments carry.
    async fn screen_generic(
        &self,
        call: &ToolCallSubject<'_>,
        level: DefconLevel,
    ) -> ScreenDecision {
        let mcp = is_mcp_tool(call.tool_name);
        // D-15: an MCP call whose ARGUMENTS carry a mutation command is Critical, whatever
        // the tool is called and whatever the sidecar says. Needs no sidecar (Pitfall 3).
        let mutation_in_args = mcp && args_carry_mutation_command(call.args);
        let subjects = extract_subjects(call.tool_name, call.args);
        if subjects.is_empty() && !mutation_in_args {
            return ScreenDecision::Skip;
        }
        // A prefixed MCP tool with a destructive verb in its name is Warn-class (D-12).
        let is_mcp_mutation = mcp && mcp_name_has_mutation_verb(call.tool_name, call.args);
        let warn_class = is_warn_class_tool(call.tool_name, is_mcp_mutation);

        let mut sidecar: Vec<SecurityVerdict> = Vec::new();
        if let Some(service) = self.service() {
            for subject in &subjects {
                sidecar.push(service.screen(subject, level, warn_class).await);
            }
        }
        let rule_v = if mutation_in_args {
            Verdict::Block
        } else {
            Verdict::Allow
        };
        // D-25: a web fetch needs the operator's approval at the strict levels. A sidecar
        // Block still wins in `combine` (D-07: Block beats everything).
        let web_fetch_v = if WEB_FETCH_TOOLS.contains(&call.tool_name)
            && level.level() <= WEB_FETCH_APPROVAL_MAX_LEVEL
        {
            Verdict::Warn
        } else {
            Verdict::Allow
        };
        let mut verdicts: Vec<Verdict> = sidecar.iter().map(|v| v.verdict).collect();
        verdicts.push(rule_v);
        verdicts.push(web_fetch_v);
        let final_v = combine(&verdicts);

        // An outage is never silent (D-12): one `sidecar_unavailable` row per event, a turn
        // ledger entry, and a critical event, with the outcome the merged verdict leads to.
        if let Some(down) = sidecar.iter().find(|v| v.unavailable.is_some())
            && let Some(why) = down.unavailable
        {
            let outcome = match final_v {
                Verdict::Block => "blocked",
                Verdict::Warn if level.yolo_bypass_active() => "bypass",
                Verdict::Warn => "approval",
                Verdict::Allow => "allowed",
            };
            self.record_outage(call, level, why, down.verdict, outcome)
                .await;
        }

        if final_v == Verdict::Allow {
            return ScreenDecision::Allow;
        }

        let mut rules: Vec<String> = Vec::new();
        let mut parts: Vec<String> = Vec::new();
        if mutation_in_args {
            rules.push(RULE_MUTATION_COMMAND.to_string());
            parts.push(rule_reason(RULE_MUTATION_COMMAND, FloorClass::Critical));
        }
        for v in &sidecar {
            for r in &v.rules {
                if !rules.contains(r) {
                    rules.push(r.clone());
                }
            }
        }
        if let Some(v) = sidecar.iter().find(|v| v.verdict == final_v) {
            parts.push(sidecar_reason(v, level));
        }
        if web_fetch_v == Verdict::Warn && final_v == Verdict::Warn {
            rules.push(RULE_WEB_FETCH_APPROVAL.to_string());
            parts.push(format!(
                "a web fetch needs the operator's approval at DEFCON {} (D-25)",
                level.level()
            ));
        }
        let reason = if parts.is_empty() {
            "security policy verdict".to_string()
        } else {
            parts.join("; ")
        };
        let cause_ids = block_cause_ids(
            false,
            sidecar
                .iter()
                .find(|v| v.verdict == Verdict::Block && v.unavailable.is_some()),
        );
        self.finish(call, level, final_v, reason, rules, cause_ids, true)
            .await
    }

    /// Record one sidecar unavailability for a registry-path call: the `sidecar_unavailable`
    /// row and turn-ledger entry (plan 07's `record_sidecar_unavailable`) and the critical
    /// event. With no turn gate the event goes to the process-local bus, so it is still seen.
    async fn record_outage(
        &self,
        call: &ToolCallSubject<'_>,
        level: DefconLevel,
        reason: UnavailableReason,
        sidecar_verdict: Verdict,
        outcome: &str,
    ) {
        record_sidecar_unavailable(
            &self.audit,
            call.gate,
            call.tool_name,
            call.session_id,
            SCREEN_SURFACE,
            "",
            level.level(),
            reason,
            sidecar_verdict,
            outcome,
        )
        .await;
        if call.gate.is_none() {
            publish_security_event(&SecurityEvent {
                kind: SecurityEventKind::SidecarUnavailable,
                tool: call.tool_name.to_string(),
                rule_ids: Vec::new(),
                level: level.level(),
                summary: format!("{reason}; outcome {outcome}"),
            });
        }
    }
}

/// The text a command tool classifies: `command` for `terminal`, `code` for `execute_code`.
fn command_text<'a>(tool_name: &str, args: &'a serde_json::Value) -> &'a str {
    let key = if tool_name == "execute_code" {
        "code"
    } else {
        "command"
    };
    args.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

/// `terminal` and `execute_code`, the two tools plan 07's gated intercept can own.
fn is_command_tool(tool_name: &str) -> bool {
    matches!(tool_name, "terminal" | "execute_code")
}

/// An MCP tool is one whose registered name carries the `server__tool` prefix.
fn is_mcp_tool(tool_name: &str) -> bool {
    tool_name.contains("__")
}

/// Tools the registry screen deliberately does not inspect, each with its reason. A tool that
/// is neither here nor screened is a gap: `every_default_tool_has_a_screen_path` fails on a
/// new default tool that is in neither place.
///
/// Only a tool that is read-only AND whose arguments carry no free text belongs here; a
/// tool that takes a query, a prompt, a URL or content is screened.
pub const SCREEN_EXEMPT_TOOLS: &[&str] = &[
    // Takes no arguments and only reads the session's todo list.
    "todo_read",
    // Read-only; its arguments are a path and a line range, not free text. Reading a file
    // changes nothing, and the sidecar has no verdict on a bare path.
    "read_file",
];

/// Every string value in `value`, depth first.
fn string_leaves(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| string_leaves(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| string_leaves(v, out)),
        _ => {}
    }
}

/// Whether any string value in `args` is a [`MUTATION_COMMAND_PATTERNS`] command line.
///
/// [`MUTATION_COMMAND_PATTERNS`]: crate::guardrail::MUTATION_COMMAND_PATTERNS
fn args_carry_mutation_command(args: &serde_json::Value) -> bool {
    let mut leaves = Vec::new();
    string_leaves(args, &mut leaves);
    leaves.iter().any(|s| matches_mutation_command(s))
}

/// The MCP destructive-verb check, the same one `McpMutationGuardrail` applies to a tool
/// name, built once with its default verbs.
static MCP_VERB_CHECK: LazyLock<McpMutationGuardrail> = LazyLock::new(McpMutationGuardrail::new);

/// Whether the prefixed MCP tool `tool_name` carries a destructive verb (`_delete`, `_purge`,
/// ...), which makes it Warn-class when the sidecar is down (D-12).
fn mcp_name_has_mutation_verb(tool_name: &str, args: &serde_json::Value) -> bool {
    matches!(
        MCP_VERB_CHECK.check(tool_name, args),
        GuardrailDecision::NeedsApproval { .. }
    )
}

fn exec_subject(text: String) -> ScreenSubject {
    ScreenSubject {
        kind: SubjectKind::Exec,
        text,
        name: None,
    }
}

fn paste_subject(text: String) -> ScreenSubject {
    ScreenSubject {
        kind: SubjectKind::Paste,
        text,
        name: None,
    }
}

/// A URL the tool will fetch, shown to the sidecar as the `curl` it effectively is (D-04d).
fn curl_subject(url: &str) -> ScreenSubject {
    exec_subject(format!("curl {}", shell_quote(url)))
}

/// File content under the target's basename, so the sidecar's file rules (which key on the
/// name, e.g. `SOUL.md`) see what is about to be written. Empty content has nothing to screen.
fn file_subject(path: Option<&str>, text: Option<&str>) -> Option<ScreenSubject> {
    let text = text.filter(|t| !t.is_empty())?;
    let name = path
        .and_then(|p| std::path::Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned());
    Some(ScreenSubject {
        kind: SubjectKind::File,
        text: text.to_string(),
        name,
    })
}

/// A free-text goal plus its context, as one paste.
fn goal_subject(goal: &str, context: Option<&str>) -> Option<ScreenSubject> {
    let context = context.filter(|c| !c.is_empty());
    match (goal.is_empty(), context) {
        (true, None) => None,
        (true, Some(c)) => Some(paste_subject(c.to_string())),
        (false, None) => Some(paste_subject(goal.to_string())),
        (false, Some(c)) => Some(paste_subject(format!("{goal}\n\n{c}"))),
    }
}

/// The subjects the sidecar is asked about for one call, and under which kind (D-04):
///
/// * MCP (`server__tool`): one exec subject, `<tool> <args as compact JSON>`, so the
///   arguments are screened and not only the name (D-15).
/// * `web_read` / `web_extract`: each URL as `curl '<url>'` (exec).
/// * `write_file` / `patch`: the new content as a file subject under the target's basename.
/// * `delegate_task` / `cronjob`: the task / prompt text as paste.
/// * `terminal` / `execute_code`: the command, or `python3 -c '<source>'`, as exec.
/// * Anything else: the string values of the arguments, joined, as one paste.
///
/// Empty when there is nothing to screen. A tool whose specific fields are missing falls back
/// to the generic paste of whatever strings its arguments do carry, so a malformed call is
/// never waved through unseen.
pub fn extract_subjects(tool_name: &str, args: &serde_json::Value) -> Vec<ScreenSubject> {
    let field = |key: &str| args.get(key).and_then(|v| v.as_str());
    let specific: Vec<ScreenSubject> = if is_mcp_tool(tool_name) {
        vec![exec_subject(format!("{tool_name} {args}"))]
    } else {
        match tool_name {
            "web_read" => field("url").map(curl_subject).into_iter().collect(),
            "web_extract" => args
                .get("urls")
                .and_then(|v| v.as_array())
                .map(|urls| {
                    urls.iter()
                        .filter_map(|u| u.as_str())
                        .map(curl_subject)
                        .collect()
                })
                .unwrap_or_default(),
            "write_file" => file_subject(field("path"), field("content"))
                .into_iter()
                .collect(),
            "patch" | "patch_file" => file_subject(
                field("path"),
                field("after")
                    .or_else(|| field("content"))
                    .or_else(|| field("new_string")),
            )
            .into_iter()
            .collect(),
            "delegate_task" => {
                let mut out: Vec<ScreenSubject> = Vec::new();
                if let Some(task) = field("task") {
                    out.extend(goal_subject(task, field("context")));
                }
                if let Some(tasks) = args.get("tasks").and_then(|v| v.as_array()) {
                    for t in tasks {
                        let goal = t.get("goal").and_then(|v| v.as_str()).unwrap_or("");
                        let context = t.get("context").and_then(|v| v.as_str());
                        out.extend(goal_subject(goal, context));
                    }
                }
                out
            }
            "cronjob" => field("prompt")
                .filter(|p| !p.is_empty())
                .map(|p| paste_subject(p.to_string()))
                .into_iter()
                .collect(),
            "terminal" | "execute_code" => {
                let text = command_text(tool_name, args);
                if text.is_empty() {
                    Vec::new()
                } else {
                    vec![exec_subject(sidecar_subject(tool_name, text))]
                }
            }
            _ => Vec::new(),
        }
    };
    if !specific.is_empty() {
        return specific;
    }
    let mut leaves = Vec::new();
    string_leaves(args, &mut leaves);
    if leaves.is_empty() {
        return Vec::new();
    }
    vec![paste_subject(leaves.join("\n"))]
}

#[async_trait]
impl ToolScreen for SecurityToolScreen {
    async fn screen(&self, call: &ToolCallSubject<'_>) -> ScreenDecision {
        if is_command_tool(call.tool_name) {
            if call.gated_intercept_present {
                return ScreenDecision::Skip;
            }
            return self
                .screen_command(call, session_level(call.session_id))
                .await;
        }
        if SCREEN_EXEMPT_TOOLS.contains(&call.tool_name) {
            return ScreenDecision::Skip;
        }
        self.screen_generic(call, session_level(call.session_id))
            .await
    }

    async fn record_resolution(
        &self,
        call: &ToolCallSubject<'_>,
        decision: &str,
        resolution: &str,
        reason: &str,
    ) {
        let level = session_level(call.session_id).level();
        self.write_row(call, level, &[], reason, decision, resolution)
            .await;
        if resolution == "bypass" {
            // Same trail a gated bypass leaves: the per-turn ledger, with the per-call notice
            // as the fail-visible fallback when no ledger is in scope.
            let recorded = crate::yolo_ledger::try_record(
                call.tool_name,
                reason,
                call.session_id,
                SCREEN_SURFACE,
                "",
            );
            if !recorded && let Some(gate) = call.gate {
                gate.notify_yolo_bypass(call.session_id, call.tool_name, reason)
                    .await;
            }
            self.emit(
                call,
                SecurityEventKind::BypassRan,
                &[],
                level,
                format!("ran without approval at DEFCON {level}"),
            )
            .await;
        } else if decision == "approved" {
            self.emit(
                call,
                SecurityEventKind::WarnRan,
                &[],
                level,
                format!("ran after approval at DEFCON {level}"),
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sessionless_loop_reads_the_process_default_without_registering_a_session() {
        let id = "__no_session__00000000-0000-0000-0000-000000000000";
        let before = session_level("screen-unit-probe-session");
        assert_eq!(
            session_level(id),
            session_defcon().process_default(),
            "a session-less id has no level of its own"
        );
        // Reading it did not create a per-id handle: setting the id afterwards must not have
        // been visible before it was set, and the probe session is unaffected.
        assert_eq!(session_level("screen-unit-probe-session"), before);
        session_defcon().forget("screen-unit-probe-session");
    }

    #[test]
    fn unscoped_delegate_session_is_strictest() {
        let id = format!("{UNSCOPED_DELEGATE_SESSION_PREFIX}x");
        session_defcon().set_process_default(DefconLevel::FIVE);
        let at_default_five = session_level(&id);
        // A `/yolo` sent with the reserved id (U-2 / WR-16) must not loosen it.
        session_defcon().set_session_level(&id, DefconLevel::FIVE);
        let after_set = session_level(&id);
        // Restore before asserting, so a failed assertion cannot leave the default at 5.
        session_defcon().set_process_default(DefconLevel::THREE);
        session_defcon().forget(&id);
        assert_eq!(
            at_default_five,
            DefconLevel::ONE,
            "an unscoped child is judged at DEFCON 1 whatever the process default is"
        );
        assert_eq!(
            after_set,
            DefconLevel::ONE,
            "a registry write for the reserved id cannot loosen it"
        );
    }

    #[tokio::test]
    async fn delegating_session_scope_round_trip() {
        assert_eq!(delegating_session(), None, "None outside any scope");
        with_delegating_session("outer".to_string(), async {
            assert_eq!(delegating_session().as_deref(), Some("outer"));
            with_delegating_session("inner".to_string(), async {
                assert_eq!(delegating_session().as_deref(), Some("inner"));
            })
            .await;
            assert_eq!(
                delegating_session().as_deref(),
                Some("outer"),
                "the outer value returns after a nested scope"
            );
        })
        .await;
        assert_eq!(delegating_session(), None, "None again after the scope");
    }

    #[test]
    fn a_tool_with_no_specific_extractor_falls_back_to_one_paste_of_its_strings() {
        let subjects = extract_subjects("some_new_tool", &json!({ "a": "one", "b": ["two", 3] }));
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0].kind, SubjectKind::Paste);
        assert!(subjects[0].text.contains("one") && subjects[0].text.contains("two"));
        assert!(extract_subjects("some_new_tool", &json!({ "n": 3 })).is_empty());
    }

    #[test]
    fn a_malformed_web_read_is_still_screened_not_waved_through() {
        // `uri` is not the field web_read reads, so the specific extractor finds nothing; the
        // generic fallback still shows the sidecar the text.
        let subjects = extract_subjects("web_read", &json!({ "uri": "http://169.254.169.254/" }));
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0].kind, SubjectKind::Paste);
    }

    #[test]
    fn mutation_check_looks_at_every_string_in_the_arguments() {
        assert!(args_carry_mutation_command(
            &json!({ "outer": { "list": ["ok", "wrangler delete prod"] } })
        ));
        assert!(!args_carry_mutation_command(
            &json!({ "text": "hello wrangler" })
        ));
        assert!(mcp_name_has_mutation_verb(
            "cloudflare_bindings__r2_bucket_delete",
            &json!({})
        ));
        assert!(!mcp_name_has_mutation_verb("demo__write_note", &json!({})));
    }

    #[test]
    fn exempt_tools_are_read_only_and_listed_once() {
        let mut seen = std::collections::HashSet::new();
        for t in SCREEN_EXEMPT_TOOLS {
            assert!(seen.insert(*t), "{t} is listed twice");
            assert!(
                !crate::screen::is_command_tool(t) && !is_mcp_tool(t),
                "{t}: command and MCP tools can never be exempt"
            );
        }
    }
}
