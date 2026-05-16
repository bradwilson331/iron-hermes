<!-- generated-by: gsd-doc-writer -->
# Configuration

IronHermes uses two configuration files, both stored under its home directory (`~/.ironhermes/` by default):

- **`~/.ironhermes/config.yaml`** — primary YAML configuration for all agent behavior, providers, tools, and subsystems.
- **`~/.ironhermes/.env`** — environment variable overrides, primarily for API secrets.

The home directory location can be overridden with the `IRONHERMES_HOME` environment variable. When using named profiles (`hermes --profile <name>`), the home directory is automatically pivoted to `~/.ironhermes/profiles/<name>/`.

Copy the bundled examples to get started:

```bash
cp cli-config.yaml.example ~/.ironhermes/config.yaml
cp env.example ~/.ironhermes/.env
```

---

## Environment Variables

Environment variables live in `~/.ironhermes/.env` (or the `IRONHERMES_HOME`-scoped `.env`). Values set in `.env` override matching YAML config values.

### LLM Provider API Keys

| Variable | Required | Description |
|---|---|---|
| `OPENROUTER_API_KEY` | Required (if using OpenRouter) | API key for OpenRouter (default provider). Format: `sk-or-...` |
| `ANTHROPIC_API_KEY` | Required (if using Anthropic direct) | API key for Anthropic direct API. Format: `sk-ant-...` |
| `OPENAI_API_KEY` | Required (if using OpenAI) | API key for OpenAI. Format: `sk-...` |
| `GOOGLE_API_KEY` | Optional | API key for Google AI / Gemini |
| `GEMINI_API_KEY` | Optional | Alternative env var for Gemini |
| `GROQ_API_KEY` | Optional | API key for Groq. Format: `gsk_...` |
| `TOGETHER_API_KEY` | Optional | API key for Together AI |
| `MISTRAL_API_KEY` | Optional | API key for Mistral |
| `PERPLEXITY_API_KEY` | Optional | API key for Perplexity |
| `DEEPSEEK_API_KEY` | Optional | API key for DeepSeek |
| `FIREWORKS_API_KEY` | Optional | API key for Fireworks AI |
| `OLLAMA_BASE_URL` | Optional | Base URL for Ollama server. Default: `http://localhost:11434` |
| `OLLAMA_API_KEY` | Optional | API key for Ollama (if required by your server) |

### Tool API Keys

| Variable | Required | Description |
|---|---|---|
| `FIRECRAWL_API_KEY` | Optional | API key for Firecrawl web scraping backend. Format: `fc-...` |
| `EXA_API_KEY` | Optional | API key for Exa search |

### Gateway / Messaging

| Variable | Required | Description |
|---|---|---|
| `TELEGRAM_BOT_TOKEN` | Required (if using Telegram gateway) | Telegram bot token |
| `TELEGRAM_ALLOWED_USERS` | Optional | Comma-separated Telegram chat IDs to allow |
| `TELEGRAM_HOME_CHANNEL` | Optional | Home channel chat ID for the Telegram gateway |
| `DISCORD_BOT_TOKEN` | Optional | Discord bot token (future) |
| `DISCORD_ALLOWED_USERS` | Optional | Comma-separated Discord user IDs to allow (future) |
| `SLACK_BOT_TOKEN` | Optional | Slack bot token (future) |
| `SLACK_APP_TOKEN` | Optional | Slack app-level token (future) |

### Terminal / Sandbox

| Variable | Required | Default | Description |
|---|---|---|---|
| `TERMINAL_BACKEND` | Optional | `local` | Sandbox backend: `local` or `docker` |
| `TERMINAL_CWD` | Optional | `.` | Default working directory for agent tool operations |
| `TERMINAL_TIMEOUT` | Optional | `30` | Command execution timeout in seconds |
| `TERMINAL_DOCKER_IMAGE` | Optional | — | Docker sandbox image (when `TERMINAL_BACKEND=docker`) |
| `TERMINAL_ENV` | Optional | — | Comma-separated env var names to pass through to sandbox |

### Code Execution

| Variable | Required | Default | Description |
|---|---|---|---|
| `EXEC_PYTHON_PATH` | Optional | `python3` | Path to Python interpreter |
| `EXEC_TIMEOUT_SECS` | Optional | `300` | Execution timeout in seconds |

### Cron Job Execution

| Variable | Required | Default | Description |
|---|---|---|---|
| `IRONHERMES_CRON_TIMEOUT` | Optional | `600` | Inactivity timeout in seconds. The cron runner polls the agent every 5 s; if no API call, tool call, or stream token has been produced for this many seconds the job is interrupted. `0` = unlimited. |
| `IRONHERMES_CRON_WALL_TIMEOUT_SECS` | Optional | `14400` | Hard wall-clock ceiling in seconds (4 h). Kills a runaway job even if it keeps producing activity. `0` = unlimited. |
| `IRONHERMES_CRON_SCRIPT_TIMEOUT` | Optional | `120` | Per-script execution timeout in seconds for jobs that use the `script` field. |
| `IRONHERMES_CRON_MAX_PARALLEL` | Optional | `0` | Maximum number of non-workdir cron jobs to run concurrently per tick. `0` = unbounded (Python-equivalent serial behavior requires `1`). |

### IronHermes Home

| Variable | Required | Default | Description |
|---|---|---|---|
| `IRONHERMES_HOME` | Optional | `~/.ironhermes` | Override the default data and config directory |

### Debug Flags

| Variable | Required | Default | Description |
|---|---|---|---|
| `RUST_LOG` | Optional | — | Rust log filter (e.g., `ironhermes=info`, `ironhermes=debug`) |
| `WEB_TOOLS_DEBUG` | Optional | `false` | Enable verbose web tool logging |
| `VISION_TOOLS_DEBUG` | Optional | `false` | Enable verbose vision tool logging |

---

## Config File Format

`~/.ironhermes/config.yaml` is a YAML file. All keys are optional — omitting a key uses the default shown. Environment variables in `.env` override corresponding YAML values.

The minimal working configuration requires a provider entry with an `api_key_env` pointing to a set environment variable:

```yaml
model:
  default: "anthropic/claude-sonnet-4"
  provider: "openrouter"

providers:
  openrouter:
    api_key_env: OPENROUTER_API_KEY
```

### Top-Level Sections

| Section | Description |
|---|---|
| `model` | Default model, provider, and auxiliary role routing |
| `agent` | Agent loop behavior: max turns, compression, delays |
| `terminal` | Shell sandbox backend and working directory |
| `web` | Web scraping backend and request settings |
| `exec` | Python code execution sandbox |
| `gateway` | Messaging platform adapters (Telegram, etc.) |
| `cron` | Scheduled job settings |
| `memory` | Memory provider selection |
| `compression` | Context compression tuning |
| `skills` | Skills subsystem enable/disable and scan paths |
| `subagent` | Subagent delegation limits |
| `rate_limit` | Per-user inbound rate limiting |
| `batch` | Batch processing worker settings |
| `security` | Secret redaction in logs |
| `providers` | Per-provider API key and endpoint overrides |
| `custom_providers` | User-defined OpenAI-compatible endpoints |
| `tools` | Per-toolset enable/disable |
| `auxiliary` | Auxiliary model routing for helper tasks |
| `browser` | Browser automation settings |
| `extract` | Web extraction (web_extract tool) tuning |
| `autonomous` | Autonomous (yolo) mode |
| `mcp_servers` | MCP server configurations (raw YAML, parsed by ironhermes-mcp) |

---

## Required vs Optional Settings

The following settings cause startup validation to fail and re-launch the setup wizard (`hermes setup model`) if absent or empty:

| Setting | Validation Rule |
|---|---|
| `providers.<main-provider>.api_key_env` | Required — must reference a non-empty env var name matching `[A-Z][A-Z0-9_]*` |
| `model.default` | Required — must be a non-empty model identifier string |
| `model.provider` | Required — must be a non-empty provider name (e.g., `openrouter`, `anthropic`) |
| `memory.provider` | Required (when `memory.memory_enabled: true`) — must be one of: `file`, `sqlite`, `grafeo`, `duckdb` |

All other settings are optional and fall back to the defaults listed below.

---

## Defaults

All default values are sourced from the Rust structs in `crates/ironhermes-core/src/config.rs`.

### Model (`model:`)

| Key | Default | Description |
|---|---|---|
| `model.default` | `anthropic/claude-sonnet-4` | Default model identifier |
| `model.provider` | `openrouter` | LLM provider |
| `model.base_url` | `null` | Override API base URL |
| `model.vision_model` | `null` | Vision model (null = use default) |
| `model.max_tokens` | `null` | Max tokens per response (null = provider default) |
| `model.context_length` | `null` | Context window (null = auto-detect) |

### Agent (`agent:`)

| Key | Default | Description |
|---|---|---|
| `agent.max_turns` | `90` | Maximum agent loop iterations per turn |
| `agent.context_compression` | `0.5` | Context compression ratio |
| `agent.tool_delay_secs` | `1.0` | Delay between tool calls in seconds |
| `agent.context_engine` | `summarizing` | Context engine: `summarizing` or `local_prune` |
| `agent.compression_threshold` | `0.5` | Fraction of context_length at which compression triggers |
| `agent.max_iterations` | `50` | Maximum agent budget iterations |
| `agent.system_message` | `""` | Optional injected system message (empty = omitted) |

### Terminal (`terminal:`)

| Key | Default | Description |
|---|---|---|
| `terminal.backend` | `local` | Sandbox backend: `local` or `docker` |
| `terminal.cwd` | `.` | Default working directory for tool operations |
| `terminal.timeout` | `30` | Command execution timeout in seconds |

### Web (`web:`)

| Key | Default | Description |
|---|---|---|
| `web.backend` | `firecrawl` | Web scraping backend: `firecrawl` or `raw` |
| `web.user_agent` | `IronHermes/1.0 (+bot)` | User-Agent header for HTTP requests |
| `web.max_content_chars` | `50000` | Maximum content length before truncation |
| `web.timeout_secs` | `30` | HTTP request timeout in seconds |

### Code Execution (`exec:`)

| Key | Default | Description |
|---|---|---|
| `exec.python_path` | `python3` | Path to Python interpreter |
| `exec.timeout_secs` | `300` | Execution timeout in seconds (5 minutes) |
| `exec.max_rpc_calls` | `50` | Maximum RPC calls per execution |
| `exec.max_output_bytes` | `50000` | Maximum stdout bytes before truncation |
| `exec.max_stderr_bytes` | `10240` | Maximum stderr bytes before truncation |

### Memory (`memory:`)

| Key | Default | Description |
|---|---|---|
| `memory.provider` | `file` | Provider: `file`, `sqlite`, `grafeo`, or `duckdb` |
| `memory.memory_enabled` | `true` | Enable/disable the memory subsystem entirely |
| `memory.user_profile_enabled` | `true` | Enable/disable the USER.md profile store |
| `memory.mirror_provider` | `null` | Optional write-only mirror provider |
| `memory.nudge_interval` | `10` | Turns between periodic memory-review nudges. `0` disables the nudge entirely. See [Periodic Memory Review Nudge](#periodic-memory-review-nudge) below. |

#### Periodic Memory Review Nudge

After every `memory.nudge_interval` successful agent turns, IronHermes fires a
**fire-and-forget** background nudge that asks the model to review the recent
conversation and decide what (if anything) is worth persisting to long-term
memory. The nudge runs in all three agent surfaces:

| Surface | File | Fire site |
|---|---|---|
| CLI REPL (`hermes chat`) | `crates/ironhermes-cli/src/main.rs` | `run_chat` post-turn (line ~2138) |
| Telegram gateway | `crates/ironhermes-gateway/src/handler.rs` | `handle_with_multimodal` post-`agent.run()` (line ~1067) |
| Embedded web UI | `crates/iron_hermes_ui/src/server/state.rs` | `run_web_turn` post-`agent.run()` (line ~171) |

**Two-tier judgment (LEARN-02).** The nudge prompt (`MEMORY_REVIEW_PROMPT`
in `crates/ironhermes-agent/src/nudge.rs`) asks the model to decide per-item
between two persistence layers:

- **Important enough to be present in every future conversation** → use the
  memory tool (persists to `MEMORY.md` / `USER.md`).
- **Useful only when topic comes up** → leave in session history (searchable
  via `session_search` later). The nudge will NOT push these into prompt
  memory.

The combined cap is **3,575 chars** (`MEMORY.md` 2,200 + `USER.md` 1,375),
so the prompt explicitly steers the model to be selective. If nothing is
worth saving, the model returns `"Nothing to save."` and the nudge exits.

**Tool isolation.** The nudge runs in a private `ToolRegistry` containing
**only** the `MemoryTool` — `session_search`, `web_read`, `execute_code`,
browser_*, and skill tools are deliberately excluded so the periodic nudge
cannot run expensive search / fetch operations on a turn-counter cadence.

**Configuration examples:**

```yaml
# Default — nudge fires every 10 user turns (recommended starting point).
memory:
  provider: file
  nudge_interval: 10

# Aggressive — nudge after every 3 turns (more memory writes, more API cost).
memory:
  provider: file
  nudge_interval: 3

# Disabled — no periodic nudge at all.
memory:
  provider: file
  nudge_interval: 0

# Disabled by another mechanism — the nudge also short-circuits when the
# entire memory subsystem is off.
memory:
  memory_enabled: false
```

**Set at runtime via the CLI:**

```bash
# Read the current value
hermes config get memory.nudge_interval

# Change interval (writes ~/.ironhermes/cli-config.yaml)
hermes config set memory.nudge_interval 5

# Disable the nudge entirely
hermes config set memory.nudge_interval 0
```

The setup wizard (`hermes setup`) also writes this key on its first run,
alongside the legacy `learning.periodic_nudge_interval_seconds` entry kept
for backward compatibility with older Python-era configs.

**Verifying the feature is live:**

```bash
# 1. Confirm config field is present and parsed
hermes config get memory.nudge_interval

# 2. Run the dedicated unit tests
cargo test -p ironhermes-core --lib config_nudge_interval   # 4 tests, all green
cargo test -p ironhermes-agent --lib nudge::tests           # 6 tests, all green

# 3. Watch the nudge fire in a live CLI session — set a small interval and
# enable tracing at info level. After 3 turns you'll see one of:
#   INFO ironhermes_agent::nudge: memory-review nudge: spawned ...
#   INFO ironhermes_agent::nudge: memory-review nudge: nothing to save
RUST_LOG=ironhermes_agent::nudge=info hermes chat
```

### Compression (`compression:`)

| Key | Default | Description |
|---|---|---|
| `compression.protect_last_tokens` | `20000` | Tokens to protect at end of conversation |
| `compression.tool_pair_shift_tokens` | `500` | Token budget for tool-pair boundary shifting |
| `compression.protect_first_n` | `3` | Number of messages protected at start of conversation |

### Gateway (`gateway:`)

| Key | Default | Description |
|---|---|---|
| `gateway.context_engine` | `local_prune` | Context engine for gateway sessions |
| `gateway.compression_threshold` | `0.85` | Compression threshold for gateway (fraction of context_length) |
| `gateway.platforms` | `{}` | Platform adapters map (empty = no platforms enabled) |

**Telegram platform defaults** (under `gateway.platforms.telegram:`):

| Key | Default | Description |
|---|---|---|
| `session_timeout_hours` | `24` | Session inactivity timeout in hours |
| `max_concurrent_runs` | `8` | Maximum concurrent agent runs |
| `whitelist` | `[]` | Allowed Telegram chat IDs (empty = deny all) |

### Cron (`cron:`)

| Key | Default | Description |
|---|---|---|
| `cron.wrap_response` | `true` | Prepend `Cronjob Response: {name}` header and append management footer to delivered output. Set to `false` to deliver raw agent output. |

### Skills (`skills:`)

| Key | Default | Description |
|---|---|---|
| `skills.enabled` | `true` | Master enable switch for the skills subsystem |
| `skills.extra_paths` | `[]` | Additional skill scan paths (appended after defaults) |
| `skills.credential_dir` | `null` | Root directory for skill credentials (null = `$HERMES_HOME/credentials`) |

Default skill scan paths (in priority order):
1. `<cwd>/.ironhermes/skills/`
2. `~/.ironhermes/skills/` (or `$IRONHERMES_HOME/skills/`)
3. `~/.agents/skills/`

### Subagent (`subagent:`)

| Key | Default | Description |
|---|---|---|
| `subagent.timeout_secs` | `300` | Timeout per subagent execution in seconds |
| `subagent.max_subagents` | `3` | Maximum concurrent subagents |
| `subagent.max_iterations` | `10` | Maximum LLM iterations per subagent |
| `subagent.default_toolsets` | `["terminal", "file", "web"]` | Default toolset groups for child agents |
| `subagent.model` | `null` | Model override for subagents (null = use parent's model) |
| `subagent.provider` | `null` | Provider override for subagents (null = use parent's provider) |
| `subagent.base_url` | `null` | API base URL override for subagents (null = use parent's) |

### Rate Limiting (`rate_limit:`)

| Key | Default | Description |
|---|---|---|
| `rate_limit.messages_per_minute` | `10` | Maximum sustained messages per minute per user |
| `rate_limit.burst_size` | `3` | Maximum burst size |

### Batch Processing (`batch:`)

| Key | Default | Description |
|---|---|---|
| `batch.workers` | `4` | Default worker concurrency |
| `batch.max_turns` | `20` | Default max agent iterations per prompt |
| `batch.output_dir` | `batch_output` | Default output directory (relative to cwd) |

### Security (`security:`)

| Key | Default | Description |
|---|---|---|
| `security.redact_secrets` | `true` | Redact secrets in logs and output |

### Browser (`browser:`)

| Key | Default | Description |
|---|---|---|
| `browser.headed` | `false` | Run with a visible window (true) or headless (false) |
| `browser.no_sandbox` | `false` | Allow `--no-sandbox` flag (required on Docker/restricted envs) |
| `browser.allowed_domains` | `[]` | Domain allowlist for browser_navigate (empty = allow all hosts) |
| `browser.allowed_schemes` | `["http", "https"]` | Scheme allowlist for browser_navigate |
| `browser.chromium_path` | `null` | Explicit chromium binary path (null = autodiscover) |
| `browser.timeout_seconds` | `30` | Per-operation timeout in seconds |
| `browser.user_data_dir` | `null` | Persistent browser profile directory (null = `$HERMES_HOME/browser-profile`) |

### Web Extract (`extract:`)

| Key | Default | Description |
|---|---|---|
| `extract.max_parallel_summaries` | `4` | Semaphore permits for parallel URL fetching and summarization |
| `extract.summary_chunk_chars` | `100000` | Chunk size in chars for tier-3 summarization |
| `extract.refuse_threshold_chars` | `2000000` | Content size above which web_extract refuses entirely |
| `extract.summary_tier2_threshold_chars` | `5000` | Boundary between tier-1 (direct) and tier-2 (light summary) |
| `extract.summary_tier3_threshold_chars` | `500000` | Boundary between tier-2 and tier-3 (chunked summary) |
| `extract.redact_url_patterns` | `[]` | Extra secret-URL patterns to redact (appended to built-in defaults) |

### Tools (`tools:`)

Toolsets enabled by default via `ToolsConfig::default()`: `memory`, `session`, `agent`, `skills`. All known toolsets (including `robotics`) are additionally ensured present via `with_default_toolsets_merged()`, which iterates over `crate::constants::ALL_TOOLSETS`.

Toolsets disabled by default: `web`, `code`, `browser`

```yaml
tools:
  toolsets:
    memory:
      enabled: true
    session:
      enabled: true
    agent:
      enabled: true
    skills:
      enabled: true
    robotics:
      enabled: true   # gates further on HEXAPOD_IP env var
    web:
      enabled: false   # opt-in required
    code:
      enabled: false   # opt-in required
    browser:
      enabled: false   # opt-in required
```

---

## Provider Configuration

The `providers:` map is the canonical place to wire API keys. Use `api_key_env` (not `api_key` literals) to keep secrets out of the config file:

```yaml
providers:
  openrouter:
    api_key_env: OPENROUTER_API_KEY   # secret lives in ~/.ironhermes/.env
    # default_model: "anthropic/claude-sonnet-4"
    # api_mode: chat_completions
    # fallback_providers: ["local-llama"]
    # disabled: false

  anthropic:
    api_key_env: ANTHROPIC_API_KEY
    api_mode: anthropic_messages
```

**Supported `api_mode` values:** `chat_completions`, `anthropic_messages`, `codex_responses`

**Custom (local) providers** can be defined under `custom_providers:` for Ollama, llama.cpp, or any OpenAI-compatible endpoint:

```yaml
custom_providers:
  - name: "local-llama"
    base_url: "http://localhost:11434/v1"
    api_key: "ollama"
    api_mode: chat_completions
    default_model: "llama3.2:latest"
```

### Auxiliary Model Routing

To route helper tasks (vision, compression, summarization, etc.) to a cheaper model, configure the `auxiliary:` block:

```yaml
auxiliary:
  provider: "openrouter"
  model: "meta-llama/llama-3.1-8b-instruct"
```

Per-task overrides are set under `model.roles:`:

```yaml
model:
  roles:
    vision:
      provider: "openrouter"
      model: "openai/gpt-4o"
    compression:
      provider: "main"   # inherits the primary provider
```

Reserved role names: `vision`, `compression`, `session_search`, `skills_hub`, `mcp_helper`, `summarization`, `curator`

---

## Per-Environment Overrides

IronHermes does not use `.env.development` / `.env.production` style files. Per-environment configuration is handled through:

1. **Named profiles** — `hermes --profile production` pivots `IRONHERMES_HOME` to `~/.ironhermes/profiles/production/`, which has its own `config.yaml` and `.env`.
2. **`IRONHERMES_HOME` env var** — set in the shell or process environment before launching to point at any directory containing `config.yaml` and `.env`.
3. **Platform secret managers** — set `IRONHERMES_HOME` and provider API key env vars via your deployment platform's secret injection (e.g., Railway, Fly.io, Docker environment). <!-- VERIFY: specific platform secret manager integration details -->
