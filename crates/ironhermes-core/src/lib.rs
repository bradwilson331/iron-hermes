pub mod commands;
pub mod config;
pub mod config_schema;
pub mod config_setter;
pub mod config_validate;
pub mod constants;
pub mod wizard;
pub mod context_scanner;
pub mod error;
pub mod memory_provider;
pub mod memory_store;
pub mod model_metadata;
pub mod models_cache;
pub mod profile;
pub mod provider;
pub mod skills;
pub mod token_estimator;
pub mod ssrf;
pub mod types;
pub mod workspace;

pub use config::{
    ApiMode, BatchConfig, Config, CustomProviderConfig, ExecConfig, ExtractConfig, ExtraTap,
    HubConfig, MemoryConfig, ModelRoleConfig, ProviderConfig, SkillsConfig, SubagentConfig,
    ToolsetEntry, ToolsConfig,
};
pub use config_schema::{ConfigField, MemoryAction, schema as config_schema};
pub use constants::*;
pub use context_scanner::{scan_context_content, truncate_content, CONTEXT_FILE_MAX_CHARS};
pub use error::{HermesError, Result};
pub use memory_provider::{MemoryEntries, MemoryProvider};
pub use memory_store::{MemoryStore, MemoryTarget};
pub use provider::{ProviderResolver, ResolvedEndpoint, SummarizationClientHandle};
pub use skills::{
    CredentialFileEntry, EnvVarEntry, HermesMetadata, SkillConfigField, SkillRecord, SkillRegistry,
    SkillSource,
};
pub use commands::{
    CommandCategory, CommandDef, CommandResult as SlashCommandResult, CommandRouter, PlatformFilter,
    ResolveResult,
};
pub use commands::context::CommandContext;
pub use model_metadata::{ModelCapabilities, ModelMetadata, ModelRegistry};
pub use models_cache::{
    ModelsCache, ModelsCacheEntry, FetchResult, fetch_all, fetch_from_models_dev,
    fetch_from_openrouter, normalize_model_id,
};
pub use token_estimator::{
    TokenEstimator, TiktokenEncoding, init_global_estimator, global_estimate_tokens,
    warm_tiktoken_singletons,
};
pub use ssrf::is_safe_url;
pub use types::*;

// Phase 25.3 D-W-1 — Workspace newtype + cwd walk-up resolution helper.
// Re-export name is `resolve_workspace_from_cwd` (aliased) to avoid collision with
// any existing or future `resolve_from_cwd` in other modules. Plan 8 wireup uses
// either `ironhermes_core::resolve_workspace_from_cwd` or the path-qualified
// `ironhermes_core::workspace::resolve_from_cwd` — both work.
pub use workspace::{Workspace, resolve_from_cwd as resolve_workspace_from_cwd};
