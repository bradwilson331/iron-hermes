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

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

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

    let router = axum::Router::new()
        .serve_dioxus_application(ServeConfig::new(), App)
        .layer(axum::Extension(app_state))
        .into_make_service();

    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    axum::serve(listener, router).await.unwrap();
}

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(App);
}
