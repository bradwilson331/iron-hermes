mod app;
mod components;
mod fonts;
mod state;
#[cfg(any(test, feature = "demo"))]
mod mocks;
mod platform;
mod server;

use app::App;

#[cfg(feature = "server")]
#[tokio::main]
async fn main() {
    use dioxus::prelude::*;

    // Initialize shared state at server startup.
    let app_state = server::state::AppState::init()
        .await
        .expect("Failed to initialize AppState");

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
