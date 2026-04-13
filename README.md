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

### Compression Tuning

Context compression behavior is configurable via `config.yaml`:

```yaml
compression:
  protect_last_tokens: 20000   # last N tokens are never pruned (default 20000)
  protect_first_n: 3           # first N messages are never pruned (default 3)
  tool_pair_shift_tokens: 500  # adaptive shift for tool-pair atomicity (default 500)
```

For UAT or testing, lower `protect_last_tokens` (e.g. to 100) to force compression to prune short conversations.

## Tools

Built-in tools available to the agent:

- **terminal** — Execute shell commands with timeout
- **read_file** — Read file contents with line numbers
- **write_file** — Write/create files
- **patch** — Find-and-replace in files
- **search_files** — Regex search across files
- **web_search** — Web search via Firecrawl API

## License

MIT
