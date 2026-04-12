use ironhermes_core::ToolSchema;
use ironhermes_state::{SearchFilter, StateStore};
use serde_json::json;

/// Returns the tool schema for session_search per D-05.
pub fn session_search_schema() -> ToolSchema {
    ToolSchema::new(
        "session_search",
        "Search past conversations. Returns FTS5 full-text search results with context.",
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "FTS5 search query. Supports keywords, phrases (\"quoted\"), boolean (AND/OR/NOT), prefix (word*)."
                },
                "role_filter": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["user", "assistant", "system", "tool"] },
                    "description": "Filter by message role. Uses first value only."
                },
                "source_filter": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["cli", "telegram"] },
                    "description": "Filter by session source. Uses first value only."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results to return (default 5, max 20).",
                    "default": 5
                }
            },
            "required": ["query"]
        }),
    )
}

/// Truncate a string to at most `max_chars` characters.
/// If truncated, appends "...".
fn truncate_context(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars - 3).collect();
        format!("{}...", truncated)
    }
}

/// Convert FTS5 snippet markers from `<<match>>` to `>>>match<<<` per D-06.
/// Single-pass scan to avoid double-substitution when replacements overlap.
fn convert_markers(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + s.len() / 4);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'<' && bytes[i + 1] == b'<' {
            result.push_str(">>>");
            i += 2;
        } else if i + 1 < bytes.len() && bytes[i] == b'>' && bytes[i + 1] == b'>' {
            result.push_str("<<<");
            i += 2;
        } else {
            result.push(s[i..].chars().next().unwrap());
            i += s[i..].chars().next().unwrap().len_utf8();
        }
    }
    result
}

/// Handle a session_search tool call.
///
/// Returns a JSON string — either an array of result objects or an error object.
pub fn handle_session_search(args: &serde_json::Value, state_store: &StateStore) -> String {
    // Extract query (required)
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(q) if !q.is_empty() => q.to_string(),
        _ => {
            return json!({
                "error": "missing_query",
                "reason": "query parameter is required"
            })
            .to_string();
        }
    };

    // Extract optional role_filter (use first element only)
    let role = args
        .get("role_filter")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .map(String::from);

    // Extract optional source_filter (use first element only)
    let source = args
        .get("source_filter")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .map(String::from);

    // Extract optional limit (default 5, clamp to max 20)
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).min(20))
        .unwrap_or(5);

    let filter = SearchFilter {
        query: Some(query),
        role,
        source,
        limit,
        raw: false,
        ..SearchFilter::default()
    };

    let results = match state_store.search_messages(&filter) {
        Ok(r) => r,
        Err(e) => {
            return json!({
                "error": "search_failed",
                "reason": e.to_string()
            })
            .to_string();
        }
    };

    if results.is_empty() {
        return json!({
            "results": [],
            "message": "No matches found"
        })
        .to_string();
    }

    let output: Vec<serde_json::Value> = results
        .into_iter()
        .map(|r| {
            let snippet = r.snippet.as_deref().map(convert_markers);
            let context_before = r
                .context_before
                .as_deref()
                .map(|s| truncate_context(s, 200));
            let context_after = r
                .context_after
                .as_deref()
                .map(|s| truncate_context(s, 200));

            json!({
                "session_id": r.session_id,
                "role": r.role,
                "snippet": snippet,
                "context_before": context_before,
                "context_after": context_after,
                "timestamp": r.timestamp,
                "source": r.session_source,
                "session_title": r.session_title,
            })
        })
        .collect();

    serde_json::to_string(&output).unwrap_or_else(|_| r#"{"error":"serialization_failed"}"#.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_state::StateStore;
    use serde_json::json;

    fn make_store() -> (StateStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.db");
        let store = StateStore::new(db_path).unwrap();
        (store, dir)
    }

    #[test]
    fn test_missing_query_returns_error() {
        let (store, _dir) = make_store();
        let result = handle_session_search(&json!({}), &store);
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["error"], "missing_query", "Expected missing_query error, got: {result}");
    }

    #[test]
    fn test_empty_query_returns_error() {
        let (store, _dir) = make_store();
        let result = handle_session_search(&json!({"query": ""}), &store);
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["error"], "missing_query", "Expected missing_query for empty query: {result}");
    }

    #[test]
    fn test_snippet_marker_conversion() {
        // Test the convert_markers helper directly
        let input = "some <<match>> text <<another>> word";
        let output = convert_markers(input);
        // <<match>> -> >>>match<<<
        assert!(output.contains(">>>match<<<"), "Expected >>>match<<< in: {output}");
        assert!(output.contains(">>>another<<<"), "Expected >>>another<<< in: {output}");
        // No original <<marker>> patterns should remain (original markers used <<...>>)
        assert!(!output.contains("<<match>>"), "Original <<match>> pattern should not remain: {output}");
        assert!(!output.contains("<<another>>"), "Original <<another>> pattern should not remain: {output}");
    }

    #[test]
    fn test_context_truncation() {
        // String exactly at 200 chars - no truncation
        let s200 = "a".repeat(200);
        let result = truncate_context(&s200, 200);
        assert_eq!(result.len(), 200, "200-char string should not be truncated");
        assert!(!result.ends_with("..."), "Should not have ellipsis");

        // String longer than 200 chars - truncate to 200
        let s201 = "a".repeat(201);
        let result = truncate_context(&s201, 200);
        assert_eq!(result.chars().count(), 200, "Should be exactly 200 chars");
        assert!(result.ends_with("..."), "Should end with ellipsis");

        // String much longer than 200 chars
        let s300 = "b".repeat(300);
        let result = truncate_context(&s300, 200);
        assert_eq!(result.chars().count(), 200, "Should be exactly 200 chars");
        assert!(result.ends_with("..."), "Should end with ellipsis");
    }

    #[test]
    fn test_no_results_returns_empty_message() {
        let (store, _dir) = make_store();
        // Query on empty DB should return empty results message
        let result = handle_session_search(&json!({"query": "nonexistent_xyz_query"}), &store);
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["results"], json!([]), "Expected empty results array: {result}");
        assert!(v["message"].as_str().is_some(), "Expected message field: {result}");
    }
}
