//! `SubagentRunner` implementation — bridges ironhermes-agent to the
//! `SubagentRunner` trait defined in ironhermes-tools/delegate_task.rs.
//!
//! This is the agent-side half of the dependency-inversion pattern
//! (same approach as RegistryDispatch for ToolDispatch in execute_code).

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::{ChatMessage, ProviderResolver};
use ironhermes_tools::delegate_task::SubagentRunner;
use ironhermes_tools::ToolRegistry;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::any_client::AnyClient;
use crate::agent_loop::AgentLoop;
use crate::budget::BudgetHandle;

/// Concrete `SubagentRunner` that spawns child `AgentLoop` instances.
///
/// Holds a cloneable `AnyClient` so each child agent gets its own loop
/// without sharing mutable state with the parent. Supports model override
/// (D-23/D-24) via the ProviderResolver.
pub struct AgentSubagentRunner {
    /// Parent's client, used when no model override is specified.
    client: AnyClient,
    /// Provider resolver for constructing override clients.
    resolver: ProviderResolver,
    /// Optional shared iteration budget handle (PROV-10 / D-15).
    /// Plan 21.7-05 switched this from `Arc<AtomicUsize>` to [`BudgetHandle`];
    /// clones of the handle share the underlying counter so parent + child
    /// subagent loops decrement the SAME budget and observe the same
    /// pressure-tier ladder.
    budget: Option<BudgetHandle>,
}

impl AgentSubagentRunner {
    pub fn new(
        client: AnyClient,
        resolver: ProviderResolver,
        budget: Option<BudgetHandle>,
    ) -> Self {
        Self {
            client,
            resolver,
            budget,
        }
    }
}

#[async_trait]
impl SubagentRunner for AgentSubagentRunner {
    async fn run_child(
        &self,
        registry: Arc<RwLock<ToolRegistry>>,
        system_prompt: String,
        max_iterations: usize,
        model_override: Option<&str>,
        cancel_token: Option<CancellationToken>,
        tool_progress: Option<ironhermes_tools::delegate_task::ChildToolProgressCallback>,
    ) -> anyhow::Result<Option<String>> {
        // D-23/D-24: construct child client with model override if specified
        let child_client = if let Some(model) = model_override {
            let endpoint = self.resolver.resolve_for_main();
            AnyClient::from_endpoint_with_model(endpoint, model)?
        } else {
            self.client.clone()
        };

        let mut agent = AgentLoop::new(child_client, registry, max_iterations);
        // D-21: Forward cancel token to child AgentLoop
        if let Some(token) = cancel_token {
            agent = agent.with_cancellation_token(token);
        }
        // D-19: Forward tool progress callback to child AgentLoop
        if let Some(cb) = tool_progress {
            agent = agent.with_tool_progress(cb);
        }
        // Wire budget if available
        if let Some(ref budget) = self.budget {
            agent = agent.with_budget(budget.clone());
        }

        let messages = vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user("Complete the task described in the system prompt."),
        ];

        let result = agent.run(messages).await?;
        Ok(result.final_response)
    }
}
