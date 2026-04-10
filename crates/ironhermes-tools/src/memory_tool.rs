use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_core::{MemoryStore, MemoryTarget, ToolSchema};
use serde_json::json;

use crate::registry::Tool;

pub struct MemoryTool {
    store: Arc<Mutex<MemoryStore>>,
    read_only: bool,
}

impl MemoryTool {
    pub fn new(store: Arc<Mutex<MemoryStore>>) -> Self {
        Self { store, read_only: false }
    }

    pub fn new_read_only(store: Arc<Mutex<MemoryStore>>) -> Self {
        Self { store, read_only: true }
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

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn toolset(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Save, update, or remove persistent facts. Memory entries appear in your system prompt at the start of each session. Use 'memory' target for your personal notes and 'user' target for user profile information."
    }

    fn schema(&self) -> ToolSchema {
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
                "Error: memory is read-only in subagent context. Only query and get actions are available.".to_string()
            );
        }

        match action {
            "add" => {
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'content' for 'add' action"))?;

                let mut store = self.store.lock().unwrap();
                store.add(target, content).map_err(|e| anyhow::anyhow!(e))
            }
            "replace" => {
                let old_text = args
                    .get("old_text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'old_text' for 'replace' action"))?;
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'content' for 'replace' action"))?;

                let mut store = self.store.lock().unwrap();
                store
                    .replace(target, old_text, content)
                    .map_err(|e| anyhow::anyhow!(e))
            }
            "remove" => {
                let old_text = args
                    .get("old_text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'old_text' for 'remove' action"))?;

                let mut store = self.store.lock().unwrap();
                store
                    .remove(target, old_text)
                    .map_err(|e| anyhow::anyhow!(e))
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
    use ironhermes_core::MemoryStore;

    fn make_tool() -> (MemoryTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join("memories");
        let mut store = MemoryStore::new(mem_dir);
        store.load_from_disk().unwrap();
        let tool = MemoryTool::new(Arc::new(Mutex::new(store)));
        (tool, dir)
    }

    #[test]
    fn test_name() {
        let (tool, _dir) = make_tool();
        assert_eq!(tool.name(), "memory");
    }

    #[tokio::test]
    async fn test_add_action() {
        let (tool, _dir) = make_tool();
        let result = tool
            .execute(json!({
                "action": "add",
                "target": "memory",
                "content": "test fact"
            }))
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("added"));
    }

    #[tokio::test]
    async fn test_replace_action() {
        let (tool, _dir) = make_tool();
        tool.execute(json!({
            "action": "add",
            "target": "memory",
            "content": "original fact"
        }))
        .await
        .unwrap();

        let result = tool
            .execute(json!({
                "action": "replace",
                "target": "memory",
                "old_text": "original",
                "content": "updated fact"
            }))
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("replaced"));
    }

    #[tokio::test]
    async fn test_remove_action() {
        let (tool, _dir) = make_tool();
        tool.execute(json!({
            "action": "add",
            "target": "memory",
            "content": "fact to remove"
        }))
        .await
        .unwrap();

        let result = tool
            .execute(json!({
                "action": "remove",
                "target": "memory",
                "old_text": "to remove"
            }))
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("removed"));
    }

    #[tokio::test]
    async fn test_read_action_rejected() {
        let (tool, _dir) = make_tool();
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

    #[test]
    fn test_new_is_not_read_only() {
        let (tool, _dir) = make_tool();
        assert!(!tool.read_only, "default MemoryTool should not be read-only");
    }

    #[tokio::test]
    async fn test_read_only_blocks_add() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join("memories");
        let mut store = MemoryStore::new(mem_dir);
        store.load_from_disk().unwrap();
        let tool = MemoryTool::new_read_only(Arc::new(Mutex::new(store)));

        let result = tool
            .execute(json!({
                "action": "add",
                "target": "memory",
                "content": "should fail"
            }))
            .await;
        assert!(result.is_ok(), "read-only should return Ok with error message");
        let output = result.unwrap();
        assert!(output.contains("read-only"), "should mention read-only: {output}");
    }

    #[tokio::test]
    async fn test_read_only_blocks_remove() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join("memories");
        let mut store = MemoryStore::new(mem_dir);
        store.load_from_disk().unwrap();
        let tool = MemoryTool::new_read_only(Arc::new(Mutex::new(store)));

        let result = tool
            .execute(json!({
                "action": "remove",
                "target": "memory",
                "old_text": "anything"
            }))
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("read-only"), "should mention read-only: {output}");
    }

    #[tokio::test]
    async fn test_missing_content_for_add() {
        let (tool, _dir) = make_tool();
        let result = tool
            .execute(json!({
                "action": "add",
                "target": "memory"
            }))
            .await;
        assert!(result.is_err());
    }
}
