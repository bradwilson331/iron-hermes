//! D-29: a Tier 1 line that leaves Tier 1 moves into the daily live log (same claim id, embedding
//! and confidence), is re-linked rather than duplicated when re-added, and is never copied while
//! staged for forgetting (D-21 / D-23).
#![cfg(feature = "test-support")]

use std::path::Path;

use ironhermes_core::MemoryEntries;
use ironhermes_core::config::MemoryConfig;
use ironhermes_core::memory_store::MemoryTarget;
use ironhermes_core::tier2::{Tier2Handle, Tier2SearchMode};
use ironhermes_tier2::Tier2Service;
use ironhermes_tier2::embed::Embedder;
use ironhermes_tier2::search::embed_pending;
use ironhermes_tier2::store::ClaimRow;
use ironhermes_tier2::test_support::FakeEmbedder;

const X: &str = "User prefers concise Telegram replies";

fn open(home: &Path) -> Tier2Service {
    let cfg = MemoryConfig {
        forgetting_enabled: true,
        ..Default::default()
    };
    Tier2Service::open_with_embedder(home, &cfg, Embedder::Fake(FakeEmbedder::new("fake:a")))
        .unwrap()
}

fn entries(memory: &[&str], user: &[&str]) -> MemoryEntries {
    let mut e = MemoryEntries::default();
    e.entries.insert(
        MemoryTarget::Memory,
        memory.iter().map(|s| s.to_string()).collect(),
    );
    e.entries.insert(
        MemoryTarget::User,
        user.iter().map(|s| s.to_string()).collect(),
    );
    e
}

/// Lines containing `[<id>]` across the `*.md` files of the live log.
fn live_lines_with(svc: &Tier2Service, id: &str) -> usize {
    let marker = format!("[{id}]");
    let Ok(dir) = std::fs::read_dir(&svc.paths().live) else {
        return 0;
    };
    dir.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .map(|s| s.lines().filter(|l| l.contains(&marker)).count())
        .sum()
}

fn tier1_rows(svc: &Tier2Service) -> Vec<ClaimRow> {
    svc.store().tier1_claims().unwrap()
}

#[tokio::test]
async fn move_readd_same_target_relinks() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    assert_eq!(svc.sync_tier1(&entries(&[X], &[])).await.unwrap().added, 1);
    let id = tier1_rows(&svc)[0].claim_id.clone();
    assert_eq!(svc.sync_tier1(&entries(&[], &[])).await.unwrap().moved, 1);
    let r = svc.sync_tier1(&entries(&[X], &[])).await.unwrap();
    assert_eq!(r.added, 0);

    let rows = tier1_rows(&svc);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].claim_id, id);
    assert_eq!(
        rows[0].source_url.as_deref(),
        Some("memory://tier1/MEMORY.md#L1")
    );
    assert_eq!(rows[0].tier1_ordinal, Some(1));

    let e = svc.explain(&id).await.unwrap().expect("active");
    assert!(
        e.evidence
            .iter()
            .any(|e| e.source.starts_with("memory://tier1/"))
    );
    assert!(
        e.evidence
            .iter()
            .any(|e| e.source.starts_with("memory://live/"))
    );
}

#[tokio::test]
async fn move_readd_other_target_relinks() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    svc.sync_tier1(&entries(&[X], &[])).await.unwrap();
    let id = tier1_rows(&svc)[0].claim_id.clone();
    assert_eq!(svc.sync_tier1(&entries(&[], &[])).await.unwrap().moved, 1);
    let r = svc.sync_tier1(&entries(&[], &[X])).await.unwrap();
    assert_eq!(r.added, 0);

    let rows = tier1_rows(&svc);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].claim_id, id);
    assert_eq!(rows[0].tier1_target.as_deref(), Some("user"));
    assert_eq!(
        rows[0].source_url.as_deref(),
        Some("memory://tier1/USER.md#L1")
    );
}

#[tokio::test]
async fn move_counts_once() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    svc.sync_tier1(&entries(&[X], &[])).await.unwrap();
    let id = tier1_rows(&svc)[0].claim_id.clone();
    assert_eq!(svc.sync_tier1(&entries(&[], &[])).await.unwrap().moved, 1);
    let again = svc.sync_tier1(&entries(&[], &[])).await.unwrap();
    assert_eq!((again.moved, again.added), (0, 0));
    assert_eq!(live_lines_with(&svc, &id), 1);
}

#[tokio::test]
async fn move_skips_staged_until_cancelled() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    svc.sync_tier1(&entries(&[X], &[])).await.unwrap();
    let id = tier1_rows(&svc)[0].claim_id.clone();
    svc.stage_forget(&id, None).await.unwrap();

    let r = svc.sync_tier1(&entries(&[], &[])).await.unwrap();
    assert_eq!(r.moved, 0);
    let row = tier1_rows(&svc).remove(0);
    assert_eq!(row.status, "staged_forget");
    assert!(row.tier1_ordinal.is_some());
    assert_eq!(live_lines_with(&svc, &id), 0);
    assert!(svc.search(X, 10).await.unwrap().results.is_empty());

    svc.unstage_forget(&id).await.unwrap();
    let r = svc.sync_tier1(&entries(&[], &[])).await.unwrap();
    assert_eq!(r.moved, 1);
    let hits = svc.search(X, 10).await.unwrap().results;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].claim_id, id);
    assert!(hits[0].source.starts_with("memory://live/"));
}

#[tokio::test]
async fn move_staged_readd_stays_hidden() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    svc.sync_tier1(&entries(&[X], &[])).await.unwrap();
    let id = tier1_rows(&svc)[0].claim_id.clone();
    assert_eq!(svc.sync_tier1(&entries(&[], &[])).await.unwrap().moved, 1);
    svc.stage_forget(&id, None).await.unwrap();

    let r = svc.sync_tier1(&entries(&[X], &[])).await.unwrap();
    assert_eq!(r.added, 0);
    let rows = tier1_rows(&svc);
    assert_eq!(rows.len(), 1, "no duplicate claim");
    assert_eq!(rows[0].status, "staged_forget");
    assert!(svc.search(X, 10).await.unwrap().results.is_empty());
}

#[tokio::test]
async fn move_keeps_embedding_for_semantic_search() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    svc.sync_tier1(&entries(&[X], &[])).await.unwrap();
    let id = tier1_rows(&svc)[0].claim_id.clone();
    embed_pending(&svc).await.unwrap();
    assert_eq!(svc.store().count_embeddings("fake:a").unwrap(), 1);

    assert_eq!(svc.sync_tier1(&entries(&[], &[])).await.unwrap().moved, 1);
    assert_eq!(svc.store().count_embeddings("fake:a").unwrap(), 1);

    let r = svc.search(X, 10).await.unwrap();
    assert_eq!(r.mode, Tier2SearchMode::Semantic);
    assert_eq!(r.results.len(), 1);
    assert_eq!(r.results[0].claim_id, id);
    assert!(r.results[0].source.starts_with("memory://live/"));
}
