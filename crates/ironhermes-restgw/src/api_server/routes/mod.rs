//! Router assembly point for [`crate::api_server`] — one function per route
//! family returning its own `axum::Router`, merged here. Kept as a family
//! split even though only three families exist as of Task 2: each later
//! plan in this phase (runs, sessions, chat, jobs) adds one family, and a
//! single assembly point is what makes their additions two-line edits
//! rather than restructurings.
//!
//! [`FAMILIES`] is the single source of truth both the bearer-auth test
//! (`every_route_requires_the_bearer_key` — enumerate, don't spot-check) and
//! the capabilities endpoint ([`meta::capabilities`]) read from, so the
//! capabilities map cannot claim a family the router never mounted (D-03 —
//! the capabilities-map-drift risk called out in this plan's `<context>`).

pub mod chat;
pub mod health;
pub mod jobs;
pub mod meta;
pub mod models;
pub mod responses;
pub mod runs;
pub mod sessions;

use std::sync::Arc;

use axum::Router;
use axum::middleware::from_fn_with_state;

use super::ApiServerAdapter;

/// One route family: a name (surfaced in the capabilities endpoint) and the
/// exact path list it registers.
pub struct RouteFamily {
    pub name: &'static str,
    pub paths: &'static [&'static str],
}

/// The router's own registered-path source of truth. Extending this array
/// (and merging the new family's `router()` below) is the whole edit a
/// later plan needs to add a route family — the bearer-auth layer and the
/// capabilities endpoint both inherit the addition automatically.
pub const FAMILIES: &[RouteFamily] = &[
    RouteFamily {
        name: "health",
        paths: health::HEALTH_PATHS,
    },
    RouteFamily {
        name: "models",
        paths: models::MODEL_PATHS,
    },
    RouteFamily {
        name: "meta",
        paths: meta::META_PATHS,
    },
    RouteFamily {
        name: "runs",
        paths: runs::RUN_PATHS,
    },
    RouteFamily {
        name: "sessions",
        paths: sessions::SESSION_PATHS,
    },
    RouteFamily {
        name: "chat",
        paths: chat::CHAT_PATHS,
    },
    RouteFamily {
        name: "responses",
        paths: responses::RESPONSES_PATHS,
    },
    RouteFamily {
        name: "jobs",
        paths: jobs::JOB_PATHS,
    },
];

/// Every path registered on this adapter's router, in family order. Used by
/// `every_route_requires_the_bearer_key`/`correct_bearer_is_admitted` to
/// drive every route rather than a hand-picked subset, and by
/// [`meta::capabilities`] to report the mounted surface honestly.
pub fn all_registered_paths() -> Vec<&'static str> {
    FAMILIES.iter().flat_map(|f| f.paths.iter().copied()).collect()
}

/// Assemble the full router: merge every family, install the adapter as
/// shared state, then apply the bearer-auth layer over the FULLY MERGED
/// router (D-06 — a router-wide layer, not a per-handler call, so a route
/// added later inherits the gate by construction).
pub fn build_router(adapter: Arc<ApiServerAdapter>) -> Router {
    let router: Router<Arc<ApiServerAdapter>> = Router::new()
        .merge(health::router())
        .merge(models::router())
        .merge(meta::router())
        .merge(runs::router())
        .merge(sessions::router())
        .merge(chat::router())
        .merge(responses::router())
        .merge(jobs::router());

    router
        .with_state(adapter.clone())
        .layer(from_fn_with_state(adapter, super::auth::require_bearer))
}
