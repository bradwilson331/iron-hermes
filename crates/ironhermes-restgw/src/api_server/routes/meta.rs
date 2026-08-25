//! `/v1/skills`, `/v1/toolsets`, `/v1/capabilities` — Task 2 of Phase 36.7.1
//! Plan 04.
//!
//! `/v1/skills` and `/v1/toolsets` read the live `SkillRegistry`/
//! `ToolRegistry` handles; an absent/empty registry returns an empty
//! collection with a success status (a fresh install with no skills or
//! tools configured is a normal state, not a failure).
//!
//! `/v1/capabilities` is built from [`super::FAMILIES`] — the same data
//! [`super::build_router`] assembles the router from — rather than a
//! hand-maintained parallel dictionary, so it cannot claim a family the
//! router never mounted (D-03). Each family's `features.{name}_api` flag is
//! `false` only for families named in [`NOT_IMPLEMENTED_FAMILIES`] below
//! (O-01: currently just `responses`, since this repository has no response
//! store backing the stateful Responses API) — a client probing
//! capabilities discovers the gap before it calls, rather than at call
//! time. Phase 36.7.1 Plan 09's `not_implemented_features_are_reported_as_unavailable`
//! generalises Plan 08's single-route assertion into one that walks every
//! feature key this map reports, so the claim and the routes' actual
//! not-implemented status can never silently drift apart for ANY family,
//! not just the one that exists today.
//!
//! Plan 09 also records the two port-target endpoints this phase
//! deliberately does not carry, each with a reason, in `omitted_endpoints`
//! — full parity honestly reported means an endpoint is either mounted and
//! working, mounted and explicitly not implemented, or named as omitted
//! with a reason, never simply absent (D-03).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;

use super::super::ApiServerAdapter;

pub const META_PATHS: &[&str] = &["/v1/skills", "/v1/toolsets", "/v1/capabilities"];

/// Families whose mounted routes are explicit not-implemented stubs, not
/// working handlers. Every name here MUST also appear as a `FAMILIES` entry
/// — `capabilities_map_and_router_do_not_drift` (Plan 09) closes that loop
/// at the family-vs-router level; this list only decides which family's
/// `features` flag reads `false`.
const NOT_IMPLEMENTED_FAMILIES: &[&str] = &["responses"];

/// The two port-target endpoints this phase deliberately does not carry
/// (source_facts #5), named with reasons rather than left silently absent.
const OMITTED_ENDPOINTS: &[(&str, &str)] = &[
    (
        "generic per-platform HTTP callback ingress",
        "no in-scope consumer — every platform that would call it is deferred by the parent \
         phase's scope cut",
    ),
    (
        "managed-cron fire webhook",
        "belongs to an external scheduling service specific to the port target, with no \
         counterpart in this deployment",
    ),
];

async fn skills(State(adapter): State<Arc<ApiServerAdapter>>) -> impl IntoResponse {
    let skills: Vec<serde_json::Value> = adapter
        .handles()
        .skill_registry
        .as_ref()
        .map(|registry| {
            registry
                .list()
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "description": s.description,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Json(serde_json::json!({ "skills": skills }))
}

async fn toolsets(State(adapter): State<Arc<ApiServerAdapter>>) -> impl IntoResponse {
    let toolsets = adapter.handles().tool_registry.read().await.list_toolsets();
    Json(serde_json::json!({ "toolsets": toolsets }))
}

async fn capabilities(State(_adapter): State<Arc<ApiServerAdapter>>) -> impl IntoResponse {
    let endpoints: serde_json::Map<String, serde_json::Value> = super::FAMILIES
        .iter()
        .map(|f| (f.name.to_string(), serde_json::json!(f.paths)))
        .collect();

    // Derived from FAMILIES + NOT_IMPLEMENTED_FAMILIES rather than a single
    // hand-maintained flag, so a family added later is reported by
    // construction (`{name}_api: true`) unless explicitly listed as not
    // implemented.
    let features: serde_json::Map<String, serde_json::Value> = super::FAMILIES
        .iter()
        .map(|f| {
            let implemented = !NOT_IMPLEMENTED_FAMILIES.contains(&f.name);
            (format!("{}_api", f.name), serde_json::json!(implemented))
        })
        .collect();

    let omitted_endpoints: Vec<serde_json::Value> = OMITTED_ENDPOINTS
        .iter()
        .map(|(name, reason)| serde_json::json!({ "name": name, "reason": reason }))
        .collect();

    Json(serde_json::json!({
        "endpoints": endpoints,
        "features": features,
        "omitted_endpoints": omitted_endpoints,
    }))
}

pub fn router() -> axum::Router<Arc<ApiServerAdapter>> {
    axum::Router::new()
        .route("/v1/skills", get(skills))
        .route("/v1/toolsets", get(toolsets))
        .route("/v1/capabilities", get(capabilities))
}
