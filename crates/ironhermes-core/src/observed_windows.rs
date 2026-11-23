//! Process-wide overlay of context windows observed from a server (Phase 58.3.1, D-07).
//!
//! A window is either `Probed` (reported by a local server) or `Learned` (parsed
//! from a provider context-length rejection). It is keyed by base_url + model
//! (not provider name: the window belongs to the server process at that URL
//! serving that model) and persisted in `models-cache.json` under
//! `context_windows`, never in `config.yaml`.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::constants::{MAX_PLAUSIBLE_CONTEXT_LENGTH, MIN_PLAUSIBLE_OBSERVED_CONTEXT_LENGTH};
use crate::models_cache::ModelsCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedWindowSource {
    /// Reported by the local server itself.
    Probed,
    /// Parsed from a provider context-length rejection.
    Learned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedWindow {
    pub tokens: usize,
    pub source: ObservedWindowSource,
    pub observed_at: DateTime<Utc>,
}

/// `"{scheme}://{host}{:port}{path}|{model}"`, host lowercased, trailing slash
/// stripped. Host, port and path are all part of the key so a LAN server's small
/// window never applies to a hosted provider's same-named model.
pub fn observed_window_key(base_url: &str, model: &str) -> String {
    let trimmed = base_url.trim();
    let base = match url::Url::parse(trimmed) {
        Ok(u) if u.host_str().is_some() => {
            let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
            format!(
                "{}://{}{}{}",
                u.scheme(),
                u.host_str().unwrap_or_default().to_ascii_lowercase(),
                port,
                u.path().trim_end_matches('/')
            )
        }
        _ => trimmed.trim_end_matches('/').to_ascii_lowercase(),
    };
    format!("{base}|{model}")
}

fn overlay() -> &'static RwLock<HashMap<String, ObservedWindow>> {
    static OVERLAY: OnceLock<RwLock<HashMap<String, ObservedWindow>>> = OnceLock::new();
    OVERLAY.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn lookup(base_url: &str, model: &str) -> Option<ObservedWindow> {
    let key = observed_window_key(base_url, model);
    overlay()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .copied()
}

/// Insert only keys not already present: an in-process observation is newer
/// than whatever is on disk.
pub fn seed(windows: &HashMap<String, ObservedWindow>) {
    if windows.is_empty() {
        return;
    }
    let mut guard = overlay().write().unwrap_or_else(|e| e.into_inner());
    for (k, w) in windows {
        guard.entry(k.clone()).or_insert(*w);
    }
}

/// Validate and store in the overlay only (no disk write). Returns the key and
/// window on success.
pub(crate) fn record_in_memory(
    base_url: &str,
    model: &str,
    tokens: usize,
    source: ObservedWindowSource,
) -> Option<(String, ObservedWindow)> {
    if !(MIN_PLAUSIBLE_OBSERVED_CONTEXT_LENGTH..=MAX_PLAUSIBLE_CONTEXT_LENGTH).contains(&tokens) {
        tracing::warn!(
            tokens,
            model,
            "rejecting implausible observed context window"
        );
        return None;
    }
    let key = observed_window_key(base_url, model);
    let window = ObservedWindow {
        tokens,
        source,
        observed_at: Utc::now(),
    };
    overlay()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.clone(), window);
    Some((key, window))
}

/// Record an observed window (latest observation wins) and persist it to
/// `models-cache.json`. Returns false when `tokens` is implausible. A
/// persistence failure is logged and does not fail the call: the in-process
/// value still applies.
pub fn record(base_url: &str, model: &str, tokens: usize, source: ObservedWindowSource) -> bool {
    let Some((key, window)) = record_in_memory(base_url, model, tokens, source) else {
        return false;
    };
    // shortcut: synchronous rewrite per new observation (rare); move to a
    // background writer if observations ever become frequent.
    if let Err(e) = ModelsCache::upsert_observed_window_at(&ModelsCache::cache_path(), &key, window)
    {
        tracing::warn!(error = %e, "failed to persist observed context window");
    }
    true
}

fn probed_keys() -> &'static Mutex<HashSet<String>> {
    static PROBED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    PROBED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Ask a local server for its loaded window once per base_url + model per
/// process (D-10, D-12) and record it as `Probed`. No-op for hosted endpoints
/// and for models whose window the operator fixed with a per-model override.
/// The key is claimed BEFORE awaiting so concurrent callers never double-probe.
pub async fn probe_once(ep: &crate::provider::ResolvedEndpoint) {
    use crate::provider::ContextLengthSource;
    if !crate::net_class::is_local_endpoint(&ep.base_url)
        || ep.context_length_source() == ContextLengthSource::PerModelConfig
    {
        return;
    }
    let key = observed_window_key(&ep.base_url, &ep.default_model);
    if !probed_keys()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key)
    {
        return;
    }
    if let Some(n) = crate::models_cache::probe_loaded_window(
        &ep.base_url,
        ep.api_key.as_deref(),
        &ep.default_model,
    )
    .await
    {
        record(
            &ep.base_url,
            &ep.default_model,
            n,
            ObservedWindowSource::Probed,
        );
    }
}

#[doc(hidden)]
pub fn reset_for_tests() {
    overlay().write().unwrap_or_else(|e| e.into_inner()).clear();
    probed_keys()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_key_isolates_base_urls_and_models() {
        let k = |u: &str, m: &str| observed_window_key(u, m);
        let base = k("http://host:8080/v1", "m");
        assert_ne!(base, k("http://other:8080/v1", "m"));
        assert_ne!(base, k("http://host:8081/v1", "m"));
        assert_ne!(base, k("http://host:8080/v2", "m"));
        assert_ne!(base, k("http://host:8080/v1", "n"));
        assert_eq!(k("http://Host:8080/v1/", "m"), base);
    }

    #[test]
    fn observed_record_rejects_implausible_windows() {
        // Unique URL per test; no reset_for_tests() here — clearing the shared
        // overlay would race parallel tier tests in provider.rs.
        let url = "http://127.0.0.1:9/observed-reject/v1";
        for bad in [0, 1_023, MAX_PLAUSIBLE_CONTEXT_LENGTH + 1] {
            assert!(record_in_memory(url, "m", bad, ObservedWindowSource::Learned).is_none());
            assert!(!record(url, "m", bad, ObservedWindowSource::Learned));
        }
        assert!(lookup(url, "m").is_none());
    }

    #[test]
    fn observed_record_persists_to_models_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let url = "http://127.0.0.1:9/observed-persist/v1";
        let prev = std::env::var("IRONHERMES_HOME").ok();
        // SAFETY: tests in this crate that touch IRONHERMES_HOME run with
        // --test-threads=1 (plan verify gate); the value is restored below.
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()) };
        let ok = record(url, "m", 8192, ObservedWindowSource::Learned);
        let cache = ModelsCache::load();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                None => std::env::remove_var("IRONHERMES_HOME"),
            }
        }
        assert!(ok);
        let w = lookup(url, "m").expect("overlay entry");
        assert_eq!(w.tokens, 8192);
        assert_eq!(w.source, ObservedWindowSource::Learned);
        let persisted = cache
            .context_windows
            .get(&observed_window_key(url, "m"))
            .expect("persisted entry");
        assert_eq!(persisted.tokens, 8192);
        assert!(tmp.path().join("models-cache.json").exists());
        assert!(!tmp.path().join("config.yaml").exists());
    }

    fn probe_ep(base_url: String, per_model: Option<usize>) -> crate::provider::ResolvedEndpoint {
        crate::provider::ResolvedEndpoint {
            base_url,
            api_key: None,
            api_mode: crate::config::ApiMode::ChatCompletions,
            default_model: "probe-model".into(),
            fallback_providers: vec![],
            model_metadata: None,
            config_context_length: None,
            provider_model_context_length: per_model,
            models: vec![],
        }
    }

    #[tokio::test]
    async fn probe_once_runs_once_per_key() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/props"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"default_generation_settings":{"n_ctx":8192}}),
                ),
            )
            .mount(&server)
            .await;
        let ep = probe_ep(format!("{}/v1", server.uri()), None);
        reset_for_tests();
        let tmp = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var("IRONHERMES_HOME").ok();
        // SAFETY: gated by --test-threads=1; restored below.
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()) };
        probe_once(&ep).await;
        let after_first = server.received_requests().await.expect("recorded").len();
        probe_once(&ep).await;
        let after_second = server.received_requests().await.expect("recorded").len();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                None => std::env::remove_var("IRONHERMES_HOME"),
            }
        }
        assert_eq!(after_first, 3, "one probe = three legs");
        assert_eq!(
            after_second, after_first,
            "second call issues zero requests"
        );
        let w = lookup(&ep.base_url, &ep.default_model).expect("recorded");
        assert_eq!((w.tokens, w.source), (8192, ObservedWindowSource::Probed));
    }

    #[tokio::test]
    async fn probe_once_skips_public_and_overridden_endpoints() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let overridden = probe_ep(format!("{}/v1", server.uri()), Some(32_768));
        // Not local: example.com is never contacted either (no request leaves).
        let public = probe_ep("https://example.com/v1".into(), None);
        probe_once(&overridden).await;
        probe_once(&public).await;
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded")
                .is_empty()
        );
    }
}
