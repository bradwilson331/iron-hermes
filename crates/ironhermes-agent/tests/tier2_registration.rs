//! Phase 58.2 Plan 03: Tier 2 tools are registered at every production composition root (D-12)
//! and absent when the flags are off (D-01).
//!
//! Every home is an explicit tempdir; nothing here touches `IRONHERMES_HOME`.

use std::sync::Arc;

use ironhermes_agent::app_runtime_factory::{
    AppRuntimeFactoryInput, build_app_runtime_bundle, build_rpc_registry,
};
use ironhermes_agent::memory::factory::build_memory_manager_at;
use ironhermes_core::config::{EmbeddingsConfig, MemoryConfig};
use ironhermes_tools::memory_tool::SharedMemoryManager;
use ironhermes_tools::tier2_tools::{TIER2_READ_ONLY_TOOL_NAMES, TIER2_TOOL_NAMES};
use tokio::sync::RwLock;

/// Tier 2 on, pointed at a refused loopback port so a CI `--all-features` run never starts a
/// built-in model download.
fn memory_cfg(tier2: bool) -> MemoryConfig {
    MemoryConfig {
        tier2_enabled: tier2,
        embeddings: EmbeddingsConfig {
            base_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn manager(home: &std::path::Path, tier2: bool) -> SharedMemoryManager {
    build_memory_manager_at(&memory_cfg(tier2), home)
        .await
        .unwrap()
        .expect("memory is enabled")
}

async fn bundle_registry(
    home: &std::path::Path,
    mgr: SharedMemoryManager,
) -> Arc<RwLock<ironhermes_tools::ToolRegistry>> {
    let config = ironhermes_core::Config::default();
    let resolver = ironhermes_core::ProviderResolver::build(&config)
        .expect("resolver should build from the default config");
    let input = AppRuntimeFactoryInput {
        config: Arc::new(config),
        resolver: Arc::new(resolver),
        cwd: home.to_path_buf(),
        process_registry: Arc::new(RwLock::new(
            ironhermes_exec::process_registry::ProcessRegistry::new_for_session(
                "tier2-registration".to_string(),
            ),
        )),
        memory_manager: Some(mgr),
        delegate_task: None,
        hooks_config: ironhermes_hooks::HooksConfig::default(),
        emit_mcp_startup_logs: false,
        session_key: None,
        telegram_adapter: None,
        resolver_handle: None,
    };
    build_app_runtime_bundle(input)
        .await
        .expect("factory bundle should build")
        .registry
}

#[tokio::test]
async fn tier2_registration_bundle_main_registry_has_all_tools() {
    let home = tempfile::tempdir().unwrap();
    let mgr = manager(home.path(), true).await;
    let reg = bundle_registry(home.path(), mgr).await;
    let reg = reg.read().await;
    let tools = reg.list_tools();
    for n in TIER2_TOOL_NAMES {
        assert!(tools.contains(n), "main registry missing Tier 2 tool {n}");
    }
}

#[tokio::test]
async fn tier2_registration_rpc_registry_read_only_subset() {
    let home = tempfile::tempdir().unwrap();
    let mgr = manager(home.path(), true).await;
    let rpc = build_rpc_registry(
        Some(mgr),
        Arc::new(ironhermes_tools::credentials::ToolCredentials::env_only()),
        None,
    );
    let tools = rpc.list_tools();
    for n in TIER2_READ_ONLY_TOOL_NAMES {
        assert!(tools.contains(n), "RPC registry missing read-only {n}");
    }
    for n in TIER2_TOOL_NAMES
        .iter()
        .filter(|n| !TIER2_READ_ONLY_TOOL_NAMES.contains(n))
    {
        assert!(!tools.contains(n), "RPC registry must not carry {n}");
    }
}

#[tokio::test]
async fn tier2_registration_cron_scope_includes_tools() {
    let home = tempfile::tempdir().unwrap();
    let mgr = manager(home.path(), true).await;
    let reg = bundle_registry(home.path(), mgr).await;
    let scoped = reg.read().await.scope_to(&["memory".to_string()]);
    let tools = scoped.list_tools();
    for n in TIER2_TOOL_NAMES {
        assert!(
            tools.contains(n),
            "cron-scoped registry lost Tier 2 tool {n}"
        );
    }
}

#[tokio::test]
async fn tier2_registration_tier2_off_registers_none() {
    let home = tempfile::tempdir().unwrap();
    let mgr = manager(home.path(), false).await;
    let reg = bundle_registry(home.path(), mgr).await;
    let reg = reg.read().await;
    let tools = reg.list_tools();
    assert!(tools.contains(&"memory"), "Tier 1 memory tool must remain");
    for n in TIER2_TOOL_NAMES {
        assert!(!tools.contains(n), "Tier 2 off but {n} is registered");
    }
}

/// Source locks: every root that registers the `memory` toolset goes through the shared seams.
/// Tier 2 attaches inside `build_memory_manager`, so every root that builds a manager gets it.
#[test]
fn tier2_registration_source_locks() {
    let factory = include_str!("../src/app_runtime_factory.rs");
    let tui = include_str!("../../ironhermes-cli/src/tui_rata/event_loop.rs");
    let cli_main = include_str!("../../ironhermes-cli/src/main.rs");
    let web = include_str!("../../iron_hermes_ui/src/server/state.rs");
    let acp = include_str!("../../ironhermes-acp/src/session_manager.rs");

    let sandboxed = "rpc_registry.register_memory_tool_sandboxed(";
    assert!(
        factory.contains(sandboxed),
        "factory RPC registry not sandboxed"
    );
    assert!(tui.contains(sandboxed), "TUI RPC registry not sandboxed");
    assert!(
        tui.contains("registry.register_memory_tool("),
        "TUI main registry must register the memory tool"
    );
    assert!(
        cli_main.matches("build_memory_manager(").count() >= 3,
        "ironhermes-cli main.rs must build a memory manager at its 3 roots"
    );
    for (name, src) in [("tui", tui), ("web", web), ("acp", acp)] {
        assert!(
            src.contains("build_memory_manager("),
            "{name} root must build its memory manager through the factory"
        );
    }
}
