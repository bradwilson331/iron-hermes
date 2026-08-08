<!-- generated-by: gsd-doc-writer -->
# Getting Started

This guide walks you from zero to a running IronHermes session. For prerequisite
details see the sections below; for configuration depth see
[CONFIGURATION.md](CONFIGURATION.md) and for architecture context see
[ARCHITECTURE.md](ARCHITECTURE.md).

---

## Prerequisites

| Requirement | Version | Notes |
|---|---|---|
| Rust toolchain | stable (2024 edition) | Install via [rustup.rs](https://rustup.rs) |
| Cargo | bundled with Rust | Required to build from source |
| LLM API key | — | OpenRouter, Anthropic, OpenAI, google, Groq, or a local Ollama instance |

> If you are using the one-line installer (prebuilt binary path below), Rust is
> not required unless no prebuilt binary exists for your platform, in which case
> the installer falls back to `cargo install` automatically.

---

## Installation

### Option 1 — One-line installer (recommended)

Downloads a prebuilt binary for your OS and architecture, scaffolds
`~/.ironhermes/`, copies config templates, and adds the binary to
`~/.local/bin`. Falls back to `cargo install` if no prebuilt is available.

```bash
curl -fsSL https://raw.githubusercontent.com/bradwilson331/iron-hermes/main/install.sh | bash
```

Restart your shell (or run `source ~/.bashrc` / `source ~/.zshrc`) so that
`~/.local/bin` is on `PATH`.

### Option 2 — Build from source

```bash
git clone https://github.com/bradwilson331/iron-hermes
cd iron-hermes
cargo build --release
# Binary lands at target/release/ironhermes
# Add it to PATH or invoke it with the full path
```

---

## First Run

### 1. Set your API key

The recommended approach is the interactive setup wizard:

```bash
ironhermes setup
```

The wizard asks whether you want a quick setup (provider + model only) or a full
setup (all sections). It writes both `~/.ironhermes/config.yaml` and
`~/.ironhermes/.env` for you.

> **Auto-launch on first run.** If you invoke `ironhermes` (or `ironhermes chat`)
> and no runnable LLM is configured — meaning none of `OPENROUTER_API_KEY`,
> `ANTHROPIC_API_KEY`, or `OPENAI_API_KEY` are set and no local Ollama URL is
> configured — the setup wizard launches automatically.

**Manual alternative:** If you prefer to configure by hand, IronHermes reads
credentials from `~/.ironhermes/.env`. The installer creates this file for you
(from the template); edit it to add your key:

```bash
# Using OpenRouter (the default provider)
echo 'OPENROUTER_API_KEY=sk-or-your-key-here' >> ~/.ironhermes/.env

# Or Anthropic direct
echo 'ANTHROPIC_API_KEY=sk-ant-your-key-here' >> ~/.ironhermes/.env
```

The file is created with mode `600` by the installer (API keys are not
world-readable).

> **Optional: encrypted vault storage.** Instead of a plaintext `.env`, keys
> can be stored in an encrypted, operator-managed vault (off by default,
> requires building with `--features rusty-vault`). See
> [CONFIGURATION.md — Vault](CONFIGURATION.md#vault-vault).

### 2. Verify your setup

```bash
ironhermes doctor
```

This checks that required environment variables are present, the config file
parses cleanly, and all configured providers are reachable.

> **Note:** If you completed the setup wizard, `ironhermes doctor` runs automatically
> before the wizard's completion summary. You do not need to run it again
> separately.

### 3. Start the agent

```bash
ironhermes
```

This opens an interactive REPL. Type a prompt and press Enter. The agent
streams a response, calling tools as needed.

For a one-shot prompt that exits when done:

```bash
ironhermes -e "Summarize the files changed in the last git commit"
```

---

## Common Setup Issues

### "No API key found" / setup wizard relaunches on every start

IronHermes requires at least one provider entry in `~/.ironhermes/config.yaml`
that references an env var via `api_key_env`. The minimal working config is
already present in the installed `config.yaml` template (the `openrouter`
block). Make sure the matching key (`OPENROUTER_API_KEY`) is set in
`~/.ironhermes/.env`.

If your key is in `.env` but the wizard still relaunches, the `api_key_env`
entry may be missing from `config.yaml`. Run `ironhermes setup model` to trigger
the wizard's backfill: it detects env vars present in `.env` and silently
writes the missing `providers.<provider>.api_key_env` entry into `config.yaml`.

### Binary not found after install

The installer adds `~/.local/bin` to your shell rc files. If `ironhermes` is
not found, reload your shell:

```bash
source ~/.bashrc   # bash
source ~/.zshrc    # zsh
```

Or invoke the binary directly: `~/.local/bin/ironhermes`.

### Port conflict (web UI)

The Dioxus web UI (`iron_hermes_ui`) binds to `127.0.0.1:8080` by default.
Override it with the `IP` / `PORT` environment variables (or `DIOXUS_ADDRESS`)
before running the standalone binary:

```bash
PORT=9090 ./target/dx/iron_hermes_ui/debug/web/iron_hermes_ui
```

### Wrong Rust edition / build fails

IronHermes uses the **2024 edition** of Rust. If `cargo build` fails with an
edition error, update your toolchain:

```bash
rustup update stable
```

---

## Talk to the agent (voice mode)

IronHermes supports two voice paths: turn-based (CLI/TUI + web) and realtime open-mic (web only, Phase 39.3).

### Turn-based voice (CLI/TUI + web)

Set a cloud STT key — `GROQ_API_KEY` (preferred) or `VOICE_TOOLS_OPENAI_KEY` — in `~/.ironhermes/.env`, then in the TUI:

- `/voice status` — show the active mode, provider, and record key
- `/voice on` — **Voice-Only mode.** Press `Ctrl+B` to record; speak, then either pause (silence ends it automatically after `silence_duration`) or press `Ctrl+B` again to send what you just said. The transcript is submitted as a normal turn, and **the agent speaks its reply back** — but only for voice turns. If you type a message, the reply stays text-only.
- `/voice tts` — **All mode.** The agent speaks its reply to *every* message, whether you typed it or spoke it.
- `/voice off` — text-only for all interactions.

Spoken replies use the TTS provider from `tts.provider` (default **Edge**, no API key needed; falls back to Edge if a configured provider is unavailable). Code blocks, markdown, and over-long replies are stripped/trimmed so the agent reads back clean prose rather than narrating code.

**Wake-word activation** (`voice.wake_word.enabled: true`, phrase default `"hey hermes"`) applies to turn-based mode only — it has no effect in open-mic mode.

### Realtime open-mic voice (web, Phase 39.3)

The web UI mic button also supports `voice.barge_in_mode: open_mic` — a full-duplex realtime voice session over the OpenAI Realtime API and WebRTC. Requires `OPENAI_API_KEY` (or the env var named in `providers.openai.api_key_env`) in `~/.ironhermes/.env`. When the key is absent or unreachable, the mic button gracefully falls back to turn-based voice.

**Open-mic is a first-class Hermes agent surface (Phase 39.3):** it runs with the full Hermes identity and skills, exposes all registered tools, and routes tool calls through the same approval gate as text chat. An Approve/Deny card appears in the orb overlay for gated tool calls; background turns show an in-flight "working…" badge and voice the result on completion. Transcripts and trajectory entries are written at the same fidelity as text turns.

> **Wake-word limitation:** `voice.wake_word` applies to turn-based mode only. In `open_mic` mode the wake-word control is greyed out in the UI with an explanatory hint.

See [CONFIGURATION.md — Voice Mode](CONFIGURATION.md#voice-mode-voice) for all tunables (barge-in mode, wake word, realtime model/voice, VAD, noise reduction, etc.).

## Next Steps

- [DEVELOPMENT.md](DEVELOPMENT.md) — local dev workflow, build commands, and code style
- [CONFIGURATION.md](CONFIGURATION.md) — full reference for `config.yaml` and all environment variables
- [ARCHITECTURE.md](ARCHITECTURE.md) — crate layout and system design
