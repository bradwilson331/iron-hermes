use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use ironhermes_core::auth::AuthStore;
use ironhermes_core::config::ToolsConfig;
use ironhermes_core::constants::get_hermes_home;
use ironhermes_core::{Config, ProviderResolver, SkillRecord, SkillRegistry, SubagentConfig};
use ironhermes_cron::JobStore;
use ironhermes_exec::process_registry::ProcessRegistry;
use ironhermes_hooks::{
    BlocklistGuardrail, HookRegistry, HooksConfig, McpMutationGuardrail, RetryQueue,
    WranglerMutationGuardrail, create_jsonl_listener, create_webhook_listener, drain_retry_queue,
};
use ironhermes_mcp::{McpManager, McpServerConfig};
use ironhermes_tools::ToolRegistry;
use ironhermes_tools::browser_session::BrowserSession;
use ironhermes_tools::delegate_task::{SubagentProgressCallback, SubagentRunner};
use ironhermes_tools::memory_tool::SharedMemoryManager;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::{AnyClientSummarizationHandle, AnyClientVisionHandle};

#[derive(Clone)]
pub struct DelegateTaskWiring {
    pub runner: Arc<dyn SubagentRunner>,
    pub semaphore: Arc<tokio::sync::Semaphore>,
    pub config: SubagentConfig,
    pub cancel_token: Option<CancellationToken>,
    pub progress_callback: Option<SubagentProgressCallback>,
}

/// Phase 36.17.7 Plan 01 (D-05 prep): derives `Default` so
/// `agent_runtime.rs::from_config` can build the startup bundle via
/// `..Default::default()`. That rewrite eliminates the literal `session_key: None,`
/// from `agent_runtime.rs` so the D-05 invariant in Plan 05 Task 6 GREENs.
/// All field types either already implement `Default` (`Arc<Config>`,
/// `PathBuf`, `HooksConfig`, `Option<_>`, `bool`) or had `Default` added
/// in Plan 01 (`Arc<ProviderResolver>`, `Arc<RwLock<ProcessRegistry>>`).
/// The default-constructed value is never used in production — callers always
/// supply real fields and `..Default::default()` only fills the residual
/// (`session_key`, `telegram_adapter`) Option slots.
#[derive(Clone, Default)]
pub struct AppRuntimeFactoryInput {
    pub config: Arc<Config>,
    pub resolver: Arc<ProviderResolver>,
    pub cwd: PathBuf,
    pub process_registry: Arc<RwLock<ProcessRegistry>>,
    pub memory_manager: Option<SharedMemoryManager>,
    pub delegate_task: Option<DelegateTaskWiring>,
    pub hooks_config: HooksConfig,
    pub emit_mcp_startup_logs: bool,
    /// Phase 36.17.5 D-15: per-session SessionKey for SendAudioTool dispatch.
    /// None at startup / global construction; Some when build_app_runtime_bundle is
    /// called per-session (gateway handler, CLI run_chat init).
    pub session_key: Option<ironhermes_core::SessionKey>,
    /// Phase 36.17.5 D-13 / D-15: gateway-supplied audio dispatcher for the Telegram arm.
    /// None for CLI/Local-only paths.
    pub telegram_adapter: Option<Arc<dyn ironhermes_tools::AudioDispatcher>>,
}

pub struct AppRuntimeBundle {
    pub registry: Arc<RwLock<ToolRegistry>>,
    pub hook_registry: Arc<HookRegistry>,
    pub mcp_manager: Option<Arc<McpManager>>,
    pub skill_registry: Arc<SkillRegistry>,
    pub active_skills: Arc<std::sync::Mutex<Vec<SkillRecord>>>,
    pub browser_session: Arc<tokio::sync::Mutex<Option<BrowserSession>>>,
    pub job_store: Arc<Mutex<JobStore>>,
    /// Phase 27.1.1-gap-02: the merged ToolsConfig (config.tools with ALL_TOOLSETS
    /// defaults filled in). Pass to RegistryToolsetSession::new and PromptBuilder
    /// construction sites instead of the raw config.tools so /toolset enable|disable
    /// mutates from the same baseline as the registry filter.
    pub merged_tools: ToolsConfig,
    /// Phase 39.2: black-box recorder initialized once per bundle and shared across
    /// all turns. AgentRuntime::from_config moves this Arc into its bb_recorder field
    /// so every run_turn is recorded to ~/.ironhermes/blackbox-YYYY-MM-DD.jsonl.
    /// None signals recording disabled (e.g., test harnesses without a home dir).
    pub bb_recorder: Option<Arc<ironhermes_blackbox::BlackBoxRecorder>>,
}

pub async fn build_app_runtime_bundle(
    input: AppRuntimeFactoryInput,
) -> anyhow::Result<AppRuntimeBundle> {
    // Phase 27.1.1-gap-02: compute the merged config once at bundle construction.
    // with_default_toolsets_merged() fills absent toolset entries with enabled=true
    // (back-compat: old configs that predate a toolset keep full access), while
    // preserving any explicit enabled:false entries from the user's config.yaml.
    let merged_tools = input.config.tools.clone().with_default_toolsets_merged();

    // Phase 41.3 Plan 11 (D-19): resolve the tool-credential snapshot HERE —
    // the async registration seam — before any tool is constructed. Mirrors
    // the production vault-open shape at `main.rs:4868-4870` exactly: only
    // open the store when the operator enabled the vault (T-41.3-53), and
    // route through `resolve_vault_config` rather than the raw
    // `input.config.vault`, because the unresolved `data_dir` sentinel is
    // exactly what `vault_resolve_integration.rs` exists to catch.
    let tool_credentials = {
        let store = if input.config.vault.enabled {
            Some(
                ironhermes_vault::open_store(&ironhermes_core::resolve_vault_config(
                    &input.config,
                ))
                .context("failed to open the vault store while resolving tool credentials")?,
            )
        } else {
            None
        };
        // `?` propagates loudly (D-19 hard error): a sealed/locked/corrupt
        // vault stops startup with an error naming the vault backend and the
        // fact that tool-credential resolution failed — never a `warn` and a
        // silently keyless default snapshot.
        Arc::new(
            ironhermes_tools::credentials::ToolCredentials::resolve(
                &input.config,
                store.as_deref(),
            )
            .await
            .context(
                "tool-credential resolution failed while opening the configured vault backend",
            )?,
        )
    };

    // Phase 36.3.12 GAP 1 (D-01/D-06/D-07/D-09): thread the operator's full resolved
    // TerminalConfig (not just the env allowlist) so `terminal.backend: docker`/`ssh`
    // actually reaches `create_environment` instead of silently no-opping to local.
    let mut registry = build_registry_with_process_registry(
        input.process_registry.clone(),
        &input.config.terminal,
        tool_credentials.clone(),
    );

    if let Some(ref manager) = input.memory_manager {
        registry.register_memory_tool(manager.clone());
    }

    // Phase 47 Plan 08 (D-08/D-09): the single per-session GenerationGuardrail
    // chokepoint — constructed ONCE here and shared (via `Arc`) across every
    // gen-tool registration below AND `register_delegate_task_tool`, so a
    // chat session's in-process delegate descendants draw from the SAME
    // `session_pool` this session's own Root reservations are exempt from
    // (D-08). `root_id` only matters when a `GenerationLedger` is attached
    // (kanban's cross-process arm, Task 3/main.rs) — Root reservations never
    // consult it, and in-process delegate descendants share the counter via
    // the `Arc` itself, not the `root_id` string.
    //
    // Deliberately NOT gated on `surfaces.chat`: a `Root` reservation always
    // succeeds regardless of the guardrail's presence (see
    // `GenerationGuardrail::try_reserve`'s doc), so gating construction on
    // `surfaces.chat` would have NO observable effect on the chat surface
    // itself — but it WOULD silently strip the guardrail out from under this
    // session's delegate descendants too (which are gated independently, by
    // `surfaces.delegate`, not `surfaces.chat`). Building it unconditionally
    // keeps the two surface toggles independent (GEN-04 adjacency).
    let root_id = input
        .session_key
        .as_ref()
        .map(|k| k.to_string_key())
        .unwrap_or_else(|| "session".to_string());
    let generation_guardrail = Arc::new(ironhermes_tools::gen_guardrail::GenerationGuardrail::new(
        input.config.generation.guardrails.session_pool,
        input.config.generation.guardrails.per_child_cap,
        root_id,
    ));

    if let Some(ref delegate) = input.delegate_task {
        // D-09: the delegate wall stays up (`"generation"` toolset group
        // resolves EMPTY) unless `surfaces.delegate` is true — `None` here is
        // what makes that fail-closed, since `DelegateTaskTool` consults
        // `self.gen_wiring.is_some()` as its sole "generation enabled" signal.
        let generation_wiring = if input.config.generation.guardrails.surfaces.delegate {
            Some(ironhermes_tools::delegate_task::GenerationSurfaceWiring {
                config: input.config.clone(),
                guardrail: generation_guardrail.clone(),
                capture_sink: Some(Arc::new(
                    ironhermes_tools::delegate_task::DelegateGenCaptureSink::new(),
                )),
            })
        } else {
            None
        };
        registry.register_delegate_task_tool(
            delegate.runner.clone(),
            delegate.semaphore.clone(),
            input.memory_manager.clone(),
            delegate.config.clone(),
            delegate.cancel_token.clone(),
            delegate.progress_callback.clone(),
            generation_wiring,
        );
    }

    let cron_dir = get_hermes_home().join("cron");
    let job_store = Arc::new(Mutex::new(JobStore::open(cron_dir)?));
    registry.register_cronjob_tool(job_store.clone());

    // Phase 46.6 Plan 04 (D-01): artifact tool — visible top-level publish
    // capability for the interactive chat surface (and any other caller of
    // this shared factory). Registered unconditionally, mirroring
    // register_image_gen_tool / register_cronjob_tool (config-taking tools
    // registered here regardless of toolset visibility); the "artifacts"
    // toolset entry (Plan 02, ALL_TOOLSETS) controls whether the LLM sees it
    // in get_definitions(), same as every other tool in this block. Do NOT
    // add this to build_rpc_registry below — the execute_code sandbox must
    // stay curated (see that function's SAFETY comment); artifact publishing
    // is a visible top-level tool call, not a sandbox-nested capability.
    registry.register_artifact_tool(input.config.clone());

    // Phase 47 Plan 08 (GEN-05/D-08): the chat surface's gen tools are always
    // constructed `Root` — a `Root` reservation is bounded exclusively by the
    // co-located per-section `session_cap` below (never `per_child_cap`, never
    // the descendant `session_pool`); see the guardrail construction comment
    // above for why this is unconditional on `surfaces.chat`.
    let chat_guardrail = Some((
        generation_guardrail.clone(),
        ironhermes_tools::gen_guardrail::ReservationKind::Root,
    ));

    // Phase 01: image_gen text-to-image tool. Registered unconditionally — the
    // tool's is_available() hides it from the LLM schema when FAL_KEY is unset
    // (GEN-03 / SAFE-01), so no fal client is built and no key is read at boot.
    // Plan 03 (SAFE-05 / D-03): per-session — the tool carries the concrete
    // SessionKey (when this bundle is built for a session) and shares the
    // registry's counter so the generation cap is scoped per chat session. The
    // interim-ack sink (D-04) is None here; the gateway may wire one later.
    registry.register_image_gen_tool(
        input.config.clone(),
        input.session_key.clone(),
        None,
        chat_guardrail.clone(),
    );

    // Phase 36.3.3 Plan 02: video_gen (T2V) + video_animate (I2V) tools.
    // Same pattern as image_gen — registered unconditionally; is_available() hides
    // them when FAL_KEY is unset (D-12). Per-session when session_key is Some so
    // the video cap is scoped per chat session (D-06, Pitfall 8).
    registry.register_video_gen_tools(
        input.config.clone(),
        input.session_key.clone(),
        None,
        chat_guardrail.clone(),
    );

    // Phase 47 Plan 08: video_to_video (v2v) — registered wherever the
    // sibling video tools are (D-14's net-new mode).
    registry.register_video_to_video_tool(
        input.config.clone(),
        input.session_key.clone(),
        None,
        chat_guardrail,
    );

    // Phase 36.17.5 D-15: TTS tools — registered only when this bundle is built
    // for a concrete session. session_key is None at startup / global construction,
    // in which case the tools are not exposed to the agent that turn (D-14 — tools
    // are tied to the agent loop, not the runtime).
    if let Some(ref session_key) = input.session_key {
        registry.register_tts_tools(
            session_key.clone(),
            input.telegram_adapter.clone(),
            input.config.clone(),
        );
    }

    let browser_session: Arc<tokio::sync::Mutex<Option<BrowserSession>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    let vision_handle = Arc::new(AnyClientVisionHandle::new(input.resolver.clone()));
    registry.register_browser_tools_with_vision(
        browser_session.clone(),
        input.resolver.clone(),
        vision_handle,
        input.config.clone(),
    );

    let skill_registry = Arc::new(SkillRegistry::load_with_config(
        &input.cwd,
        &input.config.skills,
    ));
    let active_skills: Arc<std::sync::Mutex<Vec<SkillRecord>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let credential_dir =
        ironhermes_tools::skills_tool::default_credential_dir(&input.config.skills);
    registry.register_skills_tool(
        skill_registry.clone(),
        active_skills.clone(),
        credential_dir,
        HashMap::new(),
    );

    // Phase 33 LEARN-04 / LEARN-05: skill_manage backs the 'learning' toolset
    // (autonomous skill authoring). Stateless tool — registers unconditionally;
    // visibility to the LLM is controlled by set_toolset_config below, which
    // filters tools whose toolset (`learning` here) is disabled in the merged
    // ToolsConfig. This mirrors register_skills_tool's pattern (always register;
    // toolset filter hides the schema).
    registry.register_skill_manage_tool();

    let summarization_handle = Arc::new(AnyClientSummarizationHandle::new(input.resolver.clone()));
    registry.register_web_extract_tool(summarization_handle, skill_registry.clone());

    let rpc_registry = build_rpc_registry(input.memory_manager.clone(), tool_credentials.clone());
    registry.register_execute_code_tool_with_process_registry(
        rpc_registry,
        input.config.exec.clone(),
        active_skills.clone(),
        input.process_registry.clone(),
    );

    if !input.hooks_config.blocked_tools.is_empty() {
        registry.add_guardrail(Box::new(BlocklistGuardrail::from_config(
            &input.hooks_config,
        )));
    }
    // Phase 45 D-08/D-09: intercept destructive-verb MCP tool calls.
    // Registered after BlocklistGuardrail (D-09 order: Blocklist → McpMutation).
    // Unconditional — has built-in DEFAULT_VERBS; operator config can override.
    registry.add_guardrail(Box::new(McpMutationGuardrail::from_config(
        &input.config.mcp_mutation_guardrail,
    )));
    // Phase 46 D-06: intercept `wrangler deploy`/`wrangler delete` terminal calls.
    // Registered after McpMutationGuardrail (D-09 order: Blocklist → McpMutation →
    // WranglerMutation). Unconditional — no operator override this phase.
    registry.add_guardrail(Box::new(WranglerMutationGuardrail::new()));
    registry.set_error_detail(input.hooks_config.error_detail.clone());

    // Phase 27.1.1-gap-02: push the merged toolset config into the registry AFTER
    // all register_* calls so get_definitions() filters against the full tool set.
    // Before this call, toolset_config is None → every toolset passes (no-op filter).
    // After this call, any toolset with enabled:false in the user's config.yaml is
    // hidden from the LLM's tool schema without requiring a /toolset command.
    registry.set_toolset_config(Some(merged_tools.clone()));

    let registry = Arc::new(RwLock::new(registry));
    let mcp_manager = build_mcp_manager(
        input.config.as_ref(),
        registry.clone(),
        input.emit_mcp_startup_logs,
    )
    .await;
    let hook_registry = build_hook_registry(&input.hooks_config).await?;

    // Phase 39.2: initialize the black-box recorder once per bundle.
    // Uses get_hermes_home() per MEMORY.md rule (never HERMES_HOME / std::env::home_dir).
    let bb_recorder = Some(Arc::new(ironhermes_blackbox::BlackBoxRecorder::new(
        "hermes-2026-06".to_string(),
        get_hermes_home(),
    )));

    Ok(AppRuntimeBundle {
        registry,
        hook_registry,
        mcp_manager,
        skill_registry,
        active_skills,
        browser_session,
        job_store,
        merged_tools,
        bb_recorder,
    })
}

/// Build the main tool registry for all production entry points (CLI REPL, CLI batch,
/// ratatui TUI, iron_hermes_ui, gateway). Delegates to the canonical entry point
/// `ToolRegistry::register_defaults_except` — adding a new tool to that method
/// makes it visible here automatically with zero additional edits.
fn build_registry_with_process_registry(
    process_registry: Arc<RwLock<ProcessRegistry>>,
    terminal_config: &ironhermes_core::config::TerminalConfig,
    credentials: Arc<ironhermes_tools::credentials::ToolCredentials>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    // Phase 41.3 Plan 11 (D-19): install the resolved snapshot BEFORE
    // register_defaults_except runs, so the credential-bearing tools it
    // constructs (web_search) read it instead of the env-only default.
    registry.with_credentials(credentials);
    // Skip the plain TerminalTool; add the process-registry-wired variant below
    // so background terminal spawns flow through drain_and_kill_session.
    registry.register_defaults_except(&["terminal"]);
    // Phase 36.3.12 GAP 1 (D-01/D-06/D-07/D-09): pass the operator's full resolved
    // TerminalConfig through so the wired TerminalTool applies both
    // build_terminal_safe_env (env allowlist) AND the configured backend
    // (docker/ssh/local) instead of silently defaulting to local.
    registry.register_terminal_tool_with_process_registry(process_registry, terminal_config);
    registry
}

/// Build the RPC sub-registry handed to execute_code's nested calls.
///
/// SAFETY: this sub-registry intentionally does NOT inherit from register_defaults().
/// execute_code runs agent-generated code in a sandboxed context and must NOT gain
/// new capabilities silently when tools are added to the default set. In particular:
///   - terminal: excluded — nested shell execution from within execute_code is not permitted.
///   - execute_code: excluded — recursive execute_code is not permitted.
///   - hexapod_tcp: excluded — hexapod hardware control must not be accessible from
///     within the execute_code sandbox. Hardware commands require explicit top-level
///     tool calls so the user sees them in the trajectory.
///
/// When a new default tool is added and you believe it belongs in the RPC sandbox,
/// add it here explicitly with a rationale comment. Do not switch this to delegating
/// register_defaults_except() without a security review.
fn build_rpc_registry(
    memory_manager: Option<SharedMemoryManager>,
    credentials: Arc<ironhermes_tools::credentials::ToolCredentials>,
) -> Arc<ToolRegistry> {
    use ironhermes_tools::file_tools::{
        PatchFileTool, ReadFileTool, SearchFilesTool, WriteFileTool,
    };
    use ironhermes_tools::web_answer::WebAnswerTool;
    use ironhermes_tools::web_read::WebReadTool;
    use ironhermes_tools::web_search::WebSearchTool;

    let mut rpc_registry = ToolRegistry::new();
    // Phase 41.3 Plan 11 (D-19): the RPC sandbox gets the SAME resolved
    // snapshot as the default registry — this widens no capability (the
    // sandbox's hand-rolled tool list and security posture are untouched,
    // see this function's SAFETY comment above); it only gives web_search a
    // credential source instead of defaulting to env-only.
    rpc_registry.with_credentials(credentials.clone());
    rpc_registry.register(Box::new(ReadFileTool));
    rpc_registry.register(Box::new(WriteFileTool));
    rpc_registry.register(Box::new(PatchFileTool));
    rpc_registry.register(Box::new(SearchFilesTool));
    rpc_registry.register(Box::new(WebSearchTool::new(credentials.clone())));
    // Phase 41.3 Plan 08 (D-07): web_answer is the same trust class as
    // web_search — a read-only outbound HTTP call, no filesystem/process/
    // hardware access — so it belongs in the sandbox alongside it, per this
    // function's own SAFETY comment ("add it here explicitly with a
    // rationale comment").
    rpc_registry.register(Box::new(WebAnswerTool::new(credentials)));
    rpc_registry.register(Box::new(WebReadTool));
    if let Some(manager) = memory_manager {
        rpc_registry.register_memory_tool(manager);
    }
    Arc::new(rpc_registry)
}

async fn build_hook_registry(hooks_config: &HooksConfig) -> anyhow::Result<Arc<HookRegistry>> {
    let mut hook_registry = HookRegistry::new(hooks_config.clone());

    if hooks_config.event_log.enabled {
        let log_path = hooks_config
            .event_log
            .path
            .as_ref()
            .map(std::path::PathBuf::from);
        hook_registry.add_listener(create_jsonl_listener(log_path));
    }

    let retry_queue = Arc::new(
        RetryQueue::new(RetryQueue::default_path())
            .context("Failed to initialize webhook retry queue")?,
    );
    for endpoint in &hooks_config.webhooks {
        hook_registry.add_listener(create_webhook_listener(
            endpoint.clone(),
            retry_queue.clone(),
        ));
    }

    let default_ttl = hooks_config
        .webhooks
        .first()
        .and_then(|endpoint| endpoint.queue_ttl_hours)
        .unwrap_or(24);
    drain_retry_queue(retry_queue, &hooks_config.webhooks, default_ttl).await;

    Ok(Arc::new(hook_registry))
}

async fn build_mcp_manager(
    config: &Config,
    registry: Arc<RwLock<ToolRegistry>>,
    emit_mcp_startup_logs: bool,
) -> Option<Arc<McpManager>> {
    let mcp_configs: HashMap<String, McpServerConfig> = config
        .mcp_servers
        .iter()
        .filter_map(|(name, value)| {
            serde_yaml::from_value::<McpServerConfig>(value.clone())
                .ok()
                .map(|cfg| (name.clone(), cfg))
        })
        .collect();

    if mcp_configs.is_empty() {
        return None;
    }

    // 46.1-03 (D-04): build ns -> server-url map for the real rmcp-backed
    // RefreshFn — only configs that carry both oauth_provider and url can refresh.
    let ns_to_url: HashMap<String, String> = mcp_configs
        .values()
        .filter_map(|cfg| {
            let ns = cfg.oauth_provider.as_deref()?;
            let url = cfg.url.as_deref()?;
            Some((ns.to_string(), url.to_string()))
        })
        .collect();

    // 44-05 / 46.1-03: open auth store for OAuth-enabled MCP servers, wired to
    // the REAL rmcp-backed refresh function (D-04, D-05) — not the stub.
    // Failure is non-fatal — OAuth servers are skipped with warn (D-04 headless posture).
    let auth_store: Option<Arc<AuthStore>> = match ironhermes_mcp::open_auth_store_with_mcp_refresh(
        get_hermes_home().join("auth.json"),
        ns_to_url,
    )
    .await
    {
        Ok(store) => Some(store),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "MCP: could not open OAuth token store; OAuth servers will be skipped (D-04)"
            );
            None
        }
    };

    // Spawn Phase 41 proactive refresh tasks for cached MCP OAuth namespaces
    // that actually have refresh capability (D-04: a namespace with no
    // refresh_token is never scheduled into a repeating failing refresh).
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(ref store) = auth_store {
        for cfg in mcp_configs.values() {
            if let Some(ns) = cfg.oauth_provider.as_deref()
                && let Some(tok) = store.get_token(ns).await
                && tok.refresh_token.is_some()
            {
                AuthStore::spawn_refresh_task(store.clone(), ns.to_string());
            }
        }
    }

    let manager = Arc::new(
        McpManager::new(registry)
            .with_auth_store(auth_store)
            .with_global_issuer_allowlist(config.mcp_oauth.issuer_allowlist.clone()),
    );
    let manager_clone = manager.clone();
    let configs_clone = mcp_configs.clone();
    let expected_server_count = mcp_configs.len();
    tokio::spawn(async move {
        manager_clone.start_all(configs_clone).await;

        if emit_mcp_startup_logs {
            let connected = manager_clone.connected_server_names();
            let tool_count = manager_clone.registered_tool_count().await;
            if connected.is_empty() {
                tracing::warn!("MCP startup completed with no connected servers");
            } else if connected.len() < expected_server_count {
                tracing::info!(
                    connected_servers = connected.len(),
                    requested_servers = expected_server_count,
                    registered_tools = tool_count,
                    "MCP startup completed with partial connectivity"
                );
            } else {
                tracing::info!(
                    connected_servers = connected.len(),
                    registered_tools = tool_count,
                    "MCP startup completed"
                );
            }
        }
    });

    Some(manager)
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use ironhermes_hooks::GuardrailDecision;
    use ironhermes_tools::delegate_task::ChildToolProgressCallback;

    fn default_input() -> AppRuntimeFactoryInput {
        let config = Config::default();
        let resolver =
            ProviderResolver::build(&config).expect("resolver should build from default config");
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        AppRuntimeFactoryInput {
            config: Arc::new(config),
            resolver: Arc::new(resolver),
            cwd,
            process_registry: Arc::new(RwLock::new(ProcessRegistry::new_for_session(
                "runtime-factory-test".to_string(),
            ))),
            memory_manager: None,
            delegate_task: None,
            hooks_config: HooksConfig::default(),
            emit_mcp_startup_logs: false,
            session_key: None,
            telegram_adapter: None,
        }
    }

    #[tokio::test]
    async fn factory_bundle_registers_browser_and_web_extract_tools() {
        let bundle = build_app_runtime_bundle(default_input())
            .await
            .expect("factory bundle should build");

        let names = bundle
            .registry
            .read()
            .await
            .list_tools()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();

        assert!(
            names.iter().any(|name| name == "browser_navigate"),
            "browser_navigate should be registered; got {names:?}"
        );
        assert!(
            names.iter().any(|name| name == "web_extract"),
            "web_extract should be registered; got {names:?}"
        );
    }

    /// Phase 46.6 Plan 04 (D-01): the chat agent registry (built by
    /// build_app_runtime_bundle) must include the `artifact` tool.
    #[tokio::test]
    async fn factory_bundle_registers_artifact_tool() {
        let bundle = build_app_runtime_bundle(default_input())
            .await
            .expect("factory bundle should build");

        let names = bundle
            .registry
            .read()
            .await
            .list_tools()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();

        assert!(
            names.iter().any(|name| name == "artifact"),
            "artifact tool should be registered on the app-runtime bundle; got {names:?}"
        );
    }

    #[tokio::test]
    async fn factory_applies_blocklist_guardrail_from_hooks_config() {
        let mut input = default_input();
        input.hooks_config.blocked_tools = vec!["terminal".to_string()];

        let bundle = build_app_runtime_bundle(input)
            .await
            .expect("factory bundle should build");

        let decision = bundle
            .registry
            .read()
            .await
            .check_guardrails("terminal", &serde_json::json!({}));

        assert!(
            matches!(decision, GuardrailDecision::Block { .. }),
            "terminal should be blocked when blocked_tools includes terminal"
        );
    }

    struct NoopRunner;

    #[async_trait]
    impl SubagentRunner for NoopRunner {
        async fn run_child(
            &self,
            _registry: Arc<RwLock<ToolRegistry>>,
            _system_prompt: String,
            _max_iterations: usize,
            _model_override: Option<&str>,
            _cancel_token: Option<CancellationToken>,
            _tool_progress: Option<ChildToolProgressCallback>,
            _stale_warn_seconds: u64,
        ) -> anyhow::Result<Option<String>> {
            Ok(Some("ok".to_string()))
        }
    }

    #[tokio::test]
    async fn factory_registers_delegate_task_when_wiring_is_provided() {
        let mut input = default_input();
        let config = input.config.clone();
        input.delegate_task = Some(DelegateTaskWiring {
            runner: Arc::new(NoopRunner),
            semaphore: Arc::new(tokio::sync::Semaphore::new(2)),
            config: config.delegation.clone(),
            cancel_token: None,
            progress_callback: None,
        });

        let bundle = build_app_runtime_bundle(input)
            .await
            .expect("factory bundle should build");

        let names = bundle
            .registry
            .read()
            .await
            .list_tools()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();

        assert!(
            names.iter().any(|name| name == "delegate_task"),
            "delegate_task should be registered when delegate wiring is provided"
        );
    }

    #[test]
    fn source_locks_registration_order_markers() {
        let source = include_str!("app_runtime_factory.rs");

        assert_in_order(
            source,
            "register_memory_tool(",
            "register_delegate_task_tool(",
        );
        assert_in_order(
            source,
            "register_delegate_task_tool(",
            "register_cronjob_tool(",
        );
        assert_in_order(source, "register_cronjob_tool(", "register_artifact_tool(");
        assert_in_order(
            source,
            "register_artifact_tool(",
            "register_browser_tools_with_vision(",
        );
        assert_in_order(
            source,
            "register_browser_tools_with_vision(",
            "register_skills_tool(",
        );
        assert_in_order(
            source,
            "register_skills_tool(",
            "register_web_extract_tool(",
        );
        assert_in_order(
            source,
            "register_web_extract_tool(",
            "register_execute_code_tool_with_process_registry(",
        );
    }

    #[test]
    fn source_locks_hook_registry_markers() {
        let source = include_str!("app_runtime_factory.rs");

        assert!(
            source.contains("HookRegistry::new"),
            "HookRegistry::new marker missing from runtime factory"
        );
        assert!(
            source.contains("create_jsonl_listener"),
            "create_jsonl_listener marker missing from runtime factory"
        );
        assert!(
            source.contains("create_webhook_listener"),
            "create_webhook_listener marker missing from runtime factory"
        );
        assert!(
            source.contains("drain_retry_queue"),
            "drain_retry_queue marker missing from runtime factory"
        );
    }

    fn assert_in_order(source: &str, first: &str, second: &str) {
        let first_index = source
            .find(first)
            .unwrap_or_else(|| panic!("missing marker: {first}"));
        let second_index = source
            .find(second)
            .unwrap_or_else(|| panic!("missing marker: {second}"));
        assert!(
            first_index < second_index,
            "marker order mismatch: '{first}' must appear before '{second}'"
        );
    }

    // -------------------------------------------------------------------------
    // Phase 27.1.1-gap-02: toolset_config startup wiring tests
    // -------------------------------------------------------------------------

    /// Test that a config with `web: { enabled: false }` causes build_app_runtime_bundle
    /// to produce a registry where web_search / web_read / web_extract are absent from
    /// get_definitions(None), while non-web tools (read_file, terminal) remain.
    #[tokio::test]
    async fn test_toolset_config_filters_disabled_web() {
        use ironhermes_core::config::ToolsetEntry;
        let mut config = Config::default();
        // Explicitly disable the web toolset.
        config
            .tools
            .toolsets
            .insert("web".to_string(), ToolsetEntry { enabled: false });

        let resolver = ProviderResolver::build(&config).expect("resolver should build");
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let input = AppRuntimeFactoryInput {
            config: Arc::new(config),
            resolver: Arc::new(resolver),
            cwd,
            process_registry: Arc::new(RwLock::new(ProcessRegistry::new_for_session(
                "test-web-filter".to_string(),
            ))),
            memory_manager: None,
            delegate_task: None,
            hooks_config: HooksConfig::default(),
            emit_mcp_startup_logs: false,
            session_key: None,
            telegram_adapter: None,
        };

        let bundle = build_app_runtime_bundle(input)
            .await
            .expect("factory bundle should build");

        let defs = bundle.registry.read().await.get_definitions(None);
        let names: Vec<String> = defs.iter().map(|d| d.function.name.clone()).collect();

        assert!(
            !names.contains(&"web_search".to_string()),
            "web_search must be hidden when web toolset is disabled; got: {:?}",
            names
        );
        assert!(
            !names.contains(&"web_read".to_string()),
            "web_read must be hidden when web toolset is disabled"
        );
        assert!(
            !names.contains(&"web_extract".to_string()),
            "web_extract must be hidden when web toolset is disabled"
        );
        // A tool from an enabled toolset must still appear. "skills" is in DEFAULT_TOOLSETS
        // (enabled by default) and has no external prereqs — it must survive the web filter.
        assert!(
            names.contains(&"skills".to_string()),
            "skills must still appear when web toolset is disabled; got: {:?}",
            names
        );
    }

    /// Test that a config with NO `robotics` entry (typical pre-gap-02 config) still
    /// exposes `hexapod_tcp` in get_definitions when HEXAPOD_IP is set, because
    /// with_default_toolsets_merged() adds robotics→enabled=true for absent entries.
    #[tokio::test]
    async fn test_toolset_config_robotics_default_enabled() {
        // Temporarily set HEXAPOD_IP so is_available() on HexapodTcpTool returns true.
        // Safety: env mutation is serialised by single-threaded test.
        let prev = std::env::var("HEXAPOD_IP").ok();
        unsafe {
            std::env::set_var("HEXAPOD_IP", "127.0.0.1");
        }

        let mut config = Config::default();
        // Remove any robotics entry — simulate pre-gap-02 config.
        config.tools.toolsets.remove("robotics");

        let resolver = ProviderResolver::build(&config).expect("resolver should build");
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let input = AppRuntimeFactoryInput {
            config: Arc::new(config),
            resolver: Arc::new(resolver),
            cwd,
            process_registry: Arc::new(RwLock::new(ProcessRegistry::new_for_session(
                "test-robotics-default".to_string(),
            ))),
            memory_manager: None,
            delegate_task: None,
            hooks_config: HooksConfig::default(),
            emit_mcp_startup_logs: false,
            session_key: None,
            telegram_adapter: None,
        };

        let bundle = build_app_runtime_bundle(input)
            .await
            .expect("factory bundle should build");

        let defs = bundle.registry.read().await.get_definitions(None);
        let names: Vec<String> = defs.iter().map(|d| d.function.name.clone()).collect();

        // Restore env.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("HEXAPOD_IP", v),
                None => std::env::remove_var("HEXAPOD_IP"),
            }
        }

        assert!(
            names.contains(&"hexapod_tcp".to_string()),
            "hexapod_tcp must be visible when HEXAPOD_IP is set and robotics not explicitly disabled; got: {:?}",
            names
        );
    }

    /// Test that a config with `robotics: { enabled: false }` hides `hexapod_tcp`
    /// even when HEXAPOD_IP is set.
    #[tokio::test]
    async fn test_toolset_config_robotics_explicitly_disabled() {
        use ironhermes_core::config::ToolsetEntry;

        let prev = std::env::var("HEXAPOD_IP").ok();
        unsafe {
            std::env::set_var("HEXAPOD_IP", "127.0.0.1");
        }

        let mut config = Config::default();
        config
            .tools
            .toolsets
            .insert("robotics".to_string(), ToolsetEntry { enabled: false });

        let resolver = ProviderResolver::build(&config).expect("resolver should build");
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let input = AppRuntimeFactoryInput {
            config: Arc::new(config),
            resolver: Arc::new(resolver),
            cwd,
            process_registry: Arc::new(RwLock::new(ProcessRegistry::new_for_session(
                "test-robotics-disabled".to_string(),
            ))),
            memory_manager: None,
            delegate_task: None,
            hooks_config: HooksConfig::default(),
            emit_mcp_startup_logs: false,
            session_key: None,
            telegram_adapter: None,
        };

        let bundle = build_app_runtime_bundle(input)
            .await
            .expect("factory bundle should build");

        let defs = bundle.registry.read().await.get_definitions(None);
        let names: Vec<String> = defs.iter().map(|d| d.function.name.clone()).collect();

        // Restore env.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("HEXAPOD_IP", v),
                None => std::env::remove_var("HEXAPOD_IP"),
            }
        }

        assert!(
            !names.contains(&"hexapod_tcp".to_string()),
            "hexapod_tcp must be hidden when robotics is explicitly disabled; got: {:?}",
            names
        );
    }

    /// Regression guard: set_toolset_config must be called AFTER the last register_*
    /// call (structural ordering). Source text check locks this ordering.
    #[test]
    fn source_locks_set_toolset_config_after_register_calls() {
        let source = include_str!("app_runtime_factory.rs");
        assert!(
            source.contains("set_toolset_config(Some(merged_tools"),
            "set_toolset_config(Some(merged_tools...) must appear in build_app_runtime_bundle"
        );
        assert_in_order(
            source,
            "register_execute_code_tool_with_process_registry(",
            "set_toolset_config(Some(merged_tools",
        );
        assert_in_order(
            source,
            "set_toolset_config(Some(merged_tools",
            "Arc::new(RwLock::new(registry))",
        );
    }
}
