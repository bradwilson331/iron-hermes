//! Deterministic embedder for tests (no model download, no network).

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use crate::llm::Tier2LlmHandle;

/// Scripted memory LLM: pops one queued response per call and logs every `(system, user)` prompt.
pub struct FakeLlm {
    script: Mutex<VecDeque<Result<String, String>>>,
    calls: Mutex<Vec<(String, String)>>,
}

impl FakeLlm {
    pub fn new(script: Vec<Result<String, String>>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn push(&self, response: Result<String, String>) {
        self.script.lock().unwrap().push_back(response);
    }

    pub fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Tier2LlmHandle for FakeLlm {
    async fn complete(
        &self,
        system: String,
        user: String,
        _max_tokens: u32,
    ) -> anyhow::Result<String> {
        self.calls.lock().unwrap().push((system, user));
        match self.script.lock().unwrap().pop_front() {
            Some(Ok(s)) => Ok(s),
            Some(Err(e)) => Err(anyhow::anyhow!(e)),
            None => Err(anyhow::anyhow!("FakeLlm script exhausted")),
        }
    }
}

/// Hashed bag-of-words into `dim` buckets, L2-normalised, so texts that share words have a high
/// cosine. `dim` is settable so tests can produce a wrong-length (e.g. 383) vector.
pub struct FakeEmbedder {
    pub model_id: String,
    pub dim: usize,
}

impl FakeEmbedder {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            dim: ironhermes_core::tier2::TIER2_EMBEDDING_DIM,
        }
    }

    pub fn embed(&self, texts: &[String]) -> Vec<Vec<f32>> {
        texts.iter().map(|t| self.embed_one(t)).collect()
    }

    fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; self.dim];
        for word in text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            let mut h = DefaultHasher::new();
            word.to_lowercase().hash(&mut h);
            v[(h.finish() % self.dim as u64) as usize] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_words_score_higher_than_disjoint() {
        let f = FakeEmbedder::new("fake:test");
        let v = f.embed(&[
            "user likes concise telegram replies".to_string(),
            "concise telegram replies please".to_string(),
            "completely unrelated gardening".to_string(),
        ]);
        let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        assert!(cos(&v[0], &v[1]) > cos(&v[0], &v[2]));
        assert_eq!(v[0].len(), 384);
    }
}
