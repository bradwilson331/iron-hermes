//! CLI approval gate — interactive `[o]nce / [s]ession / [a]lways / [d]eny` prompt.
//!
//! # EXEC-04 gate flow
//!
//! [`prompt_for_approval`] is the production entry point. It composes two verified
//! primitives:
//!
//! - **[`can_prompt`]** (TTY detection from `io_gate`) — D-12 fail-closed: a non-TTY
//!   (gateway, pipe, `hermes -e`) returns [`ApprovalDecision::Deny`] without reading
//!   stdin, citing the Phase-45 chat-approval bridge.
//! - **[`ApprovalsStore`]** — session memory + `approvals.json` persistence (D-01).
//!
//! # D-03 ordering invariant (Tier-2 never reaches this gate)
//!
//! **Callers MUST check `DangerousCommandGuardrail::check()` BEFORE invoking
//! `prompt_for_approval`.** If the guardrail returns `Block` the caller must reject the
//! command immediately — this function must not be called. A pre-seeded `always` approval
//! for a Tier-2 command string cannot bypass the guardrail's `Block` because the guard is
//! checked first. Tests in `approval_gate_test.rs` (test `tier2_never_reaches_gate`) prove
//! this invariant.
//!
//! # D-13 / D-21 bypass is decided upstream
//!
//! [`CliApprovalGate`] (and the `build_gated_*_intercept` builders that wrap it) carry no yolo
//! flag since Phase 57: `ironhermes_hooks::execute_gated_command` reads the session's LIVE DEFCON
//! level through `GatePolicy::for_session(&session_id)` and bypasses a Warn only at levels 4-5,
//! writing the `resolution="bypass"` audit row and the per-turn ledger entry. A gate that IS
//! consulted always asks, so it passes `false` to [`prompt_for_approval`]. A Tier-2 `Block`
//! is never bypassed (D-03 ordering): it fires before any bypass decision.
//!
//! [`prompt_for_approval`] and [`prompt_for_approval_with_reader`] keep their `yolo` parameter
//! for their one other caller, `quick_command::dispatch_quick_command`, whose caller-supplied
//! flag is unchanged.
//!
//! # C-3 TOCTOU warning
//!
//! Before showing the `[o/s/a/d]` prompt the gate prints the literal (pre-expansion)
//! command string with a warning that shell expansion may differ. The approved key is the
//! literal string, not whatever the shell may expand it to.

use ironhermes_core::{ApprovalsStore, KeyKind};
use tracing::warn;

use crate::io_gate::can_prompt;

// ─────────────────────────────────────────────────────────────────────────────
// CliApprovalGate — Phase 36.3.12 D-08: ironhermes_core::ApprovalGate impl
// ─────────────────────────────────────────────────────────────────────────────

/// CLI/TUI implementation of `ironhermes_core::ApprovalGate`, reusing the existing
/// interactive `[o/s/a/d]` prompt so `ironhermes_hooks::execute_gated_command`
/// (Plan 03's chokepoint) can gate local-surface `terminal`/`execute_code` calls
/// through the same TTY flow Quick Commands already use.
///
/// Delegates entirely to [`prompt_for_approval`] — does NOT build a parallel prompt
/// system. `request_approval`'s `args` is expected to carry a `"command"` field for
/// display + cache-key purposes (the shape `execute_gated_command` always passes);
/// when absent (e.g. an opaque `execute_code` call, D-11) the gate falls back to
/// `reason` for both the displayed string and the cache key.
///
/// `resolve()` is a no-op returning `false`: this gate has no out-of-band pending-approval
/// mechanism (unlike the gateway's chat-native `/approve` bridge) — the interactive
/// prompt already blocks synchronously inside `request_approval` until the operator
/// answers, so there is never a "pending" approval to externally resolve.
///
/// # Session-tier lifetime (WR-01, Phase 36.3.12 Plan 10)
///
/// [`store`](Self) must be constructed via [`from_shared`](Self::from_shared) from a
/// SINGLE `ApprovalsStore` loaded once (`ApprovalsStore::load()`) and shared for the
/// lifetime of the CLI/TUI process — never a fresh `load()` per dispatch. Because
/// `ApprovalsStore::load()` always builds a brand-new, empty `session` set, a
/// per-dispatch reload silently defeats the `[s]ession` tier (it degrades to the same
/// behavior as `[o]nce`). [`build_gated_terminal_intercept`] /
/// [`build_gated_execute_code_intercept`] take the shared store as a parameter for
/// exactly this reason — do not call [`new`](Self::new) with a fresh `load()` inside a
/// per-dispatch closure body.
///
/// # Send bridging (D-08 integration constraint)
///
/// `ironhermes_core::ApprovalGate` is declared `#[async_trait]` (Send-bounded futures,
/// required so the gateway's `GatewayApprovalGate` impl is usable across spawned
/// tasks). [`prompt_for_approval`]'s future holds a raw `StdinLock` across its
/// internal `.await` points and is therefore NOT `Send`. `request_approval` bridges
/// this via `tokio::task::spawn_blocking` + `Handle::block_on` — the officially
/// documented pattern for running non-`Send` async code from a `Send`-bounded caller
/// — rather than reimplementing the prompt loop, per the plan's "do NOT build a
/// parallel prompt system" instruction.
pub struct CliApprovalGate {
    store: std::sync::Arc<ApprovalsStore>,
}

impl CliApprovalGate {
    /// `store`: the `ApprovalsStore` for session/always caching (mirrors Quick
    /// Command's `store` parameter) — `Arc`-wrapped so it can move into the
    /// `spawn_blocking` bridge closure. The gate carries no yolo or DEFCON state:
    /// bypass is decided and audited upstream by `execute_gated_command` (D-21), so
    /// a gate that is consulted always asks.
    pub fn new(store: ApprovalsStore) -> Self {
        Self {
            store: std::sync::Arc::new(store),
        }
    }

    /// Construct from an already-`Arc`-wrapped store (WR-01, Phase 36.3.12 Plan 10).
    ///
    /// Unlike [`new`](Self::new), this does NOT wrap `store` in a fresh `Arc` —
    /// it takes the caller's existing `Arc<ApprovalsStore>` directly, so every
    /// `CliApprovalGate` built from the same shared handle observes the same
    /// `session` set. Callers that hoist ONE `ApprovalsStore::load()` to a
    /// process-lifetime scope (per [`build_gated_terminal_intercept`] /
    /// [`build_gated_execute_code_intercept`]'s doc) should use this
    /// constructor at every per-dispatch gate construction so an `[s]ession`
    /// grant survives to the next dispatch instead of being lost when a fresh
    /// `ApprovalsStore::load()` reset its empty `session` set.
    pub fn from_shared(store: std::sync::Arc<ApprovalsStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl ironhermes_core::ApprovalGate for CliApprovalGate {
    async fn request_approval(
        &self,
        _session_id: &str,
        _tool_name: &str,
        reason: &str,
        args: &serde_json::Value,
    ) -> ironhermes_core::ApprovalOutcome {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or(reason)
            .to_string();
        let cache_key = ApprovalsStore::normalize_command(&command);
        let store = self.store.clone();

        // Send bridge (see struct doc): run the non-Send `prompt_for_approval`
        // future to completion on a dedicated blocking-pool thread via a nested
        // `Handle::block_on` — never on an async worker thread, so this cannot
        // trigger the "cannot start a runtime from within a runtime" panic.
        let decision = tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(prompt_for_approval(
                &command,
                &cache_key,
                KeyKind::Command,
                &store,
                // D-21: never auto-approve here. A bypass (DEFCON 4-5) is decided, audited
                // and announced upstream before this gate is ever consulted.
                false,
            ))
        })
        .await
        .unwrap_or_else(|e| {
            warn!(
                target: "ironhermes::security::approval_gate",
                error = %e,
                "approval prompt task panicked/was cancelled — failing closed"
            );
            ApprovalDecision::Deny {
                reason: format!("approval prompt task failed: {e}"),
            }
        });

        match decision {
            ApprovalDecision::Allow => ironhermes_core::ApprovalOutcome::Approved,
            ApprovalDecision::Deny { .. } => ironhermes_core::ApprovalOutcome::Denied,
        }
    }

    /// No pending-approval store exists on the CLI gate — the interactive prompt
    /// blocks synchronously in `request_approval`, so there is never anything to
    /// resolve out-of-band. Returns `false` unconditionally (idempotent no-op,
    /// mirroring the trait's "safe to call with nothing pending" contract).
    async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
        false
    }

    /// D-04 (Phase 54 Finding 1): a `yolo=true` bypass is announced on stderr —
    /// never stdout, so the notice never contaminates piped command output (the
    /// same reasoning [`prompt_for_approval`]'s doc already states for why its own
    /// interactive prompt writes to stdout only when a real operator is watching).
    ///
    /// Phase 54.3: this is the PER-CALL unscoped fallback, reached only when a
    /// bypass happens outside any `YOLO_LEDGER` turn scope. The per-turn path is
    /// [`notify_yolo_bypass_turn`](ironhermes_core::ApprovalGate::notify_yolo_bypass_turn).
    async fn notify_yolo_bypass(&self, _session_id: &str, tool_name: &str, reason: &str) {
        eprintln!("\n{}", format_yolo_bypass_notice(tool_name, reason));
    }

    /// Phase 54.3 D-03/D-09: ONE short line per bypassing turn, on stderr.
    ///
    /// Stderr, never stdout: stdout is the agent's answer channel, and
    /// interleaving a notice there corrupts piped output. The text comes from
    /// [`format_yolo_bypass_turn_notice`] (the shared `short_line()`), so it is
    /// newline-free and length-capped by construction.
    async fn notify_yolo_bypass_turn(&self, summary: &ironhermes_core::YoloBypassSummary) {
        eprintln!("\n{}", format_yolo_bypass_turn_notice(summary));
    }
}

/// Pure formatter for the D-04 yolo-bypass notice — factored out of
/// [`CliApprovalGate::notify_yolo_bypass`] so its CONTENT is unit-testable.
/// Phase 54.3: serves the per-call unscoped-fallback path when no `YOLO_LEDGER`
/// scope is installed; the per-turn path is [`format_yolo_bypass_turn_notice`].
/// Raw stderr writes have no in-process capture seam in this crate today (no
/// existing `std::io::Write` abstraction to inject a test double through), so
/// this function is the strongest testable surface available: it proves the
/// message names the tool and carries the reason, even though the test cannot
/// also prove the `eprintln!` above actually reaches stderr.
fn format_yolo_bypass_notice(tool_name: &str, reason: &str) -> String {
    format!("[YOLO BYPASS] `{tool_name}` ran without approval — reason: {reason}")
}

/// Pure formatter for the Phase 54.3 per-turn yolo-bypass notice (D-03, D-09).
///
/// Delegates to [`ironhermes_core::YoloBypassSummary::short_line`] — the single
/// shared formatter every surface consumes — so the CLI never builds its own
/// count-and-tool-list string. `short_line()` already strips `\n`/`\r` and every
/// other control character per field and caps the line at 200 chars, so the CLI
/// inherits the stderr-discipline guarantee (no forged second line from a
/// remote-supplied tool name) rather than re-deriving it.
pub fn format_yolo_bypass_turn_notice(summary: &ironhermes_core::YoloBypassSummary) -> String {
    summary.short_line()
}

#[cfg(test)]
mod yolo_bypass_notice_tests {
    use super::{format_yolo_bypass_notice, format_yolo_bypass_turn_notice};
    use ironhermes_core::{YoloBypass, YoloBypassSummary};

    /// Task 3 (D-04): the formatted notice names the tool and carries the
    /// guardrail reason. This is the strongest assertion available for the CLI
    /// surface (see `format_yolo_bypass_notice`'s doc) — it fails if the
    /// override stops composing this content, though it cannot observe the
    /// live stderr write itself.
    ///
    /// Phase 54.3: this covers the PER-CALL unscoped-fallback formatter, which
    /// is retained unchanged. The per-turn contract is asserted by the sibling
    /// tests below.
    #[test]
    fn notify_yolo_bypass_message_names_tool_and_reason() {
        let msg = format_yolo_bypass_notice("execute_code", "dangerous pattern matched");
        assert!(msg.contains("execute_code"), "must name the tool: {msg}");
        assert!(
            msg.contains("dangerous pattern matched"),
            "must contain the guardrail reason: {msg}"
        );
    }

    /// Phase 54.3 D-03/D-09: N bypasses in one turn render as ONE short line
    /// carrying the aggregate count and the tool name.
    #[test]
    fn turn_notice_is_one_short_line_naming_count_and_tools() {
        let reason = "command contains shell metacharacters — string-level matching cannot verify safety; approval required (D-09)";
        let mut summary = YoloBypassSummary::single("sess", "cli-repl", "chat", "terminal", reason);
        summary.items.push(YoloBypass {
            tool: "terminal".to_string(),
            reason: reason.to_string(),
        });
        let msg = format_yolo_bypass_turn_notice(&summary);
        assert!(msg.contains('2'), "must carry the count: {msg}");
        assert!(msg.contains("terminal"), "must name the tool: {msg}");
        assert!(!msg.contains('\n'), "must be a single line: {msg:?}");
        assert!(!msg.contains('\r'), "must be a single line: {msg:?}");
        assert!(msg.chars().count() <= 200, "must be <= 200 chars: {msg}");
    }

    /// T-54.3-13: a remote-supplied MCP tool name carrying a newline must not be
    /// able to forge a second notice line on stderr.
    #[test]
    fn turn_notice_strips_newlines_from_a_hostile_tool_name() {
        let summary = YoloBypassSummary::single(
            "sess",
            "cli-repl",
            "chat",
            "terminal\nFAKE: yolo is off",
            "reason\rwith carriage return",
        );
        let msg = format_yolo_bypass_turn_notice(&summary);
        assert!(!msg.is_empty(), "must still produce a notice");
        assert!(!msg.contains('\n'), "no embedded newline: {msg:?}");
        assert!(!msg.contains('\r'), "no embedded carriage return: {msg:?}");
        assert!(
            !msg.lines().any(|l| l.trim_start().starts_with("FAKE:")),
            "a forged line must not survive: {msg:?}"
        );
    }
}

/// Phase 57 plan 13 (D-21, T-57-68): a `CliApprovalGate` carries no yolo or DEFCON state and
/// never approves by itself. Even with the session at the highest bypass level it passes
/// `false` to `prompt_for_approval`, so on the non-TTY path it returns `Denied`.
///
/// The "no operator at a terminal" condition is pinned with the `test-support` TTY override
/// rather than assumed from how the suite was launched.
#[cfg(all(test, feature = "test-support"))]
mod never_auto_approve_tests {
    use super::*;
    use ironhermes_core::config::DefconLevel;
    use ironhermes_core::defcon::session_defcon;
    use ironhermes_core::{ApprovalGate, ApprovalOutcome};

    /// Resets the process-globals this test pins, including on a failed assertion.
    struct Pins(&'static str);

    impl Drop for Pins {
        fn drop(&mut self) {
            crate::io_gate::set_force_tty_for_test(None);
            session_defcon().forget(self.0);
        }
    }

    #[tokio::test]
    async fn cli_gate_never_auto_approves() {
        let session = "s-cli-gate-never-auto";
        let _pins = Pins(session);
        session_defcon().set_session_level(session, DefconLevel::FIVE);
        crate::io_gate::set_force_tty_for_test(Some(false));

        let dir = tempfile::tempdir().expect("tempdir");
        let gate = CliApprovalGate::from_shared(std::sync::Arc::new(ApprovalsStore::with_path(
            dir.path().join("approvals.json"),
        )));
        let outcome = gate
            .request_approval(
                session,
                "terminal",
                "reason",
                &serde_json::json!({ "command": "curl https://example.com" }),
            )
            .await;

        assert_eq!(
            outcome,
            ApprovalOutcome::Denied,
            "with no operator and the session at DEFCON 5, the gate must still deny: \
             bypass is decided upstream, never here"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 36.3.12 D-08/D-10/D-11/D-12: local-surface (CLI/TUI) gating closures
// ─────────────────────────────────────────────────────────────────────────────
//
// Shared by every local-surface `TurnRequest.terminal_intercept` /
// `execute_code_intercept` site (main.rs's 3 call sites + the TUI event loop) so the
// gating construction is defined exactly once. The gateway builds its own closures
// directly in `ironhermes-gateway::handler` (it needs `GatewayApprovalGate`, not
// `CliApprovalGate`, and already depends on `ironhermes-hooks` + `ironhermes-tools`
// the same way).
//
// Both closures reuse the ALREADY-REGISTERED tool instance
// (`AgentRuntime::terminal_tool_arc()` / `execute_code_tool_arc()`) rather than
// constructing a fresh bare tool, so `background=true` keeps its `ProcessRegistry`
// wiring (the factory always registers both tools `_with_process_registry`) —
// building a fresh tool here would silently regress background support.

/// Build a gated `terminal` intercept handler for a local (CLI/TUI) surface.
///
/// Routes every LLM-issued `terminal` call through
/// `ironhermes_hooks::execute_gated_command` (the Plan 03 chokepoint): a Tier-2
/// command is blocked before any spawn, every resolution is audited (Allow
/// included), and a remote backend / non-empty `forward_env` forces approval even on
/// an `Allow` classification.
///
/// - `tool`: the already-registered `terminal` tool (`AgentRuntime::terminal_tool_arc`).
/// - `config`: the resolved config this turn is running under.
/// - `session_id` / `surface` / `chat_id`: forwarded into the audit entry + prompt.
/// - the bypass decision is NOT a parameter (Phase 57 D-21): `execute_gated_command` reads
///   the session's live DEFCON level (`GatePolicy::for_session(&session_id)`) at each call.
/// - `approvals`: a SINGLE `ApprovalsStore` loaded once by the caller and shared
///   for the lifetime of the process (WR-01, Phase 36.3.12 Plan 10) — NOT
///   reloaded inside the per-dispatch closure body. `ApprovalsStore::load()`
///   builds a fresh, empty `session` set every time it is called; loading it
///   once here and cloning the `Arc` per dispatch is what makes the
///   `[s]ession` approval tier actually persist across dispatches instead of
///   silently degrading to `[o]nce`.
pub fn build_gated_terminal_intercept(
    tool: Option<std::sync::Arc<dyn ironhermes_tools::Tool>>,
    config: std::sync::Arc<ironhermes_core::Config>,
    session_id: String,
    surface: &'static str,
    chat_id: String,
    approvals: std::sync::Arc<ApprovalsStore>,
) -> ironhermes_tools::registry::InterceptHandler {
    std::sync::Arc::new(move |args: serde_json::Value| {
        let tool = tool.clone();
        let config = config.clone();
        let session_id = session_id.clone();
        let chat_id = chat_id.clone();
        let approvals = approvals.clone();
        Box::pin(async move {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let guard = ironhermes_hooks::DangerousCommandGuardrail::from_config(
                &config.dangerous_commands,
            );
            let audit_log = ironhermes_core::AuditLog::load(config.audit.clone());
            // WR-01: use the process-lifetime store passed in by the caller — do
            // NOT call `ApprovalsStore::load()` here, which would rebuild an
            // empty `session` set on every dispatch (the exact bug this fixes).
            let gate = CliApprovalGate::from_shared(approvals);
            let is_remote_backend = config.terminal.backend == "ssh";
            let forward_env_nonempty = !config.terminal.forward_env.is_empty();

            let outcome = ironhermes_hooks::execute_gated_command(
                "terminal",
                &command,
                &guard,
                Some(&gate),
                &audit_log,
                &session_id,
                surface,
                &chat_id,
                // D-09/D-21: the session's LIVE level and the booted sidecar, read when the call
                // is decided — never a flag captured when this closure was built.
                ironhermes_hooks::GatePolicy::for_session(&session_id),
                is_remote_backend,
                forward_env_nonempty,
                || async move {
                    match tool {
                        Some(t) => t.execute(args).await,
                        None => Err(anyhow::anyhow!(
                            "terminal tool not registered on this runtime"
                        )),
                    }
                },
            )
            .await;

            Ok(outcome.to_string())
        })
    })
}

/// Build a gated `execute_code` intercept handler for a local (CLI/TUI) surface.
///
/// Mirrors [`build_gated_terminal_intercept`] but per D-01 (Phase 54 Finding 1,
/// **overturning D-11**) passes the caller's REAL Python source as `classify_arg` —
/// `DangerousCommandGuardrail::check`'s `tool_name == "execute_code"` branch
/// classifies it against `PYTHON_DANGEROUS_PATTERNS` + the evasion-marker arm,
/// instead of the previous opaque `""` that made the branch permanently dead.
/// D-11's premise (shell patterns don't apply to Python) was correct; its
/// conclusion (classify nothing) was not. Always forwards
/// `is_remote_backend=false` / `forward_env_nonempty=false` — the Python script
/// still runs on the local `Sandbox`, never a remote backend. Every resolution —
/// including `background=true` calls — is still audited (D-08/D-12).
///
/// **Operational consequence (state here for `docs/SECURITY.md`, plan 07):** under
/// `yolo=false` on a surface with no interactive operator, a Python source matching
/// [`PYTHON_DANGEROUS_PATTERNS`] or containing a [`PYTHON_EVASION_MARKERS`] entry is
/// now DENIED where it previously ran unclassified. Under `yolo=true` it still runs
/// (audited, per D-08/D-12) — this mirrors the posture `terminal` has had since
/// Phase 36.3.12.
///
/// The resulting `raw_args = {"command": classify_arg}` (built inside
/// `execute_gated_command`) carries real Python source into the audit entry, subject
/// to `AuditConfig`'s existing redaction (`token`/`api_key`/`secret`/... substring
/// match on JSON KEYS, not values — a `"command"` key is never itself redacted) and
/// `max_args_bytes` truncation (default 2048 bytes, post-redaction, before the
/// `…(truncated)` marker). No new redaction logic is added here.
///
/// Missing or non-string `code` in the incoming args coerces to an empty
/// `classify_arg` rather than panicking — `ExecuteCodeTool::execute` already rejects
/// that case downstream with its own error.
///
/// `approvals`: see [`build_gated_terminal_intercept`]'s doc — a SINGLE
/// process-lifetime `ApprovalsStore`, loaded once by the caller and shared
/// (not reloaded per dispatch) so the `[s]ession` tier persists (WR-01).
pub fn build_gated_execute_code_intercept(
    tool: Option<std::sync::Arc<dyn ironhermes_tools::Tool>>,
    config: std::sync::Arc<ironhermes_core::Config>,
    session_id: String,
    surface: &'static str,
    chat_id: String,
    approvals: std::sync::Arc<ApprovalsStore>,
) -> ironhermes_tools::registry::InterceptHandler {
    std::sync::Arc::new(move |args: serde_json::Value| {
        let tool = tool.clone();
        let config = config.clone();
        let session_id = session_id.clone();
        let chat_id = chat_id.clone();
        let approvals = approvals.clone();
        Box::pin(async move {
            let guard = ironhermes_hooks::DangerousCommandGuardrail::from_config(
                &config.dangerous_commands,
            );
            let audit_log = ironhermes_core::AuditLog::load(config.audit.clone());
            // WR-01: shared process-lifetime store — see build_gated_terminal_intercept.
            let gate = CliApprovalGate::from_shared(approvals);

            // D-01 (overturns D-11): extract the real Python source so
            // DangerousCommandGuardrail's execute_code branch can classify it.
            // Missing/non-string `code` coerces to "" rather than panicking — the
            // tool itself already rejects that case downstream.
            let classify_arg = args
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let outcome = ironhermes_hooks::execute_gated_command(
                "execute_code",
                &classify_arg,
                &guard,
                Some(&gate),
                &audit_log,
                &session_id,
                surface,
                &chat_id,
                // D-09/D-21: the session's LIVE level and the booted sidecar, read when the call
                // is decided — never a flag captured when this closure was built.
                ironhermes_hooks::GatePolicy::for_session(&session_id),
                false, // execute_code never routes to a remote backend
                false, // execute_code never forwards credentials cross-boundary
                || async move {
                    match tool {
                        Some(t) => t.execute(args).await,
                        None => Err(anyhow::anyhow!(
                            "execute_code tool not registered on this runtime"
                        )),
                    }
                },
            )
            .await;

            Ok(outcome.to_string())
        })
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// ApprovalDecision
// ─────────────────────────────────────────────────────────────────────────────

/// The outcome of the interactive approval gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// The command is allowed to proceed.
    Allow,
    /// The command was rejected. The caller must not spawn a subprocess.
    Deny {
        /// Human-readable reason (suitable for logging or display to the operator).
        reason: String,
    },
}

impl std::fmt::Display for ApprovalDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allow => write!(f, "Allow"),
            Self::Deny { reason } => write!(f, "Deny({reason})"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public gate entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Ask the operator whether to allow `command`.
///
/// # Ordering invariant (D-03)
///
/// This function **must only be called if** `DangerousCommandGuardrail::check()` returned
/// `Allow` or `Warn`. If the guard returned `Block` the caller must reject without calling
/// this function — a `Block` result is not overridable by any cached approval or yolo flag.
///
/// # Yolo (D-13)
///
/// `yolo=true` auto-allows immediately without reading stdin or the store.
///
/// # Non-TTY / gateway (D-12)
///
/// When `can_prompt(false)` returns `false` (no TTY) the gate returns
/// [`ApprovalDecision::Deny`] with the Phase-45 bridge message. No stdin read occurs.
pub async fn prompt_for_approval(
    command: &str,
    cache_key: &str,
    key_kind: KeyKind,
    store: &ApprovalsStore,
    yolo: bool,
) -> ApprovalDecision {
    let stdin_h = std::io::stdin();
    let mut reader = stdin_h.lock();
    prompt_for_approval_with_reader(
        command,
        cache_key,
        key_kind,
        store,
        yolo,
        can_prompt(false),
        &mut reader,
    )
    .await
}

// ─────────────────────────────────────────────────────────────────────────────
// Testable inner gate (public for integration-test access)
// ─────────────────────────────────────────────────────────────────────────────

/// Testable variant of [`prompt_for_approval`] with an injectable stdin reader and TTY flag.
///
/// # Parameters
///
/// - `can_prompt_val`: replaces the live `can_prompt(false)` call. Pass `false` to simulate
///   a non-TTY environment (D-12 fail-closed test). Pass `true` to test the interactive paths.
/// - `reader`: source of the operator's single-character choice (`o/s/a/d`). In production
///   this is real stdin; in tests use [`std::io::Cursor<&[u8]>`].
///
/// # Note
///
/// `#[doc(hidden)]` — this function is `pub` solely for integration-test access.
/// Use [`prompt_for_approval`] in production code.
#[doc(hidden)]
pub async fn prompt_for_approval_with_reader<R: std::io::BufRead>(
    command: &str,
    cache_key: &str,
    key_kind: KeyKind,
    store: &ApprovalsStore,
    yolo: bool,
    can_prompt_val: bool,
    reader: &mut R,
) -> ApprovalDecision {
    // ── D-13: yolo auto-allows Tier-1 without reading stdin or checking the store ──
    // (Tier-2 is Blocked upstream by DangerousCommandGuardrail — it never reaches here;
    //  see D-03 ordering invariant in module-level doc.)
    if yolo {
        return ApprovalDecision::Allow;
    }

    // ── D-12: non-TTY → fail-closed ──────────────────────────────────────────────
    // Gateway, pipe, and non-interactive (`hermes -e`) contexts have no operator to
    // respond to the prompt. Deny immediately and direct the operator to the Phase-45
    // chat-approval bridge, which provides out-of-band approval without a TTY.
    if !can_prompt_val {
        return ApprovalDecision::Deny {
            reason: "Approval required but the interactive gate is unavailable (no TTY). \
                     Phase 45 adds chat-native approval as an alternative to the live prompt."
                .to_string(),
        };
    }

    // ── Session cache: already approved this command for the current session ──────
    if store.is_session_approved(cache_key).await {
        return ApprovalDecision::Allow;
    }

    // ── Always cache: operator previously granted persistent approval ─────────────
    if store.is_always_approved(cache_key, key_kind).await {
        return ApprovalDecision::Allow;
    }

    // ── C-3 TOCTOU warning + interactive prompt ──────────────────────────────────
    //
    // Print the literal (pre-expansion) command string before asking for a decision.
    // Shell expansion of `$VAR`, `$(...)`, globs, etc. is NOT previewed — only this
    // exact string is what gets approved and stored as the cache key.
    println!(
        "\n[APPROVAL REQUIRED] Shell expansion is not previewed — the literal string below \
         is the only thing being approved:"
    );
    println!("  {command}");
    print!("Allow this command?  [o]nce  [s]ession  [a]lways  [d]eny  > ");
    // Flush stdout so the prompt appears before we block on the reader.
    {
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
    }

    // Read one line; take the first non-whitespace character as the operator's choice.
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    let choice = line.trim().chars().next().unwrap_or('d');

    match choice {
        // ── Once: allow this single invocation ───────────────────────────────────
        'o' | 'O' => ApprovalDecision::Allow,

        // ── Session: remember for the lifetime of this process ───────────────────
        's' | 'S' => {
            store.approve_session(cache_key).await;
            ApprovalDecision::Allow
        }

        // ── Always: persist to approvals.json at 0600 ────────────────────────────
        'a' | 'A' => {
            store.approve_always(cache_key, key_kind).await;
            if let Err(e) = store.save_to_disk().await {
                warn!(
                    target: "ironhermes::security::approval_gate",
                    cache_key = %cache_key,
                    "failed to persist always-approval to disk: {e}"
                );
            }
            ApprovalDecision::Allow
        }

        // ── Deny (default): reject; caller must not spawn a subprocess ────────────
        _ => ApprovalDecision::Deny {
            reason: format!("Command '{command}' was denied by the operator"),
        },
    }
}
