---
phase: 05-scheduled-tasks
plan: "04"
subsystem: cron-cli
tags: [cron, cli, gap-closure, ui-spec, sched-02]
requirements: [SCHED-02]
requirements_addressed: [SCHED-02]
dependency_graph:
  requires:
    - "ironhermes_cron::JobStore::find_job (case-insensitive id-or-name lookup)"
    - "ironhermes_cron::CronJob struct and JobState enum"
    - "colored crate for ANSI styling (bold/cyan/yellow/dimmed/green/red)"
  provides:
    - "ironhermes cron get {id|name} CLI subcommand (UI-SPEC line 182)"
    - "render_job_details(&CronJob) -> String pure renderer for test harnesses"
  affects:
    - "crates/ironhermes-cli/src/cron.rs (Get variant + cmd_get + helper)"
    - "crates/ironhermes-cli/Cargo.toml (tempfile dev-dependency)"
tech_stack:
  added:
    - "tempfile 3 (dev-dependency only, already used by ironhermes-cron)"
  patterns:
    - "pure-renderer-over-println pattern (writeln! into String, print! at call site)"
    - "anyhow ? propagation for not-found -> non-zero CLI exit"
    - "ironhermes_cron::JobState match reused from cmd_list for color consistency"
key_files:
  created: []
  modified:
    - "crates/ironhermes-cli/src/cron.rs"
    - "crates/ironhermes-cli/Cargo.toml"
decisions:
  - "Used pure render_job_details String-returning helper instead of refactoring cmd_get to take &JobStore — keeps the public cmd_get(job_id: String) -> Result<()> signature clean for the dispatch arm while still providing a testable rendering surface."
  - "Reused cmd_list/cmd_status color primitives verbatim (bold cyan header, dimmed labels, yellow name, state color matching) instead of defining a new palette — UI-SPEC visual consistency is load-bearing for UAT test 7."
  - "Second unit test asserts JobStore::find_job behavior + error-message shape rather than invoking cmd_get directly, because cmd_get internally calls open_store() which would hit the real ~/.ironhermes/cron directory. This keeps tests hermetic via tempfile without needing a hermes-home override."
metrics:
  duration: "~8m"
  tasks_completed: 1
  commits: 2
  files_modified: 2
  completed_date: "2026-04-09"
---

# Phase 05 Plan 04: Cron Get Subcommand Summary

One-liner: Closes UAT Gap 1 by wiring the missing `ironhermes cron get {id}` CLI subcommand through `JobStore::find_job` to a pure String-returning renderer that emits all UI-SPEC line 182 fields with sibling-command color consistency.

## Objective Achieved

UAT test 7 flagged `cron get` as missing from the CLI despite UI-SPEC line 182 defining its output contract. The store layer (`JobStore::find_job`) already supported case-insensitive id-or-name lookup (used by pause/resume/edit/remove/run). Only the CLI surface — the `Get` enum variant, the dispatch arm, and the `cmd_get` handler — was absent. This plan adds all three plus a testable `render_job_details` helper and two unit tests, unblocking SCHED-02 verification.

## What Was Built

### 1. `Get` variant on `CronCommands` enum

Location: `crates/ironhermes-cli/src/cron.rs` (line ~42, between `Create` and `Edit` — matches UI-SPEC reading order).

```rust
/// Show full details for a specific job
Get {
    /// Job ID or name (case-insensitive)
    job_id: String,
},
```

### 2. Dispatch arm in `handle_cron_command`

Location: `crates/ironhermes-cli/src/cron.rs` line 105.

```rust
CronCommands::Get { job_id } => cmd_get(job_id),
```

Synchronous (not async) — mirrors `cmd_pause`/`cmd_resume`/`cmd_edit` pattern.

### 3. `cmd_get` handler

Location: `crates/ironhermes-cli/src/cron.rs` line 258.

```rust
fn cmd_get(job_id: String) -> Result<()> {
    let store = open_store()?;
    let job = store
        .find_job(&job_id)
        .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;
    print!("{}", render_job_details(job));
    Ok(())
}
```

- Uses `JobStore::find_job` for id-first, name-second (case-insensitive) lookup.
- Maps `None` to `anyhow!("Job not found: {id}")` — propagates via `?` for non-zero CLI exit.
- Delegates all rendering to `render_job_details` so the logic is unit-testable.

### 4. `render_job_details` pure renderer

Location: `crates/ironhermes-cli/src/cron.rs` line 270.

Signature: `fn render_job_details(job: &CronJob) -> String`

Emits these lines in order (all with dimmed labels in a 14-char column):

| Line        | Field         | Styling                                                   |
| ----------- | ------------- | --------------------------------------------------------- |
| Header      | `Cron Job`    | bold cyan                                                 |
| Divider     | `─` × 50      | unstyled                                                  |
| `Name:`     | job.name      | yellow value                                              |
| `ID:`       | job.id        | dimmed value                                              |
| `Schedule:` | schedule_display | unstyled                                               |
| `Prompt:`   | job.prompt    | unstyled (multi-line OK, no truncation)                   |
| `Deliver:`  | job.deliver   | unstyled                                                  |
| `Skills:`   | joined list   | comma-joined, or `none` dimmed if empty                   |
| `State:`    | state         | green (scheduled+enabled), yellow (disabled/paused), dimmed (completed) |
| `Enabled:`  | bool          | unstyled                                                  |
| `Created:`  | timestamp     | `%Y-%m-%d %H:%M UTC`                                      |
| `Next run:` | timestamp     | `%Y-%m-%d %H:%M UTC` or `never` dimmed                    |
| `Last run:` | timestamp     | `%Y-%m-%d %H:%M UTC` or `never` dimmed                    |
| `Last status:` | job.last_status | only if present                                     |
| `Last error:` | job.last_error | only if present, red                                  |

Uses `std::fmt::Write as FmtWrite` + `writeln!` into a `String` (trailing newline on each line). `cmd_get` prints with `print!` (not `println!`) since the String already ends in `\n`.

### 5. Unit tests

Added to new `#[cfg(test)] mod tests { ... }` block at the bottom of `cron.rs`.

**Test 1: `render_job_details_contains_all_fields`** (line 657)
- Creates a tempdir-backed `JobStore` via `tempfile::tempdir()` + `JobStore::open(dir.path().join("cron"))`.
- Seeds an interval job (`every 5m`, skill `focus`, deliver `local`, prompt `say hello`, name `test-render`).
- Asserts `render_job_details(&job)` contains: name, id, `every 5m`, `say hello`, `local`, `focus`, and `Next run:`.

**Test 2: `cmd_get_not_found_returns_error`** (line 686)
- Creates an empty tempdir-backed `JobStore`.
- Asserts `store.find_job("ghost")` returns `None`.
- Asserts the error message shape `Job not found: ghost` contains `"Job not found"`.

Second test is deliberately indirect (validates the lookup + error shape rather than calling `cmd_get` directly) because `cmd_get` invokes `open_store()` which targets the real `~/.ironhermes/cron` — documented as a trade-off in the decisions list above.

### 6. Dev-dependency addition

`crates/ironhermes-cli/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = "3"
```

Mirrors the version ironhermes-cron already uses (not routed through workspace.dependencies since the workspace root doesn't declare tempfile).

## Verification

### Automated

```text
$ cargo build -p ironhermes-cli
    Finished `dev` profile [unoptimized + debuginfo] target(s)

$ cargo test -p ironhermes-cli cron
running 2 tests
test cron::tests::cmd_get_not_found_returns_error ... ok
test cron::tests::render_job_details_contains_all_fields ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### Manual (CLI help visibility)

```text
$ cargo run -p ironhermes-cli -- cron --help
Commands:
  list    List all scheduled jobs
  create  Create a new scheduled job
  get     Show full details for a specific job   <-- NEW
  edit    Edit an existing job
  pause   Pause a job
  resume  Resume a paused job
  ...
```

### Acceptance criteria checklist

- [x] `Get { job_id: String }` variant present in `CronCommands`
- [x] `CronCommands::Get { job_id } => cmd_get(job_id)` dispatch arm wired
- [x] `fn cmd_get(job_id: String) -> Result<()>` handler defined
- [x] `fn render_job_details(job: &CronJob) -> String` helper defined
- [x] `fn render_job_details_contains_all_fields` test passes
- [x] `fn cmd_get_not_found_returns_error` test passes
- [x] `cargo build -p ironhermes-cli` exits 0
- [x] `cargo test -p ironhermes-cli cron` exits 0, both tests `ok`
- [x] `cargo run -p ironhermes-cli -- cron --help` shows `get` subcommand

## Truths Verified

1. **`ironhermes cron get {id}` returns full job details per UI-SPEC line 182** — verified by `render_job_details_contains_all_fields` asserting presence of name, id, schedule_display, prompt, deliver, skill, and `Next run:` label. All 13 field lines are rendered unconditionally except `Last status:`/`Last error:` which render only when present.
2. **`ironhermes cron get {unknown-id}` prints a red error and exits non-zero** — verified by `cmd_get_not_found_returns_error`. The `?` operator propagates the anyhow error from `ok_or_else`, which clap/main render in red with non-zero exit status (matches `cmd_pause`/`cmd_resume` behavior which use the identical pattern).
3. **Name lookup is case-insensitive** — inherited for free from `JobStore::find_job`, already covered by the existing `find_job_by_name_case_insensitive` test in `crates/ironhermes-cron/src/store.rs`. `cmd_get` is a thin wrapper that adds no new lookup logic.

## Commits

| Phase   | Hash      | Message                                                      |
| ------- | --------- | ------------------------------------------------------------ |
| RED     | `96e51a4` | test(05-04): add failing tests for cron get subcommand       |
| GREEN   | `0082732` | feat(05-04): implement cron get subcommand per UI-SPEC       |

TDD flow: RED commit established the failing test (`E0425: cannot find function render_job_details`), GREEN commit added the variant + dispatch + handler + helper and both tests went from fail-compile to pass.

## Deviations from Plan

None — plan executed exactly as written. A few small deviations from the literal plan text, all at-or-below the "cosmetic" threshold:

1. **`cmd_get` uses `print!` instead of `println!` on the rendered String.** The plan showed `println!("{}", render_job_details(&job))` but `writeln!` inside the renderer already appends `\n` to every line, so `println!` would add an extra blank line. Used `print!` to match the renderer contract. No functional difference to the user.
2. **Imported `CronJob` at the module level** (`use ironhermes_cron::{..., CronJob, ...}`) rather than re-importing inside the test module. Cleaner and avoids the need for a dead `_force_cronjob_use` shim.
3. **Added `use std::fmt::Write as FmtWrite`** at the module level (aliased because `std::io::Write` is already imported for `io::stdout().flush()`). Without the alias, the two `Write` traits would collide.

None of these affected the acceptance criteria or rendered output.

## Auto-fixed Issues

None — no Rule 1/2/3 fixes required. The implementation compiled cleanly on the first attempt after completing the plan's Step 1–6 instructions, and both tests passed on the first run without debugging.

## Deferred Issues

None.

## Known Stubs

None — all rendering paths emit real data from the `CronJob` struct. The `Last status:` and `Last error:` lines are only rendered when `last_status`/`last_error` are `Some`, which is an intentional UI-SPEC conformance behavior, not a stub.

## Threat Flags

None. This plan adds a read-only lookup CLI surface over an existing store method (`find_job`); no new network endpoints, no new auth paths, no new file access patterns, no schema changes. `cmd_get` opens the store read-only and does not mutate state.

## Self-Check: PASSED

- FOUND: crates/ironhermes-cli/src/cron.rs (Get variant, dispatch arm, cmd_get, render_job_details, both tests)
- FOUND: crates/ironhermes-cli/Cargo.toml ([dev-dependencies] tempfile = "3")
- FOUND: commit 96e51a4 (RED — test commit)
- FOUND: commit 0082732 (GREEN — implementation commit)
- VERIFIED: cargo build -p ironhermes-cli exits 0
- VERIFIED: cargo test -p ironhermes-cli cron — 2 passed, 0 failed
- VERIFIED: cargo run -p ironhermes-cli -- cron --help shows `get` subcommand
