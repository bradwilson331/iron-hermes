# IronHermes Design System

A terminal-native design system for [IronHermes](https://github.com/NousResearch/) — the self-improving AI agent by Nous Research, rewritten in Rust. This is a **CLI + TUI + Telegram bot** product, so there is no web aesthetic, no marketing site, no app screens. The surface is a monospaced terminal with 16 named ANSI colors, a bottom status bar, a knight-rider scanner during in-flight work, and a streaming chat.

This system captures that surface so you can design new CLI features, slide decks, docs, or mock terminal screenshots that look and feel like real IronHermes.

---

## Sources

Built from the attached read-only mount `ironhermes/` (a Cargo workspace — Rust 2024 edition, ~360K lines across 7 workspace crates).

Key sources read while building this system:

- `ironhermes/README.md` — top-level product summary
- `ironhermes/.planning/PROJECT.md` — product narrative, roadmap, key decisions
- `ironhermes/Cargo.toml` — dependency and workspace structure
- `ironhermes/crates/ironhermes-cli/src/main.rs` — CLI entry, banner, status/doctor/version commands
- `ironhermes/crates/ironhermes-cli/src/tui/*` — the entire TUI module:
  - `render.rs` — render loop, DECSTBM scroll region, widget compositing
  - `status_line.rs` — dot-separated pill bar
  - `pills.rs` — color rotation palette (cyan · magenta · green · yellow · dimmed)
  - `knight_rider.rs` — 10-cell scanner with triangle-wave sweep
  - `activity.rs` — `Idle | Streaming | ToolCall { name }` state machine
  - `commands.rs` — slash command dispatch, `/help` formatting
  - `extension.rs` — TuiExtension trait, widget slots, style overrides
- `ironhermes/crates/ironhermes-agent/src/prompt_builder.rs` — `DEFAULT_AGENT_IDENTITY`
- `ironhermes/crates/ironhermes-agent/src/personality.rs` — 14 built-in personality presets

The product is **not a web app** and the repo contains no Figma, no brand guidelines, no logo files. This design system is *derived* from the code — every color, glyph, and phrase below is lifted from real Rust source, not invented.

---

## Index

| File | What it contains |
|---|---|
| `README.md` | This file. Context, content, visual, iconography. |
| `colors_and_type.css` | CSS variables for the 16 ANSI colors, semantic roles, and the monospace type scale. |
| `SKILL.md` | Agent Skill entry-point — cross-compatible with Claude Code and the IronHermes skill framework. |
| `fonts/` | Ioskeley Mono webfont files (Berkeley Mono look-alike, Iosevka-based, OFL-licensed). |
| `assets/` | Glyph reference (knight-rider scanner cells, box-drawing chars, status icons). |
| `preview/` | Small HTML cards that render each token group (Type, Colors, Spacing, Components, Brand). Populates the Design System tab. |
| `ui_kits/cli/` | React/JSX recreation of the IronHermes terminal: banner, status bar, scanner, streaming chat, slash commands, `status` and `doctor` outputs. |

---

## Product context

**IronHermes** is a single-binary Rust port of `hermes-agent`, a self-improving AI agent by Nous Research. It runs as:

1. **An interactive CLI** (`ironhermes chat`) with streaming responses, slash commands, a status bar, and a knight-rider scanner while tools execute.
2. **A Telegram bot** (`ironhermes gateway`) for always-on conversational access.
3. **A batch processor** (`ironhermes batch`) for parallel prompt runs.
4. **A cron scheduler** (`ironhermes cron`) for natural-language scheduled tasks.

The differentiator is **self-improvement**: the agent edits its own `SOUL.md` / `AGENTS.md` files to refine its personality and capabilities over time. Configuration lives at `~/.ironhermes/` (`config.yaml` + `.env`). A 10-layer prompt assembly stitches identity, memory, skills, context files, and session overlays together on every turn.

**Not in scope**: web UI, multi-user auth, plugin/extension loading, Discord/Slack (Telegram-first), mobile.

---

## CONTENT FUNDAMENTALS

### Voice

IronHermes' **default identity** (from `prompt_builder.rs`):

> You are IronHermes, an AI assistant created by Nous Research. You are helpful, harmless, and honest.

The default voice is **quiet, competent, no-nonsense engineer**. It does not brand itself. It does not add "I'd be happy to help!" flourishes. It ships.

But IronHermes also ships **14 built-in personality presets** that can fully override the default voice — from `concise` and `technical` to `pirate`, `catgirl`, and `hype` (yes, really). This means the brand has *two* tonal registers:

1. **Operator voice** — CLI output, status text, error messages, system prompts. Terse, mechanical, ANSI-colored. Never uses personality presets.
2. **Agent voice** — chat responses. Default is helpful/honest/direct; personality preset can remix it.

This doc is about the **operator voice** — the TUI, CLI, logs, status — because that's the branded surface. Agent voice is whatever SOUL.md + preset produces.

### Casing

- **Product name**: `IronHermes` (one word, inner caps) in prose; `ironhermes` (lowercase) as the binary name and in paths.
- **Command names**: lowercase — `chat`, `status`, `doctor`, `version`, `gateway`, `cron`, `batch`, `skills`, `memory`.
- **Slash commands**: lowercase with `/` prefix — `/quit`, `/clear`, `/status`, `/help`.
- **Status line pills**: Title-case for mode (`Chat`, `Agent`); model name verbatim (`claude-sonnet-4-20250514`); provider lowercase (`anthropic`, `openrouter`).
- **Banner / header**: `IronHermes Status`, `IronHermes Doctor` — Title-case with product name first.
- **Check icons**: `[OK]` green, `[MISSING]` yellow — ALL CAPS inside brackets.
- **Status values**: `configured` green, `not set` red — lowercase, no punctuation.

### Punctuation + separators

- **Dot separators** (`·`, U+00B7) between pills in the status bar. Always surrounded by single spaces: `" · "`. Always dimmed.
- **Horizontal rules** — `─` (U+2500) repeated 40 times for section dividers in `status` / `doctor` outputs.
- **Em-dashes** (`—`, U+2014) in running copy: *"IronHermes — The self-improving AI agent, rewritten in Rust"*. No spaces around in docs; spaces around in status hints.
- **Ellipses** are plain three dots (`...`), not `…`.

### I vs you

- **Command output** talks *to* the operator: *"Run `ironhermes status` for more details."*
- **Error/interrupt messages** are terse and impersonal: `"^C — turn cancelled"`, `"^C — type /quit to exit"`, `"Goodbye!"`, `"^C×3 — emergency exit"`.
- **Agent responses** use "I" — *"I'll read the file and suggest a patch"* — because the agent is a first-person character with a SOUL.
- Never "we". IronHermes is one binary, one agent.

### Emoji

**No emoji in operator voice. Ever.** No rocket ships, no checkmarks, no fire emoji. The CLI uses ANSI colors + ASCII glyphs + Unicode box-drawing only.

Agent voice *can* use emoji — specifically the `kawaii` / `catgirl` / `uwu` personality presets do, because they're designed to. But the TUI chrome itself is emoji-free.

### Examples of real copy from the codebase

```
IronHermes Status
────────────────────────────────────────
  Home:     ~/.ironhermes/
  Model:    anthropic/claude-sonnet-4-20250514
  Provider: anthropic
  Terminal: bash
  Web:      firecrawl

API Keys
  OpenRouter:  configured
  Anthropic:   configured
  OpenAI:      not set
```

```
You: help me refactor this
Hermes: I'll read the file first...
Tool: read_file {"path":"src/lib.rs"}
Chat · claude-sonnet-4 · anthropic · 12.3K/128.0K (10%) · ctrl+c cancel · /help commands
```

```
^C — turn cancelled
^C — type /quit to exit
^C×3 — emergency exit
Goodbye!
```

The hint strings are the canonical micro-copy. Reuse them verbatim; don't paraphrase.

### Personality preset micro-copy

Just to give the range — these are verbatim from `personality.rs`:

- **concise**: "Be extremely brief and to the point. Use short sentences. Omit pleasantries and filler. Bullet points over paragraphs."
- **technical**: "Respond with precise technical language. Include code examples, specifications, and implementation details. Assume expert-level knowledge."
- **noir**: "Respond as a hard-boiled film noir detective. Use moody, atmospheric language. Everything is a case. The city never sleeps."
- **hype**: "RESPOND WITH MAXIMUM ENERGY AND ENTHUSIASM! Everything is AMAZING and INCREDIBLE! Use caps, exclamation marks, and pure HYPE!"

Use these when showing the `/personality <name>` command in examples.

---

## VISUAL FOUNDATIONS

The IronHermes "canvas" is a terminal emulator. Everything below is framed in those constraints.

### Type

**Monospace only.** The design system ships **Ioskeley Mono** — a free Iosevka build configured to match Berkeley Mono — as the canonical typeface for all IronHermes HTML artifacts (decks, docs, mockups). In the actual terminal, whatever the user's terminal font is wins — it will *always* be monospaced, and you don't get to choose.

There is no display face, no serif, no italic variation. Weight range used: **400 regular** for body, **700 bold** for the banner / section headers / pill labels (via `colored`'s `.bold()`), plus **dimmed** (ANSI attr 2) as a separate register used for separators and hints.

**Type scale for HTML mockups** (from `colors_and_type.css`):

| Token | Size | Line | Use |
|---|---|---|---|
| `--type-h1` | 24px / 1.5rem | 1.2 | Section banners (`IronHermes Status`). Bold. |
| `--type-h2` | 18px / 1.125rem | 1.3 | Sub-sections (`API Keys`, `Tools`). Bold. |
| `--type-body` | 14px / 0.875rem | 1.5 | Default terminal body. |
| `--type-small` | 12px / 0.75rem | 1.4 | Dimmed hints, secondary info. |
| `--type-mono` | 14px / 0.875rem | 1.5 | Code blocks inside docs. Same as body. |

The body and mono tokens resolve to the same size because **everything is mono**. The split exists so semantic HTML docs can still differentiate prose from code visually via color.

> **Typeface**: IronHermes has no original brand font. **Ioskeley Mono** (an Iosevka build that closely matches Berkeley Mono's geometry — single-storey `g`, flat-arc parens, dotted `0`, square punctuation) is shipped locally in `fonts/` as the canonical choice. Regular / Medium / Bold / Italic / BoldItalic are all included. If you want a different face (Berkeley Mono, Commit Mono, IBM Plex Mono, Monaspace), swap the `@font-face` declaration in `colors_and_type.css`. Flagged to the user for sign-off.

### Color

The brand palette **is the ANSI palette**. IronHermes does not have hex-coded custom colors — it uses the `colored` crate's 16 named colors plus `dimmed` and `bold` attributes. The values in `colors_and_type.css` are one canonical mapping (close to VS Code / iTerm defaults), but **any terminal theme is valid**. Dark background is assumed.

**Pill rotation** (the only hard color contract, from `pills.rs`):

```
pill[0] → cyan
pill[1] → magenta
pill[2] → green
pill[3] → yellow
pill[4] → dimmed
pill[5] → cyan   (wraps)
…
hint    → always dimmed
```

**Semantic colors** (from code usage):

| Role | ANSI | Use |
|---|---|---|
| `--fg` | default | Body text. |
| `--fg-dim` | ANSI dim | Separators, hints, `Tool:` label, "Goodbye!", subtext. |
| `--accent-primary` | cyan / bright cyan | Banner title, `Hermes:` prompt, scanner lit cell, mode pill. |
| `--accent-secondary` | magenta | Model pill (pill index 1). |
| `--success` | green | `configured`, `[OK]`, `You:` prompt, provider pill (index 2). |
| `--warn` | yellow | `[MISSING]`, tool name under `Running:`, tokens pill (index 3). |
| `--danger` | red | `not set`, `^C×3 — emergency exit`, fatal errors. |

### Spacing

Terminal UI is **grid-based in monospace cells**, not in pixels. Spacing tokens in CSS map 1 cell = 8px ÷ 7.2ch (approx), but the rules are:

- **2-space indent** inside sections (`  Home:`, `  Model:`).
- **1-space gap** after `:` for label/value pairs; values align in columns (use padding to reach the widest label + 1).
- **Blank line** between sections in `status` / `doctor` output.
- **40-char horizontal rule** under section banners.
- **Single space** around the `·` dot separator in pill bars.

Radius is **0 everywhere**. Terminals do not have rounded corners. Don't add them in HTML mockups either.

### Elevation / shadow

**None.** No drop shadows, no card elevation, no inner shadows. If you need to show a "panel" in an HTML mockup, use a **1-cell border drawn with box-drawing characters** (`─ │ ┌ ┐ └ ┘`) in `--fg-dim`, not a CSS border-radius + shadow.

### Background

**Solid single color.** `#0d1117` in the default HTML theme (matches a common dark terminal). No gradients, no textures, no images, no blur, no transparency. One exception: `dimmed` text is achieved with the CSS color variable `--fg-dim`, which is a literal darker grey (`#6e7681`), not opacity.

For full-bleed HTML mockups (slides, docs), the entire page is `--bg`. Widgets *inside* the terminal do not get their own background — they inherit `--bg` and separate via the pill dot separator or by color rotation. This is a hard constraint of the real product so we honor it in the mocks.

### Borders

- **Horizontal rule**: `"─".repeat(40)` in `--fg-dim`.
- **Box frames** (optional, for HTML-only panels): single-line box-drawing set (`─ │ ┌ ┐ └ ┘`) in `--fg-dim`.
- **No double-line borders** (`═ ║ ╔ ╗`). They read as decorative in a way that clashes with the restraint of the rest of the system.

### Animation

The TUI has **exactly one** animation primitive: the **knight-rider scanner**.

- 10 cells wide (`TRACK_WIDTH = 10`).
- **100ms frame rate** (`FRAME_PERIOD = Duration::from_millis(100)` → 10 fps).
- **Triangle-wave sweep**: lit cell moves 0 → 9 → 0 over 18 ticks (1.8 sec round-trip).
- **Easing: linear** — no ease-in/out. It's a scanner, not a fade.
- **Trail**: distance 0 = `█` bright cyan; distance 1 = `▓` cyan; distance 2 = `▒` dimmed cyan; distance 3+ = `░` dimmed.
- **Visibility**: only shown when `ActivityState` is `Streaming` or `ToolCall { name }`. Hidden in `Idle`.

In HTML mockups, recreate it with a `requestAnimationFrame` loop at 10 fps that updates the lit index + trails. Do not use CSS keyframe animations — they drift from the real one.

**Other animation rules**:

- **No fade-ins, no slide-ins, no easing curves on anything else.** Output either is there or isn't.
- **Cursor blinking**: whatever the terminal does. Don't fake it in mockups.
- **Streaming**: tokens are `print!`ed one at a time as they arrive from the LLM. Do not chunk, do not add a typing animation. It already looks like typing because it *is*.

### Hover / press / focus

The terminal does not have hover. It has focus, implicitly — there is exactly one cursor. So there are **no hover states** in this design system.

**Press / activation** in HTML mockups: the only "clickable" things are slash commands and the prompt input. On activation, show **no state change** beyond the command echoing into the transcript and the scanner appearing if a tool fires. Do not add button-press CSS.

**Focus ring**: a single dim underline cursor (`_` or `▎` in `--accent-primary`) on the prompt line. No box-shadow glow.

### Transparency + blur

**Never used.** Pure solid colors. If the terminal background is translucent (e.g. in iTerm/Alacritty with blur), that's the user's OS-level config, not ours.

### Corner radii

**0px, everywhere, with no exceptions.**

### Cards

Cards as a concept do not exist in the real TUI. If you need a card in an HTML artifact (e.g. a design system preview card, a deck slide), use:

- Solid `--bg-elevated` (slightly lighter than `--bg`, no shadow).
- 1px solid `--border-subtle` border.
- 0px radius.
- 16px / 2 cells of internal padding.
- Optionally a box-drawing frame instead of CSS border.

### Imagery

**None.** No stock photos, no hero illustrations, no mascots, no icons in the raster sense. If you need imagery in a slide or doc, the only acceptable options are:

1. **A literal screenshot of the terminal** running IronHermes.
2. **A code block** rendered in the same monospace font.
3. **An ASCII/Unicode diagram** built from box-drawing characters.

No warmth, no grain, no b&w photography — imagery is text or nothing.

### Layout rules

- **Fixed bottom bar**: status line always occupies `rows - 1`. Scanner (when visible) occupies `rows - 2`. Prompt row is `rows - 3`. Reserved row count is `3` by default; extensions can push it higher (max widget height 5 per slot).
- **DECSTBM scroll region**: `\x1b[1;{scroll_end}r` reserves the bottom rows so scrolling transcript never overwrites the status bar.
- **`prepare_prompt` / `finish_prompt`**: called around `rl.readline()` to position the cursor at `rows - reserved` cleanly.
- **No modal dialogs.** Commands dispatch inline and print their result into the scrolling area.
- **No multi-pane layouts.** One transcript. One prompt. One status bar. That's the whole UI.

### Density

**Dense.** IronHermes is an operator tool, not a demo. Section banners are 40 chars wide. The status bar packs 4 pills + a hint on a single line. Default typography is 14px in HTML mockups — which is generous for a mock but gives room for the dot separators to breathe.

---

## ICONOGRAPHY

**There are no icons.** IronHermes is a CLI — no icon fonts, no SVG icon libs, no Lucide/Heroicons CDN, no PNG sprites. The codebase has zero icon assets.

What the CLI does use, and which *function* as icons:

### ASCII text tokens in brackets

- `[OK]` (green) — a configured/present check in `doctor` output.
- `[MISSING]` (yellow) — a missing config/key/file.

These are the only "status icons" in the product. Use them verbatim. Do not replace with ✓ / ✗.

### Unicode glyphs

The codebase uses these specific Unicode characters for visual roles. Treat them as the official glyph set:

| Glyph | U+ | Role |
|---|---|---|
| `·` | 00B7 | Dot separator between status pills. Dimmed. |
| `─` | 2500 | Horizontal rule under section banners. Dimmed. |
| `█` | 2588 | Knight-rider lit cell. Bright cyan. |
| `▓` | 2593 | Scanner trail, distance 1. Cyan. |
| `▒` | 2592 | Scanner trail, distance 2. Dimmed cyan. |
| `░` | 2591 | Scanner empty cell / trail distance 3+. Dimmed. |
| `│ ┌ ┐ └ ┘` | 2502/250C/2510/2514/2518 | Optional box-drawing for HTML panel frames. |
| `^C` | literal | Interrupt marker in messages like `^C — turn cancelled`. |
| `~` | literal | Home-dir shorthand in paths: `~/.ironhermes/`. |

### Emoji

**Not used in TUI / CLI output.** Exception: the `kawaii` / `catgirl` / `uwu` / `hype` personality presets produce emoji and emoticons (`(^_^)`, `=^.^=`, `~`) in agent responses — those are part of agent voice, not the TUI chrome.

### "Logos"

There is no logo file in the repo. The brand signature is **the word-mark `IronHermes` printed bold-cyan** (from `main.rs`: `"IronHermes".bold().cyan()`). That's it. No Hermes-winged-helmet icon, no Rust ferris, no Nous Research mark. The word is the logo.

In `assets/` we include:
- `wordmark.svg` — the literal text "IronHermes" rendered in Ioskeley Mono, bold, in `--accent-primary`, for use in HTML headers where you need a static image.
- `scanner.svg` — a static snapshot of the knight-rider scanner frame at tick 5 (middle of the sweep), for documentation where you can't animate.
- `glyphs.txt` — the full Unicode reference table.

### Icon substitution policy

If a future design surface needs icons (e.g. a settings screen in a hypothetical GUI wrapper), **do not substitute Lucide or Heroicons**. Instead, stick to the ASCII/Unicode vocabulary. If that's not enough, ask the user — don't silently introduce a new visual register.

---

## ui_kits

| Kit | What it mocks | Design width |
|---|---|---|
| `ui_kits/cli/` | The IronHermes terminal — banner, streaming chat, status line, scanner, slash commands, `status` and `doctor` output. | 880px (80 cols × 11px cell) |

See `ui_kits/cli/README.md` for the component inventory.

---

## Caveats

- **No original brand font.** Ioskeley Mono (OFL, Iosevka-based Berkeley Mono clone) is the canonical typeface. See Type section.
- **No logos.** The only brand mark is the word "IronHermes" in bold cyan.
- **No real marketing surface.** IronHermes is a dev-tool CLI; there is no website, no app, no slides. The UI kit recreates the only surface that exists: the terminal.
- **Color values are approximate.** The real "IronHermes blue" is whatever the user's terminal renders for ANSI cyan. We picked close-to-default values for HTML mocks; users on different terminal themes will see different shades.
- **Agent voice is not prescribed here.** It comes from `SOUL.md` + personality preset and can be anything from "concise senior engineer" to "nya~ catgirl". This doc governs the *operator* voice (TUI chrome, CLI output).
