//! Agent-side Tier 2 maintenance that needs Tier 1: promotion (D-30) and the human-run purge
//! helpers (D-28/D-32). The purge helpers are the only callers of `run_purge` / `purge_preview`
//! outside the Tier 2 crate; no agent tool and no scheduled job may reach them.

use std::sync::Arc;

use ironhermes_core::memory_store::{ENTRY_DELIMITER, MemoryTarget, char_count};
use ironhermes_core::tier2::{
    PromotionOutcome, PurgePreview, PurgeReport, PurgeSurface, Tier1Remover,
};
use tokio::sync::Mutex;

use super::MemoryManager;

/// D-30: at most this many facts are promoted per run.
pub const PROMOTE_MAX_PER_RUN: usize = 3;
/// D-30: a target takes a promoted fact only while it stays at or below this share of its limit.
pub const PROMOTE_FILL_LIMIT_PCT: usize = 80;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PromoteReport {
    pub promoted: usize,
    pub deferred: usize,
    pub rejected: usize,
}

/// D-30: write the durable user facts Tier 2 hands out into USER.md (then MEMORY.md) through
/// `MemoryManager::add`, so the scanner, dedup, char limit and file lock all apply. `add`'s own
/// `sync_tier2` links the line to its claim. No-op unless Tier 2 has promotion enabled (it then
/// returns no candidates).
pub async fn promote_tier2(manager: &Arc<Mutex<MemoryManager>>) -> anyhow::Result<PromoteReport> {
    let Some(t2) = manager.lock().await.tier2() else {
        return Ok(PromoteReport::default());
    };
    let mut report = PromoteReport::default();
    for cand in t2.claim_promotions(PROMOTE_MAX_PER_RUN).await? {
        let mut outcome = PromotionOutcome::Retry;
        let mut last = MemoryTarget::User;
        for target in [MemoryTarget::User, MemoryTarget::Memory] {
            last = target;
            let entries = {
                let mgr = manager.lock().await;
                mgr.to_memory_entries()
                    .await
                    .entries
                    .remove(&target)
                    .unwrap_or_default()
            };
            let mut after = entries;
            after.push(cand.text.clone());
            if char_count(&after, ENTRY_DELIMITER)
                > target.char_limit() * PROMOTE_FILL_LIMIT_PCT / 100
            {
                continue;
            }
            // The manager guard is held for one `add` only, like the `memory` tool; no Tier 2 lock
            // is held across it.
            let res = manager.lock().await.add(target, &cand.text).await;
            match res {
                Ok(_) => {
                    outcome = PromotionOutcome::Promoted;
                    break;
                }
                Err(e) => match add_refusal(&e) {
                    None => continue,
                    Some(o) => {
                        outcome = o;
                        break;
                    }
                },
            }
        }
        if let Err(e) = t2
            .finish_promotion(&cand.claim_id, last.label(), outcome)
            .await
        {
            tracing::warn!(claim = %cand.claim_id, error = %e, "finish_promotion failed");
        }
        match outcome {
            PromotionOutcome::Promoted => report.promoted += 1,
            PromotionOutcome::Retry => report.deferred += 1,
            PromotionOutcome::Rejected => report.rejected += 1,
        }
    }
    Ok(report)
}

/// The `error` code of a Tier 1 refusal (`{"error": "<code>", ..}`). Never a substring match: the
/// JSON also echoes the candidate text and existing entries (WR-01).
fn error_code(e: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(e)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
}

/// How a failed promotion `add` ends: `None` tries the next target (full, or USER.md disabled).
/// Only a scanner block or a duplicate is a permanent refusal; anything else (I/O, lock) is offered
/// again next run (WR-02).
fn add_refusal(e: &str) -> Option<PromotionOutcome> {
    match error_code(e).as_deref() {
        Some("blocked" | "duplicate") => Some(PromotionOutcome::Rejected),
        Some("capacity_exceeded") => None,
        _ if e.starts_with("User profile memory is disabled") => None,
        _ => Some(PromotionOutcome::Retry),
    }
}

/// D-22: removes one exact MEMORY.md / USER.md entry through the manager, so the remove still
/// takes the file lock and `remove_at`'s expected-text guard. Never a substring match.
pub struct ManagerTier1Remover {
    manager: Arc<Mutex<MemoryManager>>,
}

impl ManagerTier1Remover {
    pub fn new(manager: Arc<Mutex<MemoryManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tier1Remover for ManagerTier1Remover {
    async fn remove_tier1(&self, target: MemoryTarget, text: &str) -> anyhow::Result<bool> {
        let want = text.trim();
        let mut last = String::new();
        // A second pass covers an entry edited by hand since the snapshot: `remove_at` reloads the
        // file before it refuses, so the retry sees the live entries (and finds none: already gone).
        for _ in 0..2 {
            // The guard is held for one `remove_at` only, like the `memory` tool.
            let mgr = self.manager.lock().await;
            let entries = mgr
                .to_memory_entries()
                .await
                .entries
                .remove(&target)
                .unwrap_or_default();
            let Some((idx, entry)) = entries.iter().enumerate().find(|(_, e)| e.trim() == want)
            else {
                return Ok(false);
            };
            match mgr.remove_at(target, idx, entry).await {
                Ok(_) => return Ok(true),
                Err(e)
                    if matches!(
                        error_code(&e).as_deref(),
                        Some("entry_changed" | "index_out_of_range")
                    ) =>
                {
                    last = e;
                }
                Err(e) => anyhow::bail!("Tier 1 removal failed: {e}"),
            }
        }
        anyhow::bail!("Tier 1 entry kept changing during the purge: {last}")
    }
}

const TIER2_DISABLED: &str = "Tier 2 disabled";

/// D-28: the dry run. Read-only. The manager guard is dropped before the await.
pub async fn preview_tier2(manager: &Arc<Mutex<MemoryManager>>) -> anyhow::Result<PurgePreview> {
    let Some(t2) = manager.lock().await.tier2() else {
        return Ok(PurgePreview {
            skipped: Some(TIER2_DISABLED.into()),
            ..Default::default()
        });
    };
    t2.purge_preview().await
}

/// D-28/D-32: permanently delete the staged claims in `only` and their Tier 1 entries. Human-run
/// only (CLI, web UI). The guard is dropped before `run_purge`, which re-enters the manager through
/// the remover; holding it would deadlock.
pub async fn purge_tier2(
    manager: &Arc<Mutex<MemoryManager>>,
    surface: PurgeSurface,
    only: &[String],
) -> anyhow::Result<PurgeReport> {
    let Some(t2) = manager.lock().await.tier2() else {
        return Ok(PurgeReport {
            skipped: Some(TIER2_DISABLED.into()),
            ..Default::default()
        });
    };
    let remover = ManagerTier1Remover::new(Arc::clone(manager));
    t2.run_purge(&remover, surface, only).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_refusal_matches_the_error_code_only() {
        let j = |v: serde_json::Value| v.to_string();
        // WR-01: echoed text naming another code must not change the class.
        let blocked = j(
            serde_json::json!({"error": "blocked", "details": "User disabled capacity_exceeded"}),
        );
        assert_eq!(add_refusal(&blocked), Some(PromotionOutcome::Rejected));
        let dup = j(serde_json::json!({"error": "duplicate", "content": "disabled"}));
        assert_eq!(add_refusal(&dup), Some(PromotionOutcome::Rejected));
        let full =
            j(serde_json::json!({"error": "capacity_exceeded", "entries": ["blocked duplicate"]}));
        assert_eq!(add_refusal(&full), None);
        assert_eq!(
            add_refusal("User profile memory is disabled via configuration. Enable it."),
            None
        );
        // WR-02: a transient write failure is retried, never a permanent rejection.
        assert_eq!(
            add_refusal(r#"{"error": "Failed to write: disk full"}"#),
            Some(PromotionOutcome::Retry)
        );
        assert_eq!(
            add_refusal(r#"{"error": "Failed to write: "quoted" path"}"#),
            Some(PromotionOutcome::Retry)
        );
    }
}
