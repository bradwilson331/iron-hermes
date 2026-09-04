pub mod adapter; // Phase 36.7.1: MessageHandler/PlatformAdapter (moved from ironhermes-gateway to break a dependency cycle with ironhermes-restgw; re-exported unchanged from ironhermes_gateway::adapter)
pub mod approval_gate;
pub mod approvals;
pub mod async_bridge; // Phase 41.3 UAT gap: LocalSet-safe async→sync bridge
pub mod audit;
pub mod auth;
pub mod blueprint; // Phase 49.5 Plan 05: relocated from ironhermes-cron (Rule 4 escalation) — see module doc
pub mod browser_profile;
pub mod commands;
pub mod concurrency;
pub mod config;
pub mod config_extras;
pub mod config_schema;
pub mod config_setter;
pub mod config_validate;
pub mod constants;
pub mod context_scanner;
pub mod dispatch_gate; // Phase 47.4 GAP-1: shared pre-spawn dispatch predicate
pub mod dotenv_write; // Phase 47.6 Plan 04 (D-06): single shared .env writer
pub mod env_sanitize;
pub mod error;
pub mod gateway_status; // Phase 49.3 Plan 06 (D-08): versioned gateway heartbeat status schema
pub mod memory_provider;
pub mod memory_store;
pub mod model_metadata;
pub mod models_cache;
pub mod pricing;
pub mod pricing_cache;
pub mod profile;
pub mod provider;
pub mod queue;
pub mod session;
pub mod skills;
pub mod ssrf;
pub mod stt; // Phase 36.17.8
pub mod token_estimator;
pub mod tts; // Phase 36.17.5
pub mod types;
pub mod vault; // Phase 46.8 UAT gap G-46.8-1
pub mod webhook_route; // Phase 36.7.1: inbound webhook route config schema
pub mod wizard;
pub mod workspace;

pub use approval_gate::{ApprovalGate, ApprovalOutcome};
pub use approvals::{ApprovalsState, ApprovalsStore, KeyKind};
pub use audit::{AuditConfig, AuditEntry, AuditLog};
pub use auth::{AuthStore, DcrEntry, TokenEntry};
pub use browser_profile::{SingletonOutcome, reconcile_singleton_lock};
pub use commands::context::CommandContext;
pub use commands::{
    ApprovalNeed, CommandCategory, CommandDef, CommandResult as SlashCommandResult, CommandRouter,
    PlatformFilter, QuickCommandDef, QuickCommandPlan, QuickCommandRegistry, ResolveResult,
    prepare_quick_command,
};
pub use concurrency::{ConcurrencyLayer, Surface, TurnEntry, TurnId, TurnRegistry, TurnSummary};
pub use config::{
    ApiMode, ApprovalsGatewayConfig, AuthConfig, AuthProviderConfig, BatchConfig,
    ChannelTrust, ConcurrencyConfig, Config, CustomProviderConfig, DangerousCommandsConfig,
    ExecConfig, ExtraTap, ExtractConfig, HubConfig, McpMutationGuardrailConfig, MemoryConfig,
    ModelRoleConfig, PlatformGatewayConfig, ProviderConfig, SkillsConfig, SubagentConfig,
    ToolsConfig, ToolsetEntry,
};
pub use config_schema::{ConfigField, MemoryAction, schema as config_schema};
pub use constants::*;
pub use context_scanner::{
    CONTEXT_FILE_MAX_CHARS, scan_context_content, truncate_content, truncate_on_char_boundary,
};
pub use env_sanitize::build_terminal_safe_env;
pub use error::{HermesError, Result};
pub use memory_provider::{MemoryEntries, MemoryProvider};
pub use memory_store::{MemoryStore, MemoryTarget};
pub use model_metadata::{ModelCapabilities, ModelMetadata, ModelRegistry};
pub use models_cache::{
    FetchResult, ModelsCache, ModelsCacheEntry, fetch_all, fetch_from_models_dev,
    fetch_from_openrouter, normalize_model_id,
};
pub use pricing::{PricingEntry, PricingRegistry, compute_cost_micros};
pub use pricing_cache::{PricingCache, PricingCacheEntry};
pub use provider::{ProviderResolver, ResolvedEndpoint, SummarizationClientHandle};
pub use queue::{MAX_QUEUE_DEPTH, MessageQueue, QueueError, WARN_QUEUE_DEPTH};
pub use session::SessionKey;
/// Phase 21.8.2 D-05: expose path-scan helper for D-05 WARN-BUT-LOAD invalid_skipped reporting.
pub use skills::build_skill_search_paths;
pub use skills::{
    CredentialFileEntry, EnvVarEntry, HermesMetadata, SkillConfigField, SkillRecord, SkillRegistry,
    SkillSource,
};
pub use ssrf::is_safe_url;
pub use stt::{BUILTIN_STT_NAMES, SttProvider, SttRegistry}; // Phase 36.17.8
pub use token_estimator::{
    TiktokenEncoding, TokenEstimator, global_estimate_tokens, init_global_estimator,
    warm_tiktoken_singletons,
};
pub use tts::{BUILTIN_TTS_NAMES, TtsProvider, TtsRegistry}; // Phase 36.17.5
pub use types::*;
// Phase 46.8 UAT gap G-46.8-1: shared `data_dir` sentinel resolver — every runtime
// `open_store`/`RustyVaultStore::open` call site (server, cron-runner, CLI) routes
// through this so they all agree on the same on-disk vault location as `vault init`.
pub use vault::{resolve_vault_config, resolve_vault_config_with_home};

// Phase 36.7.1: inbound webhook route config schema — canonical definition
// lives here (not in `ironhermes-restgw`) so `PlatformGatewayConfig` can hold
// a `Vec<WebhookRoute>` field without a dependency-direction cycle.
pub use webhook_route::{
    DeliverTarget, OutboundAuth, RouteRails, SessionMode, SignatureKind, WebhookRoute,
};

// Phase 25.3 D-W-1 — Workspace newtype + cwd walk-up resolution helper.
// Re-export name is `resolve_workspace_from_cwd` (aliased) to avoid collision with
// any existing or future `resolve_from_cwd` in other modules. Plan 8 wireup uses
// either `ironhermes_core::resolve_workspace_from_cwd` or the path-qualified
// `ironhermes_core::workspace::resolve_from_cwd` — both work.
pub use workspace::{Workspace, resolve_from_cwd as resolve_workspace_from_cwd};
