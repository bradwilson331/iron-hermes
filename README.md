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

## License

MIT
