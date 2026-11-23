//! D-04/D-06/D-07 search behaviour: semantic ranking, the labelled FTS5 degrade, background
//! re-embedding, and never mixing embedding spaces.
#![cfg(feature = "test-support")]

use std::path::Path;

use ironhermes_core::config::MemoryConfig;
use ironhermes_core::tier2::Tier2SearchMode;
use ironhermes_tier2::Tier2Service;
use ironhermes_tier2::embed::Embedder;
use ironhermes_tier2::search::{embed_pending, search};
use ironhermes_tier2::store::{ClaimRow, NewClaim};
use ironhermes_tier2::test_support::FakeEmbedder;

fn open(home: &Path, e: Embedder) -> Tier2Service {
    Tier2Service::open_with_embedder(home, &MemoryConfig::default(), e).unwrap()
}

fn fake(id: &str) -> Embedder {
    Embedder::Fake(FakeEmbedder::new(id))
}

fn add(svc: &Tier2Service, text: &str) -> ClaimRow {
    svc.store()
        .insert_claim(&NewClaim {
            text: text.into(),
            kind: "world".into(),
            subject: "user".into(),
            origin: "consolidation".into(),
            confidence: 0.9,
            source_url: Some("memory://test".into()),
            tier1_target: None,
            tier1_ordinal: None,
            session_id: None,
        })
        .unwrap()
}

fn set_status(svc: &Tier2Service, pk: i64, status: &str) {
    let conn = rusqlite::Connection::open(&svc.paths().db).unwrap();
    conn.execute(
        "UPDATE claims SET status = ?2 WHERE id = ?1",
        rusqlite::params![pk, status],
    )
    .unwrap();
}

#[tokio::test]
async fn semantic_search_finds_paraphrase() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path(), fake("fake:a"));
    add(&svc, "Gardening tomatoes every summer");
    let b = add(&svc, "User prefers concise Telegram replies");
    assert_eq!(embed_pending(&svc).await.unwrap(), 2);

    let r = search(&svc, "keep replies concise", 5).await.unwrap();
    assert_eq!(r.mode, Tier2SearchMode::Semantic);
    assert!(r.note.is_none(), "semantic mode carries no note");
    assert_eq!(r.results[0].claim_id, b.claim_id);
}

#[tokio::test]
async fn degrade_fts_no_embedder() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path(), Embedder::None("no model for this test".into()));
    add(&svc, "User prefers concise Telegram replies");

    let r = search(&svc, "telegram", 5).await.unwrap();
    assert_eq!(r.mode, Tier2SearchMode::Keyword);
    let note = r.note.expect("keyword fallback must say why");
    assert!(note.contains("no model for this test"), "note was {note:?}");
    assert_eq!(r.results.len(), 1);
}

#[tokio::test]
async fn degrade_fts_writes_stored_then_embedded() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let svc = open(tmp.path(), Embedder::None("down".into()));
        add(&svc, "User prefers concise Telegram replies");
        add(&svc, "Gardening tomatoes every summer");
        assert_eq!(svc.store().count_embeddings("fake:x").unwrap(), 0);
        assert_eq!(embed_pending(&svc).await.unwrap(), 0);
    }
    let svc = open(tmp.path(), fake("fake:x"));
    assert_eq!(embed_pending(&svc).await.unwrap(), 2);
    assert_eq!(svc.store().count_embeddings("fake:x").unwrap(), 2);

    let r = search(&svc, "concise replies", 5).await.unwrap();
    assert_eq!(r.mode, Tier2SearchMode::Semantic);
    assert!(r.results[0].text.contains("Telegram"));
}

#[tokio::test]
async fn degrade_fts_endpoint_down() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = MemoryConfig::default();
    // D-04: `open` builds the embedder from config; a refused loopback port is a down endpoint.
    cfg.embeddings.base_url = Some("http://127.0.0.1:9".into());
    let svc = Tier2Service::open(tmp.path(), &cfg).unwrap();
    add(&svc, "User prefers concise Telegram replies");

    let r = search(&svc, "telegram", 5).await.unwrap();
    assert_eq!(r.mode, Tier2SearchMode::Keyword);
    let note = r.note.expect("keyword fallback must say why");
    assert!(
        note.contains("embedding request failed"),
        "note was {note:?}"
    );
    assert_eq!(r.results.len(), 1);
}

#[tokio::test]
async fn model_switch_marks_rows_for_reembed() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let svc = open(tmp.path(), fake("fake:a"));
        add(&svc, "User prefers concise Telegram replies");
        add(&svc, "Gardening tomatoes every summer");
        assert_eq!(embed_pending(&svc).await.unwrap(), 2);
        assert_eq!(
            search(&svc, "telegram", 5).await.unwrap().mode,
            Tier2SearchMode::Semantic
        );
    }
    let svc = open(tmp.path(), fake("fake:b"));
    // Before re-embedding finishes, nothing is scored against a fake:a vector.
    let before = search(&svc, "telegram", 5).await.unwrap();
    assert_eq!(before.mode, Tier2SearchMode::Keyword);
    assert!(before.note.is_some());
    assert_eq!(before.results.len(), 1);

    // The stale-row search above already kicked a background run; this one waits for it
    // (the run lock) and finds nothing left, so only the end state is asserted.
    embed_pending(&svc).await.unwrap();
    assert_eq!(svc.store().count_embeddings("fake:a").unwrap(), 0);
    assert_eq!(svc.store().count_embeddings("fake:b").unwrap(), 2);

    let after = search(&svc, "concise replies", 5).await.unwrap();
    assert_eq!(after.mode, Tier2SearchMode::Semantic);
    assert!(after.results[0].text.contains("Telegram"));
}

#[tokio::test]
async fn semantic_search_hides_non_active() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path(), fake("fake:a"));
    let old = add(&svc, "User prefers verbose Telegram replies");
    let staged = add(&svc, "User prefers Telegram replies on weekends");
    let live = add(&svc, "User prefers concise Telegram replies");
    assert_eq!(embed_pending(&svc).await.unwrap(), 3);
    set_status(&svc, old.id, "superseded");
    set_status(&svc, staged.id, "staged_forget");

    let r = search(&svc, "user prefers telegram replies", 10)
        .await
        .unwrap();
    assert_eq!(r.mode, Tier2SearchMode::Semantic);
    let ids: Vec<_> = r.results.iter().map(|h| h.claim_id.as_str()).collect();
    assert_eq!(ids, vec![live.claim_id.as_str()]);
}

// CR-01: a superseded claim keeps its old-model vector forever (only active claims are
// re-embedded), so it must not hold the backlog above zero and pin search to keyword mode.
#[tokio::test]
async fn model_switch_ignores_non_active_stale_vectors() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let svc = open(tmp.path(), fake("fake:a"));
        let old = add(&svc, "User prefers verbose Telegram replies");
        add(&svc, "User prefers concise Telegram replies");
        assert_eq!(embed_pending(&svc).await.unwrap(), 2);
        set_status(&svc, old.id, "superseded");
    }
    let svc = open(tmp.path(), fake("fake:b"));
    let _ = search(&svc, "telegram", 5).await.unwrap();
    embed_pending(&svc).await.unwrap();
    assert_eq!(svc.store().embedding_backlog("fake:b").unwrap(), (0, 0));
    let after = search(&svc, "concise replies", 5).await.unwrap();
    assert_eq!(after.mode, Tier2SearchMode::Semantic);
}

// CR-02: with the purge deferred (D-27) no tombstone is written, so a staged claim's hash
// must itself block re-learning (consolidation and the Tier 1 sync both check this).
#[tokio::test]
async fn staged_forget_hash_blocks_relearning() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path(), fake("fake:a"));
    let c = add(&svc, "User lives in Lisbon");
    assert!(!svc.store().is_tombstoned(&c.text_hash).unwrap());
    set_status(&svc, c.id, "staged_forget");
    assert!(svc.store().is_tombstoned(&c.text_hash).unwrap());
    set_status(&svc, c.id, "active");
    assert!(!svc.store().is_tombstoned(&c.text_hash).unwrap());
}
