//! `/v1/models` and `/api/model/options` — served from the live
//! `ironhermes_core::ModelRegistry`, never a hard-coded list. Task 2 of
//! Phase 36.7.1 Plan 04.
//!
//! Both endpoints are computed from the SAME `ModelRegistry::all_models()`
//! read in each handler, so the picker's option list and the model list
//! agree by construction — the picker can never offer a model the list
//! denies exists (`model_options_and_model_list_agree`).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;

use super::super::ApiServerAdapter;

pub const MODEL_PATHS: &[&str] = &["/v1/models", "/api/model/options"];

fn model_json(id: &str, meta: &ironhermes_core::ModelMetadata) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "object": "model",
        "context_length": meta.context_length,
        "max_output_tokens": meta.max_output_tokens,
        "tokenizer": meta.tokenizer,
        "capabilities": {
            "vision": meta.capabilities.vision,
            "tool_use": meta.capabilities.tool_use,
            "reasoning": meta.capabilities.reasoning,
            "streaming": meta.capabilities.streaming,
        },
    })
}

async fn list_models(State(adapter): State<Arc<ApiServerAdapter>>) -> impl IntoResponse {
    let models: Vec<serde_json::Value> = adapter
        .handles()
        .model_registry
        .all_models()
        .into_iter()
        .map(|(id, meta)| model_json(id, meta))
        .collect();
    Json(serde_json::json!({ "object": "list", "data": models }))
}

async fn model_options(State(adapter): State<Arc<ApiServerAdapter>>) -> impl IntoResponse {
    let options: Vec<&str> = adapter
        .handles()
        .model_registry
        .all_models()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    Json(serde_json::json!({ "options": options }))
}

pub fn router() -> axum::Router<Arc<ApiServerAdapter>> {
    axum::Router::new()
        .route("/v1/models", get(list_models))
        .route("/api/model/options", get(model_options))
}
