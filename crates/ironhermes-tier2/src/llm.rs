//! The memory LLM seam for consolidation (D-16).

/// One completion against the memory model. Implemented in `ironhermes-agent` through the
/// `model.roles.memory` cascade; tests use `test_support::FakeLlm`.
#[async_trait::async_trait]
pub trait Tier2LlmHandle: Send + Sync {
    async fn complete(
        &self,
        system: String,
        user: String,
        max_tokens: u32,
    ) -> anyhow::Result<String>;
}
