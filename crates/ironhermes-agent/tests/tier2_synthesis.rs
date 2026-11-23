//! Phase 58.2 Plan 07 (D-19): `ALIGNMENT_SYNTHESIS.md` reaches new sessions through
//! `PromptBuilder::load_memory` as its own block inside the Memory slot, frozen per session id,
//! and dream prose never reaches a prompt. Every home is an explicit tempdir; embeddings point at
//! a refused loopback port so no test starts a model download.
//!
//! Plan 19 (D-29): with Tier 2 attached the Memory slot also ends with the constant
//! `TIER2_MEMORY_HINT` block, after the synthesis block; with Tier 2 off it is absent.

use std::path::Path;
use std::sync::Arc;

use ironhermes_agent::MemoryManager;
use ironhermes_agent::memory::factory::build_memory_manager_at;
use ironhermes_agent::prompt_builder::{PromptBuilder, TIER2_MEMORY_HINT};
use ironhermes_core::config::{EmbeddingsConfig, MemoryConfig};
use ironhermes_tools::ToolRegistry;
use ironhermes_tools::memory_tool::SharedMemoryManager;
use serde_json::json;
use tokio::sync::Mutex;

const HEADER: &str = "## Standing guidance from long-term memory";
const DREAM_CANARY: &str = "DREAMPROSE-9921";
const TIER1_ENTRY: &str = "Prefers concise Telegram replies";

fn synth(word: &str) -> String {
    format!("# Alignment synthesis\n\n## Preferences\n- {word} [claim:0123456789ab]\n")
}

fn cfg(tier2: bool, dreams: bool) -> MemoryConfig {
    MemoryConfig {
        tier2_enabled: tier2,
        dreams_enabled: dreams,
        provider: "file".into(),
        embeddings: EmbeddingsConfig {
            base_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn manager(home: &Path, tier2: bool, dreams: bool) -> Arc<Mutex<MemoryManager>> {
    build_memory_manager_at(&cfg(tier2, dreams), home)
        .await
        .unwrap()
        .expect("memory is enabled")
}

/// One Tier 1 entry written through the real tool path, so the Memory slot has content.
async fn seed_tier1(home: &Path) {
    let mut reg = ToolRegistry::new();
    reg.register_memory_tool(manager(home, false, false).await as SharedMemoryManager);
    reg.dispatch(
        "memory",
        json!({"action": "add", "target": "user", "content": TIER1_ENTRY}),
    )
    .await
    .unwrap();
}

fn write_synthesis(home: &Path, body: &str) {
    std::fs::write(home.join("tier2/dreams/ALIGNMENT_SYNTHESIS.md"), body).unwrap();
}

/// The (durable, ephemeral) prompt halves a new session on `session` would be built with.
async fn prompt(mgr: &Arc<Mutex<MemoryManager>>, session: &str) -> (String, String) {
    let mut b = PromptBuilder::new("m", "cli").with_session_id(session);
    b.set_memory_manager(mgr.clone());
    b.load_memory().await;
    b.build_split()
}

#[tokio::test]
async fn load_memory_includes_synthesis_block() {
    let home = tempfile::tempdir().unwrap();
    seed_tier1(home.path()).await;
    let off = manager(home.path(), false, false).await;
    let (durable_off, _) = prompt(&off, "s1").await;
    assert!(durable_off.contains(TIER1_ENTRY));
    assert!(!durable_off.contains(HEADER));

    let on = manager(home.path(), true, true).await;
    write_synthesis(home.path(), &synth("ALPHA-GUIDANCE"));
    let (durable, ephemeral) = prompt(&on, "s1").await;
    let at = durable
        .find(HEADER)
        .expect("synthesis block in the Memory slot");
    assert!(durable.contains("- ALPHA-GUIDANCE"));
    assert!(!durable.contains("[claim:"));
    assert!(!ephemeral.contains(HEADER), "never in the per-turn half");
    // The Tier 1 blocks come first and are byte-identical to the Tier-2-off prompt.
    assert!(durable.find(TIER1_ENTRY).unwrap() < at);
    assert!(durable_off.starts_with(durable[..at].trim_end()));
}

#[tokio::test]
async fn load_memory_without_dreams_has_no_synthesis_block() {
    let home = tempfile::tempdir().unwrap();
    seed_tier1(home.path()).await;
    let mgr = manager(home.path(), true, false).await;
    write_synthesis(home.path(), &synth("ALPHA-GUIDANCE"));
    let (durable, ephemeral) = prompt(&mgr, "s1").await;
    assert!(durable.contains(TIER1_ENTRY));
    assert!(!format!("{durable}{ephemeral}").contains("ALPHA-GUIDANCE"));
}

#[tokio::test]
async fn load_memory_synthesis_frozen_across_builders() {
    // The gateway builds a new PromptBuilder per inbound message; the freeze is per session id.
    let home = tempfile::tempdir().unwrap();
    let mgr = manager(home.path(), true, true).await;
    write_synthesis(home.path(), &synth("ALPHA-GUIDANCE"));
    let (first, _) = prompt(&mgr, "s1").await;
    write_synthesis(home.path(), &synth("BRAVO-GUIDANCE"));
    let (again, _) = prompt(&mgr, "s1").await;
    assert_eq!(first, again, "a running session's prompt must not change");
    let (fresh, _) = prompt(&mgr, "s2").await;
    assert!(fresh.contains("BRAVO-GUIDANCE") && !fresh.contains("ALPHA-GUIDANCE"));
}

#[tokio::test]
async fn synthesis_no_dream_prose_in_prompt() {
    let home = tempfile::tempdir().unwrap();
    let mgr = manager(home.path(), true, true).await;
    write_synthesis(home.path(), &synth("ALPHA-GUIDANCE"));
    std::fs::write(
        home.path().join("tier2/dreams/2026-11-08.md"),
        format!("---\nprompt_hoisted: false\n---\n# Dream review\n- {DREAM_CANARY} [claim:0123456789ab]\n"),
    )
    .unwrap();
    let (durable, ephemeral) = prompt(&mgr, "s1").await;
    assert!(durable.contains("ALPHA-GUIDANCE"), "synthesis is injected");
    assert!(!durable.contains(DREAM_CANARY) && !ephemeral.contains(DREAM_CANARY));
}

#[tokio::test]
async fn load_memory_tier2_hint_only_when_attached() {
    let home = tempfile::tempdir().unwrap();
    seed_tier1(home.path()).await;
    let off = manager(home.path(), false, false).await;
    let (d_off, e_off) = prompt(&off, "s1").await;
    assert!(!format!("{d_off}{e_off}").contains("## Long-term memory"));

    let on = manager(home.path(), true, false).await;
    let (durable, ephemeral) = prompt(&on, "s1").await;
    assert_eq!(durable.matches(TIER2_MEMORY_HINT).count(), 1);
    assert!(durable.find(TIER1_ENTRY).unwrap() < durable.find(TIER2_MEMORY_HINT).unwrap());
    assert!(!ephemeral.contains(TIER2_MEMORY_HINT));
}

#[tokio::test]
async fn load_memory_tier2_hint_without_tier1_entries() {
    let home = tempfile::tempdir().unwrap();
    let mgr = manager(home.path(), true, false).await;
    let (durable, _) = prompt(&mgr, "s1").await;
    assert!(durable.contains(TIER2_MEMORY_HINT));
}

#[tokio::test]
async fn load_memory_tier2_hint_after_synthesis() {
    let home = tempfile::tempdir().unwrap();
    seed_tier1(home.path()).await;
    let mgr = manager(home.path(), true, true).await;
    write_synthesis(home.path(), &synth("ALPHA-GUIDANCE"));
    let (durable, _) = prompt(&mgr, "s1").await;
    let hint = durable.find(TIER2_MEMORY_HINT).expect("hint present");
    assert!(hint > durable.find(HEADER).expect("synthesis present"));
}

#[tokio::test]
async fn tier2_hint_removed_entry_found_by_memory_search() {
    let home = tempfile::tempdir().unwrap();
    let mgr = manager(home.path(), true, false).await;
    let mut reg = ToolRegistry::new();
    reg.register_memory_tool(mgr.clone() as SharedMemoryManager);
    reg.dispatch(
        "memory",
        json!({"action": "add", "target": "memory", "content": "The mascot is a heron"}),
    )
    .await
    .unwrap();
    reg.dispatch(
        "memory",
        json!({"action": "remove", "target": "memory", "old_text": "heron"}),
    )
    .await
    .unwrap();

    let (durable, _) = prompt(&mgr, "s-new").await;
    assert!(durable.contains(TIER2_MEMORY_HINT));
    let tier1 = &durable[..durable.find(TIER2_MEMORY_HINT).unwrap()];
    assert!(!tier1.contains("heron"), "removed from the Tier 1 block");

    let found: serde_json::Value = serde_json::from_str(
        &reg.dispatch("memory_search", json!({"query": "heron"}))
            .await
            .unwrap(),
    )
    .unwrap();
    let hits = found["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{found}");
    assert!(
        hits[0]["source"]
            .as_str()
            .unwrap()
            .starts_with("memory://live/"),
        "{found}"
    );
}

#[test]
fn synthesis_load_memory_roots_source_lock() {
    // Every surface builds its prompt through `PromptBuilder::load_memory`; the hook lives only
    // there, so these roots all inherit it. If one disappears or a new call site is added without
    // going through load_memory, this test must be re-read, not silently passed.
    let src = include_str!("../src/prompt_builder.rs");
    assert_eq!(
        src.matches("synthesis_snapshot(").count(),
        1,
        "exactly one hook"
    );
    let start = src.find("pub async fn load_memory").unwrap();
    let end = start + src[start..].find("pub fn load_skills").unwrap();
    assert!(
        src[start..end].contains("synthesis_snapshot("),
        "hook sits inside load_memory"
    );

    let roots: [(&str, &str, usize); 6] = [
        (
            "cron-runner runner.rs",
            include_str!("../../ironhermes-cron-runner/src/runner.rs"),
            1,
        ),
        (
            "gateway handler.rs",
            include_str!("../../ironhermes-gateway/src/handler.rs"),
            2,
        ),
        (
            "ui state.rs",
            include_str!("../../iron_hermes_ui/src/server/state.rs"),
            1,
        ),
        (
            "ui api.rs",
            include_str!("../../iron_hermes_ui/src/server/api.rs"),
            1,
        ),
        (
            "cli event_loop.rs",
            include_str!("../../ironhermes-cli/src/tui_rata/event_loop.rs"),
            1,
        ),
        (
            "cli main.rs",
            include_str!("../../ironhermes-cli/src/main.rs"),
            2,
        ),
    ];
    let mut total = 0;
    for (name, text, baseline) in roots {
        let n = text.matches(".load_memory()").count();
        assert!(
            n >= baseline,
            "{name}: {n} load_memory call sites, baseline {baseline}"
        );
        total += n;
    }
    assert!(total >= 8, "8 load_memory roots, found {total}");
}
