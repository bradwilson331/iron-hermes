//! The three health paths: `/health`, `/health/detailed`, `/v1/health`.
//! Task 1 of Phase 36.7.1 Plan 04.

use std::sync::Arc;

use axum::Json;
use axum::response::IntoResponse;
use axum::routing::get;

use super::super::ApiServerAdapter;

pub const HEALTH_PATHS: &[&str] = &["/health", "/health/detailed", "/v1/health"];

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "ironhermes-api-server",
    }))
}

async fn health_detailed() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "ironhermes-api-server",
        "detail": "listener is bound and serving",
    }))
}

pub fn router() -> axum::Router<Arc<ApiServerAdapter>> {
    axum::Router::new()
        .route("/health", get(health))
        .route("/health/detailed", get(health_detailed))
        .route("/v1/health", get(health))
}
