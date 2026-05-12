pub mod browser_profile;
pub mod commands;
pub mod config;
pub mod config_schema;
pub mod config_setter;
pub mod config_validate;
pub mod constants;
pub mod context_scanner;
pub mod error;
pub mod memory_provider;
pub mod memory_store;
pub mod model_metadata;
pub mod models_cache;
pub mod profile;
pub mod provider;
pub mod skills;
pub mod ssrf;
pub mod token_estimator;
pub mod types;
pub mod wizard;
pub mod workspace;

pub use commands::context::CommandContext;
pub use commands::{
    CommandCategory, CommandDef, CommandResult as SlashCommandResult, CommandRouter,
    PlatformFilter, ResolveResult,
};
pub use config::{
    ApiMode, BatchConfig, Config, CustomProviderConfig, ExecConfig, ExtraTap, ExtractConfig,
    HubConfig, MemoryConfig, ModelRoleConfig, ProviderConfig, SkillsConfig, SubagentConfig,
    ToolsConfig, ToolsetEntry,
};
pub use config_schema::{ConfigField, MemoryAction, schema as config_schema};
pub use constants::*;
pub use context_scanner::{CONTEXT_FILE_MAX_CHARS, scan_context_content, truncate_content};
pub use error::{HermesError, Result};
pub use memory_provider::{MemoryEntries, MemoryProvider};
pub use memory_store::{MemoryStore, MemoryTarget};
pub use model_metadata::{ModelCapabilities, ModelMetadata, ModelRegistry};
pub use models_cache::{
    FetchResult, ModelsCache, ModelsCacheEntry, fetch_all, fetch_from_models_dev,
    fetch_from_openrouter, normalize_model_id,
};
pub use provider::{ProviderResolver, ResolvedEndpoint, SummarizationClientHandle};
pub use skills::{
    CredentialFileEntry, EnvVarEntry, HermesMetadata, SkillConfigField, SkillRecord, SkillRegistry,
    SkillSource,
};
/// Phase 21.8.2 D-05: expose path-scan helper for D-05 WARN-BUT-LOAD invalid_skipped reporting.
pub use skills::build_skill_search_paths;
pub use browser_profile::{SingletonOutcome, reconcile_singleton_lock};
pub use ssrf::is_safe_url;
pub use token_estimator::{
    TiktokenEncoding, TokenEstimator, global_estimate_tokens, init_global_estimator,
    warm_tiktoken_singletons,
};
pub use types::*;

// Phase 25.3 D-W-1 — Workspace newtype + cwd walk-up resolution helper.
// Re-export name is `resolve_workspace_from_cwd` (aliased) to avoid collision with
// any existing or future `resolve_from_cwd` in other modules. Plan 8 wireup uses
// either `ironhermes_core::resolve_workspace_from_cwd` or the path-qualified
// `ironhermes_core::workspace::resolve_from_cwd` — both work.
pub use workspace::{Workspace, resolve_from_cwd as resolve_workspace_from_cwd};
