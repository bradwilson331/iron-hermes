//! `memory_search` for Tier 2: semantic ranking when an embedder is ready, labelled FTS5 keyword
//! search otherwise (D-07), and the background embedding that keeps vectors current (D-06).
//!
//! Search never scores a query against a vector made by a different model: the candidate set is
//! the active claims embedded with the *current* `model_id`, and while rows from another model
//! remain the answer is keyword search until `embed_pending` has replaced them (RESEARCH Pitfall 4).

use std::path::Path;

use ironhermes_core::tier2::{Tier2Hit, Tier2SearchMode, Tier2SearchResponse};

use crate::index::IndexState;
use crate::service::Tier2Service;
use crate::store::{ClaimRow, Store};

/// Claims embedded per embedder call.
const BATCH: usize = 32;
/// `meta` key naming the model the stored embeddings are being brought to.
const MODEL_META_KEY: &str = "embedding_model_id";

pub(crate) fn to_hit(r: ClaimRow) -> Tier2Hit {
    Tier2Hit {
        claim_id: r.claim_id,
        text: r.text,
        source: r.source_url.unwrap_or_default(),
        confidence: r.confidence,
        created_at: r.created_at,
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

/// The in-memory index for `model`, rebuilt from SQLite when absent, built for another model, or
/// published over by another process.
pub(crate) fn ensure_index<'a>(
    slot: &'a mut Option<IndexState>,
    store: &Store,
    path: &Path,
    model: &str,
) -> anyhow::Result<&'a mut IndexState> {
    let mut state = match slot.take() {
        Some(s) if s.model_id() == model => s,
        _ => IndexState::load_or_rebuild(store, path, model)?,
    };
    state.ensure_fresh(store, path)?;
    Ok(slot.insert(state))
}

pub async fn search(
    svc: &Tier2Service,
    query: &str,
    limit: usize,
) -> anyhow::Result<Tier2SearchResponse> {
    match semantic(svc, query, limit).await {
        Ok(rows) => Ok(Tier2SearchResponse {
            results: rows.into_iter().map(to_hit).collect(),
            mode: Tier2SearchMode::Semantic,
            note: None,
        }),
        Err(reason) => {
            let store = svc.store.clone();
            let query = query.to_string();
            let rows =
                tokio::task::spawn_blocking(move || store.keyword_search(&query, limit)).await??;
            Ok(Tier2SearchResponse {
                results: rows.into_iter().map(to_hit).collect(),
                mode: Tier2SearchMode::Keyword,
                note: Some(format!(
                    "semantic search unavailable ({reason}); showing keyword matches"
                )),
            })
        }
    }
}

/// `Err(reason)` means "fall back to keyword search because of `reason`".
async fn semantic(svc: &Tier2Service, query: &str, limit: usize) -> Result<Vec<ClaimRow>, String> {
    svc.embedder.readiness()?;
    let model = svc
        .embedder
        .model_id()
        .ok_or_else(|| "no embedding model".to_string())?;

    let (store, m) = (svc.store.clone(), model.clone());
    let (pending, stale) = blocking(move || store.embedding_backlog(&m)).await?;
    if pending > 0 || stale > 0 {
        // Embeddings from a readiness change or a model switch are caught up in the background.
        if svc.embed_lock.try_lock().is_ok() {
            svc.spawn_embed_pending();
        }
    }
    if stale > 0 {
        return Err(format!("re-embedding stored claims with {model}"));
    }

    let q = svc
        .embedder
        .embed(&[query.to_string()])
        .await
        .map_err(|e| e.to_string())?
        .pop()
        .ok_or_else(|| "embedder returned no vector".to_string())?;

    let guard = svc.index.clone().lock_owned().await;
    let (store, path, m) = (svc.store.clone(), svc.paths.index.clone(), model);
    let rows = blocking(move || {
        let mut slot = guard;
        let state = ensure_index(&mut slot, &store, &path, &m)?;
        let allow = store.active_embedded_pks(&m)?;
        let hits = state.search(&q, limit.saturating_mul(3), &|k| allow.contains(&k))?;
        let mut rows = Vec::new();
        for (pk, _) in hits {
            if rows.len() == limit {
                break;
            }
            rows.extend(store.claim_by_pk(pk as i64)?);
        }
        Ok(rows)
    })
    .await?;
    if rows.is_empty() && pending > 0 {
        return Err(format!("{pending} claims are still being embedded"));
    }
    Ok(rows)
}

/// Embed every active claim that has no embedding for the current model (missing, or made by
/// another model), in batches, and publish them to the index. Returns how many were embedded.
/// A change of model replaces every row and rebuilds the index for the new model (D-06).
pub async fn embed_pending(svc: &Tier2Service) -> anyhow::Result<usize> {
    let _run = svc.embed_lock.lock().await;
    let Some(model) = svc.embedder.model_id() else {
        return Ok(0);
    };
    // Not ready (model downloading, endpoint failing): leave the backlog for a later run.
    if svc.embedder.readiness().is_err() {
        return Ok(0);
    }
    let (store, m) = (svc.store.clone(), model.clone());
    tokio::task::spawn_blocking(move || store.meta_set(MODEL_META_KEY, &m)).await??;

    let mut done = 0;
    loop {
        let (store, m) = (svc.store.clone(), model.clone());
        let batch = tokio::task::spawn_blocking(move || store.claims_needing_embedding(&m, BATCH))
            .await??;
        if batch.is_empty() {
            return Ok(done);
        }
        let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
        let vectors = svc.embedder.embed(&texts).await?;

        let guard = svc.index.clone().lock_owned().await;
        let (store, path, m) = (svc.store.clone(), svc.paths.index.clone(), model.clone());
        let n = batch.len();
        tokio::task::spawn_blocking(move || {
            let mut slot = guard;
            for ((pk, _), v) in batch.iter().zip(&vectors) {
                store.upsert_embedding(*pk, &m, v)?;
            }
            let state = ensure_index(&mut slot, &store, &path, &m)?;
            for ((pk, _), v) in batch.iter().zip(&vectors) {
                state.add(*pk as u64, v)?;
            }
            state.commit_added(&store, &path)
        })
        .await??;
        done += n;
    }
}
