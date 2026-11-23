//! D-30: consolidation promotes durable, direct user facts into Tier 1 through the Tier 1 add path.
//! Tier 2 hands a candidate out once (marking it), the following sync links the new Tier 1 line to
//! that same claim, and the finish step releases only a `Retry` and audits every outcome.
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

const LISBON: &str = "User lives in Lisbon";

fn open(home: &Path, promotion: bool, consolidation: bool) -> Tier2Service {
    let cfg = MemoryConfig {
        promotion_enabled: promotion,
        consolidation_enabled: consolidation,
        forgetting_enabled: true,
        ..Default::default()
    };
    Tier2Service::open_with_embedder(home, &cfg, Embedder::Fake(FakeEmbedder::new("fake:a")))
        .unwrap()
}

fn fact(svc: &Tier2Service, text: &str, kind: &str, subject: &str, confidence: f64) -> ClaimRow {
    svc.store()
        .insert_claim(&NewClaim {
            text: text.into(),
            kind: kind.into(),
            subject: subject.into(),
            origin: "consolidation".into(),
            confidence,
            source_url: None,
            tier1_target: None,
            tier1_ordinal: None,
            session_id: None,
        })
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

#[tokio::test]
async fn promote_link_no_duplicate() {
    let home = tempfile::tempdir().unwrap();
    let svc = open(home.path(), true, true);
    let c = fact(&svc, LISBON, "world", "user", 0.95);
    let got = svc.claim_promotions(5).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].claim_id, c.claim_id);
    assert_eq!(got[0].text, LISBON);

    let r = svc.sync_tier1(&entries(&[], &[LISBON])).await.unwrap();
    assert_eq!(r.added, 0, "the promoted line links, it is not re-inserted");

    let rows = svc.store().tier1_claims().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].claim_id, c.claim_id);
    assert_eq!(rows[0].tier1_target.as_deref(), Some("user"));
    assert_eq!(rows[0].tier1_ordinal, Some(1));
    assert_eq!(
        rows[0].source_url.as_deref(),
        Some("memory://tier1/USER.md#L1")
    );
    assert_eq!(rows[0].origin, "consolidation");

    let hits = svc.search("Lisbon", 10).await.unwrap();
    assert_eq!(hits.results.len(), 1, "one claim, one hit");
}

#[tokio::test]
async fn promote_link_lands_in_memory_md() {
    let home = tempfile::tempdir().unwrap();
    let svc = open(home.path(), true, true);
    let c = fact(&svc, LISBON, "world", "user", 0.95);
    assert_eq!(svc.claim_promotions(5).await.unwrap().len(), 1);

    let r = svc.sync_tier1(&entries(&[LISBON], &[])).await.unwrap();
    assert_eq!(r.added, 0);
    let rows = svc.store().tier1_claims().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].claim_id, c.claim_id);
    assert_eq!(rows[0].tier1_target.as_deref(), Some("memory"));
    assert_eq!(rows[0].tier1_ordinal, Some(1));
}

#[tokio::test]
async fn promote_needs_both_flags() {
    for (promotion, consolidation) in [(false, true), (true, false)] {
        let home = tempfile::tempdir().unwrap();
        let svc = open(home.path(), promotion, consolidation);
        let c = fact(&svc, LISBON, "world", "user", 0.95);
        assert!(svc.claim_promotions(5).await.unwrap().is_empty());
        let row = rusqlite::Connection::open(&svc.paths().db)
            .unwrap()
            .query_row(
                "SELECT tier1_target FROM claims WHERE claim_id = ?1",
                [&c.claim_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap();
        assert_eq!(row, None, "flags {promotion}/{consolidation} must not mark");
    }
}

fn ids(c: &[ironhermes_core::tier2::PromotionCandidate]) -> Vec<String> {
    c.iter().map(|p| p.claim_id.clone()).collect()
}

fn set_col(svc: &Tier2Service, claim_id: &str, sql_set: &str) {
    rusqlite::Connection::open(&svc.paths().db)
        .unwrap()
        .execute(
            &format!("UPDATE claims SET {sql_set} WHERE claim_id = ?1"),
            [claim_id],
        )
        .unwrap();
}

#[tokio::test]
async fn promote_candidate_rule() {
    let home = tempfile::tempdir().unwrap();
    let svc = open(home.path(), true, true);
    // Already in Tier 1 under a tier1-origin claim: the consolidation twin must not promote.
    svc.sync_tier1(&entries(&[], &["User already has this line"]))
        .await
        .unwrap();

    let a = fact(&svc, "User lives in Lisbon", "world", "user", 0.95);
    let b = fact(&svc, "User dislikes mornings", "opinion", "user", 0.95);
    fact(&svc, "User might like jazz", "world", "user", 0.7);
    fact(&svc, "User went hiking today", "experience", "user", 0.95);
    fact(&svc, "User tends to overthink", "reflection", "user", 0.95);
    fact(&svc, "Dana lives in Porto", "world", "person:dana", 0.95);
    let nd = fact(&svc, "User ate lunch at noon", "world", "user", 0.95);
    set_col(&svc, &nd.claim_id, "durable = 0");
    let staged = fact(&svc, "User owns a red bike", "world", "user", 0.95);
    svc.stage_forget(&staged.claim_id, None).await.unwrap();
    fact(&svc, "User keeps a tombstoned fact", "world", "user", 0.95);
    svc.store()
        .insert_tombstone(&ironhermes_tier2::store::text_hash(
            "User keeps a tombstoned fact",
        ))
        .unwrap();
    fact(&svc, "User already has this line", "world", "user", 0.95);

    let got = svc.claim_promotions(10).await.unwrap();
    assert_eq!(ids(&got), vec![b.claim_id, a.claim_id]);

    let home2 = tempfile::tempdir().unwrap();
    let svc2 = open(home2.path(), true, true);
    fact(&svc2, "User lives in Lisbon", "world", "user", 0.95);
    fact(&svc2, "User dislikes mornings", "opinion", "user", 0.95);
    assert_eq!(svc2.claim_promotions(1).await.unwrap().len(), 1);
}

#[tokio::test]
async fn promote_claims_once() {
    let home = tempfile::tempdir().unwrap();
    let svc = open(home.path(), true, true);
    fact(&svc, LISBON, "world", "user", 0.95);
    assert_eq!(svc.claim_promotions(10).await.unwrap().len(), 1);
    assert!(svc.claim_promotions(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn promote_retry_releases_others_keep() {
    use ironhermes_core::tier2::PromotionOutcome::{Promoted, Rejected, Retry};
    let home = tempfile::tempdir().unwrap();
    let svc = open(home.path(), true, true);
    let a = fact(&svc, "User lives in Lisbon", "world", "user", 0.95);
    let b = fact(&svc, "User dislikes mornings", "world", "user", 0.95);
    let c = fact(&svc, "User drinks green tea", "world", "user", 0.95);
    assert_eq!(svc.claim_promotions(10).await.unwrap().len(), 3);

    svc.finish_promotion(&a.claim_id, "user", Retry)
        .await
        .unwrap();
    assert_eq!(
        ids(&svc.claim_promotions(10).await.unwrap()),
        vec![a.claim_id]
    );

    svc.finish_promotion(&b.claim_id, "user", Rejected)
        .await
        .unwrap();
    svc.finish_promotion(&c.claim_id, "user", Promoted)
        .await
        .unwrap();
    assert!(svc.claim_promotions(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn promote_removed_line_moves_not_repromoted() {
    let home = tempfile::tempdir().unwrap();
    let svc = open(home.path(), true, true);
    let c = fact(&svc, LISBON, "world", "user", 0.95);
    assert_eq!(svc.claim_promotions(5).await.unwrap().len(), 1);
    svc.sync_tier1(&entries(&[], &[LISBON])).await.unwrap();

    let r = svc.sync_tier1(&entries(&[], &[])).await.unwrap();
    assert_eq!(r.moved, 1);
    let rows = svc.store().tier1_claims().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].claim_id, c.claim_id);
    assert!(
        rows[0]
            .source_url
            .as_deref()
            .is_some_and(|u| u.starts_with("memory://live/")),
        "{:?}",
        rows[0].source_url
    );
    assert!(svc.claim_promotions(5).await.unwrap().is_empty());
}

#[tokio::test]
async fn promote_finish_audits() {
    use ironhermes_core::tier2::PromotionOutcome::{Promoted, Rejected, Retry};
    let home = tempfile::tempdir().unwrap();
    let svc = open(home.path(), true, true);
    let a = fact(
        &svc,
        "User CANARY-TEXT lives in Lisbon",
        "world",
        "user",
        0.95,
    );
    svc.finish_promotion(&a.claim_id, "user", Promoted)
        .await
        .unwrap();
    svc.finish_promotion(&a.claim_id, "memory", Retry)
        .await
        .unwrap();
    svc.finish_promotion(&a.claim_id, "user", Rejected)
        .await
        .unwrap();

    let log = std::fs::read_to_string(home.path().join("logs/audit.jsonl")).unwrap();
    let rows: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let kinds: Vec<&str> = rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        [
            "memory_promoted",
            "memory_promotion_deferred",
            "memory_promotion_rejected"
        ]
    );
    for (row, target) in rows.iter().zip(["user", "memory", "user"]) {
        let args: serde_json::Value = serde_json::from_str(row["args"].as_str().unwrap()).unwrap();
        assert_eq!(args["claim_id"], a.claim_id);
        assert_eq!(args["target"], target);
    }
    assert!(
        !log.contains("CANARY"),
        "audit rows never carry text: {log}"
    );
}
