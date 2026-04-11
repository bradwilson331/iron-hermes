//! `SubagentRunner` implementation — bridges ironhermes-agent to the
//! `SubagentRunner` trait defined in ironhermes-tools/delegate_task.rs.
//!
//! This is the agent-side half of the dependency-inversion pattern
//! (same approach as RegistryDispatch for ToolDispatch in execute_code).

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ChatMessage;
use ironhermes_tools::delegate_task::SubagentRunner;
use ironhermes_tools::ToolRegistry;

use crate::client::LlmClient;
use crate::agent_loop::AgentLoop;

/// Concrete `SubagentRunner` that spawns child `AgentLoop` instances.
///
/// Holds a cloneable `LlmClient` so each child agent gets its own loop
/// without sharing mutable state with the parent. Supports model override
/// (D-23/D-24) via optional override fields.
pub struct AgentSubagentRunner {
    /// Parent's client, used when no model override is specified.
    client: LlmClient,
    /// Parent's base URL, used as fallback when model override is active.
    parent_base_url: String,
    /// Parent's API key, used as fallback when model override is active.
    parent_api_key: String,
    /// Optional override base URL from SubagentConfig (D-23).
    override_base_url: Option<String>,
    /// Optional override API key from SubagentConfig (D-23).
    override_api_key: Option<String>,
}

impl AgentSubagentRunner {
    pub fn new(
        client: LlmClient,
        parent_base_url: String,
        parent_api_key: String,
        override_base_url: Option<String>,
        override_api_key: Option<String>,
    ) -> Self {
        Self {
            client,
            parent_base_url,
            parent_api_key,
            override_base_url,
            override_api_key,
        }
    }
}

#[async_trait]
impl SubagentRunner for AgentSubagentRunner {
    async fn run_child(
        &self,
        registry: Arc<ToolRegistry>,
        system_prompt: String,
        max_iterations: usize,
        model_override: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        // D-23/D-24: construct child client with model override if specified
        let child_client = if let Some(model) = model_override {
            let base = self.override_base_url.as_deref()
                .unwrap_or(&self.parent_base_url);
            let key = self.override_api_key.as_deref()
                .unwrap_or(&self.parent_api_key);
            LlmClient::new(base, key, model)
        } else {
            self.client.clone()
        };

        let agent = AgentLoop::new(child_client, registry, max_iterations);

        let messages = vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user("Complete the task described in the system prompt."),
        ];

        let result = agent.run(messages).await?;
        Ok(result.final_response)
    }
}
