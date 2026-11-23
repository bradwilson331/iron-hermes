# Voice-to-Voice (Web Orb) — How It Works & Where to Configure Voices

This guide explains IronHermes's web voice-to-voice feature (orb UI), the two
voice modes, and — most importantly — **where the ElevenLabs voice is configured**
and how per-avatar voice overrides resolve.

Audience: operators setting up the web app for hands-free voice chat. Companion to
[`CONFIGURATION.md`](CONFIGURATION.md). Phase 40.5 added per-avatar voice/provider
config and the free-mode continuous wake-word session.

---

## TL;DR

- There are **two** voice services, chosen by `voice.service` (Phase 54.7):
  - **Turn-based** (`voice.service: turn_based`, the default) — mic → STT → agent → **TTS**.
    **This is the only service that uses ElevenLabs** (or Edge / OpenAI TTS / an
    OpenAI-compatible server such as Kokoro).
  - **Realtime** (`voice.service: realtime`) — full-duplex over an OpenAI-Realtime-compatible
    server configured by the top-level `realtime:` block (`api.openai.com` by default).
    Uses the **realtime voice** (e.g. `shimmer`), **never ElevenLabs**.
- To hear an **ElevenLabs** voice, you must be on the **turn-based** service
  (`voice.service: turn_based`). `voice.barge_in_mode` now only controls how
  turn-based playback is interrupted; a config with no `voice.service` and
  `barge_in_mode: open_mic` still loads as `realtime`.
- The voice is configured in two places, with per-identity winning over global:
  1. **Global** — `tts.provider: elevenlabs` + `tts.elevenlabs.voice_id`.
  2. **Per-avatar** — `identities.<slug>.voice.free_mode_tts_provider` +
     `free_mode_tts_voice`. `None`/absent fields inherit the global value.
- Required keys in `~/.ironhermes/.env`: **`ELEVENLABS_API_KEY`** (TTS),
  an **OpenAI or Groq key** (STT), and your **LLM provider key** (e.g.
  `OPENROUTER_API_KEY`).

---

## The two modes

`voice.service` decides which code path runs when you enter voice mode. It is
decoupled from `voice.barge_in_mode`, which governs turn-based interruption only:

| `voice.service` | Path | Voices used | Notes |
|---|---|---|---|
| `turn_based` (default) | **Free / turn-based** (`start_voice_loop`) | **Edge / ElevenLabs / OpenAI TTS / `openai_compatible` TTS** | STT is `groq`, `openai` or `openai_compatible`. Wake word supported. The ElevenLabs path. |
| `realtime` | **Realtime** (`start_realtime_session`) | **The realtime voice** (`realtime.voice`) | WebRTC; the browser connects directly to `{realtime.base_url}/realtime/calls`. Falls back to turn-based if realtime is unavailable. |

| `barge_in_mode` | Meaning (turn-based service) |
|---|---|
| `push_to_interrupt` (default) | The user presses the record key to interrupt playback |
| `open_mic` | Legacy. With no `voice.service` in the config it is read as `voice.service: realtime` |
| `half_duplex` | Deferred (not wired); treated as turn-based |

> **Gotcha for testing ElevenLabs:** the realtime service tries the realtime
> endpoint first. If it can be reached (for the default OpenAI URL, an OpenAI key
> that is also needed for STT), the session **succeeds and ElevenLabs is never
> called**. It only falls back to the turn-based/ElevenLabs path when realtime is
> *unavailable*. **Set `voice.service: turn_based` to force the ElevenLabs
> turn-based path.**

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
      realtime_voice: shimmer                    # realtime voice (voice.service: realtime only)
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

With `voice.service: realtime` (formerly `voice.barge_in_mode: open_mic`), voice runs
over an **OpenAI-Realtime-compatible server + WebRTC** (`start_realtime_session`). It
uses the realtime voice, configured by the top-level `realtime:` block:

```yaml
voice:
  service: realtime
  realtime_noise_reduction: far_field   # far_field | near_field | off
  realtime_vad_mode: semantic_vad       # semantic_vad | server_vad
realtime:
  base_url: https://api.openai.com/v1   # default; a custom server must be browser-reachable
  # api_key_ref: VOICE_KEY_REALTIME     # optional; the NAME of an env var / vault entry
  model: gpt-realtime
  voice: shimmer                        # global realtime voice
  transcription_model: gpt-4o-mini-transcribe
```

- **How it connects.** The server mints an ephemeral token at
  `{base_url}/realtime/client_secrets`, then the **browser connects directly** to
  `{base_url}/realtime/calls`. The browser, not only the server, must be able to
  reach `base_url`. With **no key** (no `api_key_ref` on a custom URL) no token is
  minted and the browser connects without one.
- Per-identity override: `identities.<slug>.voice.realtime_voice`
  (`resolve_realtime_voice` + `issue_realtime_token` in `server/api.rs`).
- There is no fixed list of allowed models or voices any more: `model`, `voice` and
  `transcription_model` accept any value inside a strict charset and length rule
  (see [`CONFIGURATION.md`](CONFIGURATION.md#realtime-realtime)). For `api.openai.com`
  the panel offers the known OpenAI values; for a custom server it lists what the
  server reports, and always lets you type a custom value. If realtime cannot start,
  voice falls back to turn-based.
- The default OpenAI URL needs an OpenAI key resolvable via
  `providers.openai.api_key_env` (default `OPENAI_API_KEY`); a custom server uses
  `realtime.api_key_ref` (a `VOICE_KEY_*` variable or `secret/voice/*` vault entry —
  never another provider's key name).
- The legacy `voice.realtime_model` / `realtime_voice` / `realtime_transcription_model`
  keys are still read as fallbacks and dropped on the next web Voice save; a config
  with `barge_in_mode: open_mic` and no `voice.service` loads as `realtime`.
- **Wake word does not apply to the realtime service** (the UI greys it out).

ElevenLabs and `realtime_voice` are unrelated: ElevenLabs is turn-based TTS;
`realtime_voice` is the realtime session's voice, used only when
`voice.service: realtime`.

Turn-based replies can also use a self-hosted **OpenAI-compatible** TTS/STT server
(Kokoro, whisper.cpp): see `tts.openai_compatible` / `stt.openai_compatible` in
[`CONFIGURATION.md`](CONFIGURATION.md#text-to-speech-tts).

---

## Required API keys (free-mode ElevenLabs voice-to-voice)

All live in `~/.ironhermes/.env` (never in `config.yaml`):

| Purpose | Env var | Notes |
|---|---|---|
| **TTS (ElevenLabs)** | `ELEVENLABS_API_KEY` | Required for ElevenLabs voices. Without it, replies fall back to keyless Edge. |
| **STT (transcription)** | `OPENAI_API_KEY` **or** `VOICE_TOOLS_OPENAI_KEY` (OpenAI Whisper) **or** `GROQ_API_KEY` (Groq Whisper) — or no key for a self-hosted `stt.openai_compatible.base_url` | `stt.provider: auto` picks the first configured in the order `openai_compatible > groq > openai` (a configured self-hosted server wins over cloud keys; an empty key variable counts as unset). |
| **LLM (the agent)** | your provider key, e.g. `OPENROUTER_API_KEY` | Must match `model.provider` / `providers.<name>.api_key_env`. |

> If you set an OpenAI key for STT **and** leave `voice.service: realtime`, realtime
> will win over ElevenLabs. Use `voice.service: turn_based` for the ElevenLabs path.

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
