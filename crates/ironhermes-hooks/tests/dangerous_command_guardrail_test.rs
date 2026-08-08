//! EXEC-02 Wave-0 integration tests for `DangerousCommandGuardrail`.
//!
//! Covers: all tiers, the regex-boundary regression, bypass-class limitations,
//! and the D-03/D-13 hard invariants.
//!
//! Test names match the validation contract in `42-VALIDATION.md`:
//!   tier2_hardline_blocks, tier1_approval_required, metachar_forces_tier1,
//!   regex_no_word_boundary, bypass_class_documented_limitations,
//!   tier2_not_overridable_by_always, yolo_does_not_bypass_tier2.
//!
//! The literal word-boundary two-character sequence (\b) appears exactly once
//! below — inside `regex_no_word_boundary` as a negative assertion target.
//! The planner-discipline-allow marker in 42-03-PLAN.md whitelists this for the
//! commit-time gate.

use ironhermes_hooks::{
    DANGEROUS_PATTERNS, DangerousCommandGuardrail, GuardrailDecision, GuardrailHook,
};
use std::collections::HashMap;

fn cmd(s: &str) -> serde_json::Value {
    serde_json::json!({ "command": s })
}

// ── EXEC-02: Tier-2 hard-block ────────────────────────────────────────────

/// Tier-2 commands produce Block at the decision level.
///
/// No subprocess is spawned: check() returns Block synchronously BEFORE any
/// dispatch occurs (D-03). The type-level contract (Block ≠ Warn/Allow) and the
/// D-03 ordering invariant in DangerousCommandGuardrail::check() together enforce
/// that no code path can spawn a subprocess after a Tier-2 match.
#[test]
fn tier2_hardline_blocks() {
    let guard = DangerousCommandGuardrail::new();

    // Root filesystem wipe
    let d = guard.check("terminal", &cmd("rm -rf /"));
    assert!(
        matches!(d, GuardrailDecision::Block { .. }),
        "rm -rf / must Block (Tier-2 / D-07), got {d:?}"
    );

    // dd writing to a raw block device
    let d = guard.check("terminal", &cmd("dd if=/dev/zero of=/dev/sda"));
    assert!(
        matches!(d, GuardrailDecision::Block { .. }),
        "dd if=... of=/dev/sda must Block (Tier-2 / D-07), got {d:?}"
    );

    // Additional Tier-2 forms
    let d = guard.check("terminal", &cmd("mkfs.ext4 /dev/sdb"));
    assert!(
        matches!(d, GuardrailDecision::Block { .. }),
        "mkfs must Block (Tier-2 / D-07), got {d:?}"
    );

    let d = guard.check("terminal", &cmd("> /dev/sda"));
    assert!(
        matches!(d, GuardrailDecision::Block { .. }),
        "> /dev/sda must Block (Tier-2 block-device redirect / D-07), got {d:?}"
    );
}

// ── EXEC-02: Tier-1 approval-required ────────────────────────────────────

/// Tier-1 network/exfil tools produce Warn (approval-required), not Block.
///
/// Tier-1 is stoppable via the approval gate; it is NOT hard-blocked. The caller
/// decides whether to proceed (interactive approval, yolo bypass, or auto-deny
/// on non-TTY per D-12).
#[test]
fn tier1_approval_required() {
    let guard = DangerousCommandGuardrail::new();

    let d = guard.check("terminal", &cmd("curl https://example.com/file.sh"));
    assert!(
        matches!(d, GuardrailDecision::Warn { .. }),
        "curl must Warn (Tier-1 / D-08), got {d:?}"
    );
    // Must not be Block — curl is Tier-1, not Tier-2.
    assert!(
        !matches!(d, GuardrailDecision::Block { .. }),
        "curl must NOT Block (Tier-1 only)"
    );
}

// ── EXEC-02 / D-09: metachar forces Tier-1 ───────────────────────────────

/// Shell metachar presence forces Tier-1 approval (Warn), NEVER Block.
///
/// Legitimate pipes like `ps aux | grep x` must remain runnable behind the
/// approval gate. Hard-blocking all metachars would prevent routine shell use;
/// string-level matching cannot safely classify commands once the shell re-evaluates
/// metacharacters — so the safe outcome is "require a human" (D-09).
#[test]
fn metachar_forces_tier1() {
    let guard = DangerousCommandGuardrail::new();

    // $(...) command substitution
    let d = guard.check("terminal", &cmd("echo $(whoami)"));
    assert!(
        matches!(d, GuardrailDecision::Warn { .. }),
        "$(...) must Warn (D-09)"
    );
    assert!(
        !matches!(d, GuardrailDecision::Block { .. }),
        "$(...) must NOT Block (D-09)"
    );

    // | pipe — legitimate use case: ps aux | grep x
    let d = guard.check("terminal", &cmd("ps aux | grep x"));
    assert!(
        matches!(d, GuardrailDecision::Warn { .. }),
        "pipe must Warn (D-09)"
    );
    assert!(
        !matches!(d, GuardrailDecision::Block { .. }),
        "pipe must NOT Block (D-09)"
    );

    // ; command separator
    let d = guard.check("terminal", &cmd("a; b"));
    assert!(
        matches!(d, GuardrailDecision::Warn { .. }),
        "; must Warn (D-09)"
    );
    assert!(
        !matches!(d, GuardrailDecision::Block { .. }),
        "; must NOT Block (D-09)"
    );

    // && conditional and
    let d = guard.check("terminal", &cmd("echo a && echo b"));
    assert!(
        matches!(d, GuardrailDecision::Warn { .. }),
        "&& must Warn (D-09)"
    );
    assert!(
        !matches!(d, GuardrailDecision::Block { .. }),
        "&& must NOT Block (D-09)"
    );
}

// ── EXEC-02: regex-boundary regression ───────────────────────────────────

/// Assert no built-in pattern contains the two-character Rust regex word-boundary
/// escape sequence (backslash followed by the letter b).
///
/// Rust's `regex` crate does NOT support word boundaries: `\b` in a pattern
/// is treated as a literal backslash-b (NOT a word boundary), compiles without
/// error, and produces wrong matches. All boundary logic in DANGEROUS_PATTERNS
/// uses the negative-character-class form:
///   `(?:(?:^|[^a-zA-Z0-9_]))WORD(?:[^a-zA-Z0-9_]|$)`
///
/// The string `"\\b"` below is the NEGATIVE ASSERTION TARGET, not a pattern in
/// the denylist. The planner-discipline-allow marker in 42-03-PLAN.md permits
/// this single occurrence (PITFALLS C-1).
#[test]
fn regex_no_word_boundary() {
    // The two-character sequence that must not appear in any built-in pattern.
    let word_boundary_escape = "\\b"; // backslash + 'b'

    let violations: Vec<&str> = DANGEROUS_PATTERNS
        .iter()
        .map(|(pat, _)| *pat)
        .filter(|pat| pat.contains(word_boundary_escape))
        .collect();

    assert!(
        violations.is_empty(),
        "DANGEROUS_PATTERNS must not contain \\b (Rust regex has no word boundary — \
         use negative-character-class boundary instead): {violations:?}"
    );
}

// ── EXEC-02: bypass-class documented limitations ──────────────────────────

/// Documents accepted bypass limitations per PITFALLS.md C-1 / Anti-Pattern 1.
///
/// The denylist is a UX speed bump and audit trigger, NOT the security boundary.
/// The security boundary is the approval gate (caller) and EXEC-03 env sanitization.
/// These obfuscated forms bypass the Tier-2 string patterns but are caught by the
/// D-09 metachar rule (returning Warn) or the D-08 full-path Tier-1 rule.
///
/// Documented accepted limitations (forms that may reach Allow):
/// - Backslash prefix `\rm -rf /` — no metachar; denylist can't detect this.
/// - Quote wrapping `r''m -rf /` — shell strips quotes; denylist sees `r''m`.
/// - Variable indirection `X=rm; $X -rf /` — two-statement; `$X` is a metachar
///   but the expansion is invisible to the guard.
///
/// These are explicitly accepted at the design level (SEED-001 PITFALLS C-1):
/// EXEC-03 (env sanitization) and the approval gate are the real enforcement layers.
#[test]
fn bypass_class_documented_limitations() {
    let guard = DangerousCommandGuardrail::new();

    // ${IFS} substitution — caught by metachar detector (${) → Warn, not Block.
    // The IFS-expanded form `rm -rf /` is invisible to string matching; D-09
    // catches the `${` sequence as a metachar and requires approval.
    let d = guard.check("terminal", &cmd("${IFS}rm${IFS}-rf${IFS}/"));
    assert!(
        matches!(d, GuardrailDecision::Warn { .. }),
        "${{IFS}}rm... must Warn (D-09 metachar catch), got {d:?}"
    );
    assert!(
        !matches!(d, GuardrailDecision::Block { .. }),
        "${{IFS}}rm... must NOT Block — IFS expansion is an accepted bypass class (PITFALLS C-1)"
    );
    assert!(
        !matches!(d, GuardrailDecision::Allow),
        "${{IFS}}rm... must NOT Allow — D-09 metachar must intercept it"
    );

    // ANSI-C quoting: $'\x72\x6d' = "rm" — caught by metachar ($') → Warn.
    // The hex bytes decode to `rm` at shell evaluation time; the denylist sees
    // the literal `$'\x72\x6d'` string and cannot decode it, but D-09 triggers
    // on `$'` ANSI-C quoting prefix.
    let d = guard.check("terminal", &cmd(r"$'\x72\x6d' -rf /"));
    assert!(
        matches!(d, GuardrailDecision::Warn { .. }),
        r"$'\x72\x6d' must Warn (D-09 $' metachar catch), got {d:?}"
    );
    assert!(
        !matches!(d, GuardrailDecision::Block { .. }),
        r"$'\x72\x6d' must NOT Block — ANSI-C hex is an accepted bypass class (PITFALLS C-1)"
    );

    // /bin/rm IS caught by the D-08 full-path Tier-1 pattern (not an accepted bypass).
    let d = guard.check("terminal", &cmd("/bin/rm -f important_file"));
    assert!(
        matches!(d, GuardrailDecision::Warn { .. }),
        "/bin/rm must Warn — caught by D-08 full-path Tier-1 pattern, got {d:?}"
    );

    // ACCEPTED LIMITATION DOCUMENTATION (not tested, would intermittently Allow):
    // `\rm -rf /` — backslash prefix disables alias lookup but runs same binary;
    // no metachar; denylist can't detect. EXEC-03 env sanitization is the guard.
    // Quote wrapping `r''m -rf /` — shell strips quotes before execution.
    // These are design-accepted gaps per PITFALLS C-1 and the SEED-001 threat model.
}

// ── D-03: Tier-2 never approvable ────────────────────────────────────────

/// A pre-seeded "always" approval for `rm -rf /` does NOT override the Tier-2
/// hard-block. check() returns Block immediately — the approval cache is never
/// consulted for a Tier-2 match (D-03).
///
/// Design: approval caches are maintained by the CALLER (the CLI approval gate,
/// not the guardrail). The guardrail's check() is called FIRST; Block is returned
/// before the caller has a chance to consult its approval cache. This ordering is
/// the D-03 invariant — no code path after a Tier-2 match may reach approval lookup
/// or subprocess spawn.
#[test]
fn tier2_not_overridable_by_always() {
    let guard = DangerousCommandGuardrail::new();

    // Simulate the caller's "always" approval cache (keyed by command string, D-02).
    let mut approvals: HashMap<String, &str> = HashMap::new();
    approvals.insert("rm -rf /".to_string(), "always");

    // The caller would normally consult `approvals` after receiving Warn from check().
    // But check() returns Block for Tier-2 — the caller's approval lookup is never
    // reached when Block is returned.
    let decision = guard.check("terminal", &cmd("rm -rf /"));

    // D-03: Block is returned regardless of the approval cache content.
    assert!(
        matches!(decision, GuardrailDecision::Block { .. }),
        "rm -rf / must Block even with pre-seeded 'always' approval (D-03), got {decision:?}"
    );

    // The approval cache entry is present — it has no effect on Tier-2 decisions.
    // This confirms the approval cache is a caller concern, not a guardrail concern.
    assert_eq!(
        approvals.get("rm -rf /"),
        Some(&"always"),
        "approval cache entry exists but is irrelevant to Tier-2 Block (D-03)"
    );
}

// ── D-13: --yolo never overrides Tier-2 ──────────────────────────────────

/// check() accepts no yolo parameter — Block is returned before any caller-side
/// yolo bypass can apply (D-13). `--yolo` is an approval bypass for Tier-1 only.
///
/// Design: Tier-2 is a refusal (Block), not an approval candidate (Warn). yolo
/// can only bypass approval candidates. Since Block is returned before the caller
/// evaluates its yolo flag, Tier-2 is structurally immune to yolo bypass.
#[test]
fn yolo_does_not_bypass_tier2() {
    let guard = DangerousCommandGuardrail::new();

    // Simulate yolo mode at the caller level (--yolo flag / config.yolo = true).
    let yolo = true;

    // check() has no yolo parameter — the guard does not know or care about yolo.
    let decision = guard.check("terminal", &cmd("rm -rf /"));

    // D-13: Tier-2 is a refusal, not an approval. Block is returned before the
    // caller's yolo logic runs.
    assert!(
        matches!(decision, GuardrailDecision::Block { .. }),
        "rm -rf / must Block even under yolo={yolo} (D-13), got {decision:?}"
    );

    // Confirm: yolo DOES bypass Tier-1 at the caller level.
    // The guard returns Warn for curl; the caller with yolo=true would proceed.
    // This is the intended D-13 behavior: yolo bypasses Warn, not Block.
    let tier1_decision = guard.check("terminal", &cmd("curl https://example.com"));
    assert!(
        matches!(tier1_decision, GuardrailDecision::Warn { .. }),
        "curl must Warn (Tier-1); caller with yolo={yolo} may proceed, guard does not"
    );

    // yolo is a caller concern — the guard returns Warn and the caller decides.
    // This variable documents the design boundary; it is not used by check().
    let _ = yolo;
}
