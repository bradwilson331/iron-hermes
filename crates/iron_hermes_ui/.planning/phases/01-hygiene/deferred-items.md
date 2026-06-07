# Phase 1 Deferred Items

Items discovered during Phase 1 execution that are out of scope and deferred to a future phase.

## Pre-existing warnings (out-of-scope per executor scope-boundary rules)

### unused_imports in src/main.rs (Plan 01-02 artifact)

- **File:** src/main.rs:1
- **Warning:** `unused import: dioxus::prelude::*`
- **Discovered:** Plan 01-03 cargo build output
- **Origin:** Module split in Plan 01-02 left a dangling top-level import in `main.rs` (likely a stub or boilerplate line; the actual `App` body now lives in `src/app.rs`).
- **Why deferred:** `src/main.rs` is already committed by Plan 01-02. This warning is not directly caused by Plan 01-03 (Cargo.lock generation). Per executor scope-boundary, do not fix unrelated pre-existing lint issues.
- **Recommended action:** Address in Phase 2 (when `main.rs` is touched again for app wiring) or in a dedicated Phase 1 follow-up plan.
