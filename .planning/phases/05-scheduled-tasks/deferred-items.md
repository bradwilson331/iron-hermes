# Deferred Items — Phase 05 scheduled-tasks

Items discovered during plan execution that are out-of-scope for the current plan and deferred for later cleanup.

## From Plan 05-05 (First-Tick Burst Guard + Reload)

### Pre-existing clippy errors (cargo clippy --workspace --all-targets -- -D warnings)

Discovered during Task 4 workspace regression gate. All exist in files NOT modified by plan 05-05.
Files last touched by earlier phases (06-01, 06-02, 06-03, 07-03, 07.2-04).

**crates/ironhermes-gateway/src/stream_consumer.rs** (AdapterCall enum, phase 06-03):
- Line ~194: `fields chat_id and message_id are never read` (EditMessage variant)
- Line ~198: `fields chat_id and message_id are never read` (EditMessageMarkdown variant)
- Line ~203: `fields chat_id and content are never read` (SendMessage variant)

**crates/ironhermes-tools/src/web_read.rs** (phase 02-ish):
- Line ~307: `items after a test module` (WebReadTool struct defined AFTER #[cfg(test)] mod tests block)

**crates/ironhermes-hooks** (phases 06-01 / 06-02 / 06-03):
- `unused variable: path`
- `function make_queue is never used`
- `field assignment outside of initializer for an instance created with Default::default()` (2 occurrences)

### Resolution Plan

These are pre-existing. Per plan 05-05's out-of-scope rule ("new clippy warnings in code NOT modified by this plan... may be allowed with a noted comment in the summary"), they are logged here rather than fixed in this plan.

**Recommended:** Schedule a follow-up maintenance plan (e.g. phase 05-06 or a standalone housekeeping plan) to:
1. Add `#[allow(dead_code)]` or wire the unused AdapterCall fields into actual consumers
2. Move `WebReadTool` definition above the test module in `web_read.rs`
3. Fix the `ironhermes-hooks` warnings (unused `path`, unused `make_queue`, Default::default assignments)

**Impact:** Workspace clippy gate is currently failing pre-existing. `cargo build` and `cargo test --workspace` both pass (312 tests pass). Clippy-per-crate passes for `ironhermes-cron` (the crate modified by Task 1 + Task 2). Ironhermes-gateway per-crate clippy fails due only to the pre-existing stream_consumer.rs warnings — no new warnings introduced by Task 3.

### Also deferred (Task 1 store.rs dead_code helpers)

`cron_sched` and `once_sched_future` helper functions in `crates/ironhermes-cron/src/store.rs` tests module were flagged by clippy `-D dead_code`. These existed before plan 05-05 (introduced in 05-01). Annotated with `#[allow(dead_code)]` as a minimal fix during Task 4 to keep the `ironhermes-cron` crate clippy-clean. A future plan should either delete them or use them.
