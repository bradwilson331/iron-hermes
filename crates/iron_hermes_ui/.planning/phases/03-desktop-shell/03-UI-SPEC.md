---
phase: 3
slug: desktop-shell
status: draft
shadcn_initialized: false
preset: none
created: 2026-05-03
---

# Phase 3 — UI Design Contract

> Pixel-perfect port of the Warp × IronHermes desktop/web shell. The prototype is the canonical contract; this document translates it into checkable rules for the planner, executor, ui-checker, and ui-auditor. Source-of-truth: `warp2ironhermes/project/app/{shell,app}.jsx`, `warp-ih.css`, `colors_and_type.css` (already ported to `assets/warp-ih.css` + `assets/design-tokens.css`).

---

## Design System

| Property | Value |
|----------|-------|
| Tool | none (custom IronHermes design system; no shadcn — Rust/Dioxus stack, no Tailwind utilities consumed) |
| Preset | not applicable |
| Component library | none — primitives ported 1:1 from `shell.jsx` to `src/components/shell/*.rs` per CONTEXT D-01 |
| Icon library | none — unicode glyphs only (`❯` `✦` `⎘` `↻` `↗` `▤` `░` `▒` `▓` `█` `·` `▸` `/` `+` `×` `⌘K` `↵` `⌥+M`) |
| Font | `Ioskeley Mono` (16 woff2 weights wired in Phase 2; fallback chain `"Berkeley Mono", ui-monospace, "SF Mono", "Menlo", "Consolas", "Liberation Mono", monospace`) |
| Type variant in use | weight 400 body, weight 600 (`--weight-bold` is 700) for emphasis, weight 700 for `wh-author`/prompt-glyph/active labels |

**Design system source files (already on disk, do not modify):**
- `assets/design-tokens.css` — verbatim port of `colors_and_type.css` (ANSI palette, type scale, pill rotation tokens, scanner constants, base body/h*/p/code/pre/hr rules)
- `assets/warp-ih.css` — verbatim port of `warp-ih.css` (Warp surface scale `--w-bg-0..4`, block stripes, density/theme/block data-attribute overrides, all `.wh-*` layout classes consumed by Phase 3)
- `assets/main.css` — Phase 2 brand-stub base (delete brand-stub-only rules during Phase 3 if they conflict with `wh-app`; otherwise leave as-is)
- `assets/fonts/IoskeleyMono-*.woff2` — 16 weights resolved via relative URLs from `design-tokens.css`

**Phase 3 components must consume class names verbatim. Do not invent new classes.**

---

## Spacing Scale

The IronHermes prototype declares its own spacing scale in `design-tokens.css` and a Warp-specific density scale in `warp-ih.css`. **Phase 3 reuses these tokens — Phase 3 components do not introduce new spacing values.**

### IronHermes base scale (`design-tokens.css`)

| Token | Value | Usage in Phase 3 |
|-------|-------|------------------|
| `--space-0` | 0 | Reset margins |
| `--space-1` | 4px | Half-cell gap (icon button gap, scanner cell gap, `wh-pal-list` padding) |
| `--space-2` | 8px | One monospace cell — block-actions vertical offset, `wh-input-row` gap, `wh-stream-scroll` block gap base |
| `--space-3` | 16px | Two cells — default panel padding (`wh-stream-scroll`: `16px 16px 8px`) |
| `--space-4` | 24px | Three cells — section gap (not consumed in Phase 3 layout; reserved for future) |
| `--space-5` | 32px | Reserved |
| `--space-6` | 48px | Reserved |

### Warp shell-specific tokens (`warp-ih.css` `:root`)

| Token | Value (comfy) | Value (compact) | Usage in Phase 3 |
|-------|---------------|-----------------|------------------|
| `--w-row-y` | 8px | 4px | Vertical row padding inside blocks |
| `--w-block-pad` | 12px 14px | 8px 12px | `wh-block-head`, `wh-block-body`, `wh-cmdline` interior padding |
| `--w-pal-row` | 38px | 30px | Command palette row height |
| `--w-radius-block` | 6px | 6px (0px under `[data-block=flat\|minimal]`) | Block + input-wrap + toolcall corner radius |
| `--w-radius-pill` | 999px | 999px | Mode pill, personality pill |
| `--w-radius-key` | 8px | 8px | Reserved (keyboard key chrome) |

### Hardcoded layout dimensions (from `warp-ih.css`)

| Element | Dimension | Source |
|---------|-----------|--------|
| Title bar height | 32px | `.wh-titlebar { height: 32px; padding: 0 12px; }` |
| Title bar tab gap | 2px | `.wh-tabs { gap: 2px; }` |
| Title bar tab padding | 0 14px | `.wh-tab { padding: 0 14px; }` |
| Tab dot size | 6px circle | `.wh-tab-dot { width: 6px; height: 6px; }` |
| Tab active stripe | 2px top | `.wh-tab.is-active::before { height: 2px; }` |
| Sigil "IH" stamp | 26×26px (default), 18×18 in titlebar, 20×20 in side-panel head | `.wh-sigil` + inline overrides in `shell.jsx` |
| Status bar height | 24px | `.wh-status { height: 24px; padding: 0 12px; }` |
| Status pill separator pad | 0 6px | `.wh-status .wh-sep { padding: 0 6px; }` |
| Scanner cell gap | 1px | `.wh-scanner { gap: 1px; padding: 0 4px; }` |
| Side panel width | 360px | `.wh-side { width: 360px; }` |
| Side panel head height | 36px | `.wh-side-head { height: 36px; padding: 0 12px; }` |
| Side panel scroll padding | 12px | `.wh-side-scroll { padding: 12px; gap: 10px; }` |
| Stream scroll padding | 16px 16px 8px | `.wh-stream-scroll { padding: 16px 16px 8px; gap: 10px; }` |
| Input wrap margin | 8px 16px 14px | `.wh-input-wrap { margin: 8px 16px 14px; }` |
| Input mode row pad | 8px 12px 0 | `.wh-input-mode` |
| Input main row pad | 8px 12px 12px | `.wh-input-row` |
| Mac traffic light dot | 12×12 circle, gap 8px | inline style in `TitleBar` (red `#ff5f57`, amber `#febc2e`, green `#28c840`) |
| Block-actions buttons | 24×24px | `.wh-icon-btn { width: 24px; height: 24px; }` |
| Palette overlay top pad | 60px | `.wh-pal-overlay { padding-top: 60px; }` |
| Palette card width | min(620px, 92%), max-height 60vh | `.wh-pal { width: min(620px, 92%); max-height: 60vh; }` |
| Palette search padding | 12px 14px | `.wh-pal-search { padding: 12px 14px; }` |
| Palette row gap / padding | gap 12px / pad 0 10px | `.wh-pal-row` |
| Mobile tab bar height | 44px | `.wh-mobile-tabs` (Phase 6 — defined but unused in Phase 3) |

**Exceptions:** none. Every dimension in Phase 3 must come from the table above OR from `assets/warp-ih.css` directly. If the prototype's `shell.jsx` uses an inline `style={{ ... }}` value (e.g., the traffic-light dots, the titlebar `IronHermes` text block padding, the tabs `+` button padding `0 10px`), port that inline value verbatim into Dioxus rsx as `style: "..."`. **No rounding, no "approximately", no harmonization.**

---

## Typography

The prototype's type scale is in `design-tokens.css` and is **already loaded** as Phase 2 wired the link cascade. Phase 3 components consume CSS variables — do not hardcode pixel sizes in rsx.

| Role | Token | Value | Weight | Line Height | Usage in Phase 3 |
|------|-------|-------|--------|-------------|------------------|
| Body (default) | `--type-body` | 0.875rem (14px) | 400 (`--weight-regular`) | 1.5 (`--leading-normal`) | `.wh-app`, `.wh-block-body`, `.wh-textarea`, agent message body |
| Small / hint | `--type-small` | 0.75rem (12px) | 400 | 1.5 | `.wh-titlebar`, `.wh-block-head`, tab labels, status bar |
| Status / scanner | hardcoded 11px | 11px | 400 | 1 (status), 1.5 (scanner span inherits) | `.wh-status`, `.wh-cmdline .wh-cmd-time`, `.wh-input-mode`, `.wh-block-actions` glyph buttons, `.wh-pal-row .wh-pal-hint` |
| Mode pill | hardcoded 10px | 10px | 600 | 1 | `.wh-input-mode .wh-mode-pill` (uppercase, `letter-spacing: .04em`) |
| Section header (palette) | hardcoded 10px | 10px | 400 (`text-transform: uppercase`, `letter-spacing: .08em`) | 1 | `.wh-pal-section` |
| Sub-headline | `--type-h2` | 1.125rem (18px) | 700 | 1.3 (`--leading-snug`) | Reserved — not used in Phase 3 shell layout |
| Section banner | `--type-h1` | 1.5rem (24px) | 700 | 1.2 (`--leading-tight`) | Reserved — not used in Phase 3 shell layout |
| Palette search input | hardcoded 14px | 14px | 400 | 1.5 | `.wh-pal-search input` |
| Palette row body | hardcoded 13px | 13px | 400 | 1.5 | `.wh-pal-row` |
| Side panel msg body | hardcoded 13px | 13px | 400 | 1.5 | `.wh-msg-body` |
| Side panel msg meta | hardcoded 10px | 10px | 700 (`b`) / 400 (span) | 1 | `.wh-msg-meta` |
| Sigil "IH" stamp | size×0.46 (≈12px at 26 default) | derived | 800 | 1 | `.wh-sigil` (`font-size: size * 0.46` per `shell.jsx` line 55) |

**Hard rule:** every font-size value in Phase 3 components either (a) inherits from the ported CSS class — preferred — or (b) matches an inline `style.fontSize` value already present in `shell.jsx`. The executor must not introduce a new `font-size` value without first finding it in the prototype source.

**Italic and condensed weights are loaded but unused in Phase 3** — the 16 woff2 declarations in `design-tokens.css` cover future condensed-status-bar and italic-emphasis use; Phase 3 keeps the declarations untouched and references only weights 400 / 600 / 700 / 800.

---

## Color

IronHermes is a 16-color ANSI palette terminal aesthetic. The 60/30/10 framework is unusual here because the design **inverts dashboard color norms** — almost everything is dim foreground on dark surface; saturated color is reserved for ANSI-significant signals (error, success, warn, agent identity, tool identity). The breakdown below maps the IronHermes design intent onto the 60/30/10 framework.

### 60 / 30 / 10 split

| Role | Token | Hex | Usage |
|------|-------|-----|-------|
| **Dominant (60%)** — terminal canvas | `--w-bg-1` (= `--bg` = `--ansi-black`) | `#0d1117` | Stream scroll area, side panel base, `.wh-app` body |
| **Secondary (30%)** — chrome surfaces | `--w-bg-0` (titlebar/status), `--w-bg-2` (block + input-wrap + palette card + toolcall), `--w-bg-3` (hover, palette row active, sigil bg, code chip) | `#0a0e13` / `#161b22` / `#1c232c` | Title bar, status bar, all block bodies, input-wrap, palette card, hover targets |
| **Accent (10%)** — primary cyan | `--accent-primary` (= `--ansi-cyan`) | `#4ec9b0` | Reserved for: `is-cmd` left stripe • mode glyph (`❯`/`✦`) • `wh-prompt-glyph` color • input focus border + 3px glow ring (`color-mix 15%`) • caret-color • side-panel `HERMES` title • `is-active` tab top stripe • `⌘K` text in titlebar + palette search • palette glyph (`/` and `▸`) • run-action button (`↵`) • code chip text (`--accent-primary-hi`) • status pill[0] (mode) • scanner `lit` and `t1` cells (`--accent-primary-hi` and `--accent-primary` respectively) • mode pill background (`color-mix 14%`) |
| **Accent-hi** — bright cyan | `--accent-primary-hi` (= `--ansi-bright-cyan`) | `#6fd8c2` | Scanner `lit` cell (`█`), code chip text, mode pill text (shell mode) |

### Semantic colors (purposeful, not "10%-rationed")

| Role | Token | Hex | Reserved-for list (exclusive) |
|------|-------|-----|--------------------------------|
| **Success / OK / user identity** | `--success` (= `--ansi-green` `#3fb950`) | `#3fb950` | `is-ok` left stripe • `is-cmd` `wh-author` color • side panel `is-user` `b` color • tab `wh-tab-dot` (live tab) • `[OK]` text in `Block` head when `kind=ok` • status pill[2] (provider) • `wh-cmd-str` (string literal token in command line) |
| **Destructive / error** | `--danger` (= `--ansi-red` `#f85149`) | `#f85149` | `is-err` left stripe • `is-err` `wh-author` color • `exit N` text in `Block` head when `kind=err` • diff-line `is-del` color (output detail rows) |
| **Warn / tool identity** | `--warn` (= `--ansi-yellow` `#d29922`) | `#d29922` | `is-tool` left stripe • `is-tool` `wh-author` color • `.wh-toolcall` `b` (tool name) • `.wh-toolcall` left border (2px) • `running…` status text • status pill[3] (tokens used) |
| **Agent secondary** | `--accent-secondary` (= `--ansi-magenta` `#c678dd`) | `#c678dd` | `is-ai` left stripe • `is-ai` `wh-author` color • `wh-cmd-flag` (CLI `--flag` tokens in CommandLine) • status pill[1] (model) • mode pill background when `is-agent` (`color-mix 14%` on this token) |
| **Agent secondary bright** | `--ansi-bright-magenta` | `#d7a0ea` | Mode pill text when `is-agent` (`.wh-input-mode .wh-mode-pill.is-agent { color: var(--ansi-bright-magenta); }`) |
| **Brand orange** | `--brand` | `#f0883e` | Reserved for IronHermes wordmark fill (rendered inside `wordmark.svg`) and section titles. **Phase 3 does not introduce additional brand-orange usage** — the wordmark SVG carries its own fill; do not retint it from Rust. |
| **Brand orange hi** | `--brand-hi` | `#ffa657` | Reserved for wordmark hover (Phase 3: not interactive, so unused) |

### Foreground tints

| Token | Hex | Usage |
|-------|-----|-------|
| `--fg` (= `--ansi-white`) | `#c9d1d9` | Default body text, `wh-block-body`, `wh-cmd-arg`, `wh-pal-row` |
| `--fg-strong` (= `--ansi-bright-white`) | `#f0f6fc` | Active tab label, `wh-cmdline .wh-cmd` (bin token), `.wh-textarea` text, `wh-pal-search input`, `wh-pal-row` `wh-pal-glyph`-adjacent strong text |
| `--fg-dim` | `#6e7681` | All hint text — title bar default, status bar separators, `wh-block-head` meta, `wh-out` text, `wh-msg-meta` span, `wh-personality` pill, palette label dim, `wh-cmd-time` |
| `--fg-disabled` | `#484f58` | Disabled/inactive (the tab `×` close glyph, `wh-out-line .wh-ln` line numbers) |

### Status pill rotation (canonical, from `pills.rs` per `design-tokens.css` lines 80-85)

| Pill index | Token | Hex | Status-bar position (per `app.jsx` `<StatusBar>`) |
|------------|-------|-----|---------------------------------------------------|
| 0 | `--pill-0` | `--accent-primary` `#4ec9b0` | Mode (`Chat` / `Agent`) |
| 1 | `--pill-1` | `--accent-secondary` `#c678dd` | Model (`claude-sonnet-4`) |
| 2 | `--pill-2` | `--success` `#3fb950` | Provider (`anthropic`) |
| 3 | `--pill-3` | `--warn` `#d29922` | Token count (`12.3K/128K (10%)`) |
| 4 | `--pill-4` | `--fg-dim` `#6e7681` | Reserved (hint position) |

**Phase 3 hardcodes the full status-bar value set verbatim from `app.jsx` per CONTEXT D-20:**
- Mode: `"Chat"` (default `mode === "shell"` per `shell.jsx` line 215)
- Model: `"claude-sonnet-4"`
- Provider: `"anthropic"`
- Tokens: `{ used: 12300, max: 128000 }` rendered as `"12.3K/128K (10%)"`
- Hint: `"/help · ⌃C cancel · ⌘K palette"` (shell mode default)
- Scanner: rendered with all 10 cells visible (see Scanner Animation Contract below)

### Block-stripe color contract (SHELL-02 + the unmapped Tool variant per CONTEXT D-10)

| Variant (Rust enum) | CSS class | Stripe token | Stripe hex |
|---------------------|-----------|--------------|------------|
| `Block::Cmd` | `wh-block is-cmd` | `--w-stripe-cmd` (= `--accent-primary`) | `#4ec9b0` (cyan) |
| `Block::Out` | `wh-block` (no `is-*` modifier — uses default `--w-stripe-out`) | `--w-stripe-out` (= `--fg-dim`) | `#6e7681` (dim) |
| `Block::Ai` | `wh-block is-ai` | `--w-stripe-ai` (= `--accent-secondary`) | `#c678dd` (magenta) |
| `Block::Ok` | `wh-block is-ok` | `--w-stripe-ok` (= `--success`) | `#3fb950` (green) |
| `Block::Err` | `wh-block is-err` | `--w-stripe-err` (= `--danger`) | `#f85149` (red) |
| `Block::Tool` | `wh-block is-tool` | `--w-stripe-tool` (= `--warn`) | `#d29922` (yellow) |

**Resolution of CONTEXT "Claude's Discretion: is-tool stripe color":** the ported `warp-ih.css` already defines `.wh-block.is-tool::before { background: var(--w-stripe-tool); }` (line 186) and `--w-stripe-tool: var(--warn)` (line 30). The `Tool` variant maps to the `is-tool` class — the planner does not need to choose a stripe color; the CSS already chose yellow. The Rust `BlockKind` discriminant for `Block::Tool` emits class string `"is-tool"`.

### Out-block stripe (default sixth variant)

When `Block::Out` is rendered, the class is `wh-block` with **no `is-*` modifier**. The `.wh-block::before` default rule at line 177-182 paints the stripe `--w-stripe-out` (= `--fg-dim`). This matches `app.jsx` line 290 `<div className={"wh-block is-" + b.kind}>` which produces `is-out` — but `warp-ih.css` has no `is-out::before` override, so the stripe falls back to the default `--w-stripe-out`. **Phase 3 emits `is-out` on the wrapper for parity with the prototype** (`format!("wh-block is-{}", kind_class)`); the CSS handles the rest.

---

## Copywriting Contract

Phase 3 is a static port — copywriting is **verbatim from the prototype source**, not authored. Listed below for ui-checker validation against `app.jsx` / `shell.jsx`.

### Title bar

| Element | Copy |
|---------|------|
| Brand label (right of sigil) | `IronHermes` |
| Tab 0 label | `ironhermes chat` (`live: true` — green dot) |
| Tab 1 label | `cargo watch` (`live: true` — green dot) |
| Tab 2 label | `agent · scratch` (`live: false` — dim dot) |
| New-tab affordance | `+` (dim, no click handler in Phase 3) |
| Tab-close glyph | `×` (per tab, `--fg-disabled`, no click handler in Phase 3) |
| Palette shortcut display | `⌘K` |
| Active tab indicator | first tab is `is-active` (per `app.jsx` `useState(0)`) |

### Status bar (defaults from `app.jsx`)

| Pill position | Copy / value |
|---------------|--------------|
| 0 — mode | `Chat` |
| 1 — model | `claude-sonnet-4` |
| 2 — provider | `anthropic` |
| 3 — tokens | `12.3K/128K (10%)` (rendered from `{ used: 12300, max: 128000 }`) |
| Hint (right) | `/help · ⌃C cancel · ⌘K palette` |
| Separator | `·` |

### Input box (shell mode default)

| Element | Copy |
|---------|------|
| Mode pill | `Shell` (uppercase via CSS `text-transform`) |
| Mode hint left | `⌥+M to switch` |
| Mode hint right | `↵ run · ⇧↵ newline · ⌃C cancel` |
| Prompt glyph | `❯` (shell mode) |
| Textarea placeholder | `Type a command, or \`/\` for commands` |
| Action buttons (right of textarea) | `@` (attach) · `●` (voice) · `↵` (run, accent-primary color) |

**Agent-mode variant** (referenced in Phase 4 wiring, but the prototype defaults to shell — Phase 3 hardcodes shell mode):
- Mode pill: `Agent` with `is-agent` class
- Prompt glyph: `✦`
- Placeholder: `Ask IronHermes anything…`

### Side panel (default seed messages from `seedMessages()` in `app.jsx`)

| Element | Copy |
|---------|------|
| Sigil | `IH` (rendered via `.wh-sigil` 26×26 stamp at 20px in side-panel head) |
| Title | `HERMES` (uppercase, accent-primary) |
| Personality pill | `/default` |
| Message 1 (user, 00:14:42) | `Pull request feedback on the personality refactor — did I miss anything?` |
| Message 2 (hermes, 00:14:43) | tool call: `read_file` args `{"path":"crates/ironhermes-agent/src/personality.rs"}` status `done` |
| Message 3 (hermes, 00:14:46) | `I'll read the file first…\n\nThe new preset registry is clean. One nit: \`personality.rs:84\` builds the system-prompt prefix with \`format!\`, but the old code interned it via \`PROMPT_CACHE\`. Worth restoring to avoid an alloc per turn.` |
| Message 4 (user, 00:14:50) | `Good catch. Patch it.` |
| Message 5 (hermes, 00:14:51) | tool call: `edit_file` args `{"path":"...","find":"format!","replace":"PROMPT_CACHE.intern(format!"}` status `running` |

### Block stream seed (`seedBlocks()` from `app.jsx`, planner extends per CONTEXT D-18)

| Block | kind | Author | Time | Body |
|-------|------|--------|------|------|
| b1 | `cmd` | — | `0.4s` | `CommandLine[Bin("ironhermes"), Arg("doctor")]` |
| b2 | `out` | `doctor` | `00:14:02` | `IronHermes Doctor` heading + 40-char rule + 8 doctor lines (`Rust toolchain 1.81.0 stable [OK]` … `OpenAI key not set [MISSING]`) |
| b3 | `cmd` | — | `0.1s` | `CommandLine[Bin("git"), Arg("diff"), Flag("--stat")]` |
| b4 | `ok` | `git` | `00:14:31` | `<pre>` 4-line diff stat |
| b5 | `ai` | `Hermes` | `00:14:48` | `The diff looks clean — the new `concise` personality slot is wired through `personality.rs` and the status line picks it up via the existing pill rotation. Want me to add a test that snapshots the rendered status line for each preset?` |

**Phase 3 extends `seedBlocks()` with one `Err` block and four `Tool` blocks (one per `ToolStatus` variant) per CONTEXT D-18, reaching ≈8-10 blocks total. Suggested additions:**

| Block | kind | Author | Time | Body |
|-------|------|--------|------|------|
| b6 | `err` | `cargo` | `00:14:55` | `error[E0282]: type annotations needed\n  --> src/main.rs:42:9` (exit 1) |
| b7 | `tool` | — | — | `ToolCall { name: "read_file", args_summary: "{\"path\":\"src/lib.rs\"}", status: Pending }` |
| b8 | `tool` | — | — | `ToolCall { name: "edit_file", args_summary: "{\"path\":\"src/main.rs\",\"line\":42}", status: Running }` |
| b9 | `tool` | — | — | `ToolCall { name: "search", args_summary: "{\"q\":\"PROMPT_CACHE\"}", status: Done }` |
| b10 | `tool` | — | — | `ToolCall { name: "compile", args_summary: "{\"target\":\"wasm32\"}", status: Failed }` |

The four `Tool` blocks demonstrate all four `ToolStatus` variants in one screen, satisfying SHELL-05 visually. Args summaries are short JSON-ish strings — exact wording is the planner's call as long as each variant renders distinctly.

### Command palette (open by default per CONTEXT D-19)

| Element | Copy |
|---------|------|
| `⌘K` left badge | `⌘K` (accent-primary, weight 700) |
| Search placeholder | `Search commands, files, recent…` |
| `esc` keyboard hint right | `esc` (in `.wh-kbd` chip styling) |
| Section header 1 | `Slash commands` (uppercase, dim) |
| Section header 2 | `Workflows` (uppercase, dim) |

Slash command rows (from `PALETTE_ITEMS` in `app.jsx`, render in this order):

| Glyph | Cmd | Label | Keyboard hint chip(s) |
|-------|-----|-------|------------------------|
| `/` | `/help` | `Show available commands` | `?` |
| `/` | `/status` | `IronHermes status` | `⌘` `I` |
| `/` | `/doctor` | `Run doctor checks` | (none) |
| `/` | `/personality` | `Change personality preset` | (none) |
| `/` | `/clear` | `Clear scrollback` | `⌘` `K` |
| `/` | `/quit` | `Exit chat` | `⌘` `Q` |

Workflow rows:

| Glyph | Label | Cmd (dim, right) |
|-------|-------|-------------------|
| `▸` | `Git: working tree status` | `git status` |
| `▸` | `Cargo: build workspace` | `cargo build` |
| `▸` | `Start chat session` | `ironhermes chat` |
| `▸` | `Run config doctor` | `ironhermes doctor` |

**First row (`/help`) renders with `is-active` class** so the `--w-bg-3` highlight is visible during UAT (matches prototype's `useState(0)` initial active index).

### CTA / Empty state / Error state / Destructive

| Element | Copy | Note |
|---------|------|------|
| Primary CTA | Run-arrow button `↵` in `.wh-input-actions` (accent-primary) | Static in Phase 3 — no onclick |
| Empty state heading | not applicable | `seedBlocks()` always returns ≥5 blocks; the stream is never empty in Phase 3 |
| Empty state body | not applicable | — |
| Error state | `is-err` block with `exit N` chip + `--danger` left stripe + danger-tinted author | Pre-existing visual contract; no error UI is invoked in Phase 3 |
| Destructive confirmation | not applicable | Phase 3 has no destructive actions (no clear, no delete, no quit). `/clear` and `/quit` palette rows render but onclick is Phase 4. |

---

## Visuals & Interaction Contracts (Phase-3-specific extensions)

The 6-dimension UI checker template doesn't carry rows for animation, cursor, hover, focus, or per-component contracts. These are critical for IronHermes — added below.

### Scanner Animation Contract (SHELL-09)

The prototype's `Scanner` (in `shell.jsx`) drives the 10-cell knight-rider via React `setInterval` at 100ms. CONTEXT D-08 mandates **CSS `@keyframes` instead of signal-driven re-renders** for Phase 3, hardcoded to `is-active`.

| Property | Value |
|----------|-------|
| Cells | 10 `<span>` elements inside `.wh-scanner` |
| Cell glyphs | `░` (off), `▒` (t2 — half-dim), `▓` (t1 — bright cyan), `█` (lit — bright-cyan-hi) |
| Triangle wave period | 1800ms (18 ticks @ 100ms each) — declared as `--scanner-period: 1800ms` in `design-tokens.css` |
| Frame rate | 10 fps (`--scanner-fps: 10`) |
| Cells per pass | 10 (`--scanner-width: 10`) |
| Lit cell color | `--accent-primary-hi` (`#6fd8c2`) — cells with class `lit` |
| t1 cell color | `--accent-primary` (`#4ec9b0`) — cells with class `t1` |
| t2 cell color | `color-mix(in oklab, var(--accent-primary) 50%, var(--fg-dim))` — cells with class `t2` |
| Default cell color | `--fg-dim` (`#6e7681`) |
| Active state | `is-active` class on `.wh-scanner` triggers the animation; in Phase 3 this is hardcoded `true` |
| Phase 4 override | inactive by default; pulses `is-active` for 1400ms post-submission (deferred) |

**Implementation note for the planner — gap to resolve:**

The ported `assets/warp-ih.css` (lines 326-330) defines the **static color rules** for `lit`, `t1`, `t2` cell classes but contains **no `@keyframes` rule** to advance which cell carries which class. The prototype's React `Scanner` component (`shell.jsx` lines 5-27) computes the lit-cell index per tick from JavaScript state.

Planner picks one of:
1. **(Recommended)** Add `@keyframes` rules to `assets/warp-ih.css` (or a new `assets/warp-ih-anim.css` link) that animate the 10 cells through the triangle wave — pure CSS, GPU-compositor friendly, satisfies CONTEXT D-08 verbatim. Each cell gets a staggered animation-delay; the animation cycles `█ → ▓ → ▒ → ░ → ░ → ▒ → ▓ → █` over the 1800ms period at fixed positions OR uses `animation-delay: calc(var(--cell-index) * -100ms)` with a shared keyframe that progresses through the four states. The `:nth-child` approach is also viable.
2. **Acceptable fallback** — render 10 cells with **fixed** classes that show one frame of the animation (e.g., cell 5 = `lit`, cells 4/6 = `t1`, cells 3/7 = `t2`, rest = default). Static screenshot of the knight-rider effect. Fails the "knight-rider animation" sub-bullet of SHELL-09 visually but proves the layout/colors. Phase 4 then adds the animation as part of the scanner-pulse work. Reject this fallback — SHELL-09 explicitly requires the animation, and CONTEXT D-08 explicitly says the scanner must run on page load in Phase 3.

**Phase 3 ships option 1.** The planner authors a small `@keyframes wh-scanner-tick` block in a new file (e.g., `assets/scanner-anim.css`) or appends to `assets/warp-ih.css` (breaks verbatim-port but resolves the gap; planner judges). This is the only addition allowed beyond the verbatim port.

### Hover affordance contract (SHELL-03)

| Property | Value |
|----------|-------|
| Buttons | `⎘` (copy, `title="copy"`) · `↻` (rerun, `title="rerun"`) · `↗` (share, `title="share"`) — three buttons for `is-out`/`is-ai`/`is-ok`/`is-err`/`is-tool`; `is-cmd` shows only copy + rerun (per `app.jsx` `RenderBlock` line 285) |
| Default opacity | 0 |
| Hover opacity | 1 (transition `opacity .12s` per `warp-ih.css` line 226) |
| Trigger | pure CSS `.wh-block:hover .wh-block-actions` — no Rust state |
| Position | `position: absolute; top: 6px; right: 6px;` |
| Button size | 24×24px |
| Button bg hover | `--w-bg-3` |
| Button color default | `--fg-dim` |
| Button color hover | `--fg-strong` |
| Cursor | `default` (not pointer — the `wh-icon-btn` rule uses `cursor: default`) |
| Aria-label | use `title` attribute matching prototype (`copy` / `rerun` / `share`) |
| Click handler | **none in Phase 3** (Phase 4 wires KBD-05) |

### Focus ring contract (SHELL-06)

| Property | Value |
|----------|-------|
| Trigger | `is-focus` class on `.wh-input-wrap` (Phase 3 hardcoded **off** — focused state visible only via real focus, but `<textarea>` is focusable) |
| Border default | `1px solid var(--w-border-hi)` (`#30363d`) |
| Border focused | `1px solid var(--accent-primary)` (`#4ec9b0`) |
| Glow | `box-shadow: 0 0 0 3px color-mix(in oklab, var(--accent-primary) 15%, transparent)` |
| Transition | `border-color .12s, box-shadow .12s` |
| Caret color | `var(--accent-primary)` |

**Phase 3 note:** The textarea is a real `<textarea>`. When the user clicks into it during UAT, the browser will give it `:focus`, but the prototype's `is-focus` class wrapping is **JS-driven** (`onFocus`/`onBlur` props in `shell.jsx`). For Phase 3 visual UAT, the planner can either:
- (A) hardcode `is-focus` on the wrap so the focus ring always shows (proves SHELL-06 visually without interaction);
- (B) leave `is-focus` off and rely on the user to click into the textarea during UAT to see the ring.

**Recommended: option (A)** — hardcode `is-focus` for Phase 3 UAT only. Phase 4 wires the real focus signal. Document the Phase 3 deviation in the UAT script.

### Side panel scroll contract (SHELL-07)

| Property | Value |
|----------|-------|
| Width | 360px (fixed; unchanged across density) |
| Default position | right (`data-agent="right"` is the wrapper default) |
| Hidden trigger | `data-agent="hidden"` sets `display: none` (Phase 5 wires; Phase 3 never sets) |
| Bottom trigger | `data-agent="bottom"` flips main flex to column, panel becomes 240px tall (Phase 5 wires; Phase 3 never sets) |
| Scroll auto-bottom | `useEffect` sets `scrollTop = scrollHeight` — **JS behavior in prototype**. Phase 3 omits this (it's a load-time scroll on mount; prototype scrolls only when `messages.length` changes). Phase 4 reintroduces if needed. |
| Inline tool call card | Rendered via `<ToolCall>` per message. Hermes message at `00:14:43` and `00:14:51` exercise this on the default seed. |

### Auto-grow textarea (SHELL-06)

Per CONTEXT D-09: the textarea's `rows={1}` produces a 22px `min-height` (via `.wh-textarea { min-height: 22px; line-height: 1.5; font-size: var(--type-body); }`). The prototype's `shell.jsx` does **not** implement JS scrollHeight measurement — it relies on the textarea's intrinsic browser behavior. **Phase 3 ships rows=1 + min-height: 22px and accepts the browser's default text-area expansion behavior.** Phase 4 adds explicit auto-grow via `oninput` if the prototype's behavior is unsatisfactory in side-by-side review.

### Tab strip details (SHELL-01)

Per `shell.jsx` `TitleBar`:
- Three traffic-light dots (red `#ff5f57`, amber `#febc2e`, green `#28c840`), 12×12 circles, gap 8px, paddingRight 8px wrapper, only when `showTrafficLights={true}` (= `variant === "classic"`, the default).
- IronHermes brand block: `<Sigil size={18}>` + `IronHermes` text in `--accent-primary` `fontWeight: 700` `fontSize: 12` with `paddingRight: 12` and `borderRight: 1px solid var(--w-border)` and `height: 100%`. **Port these inline styles verbatim into Dioxus rsx using `style: "..."` attribute strings.**
- Tab dot 6×6 circle, `background: var(--success)` if `live: true` else `var(--fg-dim)`.
- Tab `×` close glyph: `color: var(--fg-disabled)`, `marginLeft: 4`, `fontSize: 11`.
- New-tab `+` button: `padding: 0 10px`, `color: var(--fg-dim)`, `fontSize: 14`, `fontWeight: 700`.
- Right-side `⌘K` text: `fontSize: 11` (inside `.wh-titlebar-actions`).

### Sigil "IH" stamp (`Sigil` primitive)

Per `shell.jsx` lines 53-59:
- `width: size` `height: size` `font-size: size * 0.46` (inline)
- Default size 26 (= 12px font-size); 18 in title bar (= 8.28px ≈ 8px font); 20 in side panel head (= 9.2px ≈ 9px font).
- Background `var(--w-bg-3)`, color `var(--accent-primary)`, border-radius 6px, font-weight 800, letter-spacing -.04em (per `.wh-sigil` rule in CSS).
- Text content: literal `IH`.

**CONTEXT "Sigil primitive handling" resolution:** the prototype defines `Sigil` as a 6-line component used in 3 places (title bar, side panel head, inline-sigil agent variant). Following CONTEXT D-01's 1:1 rule, planner creates `src/components/shell/sigil.rs` with a `size: u16` prop. **Do NOT inline into `input_box.rs` — it isn't used there; the input box uses `wh-prompt-glyph` (`❯` / `✦`), not `Sigil`.**

### `Ai` content rendering (per CONTEXT D-16)

| Property | Value |
|----------|-------|
| Container | `.wh-block-body` (already has `white-space: pre-wrap`, `word-break: break-word`) |
| Markdown | **None in Phase 3.** Inline backticks in seed text render as plain backticks visually — they do not become `<code>` chips. The CSS `.wh-block-body code { background: var(--w-bg-3); ... }` rule applies if the executor decides to wrap inline-backtick spans manually, which the planner explicitly rejects for Phase 3 (Phase 4 owns markdown). |
| Newline preservation | inherited from `.wh-block-body` `white-space: pre-wrap`. **No `<pre>` wrapper needed for `Ai` blocks.** |
| Line breaks | the seed `b5` AI text is one paragraph followed by a question — render as one `<div>` with whitespace preserved by CSS. |

### Out-block render mode

The prototype's `b2` (doctor) and `b4` (git diff) bodies use `render: () => <div>...</div>` and `render: () => <pre>...</pre>`. **For Phase 3, the executor renders these as plain `<pre>` content with the literal text** — colored doctor lines and the green/dim spans inside `b2` are styling fidelity concerns, not visible-character concerns. Two acceptable approaches:

- **Approach A (recommended):** the `Block::Out` body in Rust state takes a `text: String` — the doctor block becomes a multi-line plain-text dump. Loses the per-line colorization the prototype shows but keeps the layout; ui-checker accepts because the SHELL-02 stripe is correct and the body is dim.
- **Approach B (high fidelity):** the `Block::Out` variant has a richer body type (`Vec<DoctorLine>` or `Vec<OutLine>`) that the `block.rs` component matches on. Planner only ships this if approach A reads as "obviously low-fidelity" in side-by-side UAT.

**Default to A.** Escalate to B only if UAT review specifically calls out the doctor block's loss of per-line color.

### Cursor contract

| Element | Cursor |
|---------|--------|
| `.wh-tab` | `default` (per CSS) |
| `.wh-icon-btn` | `default` (per CSS) |
| `.wh-pal-row` | inherited (browser default — text cursor inside, default outside) |
| `.wh-personality` | `default` (set inline in `shell.jsx`) |
| `.wh-textarea` | `text` (browser default for textarea) |

**The shell deliberately uses `cursor: default` for buttons — this is a design choice mirroring TUI feel, not a bug.** Do not change.

---

## Component Inventory

Mirrors `shell.jsx` 1:1 per CONTEXT D-01, plus the `block_stream.rs` extension per D-02. Every component listed is a Phase 3 deliverable.

| File | Component | Props | State | Rendered classes |
|------|-----------|-------|-------|------------------|
| `src/components/warp_hermes.rs` | `WarpHermes` | none (top-level) | none (D-06) | `.wh-app` (with `data-theme=cyan`, `data-density=comfy`, `data-block=framed`, `data-agent=right` — all hardcoded per CONTEXT "Deferred Ideas") |
| `src/components/shell/title_bar.rs` | `TitleBar` | `tabs: Vec<Tab>`, `active_tab: usize`, `show_traffic_lights: bool` (default true) | none | `.wh-titlebar`, `.wh-tabs`, `.wh-tab[.is-active]`, `.wh-tab-dot`, `.wh-titlebar-actions` |
| `src/components/shell/sigil.rs` | `Sigil` | `size: u16` (default 26) | none | `.wh-sigil` |
| `src/components/shell/block_stream.rs` | `BlockStream` | `blocks: Vec<Block>` | none (D-14) | `.wh-stream`, `.wh-stream-scroll` |
| `src/components/shell/block.rs` | `Block` | `data: Block` (state enum) | none | `.wh-block.is-{kind}`, `.wh-block-head`, `.wh-block-body`, `.wh-block-actions`, `.wh-icon-btn`, `.wh-author` |
| `src/components/shell/command_line.rs` | `CommandLine` | `tokens: Vec<Token>`, `time: Option<String>`, `cwd: Option<String>`, `glyph: Option<String>` | none | `.wh-cmdline`, `.wh-prompt-glyph`, `.wh-cmd[.wh-cmd-bin\|arg\|flag\|str]`, `.wh-cmd-time` |
| `src/components/shell/tool_call.rs` | `ToolCall` | `name: String`, `args_summary: String`, `status: ToolStatus` | none | `.wh-toolcall` |
| `src/components/shell/input_box.rs` | `InputBox` | `mode: Mode`, `focused: bool` (default true for Phase 3 UAT — see "Focus ring contract" recommended option A) | none | `.wh-input-wrap[.is-focus]`, `.wh-input-mode`, `.wh-mode-pill[.is-agent]`, `.wh-input-row`, `.wh-prompt-glyph`, `.wh-textarea`, `.wh-input-actions` |
| `src/components/shell/agent_panel.rs` | `AgentPanel` | `messages: Vec<Message>`, `personality: String` | none | `.wh-side`, `.wh-side-head`, `.wh-side-title`, `.wh-personality`, `.wh-side-scroll`, `.wh-msg.is-{user\|hermes}`, `.wh-msg-meta`, `.wh-msg-body` |
| `src/components/shell/status_bar.rs` | `StatusBar` | `mode: String`, `model: String`, `provider: String`, `tokens: TokenBudget`, `scanner_active: bool` (Phase 3 hardcoded true), `hint: String` | none | `.wh-status`, `.wh-pill`, `.wh-sep`, `.wh-hint` |
| `src/components/shell/scanner.rs` | `Scanner` | `active: bool` (Phase 3 hardcoded true) | none | `.wh-scanner[.is-active]`, span children with `.lit\|.t1\|.t2\|""` |
| `src/components/shell/command_palette.rs` | `CommandPalette` | `items: Vec<PaletteItem>`, `query: String` (Phase 3 hardcoded `""`), `open: bool` (Phase 3 hardcoded true per D-19) | none | `.wh-pal-overlay`, `.wh-pal`, `.wh-pal-search`, `.wh-pal-list`, `.wh-pal-section`, `.wh-pal-row[.is-active]`, `.wh-pal-glyph`, `.wh-pal-hint`, `.wh-pal-kbd`, `.wh-kbd` |
| `src/state.rs` | `Block`, `BlockKind`, `Mode`, `CommandLine`, `Token`, `ToolCall`, `ToolStatus`, `PaletteItem`, `Tab`, `Message`, `TokenBudget`, `demo_blocks()`, `demo_messages()`, `demo_palette_items()`, `demo_tabs()` | — | — (data-only module) |
| `src/components/mod.rs` | re-exports | — | — | declares `pub mod warp_hermes; pub mod shell;` |
| `src/components/shell/mod.rs` | re-exports | — | — | re-exports each primitive at module root |

**Component count:** 12 component files + state module + 2 mod files = **15 source files touched in Phase 3** (creates + 1 delete: `src/components/hero.rs`).

---

## Asset Migration Map

Per CONTEXT D-05 (asset constants colocate with consuming primitive):

| Asset | From (Phase 2) | To (Phase 3) | Consumed by |
|-------|----------------|--------------|-------------|
| `assets/wordmark.svg` (`WORDMARK_SVG`) | `src/components/hero.rs` | `src/components/shell/title_bar.rs` (declared at module top) | Optional — the prototype's title bar uses the literal text `IronHermes`, not the wordmark SVG. **Likely outcome: the wordmark const moves to title_bar.rs but is unused in Phase 3 rsx, OR moves to a Phase 5 TweaksPanel mount, OR is kept declared in `app.rs` for now.** Planner reads `app.jsx` and `shell.jsx` and decides; if unused, mark with a `#[allow(dead_code)]` or move ownership to `state.rs` for centralized brand-asset declarations. |
| `assets/ih-shield.png` (`IH_SHIELD_PNG`) | `src/components/hero.rs` | likely `src/components/shell/title_bar.rs` or unused | Same as above — the title bar uses the `Sigil` "IH" text stamp, not the shield PNG. **Likely outcome: declared but unused in Phase 3, with deletion deferred to a future asset cleanup.** Planner verifies. |
| `assets/header.svg` | already removed in Phase 2 | — | — |
| `assets/scanner.svg` | not yet copied (deferred from Phase 2) | **DO NOT COPY in Phase 3** — confirmed unused after grep of `shell.jsx` and `warp-ih.css`. Scanner is unicode-only. | — |

**Resolution of CONTEXT "Scanner SVG copy" discretion:** confirmed unused. The 10 scanner cells are unicode glyphs (`░ ▒ ▓ █`) per `shell.jsx` lines 14-21 and `warp-ih.css` lines 326-330; no `<img>` or `background-image: url(scanner.svg)` reference exists in either source. **Skip the scanner.svg copy entirely.**

**Resolution of CONTEXT "Hover-action button glyphs" discretion:** the prototype glyphs are `⎘` (copy) `↻` (rerun) `↗` (share) per `shell.jsx` line 106-108. Aria-labels are the lowercase `title` attribute values (`copy` / `rerun` / `share`). For Dioxus, use `title: "copy"` on the `<button>` tag; native HTML `title` attribute renders the browser's tooltip and is the prototype's accessibility model.

---

## Wrapper data-attributes (Phase 3 hardcoded values)

The `WarpHermes` root `div.wh-app` carries four data-attributes per `app.jsx` line 232-237. Phase 3 hardcodes the prototype defaults (Phase 5 introduces TweaksPanel runtime switching).

| Attribute | Phase 3 value | Source |
|-----------|---------------|--------|
| `data-theme` | `cyan` | `tweaks?.theme || "cyan"` (default branch) |
| `data-density` | `comfy` | `tweaks?.density || "comfy"` |
| `data-block` | `framed` | `tweaks?.block || "framed"` |
| `data-agent` | `right` | `tweaks?.agent || (variant === "bottom" ? "bottom" : variant === "inline" ? "hidden" : "right")` (right branch for `variant="classic"` default) |

These four attributes are emitted as literal Dioxus rsx attributes:
```
div { class: "wh-app",
      "data-theme": "cyan",
      "data-density": "comfy",
      "data-block": "framed",
      "data-agent": "right",
      ... }
```

---

## Registry Safety

| Registry | Blocks Used | Safety Gate |
|----------|-------------|-------------|
| (none) | — | not applicable |

**Phase 3 introduces no third-party UI components.** The full component tree is hand-ported from `warp2ironhermes/project/app/shell.jsx` to Rust/Dioxus per CONTEXT D-01. No npm packages, no Rust UI crates beyond Dioxus core, no shadcn (project uses Rust/Dioxus, not React/Next.js — shadcn is not applicable to the stack). Registry safety gate is a no-op for this phase.

The Dioxus framework itself is pinned via `Cargo.toml` (`dioxus = "=0.7.1"`) per HYG-01 and is the only UI dependency. No new Cargo dependencies are added in Phase 3 (markdown crate is deferred to Phase 4 per CONTEXT "Deferred Ideas").

---

## Dimension Cross-Reference (for ui-checker)

| Checker dimension | Where validated in this spec |
|-------------------|------------------------------|
| 1. Copywriting | "Copywriting Contract" section — every visible string traceable to `app.jsx` / `shell.jsx` |
| 2. Visuals | "Visuals & Interaction Contracts" — scanner animation, hover affordance, focus ring, sidebar, textarea, sigil, cursor |
| 3. Color | "Color" section — 60/30/10 split, semantic reserved-for tables, stripe contract, pill rotation |
| 4. Typography | "Typography" section — exact px/rem + weight + line-height per role, traceable to `design-tokens.css` and `shell.jsx` inline styles |
| 5. Spacing | "Spacing Scale" section — IH base scale + Warp shell tokens + every hardcoded layout dimension |
| 6. Registry safety | "Registry Safety" section — no third-party blocks; native Dioxus stack |

---

## Checker Sign-Off

- [ ] Dimension 1 Copywriting: PASS
- [ ] Dimension 2 Visuals: PASS
- [ ] Dimension 3 Color: PASS
- [ ] Dimension 4 Typography: PASS
- [ ] Dimension 5 Spacing: PASS
- [ ] Dimension 6 Registry Safety: PASS

**Approval:** pending

---

## Appendix — Pre-Population Provenance

| Source | Decisions used |
|--------|----------------|
| `.planning/REQUIREMENTS.md` | SHELL-01..SHELL-10 acceptance criteria mapped to component inventory + scanner contract |
| `.planning/ROADMAP.md` | Phase 3 goal statement + 6 success criteria |
| `.planning/phases/03-desktop-shell/03-CONTEXT.md` | All 20 D-XX decisions consumed: D-01..05 component decomposition, D-06..09 static rendering scope, D-10..16 block data model, D-17..20 demo composition. All 5 "Claude's Discretion" items resolved (is-tool stripe = `--w-stripe-tool` per CSS; scanner.svg = skip; agent panel content = `seedMessages()` from `app.jsx`; Sigil = own file `sigil.rs`; hover glyphs = `⎘ ↻ ↗`; pre vs div for Ai = div with `white-space: pre-wrap`). |
| `.planning/phases/02-design-system/02-CONTEXT.md` | Phase 2 D-08 (scanner via `@keyframes`) inherited; asset colocation pattern; CSS cascade order |
| `assets/warp-ih.css` (ported) | Every `.wh-*` class name, every dimension, every CSS variable consumption, hover/focus rules, `is-active` palette row, `is-agent` mode pill |
| `assets/design-tokens.css` (ported) | All ANSI palette hex values, type scale, weight scale, spacing scale, scanner constants, pill rotation tokens |
| `warp2ironhermes/project/app/shell.jsx` | Component primitive list, prop signatures, inline styles (traffic lights, brand block, tab dot/×/+, sigil sizing, status pills) |
| `warp2ironhermes/project/app/app.jsx` | Tab labels (`ironhermes chat`/`cargo watch`/`agent · scratch`), status defaults (`Chat`/`claude-sonnet-4`/`anthropic`/`12300/128000`), hint copy, `seedBlocks()`, `seedMessages()`, `PALETTE_ITEMS` |
| User (interactive) | none — every contract field was answerable from upstream sources |

**Gaps explicitly flagged for the planner:**

1. **Scanner `@keyframes` rule not present in ported CSS.** Phase 3 must add a small CSS animation block (option 1 in "Scanner Animation Contract") to satisfy SHELL-09 + CONTEXT D-08 simultaneously. Planner authors `assets/scanner-anim.css` or appends to `assets/warp-ih.css` (verbatim-port deviation; planner decides which is more honest to the source-of-truth principle).

2. **Wordmark/shield asset destination ambiguous.** The prototype's title bar uses `Sigil` text + literal `IronHermes` text — neither asset is rendered in the desktop shell. Planner either declares the consts in `title_bar.rs` for forward use (Phase 5 TweaksPanel may render the wordmark) and `#[allow(dead_code)]`s, or moves them to a centralized `state.rs` brand-asset section. **Recommendation:** declare both consts in `title_bar.rs` with the dead-code allow; Phase 5 work is the natural consumer.

3. **`Block::Out` body fidelity — approach A vs B.** Default to approach A (plain text body, accept color-fidelity loss for the doctor block). Escalate to approach B only if UAT side-by-side review explicitly fails on this.

4. **Focus ring activation** — recommend hardcoding `is-focus` on the input wrap for Phase 3 UAT (approach A in "Focus ring contract") to make SHELL-06 visually verifiable without requiring the reviewer to click into the textarea. Phase 4 inverts this when real focus signals land.

These four items are the only design decisions left for the planner to finalize. Everything else is fully prescribed above.
