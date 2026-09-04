// Phase 26.2.1 module layout, updated 49.4-02: the pre-26.2.1 Warp-style
// shell (`shell_legacy` + the `warp_hermes` composition root) and its
// opt-in `legacy-shell` Cargo feature have been removed (folded todo
// 2026-05-29). `hermes_app` is the crate's only root component — there is
// no compile-time branch selecting a different one.

pub mod hermes_app;
