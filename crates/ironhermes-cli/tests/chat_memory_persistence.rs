//! Plan 20-03 Task 02 — integration regression for Fix 2 of pending todo
//! 2026-04-16-chat-and-single-cli-modes-have-no-memory-wiring.
//!
//! These tests drive the same factory + MemoryManager code path that the
//! CLI's `run_chat` and `run_single` now depend on. They assert that a
//! memory write in one invocation is visible to the next invocation at
//! the same `IRONHERMES_HOME` — proving the disk persistence that chat
//! mode was previously missing.

use ironhermes_core::memory_store::MemoryTarget;
use tempfile::TempDir;

/// Process-global env lock — `IRONHERMES_HOME` is shared across the whole
/// test binary and multiple tests can race.
fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[tokio::test]
async fn memory_persists_across_invocations_with_file_provider() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    unsafe {
        std::env::set_var("IRONHERMES_HOME", tmp.path());
    }

    let mut cfg = ironhermes_core::config::MemoryConfig::default();
    cfg.provider = "file".to_string();

    // First invocation: build the manager, add a memory line.
    {
        let mgr = ironhermes_agent::memory::factory::build_memory_manager(&cfg)
            .await
            .expect("first build_memory_manager")
            .expect("memory_enabled is true so manager must be Some");
        {
            let guard = mgr.lock().await;
            guard
                .add(MemoryTarget::Memory, "persisted-file-fact")
                .await
                .expect("add memory");
        }
        // Drop the manager explicitly — in real CLI use, process exit closes
        // the handle.
        drop(mgr);
    }

    // Re-reassert env — the test-global lock prevents other tests from
    // stomping it, but being explicit protects against future re-ordering.
    unsafe {
        std::env::set_var("IRONHERMES_HOME", tmp.path());
    }

    // Second invocation: build again. The memory line must still be there.
    let mgr2 = ironhermes_agent::memory::factory::build_memory_manager(&cfg)
        .await
        .expect("second build_memory_manager")
        .expect("memory_enabled is true so manager must be Some");
    let guard2 = mgr2.lock().await;
    let block = guard2
        .format_for_system_prompt(MemoryTarget::Memory)
        .await
        .expect("memory block should be populated on second run");
    assert!(
        block.contains("persisted-file-fact"),
        "Fix 2 regression: file-provider memory did not persist across invocations; block was: {block}"
    );
}

#[cfg(feature = "memory-sqlite")]
#[tokio::test]
async fn memory_persists_across_invocations_with_sqlite_provider() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    unsafe {
        std::env::set_var("IRONHERMES_HOME", tmp.path());
    }

    let mut cfg = ironhermes_core::config::MemoryConfig::default();
    cfg.provider = "sqlite".to_string();

    let mgr1 = ironhermes_agent::memory::factory::build_memory_manager(&cfg)
        .await
        .expect("first sqlite build_memory_manager")
        .expect("memory_enabled is true so manager must be Some");
    {
        let guard = mgr1.lock().await;
        guard
            .add(MemoryTarget::Memory, "sqlite-cross-run-fact")
            .await
            .expect("add memory");
    }
    drop(mgr1);

    unsafe {
        std::env::set_var("IRONHERMES_HOME", tmp.path());
    }

    let mgr2 = ironhermes_agent::memory::factory::build_memory_manager(&cfg)
        .await
        .expect("second sqlite build_memory_manager")
        .expect("memory_enabled is true so manager must be Some");
    let guard2 = mgr2.lock().await;
    let block = guard2
        .format_for_system_prompt(MemoryTarget::Memory)
        .await
        .expect("sqlite memory block should be populated on second run");
    assert!(
        block.contains("sqlite-cross-run-fact"),
        "Fix 2 regression: sqlite memory did not persist across invocations; block was: {block}"
    );
}

/// Static text-level regression: guarantees the three wiring calls land in
/// both `run_chat` and `run_single` (Fix 2). If a future refactor deletes
/// or renames any of them this test fires a human-readable error before
/// a user ever hits the silent-memory-drop bug.
#[test]
fn run_chat_and_run_single_both_wire_memory_manager() {
    let main_src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
    )
    .expect("read main.rs");

    let build_manager_count = main_src.matches("build_memory_manager").count();
    let memory_manager_input_count = main_src.matches("memory_manager:").count();
    let set_manager_count = main_src.matches("set_memory_manager").count();

    assert!(
        build_manager_count >= 3,
        "expected >=3 build_memory_manager calls (run_gateway + run_chat + run_single), got {build_manager_count}"
    );
    // Phase 25.6 moved the actual `register_memory_tool` call into the shared
    // runtime factory (build_app_runtime_bundle). main.rs no longer registers the
    // tool inline; it builds the manager per run path and threads it into the
    // factory via the AppRuntimeFactoryInput `memory_manager:` field. Assert both
    // halves of that contract: the factory registers the tool, and main.rs feeds
    // the manager into the factory at every run path.
    let factory_src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ironhermes-agent/src/app_runtime_factory.rs"),
    )
    .expect("read app_runtime_factory.rs");
    assert!(
        factory_src.contains("register_memory_tool("),
        "Fix 2 regression: the shared runtime factory must call register_memory_tool() — \
         memory wiring moved here in Phase 25.6"
    );
    assert!(
        memory_manager_input_count >= 3,
        "expected >=3 `memory_manager:` factory-input fields (gateway + chat + single threading \
         the manager into build_app_runtime_bundle); got {memory_manager_input_count}"
    );
    assert!(
        set_manager_count >= 3,
        "expected >=3 set_memory_manager calls (gateway runner + chat prompt_builder + single prompt_builder); got {set_manager_count}"
    );
}
