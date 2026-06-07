//! Tracing subscriber install for the `iron_hermes_ui` server binary.
//!
//! Phase 36.17: mirrors `crates/ironhermes-cli/src/tui_rata/event_loop.rs`'s
//! `install_tui_logger_subscriber` pattern, scaled from one file appender to
//! two (`web.log` for app/agent events, `web-access.log` for HTTP access).

use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, registry, EnvFilter};

/// Install the global tracing subscriber for the `iron_hermes_ui` server.
///
/// Composes three layers on a single `tracing_subscriber::registry()`:
///
/// 1. **Console (stdout)** — keeps default ANSI; filter honors `RUST_LOG` with
///    a default of `ironhermes=info,iron_hermes_ui=info` (D-04 / D-06 / D-08 /
///    D-09).
/// 2. **Web file** — daily-rolling `$IRONHERMES_HOME/logs/web.log.YYYY-MM-DD`
///    (D-01 / D-02 / D-15). Non-blocking writer (D-02), ANSI stripped (D-03),
///    same `RUST_LOG`-aware filter as the console layer (D-04 / D-06).
/// 3. **Access file** — daily-rolling
///    `$IRONHERMES_HOME/logs/web-access.log.YYYY-MM-DD` (D-01 / D-02 / D-15).
///    Non-blocking writer (D-02), ANSI stripped (D-03), fixed per-layer filter
///    `tower_http::trace=info` independent of `RUST_LOG` (D-05 / D-06).
///
/// Registry composition uses `try_init()` so a double-install (e.g. in tests
/// or when a prior subscriber is already global) is a silent no-op rather
/// than a panic (D-18).
///
/// Returns the two `WorkerGuard`s for the file appenders. Both MUST be held
/// by the caller (typically `main()`) for the lifetime of
/// `axum::serve(...).await` — dropping a guard shuts down its writer thread
/// and any buffered log lines are lost (D-17).
///
/// If `$IRONHERMES_HOME/logs/` can't be created (e.g. read-only home in
/// tests), the function silently falls back to installing only the console
/// layer and returns `(None, None)`. This matches the TUI fallback at
/// `crates/ironhermes-cli/src/tui_rata/event_loop.rs:88-100` (D-16) — the
/// server stays usable, just without file logging.
///
/// `tracing::Level` is imported so callers / docs reference it consistently
/// with the matching `TraceLayer` setup in `main.rs` (Plan 03), which uses
/// `DefaultOnRequest::new().level(Level::INFO)` to ensure events match the
/// fixed `tower_http::trace=info` filter on the access file layer.
pub fn install_web_logger_subscriber() -> (Option<WorkerGuard>, Option<WorkerGuard>) {
    // Mention Level in code so the import isn't dead — the type doubles as a
    // compile-time anchor for the documented "events emit at INFO" contract
    // that main.rs's TraceLayer setup relies on.
    let _info_level: Level = Level::INFO;

    let log_dir = ironhermes_core::constants::get_hermes_home().join("logs");

    let app_filter = || {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("ironhermes=info,iron_hermes_ui=info"))
    };

    let console_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_target(true)
        .with_filter(app_filter());

    match std::fs::create_dir_all(&log_dir) {
        Ok(_) => {
            let web_appender = tracing_appender::rolling::daily(&log_dir, "web.log");
            let (web_nb, web_guard) = tracing_appender::non_blocking(web_appender);
            let access_appender = tracing_appender::rolling::daily(&log_dir, "web-access.log");
            let (access_nb, access_guard) = tracing_appender::non_blocking(access_appender);

            let web_file_layer = fmt::layer()
                .with_writer(web_nb)
                .with_ansi(false)
                .with_target(true)
                .with_filter(app_filter());

            let access_file_layer = fmt::layer()
                .with_writer(access_nb)
                .with_ansi(false)
                .with_target(true)
                .with_filter(EnvFilter::new("tower_http::trace=info"));

            let _ = registry()
                .with(console_layer)
                .with(web_file_layer)
                .with(access_file_layer)
                .try_init();

            (Some(web_guard), Some(access_guard))
        }
        Err(_) => {
            let _ = registry().with(console_layer).try_init();
            (None, None)
        }
    }
}
