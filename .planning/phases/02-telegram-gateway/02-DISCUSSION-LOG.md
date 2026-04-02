# Phase 2: Telegram Gateway - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-01
**Phase:** 02-telegram-gateway
**Areas discussed:** Streaming UX, Message scope, Session lifecycle, Error presentation, Media handling, Whitelist behavior, Concurrency UX, Bot commands menu

---

## Streaming UX

### Cursor indicator style

| Option | Description | Selected |
|--------|-------------|----------|
| Block cursor █ | Blinking-style block appended to end of text — mimics terminal typing effect | ✓ |
| Ellipsis … | Three dots appended — simple, universally understood | |
| Spinner emoji ⏳ | Hourglass or similar emoji — more visual but adds clutter | |
| You decide | Claude picks what works best for Telegram rendering | |

**User's choice:** Block cursor █
**Notes:** None

### Tool execution visibility

| Option | Description | Selected |
|--------|-------------|----------|
| Tool name shown | Show "⚙️ Running: terminal..." as temporary status line | ✓ |
| Silent processing | Keep showing last LLM output with cursor | |
| You decide | Claude picks balanced approach | |

**User's choice:** Tool name shown
**Notes:** None

### Markdown handling during streaming

| Option | Description | Selected |
|--------|-------------|----------|
| Send as Markdown | Use Markdown parse mode throughout — risk of broken formatting | |
| Plain during stream, Markdown final | Stream as plain text, final edit switches to Markdown | ✓ |
| MarkdownV2 throughout | MarkdownV2 with proper escaping — more robust but complex | |
| You decide | Claude picks based on Telegram API behavior | |

**User's choice:** Plain during stream, Markdown final
**Notes:** None

### 4096-char overflow handling

| Option | Description | Selected |
|--------|-------------|----------|
| Chain messages | Split into multiple messages at natural breakpoints | ✓ |
| Truncate with notice | Cut at 4096 chars with truncation notice | |
| You decide | Claude picks best approach | |

**User's choice:** Chain messages
**Notes:** None

---

## Message Scope

### Message types beyond text

| Option | Description | Selected |
|--------|-------------|----------|
| Text only | Ignore photos, docs, voice, stickers | |
| Text + captions | Process text and captions on photos/docs | |
| Text + documents | Process text and accept document uploads | |
| You decide | Claude picks based on hermes-agent | |

**User's choice:** Text + documents + PDFs + images (free text)
**Notes:** User expanded scope beyond offered options to include all document types and images

### Group chat behavior

| Option | Description | Selected |
|--------|-------------|----------|
| @mention only | Only respond when @mentioned | ✓ |
| All messages | Respond to every message in group | |
| No group support | DMs only in Phase 2 | |
| You decide | Claude picks for single-operator bot | |

**User's choice:** @mention only
**Notes:** User added requirement for a whitelist on what Telegram people can talk to the agent

### Whitelist location

| Option | Description | Selected |
|--------|-------------|----------|
| Config YAML | List of allowed Telegram user IDs in config.yaml | ✓ |
| Environment variable | Comma-separated user IDs in env var | |
| Both (config + env) | Config YAML primary, env var override | |
| You decide | Claude picks based on existing patterns | |

**User's choice:** Config YAML
**Notes:** None

### Unauthorized user behavior

| Option | Description | Selected |
|--------|-------------|----------|
| Silent ignore | No response — bot appears offline | ✓ |
| Polite rejection | Reply with "I'm not configured to chat with you" | |
| You decide | Claude picks security posture | |

**User's choice:** Silent ignore
**Notes:** None

---

## Session Lifecycle

### Session management commands

| Option | Description | Selected |
|--------|-------------|----------|
| /new and /clear | /new starts fresh (archives old), /clear wipes without archiving | ✓ |
| /new only | Single command to start fresh | |
| No slash commands | Sessions managed by timeout only | |
| You decide | Claude picks right set | |

**User's choice:** /new and /clear
**Notes:** None

### Session timeout

| Option | Description | Selected |
|--------|-------------|----------|
| No timeout | Sessions persist until explicitly cleared | |
| 24 hours | Session expires after 24h of inactivity | ✓ |
| 1 hour | Aggressive cleanup after 1h | |
| You decide | Claude picks sensible default | |

**User's choice:** 24 hours but configurable
**Notes:** User specified timeout should be configurable

### First conversation greeting

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, from SOUL.md | Bot introduces itself using personality from SOUL.md | ✓ |
| Simple static greeting | Fixed "Hello, I'm IronHermes" message | |
| No greeting | Bot only speaks when spoken to | |
| You decide | Claude picks for personality-driven agent | |

**User's choice:** Yes, from SOUL.md
**Notes:** None

### Typing indicator behavior

| Option | Description | Selected |
|--------|-------------|----------|
| Continuous typing | Send typing action every 5s throughout entire agent run | ✓ |
| Only before first chunk | Show typing until first streamed edit, then stop | |
| You decide | Claude picks based on responsiveness | |

**User's choice:** Continuous typing
**Notes:** None

---

## Error Presentation

### Agent error mid-response

| Option | Description | Selected |
|--------|-------------|----------|
| Append error notice | Append warning to whatever was already streamed — preserves partial | ✓ |
| Replace with error | Replace streamed message with error — cleaner but loses content | |
| You decide | Claude picks most useful approach | |

**User's choice:** Append error notice
**Notes:** None

### Rate limit visibility

| Option | Description | Selected |
|--------|-------------|----------|
| Silent retry | Automatically retry with backoff — user never sees issues | |
| Visible after 3 retries | Retry silently up to 3 times, then inform user | ✓ |
| You decide | Claude picks retry/visibility balance | |

**User's choice:** Visible after 3 retries
**Notes:** None

### Tool error visibility

| Option | Description | Selected |
|--------|-------------|----------|
| Agent handles it | Tool errors go back to LLM — agent decides what to tell user | ✓ |
| Show tool errors | Expose tool error messages directly to user | |
| You decide | Claude picks matching existing pattern | |

**User's choice:** Agent handles it
**Notes:** None

---

## Media Handling

### Image processing

| Option | Description | Selected |
|--------|-------------|----------|
| Download + pass as vision | Download and pass to LLM as vision/image input | ✓ |
| Download + describe | Download and tell agent "User sent an image: [filename]" | |
| You decide | Claude picks based on LLM client support | |

**User's choice:** Download + pass as vision
**Notes:** None

### Document/PDF processing

| Option | Description | Selected |
|--------|-------------|----------|
| Download + extract text | Download, extract text content, inject as user message context | ✓ |
| Download + save to workspace | Download to temp dir, agent uses read_file tool | |
| You decide | Claude picks fitting approach | |

**User's choice:** Download + extract text
**Notes:** None

### File size limit

| Option | Description | Selected |
|--------|-------------|----------|
| 20MB (Telegram max) | Accept anything Telegram allows | ✓ |
| 5MB | Conservative limit for memory safety | |
| You decide | Claude picks sensible default | |

**User's choice:** 20MB (Telegram max)
**Notes:** None

---

## Whitelist Behavior

### Whitelist format

| Option | Description | Selected |
|--------|-------------|----------|
| Telegram user IDs | Numeric user IDs — stable, doesn't change on rename | ✓ |
| Telegram usernames | @usernames — human-readable but can change | |
| Both supported | Accept either, resolve usernames on first contact | |
| You decide | Claude picks most reliable | |

**User's choice:** Telegram user IDs
**Notes:** None

### Group chat authorization

| Option | Description | Selected |
|--------|-------------|----------|
| Whitelist applies everywhere | Only whitelisted users trigger bot, even in groups | ✓ |
| Groups are open | Anyone in group can @mention, whitelist only for DMs | |
| You decide | Claude picks security posture | |

**User's choice:** Whitelist applies everywhere
**Notes:** None

### Empty whitelist default

| Option | Description | Selected |
|--------|-------------|----------|
| Empty = deny all | No entries means nobody can talk — secure by default | ✓ |
| Empty = allow all | No whitelist means open access — easier to start | |
| You decide | Claude picks based on security principles | |

**User's choice:** Empty = deny all
**Notes:** None

---

## Concurrency UX

### Overlapping messages

| Option | Description | Selected |
|--------|-------------|----------|
| Queue it | New message waits until current run completes | ✓ |
| Cancel + new | Cancel in-flight run, start with new message | |
| Parallel runs | Both execute simultaneously | |
| You decide | Claude picks for conversational UX | |

**User's choice:** Queue it
**Notes:** None

### Queue acknowledgment

| Option | Description | Selected |
|--------|-------------|----------|
| React with emoji | Add 👀 reaction to queued message | ✓ |
| No acknowledgment | Silently queue, typing indicator continues | |
| You decide | Claude picks most natural approach | |

**User's choice:** React with emoji
**Notes:** None

---

## Bot Commands Menu

### Commands to register

| Option | Description | Selected |
|--------|-------------|----------|
| /start | Standard Telegram entry point — triggers SOUL.md greeting | ✓ |
| /new | Start fresh conversation session | ✓ |
| /clear | Clear conversation history | ✓ |
| /help | Show available commands and capabilities | ✓ |

**User's choice:** All four commands
**Notes:** None

### Command registration method

| Option | Description | Selected |
|--------|-------------|----------|
| Auto-register on startup | Call setMyCommands at boot | ✓ |
| Manual BotFather setup | User registers manually | |
| You decide | Claude picks most practical | |

**User's choice:** Auto-register on startup
**Notes:** None

---

## Claude's Discretion

- Exact message splitting algorithm for chain messages
- PDF text extraction library choice
- Queue depth limit per user
- Exact /help text content
- sendChatAction timing details

## Deferred Ideas

None — discussion stayed within phase scope
