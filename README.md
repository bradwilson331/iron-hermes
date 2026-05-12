# IronHermes

The self-improving AI agent, rewritten in Rust. A port of [hermes-agent](https://github.com/NousResearch/hermes-agent) by Nous Research.

## Architecture

IronHermes is organized as a Cargo workspace with modular crates:

| Crate | Description |
|-------|-------------|
| `ironhermes-core` | Shared types, config, constants, error handling |
| `ironhermes-state` | SQLite state store with FTS5 search (session persistence) |
| `ironhermes-tools` | Tool registry + implementations (terminal, file ops, web search) |
| `ironhermes-agent` | LLM client, agent loop, prompt builder, context compression |
| `ironhermes-cli` | Interactive CLI binary |
| `ironhermes-gateway` | Multi-platform messaging gateway (Telegram adapter) |
| `ironhermes-cron` | Cron job scheduler |
| `iron_hermes_ui` | Dioxus 0.7 web UI — terminal-style chat shell with streaming LLM responses |

## Quick Start

```bash
# Build
cargo build --release

# Run interactive chat
cargo run --bin ironhermes

# Run a single prompt
cargo run --bin ironhermes -- -e "What is the capital of France?"

# Show status
cargo run --bin ironhermes -- status

# Check configuration
cargo run --bin ironhermes -- doctor
```

## Configuration

IronHermes looks for configuration in `~/.ironhermes/`:

- `config.yaml` — Model, provider, terminal, web, and gateway settings
- `.env` — API keys (OPENROUTER_API_KEY, ANTHROPIC_API_KEY, etc.)

Set `IRONHERMES_HOME` to override the default home directory.

### Minimal Setup

```bash
mkdir -p ~/.ironhermes
echo "OPENROUTER_API_KEY=your-key-here" > ~/.ironhermes/.env
```

### Provider fallback

Set `fallback_providers` on a provider to swap to another provider on hard
failure (HTTP 401, 403, 404, 429, or 5xx). The swap is one-shot per session
and the fallback uses the **fallback provider's own `default_model`** — the
primary's model string is not carried over, so a local model server doesn't
need to know the primary's model name.

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

#### Quick setup via CLI

```bash
hermes config set providers.openrouter.fallback_providers[0] local-ollama
```

#### Cloud-primary + local Ollama fallback (canonical example)

Use OpenRouter as the primary and a local Ollama server as the fallback. If OpenRouter
returns 429 (rate limit), 5xx (server error), or 401 (auth failure), IronHermes
retries the request using the fallback provider. The fallback request uses the
**fallback provider's own `default_model`** — not the primary's model name — so the
local Ollama server only needs to have `gemma4:latest` loaded.

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
    api_key_env: OPENROUTER_API_KEY    # secret lives in ~/.ironhermes/.env
    fallback_providers:
      - local-ollama
```

The `fallback_providers` list is tried in order. The swap is one-shot per session;
if the fallback also fails the error is returned to the caller.

## Tools

Built-in tools available to the agent:

- **terminal** — Execute shell commands with timeout
- **read_file** — Read file contents with line numbers
- **write_file** — Write/create files
- **patch** — Find-and-replace in files
- **search_files** — Regex search across files
- **web_search** — Web search via Firecrawl API

## Web UI (`iron_hermes_ui`)

A Dioxus 0.7 fullstack web app — a terminal-style chat shell that streams LLM responses
directly from an embedded agent server. Requires the Dioxus CLI (`dx`).

### Prerequisites

```bash
# Install the Dioxus CLI
cargo install dioxus-cli

# Ensure your API key is in ~/.ironhermes/.env
echo "OPENROUTER_API_KEY=your-key-here" >> ~/.ironhermes/.env
```

### Build and run (standalone binary)

```bash
# Build the web bundle (run from workspace root)
dx bundle --platform web -p iron_hermes_ui

# Run the server
RUST_LOG=info ./target/dx/iron_hermes_ui/debug/web/iron_hermes_ui
```

The server starts on `http://localhost:8080` by default. Set `IP` / `PORT` environment
variables or use `DIOXUS_ADDRESS` to change the bind address.

### Development mode (hot reload)

```bash
# Run from workspace root — proxies through dx serve
dx serve --package iron_hermes_ui
```

> **Note:** `dx serve` routes WebSocket traffic through a proxy that may impose a short
> idle-close timeout (~9 seconds). The standalone binary above does not have this
> limitation and is recommended for normal use.

### Release build

```bash
dx bundle --platform web -p iron_hermes_ui --release
./target/dx/iron_hermes_ui/release/web/iron_hermes_ui
```

## Gateway deployment (launchd / systemd / cron)

Run the Telegram gateway as a long-lived background service. Templates and an
OS-detecting installer live in `scripts/deploy/`:

```
scripts/deploy/
├── install.sh                    OS-detecting installer
├── uninstall.sh                  symmetric removal
├── gateway-run.sh                shared launcher (loads ~/.ironhermes/.env, exec's the binary)
├── gateway-watchdog.sh           cron fallback — relaunches if the pid is dead
├── com.ironhermes.gateway.plist  macOS LaunchAgent template
└── ironhermes-gateway.service    Linux systemd --user unit
```

Preflight: build the release binary (`cargo build --release`) and put a
`TELEGRAM_BOT_TOKEN` in `~/.ironhermes/.env`. The installer runs
`ironhermes doctor` before doing anything.

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

The plist uses `KeepAlive={Crashed:true, SuccessfulExit:false}` (restart on
crash, not after a clean exit) and `ThrottleInterval=30` to cap restart storms.

### Linux (systemd --user)

```bash
scripts/deploy/install.sh
# verify
systemctl --user status ironhermes-gateway
journalctl --user -u ironhermes-gateway -f
# headless servers (no graphical login) — run once:
loginctl enable-linger $USER
```

### Cron watchdog (fallback)

For hosts without launchd/systemd, a 1-minute cron entry runs
`gateway-watchdog.sh`, which parses `~/.ironhermes/gateway.pid` and relaunches
the gateway via `gateway-run.sh` if the process is dead:

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

Set `IRONHERMES_PROFILE` in the environment (or in the systemd unit /
`EnvironmentVariables` block of the plist) and `gateway-run.sh` will pass
`--profile $IRONHERMES_PROFILE` to the binary.

## License

MIT
