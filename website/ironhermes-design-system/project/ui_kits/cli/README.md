# CLI UI kit

High-fidelity web recreation of the IronHermes terminal UI, built directly from the Rust source in `crates/tui`.

## Files

- `index.html` — mounts everything and adds a small demo shelf (trigger tool call, run `/status`, etc.)
- `Terminal.jsx` — top-level layout; wires transcript + bottom bar; implements `/status /doctor /help /clear /model /personality /quit`
- `Transcript.jsx` — scrolling chat log with `You:` / `Hermes:` / `Tool:` prompts, and `[OK] / [MISSING]` doctor rows
- `Scanner.jsx` — knight-rider scanner (10 cells, 100 ms tick, triangle sweep). Matches `knight_rider.rs`
- `StatusLine.jsx` — `mode · model · provider · tokens · hint` with positional pill colors and `·` separators
- `Prompt.jsx` — bottom input with live slash-command suggestions (↑/↓/Tab)
- `terminal.css` — imports `../../colors_and_type.css` and adds terminal chrome

## Source-of-truth mapping

| Module       | Rust file                                   |
|--------------|---------------------------------------------|
| Scanner      | `crates/tui/src/knight_rider.rs`            |
| StatusLine   | `crates/tui/src/render.rs` (bottom-bar ring)|
| Transcript   | `crates/tui/src/render.rs` (transcript)     |
| Slash cmds   | `crates/tui/src/app.rs` (command router)    |
| Identity     | `crates/agent/src/prompt_builder.rs`        |

## What's cosmetic vs real

- All agent/tool behaviour is **faked**. Typing `hello` or `refactor` picks a canned reply; the tool-call shelf button fakes a `read_file` call with a 2.2 s delay.
- The scanner and streaming/tool states are **visually accurate** to the real TUI.
- Slash-command output is accurate to the Rust implementation (labels, indent, rule width).
- Colors come from the xterm/ANSI palette used by the real terminal, via `colors_and_type.css`.

## Try

1. Type `/help` — suggestions appear; press `Tab` to accept.
2. Type `/status` or `/doctor` — run full output.
3. Type any message — Hermes streams a reply; scanner animates.
4. Press `Ctrl+C` during streaming — turn cancels.
5. Use the shelf on the right to trigger a tool call or clear the transcript.
