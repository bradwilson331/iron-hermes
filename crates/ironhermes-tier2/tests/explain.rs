//! D-12 `memory_explain` / `list_recent` behaviour at the store + service layer: evidence,
//! supersession back-links, Tier 1 evidence, and the active-only visibility rule (T-58.2-14).
#![cfg(feature = "test-support")]

use std::path::Path;

use ironhermes_core::MemoryEntries;
use ironhermes_core::config::MemoryConfig;
use ironhermes_core::memory_store::MemoryTarget;
use ironhermes_core::tier2::Tier2Handle;
use ironhermes_tier2::Tier2Service;
use ironhermes_tier2::embed::Embedder;
use ironhermes_tier2::store::{ClaimRow, NewClaim};
use ironhermes_tier2::test_support::FakeEmbedder;

fn open(home: &Path) -> Tier2Service {
    Tier2Service::open_with_embedder(
        home,
        &MemoryConfig::default(),
        Embedder::Fake(FakeEmbedder::new("fake:a")),
    )
    .unwrap()
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

fn exec(svc: &Tier2Service, sql: &str, p: &[&dyn rusqlite::ToSql]) {
    rusqlite::Connection::open(&svc.paths().db)
        .unwrap()
        .execute(sql, p)
        .unwrap();
}

#[tokio::test]
async fn explain_returns_evidence_and_supersedes() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let a = add(&svc, "User lives in Berlin");
    let b = add(&svc, "User lives in Lisbon");
    exec(
        &svc,
        "UPDATE claims SET status = 'superseded', superseded_by = ?2 WHERE id = ?1",
        &[&a.id, &b.id],
    );
    svc.store()
        .insert_evidence(
            b.id,
            "session://s1#m4",
            "I moved to Lisbon",
            Some("s1"),
            Some(4),
        )
        .unwrap();
    svc.store()
        .insert_evidence(
            b.id,
            "session://s2#m9",
            "Lisbon is home now",
            Some("s2"),
            Some(9),
        )
        .unwrap();

    let e = svc
        .explain(&b.claim_id)
        .await
        .unwrap()
        .expect("active claim");
    assert_eq!(e.claim_id, b.claim_id);
    assert_eq!(e.text, "User lives in Lisbon");
    assert_eq!(e.confidence, 0.9);
    assert_eq!(e.supersedes, vec![a.claim_id.clone()]);
    assert_eq!(e.evidence.len(), 2);
    assert_eq!(e.evidence[0].source, "session://s1#m4");
    assert_eq!(e.evidence[0].quote, "I moved to Lisbon");
    assert!(!e.evidence[0].timestamp.is_empty());
    assert_eq!(e.evidence[1].source, "session://s2#m9");

    // The superseded-away claim is no longer explainable.
    assert!(svc.explain(&a.claim_id).await.unwrap().is_none());
}

#[tokio::test]
async fn explain_tier1_claim_has_tier1_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let mut entries = MemoryEntries::default();
    entries.entries.insert(
        MemoryTarget::Memory,
        vec!["User prefers concise Telegram replies".to_string()],
    );
    let report = svc.sync_tier1(&entries).await.unwrap();
    assert_eq!(report.added, 1);

    let claim = svc.store().tier1_claims().unwrap().remove(0);
    let e = svc.explain(&claim.claim_id).await.unwrap().expect("active");
    assert_eq!(e.evidence.len(), 1);
    assert!(e.evidence[0].source.starts_with("memory://tier1/"));
    assert_eq!(e.evidence[0].quote, "User prefers concise Telegram replies");
    assert!(e.supersedes.is_empty());
}

#[tokio::test]
async fn explain_unknown_or_staged_is_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add(&svc, "secret staged for forgetting");
    assert!(svc.explain("claim:000000000000").await.unwrap().is_none());
    exec(
        &svc,
        "UPDATE claims SET status = 'staged_forget' WHERE id = ?1",
        &[&c.id],
    );
    assert!(svc.explain(&c.claim_id).await.unwrap().is_none());
}

#[tokio::test]
async fn list_recent_newest_first_active_only() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c1 = add(&svc, "first");
    let c2 = add(&svc, "second");
    let gone = add(&svc, "superseded one");
    let c3 = add(&svc, "third");
    exec(
        &svc,
        "UPDATE claims SET status = 'superseded', superseded_by = ?2 WHERE id = ?1",
        &[&gone.id, &c3.id],
    );
    let got: Vec<String> = svc
        .list_recent(10)
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.claim_id)
        .collect();
    assert_eq!(got, vec![c3.claim_id, c2.claim_id, c1.claim_id]);
    assert_eq!(svc.list_recent(2).await.unwrap().len(), 2);
}
