# Voice-to-Voice (Web Orb) — How It Works & Where to Configure Voices

This guide explains IronHermes's web voice-to-voice feature (orb UI), the two
voice modes, and — most importantly — **where the ElevenLabs voice is configured**
and how per-avatar voice overrides resolve.

Audience: operators setting up the web app for hands-free voice chat. Companion to
[`CONFIGURATION.md`](CONFIGURATION.md). Phase 40.5 added per-avatar voice/provider
config and the free-mode continuous wake-word session.

---

## TL;DR

- There are **two** voice modes, chosen by `voice.barge_in_mode`:
  - **Free mode** (`push_to_interrupt`) — turn-based: mic → STT → agent → **TTS**.
    **This is the only mode that uses ElevenLabs** (or Edge / OpenAI TTS).
  - **Realtime mode** (`open_mic`) — full-duplex over the OpenAI Realtime API.
    Uses **OpenAI realtime voices only** (e.g. `shimmer`), **never ElevenLabs**.
- To hear an **ElevenLabs** voice, you must be in **free mode**
  (`voice.barge_in_mode: push_to_interrupt`).
- The voice is configured in two places, with per-identity winning over global:
  1. **Global** — `tts.provider: elevenlabs` + `tts.elevenlabs.voice_id`.
  2. **Per-avatar** — `identities.<slug>.voice.free_mode_tts_provider` +
     `free_mode_tts_voice`. `None`/absent fields inherit the global value.
- Required keys in `~/.ironhermes/.env`: **`ELEVENLABS_API_KEY`** (TTS),
  an **OpenAI or Groq key** (STT), and your **LLM provider key** (e.g.
  `OPENROUTER_API_KEY`).

---

## The two modes

`voice.barge_in_mode` decides which code path runs when you enter voice mode
(`crates/iron_hermes_ui/src/components/hermes_app/screens/voice_mode.rs:282-323`):

| `barge_in_mode` | Path | Voices used | Notes |
|---|---|---|---|
| `push_to_interrupt` (default) | **Free / turn-based** (`start_voice_loop`) | **Edge / ElevenLabs / OpenAI TTS** | Wake word supported. The ElevenLabs path. |
| `open_mic` | **Realtime** (`start_realtime_session`) | **OpenAI realtime voices only** | WebRTC; needs an OpenAI key. Falls back to free mode if realtime is unavailable. |
| `half_duplex` | Deferred (not wired) | — | Treated as turn-based. |

> **Gotcha for testing ElevenLabs:** `open_mic` tries the OpenAI Realtime API
> first. If you have an OpenAI key configured (which you also need for STT), the
> realtime session **succeeds and ElevenLabs is never called**. It only falls back
> to the free/ElevenLabs path when realtime is *unavailable* (no resolvable OpenAI
> key). **Set `voice.barge_in_mode: push_to_interrupt` to force the ElevenLabs
> free-mode path.**

---

## Free-mode flow (the ElevenLabs path)

```
   ┌─ enter voice mode (orb) ─ voice.barge_in_mode = push_to_interrupt
   │
   ▼
 [Armed]  ── wake word? (voice.wake_word.enabled) ────────────────┐
   │  speak "hey hermes"                                          │ (if wake word off,
   │  → chime (voice.beep_enabled)                                │  capture starts directly)
   ▼                                                              │
 [Listening] ── browser captures mic, VAD ends turn on silence ◄──┘
   │
   ▼
 STT  ── server transcribes audio (stt.provider: openai/groq whisper)
   │     active_identity slug is frozen at session start
   ▼
 Agent turn ── ChatRequest { active_identity } → full Hermes agent (LLM + tools)
   │
   ▼
 auto_speak_reply(text, dispatcher, active_identity)        ← ws.rs:2005
   │   • re-reads config fresh
   │   • validates the slug (is_known_identity)
   │   • effective = config.effective_tts_config_for_identity(slug)   ← THE voice resolution
   │   • provider = build_tts_registry(effective).get(effective.provider)
   │   • provider.synthesize(text) → ~/.ironhermes/audio_cache/<uuid>.mp3
   ▼
 AudioOut binary frames → browser plays the MP3
   │   AudioPlaybackActiveCtx = true while playing → mic capture paused (half-duplex)
   ▼
 [back to Armed] ── 'rearm loop; ~15 s idle auto-exits; "Stop Listening" exits
```

Key source points:
- Free/realtime decision: `screens/voice_mode.rs:282`
- Wake-word arming + session loops + chime: `voice_loop.rs` (`'rearm`/`'session`, `play_wake_chime`)
- Active identity frozen at session start: `voice_loop.rs:326` (`frozen_active_identity`, from `AvatarModeCtx.active_identity`, validated by `is_known_identity`)
- STT + agent turn + auto-speak: `server/ws.rs` (`auto_speak_reply` at line 2005)
- Per-identity TTS resolution: `crates/ironhermes-core/src/config.rs` (`effective_tts_config_for_identity`)

---

## Where the ElevenLabs voice is configured

There are two layers. **Per-identity overrides win; unset fields inherit global.**

### 1. Global default (`tts:`)

Applies to every spoken reply unless an active identity overrides it.

```yaml
tts:
  provider: elevenlabs            # edge | elevenlabs | openai
  elevenlabs:
    voice_id: pNInz6obpgDQGcFmaJgB   # Adam (default). This is the ElevenLabs voice.
    model_id: eleven_multilingual_v2
    output_format: mp3
```

`tts.elevenlabs.voice_id` is the ElevenLabs **voice ID** (the value from your
ElevenLabs dashboard's Voice Library), not a display name. Output is MP3 in v1.

### 2. Per-avatar override (`identities.<slug>.voice`)

Each orb/avatar identity may override the provider and/or voice for free-mode
turns. This is what makes "each avatar carries its own voice" work.

```yaml
identities:
  orb_bloom:
    display_name: Bloom
    voice:
      free_mode_tts_provider: elevenlabs        # edge | openai | elevenlabs | null(=inherit)
      free_mode_tts_voice: pNInz6obpgDQGcFmaJgB  # Adam — overrides tts.elevenlabs.voice_id
      realtime_voice: shimmer                    # OpenAI realtime voice (open_mic only)
```

### Resolution precedence (exactly how it works)

`Config::effective_tts_config_for_identity(slug)` (config.rs) clones the global
`tts:` config, then applies the active identity's overrides:

1. If `identities.<slug>.voice.free_mode_tts_provider` is set → it replaces
   `effective.provider`. Otherwise the global `tts.provider` stays.
2. If `identities.<slug>.voice.free_mode_tts_voice` is set → it is written into the
   **matching provider's** voice field via this match:
   - `"elevenlabs"` → `effective.elevenlabs.voice_id = voice`
   - `"openai"`     → `effective.openai.voice = voice`
   - `"edge"`       → `effective.edge.voice = voice`
3. Any `None`/absent field inherits the global value (partial override).
4. If the active slug is `None` or unknown, the **global `tts:`** config is used
   verbatim.

So for an avatar to speak with a specific ElevenLabs voice, set **both**
`free_mode_tts_provider: elevenlabs` **and** `free_mode_tts_voice: <voice_id>` on
that identity. Setting only the voice (without provider) writes the voice into the
sub-config of whatever provider is active — so keep them consistent.

### Which identity is "active"?

- The active identity is `AvatarPrefs.active_identity` (persisted in the browser's
  localStorage), set when you pick an orb/avatar in the UI.
- It is **frozen at the start of each voice session** — mid-session avatar switches
  do not change the running turn's voice (parity with the realtime token path).
- In the **Voice Settings** panel, the **"Applies to"** selector lets you edit Global
  vs each identity's voice; inherited fields show a "(from Global)" badge.

### Seeded identities (shipped defaults)

`default_seed_identities()` (config.rs) ships these; they are written into
`config.yaml` on first run and back-filled if missing:

| Slug | Display | Free-mode TTS | Realtime voice | Appearance |
|---|---|---|---|---|
| `orb_bloom` | Bloom | ElevenLabs `pNInz6obpgDQGcFmaJgB` (Adam) | `shimmer` | bloom, hue 280, glow 0.8 |
| `groovy` | Groovy | ElevenLabs `21m00Tcm4TlvDq8ikWAM` (Rachel) | `nova` | — (head/no orb) |
| `orb_classic` | Classic | inherit global | inherit global | classic, hue 186 |
| `orb_ascii` | ASCII | inherit global | inherit global | ascii, hue 120 |
| `orb_network` | Network | inherit global | inherit global | network, hue 200, size 1.2 |
| `facecap` | Morph Head | inherit global | inherit global | — (head avatar) |

To test the ElevenLabs path quickly, make **`orb_bloom`** the active identity — it
is pre-wired to ElevenLabs/Adam.

---

## The ElevenLabs provider itself

`crates/ironhermes-tools/src/tts/elevenlabs.rs`:

- **Requires `ELEVENLABS_API_KEY`** in the process environment
  (`~/.ironhermes/.env`). `is_available()` returns true iff that var is set; when
  unset, spoken replies fall back to Edge.
- Calls `POST https://api.elevenlabs.io/v1/text-to-speech/{voice_id}` with header
  `xi-api-key: <ELEVENLABS_API_KEY>` and body
  `{ text, model_id, output_format: "mp3_44100_128" }`.
- Output is **MP3** (Opus voice-bubble output is deferred).
- Input text is truncated to **10,000 chars** (the `eleven_multilingual_v2` limit).
- The API key is never logged or written to trajectory output.

---

## Realtime mode (contrast — does NOT use ElevenLabs)

With `voice.barge_in_mode: open_mic`, voice runs over the **OpenAI Realtime API +
WebRTC** (`start_realtime_session`). It uses OpenAI realtime voices, configured by:

```yaml
voice:
  barge_in_mode: open_mic
  realtime_model: gpt-realtime
  realtime_voice: shimmer        # global realtime voice
  realtime_transcription_model: gpt-4o-mini-transcribe
  realtime_noise_reduction: far_field   # far_field | near_field | off
  realtime_vad_mode: semantic_vad       # semantic_vad | server_vad
```

- Per-identity override: `identities.<slug>.voice.realtime_voice`
  (`resolve_realtime_voice` + `issue_realtime_token` in `server/api.rs`).
- Allowed realtime voices (server-side whitelist `REALTIME_ALLOWED_VOICES`):
  **`alloy`, `shimmer`, `echo`, `verse`, `ash`, `ballad`, `coral`, `sage`**.
  An unlisted value triggers graceful fallback to turn-based voice.
- Realtime needs an OpenAI key resolvable via `providers.openai.api_key_env`
  (default `OPENAI_API_KEY`).
- **Wake word does not apply in `open_mic`** (the UI greys it out).

ElevenLabs and `realtime_voice` are unrelated: ElevenLabs is free-mode TTS;
`realtime_voice` is an OpenAI voice used only in `open_mic`.

---

## Required API keys (free-mode ElevenLabs voice-to-voice)

All live in `~/.ironhermes/.env` (never in `config.yaml`):

| Purpose | Env var | Notes |
|---|---|---|
| **TTS (ElevenLabs)** | `ELEVENLABS_API_KEY` | Required for ElevenLabs voices. Without it, replies fall back to keyless Edge. |
| **STT (transcription)** | `OPENAI_API_KEY` **or** `VOICE_TOOLS_OPENAI_KEY` (OpenAI Whisper) **or** `GROQ_API_KEY` (Groq Whisper) | `stt.provider: auto` picks the first present (Groq preferred). |
| **LLM (the agent)** | your provider key, e.g. `OPENROUTER_API_KEY` | Must match `model.provider` / `providers.<name>.api_key_env`. |

> If you set an OpenAI key for STT **and** leave `barge_in_mode: open_mic`, realtime
> will win over ElevenLabs. Use `push_to_interrupt` for the ElevenLabs path.

---

## Quick recipes

### Hear ElevenLabs on every reply (global)
```yaml
voice:
  barge_in_mode: push_to_interrupt
tts:
  provider: elevenlabs
  elevenlabs:
    voice_id: pNInz6obpgDQGcFmaJgB   # pick any voice ID from your ElevenLabs library
```
…and `ELEVENLABS_API_KEY` set in `.env`.

### Give one avatar its own ElevenLabs voice
```yaml
voice:
  barge_in_mode: push_to_interrupt
identities:
  orb_bloom:
    voice:
      free_mode_tts_provider: elevenlabs
      free_mode_tts_voice: <your_voice_id>
```
Make `orb_bloom` the active orb in the UI; other avatars keep the global voice.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Replies use Edge / a generic voice, not ElevenLabs | `ELEVENLABS_API_KEY` unset, or you're in `open_mic` | Set the key; set `barge_in_mode: push_to_interrupt` |
| You hear `shimmer`/an OpenAI voice, not ElevenLabs | `open_mic` realtime succeeded | Switch to `push_to_interrupt` |
| No transcription / nothing happens after you speak | No STT key | Set `OPENAI_API_KEY`/`VOICE_TOOLS_OPENAI_KEY` or `GROQ_API_KEY` |
| Per-avatar voice ignored | Identity not active, or only `free_mode_tts_voice` set without `free_mode_tts_provider` | Pick the avatar; set both provider + voice on the identity |
| `ElevenLabs API returned 401` | Bad/expired key | Check `ELEVENLABS_API_KEY` |
| `ElevenLabs API returned 4xx invalid_voice_id` | Wrong `voice_id` | Use a voice ID from your ElevenLabs library |
| Config change not taking effect | — | `auto_speak_reply` re-reads config each turn; just start a new turn. No restart needed for voice/identity changes. |

See [`UAT-40.5-VOICE-ORB.md`](UAT-40.5-VOICE-ORB.md) for a step-by-step test runbook.
