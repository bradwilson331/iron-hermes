use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_core::{MemoryStore, ToolSchema};

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
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
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
        use crate::web_search::WebSearchTool;

        self.register(Box::new(TerminalTool));
        self.register(Box::new(ReadFileTool));
        self.register(Box::new(WriteFileTool));
        self.register(Box::new(PatchFileTool));
        self.register(Box::new(SearchFilesTool));
        self.register(Box::new(WebSearchTool));
    }

    /// Register the memory tool with a shared MemoryStore.
    /// Called separately from register_defaults() because it requires a MemoryStore instance.
    pub fn register_memory_tool(&mut self, store: Arc<Mutex<MemoryStore>>) {
        use crate::memory_tool::MemoryTool;
        self.register(Box::new(MemoryTool::new(store)));
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
