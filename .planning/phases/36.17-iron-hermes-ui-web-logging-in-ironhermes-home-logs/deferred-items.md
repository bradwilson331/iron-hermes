# Phase 36.17 — Deferred items

Items discovered during execution that are out-of-scope per the executor
"SCOPE BOUNDARY" rule (pre-existing issues in unrelated files, not caused
by this phase's changes).

## Pre-existing clippy lints in `ironhermes-core` (discovered during plan 02 Task 2)

Running `cargo clippy -p iron_hermes_ui --features server -- -D warnings`
exits with code 101 due to **15 pre-existing clippy errors in
`crates/ironhermes-core/`**:

| File | Lint | Notes |
|------|------|-------|
| `src/browser_profile.rs:77,78,141` | `clippy::collapsible_if` x3 | Nested `if let` patterns predating let-else |
| `src/commands/handlers.rs:535,704` | `clippy::collapsible_if` x2 | Same nested-if pattern |
| `src/commands/handlers.rs:1259` | `clippy::manual_div_ceil` | `(a + b - 1) / b` → `a.div_ceil(b)` |
| `src/commands/handlers.rs:1331` | `clippy::field_reassign_with_default` | Inline init suggested |
| `src/commands/typo.rs:59,62` | `clippy::needless_range_loop` x2 | `for i in 0..n` → `iter().enumerate()` |
| `src/config.rs:71` | `clippy::manual_contains` | `iter().any(|r| *r == name)` → `contains(&name)` |
| `src/config.rs:225,281` | `clippy::derivable_impls` x2 | Manual `Default` impls |
| `src/memory_store.rs:435` | `clippy::manual_is_multiple_of` | `x % n == 0` → `x.is_multiple_of(n)` |
| `src/skills.rs:139` | `clippy::derivable_impls` | Manual `Default` impl on enum |
| `src/skills.rs:571` | `clippy::collapsible_if` | Same nested-if pattern |

**Why deferred:** Plan 36.17-02 only touches `crates/iron_hermes_ui/src/server/{logging.rs,mod.rs}`.
The 15 errors above are entirely in `crates/ironhermes-core/` and existed on
the base ref `c26106315766c49cbb6772103c75a1cb359176a6` before this plan's
changes. Fixing them is out of scope per the executor SCOPE BOUNDARY rule
(`@$HOME/.claude/get-shit-done/agents/gsd-executor.md` — "Only auto-fix
issues DIRECTLY caused by the current task's changes").

**Evidence of pre-existence:** Errors reference only `ironhermes-core/src/*.rs`
files; zero references to `iron_hermes_ui/src/server/logging.rs` (the only
file this plan created). Verified via:

```
$ grep -c 'logging\.rs' /tmp/clippy-server.log
0
```

**Build status:** `cargo build -p iron_hermes_ui --features server` exits 0
(no warnings or errors from logging.rs). `cargo build -p iron_hermes_ui` (default
features) also exits 0. The plan's structural acceptance criteria all pass.

**Suggested follow-up:** A separate small chore phase (`chore(core): apply
clippy fixes`) — most are mechanical `clippy --fix` candidates. Out of
scope here.
