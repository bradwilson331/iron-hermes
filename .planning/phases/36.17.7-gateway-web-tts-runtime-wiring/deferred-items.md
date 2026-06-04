# Phase 36.17.7 — Deferred Items

Items discovered during phase execution that are out of scope per the Scope Boundary rule.

## Plan 01 (foundations) discoveries

### Pre-existing clippy errors in `crates/ironhermes-core/src/tts.rs`

- **Discovered:** 2026-06-04 during Plan 01 verify_plan check 5 (`cargo clippy -p ironhermes-agent -p ironhermes-tools --tests -- -D warnings`).
- **File:** `crates/ironhermes-core/src/tts.rs` (last modified in `b9d5b018`, Phase 36.17.5-01).
- **Issue:** Multiple `clippy::ptr_arg` errors — function signatures like `output_path: &PathBuf` should be `&Path`. Example at line 52: `async fn synthesize(&self, text: &str, output_path: &PathBuf) -> anyhow::Result<PathBuf>;`.
- **Total errors:** ~17 (clippy reports "17 previous errors" when run with `-D warnings`).
- **Plan 01 status:** Out of scope — Plan 01 did not modify this file. Per the Scope Boundary rule, pre-existing lint errors in unrelated files are deferred.
- **Recommended fix:** A single-pass clippy clean-up phase across `ironhermes-core` replacing `&PathBuf` parameter types with `&Path` (clippy's suggestion). No behavior change expected.
- **Where this surfaces:** Any clippy run with `-D warnings` against `ironhermes-core` or any dependent crate. Plan 01's verify_plan check 5 fails on this; the failure is documented in `36.17.7-01-SUMMARY.md` "Deferred Issues" section.
