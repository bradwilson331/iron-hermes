//! Shared envelope helpers for all 11 kanban LLM tools (Plan 07, D-08).
//!
//! # Why this module exists
//!
//! Every kanban tool's success AND rejection envelope must carry `"board"` and
//! `"board_source"` fields (D-08 invariant).  Without a shared helper those two
//! fields would be copy-pasted 11 × 2 = 22 times, creating envelope drift that
//! would make cross-board debugging harder (T-5 mitigation).
//!
//! # Exports
//!
//! - [`resolve_board_context_from_args`] — extract `args["board"]`, call the
//!   4-tier resolver, return `(BoardContext, Option<BoardSlugError>)` so callers
//!   always have a valid context for envelope injection even when the slug is
//!   invalid.
//! - [`inject_board_fields`] — add `"board"` + `"board_source"` to an existing
//!   `serde_json::Value`.
//! - [`reject_with_board`] — build a rejection envelope JSON string including
//!   the two provenance fields.
//! - [`ok_with_board`] — inject board fields into a success value and serialize
//!   with `serde_json::to_string_pretty`.

use serde_json::{Value, json};

use crate::board::{BoardContext, BoardSlugError, default_board_context, resolve_board_context};

// ---------------------------------------------------------------------------
// Resolver shim
// ---------------------------------------------------------------------------

/// Extract `args["board"]` and run the 4-tier resolver.
///
/// Returns `(ctx, None)` on success.  Returns `(default_board_context(), Some(err))` when
/// the slug fails validation — the caller MUST use the returned error to emit a rejection,
/// but still has a valid `BoardContext` for envelope serialisation.
pub fn resolve_board_context_from_args(args: &Value) -> (BoardContext, Option<BoardSlugError>) {
    let slug_opt = args.get("board").and_then(|v| v.as_str());
    match resolve_board_context(slug_opt) {
        Ok(ctx) => (ctx, None),
        Err(e) => (default_board_context(), Some(e)),
    }
}

// ---------------------------------------------------------------------------
// Success-path helper
// ---------------------------------------------------------------------------

/// Inject `"board"` and `"board_source"` into a success `Value`, then serialize
/// with `serde_json::to_string_pretty`.
///
/// Returns the serialized string wrapped in `anyhow::Result`.
pub fn ok_with_board(value: Value, ctx: &BoardContext) -> anyhow::Result<String> {
    let enriched = inject_board_fields(value, ctx);
    Ok(serde_json::to_string_pretty(&enriched)?)
}

// ---------------------------------------------------------------------------
// Injection helper
// ---------------------------------------------------------------------------

/// Add `"board": <slug>` and `"board_source": <source>` to an existing `Value`.
///
/// - If `value` is a JSON object: the two fields are inserted in-place.
/// - If `value` is not an object (e.g. an array or scalar): the original value
///   is wrapped as `{ "result": <original>, "board": ..., "board_source": ... }`.
///
/// Returns the (possibly restructured) value.
pub fn inject_board_fields(mut value: Value, ctx: &BoardContext) -> Value {
    if let Some(map) = value.as_object_mut() {
        map.insert("board".to_string(), json!(ctx.slug));
        map.insert("board_source".to_string(), json!(ctx.source.as_str()));
        value
    } else {
        json!({
            "result": value,
            "board": ctx.slug,
            "board_source": ctx.source.as_str(),
        })
    }
}

// ---------------------------------------------------------------------------
// Rejection-path helper
// ---------------------------------------------------------------------------

/// Build a rejection envelope JSON string.
///
/// When `ctx` is `Some`, the envelope includes `"board"` and `"board_source"`
/// (D-08 invariant — T-5 mitigation: cross-board debugging surface on ALL
/// rejection paths).
///
/// When `ctx` is `None` (defensive — should not be needed because
/// `resolve_board_context_from_args` always returns a valid context), the
/// two fields are omitted.
///
/// `detail` is sanitized (backslash + quote escaping) to keep the output valid
/// inside a JSON string literal without depending on a full serializer.
pub fn reject_with_board(reason: &str, detail: &str, ctx: Option<&BoardContext>) -> String {
    let safe_detail = detail.replace('\\', "\\\\").replace('"', "\\\"");
    match ctx {
        Some(c) => format!(
            r#"{{"status":"rejected","reason":"{reason}","detail":"{safe_detail}","board":"{board}","board_source":"{source}"}}"#,
            board = c.slug,
            source = c.source.as_str(),
        ),
        None => format!(r#"{{"status":"rejected","reason":"{reason}","detail":"{safe_detail}"}}"#,),
    }
}

/// Build a rejection envelope from a rich `Value` (which must be a JSON object),
/// injecting `"board"` and `"board_source"` from `ctx`.
///
/// Use this when the rejection needs structured fields beyond `"detail"` (e.g.
/// `"task_id"`, `"current"`, `"expected"`, `"parent_id"`, `"child_id"`) to
/// preserve backward-compatible envelope shapes while adding D-08 provenance.
pub fn reject_value_with_board(
    mut value: Value,
    ctx: Option<&BoardContext>,
) -> anyhow::Result<String> {
    if let Some(c) = ctx
        && let Some(map) = value.as_object_mut()
    {
        map.insert("board".to_string(), json!(c.slug));
        map.insert("board_source".to_string(), json!(c.source.as_str()));
    }
    Ok(serde_json::to_string_pretty(&value)?)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{BoardContext, BoardSource};
    use std::path::PathBuf;

    fn make_ctx(slug: &str, source: BoardSource) -> BoardContext {
        BoardContext {
            slug: slug.to_string(),
            db_path: PathBuf::from(format!("/tmp/test/{}.db", slug)),
            source,
        }
    }

    // ── inject_board_fields ─────────────────────────────────────────────────

    #[test]
    fn inject_into_object_adds_two_fields() {
        let ctx = make_ctx("alpha", BoardSource::Flag);
        let input = json!({ "status": "ok", "task_id": "t_001" });
        let result = inject_board_fields(input, &ctx);

        assert_eq!(result["board"], "alpha");
        assert_eq!(result["board_source"], "flag");
        // Original fields preserved.
        assert_eq!(result["status"], "ok");
        assert_eq!(result["task_id"], "t_001");
    }

    #[test]
    fn inject_into_non_object_wraps_with_result_field() {
        let ctx = make_ctx("beta", BoardSource::Env);
        let input = json!(["item1", "item2"]);
        let result = inject_board_fields(input.clone(), &ctx);

        assert_eq!(result["board"], "beta");
        assert_eq!(result["board_source"], "env");
        // Original value moved to "result".
        assert_eq!(result["result"], input);
    }

    // ── reject_with_board ───────────────────────────────────────────────────

    #[test]
    fn reject_with_board_some_ctx_has_board_fields() {
        let ctx = make_ctx("gamma", BoardSource::File);
        let json_str = reject_with_board("invalid_board", "bad slug", Some(&ctx));

        assert!(
            json_str.contains("\"board\":\"gamma\""),
            "missing board field: {json_str}"
        );
        assert!(
            json_str.contains("\"board_source\":\"file\""),
            "missing board_source: {json_str}"
        );
        assert!(
            json_str.contains("\"status\":\"rejected\""),
            "missing status: {json_str}"
        );
        assert!(
            json_str.contains("\"reason\":\"invalid_board\""),
            "missing reason: {json_str}"
        );
        assert!(
            json_str.contains("\"detail\":\"bad slug\""),
            "missing detail: {json_str}"
        );
        // Valid JSON.
        let parsed: Value = serde_json::from_str(&json_str).expect("must be valid JSON");
        assert_eq!(parsed["board"], "gamma");
    }

    #[test]
    fn reject_with_board_none_ctx_omits_board_fields() {
        let json_str = reject_with_board("err", "detail text", None);

        assert!(
            !json_str.contains("\"board\""),
            "should not have board: {json_str}"
        );
        assert!(
            !json_str.contains("\"board_source\""),
            "should not have board_source: {json_str}"
        );
        let parsed: Value = serde_json::from_str(&json_str).expect("must be valid JSON");
        assert_eq!(parsed["status"], "rejected");
    }

    #[test]
    fn reject_detail_escaping() {
        let ctx = make_ctx("delta", BoardSource::Default);
        // Detail contains backslash and double-quote — these must be escaped.
        let json_str = reject_with_board("err", r#"path\to\"file""#, Some(&ctx));
        // Must parse as valid JSON.
        let parsed: Value =
            serde_json::from_str(&json_str).expect("must be valid JSON after escaping");
        assert!(parsed["detail"].as_str().unwrap().contains("path"));
    }

    // ── resolve_board_context_from_args ─────────────────────────────────────

    #[test]
    fn resolve_from_args_with_string_board_returns_flag_source() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Ensure env does not interfere.
        unsafe {
            std::env::remove_var("HERMES_KANBAN_BOARD");
        }

        let args = json!({ "board": "myproject" });
        let (ctx, err) = resolve_board_context_from_args(&args);

        assert!(err.is_none(), "unexpected error: {:?}", err);
        assert_eq!(ctx.slug, "myproject");
        assert_eq!(ctx.source, BoardSource::Flag);
    }

    #[test]
    fn resolve_from_args_with_no_board_returns_default_source_when_no_env_file() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Clear env and use a tempdir as IRONHERMES_HOME so no current file exists.
        unsafe {
            std::env::remove_var("HERMES_KANBAN_BOARD");
        }
        let dir = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", dir.path());
        }

        let args = json!({});
        let (ctx, err) = resolve_board_context_from_args(&args);

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }

        assert!(err.is_none(), "unexpected error: {:?}", err);
        assert_eq!(ctx.slug, "default");
        assert_eq!(ctx.source, BoardSource::Default);
    }

    #[test]
    fn resolve_from_args_with_invalid_slug_returns_err_and_default_ctx() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("HERMES_KANBAN_BOARD");
        }

        // Path traversal slug must fail validation.
        let args = json!({ "board": "../etc/passwd" });
        let (ctx, err) = resolve_board_context_from_args(&args);

        assert!(err.is_some(), "expected a validation error");
        // Even on error, ctx falls back to default so envelope injection still works.
        assert_eq!(ctx.slug, "default");
    }

    // ── ok_with_board ───────────────────────────────────────────────────────

    #[test]
    fn ok_with_board_serializes_pretty_json_with_board_fields() {
        let ctx = make_ctx("epsilon", BoardSource::Env);
        let value = json!({ "status": "ok" });
        let s = ok_with_board(value, &ctx).expect("should serialize");
        let parsed: Value = serde_json::from_str(&s).expect("must be valid JSON");
        assert_eq!(parsed["board"], "epsilon");
        assert_eq!(parsed["board_source"], "env");
        assert_eq!(parsed["status"], "ok");
    }
}
