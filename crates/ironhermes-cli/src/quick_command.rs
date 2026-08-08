//! Phase 42 EXEC-01: Quick Command dispatch — guard → approval → TerminalTool (LLM-free).
//!
//! [`dispatch_quick_command`] is the single entry point for dispatching a `type: exec`
//! Quick Command at the CLI. It is deliberately **LLM-free**: no `AgentRuntime` parameter,
//! no model call, no round-trip to a provider.
//!
//! # Pipeline (D-11)
//!
//! ```text
//! prepare_quick_command(def, global_allowlist)
//!          │
//!          ├─ approval_need == Skip (dangerous:false)?
//!          │       └──────────────────────────────────► TerminalTool (env scrub always)
//!          │
//!          ├─ registry_guard.check("terminal", {"command": ...})
//!          │       ├─ Block (Tier-2) ─────────────────► Error — NO spawn (D-03)
//!          │       │
//!          │       ├─ Warn (Tier-1 or metachar)
//!          │       │       └─ prompt_for_approval()
//!          │       │               ├─ Deny ──────────► Error — NO spawn
//!          │       │               └─ Allow ─────────► TerminalTool (env scrub always)
//!          │       │
//!          │       └─ Allow
//!          │               ├─ approval_need == Force (dangerous:true)?
//!          │               │       └─ prompt_for_approval()
//!          │               │               ├─ Deny ──► Error — NO spawn
//!          │               │               └─ Allow ─► TerminalTool (env scrub always)
//!          │               │
//!          │               └─ Auto → TerminalTool (env scrub always, no prompt)
//! ```
//!
//! # D-11 env-scrub invariant
//!
//! `build_terminal_safe_env` (via `TerminalTool::with_env_allowlist`) is called on
//! **every branch that spawns** — including the `dangerous:false` (Skip) path that
//! bypasses the guard and approval gate. Env sanitization is never waived.
//!
//! # D-03 Tier-2 hard-block
//!
//! `DangerousCommandGuardrail::check()` returns `Block` for Tier-2 patterns before
//! any approval logic runs. The `Block` arm returns an `Err` immediately — no subprocess
//! is spawned, no approval prompt is shown, and no yolo flag can override it.

use anyhow::Result;
use ironhermes_core::{
    ApprovalNeed, ApprovalsStore, KeyKind, QuickCommandDef, prepare_quick_command,
};
use ironhermes_hooks::{DangerousCommandGuardrail, GuardrailDecision, GuardrailHook};
use ironhermes_tools::Tool;
use ironhermes_tools::terminal::TerminalTool;
use serde_json::json;

use crate::approval_gate::{ApprovalDecision, prompt_for_approval};

/// Dispatch a `type: exec` Quick Command to a subprocess.
///
/// # LLM-free (EXEC-01)
///
/// This function accepts no `AgentRuntime` or LLM client. The entire pipeline —
/// guard classification, optional approval gate, and subprocess execution — is
/// deterministic and does not call any language model.
///
/// # Parameters
///
/// - `def`: the Quick Command definition loaded from config.
/// - `registry_guard`: a pre-built `DangerousCommandGuardrail` (Plan 03).
/// - `store`: the `ApprovalsStore` for session/always caching (Plan 04).
/// - `global_allowlist`: `TerminalConfig.terminal_env_allowlist` from config (Plan 02).
/// - `yolo`: true if the user launched with `--yolo`; bypasses Tier-1 approval (D-13)
///   but never bypasses Tier-2 Block (D-03).
///
/// # Returns
///
/// `Ok(stdout+stderr)` on success, `Err` on guard Block, approval Deny, or spawn failure.
pub async fn dispatch_quick_command(
    def: &QuickCommandDef,
    registry_guard: &DangerousCommandGuardrail,
    store: &ApprovalsStore,
    global_allowlist: &[String],
    yolo: bool,
) -> Result<String> {
    // ── Step 1: Build the dispatch plan (LLM-free; pure, no I/O) ─────────────
    //
    // `prepare_quick_command` builds the sanitized env, resolves the D-11 approval
    // disposition, and sets the D-02 cache key to the command NAME (not the string).
    // The env is built here too, but we also build the union allowlist for
    // the TerminalTool call so that per-command `pass_env` is included.
    let plan = prepare_quick_command(def, global_allowlist);

    // Union allowlist: global + per-command pass_env.  TerminalTool::with_env_allowlist
    // injects this into `build_terminal_safe_env` on EVERY spawn (D-11 env-scrub always).
    let pass_env: Vec<String> = def.pass_env.as_deref().unwrap_or(&[]).to_vec();
    let union_allowlist: Vec<String> = global_allowlist
        .iter()
        .chain(pass_env.iter())
        .cloned()
        .collect();

    // ── Step 2: D-11 approval disposition resolution ──────────────────────────
    if plan.approval_need != ApprovalNeed::Skip {
        // `Skip` (dangerous:false) → operator vouches; skip guard + approval.
        // All other dispositions run the guard first.

        // EXEC-01 success criterion #1: guard runs BEFORE any spawn.
        let decision = registry_guard.check("terminal", &json!({"command": def.command}));

        match decision {
            // ── D-03: Tier-2 hard-block ───────────────────────────────────────
            // Block fires before any approval cache lookup, yolo check, or spawn.
            // No code path may reach a subprocess after a Tier-2 match.
            GuardrailDecision::Block { reason } => {
                return Err(anyhow::anyhow!(
                    "Command '{}' blocked by dangerous-command guardrail (D-03): {}",
                    def.command,
                    reason
                ));
            }

            // ── Phase 45: NeedsApproval — fail-closed (DangerousCommandGuardrail ─
            // never returns this; defensive arm for exhaustiveness / future changes)
            GuardrailDecision::NeedsApproval { reason } => {
                return Err(anyhow::anyhow!(
                    "Command '{}' requires approval (NeedsApproval guardrail): {}",
                    def.command,
                    reason
                ));
            }

            // ── Tier-1 / metachar → approval required ─────────────────────────
            GuardrailDecision::Warn { .. } => {
                let gate =
                    prompt_for_approval(&def.command, &plan.cache_key, KeyKind::Name, store, yolo)
                        .await;
                if let ApprovalDecision::Deny { reason } = gate {
                    return Err(anyhow::anyhow!(
                        "Command '{}' denied: {}",
                        def.command,
                        reason
                    ));
                }
                // Allow → fall through to execute
            }

            // ── Allow: Auto runs; Force requires an approval prompt (D-11) ────
            GuardrailDecision::Allow => {
                if plan.approval_need == ApprovalNeed::Force {
                    // dangerous:true forces approval even when the guard clears the command.
                    let gate = prompt_for_approval(
                        &def.command,
                        &plan.cache_key,
                        KeyKind::Name,
                        store,
                        yolo,
                    )
                    .await;
                    if let ApprovalDecision::Deny { reason } = gate {
                        return Err(anyhow::anyhow!(
                            "Command '{}' denied: {}",
                            def.command,
                            reason
                        ));
                    }
                    // Allow → fall through to execute
                }
                // Auto + Allow → fall through to execute (no prompt)
            }
        }
    }
    // dangerous:false (Skip) falls through here — guard + approval bypassed,
    // but the env scrub in step 3 STILL applies (D-11 invariant).

    // ── Step 3: Execute via TerminalTool with sanitized env (EXEC-03/D-11) ────
    //
    // `with_env_allowlist` installs the union allowlist; `execute()` calls
    // `env_clear()` + `build_terminal_safe_env(&self.env_allowlist, &[])` so
    // the subprocess receives only the safe allowlist — never the raw parent env.
    // This applies on EVERY branch that reaches this point, including dangerous:false.
    let tool = TerminalTool::new().with_env_allowlist(union_allowlist);
    let output = tool.execute(json!({"command": def.command})).await?;
    Ok(output)
}
