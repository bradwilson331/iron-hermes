//! `/api/jobs*` — scheduled-job CRUD, pause/resume/trigger over the real cron
//! job store. Phase 36.7.1 Plan 09.
//!
//! Projects the existing `ironhermes_cron::JobStore` — the SAME store handle
//! the gateway's own cron tick loop reads (`GatewayRunner::start()`'s
//! `job_store` field, threaded into `ApiServerHandles` at Plan 04's
//! construction site) — rather than inventing a REST-local job list. A job
//! created here is picked up by the scheduler that is already running in
//! this process; mutations persist through the store's own `save()` so they
//! survive a reload.
//!
//! Mirrors `routes::sessions`'s not-found discipline: every per-job verb
//! resolves the path identifier through the store's own `find_job` (id OR
//! name) to a canonical id first, and returns not-found for an identifier
//! that does not resolve — never creating a job implicitly.
//!
//! D-16: the create and patch handlers write only the store's existing job
//! fields (name/prompt/deliver/schedule/skills). No new command or script
//! field is introduced — whatever execution the job store already supports
//! is unchanged by this phase.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use ironhermes_cron::{CronJob, JobStore, JobUpdate, ScheduleParsed, parse_schedule};

use super::super::ApiServerAdapter;

pub const JOB_PATHS: &[&str] = &[
    "/api/jobs",
    "/api/jobs/{id}",
    "/api/jobs/{id}/pause",
    "/api/jobs/{id}/resume",
    "/api/jobs/{id}/run",
];

type ArcJobStore = Arc<Mutex<JobStore>>;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    pub name: String,
    pub prompt: String,
    /// Raw schedule string — the same input `ironhermes_cron::parse_schedule`
    /// accepts on the CLI and the web UI's Schedules editor (e.g. `"every
    /// 2h"`, `"0 9 * * *"`, an RFC3339 timestamp). Parsed and validated
    /// before any store mutation; an unparseable value never persists.
    pub schedule: String,
    #[serde(default)]
    pub deliver: Option<String>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateJobRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub deliver: Option<String>,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct JobsListResponse {
    jobs: Vec<CronJob>,
}

#[derive(Debug, Serialize)]
struct ClientErrorBody {
    error: String,
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn client_error(msg: impl Into<String>) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(ClientErrorBody { error: msg.into() }),
    )
        .into_response()
}

fn schedule_display_of(schedule: &ScheduleParsed) -> String {
    match schedule {
        ScheduleParsed::Once { display, .. } => display.clone(),
        ScheduleParsed::Interval { display, .. } => display.clone(),
        ScheduleParsed::Cron { display, .. } => display.clone(),
    }
}

/// Access the shared job store, or fail with `503` when the gateway was
/// started without one — distinct from `404` (a real store with an unknown
/// identifier) and `500` (a store operation itself failed).
///
/// The error is boxed (`clippy::result_large_err`) — an `axum::Response` is
/// well over 128 bytes, and every call site already matches on `Err`
/// immediately, so boxing costs nothing but keeps the `Result` itself small.
fn require_job_store(adapter: &ApiServerAdapter) -> Result<ArcJobStore, Box<Response>> {
    adapter
        .handles()
        .job_store
        .clone()
        .ok_or_else(|| Box::new(StatusCode::SERVICE_UNAVAILABLE.into_response()))
}

/// Resolve a path identifier (id OR name — `JobStore::find_job`'s own
/// resolution) to the job's canonical id, or `404` when nothing matches.
/// Every mutating call below resolves through this first, because
/// `JobStore::update_job`/`toggle_job`/`remove_job` all key on the exact id
/// and have no name fallback of their own — resolving here is what makes a
/// client's typo indistinguishable from nothing, not a silently-created job.
fn resolve_canonical_id(store: &JobStore, id_or_name: &str) -> Result<String, Box<Response>> {
    store
        .find_job(id_or_name)
        .map(|j| j.id.clone())
        .ok_or_else(|| Box::new(StatusCode::NOT_FOUND.into_response()))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/jobs` — every job in the real store.
async fn list(State(adapter): State<Arc<ApiServerAdapter>>) -> impl IntoResponse {
    let store = match require_job_store(&adapter) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    let store = store.lock().unwrap();
    Json(JobsListResponse {
        jobs: store.list_jobs().to_vec(),
    })
    .into_response()
}

/// `POST /api/jobs` — create a job through the store's own `add_job`.
/// Validates the schedule expression BEFORE any mutation: persisting a job
/// that can never fire produces a client that believes it scheduled work and
/// a store that quietly holds a job the scheduler skips forever.
async fn create(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Json(req): Json<CreateJobRequest>,
) -> impl IntoResponse {
    let store = match require_job_store(&adapter) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };

    if req.name.trim().is_empty() {
        return client_error("name must not be empty");
    }
    if req.prompt.trim().is_empty() {
        return client_error("prompt must not be empty");
    }
    if let Err(e) = ironhermes_cron::scan_cron_prompt(&req.prompt) {
        return client_error(e);
    }
    let parsed = match parse_schedule(&req.schedule) {
        Ok(p) => p,
        Err(e) => return client_error(format!("invalid schedule: {e}")),
    };

    let display = schedule_display_of(&parsed);
    let deliver = req
        .deliver
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| "local".to_string());
    let skills = req.skills.unwrap_or_default();

    let mut store = store.lock().unwrap();
    match store.add_job(req.name, req.prompt, parsed, display, deliver, skills, None) {
        Ok(job) => (StatusCode::OK, Json(job)).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "add_job failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/jobs/{id}` — fetch by id OR name (`JobStore::find_job`'s own
/// resolution), projecting the real record.
async fn fetch(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let store = match require_job_store(&adapter) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    let store = store.lock().unwrap();
    match store.find_job(&id) {
        Some(job) => Json(job.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `PATCH /api/jobs/{id}` — partial update through the store's own
/// `JobUpdate`. Only the fields present in the request are written; a field
/// the client did not mention keeps its stored value (`JobUpdate`'s own
/// `Option<T>` contract — an omitted field is `None`, and `update_job`
/// leaves `None` fields untouched).
async fn update(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateJobRequest>,
) -> impl IntoResponse {
    let store = match require_job_store(&adapter) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    let mut store = store.lock().unwrap();
    let canonical_id = match resolve_canonical_id(&store, &id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    if let Some(name) = &req.name
        && name.trim().is_empty()
    {
        return client_error("name must not be empty");
    }
    if let Some(prompt) = &req.prompt {
        if prompt.trim().is_empty() {
            return client_error("prompt must not be empty");
        }
        if let Err(e) = ironhermes_cron::scan_cron_prompt(prompt) {
            return client_error(e);
        }
    }

    let (schedule, schedule_display) = match &req.schedule {
        Some(raw) => match parse_schedule(raw) {
            Ok(parsed) => {
                let display = schedule_display_of(&parsed);
                (Some(parsed), Some(display))
            }
            Err(e) => return client_error(format!("invalid schedule: {e}")),
        },
        None => (None, None),
    };

    let updates = JobUpdate {
        name: req.name,
        prompt: req.prompt,
        deliver: req.deliver,
        schedule,
        schedule_display,
        skills: req.skills,
    };

    match store.update_job(&canonical_id, updates) {
        Ok(job) => Json(job).into_response(),
        Err(e) => {
            tracing::error!(error = %e, job_id = %canonical_id, "update_job failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `DELETE /api/jobs/{id}` — remove the job. Not-found (not a vacuous
/// success) when the identifier never resolved.
async fn delete_one(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let store = match require_job_store(&adapter) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    let mut store = store.lock().unwrap();
    let canonical_id = match resolve_canonical_id(&store, &id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    match store.remove_job(&canonical_id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, job_id = %canonical_id, "remove_job failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Shared body for `pause`/`resume`: both map onto the store's own
/// `toggle_job(id, enabled)`, differing only in the boolean passed — no
/// second "paused" flag is introduced, so the running scheduler (which reads
/// `enabled`/`state` directly, `get_due_jobs`) observes the same change a
/// client made through this route.
async fn set_enabled(adapter: Arc<ApiServerAdapter>, id: String, enabled: bool) -> Response {
    let store = match require_job_store(&adapter) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    let mut store = store.lock().unwrap();
    let canonical_id = match resolve_canonical_id(&store, &id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    if let Err(e) = store.toggle_job(&canonical_id, enabled) {
        tracing::error!(error = %e, job_id = %canonical_id, "toggle_job failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    match store.get_job(&canonical_id) {
        Some(job) => Json(job.clone()).into_response(),
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `POST /api/jobs/{id}/pause` — `toggle_job(id, false)`.
async fn pause(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    set_enabled(adapter, id, false).await
}

/// `POST /api/jobs/{id}/resume` — `toggle_job(id, true)`.
async fn resume(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    set_enabled(adapter, id, true).await
}

/// `POST /api/jobs/{id}/run` — manual trigger through the store's own
/// `trigger_job`, so a manual run behaves exactly like a trigger issued
/// anywhere else in the system (CLI, web UI).
async fn run_now(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let store = match require_job_store(&adapter) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    let mut store = store.lock().unwrap();
    let canonical_id = match resolve_canonical_id(&store, &id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    if let Err(e) = store.trigger_job(&canonical_id) {
        tracing::error!(error = %e, job_id = %canonical_id, "trigger_job failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    match store.get_job(&canonical_id) {
        Some(job) => Json(job.clone()).into_response(),
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub fn router() -> axum::Router<Arc<ApiServerAdapter>> {
    axum::Router::new()
        .route("/api/jobs", get(list).post(create))
        .route(
            "/api/jobs/{id}",
            get(fetch).patch(update).delete(delete_one),
        )
        .route("/api/jobs/{id}/pause", post(pause))
        .route("/api/jobs/{id}/resume", post(resume))
        .route("/api/jobs/{id}/run", post(run_now))
}
