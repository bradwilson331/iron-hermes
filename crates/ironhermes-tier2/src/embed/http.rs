//! OpenAI-compatible `POST <base>/v1/embeddings` embedder (D-04).
//!
//! Security shape (T-58.2-05/06/07): the URL is operator config, never model input; redirects are
//! refused so a Bearer key cannot be forwarded elsewhere; the key is read from the env var named
//! by `api_key_env` on every call (D-08/D-25) and is never stored or logged.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::EmbedError;

/// After a failure, `readiness()` reports unavailable for this long.
const FAILURE_BACKOFF: Duration = Duration::from_secs(60);

pub struct HttpEmbedder {
    client: reqwest::Client,
    url: String,
    model: String,
    api_key_env: Option<String>,
    last_failure: Mutex<Option<Instant>>,
}

#[derive(Deserialize)]
struct Response {
    data: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    embedding: Vec<f32>,
    #[serde(default)]
    index: usize,
}

impl HttpEmbedder {
    pub fn new(base_url: &str, model: &str, api_key_env: Option<String>) -> Result<Self, String> {
        let base = base_url.trim_end_matches('/');
        let url = if base.ends_with("/v1") {
            format!("{base}/embeddings")
        } else {
            format!("{base}/v1/embeddings")
        };
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("building embeddings HTTP client: {e}"))?;
        Ok(Self {
            client,
            url,
            model: model.to_string(),
            api_key_env,
            last_failure: Mutex::new(None),
        })
    }

    pub fn model_id(&self) -> String {
        format!("http:{}", self.model)
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let result = self.request(texts).await;
        *self.last_failure.lock().unwrap_or_else(|p| p.into_inner()) =
            result.is_err().then(Instant::now);
        result
    }

    async fn request(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let mut req = self.client.post(&self.url).json(&serde_json::json!({
            "input": texts,
            "model": self.model,
            "encoding_format": "float",
        }));
        if let Some(name) = &self.api_key_env
            && let Ok(key) = std::env::var(name)
            && !key.is_empty()
        {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| EmbedError::Transport(e.without_url().to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(EmbedError::Transport(format!("HTTP {status}")));
        }
        let mut parsed: Response = resp.json().await.map_err(|e| {
            EmbedError::Transport(format!("bad response body: {}", e.without_url()))
        })?;
        parsed.data.sort_by_key(|i| i.index);
        Ok(parsed.data.into_iter().map(|i| i.embedding).collect())
    }

    pub fn readiness(&self) -> Result<(), String> {
        match *self.last_failure.lock().unwrap_or_else(|p| p.into_inner()) {
            Some(at) if at.elapsed() < FAILURE_BACKOFF => {
                Err("embeddings endpoint failed recently".to_string())
            }
            _ => Ok(()),
        }
    }
}
