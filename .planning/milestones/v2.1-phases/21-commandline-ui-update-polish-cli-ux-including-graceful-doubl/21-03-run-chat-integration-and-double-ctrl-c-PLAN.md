---
phase: 21-commandline-ui-update-polish-cli-ux-including-graceful-doubl
plan: 03
type: execute
wave: 3
depends_on:
  - 21-01
  - 21-02
files_modified:
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-cli/src/tui/mod.rs
  - crates/ironhermes-cli/tests/run_chat_invariants.rs
  - .planning/todos/pending/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md
  - .planning/todos/completed/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md
autonomous: false
requirements: []
decisions_addressed:
  - D-03
  - D-05
  - D-08
  - D-09
  - D-10
  - D-11
  - D-12
  - D-13
  - D-14
  - D-22

must_haves:
  truths:
    - "run_chat spawns TuiHandle before the REPL loop and calls shutdown on the clean-exit path"
    - "run_chat wires `with_streaming` callback to publish ActivityState::Streaming then Idle"
    - "run_chat wires `with_tool_progress` callback to publish ActivityState::ToolCall{name} — and REMOVES the old `eprint!(\"\\r Running: {}…\", name)` clutter (D-08)"
    - "run_chat wraps run_agent_turn in tokio::select! with tokio::signal::ctrl_c() per D-10"
    - "First ctrl-c during in-flight turn cancels via chat_cancel_token, prints `^C — turn cancelled`, installs a fresh CancellationToken (D-11, D-13)"
    - "Second ctrl-c within 1.5s persists session as interrupted, flushes memory_manager, prints Goodbye!, exits 0 (D-12)"
    - "Rustyline-Interrupted branch still prints `^C — type /quit to exit` and loops (D-14 regression guard)"
    - "run_single does NOT contain any tokio::signal::ctrl_c call (D-10, INV-3)"
    - "Third ctrl-c within 3s of first triggers std::process::exit(130) emergency escape (RESEARCH §Pitfall 7 footgun fix)"
    - "Rolled-in todo file moved from pending/ to completed/"
  artifacts:
    - path: "crates/ironhermes-cli/src/main.rs"
      provides: "Integrated TuiHandle + ctrl-c state machine in run_chat"
      contains: "tokio::signal::ctrl_c"
    - path: "crates/ironhermes-cli/tests/run_chat_invariants.rs"
      provides: "Static-grep regression tests locking INV-1 through INV-6"
      exports: []
    - path: ".planning/todos/completed/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md"
      provides: "Proof that the rolled-in todo is resolved"
  key_links:
    - from: "crates/ironhermes-cli/src/main.rs::run_chat"
      to: "crates/ironhermes-cli/src/tui/mod.rs::TuiHandle"
      via: "spawn before loop, shutdown on exit"
      pattern: "TuiHandle::new|tui\\.shutdown\\(\\)"
    - from: "crates/ironhermes-cli/src/main.rs::run_chat"
      to: "tokio::signal::ctrl_c"
      via: "tokio::select! arm inside the in-flight await"
      pattern: "tokio::signal::ctrl_c"
    - from: "crates/ironhermes-cli/src/main.rs::run_chat"
      to: "DoubleCtrlCState::on_ctrl_c"
      via: "state machine drives CancelTurn/ExitCleanly decisions"
      pattern: "DoubleCtrlCState|on_ctrl_c"
---

<objective>
Integrate Plans 21-01 and 21-02 into `run_chat`: spawn the TuiHandle at session start, publish ActivityState from streaming + tool-progress callbacks, wrap the in-flight agent call in `tokio::select!` with `tokio::signal::ctrl_c()`, drive the `DoubleCtrlCState` to decide cancel-vs-exit, and clean up on exit paths.

Add an integration-test file `tests/run_chat_invariants.rs` that uses static-grep over `main.rs` to lock in the six structural invariants (INV-1..INV-6) from RESEARCH.md — matching the Phase 20 precedent of regression tests that survive future refactors.

Final task of the plan is a manual-verification checkpoint where the user runs `cargo run -- chat` and walks the VALIDATION.md Per-Task Verification Map by hand (D-22).

Move the rolled-in todo file from `pending/` to `completed/` as the phase-completion signal.

Purpose: Translate tested, isolated building blocks into user-visible behavior. This is the ONLY plan that modifies `run_chat` so the diff is focused, reviewable, and reversible.

Output: Modified `run_chat` in main.rs; new `tests/run_chat_invariants.rs`; todo file moved; manual-verification sign-off.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md
@.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-RESEARCH.md
@.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-VALIDATION.md
@.planning/todos/pending/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md
@crates/ironhermes-cli/src/main.rs
@crates/ironhermes-cli/src/tui/mod.rs
@crates/ironhermes-cli/src/tui/render.rs
@crates/ironhermes-cli/src/tui/double_ctrl_c.rs
@crates/ironhermes-cli/src/tui/activity.rs
@crates/ironhermes-cli/src/tui/status_line.rs

<interfaces>
<!-- Plan 21-01 + 21-02 public API to consume here -->

From `crates/ironhermes-cli/src/tui/mod.rs`:
```rust
pub use activity::ActivityState;          // Idle | Thinking | Streaming | ToolCall{name}
pub use double_ctrl_c::{CtrlCDecision, DoubleCtrlCState};
pub use render::TuiHandle;
pub use status_line::StatusLineState;
```

TuiHandle API (from Plan 21-02):
```rust
impl TuiHandle {
    pub fn new(initial_status: StatusLineState) -> Self;
    pub fn set_activity(&self, state: ActivityState);
    pub fn set_status(&self, state: StatusLineState);
    pub async fn shutdown(mut self);
}
```

DoubleCtrlCState API (from Plan 21-01):
```rust
impl DoubleCtrlCState {
    pub fn new() -> Self;  // 1.5s window baked in per D-12
    pub fn on_ctrl_c(&mut self, now: Instant, in_flight: bool) -> CtrlCDecision;
    pub fn reset(&mut self);  // call on turn-complete, fresh-user-input
}
```

Current `run_chat` callbacks to replace (main.rs:600-607):
```rust
.with_streaming(Box::new(|delta| {
    print!("{}", delta);
    io::stdout().flush().ok();
}))
.with_tool_progress(Box::new(|name, _args| {
    eprint!("\r{} {}...", "Running:".dimmed(), name.yellow());
    io::stderr().flush().ok();
}));
```

Current readline-Interrupted branch to preserve (main.rs:558-560):
```rust
Err(rustyline::error::ReadlineError::Interrupted) => {
    println!("{}", "^C — type /quit to exit".dimmed());
}
```

Current clean-exit path to mirror for the D-12 interrupted path (main.rs:572-573):
```rust
state_store.end_session(&session_id, "completed")
    .context("failed to end CLI session")?;
```
</interfaces>
</context>

<tasks>

<task type="auto">
  <name>Task 1: Wire TuiHandle into run_chat — spawn, publish activity from callbacks, shutdown on exit</name>
  <files>
    crates/ironhermes-cli/src/main.rs
  </files>
  <read_first>
    - crates/ironhermes-cli/src/main.rs (full run_chat 374-575 and run_agent_turn 578-640 — MUST read current state before editing)
    - crates/ironhermes-cli/src/tui/mod.rs (confirm public re-exports from 21-02)
    - crates/ironhermes-cli/src/tui/render.rs (TuiHandle API)
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md (D-03, D-05, D-08, D-09)
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-RESEARCH.md (Pattern 3 watch channel, §Pitfall 5 ExternalPrinter frequency, §Pitfall 6 stderr collision)
  </read_first>
  <action>
This task focuses on TuiHandle wiring only. Ctrl-c handling lands in Task 2; it's isolated so the diff is smaller and each task is independently reviewable.

Step 1 — Add imports at the top of `crates/ironhermes-cli/src/main.rs`:

```rust
use crate::tui::{ActivityState, StatusLineState, TuiHandle};
```

Step 2 — Replace the current inline `print_banner()` call on the first line of `run_chat` (line 375) with a call that ALSO builds an initial status line. Keep the ASCII banner — D-22 uses it as a visual anchor. The status line is additive.

Step 3 — Inside `run_chat`, immediately after `state_store.create_session(...)` (line ~384), construct the initial status-line state and spawn the TuiHandle. The values come from the Config + ProviderResolver that are already in scope:

```rust
// Plan 21-03: spawn the bottom-bar TUI (status line + knight-rider scanner).
// Activity is Idle at startup; turns publish ActivityState::Streaming/ToolCall.
let initial_status = StatusLineState {
    mode: "Chat".to_string(),
    model_short: client.model().to_string(),
    provider: config.model.provider.clone(),
    tokens_used: 0,
    tokens_limit: 128_000,                       // matches with_compression(128_000, …) at main.rs:598
    hint: "ctrl+c cancel · /help commands".to_string(),
};
let tui = TuiHandle::new(initial_status);
let tui = std::sync::Arc::new(tui);
```

Wrapping in `Arc` lets us share the handle across the streaming callback (which must be `'static + Send`) without cloning the watch senders directly.

Step 4 — REPLACE the current `with_streaming` + `with_tool_progress` callbacks in `run_agent_turn`. Because `run_agent_turn` is a separate function that does NOT take a TuiHandle today, the cleanest fix is to add `tui: Arc<TuiHandle>` as a parameter and forward both call sites. Change the signature:

```rust
async fn run_agent_turn(
    client: &AnyClient,
    registry: Arc<ToolRegistry>,
    messages: &mut Vec<ChatMessage>,
    max_turns: usize,
    config: &Config,
    resolver: &ProviderResolver,
    budget: &Arc<AtomicUsize>,
    session_id: &str,
    pressure_tracker: Arc<PressureTracker>,
    compression_count: Arc<AtomicUsize>,
    tui: Arc<TuiHandle>,   // NEW
) -> Result<Option<String>> {
    // ...
    let tui_stream = tui.clone();
    let tui_tool = tui.clone();
    let mut agent = AgentLoop::new(client.clone(), registry, max_turns)
        .with_budget(budget.clone())
        .with_compression(128_000, config.agent.context_compression)
        .with_compression_count(starting_count)
        .with_streaming(Box::new(move |delta| {
            // Still print to stdout (D-22: stream still appears inline above the prompt).
            // Keep the high-frequency path on stdout to avoid the ExternalPrinter
            // jitter warned in RESEARCH §Pitfall 5.
            print!("{}", delta);
            io::stdout().flush().ok();
            // Publish coarse state change (best-effort; watch coalesces).
            tui_stream.set_activity(ActivityState::Streaming);
        }))
        .with_tool_progress(Box::new(move |name, _args| {
            // D-08: REPLACE the old `eprint!("\r Running: ...")` clutter with a
            // watch publish. The render task renders the scanner + label on the
            // bottom row every 100ms — no more inline stderr spray.
            tui_tool.set_activity(ActivityState::ToolCall { name: name.to_string() });
        }));
    // ... rest unchanged ...

    let result = agent.run(messages.clone()).await?;

    // After the turn completes, reset activity to Idle so the scanner hides (D-08).
    tui.set_activity(ActivityState::Idle);

    // Update the status line with post-turn token count (D-05).
    if let Some(used) = extract_total_tokens(&result) {
        tui.set_status(StatusLineState {
            mode: "Chat".to_string(),
            model_short: client.model().to_string(),
            provider: config.model.provider.clone(),
            tokens_used: used,
            tokens_limit: 128_000,
            hint: "ctrl+c cancel · /help commands".to_string(),
        });
    }

    compression_count.store(result.compression_count_after, Ordering::SeqCst);
    *messages = result.messages;
    Ok(result.final_response)
}
```

Add a small helper for extracting the token count from AgentResult. Inspect `ironhermes_agent::AgentResult` (referenced in main.rs via `agent.run(...).await?` and `result.compression_count_after`); typical shape per RESEARCH.md:815 is a field `total_usage.total_tokens`. Add this helper in main.rs below `run_agent_turn`:

```rust
fn extract_total_tokens(result: &ironhermes_agent::AgentResult) -> Option<u64> {
    // AgentResult.total_usage.total_tokens is already used by PressureTracker
    // (see crates/ironhermes-agent/src/agent_loop.rs:30-37). Read the same path.
    Some(result.total_usage.total_tokens as u64)
}
```

If the exact field name differs (`total_usage` vs `usage`, `total_tokens` vs `total`), grep crates/ironhermes-agent/src/agent_loop.rs for `total_usage|total_tokens` and adapt. The goal is simple: populate tokens_used with the agent's own accounting.

Step 5 — Update every call-site of `run_agent_turn` inside `run_chat` (there are two — the initial-message path at line ~477 and the loop path at line ~536) to pass `tui.clone()` as the last argument.

Step 6 — On the clean-exit path (where `run_chat` returns Ok after the REPL loop ends), call `tui.shutdown()`. Since `tui` is `Arc<TuiHandle>` and shutdown takes `mut self`, unwrap the Arc first. The cleanest pattern:

```rust
// Before the final end_session call:
if let Ok(tui_handle) = std::sync::Arc::try_unwrap(tui) {
    tui_handle.shutdown().await;
}
state_store.end_session(&session_id, "completed")
    .context("failed to end CLI session")?;
Ok(())
```

If `Arc::try_unwrap` fails (outstanding clone in a callback closure that survived), that's acceptable — the render task is cancelled on runtime drop anyway. Log a `tracing::debug` in the else branch.

Step 7 — Do NOT modify `run_single` (per D-10 it stays single-shot with no TUI — confirmed by INV-3 static-grep in Task 3).

Step 8 — Build and run the existing test suite to confirm no regressions:

```bash
cargo build -p ironhermes-cli
cargo test -p ironhermes-cli --lib
```

Existing tests must still pass; `tui::` tests from 21-01 + 21-02 still pass (they don't depend on main.rs).
  </action>
  <verify>
    <automated>cargo build -p ironhermes-cli && cargo test -p ironhermes-cli --lib</automated>
  </verify>
  <done>
    - `run_agent_turn` signature takes `tui: Arc<TuiHandle>` as the last parameter
    - Both call sites in `run_chat` pass the shared TuiHandle
    - `with_streaming` callback publishes ActivityState::Streaming
    - `with_tool_progress` callback publishes ActivityState::ToolCall{name} and the old `eprint!("\r Running: ...")` line is removed
    - After agent.run() completes, tui.set_activity(Idle) + tui.set_status(...) with live token count
    - `run_chat` shuts down the TuiHandle on the clean-exit path (before end_session "completed")
    - `run_single` unchanged — no TuiHandle, no ctrl-c
    - cargo build -p ironhermes-cli exits 0
    - cargo test -p ironhermes-cli --lib exits 0 (all existing + all tui:: tests pass)
  </done>
  <acceptance_criteria>
    - `rg -n "use crate::tui::\{ActivityState, StatusLineState, TuiHandle\}" crates/ironhermes-cli/src/main.rs` returns a match
    - `rg -n "TuiHandle::new\(initial_status\)" crates/ironhermes-cli/src/main.rs` returns a match
    - `rg -n "tui: Arc<TuiHandle>" crates/ironhermes-cli/src/main.rs` returns at least one match (run_agent_turn signature)
    - `rg -n "ActivityState::Streaming" crates/ironhermes-cli/src/main.rs` returns a match (with_streaming callback)
    - `rg -n "ActivityState::ToolCall" crates/ironhermes-cli/src/main.rs` returns a match (with_tool_progress callback)
    - `rg -n "ActivityState::Idle" crates/ironhermes-cli/src/main.rs` returns a match (post-turn reset)
    - `rg -n "eprint!\(\"\\\\r.+Running:" crates/ironhermes-cli/src/main.rs` returns NO matches — the old clutter is REMOVED (D-08)
    - `rg -n "tui\.shutdown\(\)" crates/ironhermes-cli/src/main.rs` returns at least one match
    - `rg -n "fn run_single" crates/ironhermes-cli/src/main.rs` still returns a match (function still exists)
    - `rg -n "TuiHandle" crates/ironhermes-cli/src/main.rs | rg "run_single"` returns NO matches (INV-3 baseline — run_single untouched)
    - `cargo build -p ironhermes-cli` exits 0
    - `cargo test -p ironhermes-cli --lib` exits 0 reporting all existing + all tui:: tests green
    - `cargo clippy -p ironhermes-cli -- -D warnings` exits 0
    - `git diff HEAD -- crates/ironhermes-cli/Cargo.toml` produces empty output (INV-6)
  </acceptance_criteria>
</task>

<task type="auto">
  <name>Task 2: Install tokio::signal::ctrl_c + DoubleCtrlCState in run_chat; child CancellationToken per turn; 3rd-press emergency escape</name>
  <files>
    crates/ironhermes-cli/src/main.rs
  </files>
  <read_first>
    - crates/ironhermes-cli/src/main.rs (post-Task-1 state — read the updated run_chat before further edits)
    - crates/ironhermes-cli/src/tui/double_ctrl_c.rs (Plan 21-01 — DoubleCtrlCState API)
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md (D-10, D-11, D-12, D-13, D-14)
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-RESEARCH.md (Pattern 4 tokio::select!, §Pitfall 1 rustyline eats ctrl-c, §Pitfall 2 CancellationToken permanent, §Pitfall 7 tokio SIGINT permanent override)
  </read_first>
  <action>
Step 1 — Add imports at the top of main.rs (if not already present from Task 1):

```rust
use crate::tui::{CtrlCDecision, DoubleCtrlCState};
use std::time::Instant;
```

Step 2 — Inside `run_chat`, before the REPL loop (after the existing `let chat_cancel_token = CancellationToken::new();` at line ~409), replace that single assignment with the parent/child pattern:

```rust
// Plan 21-03: parent CancellationToken lives the full chat session; per-turn
// children are issued via `.child_token()` so cancelling one turn does NOT
// poison the session. (See RESEARCH §Pitfall 2: CancellationToken cancel is permanent.)
let chat_cancel_parent = CancellationToken::new();
let mut chat_cancel_token = chat_cancel_parent.child_token();

// Double-ctrl-c state machine (D-10..D-14). 1.5s debounce window is baked in.
let mut double_ctrl_c = DoubleCtrlCState::new();

// Emergency 3rd-press escape per RESEARCH §Pitfall 7: track first-press time
// across the whole session. If 3 ctrl-c events arrive within 3 seconds of the
// FIRST press, we std::process::exit(130) to avoid tokio's permanent-handler
// footgun where shutdown itself could hang.
let mut emergency_first_press: Option<Instant> = None;
let mut emergency_press_count: u32 = 0;
```

Step 3 — The `register_delegate_task_tool` call at line ~441 currently passes `Some(chat_cancel_token.clone())`. Change it to pass `Some(chat_cancel_parent.child_token())` so the delegate tool holds a long-lived child distinct from the per-turn child. This ensures cancelling a turn does not kill subagents spawned for delegate_task.

```rust
registry.register_delegate_task_tool(
    subagent_runner,
    subagent_semaphore,
    Some(memory_manager.clone()),
    config.subagent.clone(),
    Some(chat_cancel_parent.child_token()),   // CHANGED from chat_cancel_token.clone()
    Some(subagent_progress),
);
```

Step 4 — Replace each call site of `run_agent_turn` inside `run_chat` with a `tokio::select!` that races the agent future against `tokio::signal::ctrl_c()`. Per D-10: tokio::select is around the agent call ONLY, not around readline.

The canonical pattern (use this form — it compiles cleanly because the future is pinned outside the loop, so CancelTurn can `continue` and keep awaiting the same future):

```rust
// In-flight: publish Thinking state so the scanner shows up immediately.
tui.set_activity(ActivityState::Thinking);

let mut run_fut = Box::pin(run_agent_turn(
    &client,
    registry.clone(),
    &mut messages,
    max_turns,
    &config,
    &resolver,
    &budget,
    &session_id,
    pressure_tracker.clone(),
    compression_count.clone(),
    tui.clone(),
));

let response: Option<String> = loop {
    tokio::select! {
        biased;
        _ = tokio::signal::ctrl_c() => {
            let now = Instant::now();
            // Emergency escape: 3 presses within 3s of first → hard exit 130.
            emergency_press_count += 1;
            if emergency_first_press.is_none() {
                emergency_first_press = Some(now);
            }
            if let Some(first) = emergency_first_press {
                if emergency_press_count >= 3
                    && now.duration_since(first) <= std::time::Duration::from_secs(3)
                {
                    eprintln!("{}", "^C×3 — emergency exit".red());
                    std::process::exit(130);
                }
            }

            match double_ctrl_c.on_ctrl_c(now, /* in_flight = */ true) {
                CtrlCDecision::CancelTurn => {
                    chat_cancel_token.cancel();
                    println!("{}", "^C — turn cancelled".dimmed());
                    tui.set_activity(ActivityState::Idle);
                    // Stay in the select loop so the cancel propagates and the
                    // agent future resolves naturally (it sees the token.cancelled()).
                    continue;
                }
                CtrlCDecision::ExitCleanly => {
                    chat_cancel_token.cancel();
                    println!("{}", "Goodbye!".dimmed());
                    // D-12: flush memory (mirrors gateway path per RESEARCH Open Q1).
                    {
                        let mut mgr = memory_manager.lock().await;
                        let _ = mgr.flush_to_disk().await;
                    }
                    // Clear the bottom bar before exit.
                    if let Ok(tui_handle) = std::sync::Arc::try_unwrap(tui.clone()) {
                        tui_handle.shutdown().await;
                    }
                    let _ = state_store.end_session(&session_id, "interrupted");
                    std::process::exit(0);
                }
                CtrlCDecision::ShowPromptHint => {
                    // Unreachable here — we're in-flight. Defensive no-op.
                    continue;
                }
            }
        }
        r = &mut run_fut => { break r?; }
    }
};
// Turn completed cleanly: reset debounce + emergency + issue fresh child.
double_ctrl_c.reset();
emergency_press_count = 0;
emergency_first_press = None;
chat_cancel_token = chat_cancel_parent.child_token();
```

If `memory_manager.lock().await` returns a MemoryManager that does not expose `flush_to_disk`, grep `crates/ironhermes-agent/src/memory/manager.rs` for the public flush method (likely `flush()` or `on_session_end()`). Call whichever exists; if none does, log `tracing::debug!("memory flush not available — skipping")` and continue.

Step 5 — Preserve the existing readline-Interrupted branch at lines 558-560 verbatim. D-14 requires ctrl-c at the prompt to print `^C — type /quit to exit` and loop. Do NOT remove it.

Also add `double_ctrl_c.reset();` immediately after `rl.readline()` returns Ok with non-empty input (fresh user input resets the 1.5s window per D-13). Place it right after `let input = line.trim().to_string(); if input.is_empty() { continue; }`.

Step 6 — Update the clean-exit path (line ~572) to also flush memory before end_session and shutdown TUI:

```rust
{
    let mut mgr = memory_manager.lock().await;
    let _ = mgr.flush_to_disk().await;
}
if let Ok(tui_handle) = std::sync::Arc::try_unwrap(tui) {
    tui_handle.shutdown().await;
}
state_store.end_session(&session_id, "completed")
    .context("failed to end CLI session")?;
Ok(())
```

Step 7 — Build + run tests:

```bash
cargo build -p ironhermes-cli
cargo test -p ironhermes-cli --lib
cargo clippy -p ironhermes-cli -- -D warnings
```

All existing tests must still pass. The tui::double_ctrl_c tests (6) lock in the state machine behavior — they continue to pass because the state machine module itself is unchanged.
  </action>
  <verify>
    <automated>cargo build -p ironhermes-cli && cargo test -p ironhermes-cli --lib && cargo clippy -p ironhermes-cli -- -D warnings</automated>
  </verify>
  <done>
    - run_chat contains `tokio::signal::ctrl_c()` inside a `tokio::select!` that races the agent future
    - DoubleCtrlCState::new() instantiated before the REPL loop
    - First ctrl-c: chat_cancel_token.cancel() + "^C — turn cancelled" + loop continues awaiting agent future
    - Second ctrl-c within window: memory flush + end_session(interrupted) + exit(0)
    - 3rd ctrl-c within 3s: std::process::exit(130) emergency escape
    - Parent CancellationToken pattern: chat_cancel_parent survives, per-turn children issued via .child_token()
    - After successful turn: double_ctrl_c.reset() + emergency counter reset + fresh child token
    - rustyline-Interrupted branch preserved verbatim (D-14)
    - Reset on fresh user input (D-13)
    - Clean-exit path flushes memory + shuts down TUI before end_session
    - run_single unchanged (INV-3)
    - All tests green, clippy green, Cargo.toml unchanged
  </done>
  <acceptance_criteria>
    - `rg -n "tokio::signal::ctrl_c\(\)" crates/ironhermes-cli/src/main.rs` returns at least one match (INV-1)
    - `rg -n "DoubleCtrlCState::new\(\)" crates/ironhermes-cli/src/main.rs` returns a match
    - `rg -n "CtrlCDecision::CancelTurn|CtrlCDecision::ExitCleanly" crates/ironhermes-cli/src/main.rs` returns at least 2 matches
    - `rg -n "child_token\(\)" crates/ironhermes-cli/src/main.rs` returns at least 2 matches (parent issues children — INV-2)
    - `rg -n "chat_cancel_parent" crates/ironhermes-cli/src/main.rs` returns at least 2 matches (parent pattern present)
    - `rg -n "std::process::exit\(130\)" crates/ironhermes-cli/src/main.rs` returns a match (emergency escape)
    - `rg -n "std::process::exit\(0\)" crates/ironhermes-cli/src/main.rs` returns a match (D-12 clean exit)
    - `rg -n "end_session\(&session_id, \"interrupted\"\)" crates/ironhermes-cli/src/main.rs` returns a match (D-12)
    - `rg -n "end_session\(&session_id, \"completed\"\)" crates/ironhermes-cli/src/main.rs` returns a match (D-14 normal /quit)
    - `rg -n "\\^C — type /quit to exit" crates/ironhermes-cli/src/main.rs` returns a match (D-14 rustyline-interrupted branch preserved)
    - `rg -n "\\^C — turn cancelled" crates/ironhermes-cli/src/main.rs` returns a match (D-11)
    - `rg -n "double_ctrl_c\.reset\(\)" crates/ironhermes-cli/src/main.rs` returns at least 2 matches (reset after turn + reset on fresh user input per D-13)
    - Scope check: `awk '/^async fn run_single/,/^}/' crates/ironhermes-cli/src/main.rs | rg "tokio::signal::ctrl_c"` returns NO matches (INV-3: ctrl-c NOT in run_single)
    - `cargo build -p ironhermes-cli` exits 0
    - `cargo test -p ironhermes-cli --lib` exits 0 — all tui::* + existing tests pass
    - `cargo clippy -p ironhermes-cli -- -D warnings` exits 0
    - `git diff HEAD -- crates/ironhermes-cli/Cargo.toml` produces empty output (INV-6)
  </acceptance_criteria>
</task>

<task type="auto">
  <name>Task 3: Add static-grep integration test + move rolled-in todo to completed/</name>
  <files>
    crates/ironhermes-cli/tests/run_chat_invariants.rs,
    .planning/todos/pending/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md,
    .planning/todos/completed/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md
  </files>
  <read_first>
    - crates/ironhermes-cli/src/main.rs (final post-Task-2 state)
    - .planning/todos/pending/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-RESEARCH.md (§Invariants INV-1 through INV-6)
  </read_first>
  <behavior>
    - Test: INV-1 — main.rs contains `tokio::signal::ctrl_c` AND `tokio::select!` inside run_chat
    - Test: INV-2 — main.rs contains `child_token()` pattern + `chat_cancel_parent` name (fresh token per turn)
    - Test: INV-3 — run_single function body does NOT contain `ctrl_c` or `DoubleCtrlCState`
    - Test: INV-4 — tui/render.rs pairs SavePosition with RestorePosition (count equality)
    - Test: INV-5 — no `println!` or `print!` in `crates/ironhermes-cli/src/tui/` outside `#[cfg(test)]`
    - Test: INV-6 — Cargo.toml contains no known forbidden new deps (`ratatui`, `reedline`, `ctrlc`, `signal-hook`)
  </behavior>
  <action>
Step 1 — Create `crates/ironhermes-cli/tests/run_chat_invariants.rs`. This is a Rust integration test that reads source files as strings and asserts regexes — matching the Phase 20 `run_chat_and_run_single_both_wire_memory_manager` precedent.

```rust
//! Static-grep regression tests locking the six Phase 21 structural invariants
//! (RESEARCH.md §Invariants). These are intentionally brittle — if a future
//! refactor changes the structure, the test tells you exactly what invariant
//! was broken so you can either (a) fix the invariant or (b) update the test
//! with explicit justification.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    let full = crate_root().join(path);
    fs::read_to_string(&full).unwrap_or_else(|e| panic!("read {:?}: {}", full, e))
}

/// Extract the body of a top-level `async fn NAME` block from main.rs.
/// Matches from `async fn NAME` through the first balanced `}` at indent 0.
fn extract_fn_body(src: &str, name: &str) -> String {
    let needle = format!("async fn {}", name);
    let start = src.find(&needle).unwrap_or_else(|| {
        panic!("function `async fn {}` not found in main.rs", name)
    });
    let bytes = src.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i] != b'{' {
        i += 1;
    }
    if i >= bytes.len() {
        panic!("opening brace for {} not found", name);
    }
    let body_start = i;
    let mut depth = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return src[body_start..=i].to_string();
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("closing brace for {} not found", name);
}

#[test]
fn inv_1_run_chat_has_tokio_select_with_ctrl_c() {
    let src = read("src/main.rs");
    let run_chat = extract_fn_body(&src, "run_chat");
    assert!(
        run_chat.contains("tokio::signal::ctrl_c"),
        "INV-1: run_chat must wrap in-flight agent future with tokio::signal::ctrl_c — not found"
    );
    assert!(
        run_chat.contains("tokio::select!"),
        "INV-1: run_chat must use tokio::select! for ctrl-c handling — not found"
    );
}

#[test]
fn inv_2_fresh_child_token_per_turn() {
    let src = read("src/main.rs");
    let run_chat = extract_fn_body(&src, "run_chat");
    assert!(
        run_chat.contains("child_token()"),
        "INV-2: run_chat must issue fresh child CancellationToken per turn (RESEARCH §Pitfall 2) — child_token() not found"
    );
    assert!(
        run_chat.contains("chat_cancel_parent"),
        "INV-2: expected chat_cancel_parent parent-token name — not found"
    );
}

#[test]
fn inv_3_run_single_does_not_install_ctrl_c() {
    let src = read("src/main.rs");
    let run_single = extract_fn_body(&src, "run_single");
    assert!(
        !run_single.contains("tokio::signal::ctrl_c"),
        "INV-3: run_single must NOT install ctrl-c handler (D-10)"
    );
    assert!(
        !run_single.contains("DoubleCtrlCState"),
        "INV-3: run_single must NOT use the double-ctrl-c state machine (D-10)"
    );
}

#[test]
fn inv_4_render_pairs_save_and_restore_position() {
    let render = read("src/tui/render.rs");
    let saves = render.matches("SavePosition").count();
    let restores = render.matches("RestorePosition").count();
    assert!(
        saves >= 1 && restores >= 1,
        "INV-4: tui/render.rs must use both SavePosition and RestorePosition — saves={}, restores={}",
        saves, restores
    );
    assert!(
        restores >= saves,
        "INV-4: every SavePosition should have a matching RestorePosition (saves={} restores={})",
        saves, restores
    );
}

#[test]
fn inv_5_no_stdout_prints_in_tui_module() {
    let tui_dir = crate_root().join("src/tui");
    let entries = fs::read_dir(&tui_dir)
        .unwrap_or_else(|e| panic!("read_dir {:?}: {}", tui_dir, e));
    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let src = fs::read_to_string(&path).unwrap();
        // Split the file at `#[cfg(test)]` — only check everything BEFORE the
        // first test module annotation. This is a conservative heuristic:
        // production code is written above tests by convention.
        let prod_slice = match src.find("#[cfg(test)]") {
            Some(idx) => &src[..idx],
            None => &src[..],
        };
        for (lineno, line) in prod_slice.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("println!") || trimmed.starts_with("print!(") {
                panic!(
                    "INV-5: println!/print! found in {:?} line {} (production code): {}",
                    path,
                    lineno + 1,
                    line
                );
            }
        }
    }
}

#[test]
fn inv_6_no_forbidden_new_deps_in_cargo_toml() {
    let cargo = read("Cargo.toml");
    for forbidden in &["ratatui", "reedline", "ctrlc = ", "signal-hook"] {
        assert!(
            !cargo.contains(forbidden),
            "INV-6: forbidden dep `{}` found in Cargo.toml (D-18: no new deps this phase)",
            forbidden
        );
    }
}
```

Step 2 — Move the rolled-in todo from pending/ to completed/:

```bash
mv .planning/todos/pending/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md \
   .planning/todos/completed/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md
```

Append a Resolution section to the moved file (keep existing content intact):

```markdown

---

## Resolution

Completed in Phase 21. See:
- Plan `21-03-run-chat-integration-and-double-ctrl-c-PLAN.md`
- D-10..D-14 in `.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md`
- Integration tests locked via INV-1, INV-2, INV-3 in `crates/ironhermes-cli/tests/run_chat_invariants.rs`
```

Step 3 — Run the new integration test plus full cli crate test suite:

```bash
cargo test -p ironhermes-cli --test run_chat_invariants
cargo test -p ironhermes-cli
cargo clippy -p ironhermes-cli -- -D warnings
```

All six invariant tests must pass.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-cli --test run_chat_invariants && cargo test -p ironhermes-cli && cargo clippy -p ironhermes-cli -- -D warnings</automated>
  </verify>
  <done>
    - crates/ironhermes-cli/tests/run_chat_invariants.rs exists with 6 invariant tests
    - All 6 tests pass
    - Rolled-in todo moved from pending/ to completed/
    - Moved todo file has a Resolution section appended
    - Pending todo file no longer exists
    - Full cli crate test suite green
    - Clippy green
    - Cargo.toml unchanged
  </done>
  <acceptance_criteria>
    - File exists: crates/ironhermes-cli/tests/run_chat_invariants.rs
    - `rg -n "fn inv_1_run_chat_has_tokio_select_with_ctrl_c" crates/ironhermes-cli/tests/run_chat_invariants.rs` returns a match
    - `rg -n "fn inv_2_fresh_child_token_per_turn" crates/ironhermes-cli/tests/run_chat_invariants.rs` returns a match
    - `rg -n "fn inv_3_run_single_does_not_install_ctrl_c" crates/ironhermes-cli/tests/run_chat_invariants.rs` returns a match
    - `rg -n "fn inv_4_render_pairs_save_and_restore_position" crates/ironhermes-cli/tests/run_chat_invariants.rs` returns a match
    - `rg -n "fn inv_5_no_stdout_prints_in_tui_module" crates/ironhermes-cli/tests/run_chat_invariants.rs` returns a match
    - `rg -n "fn inv_6_no_forbidden_new_deps_in_cargo_toml" crates/ironhermes-cli/tests/run_chat_invariants.rs` returns a match
    - File exists: .planning/todos/completed/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md
    - File does NOT exist: .planning/todos/pending/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md
    - `rg -n "## Resolution" .planning/todos/completed/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md` returns a match
    - `cargo test -p ironhermes-cli --test run_chat_invariants` exits 0 reporting 6 tests passing
    - `cargo test -p ironhermes-cli` exits 0 (full crate suite)
    - `cargo clippy -p ironhermes-cli -- -D warnings` exits 0
    - `git diff HEAD -- crates/ironhermes-cli/Cargo.toml` produces empty output (INV-6)
  </acceptance_criteria>
</task>

<task type="checkpoint:human-verify" gate="blocking">
  <name>Task 4: Manual QA — VALIDATION.md walkthrough (D-22)</name>
  <files>
    (human-driven — no file edits required)
  </files>
  <read_first>
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-VALIDATION.md (Manual-Only Verifications table)
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md (D-22)
  </read_first>
  <action>
Manual verification checkpoint. No code changes. The executor MUST prompt the user to perform the 9 verification scenarios below and collect PASS/FAIL responses before marking the phase complete.

**Setup:** In one terminal, run `cargo run -p ironhermes-cli -- chat` (use any configured model).

1. **Status line present and colored correctly (D-03, D-04, D-05)**
   - Observe bottom row: `Chat · <model> · <provider> · 0/128.0K (0%) · ctrl+c cancel · /help commands`
   - Pills should alternate cyan / magenta / green / yellow / dimmed. Dots are dimmed. Hint at far right is dimmed.
   - Expected: PASS if visible and colored per D-04.

2. **Knight rider animates during turn (D-06, D-07, D-09)**
   - Send a prompt that triggers a tool call (e.g. "list files in /tmp using the bash tool").
   - Watch bottom-LEFT row (one above status row): 10-cell track with a bright cyan block sweeping left-right, trailing fade, updating ~10fps, labeled `Running: bash` or `Streaming` to the right.
   - Expected: PASS if the sweep is visible and the label updates per activity.

3. **Scanner hides when idle (D-08)**
   - After the turn completes, the scanner row is blank.
   - Expected: PASS if the scanner row is cleared between turns.

4. **First ctrl-c cancels mid-stream (D-11)**
   - Send a long prompt so streaming is active for >2s. Press ctrl-c ONCE.
   - Expected: prints `^C — turn cancelled` (dimmed). Prompt returns. Conversation history retained. Can send another prompt immediately.

5. **Second ctrl-c within 1.5s exits cleanly (D-12)**
   - Send another long prompt. Press ctrl-c, then ctrl-c again within 1 second.
   - Expected: prints `Goodbye!`. Process exits with status 0. Session persisted with status = "interrupted" (verify via sqlite query or `/sessions` command in a new chat).

6. **Ctrl-c at prompt does NOT exit (D-14)**
   - Run chat again. At the empty prompt (no in-flight work), press ctrl-c.
   - Expected: prints `^C — type /quit to exit` (dimmed). Prompt continues. Press ctrl-c again — same message, still no exit.
   - Type `/quit`. Expected: prints `Goodbye!`. Exit 0.

7. **3rd ctrl-c emergency escape within 3s (RESEARCH §Pitfall 7 fix)**
   - Run chat, send a long prompt. Tap ctrl-c three times rapidly (within 3s).
   - Expected: by the 2nd tap, `Goodbye!` + exit. If shutdown hangs, the 3rd tap triggers `^C×3 — emergency exit` (red) and exit code 130.

8. **Terminal resize doesn't corrupt bar (Claude's Discretion)**
   - During chat, resize the terminal window. Wait 1 tick.
   - Expected: status bar redraws at the new bottom. Occasional 1-frame glitch on resize is acceptable per RESEARCH Open Question 4.

9. **Non-tty pipe doesn't render the bar (RESEARCH Open Question 5)**
   - Run `cargo run -p ironhermes-cli -- chat --execute "hello" | cat`.
   - Expected: no ANSI garbage for the status line / scanner in the piped output. Output is clean.

The executor asks the user to respond PASS/FAIL for each numbered scenario, collects the responses, and proceeds only when all 9 pass or the user explicitly waives specific scenarios.
  </action>
  <verify>
    <automated>MISSING — this is a checkpoint:human-verify task. Automated verification lives in Tasks 1-3 of this plan and in Plans 21-01, 21-02.</automated>
  </verify>
  <done>
    User has responded PASS (or documented waiver) for all 9 manual-verification scenarios from VALIDATION.md §Manual-Only Verifications.
  </done>
  <acceptance_criteria>
    - User response is recorded with PASS/FAIL per scenario
    - Any FAIL results in task revision (loop back to Plan 21-03 Task 1 or 2) — not phase completion
    - PASS on all 9 scenarios (or explicit waiver per scenario) unblocks `/gsd-verify-work`
  </acceptance_criteria>
  <what-built>
    Phase 21 runtime integration is complete. The CLI now has:
    - Persistent bottom status line with alternating pill colors (D-03, D-04, D-05)
    - Knight-rider scanner animating during in-flight turns (D-06, D-07, D-08, D-09)
    - Graceful double ctrl-c (D-10..D-14)
    - 3rd-ctrl-c emergency exit within 3s window
    - Zero new dependencies (D-18)
    - 29+ unit/integration tests green, 6 static-grep invariant tests green
  </what-built>
  <how-to-verify>
    Execute the 9 scenarios in <action> above and respond with PASS/FAIL per scenario.
  </how-to-verify>
  <resume-signal>Type "approved" to complete the phase, or describe failures for revision.</resume-signal>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| SIGINT (OS) → tokio::signal::ctrl_c | Kernel-delivered signal; tokio installs a permanent process-global handler (RESEARCH §Pitfall 7) |
| User input (rustyline) → DoubleCtrlCState | Indirect — ctrl-c press is the input, not typed text |
| chat_cancel_parent → per-turn child tokens | Parent-child token propagation ensures one turn's cancel cannot poison others |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-21-11 | DoS — hung shutdown via tokio permanent SIGINT handler | run_chat exit paths | mitigate | 3rd-ctrl-c within 3s → `std::process::exit(130)` emergency escape (RESEARCH §Pitfall 7 footgun fix) |
| T-21-12 | DoS — CancellationToken::cancel permanent | per-turn agent future | mitigate | `chat_cancel_parent.child_token()` per turn; parent survives session so subagent tools keep a valid token (RESEARCH §Pitfall 2) |
| T-21-13 | DoS — race: stderr collision between render task and eprintln subagent progress | redraw + subagent_progress | accept | 100ms render cadence means stale subagent lines are overwritten within one frame; full serialization through the render task is a future-phase enhancement |
| T-21-14 | Information Disclosure — session_id in status line | status line render | mitigate | D-03 explicitly lists mode/model/provider/tokens — session_id is NOT included (RESEARCH Open Question 2) |
| T-21-15 | Tampering — ANSI injection from model-generated tool-name | ActivityState::ToolCall | accept | Tool names come from the tool registry (compile-time constants); model cannot inject arbitrary tool names — registry validates |
| T-21-16 | Integrity — partial session state on hard-exit path | 3rd-ctrl-c emergency escape | accept | `std::process::exit(130)` bypasses end_session; documented as emergency-only behavior. Normal 2nd-ctrl-c path flushes memory + marks session "interrupted". |
</threat_model>

<verification>
## Plan-Level Verification

```bash
# Static-grep invariants (hard gates):
cargo test -p ironhermes-cli --test run_chat_invariants

# Full cli crate:
cargo test -p ironhermes-cli

# Clippy:
cargo clippy -p ironhermes-cli -- -D warnings

# Phase-level gate:
cargo test --workspace     # before /gsd-verify-work
```

## Phase-Level Verification (post-Plan 21-03)

- INV-1: `rg -n "tokio::signal::ctrl_c" crates/ironhermes-cli/src/main.rs` → ≥1 match
- INV-2: `rg -n "child_token\(\)" crates/ironhermes-cli/src/main.rs` → ≥2 matches
- INV-3: `awk '/^async fn run_single/,/^}/' crates/ironhermes-cli/src/main.rs | rg "tokio::signal::ctrl_c"` → 0 matches
- INV-4: `rg -n "SavePosition" crates/ironhermes-cli/src/tui/render.rs` ≥1; `rg -n "RestorePosition" ...` ≥1
- INV-5: no `println!` / `print!` in `crates/ironhermes-cli/src/tui/` outside `#[cfg(test)]` (enforced by inv_5 test)
- INV-6: `git diff HEAD -- crates/ironhermes-cli/Cargo.toml` → empty
- Todo closure: `ls .planning/todos/pending/ | rg "2026-04-13-double-ctrl-c"` → no match; `ls .planning/todos/completed/ | rg "2026-04-13-double-ctrl-c"` → 1 match
</verification>

<success_criteria>
- run_chat integrates TuiHandle + DoubleCtrlCState + tokio::select! with ctrl_c
- run_single remains untouched (INV-3)
- Static-grep invariant test file exists with 6 tests, all green
- Rolled-in todo moved to completed/ with Resolution section appended
- Manual QA sign-off on all 9 VALIDATION.md scenarios (D-22)
- Full cli crate test suite green
- Clippy green
- Cargo.toml unchanged (D-18 enforced by INV-6 test)
- Phase verification: `cargo test --workspace` green before `/gsd-verify-work`
</success_criteria>

<output>
After completion, create `.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-03-SUMMARY.md` capturing:
- Final main.rs diff stats (lines changed, functions modified)
- Static-grep invariant tests added
- VALIDATION.md manual-QA results (copy the PASS/FAIL grid from Task 4)
- Confirmation the rolled-in todo file is in completed/
- Full-workspace test result
</output>
