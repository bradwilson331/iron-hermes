//! `Tier2Service`: the `Tier2Handle` implementation. SQLite work runs in `spawn_blocking`
//! (never `block_in_place`, which panics in the web server's per-connection `LocalSet`).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use ironhermes_core::MemoryEntries;
use ironhermes_core::config::MemoryConfig;
use ironhermes_core::memory_store::MemoryTarget;
use ironhermes_core::tier2::{
    ConsolidationReport, DreamsReport, ForgetOutcome, NoteOutcome, PromotionCandidate,
    PromotionOutcome, PurgePreview, PurgeReport, PurgeSurface, Tier1Remover, Tier1SyncReport,
    Tier2Evidence, Tier2Explanation, Tier2Handle, Tier2Hit, Tier2SearchResponse,
};

use crate::embed::Embedder;
use crate::index::IndexState;
use crate::live;
use crate::llm::Tier2LlmHandle;
use crate::paths::Tier2Paths;
use crate::store::{NewClaim, Store, new_claim_id, text_hash};
use crate::synthesis::{SynthesisCache, build_block};

/// Longest accepted daily note, in characters (D-31).
pub const MAX_NOTE_CHARS: usize = 1000;

/// Largest `ALIGNMENT_SYNTHESIS.md` the session snapshot will read.
const MAX_SYNTHESIS_FILE_BYTES: u64 = 64 * 1024;

/// Cheap to clone (every field is shared or small) so background embedding can own a copy.
#[derive(Clone)]
pub struct Tier2Service {
    pub(crate) paths: Tier2Paths,
    pub(crate) store: Arc<Store>,
    pub(crate) embedder: Arc<Embedder>,
    /// Built lazily for the active embedding model on first use (see `search::ensure_index`).
    pub(crate) index: Arc<tokio::sync::Mutex<Option<IndexState>>>,
    /// Serialises `embed_pending` runs so a run started after a write sees that write.
    pub(crate) embed_lock: Arc<tokio::sync::Mutex<()>>,
    pub(crate) cfg: MemoryConfig,
    /// The memory model used by consolidation (D-16); `None` until the host wires one.
    pub(crate) llm: Option<Arc<dyn Tier2LlmHandle>>,
    /// D-19: per-session frozen `ALIGNMENT_SYNTHESIS.md` blocks.
    pub(crate) synthesis: Arc<SynthesisCache>,
}

impl Tier2Service {
    /// D-04: the embedder comes from `cfg.embeddings` (an endpoint when `base_url` is set, else
    /// the built-in model). Claims without an embedding are embedded in the background.
    pub fn open(home: &Path, cfg: &MemoryConfig) -> anyhow::Result<Self> {
        let embedder = Embedder::from_config(&cfg.embeddings, &Tier2Paths::from_home(home).models);
        let svc = Self::build(home, cfg, embedder)?;
        svc.spawn_embed_pending();
        Ok(svc)
    }

    /// Test seam: a chosen embedder and no background work, so tests control when embedding runs.
    #[cfg(any(test, feature = "test-support"))]
    pub fn open_with_embedder(
        home: &Path,
        cfg: &MemoryConfig,
        embedder: Embedder,
    ) -> anyhow::Result<Self> {
        Self::build(home, cfg, embedder)
    }

    fn build(home: &Path, cfg: &MemoryConfig, embedder: Embedder) -> anyhow::Result<Self> {
        let paths = Tier2Paths::from_home(home);
        paths.ensure_dirs()?;
        let store = Arc::new(Store::open(&paths.db)?);
        // Repair a stage/cancel that was interrupted between pending.json and the claims table.
        if let Err(e) = crate::forget::reconcile(&store, &paths) {
            tracing::warn!("tier2 forget reconcile failed: {e:#}");
        }
        Ok(Self {
            paths,
            store,
            embedder: Arc::new(embedder),
            index: Arc::default(),
            embed_lock: Arc::default(),
            cfg: cfg.clone(),
            llm: None,
            synthesis: Arc::default(),
        })
    }

    /// Attach the memory LLM that consolidation extracts claims with.
    pub fn with_llm(mut self, llm: Arc<dyn Tier2LlmHandle>) -> Self {
        self.llm = Some(llm);
        self
    }

    /// One consolidation run (D-15): extract, screen and store claims from new conversation turns.
    pub async fn consolidate_now(&self) -> anyhow::Result<ConsolidationReport> {
        crate::consolidate::run(self).await
    }

    /// Run `search::embed_pending` on the runtime, if there is one. Errors are logged, never
    /// surfaced: embedding is best effort and must not block or fail a write (T-58.2-49).
    pub(crate) fn spawn_embed_pending(&self) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let svc = self.clone();
        rt.spawn(async move {
            if let Err(e) = crate::search::embed_pending(&svc).await {
                tracing::warn!("tier2 background embedding failed: {e:#}");
            }
        });
    }

    /// One audit row per stage / cancel. Carries the claim id only, never the claim text
    /// (T-58.2-29).
    fn audit_forget(&self, kind: &str, claim_id: &str) {
        let home = self.paths.root.parent().unwrap_or(&self.paths.root);
        let cfg = ironhermes_core::audit::AuditConfig::default();
        let log = ironhermes_core::audit::AuditLog::with_path(
            home.join("logs").join("audit.jsonl"),
            cfg.clone(),
        );
        let entry = ironhermes_core::audit::AuditEntry::new(
            kind,
            "memory_forget",
            None,
            "",
            "tier2",
            "",
            "tier2 forget",
            "approved",
            "executed",
            &serde_json::json!({ "claim_id": claim_id }),
            &cfg,
        );
        if let Err(e) = log.append_sync(&entry) {
            tracing::warn!("tier2 audit write failed: {e}");
        }
    }

    /// One audit row per purged claim (T-58.2-80): claim id, text hash and surface, never the text.
    pub(crate) fn audit_purged(&self, claim_id: &str, text_hash: &str, surface: &str) {
        let home = self.paths.root.parent().unwrap_or(&self.paths.root);
        let cfg = ironhermes_core::audit::AuditConfig::default();
        let log = ironhermes_core::audit::AuditLog::with_path(
            home.join("logs").join("audit.jsonl"),
            cfg.clone(),
        );
        let entry = ironhermes_core::audit::AuditEntry::new(
            "memory_forget_purged",
            "memory_forget",
            None,
            "",
            "tier2",
            "",
            "tier2 forget purge",
            "approved",
            "executed",
            &serde_json::json!({ "claim_id": claim_id, "text_hash": text_hash, "surface": surface }),
            &cfg,
        );
        if let Err(e) = log.append_sync(&entry) {
            tracing::warn!("tier2 audit write failed: {e}");
        }
    }

    /// One audit row per promotion finish (D-30). Claim id and target only, never the text.
    fn audit_promotion(&self, kind: &str, claim_id: &str, target: &str) {
        let home = self.paths.root.parent().unwrap_or(&self.paths.root);
        let cfg = ironhermes_core::audit::AuditConfig::default();
        let log = ironhermes_core::audit::AuditLog::with_path(
            home.join("logs").join("audit.jsonl"),
            cfg.clone(),
        );
        let entry = ironhermes_core::audit::AuditEntry::new(
            kind,
            "memory_consolidate",
            None,
            "",
            "tier2",
            "",
            "tier2 promotion",
            "approved",
            "executed",
            &serde_json::json!({ "claim_id": claim_id, "target": target }),
            &cfg,
        );
        if let Err(e) = log.append_sync(&entry) {
            tracing::warn!("tier2 audit write failed: {e}");
        }
    }

    pub fn paths(&self) -> &Tier2Paths {
        &self.paths
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }
}

/// One Tier 1 entry: (target label, file name, 1-based ordinal, text).
fn snapshot(entries: &MemoryEntries) -> Vec<(&'static str, &'static str, i64, String)> {
    let mut out = Vec::new();
    for (target, label, file) in [
        (MemoryTarget::Memory, "memory", "MEMORY.md"),
        (MemoryTarget::User, "user", "USER.md"),
    ] {
        for (i, text) in entries
            .entries
            .get(&target)
            .into_iter()
            .flatten()
            .enumerate()
        {
            out.push((label, file, i as i64 + 1, text.clone()));
        }
    }
    out
}

#[async_trait::async_trait]
impl Tier2Handle for Tier2Service {
    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Tier2SearchResponse> {
        crate::search::search(self, query, limit).await
    }

    async fn sync_tier1(&self, entries: &MemoryEntries) -> anyhow::Result<Tier1SyncReport> {
        let snap = snapshot(entries);
        let store = Arc::clone(&self.store);
        let live_dir = self.paths.live.clone();
        let report = tokio::task::spawn_blocking(move || {
            let existing = store.tier1_claims()?;
            let mut report = Tier1SyncReport::default();
            // Keyed by (target, text_hash); a duplicate entry in one file indexes once.
            let mut live: HashSet<(&str, String)> = HashSet::new();
            // Claim ids matched to a current entry this pass.
            let mut kept: HashSet<i64> = HashSet::new();
            for (label, file, ordinal, text) in &snap {
                let hash = text_hash(text);
                if !live.insert((label, hash.clone())) {
                    continue;
                }
                let url = format!("memory://tier1/{file}#L{ordinal}");
                let matched = existing
                    .iter()
                    .find(|r| {
                        r.tier1_target.as_deref() == Some(label)
                            && r.text_hash == hash
                            && !kept.contains(&r.id)
                    })
                    // An entry moved from MEMORY.md to USER.md keeps one claim: a moved row
                    // (ordinal NULL) with the same text re-links whichever file it left.
                    .or_else(|| {
                        existing.iter().find(|r| {
                            r.tier1_ordinal.is_none()
                                && r.text_hash == hash
                                && !kept.contains(&r.id)
                        })
                    });
                match matched {
                    // Entries shift when an earlier one is removed; keep the citation accurate.
                    // A moved row re-links here too (a staged one stays staged, so stays hidden).
                    Some(row) => {
                        kept.insert(row.id);
                        if row.tier1_ordinal != Some(*ordinal)
                            || row.tier1_target.as_deref() != Some(label)
                        {
                            store.set_tier1_position(row.id, label, *ordinal, &url)?
                        }
                    }
                    None if store.is_tombstoned(&hash)? => report.skipped_tombstoned += 1,
                    None => {
                        let row = store.insert_claim(&NewClaim {
                            text: text.clone(),
                            kind: "world".into(),
                            subject: "user".into(),
                            origin: "tier1".into(),
                            confidence: 0.95,
                            source_url: Some(url.clone()),
                            tier1_target: Some((*label).to_string()),
                            tier1_ordinal: Some(*ordinal),
                            session_id: None,
                        })?;
                        store.insert_evidence(row.id, &url, text, None, None)?;
                        report.added += 1;
                    }
                }
            }
            // D-29: a line that left Tier 1 moves into today's live log; its claim keeps its id,
            // embedding and confidence. Only active rows move: a staged claim is never copied
            // into a new file (D-21), a superseded one keeps its supersession link.
            let departing: Vec<_> = existing
                .iter()
                .filter(|r| {
                    r.tier1_ordinal.is_some() && r.status == "active" && !kept.contains(&r.id)
                })
                .collect();
            if !departing.is_empty() {
                let date = chrono::Utc::now().date_naive();
                let lines: Vec<String> = departing
                    .iter()
                    .map(|r| live::claim_line(&r.claim_id, &r.text))
                    .collect();
                // shortcut: two concurrent syncs of one removal each append a line; the
                // conditional update re-points the claim once and the second line is an orphan.
                // Upgrade if duplicate log lines ever matter.
                let numbers = live::append_lines(&live_dir, date, &lines)?;
                let moves: Vec<(i64, String, String)> = departing
                    .iter()
                    .zip(numbers)
                    .zip(&lines)
                    .map(|((r, n), line)| {
                        let quote = line
                            .strip_prefix(&format!("- [{}] ", r.claim_id))
                            .unwrap_or(line)
                            .to_string();
                        (r.id, live::live_url(date, n), quote)
                    })
                    .collect();
                report.moved = store.move_tier1_to_live(&moves)?;
            }
            Ok::<_, anyhow::Error>(report)
        })
        .await??;
        // After the blocking write, so no Tier 1 lock is held across an embedding (T-58.2-49).
        if report.added > 0 {
            self.spawn_embed_pending();
        }
        Ok(report)
    }

    async fn explain(&self, claim_id: &str) -> anyhow::Result<Option<Tier2Explanation>> {
        let store = Arc::clone(&self.store);
        let claim_id = claim_id.to_string();
        let found = tokio::task::spawn_blocking(move || store.explain_row(&claim_id)).await??;
        Ok(found.map(|(c, evidence, supersedes)| Tier2Explanation {
            claim_id: c.claim_id,
            text: c.text,
            evidence: evidence
                .into_iter()
                .map(|e| Tier2Evidence {
                    source: e.source_url,
                    quote: e.quote,
                    timestamp: e.ts,
                })
                .collect(),
            supersedes,
            confidence: c.confidence,
        }))
    }

    async fn list_recent(&self, limit: usize) -> anyhow::Result<Vec<Tier2Hit>> {
        let store = Arc::clone(&self.store);
        let rows = tokio::task::spawn_blocking(move || store.list_recent(limit)).await??;
        Ok(rows.into_iter().map(crate::search::to_hit).collect())
    }

    async fn consolidate(&self) -> anyhow::Result<ConsolidationReport> {
        self.consolidate_now().await
    }

    async fn run_dreams(&self) -> anyhow::Result<DreamsReport> {
        crate::dreams::run(self).await
    }

    async fn synthesis_snapshot(&self, session_id: Option<&str>) -> Option<String> {
        if !self.cfg.dreams_enabled {
            return None;
        }
        if let Some(frozen) = self.synthesis.lookup(session_id) {
            return frozen.map(|b| b.to_string());
        }
        let path = self.paths.dreams.join("ALIGNMENT_SYNTHESIS.md");
        // A hand-edited giant file is skipped rather than read and scanned whole.
        let small = tokio::fs::metadata(&path)
            .await
            .is_ok_and(|m| m.len() <= MAX_SYNTHESIS_FILE_BYTES);
        let block = if small {
            let store = self.store.clone();
            // Fail closed: if the staged ids cannot be read, inject nothing this session.
            let staged: Option<Vec<String>> =
                tokio::task::spawn_blocking(move || store.staged_claims())
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .map(|rows| rows.into_iter().map(|r| r.claim_id).collect());
            match staged {
                Some(staged) => tokio::fs::read_to_string(&path).await.ok().and_then(|raw| {
                    build_block(&crate::synthesis::drop_lines_citing(&raw, &staged))
                }),
                None => None,
            }
        } else {
            None
        };
        self.synthesis
            .store(session_id, block.map(Arc::from))
            .map(|b| b.to_string())
    }

    async fn stage_forget(
        &self,
        claim_id: &str,
        reason: Option<&str>,
    ) -> anyhow::Result<ForgetOutcome> {
        let (store, paths, cfg) = (self.store.clone(), self.paths.clone(), self.cfg.clone());
        let (id, reason) = (claim_id.to_string(), reason.map(str::to_string));
        let out = tokio::task::spawn_blocking(move || {
            {
                let out = crate::forget::stage(&store, &paths, &cfg, &id, reason.as_deref())?;
                // WR-01: derived files must stop (or resume) showing this claim now, not at the
                // next consolidation that happens to change something.
                if let Err(e) = crate::render::render_all(&store, &paths) {
                    tracing::warn!("tier2: re-render after forget stage failed: {e:#}");
                }
                anyhow::Ok(out)
            }
        })
        .await??;
        self.audit_forget("memory_forget_staged", &out.claim_id);
        Ok(out)
    }

    async fn unstage_forget(&self, claim_id: &str) -> anyhow::Result<ForgetOutcome> {
        let (store, paths, cfg) = (self.store.clone(), self.paths.clone(), self.cfg.clone());
        let id = claim_id.to_string();
        let out = tokio::task::spawn_blocking(move || {
            let out = crate::forget::unstage(&store, &paths, &cfg, &id)?;
            // WR-01: derived files must stop (or resume) showing this claim now, not at the
            // next consolidation that happens to change something.
            if let Err(e) = crate::render::render_all(&store, &paths) {
                tracing::warn!("tier2: re-render after forget unstage failed: {e:#}");
            }
            anyhow::Ok(out)
        })
        .await??;
        self.audit_forget("memory_forget_unstaged", &out.claim_id);
        Ok(out)
    }

    async fn list_staged(&self) -> anyhow::Result<Vec<Tier2Hit>> {
        let (store, paths) = (self.store.clone(), self.paths.clone());
        let rows = tokio::task::spawn_blocking(move || crate::forget::list_staged(&store, &paths))
            .await??;
        Ok(rows.into_iter().map(crate::search::to_hit).collect())
    }

    async fn claim_promotions(&self, limit: usize) -> anyhow::Result<Vec<PromotionCandidate>> {
        if !(self.cfg.promotion_enabled && self.cfg.consolidation_enabled) {
            return Ok(Vec::new());
        }
        let store = Arc::clone(&self.store);
        let rows = tokio::task::spawn_blocking(move || store.claim_promotions(limit)).await??;
        Ok(rows
            .into_iter()
            .map(|(claim_id, text)| PromotionCandidate { claim_id, text })
            .collect())
    }

    async fn finish_promotion(
        &self,
        claim_id: &str,
        target: &str,
        outcome: PromotionOutcome,
    ) -> anyhow::Result<()> {
        // Only a Retry releases the mark: a refused fact, or a promoted line the user later
        // removes, is never offered again (D-30).
        let kind = match outcome {
            PromotionOutcome::Retry => {
                let (store, id) = (Arc::clone(&self.store), claim_id.to_string());
                tokio::task::spawn_blocking(move || store.release_promotion(&id)).await??;
                "memory_promotion_deferred"
            }
            PromotionOutcome::Promoted => "memory_promoted",
            PromotionOutcome::Rejected => "memory_promotion_rejected",
        };
        self.audit_promotion(kind, claim_id, target);
        Ok(())
    }

    async fn run_purge(
        &self,
        tier1: &dyn Tier1Remover,
        surface: PurgeSurface,
        only: &[String],
    ) -> anyhow::Result<PurgeReport> {
        crate::forget::purge(self, tier1, surface, only).await
    }

    async fn purge_preview(&self) -> anyhow::Result<PurgePreview> {
        let (store, paths, cfg) = (self.store.clone(), self.paths.clone(), self.cfg.clone());
        tokio::task::spawn_blocking(move || crate::forget::preview(&store, &paths, &cfg)).await?
    }

    async fn write_note(&self, text: &str) -> anyhow::Result<NoteOutcome> {
        let text = text.trim();
        if text.is_empty() {
            anyhow::bail!("'content' is empty");
        }
        if text.chars().count() > MAX_NOTE_CHARS {
            anyhow::bail!("'content' is too long (max {MAX_NOTE_CHARS} characters)");
        }
        // The scan and the tombstone check come before the append, so a refused note leaves no
        // line on disk; a failed DB write retracts the line it appended.
        if ironhermes_core::context_scanner::scan_context_content(text, "tier2-note")
            .contains("[BLOCKED:")
        {
            anyhow::bail!("note blocked: content contains potential prompt injection");
        }
        let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let (store, live_dir) = (Arc::clone(&self.store), self.paths.live.clone());
        let (outcome, inserted) = tokio::task::spawn_blocking(move || {
            let hash = text_hash(&normalized);
            if store.is_tombstoned(&hash)? {
                anyhow::bail!("this text was forgotten; not recorded");
            }
            let existing = store.active_by_hash(&hash, "user", "experience")?;
            let id = existing
                .as_ref()
                .map_or_else(new_claim_id, |(_, id)| id.clone());
            let date = chrono::Utc::now().date_naive();
            let n = live::append_lines(&live_dir, date, &[live::claim_line(&id, &normalized)])?;
            let url = live::live_url(date, n[0]);
            let written = match existing {
                Some((pk, _)) => store
                    .insert_evidence(pk, &url, &normalized, None, None)
                    .map(|_| false),
                None => store.insert_note(&id, &normalized, &url).map(|_| true),
            };
            let inserted = match written {
                Ok(v) => v,
                Err(e) => {
                    // WR-06: a line whose claim row is missing (a new note, or a claim purged
                    // meanwhile) could never be purged by id; take it back. A live claim keeps
                    // its lines, and its own purge retracts this one.
                    if store.claim_by_claim_id(&id).ok().flatten().is_none()
                        && let Err(r) =
                            live::retract_claim_lines(&live_dir, std::slice::from_ref(&id))
                    {
                        tracing::warn!("tier2 note: could not retract the orphan line: {r:#}");
                    }
                    return Err(e);
                }
            };
            Ok::<_, anyhow::Error>((
                NoteOutcome {
                    claim_id: id,
                    source: url,
                },
                inserted,
            ))
        })
        .await??;
        if inserted {
            self.spawn_embed_pending();
        }
        Ok(outcome)
    }
}
