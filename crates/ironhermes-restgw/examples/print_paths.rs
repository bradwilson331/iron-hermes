//! Phase 49.1 Plan 07 (D-05/D-15): prints every path
//! `ironhermes_restgw::api_server::routes::all_registered_paths()` returns,
//! one per line, so `capture/probe-07-authed-surface.sh` can drive its
//! restgw probe list from the router's own source of truth instead of a
//! hand-maintained list — a route added to `FAMILIES`
//! (`api_server/routes/mod.rs`) is picked up here automatically, with no
//! edit to this file or the probe script.
//!
//! Usage: `cargo run -p ironhermes-restgw --example print_paths -- --print-paths`
//! (the `--print-paths` flag is accepted and required for clarity at the
//! call site; the example has no other mode, so omitting it is also fine —
//! any unrecognized argument is ignored rather than treated as an error, so
//! this stays a trivial, dependency-free print utility.)

fn main() {
    for path in ironhermes_restgw::api_server::routes::all_registered_paths() {
        println!("{path}");
    }
}
