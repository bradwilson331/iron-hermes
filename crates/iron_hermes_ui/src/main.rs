mod app;
mod components;
mod fonts;
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
    axum::serve(listener, router).await.unwrap();
}

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(App);
}
