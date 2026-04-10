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
/// without sharing mutable state with the parent.
pub struct AgentSubagentRunner {
    client: LlmClient,
}

impl AgentSubagentRunner {
    pub fn new(client: LlmClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SubagentRunner for AgentSubagentRunner {
    async fn run_child(
        &self,
        registry: Arc<ToolRegistry>,
        system_prompt: String,
        max_iterations: usize,
    ) -> anyhow::Result<Option<String>> {
        let agent = AgentLoop::new(self.client.clone(), registry, max_iterations);

        let messages = vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user("Complete the task described in the system prompt."),
        ];

        let result = agent.run(messages).await?;
        Ok(result.final_response)
    }
}
