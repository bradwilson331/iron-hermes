# Phase 21: Commandline UI Update — Research

**Researched:** 2026-04-16
**Domain:** Rust CLI UX (crossterm + rustyline + tokio)
**Confidence:** HIGH (primary APIs verified against docs and source), MEDIUM (rustyline↔tokio signal interaction — requires a smoke test on first build)

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Visual Reference**
- **D-01:** Target aesthetic is the attached reference image (OpenCode Zen-style terminal UI): clean dark background, dot-separated pill stats in the bottom status line, tree-style progress hierarchy, thin input prompt bar at the very bottom.
- **D-02:** Respect the existing IronHermes color identity — `.cyan()` for brand, `.green()` for affirmative, `.yellow()` for pending/warnings, `.dimmed()` for secondary text. Do NOT introduce a new color palette. Alternating colors for status pills draw from this palette (e.g., cyan / magenta / green / yellow rotating).

**Status Line**
- **D-03:** Render a persistent one-line status bar pinned to the terminal bottom while the REPL is active. Stats shown (left-to-right, dot-separated): `{mode} · {model_short} · {provider} · {tokens}/{limit} ({pct}%) · {hint}`. Example: `Agent · claude-sonnet-4 · anthropic · 107.7K (54%) · ctrl+p commands`.
- **D-04:** Pills use alternating colors. Rotation: pill[0]=cyan, pill[1]=magenta, pill[2]=green, pill[3]=yellow, pill[4]=dimmed. Dots (`·`) stay `.dimmed()`. Hint at the far right stays `.dimmed()` regardless of rotation.
- **D-05:** Stats update on a tick (~200ms) driven by the agent loop's pressure tracker / budget counter (`Arc<AtomicUsize>` already exists). Token count comes from `PressureTracker` / `result.total_usage.total_tokens`.

**Knight Rider Activity Indicator**
- **D-06:** Bottom-left activity indicator is a horizontal "scanner" bar that sweeps left↔right across a fixed-width track (e.g., 10 cells wide) while a turn or tool call is in flight.
- **D-07:** Track dimensions: 10 columns wide, 1 row tall. Lit cell is bright cyan, trailing cells fade with `.dimmed()` for a 2–3 cell tail. Frame rate ~10 fps (100ms per frame).
- **D-08:** The scanner is VISIBLE only when an agent turn or streaming call is active. When idle (at prompt), the indicator is hidden or replaced with an `esc interrupt` hint. Replaces the current `\r Running: {tool}...` clutter in `run_agent_turn`'s `with_tool_progress` callback.
- **D-09:** Label shown to the right of the scanner: the current activity name (e.g., `Thinking`, `Running: bash`, `Streaming`). Uses the existing callback surface (`with_streaming`, `with_tool_progress`) — no new plumbing from the agent crate.

**Graceful Double Ctrl-C**
- **D-10:** Install a `tokio::signal::ctrl_c` handler in the chat/agent run loop (NOT in `run_single`). Use the existing `chat_cancel_token: CancellationToken` that's already threaded through `register_delegate_task_tool`.
- **D-11:** First ctrl-c during in-flight work: trigger `chat_cancel_token.cancel()`, abort the provider request / tool call, flush any partial response, print `"^C — turn cancelled"`, clear the in-flight flag, return to the prompt with conversation state preserved.
- **D-12:** Second ctrl-c within 1.5 seconds (configurable constant, NOT config-file): persist session state (mirror `/quit` path — `state_store.end_session(session_id, "interrupted")`), print `"Goodbye!"`, exit cleanly with status 0.
- **D-13:** Counter resets on: (a) user input received, (b) successful turn completion, (c) 1.5s window expires. A fresh `CancellationToken` must be installed after each cancellation.
- **D-14:** At the prompt (not in-flight), ctrl-c behaves like today's rustyline Interrupted branch — prints `"^C — type /quit to exit"` and loops. Double-tap at the prompt does NOT exit. The double-ctrl-c exit path is specifically for the in-flight case.

**Architecture**
- **D-15:** Create a new module `crates/ironhermes-cli/src/tui.rs` (or `tui/` dir if it grows) that owns `StatusLine`, `KnightRider`, and a single `tokio::task` rendering loop that ticks every 100ms.
- **D-16:** Status-line rendering uses `crossterm::cursor` + `crossterm::terminal`. Do NOT add `ratatui` or other heavy TUI crates.
- **D-17:** Render the status bar + knight rider to **stderr** on the bottom two lines using absolute cursor positioning; let rustyline own stdout for the input prompt.
- **D-18:** No dependency additions this phase.

**Testing**
- **D-19:** Unit test the alternating-color rotation function (deterministic, no IO).
- **D-20:** Unit test the knight-rider frame generator (deterministic).
- **D-21:** Integration test the double-ctrl-c counter state machine (simulate signal pairs, assert token behavior). Use a test-only trait or direct state inspection — do NOT send actual SIGINT.
- **D-22:** Manual verification: run `cargo run -- chat`, verify status line appears, knight rider animates during a tool call, ctrl-c cancels mid-stream, double-tap exits.

### Claude's Discretion
- Exact pill color rotation sequence (starting index, 24-bit vs ANSI-16)
- Knight-rider glyphs (`█▓▒░` fade vs simpler `■ ■ ·` style)
- Dot-separator character (`·` vs `•` vs `│`)
- Model-name truncation (short vs full)
- Error recovery on terminal resize (SIGWINCH)

### Deferred Ideas (OUT OF SCOPE)
- Full ratatui migration
- Mouse support, click-to-expand subagent tree
- Customizable color themes / user-config palette
- Persistent history scroll-back UI
- Gateway bot TUI
</user_constraints>

<phase_requirements>
## Phase Requirements

This phase has no mapped REQ-IDs in `.planning/REQUIREMENTS.md`. The decisions D-01 through D-22 in CONTEXT.md **ARE** the requirements for this phase.

The REQUIREMENTS.md traceability table (GW-01..GW-11) mapping "Phase 21" refers to an earlier roadmap layout and is stale for the gateway-architecture items. Phase 21 as currently scoped is pure CLI UX polish.

| ID | Description | Research Support |
|----|-------------|------------------|
| D-01..D-05 | Status line + pill colors | §Standard Stack (crossterm cursor API), §Code Examples (ExternalPrinter pattern) |
| D-06..D-09 | Knight Rider scanner | §Code Examples (frame generator), §Don't Hand-Roll (no existing crate fits) |
| D-10..D-14 | Double ctrl-c | §Common Pitfalls (rustyline raw-mode consumes Ctrl+C), §Code Examples (tokio::select! + CancellationToken) |
| D-15..D-17 | tui.rs module | §Architecture Patterns |
| D-18 | No new deps | §Standard Stack (all listed libs already in Cargo.toml) |
| D-19..D-22 | Tests | §Validation Architecture |
</phase_requirements>

## Summary

Phase 21 polishes the IronHermes CLI REPL UX on the stack already in `Cargo.toml`: `crossterm = "0.28"`, `colored = "3"`, `rustyline = "15"`, `tokio`, `tokio-util = "0.7"` with the `rt` feature (CancellationToken). No dependency additions are required — all three deliverables (bottom status line, Knight Rider scanner, graceful double ctrl-c) are buildable today.

The single architectural constraint that shapes everything else is **rustyline owns stdout during `rl.readline()`**. This means the conventional approach (spawn a rendering task that `print!`s to stdout) will race and corrupt the prompt line. Rustyline 15 solves this with the `ExternalPrinter` API (`rl.create_external_printer()` — confirmed present in v15.0.0) which serializes prints through the readline state machine. However, `ExternalPrinter::print` is designed for scrollback-style messages, not absolute-position bottom bars. The robust pattern is therefore:

1. Render status line + Knight Rider to **stderr** (not stdout) using absolute `crossterm::cursor::MoveTo` to the bottom rows, guarded by `SavePosition`/`RestorePosition`.
2. Queue commands with `crossterm::queue!` and flush atomically to avoid tearing mid-frame.
3. Drive both elements from a single `tokio::task` ticking every 100ms, reading activity state via `tokio::sync::watch` (not Mutex — no contention with high-rate streaming callbacks).

For double ctrl-c, rustyline intercepts SIGINT during `rl.readline()` (converting it to `ReadlineError::Interrupted` — this is the existing code path). `tokio::signal::ctrl_c()` fires only when rustyline is NOT active (i.e., during `agent.run(...)`). The idiomatic pattern is `tokio::select! { _ = agent.run(...) => ..., _ = ctrl_c() => cancel_token.cancel() }` around the agent call only. A plain `std::time::Instant` guards the 1.5s debounce window for the second ctrl-c → exit transition. A fresh `CancellationToken` is created after each cancel because `CancellationToken` cannot be un-cancelled.

**Primary recommendation:** Build `tui.rs` with three pure-function cores (`pill_rotate`, `knight_rider_frame`, `debounce_decide`) + one `tokio::task` render loop driven by `tokio::sync::watch<ActivityState>`. Keep render I/O on stderr with `crossterm::queue!` + explicit flush. For ctrl-c, wrap the `agent.run()` future only — not rustyline — in `tokio::select!` with `ctrl_c()`. Use `rustyline::Config::builder().auto_add_history(true).build()` with `enable_signals(false)` (the default) to preserve the current `ReadlineError::Interrupted` path. Ship a 50-line pure-function test suite covering the three cores + a state-machine test harness for the double-tap debounce that injects `cancel_token.cancel()` directly (no real SIGINT).

## Project Constraints (from CLAUDE.md)

`./CLAUDE.md` does not exist in the repo root. No project-level constraints beyond CONTEXT.md decisions. The global `~/.claude/CLAUDE.md` (oh-my-claudecode) governs agent behavior but does not constrain IronHermes implementation details. [VERIFIED: Read tool returned "File does not exist" for /Users/twilson/code/ironhermes/CLAUDE.md]

## Standard Stack

### Core (all already in `Cargo.toml` — NO new deps)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `crossterm` | 0.28 (workspace) | Cursor positioning, terminal size, raw-mode, queue! macro | Only cross-platform Rust terminal lib that doesn't pull a full TUI framework [VERIFIED: Cargo.toml line 64, `crossterm = "0.28"`] |
| `rustyline` | 15.0.0 | Readline with history and `ExternalPrinter` | In repo today; v15 has stable `create_external_printer()` [VERIFIED: Cargo.lock line 3397-3398; source-verified at github.com/kkawakam/rustyline/blob/v15.0.0/src/lib.rs line 1044] |
| `colored` | 3 | ANSI coloring via `.cyan()`, `.dimmed()` etc. | Already the entire codebase's color convention [VERIFIED: Cargo.toml line 65] |
| `tokio` | workspace | Async runtime, `tokio::signal::ctrl_c`, `tokio::sync::watch` | Already the async runtime [VERIFIED: Cargo.toml line 36] |
| `tokio-util` | 0.7 `rt` | `CancellationToken` (cooperative cancel) | Already wired through `chat_cancel_token` and `register_delegate_task_tool` [VERIFIED: Cargo.toml line 37; repo grep at main.rs:409] |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `std::time::Instant` | stdlib | 1.5s debounce window for double-ctrl-c | Avoid bringing `tokio::time::Instant` into pure-function state machine |
| `std::sync::atomic::AtomicBool` | stdlib | Alternative to `watch::channel` for "in-flight" flag if watch feels heavy | Only if profiling shows watch sends cost too much with high streaming-callback rates |
| `tokio::sync::watch` | (in tokio) | Shared activity state between callbacks and render task | `tokio::sync::watch::channel::<ActivityState>(Idle)` — receiver sees only latest value, no backpressure, cheap `send()` [CITED: tokio.rs/tokio/tutorial/select] |

### Alternatives Considered

| Instead of | Could Use | Tradeoff | Why Rejected |
|------------|-----------|----------|--------------|
| `crossterm` absolute positioning | `ratatui` | Full TUI framework with layouts, widgets | CONTEXT.md D-16, D-18 explicitly forbid |
| `rustyline` + external_printer | `reedline` (nushell's editor) | Built-in status-bar + prompt multiline support | Swaps the editor dep — out of scope; rustyline `ExternalPrinter` is sufficient |
| `tokio::sync::watch` | `Arc<Mutex<ActivityState>>` | Simpler | Streaming callback fires per-token; watch has lock-free read and single-writer semantics |
| `tokio::sync::watch` | `tokio::sync::broadcast` | Supports multiple listeners | Only one listener (render task); watch is cheaper |
| `tokio::signal::ctrl_c()` | `ctrlc` crate | Synchronous handler | `tokio::signal::ctrl_c` already integrates with tokio and needs no new dep [CITED: docs.rs/tokio/latest/tokio/signal/fn.ctrl_c.html] |

**Installation:**
```bash
# No action — all deps already resolved.
# Version-verify at phase start:
cargo tree -p ironhermes-cli | grep -E "rustyline|crossterm|colored|tokio-util"
```

**Version verification performed:**
- `rustyline = "15.0.0"` — [VERIFIED: Cargo.lock:3398, 2026-04-16]
- `crossterm = "0.28"` — workspace root [VERIFIED: Cargo.toml:64, 2026-04-16]
- `colored = "3"` — [VERIFIED: Cargo.toml:65]
- Current rustyline on crates.io is 17.0.2 [CITED: docs.rs/crate/rustyline/latest/source/README.md]. We stay on 15 (no upgrade needed; `ExternalPrinter` stable in 15).

## Architecture Patterns

### Recommended Module Layout
```
crates/ironhermes-cli/src/
├── main.rs              # Existing. Adds 4 call sites: create_tui, activity.send(Working),
│                        # activity.send(Idle), tui.shutdown()
└── tui/
    ├── mod.rs           # Public API: TuiHandle, ActivityState, spawn_render_task()
    ├── status_line.rs   # StatusLine::render() — pure fn producing String (testable)
    ├── knight_rider.rs  # frame_for_tick(tick: u64) -> String (pure, testable)
    ├── pills.rs         # rotate_colors(pills: &[&str]) -> Vec<ColoredString>
    └── ctrl_c.rs        # DoubleCtrlCState — pure state machine for D-10..D-14
```

Rationale: four tiny files each pure-function-heavy scale well with test coverage. `mod.rs` holds the only code that does I/O (the render task).

### Pattern 1: rustyline `ExternalPrinter` for "don't corrupt the prompt"

**What:** rustyline 15 exposes `rl.create_external_printer()` which returns an `impl ExternalPrinter + Send` (confirmed via source at rustyline master & v15 tag). Calls to `printer.print(msg)` serialize with rustyline's internal readline state — the prompt line is redrawn after the message.

**When to use:** scrollback-style messages during a live prompt (e.g., streamed agent tokens, tool-progress lines).

**When NOT to use:** absolute-position bottom-bar rendering. ExternalPrinter always writes into scrollback; it cannot draw at a fixed row. For the status line and Knight Rider we use crossterm absolute positioning instead.

**Example:**
```rust
// Source: github.com/kkawakam/rustyline/blob/master/examples/external_print.rs [VERIFIED]
let mut rl = DefaultEditor::new()?;
let mut printer = rl.create_external_printer()?;
thread::spawn(move || {
    loop {
        printer.print(format!("External message #{i}"))
            .expect("External print failure");
        thread::sleep(Duration::from_millis(1000));
    }
});
```

### Pattern 2: Bottom-Bar Render via Absolute Positioning (the D-17 approach)

**What:** Reserve the last 2 rows for our UI. Every 100ms the render task saves the cursor, moves to bottom-1 row, clears, writes status line, moves to bottom-0 row (left edge), writes Knight Rider frame, restores cursor.

**Key crossterm commands (all in dep already):**
- `crossterm::terminal::size()` → `(cols, rows)` — compute bottom row index
- `crossterm::cursor::SavePosition` / `RestorePosition` — `\x1B7` / `\x1B8` ANSI escape [CITED: docs.rs/crossterm/latest/crossterm/cursor/struct.SavePosition.html]
- `crossterm::cursor::MoveTo(col, row)` — absolute position, 0-indexed
- `crossterm::terminal::Clear(ClearType::CurrentLine)`
- `crossterm::queue!(stderr, …)` + `stderr.flush()` — atomic frame write

**Example:**
```rust
// Source: docs.rs/crossterm + repo Cargo.toml:64
// [VERIFIED API signatures from docs.rs/crossterm/latest/crossterm/cursor]
use crossterm::{queue, cursor::{MoveTo, SavePosition, RestorePosition, Hide, Show},
                terminal::{size, Clear, ClearType}};
use std::io::{stderr, Write};

fn redraw(status: &str, scanner: &str) -> std::io::Result<()> {
    let (cols, rows) = size()?;
    let mut out = stderr();
    queue!(
        out,
        SavePosition,
        Hide,
        MoveTo(0, rows.saturating_sub(1)), Clear(ClearType::CurrentLine), Print(status),
        MoveTo(0, rows.saturating_sub(0)), Clear(ClearType::CurrentLine), Print(scanner),
        Show,
        RestorePosition,
    )?;
    out.flush()
}
```

**Important:** `crossterm::cursor::Hide` during the redraw prevents visible cursor flicker at the status-line row — then restored to the prompt row by `RestorePosition`. [CITED: crossterm docs]

**CAVEAT on `rows - 1`:** Terminal rows are 0-indexed and `size()` returns 1-indexed count. Bottom row is `rows - 1`, so status (line N-1) + input prompt (line N-2 held by rustyline). We must account for rustyline moving the prompt as it scrolls — in practice rustyline and our status line will fight for the last-row cell. **Mitigation:** write status bar to `rows - 1` and let rustyline's prompt be one line up via the fact that rustyline always reserves one blank line after its prompt in DefaultEditor mode. This is the same approach mprocs / atuin use. [ASSUMED — validate on first build in iTerm2, macOS Terminal, xterm]

### Pattern 3: Single Render Task with `watch::channel` for activity state

**What:** One `tokio::task` reads `activity_rx.borrow().clone()` each tick and draws. Callbacks (`with_streaming`, `with_tool_progress`) do `activity_tx.send(ActivityState::Streaming)` etc. Watch channel has no backpressure — rapid streaming callbacks coalesce into "latest value wins," which is exactly what we want. [CITED: docs.rs/tokio/latest/tokio/sync/watch]

**Why not Mutex:** Streaming callbacks fire per-token (potentially thousands/sec). `Mutex::lock()` contention would serialize the agent's stream against the renderer. `watch::Sender::send` is lock-free for a single sender.

**Why not RwLock:** Over-engineered; we only ever write from callbacks and read from the render loop.

**Example:**
```rust
// tui/mod.rs
#[derive(Clone, Debug)]
pub enum ActivityState {
    Idle,
    Streaming,
    ToolCall { name: String },
    Thinking,
}

pub struct TuiHandle {
    activity_tx: tokio::sync::watch::Sender<ActivityState>,
    render_task: tokio::task::JoinHandle<()>,
    shutdown: tokio_util::sync::CancellationToken,
}

impl TuiHandle {
    pub fn set_activity(&self, state: ActivityState) {
        let _ = self.activity_tx.send(state);  // ignore lag: always latest-wins
    }
    pub async fn shutdown(self) {
        self.shutdown.cancel();
        let _ = self.render_task.await;
    }
}
```

### Pattern 4: tokio::select! around `agent.run()` only

**What:** Wrap only the agent future in `select!` with `tokio::signal::ctrl_c()`. Do NOT wrap `rl.readline()` — rustyline handles SIGINT itself and converts to `ReadlineError::Interrupted`.

**Example:**
```rust
// run_chat's turn dispatch
let run_fut = run_agent_turn(/* … */);
let result = tokio::select! {
    r = run_fut => r?,
    _ = tokio::signal::ctrl_c() => {
        // First ctrl-c during in-flight → cancel and continue REPL
        cancel_token.cancel();
        // drain with a timeout so AgentLoop sees the cancel_token
        // (agent_loop.rs:465-483 already checks token.cancelled() in select!)
        println!("{}", "^C — turn cancelled".dimmed());
        last_cancel_at = Some(Instant::now());
        // Install fresh token for next turn (D-13)
        cancel_token = CancellationToken::new();
        continue;  // back to readline
    }
};
```

Then inside the readline loop, on each iteration, check `last_cancel_at` elapsed <1.5s and a second ctrl-c arrived — but this specific check lives in the ctrl-c branch itself with `tokio::signal::ctrl_c()` fired a second time while still inside the `tokio::select!`. See §Common Pitfalls: rustyline eats Ctrl+C at prompt for why this works.

### Anti-Patterns to Avoid

- **Spawning a background task that `print!`s to stdout during rustyline.** The prompt will be overwritten and scrolled incorrectly. Use stderr + absolute positioning OR rustyline's `ExternalPrinter`.
- **Calling `io::stdout().flush()` from multiple tasks without coordination.** Interleaves partial ANSI escape sequences and corrupts the display.
- **Using `Arc<Mutex<ActivityState>>` for hot-path updates.** The streaming callback can fire per-token; serializing on a mutex creates visible jitter.
- **Handling ctrl-c with a synchronous `ctrlc::set_handler`.** Creates a process-global handler that fights tokio. Stay with `tokio::signal::ctrl_c()`. [CITED: docs.rs/tokio/latest/tokio/signal]
- **Forgetting to issue a fresh `CancellationToken` after cancel.** `CancellationToken::cancel()` is permanent; any subsequent `is_cancelled()` check returns `true` forever. D-13 is explicit about this.
- **Writing to the alt screen.** Would hide the user's conversation history on exit — antithetical to REPL UX.
- **Calling `crossterm::terminal::enable_raw_mode()`.** Rustyline already enters raw mode during `readline()`. Double-enabling breaks terminal state.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Cross-platform terminal size | `ioctl TIOCGWINSZ` wrapper | `crossterm::terminal::size()` | Windows Console API + Unix ioctl + WASI all handled |
| Cursor save/restore | Raw ANSI `\x1B7` / `\x1B8` | `crossterm::cursor::{SavePosition, RestorePosition}` | crossterm emits correct sequences per backend |
| ANSI color | Format! escape codes | `colored::Colorize` | Already the convention; auto-respects NO_COLOR env |
| Async SIGINT | `signal-hook` + custom bridge | `tokio::signal::ctrl_c()` | Already in tokio; no new dep |
| Cooperative cancel | Custom `Arc<AtomicBool>` flag | `tokio_util::sync::CancellationToken` | Already threaded through agent loop via `with_cancellation_token` [VERIFIED: agent_loop.rs:200] |
| Readline with history | Custom line buffer | `rustyline::DefaultEditor` | Already in use; battle-tested |
| Shared "latest value" state | `Arc<Mutex<T>>` polled by render task | `tokio::sync::watch::channel::<T>()` | No lock, no backpressure, single-writer friendly |

**Key insight:** Every primitive this phase needs is already in the dep graph. The design challenge is composition, not implementation.

## Runtime State Inventory

**Not applicable.** This phase has no rename/refactor/migration component. All changes are additive code in `crates/ironhermes-cli/src/`:

- Stored data: **None** — no DB/schema/collection changes.
- Live service config: **None** — no external services configured from this code path.
- OS-registered state: **None** — no scheduler/pm2/systemd involvement.
- Secrets/env vars: **None** — no new env-var reads.
- Build artifacts: **None** — incremental compile of ironhermes-cli crate only.

(Section included per the Step 2.5 protocol with explicit "None" per category.)

## Common Pitfalls

### Pitfall 1: Rustyline consumes Ctrl+C before `tokio::signal::ctrl_c()` sees it

**What goes wrong:** During `rl.readline()`, rustyline enables termios raw mode which disables the OS-level SIGINT-on-Ctrl+C generation. Rustyline reads the raw byte `\x03` directly and returns `Err(ReadlineError::Interrupted)`. The `tokio::signal::ctrl_c()` future **never wakes** because no SIGINT was ever delivered.

**Why it happens:** Raw-mode terminals don't generate SIGINT; the ISIG termios flag is cleared. [CITED: viewsourcecode.org/snaptoken/kilo/02.enteringRawMode.html and rust-cli.github.io/book/in-depth/signals.html]

**How to avoid:** Don't try to make both mechanisms cooperate. Use them in disjoint phases:
- At the readline prompt → rustyline's `ReadlineError::Interrupted` branch (current code at main.rs:558)
- During `agent.run(...)` → `tokio::signal::ctrl_c()` in a `tokio::select!` arm

D-14 already matches this (double-tap at prompt does NOT exit). [VERIFIED: CONTEXT.md D-14]

**Warning signs:** Dev hits Ctrl+C mid-stream, nothing happens. Means the `select!` arm is missing or the agent future isn't `cancel_token`-aware.

### Pitfall 2: `CancellationToken::cancel()` is permanent

**What goes wrong:** After `chat_cancel_token.cancel()`, every call to `is_cancelled()` returns `true` forever. Next turn's agent loop thinks it's already cancelled and returns `"Cancelled by parent"` immediately.

**Why it happens:** `CancellationToken` is a single-fire latch; no `reset()` method exists. [CITED: docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html]

**How to avoid:** Create a fresh `CancellationToken::new()` after each cancel, reassign `chat_cancel_token`, and re-thread it into tools that need it. D-13 explicitly requires this.

**Implementation note:** The CancellationToken is also held by `register_delegate_task_tool`. Either (a) re-register the delegate_task tool per turn (cheap — a few lines of Arc swapping) or (b) use `CancellationToken::child_token()` pattern where a parent lives for the session and a child is spawned per turn. Option (b) is cleaner — `parent.child_token()` gives a token that's cancelled when the parent or when `.cancel()` is called on the child. [CITED: docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html#method.child_token]

**Warning signs:** Second user message after a cancelled first turn immediately responds "Cancelled by parent" with no LLM call.

### Pitfall 3: Status bar flickers during rapid redraws

**What goes wrong:** Each 100ms tick redraws the bottom 2 rows. Without `Hide`/`Show`, the cursor is briefly seen at position (0, rows-1) between `MoveTo` and `RestorePosition`, causing visible flicker.

**Why it happens:** Most terminals render every ANSI command as soon as the byte arrives (no transaction boundary).

**How to avoid:** (a) `crossterm::cursor::Hide` as the first queued command, (b) write full status line + scanner, (c) `Show` as the last queued command before `RestorePosition`, (d) flush once per frame. Modern terminals also support `BeginSynchronizedUpdate` / `EndSynchronizedUpdate` (mode 2026) which crossterm exposes; use if targeting iTerm2 only. [CITED: docs.rs/crossterm/latest/crossterm/terminal/struct.BeginSynchronizedUpdate.html]

**Warning signs:** Visible cursor "bounce" at the bottom row on each tick.

### Pitfall 4: SIGWINCH (terminal resize) desyncs the status bar position

**What goes wrong:** User resizes terminal → `rows` changes → status bar is now drawn at the old row index → garbage on a mid-screen line.

**Why it happens:** We cached `rows` at spawn time OR we redraw based on a stale `size()`.

**How to avoid:** Call `crossterm::terminal::size()` each tick (cheap — a single `ioctl`). Do NOT subscribe to SIGWINCH; just re-query per frame. macOS and Linux xterm both return current dimensions immediately. [CITED: docs.rs/crossterm/latest/crossterm/terminal/fn.size.html]

**Warning signs:** After resizing, the status bar appears on a line mid-scrollback.

### Pitfall 5: `ExternalPrinter::print` redraws the prompt on each call → jitter during streaming

**What goes wrong:** The streaming callback fires per-token (potentially 50+/sec). Routing every delta through `printer.print(delta)` causes the prompt to redraw 50×/sec, jittering.

**Why it happens:** ExternalPrinter was designed for occasional messages, not high-frequency output. [CITED: generalistprogrammer.com/tutorials/rustyline-rust-crate-guide]

**How to avoid:** Route streaming deltas directly to stdout via the existing `print!("{}", delta)` + `stdout.flush()` path (current main.rs:601). Only route coarse-grained "activity changed" events (Idle → Streaming → ToolCall → Idle) through the render task's activity state. High-frequency token output is fine on stdout because rustyline is NOT active (readline returned before agent.run() started).

**Warning signs:** Stream output visible but CPU hot, prompt flickering rapidly.

### Pitfall 6: Writing to stderr conflicts with `eprintln!` in other code paths

**What goes wrong:** `subagent_progress` callback at main.rs:417-438 already uses `eprintln!`. If the render task's `MoveTo(0, rows-1)` collides with a concurrent `eprintln!`, the subagent progress line lands in the status-bar row.

**Why it happens:** Both paths share `stderr` with no synchronization.

**How to avoid:** Serialize all stderr through one channel: either (a) the render task owns the only `Stderr` handle and `SubagentProgress` events get pushed via the same `activity_tx` channel as `ActivityState::SubagentRunning { id, task }`, or (b) wrap stderr in an `Arc<Mutex<Stderr>>` and acquire for the full duration of `queue! + flush`. Option (a) is cleaner and matches D-08's replacement of ad-hoc `\r Running: {tool}...` output.

**Warning signs:** Subagent progress lines appearing at random scroll positions overlapping the status bar.

### Pitfall 7: tokio::signal::ctrl_c permanently overrides SIGINT

**What goes wrong:** Even after dropping the ctrl_c future, subsequent SIGINT is captured by tokio; the default "terminate the process" behavior is gone. If the user Ctrl+C's **during shutdown**, nothing happens and the process hangs.

**Why it happens:** tokio installs a process-global signal handler on first poll and never removes it. [CITED: docs.rs/tokio/latest/tokio/signal/fn.ctrl_c.html — "even if this Signal instance is dropped, subsequent SIGINT deliveries will end up captured by Tokio, and the default platform behavior will NOT be reset"]

**How to avoid:** Always have a live `ctrl_c()` future somewhere in a `select!` arm, OR accept that `/quit` / D-12's clean-exit path is the only way out. Since D-12 already specifies the clean exit, this is fine in practice — but add a fallback at the top of `run_chat` to install a "hard exit on 3rd ctrl-c within 3s" escape hatch to avoid a hung-shell footgun.

**Warning signs:** User Ctrl+C during goodbye sequence hangs the terminal.

## Code Examples

Verified patterns from official sources. Annotate with source URL + date.

### Example 1: ExternalPrinter from another thread (the canonical rustyline pattern)

```rust
// Source: github.com/kkawakam/rustyline/blob/master/examples/external_print.rs
// [VERIFIED: fetched 2026-04-16]
use std::thread;
use std::time::Duration;
use rustyline::{DefaultEditor, ExternalPrinter as _, Result};

fn main() -> Result<()> {
    let mut rl = DefaultEditor::new()?;
    let mut printer = rl.create_external_printer()?;
    thread::spawn(move || {
        let mut i = 0usize;
        loop {
            printer.print(format!("External message #{i}"))
                .expect("External print failure");
            thread::sleep(Duration::from_millis(1000));
            i += 1;
        }
    });

    loop {
        let line = rl.readline("> ")?;
        rl.add_history_entry(line.as_str())?;
        println!("Line: {line}");
    }
}
```

**Key APIs (rustyline 15.0.0):**
- `Editor::create_external_printer(&mut self) -> Result<<Terminal as Term>::ExternalPrinter>` [VERIFIED: github.com/kkawakam/rustyline/blob/v15.0.0/src/lib.rs line 1044]
- `trait ExternalPrinter { fn print(&mut self, msg: String) -> Result<()>; }` [VERIFIED: docs.rs/rustyline/15.0.0]
- Returned printer is `Send` (thread::spawn works without Sync). [CITED: example above demonstrates Send]

### Example 2: tokio::select! with ctrl_c + CancellationToken

```rust
// Source: tokio.rs/tokio/topics/shutdown + docs.rs/tokio/latest/tokio/signal
// [CITED: official Tokio tutorial]
use tokio::signal;
use tokio_util::sync::CancellationToken;

async fn run_turn(cancel_token: CancellationToken, /* …deps… */) -> Result<Outcome> {
    tokio::select! {
        // Wrap agent.run() — it checks cancel_token itself (agent_loop.rs:465-483)
        result = agent.run(messages.clone()) => result,

        // Ctrl-c: flip cancel flag and wait for agent to notice
        _ = signal::ctrl_c() => {
            cancel_token.cancel();
            // Agent will see cancelled flag and return "Cancelled by parent"
            // on its next iteration (up to ~1 HTTP round-trip latency)
            Ok(Outcome::Cancelled)
        }
    }
}
```

### Example 3: Bottom-bar absolute positioning with crossterm

```rust
// Source: docs.rs/crossterm APIs (docs.rs/crossterm/latest/crossterm/cursor,
// docs.rs/crossterm/latest/crossterm/terminal) + Medium example
// [CITED: medium.com/@otukof/build-your-text-editor-with-rust-part-4]
use crossterm::{
    cursor::{Hide, MoveTo, RestorePosition, SavePosition, Show},
    queue,
    style::Print,
    terminal::{size, Clear, ClearType},
};
use std::io::{stderr, Write};

pub fn draw_bottom_bar(status_line: &str, scanner: &str) -> std::io::Result<()> {
    let (_cols, rows) = size()?;  // current, handles SIGWINCH implicitly per-frame
    let bottom = rows.saturating_sub(1);
    let scanner_row = rows.saturating_sub(2);  // one row above bottom
    let mut out = stderr();
    queue!(
        out,
        SavePosition,
        Hide,
        MoveTo(0, scanner_row),
        Clear(ClearType::CurrentLine),
        Print(scanner),
        MoveTo(0, bottom),
        Clear(ClearType::CurrentLine),
        Print(status_line),
        Show,
        RestorePosition,
    )?;
    out.flush()
}
```

### Example 4: Pure Knight Rider frame generator (testable)

```rust
// Original — composition of stdlib + `colored`
// Pure function: no I/O, deterministic, easy to property-test
use colored::Colorize;

const TRACK_WIDTH: usize = 10;

/// Given a monotonic tick, produce the 10-cell Knight Rider frame.
/// Triangle wave over TRACK_WIDTH cells: 0 → 9 → 0 → 9…
pub fn knight_rider_frame(tick: u64) -> String {
    let period = (TRACK_WIDTH as u64 - 1) * 2;
    let phase = tick % period;
    let lit = if phase < TRACK_WIDTH as u64 {
        phase as usize
    } else {
        (period - phase) as usize
    };

    (0..TRACK_WIDTH)
        .map(|i| {
            let distance = (i as i32 - lit as i32).unsigned_abs() as usize;
            match distance {
                0 => "█".bright_cyan().to_string(),
                1 => "▓".cyan().to_string(),
                2 => "▒".cyan().dimmed().to_string(),
                _ => "░".dimmed().to_string(),
            }
        })
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn triangle_wave_spans_full_width() {
        // lit-cell index should reach 0 and 9 (endpoints) across one period
        let positions: Vec<usize> = (0..18)
            .map(|t| {
                let period = 18u64;
                let phase = (t as u64) % period;
                if phase < 10 { phase as usize } else { (period - phase) as usize }
            })
            .collect();
        assert!(positions.contains(&0));
        assert!(positions.contains(&9));
    }
    #[test]
    fn frame_width_is_constant() {
        // Must use char count (not byte len) — each cell is a multi-byte glyph
        for tick in 0..30 {
            assert_eq!(knight_rider_frame(tick).chars()
                .filter(|c| ['█','▓','▒','░'].contains(c)).count(), TRACK_WIDTH);
        }
    }
}
```

### Example 5: Pure pill color rotation (testable)

```rust
// Original — composition of stdlib + `colored`
use colored::{ColoredString, Colorize};

/// Rotate pill colors per D-04: cyan, magenta, green, yellow, dimmed.
/// The last pill (hint) is ALWAYS dimmed regardless of rotation.
pub fn rotate_pill_colors(pills: &[String], hint: Option<&str>) -> Vec<ColoredString> {
    let palette: [fn(&str) -> ColoredString; 5] = [
        |s| s.cyan(),
        |s| s.magenta(),
        |s| s.green(),
        |s| s.yellow(),
        |s| s.dimmed(),
    ];
    let mut out: Vec<ColoredString> = pills
        .iter()
        .enumerate()
        .map(|(i, p)| palette[i % palette.len()](p.as_str()))
        .collect();
    if let Some(h) = hint {
        out.push(h.dimmed());  // hint always dimmed (D-04)
    }
    out
}
```

### Example 6: Double-ctrl-c state machine (pure — testable)

```rust
// Original — pure fn, no tokio, no real SIGINT needed for tests
use std::time::{Duration, Instant};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CtrlCDecision {
    CancelTurn,          // first ctrl-c: cancel in-flight work, return to prompt
    ExitCleanly,         // second ctrl-c within window: persist + exit 0
    ShowPromptHint,      // at prompt, not in-flight: print "^C — type /quit"
}

pub struct DoubleCtrlCState {
    window: Duration,
    last_cancel_at: Option<Instant>,
}

impl DoubleCtrlCState {
    pub fn new() -> Self {
        Self { window: Duration::from_millis(1500), last_cancel_at: None }
    }

    /// Returns the decision for THIS ctrl-c event.
    /// Caller tracks in_flight externally.
    pub fn on_ctrl_c(&mut self, now: Instant, in_flight: bool) -> CtrlCDecision {
        if !in_flight {
            return CtrlCDecision::ShowPromptHint;
        }
        let within_window = self.last_cancel_at
            .map(|t| now.duration_since(t) < self.window)
            .unwrap_or(false);
        if within_window {
            CtrlCDecision::ExitCleanly
        } else {
            self.last_cancel_at = Some(now);
            CtrlCDecision::CancelTurn
        }
    }

    /// Reset on successful turn completion OR on fresh user input.
    pub fn reset(&mut self) { self.last_cancel_at = None; }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn first_ctrlc_in_flight_cancels() {
        let mut s = DoubleCtrlCState::new();
        assert_eq!(s.on_ctrl_c(Instant::now(), true), CtrlCDecision::CancelTurn);
    }
    #[test] fn second_ctrlc_within_window_exits() {
        let mut s = DoubleCtrlCState::new();
        let t0 = Instant::now();
        s.on_ctrl_c(t0, true);
        let t1 = t0 + Duration::from_millis(500);
        assert_eq!(s.on_ctrl_c(t1, true), CtrlCDecision::ExitCleanly);
    }
    #[test] fn second_ctrlc_after_window_cancels_again() {
        let mut s = DoubleCtrlCState::new();
        let t0 = Instant::now();
        s.on_ctrl_c(t0, true);
        let t1 = t0 + Duration::from_millis(1600);  // beyond 1500ms
        assert_eq!(s.on_ctrl_c(t1, true), CtrlCDecision::CancelTurn);
    }
    #[test] fn ctrlc_at_prompt_is_hint() {
        let mut s = DoubleCtrlCState::new();
        assert_eq!(s.on_ctrl_c(Instant::now(), false), CtrlCDecision::ShowPromptHint);
    }
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `\r Running: {tool}...` inline with agent output (today's main.rs:604-607) | Dedicated bottom row with Knight Rider + label | Phase 21 | Removes scroll pollution; persistent visibility |
| Rustyline only, ad-hoc `println!` from streaming callback | Rustyline + `ExternalPrinter` for out-of-band messages + crossterm absolute positioning for the bar | Rustyline 12 → 13+ added ExternalPrinter; we're on 15 | Eliminates prompt-corruption races |
| Synchronous `ctrlc::set_handler` | `tokio::signal::ctrl_c()` in `tokio::select!` | Tokio 1.0+ | Integrated with runtime; no cross-thread bridge needed |
| Raw `\x1B[…m` ANSI codes | `colored::Colorize` | Long established | Respects NO_COLOR, term capability detection |
| Full TUI (ratatui/cursive) | Minimal crossterm primitives | When scope fits | Keeps binary small, avoids framework lock-in |

**Deprecated/outdated:**
- **`ctrlc` crate** for async tokio apps: use `tokio::signal::ctrl_c()` instead — no separate handler installation, no threading bridge. [CITED: docs.rs/tokio/latest/tokio/signal]
- **`rustyline` signal bindings (`enable_signals(true)` in Config)**: default is `false`; leave it false. Setting to true makes SIGINT reach the outer handler during readline — we explicitly DON'T want that (we want the `ReadlineError::Interrupted` branch for D-14). [CITED: docs.rs/rustyline/latest/rustyline/config]

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Rustyline 15's `DefaultEditor` reserves exactly one line for the prompt, so writing to `rows - 1` and `rows - 2` doesn't collide with the input line | Pattern 2 (Bottom-Bar) | Status bar overlaps prompt on small terminals. Mitigation: test on 24-row tmux, iTerm2, macOS Terminal before shipping. |
| A2 | `tokio::sync::watch::Sender::send` from a hot streaming callback is cheaper than `Arc<Mutex<T>>::lock` under typical stream rates (~50 tokens/sec) | Pattern 3 | If false, scanner updates laggy. Mitigation: instrument with tracing and a benchmark in Wave 0. |
| A3 | `crossterm::terminal::size()` each 100ms tick is cheap enough not to matter (< 50µs on macOS/Linux per call) | Pitfall 4 | Elevated CPU on slow terminals. Mitigation: cache size and refresh every 1s, re-query on redraw error. |
| A4 | Synchronized-update (mode 2026) is NOT supported on macOS Terminal.app (only iTerm2) — so we should NOT rely on it | Pattern 2 | If supported, we could drop Hide/Show dance. Acceptable: current cursor Hide/Show is a well-known fallback. |
| A5 | Rustyline's raw-mode stays enabled only during `rl.readline()` — when that returns, stdin goes back to canonical mode and tokio::signal::ctrl_c() works normally | Pitfall 1 | If rustyline leaves raw mode on between calls, ctrl-c-during-agent won't fire SIGINT. Mitigation: smoke test on first build; if broken, explicitly call `crossterm::terminal::disable_raw_mode()` after readline. |
| A6 | The 1.5-second debounce window uses wall-clock `Instant`, not `tokio::time::Instant`, so the state machine stays testable without mocking the tokio clock | Example 6 | Low risk — `std::time::Instant` is deterministic in tests via injected `now` parameter. |
| A7 | Subagent progress and main-agent progress can share the same `ActivityState` enum — D-08 implies replacement of today's ad-hoc stderr output | Pitfall 6 | Could require richer state (tree view). But D-01 notes subagent tree is already handled separately via `SubagentProgressCallback` (main.rs:412-439); Phase 21 only replaces the single-agent `Running: {tool}` clutter, so a flat enum is sufficient. |
| A8 | `create_external_printer` returns a type that is `Send` (we can `tokio::spawn` with it). The example uses `thread::spawn` which also needs Send, confirming it. | Pattern 1 | Low risk. |

**All claims tagged with inline `[VERIFIED]`, `[CITED: url]`, or `[ASSUMED]` markers per the provenance rule. The assumptions above are the only items the planner/user should review before locking in the plan.**

## Open Questions

1. **Should the ctrl-c "exit cleanly" path (D-12) also flush memory via `MemoryManager::flush_to_disk`?**
   - What we know: `state_store.end_session(session_id, "interrupted")` is explicitly called out in D-12. Gateway's equivalent `on_session_end` flushes memory. CLI today at `run_chat` line ~572 calls `end_session("completed")` only — no memory flush.
   - What's unclear: whether "interrupted" should also trigger `memory_manager.flush()`.
   - Recommendation: mirror the gateway path — flush memory on D-12 exit. The planner should resolve this during task decomposition by grepping `on_session_end` in `crates/ironhermes-gateway`.

2. **Should the status line display the session_id, or just mode/model/provider?**
   - What we know: D-03 specifies `{mode} · {model_short} · {provider} · {tokens}/{limit} · {hint}`. No session_id.
   - What's unclear: whether useful in multi-session debugging.
   - Recommendation: stick to D-03 exactly. Defer to a future phase if needed.

3. **What does `{limit}` resolve to when using a provider we don't know the context window of?**
   - What we know: `AgentLoop` uses `with_compression(128_000, config.agent.context_compression)` in main.rs:598 — so 128K is the default assumed limit.
   - What's unclear: whether the status line should use a per-provider map (claude=200K, gpt-4=128K, etc.) or always show 128K.
   - Recommendation: initially show the hard-coded 128K (matches what the agent loop is using internally). Per-provider mapping is a follow-up.

4. **How does rustyline behave when the terminal is resized mid-`readline()`?**
   - What we know: rustyline 15 handles SIGWINCH internally (redraws prompt).
   - What's unclear: whether our background crossterm render overlaps rustyline's resize redraw, causing a 1-frame glitch.
   - Recommendation: accept occasional 1-frame glitch on resize. If severe, add a 200ms render pause on detected size change.

5. **Should we expose a `--no-tui` flag for users piping output or running under ssh-on-slow-link?**
   - What we know: CONTEXT.md doesn't mention.
   - What's unclear: degraded-terminal fallback.
   - Recommendation: detect `!crossterm::tty::IsTty::is_tty(&std::io::stderr())` and disable render task automatically. Low-effort belt-and-suspenders.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain (stable) | Compile crate | ✓ | present (project already builds) | — |
| cargo | Build | ✓ | present | — |
| `crossterm` 0.28 | TUI rendering | ✓ | 0.28 [VERIFIED: Cargo.toml] | — |
| `rustyline` 15 | Readline + ExternalPrinter | ✓ | 15.0.0 [VERIFIED: Cargo.lock:3398] | — |
| `colored` 3 | ANSI styling | ✓ | 3 [VERIFIED: Cargo.toml] | — |
| `tokio` (full) | Runtime + signal + sync::watch | ✓ | workspace | — |
| `tokio-util` 0.7 `rt` | CancellationToken | ✓ | 0.7 [VERIFIED: Cargo.toml:71] | — |
| TTY for stderr (manual QA) | D-22 manual test | ✓ | iTerm2 / macOS Terminal dev env | — |
| xterm / tmux (D-17 cross-terminal check) | Verification | — | not verified this session | Deferred to manual QA (per D-22); no blocker for planning |

**Missing dependencies with no fallback:** None — phase builds with current toolchain.

**Missing dependencies with fallback:** None.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | `cargo test` (built-in) — unit tests in `#[cfg(test)] mod tests` blocks inside each tui submodule |
| Config file | none (cargo standard) |
| Quick run command | `cargo test -p ironhermes-cli --lib tui::` |
| Full suite command | `cargo test -p ironhermes-cli` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| D-04 | Pill color rotation cycles cyan/magenta/green/yellow/dimmed; hint always dimmed | unit | `cargo test -p ironhermes-cli --lib tui::pills::tests::` | ❌ Wave 0 |
| D-05 | Stats reflect current token count (derived from `total_usage.total_tokens`) | unit | `cargo test -p ironhermes-cli --lib tui::status_line::tests::renders_token_count` | ❌ Wave 0 |
| D-06, D-07 | Knight Rider: triangle-wave sweep over 10 cells; fixed-width output | unit (property) | `cargo test -p ironhermes-cli --lib tui::knight_rider::tests::` | ❌ Wave 0 |
| D-08 | Scanner visible iff in-flight; idle state renders empty/hint | unit | `cargo test -p ironhermes-cli --lib tui::tests::idle_does_not_render_scanner` | ❌ Wave 0 |
| D-11 | First ctrl-c cancels in-flight, returns to prompt | integration (state-machine) | `cargo test -p ironhermes-cli --lib tui::ctrl_c::tests::first_ctrlc_in_flight_cancels` | ❌ Wave 0 |
| D-12 | Second ctrl-c within 1.5s → exit decision | integration (state-machine) | `cargo test -p ironhermes-cli --lib tui::ctrl_c::tests::second_ctrlc_within_window_exits` | ❌ Wave 0 |
| D-13 | 1.5s window expiry resets counter | integration (state-machine) | `cargo test -p ironhermes-cli --lib tui::ctrl_c::tests::second_ctrlc_after_window_cancels_again` | ❌ Wave 0 |
| D-14 | ctrl-c at prompt returns `ShowPromptHint` (never Exit) | unit | `cargo test -p ironhermes-cli --lib tui::ctrl_c::tests::ctrlc_at_prompt_is_hint` | ❌ Wave 0 |
| D-22 | Manual QA: chat, status line visible, scanner animates, ctrl-c works | manual | `cargo run -- chat` + visual inspection | — (human-only, per D-22) |

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-cli --lib tui::` (runs only the new module's pure-function tests — <5s)
- **Per wave merge:** `cargo test -p ironhermes-cli` (whole crate, ensures no regression in existing tests like the `run_chat_and_run_single_both_wire_memory_manager` static-grep regression from Phase 20)
- **Phase gate:** `cargo test --workspace` green before `/gsd-verify-work`, plus human signoff on D-22 manual verification.

### Wave 0 Gaps
- [ ] `crates/ironhermes-cli/src/tui/mod.rs` — module shell + `TuiHandle` public API
- [ ] `crates/ironhermes-cli/src/tui/status_line.rs` — covers D-03..D-05
- [ ] `crates/ironhermes-cli/src/tui/knight_rider.rs` — covers D-06, D-07
- [ ] `crates/ironhermes-cli/src/tui/pills.rs` — covers D-04
- [ ] `crates/ironhermes-cli/src/tui/ctrl_c.rs` — covers D-10..D-14
- [ ] `crates/ironhermes-cli/src/main.rs` mod tests — integration: "double-ctrl-c state transitions match in run_chat" (static-grep regression: verify `on_ctrl_c`, `tokio::select!`, fresh `CancellationToken` all present in run_chat)

### Invariants (Static-Grep Testable — to lock into the codebase)

Per D-20/D-21 and the Phase 20 precedent of locking structural invariants with static-grep tests:

- **INV-1:** `run_chat` contains a `tokio::select!` with `ctrl_c()` arm (grep `run_chat` scope for `signal::ctrl_c`)
- **INV-2:** A fresh `CancellationToken::new()` is constructed after each cancel in `run_chat` (grep post-cancel block for `CancellationToken::new()`)
- **INV-3:** `run_single` does NOT install ctrl_c handler (D-10) — grep `run_single` for absence of `ctrl_c`
- **INV-4:** The TUI render task always releases stderr with `RestorePosition` (grep `tui/mod.rs` for `RestorePosition` appearing after every `MoveTo`)
- **INV-5:** No `println!` call inside the tui module — all output via `stderr` (grep `tui/` for `println!` → must be zero)
- **INV-6:** No new crates added to `Cargo.toml` or workspace root (grep dep list before/after — diff must be empty). Enforces D-18.

## Security Domain

> Per research protocol, included by default. `security_enforcement` is not explicitly `false` in config.

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | — (CLI-local; no auth surface) |
| V3 Session Management | partial | session_id is already generated by `uuid::Uuid::new_v4()`; D-12's "interrupted" status propagates existing state_store semantics |
| V4 Access Control | no | — |
| V5 Input Validation | yes | User input via rustyline is already trimmed/length-bounded; no new input surface this phase |
| V6 Cryptography | no | — |
| V7 Error Handling & Logging | yes | Render-task errors must NOT panic (use `let _ = …` on `flush()` and `send()`); log to tracing::debug instead |
| V8 Data Protection | no | — |
| V11 Business Logic | partial | Double-ctrl-c state machine is the main new logic surface — covered by D-21 unit tests |
| V14 Configuration | yes | D-12 window is a compile-time constant (1.5s) per CONTEXT.md — not config-file driven (avoids supply-chain injection of adversarial debounce timing) |

### Known Threat Patterns for {crossterm/rustyline stack}

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| ANSI escape injection via model-generated text into status line | Tampering | `{model_short}` and `{provider}` are derived from our own config, not model output — no sanitization needed |
| Terminal state corruption on panic (raw-mode left on) | DoS | `std::panic::catch_unwind` in render task + `crossterm::terminal::disable_raw_mode()` in Drop; rustyline already handles this for its readline path |
| Signal handler race — user spams ctrl-c during shutdown and the 3rd+ ctrl-c is swallowed by tokio's permanent handler [CITED: tokio docs] | DoS | Either (a) accept — D-12 exits after 2nd; (b) add an emergency escape hatch: after 3rd ctrl-c within 3s, `std::process::exit(130)` directly |
| Truncated ANSI sequence mid-flush garbles terminal | Integrity | `crossterm::queue!` + single `flush()` per frame; acquire stderr lock for full frame |
| Log scraping: status line tokens (pct, provider) accidentally logged to tracing → leak | Info Disclosure | Render task only writes to stderr, not tracing subscribers; no persistence |

## Sources

### Primary (HIGH confidence)
- **Cargo.toml & Cargo.lock** (local) — dep versions verified at 2026-04-16
- **rustyline 15.0.0 source** — `github.com/kkawakam/rustyline/blob/v15.0.0/src/lib.rs` line 1044 — `create_external_printer` signature
- **rustyline external_print example** — `github.com/kkawakam/rustyline/blob/master/examples/external_print.rs` — canonical ExternalPrinter usage
- **crossterm docs** — `docs.rs/crossterm/latest/crossterm/cursor/struct.SavePosition.html`, `.../struct.RestorePosition.html`, `.../struct.MoveTo.html`, `.../terminal/index.html`
- **tokio signal docs** — `docs.rs/tokio/latest/tokio/signal/fn.ctrl_c.html` — fire-once semantics + permanent override warning
- **tokio-util CancellationToken docs** — `docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html` — child_token pattern
- **IronHermes source**:
  - `crates/ironhermes-cli/src/main.rs:374-575` (run_chat), `:580-640` (run_agent_turn), `:605-607` (current Running clutter)
  - `crates/ironhermes-agent/src/agent_loop.rs:30-37` (AgentResult fields), `:200-201` (with_cancellation_token), `:464-483` (select! around LLM call — already cancel-aware)
  - `crates/ironhermes-agent/src/pressure_warning.rs` (PressureTracker — already session-scoped and Arc-shareable)

### Secondary (MEDIUM confidence — verified with primary source)
- Tokio tutorial shutdown patterns — `tokio.rs/tokio/topics/shutdown`
- LogRocket Rust signal handling guide — `blog.logrocket.com/guide-signal-handling-rust/`
- rust-cli book on signals — `rust-cli.github.io/book/in-depth/signals.html`
- Medium text-editor-with-rust status-bar example — `medium.com/@otukof/build-your-text-editor-with-rust-part-4-fd4a8b8641f8`

### Tertiary (LOW confidence — treat as background only)
- Generalistprogrammer rustyline guide — `generalistprogrammer.com/tutorials/rustyline-rust-crate-guide` (ExternalPrinter use case — corroborates example)
- Rustyline issue #125 "Allow interrupts to be handled without reading a line" (historical context for SIGINT behavior; confirms raw-mode+SIGINT interaction)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all versions verified against Cargo.lock and Cargo.toml
- Architecture: HIGH — Rustyline ExternalPrinter and crossterm absolute positioning both source-verified
- Signal interaction (rustyline ↔ tokio::ctrl_c): MEDIUM — documented separately but never explicitly verified together in official docs. Raw-mode disables SIGINT generation [CITED: kilo tutorial] is the authoritative fact; its implication for our ctrl-c-during-agent-only design is ASSUMED until smoke-tested on first build (A5).
- Ctrl-c state machine: HIGH — pure logic, fully testable, no runtime dependencies
- Pitfalls: HIGH — each has a citation or verified source, plus explicit warning signs

**Research date:** 2026-04-16
**Valid until:** 2026-05-16 (30 days — Rust ecosystem is stable; rustyline 15 → 17 is backward-compatible for `ExternalPrinter`; no risk window)

**Caveat for planner:** CONTEXT.md's `{mode_short}` example uses "claude-sonnet-4" — confirm with user whether this is display-only short form or pulls from `anthropic_adapter` model strings. This is Claude's Discretion per CONTEXT.md but worth surfacing.
