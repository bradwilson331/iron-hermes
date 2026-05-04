mod app;
mod components;
mod fonts;
mod state;
mod mocks;
mod platform;
#[cfg(feature = "server")]
mod server;

use app::App;

#[cfg(feature = "server")]
#[tokio::main]
async fn main() {
    use dioxus::prelude::*;

    let address = dioxus::cli_config::fullstack_address_or_localhost();

    let router = axum::Router::new()
        .serve_dioxus_application(ServeConfig::new(), App)
        .into_make_service();

    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    axum::serve(listener, router).await.unwrap();
}

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(App);
}
