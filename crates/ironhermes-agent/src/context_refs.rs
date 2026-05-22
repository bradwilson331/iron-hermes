//! `@`-reference expansion module for Phase 34b.
//!
//! Port of `hermes-agent/agent/context_references.py` (Phase 34b Plan 01).
//!
//! Provides:
//!   - [`ContextReference`] — a single parsed `@`-reference (kind, target, offsets, line range)
//!   - [`ContextReferenceResult`] — expansion output (message, warnings, token budget)
//!   - [`parse_context_references`] — regex-based parser matching Python's `REFERENCE_PATTERN`
//!   - [`preprocess_context_references_async`] — async expander with budget enforcement (Task 2)

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

// ---------------------------------------------------------------------------
// REFERENCE_PATTERN — mirrors Python's `REFERENCE_PATTERN` exactly.
//
// Python source (context_references.py ~line 17):
//   _QUOTED_REFERENCE_VALUE = r'(?:`[^`\n]+`|"[^"\n]+"|\'[^\'\n]+\')'
//   REFERENCE_PATTERN = re.compile(
//       rf"(?<![\w/])@(?:(?P<simple>diff|staged)\b|
//           (?P<kind>file|folder|git|url):
//           (?P<value>{_QUOTED_REFERENCE_VALUE}(?::\d+(?:-\d+)?)?|\S+))"
//   )
//
// Rust `regex` crate does not support lookbehind assertions. We emulate the
// `(?<![\w/])` negative lookbehind by prepending an optional capture group
// `(?P<pre>[\w/])` and skipping any match where `pre` is captured.
fn reference_pattern() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| {
        // Mirrors Python's REFERENCE_PATTERN. Rust regex lacks lookbehind, so we
        // prepend an optional `pre` capture group and skip matches where it fires.
        //
        // Value group matches (in priority order):
        //   1. Quoted forms (backtick/double/single) optionally followed by :N[-M]
        //   2. Unquoted non-whitespace (\S+)
        //
        // The double-quote literal must appear as a regular string char (not raw).
        let quoted_val = concat!(
            "(?:`[^`\\n]+`",    // backtick-quoted
            "|\"[^\"\\n]+\"",   // double-quoted
            "|'[^'\\n]+')",     // single-quoted
        );
        let pat = format!(
            r"(?P<pre>[\w/])?@(?:(?P<simple>diff|staged)\b|(?P<kind>file|folder|git|url):(?P<value>{quoted_val}(?::\d+(?:-\d+)?)?|\S+))"
        );
        Regex::new(&pat).expect("REFERENCE_PATTERN must compile")
    })
}

// No single regex with backreferences (regex crate doesn't support them).
// We use three separate patterns for backtick, double-quote, single-quote.
fn quoted_backtick_re() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| {
        Regex::new(r"^`(?P<path>[^`\n]+)`(?::(?P<start>\d+)(?:-(?P<end>\d+))?)?$")
            .expect("QUOTED_BACKTICK_RE must compile")
    })
}

fn quoted_double_re() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| {
        // double-quote delimited; use concat! to avoid raw-string confusion
        let pat = concat!(
            "^\"(?P<path>[^\"\n]+)\"(?::(?P<start>\\d+)(?:-(?P<end>\\d+))?)?$"
        );
        Regex::new(pat).expect("QUOTED_DOUBLE_RE must compile")
    })
}

fn quoted_single_re() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| {
        Regex::new(r"^'(?P<path>[^'\n]+)'(?::(?P<start>\d+)(?:-(?P<end>\d+))?)?$")
            .expect("QUOTED_SINGLE_RE must compile")
    })
}

fn range_re() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| {
        Regex::new(r"^(?P<path>.+?):(?P<start>\d+)(?:-(?P<end>\d+))?$")
            .expect("RANGE_RE must compile")
    })
}

fn ws_re() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| Regex::new(r"\s{2,}").unwrap())
}

fn punct_re() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| Regex::new(r"\s+([,.;:!?])").unwrap())
}

// ---------------------------------------------------------------------------
// Trailing punctuation set (mirrors Python's TRAILING_PUNCTUATION = ",.;!?").

const TRAILING_PUNCTUATION: &str = ",.;!?";

// ---------------------------------------------------------------------------
// Sensitive-path constant lists — mirrors Python's three _SENSITIVE_* tuples.

/// Directories under `$HOME` that are sensitive (any path inside is blocked).
const SENSITIVE_HOME_DIRS: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    ".kube",
    ".docker",
    ".azure",
    ".config/gh",
];

/// Exact files under `$HOME` that are sensitive.
const SENSITIVE_HOME_FILES: &[&str] = &[
    ".ssh/authorized_keys",
    ".ssh/id_rsa",
    ".ssh/id_ed25519",
    ".ssh/config",
    ".bashrc",
    ".zshrc",
    ".profile",
    ".bash_profile",
    ".zprofile",
    ".netrc",
    ".pgpass",
    ".npmrc",
    ".pypirc",
];

/// Directories under `$HERMES_HOME` that are sensitive.
const SENSITIVE_HERMES_DIRS: &[&str] = &["skills/.hub"];

// ---------------------------------------------------------------------------
// Public types — field-for-field match of Python dataclasses.

/// A single parsed `@`-reference in a user message.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextReference {
    /// The raw matched text (e.g. `@file:src/foo.rs`).
    pub raw: String,
    /// Reference kind: `"file"`, `"folder"`, `"git"`, `"url"`, `"diff"`, `"staged"`.
    pub kind: String,
    /// Resolved target (path, URL, git count string). Empty for `diff`/`staged`.
    pub target: String,
    /// Byte offset of the match start in the original message.
    pub start: usize,
    /// Byte offset of the match end in the original message.
    pub end: usize,
    /// Optional 1-based start line for `@file:` references.
    pub line_start: Option<usize>,
    /// Optional 1-based end line (inclusive) for `@file:` references.
    pub line_end: Option<usize>,
}

/// Result of expanding all `@`-references in a user message.
#[derive(Debug, Clone)]
pub struct ContextReferenceResult {
    /// The final message text (refs stripped; warnings/blocks appended).
    pub message: String,
    /// The original, unmodified message.
    pub original_message: String,
    /// All parsed references (in source order).
    pub references: Vec<ContextReference>,
    /// Expansion warnings (blocklist hits, budget violations, fetch failures).
    pub warnings: Vec<String>,
    /// Estimated tokens injected by the expanded blocks.
    pub injected_tokens: usize,
    /// `true` if any expansion or warning occurred.
    pub expanded: bool,
    /// `true` if the hard token budget was exceeded and all expansion was suppressed.
    pub blocked: bool,
}

// ---------------------------------------------------------------------------
// Parser — public API.

/// Parse all `@`-references out of `message`, returning them in source order.
///
/// Mirrors Python's `parse_context_references` function.
pub fn parse_context_references(message: &str) -> Vec<ContextReference> {
    if message.is_empty() {
        return Vec::new();
    }

    let mut refs = Vec::new();

    for cap in reference_pattern().captures_iter(message) {
        // Negative-lookbehind emulation: skip if preceded by a word char or '/'.
        if cap.name("pre").is_some() {
            continue;
        }

        let full_match = cap.get(0).unwrap();
        let raw = full_match.as_str().to_string();
        let start = full_match.start();
        let end = full_match.end();

        if let Some(simple) = cap.name("simple") {
            refs.push(ContextReference {
                raw,
                kind: simple.as_str().to_string(),
                target: String::new(),
                start,
                end,
                line_start: None,
                line_end: None,
            });
            continue;
        }

        let kind = cap.name("kind").unwrap().as_str().to_string();
        let raw_value = cap.name("value").map(|m| m.as_str()).unwrap_or("");
        let stripped_value = strip_trailing_punctuation(raw_value);

        let (target, line_start, line_end) = if kind == "file" {
            parse_file_reference_value(&stripped_value)
        } else {
            (strip_reference_wrappers(&stripped_value).to_string(), None, None)
        };

        refs.push(ContextReference {
            raw,
            kind,
            target,
            start,
            end,
            line_start,
            line_end,
        });
    }

    refs
}

// ---------------------------------------------------------------------------
// Path helpers (D-03/D-04).

/// Resolve `target` relative to `cwd`, then assert the result is within
/// `allowed_root` (fixed to cwd — no escape hatch per D-04). Returns `None`
/// if the resolved path escapes the root.
pub fn resolve_within_root(cwd: &Path, target: &str, allowed_root: &Path) -> Option<PathBuf> {
    let expanded = if target.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(&target[2..])
        } else {
            PathBuf::from(target)
        }
    } else {
        PathBuf::from(target)
    };

    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };

    let resolved = match absolute.canonicalize() {
        Ok(p) => p,
        // File may not exist yet; use lexical normalization as fallback.
        Err(_) => normalize_path(&absolute),
    };

    if resolved.starts_with(allowed_root) {
        Some(resolved)
    } else {
        None
    }
}

/// Lexically normalize a path (remove `.` / `..` components) without
/// requiring the path to exist on disk. Used as fallback in `resolve_within_root`.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            c => components.push(c),
        }
    }
    components.iter().collect()
}

/// Return `true` if `resolved` matches any entry in the sensitive-path blocklist.
///
/// Mirrors Python's `_ensure_reference_path_allowed` predicate.
pub fn is_sensitive_path(resolved: &Path, home: &Path, hermes_home: &Path) -> bool {
    // Exact-file blocklist (home files).
    for rel in SENSITIVE_HOME_FILES {
        if resolved == home.join(rel) {
            return true;
        }
    }
    // Hermes-specific exact file.
    if resolved == hermes_home.join(".env") {
        return true;
    }
    // Directory blocklist — any path inside counts.
    for dir in SENSITIVE_HOME_DIRS {
        if resolved.starts_with(home.join(dir)) {
            return true;
        }
    }
    for dir in SENSITIVE_HERMES_DIRS {
        if resolved.starts_with(hermes_home.join(dir)) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Internal helpers.

fn strip_trailing_punctuation(value: &str) -> String {
    let mut s = value
        .trim_end_matches(|c: char| TRAILING_PUNCTUATION.contains(c))
        .to_string();
    loop {
        if s.ends_with(')') || s.ends_with(']') || s.ends_with('}') {
            let closer = s.chars().last().unwrap();
            let opener = match closer {
                ')' => '(',
                ']' => '[',
                '}' => '{',
                _ => break,
            };
            let close_count = s.chars().filter(|&c| c == closer).count();
            let open_count = s.chars().filter(|&c| c == opener).count();
            if close_count > open_count {
                s.pop();
                continue;
            }
        }
        break;
    }
    s
}

fn strip_reference_wrappers(value: &str) -> &str {
    if value.len() >= 2 {
        let mut chars = value.chars();
        let first = chars.next().unwrap();
        let last = value.chars().last().unwrap();
        if first == last && (first == '`' || first == '"' || first == '\'') {
            return &value[1..value.len() - 1];
        }
        let _ = chars; // silence unused warning
    }
    value
}

/// Parse a file-reference value like `foo.rs:10-25` or `"path with spaces.rs":12-20`.
/// Returns `(target, line_start, line_end)`.
fn parse_file_reference_value(value: &str) -> (String, Option<usize>, Option<usize>) {
    // Helper: extract (path, line_start, line_end) from a regex capture with
    // named groups `path`, `start`, `end`.
    fn extract_quoted(cap: regex::Captures<'_>) -> (String, Option<usize>, Option<usize>) {
        let path = cap.name("path").unwrap().as_str().to_string();
        let line_start = cap
            .name("start")
            .and_then(|m| m.as_str().parse::<usize>().ok());
        let line_end = cap
            .name("end")
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .or(line_start);
        (path, line_start, line_end)
    }

    // Try each quoted form (backtick, double-quote, single-quote).
    if let Some(cap) = quoted_backtick_re().captures(value) {
        return extract_quoted(cap);
    }
    if let Some(cap) = quoted_double_re().captures(value) {
        return extract_quoted(cap);
    }
    if let Some(cap) = quoted_single_re().captures(value) {
        return extract_quoted(cap);
    }

    // Try unquoted range form: `path:start[-end]`
    if let Some(cap) = range_re().captures(value) {
        let path = cap.name("path").unwrap().as_str().to_string();
        let line_start = cap
            .name("start")
            .map(|m| m.as_str().parse::<usize>().unwrap());
        let line_end = cap
            .name("end")
            .map(|m| m.as_str().parse::<usize>().unwrap())
            .or(line_start);
        return (path, line_start, line_end);
    }

    // Plain value (strip wrappers if any).
    (strip_reference_wrappers(value).to_string(), None, None)
}

/// Remove `@`-reference tokens from the message, collapsing extra whitespace.
/// Mirrors Python's `_remove_reference_tokens`.
pub(crate) fn remove_reference_tokens(message: &str, refs: &[ContextReference]) -> String {
    let mut pieces: Vec<&str> = Vec::new();
    let mut cursor = 0usize;
    for r in refs {
        pieces.push(&message[cursor..r.start]);
        cursor = r.end;
    }
    pieces.push(&message[cursor..]);
    let joined = pieces.join("");
    let s = ws_re().replace_all(&joined, " ");
    let s = punct_re().replace_all(&s, "$1");
    s.trim().to_string()
}

// ---------------------------------------------------------------------------
// UrlFetcher type alias (used by Task 2 expander).

/// Injected URL fetcher for `preprocess_context_references_async`.
/// Production code passes a closure over `WebExtractTool`; tests inject hermetic fakes.
pub type UrlFetcher = Box<
    dyn Fn(
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String, String>> + Send>,
        >
        + Send
        + Sync,
>;

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::constants::get_hermes_home;

    // ── Parser tests ─────────────────────────────────────────────────────────

    /// parse "see @diff and @staged" → two refs, kinds "diff"/"staged", empty targets.
    #[test]
    fn test_parse_simple_diff_staged() {
        let refs = parse_context_references("see @diff and @staged");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].kind, "diff");
        assert_eq!(refs[0].target, "");
        assert_eq!(refs[1].kind, "staged");
        assert_eq!(refs[1].target, "");
    }

    /// parse "@file:src/foo.rs" → ContextReference{ kind:"file", target:"src/foo.rs" }
    #[test]
    fn test_parse_file_kind_value() {
        let refs = parse_context_references("look at @file:src/foo.rs here");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, "file");
        assert_eq!(refs[0].target, "src/foo.rs");
        assert!(refs[0].line_start.is_none());
        assert!(refs[0].line_end.is_none());
    }

    /// parse `@file:"path with spaces.rs":12-20` → target "path with spaces.rs", line_start 12, line_end 20.
    #[test]
    fn test_parse_quoted_file_with_line_range() {
        let refs = parse_context_references(r#"@file:"path with spaces.rs":12-20"#);
        assert_eq!(refs.len(), 1, "Expected 1 ref, got {:?}", refs);
        assert_eq!(refs[0].kind, "file");
        assert_eq!(refs[0].target, "path with spaces.rs");
        assert_eq!(refs[0].line_start, Some(12));
        assert_eq!(refs[0].line_end, Some(20));
    }

    /// "@file:foo.rs:10-25" → line_start 10, line_end 25.
    /// "@file:foo.rs:10"    → line_start 10, line_end Some(10).
    #[test]
    fn test_parse_line_range() {
        let refs = parse_context_references("@file:foo.rs:10-25");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].line_start, Some(10));
        assert_eq!(refs[0].line_end, Some(25));

        let refs2 = parse_context_references("@file:foo.rs:10");
        assert_eq!(refs2.len(), 1);
        assert_eq!(refs2[0].line_start, Some(10));
        assert_eq!(refs2[0].line_end, Some(10)); // single line: end == start
    }

    /// "look at @file:foo.rs." → target "foo.rs" (trailing '.' stripped, balanced-paren-aware).
    #[test]
    fn test_parse_trailing_punctuation_stripped() {
        let refs = parse_context_references("look at @file:foo.rs.");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].target, "foo.rs");
    }

    /// Parse a message with @file:, @folder:, @url: → three refs in source order with correct offsets.
    #[test]
    fn test_parse_multiple_refs_source_order() {
        let msg = "a @file:src/main.rs b @folder:src c @url:https://example.com d";
        let refs = parse_context_references(msg);
        assert_eq!(refs.len(), 3, "Expected 3 refs, got {:?}", refs);
        assert_eq!(refs[0].kind, "file");
        assert_eq!(refs[1].kind, "folder");
        assert_eq!(refs[2].kind, "url");
        // Verify source order by start offsets.
        assert!(refs[0].start < refs[1].start);
        assert!(refs[1].start < refs[2].start);
        // Verify byte offsets round-trip to the raw text.
        assert_eq!(&msg[refs[0].start..refs[0].end], refs[0].raw);
        assert_eq!(&msg[refs[1].start..refs[1].end], refs[1].raw);
        assert_eq!(&msg[refs[2].start..refs[2].end], refs[2].raw);
    }

    // ── Sensitive-path blocklist test ─────────────────────────────────────────

    /// Every entry in the three _SENSITIVE_* lists triggers is_sensitive_path → true.
    /// A known-safe path returns false.
    #[test]
    fn test_sensitive_path_blocklist_all_entries() {
        let home = dirs::home_dir().expect("home dir must be resolvable in tests");
        let hermes_home = get_hermes_home();

        // Exact home files (parameterised over every SENSITIVE_HOME_FILES entry).
        for rel in SENSITIVE_HOME_FILES {
            let path = home.join(rel);
            assert!(
                is_sensitive_path(&path, &home, &hermes_home),
                "Expected sensitive (exact home file): {path:?}"
            );
        }

        // Home directories — a file inside each dir should be blocked.
        for dir in SENSITIVE_HOME_DIRS {
            let path = home.join(dir).join("some_file");
            assert!(
                is_sensitive_path(&path, &home, &hermes_home),
                "Expected sensitive (home dir containment): {path:?}"
            );
        }

        // Hermes .env exact file.
        let hermes_env = hermes_home.join(".env");
        assert!(
            is_sensitive_path(&hermes_env, &home, &hermes_home),
            "Expected sensitive: {hermes_env:?}"
        );

        // Hermes skills/.hub directory containment.
        for dir in SENSITIVE_HERMES_DIRS {
            let path = hermes_home.join(dir).join("some_file");
            assert!(
                is_sensitive_path(&path, &home, &hermes_home),
                "Expected sensitive (hermes dir): {path:?}"
            );
        }

        // A non-sensitive path must return false.
        let safe = home.join("projects/myapp/src/main.rs");
        assert!(
            !is_sensitive_path(&safe, &home, &hermes_home),
            "Expected NOT sensitive: {safe:?}"
        );
    }
}
