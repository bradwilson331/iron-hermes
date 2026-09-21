//! Production judge-builder for goal-mode evaluation loops (D-03 lift).
//!
//! Phase 49.7 D-03: lifted verbatim (behavior-for-behavior) out of
//! `ironhermes-cli/src/kanban/commands.rs` so a crate with no
//! `ironhermes-kanban` dependency — the session-level `/goal` loop in
//! `ironhermes-agent/src/goal_session_loop.rs`, and eventually the gateway
//! and web server — can build a production [`JudgeFn`] without pulling in
//! kanban's SQLite-backed store.
//!
//! The only field any of these functions ever read off kanban's
//! `KanbanConfig` was `.judge_model`, so every signature here takes
//! `judge_model: &str` instead of `&KanbanConfig` — carrying the whole
//! config type into this crate would recreate the coupling D-03 exists to
//! remove for the sake of naming one parameter type.
//!
//! `ironhermes-cli/src/kanban/commands.rs` keeps same-named delegating
//! wrappers (`&KanbanConfig`-taking) so its three existing external callers
//! — `main.rs`, `judge_model_resolution.rs`, and
//! `iron_hermes_ui/src/server/profile_verify_api.rs` — stay byte-unchanged.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use ironhermes_core::ChatMessage;
use ironhermes_core::config::Config;
use ironhermes_core::judge::{JudgeError, JudgeFn, JudgeOutput, JudgeRequest, JudgeVerdict};
use ironhermes_core::models_cache::ModelsCache;
use ironhermes_core::provider::ProviderResolver;

use crate::client::LlmClient;

/// Resolve the judge's provider endpoint and model via the three-tier
/// cascade (D-05): `judge_model` (tier 1) -> `model.roles["kanban_judge"]`
/// (tier 2) -> main provider's default model (tier 3).
///
/// Returns `Err` with an actionable message naming BOTH `kanban.judge_model`
/// AND `auxiliary.kanban_judge` when no provider can produce an API key —
/// mirrors the decomposer error pattern the move source cites
/// (`ironhermes-cli/src/kanban/commands.rs:1448-1454`).
pub fn resolve_judge_model_and_endpoint(
    judge_model: &str,
    main_config: &Config,
) -> anyhow::Result<(ironhermes_core::provider::ResolvedEndpoint, String)> {
    resolve_judge_model_and_endpoint_with_env_overrides(judge_model, main_config, &HashMap::new())
}

/// [`resolve_judge_model_and_endpoint`] with a profile-scoped API-key override
/// map (Phase 47.4 D-14).
///
/// The web server resolves a judge cascade for an arbitrary kanban worker
/// profile inside one running (multi-threaded) process, so the ordinary
/// `ProviderResolver::build` — which reads keys from the *server process*
/// environment (the ROOT profile) — would validate the operator's own key
/// instead of the profile being probed. This sibling threads `overrides`
/// (populated by the caller from the target profile's `.env`) through to
/// [`ironhermes_core::provider::ProviderResolver::build_with_env_overrides`]
/// so the cascade, the `with_context` message, and the fail-closed
/// empty-`api_key` bail all live in exactly one place — this function.
/// `resolve_judge_model_and_endpoint` delegates here with an empty map, so
/// its behavior for every existing caller is unchanged.
///
/// Phase 47.4 Plan 17 (CR-01): this is the operator's own interactive
/// resolution — `overrides` misses still fall back to the *process*
/// environment. For any "what would the SCRUBBED spawned worker see?"
/// question, use [`resolve_judge_model_and_endpoint_with_env_overrides_strict`]
/// instead; this function's behavior for every existing caller is unchanged.
pub fn resolve_judge_model_and_endpoint_with_env_overrides(
    judge_model: &str,
    main_config: &Config,
    overrides: &HashMap<String, String>,
) -> anyhow::Result<(ironhermes_core::provider::ResolvedEndpoint, String)> {
    resolve_judge_model_and_endpoint_with_env_scope(judge_model, main_config, overrides, true)
}

/// [`resolve_judge_model_and_endpoint_with_env_overrides`] with the
/// process-environment fallback **disabled** (Phase 47.4 Plan 17, CR-01) —
/// `overrides` is the entire world, mirroring
/// [`ironhermes_core::provider::ProviderResolver::build_with_env_overrides_strict`].
///
/// Answers "what would the SCRUBBED spawned worker see?" — the question
/// VERIFY (`iron_hermes_ui`'s `build_probe_setup`) needs answered. A
/// judge-tier key present only in the *server process* environment (e.g. the
/// ROOT `.env` `main.rs` loads at startup) must never make this resolve `Ok`
/// for a DIFFERENT profile being probed.
pub fn resolve_judge_model_and_endpoint_with_env_overrides_strict(
    judge_model: &str,
    main_config: &Config,
    overrides: &HashMap<String, String>,
) -> anyhow::Result<(ironhermes_core::provider::ResolvedEndpoint, String)> {
    resolve_judge_model_and_endpoint_with_env_scope(judge_model, main_config, overrides, false)
}

/// Shared body for [`resolve_judge_model_and_endpoint_with_env_overrides`]
/// and its `_strict` sibling (Phase 47.4 Plan 17, CR-01) — the three-tier
/// cascade, the `with_context` message, and the fail-closed empty-`api_key`
/// bail live in exactly this one place. `allow_process_env` selects between
/// [`ironhermes_core::provider::ProviderResolver::build_with_env_overrides`]
/// and its `_strict` sibling, mirroring `build_with_env_scope`'s own shape
/// (`ironhermes-core/src/provider.rs:244`).
fn resolve_judge_model_and_endpoint_with_env_scope(
    judge_model: &str,
    main_config: &Config,
    overrides: &HashMap<String, String>,
    allow_process_env: bool,
) -> anyhow::Result<(ironhermes_core::provider::ResolvedEndpoint, String)> {
    let registry = if allow_process_env {
        ProviderResolver::build_with_env_overrides(main_config, ModelsCache::load(), overrides)
    } else {
        ProviderResolver::build_with_env_overrides_strict(main_config, ModelsCache::load(), overrides)
    }
    .with_context(|| {
        "judge model not configured — set `kanban.judge_model` in config.yaml \
     OR `auxiliary.kanban_judge` OR ensure `model.default` is set with a \
     valid provider"
    })?;

    // Resolve endpoint + model per the three-tier cascade.
    let (endpoint, resolved_model) = if !judge_model.is_empty() {
        // Tier 1: judge_model is set — use main provider with this model.
        let ep = registry.resolve_for_main().clone();
        let model = judge_model.to_string();
        (ep, model)
    } else if let Some(ep) = registry.resolve_role("kanban_judge") {
        // Tier 2: model.roles["kanban_judge"] resolves (carries endpoint + model).
        let model = ep.default_model.clone();
        (ep, model)
    } else {
        // Tier 3: fall back to main provider's default model.
        let ep = registry.resolve_for_main().clone();
        let model = ep.default_model.clone();
        (ep, model)
    };

    // CR-03 mirror (commands.rs:1442-1454): fail closed when no API key is
    // resolvable — surface the actionable message rather than silently
    // calling chat_completion with an empty Bearer token.
    let api_key = endpoint.api_key.clone().unwrap_or_default();
    if api_key.is_empty() {
        anyhow::bail!(
            "judge model not configured — set `kanban.judge_model` in \
             config.yaml OR `auxiliary.kanban_judge` OR ensure `model.default` \
             is set with a valid API key (endpoint: {})",
            endpoint.base_url
        );
    }

    Ok((endpoint, resolved_model))
}

/// Build the production [`JudgeFn`] from operator config (D-05).
///
/// The resulting closure builds a static, trusted system prompt asking the
/// judge to return `{verdict: "met"|"not_met", reason: "..."}` JSON, sends
/// `title + body + worker_turn_output` as the user message, calls the
/// provider's text-completion endpoint via `LlmClient::chat_completion`,
/// and parses the response with fail-closed semantics:
///
/// - Transport/provider failure -> `JudgeError::Provider`.
/// - JSON parse failure -> `JudgeError::ResponseNotJson` with the raw
///   response truncated to 200 chars (T-36.3.7.12-03-T01 mitigation).
/// - Missing `verdict` field -> `JudgeError::MissingVerdict` with the same
///   truncation.
/// - `verdict` value not in {"met","not_met"} (case-sensitive) ->
///   `JudgeError::UnrecognizedVerdict` (T-36.3.7.12-03-T02 mitigation).
pub fn build_runtime_judge_fn(judge_model: &str, main_config: &Config) -> anyhow::Result<JudgeFn> {
    build_runtime_judge_fn_with_env_overrides(judge_model, main_config, &HashMap::new())
}

/// [`build_runtime_judge_fn`] with a profile-scoped API-key override map
/// (Phase 47.4 D-14).
///
/// Builds the same `JudgeFn` closure — trusted static system prompt,
/// `LlmClient` call, and fail-closed JSON verdict parsing — against a
/// specific kanban worker profile's key material instead of the server
/// process's own environment. `build_runtime_judge_fn` delegates here with
/// an empty map.
///
/// Phase 47.4 Plan 17 (CR-01): this is the operator's own interactive
/// resolution — it resolves through
/// [`resolve_judge_model_and_endpoint_with_env_overrides`], which falls back
/// to the process environment. For any "what would the SCRUBBED spawned
/// worker see?" question, use
/// [`build_runtime_judge_fn_with_env_overrides_strict`] instead.
pub fn build_runtime_judge_fn_with_env_overrides(
    judge_model: &str,
    main_config: &Config,
    overrides: &HashMap<String, String>,
) -> anyhow::Result<JudgeFn> {
    build_runtime_judge_fn_with_env_scope(judge_model, main_config, overrides, true)
}

/// [`build_runtime_judge_fn_with_env_overrides`] with the process-environment
/// fallback **disabled** (Phase 47.4 Plan 17, CR-01) — resolves through
/// [`resolve_judge_model_and_endpoint_with_env_overrides_strict`], so
/// `overrides` (the target profile's own `.env`) is the entire world.
///
/// Answers "what would the SCRUBBED spawned worker see?" — the question
/// VERIFY (`iron_hermes_ui`'s `build_probe_setup`) needs answered. A
/// judge-tier key present only in the *server process* environment must
/// never let this build a closure the spawned worker's scrubbed environment
/// (`.env_clear()` + profile `.env`) could not itself build.
pub fn build_runtime_judge_fn_with_env_overrides_strict(
    judge_model: &str,
    main_config: &Config,
    overrides: &HashMap<String, String>,
) -> anyhow::Result<JudgeFn> {
    build_runtime_judge_fn_with_env_scope(judge_model, main_config, overrides, false)
}

/// Shared body for [`build_runtime_judge_fn_with_env_overrides`] and its
/// `_strict` sibling (Phase 47.4 Plan 17, CR-01) — the `JudgeFn` closure
/// construction (trusted static system prompt, `LlmClient` call, fail-closed
/// JSON verdict parsing) lives in exactly this one place. `allow_process_env`
/// selects between [`resolve_judge_model_and_endpoint_with_env_overrides`]
/// and its `_strict` sibling for the underlying cascade resolution.
fn build_runtime_judge_fn_with_env_scope(
    judge_model: &str,
    main_config: &Config,
    overrides: &HashMap<String, String>,
    allow_process_env: bool,
) -> anyhow::Result<JudgeFn> {
    let (endpoint, resolved_model) = if allow_process_env {
        resolve_judge_model_and_endpoint_with_env_overrides(judge_model, main_config, overrides)?
    } else {
        resolve_judge_model_and_endpoint_with_env_overrides_strict(judge_model, main_config, overrides)?
    };

    let base_url = endpoint.base_url.clone();
    let api_key = endpoint.api_key.clone().unwrap_or_default();
    let model_for_closure: String = resolved_model;

    let judge_fn: JudgeFn = Arc::new(move |req: JudgeRequest| {
        let base_url = base_url.clone();
        let api_key = api_key.clone();
        let model = model_for_closure.clone();

        Box::pin(async move {
            let client = LlmClient::new(&base_url, &api_key, &model);

            // System prompt: TRUSTED, static. The judge evaluates worker
            // output against `title + body` (= literal acceptance criteria,
            // CONTEXT.md D-01). Response shape is fixed JSON so the loop
            // wrapper can dispatch on `verdict` without LLM-side prose
            // creep. T-36.3.7.12-03-T03: even on a successful prompt
            // injection in `body`, the response space is still constrained
            // to the JSON parser below.
            let system_prompt = "You evaluate whether worker output meets the acceptance criteria in \
                 title + body. Respond with JSON: \
                 {\"verdict\": \"met\" | \"not_met\", \"reason\": \"<short explanation>\"}.";

            let user_prompt = format!(
                "Title: {}\n\nAcceptance criteria (body):\n{}\n\nWorker output:\n{}",
                req.title, req.body, req.worker_turn_output,
            );

            let messages = vec![
                ChatMessage::system(system_prompt),
                ChatMessage::user(&user_prompt),
            ];

            let response = client
                .chat_completion(&messages, None, None, Some(1024), Some(0.0), None)
                .await
                .map_err(|e| JudgeError::Provider(e.to_string()))?;

            // Extract text content from the first choice — same pattern as
            // build_runtime_decompose_fn at commands.rs:1507-1514.
            let raw_text = response
                .choices
                .first()
                .and_then(|c| c.message.content.as_ref())
                .and_then(|mc| mc.as_text())
                .unwrap_or("")
                .trim()
                .to_string();

            // 200-char char-boundary-safe preview (T-36.3.7.12-03-I01 V8
            // bounded-disclosure mitigation).
            let preview: String = raw_text.chars().take(200).collect();

            // Parse JSON — fail-closed on malformed body.
            let parsed: serde_json::Value = serde_json::from_str(&raw_text).map_err(|source| {
                JudgeError::ResponseNotJson {
                    source,
                    preview: preview.clone(),
                }
            })?;

            // Required `verdict` string field — fail-closed on absence.
            let verdict_str = parsed
                .get("verdict")
                .and_then(|v| v.as_str())
                .ok_or_else(|| JudgeError::MissingVerdict {
                    preview: preview.clone(),
                })?;

            // Case-sensitive match against the two locked literals
            // (T-36.3.7.12-03-T02 mitigation).
            let verdict = match verdict_str {
                "met" => JudgeVerdict::Met,
                "not_met" => JudgeVerdict::NotMet,
                other => {
                    return Err(JudgeError::UnrecognizedVerdict {
                        literal: other.to_string(),
                        preview: preview.clone(),
                    });
                }
            };

            // Reason field is optional; empty string when absent.
            let reason = parsed
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // turn is informational on the closure side — the wrapper that
            // emits judge_verdict events uses req.turn directly.
            let _ = req.turn;

            Ok(JudgeOutput { verdict, reason })
        })
    });

    Ok(judge_fn)
}
