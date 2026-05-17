pub mod agent_loop;
pub mod agent_wiring;
pub mod anthropic_client;
pub mod any_client;
pub mod app_runtime_factory;
pub mod budget;
pub mod client;
pub mod context_compressor;
pub mod context_engine;
pub mod context_loader;
pub mod engine_factory;
pub mod memory;
pub mod memory_flush_handler;
pub mod nudge;
pub mod personality;
pub mod pressure_warning;
pub mod prompt_builder;
pub mod session_search;
pub mod shrike;
pub mod subagent_registry;
pub mod subagent_runner;
pub mod subdir_discovery;
pub mod summarizing_engine;
pub mod tool_pair;
pub mod transcript;

pub use agent_loop::{AgentLoop, AgentResult, AggregatedUsage};
pub use agent_wiring::attach_context_engine;
pub use anthropic_client::AnthropicClient;
pub use any_client::{
    AnyClient, AnyClientSummarizationHandle, AnyClientVisionHandle, build_client,
    build_main_client, build_role_client, wire_fallback_if_configured,
};
pub use app_runtime_factory::{
    AppRuntimeBundle, AppRuntimeFactoryInput, DelegateTaskWiring, build_app_runtime_bundle,
};
pub use client::LlmClient;
pub use context_compressor::ContextCompressor;
pub use ironhermes_core::{CONTEXT_FILE_MAX_CHARS, scan_context_content, truncate_content};
pub use memory::{MemoryManager, SharedProvider};
pub use personality::PersonalityRegistry;
pub use pressure_warning::PressureTracker;
pub use prompt_builder::{PromptBuilder, PromptSlot};
pub use shrike::{KillResult, ShrikeService};
pub use subagent_runner::AgentSubagentRunner;
