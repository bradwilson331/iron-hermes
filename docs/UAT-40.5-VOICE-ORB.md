# UAT Runbook — Phase 40.5: Orb Customization + Per-Avatar Voice + Free-Mode Wake Word

> **STATUS: PAUSED mid-UAT (2026-06-27).** Current results:
> - **Test 1** (orb render-mode switching) — fix shipped (`2f234b99a`); **retest pending**.
> - **Test 2** (per-identity voice) — **PASS** (open_mic realtime: Bloom→shimmer, Groovy→nova/etc.).
> - **Test 3** (wake-word session) — turn-based crash fixed (`ee35de3a3`); **retest pending** (set `push_to_interrupt`).
> - **Test 4** (orb knobs live preview) — covered by the Test 1 hydration fix; **retest pending**.
>
> Open out-of-scope issue deferred to **Phase 40.6** (`.planning/phases/40.6-head-avatar-glb-fix/40.6-NOTES.md`):
> Groovy head-avatar GLB fallback ("Avatar unavailable") + wheel.rs signal-scope warnings.
> Resume: rebuild (`dx serve --package iron_hermes_ui`) → re-run Tests 1/3/4 → `/gsd-verify-work 40.5`.

Step-by-step manual test guide for the web orb voice features shipped in phase 40.5.
Run these in a browser after the automated verification passed (see
`.planning/phases/40.5-orb-customize-voice-provider-per-avatar/40.5-VERIFICATION.md`).

Background on how the feature works: [`VOICE-TO-VOICE.md`](VOICE-TO-VOICE.md).
Config reference: [`CONFIGURATION.md`](CONFIGURATION.md).

---

## Gap-closure round 1 (2026-06-27)

From the first UAT pass:

- **Test 1 (orb appearance) — FIXED** (`2f234b99a`). Selecting an orb identity in
  "Applies to" now hydrates that identity's saved style/hue/size/glow into the
  editor cards **and** the live orb. Previously the editor stayed on "classic" and
  the orb never changed, because the read snapshot didn't carry per-identity
  appearance and nothing updated the orb context on identity switch. **Rebuild/
  re-serve and re-run Test 1.**
- **Test 3 (wake word) — NOT a bug; by design.** In `open_mic` the wake-word toggle
  is intentionally disabled and renders OFF (the realtime session is always-on
  full-duplex; there is no armed/wake step). To exercise the wake-word free session,
  set `voice.barge_in_mode: push_to_interrupt`.
- **Voice change in `config.yaml` not reflected in UI — refresh behavior.** Server
  TTS delivery re-reads config every turn (so the new voice plays), but the settings
  panel caches its snapshot at open. **Reopen the settings panel** (in voice mode it
  remounts and re-fetches) or reload the page to see externally-edited values. Edits
  made *through the UI* persist and display immediately.

---

## Gap-closure round 2 (2026-06-27)

- **Turn-based voice crash — FIXED** (`ee35de3a3`). Entering `push_to_interrupt`
  voice mode panicked with *"Could not find context BeepEnabledCtx"*. The beep ctx
  was provided inside the VoiceSettings child but consumed by `voice_loop` (a
  non-descendant); it's now provided at the HermesApp root like the other voice
  ctxs. `open_mic` never hit this path (it uses the realtime session), which is why
  it only appeared after switching to turn-based. **Rebuild and re-test turn-based +
  wake word.**
- **"Avatar unavailable — using orb" (Groovy) — separate Phase 40.2 head-avatar
  issue, needs browser console.** `avatar.js` posts `__ihAvatarError` (either
  `GLB_LOAD_FAIL` from the GLTFLoader, or a runtime throw in the rigged-avatar
  setup/animate path) and the Rust error-poll swaps to the orb + shows the notice.
  The Groovy model is a 17.4 MB rigged GLB (skeleton + viseme morphs) — more complex
  than facecap. To pinpoint it, capture the browser console when the notice appears
  (look for `[ironHermesAvatar] GLB load failed: …` or a JS exception). Not in the
  40.5 (orb/voice-provider) scope.
- **`wheel.rs` "Copy Value created in ScopeId" warnings** — Dioxus signal-ownership
  warnings from the avatar wheel picker (40.2); non-fatal but worth a separate
  cleanup (move the signals up to the owning parent scope).

---

## 0. Prerequisites

### 0.1 API keys — `~/.ironhermes/.env`

| Needed for | Env var | Required? |
|---|---|---|
| ElevenLabs TTS (free-mode voice) | `ELEVENLABS_API_KEY` | Test 2 (per-identity voice) |
| STT (transcription) | `OPENAI_API_KEY` or `VOICE_TOOLS_OPENAI_KEY` or `GROQ_API_KEY` | Tests 2 & 3 |
| LLM (the agent) | `OPENROUTER_API_KEY` (matches `model.provider`) | Tests 2 & 3 |

> Tests 1 and 4 (orb visuals + knobs) are pure front-end and need **no keys**.

### 0.2 Config — `~/.ironhermes/config.yaml`

The seeded identities and ElevenLabs defaults are already present. The one value
that matters for exercising the **ElevenLabs free-mode path** is `barge_in_mode`:

```yaml
voice:
  barge_in_mode: push_to_interrupt   # REQUIRED for Tests 2 & 3 to use ElevenLabs.
                                      # Leave as `open_mic` ONLY if you intend to
                                      # test the OpenAI realtime path instead.
  beep_enabled: true
  wake_word:
    enabled: true
    phrase: hey hermes

tts:
  provider: elevenlabs               # global fallback voice
  elevenlabs:
    voice_id: pNInz6obpgDQGcFmaJgB   # Adam
    model_id: eleven_multilingual_v2

# Seeded in config.yaml already — orb_bloom is pre-wired to ElevenLabs/Adam:
identities:
  orb_bloom:
    display_name: Bloom
    appearance: { style: bloom, base_hue: 280, size: 1.0, glow: 0.8 }
    voice:
      free_mode_tts_provider: elevenlabs
      free_mode_tts_voice: pNInz6obpgDQGcFmaJgB
      realtime_voice: shimmer
```

> **Why `push_to_interrupt`?** `open_mic` tries the OpenAI Realtime API first and,
> if it succeeds (which it will whenever an OpenAI key is present for STT), you'll
> hear an OpenAI realtime voice (`shimmer`) — **not** ElevenLabs. See the gotcha in
> `VOICE-TO-VOICE.md`. Voice/identity config changes take effect on the next turn
> (no restart needed); a `barge_in_mode` change should be made before entering voice
> mode.

### 0.3 Build & serve the web app

`iron_hermes_ui` is excluded from workspace default-members, so always pass the
package explicitly:

```bash
# From the repo root:
dx serve --package iron_hermes_ui
# (or a production-style build:)
# dx build --platform web --package iron_hermes_ui
```

Open the URL `dx` prints (typically `http://localhost:8080`). Use a Chromium-based
browser and **grant microphone permission** when prompted (Tests 2 & 3).

> Tip: open the browser devtools Console — orb/voice errors and the WebGL lifecycle
> surface there, which helps diagnose any failures.

---

## How to record results

For each test, mark **PASS / FAIL** and note anything unexpected. When all pass,
close the phase with `/gsd-verify-work 40.5` (it walks the same items and marks the
phase complete).

---

## Test 1 — Orb render-mode switching  *(no keys needed)*

**Goal:** Each preset style switches the live orb render mode cleanly.

1. Open the app; the orb should be visible (orb avatar active, not a head avatar).
2. Open **Voice Settings**.
3. In **"Applies to,"** select an orb identity (e.g. **Bloom**).
4. The **Appearance** section shows a 2×2 grid: **Classic / Bloom / ASCII / Network**.
5. Click each style card in turn and watch the orb:

| Style | Expected |
|---|---|
| Classic | Icosahedron mesh (clean 3D shape) |
| Bloom | Glow / postprocessing halo around the orb |
| ASCII | Text-art overlay replaces the 3D canvas |
| Network | Animated connection lines |

**Expected:** Each click switches modes immediately. **No** black flicker, JS
errors in the console, or "WebGL context lost."

**Result:** ☐ PASS ☐ FAIL — notes: ________________________________

---

## Test 2 — Per-identity TTS voice playback  *(needs ElevenLabs + STT + LLM keys, `push_to_interrupt`)*

**Goal:** An avatar speaks with its own configured ElevenLabs voice; an avatar with
no override uses the global voice.

1. Confirm `voice.barge_in_mode: push_to_interrupt` and `ELEVENLABS_API_KEY` set.
2. Make **Bloom (`orb_bloom`)** the active orb/identity.
3. Enter voice mode and have a short spoken (or typed) exchange so the agent replies.
4. Listen to the spoken reply.

**Expected:** The reply is spoken in the **ElevenLabs Adam** voice (the `orb_bloom`
override).

5. Now switch the active identity to **Classic (`orb_classic`)** (no voice override).
6. Trigger another reply.

**Expected:** The reply uses the **global** `tts:` provider/voice instead (still
ElevenLabs Adam if global is set as above — to make the difference audible, set a
*different* global `tts.elevenlabs.voice_id`, or set global `tts.provider: edge`,
before this step).

**Result:** ☐ PASS ☐ FAIL — notes: ________________________________

> If you instead hear `shimmer`/an OpenAI voice, you're in `open_mic` realtime mode —
> switch `barge_in_mode` to `push_to_interrupt`.

> **`open_mic` variant (your current config).** In `open_mic` you are testing the
> **per-identity *realtime* voice** (OpenAI), not ElevenLabs. The seeded overrides
> are **Bloom → `shimmer`**, **Groovy → `nova`**. Switch the active avatar between
> Bloom and Groovy and confirm the spoken voice changes accordingly; avatars with no
> `realtime_voice` override use the global `voice.realtime_voice`. (To test the
> ElevenLabs path instead, set `voice.barge_in_mode: push_to_interrupt` and use the
> steps above.)

---

## Test 3 — Wake-word free-mode session end-to-end  *(needs STT + LLM keys, `push_to_interrupt`)*

> **Requires `push_to_interrupt`.** The wake word applies to turn-based mode only —
> in `open_mic` (your current config) the wake-word control is **greyed out** and has
> no effect (the realtime session is always-on full-duplex; there is no "armed/wake"
> step). To run this test, temporarily set `voice.barge_in_mode: push_to_interrupt`,
> then revert to `open_mic` afterward if desired. Skip/N/A this test if you are only
> validating the realtime path.

**Goal:** Hands-free wake → listen → reply → re-arm loop with idle timeout.

1. Confirm `voice.wake_word.enabled: true`, `phrase: hey hermes`,
   `barge_in_mode: push_to_interrupt`, `beep_enabled: true`.
2. Enter voice mode. The orb should enter the **Armed** state (waiting for the wake
   phrase).
3. Say **"hey hermes."**

**Expected:** A **chime** plays, the orb transitions to **Listening**, your speech is
captured, the agent replies (and speaks the reply per Test 2).

4. Remain silent for ~15 seconds.

**Expected:** The session **auto-exits** back to Armed (idle timeout ~150 polls).

5. Re-trigger with "hey hermes," then click **"Stop Listening."**

**Expected:** The "Stop Listening" button exits the listening session immediately.

6. During the agent's spoken reply, confirm the mic does **not** re-capture the
   agent's own audio (half-duplex — capture is paused while playback is active).

**Result:** ☐ PASS ☐ FAIL — notes: ________________________________

---

## Test 4 — Orb appearance knobs live preview  *(no keys needed)*

**Goal:** Hue / size / glow knobs update the orb in real time (before Save).

1. With an orb identity active and the orb visible, open **Voice Settings →
   Appearance**.
2. Drag the **Base hue** slider (0–360°).

**Expected:** All four per-state orb colors (idle / listening / thinking / speaking)
shift proportionally relative to the new base hue — live, immediately.

3. Drag the **Size** slider (0.5–2.0×).

**Expected:** The orb scales up/down in real time.

4. Drag the **Glow** slider (0.0–1.0). Best observed on a **Bloom**-style orb.

**Expected:** Bloom/glow intensity changes immediately.

5. (Optional) **Save**, reload the page, reopen the avatar — values persist
   (written to `identities.<slug>.appearance` in `config.yaml`).

**Result:** ☐ PASS ☐ FAIL — notes: ________________________________

---

## Summary

| Test | Needs keys? | Result |
|---|---|---|
| 1. Orb render-mode switching | No | ☐ PASS ☐ FAIL |
| 2. Per-identity TTS voice playback | ElevenLabs + STT + LLM | ☐ PASS ☐ FAIL |
| 3. Wake-word free-mode session | STT + LLM | ☐ PASS ☐ FAIL |
| 4. Orb appearance knobs live preview | No | ☐ PASS ☐ FAIL |

When all four pass, run `/gsd-verify-work 40.5` to mark the phase complete.

If any fail, capture the browser console output and the relevant
`~/.ironhermes/config.yaml` values, and report — they become gap-closure items.
