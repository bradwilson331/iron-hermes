# `ironhermes-cli` Integration Tests

Most TUI tests in this directory are gated behind the `test-support` feature.
Run the full suite with:

```bash
cargo test -p ironhermes-cli --features test-support
```

## Table of Contents

- [Phase 36.17.3 D-12 negative-control verification](#phase-36173-d-12-negative-control-verification)

## Phase 36.17.3 D-12 negative-control verification

Phase 36.17.3 ships an integration test
(`tui_queue_regression_negative_control` in `tui_queue_drain.rs`) that asserts
the `/queue` slash command pushes onto the shared FIFO queue rather than
pre-populating the textarea. Decision D-12 mandates that the test must **fail**
when run against the pre-fix textarea-prepopulate handler — otherwise the test
is not a real regression contract.

Perform the rebase verification once, before the final merge of Phase 36.17.3:

1. Before final merge of Phase 36.17.3, run `git stash`.
2. `git checkout <pre-fix-sha> -- crates/ironhermes-cli/src/tui_rata/commands.rs`
   to restore the pre-fix textarea-prepopulate `/queue` handler.
3. Run `cargo test -p ironhermes-cli --features test-support tui_queue_regression_negative_control -- --ignored`
   and confirm the test **FAILS** (D-12 contract).
4. Restore current code via `git checkout HEAD -- crates/ironhermes-cli/src/tui_rata/commands.rs`
   and `git stash pop`.
5. Record the result (PASS-against-new / FAIL-against-old) in the phase
   SUMMARY before merging.

The `<pre-fix-sha>` is the commit immediately before Phase 36.17.3 Plan 05
landed the production `/queue` rewrite — typically the tip of the develop
branch at the moment Phase 36.17.3 was opened. Use `git log` on
`crates/ironhermes-cli/src/tui_rata/commands.rs` to identify it.
