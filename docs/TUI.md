# The IronHermes TUI

The terminal UI is the in-process `ratatui` surface in
`crates/ironhermes-cli/src/tui_rata/`. It renders the same agent core as every
other surface — this document covers what the terminal adds on top: scrolling,
selection and clipboard, shell execution, hyperlinks, images, and recall history.

Launch it with:

```bash
hermes chat
# or, from a checkout:
cargo run -p ironhermes-cli -- chat
```

Press `?` for the in-app Help overlay, or `/` to open the command palette. Both
are generated from the live keybinding registry, so they never drift from the
build you are running. This document is a companion to them, not a replacement.

## Read this first: links, and why clicking does nothing

**Mouse capture is ON by default.** With capture on, your terminal forwards
clicks to the TUI instead of handling them itself — which means the terminal's
own hyperlink-click handling never fires. A link will render correctly (cyan and
underlined) and a Ctrl/Cmd-click will merely *highlight* it.

To click a link, turn capture off first:

```
/mouse off
```

…then Ctrl/Cmd-click. `/mouse on` restores scroll-wheel scrolling and in-app
selection. `/mouse` with no argument reports the current state.

This is tracked as **G-14**. The hyperlink implementation itself is correct — the
escape sequences reach the terminal and the link opens as soon as capture is out
of the way.

> **macOS Terminal.app does not implement OSC8 hyperlinks at all.** Links will
> never be clickable there no matter what you do with mouse capture. Use iTerm2,
> Ghostty, WezTerm, or kitty if you want clickable links.

## Keybindings

Taken from the live registry (`keybindings.rs::build_help_registry`); `?` shows
the same list in-app.

| Key | Action |
|-----|--------|
| `Enter` | Submit |
| `Shift+Enter` | Insert newline without submitting |
| `Up` / `Down` | Recall previous / next history entry |
| `PageUp` / `PageDown` | Scroll up / down |
| `End` | Jump to bottom |
| `Esc` | Close overlay / clear input |
| `?` | Open the Help overlay (types a literal `?` if the input line is non-empty) |
| `/` | Open the command palette (type `/` at the start of input) |
| `Tab` | Palette: insert the highlighted command |
| `v` | Start visual selection (vim-style) |
| `Ctrl+Y` | Yank (copy) the current selection |
| `!` | Run a shell command (prefix your input) |
| `Ctrl+T` | Toggle the expanded thinking panel |
| `Ctrl+K` | Toggle the Skills Hub |
| `Ctrl+B` | Toggle push-to-talk voice capture |
| `Ctrl+C` | Cancel (press twice to force-quit) |

## Selection and clipboard

Two ways to select, both landing in your system clipboard:

- **Mouse** — drag-select directly in the transcript. This works with mouse
  capture ON (the default). Double-click selects a word, triple-click a line.
- **Keyboard** — press `v` to enter visual mode, extend with `hjkl` or the arrow
  keys, `y` to yank, `Esc` to cancel. `Ctrl+Y` yanks the current selection
  without entering visual mode.

Selecting a hyperlink copies the visible label text, never the invisible escape
bytes behind it.

### How the copy actually happens

Two mechanisms fire, and which one delivers depends on your terminal:

- **OSC52** — an escape sequence asking the terminal to set the clipboard. Works
  over SSH and inside tmux when the terminal supports it. Apple Terminal.app does
  not implement OSC52.
- **`pbcopy`** (macOS only) — an additive native write that runs alongside OSC52
  on local sessions. It is deliberately **skipped under SSH**
  (`SSH_CONNECTION` / `SSH_TTY` / `SSH_CLIENT`), where it would target the remote
  host's clipboard rather than yours.

The status-line toast reflects what was actually observed rather than assuming
success: it reports a confirmed copy when the native write succeeded, and says so
plainly when the copy could only be attempted.

## `!` shell execution

Prefix any input with `!` to run it in your shell:

```
!ls -la
!git status
!echo $MY_VAR
```

- Commands inherit **your real shell environment**, by design — `!echo $MY_VAR`
  resolves the variable you exported before launching.
- Output renders in the transcript **and enters the conversation**, so you can
  ask a follow-up question about it.
- Commands are bounded by a 30-second timeout; partial output is preserved.
- Interactive programs are refused rather than run, because a full-screen program
  would fight the TUI for the terminal: `vim`, `vi`, `nvim`, `emacs`, `nano`,
  `less`, `more`, `top`, `htop`, `man`, `watch`, `tmux`, `screen`, `ssh`,
  `mysql`, `psql`, `python`, `node`, `irb`, `ranger`, and similar.

> **Known limitation (CR-01).** The refusal check inspects only the *first* word
> of your command, while the whole string is handed to `sh -c`. A command like
> `!true; vim` therefore slips past it, and because the child inherits the
> terminal's stdin, the interactive program can attach to your live terminal and
> garble the display. There is no security boundary here — `!` runs whatever you
> type by design — but the failure is messier than the refusal implies. Avoid
> chaining interactive programs behind a metacharacter.

## Hyperlinks

Bare `http(s)://` URLs and markdown `[label](url)` links in the transcript become
real terminal hyperlinks via OSC8 escape sequences — not an in-app click
hit-test. They are rendered cyan and underlined so the affordance is visible even
in terminals that cannot follow them.

Only `http` and `https` targets are linkified. Other schemes — `javascript:`,
`file:`, `data:` — render as plain, unstyled text and emit no escape bytes.

See [the note above](#read-this-first-links-and-why-clicking-does-nothing) for
why you need `/mouse off` before clicking.

## Images

The TUI renders images inline rather than showing raw tag text:

- `<MEDIA:...>` tags in agent replies become clickable chips.
- `/image <path>` shows a chip for a local file.

Opening a chip launches an overlay viewer. In a terminal with a graphics protocol
(kitty, iTerm2) images render at full fidelity; elsewhere they degrade to
halfblocks. Degradation is graceful, but halfblocks imposes a hard pixel ceiling —
a large source image will look coarse. Improving overlay fidelity is tracked
separately as Phase 36.6.5.

## Recall history

`Up` and `Down` walk your input history — and that history includes **slash
commands and `!` shell commands**, not just prose sent to the model. Recalled
slash and `!` entries never leak into the conversation as messages.

## Slash commands

Press `/` for the palette, or `/commands` for the full list. A few that matter in
the terminal specifically:

| Command | Purpose |
|---------|---------|
| `/mouse on\|off` | Toggle mouse capture — **required to click links** |
| `/image <path>` | Show a chip for a local image file |
| `/model` | Open the model picker overlay |
| `/clear` | Clear the conversation |
| `/help` | Help |

## Known limitations

Open and tracked as of Phase 36.6.4:

| ID | Issue |
|----|-------|
| **G-14** | Mouse capture (on by default) blocks terminal-native link clicks; `/mouse off` is the way through. |
| **G-13** | A one-character selection reports `1 chars` instead of `1 char`. Cosmetic. |
| **G-11** | `/clear` empties the visible transcript, but `!` shell blocks and image chips remain. |
| **CR-01** | The `!` interactive-command refusal is bypassable with a shell metacharacter, and the child inherits stdin (see above). |
| **WR-01** | `mosh` sessions set none of the `SSH_*` variables, so the macOS `pbcopy` write fires on the remote host and reports a copy you cannot paste locally. |

OSC52 clipboard *delivery* has never been observed end-to-end in isolation,
because on macOS `pbcopy` now fires alongside it on every local session — so a
successful paste cannot be attributed to one mechanism or the other. Both are
attempted, the clipboard demonstrably works, and OSC52 remains the only mechanism
for the SSH case.
