//! IronHermes CLI library — exposes modules for integration test access.

pub mod skills_cmd;

// `memory_setup` is intentionally NOT re-exported from the library crate.
// It references `crate::Cli` which lives in `main.rs` (the binary crate),
// so the module is compiled as part of the binary only. The integration
// tests exercise the factory + MemoryManager path directly rather than
// re-entering the binary's Cli surface.
