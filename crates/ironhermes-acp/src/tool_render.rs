//! Maps IronHermes tool calls to ACP `ToolKind` + `ToolCallContent` (CLI-07), and
//! classifies workspace-write targets that escape the session cwd (D-16, CLI-08 boundary
//! contract).
//!
//! # Design
//!
//! Everything here is a pure, synchronous function over already-available data — no
//! network, no locks held across `.await` — so `event_bridge.rs`'s synchronous callback
//! closures (plan 03) and `approval_bridge.rs`'s permission-request builder (plan 04) can
//! both call straight into this module without any async plumbing.
//!
//! # Encoding contract
//!
//! All rendered content is UTF-8 `String`. Two places genuinely need to detect invalid
//! UTF-8 rather than lossy-converting it: [`render_result_content`] (raw tool-output
//! bytes) and [`render_edit_diff`] (a target file's on-disk bytes). Both check explicitly
//! (`std::str::from_utf8` / `String::from_utf8`) and return an explanatory error
//! `ToolCallContent` instead — this file never calls `from_utf8_lossy` (verified by a
//! negative grep in the plan's acceptance criteria). Every bounded preview truncates on a
//! character boundary via [`bounded_preview`] and names how many bytes were omitted.
//!
//! Note: the real `ToolResultCallback` call chain today always hands
//! [`render_result_content`] valid UTF-8 — `ironhermes-exec::sandbox` already
//! lossy-converts raw process bytes to `String` before a tool's `Result<String>` ever
//! reaches this crate. The non-UTF-8 branch is exercised directly by this module's own
//! tests (which can construct an invalid `&[u8]` a `String`-typed callback never could) so
//! the function stays correct if that upstream conversion is ever changed or bypassed.

use std::fs;
use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{ContentBlock, Diff, TextContent, ToolCallContent, ToolKind};

/// Bound (characters) on any rendered preview — a Denial-of-Service mitigation
/// (T-36.8-26): an unbounded tool output or argument blob must not flow unbounded into a
/// `session/update` notification.
const MAX_PREVIEW_CHARS: usize = 4000;

/// Plan 47.7-02 (D-12): the human-readable line rendered ahead of the reason text when a
/// failed tool result carries [`crate::handlers::POLICY_DENIAL_PREFIX`] — the leading
/// content a client that renders `tool_call_update` content shows FIRST, before the detail.
/// ASCII, no `"` character (see `POLICY_DENIAL_PREFIX`'s own doc for why).
pub const DENIAL_HEADLINE: &str = "BLOCKED - this tool call was not executed.";

/// Namespace for the CLI-07 tool-call/edit rendering helpers. Stateless — every method
/// is a thin wrapper over this module's free functions (kept as free functions too so
/// `event_bridge.rs`'s per-callback closures and `approval_bridge.rs`'s permission-request
/// builder can call them directly without holding an instance alive across an `.await`).
pub struct AcpToolRender;

impl AcpToolRender {
    pub fn tool_kind(tool_name: &str) -> ToolKind {
        tool_kind(tool_name)
    }

    pub fn render_call_content(tool_name: &str, args_json: &str, cwd: &Path) -> Vec<ToolCallContent> {
        render_call_content(tool_name, args_json, cwd)
    }

    pub fn render_result_content(
        tool_name: &str,
        output: &[u8],
        ok: bool,
        denied: bool,
    ) -> Vec<ToolCallContent> {
        render_result_content(tool_name, output, ok, denied)
    }

    pub fn render_edit_diff(path: &Path, next: Option<&str>) -> ToolCallContent {
        render_edit_diff(path, next)
    }

    pub fn is_outside_workspace(session_cwd: &Path, target: &Path) -> bool {
        is_outside_workspace(session_cwd, target)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Task 1: tool-kind classification + call/result content
// ─────────────────────────────────────────────────────────────────────────

/// Classifies `tool_name` into its ACP-editor-facing affordance (CLI-07). The catch-all
/// arm returns [`ToolKind::Other`] so a newly registered tool this table doesn't yet know
/// about still renders as SOMETHING in the editor rather than being silently dropped.
pub fn tool_kind(tool_name: &str) -> ToolKind {
    match tool_name {
        "terminal" | "process" | "execute_code" | "delegate_task" | "image_generate"
        | "text_to_speech" | "video_gen" | "video_animate" => ToolKind::Execute,
        "write_file" | "patch" | "skill_manage" => ToolKind::Edit,
        "read_file" | "skill_view" | "skills_list" | "vision_analyze" => ToolKind::Read,
        "search_files" | "session_search" => ToolKind::Search,
        "web_search" | "web_extract" | "web_read" | "web_answer" => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

/// Pending-call content for `tool_name`, built from its raw JSON arguments string.
///
/// - Shell/terminal (`terminal`/`process`): the call content carries the command text.
/// - `execute_code`: the call content carries the script source.
/// - File-edit tools (`write_file`/`patch`): routed through [`render_pending_edit`] (task
///   2) so the SAME diff populates both the `tool_call` update and (task 3) the
///   permission-request subject (T-36.8-25).
/// - Everything else: a bounded text preview of the raw arguments.
///
/// Malformed (non-JSON) `args_json` never panics — it degrades to a bounded raw-text
/// preview of whatever was received.
pub fn render_call_content(tool_name: &str, args_json: &str, cwd: &Path) -> Vec<ToolCallContent> {
    match tool_name {
        "terminal" | "process" => vec![text_content(bounded_preview(
            &extract_json_string_field(args_json, "command").unwrap_or_else(|| args_json.to_string()),
        ))],
        "execute_code" => vec![text_content(bounded_preview(
            &extract_json_string_field(args_json, "code").unwrap_or_else(|| args_json.to_string()),
        ))],
        "write_file" | "patch" => match serde_json::from_str::<serde_json::Value>(args_json) {
            Ok(args) => vec![render_pending_edit(tool_name, &args, cwd)],
            Err(_) => vec![text_content(bounded_preview(args_json))],
        },
        _ => vec![text_content(bounded_preview(args_json))],
    }
}

/// Terminal (post-execution) content for a tool result. `output` is raw bytes rather than
/// `&str` specifically so this function can detect and honestly report non-UTF-8 output
/// (see module doc) rather than lossy-converting it. An empty `output` returns an empty
/// `Vec` — the caller (`event_bridge.rs`) still emits the terminal `tool_call_update`
/// itself regardless of whether there is any content to attach, so a tool that produced no
/// output does not leave the editor waiting on a call it already rendered.
///
/// `denied` (D-12) says this call was refused by the ACP approval gate and therefore never
/// executed. It is supplied by the caller from [`crate::handlers::DenialLedger`], which the
/// gate sites write directly — deliberately NOT re-derived here by looking for
/// [`crate::handlers::POLICY_DENIAL_PREFIX`] in `output`. `output` is the tool's own text, the model chooses
/// tool arguments, and most tool errors echo their arguments back, so a marker found there
/// may have been planted: searching for it would let a model make a call that DID execute
/// render as "was not executed", which is precisely the claim this label exists to make
/// trustworthy.
pub fn render_result_content(
    tool_name: &str,
    output: &[u8],
    ok: bool,
    denied: bool,
) -> Vec<ToolCallContent> {
    if output.is_empty() {
        return Vec::new();
    }
    match std::str::from_utf8(output) {
        Ok(text) => {
            if denied {
                render_denial_content(text)
            } else {
                vec![text_content(bounded_preview(text))]
            }
        }
        Err(_) => vec![text_content(format!(
            "<{tool_name} produced {} bytes of non-UTF-8 output{} — cannot render as text>",
            output.len(),
            if ok { "" } else { " (tool failed)" }
        ))],
    }
}

/// Plan 47.7-02 (D-12): renders a policy-denied tool result as [`DENIAL_HEADLINE`] followed
/// by a bounded preview of the reason text, so a client that renders `tool_call_update`
/// content shows the verdict ("this was blocked") before the detail ("...because..."). Only
/// reached from [`render_result_content`] when the caller's `denied` flag — sourced from the
/// gate's own [`crate::handlers::DenialLedger`], never from the output text — is set. Every
/// other failure path is untouched.
fn render_denial_content(text: &str) -> Vec<ToolCallContent> {
    vec![text_content(format!("{DENIAL_HEADLINE}\n{}", bounded_preview(text)))]
}

// ─────────────────────────────────────────────────────────────────────────
// Task 2: file edits as ACP diff content
// ─────────────────────────────────────────────────────────────────────────

/// Builds the pending-edit content for `write_file`/`patch` given their real argument
/// shape (read from the actual tool definitions in `ironhermes-tools::file_tools`, not
/// assumed): `write_file` is `{path, content}` (full-content replacement); `patch` is
/// `{path, before, after}` (single-occurrence exact-string replace). Shared by
/// `render_call_content` (task 1's `tool_call` wiring) and `approval_bridge.rs`'s
/// permission-request subject (task 3) — the SAME function renders both, so the diff the
/// user approves is the diff the editor shows (T-36.8-25).
pub fn render_pending_edit(tool_name: &str, args: &serde_json::Value, cwd: &Path) -> ToolCallContent {
    let Some(rel_path) = args.get("path").and_then(|v| v.as_str()) else {
        return text_content(format!("{tool_name}: missing required 'path' argument"));
    };
    let target = cwd.join(rel_path);

    match tool_name {
        "write_file" => {
            let next = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            render_edit_diff(&target, Some(next))
        }
        "patch" => {
            let before = args.get("before").and_then(|v| v.as_str()).unwrap_or("");
            let after = args.get("after").and_then(|v| v.as_str()).unwrap_or("");
            match fs::read(&target) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(original) => {
                        if original.contains(before) {
                            let patched = original.replacen(before, after, 1);
                            render_edit_diff(&target, Some(&patched))
                        } else {
                            text_content(format!(
                                "patch preview unavailable: text to replace was not found in {}",
                                target.display()
                            ))
                        }
                    }
                    Err(_) => non_utf8_error_content(&target),
                },
                Err(e) => text_content(format!(
                    "patch preview unavailable: could not read {}: {e}",
                    target.display()
                )),
            }
        }
        _ => text_content(format!("{tool_name}: not an edit tool")),
    }
}

/// Renders a file edit as ACP diff content (CLI-07, D-16). Reads the CURRENT on-disk text
/// at `path` itself — never a caller-cached/stale copy — so the prior side always reflects
/// reality at render time:
///
/// - The file does not exist yet: the prior side is `None` (absent), never an
///   empty-string. An empty-string prior on a creation would read to the editor as "this
///   file was empty and stayed empty," which is a lie about what actually happened.
/// - The file exists and is valid UTF-8: the prior side is `Some(current_text)`.
/// - The file exists but is NOT valid UTF-8: returns an explanatory error `Content`
///   instead of a `Diff` — never a lossy-converted diff.
///
/// `next` is the post-edit text the tool intends to write; `None` signals a delete. The
/// pinned SDK schema's `Diff::new_text` has no `Option` wrapper (verified against the
/// installed `agent-client-protocol-schema` crate source — unlike `old_text`, which is
/// `Option<String>`), so there is no way to serialize a literally-absent new side on the
/// wire. The closest truthful representation of "no new content" within that constraint is
/// an empty string, which is also the standard unified-diff convention for a deleted file
/// (the `/dev/null` "after" side) — not a defaulted/lazy empty string, a deliberate,
/// documented choice for the one case the wire format cannot express any other way.
///
/// The rendered path is always the best-effort CANONICALIZED absolute path (see
/// [`resolve_best_effort_absolute`]), so the editor opens the right file regardless of how
/// the tool was invoked (relative path, `..` traversal, or a symlinked ancestor).
pub fn render_edit_diff(path: &Path, next: Option<&str>) -> ToolCallContent {
    let resolved = resolve_best_effort_absolute(path);

    let prior: Option<String> = match fs::read(&resolved) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            Err(_) => return non_utf8_error_content(&resolved),
        },
        Err(_) => None, // file does not exist yet — absent prior side (creation)
    };

    let new_text = next.map(str::to_string).unwrap_or_default();
    ToolCallContent::Diff(Diff::new(resolved, new_text).old_text(prior))
}

fn non_utf8_error_content(path: &Path) -> ToolCallContent {
    text_content(format!(
        "cannot render diff: {} is not valid UTF-8",
        path.display()
    ))
}

// ─────────────────────────────────────────────────────────────────────────
// Task 3: the outside-workspace boundary predicate (D-16, CLI-08)
// ─────────────────────────────────────────────────────────────────────────

/// True when `target`'s resolved path does NOT live under `session_cwd` (D-16, CLI-08
/// boundary contract). Canonicalizes BOTH sides via [`resolve_best_effort_absolute`]
/// before comparing — the target's nearest-existing-ancestor fallback means a
/// not-yet-existing leaf is classified by its PARENT directory, not treated as
/// automatically "outside" merely because it doesn't exist yet.
///
/// Comparison is by path COMPONENT (`Path::starts_with`), never by string prefix, so a
/// sibling directory whose name merely starts with the cwd's name (e.g. `/workspace-evil`
/// vs. `/workspace`) is never mistaken for a child.
///
/// Because canonicalization resolves symlinks, a symlink that lives INSIDE the workspace
/// but points to a target OUTSIDE it correctly classifies as outside — canonicalizing the
/// symlink path itself (not the symlink's own location) is what makes this true.
pub fn is_outside_workspace(session_cwd: &Path, target: &Path) -> bool {
    let canon_cwd = resolve_best_effort_absolute(session_cwd);
    let canon_target = resolve_best_effort_absolute(target);
    !canon_target.starts_with(&canon_cwd)
}

/// Best-effort absolute-path resolution, shared by [`render_edit_diff`] (the rendered
/// path) and [`is_outside_workspace`] (both sides of the boundary comparison).
///
/// `std::fs::canonicalize` requires the ENTIRE path to exist, so a not-yet-written file
/// (the common case for `write_file` creating something new) always fails it outright.
/// This walks up from `path`, tracking the skipped (not-yet-existing) trailing components,
/// until it finds an ancestor that DOES canonicalize successfully — which also resolves
/// any `..` traversal or symlink in that existing prefix — then rejoins the tracked
/// components onto the canonicalized ancestor. Falls back to `path` joined onto the
/// process's current directory (if relative) when NO ancestor exists at all.
fn resolve_best_effort_absolute(path: &Path) -> PathBuf {
    if let Ok(canon) = fs::canonicalize(path) {
        return canon;
    }

    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    let mut ancestor = path;
    loop {
        match ancestor.parent() {
            Some(parent) => {
                if let Some(name) = ancestor.file_name() {
                    remainder.push(name.to_os_string());
                }
                if let Ok(canon_parent) = fs::canonicalize(parent) {
                    let mut result = canon_parent;
                    for comp in remainder.iter().rev() {
                        result.push(comp);
                    }
                    return result;
                }
                ancestor = parent;
            }
            None => {
                return std::env::current_dir()
                    .map(|cwd| cwd.join(path))
                    .unwrap_or_else(|_| path.to_path_buf());
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Shared helpers
// ─────────────────────────────────────────────────────────────────────────

fn text_content(text: String) -> ToolCallContent {
    ToolCallContent::from(ContentBlock::Text(TextContent::new(text)))
}

/// Best-effort extraction of a top-level string field from a JSON args blob. Returns
/// `None` (never panics) on invalid JSON, a missing field, or a non-string value — callers
/// fall back to a bounded raw-text preview in every `None` case.
fn extract_json_string_field(args_json: &str, field: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(args_json).ok()?;
    value.get(field)?.as_str().map(str::to_string)
}

/// Character-boundary-safe bounded preview with an explicit truncation marker naming how
/// many bytes were omitted (T-36.8-26 DoS mitigation; the encoding contract). Uses
/// `char_indices` (never a byte slice) so the returned string is always valid UTF-8 with
/// no split multi-byte character, even when the bound lands mid-way through a run of
/// multi-byte characters.
fn bounded_preview(s: &str) -> String {
    match s.char_indices().nth(MAX_PREVIEW_CHARS) {
        None => s.to_string(),
        Some((byte_idx, _)) => {
            let omitted = s.len() - byte_idx;
            format!("{}\u{2026}(truncated, {omitted} bytes omitted)", &s[..byte_idx])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_kind_classifies_known_tools() {
        assert_eq!(tool_kind("terminal"), ToolKind::Execute);
        assert_eq!(tool_kind("execute_code"), ToolKind::Execute);
        assert_eq!(tool_kind("write_file"), ToolKind::Edit);
        assert_eq!(tool_kind("patch"), ToolKind::Edit);
        assert_eq!(tool_kind("read_file"), ToolKind::Read);
        assert_eq!(tool_kind("search_files"), ToolKind::Search);
        assert_eq!(tool_kind("web_search"), ToolKind::Fetch);
    }

    #[test]
    fn tool_kind_catch_all_returns_other() {
        assert_eq!(tool_kind("some_future_tool_this_table_has_never_heard_of"), ToolKind::Other);
    }

    #[test]
    fn bounded_preview_truncates_multibyte_safely() {
        // Build a string of multi-byte characters that exceeds the bound.
        let s: String = std::iter::repeat_n('\u{1F600}', MAX_PREVIEW_CHARS + 50).collect();
        let preview = bounded_preview(&s);
        assert!(preview.chars().count() < s.chars().count());
        assert!(preview.contains("truncated"));
        // Must be valid UTF-8 with no split character — String type already guarantees
        // this, but assert the byte length is on a char boundary as an explicit check.
        assert!(preview.is_char_boundary(preview.len()));
    }

    #[test]
    fn bounded_preview_leaves_short_strings_untouched() {
        assert_eq!(bounded_preview("short"), "short");
    }
}
