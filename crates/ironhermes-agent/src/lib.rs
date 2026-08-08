pub mod agent_loop;
pub mod agent_runtime;
pub mod agent_wiring;
pub mod anthropic_client;
pub mod any_client;
pub mod app_runtime_factory;
pub mod budget;
pub mod client;
pub mod codex_client;
pub mod context_compressor;
pub mod context_engine;
pub mod context_loader;
pub mod context_refs;
pub mod engine_factory;
pub mod error_classifier;
pub mod memory;
pub mod memory_context;
pub mod memory_flush_handler;
pub mod nudge;
pub mod personality;
pub mod pressure_warning;
pub mod prompt_builder;
pub mod rate_limit_tracker;
pub mod session_search;
pub mod shrike;
pub mod streaming_scrubber;
pub mod subagent_registry;
pub mod subagent_runner;
pub mod subdir_discovery;
pub mod summarizing_engine;
pub mod tool_pair;
pub mod transcript;

pub use agent_loop::{AgentLoop, AgentResult, AggregatedUsage};
pub use agent_runtime::{
    AgentRuntime, AgentRuntimeInput, MessagingPerTurnWiring, TtsPerTurnWiring, TurnRequest,
};
pub use agent_wiring::attach_context_engine;
pub use anthropic_client::AnthropicClient;
pub use any_client::{
    AnyClient, AnyClientSummarizationHandle, AnyClientVisionHandle, build_client,
    build_main_client, build_main_client_with_model, build_role_client,
    wire_fallback_if_configured,
};
pub use app_runtime_factory::{
    AppRuntimeBundle, AppRuntimeFactoryInput, DelegateTaskWiring, build_app_runtime_bundle,
};
pub use client::LlmClient;
pub use codex_client::CodexClient;
pub use context_compressor::ContextCompressor;
pub use error_classifier::{ProviderError, classify_llm_error_typed};
pub use ironhermes_core::{CONTEXT_FILE_MAX_CHARS, scan_context_content, truncate_content};
pub use memory::{MemoryManager, SharedProvider};
pub use personality::PersonalityRegistry;
pub use pressure_warning::PressureTracker;
pub use prompt_builder::{PromptBuilder, PromptSlot};
pub use rate_limit_tracker::{
    RateLimitEvent, RateLimitKey, RateLimitSeverity, RateLimitSource, RateLimitTracker,
    TrackerState, hash_api_key,
};
pub use shrike::{KillResult, ShrikeService};
pub use subagent_runner::AgentSubagentRunner;
