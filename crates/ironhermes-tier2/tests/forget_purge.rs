//! D-20 / D-22 / D-28 / D-32: the human-run purge deletes previewed, staged claims (and the older
//! claims they superseded), tombstones them without content, removes their Tier 1 line first, and
//! audits each one. Artifact retraction (live lines, dreams, render, index) is plan 58.2-25.
#![cfg(feature = "test-support")]

use std::path::Path;
use std::sync::Mutex;

use ironhermes_core::config::MemoryConfig;
use ironhermes_core::memory_store::MemoryTarget;
use ironhermes_core::tier2::{PurgeSurface, Tier1Remover, Tier2Handle};
use ironhermes_tier2::Tier2Service;
use ironhermes_tier2::embed::Embedder;
use ironhermes_tier2::forget::load_pending;
use ironhermes_tier2::search::embed_pending;
use ironhermes_tier2::store::{ClaimRow, NewClaim};
use ironhermes_tier2::test_support::FakeEmbedder;

fn open_with(home: &Path, enabled: bool) -> Tier2Service {
    let cfg = MemoryConfig {
        forgetting_enabled: enabled,
        ..Default::default()
    };
    Tier2Service::open_with_embedder(home, &cfg, Embedder::Fake(FakeEmbedder::new("fake:a")))
        .unwrap()
}

fn open(home: &Path) -> Tier2Service {
    open_with(home, true)
}

fn add_linked(
    svc: &Tier2Service,
    text: &str,
    origin: &str,
    tier1: Option<(&str, Option<i64>)>,
) -> ClaimRow {
    svc.store()
        .insert_claim(&NewClaim {
            text: text.into(),
            kind: "world".into(),
            subject: "user".into(),
            origin: origin.into(),
            confidence: 0.9,
            source_url: Some("memory://test".into()),
            tier1_target: tier1.map(|(t, _)| t.to_string()),
            tier1_ordinal: tier1.and_then(|(_, o)| o),
            session_id: None,
        })
        .unwrap()
}

fn add(svc: &Tier2Service, text: &str) -> ClaimRow {
    add_linked(svc, text, "consolidation", None)
}

fn db(svc: &Tier2Service) -> rusqlite::Connection {
    rusqlite::Connection::open(&svc.paths().db).unwrap()
}

fn count(svc: &Tier2Service, sql: &str, p: impl rusqlite::Params) -> i64 {
    db(svc).query_row(sql, p, |r| r.get(0)).unwrap()
}

fn evidence(svc: &Tier2Service, pk: i64, mid: i64, quote: &str) {
    svc.store()
        .insert_evidence(
            pk,
            &format!("session://s1#m{mid}"),
            quote,
            Some("s1"),
            Some(mid),
        )
        .unwrap();
}

/// `old` was superseded by `new`.
fn supersede(svc: &Tier2Service, old: &ClaimRow, new: &ClaimRow) {
    db(svc)
        .execute(
            "UPDATE claims SET superseded_by = ?1, status = 'superseded' WHERE id = ?2",
            rusqlite::params![new.id, old.id],
        )
        .unwrap();
}

fn claim_exists(svc: &Tier2Service, claim_id: &str) -> bool {
    count(
        svc,
        "SELECT COUNT(*) FROM claims WHERE claim_id = ?1",
        [claim_id],
    ) == 1
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

/// Records each `remove_tier1` call. While the call runs it probes, through its own handles, that
/// the claim row still exists and that no store write transaction or pending.json lock is held
/// (T-58.2-82: Tier 1's own write must be able to re-enter Tier 2).
struct Recorder {
    home: std::path::PathBuf,
    calls: Mutex<Vec<(MemoryTarget, String)>>,
    row_existed: Mutex<Vec<bool>>,
    probes_ok: Mutex<Vec<bool>>,
    fail_on: Option<String>,
}

impl Recorder {
    fn new(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
            calls: Mutex::default(),
            row_existed: Mutex::default(),
            probes_ok: Mutex::default(),
            fail_on: None,
        }
    }

    fn failing_on(mut self, text: &str) -> Self {
        self.fail_on = Some(text.into());
        self
    }

    fn calls(&self) -> Vec<(MemoryTarget, String)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Tier1Remover for Recorder {
    async fn remove_tier1(&self, target: MemoryTarget, text: &str) -> anyhow::Result<bool> {
        let conn = rusqlite::Connection::open(self.home.join("tier2/tier2.db")).unwrap();
        conn.busy_timeout(std::time::Duration::from_millis(200))
            .unwrap();
        let existed: i64 = conn
            .query_row("SELECT COUNT(*) FROM claims WHERE text = ?1", [text], |r| {
                r.get(0)
            })
            .unwrap();
        self.row_existed.lock().unwrap().push(existed > 0);
        // A write through a second connection fails with SQLITE_BUSY if the purge holds a
        // write transaction; the lock file probe fails if the pending lock is held.
        let wrote = conn
            .execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('probe', '1')",
                [],
            )
            .is_ok();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.home.join("tier2/forget/pending.json.lock"))
            .unwrap();
        let unlocked = lock.try_lock().is_ok();
        self.probes_ok.lock().unwrap().push(wrote && unlocked);
        if self.fail_on.as_deref() == Some(text) {
            anyhow::bail!("simulated Tier 1 write failure");
        }
        self.calls.lock().unwrap().push((target, text.to_string()));
        Ok(true)
    }
}

async fn stage(svc: &Tier2Service, c: &ClaimRow) {
    svc.stage_forget(&c.claim_id, None).await.unwrap();
}

#[tokio::test]
async fn purge_requires_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let c = {
        let svc = open(tmp.path());
        let c = add(&svc, "User prefers concise Telegram replies");
        stage(&svc, &c).await;
        c
    };
    let svc = open_with(tmp.path(), false);
    let rec = Recorder::new(tmp.path());
    let report = svc
        .run_purge(&rec, PurgeSurface::Cli, std::slice::from_ref(&c.claim_id))
        .await
        .unwrap();
    assert!(report.skipped.is_some(), "{report:?}");
    assert!(report.purged.is_empty());
    assert!(rec.calls().is_empty());
    assert_eq!(status(&svc, &c.claim_id), "staged_forget");
    assert_eq!(load_pending(svc.paths()).unwrap().staged.len(), 1);
}

#[tokio::test]
async fn purge_removes_claim_and_tombstones() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add(&svc, "User lives in Lisbon");
    let old = add(&svc, "User lives in Porto");
    let other = add(&svc, "User enjoys Lisbon trams");
    supersede(&svc, &old, &c);
    evidence(&svc, c.id, 11, "I live in Lisbon");
    evidence(&svc, old.id, 12, "I live in Porto");
    // `other` quotes a message the purged claim also cites, and one of its own.
    evidence(&svc, other.id, 11, "Lisbon trams are great");
    evidence(&svc, other.id, 13, "I ride trams");
    embed_pending(&svc).await.unwrap();
    svc.store()
        .upsert_embedding(old.id, "fake:a", &vec![0.1f32; 384])
        .unwrap();
    assert_eq!(count(&svc, "SELECT COUNT(*) FROM embeddings", []), 3);
    stage(&svc, &c).await;

    let rec = Recorder::new(tmp.path());
    let report = svc
        .run_purge(&rec, PurgeSurface::Cli, std::slice::from_ref(&c.claim_id))
        .await
        .unwrap();
    assert_eq!(report.purged, vec![c.claim_id.clone()], "{report:?}");
    assert_eq!(report.evidence_redacted, 1);
    assert!(report.skipped.is_none() && report.not_staged.is_empty());

    assert!(!claim_exists(&svc, &c.claim_id));
    assert!(!claim_exists(&svc, &old.claim_id), "predecessor purged too");
    assert!(claim_exists(&svc, &other.claim_id));
    for pk in [c.id, old.id] {
        assert_eq!(
            count(
                &svc,
                "SELECT COUNT(*) FROM evidence WHERE claim_pk = ?1",
                [pk]
            ),
            0
        );
        assert_eq!(
            count(
                &svc,
                "SELECT COUNT(*) FROM embeddings WHERE claim_pk = ?1",
                [pk]
            ),
            0
        );
    }
    // Content-free tombstones: exactly the two text hashes and the two message ids.
    let hashes: Vec<String> = db(&svc)
        .prepare("SELECT text_hash FROM tombstones ORDER BY text_hash")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut want = vec![c.text_hash.clone(), old.text_hash.clone()];
    want.sort();
    assert_eq!(hashes, want);
    assert!(
        hashes
            .iter()
            .all(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
    );
    let mids: Vec<i64> = db(&svc)
        .prepare("SELECT message_id FROM tombstone_messages ORDER BY message_id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(mids, vec![11, 12]);
    // The other claim keeps its row; only the quote sharing message 11 is retracted.
    let quotes: Vec<(i64, String)> = db(&svc)
        .prepare("SELECT message_id, quote FROM evidence WHERE claim_pk = ?1 ORDER BY message_id")
        .unwrap()
        .query_map([other.id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        quotes,
        vec![
            (11, "[retracted]".to_string()),
            (13, "I ride trams".to_string())
        ]
    );
    assert!(load_pending(svc.paths()).unwrap().staged.is_empty());
    assert!(svc.explain(&c.claim_id).await.unwrap().is_none());
    let hits = svc.search("Lisbon", 10).await.unwrap().results;
    assert!(
        hits.iter()
            .all(|h| h.claim_id != c.claim_id && h.claim_id != old.claim_id)
    );
    assert_eq!(
        count(
            &svc,
            "SELECT COUNT(*) FROM claims_fts WHERE claims_fts MATCH 'porto'",
            []
        ),
        0
    );
}

#[tokio::test]
async fn purge_only_previewed_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let a = add(&svc, "Alpha fact about gardening");
    let b = add(&svc, "Beta fact about sailing");
    let live = add(&svc, "Gamma fact never staged");
    stage(&svc, &a).await;
    stage(&svc, &b).await;

    let rec = Recorder::new(tmp.path());
    let only = vec![a.claim_id.clone(), live.claim_id.clone()];
    let report = svc.run_purge(&rec, PurgeSurface::Ui, &only).await.unwrap();
    assert_eq!(report.purged, vec![a.claim_id.clone()]);
    assert_eq!(report.not_staged, vec![live.claim_id.clone()]);
    assert!(!claim_exists(&svc, &a.claim_id));
    assert_eq!(
        status(&svc, &b.claim_id),
        "staged_forget",
        "staged after the preview survives"
    );
    assert_eq!(status(&svc, &live.claim_id), "active");
    let pending = load_pending(svc.paths()).unwrap();
    assert_eq!(
        pending
            .staged
            .iter()
            .map(|e| e.claim_id.as_str())
            .collect::<Vec<_>>(),
        vec![b.claim_id.as_str()]
    );
}

#[tokio::test]
async fn purge_linked_tier1_calls_remover_first() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    // tier1-origin claim linked to MEMORY.md line 2.
    let linked = add_linked(
        &svc,
        "User keeps tomato plants",
        "tier1",
        Some(("memory", Some(2))),
    );
    // A D-30 promoted consolidation claim linked to USER.md line 1: any origin gets the call.
    let promoted = add_linked(
        &svc,
        "User plays chess",
        "consolidation",
        Some(("user", Some(1))),
    );
    // D-29: moved out of Tier 1 (ordinal NULL). Still called (WR-03: a NULL ordinal can also be
    // an unsynced promotion); the real remover's exact match returns false when the line is gone.
    let moved = add_linked(
        &svc,
        "User once wore a hat",
        "tier1",
        Some(("memory", None)),
    );
    // The Tier 1 write fails for this one: it stays staged and the loop continues.
    let stuck = add_linked(&svc, "User owns a boat", "tier1", Some(("memory", Some(3))));
    let plain = add(&svc, "User has a cat");
    let all = [&linked, &promoted, &moved, &stuck, &plain];
    for c in all {
        stage(&svc, c).await;
    }

    let rec = Recorder::new(tmp.path()).failing_on("User owns a boat");
    let only: Vec<String> = all.iter().map(|c| c.claim_id.clone()).collect();
    let report = svc.run_purge(&rec, PurgeSurface::Cli, &only).await.unwrap();

    assert_eq!(
        rec.calls(),
        vec![
            (MemoryTarget::Memory, "User keeps tomato plants".to_string()),
            (MemoryTarget::User, "User plays chess".to_string()),
            (MemoryTarget::Memory, "User once wore a hat".to_string()),
        ],
        "one exact-text call per Tier 1-linked claim, none for the unlinked one"
    );
    // Linked rows still existed at call time (the failing call included).
    assert!(rec.row_existed.lock().unwrap().iter().all(|e| *e));
    assert_eq!(rec.row_existed.lock().unwrap().len(), 4);
    // T-58.2-82: no store write transaction and no pending lock was held across any call.
    assert!(rec.probes_ok.lock().unwrap().iter().all(|ok| *ok));
    assert_eq!(report.tier1_lines_removed, 3);
    assert_eq!(
        report.purged,
        vec![
            linked.claim_id.clone(),
            promoted.claim_id.clone(),
            moved.claim_id.clone(),
            plain.claim_id.clone()
        ]
    );
    assert_eq!(status(&svc, &stuck.claim_id), "staged_forget");
    assert_eq!(
        load_pending(svc.paths()).unwrap().staged.len(),
        1,
        "only the failed claim stays in pending.json"
    );
}

#[tokio::test]
async fn purge_skips_when_maintenance_running() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add(&svc, "User prefers concise Telegram replies");
    stage(&svc, &c).await;
    let only = std::slice::from_ref(&c.claim_id);
    let rec = Recorder::new(tmp.path());

    // Consolidation holds its lease.
    assert!(svc.store().acquire_lease(600).unwrap());
    let report = svc.run_purge(&rec, PurgeSurface::Cli, only).await.unwrap();
    assert!(report.skipped.is_some(), "{report:?}");
    assert!(report.purged.is_empty());
    assert_eq!(status(&svc, &c.claim_id), "staged_forget");
    svc.store().release_lease().unwrap();

    // Dreams holds its lease: the consolidation lease the purge took first must be released.
    assert!(
        svc.store()
            .acquire_lease_for(ironhermes_tier2::store::DREAMS_LEASE_KEY, 600)
            .unwrap()
    );
    let report = svc.run_purge(&rec, PurgeSurface::Cli, only).await.unwrap();
    assert!(report.skipped.is_some(), "{report:?}");
    assert_eq!(status(&svc, &c.claim_id), "staged_forget");
    assert_eq!(
        svc.store()
            .meta_get(ironhermes_tier2::store::LEASE_KEY)
            .unwrap(),
        None,
        "a taken lease is released when the other is held"
    );
    svc.store()
        .release_lease_for(ironhermes_tier2::store::DREAMS_LEASE_KEY)
        .unwrap();

    // Nothing running: it purges, and both leases are free afterwards.
    let report = svc.run_purge(&rec, PurgeSurface::Cli, only).await.unwrap();
    assert_eq!(report.purged.len(), 1, "{report:?}");
    for key in [
        ironhermes_tier2::store::LEASE_KEY,
        ironhermes_tier2::store::DREAMS_LEASE_KEY,
    ] {
        assert_eq!(svc.store().meta_get(key).unwrap(), None, "{key} left held");
    }
}

#[tokio::test]
async fn purge_audited() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let a = add(&svc, "User prefers CANARY-ONE replies");
    let b = add(&svc, "User dislikes CANARY-TWO weather");
    stage(&svc, &a).await;
    stage(&svc, &b).await;
    let rec = Recorder::new(tmp.path());
    let only = vec![a.claim_id.clone(), b.claim_id.clone()];
    svc.run_purge(&rec, PurgeSurface::Cli, &only).await.unwrap();

    let log = std::fs::read_to_string(tmp.path().join("logs/audit.jsonl")).unwrap();
    let rows: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .filter(|v: &serde_json::Value| v["kind"] == "memory_forget_purged")
        .collect();
    assert_eq!(rows.len(), 2, "{log}");
    for (row, c) in rows.iter().zip([&a, &b]) {
        let args: serde_json::Value = serde_json::from_str(row["args"].as_str().unwrap()).unwrap();
        assert_eq!(args["claim_id"], c.claim_id.as_str());
        assert_eq!(args["text_hash"], c.text_hash.as_str());
        assert_eq!(args["surface"], "cli");
    }
    assert!(
        !log.contains("CANARY"),
        "audit rows never carry claim text: {log}"
    );
}

/// Path -> content hash of every file under `<home>/{tier2,memories,logs}`.
fn snapshot(home: &Path) -> std::collections::BTreeMap<String, u64> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    fn walk(dir: &Path, out: &mut std::collections::BTreeMap<String, u64>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                let mut h = DefaultHasher::new();
                std::fs::read(&p).unwrap().hash(&mut h);
                out.insert(p.display().to_string(), h.finish());
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    for d in ["tier2", "memories", "logs"] {
        walk(&home.join(d), &mut out);
    }
    out
}

#[tokio::test]
async fn purge_preview_lists_everything_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add_linked(
        &svc,
        "User lives in Lisbon",
        "tier1",
        Some(("user", Some(1))),
    );
    let old = add(&svc, "User lives in Porto");
    let other = add(&svc, "User enjoys Lisbon trams");
    supersede(&svc, &old, &c);
    evidence(&svc, c.id, 11, "I live in Lisbon");
    evidence(&svc, other.id, 11, "Lisbon trams are great");
    // Two live lines cite the claim; a third cites another claim.
    std::fs::write(
        svc.paths().live.join("2026-01-01.md"),
        format!(
            "# Live memory 2026-01-01\n- [{}] User lives in Lisbon\n- [{}] unrelated\n- [{}] User lives in Lisbon\n",
            c.claim_id, other.claim_id, c.claim_id
        ),
    )
    .unwrap();
    stage(&svc, &c).await;

    let before = snapshot(tmp.path());
    let preview = svc.purge_preview().await.unwrap();
    assert_eq!(
        snapshot(tmp.path()),
        before,
        "the dry run must not write anything"
    );
    assert_eq!(
        count(
            &svc,
            "SELECT COUNT(*) FROM meta WHERE key LIKE '%lease%'",
            []
        ),
        0
    );

    assert!(preview.skipped.is_none() && !preview.scrub_pending);
    assert_eq!(preview.claims.len(), 1);
    let p = &preview.claims[0];
    assert_eq!(p.claim_id, c.claim_id);
    assert_eq!(p.text, "User lives in Lisbon");
    assert_eq!(p.predecessors, vec![old.claim_id.clone()]);
    assert_eq!(
        p.tier1_entries,
        vec![("user".to_string(), "User lives in Lisbon".to_string())]
    );
    assert_eq!(p.live_lines, 2);
    assert_eq!(preview.redacted_quotes, 1);
}

#[tokio::test]
async fn purge_preview_requires_flag() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let svc = open(tmp.path());
        let c = add(&svc, "User prefers concise Telegram replies");
        stage(&svc, &c).await;
    }
    let preview = open_with(tmp.path(), false).purge_preview().await.unwrap();
    assert!(preview.skipped.is_some(), "{preview:?}");
    assert!(preview.claims.is_empty());
}

fn today_live(svc: &Tier2Service) -> std::path::PathBuf {
    svc.paths()
        .live
        .join(format!("{}.md", chrono::Utc::now().format("%Y-%m-%d")))
}

/// Append the claim's line to today's live log, as consolidation does.
fn live_line(svc: &Tier2Service, c: &ClaimRow) {
    ironhermes_tier2::live::append_lines(
        &svc.paths().live,
        chrono::Utc::now().date_naive(),
        &[ironhermes_tier2::live::claim_line(&c.claim_id, &c.text)],
    )
    .unwrap();
}

fn row_by_text(svc: &Tier2Service, text: &str) -> ClaimRow {
    let id = svc
        .store()
        .list_recent(100)
        .unwrap()
        .into_iter()
        .find(|c| c.text == text)
        .unwrap()
        .claim_id;
    svc.store().claim_by_claim_id(&id).unwrap().unwrap()
}

#[tokio::test]
async fn purge_live_lines_placeholdered() {
    use ironhermes_core::MemoryEntries;
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    // A consolidation claim, a D-29 moved Tier 1 line and a D-31 note, plus an unstaged bystander.
    let a = add(&svc, "User keeps bees alpha");
    live_line(&svc, &a);
    let mut entries = MemoryEntries::default();
    entries.entries.insert(
        MemoryTarget::Memory,
        vec!["User wears a hat beta".to_string()],
    );
    svc.sync_tier1(&entries).await.unwrap();
    svc.sync_tier1(&MemoryEntries::default()).await.unwrap();
    let keep = add(&svc, "Bystander gamma");
    live_line(&svc, &keep);
    let note = svc.write_note("User plays oboe delta").await.unwrap();
    embed_pending(&svc).await.unwrap();
    let b = row_by_text(&svc, "User wears a hat beta");
    let c = svc
        .store()
        .claim_by_claim_id(&note.claim_id)
        .unwrap()
        .unwrap();

    let path = today_live(&svc);
    let before = std::fs::read_to_string(&path).unwrap();
    let keep_at = before
        .lines()
        .position(|l| l.contains(&keep.claim_id))
        .unwrap();
    for x in [&a, &b, &c] {
        stage(&svc, x).await;
    }
    let rec = Recorder::new(tmp.path());
    let ids = [a.claim_id.clone(), b.claim_id.clone(), c.claim_id.clone()];
    let report = svc.run_purge(&rec, PurgeSurface::Cli, &ids).await.unwrap();
    assert_eq!(report.purged.len(), 3, "{report:?}");
    assert_eq!(report.live_lines_retracted, 3);

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after.lines().count(), before.lines().count());
    assert_eq!(
        after.lines().filter(|l| *l == "<!-- retracted -->").count(),
        3
    );
    for id in &ids {
        assert!(!after.contains(id.as_str()), "{id} still cited:\n{after}");
    }
    assert_eq!(
        after.lines().nth(keep_at).unwrap(),
        format!("- [{}] Bystander gamma", keep.claim_id),
        "a bystander's #L<n> citation must still resolve to its own text"
    );
    assert_eq!(after.lines().next(), before.lines().next(), "header kept");
}

// ------------------------------------------------------------ artifacts, scrub, re-learning

const NEEDLE: &str = "zqxneedle7731";

/// A service with a scripted memory LLM and a real `state.db` holding one session.
struct Full {
    tmp: tempfile::TempDir,
    svc: Tier2Service,
    llm: std::sync::Arc<ironhermes_tier2::test_support::FakeLlm>,
    state: ironhermes_state::StateStore,
}

fn full() -> Full {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = ironhermes_state::StateStore::new(tmp.path().join("state.db")).unwrap();
    state
        .create_session("s1", "cli", None, None, None, None)
        .unwrap();
    let cfg = MemoryConfig {
        forgetting_enabled: true,
        consolidation_enabled: true,
        dreams_enabled: true,
        ..Default::default()
    };
    let llm = std::sync::Arc::new(ironhermes_tier2::test_support::FakeLlm::new(vec![]));
    let svc = Tier2Service::open_with_embedder(
        tmp.path(),
        &cfg,
        Embedder::Fake(FakeEmbedder::new("fake:a")),
    )
    .unwrap()
    .with_llm(llm.clone());
    Full {
        tmp,
        svc,
        llm,
        state,
    }
}

impl Full {
    fn message(&mut self, text: &str) -> i64 {
        self.state
            .add_message("s1", &ironhermes_core::ChatMessage::user(text))
            .unwrap()
    }

    fn script_claims(&self, claims: &[(&str, &str, i64)]) {
        let claims: Vec<serde_json::Value> = claims
            .iter()
            .map(|(text, subject, mid)| {
                serde_json::json!({"text": text, "kind": "world", "subject": subject,
                    "direct": true, "durable": true, "message_ids": [mid]})
            })
            .collect();
        self.llm
            .push(Ok(serde_json::json!({ "claims": claims }).to_string()));
    }
}

/// Relative paths of every file under `<home>/tier2/` (hidden, WAL and index included) whose
/// bytes contain `needle`. A real recursive scan: a walk that errors cannot read as "absent".
fn files_containing(home: &Path, needle: &str) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, needle: &[u8], out: &mut Vec<String>, seen: &mut usize) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, needle, out, seen);
            } else {
                *seen += 1;
                let bytes = std::fs::read(&p).unwrap();
                if bytes.windows(needle.len()).any(|w| w == needle) {
                    out.push(p.strip_prefix(root).unwrap().display().to_string());
                }
            }
        }
    }
    let root = home.join("tier2");
    let (mut out, mut seen) = (Vec::new(), 0);
    walk(&root, &root, needle.as_bytes(), &mut out, &mut seen);
    assert!(seen > 5, "the scan walked only {seen} files");
    out
}

#[tokio::test]
async fn purge_needle_absent_everywhere() {
    use ironhermes_core::MemoryEntries;
    let mut f = full();
    let mid = f.message(&format!("I keep {NEEDLE} kilns"));
    let (ta, tb) = (
        format!("User keeps {NEEDLE} kilns"),
        format!("Dana repairs {NEEDLE} kilns"),
    );
    f.script_claims(&[(&ta, "user", mid), (&tb, "person:Dana", mid)]);
    f.svc.consolidate_now().await.unwrap();
    let (a, a2) = (row_by_text(&f.svc, &ta), row_by_text(&f.svc, &tb));

    // Dreams: a review, a synthesis bullet, a repair thread and the YAML companions.
    let item =
        |t: &str| serde_json::json!({"text": format!("{t} {NEEDLE}"), "cites": [&a.claim_id]});
    f.llm.push(Ok(serde_json::json!({
        "review": [item("Review")],
        "synthesis": {"preferences": [item("Prefers")], "boundaries": [], "friction": [], "guidance": []},
        "repair_threads": [{"thread": format!("Kiln {NEEDLE} repair"), "status": "open", "cites": [&a.claim_id]}],
        "progression": [{"change": format!("Now {NEEDLE}"), "cites": [&a.claim_id]}],
        "alignment_summary": [item("Summary")]
    })
    .to_string()));
    let dreams = f.svc.run_dreams().await.unwrap();
    assert!(dreams.skipped.is_none(), "{dreams:?}");

    // D-29: a Tier 1 line that moved to the live log. D-31: a note.
    let tier1 = format!("User owns {NEEDLE} glaze");
    let mut entries = MemoryEntries::default();
    entries
        .entries
        .insert(MemoryTarget::Memory, vec![tier1.clone()]);
    f.svc.sync_tier1(&entries).await.unwrap();
    f.svc.sync_tier1(&MemoryEntries::default()).await.unwrap();
    let b = row_by_text(&f.svc, &tier1);
    let note = f
        .svc
        .write_note(&format!("User fired {NEEDLE} bowls"))
        .await
        .unwrap();
    let c = f
        .svc
        .store()
        .claim_by_claim_id(&note.claim_id)
        .unwrap()
        .unwrap();
    embed_pending(&f.svc).await.unwrap();

    // The scan must see the needle first, or "found nowhere" proves nothing.
    let before = files_containing(f.tmp.path(), NEEDLE);
    for dir in ["live/", "bank/", "people/", "dreams/", "tier2.db"] {
        assert!(
            before.iter().any(|p| p.starts_with(dir)),
            "needle not seeded under {dir}: {before:?}"
        );
    }

    let ids: Vec<String> = [&a, &a2, &b, &c].map(|x| x.claim_id.clone()).to_vec();
    for x in [&a, &a2, &b, &c] {
        stage(&f.svc, x).await;
    }
    let rec = Recorder::new(f.tmp.path());
    let report = f
        .svc
        .run_purge(&rec, PurgeSurface::Cli, &ids)
        .await
        .unwrap();
    assert_eq!(report.purged.len(), 4, "{report:?}");
    assert!(report.scrub_incomplete.is_none(), "{report:?}");

    let after = files_containing(f.tmp.path(), NEEDLE);
    assert!(after.is_empty(), "needle survives in {after:?}");
}

#[tokio::test]
async fn purge_supersession_chain_and_shared_quotes() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let new = add(&svc, "User lives in Lisbon");
    let old = add(&svc, "User lives in Porto");
    let other = add(&svc, "User enjoys Lisbon trams");
    supersede(&svc, &old, &new);
    live_line(&svc, &old);
    live_line(&svc, &other);
    evidence(&svc, new.id, 11, "I live in Lisbon");
    evidence(&svc, old.id, 12, "I live in Porto");
    evidence(&svc, other.id, 11, "Lisbon trams are great");
    stage(&svc, &new).await;

    let rec = Recorder::new(tmp.path());
    let report = svc
        .run_purge(&rec, PurgeSurface::Cli, std::slice::from_ref(&new.claim_id))
        .await
        .unwrap();
    assert_eq!(report.purged, vec![new.claim_id.clone()], "{report:?}");
    assert!(!claim_exists(&svc, &old.claim_id), "predecessor purged");
    assert!(claim_exists(&svc, &other.claim_id));
    assert_eq!(
        count(
            &svc,
            "SELECT COUNT(*) FROM evidence WHERE claim_pk = ?1 AND quote = '[retracted]'",
            [other.id]
        ),
        1,
        "the unrelated claim keeps its evidence row, quote retracted"
    );
    // The predecessor's live line goes with it; the bystander's stays.
    let live = std::fs::read_to_string(today_live(&svc)).unwrap();
    assert!(!live.contains(&old.claim_id), "{live}");
    assert!(live.contains(&other.claim_id), "{live}");
    assert_eq!(report.live_lines_retracted, 1);
}

#[tokio::test]
async fn purge_index_rebuilt() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let gone = add(&svc, "User restores vintage bicycles");
    let kept = add(&svc, "User bakes sourdough bread");
    embed_pending(&svc).await.unwrap();
    // Load the index into this process's cache, as a running gateway would have.
    assert!(!svc.search("bicycles", 5).await.unwrap().results.is_empty());
    let generation = svc.store().index_generation().unwrap();
    stage(&svc, &gone).await;

    let rec = Recorder::new(tmp.path());
    svc.run_purge(
        &rec,
        PurgeSurface::Cli,
        std::slice::from_ref(&gone.claim_id),
    )
    .await
    .unwrap();
    assert!(svc.store().index_generation().unwrap() > generation);
    assert!(!svc.paths().index.exists(), "index file removed, not saved");

    let res = svc.search(&kept.text, 5).await.unwrap();
    assert!(res.results.iter().all(|h| h.claim_id != gone.claim_id));
    assert!(res.results.iter().any(|h| h.claim_id == kept.claim_id));
    let again = svc.search(&gone.text, 5).await.unwrap();
    assert!(again.results.iter().all(|h| h.claim_id != gone.claim_id));
}

#[tokio::test]
async fn purge_tombstone_blocks_relearning() {
    use ironhermes_core::MemoryEntries;
    let mut f = full();
    let text = "User enjoys alpine pottery";
    let mid = f.message("I enjoy alpine pottery");
    f.script_claims(&[(text, "user", mid)]);
    f.svc.consolidate_now().await.unwrap();
    let c = row_by_text(&f.svc, text);
    stage(&f.svc, &c).await;
    let rec = Recorder::new(f.tmp.path());
    let report = f
        .svc
        .run_purge(&rec, PurgeSurface::Cli, std::slice::from_ref(&c.claim_id))
        .await
        .unwrap();
    assert_eq!(report.purged.len(), 1, "{report:?}");
    let calls = f.llm.calls().len();

    // Reset the watermark and replay the same messages with the same script.
    f.svc
        .store()
        .meta_set(ironhermes_tier2::store::WATERMARK_KEY, "0")
        .unwrap();
    f.script_claims(&[(text, "user", mid)]);
    f.svc.consolidate_now().await.unwrap();
    assert_eq!(
        f.llm.calls().len(),
        calls,
        "a tombstoned message reached the LLM"
    );
    assert!(f.svc.store().list_recent(100).unwrap().is_empty());

    let mut entries = MemoryEntries::default();
    entries
        .entries
        .insert(MemoryTarget::Memory, vec![text.to_string()]);
    let sync = f.svc.sync_tier1(&entries).await.unwrap();
    assert_eq!(sync.skipped_tombstoned, 1, "{sync:?}");
    assert!(f.svc.store().list_recent(100).unwrap().is_empty());
    assert!(f.svc.write_note(text).await.is_err());
}

#[tokio::test]
async fn purge_idempotent_and_resumable() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let a = add_linked(
        &svc,
        "User owns a canoe",
        "tier1",
        Some(("memory", Some(1))),
    );
    let b = add_linked(&svc, "User owns a boat", "tier1", Some(("memory", Some(2))));
    stage(&svc, &a).await;
    stage(&svc, &b).await;
    let both = vec![a.claim_id.clone(), b.claim_id.clone()];

    // The Tier 1 write fails for the second claim: the first is purged, the second stays staged.
    let rec = Recorder::new(tmp.path()).failing_on("User owns a boat");
    let r1 = svc.run_purge(&rec, PurgeSurface::Cli, &both).await.unwrap();
    assert_eq!(r1.purged, vec![a.claim_id.clone()], "{r1:?}");
    assert_eq!(status(&svc, &b.claim_id), "staged_forget");
    let r2 = svc
        .run_purge(&Recorder::new(tmp.path()), PurgeSurface::Cli, &both)
        .await
        .unwrap();
    assert_eq!(r2.purged, vec![b.claim_id.clone()], "{r2:?}");
    let r3 = svc
        .run_purge(&Recorder::new(tmp.path()), PurgeSurface::Cli, &both)
        .await
        .unwrap();
    assert!(r3.purged.is_empty(), "{r3:?}");
    assert_eq!(r3.live_lines_retracted, 0);

    // A crash after the row was deleted: its pending entry completes and the retractions run.
    let c = add(&svc, "User keeps goats");
    live_line(&svc, &c);
    stage(&svc, &c).await;
    db(&svc)
        .execute("DELETE FROM claims WHERE id = ?1", [c.id])
        .unwrap();
    let r4 = svc
        .run_purge(
            &Recorder::new(tmp.path()),
            PurgeSurface::Cli,
            std::slice::from_ref(&c.claim_id),
        )
        .await
        .unwrap();
    assert_eq!(r4.purged, vec![c.claim_id.clone()], "{r4:?}");
    assert_eq!(r4.live_lines_retracted, 1);
    assert!(load_pending(svc.paths()).unwrap().staged.is_empty());
}

#[tokio::test]
async fn purge_finishes_pending_scrub_on_empty_run() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let marker = tmp.path().join("tier2/forget/scrub_pending");
    let a = add(&svc, "User keeps goats");
    stage(&svc, &a).await;
    let rec = Recorder::new(tmp.path());
    svc.run_purge(&rec, PurgeSurface::Cli, std::slice::from_ref(&a.claim_id))
        .await
        .unwrap();
    assert!(!marker.exists(), "a complete scrub removes the marker");
    assert!(!svc.purge_preview().await.unwrap().scrub_pending);

    // Nothing staged and no marker: nothing happens, the index generation included.
    let generation = svc.store().index_generation().unwrap();
    let r = svc.run_purge(&rec, PurgeSurface::Cli, &[]).await.unwrap();
    assert!(r.purged.is_empty() && r.skipped.is_none(), "{r:?}");
    assert_eq!(svc.store().index_generation().unwrap(), generation);
    assert!(!marker.exists());

    // An unfinished scrub (marker left behind) is finished by the next run, staged or not.
    std::fs::write(&marker, b"").unwrap();
    assert!(svc.purge_preview().await.unwrap().scrub_pending);
    let r = svc.run_purge(&rec, PurgeSurface::Cli, &[]).await.unwrap();
    assert!(r.purged.is_empty(), "{r:?}");
    assert!(svc.store().index_generation().unwrap() > generation);
    assert!(r.scrub_incomplete.is_none(), "{r:?}");
    assert!(!marker.exists());
    assert!(!svc.purge_preview().await.unwrap().scrub_pending);
}

/// CR-01: the artifact half fails after the rows are deleted. The report still names the purged
/// claim (not an error), the marker keeps its id across a restart (`reconcile` drops the pending
/// entry), and the next empty run retracts the live line and the dreams citation.
#[cfg(unix)]
#[tokio::test]
async fn purge_recovers_scrub_after_artifact_failure() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let marker = tmp.path().join("tier2/forget/scrub_pending");
    let needle = "User hides the spare key under the blue pot";
    let c = add(&svc, needle);
    live_line(&svc, &c);
    let dreams = &svc.paths().dreams;
    std::fs::create_dir_all(dreams).unwrap();
    let dream = dreams.join("2026-01-01.md");
    std::fs::write(&dream, format!("# d\n- insight [{}]\n", c.claim_id)).unwrap();
    stage(&svc, &c).await;

    let live = svc.paths().live.clone();
    let has_needle = || {
        std::fs::read_dir(&live).unwrap().any(|e| {
            let p = e.unwrap().path();
            p.extension().is_some_and(|x| x == "md")
                && std::fs::read_to_string(&p).unwrap().contains(needle)
        })
    };
    assert!(has_needle());

    // Inject: the live-log rewrite cannot create its tmp file.
    std::fs::set_permissions(&live, std::fs::Permissions::from_mode(0o555)).unwrap();
    let rec = Recorder::new(tmp.path());
    let r = svc
        .run_purge(&rec, PurgeSurface::Cli, std::slice::from_ref(&c.claim_id))
        .await;
    std::fs::set_permissions(&live, std::fs::Permissions::from_mode(0o755)).unwrap();
    let r = r.expect("deleted rows must be reported, not turned into an error");
    assert_eq!(r.purged, vec![c.claim_id.clone()], "{r:?}");
    assert!(r.scrub_incomplete.is_some(), "{r:?}");
    assert!(!claim_exists(&svc, &c.claim_id));
    assert!(has_needle(), "injection did not stop the retraction");
    assert!(marker.exists());
    drop(svc);

    // A restart reconciles pending.json; the marker still carries the id.
    let svc = open(tmp.path());
    assert!(svc.purge_preview().await.unwrap().scrub_pending);

    // An unreadable YAML that still cites the id keeps the scrub pending (the guard holds).
    let yaml = svc.paths().dreams.join("ALIGNMENT_STATE.yaml");
    std::fs::write(&yaml, format!(":\n  - [{}\n", c.claim_id)).unwrap();
    assert!(
        svc.run_purge(&rec, PurgeSurface::Cli, &[]).await.is_err(),
        "a scrub that left the id on disk must not report success"
    );
    assert!(marker.exists());
    std::fs::remove_file(&yaml).unwrap();

    let r = svc.run_purge(&rec, PurgeSurface::Cli, &[]).await.unwrap();
    assert!(r.scrub_incomplete.is_none(), "{r:?}");
    assert!(!has_needle(), "the live-log line survived the recovery");
    assert!(
        !std::fs::read_to_string(&dream)
            .unwrap()
            .contains(&c.claim_id),
        "the dreams citation survived the recovery"
    );
    assert!(!marker.exists());
}

/// WR-03: a claim linked to Tier 1 whose ordinal is not synced yet (a promotion mark, or a failed
/// post-add sync) still has its Tier 1 line removed before the tombstone hides it from the sync.
#[tokio::test]
async fn purge_removes_tier1_line_without_ordinal() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path());
    let c = add_linked(
        &svc,
        "User is allergic to peanuts",
        "consolidation",
        Some(("user", None)),
    );
    stage(&svc, &c).await;
    let rec = Recorder::new(tmp.path());
    let r = svc
        .run_purge(&rec, PurgeSurface::Cli, std::slice::from_ref(&c.claim_id))
        .await
        .unwrap();
    assert_eq!(r.purged, vec![c.claim_id.clone()], "{r:?}");
    assert_eq!(
        rec.calls(),
        vec![(
            MemoryTarget::User,
            "User is allergic to peanuts".to_string()
        )]
    );
}
