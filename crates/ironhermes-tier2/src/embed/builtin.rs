//! Built-in fastembed (ONNX) embedder (D-05, D-26).
//!
//! The model downloads on first use into `<home>/tier2/models` in a background task, so a cold
//! download never blocks a turn: [`BuiltinEmbedder::readiness`] returns `Err` until it is loaded
//! (plan 14 turns that into the D-07 keyword degrade). A failed load is retried by that same
//! background task, from 5s up to 10 minutes, and init is serialised across processes by
//! `models/init.lock`. Inference runs in `spawn_blocking` (`TextEmbedding::embed` is `&mut self`
//! and CPU-bound).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use tokio::sync::OnceCell;

use super::EmbedError;

/// First background retry delay after a failed load; doubles per attempt up to [`RETRY_MAX`].
const RETRY_MIN: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(600);

const NOT_READY: &str = "embedding model is downloading/initialising";

/// Cross-process init lock file under the models dir.
const INIT_LOCK_FILE: &str = "init.lock";

fn retry_delay(attempt: u32) -> Duration {
    RETRY_MIN
        .saturating_mul(2u32.saturating_pow(attempt))
        .min(RETRY_MAX)
}

/// Map a `TIER2_BUILTIN_MODELS_384` name to its fastembed variant.
pub fn builtin_model_for(name: &str) -> Option<EmbeddingModel> {
    Some(match name {
        "all-MiniLM-L6-v2" => EmbeddingModel::AllMiniLML6V2,
        "all-MiniLM-L12-v2" => EmbeddingModel::AllMiniLML12V2,
        "bge-small-en-v1.5" => EmbeddingModel::BGESmallENV15,
        "paraphrase-multilingual-MiniLM-L12-v2" => EmbeddingModel::ParaphraseMLMiniLML12V2,
        "multilingual-e5-small" => EmbeddingModel::MultilingualE5Small,
        _ => return None,
    })
}

type Cell = OnceCell<Arc<Mutex<TextEmbedding>>>;

/// Owns the background load/retry loop; the loop must not outlive its embedder, so the last
/// `BuiltinEmbedder` clone dropping aborts it.
struct LoadTask(Mutex<Option<tokio::task::JoinHandle<()>>>);

impl Drop for LoadTask {
    fn drop(&mut self) {
        if let Some(h) = self.0.get_mut().unwrap_or_else(|p| p.into_inner()).take() {
            h.abort();
        }
    }
}

#[derive(Clone)]
pub struct BuiltinEmbedder {
    name: String,
    model: EmbeddingModel,
    cache_dir: PathBuf,
    cell: Arc<Cell>,
    task: Arc<LoadTask>,
    last_error: Arc<Mutex<Option<String>>>,
}

/// One load attempt: take the cross-process lock, build the model, record any failure.
async fn load(
    cell: &Cell,
    last_error: &Mutex<Option<String>>,
    model: &EmbeddingModel,
    cache_dir: &Path,
) -> Result<(), String> {
    let result = cell
        .get_or_try_init(|| async {
            let opts = TextInitOptions::new(model.clone())
                .with_cache_dir(cache_dir.to_path_buf())
                .with_show_download_progress(false);
            let lock_path = cache_dir.join(INIT_LOCK_FILE);
            let model = tokio::task::spawn_blocking(move || -> anyhow::Result<TextEmbedding> {
                // The lock spans `try_new`: hf-hub's own per-blob lock gives up after ~5s, so
                // a second process waits here instead and then loads from the finished cache.
                let _lock = anyhow::Context::with_context(
                    crate::paths::lock_exclusive(&lock_path),
                    || format!("locking {}", lock_path.display()),
                )?;
                Ok(TextEmbedding::try_new(opts)?)
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?;
            Ok::<_, String>(Arc::new(Mutex::new(model)))
        })
        .await
        .map(|_| ());
    if let Err(e) = &result {
        tracing::warn!("tier2 built-in embedder failed to initialise: {e}");
        *last_error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.clone());
    }
    result
}

impl BuiltinEmbedder {
    pub fn new(name: &str, model: EmbeddingModel, models_dir: &Path) -> Self {
        Self {
            name: name.to_string(),
            model,
            cache_dir: models_dir.to_path_buf(),
            cell: Arc::new(OnceCell::new()),
            task: Arc::new(LoadTask(Mutex::new(None))),
            last_error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Load (downloading if needed) the model and wait for it. Normal callers use
    /// [`readiness`](Self::readiness) and never wait; this exists for the manual real-model test.
    pub async fn init_blocking(&self) -> Result<(), EmbedError> {
        load(&self.cell, &self.last_error, &self.model, &self.cache_dir)
            .await
            .map_err(EmbedError::Unavailable)
    }

    /// Start the background load/retry loop if it is not already running.
    fn kick(&self) -> Result<(), String> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| "no async runtime to initialise the embedding model".to_string())?;
        let mut slot = self.task.0.lock().unwrap_or_else(|p| p.into_inner());
        // A finished handle with no loaded model means its runtime shut down mid-load, so a new
        // loop starts.
        if slot.as_ref().is_some_and(|h| !h.is_finished()) {
            return Ok(());
        }
        // Capture only the pieces the loop needs, never a `BuiltinEmbedder` or the task Arc, or
        // `LoadTask` would never drop.
        let (cell, last_error) = (self.cell.clone(), self.last_error.clone());
        let (model, cache_dir) = (self.model.clone(), self.cache_dir.clone());
        *slot = Some(handle.spawn(async move {
            let mut attempt = 0u32;
            while load(&cell, &last_error, &model, &cache_dir).await.is_err() {
                tokio::time::sleep(retry_delay(attempt)).await;
                attempt = attempt.saturating_add(1);
            }
        }));
        Ok(())
    }

    pub fn readiness(&self) -> Result<(), String> {
        if self.cell.initialized() {
            return Ok(());
        }
        self.kick()?;
        match &*self.last_error.lock().unwrap_or_else(|p| p.into_inner()) {
            Some(e) => Err(format!("embedding model failed to load: {e}")),
            None => Err(NOT_READY.to_string()),
        }
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.readiness().map_err(EmbedError::Unavailable)?;
        let model = self
            .cell
            .get()
            .cloned()
            .ok_or_else(|| EmbedError::Unavailable(NOT_READY.to_string()))?;
        let texts = texts.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut m = model.lock().unwrap_or_else(|p| p.into_inner());
            m.embed(texts, None)
                .map_err(|e| EmbedError::Transport(e.to_string()))
        })
        .await
        .map_err(|e| EmbedError::Transport(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_cache() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, b"").unwrap();
        (dir, file)
    }

    #[tokio::test]
    async fn init_failure_reports_error_chain() {
        let (_dir, file) = file_cache();
        let e = BuiltinEmbedder::new("all-MiniLM-L6-v2", EmbeddingModel::AllMiniLML6V2, &file);
        let err = e.init_blocking().await.unwrap_err().to_string();
        assert!(err.contains("not-a-dir"), "{err}");
        assert!(err.contains("os error"), "{err}");
    }

    #[test]
    fn retry_delay_backs_off_in_seconds_and_caps() {
        assert_eq!(retry_delay(0), Duration::from_secs(5));
        assert_eq!(retry_delay(1), Duration::from_secs(10));
        assert_eq!(retry_delay(2), Duration::from_secs(20));
        assert_eq!(retry_delay(7), Duration::from_secs(600));
        assert_eq!(retry_delay(u32::MAX), Duration::from_secs(600));
        let mut prev = Duration::ZERO;
        for a in 0..64 {
            let d = retry_delay(a);
            assert!(d >= prev && d <= Duration::from_secs(600));
            prev = d;
        }
    }

    #[tokio::test]
    async fn retry_loop_stops_when_embedder_dropped() {
        let (_dir, file) = file_cache();
        let e = BuiltinEmbedder::new("all-MiniLM-L6-v2", EmbeddingModel::AllMiniLML6V2, &file);
        let weak = Arc::downgrade(&e.cell);
        let _ = e.readiness();
        assert!(weak.strong_count() >= 2, "loop must hold the cell");
        drop(e);
        // tokio sleep, never std::thread::sleep: blocking would starve the aborted task's drop.
        for _ in 0..200 {
            if weak.strong_count() == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("retry loop outlived its embedder");
    }
}
