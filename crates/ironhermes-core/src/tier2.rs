//! Tier 2 long-term memory contract (ADR-0008, phase 58.2 D-10).
//!
//! Tier 2 is a standalone subsystem beside Tier 1 (`MEMORY.md` / `USER.md`), never the
//! `mirror_provider` slot. The trait lives here and the implementation in `ironhermes-tier2`,
//! the same dependency inversion as [`crate::provider::SummarizationClientHandle`]: consumers
//! (`ironhermes-agent`'s `MemoryManager`, `ironhermes-tools`' `memory_search`) hold an
//! `Arc<dyn Tier2Handle>` without depending on the SQLite implementation.

use serde::{Deserialize, Serialize};

/// Directory under the profile home that holds every Tier 2 file (D-11).
pub const TIER2_DIRNAME: &str = "tier2";

/// Embedding dimension fixed by D-06: stored vectors and the index depend on it.
pub const TIER2_EMBEDDING_DIM: usize = 384;

/// Built-in embedding model names accepted when no `memory.embeddings.base_url` is set.
///
/// D-06: every entry emits 384-dim vectors; plan 02's `builtin_model_for` must map each name
/// and its test fails on any name without a 384-dim fastembed variant.
pub const TIER2_BUILTIN_MODELS_384: &[&str] = &[
    "all-MiniLM-L6-v2", // default
    "all-MiniLM-L12-v2",
    "bge-small-en-v1.5",
    "paraphrase-multilingual-MiniLM-L12-v2",
    "multilingual-e5-small",
];

/// How a search was served.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tier2SearchMode {
    Semantic,
    Keyword,
}

/// One search hit: a claim and where it came from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2Hit {
    pub claim_id: String,
    pub text: String,
    pub source: String,
    pub confidence: f64,
    pub created_at: String,
}

/// Result of [`Tier2Handle::search`]. `note` is set when the mode is a degraded fallback (D-07).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2SearchResponse {
    pub results: Vec<Tier2Hit>,
    pub mode: Tier2SearchMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// D-30: a claim handed out for promotion into Tier 1: its id and the text to add.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PromotionCandidate {
    pub claim_id: String,
    pub text: String,
}

/// D-30: how the Tier 1 add for a promotion candidate ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromotionOutcome {
    /// The line was written to Tier 1.
    Promoted,
    /// Tier 1 had no room this time; the claim is offered again on a later run.
    Retry,
    /// Tier 1 refused it permanently (blocked or duplicate); never offered again.
    Rejected,
}

/// What a Tier 1 -> Tier 2 sync changed.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tier1SyncReport {
    pub added: usize,
    /// Tier 1 lines that left Tier 1 since the last sync; their claims moved to the daily live
    /// log (D-29).
    pub moved: usize,
    pub skipped_tombstoned: usize,
}

/// One evidence row backing a claim (D-12 / D-18: the citation `memory_explain` surfaces).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2Evidence {
    pub source: String,
    pub quote: String,
    pub timestamp: String,
}

/// ADR-0008 `memory_explain` result: where a claim came from and what it replaced.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2Explanation {
    pub claim_id: String,
    pub text: String,
    pub evidence: Vec<Tier2Evidence>,
    pub supersedes: Vec<String>,
    pub confidence: f64,
}

/// D-19: cap on the injected `ALIGNMENT_SYNTHESIS.md` body, in chars (the header is extra). It
/// sits beside the 3,575-char Tier 1 budget and counts against the cache-stable Memory slot.
pub const TIER2_SYNTHESIS_CHAR_CAP: usize = 1500;

/// What one consolidation run did (D-15). `skipped` carries the reason when nothing ran.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ConsolidationReport {
    pub messages_read: usize,
    pub claims_added: usize,
    pub claims_merged: usize,
    pub claims_superseded: usize,
    pub claims_rejected: usize,
    pub llm_calls: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// What one nightly dreams run did (ADR-0008 Dreams contract). `skipped` carries the reason when
/// nothing ran.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct DreamsReport {
    pub claims_reviewed: usize,
    pub items_written: usize,
    pub items_rejected: usize,
    pub llm_calls: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// What staging or cancelling a forget did (D-21, D-23). `status` is `"staged"` or `"cancelled"`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ForgetOutcome {
    pub claim_id: String,
    pub status: String,
    pub note: String,
}

/// What writing a daily note did (D-31): the claim it created (or reused) and the live-log line
/// that cites it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NoteOutcome {
    pub claim_id: String,
    pub source: String,
}

/// True for exactly `claim:` followed by 12 lowercase hex digits.
pub fn is_valid_claim_id(s: &str) -> bool {
    s.strip_prefix("claim:").is_some_and(|hex| {
        hex.len() == 12 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// D-22: removes one exact Tier 1 entry during a purge. Implemented in `ironhermes-agent` over the
/// memory manager. `Ok(true)` means an entry was removed, `Ok(false)` that none matched exactly (an
/// entry already gone is not an error, and there is never a substring fallback).
#[async_trait::async_trait]
pub trait Tier1Remover: Send + Sync {
    async fn remove_tier1(
        &self,
        target: crate::memory_store::MemoryTarget,
        text: &str,
    ) -> anyhow::Result<bool>;
}

/// D-28: which human entry point started a purge (recorded in the audit row).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PurgeSurface {
    Cli,
    Ui,
}

/// What one purge did (D-20, D-32). `skipped` carries the reason when nothing ran.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct PurgeReport {
    pub purged: Vec<String>,
    pub tier1_lines_removed: usize,
    pub live_lines_retracted: usize,
    pub evidence_redacted: usize,
    /// Ids that were asked for but are not staged (never purged).
    pub not_staged: Vec<String>,
    pub skipped: Option<String>,
    pub scrub_incomplete: Option<String>,
}

/// D-28: one staged claim in a purge preview and everything its purge would delete.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PurgePreviewClaim {
    pub claim_id: String,
    pub text: String,
    /// Superseded predecessors that go with it.
    pub predecessors: Vec<String>,
    /// `(target label, exact text)` of each linked MEMORY.md / USER.md entry.
    pub tier1_entries: Vec<(String, String)>,
    /// Live-log lines citing the claim (a count only, D-34).
    pub live_lines: usize,
}

/// D-28: the dry run. Read-only: the exact list a purge of these ids deletes.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct PurgePreview {
    pub claims: Vec<PurgePreviewClaim>,
    /// Other claims' evidence quotes that share a source message and would become `[retracted]`.
    pub redacted_quotes: usize,
    pub skipped: Option<String>,
    pub scrub_pending: bool,
}

/// Handle on the Tier 2 service. Later plans append methods.
#[async_trait::async_trait]
pub trait Tier2Handle: Send + Sync {
    /// Search long-term memory (semantic when an embedder is available, keyword otherwise).
    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Tier2SearchResponse>;

    /// Reconcile the Tier 2 index of Tier 1 entries with the live Tier 1 snapshot (D-03 as amended
    /// by D-29: index Tier 1; a line that leaves Tier 1 moves into the daily live log).
    async fn sync_tier1(&self, entries: &crate::MemoryEntries) -> anyhow::Result<Tier1SyncReport>;

    /// Evidence and supersession links of an active claim; `None` for an unknown id or one that
    /// is not active (superseded or staged for forgetting).
    async fn explain(&self, claim_id: &str) -> anyhow::Result<Option<Tier2Explanation>>;

    /// Active claims, newest first.
    async fn list_recent(&self, limit: usize) -> anyhow::Result<Vec<Tier2Hit>>;

    /// One consolidation run now (D-15): extract claims from new conversation turns.
    async fn consolidate(&self) -> anyhow::Result<ConsolidationReport>;

    /// One nightly dreams run now (ADR-0008 Stage 3): cited review, synthesis and companion files.
    async fn run_dreams(&self) -> anyhow::Result<DreamsReport>;

    /// D-19: the labelled, capped, scanned `ALIGNMENT_SYNTHESIS.md` block for a new session's
    /// system prompt, or `None`. Frozen per session id: the first call for an id decides what that
    /// session sees (a missing file included), so a nightly rewrite never changes a running
    /// session and its prompt cache stays valid. `None` for the id shares one per-process snapshot.
    async fn synthesis_snapshot(&self, session_id: Option<&str>) -> Option<String>;

    /// D-21: stage a claim for forgetting (purge deferred, D-27). It is hidden from
    /// search, explain and list_recent at once; nothing is deleted. Needs `memory.forgetting_enabled`.
    async fn stage_forget(
        &self,
        claim_id: &str,
        reason: Option<&str>,
    ) -> anyhow::Result<ForgetOutcome>;

    /// D-23: cancel a staged forget before the purge, restoring the claim's prior status.
    async fn unstage_forget(&self, claim_id: &str) -> anyhow::Result<ForgetOutcome>;

    /// Claims currently staged for forgetting, newest first (the UI's undo list).
    async fn list_staged(&self) -> anyhow::Result<Vec<Tier2Hit>>;

    /// D-31: append a day-to-day note to today's live log and index it as a claim. It is never
    /// part of the prompt; `search` finds it. The default refuses so test doubles need no body.
    async fn write_note(&self, _text: &str) -> anyhow::Result<NoteOutcome> {
        anyhow::bail!("daily notes are not supported by this Tier 2 handle")
    }

    /// D-30: atomically claim up to `limit` durable, direct user facts for promotion into Tier 1.
    /// Each is handed out at most once. Empty unless `memory.promotion_enabled` and
    /// `consolidation_enabled`.
    async fn claim_promotions(&self, _limit: usize) -> anyhow::Result<Vec<PromotionCandidate>> {
        Ok(Vec::new())
    }

    /// D-30: record how the Tier 1 add for a claimed candidate ended. `Retry` releases the claim;
    /// the other outcomes keep it marked. Writes one audit row.
    async fn finish_promotion(
        &self,
        _claim_id: &str,
        _target: &str,
        _outcome: PromotionOutcome,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// D-28/D-32: permanently delete the staged claims listed in `only` (ids from a preview),
    /// their superseded predecessors, evidence and vectors, and their Tier 1 lines. Human-run
    /// only; no agent tool may call it. The default refuses so test doubles need no body.
    async fn run_purge(
        &self,
        _tier1: &dyn Tier1Remover,
        _surface: PurgeSurface,
        _only: &[String],
    ) -> anyhow::Result<PurgeReport> {
        anyhow::bail!("purge is not supported by this Tier 2 handle")
    }

    /// D-28: the dry run for [`Self::run_purge`]; writes nothing.
    async fn purge_preview(&self) -> anyhow::Result<PurgePreview> {
        anyhow::bail!("purge is not supported by this Tier 2 handle")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_id_shape() {
        assert!(is_valid_claim_id("claim:0123456789ab"));
        for bad in [
            "",
            "claim:",
            "claim:0123456789a",
            "claim:0123456789abc",
            "claim:0123456789AB",
            "claim:0123456789ag",
            "claim 0123456789ab",
            "Claim:0123456789ab",
        ] {
            assert!(!is_valid_claim_id(bad), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn response_omits_absent_note_and_lowercases_mode() {
        let r = Tier2SearchResponse {
            results: vec![],
            mode: Tier2SearchMode::Semantic,
            note: None,
        };
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"results":[],"mode":"semantic"}"#
        );
    }
}
