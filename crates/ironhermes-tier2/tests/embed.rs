//! D-04/D-05/D-06/D-08/D-25/D-26 embedder behaviour: the OpenAI-compatible endpoint path, the
//! 384-dim guard, env-only Bearer auth, refused redirects, and the built-in model name map.

use std::path::Path;

use ironhermes_core::config::EmbeddingsConfig;
use ironhermes_core::tier2::TIER2_EMBEDDING_DIM;
use ironhermes_tier2::embed::{EmbedError, Embedder};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Serialises tests that touch process environment variables.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cfg(base_url: &str, model: Option<&str>, key_env: Option<&str>) -> EmbeddingsConfig {
    EmbeddingsConfig {
        base_url: Some(base_url.to_string()),
        model: model.map(str::to_string),
        api_key_env: key_env.map(str::to_string),
    }
}

fn embedder(c: &EmbeddingsConfig) -> Embedder {
    Embedder::from_config(c, Path::new("/nonexistent/models"))
}

fn body(items: &[(usize, f32, usize)]) -> serde_json::Value {
    let data: Vec<_> = items
        .iter()
        .map(|(index, fill, dim)| {
            serde_json::json!({"object": "embedding", "index": index, "embedding": vec![*fill; *dim]})
        })
        .collect();
    serde_json::json!({"object": "list", "model": "m", "data": data})
}

async fn mount_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body(&[
            (1, 0.2, TIER2_EMBEDDING_DIM),
            (0, 0.1, TIER2_EMBEDDING_DIM),
        ])))
        .mount(server)
        .await;
}

fn texts() -> Vec<String> {
    vec!["first".to_string(), "second".to_string()]
}

#[tokio::test]
async fn embed_http_request_shape() {
    for suffix in ["", "/v1", "/v1/"] {
        let server = MockServer::start().await;
        mount_ok(&server).await;
        let e = embedder(&cfg(
            &format!("{}{suffix}", server.uri()),
            Some("my-model"),
            None,
        ));
        let out = e.embed(&texts()).await.unwrap();

        // Out-of-order `data[].index` is re-sorted to input order.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0][0], 0.1);
        assert_eq!(out[1][0], 0.2);

        let reqs = server.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 1, "suffix {suffix:?}");
        let sent: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(sent["input"], serde_json::json!(["first", "second"]));
        assert_eq!(sent["model"], "my-model");
        assert_eq!(sent["encoding_format"], "float");
    }
}

#[tokio::test]
async fn embed_http_overrides_builtin() {
    let e = embedder(&cfg("http://127.0.0.1:9", None, None));
    assert!(matches!(e, Embedder::Http(_)));
    assert_eq!(e.model_id().as_deref(), Some("http:all-MiniLM-L6-v2"));
    let e = embedder(&cfg("http://127.0.0.1:9", Some("bge"), None));
    assert_eq!(e.model_id().as_deref(), Some("http:bge"));
}

#[tokio::test]
async fn embed_auth_bearer_iff_env_set() {
    let _guard = ENV_LOCK.lock().await;
    let var = "IH_TIER2_TEST_EMBED_KEY";
    let server = MockServer::start().await;
    mount_ok(&server).await;
    let e = embedder(&cfg(&server.uri(), None, Some(var)));

    // SAFETY: serialised by ENV_LOCK; no other test reads this variable.
    unsafe { std::env::set_var(var, "sekret") };
    e.embed(&texts()).await.unwrap();
    unsafe { std::env::set_var(var, "") };
    e.embed(&texts()).await.unwrap();
    unsafe { std::env::remove_var(var) };
    e.embed(&texts()).await.unwrap();

    let reqs = server.received_requests().await.unwrap();
    let auth: Vec<Option<String>> = reqs
        .iter()
        .map(|r| {
            r.headers
                .get("authorization")
                .map(|v| v.to_str().unwrap().to_string())
        })
        .collect();
    assert_eq!(auth, vec![Some("Bearer sekret".to_string()), None, None]);
}

#[tokio::test]
async fn embed_auth_redirect_refused() {
    let _guard = ENV_LOCK.lock().await;
    let target = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&target)
        .await;
    let origin = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", format!("{}/steal", target.uri())),
        )
        .mount(&origin)
        .await;

    let var = "IH_TIER2_TEST_REDIRECT_KEY";
    // SAFETY: serialised by ENV_LOCK.
    unsafe { std::env::set_var(var, "sekret") };
    let e = embedder(&cfg(&origin.uri(), None, Some(var)));
    let err = e.embed(&texts()).await.unwrap_err();
    unsafe { std::env::remove_var(var) };

    assert!(matches!(err, EmbedError::Transport(_)), "got {err:?}");
    assert!(target.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn dim_guard_http_rejects_non_384() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(body(&[(0, 0.1, 383), (1, 0.1, 383)])),
        )
        .mount(&server)
        .await;
    let e = embedder(&cfg(&server.uri(), None, None));
    match e.embed(&texts()).await {
        Err(EmbedError::DimensionMismatch { expected, got }) => {
            assert_eq!((expected, got), (384, 383));
        }
        other => panic!("expected DimensionMismatch, got {other:?}"),
    }
}

#[cfg(feature = "builtin-embedder")]
mod builtin {
    use ironhermes_core::tier2::TIER2_BUILTIN_MODELS_384;
    use ironhermes_tier2::embed::builtin::builtin_model_for;

    #[test]
    fn dim_guard_builtin_names_map_to_384() {
        for name in TIER2_BUILTIN_MODELS_384 {
            let model = builtin_model_for(name)
                .unwrap_or_else(|| panic!("{name} has no fastembed variant"));
            let info = fastembed::TextEmbedding::get_model_info(&model).unwrap();
            assert_eq!(info.dim, 384, "{name}");
        }
    }

    #[test]
    fn dim_guard_unknown_builtin_rejected() {
        assert!(builtin_model_for("text-embedding-3-large").is_none());
    }

    /// Manual, G-58.2-1/2: downloads ~90 MB once. Two embedders (one per process in production)
    /// start together on one empty cache and both must load. Before the init lock, the second one
    /// failed ~5s in with hf-hub's lock-acquisition error whenever the download took longer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn real_model_two_embedders_share_one_cache() {
        use ironhermes_tier2::embed::builtin::BuiltinEmbedder;

        let dir = tempfile::tempdir().unwrap();
        let mk = || {
            BuiltinEmbedder::new(
                "all-MiniLM-L6-v2",
                builtin_model_for("all-MiniLM-L6-v2").unwrap(),
                dir.path(),
            )
        };
        let (a, b) = (mk(), mk());
        let (ra, rb) = tokio::join!(a.init_blocking(), b.init_blocking());
        ra.unwrap_or_else(|e| panic!("a: {e}"));
        rb.unwrap_or_else(|e| panic!("b: {e}"));
        assert!(a.readiness().is_ok());
        assert!(b.readiness().is_ok());
        let v = b.embed(&["the cat sat".to_string()]).await.unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].len(), 384);
    }

    /// Manual: downloads ~90 MB from huggingface.co. `cargo test -- --ignored real_model`.
    #[tokio::test]
    #[ignore]
    async fn real_model_minilm_384() {
        use ironhermes_core::config::EmbeddingsConfig;
        use ironhermes_tier2::embed::Embedder;

        let dir = tempfile::tempdir().unwrap();
        let e = Embedder::from_config(&EmbeddingsConfig::default(), dir.path());
        let Embedder::Builtin(b) = &e else {
            panic!("default config must select the built-in embedder")
        };
        b.init_blocking().await.unwrap();
        assert!(e.readiness().is_ok());
        let v = e
            .embed(&["the cat sat".to_string(), "a feline rested".to_string()])
            .await
            .unwrap();
        assert!(v.iter().all(|x| x.len() == 384));
        let dot: f32 = v[0].iter().zip(&v[1]).map(|(a, b)| a * b).sum();
        let norm = |x: &[f32]| x.iter().map(|a| a * a).sum::<f32>().sqrt();
        println!("cosine = {}", dot / (norm(&v[0]) * norm(&v[1])));
    }
}
