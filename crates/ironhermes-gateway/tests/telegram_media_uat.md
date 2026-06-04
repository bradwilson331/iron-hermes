# Manual UAT: Telegram Media Delivery — Live UAT

> **Phase reference:** Phase 36.17.2.2
> `.planning/phases/36.17.2.2-ironhermes-telegram-client-delivers-streaming-final-media-me/`
>
> **Architecture (locked):** Final-edit rendered as MarkdownV2 with smart escape; streaming intermediate edits stay plain text per prior-phase 36.17.2 D-03; `<MEDIA: path|url>` tags extracted and dispatched as native Telegram attachments AFTER the final edit completes; failed attachments reinsert into the placeholder; tags inside fenced code pass through verbatim.
>
> **Locked decisions exercised here:**
> - **D-01** — Final edit sends `parse_mode: MarkdownV2` with smart escape.
> - **D-02** — Single retry as plain text on parse-mode 400.
> - **D-07** — Text-first, attachment-second; no caption on attachments.
> - **D-09** — Tags inside ```` ``` ```` fences pass through verbatim, no extraction.
> - **D-10** — Missing / unreadable / oversized media → warn + reinsert tag literal into placeholder.
> - **D-11** — Multiple tags = sequential `send_media` calls in stream order.
> - **D-12** — URL form passes through to Telegram unchanged (no local fetch).
> - **D-14** — `.ogg`/`.opus` → `sendVoice` (inline bubble); `.mp3`/`.m4a`/`.flac`/`.wav` → `sendAudio` (music player).
> - **D-15** — File size pre-check before multipart upload.
> - **D-20** — Live Telegram UAT runbook gated as a `checkpoint:human-verify` task.
>
> **Inherited contracts (from 36.17.2 / 36.17.2.1 / 36.17.2.2):**
> - D-03 (36.17.2) — Streaming intermediate edits stay plain text; cursor █ strip preserved.
> - D-06 (36.17.2) — Per-chat worker pop-loop unchanged; media dispatch happens INSIDE `handle_with_multimodal` so the queue/worker shape is preserved.
> - D-22 (36.17.2) — Live Telegram UAT runbook gated on operator `approved` reply.
>
> **Why this runbook exists:** the automated suite in `tests/telegram_media_delivery.rs` covers the in-process handler + adapter paths exhaustively. What automated tests cannot verify is Telegram client RENDERING — MarkdownV2 bold/italic visible, voice-bubble UI vs music-player UI, the user's actual perception of attachment order, missing-path reinsert appearance in the chat. This runbook exists to confirm those user-visible properties.

---

## Prerequisites

Before running any scenario:

| Item | Value | How to set |
|------|-------|------------|
| `TELEGRAM_BOT_TOKEN` | Your test bot's token from `@BotFather` | `export TELEGRAM_BOT_TOKEN=...` |
| `IRONHERMES_HOME` | Test config dir — separate from production | `export IRONHERMES_HOME=/tmp/uat-36.17.2.2-home` |
| Test chat | A Telegram chat where the bot is admin'd | Add bot, `/start`, send "hi" to confirm |
| Agent config | Free-text messages must produce a real agent turn | Pick a model in `cli-config.yaml`; verify the bot responds |
| Test media files | Pre-create local files for scenarios 2-5 | `/tmp/uat-photo.png` (any small PNG ≤ 1 MiB), `/tmp/uat-voice.ogg` (any small Opus ≤ 1 MiB), `/tmp/uat-music.mp3` (any small MP3 ≤ 5 MiB), `/tmp/uat-doc.pdf` (any small PDF ≤ 5 MiB), `/tmp/uat-oversize.png` (a 21 MiB file — `dd if=/dev/zero of=/tmp/uat-oversize.png bs=1M count=21`) |
| Public URL | A reachable public-internet PNG URL for scenario 6 | e.g. `https://www.gstatic.com/webp/gallery/1.webp` or any operator-chosen stable URL |

Start the gateway in a terminal you can `ctrl+c` later:

```bash
cargo run --release --bin ironhermes -- gateway 2>&1 | tee /tmp/uat-36.17.2.2-gateway.log
```

Keep this terminal visible — Scenarios 7 and 8 read the log to verify the
`tracing::warn` reinsert / parse-fallback lines.

---

## Scenario 1 — Text-only MarkdownV2 final edit (D-01)

**What this verifies (D-01):** Final-edit body renders with bold + italic visible in the Telegram client. `parse_mode: MarkdownV2` with smart escape converts `**bold**`/`*italic*`/`` `code` ``/`[link](url)` into the proper Telegram client rendering.

**Steps:**

1. In your test chat, send: `Reply with the literal text: **bold** and *italic* and `code` and a [link](https://example.com).`
2. Wait for the agent's reply to finalize (cursor █ stops).

**Expected:** The reply renders `**bold**` as bold text, `*italic*` as italic text, `` `code` `` as a monospace span, and `[link](https://example.com)` as a clickable link. No raw asterisks or backticks visible.

**Regression signal:** If raw `**bold**` or `*italic*` text appears literally, MarkdownV2 escape is over-escaping; if 👀 reaction appears but no body, the V2 edit silently 400'd and D-02 fallback did not catch it.

---

## Scenario 2 — Single photo (D-07)

**What this verifies (D-07):** Text edit + `sendPhoto` arrive in the correct order — placeholder text first (with the `<MEDIA: ...>` tag stripped), then the native photo attachment as a SECOND Telegram message.

**Steps:**

1. Send: `Reply with the literal text "Here is the picture:" followed by <MEDIA: /tmp/uat-photo.png> on its own line.`
2. Wait for the agent's reply.

**Expected:** The placeholder message renders as "Here is the picture:" (the tag stripped). A SECOND message appears immediately after with the photo as a native Telegram attachment.

**Regression signal:** Photo arrives BEFORE text → D-07 ordering broken; tag literal `<MEDIA: /tmp/uat-photo.png>` visible in the placeholder → extractor not wired; the photo never arrives → `media_sender` not set in `runner.rs` (Pitfall 3).

---

## Scenario 3 — Voice .ogg (D-14)

**What this verifies (D-14):** `.ogg` files dispatch to `sendVoice` and render as the inline round voice-bubble UI (NOT a music-player and NOT a generic file attachment).

**Steps:**

1. Send: `Reply with <MEDIA: /tmp/uat-voice.ogg>.`
2. Wait.

**Expected:** A Telegram voice message bubble (round play-button UI) appears, not a generic file attachment or audio music-player.

**Regression signal:** If a music-player UI appears, D-14 dispatch is routing `.ogg` to `sendAudio` instead of `sendVoice`; if a generic document UI appears, dispatch fell through to `sendDocument` (wrong extension table).

---

## Scenario 4 — Audio .mp3 (D-14)

**What this verifies (D-14):** `.mp3` files dispatch to `sendAudio` and render as the music-player UI (waveform + play button + title metadata) — NOT as a voice bubble.

**Steps:**

1. Send: `Reply with <MEDIA: /tmp/uat-music.mp3>.`
2. Wait.

**Expected:** A Telegram audio attachment with the music-player UI (waveform + play button + title metadata).

**Regression signal:** Voice-bubble appears instead → dispatch routed `.mp3` to `sendVoice` (wrong; Telegram rejects non-opus on `sendVoice` anyway, so the D-10 reinsert may fire showing the tag literal — a different failure signature).

---

## Scenario 5 — Multi-tag (D-11)

**What this verifies (D-11):** Three tags produce one text edit + 3 attachments in correct order. `D-15` size cap is implicitly verified by the files being well under the limits.

**Steps:**

1. Send: `Reply with three attachments in this exact order: photo, then voice, then document. The literal text body should say "Sending three things:". Use <MEDIA: /tmp/uat-photo.png>, <MEDIA: /tmp/uat-voice.ogg>, <MEDIA: /tmp/uat-doc.pdf>.`
2. Wait until all 4 messages have settled.

**Expected:** 4 messages in order: (1) placeholder rendered as "Sending three things:" (or similar — tags stripped), (2) photo attachment, (3) voice bubble, (4) document attachment.

**Regression signal:** Out-of-order arrival → D-11 sequential await broken; missing one → D-15 size cap triggered (verify file sizes); single combined message instead of 4 → extractor not extracting individual tags.

---

## Scenario 6 — URL form (D-12)

**What this verifies (D-12):** A `<MEDIA: https://...>` tag passes through to Telegram which fetches and renders. No local fetch happens; the gateway emits the URL verbatim and Telegram's servers retrieve it.

**Steps:**

1. Send: `Reply with <MEDIA: https://www.gstatic.com/webp/gallery/1.webp>.` (or operator-chosen stable URL).
2. Wait.

**Expected:** Photo renders in chat as a native attachment.

**Regression signal:** D-10 reinsert appears with the URL literal → Telegram's fetch failed (verify URL is publicly reachable from Telegram's servers, not just from your local machine); 404-style error in `/tmp/uat-36.17.2.2-gateway.log` → URL was rejected before passthrough.

---

## Scenario 7 — Missing path reinsert (D-10)

**What this verifies (D-10):** A nonexistent path triggers `warn!` + reinsert of the tag literal in the placeholder. The log line MUST contain only the filename (not the full path) per T-LOG-LEAK mitigation.

**Steps:**

1. Send: `Reply with literal text "Sending file:" followed by <MEDIA: /tmp/this-file-does-not-exist-xxx.png>.`
2. Wait.

**Expected:** Placeholder updates to show "Sending file:" AND the tag literal `<MEDIA: /tmp/this-file-does-not-exist-xxx.png>` appended (visible as text). The gateway log contains a `warn!` line referencing the path's filename.

**Regression signal:** Tag literal does NOT appear in the placeholder → D-10 reinsert path broken; full path (not just filename) appears in the log → T-LOG-LEAK mitigation broken.

---

## Scenario 8 — Markdown parse error → plain-text fallback (D-02)

**What this verifies (D-02):** A malformed MarkdownV2 body (unclosed `**`) triggers the D-02 silent retry as plain text. The reply text still reaches the user (rendered plainly) and a `warn!` line confirms the fallback fired.

**Steps:**

1. Send: ``Reply with the literal text "**unclosed bold and `unclosed_code` and a trailing dot."`` (Note the intentional dangling `**` and dangling backtick — invalid MarkdownV2.)
2. Wait.

**Expected:** The reply text appears in the chat (as plain text, NOT bold-rendered). The gateway log contains a `warn!` line "MarkdownV2 parse failed; retrying as plain text (D-02 fallback)" with the `message_id` of the failed edit.

**Regression signal:** 👀 reaction but no body → D-02 second-failure path fired; the body shows bold-rendered text → escape function silently fixed the malformed input rather than letting D-02 catch it (unexpected, but not necessarily wrong — note the deviation).

---

## Scenario 9 — Tag inside fence passes through (D-09)

**What this verifies (D-09):** A `<MEDIA: ...>` tag inside a fenced code block is NOT extracted; the literal text reaches the user inside the fence. No attachment dispatch happens.

**Steps:**

1. Send: ``Reply with a fenced code block containing literally `<MEDIA: /tmp/uat-photo.png>` on its own line, with the surrounding text "Here is the tag syntax:" before the fence.``
2. Wait.

**Expected:** Placeholder shows "Here is the tag syntax:" followed by a fenced code block containing the literal text `<MEDIA: /tmp/uat-photo.png>`. NO photo attachment arrives.

**Regression signal:** Photo attachment arrives → extractor's D-09 fence skip is broken; the tag is stripped from the visible body → same.

---

## Sign-off Checklist

Run each scenario once on a live Telegram bot. Tick when verified.

- [ ] **Scenario 1** — Text-only MarkdownV2 renders.
- [ ] **Scenario 2** — Single photo arrives AFTER the text edit.
- [ ] **Scenario 3** — `.ogg` voice bubble appears.
- [ ] **Scenario 4** — `.mp3` music-player UI appears.
- [ ] **Scenario 5** — Multi-tag arrives in correct order (photo, voice, document).
- [ ] **Scenario 6** — URL form fetches and renders.
- [ ] **Scenario 7** — Missing path triggers warn + reinsert.
- [ ] **Scenario 8** — Malformed Markdown triggers D-02 plain-text fallback.
- [ ] **Scenario 9** — Tag inside fence passes through; no attachment.

Verifier (your name): ____________________

Date (YYYY-MM-DD): ____________________

Notes / failures (attach screenshots + log excerpts if any scenario fails):

```
[paste here]
```

---

## Document History

- **2026-06-03 (Phase 36.17.2.2-07):** Initial runbook for `<MEDIA: ...>` media delivery and MarkdownV2 final-text rendering. Mirrors the 36.17.2 D-22 protocol that closed the session-queue phase.

---

*Phase: 36.17.2.2 — ironhermes-telegram-client-delivers-streaming-final-media-me*
*Runbook updated: 2026-06-03*
