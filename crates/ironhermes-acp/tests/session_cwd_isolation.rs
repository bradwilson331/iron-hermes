//! CLI-08 cwd-isolation contract test (Phase 36.8, plan 02, task 1).
//!
//! RESEARCH Pitfall 2: if a future regression reverts `AcpSessionManager::create` to
//! sharing ONE `AgentRuntime` across every session in a process (instead of building a
//! fresh one per session), the FIRST session's project root silently leaks into every
//! later session's context-file discovery. This test creates two ACP sessions rooted at
//! two distinct tempdirs, each holding a distinct `AGENTS.md`, and asserts each session's
//! REAL resolved `context_file_paths()` (read via `AgentRuntime`'s own introspection
//! accessor, not a hand-rolled recomputation) contains only its own directory's file.

use std::sync::{Arc, Mutex as StdMutex};

use ironhermes_acp::session_manager::AcpSessionManager;
use ironhermes_core::{Config, ModelsCache, ProviderConfig, ProviderResolver};
use ironhermes_state::StateStore;

fn build_config_and_resolver() -> (Arc<Config>, Arc<ProviderResolver>) {
    let mut config = Config::default();
    // `Config::default()`'s main provider is `openrouter` with its real production base_url,
    // and `ProviderResolver::build` resolves API keys from the process environment while
    // reading the operator's real `$IRONHERMES_HOME/models-cache.json`. Pin the endpoint to
    // loopback port 1 with a literal key so no env var is consulted, and use
    // `build_with_cache` so the static model table wins regardless of what is on disk.
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            base_url: Some("http://127.0.0.1:1/v1".to_string()),
            api_key: Some("test-key-not-a-real-credential".to_string()),
            ..Default::default()
        },
    );
    let resolver = ProviderResolver::build_with_cache(&config, ModelsCache::default())
        .expect("ProviderResolver::build_with_cache with isolated Config");
    (Arc::new(config), Arc::new(resolver))
}

fn build_state_store() -> (Arc<StdMutex<StateStore>>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir for state.db");
    let store = StateStore::new(tmp.path().join("state.db")).expect("StateStore::new");
    (Arc::new(StdMutex::new(store)), tmp)
}

#[tokio::test]
async fn two_sessions_rooted_at_different_cwds_discover_only_their_own_context_files() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let mut manager = AcpSessionManager::new(state_store, config, resolver);

    let dir_a = tempfile::tempdir().expect("tempdir a");
    let dir_b = tempfile::tempdir().expect("tempdir b");
    std::fs::write(dir_a.path().join("AGENTS.md"), "project A instructions")
        .expect("write dir_a AGENTS.md");
    std::fs::write(dir_b.path().join("AGENTS.md"), "project B instructions")
        .expect("write dir_b AGENTS.md");

    let id_a = manager
        .create(dir_a.path().to_path_buf(), None)
        .await
        .expect("create session a");
    let id_b = manager
        .create(dir_b.path().to_path_buf(), None)
        .await
        .expect("create session b");

    let agents_md_a = dir_a.path().join("AGENTS.md");
    let agents_md_b = dir_b.path().join("AGENTS.md");

    let session_a = manager.get(&id_a).expect("session a must exist");
    let paths_a = session_a.runtime.context_file_paths();
    assert!(
        paths_a.contains(&agents_md_a),
        "session A's resolved context-file paths must include its OWN directory's AGENTS.md"
    );
    assert!(
        !paths_a.contains(&agents_md_b),
        "session A's resolved context-file paths must NOT include session B's directory \
         (this is the exact regression a shared, process-wide AgentRuntime would reintroduce)"
    );

    let session_b = manager.get(&id_b).expect("session b must exist");
    let paths_b = session_b.runtime.context_file_paths();
    assert!(
        paths_b.contains(&agents_md_b),
        "session B's resolved context-file paths must include its OWN directory's AGENTS.md"
    );
    assert!(
        !paths_b.contains(&agents_md_a),
        "session B's resolved context-file paths must NOT include session A's directory"
    );
}
