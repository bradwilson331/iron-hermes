//! Security primitives for the skills.sh install pipeline (Phase 21.8).
//!
//! All functions in this module are PURE (no I/O) except `is_path_safe` /
//! `is_contained_in` / `assert_temp_contained` which take paths and may
//! canonicalize them. This module implements D-16 (terminal escape
//! stripping), D-17 (YAML-only frontmatter), D-18 (path traversal guards),
//! D-20 (temp dir containment).
//!
//! All functions handling server-originated strings MUST run through this
//! module before any filesystem write, YAML parse, or terminal print.

use crate::{HubError, HubErrorKind};
use regex::Regex;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

// SP-1: per-module private typed() helper (matches tarball.rs:16-23 verbatim).
fn typed(kind: HubErrorKind, msg: impl Into<String>) -> HubError {
    HubError::Typed {
        kind,
        message: msg.into(),
        suggestion: None,
        retry_after_s: None,
    }
}

// ============================================================================
// D-16: strip_terminal_escapes (ports sanitize.ts:19-52)
// ============================================================================

// CSI: ESC[ ... final byte.
static CSI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[\x30-\x3f]*[\x20-\x2f]*[\x40-\x7e]").unwrap());
// OSC: ESC] ... terminator (BEL or ESC\). `(?s)` enables DOTALL — OSC
// payloads may span newlines per reference sanitize.ts:30.
static OSC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\x1b\].*?(?:\x07|\x1b\\)").unwrap());
// DCS, PM, APC: ESC P|^|_ ... ESC\.
static DCS_PM_APC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\x1b[P^_].*?\x1b\\").unwrap());
// Simple ESC + single printable char.
static SIMPLE_ESC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b[\x20-\x7e]").unwrap());
// C1 control codes.
static C1_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\x80-\x9f]").unwrap());
// Raw controls except \t (0x09) and \n (0x0a).
static CONTROL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\x00-\x06\x07\x08\x0b\x0c\x0d-\x1a\x1c-\x1f\x7f]").unwrap()
});

/// Strip all terminal escape sequences and raw control chars from
/// server-originated strings. Defends the CLI stdout trust boundary
/// (CWE-150). Order matters: the longest / most-specific patterns run first
/// so CSI doesn't eat the leading ESC of an OSC.
pub fn strip_terminal_escapes(s: &str) -> String {
    let s = OSC_RE.replace_all(s, "");
    let s = DCS_PM_APC_RE.replace_all(&s, "");
    let s = CSI_RE.replace_all(&s, "");
    let s = SIMPLE_ESC_RE.replace_all(&s, "");
    let s = C1_RE.replace_all(&s, "");
    let s = CONTROL_RE.replace_all(&s, "");
    s.into_owned()
}

static NL_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\r\n]+").unwrap());

/// Run `strip_terminal_escapes`, then collapse runs of CR/LF into a single
/// space and trim surrounding whitespace. Used for metadata fields that
/// surface in prompts or TUI headers where embedded newlines would corrupt
/// rendering.
pub fn sanitize_metadata(s: &str) -> String {
    let clean = strip_terminal_escapes(s);
    NL_RUN.replace_all(&clean, " ").trim().to_string()
}

// ============================================================================
// D-18: sanitize_name (ports installer.ts:40-55)
// ============================================================================

static NON_SAFE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9._]+").unwrap());
static EDGE_DOTS_DASHES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[.\-]+|[.\-]+$").unwrap());

/// Lower-case + replace any non-`[a-z0-9._]` run with `-`, trim leading and
/// trailing dots/dashes, cap length at 255 chars, fall back to
/// `unnamed-skill` on empty result.
pub fn sanitize_name(name: &str) -> String {
    let lower = name.to_lowercase();
    let step1 = NON_SAFE.replace_all(&lower, "-");
    let step2 = EDGE_DOTS_DASHES.replace_all(&step1, "").into_owned();
    let truncated: String = step2.chars().take(255).collect();
    if truncated.is_empty() {
        "unnamed-skill".to_string()
    } else {
        truncated
    }
}

// ============================================================================
// D-18: sanitize_subpath (ports source-parser.ts:89-105)
// ============================================================================

/// Validate a server-originated subpath. Rejects `..`, NUL, absolute paths,
/// drive prefixes, and root components. Normalizes `\` → `/` before the
/// segment check (reference source-parser.ts:90).
pub fn sanitize_subpath(subpath: &str) -> Result<String, HubError> {
    let normalized: String = subpath
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .collect();

    for seg in normalized.split('/') {
        if seg == ".." {
            return Err(typed(
                HubErrorKind::PathTraversal,
                format!("Unsafe subpath: {subpath:?} contains '..' traversal segment"),
            ));
        }
        if seg.contains('\0') {
            return Err(typed(
                HubErrorKind::PathTraversal,
                format!("NUL byte in subpath segment: {subpath:?}"),
            ));
        }
    }

    // Defense in depth: reject absolute and drive-prefix via Path::components.
    let p = Path::new(&normalized);
    if p.is_absolute() {
        return Err(typed(
            HubErrorKind::PathTraversal,
            format!("absolute subpath rejected: {subpath:?}"),
        ));
    }
    for comp in p.components() {
        match comp {
            Component::Prefix(_) => {
                return Err(typed(
                    HubErrorKind::PathTraversal,
                    format!("drive-prefix rejected: {subpath:?}"),
                ));
            }
            Component::RootDir => {
                return Err(typed(
                    HubErrorKind::PathTraversal,
                    format!("root component rejected: {subpath:?}"),
                ));
            }
            Component::ParentDir => {
                return Err(typed(
                    HubErrorKind::PathTraversal,
                    format!(".. component rejected: {subpath:?}"),
                ));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }

    Ok(normalized)
}

// ============================================================================
// D-18: is_path_safe + is_contained_in (ports installer.ts:63-68)
// ============================================================================

/// Returns `Ok(true)` iff `target` resolves inside `base`. Falls back to a
/// lexical resolver when either path does not yet exist on disk (e.g., we
/// are checking the intended install destination).
pub fn is_path_safe(base: &Path, target: &Path) -> std::io::Result<bool> {
    let norm_base: PathBuf = base.canonicalize().unwrap_or_else(|_| normalize_lexical(base));
    let norm_target: PathBuf = target
        .canonicalize()
        .unwrap_or_else(|_| normalize_lexical(target));
    Ok(norm_target.starts_with(&norm_base))
}

/// Semantic alias matching the reference TS `isContainedIn(parent, child)`
/// name. Implemented in terms of [`is_path_safe`].
pub fn is_contained_in(parent: &Path, child: &Path) -> std::io::Result<bool> {
    is_path_safe(parent, child)
}

fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            Component::Normal(s) => out.push(s),
            Component::RootDir | Component::Prefix(_) => out.push(comp.as_os_str()),
        }
    }
    out
}

// ============================================================================
// D-20: assert_temp_contained (Pitfall 4 — symlink-swap guard before
// remove_dir_all)
// ============================================================================

/// Assert `path` canonicalizes inside `std::env::temp_dir()`. Callers MUST
/// invoke before any `remove_dir_all` on a temp path so a symlink swap by
/// an attacker can never point the deletion at user data.
pub fn assert_temp_contained(path: &Path) -> Result<(), HubError> {
    let temp_root = std::env::temp_dir().canonicalize().map_err(|e| {
        typed(
            HubErrorKind::Io,
            format!("failed to canonicalize temp_dir: {e}"),
        )
    })?;
    let actual = path.canonicalize().map_err(|e| {
        typed(
            HubErrorKind::Io,
            format!("failed to canonicalize {}: {e}", path.display()),
        )
    })?;
    if !actual.starts_with(&temp_root) {
        return Err(typed(
            HubErrorKind::PathTraversal,
            format!(
                "temp path escaped temp_dir: {} is not under {}",
                actual.display(),
                temp_root.display()
            ),
        ));
    }
    Ok(())
}

// ============================================================================
// D-17: strict_yaml_delimiter (Pitfall 5 — reject ---js / ---javascript /
// non-yaml)
// ============================================================================

/// Reject any frontmatter delimiter that is not plain `---`. Tolerates a
/// trailing `\r` for CRLF line endings. Must run BEFORE `serde_yaml` sees
/// the input so legacy gray-matter-style `---js` RCE vectors are closed.
pub fn strict_yaml_delimiter(content: &str) -> Result<(), HubError> {
    let first_line_end = content.find('\n').ok_or_else(|| {
        typed(
            HubErrorKind::Parse,
            "SKILL.md has no newline; missing YAML frontmatter",
        )
    })?;
    let first_line = &content[..first_line_end];
    let first_line = first_line.trim_end_matches('\r');
    if first_line != "---" {
        return Err(typed(
            HubErrorKind::Parse,
            format!(
                "SKILL.md frontmatter must use YAML-only '---' delimiter, got: {first_line:?}"
            ),
        ));
    }
    Ok(())
}

// ============================================================================
// Slug derivation (ports blob.ts:55-62 — MUST match reference byte-for-byte)
// ============================================================================

// ASCII-only \s (space, tab, \n, \r, \f, \v) + underscore. Rust's regex
// crate default-enables Unicode; we intentionally use an explicit char
// class to match the JS reference /[\s_]+/g (no `u` flag, ASCII-only \s).
static WS_UNDERSCORE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t\n\r\x0c\x0b_]+").unwrap());
static NON_SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9-]").unwrap());
static COLLAPSE_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-+").unwrap());

/// URL slug derivation. Must match reference `blob.ts:55-62` byte-for-byte
/// — any drift produces silent 404s against skills.sh.
pub fn to_skill_slug(name: &str) -> String {
    let lower = name.to_lowercase();
    let step1 = WS_UNDERSCORE.replace_all(&lower, "-").into_owned();
    let step2 = NON_SLUG.replace_all(&step1, "").into_owned();
    let step3 = COLLAPSE_DASH.replace_all(&step2, "-").into_owned();
    step3.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // strip_terminal_escapes
    // -----------------------------------------------------------------

    #[test]
    fn strip_csi_sequence() {
        assert_eq!(strip_terminal_escapes("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn strip_osc_bel_terminator() {
        assert_eq!(strip_terminal_escapes("\x1b]0;title\x07rest"), "rest");
    }

    #[test]
    fn strip_raw_control_chars() {
        let out = strip_terminal_escapes("\x00\x01hello\x07\x08");
        // No bytes below 0x20 except \t/\n.
        for b in out.bytes() {
            if b < 0x20 {
                assert!(b == b'\t' || b == b'\n', "unexpected control byte {b}");
            }
        }
        assert!(out.contains("hello"));
    }

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(strip_terminal_escapes("plain text"), "plain text");
    }

    #[test]
    fn preserves_tab_and_newline() {
        assert_eq!(strip_terminal_escapes("a\tb\nc"), "a\tb\nc");
    }

    #[test]
    fn strips_dcs_sequence() {
        assert_eq!(strip_terminal_escapes("\x1bPpayload\x1b\\keep"), "keep");
    }

    #[test]
    fn strips_c1_control_codes() {
        // U+0085 (NEL) is in the C1 range \x80-\x9f. Rust string literals
        // require `\u{..}` for bytes > 0x7f.
        assert_eq!(strip_terminal_escapes("a\u{85}b"), "ab");
    }

    // -----------------------------------------------------------------
    // sanitize_metadata
    // -----------------------------------------------------------------

    #[test]
    fn metadata_collapses_newlines_and_trims() {
        assert_eq!(
            sanitize_metadata("  hello\r\n\r\nworld  "),
            "hello world"
        );
    }

    #[test]
    fn metadata_runs_escape_stripping() {
        assert_eq!(sanitize_metadata("\x1b[31mred\x1b[0m"), "red");
    }

    // -----------------------------------------------------------------
    // sanitize_name
    // -----------------------------------------------------------------

    #[test]
    fn sanitize_name_basic() {
        assert_eq!(sanitize_name("Hello World!"), "hello-world");
    }

    #[test]
    fn sanitize_name_empty_fallback() {
        assert_eq!(sanitize_name("!!!"), "unnamed-skill");
        assert_eq!(sanitize_name(""), "unnamed-skill");
    }

    #[test]
    fn sanitize_name_trims_edge_dots_and_dashes() {
        assert_eq!(sanitize_name("-.hello-.-"), "hello");
        assert_eq!(sanitize_name("...a..."), "a");
    }

    #[test]
    fn sanitize_name_preserves_safe_chars() {
        // NON_SAFE = `[^a-z0-9._]+` — underscore IS in the safe set, unlike
        // to_skill_slug which strips it. This matches installer.ts:40-55.
        assert_eq!(sanitize_name("foo.bar_baz"), "foo.bar_baz");
        assert_eq!(sanitize_name("Foo Bar"), "foo-bar");
    }

    // -----------------------------------------------------------------
    // sanitize_subpath
    // -----------------------------------------------------------------

    #[test]
    fn sanitize_subpath_rejects_parent_dir() {
        let err = sanitize_subpath("foo/../bar").unwrap_err();
        match err {
            HubError::Typed { kind, .. } => assert_eq!(kind, HubErrorKind::PathTraversal),
            _ => panic!("expected Typed"),
        }
    }

    #[test]
    fn sanitize_subpath_normalizes_backslash() {
        let got = sanitize_subpath("foo\\bar").unwrap();
        assert_eq!(got, "foo/bar");
    }

    #[test]
    fn sanitize_subpath_rejects_nul_byte() {
        let err = sanitize_subpath("foo\0bar").unwrap_err();
        match err {
            HubError::Typed { kind, .. } => assert_eq!(kind, HubErrorKind::PathTraversal),
            _ => panic!("expected Typed"),
        }
    }

    #[test]
    fn sanitize_subpath_allows_normal_path() {
        assert_eq!(sanitize_subpath("normal/path.md").unwrap(), "normal/path.md");
    }

    #[test]
    fn sanitize_subpath_rejects_absolute() {
        assert!(sanitize_subpath("/etc/passwd").is_err());
    }

    // -----------------------------------------------------------------
    // is_path_safe / is_contained_in
    // -----------------------------------------------------------------

    #[test]
    fn is_path_safe_inside_base() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let target = base.join("sub");
        std::fs::create_dir(&target).unwrap();
        assert!(is_path_safe(base, &target).unwrap());
    }

    #[test]
    fn is_path_safe_outside_base() {
        let tmp_a = tempfile::tempdir().unwrap();
        let tmp_b = tempfile::tempdir().unwrap();
        assert!(!is_path_safe(tmp_a.path(), tmp_b.path()).unwrap());
    }

    #[test]
    fn is_path_safe_lexical_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        // Non-existent path with .. traversal: lexical fallback should
        // resolve it outside the base.
        let traversal = base.join("a").join("..").join("..").join("outside");
        assert!(!is_path_safe(base, &traversal).unwrap());
    }

    #[test]
    fn is_contained_in_delegates_to_is_path_safe() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let target = base.join("inside");
        std::fs::create_dir(&target).unwrap();
        assert!(is_contained_in(base, &target).unwrap());
    }

    // -----------------------------------------------------------------
    // strict_yaml_delimiter
    // -----------------------------------------------------------------

    #[test]
    fn yaml_delimiter_accepts_plain() {
        assert!(strict_yaml_delimiter("---\nname: x\n---\nbody").is_ok());
    }

    #[test]
    fn yaml_delimiter_rejects_js_variant() {
        let err = strict_yaml_delimiter("---js\nname: x\n---\nbody").unwrap_err();
        match err {
            HubError::Typed { kind, .. } => assert_eq!(kind, HubErrorKind::Parse),
            _ => panic!("expected Typed"),
        }
    }

    #[test]
    fn yaml_delimiter_rejects_javascript_variant() {
        assert!(strict_yaml_delimiter("---javascript\n...").is_err());
    }

    #[test]
    fn yaml_delimiter_rejects_non_yaml() {
        assert!(strict_yaml_delimiter("not yaml\nfoo").is_err());
    }

    #[test]
    fn yaml_delimiter_tolerates_crlf() {
        assert!(strict_yaml_delimiter("---\r\nname: x\n---\n").is_ok());
    }

    #[test]
    fn yaml_delimiter_rejects_no_newline() {
        assert!(strict_yaml_delimiter("---").is_err());
    }

    // -----------------------------------------------------------------
    // assert_temp_contained
    // -----------------------------------------------------------------

    #[test]
    fn assert_temp_contained_ok() {
        let tmp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        assert!(assert_temp_contained(tmp.path()).is_ok());
    }

    #[test]
    fn assert_temp_contained_rejects_non_temp() {
        // `/` is guaranteed to not canonicalize under temp_dir.
        let err = assert_temp_contained(Path::new("/")).unwrap_err();
        match err {
            HubError::Typed { kind, .. } => assert_eq!(kind, HubErrorKind::PathTraversal),
            _ => panic!("expected Typed"),
        }
    }

    // -----------------------------------------------------------------
    // to_skill_slug — inline golden vectors (24 pairs, exceeds 20 minimum).
    // The tests/to_skill_slug.rs integration test covers the same cases
    // via tests/fixtures/slug_vectors.json to catch fixture drift.
    // -----------------------------------------------------------------

    const GOLDEN: &[(&str, &str)] = &[
        ("ASCII Art", "ascii-art"),
        ("tenor-gif", "tenor-gif"),
        ("Hello_World", "hello-world"),
        ("React Best Practices", "react-best-practices"),
        ("-hello-world-", "hello-world"),
        ("  spaced  ", "spaced"),
        ("--double--dash--", "double-dash"),
        ("Foo/Bar", "foobar"),
        ("Foo:Bar", "foobar"),
        ("it's cool", "its-cool"),
        ("React + Redux", "react-redux"),
        ("C++ Skill", "c-skill"),
        ("A.B.C", "abc"),
        ("OAuth2 Helper", "oauth2-helper"),
        ("Skill-v1.0", "skill-v10"),
        ("HELLO", "hello"),
        ("\u{1F389} Party", "party"),
        ("!!!", ""),
        ("---", ""),
        ("", ""),
        ("hello\nworld", "hello-world"),
        ("hello\tworld", "hello-world"),
        ("Tenor GIF Search", "tenor-gif-search"),
        ("Wiki Research Helper", "wiki-research-helper"),
    ];

    #[test]
    fn to_skill_slug_golden_inline() {
        let mut failures = Vec::new();
        for (input, expected) in GOLDEN {
            let actual = to_skill_slug(input);
            if actual != *expected {
                failures.push(format!(
                    "input={input:?} expected={expected:?} actual={actual:?}"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "slug golden-vector failures:\n{}",
            failures.join("\n")
        );
    }
}
