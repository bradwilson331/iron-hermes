use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::{MemoryTarget, ToolSchema};
use serde_json::json;
use tokio::sync::Mutex;

use crate::memory_manager_handle::MemoryManagerHandle;
use crate::registry::Tool;

/// Shared handle over a MemoryManager-like object. See `MemoryManagerHandle`.
///
/// Plan 20-02: `MemoryTool` no longer owns a raw provider — it delegates
/// to a manager handle so writes fan out to the optional mirror provider.
pub type SharedMemoryManager = Arc<Mutex<dyn MemoryManagerHandle + Send>>;

pub struct MemoryTool {
    manager: SharedMemoryManager,
    read_only: bool,
}

impl MemoryTool {
    pub fn new(manager: SharedMemoryManager) -> Self {
        Self {
            manager,
            read_only: false,
        }
    }

    pub fn new_read_only(manager: SharedMemoryManager) -> Self {
        Self {
            manager,
            read_only: true,
        }
    }
}

fn parse_target(s: &str) -> anyhow::Result<MemoryTarget> {
    match s {
        "memory" => Ok(MemoryTarget::Memory),
        "user" => Ok(MemoryTarget::User),
        _ => Err(anyhow::anyhow!(
            "Unknown target '{}'. Valid targets: memory, user",
            s
        )),
    }
}

/// Format a number with thousands separators (e.g. 2200 -> "2,200").
fn fmt_commas(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    let len = bytes.len();
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}

/// Convert a raw JSON success response from MemoryStore into a human-readable string per D-14.
/// action: "Added" | "Replaced" | "Removed"
fn format_success_response(action: &str, target: MemoryTarget, json_str: &str) -> String {
    let target_label = match target {
        MemoryTarget::Memory => "Memory",
        MemoryTarget::User => "User Profile",
    };
    match serde_json::from_str::<serde_json::Value>(json_str) {
        Ok(v) => {
            let chars_used = v.get("chars_used").and_then(|x| x.as_u64()).unwrap_or(0);
            let chars_limit = v.get("chars_limit").and_then(|x| x.as_u64()).unwrap_or(0);
            let entries = v.get("entries").and_then(|x| x.as_u64()).unwrap_or(0);
            let pct = if chars_limit > 0 {
                chars_used * 100 / chars_limit
            } else {
                0
            };
            format!(
                "{} to memory. {}: {}% -- {}/{} chars ({} {})",
                action,
                target_label,
                pct,
                fmt_commas(chars_used),
                fmt_commas(chars_limit),
                entries,
                if entries == 1 { "entry" } else { "entries" }
            )
        }
        Err(_) => format!("{} to memory.", action),
    }
}

/// Reformat a raw JSON error response from MemoryStore into D-15 structured envelopes.
fn format_error_response(json_str: &str, content: Option<&str>) -> String {
    match serde_json::from_str::<serde_json::Value>(json_str) {
        Ok(v) => {
            let error_type = v.get("error").and_then(|x| x.as_str()).unwrap_or("unknown");
            match error_type {
                "capacity_exceeded" => {
                    let chars_used = v.get("chars_used").and_then(|x| x.as_u64()).unwrap_or(0);
                    let chars_limit = v.get("chars_limit").and_then(|x| x.as_u64()).unwrap_or(0);
                    let entry_size = content.map(|c| c.len() as u64).unwrap_or(0);
                    serde_json::json!({
                        "error": "capacity_exceeded",
                        "current": chars_used,
                        "limit": chars_limit,
                        "entry_size": entry_size,
                        "suggestion": "Remove an entry first"
                    })
                    .to_string()
                }
                "blocked" => serde_json::json!({
                    "error": "content_rejected",
                    "reason": "injection_pattern_detected"
                })
                .to_string(),
                _ => json_str.to_string(),
            }
        }
        Err(_) => json_str.to_string(),
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn toolset(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        if self.read_only {
            "Query persistent facts from memory. This is a read-only view; add/replace/remove are not available in subagent context. Memory facts are provided in the system prompt."
        } else {
            "Save, update, or remove persistent facts. Memory entries appear in your system prompt at the start of each session. Use 'memory' target for your personal notes and 'user' target for user profile information."
        }
    }

    fn schema(&self) -> ToolSchema {
        if self.read_only {
            // Read-only schema: no write actions available (D-12)
            ToolSchema::new(
                "memory",
                self.description(),
                json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["get"],
                            "description": "Action to perform on memory. Only 'get' is available in read-only subagent context."
                        },
                        "target": {
                            "type": "string",
                            "enum": ["memory", "user"],
                            "description": "Which memory store to query. 'memory' for personal notes, 'user' for user profile."
                        }
                    },
                    "required": ["action", "target"]
                }),
            )
        } else {
            ToolSchema::new(
                "memory",
                self.description(),
                json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["add", "replace", "remove"],
                            "description": "Action to perform on memory."
                        },
                        "target": {
                            "type": "string",
                            "enum": ["memory", "user"],
                            "description": "Which memory store to modify. 'memory' for personal notes (2200 char limit), 'user' for user profile (1375 char limit)."
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to add or replacement content for 'replace' action."
                        },
                        "old_text": {
                            "type": "string",
                            "description": "Unique substring identifying the entry to replace or remove. Required for 'replace' and 'remove' actions."
                        }
                    },
                    "required": ["action", "target"]
                }),
            )
        }
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'action'"))?;

        let target_str = args
            .get("target")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'target'"))?;

        let target = parse_target(target_str)?;

        // Block write actions in read-only mode (D-12: subagent memory isolation)
        if self.read_only && matches!(action, "add" | "replace" | "remove") {
            return Ok(
                "Error: memory is read-only in subagent context. Memory facts are available in the system prompt; add/replace/remove actions are disabled.".to_string()
            );
        }

        // Plan 20-02: Convert user-facing verb -> manager tool name so the
        // manager's `infer_action_target_content` routes the mirror correctly.
        // The manager returns the raw JSON envelope from the primary provider;
        // we format it into D-14 / D-15 shapes below.
        match action {
            "add" => {
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Missing required parameter 'content' for 'add' action")
                    })?
                    .to_string();

                let mgr_args = serde_json::json!({
                    "target": target_str,
                    "content": content,
                });
                let result = {
                    let mgr = self.manager.lock().await;
                    mgr.handle_tool_call("memory_add", mgr_args).await
                };
                match result {
                    Ok(json_str) => Ok(format_success_response("Added", target, &json_str)),
                    Err(json_str) => Ok(format_error_response(&json_str, Some(&content))),
                }
            }
            "replace" => {
                let old_text = args
                    .get("old_text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Missing required parameter 'old_text' for 'replace' action"
                        )
                    })?
                    .to_string();
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Missing required parameter 'content' for 'replace' action")
                    })?
                    .to_string();

                let mgr_args = serde_json::json!({
                    "target": target_str,
                    "old_text": old_text,
                    "new_content": content,
                });
                let result = {
                    let mgr = self.manager.lock().await;
                    mgr.handle_tool_call("memory_replace", mgr_args).await
                };
                match result {
                    Ok(json_str) => Ok(format_success_response("Replaced", target, &json_str)),
                    Err(json_str) => Ok(format_error_response(&json_str, Some(&content))),
                }
            }
            "remove" => {
                let old_text = args
                    .get("old_text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Missing required parameter 'old_text' for 'remove' action")
                    })?
                    .to_string();

                let mgr_args = serde_json::json!({
                    "target": target_str,
                    "old_text": old_text,
                });
                let result = {
                    let mgr = self.manager.lock().await;
                    mgr.handle_tool_call("memory_remove", mgr_args).await
                };
                match result {
                    Ok(json_str) => Ok(format_success_response("Removed", target, &json_str)),
                    Err(json_str) => Ok(format_error_response(&json_str, None)),
                }
            }
            other => Err(anyhow::anyhow!(
                "Unknown action '{}'. Valid actions: add, replace, remove",
                other
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::memory_store::MemoryResult;
    use std::sync::Mutex as StdMutex;

    /// Minimal MemoryManagerHandle mock that records every handle_tool_call.
    struct MockManager {
        writes: Arc<StdMutex<Vec<(String, serde_json::Value)>>>,
        response: Result<String, String>,
    }

    impl MockManager {
        fn new_ok(response: &str) -> (Self, Arc<StdMutex<Vec<(String, serde_json::Value)>>>) {
            let writes = Arc::new(StdMutex::new(Vec::new()));
            (
                Self {
                    writes: Arc::clone(&writes),
                    response: Ok(response.to_string()),
                },
                writes,
            )
        }
    }

    #[async_trait]
    impl MemoryManagerHandle for MockManager {
        async fn handle_tool_call(&self, name: &str, args: serde_json::Value) -> MemoryResult {
            self.writes.lock().unwrap().push((name.to_string(), args));
            self.response.clone()
        }
    }

    fn make_tool_with_ok(
        response: &str,
    ) -> (MemoryTool, Arc<StdMutex<Vec<(String, serde_json::Value)>>>) {
        let (mock, writes) = MockManager::new_ok(response);
        let manager: SharedMemoryManager = Arc::new(Mutex::new(mock));
        (MemoryTool::new(manager), writes)
    }

    #[tokio::test]
    async fn test_name() {
        let (tool, _writes) =
            make_tool_with_ok(r#"{"chars_used": 10, "chars_limit": 2200, "entries": 1}"#);
        assert_eq!(tool.name(), "memory");
    }

    #[tokio::test]
    async fn test_add_delegates_to_manager() {
        let (tool, writes) =
            make_tool_with_ok(r#"{"chars_used": 9, "chars_limit": 2200, "entries": 1}"#);
        let result = tool
            .execute(json!({
                "action": "add",
                "target": "memory",
                "content": "test fact"
            }))
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.contains("Added to memory"),
            "Expected 'Added to memory' in: {output}"
        );

        // Verify the manager saw the delegated call with the canonical tool name.
        let writes = writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, "memory_add");
        assert_eq!(writes[0].1["target"], "memory");
        assert_eq!(writes[0].1["content"], "test fact");
    }

    #[tokio::test]
    async fn test_replace_delegates_with_new_content() {
        let (tool, writes) =
            make_tool_with_ok(r#"{"chars_used": 10, "chars_limit": 2200, "entries": 1}"#);
        let result = tool
            .execute(json!({
                "action": "replace",
                "target": "memory",
                "old_text": "original",
                "content": "updated fact"
            }))
            .await;
        assert!(result.is_ok(), "replace must succeed: {:?}", result.err());
        let writes = writes.lock().unwrap();
        assert_eq!(writes[0].0, "memory_replace");
        assert_eq!(writes[0].1["new_content"], "updated fact");
        assert_eq!(writes[0].1["old_text"], "original");
    }

    #[tokio::test]
    async fn test_remove_delegates_to_manager() {
        let (tool, writes) =
            make_tool_with_ok(r#"{"chars_used": 0, "chars_limit": 2200, "entries": 0}"#);
        let result = tool
            .execute(json!({
                "action": "remove",
                "target": "memory",
                "old_text": "to remove"
            }))
            .await;
        assert!(result.is_ok());
        let writes = writes.lock().unwrap();
        assert_eq!(writes[0].0, "memory_remove");
        assert_eq!(writes[0].1["old_text"], "to remove");
    }

    #[tokio::test]
    async fn test_read_only_blocks_add() {
        let (mock, _writes) = MockManager::new_ok("{}");
        let manager: SharedMemoryManager = Arc::new(Mutex::new(mock));
        let tool = MemoryTool::new_read_only(manager);

        let result = tool
            .execute(json!({
                "action": "add",
                "target": "memory",
                "content": "should fail"
            }))
            .await;
        assert!(
            result.is_ok(),
            "read-only should return Ok with error message"
        );
        let output = result.unwrap();
        assert!(
            output.contains("read-only"),
            "should mention read-only: {output}"
        );
    }

    #[tokio::test]
    async fn test_read_only_blocks_remove() {
        let (mock, _writes) = MockManager::new_ok("{}");
        let manager: SharedMemoryManager = Arc::new(Mutex::new(mock));
        let tool = MemoryTool::new_read_only(manager);

        let result = tool
            .execute(json!({
                "action": "remove",
                "target": "memory",
                "old_text": "anything"
            }))
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.contains("read-only"),
            "should mention read-only: {output}"
        );
    }

    #[tokio::test]
    async fn test_missing_content_for_add() {
        let (tool, _writes) = make_tool_with_ok("{}");
        let result = tool
            .execute(json!({
                "action": "add",
                "target": "memory"
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_action_rejected() {
        let (tool, _writes) = make_tool_with_ok("{}");
        let result = tool
            .execute(json!({
                "action": "read",
                "target": "memory"
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown action"));
    }
}
