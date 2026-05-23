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

pub mod delivery;
pub mod prompt_builder;
pub mod runner;
pub mod script_runner;
pub mod tick_loop;
pub mod timeout;

pub use delivery::dispatch_all_targets;
pub use runner::{CronRunnerContext, run_cron_job};
pub use tick_loop::{prepare_mcp_for_tick, run_tick_loop};

#[cfg(test)]
pub(crate) mod test_util {
    use std::sync::{Mutex, OnceLock};

    // Serializes tests that mutate process-wide env vars (IRONHERMES_HOME,
    // BASH_PATH, PYTHON_PATH, IRONHERMES_CRON_SCRIPT_TIMEOUT). Both
    // `prompt_builder` and `script_runner` tests share this lock.
    pub fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }
}

// Per-job delivery context. Propagated to the agent's tool layer
// (e.g. `send_message`) without env-var mutation. Survives `.await`
// within the same tokio task; explicit `scope()` wrapping is required
// for any `tokio::spawn` children.
tokio::task_local! {
    pub static CRON_AUTO_DELIVER_PLATFORM: String;
    pub static CRON_AUTO_DELIVER_CHAT_ID: String;
    pub static CRON_AUTO_DELIVER_THREAD_ID: String;
}
