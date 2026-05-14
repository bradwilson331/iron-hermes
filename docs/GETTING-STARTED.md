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
curl -fsSL https://raw.githubusercontent.com/bradwilson331/ironhermes/main/install.sh | bash
```

Restart your shell (or run `source ~/.bashrc` / `source ~/.zshrc`) so that
`~/.local/bin` is on `PATH`.

### Option 2 — Build from source

```bash
git clone https://github.com/bradwilson331/ironhermes
cd ironhermes
cargo build --release
# Binary lands at target/release/ironhermes
# Add it to PATH or invoke it with the full path
```

---

## First Run

### 1. Set your API key

IronHermes reads credentials from `~/.ironhermes/.env`. The installer creates
this file for you (from the template); edit it to add your key:

```bash
# Using OpenRouter (the default provider)
echo 'OPENROUTER_API_KEY=sk-or-your-key-here' >> ~/.ironhermes/.env

# Or Anthropic direct
echo 'ANTHROPIC_API_KEY=sk-ant-your-key-here' >> ~/.ironhermes/.env
```

The file is created with mode `600` by the installer (API keys are not
world-readable).

### 2. Verify your setup

```bash
ironhermes doctor
```

This checks that required environment variables are present, the config file
parses cleanly, and all configured providers are reachable.

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

### Binary not found after install

The installer adds `~/.local/bin` to your shell rc files. If `ironhermes` is
not found, reload your shell:

```bash
source ~/.bashrc   # bash
source ~/.zshrc    # zsh
```

Or invoke the binary directly: `~/.local/bin/ironhermes`.

### Port conflict (web UI)

The Dioxus web UI (`iron_hermes_ui`) listens on a port that can be overridden
via the `PORT` environment variable before running the server: <!-- VERIFY: confirm the env var name and default port used by iron_hermes_ui -->

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

## Next Steps

- [DEVELOPMENT.md](DEVELOPMENT.md) — local dev workflow, build commands, and code style
- [CONFIGURATION.md](CONFIGURATION.md) — full reference for `config.yaml` and all environment variables
- [ARCHITECTURE.md](ARCHITECTURE.md) — crate layout and system design
