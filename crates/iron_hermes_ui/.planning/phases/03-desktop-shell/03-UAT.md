---
status: complete
phase: 03-desktop-shell
source: [03-01-SUMMARY.md, 03-02-SUMMARY.md, 03-03-SUMMARY.md, 03-04-SUMMARY.md, 03-05-SUMMARY.md]
started: 2026-05-03T00:00:00Z
updated: 2026-05-03T00:00:00Z
---

## Current Test

[testing complete]

## Tests

### 1. Cold Start Smoke Test
expected: Run `dx serve --features web` from project root. Server boots without errors, Tailwind compiles, the printed local URL loads in a browser, the WarpHermes shell paints (title bar, block stream, agent panel, status bar, command palette overlay) without console errors and without missing-asset 404s.
result: pass

### 2. SHELL-01 — Title Bar Chrome
expected: Top of viewport shows: 3 macOS traffic lights (red `#ff5f57`, yellow `#febc2e`, green `#28c840`), an IronHermes brand block (small `IH` sigil + cyan-accent "IronHermes" label, right border separator), a tab strip with 3 tabs (`ironhermes chat` (active+live dot), `cargo watch` (live dot), `agent · scratch` (no dot)), a `+` new-tab button, and a right-aligned `⌘K` shortcut chip.
result: pass

### 3. SHELL-02 — Block Stream Stripe Types
expected: The main stream shows 10 blocks each with a 2px left accent stripe color-coded by kind: `is-cmd` (user command, accent-primary cyan), `is-out` (output, dim), `is-ai` (Hermes reply, magenta), `is-ok` (success, green), `is-err` (error, red), `is-tool` (tool call, yellow). All 6 stripe types are visible somewhere in the 10-block fixture stream.
result: pass

### 4. SHELL-03 — Hover Affordances
expected: Hovering any block reveals a row of action icon buttons. Cmd blocks show 2 buttons (copy `⎘`, rerun `↻`); non-Cmd blocks show 3 buttons (copy `⎘`, rerun `↻`, share `↗`). Buttons fade in on hover via CSS `:hover` (no JS state); the action row is hidden when the cursor leaves the block.
result: pass

### 5. SHELL-04 — CommandLine Token Coloring
expected: In Cmd blocks (e.g. `❯ ironhermes doctor` and `❯ git diff --stat`), the prompt glyph `❯`, the binary token (`ironhermes`, `git`), the arg tokens (`doctor`, `diff`), and the flag tokens (`--stat`) each render in distinct prototype colors per `.wh-cmd-bin` / `.wh-cmd-arg` / `.wh-cmd-flag` classes. The `bin` token is rendered with stronger weight (700).
result: pass

### 6. SHELL-05 — Tool-Call Block Status States
expected: The fixture stream contains 4 `is-tool` blocks rendering each `ToolStatus` state: `read_file` shows `pending…` in yellow, `edit_file` shows `running…` in yellow, `search` shows `[OK]` in green, `compile` shows `failed` in red. Each tool-call card has a yellow border and shows `Tool:` dim label + bold tool name.
result: pass

### 7. SHELL-06 — Input Box Mode Glyph + Focus Ring
expected: Below the block stream sits the input box with a `Shell` mode pill, the `❯` prompt glyph, and the placeholder text `Type a command, or '/' for commands`. Because `focused: true` is hardcoded for UAT, the wrapper carries an accent-primary focus ring with a soft glow. Right-side action buttons `@`, `●`, `↵` are visible.
result: pass

### 8. SHELL-07 — Agent Side Panel
expected: A right-side panel ~360px wide shows: `IH` sigil (size 20) + `HERMES` title + a `/default` personality pill, followed by a scrollable message list. Messages alternate `is-user` / `is-hermes` styling; tool-call messages render an inline `ToolCall` card. The panel is visible because the `.wh-app` wrapper carries `data-agent="right"`.
result: pass

### 9. SHELL-08 — Status Bar Pills + Scanner
expected: At the bottom of the main column: 4 colored `.wh-pill` chips separated by `·` middots — `Chat`, `claude-sonnet-4`, `anthropic`, `12.3K/128K (10%)` — followed by the 10-cell scanner, then a right-aligned hint reading `/help · ⌃C cancel · ⌘K palette`.
result: pass

### 10. SHELL-09 — Scanner Knight-Rider Animation
expected: The status-bar scanner shows 10 cells (each starting as `░`) animating continuously: a bright wave bounces back and forth, with each cell briefly progressing through `░ → ▒ → ▓ → █ → ▓ → ▒ → ░` color levels via the CSS `@keyframes wh-scanner-tick` rule. Animation is purely CSS (no setInterval); period is ~1800ms across all 10 cells with staggered `:nth-child` delays.
result: pass

### 11. SHELL-10 — Command Palette Overlay
expected: A modal overlay covers the viewport showing the command palette: a `⌘K` accented search row at top with empty value and an `esc` kbd chip on the right; below, two sections — `Slash commands` (6 rows: `/help`, `/clear`, `/init`, `/k`, `/quit`, etc., with kbd shortcut chips like `⌘ I`, `⌘ K`) and `Workflows` (4 rows). The first slash row carries an `is-active` highlight by default.
result: pass

## Summary

total: 11
passed: 11
issues: 0
pending: 0
skipped: 0
blocked: 0

## Gaps

[none yet]
