use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_core::{MemoryStore, ToolSchema};
use ironhermes_cron::JobStore;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn toolset(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;

    fn is_available(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    guardrails: Vec<Box<dyn ironhermes_hooks::GuardrailHook>>,
    error_detail: ironhermes_hooks::ErrorDetailLevel,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            guardrails: Vec::new(),
            error_detail: ironhermes_hooks::ErrorDetailLevel::Full,
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Add a guardrail hook that will be checked before every tool dispatch.
    /// Guardrails are checked in registration order.
    /// Per D-05: register BlocklistGuardrail first, custom trait hooks second.
    pub fn add_guardrail(&mut self, hook: Box<dyn ironhermes_hooks::GuardrailHook>) {
        self.guardrails.push(hook);
    }

    /// Set the error detail level for guardrail block messages.
    pub fn set_error_detail(&mut self, level: ironhermes_hooks::ErrorDetailLevel) {
        self.error_detail = level;
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn get_definitions(&self, enabled_tools: Option<&[String]>) -> Vec<ToolSchema> {
        self.tools
            .values()
            .filter(|t| t.is_available())
            .filter(|t| {
                enabled_tools
                    .map(|list| list.iter().any(|name| name == t.name()))
                    .unwrap_or(true)
            })
            .map(|t| t.schema())
            .collect()
    }

    pub async fn dispatch(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", name))?;

        if !tool.is_available() {
            return Err(anyhow::anyhow!("Tool '{}' is not available", name));
        }

        // Guardrail intercept (HOOK-02): check all guardrails before dispatch.
        // Per D-05: config blocklist is registered first, trait hooks second.
        // T-06-04: args reference is the same one passed to tool.execute() — no copy-after-check gap.
        for guardrail in &self.guardrails {
            match guardrail.check(name, &args) {
                ironhermes_hooks::GuardrailDecision::Allow => {}
                ironhermes_hooks::GuardrailDecision::Warn { reason } => {
                    tracing::warn!(
                        tool = %name,
                        guardrail = %guardrail.name(),
                        reason = %reason,
                        "Guardrail warning (proceeding)"
                    );
                    // Continue to next guardrail — warn does not block
                }
                ironhermes_hooks::GuardrailDecision::Block { reason } => {
                    let error_msg = ironhermes_hooks::format_guardrail_error(
                        name,
                        &reason,
                        guardrail.name(),
                        &self.error_detail,
                    );
                    return Err(anyhow::anyhow!("{}", error_msg));
                }
            }
        }

        tool.execute(args).await
    }

    pub fn list_tools(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.tools.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    pub fn register_defaults(&mut self) {
        use crate::file_tools::{PatchFileTool, ReadFileTool, SearchFilesTool, WriteFileTool};
        use crate::terminal::TerminalTool;
        use crate::web_read::WebReadTool;
        use crate::web_search::WebSearchTool;

        self.register(Box::new(TerminalTool));
        self.register(Box::new(ReadFileTool));
        self.register(Box::new(WriteFileTool));
        self.register(Box::new(PatchFileTool));
        self.register(Box::new(SearchFilesTool));
        self.register(Box::new(WebSearchTool));
        self.register(Box::new(WebReadTool));
    }

    /// Register the memory tool with a shared MemoryStore.
    /// Called separately from register_defaults() because it requires a MemoryStore instance.
    pub fn register_memory_tool(&mut self, store: Arc<Mutex<MemoryStore>>) {
        use crate::memory_tool::MemoryTool;
        self.register(Box::new(MemoryTool::new(store)));
    }

    /// Register the cronjob tool with a shared JobStore.
    /// Called separately from register_defaults() because it requires a JobStore instance.
    pub fn register_cronjob_tool(&mut self, store: Arc<Mutex<JobStore>>) {
        use crate::cronjob_tool::CronjobTool;
        self.register(Box::new(CronjobTool::new(store)));
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::ToolSchema;
    use ironhermes_hooks::{BlocklistGuardrail, GuardrailDecision, GuardrailHook};

    // ---------------------------------------------------------------------------
    // Mock tool for testing
    // ---------------------------------------------------------------------------

    struct MockTool {
        tool_name: &'static str,
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            self.tool_name
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "mock tool for testing"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                self.tool_name,
                self.description(),
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("mock result".to_string())
        }
    }

    // ---------------------------------------------------------------------------
    // Warn-only guardrail for testing
    // ---------------------------------------------------------------------------

    struct WarnGuardrail;

    impl GuardrailHook for WarnGuardrail {
        fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> GuardrailDecision {
            GuardrailDecision::Warn {
                reason: "always warn".to_string(),
            }
        }
        fn name(&self) -> &str {
            "warn-always"
        }
    }

    fn make_registry_with_tool(tool_name: &'static str) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool { tool_name }));
        registry
    }

    // ---------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_dispatch_with_no_guardrails_passes() {
        let registry = make_registry_with_tool("test_tool");
        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(result.unwrap(), "mock result");
    }

    #[tokio::test]
    async fn test_dispatch_blocked_by_guardrail() {
        let mut registry = make_registry_with_tool("test_tool");
        registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec![
            "test_tool".to_string(),
        ])));

        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_err(), "expected Err (blocked), got Ok");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.to_lowercase().contains("blocked")
                || err_msg.contains("blocklist")
                || err_msg.contains("security policy"),
            "error should mention block: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_dispatch_allowed_by_guardrail() {
        let mut registry = make_registry_with_tool("test_tool");
        registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec![
            "other_tool".to_string(),
        ])));

        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_ok(), "expected Ok (allowed), got {result:?}");
        assert_eq!(result.unwrap(), "mock result");
    }

    #[tokio::test]
    async fn test_dispatch_warn_guardrail_proceeds() {
        let mut registry = make_registry_with_tool("test_tool");
        registry.add_guardrail(Box::new(WarnGuardrail));

        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_ok(), "warn guardrail must not block: {result:?}");
        assert_eq!(result.unwrap(), "mock result");
    }

    #[tokio::test]
    async fn test_guardrail_error_detail_minimal() {
        let mut registry = make_registry_with_tool("secret_tool");
        registry.set_error_detail(ironhermes_hooks::ErrorDetailLevel::Minimal);
        registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec![
            "secret_tool".to_string(),
        ])));

        let result = registry
            .dispatch("secret_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert_eq!(
            err_msg, "Tool call blocked by security policy",
            "minimal detail must not leak tool name: {err_msg}"
        );
        assert!(
            !err_msg.contains("secret_tool"),
            "tool name must not appear in minimal error: {err_msg}"
        );
    }
}
