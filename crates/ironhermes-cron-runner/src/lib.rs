//! Agent execution runner for IronHermes cron jobs.
//!
//! This crate owns end-to-end execution of a due `CronJob`: prompt
//! assembly, sandboxed script execution, agent loop invocation,
//! timeout enforcement, and per-target delivery dispatch.
//!
//! Plan 32.1-05a lands the crate skeleton + `script_runner`.
//! Plan 32.1-05b lands `prompt_builder` and `timeout`.
//! Plan 32.1-06 lands the orchestration modules (`runner`,
//! `delivery`, `tick_loop`) and the `run_cron_job` public entry point.

pub mod prompt_builder;
pub mod script_runner;
pub mod timeout;

// Per-job delivery context. Propagated to the agent's tool layer
// (e.g. `send_message`) without env-var mutation. Survives `.await`
// within the same tokio task; explicit `scope()` wrapping is required
// for any `tokio::spawn` children.
tokio::task_local! {
    pub static CRON_AUTO_DELIVER_PLATFORM: String;
    pub static CRON_AUTO_DELIVER_CHAT_ID: String;
    pub static CRON_AUTO_DELIVER_THREAD_ID: String;
}
