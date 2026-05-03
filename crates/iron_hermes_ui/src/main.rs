mod app;
mod components;
mod fonts;
mod state;
mod mocks;
mod platform;

use app::App;

fn main() {
    dioxus::launch(App);
}
