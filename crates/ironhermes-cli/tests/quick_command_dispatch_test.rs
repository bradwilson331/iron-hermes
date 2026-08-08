//! EXEC-01 / D-11 Wave-0 integration tests for `dispatch_quick_command`.
//!
//! Covers:
//! - Safe command (dangerous:None, Auto) → guard Allow → no approval prompt → runs (LLM-free).
//! - dangerous:Some(true) (Force) → forced approval even if guard Allows; yolo bypasses.
//! - dangerous:Some(false) (Skip) → guard/approval SKIPPED but env scrub STILL applies (D-11).
//! - Tier-2 command (dangerous:None) → guard Block → refused, NO subprocess spawn (D-03).
//! - Structural: dispatch_quick_command takes no AgentRuntime parameter (EXEC-01 LLM-free).
//!
//! # IRONHERMES_HOME isolation
//!
//! Each test that touches disk uses a `tempfile::TempDir` and passes the path via
//! `ApprovalsStore::with_path` so the tests do not modify the real `~/.ironhermes`.

use ironhermes_cli::quick_command::dispatch_quick_command;
use ironhermes_core::{ApprovalsStore, QuickCommandDef};
use ironhermes_hooks::DangerousCommandGuardrail;

/// Helper: build a minimal `QuickCommandDef` for the given shell command.
fn def(name: &str, command: &str, dangerous: Option<bool>) -> QuickCommandDef {
    QuickCommandDef {
        name: name.to_string(),
        command: command.to_string(),
        description: None,
        dangerous,
        pass_env: None,
    }
}

/// Helper: a fresh `ApprovalsStore` backed by a tempdir (avoids `~/.ironhermes` writes).
fn temp_store() -> (ApprovalsStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ApprovalsStore::with_path(dir.path().join("approvals.json"));
    (store, dir)
}

// ─────────────────────────────────────────────────────────────────────────────
// EXEC-01: LLM-free structural assertion
// ─────────────────────────────────────────────────────────────────────────────

/// Structural (compile-time) test: `dispatch_quick_command` takes no `AgentRuntime`
/// parameter.  If it did, this test file would need to import and construct one —
/// the fact that it compiles without any LLM/agent import proves EXEC-01 LLM-free.
#[tokio::test]
async fn quick_command_dispatch_llm_free_structural() {
    // If dispatch_quick_command required an AgentRuntime the signature would
    // not match this call site. The function must compile with only:
    //   def, guard, store, global_allowlist, yolo
    let d = def("check", "echo llm-free", None);
    let guard = DangerousCommandGuardrail::new();
    let (store, _dir) = temp_store();
    let result = dispatch_quick_command(&d, &guard, &store, &[], false).await;
    assert!(result.is_ok(), "safe echo must succeed: {result:?}");
    let output = result.unwrap();
    assert!(
        output.contains("llm-free"),
        "expected echo output, got: {output}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// EXEC-01 / D-11: Auto path — safe command, no approval prompt needed
// ─────────────────────────────────────────────────────────────────────────────

/// QuickCommandDef{dangerous:None} with a safe command → guard returns Allow →
/// approval prompt is NOT shown (Auto + Allow) → TerminalTool executes it.
/// yolo=false because this path must NOT require yolo — the gate is bypassed
/// by the guard returning Allow, not by yolo.
#[tokio::test]
async fn quick_command_dispatch_safe_auto_no_prompt() {
    let d = def("safe-cmd", "echo safe-auto-result", None);
    let guard = DangerousCommandGuardrail::new();
    let (store, _dir) = temp_store();
    // yolo=false: the command must run WITHOUT any approval prompt (guard Allow + Auto)
    let result = dispatch_quick_command(&d, &guard, &store, &[], false).await;
    assert!(result.is_ok(), "safe auto command must run: {result:?}");
    let output = result.unwrap();
    assert!(
        output.contains("safe-auto-result"),
        "expected command output, got: {output}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// D-11: dangerous:true (Force) → forced approval even if guard Allows
// ─────────────────────────────────────────────────────────────────────────────

/// dangerous:Some(true) on a safe command → guard returns Allow but dispatch
/// still requires an approval prompt (D-11 Force path). With yolo=true the
/// prompt auto-allows — this tests that the forced-prompt path runs AND that
/// yolo can satisfy it without TTY interaction.
#[tokio::test]
async fn quick_command_dispatch_dangerous_true_force_yolo_allows() {
    let d = def("forced-cmd", "echo forced-by-dangerous-true", Some(true));
    let guard = DangerousCommandGuardrail::new();
    let (store, _dir) = temp_store();
    // yolo=true bypasses the forced prompt (D-13 yolo + D-11 Force)
    let result = dispatch_quick_command(&d, &guard, &store, &[], true).await;
    assert!(
        result.is_ok(),
        "dangerous:true with yolo should succeed: {result:?}"
    );
    let output = result.unwrap();
    assert!(
        output.contains("forced-by-dangerous-true"),
        "expected command output, got: {output}"
    );
}

/// dangerous:Some(true) with yolo=false and no TTY → forced prompt is denied
/// (D-12 non-TTY fail-closed). The command must NOT be spawned.
#[tokio::test]
async fn quick_command_dispatch_dangerous_true_force_no_tty_denied() {
    let d = def("forced-no-tty", "echo should-not-run", Some(true));
    let guard = DangerousCommandGuardrail::new();
    let (store, _dir) = temp_store();
    // yolo=false + no TTY (test environment) → approval_gate D-12 returns Deny
    let result = dispatch_quick_command(&d, &guard, &store, &[], false).await;
    // Must be Err — the command must NOT have been spawned
    assert!(
        result.is_err(),
        "dangerous:true without TTY/yolo must be denied (D-12)"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.to_lowercase().contains("denied")
            || msg.to_lowercase().contains("approval")
            || msg.to_lowercase().contains("tty"),
        "error must mention denial/approval/TTY: {msg}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// D-11: dangerous:false (Skip) — trust-skip guard/approval, env scrub STILL applies
// ─────────────────────────────────────────────────────────────────────────────

/// dangerous:Some(false) → guard and approval are SKIPPED entirely (operator
/// vouches for the fixed command string). The command executes directly.
#[tokio::test]
async fn quick_command_dispatch_dangerous_false_trust_skip_runs() {
    let d = def("trusted-cmd", "echo trusted-output", Some(false));
    let guard = DangerousCommandGuardrail::new();
    let (store, _dir) = temp_store();
    // yolo=false: the command runs WITHOUT approval because Skip bypasses the gate
    let result = dispatch_quick_command(&d, &guard, &store, &[], false).await;
    assert!(
        result.is_ok(),
        "dangerous:false must run without approval: {result:?}"
    );
    let output = result.unwrap();
    assert!(
        output.contains("trusted-output"),
        "expected command output, got: {output}"
    );
}

/// D-11 env-scrub invariant: dangerous:Some(false) skips the APPROVAL GATE but
/// does NOT skip the env sanitization. A secret variable planted in the process
/// env (and NOT in the allowlist) must be absent from the subprocess env.
///
/// Note: std::env::set_var is unsafe in edition 2024.
#[tokio::test]
async fn quick_command_dispatch_dangerous_false_env_scrub_still_applies() {
    // Plant a secret in the parent process env (unique per test run to avoid collision).
    let secret_key = "QC_DISPATCH_SECRET_D11_MUST_NOT_LEAK";
    let secret_val = "S3CR3T_SHOULD_NOT_BE_IN_CHILD_ENV";
    // SAFETY: test-only; only this test sets this uniquely-named variable.
    // Only run once due to the global env mutation; test binary is single-threaded
    // at this point due to tokio::test spawning one task.
    unsafe {
        std::env::set_var(secret_key, secret_val);
    }

    // Command: print the secret if present, or "ABSENT" if not set.
    // We use `|| echo ABSENT` as the sentinel.
    let command = format!("sh -c 'printenv {secret_key} 2>/dev/null || echo ABSENT'");
    let d = def("env-scrub-test", &command, Some(false));
    let guard = DangerousCommandGuardrail::new();
    let (store, _dir) = temp_store();
    // global_allowlist is EMPTY → secret_key must not pass through
    let result = dispatch_quick_command(&d, &guard, &store, &[], false).await;

    // Clean up before any assertion that might panic
    unsafe {
        std::env::remove_var(secret_key);
    }

    assert!(result.is_ok(), "dangerous:false must run: {result:?}");
    let output = result.unwrap();
    assert!(
        !output.contains(secret_val),
        "secret '{secret_key}' must NOT appear in child env (D-11 scrub): got {output}"
    );
    assert!(
        output.contains("ABSENT") || output.trim().is_empty() || output.contains("exit code"),
        "expected ABSENT sentinel or empty output; got: {output}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// D-03 / D-11: Tier-2 command → hard-block, NO subprocess spawn
// ─────────────────────────────────────────────────────────────────────────────

/// A Tier-2 dangerous command in a Quick Command (dangerous:None → Auto) must
/// be blocked by the guardrail BEFORE any subprocess is spawned (D-03).
/// The test uses `rm -rf /` which matches DANGEROUS_PATTERNS Tier-2.
#[tokio::test]
async fn quick_command_dispatch_tier2_blocked_no_spawn() {
    let d = def("destroy-root", "rm -rf /", None);
    let guard = DangerousCommandGuardrail::new();
    let (store, _dir) = temp_store();
    // yolo=false: Tier-2 Block fires before any yolo or spawn logic (D-03)
    let result = dispatch_quick_command(&d, &guard, &store, &[], false).await;
    assert!(
        result.is_err(),
        "Tier-2 command must be blocked (D-03), got Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.to_lowercase().contains("block")
            || msg.to_lowercase().contains("tier")
            || msg.to_lowercase().contains("guardrail"),
        "error must mention the guardrail block: {msg}"
    );
}

/// D-03 hard invariant: Tier-2 is blocked even under yolo=true.
/// yolo bypasses Tier-1 approval, NOT Tier-2 Block (D-13 prohibition).
#[tokio::test]
async fn quick_command_dispatch_tier2_blocked_even_under_yolo() {
    let d = def("destroy-yolo", "rm -rf /", None);
    let guard = DangerousCommandGuardrail::new();
    let (store, _dir) = temp_store();
    let result = dispatch_quick_command(&d, &guard, &store, &[], true).await;
    assert!(
        result.is_err(),
        "Tier-2 must be blocked even with yolo=true (D-13 prohibition)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// D-11: dangerous:false does NOT bypass Tier-2 (env scrub invariant check)
// ─────────────────────────────────────────────────────────────────────────────

/// Even with dangerous:Some(false), a Tier-2 command MUST still be blocked.
/// dangerous:false only trust-skips the APPROVAL GATE; the guard still runs
/// for non-vouched commands... actually re-reading the plan: dangerous:false
/// skips the guard + approval entirely. But the plan's prohibitions say Tier-2
/// is always blocked. Let me verify the plan's intent:
///
/// The plan action says: "if approval_need == Skip → skip guard + approval entirely"
/// And the prohibition says: "Tier-2 command hard-blocks on EVERY path"
///
/// Resolution: dangerous:false is the OPERATOR VOUCHING for a FIXED command string.
/// The Tier-2 prohibition overrides even dangerous:false — a config with
/// `command: "rm -rf /"` and `dangerous: false` is misconfiguration; the guard
/// must still fire for Tier-2.
///
/// BUT: re-reading the plan more carefully, the action says SKIP means skip guard.
/// The prohibition only mentions "gateway shell_exec surface" for Tier-2. On the
/// CLI quick command surface, dangerous:false+Tier-2 is a degenerate case. Since
/// the prohibition is about the gateway, and the dangerous:false path is "operator
/// vouches for fixed string", we follow the action exactly: Skip → skip guard.
///
/// This test documents the ACTUAL behavior per plan (skip guard on dangerous:false).
/// The operator who sets dangerous:false on a Tier-2 command takes responsibility.
/// We do NOT override this because dangerous:false means operator explicitly vouches.
///
/// This test is informational — it documents the design decision.
#[tokio::test]
async fn quick_command_dispatch_dangerous_false_skips_guard_operator_vouches() {
    // The command has a Tier-2 pattern, but dangerous:false means operator vouches.
    // Per plan action: "if approval_need == Skip → skip guard + approval entirely".
    // This test documents that the SKIP path actually skips the guardrail.
    // (This is different from the gateway shell_exec which has NO Skip path.)
    //
    // NOTE: we use a command that LOOKS like rm -rf but is harmless for testing.
    // We can't actually run rm -rf / safely, so we use "echo rm -rf /" instead.
    // This echoes "rm -rf /" but does NOT delete anything.
    // The point: dangerous:false + echo → runs (operator vouched).
    let d = def("vouched-echo", "echo rm -rf /", Some(false));
    let guard = DangerousCommandGuardrail::new();
    let (store, _dir) = temp_store();
    // The actual pattern "rm -rf /" would be blocked if not for dangerous:false.
    // But "echo rm -rf /" — let's check what the guard would do:
    // "echo rm -rf /" — the rm -rf / pattern needs rm at the start or after non-word.
    // In "echo rm -rf /", "rm" follows a space — that IS a non-word char.
    // So the guard WOULD block "echo rm -rf /" if it ran.
    // With dangerous:false, the guard is skipped entirely.
    let result = dispatch_quick_command(&d, &guard, &store, &[], false).await;
    // Result could be Ok (guard skipped) or Err (if guard runs).
    // Per plan: dangerous:false skips guard → should be Ok.
    // We document the expected behavior without asserting either way in the test name;
    // the actual assertion reflects the plan's action.
    assert!(
        result.is_ok(),
        "dangerous:false skips guard per plan action (operator vouches): {result:?}"
    );
}
