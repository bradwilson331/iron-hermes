//! D-21 / D-23: staging a forget hides the claim on every read surface at once without deleting
//! anything, cancelling restores the exact prior status, and pending.json and the claims table
//! agree after an interruption.
#![cfg(feature = "test-support")]

use std::path::Path;

use ironhermes_core::config::MemoryConfig;
use ironhermes_core::tier2::{Tier2Handle, Tier2SearchMode};
use ironhermes_tier2::Tier2Service;
use ironhermes_tier2::embed::Embedder;
use ironhermes_tier2::forget::{PendingFile, load_pending};
use ironhermes_tier2::search::embed_pending;
use ironhermes_tier2::store::{ClaimRow, NewClaim};
use ironhermes_tier2::test_support::FakeEmbedder;

fn open_with(home: &Path, enabled: bool, embedder: Embedder) -> Tier2Service {
    let cfg = MemoryConfig {
        forgetting_enabled: enabled,
        ..Default::default()
    };
    Tier2Service::open_with_embedder(home, &cfg, embedder).unwrap()
}

fn open(home: &Path) -> Tier2Service {
    open_with(home, true, Embedder::Fake(FakeEmbedder::new("fake:a")))
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

fn db(svc: &Tier2Service) -> rusqlite::Connection {
    rusqlite::Connection::open(&svc.paths().db).unwrap()
}

fn status(svc: &Tier2Service, claim_id: &str) -> String {
    db(svc)
        .query_row(
            "SELECT status FROM claims WHERE claim_id = ?1",
            [claim_id],
            |r| r.get(0),
        )
        .unwrap()
}

fn pending(svc: &Tier2Service) -> PendingFile {
    load_pending(svc.paths()).unwrap()
}

async fn found_by_search(svc: &Tier2Service, q: &str, claim_id: &str) -> bool {
    svc.search(q, 10)
        .await
        .unwrap()
        .results
        .iter()
        .any(|h| h.claim_id == claim_id)
}

#[tokio::test]
async fn forget_stage_hides_immediately() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add(&svc, "User prefers concise Telegram replies");
    add(&svc, "Gardening tomatoes every summer");
    embed_pending(&svc).await.unwrap();
    // The derived-file and dreams inputs see the claim before it is staged.
    assert_eq!(svc.store().derived_claims().unwrap().len(), 2);

    let r = svc.search("concise replies", 10).await.unwrap();
    assert_eq!(r.mode, Tier2SearchMode::Semantic);
    assert!(r.results.iter().any(|h| h.claim_id == c.claim_id));

    let out = svc.stage_forget(&c.claim_id, None).await.unwrap();
    assert_eq!(out.status, "staged");
    assert!(out.note.contains("not deleted"), "{}", out.note);

    // Semantic path.
    let r = svc.search("concise replies", 10).await.unwrap();
    assert_eq!(r.mode, Tier2SearchMode::Semantic);
    assert!(!r.results.iter().any(|h| h.claim_id == c.claim_id));
    // FTS5 degrade path.
    let kw = svc.store().keyword_search("telegram", 10).unwrap();
    assert!(kw.iter().all(|row| row.claim_id != c.claim_id));
    // explain, list_recent, derived renders and the dreams window.
    assert!(svc.explain(&c.claim_id).await.unwrap().is_none());
    let recent = svc.list_recent(10).await.unwrap();
    assert!(recent.iter().all(|h| h.claim_id != c.claim_id));
    assert_eq!(svc.store().derived_claims().unwrap().len(), 1);
    assert!(
        svc.store()
            .consolidation_claims_since("1970-01-01T00:00:00Z")
            .unwrap()
            .iter()
            .all(|row| row.claim_id != c.claim_id)
    );
    assert!(
        svc.store()
            .top_durable_claims(10)
            .unwrap()
            .iter()
            .all(|row| row.claim_id != c.claim_id)
    );

    let p = pending(&svc);
    assert_eq!(p.version, 1);
    assert_eq!(p.staged.len(), 1);
    assert_eq!(p.staged[0].claim_id, c.claim_id);
    assert_eq!(p.staged[0].prior_status, "active");
    assert!(
        svc.paths().forget.join("pending.json").is_file(),
        "pending.json lives under tier2/forget"
    );
}

#[tokio::test]
async fn forget_stage_hides_without_deleting() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add(&svc, "User prefers concise Telegram replies");
    svc.store()
        .insert_evidence(
            c.id,
            "session://s1#m1",
            "keep it short",
            Some("s1"),
            Some(1),
        )
        .unwrap();
    embed_pending(&svc).await.unwrap();

    svc.stage_forget(&c.claim_id, Some("user asked"))
        .await
        .unwrap();

    assert_eq!(status(&svc, &c.claim_id), "staged_forget");
    let n = |table: &str| -> i64 {
        db(&svc)
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE claim_pk = ?1"),
                [c.id],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(n("evidence"), 1, "evidence survives staging");
    assert_eq!(n("embeddings"), 1, "embedding survives staging");
    let text: String = db(&svc)
        .query_row("SELECT text FROM claims WHERE id = ?1", [c.id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(text, "User prefers concise Telegram replies");
    // Derived bank/people/groups files are regenerated views; staging re-renders them (WR-01,
    // see forget_stage_clears_derived_files_and_synthesis), so they are not asserted here.
    assert_eq!(
        pending(&svc).staged[0].reason.as_deref(),
        Some("user asked")
    );
}

#[tokio::test]
async fn forget_unstage_restores_prior_status() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let a = add(&svc, "User prefers concise Telegram replies");
    let old = add(&svc, "User lives in Berlin");
    let new = add(&svc, "User lives in Lisbon");
    db(&svc)
        .execute(
            "UPDATE claims SET status = 'superseded', superseded_by = ?2 WHERE id = ?1",
            [old.id, new.id],
        )
        .unwrap();
    embed_pending(&svc).await.unwrap();

    svc.stage_forget(&a.claim_id, None).await.unwrap();
    svc.stage_forget(&old.claim_id, None).await.unwrap();
    assert_eq!(pending(&svc).staged.len(), 2);
    assert!(!found_by_search(&svc, "concise replies", &a.claim_id).await);

    let out = svc.unstage_forget(&a.claim_id).await.unwrap();
    assert_eq!(out.status, "cancelled");
    svc.unstage_forget(&old.claim_id).await.unwrap();

    assert_eq!(status(&svc, &a.claim_id), "active");
    assert_eq!(status(&svc, &old.claim_id), "superseded");
    assert!(found_by_search(&svc, "concise replies", &a.claim_id).await);
    assert!(pending(&svc).staged.is_empty());
    assert!(svc.list_staged().await.unwrap().is_empty());
}

#[tokio::test]
async fn forget_stage_requires_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open_with(
        tmp.path(),
        false,
        Embedder::Fake(FakeEmbedder::new("fake:a")),
    );
    let c = add(&svc, "User prefers concise Telegram replies");

    let err = svc.stage_forget(&c.claim_id, None).await.unwrap_err();
    assert!(
        err.to_string().contains("memory.forgetting_enabled"),
        "{err}"
    );
    assert_eq!(status(&svc, &c.claim_id), "active");
    assert!(!svc.paths().forget.join("pending.json").exists());

    // Cancel needs the flag too.
    let err = svc.unstage_forget(&c.claim_id).await.unwrap_err();
    assert!(
        err.to_string().contains("memory.forgetting_enabled"),
        "{err}"
    );
}

#[tokio::test]
async fn forget_stage_rejects_bad_or_unknown_id() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add(&svc, "User prefers concise Telegram replies");
    for bad in [
        "",
        "claim:XYZ",
        "abc",
        "claim:0123456789ab'; DROP TABLE claims",
    ] {
        assert!(svc.stage_forget(bad, None).await.is_err(), "{bad:?}");
    }
    let too_long = "x".repeat(201);
    assert!(
        svc.stage_forget(&c.claim_id, Some(&too_long))
            .await
            .is_err()
    );
    let err = svc
        .stage_forget("claim:000000000000", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not_found"), "{err}");
    let err = svc.unstage_forget("claim:000000000000").await.unwrap_err();
    assert!(err.to_string().contains("not_found"), "{err}");
    assert_eq!(status(&svc, &c.claim_id), "active");
    assert!(!svc.paths().forget.join("pending.json").exists());
}

#[tokio::test]
async fn forget_stage_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add(&svc, "User prefers concise Telegram replies");
    svc.stage_forget(&c.claim_id, None).await.unwrap();
    let again = svc.stage_forget(&c.claim_id, None).await.unwrap();
    assert_eq!(again.status, "staged");
    let p = pending(&svc);
    assert_eq!(p.staged.len(), 1);
    assert_eq!(
        p.staged[0].prior_status, "active",
        "second stage keeps the first prior"
    );
    assert_eq!(svc.list_staged().await.unwrap().len(), 1);
    // Cancelling something that is not staged is a harmless no-op, not a status change.
    svc.unstage_forget(&c.claim_id).await.unwrap();
    svc.unstage_forget(&c.claim_id).await.unwrap();
    assert_eq!(status(&svc, &c.claim_id), "active");
}

#[tokio::test]
async fn forget_stage_reconcile_after_interruption() {
    let tmp = tempfile::tempdir().unwrap();
    let (a_id, b_id);
    {
        let svc = open(tmp.path());
        let a = add(&svc, "Pending entry whose DB row is still active");
        let b = add(&svc, "DB row staged but missing from pending.json");
        (a_id, b_id) = (a.claim_id.clone(), b.claim_id.clone());
        // (a) interruption after pending.json was written, before the DB update.
        std::fs::write(
            svc.paths().forget.join("pending.json"),
            serde_json::json!({"version": 1, "staged": [{
                "claim_id": a_id, "staged_at": "2026-01-01T00:00:00Z",
                "prior_status": "active", "reason": null
            }]})
            .to_string(),
        )
        .unwrap();
        // (b) interruption after the DB update, before pending.json.
        db(&svc)
            .execute(
                "UPDATE claims SET status = 'staged_forget' WHERE id = ?1",
                [b.id],
            )
            .unwrap();
        assert_eq!(status(&svc, &a_id), "active");
    }

    let svc = open(tmp.path());
    assert_eq!(status(&svc, &a_id), "staged_forget", "pending entry wins");
    let p = pending(&svc);
    let mut ids: Vec<_> = p.staged.iter().map(|e| e.claim_id.clone()).collect();
    ids.sort();
    let mut want = vec![a_id.clone(), b_id.clone()];
    want.sort();
    assert_eq!(ids, want);
    let mut staged: Vec<_> = svc
        .list_staged()
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.claim_id)
        .collect();
    staged.sort();
    assert_eq!(staged, want);

    // An entry whose claim no longer exists is dropped.
    db(&svc)
        .execute("DELETE FROM claims WHERE claim_id = ?1", [&a_id])
        .unwrap();
    drop(svc);
    let svc = open(tmp.path());
    assert_eq!(pending(&svc).staged.len(), 1);
}

#[tokio::test]
async fn forget_stage_audited() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add(&svc, "User prefers CANARY-TEXT telegram replies");
    svc.stage_forget(&c.claim_id, Some("CANARY-REASON"))
        .await
        .unwrap();
    svc.unstage_forget(&c.claim_id).await.unwrap();

    let log = std::fs::read_to_string(tmp.path().join("logs/audit.jsonl")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert!(
        lines[0].contains("\"kind\":\"memory_forget_staged\""),
        "{log}"
    );
    assert!(
        lines[1].contains("\"kind\":\"memory_forget_unstaged\""),
        "{log}"
    );
    for l in &lines {
        assert!(l.contains(&c.claim_id), "{l}");
        assert!(
            !l.contains("CANARY"),
            "audit rows never carry claim text or reason: {l}"
        );
    }
}

fn bank_text(svc: &Tier2Service) -> String {
    std::fs::read_dir(&svc.paths().bank)
        .map(|it| {
            it.filter_map(Result::ok)
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .collect()
        })
        .unwrap_or_default()
}

// WR-01: staging must reach the derived files and the session synthesis block at once, not at
// the next consolidation or dreams run that happens to change something; cancelling restores.
#[tokio::test]
async fn forget_stage_clears_derived_files_and_synthesis() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = MemoryConfig {
        forgetting_enabled: true,
        dreams_enabled: true,
        ..Default::default()
    };
    let svc = Tier2Service::open_with_embedder(
        tmp.path(),
        &cfg,
        Embedder::Fake(FakeEmbedder::new("fake:a")),
    )
    .unwrap();
    let gone = add(&svc, "User lives in Lisbon");
    let kept = add(&svc, "User prefers concise replies");
    ironhermes_tier2::render::render_all(svc.store(), svc.paths()).unwrap();
    assert!(bank_text(&svc).contains(&gone.claim_id));
    std::fs::write(
        tmp.path().join("tier2/dreams/ALIGNMENT_SYNTHESIS.md"),
        format!(
            "# Alignment synthesis\n\n## Preferences\n- LISBON-GUIDANCE [{}]\n- CONCISE-GUIDANCE [{}]\n",
            gone.claim_id, kept.claim_id
        ),
    )
    .unwrap();

    // The real dreams format: the claim id (`claim:<hex>`) inside brackets.
    assert!(gone.claim_id.starts_with("claim:"));
    let fresh = svc
        .synthesis_snapshot(Some("s-before"))
        .await
        .expect("block");
    assert!(fresh.contains("LISBON-GUIDANCE"), "{fresh}");

    svc.stage_forget(&gone.claim_id, None).await.unwrap();
    let bank = bank_text(&svc);
    assert!(!bank.contains(&gone.claim_id), "{bank}");
    assert!(bank.contains(&kept.claim_id), "{bank}");
    let block = svc
        .synthesis_snapshot(Some("s-staged"))
        .await
        .expect("block");
    assert!(!block.contains("LISBON-GUIDANCE"), "{block}");
    assert!(block.contains("CONCISE-GUIDANCE"), "{block}");

    svc.unstage_forget(&gone.claim_id).await.unwrap();
    assert!(bank_text(&svc).contains(&gone.claim_id));
    let block = svc
        .synthesis_snapshot(Some("s-restored"))
        .await
        .expect("block");
    assert!(block.contains("LISBON-GUIDANCE"), "{block}");
}

fn hold_pending_lock(svc: &Tier2Service) -> std::fs::File {
    std::fs::create_dir_all(&svc.paths().forget).unwrap();
    let f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(
            svc.paths()
                .forget
                .join(ironhermes_tier2::forget::PENDING_LOCK_FILE),
        )
        .unwrap();
    f.lock().unwrap();
    f
}

fn enabled_cfg() -> MemoryConfig {
    MemoryConfig {
        forgetting_enabled: true,
        ..Default::default()
    }
}

// G-58.2-3: another process holding the pending lock must make stage wait, not interleave.
#[test]
fn forget_stage_waits_for_cross_process_lock() {
    use std::time::Duration;
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add(&svc, "User prefers concise Telegram replies");
    let held = hold_pending_lock(&svc);

    let (tx, rx) = std::sync::mpsc::channel();
    let (worker, id) = (svc.clone(), c.claim_id.clone());
    let t = std::thread::spawn(move || {
        let r = ironhermes_tier2::forget::stage(
            worker.store(),
            worker.paths(),
            &enabled_cfg(),
            &id,
            None,
        );
        tx.send(r.map(|_| ()).map_err(|e| format!("{e:#}")))
            .unwrap();
    });
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    drop(held);
    rx.recv_timeout(Duration::from_secs(5))
        .expect("stage returns once the lock is free")
        .unwrap();
    t.join().unwrap();
    assert_eq!(pending(&svc).staged.len(), 1);
    assert_eq!(pending(&svc).staged[0].claim_id, c.claim_id);
}

// G-58.2-3: two writers (two services = two SQLite connections, as two processes would have)
// staging different claims at once must both land in pending.json.
#[test]
fn forget_concurrent_stages_all_survive() {
    let tmp = tempfile::tempdir().unwrap();
    let svc_a = open(tmp.path());
    let svc_b = open(tmp.path());
    let ids: Vec<String> = (0..20)
        .map(|i| add(&svc_a, &format!("Concurrent claim number {i}")).claim_id)
        .collect();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|s| {
        for (svc, chunk) in [(&svc_a, &ids[..10]), (&svc_b, &ids[10..])] {
            let barrier = &barrier;
            s.spawn(move || {
                barrier.wait();
                for id in chunk {
                    ironhermes_tier2::forget::stage(
                        svc.store(),
                        svc.paths(),
                        &enabled_cfg(),
                        id,
                        None,
                    )
                    .unwrap();
                }
            });
        }
    });
    let mut got: Vec<String> = pending(&svc_a)
        .staged
        .into_iter()
        .map(|e| e.claim_id)
        .collect();
    let mut want = ids.clone();
    got.sort();
    want.sort();
    assert_eq!(got, want);
    for id in &ids {
        assert_eq!(status(&svc_a, id), "staged_forget");
    }
}

// T-58.2-56: boot-time reconcile runs on an async thread, so a busy lock means skip, never wait.
#[test]
fn forget_reconcile_skips_when_locked() {
    use std::time::Duration;
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add(&svc, "User prefers concise Telegram replies");
    ironhermes_tier2::forget::stage(svc.store(), svc.paths(), &enabled_cfg(), &c.claim_id, None)
        .unwrap();
    db(&svc)
        .execute(
            "UPDATE claims SET status = 'active' WHERE claim_id = ?1",
            [&c.claim_id],
        )
        .unwrap();
    let held = hold_pending_lock(&svc);

    let (tx, rx) = std::sync::mpsc::channel();
    let worker = svc.clone();
    let t = std::thread::spawn(move || {
        let r = ironhermes_tier2::forget::reconcile(worker.store(), worker.paths());
        tx.send(r.map_err(|e| format!("{e:#}"))).unwrap();
    });
    let r = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("reconcile must not wait for the lock");
    assert_eq!(r, Ok(()));
    t.join().unwrap();
    assert_eq!(status(&svc, &c.claim_id), "active", "skipped, not repaired");

    drop(held);
    ironhermes_tier2::forget::reconcile(svc.store(), svc.paths()).unwrap();
    assert_eq!(status(&svc, &c.claim_id), "staged_forget");
}
