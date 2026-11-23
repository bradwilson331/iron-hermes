//! Phase 58.2 Plan 13: the Tier 2 boot attach (D-10) and the all-flags-off guarantee (D-01).
//!
//! Every home is an explicit tempdir; nothing here touches `IRONHERMES_HOME`.

use ironhermes_agent::memory::factory::build_memory_manager_at;
use ironhermes_core::config::{EmbeddingsConfig, MemoryConfig};
use ironhermes_tools::ToolRegistry;
use ironhermes_tools::memory_tool::SharedMemoryManager;

/// Tier 2 on, pointed at a refused loopback port so a CI `--all-features` run never starts a
/// built-in model download.
fn tier2_on() -> MemoryConfig {
    MemoryConfig {
        tier2_enabled: true,
        embeddings: EmbeddingsConfig {
            base_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn boot_attaches_tier2_when_enabled() {
    let home = tempfile::tempdir().unwrap();
    let mgr = build_memory_manager_at(&tier2_on(), home.path())
        .await
        .unwrap()
        .expect("memory is enabled");
    let m = mgr.lock().await;
    assert!(m.tier2().is_some(), "tier2_enabled must attach the handle");
    assert!(
        m.mirror_handle().is_none(),
        "Tier 2 is never the mirror (D-10)"
    );
    assert!(home.path().join("tier2/tier2.db").is_file());
}

#[tokio::test]
async fn flags_off_creates_no_tier2_dir_and_registers_only_memory() {
    let home = tempfile::tempdir().unwrap();
    let mgr = build_memory_manager_at(&MemoryConfig::default(), home.path())
        .await
        .unwrap()
        .expect("memory is enabled");
    assert!(mgr.lock().await.tier2().is_none());
    assert!(
        !home.path().join("tier2").exists(),
        "D-01: no tier2/ dir with flags off"
    );

    let mut reg = ToolRegistry::new();
    reg.register_memory_tool(mgr as SharedMemoryManager);
    assert_eq!(reg.list_tools(), vec!["memory"]);
}

#[tokio::test]
async fn tier2_requires_memory_enabled() {
    let home = tempfile::tempdir().unwrap();
    let cfg = MemoryConfig {
        memory_enabled: false,
        ..tier2_on()
    };
    let mgr = build_memory_manager_at(&cfg, home.path()).await.unwrap();
    assert!(mgr.is_none());
    assert!(!home.path().join("tier2").exists());
}

#[tokio::test]
async fn invalid_tier2_config_keeps_tier1_working() {
    let home = tempfile::tempdir().unwrap();
    let cfg = MemoryConfig {
        tier2_enabled: true,
        embeddings: EmbeddingsConfig {
            model: Some("text-embedding-3-large".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let mgr = build_memory_manager_at(&cfg, home.path())
        .await
        .unwrap()
        .expect("Tier 1 must survive a bad Tier 2 config");
    assert!(mgr.lock().await.tier2().is_none());

    let mut reg = ToolRegistry::new();
    reg.register_memory_tool(mgr as SharedMemoryManager);
    let out = reg
        .dispatch(
            "memory",
            serde_json::json!({"action": "add", "target": "memory", "content": "still works"}),
        )
        .await
        .unwrap();
    assert!(!out.contains("\"success\":false"), "{out}");
    let md = std::fs::read_to_string(home.path().join("memories/MEMORY.md")).unwrap();
    assert!(md.contains("still works"));
}
