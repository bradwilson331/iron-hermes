mod app;
mod components;
mod fonts;
// Phase 36.3.7.11 Plan 02: client-side kanban utilities (transition
// validator + drag-blocked hint copy). The server-side `kanban_api`
// imports `kanban::transitions::is_drag_allowed` for D-14 defense-in-depth.
mod kanban;
mod mocks;
mod platform;
mod protocol;
mod server;
mod state;
mod ui_prefs;

use app::App;

#[cfg(feature = "server")]
#[tokio::main]
async fn main() {
    use dioxus::prelude::*;
    use tower_http::trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer};
    use tracing::Level;

    // Install the global tracing subscriber: stdout console + daily-rolling
    // web.log (app/agent) + daily-rolling web-access.log (HTTP access). Both
    // WorkerGuards MUST be held for the lifetime of axum::serve(...).await —
    // dropping them shuts down the non-blocking writer threads and any
    // buffered log lines are lost. The underscore prefix silences the
    // unused-variable lint while extending the lifetime to the end of main().
    let (_web_log_guard, _access_log_guard) = server::logging::install_web_logger_subscriber();

    // Load ~/.ironhermes/.env so OPENROUTER_API_KEY / ANTHROPIC_API_KEY etc.
    // are available to the embedded agent — mirrors what the CLI does in main.rs.
    let env_path = ironhermes_core::config::Config::env_path();
    if env_path.exists() {
        dotenvy::from_path(&env_path).ok();
    }

    // Initialize shared state at server startup.
    let app_state = server::state::AppState::init()
        .await
        .expect("Failed to initialize AppState");
    server::state::install_global_app_state(app_state.clone())
        .expect("Failed to install global AppState");

    let address = dioxus::cli_config::fullstack_address_or_localhost();

    // Explicit Level::INFO on both request and response callbacks is mandatory:
    // tower_http::trace::DefaultOnRequest / DefaultOnResponse default to
    // Level::DEBUG, which the locked access-log filter `tower_http::trace=info`
    // (server/logging.rs) would silently drop — leaving web-access.log empty.
    // See RESEARCH.md Q2 / Pitfall 1.
    let trace_layer = TraceLayer::new_for_http()
        .on_request(DefaultOnRequest::new().level(Level::INFO))
        .on_response(DefaultOnResponse::new().level(Level::INFO));

    let router = axum::Router::new()
        .serve_dioxus_application(ServeConfig::new(), App)
        .layer(axum::Extension(app_state))
        .layer(trace_layer)
        .into_make_service();

    let listener = tokio::net::TcpListener::bind(address).await.unwrap();

    // Deterministic startup marker — fires before the server starts accepting
    // traffic so web.log has at least one INFO line on every boot, even when
    // the agent loop never runs (e.g. brief UAT lifecycle). Without this,
    // a short-lived process may only emit lower-priority events that match the
    // filter, leaving web.log empty by happenstance.
    tracing::info!(target: "iron_hermes_ui", "server bound to {address}");

    // Graceful shutdown is load-bearing for file logging: SIGTERM / Ctrl-C
    // signal axum to complete in-flight requests and return, which lets
    // main() unwind and drop both _web_log_guard and _access_log_guard. The
    // guard Drop impls synchronously join the non-blocking writer threads,
    // flushing any buffered log lines to disk. Without with_graceful_shutdown,
    // the runtime is hard-killed before main() returns — guards never drop —
    // and the last batch of buffered tracing events is silently lost.
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

// Resolves when the process receives SIGTERM (POSIX container shutdown) or
// SIGINT (terminal Ctrl-C). Wired into axum's with_graceful_shutdown so the
// WorkerGuards held in main() get a chance to flush before process exit.
#[cfg(feature = "server")]
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(App);
}
