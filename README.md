<!-- generated-by: gsd-doc-writer -->
# IronHermes

The self-improving AI agent, rewritten in Rust. A port of [hermes-agent](https://github.com/NousResearch/hermes-agent) by Nous Research — built for developers who live in the terminal.

---

## What it is

IronHermes is a terminal-native AI agent runtime. Users interact with an AI (Hermes) through a streaming shell where commands, responses, tool calls, and errors surface as typed blocks. Multiple sessions, personality presets, and a keyboard-driven command palette make it a configurable, expert-level tool.

The system accepts prompts through three entry points: an interactive CLI REPL, a Telegram gateway, and a Dioxus 0.7 web UI with an embedded agent server. At its core is a Cargo workspace of 13 focused crates organized in a layered architecture — shared types at the base, an agentic loop in the middle, and multiple frontends at the top.

---

## Installation

### Option 1 — one-line installer (recommended)

Downloads a prebuilt binary, scaffolds `~/.ironhermes/`, copies config templates, and adds the binary to `~/.local/bin`. Falls back to `cargo install` if no prebuilt binary is available for your platform.

```bash
curl -fsSL https://raw.githubusercontent.com/bradwilson331/ironhermes/main/install.sh | bash
```

Reload your shell after install:

```bash
source ~/.bashrc   # bash
source ~/.zshrc    # zsh
```

### Option 2 — build from source

Requires a stable Rust toolchain (2024 edition). Install via [rustup.rs](https://rustup.rs) if needed.

```bash
git clone https://github.com/bradwilson331/ironhermes
cd ironhermes
cargo build --release
# Binary: target/release/ironhermes
```

---

## Quick Start

**1. Set your API key**

```bash
mkdir -p ~/.ironhermes
echo "OPENROUTER_API_KEY=your-key-here" > ~/.ironhermes/.env
```

OpenRouter is the default provider. Anthropic, OpenAI, Gemini, Groq, and local Ollama are also supported — see [Configuration](#configuration).

**2. Verify setup**

```bash
ironhermes doctor
```

Checks that environment variables are present, the config parses cleanly, and configured providers are reachable.

**3. Start the agent**

```bash
ironhermes
```

Opens an interactive REPL. Type a prompt and press Enter. The agent streams a response, calling tools as needed.

**One-shot mode:**

```bash
ironhermes -e "Summarize the files changed in the last git commit"
```

---

## Usage

**Interactive chat session:**

```bash
ironhermes
```

Starts a REPL where you can send prompts and the agent responds with tool calls and streaming output.

**One-shot prompt:**

```bash
ironhermes -e "Summarize the files changed in the last git commit"
```

The agent runs the prompt to completion and exits.

**Diagnose configuration:**

```bash
ironhermes doctor
```

Checks that required environment variables are set, the config file is valid, and all configured providers are reachable.

**Check agent status:**

```bash
ironhermes status
```

Prints the active provider, model, and session store path.

---

## Configuration

IronHermes looks for configuration in `~/.ironhermes/`:

- `config.yaml` — model, provider, terminal, web, and gateway settings
- `.env` — API keys (`OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`, etc.)

Set `IRONHERMES_HOME` to override the default home directory.

### Provider fallback

Set `fallback_providers` on a provider to swap to another provider on hard failure (HTTP 401, 403, 404, 429, or 5xx). The swap is one-shot per session and the fallback uses the **fallback provider's own `default_model`**.

```yaml
providers:
  openrouter:
    api_key_env: OPENROUTER_API_KEY
    fallback_providers: ["local-llama"]

custom_providers:
  - name: local-llama
    base_url: http://localhost:11434/v1
    api_key: ollama
    api_mode: chat_completions
    default_model: llama3.2:latest
```

#### Cloud-primary + local Ollama fallback (canonical example)

```yaml
custom_providers:
  local-ollama:
    base_url: "http://localhost:11434/v1"
    api_key_env: ""          # Ollama requires no API key
    default_model: "gemma4:latest"
    api_mode: chat_completions

model:
  default: "anthropic/claude-sonnet-4"

providers:
  openrouter:
    api_key_env: OPENROUTER_API_KEY
    fallback_providers:
      - local-ollama
```

The `fallback_providers` list is tried in order. The swap is one-shot per session; if the fallback also fails the error is returned to the caller.

See [docs/CONFIGURATION.md](docs/CONFIGURATION.md) for the full reference and [docs/providers.md](docs/providers.md) for per-provider setup.

---

## Tools

Built-in tools available to the agent:

- **terminal** — Execute shell commands with timeout
- **read_file** — Read file contents with line numbers
- **write_file** — Write/create files
- **patch** — Find-and-replace in files
- **search_files** — Regex search across files
- **web_search** — Web search via Firecrawl API

---

## Architecture

IronHermes is organized as a Cargo workspace with modular crates:

| Crate | Description |
|-------|-------------|
| `ironhermes-core` | Shared types, config, constants, provider abstraction, skill registry, SSRF guard |
| `ironhermes-state` | SQLite state store with FTS5 search (session persistence) |
| `ironhermes-trajectory` | Append-only JSONL tool-call ledger |
| `ironhermes-tools` | Tool registry + implementations (terminal, file ops, web, browser, memory, MCP bridge) |
| `ironhermes-agent` | LLM client, agent loop, prompt builder, context compression, subagent runner |
| `ironhermes-cli` | Interactive CLI binary + ratatui REPL |
| `ironhermes-gateway` | Multi-platform messaging gateway (Telegram adapter) |
| `ironhermes-cron` | Cron job scheduler |
| `ironhermes-hooks` | Event hook registry, webhook delivery, guardrails, hot-reload config |
| `ironhermes-exec` | Python sandbox via Unix socket RPC |
| `ironhermes-hub` | Skills Hub: install / update / uninstall from GitHub / skills.sh |
| `ironhermes-mcp` | MCP client: stdio + HTTP transports, per-server task, sampling handler |
| `iron_hermes_ui` | Dioxus 0.7 fullstack web/desktop UI with embedded agent server |

Memory providers (pluggable): `providers/memory-sqlite`, `providers/memory-grafeo`, `providers/memory-duckdb`.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/crates.md](docs/crates.md) for full detail.

---

## Web UI (`iron_hermes_ui`)

A Dioxus 0.7 fullstack web app — a terminal-style chat shell that streams LLM responses directly from an embedded agent server. Requires the Dioxus CLI (`dx`).

### Prerequisites

```bash
cargo install dioxus-cli
echo "OPENROUTER_API_KEY=your-key-here" >> ~/.ironhermes/.env
```

### Build and run (standalone binary)

```bash
dx bundle --platform web -p iron_hermes_ui
RUST_LOG=info ./target/dx/iron_hermes_ui/debug/web/iron_hermes_ui
```

The server starts on `http://localhost:8080` by default. Set `IP` / `PORT` environment variables or use `DIOXUS_ADDRESS` to change the bind address.

### Development mode (hot reload)

```bash
dx serve --package iron_hermes_ui
```

> **Note:** `dx serve` routes WebSocket traffic through a proxy that may impose a short idle-close timeout (~9 seconds). The standalone binary above does not have this limitation and is recommended for normal use.

### Release build

```bash
dx bundle --platform web -p iron_hermes_ui --release
./target/dx/iron_hermes_ui/release/web/iron_hermes_ui
```

---

## Gateway Deployment (launchd / systemd / cron)

Run the Telegram gateway as a long-lived background service. Templates and an OS-detecting installer live in `scripts/deploy/`:

```
scripts/deploy/
├── install.sh                    OS-detecting installer
├── uninstall.sh                  symmetric removal
├── gateway-run.sh                shared launcher (loads ~/.ironhermes/.env, exec's the binary)
├── gateway-watchdog.sh           cron fallback — relaunches if the pid is dead
├── com.ironhermes.gateway.plist  macOS LaunchAgent template
└── ironhermes-gateway.service    Linux systemd --user unit
```

Preflight: build the release binary (`cargo build --release`) and set `TELEGRAM_BOT_TOKEN` in `~/.ironhermes/.env`. The installer runs `ironhermes doctor` before doing anything.

### macOS (launchd)

```bash
scripts/deploy/install.sh
# verify
launchctl print gui/$UID/com.ironhermes.gateway | grep -E 'state|pid'
tail -f ~/.ironhermes/logs/gateway.out.log
# restart / stop
launchctl kickstart -k gui/$UID/com.ironhermes.gateway
launchctl bootout    gui/$UID/com.ironhermes.gateway
```

### Linux (systemd --user)

```bash
scripts/deploy/install.sh
systemctl --user status ironhermes-gateway
journalctl --user -u ironhermes-gateway -f
# headless servers (no graphical login) — run once:
loginctl enable-linger $USER
```

### Cron watchdog (fallback)

For hosts without launchd/systemd:

```bash
scripts/deploy/install.sh --cron
crontab -l | grep ironhermes
tail -f ~/.ironhermes/logs/gateway.log
```

### Uninstall

```bash
scripts/deploy/uninstall.sh         # native service for this OS
scripts/deploy/uninstall.sh --cron  # remove watchdog crontab entry
scripts/deploy/uninstall.sh --all   # both, plus staged scripts in ~/.ironhermes/scripts/
```

Logs in `~/.ironhermes/logs/` are kept on uninstall.

### Profiles

Set `IRONHERMES_PROFILE` in the environment (or in the systemd unit / `EnvironmentVariables` block of the plist) and `gateway-run.sh` will pass `--profile $IRONHERMES_PROFILE` to the binary.

---

## Documentation

| Doc | Description |
|---|---|
| [docs/GETTING-STARTED.md](docs/GETTING-STARTED.md) | Prerequisites, install options, first run |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Crate layout, component diagram, data flow |
| [docs/CONFIGURATION.md](docs/CONFIGURATION.md) | `config.yaml` and environment variable reference |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | Local dev workflow, build commands, code style |
| [docs/TESTING.md](docs/TESTING.md) | Test framework, running tests, CI integration |
| [docs/crates.md](docs/crates.md) | Per-crate public API reference |
| [docs/providers.md](docs/providers.md) | LLM provider configuration |
| [docs/skills-system.md](docs/skills-system.md) | Skills architecture |
| [docs/skills-cli.md](docs/skills-cli.md) | Skills CLI reference |
| [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) | Docker and deployment options |
| [SECURITY.md](SECURITY.md) | Security model and DEFCON scale |
| [troubleshooting_guide.md](troubleshooting_guide.md) | Common issues and fixes |

---

## License

MIT — Authors: Nous Research.
