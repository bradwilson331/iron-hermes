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
pub mod config_yaml_edit; // Phase 54.13: pure mask/unmask/validate text fns for the raw config editor
pub mod constants;
pub mod content_screen; // Phase 57 Plan 05 (D-02/D-03): process-wide ContentScreen seam + ScreenVerdict
pub mod context_scanner;
pub mod defcon; // Phase 57 Plan 02 (D-09): process-wide per-session DEFCON level registry
pub mod dispatch_gate; // Phase 47.4 GAP-1: shared pre-spawn dispatch predicate
pub mod dotenv_write; // Phase 47.6 Plan 04 (D-06): single shared .env writer
pub mod env_sanitize;
pub mod error;
pub mod gateway_status; // Phase 49.3 Plan 06 (D-08): versioned gateway heartbeat status schema
pub mod judge; // Phase 49.7 Plan 02 (D-03): judge contract moved from ironhermes-kanban
pub mod memory_provider;
pub mod memory_store;
pub mod model_metadata;
pub mod models_cache;
pub mod net_class; // Phase 58.3.1 (D-11): loopback/LAN endpoint classification
pub mod observed_windows; // Phase 58.3.1 (D-07): probed/learned context-window overlay
pub mod pricing;
pub mod pricing_cache;
pub mod profile;
pub mod profile_credentials; // Phase 51 Plan 06 (D-11/D-12/D-13): host facade for the per-profile credential endpoint
pub mod provider;
pub mod provider_env; // Phase 51 Plan 15 (CR-04): single shared provider->api-key-env-var resolver
pub mod queue;
pub mod session;
pub mod sites; // Phase 54.8 Plan 08 (D-20/D-11/D-12): site project registry
pub mod skills;
pub mod ssrf;
pub mod stt; // Phase 36.17.8
pub mod tier2; // Phase 58.2 D-10: Tier 2 memory handle contract (impl in ironhermes-tier2)
pub mod token_estimator;
pub mod tts; // Phase 36.17.5
pub mod types;
pub mod vault; // Phase 46.8 UAT gap G-46.8-1
pub mod voice_endpoint; // Phase 54.7 (D-01/D-03/D-04/D-11): pure voice endpoint validators
pub mod voice_http; // Phase 54.7 (D-02/D-04): shared no-redirect voice HTTP primitives
pub mod webhook_route; // Phase 36.7.1: inbound webhook route config schema
pub mod wizard;
pub mod workspace;

pub use approval_gate::{
    ApprovalGate, ApprovalOutcome, SecurityEvent, SecurityEventKind, YoloBypass, YoloBypassSummary,
    publish_security_event, sanitize_notice_field, subscribe_security_events,
};
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
    CONTEXT_FILE_MAX_CHARS, scan_context_content, scan_context_content_for, truncate_content,
    truncate_on_char_boundary,
};
pub use env_sanitize::build_terminal_safe_env;
pub use error::{HermesError, Result};
pub use memory_provider::{MemoryEntries, MemoryProvider};
pub use memory_store::{MemoryStore, MemoryTarget};
pub use model_metadata::{ModelCapabilities, ModelMetadata, ModelRegistry, PartialModelMetadata};
pub use models_cache::{
    FetchResult, ModelsCache, ModelsCacheEntry, ProviderProbeOutcome, configured_id_drift,
    fetch_all, fetch_from_models_dev, fetch_from_openrouter, fetch_from_provider,
    normalize_model_id, parse_provider_models_response,
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
