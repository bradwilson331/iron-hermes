---
name: ironhermes-design
description: Use this skill to generate well-branded interfaces and assets for IronHermes, either for production or throwaway prototypes/mocks/etc. Contains essential design guidelines, colors, type, fonts, assets, and UI kit components for prototyping.
user-invocable: true
---

Read the README.md file within this skill, and explore the other available files.

If creating visual artifacts (slides, mocks, throwaway prototypes, etc), copy assets out and create static HTML files for the user to view. If working on production code, you can copy assets and read the rules here to become an expert in designing with this brand.

If the user invokes this skill without any other guidance, ask them what they want to build or design, ask some questions, and act as an expert designer who outputs HTML artifacts _or_ production code, depending on the need.

## Quick orientation

IronHermes is a **terminal-native** AI CLI. There is no website, no mobile app, no marketing surface — the product IS the terminal UI. Everything is monospace, ANSI-colored, and rendered with `ratatui` + `crossterm`. When in doubt: terminal vocabulary, no emoji in operator voice, no rounded corners, no shadows, no gradients.

## Key files

- `README.md` — full context, content fundamentals, visual foundations, iconography
- `colors_and_type.css` — design tokens (colors, type, spacing); import this in any CSS you write
- `fonts/` — Ioskeley Mono webfont files (Iosevka-based Berkeley Mono look-alike, OFL-licensed)
- `ui_kits/cli/` — React recreation of the TUI. Use `Terminal.jsx`, `Scanner.jsx`, `StatusLine.jsx`, `Transcript.jsx`, `Prompt.jsx` as the source of truth for component structure
- `preview/` — small demonstration cards for every foundation

## Hard rules

1. **Monospace everywhere.** No proportional fonts, ever.
2. **No emoji** in operator voice. Use `[OK]` / `[MISSING]` / `[FAIL]`, Unicode box-drawing (`─ │ ┌ ┐ └ ┘`), and the scanner glyphs (`█ ▓ ▒ ░`). Emoji ARE allowed in agent voice if the personality preset (e.g. catgirl, uwu) calls for it.
3. **Colors come from the ANSI 16 + dim/bright.** Don't invent new hexes. The only brand color is `--ansi-cyan` for IronHermes itself.
4. **Radius 0. Shadow none.** Panels separate via 1px `--border-subtle` or Unicode box-drawing.
5. **Dot separator `·` (U+00B7)** between status pills. Never `|`, `/`, or `-`.
6. **Section banner pattern:** bold cyan title + 40-char `─` rule underneath + 2-space-indented key/value body.
7. **Knight-rider scanner** only renders during `Streaming` or `ToolCall`. 10 cells, 100ms tick, triangle-wave sweep. Use `Scanner.jsx` or copy the pattern.
