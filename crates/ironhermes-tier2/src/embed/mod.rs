//! Embedders for Tier 2 semantic search (D-04, D-05, D-06).
//!
//! Two real paths behind one enum: the built-in fastembed model (default-on cargo feature
//! `builtin-embedder`, D-05) and an OpenAI-compatible HTTP endpoint that overrides it when
//! `memory.embeddings.base_url` is set (D-04). Every vector that leaves [`Embedder::embed`] is
//! exactly [`TIER2_EMBEDDING_DIM`] long (D-06).

use std::path::Path;

use ironhermes_core::config::EmbeddingsConfig;
use ironhermes_core::tier2::TIER2_EMBEDDING_DIM;

#[cfg(feature = "builtin-embedder")]
pub mod builtin;
pub mod http;

/// Built-in model used when `memory.embeddings.model` is unset.
pub const DEFAULT_MODEL: &str = "all-MiniLM-L6-v2";

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("embedding dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },
    #[error("embedder unavailable: {0}")]
    Unavailable(String),
    #[error("embedding request failed: {0}")]
    Transport(String),
}

pub enum Embedder {
    #[cfg(feature = "builtin-embedder")]
    Builtin(builtin::BuiltinEmbedder),
    Http(http::HttpEmbedder),
    #[cfg(any(test, feature = "test-support"))]
    Fake(crate::test_support::FakeEmbedder),
    /// No embedder can be built; the string says why (plan 14 degrades search to keyword on it).
    None(String),
}

impl Embedder {
    /// `base_url` set selects the HTTP endpoint; otherwise the built-in model (when compiled in).
    pub fn from_config(cfg: &EmbeddingsConfig, models_dir: &Path) -> Embedder {
        let model = cfg
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        match cfg.base_url.as_deref().filter(|u| !u.is_empty()) {
            Some(url) => match http::HttpEmbedder::new(url, &model, cfg.api_key_env.clone()) {
                Ok(h) => Embedder::Http(h),
                Err(e) => Embedder::None(e),
            },
            None => Self::builtin(&model, models_dir),
        }
    }

    #[cfg(feature = "builtin-embedder")]
    fn builtin(model: &str, models_dir: &Path) -> Embedder {
        match builtin::builtin_model_for(model) {
            Some(m) => Embedder::Builtin(builtin::BuiltinEmbedder::new(model, m, models_dir)),
            None => Embedder::None(format!("unknown built-in embedding model {model:?}")),
        }
    }

    #[cfg(not(feature = "builtin-embedder"))]
    fn builtin(_model: &str, _models_dir: &Path) -> Embedder {
        Embedder::None("built-in embedder compiled out (feature builtin-embedder off)".to_string())
    }

    /// Identity stored beside every vector: `fastembed:<name>` or `http:<model>`.
    pub fn model_id(&self) -> Option<String> {
        match self {
            #[cfg(feature = "builtin-embedder")]
            Embedder::Builtin(b) => Some(format!("fastembed:{}", b.name())),
            Embedder::Http(h) => Some(h.model_id()),
            #[cfg(any(test, feature = "test-support"))]
            Embedder::Fake(f) => Some(f.model_id.clone()),
            Embedder::None(_) => None,
        }
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let out = match self {
            #[cfg(feature = "builtin-embedder")]
            Embedder::Builtin(b) => b.embed(texts).await?,
            Embedder::Http(h) => h.embed(texts).await?,
            #[cfg(any(test, feature = "test-support"))]
            Embedder::Fake(f) => f.embed(texts),
            Embedder::None(why) => return Err(EmbedError::Unavailable(why.clone())),
        };
        if out.len() != texts.len() {
            return Err(EmbedError::Transport(format!(
                "expected {} vectors, got {}",
                texts.len(),
                out.len()
            )));
        }
        if let Some(bad) = out.iter().find(|v| v.len() != TIER2_EMBEDDING_DIM) {
            return Err(EmbedError::DimensionMismatch {
                expected: TIER2_EMBEDDING_DIM,
                got: bad.len(),
            });
        }
        Ok(out)
    }

    /// Non-blocking: `Err` with a human reason while embedding cannot be served right now (model
    /// still downloading, endpoint failed within the last minute, nothing configured). Calling it
    /// starts the built-in model's background initialisation.
    pub fn readiness(&self) -> Result<(), String> {
        match self {
            #[cfg(feature = "builtin-embedder")]
            Embedder::Builtin(b) => b.readiness(),
            Embedder::Http(h) => h.readiness(),
            #[cfg(any(test, feature = "test-support"))]
            Embedder::Fake(_) => Ok(()),
            Embedder::None(why) => Err(why.clone()),
        }
    }
}
