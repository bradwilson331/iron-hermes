# Phase 21: Commandline UI Update — Context

**Gathered:** 2026-04-16
**Status:** Ready for planning
**Source:** User design direction provided inline with image reference during /gsd-plan-phase

<domain>
## Phase Boundary

Polish the interactive CLI UX in `crates/ironhermes-cli/` with two visible upgrades and one behavior fix:

1. **Persistent bottom status line** rendered while the CLI is running (chat/agent mode), with
   dot-separated stat pills using alternating colors.
2. **"Knight Rider" horizontal activity indicator** in the bottom-left, animating while
   a turn/stream/tool-call is in flight so the user has a visible heartbeat.
3. **Graceful double ctrl-c** (rolled-in todo, 2026-04-13): first ctrl-c cancels the
   in-flight turn/stream/tool-call and returns to the prompt without losing conversation
   state; second ctrl-c (within a short window) exits cleanly.

In scope: `run_chat` / `run_agent_turn` in `crates/ironhermes-cli/src/main.rs`, the
`with_streaming` / `with_tool_progress` callbacks, and a new rendering module for the
status line + activity indicator.

Out of scope: Gateway TUI (no interactive frontend), batch/cron subcommands,
non-interactive `run_single`, terminal multiplexing, mouse input, full TUI framework
migration (e.g., ratatui) — this phase stays on `colored` + `crossterm` primitives
already in the dep graph.

</domain>

<decisions>
## Implementation Decisions

### Visual Reference
- **D-01:** The target aesthetic is the attached reference image (OpenCode Zen-style
  terminal UI): clean dark background, dot-separated pill stats in the bottom status line,
  tree-style progress hierarchy, thin input prompt bar at the very bottom.
- **D-02:** Respect the existing IronHermes color identity — `.cyan()` for brand, `.green()`
  for affirmative, `.yellow()` for pending/warnings, `.dimmed()` for secondary text.
  Do NOT introduce a new color palette. Alternating colors for status pills draw from
  this palette (e.g., cyan / magenta / green / yellow rotating).

### Status Line (Bottom Bar)
- **D-03:** Render a persistent one-line status bar pinned to the terminal bottom while
  the REPL is active. Stats shown (left-to-right, dot-separated):
  `{mode} · {model_short} · {provider} · {tokens}/{limit} ({pct}%) · {hint}`
  Example: `Agent · claude-sonnet-4 · anthropic · 107.7K (54%) · ctrl+p commands`
- **D-04:** Pills use alternating colors. Rotation: pill[0]=cyan, pill[1]=magenta,
  pill[2]=green, pill[3]=yellow, pill[4]=dimmed. Dots (`·`) stay `.dimmed()`. Hint
  at the far right stays `.dimmed()` regardless of rotation.
- **D-05:** Stats update on a tick (every ~200ms) driven by the agent loop's pressure
  tracker / budget counter (`Arc<AtomicUsize>` already exists) — no new state
  machine. Token count comes from `PressureTracker` / `result.total_usage.total_tokens`.

### Knight Rider Activity Indicator
- **D-06:** Bottom-left activity indicator is a horizontal "scanner" bar (the Knight
  Rider effect): a bright segment that sweeps left↔right across a fixed-width track
  (e.g., 10 cells wide) while a turn or tool call is in flight.
- **D-07:** Track dimensions: 10 columns wide, 1 row tall. Lit cell is bright cyan,
  trailing cells fade with `.dimmed()` for a 2-3 cell tail. Frame rate ~10 fps (100ms
  per frame).
- **D-08:** The scanner is VISIBLE only when an agent turn or streaming call is active.
  When idle (at prompt), the indicator is hidden or replaced with `esc interrupt` hint.
  Replaces the current `\r Running: {tool}...` clutter in `run_agent_turn`'s
  `with_tool_progress` callback.
- **D-09:** Label shown to the right of the scanner: the current activity name
  (e.g., `Thinking`, `Running: bash`, `Streaming`). Uses the same callback surface
  (`with_streaming`, `with_tool_progress`) — no new plumbing from the agent crate.

### Graceful Double Ctrl-C (Rolled-In Todo)
- **D-10:** Install a `tokio::signal::ctrl_c` handler in the chat/agent run loop
  (NOT in `run_single` — that stays as-is, single-shot). Use the existing
  `chat_cancel_token: CancellationToken` that's already threaded through
  `register_delegate_task_tool`.
- **D-11:** First ctrl-c during in-flight work: trigger `chat_cancel_token.cancel()`,
  abort the provider request / tool call, flush any partial response to the screen,
  print `"^C — turn cancelled"`, clear the in-flight flag, return to the prompt with
  conversation state preserved.
- **D-12:** Second ctrl-c within 1.5 seconds (configurable constant, NOT config-file):
  persist session state (mirror `/quit` path — `state_store.end_session(session_id, "interrupted")`),
  print `"Goodbye!"`, exit cleanly with status 0.
- **D-13:** Counter resets on: (a) user input received, (b) successful turn completion,
  (c) 1.5s window expires. A fresh `CancellationToken` must be installed after each
  cancellation so the next turn has an un-cancelled token.
- **D-14:** At the prompt (not in-flight), ctrl-c behaves like today's rustyline
  Interrupted branch — prints `"^C — type /quit to exit"` and loops. Double-tap at
  the prompt does NOT exit (must use `/quit`). The double-ctrl-c exit path is
  specifically for the in-flight case where the user wants to bail out entirely.

### Architecture
- **D-15:** Create a new module `crates/ironhermes-cli/src/tui.rs` (or `tui/` dir if
  it grows) that owns:
  - `StatusLine` struct (stats state + render to stderr using crossterm cursor
    positioning — save cursor, move to bottom row, clear line, write, restore cursor).
  - `KnightRider` struct (animation frame counter + render).
  - A single `tokio::task` rendering loop that ticks every 100ms and redraws both
    elements. Started when `run_chat` begins, stopped on exit.
- **D-16:** Status-line rendering uses `crossterm::cursor` + `crossterm::terminal`
  (both already in deps via `crossterm = { workspace = true }`). Do NOT add `ratatui`
  or other heavy TUI crates.
- **D-17:** Rustyline + manual crossterm rendering is the known-hard case. Use
  rustyline's `set_helper` / prompt hooks if needed, but the simpler path is: render
  the status bar + knight rider to **stderr** on the bottom two lines using absolute
  cursor positioning; let rustyline own stdout for the input prompt. Test on iTerm2,
  macOS Terminal, and Linux xterm minimally.
- **D-18:** No dependency additions this phase. `crossterm`, `colored`, `tokio`,
  `rustyline` are sufficient.

### Testing
- **D-19:** Unit test the alternating-color rotation function (given N pills, produce
  N color-coded strings — deterministic, no IO).
- **D-20:** Unit test the knight-rider frame generator (given tick N, produce the
  correct 10-cell string — deterministic).
- **D-21:** Integration test the double-ctrl-c counter state machine (simulate signal
  pairs, assert token behavior). Use a test-only trait or direct state inspection —
  do NOT send actual SIGINT in tests.
- **D-22:** Manual verification: run `cargo run -- chat`, verify status line appears,
  verify knight rider animates during a tool call, verify ctrl-c cancels mid-stream,
  verify double-tap exits. This is a UI phase — automated tests cover logic, human
  eyes cover feel.

### Claude's Discretion
- Exact pill color rotation sequence (starting index, whether to use 24-bit vs ANSI-16)
- Whether the knight rider uses `█`, `▓`, `▒`, `░` for fade or simpler `■ ■ ·` style
- Character for the dot separator (`·` vs `•` vs `│`)
- Whether to show model name truncated (`claude-sonnet-4`) or full
- Error recovery when terminal resizes mid-render (SIGWINCH)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### CLI entrypoints (current state)
- `crates/ironhermes-cli/src/main.rs` — contains `run_chat` (line ~374), `run_agent_turn`
  (line ~580), `run_single` (line ~258), `print_banner` (line ~805). This is the primary
  file modified by this phase.
- `crates/ironhermes-cli/Cargo.toml` — confirms `crossterm`, `colored`, `rustyline`,
  `tokio-util` (CancellationToken) are already deps.

### Rolled-in todo
- `.planning/todos/pending/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md` —
  source spec for D-10..D-14. Must be moved to `.planning/todos/completed/` after phase ships.

### Agent loop surfaces (callbacks used by the TUI)
- `crates/ironhermes-agent/src/` — `AgentLoop::with_streaming`, `with_tool_progress`.
  Callbacks already fire per-delta and per-tool-call; the TUI plumbs these into the
  knight-rider label/activity state. Do NOT change the callback signatures.
- `crates/ironhermes-agent/src/` — `CancellationToken` is accepted by
  `register_delegate_task_tool`. The chat loop already creates `chat_cancel_token`
  (main.rs line ~409). Reuse it for the ctrl-c handler; don't introduce a second token.

### Project instructions
- `CLAUDE.md` (repo root, if present) — project-specific guidelines.

</canonical_refs>

<specifics>
## Specific Ideas

- Reference image is a terminal screenshot (user-provided) showing:
  - Tree-structured subagent progress with `└` and `┌` characters
  - `ctrl+x down view subagents` hint row
  - Italicized `Thinking:` blocks
  - Code blocks shown inline (no special formatting)
  - Bottom input line with `[]` placeholder
  - Status line: `Build · Big Pickle · OpenCode Zen · 107.7K (54%) · ctrl+p commands`
  - `esc interrupt` hint visible bottom-left
- IronHermes does not need to replicate the subagent tree view in this phase (that
  already exists at `main.rs:412-439` via `SubagentProgressCallback`). Phase 21 is
  specifically the **bottom status bar + scanner + ctrl-c** polish.
- Useful example for knight rider: the `indicatif` crate's spinner styles include
  `"←↖↑↗→↘↓↙"` patterns but NOT a horizontal sweep. This phase rolls its own 10-cell
  frame generator — it's ~20 lines of Rust.

</specifics>

<deferred>
## Deferred Ideas

- Full ratatui migration — out of scope; stay on crossterm primitives.
- Mouse support, click-to-expand subagent tree — future phase.
- Customizable color themes / user-config palette — future phase.
- Persistent history scroll-back UI (like Claude Code's) — future phase.
- Gateway bot TUI (telegram doesn't need one) — permanently out of scope.

</deferred>

---

*Phase: 21-commandline-ui-update-polish-cli-ux-including-graceful-doubl*
*Context gathered: 2026-04-16 via inline direction + image reference during /gsd-plan-phase*
