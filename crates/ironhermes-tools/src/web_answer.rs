//! Phase 41.3 D-07/D-08/D-09/D-13/D-14/D-19 (Plan 08): `web_answer` — the
//! answer half of D-07's tool-surface split. `web_search` (Plan 07) keeps
//! the results-list contract; `web_answer` fronts synthesized answers from
//! Perplexity, Exa, Brave's machine-consumption endpoint, and DuckDuckGo's
//! keyless Instant Answer API — a separate tool with its own honest
//! contract, not a `mode` parameter on `web_search` (D-07 explicitly rejects
//! that shape: it makes backend selection capability-dependent, so a
//! request could be unserviceable by whichever backend the chain selected).

pub mod backends;

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;

use crate::credentials::ToolCredentials;
use crate::registry::Tool;

/// Phase 41.3 D-07/D-13/D-14 (Plan 08): one backend's answer outcome — the
/// synthesized text, its citation URLs (empty when the provider supplied
/// none), its optional cost figure (D-14, capture-only — never enforced),
/// and which provider answered. Every backend returns this exact shape
/// regardless of provider — `web_answer`'s one honest contract.
#[derive(Debug, Clone)]
pub struct AnswerResult {
    pub text: String,
    pub citations: Vec<String>,
    pub cost_dollars: Option<f64>,
    pub provider: &'static str,
}

/// Phase 41.3 D-09 (Plan 08): any-of group id shared by every KEYED
/// `web_answer` provider's prerequisite (Perplexity, Exa, Brave) — DDG needs
/// no key and contributes none. Mirrors `web_search::WEB_SEARCH_PROVIDER_GROUP`
/// (Plan 07) exactly.
pub const WEB_ANSWER_PROVIDER_GROUP: &str = "web_answer_provider";

/// Phase 41.3 D-13 (Plan 08): coarse progress-phase callback — fired with a
/// phase literal (`"searching"` before the winning provider's request
/// begins, `"synthesizing"` once its response has been fully collected)
/// from inside `execute()`. `Arc` (not `Box`, mirroring
/// `delegate_task::SubagentProgressCallback` rather than
/// `agent_loop::ToolProgressCallback`'s `Box`) so a caller may share one
/// callback across more than one constructed tool instance. This is a
/// constructor-injected seam — the same shape `delegate_task` already uses
/// (`registry.rs`'s `register_delegate_task_tool` / `with_progress_callback`)
/// — not a wire into `agent_loop::ToolProgressCallback` itself, which fires
/// pre-execution only and carries no in-flight progress handle
/// (`agent_loop.rs:2354`). D-13 prohibits a new `StreamEvent` variant; this
/// callback carries only the two fixed phase literals, never provider
/// content, so it never needs one.
pub type WebAnswerProgressCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// Phase 41.3 D-19 (Plan 08): stateful from construction — carries the
/// resolved credential snapshot (mirrors `web_search::WebSearchTool`, which
/// became stateful under Plan 11; `WebAnswerTool` is a new struct built
/// stateful from day one) and an optional D-13 progress callback.
pub struct WebAnswerTool {
    creds: Arc<ToolCredentials>,
    progress_callback: Option<WebAnswerProgressCallback>,
}

impl WebAnswerTool {
    /// Construct with a resolved credential snapshot — the production path.
    /// `register_defaults_except` and the RPC-sandbox construction sites
    /// pass the registering `ToolRegistry`'s own snapshot here.
    pub fn new(creds: Arc<ToolCredentials>) -> Self {
        Self {
            creds,
            progress_callback: None,
        }
    }

    /// D-13: install a coarse progress-phase callback. Builder-style —
    /// mirrors `delegate_task::DelegateTaskTool::with_progress_callback`.
    pub fn with_progress_callback(mut self, cb: WebAnswerProgressCallback) -> Self {
        self.progress_callback = Some(cb);
        self
    }

    fn emit_phase(&self, phase: &str) {
        if let Some(cb) = &self.progress_callback {
            cb(phase);
        }
    }

    /// Phase 41.3 D-08/D-19 (Plan 08): the config-ordered chain walk,
    /// returning the structured `AnswerResult` — mirrors
    /// `web_search::WebSearchTool::search_chain` (Plan 07) exactly: for each
    /// chain entry, asks `self.creds` (the D-19 snapshot) for that
    /// provider's key and skips the entry — before any network attempt —
    /// when the snapshot has none. A key held only in `tools.credentials` or
    /// only in the vault therefore counts as configured exactly like an env
    /// var would (D-19); this never reads a provider key directly from the
    /// process environment. On a backend error (including DDG's clean-miss
    /// `Err`), warns and continues to the next entry; returns on the first
    /// success. An unknown entry that slipped past `ToolsConfig::validate_chains()`
    /// is warned once and skipped, never a panic.
    async fn answer_chain(&self, query: &str, chain: &[String]) -> anyhow::Result<AnswerResult> {
        let mut skip_or_fail_reasons: Vec<String> = Vec::new();

        for entry in chain {
            match entry.as_str() {
                "perplexity" => {
                    let Some(api_key) = self.creds.credential("PERPLEXITY_API_KEY") else {
                        tracing::debug!(
                            "web_answer: skipping perplexity in chain — PERPLEXITY_API_KEY not configured"
                        );
                        skip_or_fail_reasons.push("perplexity: no key configured".to_string());
                        continue;
                    };
                    match backends::perplexity::answer(query, &api_key).await {
                        Ok(outcome) => return Ok(outcome),
                        Err(e) => {
                            tracing::warn!("web_answer: perplexity failed: {}", e);
                            skip_or_fail_reasons.push(format!("perplexity: {e}"));
                        }
                    }
                }
                "exa" => {
                    let Some(api_key) = self.creds.credential("EXA_API_KEY") else {
                        tracing::debug!(
                            "web_answer: skipping exa in chain — EXA_API_KEY not configured"
                        );
                        skip_or_fail_reasons.push("exa: no key configured".to_string());
                        continue;
                    };
                    match backends::exa::answer(query, &api_key).await {
                        Ok(outcome) => return Ok(outcome),
                        Err(e) => {
                            tracing::warn!("web_answer: exa failed: {}", e);
                            skip_or_fail_reasons.push(format!("exa: {e}"));
                        }
                    }
                }
                "brave" => {
                    let Some(api_key) = self.creds.credential("BRAVE_API_KEY") else {
                        tracing::debug!(
                            "web_answer: skipping brave in chain — BRAVE_API_KEY not configured"
                        );
                        skip_or_fail_reasons.push("brave: no key configured".to_string());
                        continue;
                    };
                    match backends::brave::answer(query, &api_key).await {
                        Ok(outcome) => return Ok(outcome),
                        Err(e) => {
                            tracing::warn!("web_answer: brave failed: {}", e);
                            skip_or_fail_reasons.push(format!("brave: {e}"));
                        }
                    }
                }
                "ddg" => match backends::ddg::answer(query).await {
                    Ok(outcome) => return Ok(outcome),
                    Err(e) => {
                        tracing::warn!("web_answer: ddg failed: {}", e);
                        skip_or_fail_reasons.push(format!("ddg: {e}"));
                    }
                },
                unknown => {
                    tracing::warn!(
                        "web_answer: unknown provider '{}' in tools.web_answer.chain — skipping",
                        unknown
                    );
                    skip_or_fail_reasons
                        .push(format!("{unknown}: unrecognised provider, skipped"));
                }
            }
        }

        Err(anyhow::anyhow!(
            "web_answer: no provider in the configured chain returned an answer ({})",
            if skip_or_fail_reasons.is_empty() {
                "chain is empty".to_string()
            } else {
                skip_or_fail_reasons.join("; ")
            }
        ))
    }
}

impl Default for WebAnswerTool {
    /// Test/inspection-only: builds an env-only snapshot
    /// (`ToolCredentials::env_only()`). Mirrors `web_search::WebSearchTool::default()`'s
    /// doc comment — a PRODUCTION call site using this silently loses the
    /// config and vault credential tiers.
    fn default() -> Self {
        Self::new(Arc::new(ToolCredentials::env_only()))
    }
}

/// Renders an `AnswerResult` into the tool's output text: the answer text,
/// followed by a citations list when the backend supplied one.
fn format_answer_output(outcome: &AnswerResult) -> String {
    if outcome.citations.is_empty() {
        return outcome.text.clone();
    }

    let mut output = outcome.text.clone();
    output.push_str("\n\nCitations:\n");
    for citation in &outcome.citations {
        output.push_str(&format!("- {citation}\n"));
    }
    output.trim_end().to_string()
}

#[async_trait]
impl Tool for WebAnswerTool {
    fn name(&self) -> &str {
        "web_answer"
    }

    fn toolset(&self) -> &str {
        "web"
    }

    fn description(&self) -> &str {
        "Get a synthesized, cited answer to a question by querying the web — returns prose \
         with citations, not a list of links. Use web_search instead when you want a ranked \
         list of results. Walks a config-ordered chain of providers (Perplexity, Exa, Brave, \
         DuckDuckGo by default) and is available even with no API key configured — \
         DuckDuckGo needs none."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "web_answer",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The question to answer."
                    }
                },
                "required": ["query"]
            }),
        )
    }

    /// D-09: `web_answer` is available even with zero provider keys
    /// configured. The default chain always terminates in DDG, which needs
    /// no key, so a fresh install never hard-fails here. Mirrors
    /// `web_search::WebSearchTool::is_available()`'s doc comment: this
    /// intentionally does NOT delegate to the trait default's group-aware
    /// AND/OR walk — DDG contributes no prerequisite entry at all, so an
    /// empty-group edge case in that generic walk must not be the thing
    /// deciding availability for this tool.
    fn is_available(&self) -> bool {
        true
    }

    /// D-09: one grouped, non-required prerequisite per KEYED provider — the
    /// wizard prompts "pick a provider" and `doctor` (Plan 09) can render an
    /// N-of-M count. DDG requires nothing and contributes no entry.
    fn prerequisites(&self) -> Vec<crate::registry::Prerequisite> {
        vec![
            crate::registry::Prerequisite::grouped_env_var(
                "PERPLEXITY_API_KEY",
                "Perplexity API key (https://www.perplexity.ai/settings/api) — one of \
                 several interchangeable web_answer providers.",
                WEB_ANSWER_PROVIDER_GROUP,
            ),
            crate::registry::Prerequisite::grouped_env_var(
                "EXA_API_KEY",
                "Exa API key (https://dashboard.exa.ai) — one of several interchangeable \
                 web_answer providers.",
                WEB_ANSWER_PROVIDER_GROUP,
            ),
            crate::registry::Prerequisite::grouped_env_var(
                "BRAVE_API_KEY",
                "Brave Search API key (https://api-dashboard.search.brave.com) — one of \
                 several interchangeable web_answer providers.",
                WEB_ANSWER_PROVIDER_GROUP,
            ),
            // DDG needs no key and contributes no prerequisite — it is the
            // keyless terminal of the default chain (D-09).
        ]
    }

    fn credentials(&self) -> Option<&crate::credentials::ToolCredentials> {
        Some(self.creds.as_ref())
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: query"))?;

        // D-13: one coarse phase before the winning provider's request begins.
        self.emit_phase("searching");

        // D-08: read the chain at call time — never cached at construction —
        // mirroring WebSearchTool's own convention.
        let cfg = ironhermes_core::config::Config::load().unwrap_or_default();
        let outcome = self.answer_chain(query, &cfg.tools.web_answer.chain).await?;

        // D-13: one coarse phase once the response has been fully collected
        // — every backend collects fully before returning (never a partial
        // slice), so this fires once, right before formatting the result.
        self.emit_phase("synthesizing");

        // D-14: capture only, never enforced. One structured tracing event,
        // the same observability posture as Plan 01's timeout counter and
        // Plan 07's web_search cost record.
        if let Some(cost) = outcome.cost_dollars {
            tracing::info!(
                tool = "web_answer",
                provider = outcome.provider,
                cost_dollars = cost,
                "web_answer provider cost captured"
            );
        }

        Ok(format_answer_output(&outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex as StdMutex, OnceLock};
    use tokio::sync::Mutex as TokioMutex;

    /// env_lock — serialise tests that mutate provider env vars. Mirrors
    /// `web_search::tests::env_lock` exactly (async-aware `tokio::sync::Mutex`
    /// since some tests hold the guard across an `.await`).
    fn env_lock() -> &'static TokioMutex<()> {
        static LOCK: OnceLock<TokioMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| TokioMutex::new(()))
    }

    fn clear_all_provider_keys() {
        // SAFETY: env_lock held, --test-threads=1 convention.
        unsafe {
            std::env::remove_var("PERPLEXITY_API_KEY");
            std::env::remove_var("EXA_API_KEY");
            std::env::remove_var("BRAVE_API_KEY");
        }
    }

    /// D-09: `is_available()` must stay `true` with every provider key
    /// absent from the snapshot — DDG terminates the chain.
    #[tokio::test]
    async fn is_available_is_true_with_no_provider_keys_set() {
        let _g = env_lock().lock().await;
        clear_all_provider_keys();

        let creds = ToolCredentials::resolve(&ironhermes_core::Config::default(), None)
            .await
            .expect("resolve must succeed");
        let tool = WebAnswerTool::new(Arc::new(creds));

        assert!(
            tool.is_available(),
            "web_answer must be available with zero provider keys — DDG terminates the chain (D-09)"
        );
    }

    /// D-09: every returned prerequisite shares the any-of provider group,
    /// none is independently required, and the set is exactly the three
    /// keyed providers — DDG contributes none.
    #[test]
    fn prerequisites_are_all_grouped_under_the_answer_provider_group() {
        let tool = WebAnswerTool::default();
        let prereqs = tool.prerequisites();

        assert_eq!(
            prereqs.len(),
            3,
            "one grouped prerequisite per keyed provider, DDG contributes none"
        );
        for p in &prereqs {
            assert_eq!(
                p.group.as_deref(),
                Some(WEB_ANSWER_PROVIDER_GROUP),
                "every web_answer prerequisite must share the any-of provider group, got {p:?}"
            );
            assert!(
                !p.required,
                "a grouped prerequisite must never be independently required, got {p:?}"
            );
            assert_eq!(
                p.kind, "env_var",
                "web_answer prerequisites are env_var-kind, got {p:?}"
            );
        }

        let mut names: Vec<&str> = prereqs.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["BRAVE_API_KEY", "EXA_API_KEY", "PERPLEXITY_API_KEY"],
            "web_answer's grouped prerequisites must name exactly the three keyed providers"
        );
    }

    /// D-13: with a test progress callback injected, executing against
    /// DDG's keyless terminal (stubbed via wiremock) records the phase
    /// strings in order — "searching" before "synthesizing" — and the tool
    /// still returns one complete string.
    #[tokio::test]
    async fn progress_phases_fire_in_order() {
        let _g = env_lock().lock().await;
        clear_all_provider_keys();

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
                r#"{"AbstractText":"Progress phase probe answer.","Answer":"","Definition":""}"#,
            ))
            .mount(&server)
            .await;
        // SAFETY: env_lock held, --test-threads=1 convention.
        unsafe { std::env::set_var("DDG_API_ENDPOINT_OVERRIDE", server.uri()) };

        let phases: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let recorder = phases.clone();
        let cb: WebAnswerProgressCallback = Arc::new(move |phase: &str| {
            recorder
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(phase.to_string());
        });

        let creds = ToolCredentials::resolve(&ironhermes_core::Config::default(), None)
            .await
            .expect("resolve must succeed");
        let tool = WebAnswerTool::new(Arc::new(creds)).with_progress_callback(cb);

        let result = tool
            .execute(json!({ "query": "progress phase probe" }))
            .await
            .expect("web_answer must succeed via ddg with zero provider keys configured");

        assert!(result.contains("Progress phase probe answer."), "got: {result}");
        let recorded = phases.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(
            *recorded,
            vec!["searching".to_string(), "synthesizing".to_string()],
            "phases must fire in order: searching before synthesizing"
        );

        // SAFETY: env_lock held.
        unsafe { std::env::remove_var("DDG_API_ENDPOINT_OVERRIDE") };
    }

    /// The returned value is the whole answer text — no partial value is
    /// emitted through any other channel (D-13).
    #[tokio::test]
    async fn execute_returns_a_single_complete_string() {
        let _g = env_lock().lock().await;
        clear_all_provider_keys();

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
                r#"{"AbstractText":"A complete, non-truncated answer body.","Answer":"","Definition":""}"#,
            ))
            .mount(&server)
            .await;
        // SAFETY: env_lock held, --test-threads=1 convention.
        unsafe { std::env::set_var("DDG_API_ENDPOINT_OVERRIDE", server.uri()) };

        let creds = ToolCredentials::resolve(&ironhermes_core::Config::default(), None)
            .await
            .expect("resolve must succeed");
        let tool = WebAnswerTool::new(Arc::new(creds));

        let result = tool
            .execute(json!({ "query": "completeness probe" }))
            .await
            .expect("web_answer must succeed via ddg with zero provider keys configured");

        assert_eq!(result, "A complete, non-truncated answer body.");

        // SAFETY: env_lock held.
        unsafe { std::env::remove_var("DDG_API_ENDPOINT_OVERRIDE") };
    }
}
