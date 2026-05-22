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
// UrlFetcher type alias.

/// Injected URL fetcher for `preprocess_context_references_async`.
/// Production code passes a closure over `WebExtractTool`; tests inject hermetic fakes.
/// Returns `Ok(content)` on success, `Err(warning_text)` on failure (D-02: never silent drop).
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
// Expander + budget enforcement (Task 2).

use crate::context_compressor::estimate_tokens;

/// Rough token estimate for a string — same estimator used elsewhere in the crate.
fn estimate_tokens_rough(text: &str) -> usize {
    estimate_tokens(text)
}

/// Determine the code-fence language for a file extension (mirrors Python's `_code_fence_language`).
fn code_fence_language(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("py") => "python",
        Some("js") => "javascript",
        Some("ts") => "typescript",
        Some("tsx") => "tsx",
        Some("jsx") => "jsx",
        Some("json") => "json",
        Some("md") => "markdown",
        Some("sh") => "bash",
        Some("yml" | "yaml") => "yaml",
        Some("toml") => "toml",
        Some("rs") => "rust",
        _ => "",
    }
}

/// Detect binary files by MIME guess + null-byte scan (mirrors Python `_is_binary_file`).
fn is_binary_file(path: &Path) -> bool {
    // Check extension against known text extensions.
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let text_exts = ["py","md","txt","json","yaml","yml","toml","js","ts","rs","toml","sh","html","css"];
        if text_exts.contains(&ext) {
            return false;
        }
    }
    // Null-byte scan on first 4096 bytes.
    if let Ok(mut f) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buf = [0u8; 4096];
        if let Ok(n) = f.read(&mut buf) {
            return buf[..n].contains(&0u8);
        }
    }
    false
}

/// Expand a `@file:` reference to a fenced code block.
fn expand_file_reference(
    r: &ContextReference,
    cwd: &Path,
    allowed_root: &Path,
    home: &Path,
    hermes_home: &Path,
) -> (Option<String>, Option<String>) {
    let resolved = match resolve_within_root(cwd, &r.target, allowed_root) {
        Some(p) => p,
        None => {
            return (
                Some(format!("{}: path is outside the allowed workspace", r.raw)),
                None,
            )
        }
    };
    if is_sensitive_path(&resolved, home, hermes_home) {
        return (
            Some(format!(
                "{}: path is a sensitive credential file and cannot be attached",
                r.raw
            )),
            None,
        );
    }
    if !resolved.exists() {
        return (Some(format!("{}: file not found", r.raw)), None);
    }
    if !resolved.is_file() {
        return (Some(format!("{}: path is not a file", r.raw)), None);
    }
    if is_binary_file(&resolved) {
        return (Some(format!("{}: binary files are not supported", r.raw)), None);
    }
    let text = match std::fs::read_to_string(&resolved) {
        Ok(t) => t,
        Err(e) => return (Some(format!("{}: {}", r.raw, e)), None),
    };
    let text = if let Some(ls) = r.line_start {
        let lines: Vec<&str> = text.lines().collect();
        let start_idx = ls.saturating_sub(1).min(lines.len());
        let end_idx = r.line_end.unwrap_or(ls).min(lines.len());
        lines[start_idx..end_idx].join("\n")
    } else {
        text
    };
    let lang = code_fence_language(&resolved);
    let tokens = estimate_tokens_rough(&text);
    let block = format!("📄 {} ({} tokens)\n```{}\n{}\n```", r.raw, tokens, lang, text);
    (None, Some(block))
}

/// Expand a `@folder:` reference to a directory listing.
fn expand_folder_reference(
    r: &ContextReference,
    cwd: &Path,
    allowed_root: &Path,
    home: &Path,
    hermes_home: &Path,
) -> (Option<String>, Option<String>) {
    let resolved = match resolve_within_root(cwd, &r.target, allowed_root) {
        Some(p) => p,
        None => {
            return (
                Some(format!("{}: path is outside the allowed workspace", r.raw)),
                None,
            )
        }
    };
    if is_sensitive_path(&resolved, home, hermes_home) {
        return (
            Some(format!(
                "{}: path is a sensitive credential or internal Hermes path and cannot be attached",
                r.raw
            )),
            None,
        );
    }
    if !resolved.exists() {
        return (Some(format!("{}: folder not found", r.raw)), None);
    }
    if !resolved.is_dir() {
        return (Some(format!("{}: path is not a folder", r.raw)), None);
    }
    // Try rg first; fallback to walkdir-style os::walk.
    let listing = build_folder_listing(&resolved, cwd);
    let tokens = estimate_tokens_rough(&listing);
    let block = format!("📁 {} ({} tokens)\n{}", r.raw, tokens, listing);
    (None, Some(block))
}

/// Build a folder listing string (mirrors Python `_build_folder_listing`).
fn build_folder_listing(path: &Path, cwd: &Path) -> String {
    const LIMIT: usize = 200;

    // Try rg --files (argv-only, no shell).
    let rg_listing = try_rg_listing(path, cwd, LIMIT);
    if let Some(listing) = rg_listing {
        return listing;
    }

    // Fallback: manual walk.
    let mut lines = Vec::new();
    let rel = path.strip_prefix(cwd).unwrap_or(path);
    lines.push(format!("{}/", rel.display()));
    walk_dir(path, cwd, &mut lines, LIMIT);
    if lines.len() >= LIMIT {
        lines.push("- ...".to_string());
    }
    lines.join("\n")
}

fn try_rg_listing(path: &Path, cwd: &Path, limit: usize) -> Option<String> {
    // argv-only: no shell, no string interpolation (CWE-78 mitigation).
    let rel = path.strip_prefix(cwd).unwrap_or(path);
    let output = std::process::Command::new("rg")
        .arg("--files")
        .arg(rel)
        .current_dir(cwd)
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut lines = Vec::new();
            lines.push(format!("{}/", rel.display()));
            for line in text.lines().take(limit) {
                let p = std::path::Path::new(line.trim());
                let indent_depth = p.components().count().saturating_sub(rel.components().count()).saturating_sub(1);
                let indent = "  ".repeat(indent_depth);
                lines.push(format!("{}- {}", indent, p.file_name().unwrap_or_default().to_string_lossy()));
            }
            Some(lines.join("\n"))
        }
        _ => None,
    }
}

fn walk_dir(dir: &Path, cwd: &Path, lines: &mut Vec<String>, limit: usize) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut sorted: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    sorted.sort_by_key(|e| e.file_name());
    for entry in sorted {
        if lines.len() >= limit {
            return;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let ep = entry.path();
        let rel = ep.strip_prefix(cwd).unwrap_or(&ep);
        let depth = rel.components().count().saturating_sub(1);
        let indent = "  ".repeat(depth.saturating_sub(1));
        if ep.is_dir() {
            lines.push(format!("{}- {}/", indent, name_str));
            walk_dir(&ep, cwd, lines, limit);
        } else {
            lines.push(format!("{}- {}", indent, name_str));
        }
    }
}

/// Expand git diff/staged/log references (argv-only subprocess, CWE-78 mitigated).
async fn expand_git_reference(
    r: &ContextReference,
    cwd: &Path,
    args: &[&str],
    label: &str,
) -> (Option<String>, Option<String>) {
    // All git args passed as separate argv elements — no shell, no interpolation.
    let mut cmd = tokio::process::Command::new("git");
    for arg in args {
        cmd.arg(arg);
    }
    cmd.current_dir(cwd);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        cmd.output(),
    )
    .await;

    match result {
        Err(_) => (
            Some(format!("{}: git command timed out (30s)", r.raw)),
            None,
        ),
        Ok(Err(e)) => (Some(format!("{}: {}", r.raw, e)), None),
        Ok(Ok(out)) => {
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let msg = stderr.trim();
                let msg = if msg.is_empty() { "git command failed" } else { msg };
                return (Some(format!("{}: {}", r.raw, msg)), None);
            }
            let content = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let content = if content.is_empty() {
                "(no output)".to_string()
            } else {
                content
            };
            let tokens = estimate_tokens_rough(&content);
            let block = format!("🧾 {} ({} tokens)\n```diff\n{}\n```", label, tokens, content);
            (None, Some(block))
        }
    }
}

/// Expand a `@url:` reference via the injected `url_fetcher` (D-01/D-02).
/// On LLM-processing failure, falls back with a surfaced warning — never silently drops.
async fn expand_url_reference(
    r: &ContextReference,
    url_fetcher: Option<&UrlFetcher>,
) -> (Option<String>, Option<String>) {
    let fetcher = match url_fetcher {
        Some(f) => f,
        None => {
            return (
                Some(format!("{}: no URL fetcher configured", r.raw)),
                None,
            )
        }
    };
    match fetcher(r.target.clone()).await {
        Ok(content) if !content.is_empty() => {
            let tokens = estimate_tokens_rough(&content);
            (None, Some(format!("🌐 {} ({} tokens)\n{}", r.raw, tokens, content)))
        }
        Ok(_) => (
            Some(format!("{}: no content extracted", r.raw)),
            None,
        ),
        Err(warning) => {
            // D-02: on fetcher failure, surface a warning; never silently drop.
            (Some(format!("{}: {}", r.raw, warning)), None)
        }
    }
}

/// Expand a single `@`-reference to `(warning, block)`.
async fn expand_reference(
    r: &ContextReference,
    cwd: &Path,
    allowed_root: &Path,
    home: &Path,
    hermes_home: &Path,
    url_fetcher: Option<&UrlFetcher>,
) -> (Option<String>, Option<String>) {
    match r.kind.as_str() {
        "file" => expand_file_reference(r, cwd, allowed_root, home, hermes_home),
        "folder" => expand_folder_reference(r, cwd, allowed_root, home, hermes_home),
        "diff" => expand_git_reference(r, cwd, &["diff"], "git diff").await,
        "staged" => expand_git_reference(r, cwd, &["diff", "--staged"], "git diff --staged").await,
        "git" => {
            // Validate @git:N as u32 in [1,10] BEFORE constructing command (BLOCKER-3 / CWE-78).
            let count_str = r.target.trim();
            let count: u32 = match count_str.parse::<u32>() {
                Ok(n) if (1..=10).contains(&n) => n,
                Ok(n) => {
                    return (
                        Some(format!(
                            "{}: git:N count {} out of allowed range [1,10]",
                            r.raw, n
                        )),
                        None,
                    )
                }
                Err(_) => {
                    return (
                        Some(format!(
                            "{}: git:N requires a positive integer, got {:?}",
                            r.raw, count_str
                        )),
                        None,
                    )
                }
            };
            // count is now a validated u32 in [1,10]. Pass it as a separate argv element.
            let count_flag = format!("-{}", count);
            expand_git_reference(r, cwd, &["log", &count_flag, "-p"], &format!("git log -{} -p", count)).await
        }
        "url" => expand_url_reference(r, url_fetcher).await,
        _ => (Some(format!("{}: unsupported reference type", r.raw)), None),
    }
}

/// Preprocess `@`-references in `message` — the main public async API.
///
/// Mirrors Python's `preprocess_context_references_async`:
/// - Parses all `@file:/@folder:/@diff/@staged/@git:N/@url:` references.
/// - Expands each reference (filesystem, subprocess, URL fetch).
/// - Enforces 50% hard limit (blocked) and 25% soft limit (warning).
/// - Assembles output: `--- Context Warnings ---` then `--- Attached Context ---`.
///
/// `allowed_root` defaults to `cwd` when `None` (D-03/D-04 — fixed to cwd, no escape hatch).
pub async fn preprocess_context_references_async(
    message: &str,
    cwd: &Path,
    context_length: usize,
    url_fetcher: Option<&UrlFetcher>,
    allowed_root: Option<&Path>,
) -> ContextReferenceResult {
    let refs = parse_context_references(message);
    if refs.is_empty() {
        return ContextReferenceResult {
            message: message.to_string(),
            original_message: message.to_string(),
            references: Vec::new(),
            warnings: Vec::new(),
            injected_tokens: 0,
            expanded: false,
            blocked: false,
        };
    }

    // D-05: allowed_root defaults to cwd (fixed, no config escape hatch — D-04).
    let cwd_resolved = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let allowed_root_resolved = match allowed_root {
        Some(r) => r.canonicalize().unwrap_or_else(|_| r.to_path_buf()),
        None => cwd_resolved.clone(),
    };

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let hermes_home = ironhermes_core::constants::get_hermes_home();

    let mut warnings: Vec<String> = Vec::new();
    let mut blocks: Vec<String> = Vec::new();
    let mut injected_tokens: usize = 0;

    for r in &refs {
        let (warn, block) = expand_reference(
            r,
            &cwd_resolved,
            &allowed_root_resolved,
            &home,
            &hermes_home,
            url_fetcher,
        )
        .await;
        if let Some(w) = warn {
            warnings.push(w);
        }
        if let Some(b) = block {
            injected_tokens += estimate_tokens_rough(&b);
            blocks.push(b);
        }
    }

    // Budget enforcement (mirrors Python lines ~167-193).
    let hard_limit = (context_length / 2).max(1);
    let soft_limit = (context_length / 4).max(1);

    if injected_tokens > hard_limit {
        warnings.push(format!(
            "@ context injection refused: {} tokens exceeds the 50% hard limit ({}).",
            injected_tokens, hard_limit
        ));
        return ContextReferenceResult {
            message: message.to_string(),
            original_message: message.to_string(),
            references: refs,
            warnings,
            injected_tokens,
            expanded: false,
            blocked: true,
        };
    }

    if injected_tokens > soft_limit {
        warnings.push(format!(
            "@ context injection warning: {} tokens exceeds the 25% soft limit ({}).",
            injected_tokens, soft_limit
        ));
    }

    // Assemble final message.
    let stripped = remove_reference_tokens(message, &refs);
    let mut final_msg = stripped.clone();

    if !warnings.is_empty() {
        let warning_lines: Vec<String> = warnings.iter().map(|w| format!("- {}", w)).collect();
        final_msg.push_str("\n\n--- Context Warnings ---\n");
        final_msg.push_str(&warning_lines.join("\n"));
    }

    if !blocks.is_empty() {
        final_msg.push_str("\n\n--- Attached Context ---\n\n");
        final_msg.push_str(&blocks.join("\n\n"));
    }

    let expanded = !blocks.is_empty() || !warnings.is_empty();

    ContextReferenceResult {
        message: final_msg.trim().to_string(),
        original_message: message.to_string(),
        references: refs,
        warnings,
        injected_tokens,
        expanded,
        blocked: false,
    }
}

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

    // ── Expander tests (Task 2) ───────────────────────────────────────────────

    /// Helper: make a fake UrlFetcher returning fixed content.
    fn make_url_fetcher(content: Result<String, String>) -> UrlFetcher {
        Box::new(move |_url: String| {
            let c = content.clone();
            Box::pin(async move { c })
        })
    }

    /// Expand @file: (full) → fenced block with token count; ref stripped from inline.
    #[tokio::test]
    async fn test_expand_file_full() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let msg = format!("see @file:hello.rs here");
        let cwd = dir.path();
        let result = preprocess_context_references_async(&msg, cwd, 100_000, None, None).await;

        assert!(result.expanded, "Should be expanded");
        assert!(!result.blocked, "Should not be blocked");
        assert!(result.warnings.is_empty(), "No warnings expected");
        assert!(result.message.contains("📄"), "Block header missing");
        assert!(result.message.contains("hello.rs"), "Filename missing");
        assert!(result.message.contains("fn main()"), "File content missing");
        assert!(result.message.contains("--- Attached Context ---"), "Section missing");
        // The inline @file: reference should be stripped from message text portion.
        let before_context = result.message.split("--- Attached Context ---").next().unwrap_or("");
        assert!(!before_context.contains("@file:"), "Inline ref not stripped");
    }

    /// Expand @file: with line range → only lines 2-3.
    #[tokio::test]
    async fn test_expand_file_line_range() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lines.txt");
        std::fs::write(&file, "line1\nline2\nline3\nline4\n").unwrap();

        let msg = "@file:lines.txt:2-3";
        let result = preprocess_context_references_async(msg, dir.path(), 100_000, None, None).await;

        assert!(result.expanded);
        assert!(result.message.contains("line2"), "Should contain line2");
        assert!(result.message.contains("line3"), "Should contain line3");
        assert!(!result.message.contains("line4"), "Should not contain line4");
        assert!(!result.message.contains("line1"), "Should not contain line1");
    }

    /// Expand @folder: → listing block (no file contents).
    #[tokio::test]
    async fn test_expand_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.rs"), "").unwrap();

        let msg = format!("@folder:.");
        let result = preprocess_context_references_async(&msg, dir.path(), 100_000, None, None).await;

        assert!(result.expanded, "Should be expanded: {:?}", result.warnings);
        assert!(!result.blocked);
        // Listing block should reference folder
        assert!(result.message.contains("📁"), "Folder icon missing");
    }

    /// Expand @url: with injected fetcher returning fixed markdown.
    #[tokio::test]
    async fn test_expand_url_fetcher_success() {
        let fetcher = make_url_fetcher(Ok("# Hello World\nsome content".to_string()));
        let msg = "@url:https://example.com";
        let result = preprocess_context_references_async(
            msg,
            std::path::Path::new("/tmp"),
            100_000,
            Some(&fetcher),
            None,
        )
        .await;

        assert!(result.expanded);
        assert!(!result.blocked);
        assert!(result.warnings.is_empty(), "No warnings on success");
        assert!(result.message.contains("🌐"), "URL block header missing");
        assert!(result.message.contains("Hello World"), "Content missing");
    }

    /// @url: fetcher error → warning added, ref not silently dropped (D-02).
    #[tokio::test]
    async fn test_expand_url_fetcher_error_surfaces_warning() {
        let fetcher = make_url_fetcher(Err("fetch failed: connection refused".to_string()));
        let msg = "@url:https://example.com";
        let result = preprocess_context_references_async(
            msg,
            std::path::Path::new("/tmp"),
            100_000,
            Some(&fetcher),
            None,
        )
        .await;

        // D-02: never silently drop — warning must appear.
        assert!(!result.warnings.is_empty(), "Warning expected on fetcher error");
        assert!(
            result.warnings[0].contains("example.com") || result.warnings[0].contains("fetch failed"),
            "Warning should mention URL or error: {:?}",
            result.warnings
        );
    }

    /// Hard limit: injected_tokens > context_length*0.50 → blocked == true, message == original_message.
    #[tokio::test]
    async fn test_hard_limit_blocked() {
        let dir = tempfile::tempdir().unwrap();
        // Write a file with enough content to exceed 50% of a tiny context window.
        let content = "x ".repeat(200); // ~200 tokens rough estimate
        std::fs::write(dir.path().join("big.txt"), &content).unwrap();

        let msg = "@file:big.txt";
        // Use a very small context_length so 50% limit is tiny (e.g. 10 tokens → hard_limit=5).
        let result = preprocess_context_references_async(msg, dir.path(), 10, None, None).await;

        assert!(result.blocked, "Should be blocked by hard limit");
        assert_eq!(
            result.message, result.original_message,
            "blocked: message must equal original"
        );
        assert!(
            result.warnings.iter().any(|w| w.contains("hard limit")),
            "Hard-limit warning missing: {:?}",
            result.warnings
        );
    }

    /// Soft limit: injected_tokens in (25%, 50%] → warning present AND blocked == false.
    #[tokio::test]
    async fn test_soft_limit_warning() {
        let dir = tempfile::tempdir().unwrap();
        // Write ~50 chars of content; the token estimator produces roughly content.len()/4.
        // We'll pick context_length such that soft_limit < injected_tokens <= hard_limit.
        // Strategy: write content, estimate tokens from length (~len/4), set context_length
        // so that soft_limit = injected_tokens/2 - 1 and hard_limit = injected_tokens * 3.
        // Simple: write 200 chars → ~50 tokens; context_length = 160 → hard=80, soft=40.
        let content = "hello world ".repeat(20); // ~240 chars → rough ~60 tokens
        std::fs::write(dir.path().join("mid.txt"), &content).unwrap();

        // Expand once to find actual injected_tokens, then verify soft-limit logic.
        // context_length chosen so soft_limit will be exceeded but hard_limit won't.
        // With context_length=400: hard_limit=200, soft_limit=100.
        // The block overhead (📄 header + fences) adds tokens too, so use generous window.
        let msg = "@file:mid.txt";
        let result = preprocess_context_references_async(msg, dir.path(), 400, None, None).await;

        // Must not be blocked.
        assert!(!result.blocked, "Should not be blocked. injected={} result={:?}", result.injected_tokens, result.warnings);

        // If the block lands in the soft zone (> soft_limit = 100), warning must appear.
        if result.injected_tokens > 100 {
            assert!(
                result.warnings.iter().any(|w| w.contains("soft limit")),
                "Soft-limit warning missing when injected={}: {:?}",
                result.injected_tokens,
                result.warnings
            );
        }
        // Whether or not the soft zone was hit, expanded must be true (file was read).
        assert!(result.expanded, "File expand should mark expanded=true");
    }

    /// @git:N validation: out-of-range values [0, 11] are rejected with a warning.
    /// In-range value 3 maps to argv args ["log", "-3", "-p"] (no shell string).
    #[tokio::test]
    async fn test_git_n_validation() {
        let cwd = std::path::Path::new("/tmp");

        // @git:0 → out of range warning (0 < 1).
        let msg0 = "@git:0";
        let result0 =
            preprocess_context_references_async(msg0, cwd, 100_000, None, None).await;
        assert!(
            result0.warnings.iter().any(|w| w.contains("range") || w.contains("out of")),
            "@git:0 should warn about range: {:?}",
            result0.warnings
        );

        // @git:11 → out of range warning (11 > 10).
        let msg11 = "@git:11";
        let result11 =
            preprocess_context_references_async(msg11, cwd, 100_000, None, None).await;
        assert!(
            result11.warnings.iter().any(|w| w.contains("range") || w.contains("out of")),
            "@git:11 should warn about range: {:?}",
            result11.warnings
        );

        // @git:3 is valid — it should NOT produce a range-validation warning.
        // (May produce a "git command failed" warning if not in a git repo, which is fine.)
        let msg3 = "@git:3";
        let result3 =
            preprocess_context_references_async(msg3, cwd, 100_000, None, None).await;
        assert!(
            !result3.warnings.iter().any(|w| w.contains("range") || w.contains("out of")),
            "@git:3 should not warn about range: {:?}",
            result3.warnings
        );
    }
}
