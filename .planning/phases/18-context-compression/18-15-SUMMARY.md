---
phase: 18-context-compression
plan: 15
subsystem: memory
tags: [gap-closure, memory-provider, factory, deprecation-removal, regression-test]
gap_closure: true
gap_id: GAP-18-UAT-02
dependency_graph:
  requires: []
  provides: [memory-factory-migration, deprecated-symbol-removal, factory-regression-tests]
  affects: [crates/ironhermes-cli, crates/ironhermes-core, crates/ironhermes-agent]
tech_stack:
  added: []
  patterns: [feature-gated factory, warn-on-error disk-load, cfg(test) regression guard]
key_files:
  created: []
  modified:
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-core/src/memory_provider.rs
    - crates/ironhermes-core/src/lib.rs
    - crates/ironhermes-agent/src/memory/factory.rs
decisions:
  - "Disk-load responsibility moved into agent factory's file branch (not gateway), keeping run_gateway to a single factory call"
  - "Used .err().unwrap() instead of .unwrap_err() / .expect_err() to extract error from Result where T (Arc<Mutex<dyn MemoryProvider>>) lacks Debug"
metrics:
  duration_minutes: 3
  completed_date: "2026-04-16"
  tasks_completed: 3
  files_modified: 4
---

# Phase 18 Plan 15: Memory Factory Migration and Deprecation Removal Summary

**One-liner:** Migrated `run_gateway` from the buggy core factory to the feature-gated agent factory, deleted the deprecated `ironhermes_core::build_memory_provider` symbol, and added a four-test regression suite guarding the sqlite provider path — unblocking UAT Test 2.

## Before / After: run_gateway diff summary

**Before (broken):**
```rust
// line 5 — imported deprecated symbol
use ironhermes_core::{..., build_memory_provider};

// lines 609-618 in run_gateway — two-step antipattern
let _ = build_memory_provider(&config.memory)?;   // preflight: always bailed on sqlite
let memory_dir = ironhermes_core::get_hermes_home().join("memories");
let mut store = MemoryStore::new(memory_dir);
if let Err(e) = store.load_from_disk() { warn!("..."); }
let memory_store: Arc<Mutex<dyn MemoryProvider + Send>> = Arc::new(Mutex::new(store));
// ^^^ hardcoded file store regardless of config.memory.provider
```

The deprecated `ironhermes_core::build_memory_provider` (lines 135-159 in memory_provider.rs) hardcoded a non-feature-gated bail for `sqlite`/`grafeo`/`duckdb` with the message "requires a feature flag that is not enabled. Available providers: file" — ignoring `--features memory-sqlite` entirely. Even if the preflight had passed, lines 612-618 would still have constructed a hardcoded `MemoryStore` regardless of `config.memory.provider`.

**After (fixed):**
```rust
// line 5 — deprecated import removed
use ironhermes_core::{ChatMessage, Config, MemoryProvider, ProviderResolver, SkillRegistry};

// run_gateway — single authoritative call
let memory_store: Arc<Mutex<dyn MemoryProvider + Send>> =
    ironhermes_agent::memory::factory::build_memory_provider(&config.memory)?;
```

The agent factory's `"file"` branch was also updated to call `store.load_from_disk()` (warn-on-error) before wrapping, so the gateway needs no special-case disk-load logic.

## Grep evidence: zero remaining references to deprecated symbol

```
$ grep -rn "ironhermes_core::build_memory_provider" crates/ --include="*.rs"
(no output)

$ grep -rn "build_memory_provider" crates/ironhermes-core/ --include="*.rs"
(no output)

$ grep -rn "build_memory_provider" crates/ironhermes-cli/ --include="*.rs"
crates/ironhermes-cli/src/main.rs:613:        ironhermes_agent::memory::factory::build_memory_provider(&config.memory)?;
```

Exactly one match in CLI — the corrected call site referencing the agent factory path.

## Test output

### Without --features memory-sqlite (3 factory tests run)

```
test memory::factory::tests::sqlite_provider_without_feature_returns_err_naming_feature ... ok
test memory::factory::tests::file_provider_returns_ok ... ok
test memory::factory::tests::unknown_provider_returns_err_with_message ... ok

test result: ok. 189 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
```

### With --features memory-sqlite (3 factory tests run, sqlite-ok test active)

```
test memory::factory::tests::file_provider_returns_ok ... ok
test memory::factory::tests::unknown_provider_returns_err_with_message ... ok
test memory::factory::tests::sqlite_provider_with_feature_returns_ok ... ok

test result: ok. 189 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
```

Both `cargo build --workspace` and `cargo build -p ironhermes-cli --features memory-sqlite` complete with no deprecation warnings.

## UAT Test 2 unblocked

**GAP-18-UAT-02 is now closed.** The gateway's memory construction path now routes through `ironhermes_agent::memory::factory::build_memory_provider`, which correctly branches on the `memory-sqlite` feature flag. A binary built with `--features memory-sqlite` will accept `memory.provider=sqlite` and construct a `SqliteMemoryProvider` without bailing. The old "requires a feature flag that is not enabled. Available providers: file" error will no longer appear when the feature is present.

UAT Test 2 can now be re-verified by the human tester:
```bash
cargo run -p ironhermes-cli --features memory-sqlite -- gateway --token $TELEGRAM_TOKEN
# with config.memory.provider = "sqlite"
# Expected: gateway boots, memory tool uses SqliteMemoryProvider
```

UAT Gaps.missing items satisfied:
- Item 1: Switch run_gateway to call `ironhermes_agent::memory::factory::build_memory_provider` — DONE
- Item 2: Remove the #[deprecated] fallback from ironhermes-core — DONE (symbol compiler-removed)
- Item 3: REPL path confirmed not affected (REPL never constructs a memory provider — UAT Test 1 passes unchanged)
- Item 4: Regression test for sqlite provider under `cfg(feature = "memory-sqlite")` — DONE

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] `.unwrap_err()` and `.expect_err()` require `T: Debug`**
- **Found during:** Task 3 (first test compile attempt)
- **Issue:** `Arc<Mutex<dyn MemoryProvider + Send>>` does not implement `Debug`, so `Result::unwrap_err()` and `Result::expect_err()` both require `T: Debug` — compile error E0277
- **Fix:** Used `.err().unwrap()` to extract the error value from the `Result` — `Option::unwrap()` has no `T: Debug` bound
- **Files modified:** `crates/ironhermes-agent/src/memory/factory.rs`
- **Commit:** 6524155

## Self-Check: PASSED

Files created/modified:
- FOUND: crates/ironhermes-cli/src/main.rs
- FOUND: crates/ironhermes-core/src/memory_provider.rs
- FOUND: crates/ironhermes-core/src/lib.rs
- FOUND: crates/ironhermes-agent/src/memory/factory.rs

Commits:
- FOUND: 30c9dc7 (Task 1 — migrate gateway)
- FOUND: 317e6b5 (Task 2 — delete deprecated factory)
- FOUND: 6524155 (Task 3 — regression tests)
