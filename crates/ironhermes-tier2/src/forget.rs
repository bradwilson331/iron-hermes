//! ADR-0008 Stage 4, first half: stage a forget (D-21), cancel it (D-23), reconcile after a crash.
//!
//! Staging only flips `claims.status` to `staged_forget` (every read surface already filters to
//! `active`) and records the claim in `<home>/tier2/forget/pending.json`. Nothing is deleted here;
//! the irreversible purge is a later plan. `pending.json` is the record of intent: it is written
//! first when staging, and on disagreement `reconcile` re-stages the DB from it. Every pending.json
//! read-modify-write holds `pending.json.lock` (cross-process), the later human-run purge (D-28)
//! included.

use std::collections::HashMap;
use std::io::Write;

use anyhow::Context;
use ironhermes_core::config::MemoryConfig;
use ironhermes_core::memory_store::MemoryTarget;
use ironhermes_core::tier2::{
    ForgetOutcome, PurgePreview, PurgePreviewClaim, PurgeReport, PurgeSurface, Tier1Remover,
    is_valid_claim_id,
};
use serde::{Deserialize, Serialize};

use crate::consolidate::LEASE_SECS;
use crate::paths::Tier2Paths;
use crate::service::Tier2Service;
use crate::store::{ClaimRow, DREAMS_LEASE_KEY, LEASE_KEY, Store};

pub const PENDING_FILE: &str = "pending.json";
pub const PENDING_LOCK_FILE: &str = "pending.json.lock";
const PENDING_VERSION: u32 = 1;
const MAX_REASON_CHARS: usize = 200;
const STAGED: &str = "staged_forget";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PendingFile {
    pub version: u32,
    pub staged: Vec<PendingEntry>,
}

impl Default for PendingFile {
    fn default() -> Self {
        Self {
            version: PENDING_VERSION,
            staged: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PendingEntry {
    pub claim_id: String,
    pub staged_at: String,
    /// `active` or `superseded`: what a cancel restores.
    pub prior_status: String,
    pub reason: Option<String>,
}

/// Blocking exclusive lock for a pending.json read-modify-write; only call from a blocking thread.
fn lock(paths: &Tier2Paths) -> anyhow::Result<std::fs::File> {
    let path = paths.forget.join(PENDING_LOCK_FILE);
    crate::paths::lock_exclusive(&path).with_context(|| format!("locking {}", path.display()))
}

fn try_lock(paths: &Tier2Paths) -> anyhow::Result<Option<std::fs::File>> {
    let path = paths.forget.join(PENDING_LOCK_FILE);
    crate::paths::try_lock_exclusive(&path).with_context(|| format!("locking {}", path.display()))
}

/// A missing file is an empty list; an unreadable or malformed one is an error.
pub fn load_pending(paths: &Tier2Paths) -> anyhow::Result<PendingFile> {
    let path = paths.forget.join(PENDING_FILE);
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PendingFile::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// tmp + rename, file mode 0600 on unix.
pub fn save_pending(paths: &Tier2Paths, file: &PendingFile) -> anyhow::Result<()> {
    std::fs::create_dir_all(&paths.forget)?;
    let tmp = paths.forget.join(format!(".{PENDING_FILE}.tmp"));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts
        .open(&tmp)
        .with_context(|| format!("writing {}", tmp.display()))?;
    f.write_all(serde_json::to_string_pretty(file)?.as_bytes())?;
    f.sync_all()?;
    let dest = paths.forget.join(PENDING_FILE);
    std::fs::rename(&tmp, &dest).with_context(|| format!("renaming to {}", dest.display()))?;
    Ok(())
}

/// Flag and id checks that must pass before any store or file access (T-58.2-28, T-58.2-31).
fn guard(cfg: &MemoryConfig, claim_id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        cfg.forgetting_enabled,
        "forgetting is disabled: set memory.forgetting_enabled to true to use memory_forget"
    );
    anyhow::ensure!(
        is_valid_claim_id(claim_id),
        "'claim_id' must look like claim:0123456789ab"
    );
    Ok(())
}

fn outcome(claim_id: &str, status: &str, note: &str) -> ForgetOutcome {
    ForgetOutcome {
        claim_id: claim_id.into(),
        status: status.into(),
        note: note.into(),
    }
}

fn not_found() -> anyhow::Error {
    anyhow::anyhow!("not_found: no such claim")
}

/// Hide a claim now; it stays hidden until cancelled (no purge exists yet, D-27). Staging an
/// already-staged claim changes nothing.
pub fn stage(
    store: &Store,
    paths: &Tier2Paths,
    cfg: &MemoryConfig,
    claim_id: &str,
    reason: Option<&str>,
) -> anyhow::Result<ForgetOutcome> {
    guard(cfg, claim_id)?;
    if let Some(r) = reason {
        anyhow::ensure!(
            r.chars().count() <= MAX_REASON_CHARS,
            "'reason' is too long (max {MAX_REASON_CHARS} characters)"
        );
    }
    let _g = lock(paths)?;
    let row = store.claim_by_claim_id(claim_id)?.ok_or_else(not_found)?;
    let mut pending = load_pending(paths)?;
    let known = pending.staged.iter().any(|e| e.claim_id == claim_id);
    if row.status == STAGED {
        if !known {
            pending.staged.push(entry(claim_id, "active", reason));
            save_pending(paths, &pending)?;
        }
    } else {
        // Intent first: a crash before the DB update is repaired by `reconcile`.
        pending.staged.retain(|e| e.claim_id != claim_id);
        pending.staged.push(entry(claim_id, &row.status, reason));
        save_pending(paths, &pending)?;
        if let Err(e) = store.set_claim_status(claim_id, STAGED) {
            pending.staged.retain(|e| e.claim_id != claim_id);
            let _ = save_pending(paths, &pending);
            return Err(e);
        }
    }
    Ok(outcome(
        claim_id,
        "staged",
        "hidden now and stays hidden until cancelled; not deleted",
    ))
}

fn entry(claim_id: &str, prior: &str, reason: Option<&str>) -> PendingEntry {
    PendingEntry {
        claim_id: claim_id.into(),
        staged_at: chrono::Utc::now().to_rfc3339(),
        // Only these two can be restored to; anything else collapses to the safe visible state.
        prior_status: if prior == "superseded" {
            "superseded"
        } else {
            "active"
        }
        .into(),
        reason: reason.map(str::to_string),
    }
}

/// Cancel a staged forget, restoring the claim's prior status. A claim that is not staged is left
/// untouched.
pub fn unstage(
    store: &Store,
    paths: &Tier2Paths,
    cfg: &MemoryConfig,
    claim_id: &str,
) -> anyhow::Result<ForgetOutcome> {
    guard(cfg, claim_id)?;
    let _g = lock(paths)?;
    let row = store.claim_by_claim_id(claim_id)?.ok_or_else(not_found)?;
    let mut pending = load_pending(paths)?;
    let prior = pending
        .staged
        .iter()
        .find(|e| e.claim_id == claim_id)
        .map_or("active", |e| e.prior_status.as_str())
        .to_string();
    // DB first. A crash before the file write leaves a stale entry that `reconcile` re-stages:
    // the claim stays hidden (never deleted early) and the user can cancel again.
    if row.status == STAGED {
        store.set_claim_status(claim_id, &prior)?;
    }
    let before = pending.staged.len();
    pending.staged.retain(|e| e.claim_id != claim_id);
    if pending.staged.len() != before {
        save_pending(paths, &pending)?;
    }
    Ok(outcome(claim_id, "cancelled", "restored; searchable again"))
}

/// Staged claims, most recently staged first.
pub fn list_staged(store: &Store, paths: &Tier2Paths) -> anyhow::Result<Vec<ClaimRow>> {
    let at: HashMap<String, String> = load_pending(paths)?
        .staged
        .into_iter()
        .map(|e| (e.claim_id, e.staged_at))
        .collect();
    let mut rows = store.staged_claims()?;
    rows.sort_by(|a, b| at.get(&b.claim_id).cmp(&at.get(&a.claim_id)));
    Ok(rows)
}

/// Make pending.json and the claims table agree. The file wins: a listed claim that is not staged
/// in the DB is staged; a DB-staged claim missing from the file is added; an entry whose claim no
/// longer exists is dropped.
pub fn reconcile(store: &Store, paths: &Tier2Paths) -> anyhow::Result<()> {
    // Runs synchronously from async boot code, so it must never wait on another process. The holder
    // is mid read-modify-write and leaves file and DB in agreement; crash leftovers are repaired at
    // the next start.
    let Some(_g) = try_lock(paths)? else {
        tracing::warn!(
            "tier2 forget: pending.json is locked by another process; reconcile skipped until the next start"
        );
        return Ok(());
    };
    let mut pending = match load_pending(paths) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("tier2 forget: ignoring unreadable pending file: {e:#}");
            PendingFile::default()
        }
    };
    let mut changed = false;
    let mut kept = Vec::with_capacity(pending.staged.len());
    for e in std::mem::take(&mut pending.staged) {
        match store.claim_by_claim_id(&e.claim_id)? {
            None => changed = true,
            Some(row) => {
                if row.status != STAGED {
                    store.set_claim_status(&e.claim_id, STAGED)?;
                }
                kept.push(e);
            }
        }
    }
    pending.staged = kept;
    for row in store.staged_claims()? {
        if !pending.staged.iter().any(|e| e.claim_id == row.claim_id) {
            pending.staged.push(entry(&row.claim_id, "active", None));
            changed = true;
        }
    }
    if changed {
        save_pending(paths, &pending)?;
    }
    Ok(())
}

/// Take the consolidation and dreams leases once each (a human is waiting, so no retry loop).
/// `false`, with nothing left held, when either is running.
fn take_leases(store: &Store) -> anyhow::Result<bool> {
    if !store.acquire_lease_for(LEASE_KEY, LEASE_SECS)? {
        return Ok(false);
    }
    match store.acquire_lease_for(DREAMS_LEASE_KEY, LEASE_SECS) {
        Ok(true) => Ok(true),
        other => {
            store.release_lease_for(LEASE_KEY)?;
            other
        }
    }
}

fn tier1_target(label: &str) -> Option<MemoryTarget> {
    match label {
        "memory" => Some(MemoryTarget::Memory),
        "user" => Some(MemoryTarget::User),
        _ => None,
    }
}

/// D-20, D-22, D-28, D-32 (database half): permanently delete the staged claims in `only`, with
/// their superseded predecessors, evidence and vectors. Per claim the Tier 1 line goes first,
/// outside every lock (so Tier 1's own write can re-enter Tier 2); then one store transaction
/// writes content-free tombstones, blanks other claims' quotes of the same messages and deletes.
/// Pending entries are removed after the loop. Ids not staged are reported and left alone, so a
/// claim staged after the human's preview survives (T-58.2-77).
///
/// `pub(crate)`: other crates reach it only through `Tier2Handle::run_purge`, which the
/// `no_agent_tool_can_purge` source lock watches. No scheduled job calls it (D-28).
pub(crate) async fn purge(
    svc: &Tier2Service,
    tier1: &dyn Tier1Remover,
    surface: PurgeSurface,
    only: &[String],
) -> anyhow::Result<PurgeReport> {
    let mut report = PurgeReport::default();
    if !svc.cfg.forgetting_enabled {
        report.skipped =
            Some("forgetting is disabled: set memory.forgetting_enabled to true to purge".into());
        return Ok(report);
    }
    let store = svc.store.clone();
    let took = tokio::task::spawn_blocking(move || take_leases(&store)).await??;
    if !took {
        report.skipped = Some("consolidation or dreams is running; try again in a minute".into());
        return Ok(report);
    }
    let outcome = purge_locked(svc, tier1, surface, only, &mut report).await;
    // Every exit path, errors included, frees the leases.
    let store = svc.store.clone();
    let freed = tokio::task::spawn_blocking(move || {
        let a = store.release_lease_for(LEASE_KEY);
        let b = store.release_lease_for(DREAMS_LEASE_KEY);
        a.and(b)
    })
    .await;
    if !matches!(freed, Ok(Ok(()))) {
        tracing::warn!("tier2 purge: lease release failed: {freed:?}");
    }
    match outcome {
        // Rows are already gone: report them, never "could not purge". The marker keeps their ids,
        // so the next run (staged or not) finishes the scrub.
        Err(e) if !report.purged.is_empty() => {
            tracing::warn!("tier2 purge: rows deleted, on-disk scrub stopped: {e:#}");
            report.scrub_incomplete = Some(format!(
                "the claims were deleted but the on-disk scrub stopped: {e:#}"
            ));
            Ok(report)
        }
        o => o.map(|()| report),
    }
}

async fn purge_locked(
    svc: &Tier2Service,
    tier1: &dyn Tier1Remover,
    surface: PurgeSurface,
    only: &[String],
    report: &mut PurgeReport,
) -> anyhow::Result<()> {
    let mut want: Vec<String> = Vec::new();
    for id in only {
        if !want.contains(id) {
            want.push(id.clone());
        }
    }
    let (store, paths) = (svc.store.clone(), svc.paths.clone());
    // Sort the ids into staged rows, finished-but-unrecorded orphans and not-staged.
    let (staged, orphans, not_staged) = tokio::task::spawn_blocking(move || {
        let before = load_pending(&paths).unwrap_or_default();
        reconcile(&store, &paths)?;
        let (mut staged, mut orphans, mut not_staged) = (Vec::new(), Vec::new(), Vec::new());
        for id in want {
            match store.claim_by_claim_id(&id)? {
                Some(row) if row.status == STAGED => staged.push(row),
                Some(_) => not_staged.push(id),
                // A crash after the delete but before the pending write: finish that entry.
                None if before.staged.iter().any(|e| e.claim_id == id) => orphans.push(id),
                None => not_staged.push(id),
            }
        }
        anyhow::Ok((staged, orphans, not_staged))
    })
    .await??;
    report.not_staged = not_staged;

    // Claim ids (purged claims and their predecessors) whose artifacts must be retracted.
    let mut retract: Vec<String> = Vec::new();
    let looped = purge_claims(svc, tier1, surface, staged, report, &mut retract).await;

    report.purged.extend(orphans.iter().cloned());
    retract.extend(orphans);
    let after = after_loop(svc, retract, report).await;
    looped.and(after)
}

/// D-35: file beside pending.json, one claim id per line: every id whose rows a purge is about to
/// delete, written and fsynced before the delete. Removed only when every after-loop step succeeds
/// with a complete scrub, so the next run retracts those ids' artifacts again (CR-01).
pub const SCRUB_PENDING_FILE: &str = "scrub_pending";

/// Append `ids` to the scrub marker (creating it) and fsync. Blocking.
fn record_scrub_ids(paths: &Tier2Paths, ids: &[String]) -> anyhow::Result<()> {
    std::fs::create_dir_all(&paths.forget)?;
    let path = paths.forget.join(SCRUB_PENDING_FILE);
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts
        .open(&path)
        .with_context(|| format!("writing {}", path.display()))?;
    for id in ids {
        writeln!(f, "{id}")?;
    }
    f.sync_all()?;
    // The marker's own directory entry must survive a crash too.
    #[cfg(unix)]
    std::fs::File::open(&paths.forget)?.sync_all()?;
    Ok(())
}

/// Ids in the scrub marker; none when it is absent (or an old empty marker).
fn read_scrub_ids(paths: &Tier2Paths) -> anyhow::Result<Vec<String>> {
    let path = paths.forget.join(SCRUB_PENDING_FILE);
    match std::fs::read_to_string(&path) {
        Ok(raw) => Ok(raw
            .lines()
            .map(str::trim)
            .filter(|l| is_valid_claim_id(l))
            .map(str::to_string)
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// D-32 after-loop sequence, run when something was purged or completed or a scrub is pending
/// (D-35). Each step takes its own lock and none are nested; no lock is held across `remove_tier1`
/// (that ran earlier). File and SQLite work runs in `spawn_blocking`, never `block_in_place`: the
/// web UI calls this on a per-connection `LocalSet` (T-58.2-89). Clearing the index slot is the
/// only `.await` among the steps.
async fn after_loop(
    svc: &Tier2Service,
    retract: Vec<String>,
    report: &mut PurgeReport,
) -> anyhow::Result<()> {
    let marker = svc.paths.forget.join(SCRUB_PENDING_FILE);
    if report.purged.is_empty() && !marker.exists() {
        return Ok(());
    }
    let (store, paths) = (svc.store.clone(), svc.paths.clone());
    let done = report.purged.clone();
    report.live_lines_retracted = tokio::task::spawn_blocking(move || {
        record_scrub_ids(&paths, &[])?;
        // Ids an earlier, unfinished run deleted. A recorded id whose row still exists was
        // cancelled before its delete and is not retracted.
        let mut retract = retract;
        for id in read_scrub_ids(&paths)? {
            if !retract.contains(&id) && store.claim_by_claim_id(&id)?.is_none() {
                retract.push(id);
            }
        }
        // 1. Daily live-log lines (holds live/.append.lock).
        let live_lines = crate::live::retract_claim_lines(&paths.live, &retract)?;
        // 2. Dreams, synthesis and YAML (the dreams lease is held by `purge`).
        crate::dreams::retract_citations(&paths, &retract)?;
        // 3. Finished entries leave pending.json (pending lock, taken alone).
        {
            let _g = lock(&paths)?;
            let mut pending = load_pending(&paths)?;
            let before = pending.staged.len();
            pending.staged.retain(|e| !done.contains(&e.claim_id));
            if pending.staged.len() != before {
                save_pending(&paths, &pending)?;
            }
        }
        // 4. bank/people/groups from the surviving rows (render lock, taken alone).
        crate::render::render_all(&store, &paths)?;
        anyhow::Ok(live_lines)
    })
    .await??;

    // 5. The vector index is a derived cache: remove the file under a generation bump, so every
    // process rebuilds from the surviving rows (no embedding model needed), then drop ours.
    let (store, index) = (svc.store.clone(), svc.paths.index.clone());
    tokio::task::spawn_blocking(move || {
        store.commit_index_write(|_, _| match std::fs::remove_file(&index) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("removing {}", index.display()))
            }
            _ => Ok(()),
        })
    })
    .await??;
    *svc.index.lock().await = None;

    // 6. Scrub. Rows are already overwritten by `secure_delete`, but FTS5 segments and free pages
    // need the rebuild and VACUUM (RESEARCH Pitfall 1). Another process (gateway, web UI) may hold
    // the database open, so VACUUM and the checkpoint are best effort and reported; the operator
    // stops those processes and runs the purge again for a guaranteed scrub (the marker, D-35).
    let store = svc.store.clone();
    let incomplete = tokio::task::spawn_blocking(move || {
        store.with_conn(|conn| {
            conn.execute("INSERT INTO claims_fts(claims_fts) VALUES('rebuild')", [])?;
            let mut why = Vec::new();
            if let Err(e) = conn.execute_batch("VACUUM") {
                why.push(format!("VACUUM failed: {e}"));
            }
            // A held WAL is a returned row (`busy` = 1), not an error.
            match conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                r.get::<_, i64>(0)
            }) {
                Ok(0) => {}
                Ok(_) => why.push("WAL checkpoint busy: another process holds the database".into()),
                Err(e) => why.push(format!("WAL checkpoint failed: {e}")),
            }
            Ok((!why.is_empty()).then(|| why.join("; ")))
        })
    })
    .await??;
    if incomplete.is_none() {
        std::fs::remove_file(&marker)?;
    }
    report.scrub_incomplete = incomplete;
    Ok(())
}

async fn purge_claims(
    svc: &Tier2Service,
    tier1: &dyn Tier1Remover,
    surface: PurgeSurface,
    staged: Vec<ClaimRow>,
    report: &mut PurgeReport,
    retract: &mut Vec<String>,
) -> anyhow::Result<()> {
    let surface_label = match surface {
        PurgeSurface::Cli => "cli",
        PurgeSurface::Ui => "ui",
    };
    for claim in staged {
        let store = svc.store.clone();
        let main = claim.clone();
        let (rows, mids) = tokio::task::spawn_blocking(move || {
            let mut rows = vec![main];
            rows.extend(store.supersession_chain(rows[0].id)?);
            let pks: Vec<i64> = rows.iter().map(|r| r.id).collect();
            let mids = store.evidence_message_ids(&pks)?;
            anyhow::Ok((rows, mids))
        })
        .await??;

        // D-22: the Tier 1 line first, while the rows still exist, holding no lock. An error
        // leaves this claim staged for the next run; the loop moves on. Any Tier 1 link counts,
        // not only a synced ordinal: a promotion mark (or a failed post-add sync) has a NULL
        // ordinal while its line is in USER.md / MEMORY.md (WR-03). The remove is an exact match
        // and returns `false` when the line is absent (a D-29 moved row).
        let mut failed = false;
        for row in rows.iter().filter(|r| r.tier1_target.is_some()) {
            let Some(target) = row.tier1_target.as_deref().and_then(tier1_target) else {
                tracing::warn!(
                    "tier2 purge: {} has an unknown Tier 1 target; left staged",
                    row.claim_id
                );
                failed = true;
                break;
            };
            match tier1.remove_tier1(target, &row.text).await {
                Ok(removed) => report.tier1_lines_removed += usize::from(removed),
                Err(e) => {
                    tracing::warn!(
                        "tier2 purge: Tier 1 removal for {} failed, left staged: {e:#}",
                        row.claim_id
                    );
                    failed = true;
                    break;
                }
            }
        }
        if failed {
            continue;
        }

        let (store, paths) = (svc.store.clone(), svc.paths.clone());
        let hashes: Vec<(i64, String)> = rows.iter().map(|r| (r.id, r.text_hash.clone())).collect();
        let ids: Vec<String> = rows.iter().map(|r| r.claim_id.clone()).collect();
        match tokio::task::spawn_blocking(move || {
            // CR-01: the ids to scrub reach disk before the irreversible delete.
            record_scrub_ids(&paths, &ids)?;
            store.purge_rows(&hashes, &mids)
        })
        .await??
        {
            Some(redacted) => {
                report.evidence_redacted += redacted;
                svc.audit_purged(&claim.claim_id, &claim.text_hash, surface_label);
                retract.extend(rows.iter().map(|r| r.claim_id.clone()));
                report.purged.push(claim.claim_id);
            }
            // Cancelled between the preview and now: it is no longer ours to delete.
            None => report.not_staged.push(claim.claim_id),
        }
    }
    Ok(())
}

/// Lines across the live logs that cite one of `ids` (`- [claim:..] text`). Read-only: live files
/// are append-only or renamed whole, so no lock is needed. Counts only (D-34).
fn count_live_lines(paths: &Tier2Paths, ids: &[&str]) -> anyhow::Result<usize> {
    let marks: Vec<String> = ids.iter().map(|id| format!("[{id}]")).collect();
    let dir = match std::fs::read_dir(&paths.live) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e).with_context(|| format!("reading {}", paths.live.display())),
    };
    let mut n = 0;
    for entry in dir {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        n += raw
            .lines()
            .filter(|l| marks.iter().any(|m| l.contains(m.as_str())))
            .count();
    }
    Ok(n)
}

/// D-28: the dry run. Everything a purge of the staged claims would delete, from SELECTs and file
/// reads only: no lease, no audit row, no file written.
///
/// shortcut: `redacted_quotes` is computed over every staged claim, so a purge of a subset can
/// retract fewer quotes; upgrade if the preview must be exact per selection.
///
/// `pub(crate)` like [`purge`]: other crates reach it only through `Tier2Handle::purge_preview`.
pub(crate) fn preview(
    store: &Store,
    paths: &Tier2Paths,
    cfg: &MemoryConfig,
) -> anyhow::Result<PurgePreview> {
    let mut out = PurgePreview::default();
    if !cfg.forgetting_enabled {
        out.skipped =
            Some("forgetting is disabled: set memory.forgetting_enabled to true to purge".into());
        return Ok(out);
    }
    // D-35: a metadata read only; the preview still writes nothing.
    out.scrub_pending = paths.forget.join(SCRUB_PENDING_FILE).exists();
    let (mut all_pks, mut all_mids) = (Vec::new(), Vec::new());
    for claim in list_staged(store, paths)? {
        let chain = store.supersession_chain(claim.id)?;
        let rows: Vec<&ClaimRow> = std::iter::once(&claim).chain(chain.iter()).collect();
        let pks: Vec<i64> = rows.iter().map(|r| r.id).collect();
        all_mids.extend(store.evidence_message_ids(&pks)?);
        all_pks.extend(pks);
        let ids: Vec<&str> = rows.iter().map(|r| r.claim_id.as_str()).collect();
        out.claims.push(PurgePreviewClaim {
            claim_id: claim.claim_id.clone(),
            text: claim.text.clone(),
            predecessors: chain.iter().map(|r| r.claim_id.clone()).collect(),
            tier1_entries: rows
                .iter()
                .filter(|r| r.tier1_ordinal.is_some())
                .filter_map(|r| Some((r.tier1_target.clone()?, r.text.clone())))
                .collect(),
            live_lines: count_live_lines(paths, &ids)?,
        });
    }
    all_mids.sort_unstable();
    all_mids.dedup();
    out.redacted_quotes = store.shared_quote_count(&all_pks, &all_mids)?;
    Ok(out)
}
