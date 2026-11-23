//! Vector index for Tier 2 semantic search (D-14): USearch HNSW, cosine, f32, 384-dim, one file
//! (`<home>/tier2/index.usearch`). The file is a derived cache: the `embeddings` table in SQLite is
//! the source of truth, so a corrupt or stale file is rebuilt, never migrated.
//!
//! [`BruteForceIndex`] is the exact twin: the test oracle and the runtime fallback if USearch
//! cannot construct an index.

use std::path::{Path, PathBuf};

use anyhow::Context;
use ironhermes_core::tier2::TIER2_EMBEDDING_DIM;
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

use crate::store::Store;

fn check_dim(v: &[f32]) -> anyhow::Result<()> {
    anyhow::ensure!(
        v.len() == TIER2_EMBEDDING_DIM,
        "vector has {} dimensions, expected {TIER2_EMBEDDING_DIM}",
        v.len()
    );
    Ok(())
}

/// A set of `(key, vector)` pairs searchable by cosine similarity.
pub trait VectorIndex: Send + Sync {
    /// Insert, replacing any vector already stored under `key`.
    fn add(&mut self, key: u64, v: &[f32]) -> anyhow::Result<()>;
    /// Up to `k` `(key, similarity)` pairs, best first, restricted to keys `allow` accepts.
    /// Similarity is `1 - cosine distance`.
    fn search(
        &self,
        q: &[f32],
        k: usize,
        allow: &dyn Fn(u64) -> bool,
    ) -> anyhow::Result<Vec<(u64, f32)>>;
    fn size(&self) -> usize;
    /// Write `<path>.tmp` then rename it over `path`.
    fn save(&self, path: &Path) -> anyhow::Result<()>;
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

pub struct UsearchIndex {
    inner: Index,
}

impl UsearchIndex {
    fn options() -> IndexOptions {
        IndexOptions {
            dimensions: TIER2_EMBEDDING_DIM,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F32,
            // Wider than USearch's defaults (16/128/64): the claim store is small, so the extra
            // build/query cost is negligible and recall stays >= 0.95 even on unstructured vectors.
            connectivity: 32,
            expansion_add: 200,
            expansion_search: 128,
            ..Default::default()
        }
    }

    pub fn new() -> anyhow::Result<Self> {
        let inner = Index::new(&Self::options())
            .map_err(|e| anyhow::anyhow!("usearch index construction failed: {e}"))?;
        Ok(Self { inner })
    }

    /// Load a previously [`save`](VectorIndex::save)d index.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let me = Self::new()?;
        let p = path.to_str().context("index path is not valid UTF-8")?;
        me.inner
            .load(p)
            .map_err(|e| anyhow::anyhow!("loading {}: {e}", path.display()))?;
        Ok(me)
    }
}

impl VectorIndex for UsearchIndex {
    fn add(&mut self, key: u64, v: &[f32]) -> anyhow::Result<()> {
        check_dim(v)?;
        if self.inner.contains(key) {
            self.inner
                .remove(key)
                .map_err(|e| anyhow::anyhow!("usearch remove: {e}"))?;
        }
        // USearch requires reserve() before add() once capacity is reached (RESEARCH Pitfall 10).
        if self.inner.size() >= self.inner.capacity() {
            self.inner
                .reserve((self.inner.size() * 2).max(64))
                .map_err(|e| anyhow::anyhow!("usearch reserve: {e}"))?;
        }
        self.inner
            .add(key, v)
            .map_err(|e| anyhow::anyhow!("usearch add: {e}"))
    }

    fn search(
        &self,
        q: &[f32],
        k: usize,
        allow: &dyn Fn(u64) -> bool,
    ) -> anyhow::Result<Vec<(u64, f32)>> {
        check_dim(q)?;
        if k == 0 || self.inner.size() == 0 {
            return Ok(Vec::new());
        }
        let m = self
            .inner
            .filtered_search(q, k, allow)
            .map_err(|e| anyhow::anyhow!("usearch search: {e}"))?;
        Ok(m.keys
            .into_iter()
            .zip(m.distances.into_iter().map(|d| 1.0 - d))
            .collect())
    }

    fn size(&self) -> usize {
        self.inner.size()
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        let tmp = tmp_path(path);
        let t = tmp.to_str().context("index path is not valid UTF-8")?;
        self.inner
            .save(t)
            .map_err(|e| anyhow::anyhow!("saving {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))
    }
}

/// Exact cosine search over a plain vector list.
#[derive(Default)]
pub struct BruteForceIndex {
    rows: Vec<(u64, Vec<f32>)>,
}

impl BruteForceIndex {
    pub fn new() -> Self {
        Self::default()
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let denom = (dot(a, a) * dot(b, b)).sqrt();
    if denom == 0.0 { 0.0 } else { dot(a, b) / denom }
}

impl VectorIndex for BruteForceIndex {
    fn add(&mut self, key: u64, v: &[f32]) -> anyhow::Result<()> {
        check_dim(v)?;
        match self.rows.iter_mut().find(|(k, _)| *k == key) {
            Some(row) => row.1 = v.to_vec(),
            None => self.rows.push((key, v.to_vec())),
        }
        Ok(())
    }

    fn search(
        &self,
        q: &[f32],
        k: usize,
        allow: &dyn Fn(u64) -> bool,
    ) -> anyhow::Result<Vec<(u64, f32)>> {
        check_dim(q)?;
        let mut hits: Vec<(u64, f32)> = self
            .rows
            .iter()
            .filter(|(key, _)| allow(*key))
            .map(|(key, v)| (*key, cosine(q, v)))
            .collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1));
        hits.truncate(k);
        Ok(hits)
    }

    fn size(&self) -> usize {
        self.rows.len()
    }

    /// Not persisted: the fallback index is rebuilt from SQLite on every load.
    fn save(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }
}

fn new_empty() -> Box<dyn VectorIndex> {
    match UsearchIndex::new() {
        Ok(i) => Box::new(i),
        Err(e) => {
            tracing::warn!("{e:#}; falling back to the brute-force vector index");
            Box::new(BruteForceIndex::new())
        }
    }
}

fn build(rows: Vec<(i64, Vec<f32>)>) -> anyhow::Result<Box<dyn VectorIndex>> {
    let mut index = new_empty();
    for (pk, v) in rows {
        index.add(pk as u64, &v)?;
    }
    Ok(index)
}

/// The in-memory index of one process plus the file generation it reflects.
pub struct IndexState {
    index: Box<dyn VectorIndex>,
    generation: u64,
    model_id: String,
}

impl IndexState {
    /// Rebuild purely from the `embeddings` rows of `model_id` (nothing is written to disk).
    pub fn rebuild_from_store(store: &Store, model_id: &str) -> anyhow::Result<Self> {
        let generation = store.index_generation()?;
        Ok(Self {
            index: build(store.embeddings_for_model(model_id)?)?,
            generation,
            model_id: model_id.to_string(),
        })
    }

    /// Load `path`; when it is missing, unreadable, or holds a different number of vectors than
    /// `embeddings` has for `model_id`, rebuild from SQLite and rewrite the file.
    pub fn load_or_rebuild(store: &Store, path: &Path, model_id: &str) -> anyhow::Result<Self> {
        // Read the generation first: a writer publishing after this point makes us reload later.
        let generation = store.index_generation()?;
        let want = store.count_embeddings(model_id)? as usize;
        match UsearchIndex::load(path) {
            Ok(i) if i.size() == want => {
                return Ok(Self {
                    index: Box::new(i),
                    generation,
                    model_id: model_id.to_string(),
                });
            }
            Ok(i) => tracing::warn!(
                "tier2 index has {} vectors but SQLite has {want}; rebuilding",
                i.size()
            ),
            Err(e) if path.exists() => tracing::warn!("tier2 index unreadable ({e:#}); rebuilding"),
            Err(_) => {}
        }
        let mut me = Self::rebuild_from_store(store, model_id)?;
        me.save(store, path)?;
        Ok(me)
    }

    /// Reload from `path` if another process published a newer generation. Returns whether it did.
    pub fn ensure_fresh(&mut self, store: &Store, path: &Path) -> anyhow::Result<bool> {
        if store.index_generation()? == self.generation {
            return Ok(false);
        }
        *self = Self::load_or_rebuild(store, path, &self.model_id.clone())?;
        Ok(true)
    }

    pub fn add(&mut self, key: u64, v: &[f32]) -> anyhow::Result<()> {
        self.index.add(key, v)
    }

    /// Publish vectors added since the last save: rewrite the file and bump the generation under
    /// one `BEGIN IMMEDIATE`. If another process published first, our in-memory index lacks its
    /// vectors, so it is rebuilt from SQLite (which holds every committed embedding) before saving.
    pub fn commit_added(&mut self, store: &Store, path: &Path) -> anyhow::Result<()> {
        self.save(store, path)
    }

    fn save(&mut self, store: &Store, path: &Path) -> anyhow::Result<()> {
        let mine = self.generation;
        let model = self.model_id.clone();
        let index = &mut self.index;
        self.generation = store.commit_index_write(|cur, conn| {
            if cur != mine {
                *index = build(crate::store::embeddings_on(conn, &model)?)?;
            }
            index.save(path)
        })?;
        Ok(())
    }

    pub fn search(
        &self,
        q: &[f32],
        k: usize,
        allow: &dyn Fn(u64) -> bool,
    ) -> anyhow::Result<Vec<(u64, f32)>> {
        self.index.search(q, k, allow)
    }

    pub fn size(&self) -> usize {
        self.index.size()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }
}
