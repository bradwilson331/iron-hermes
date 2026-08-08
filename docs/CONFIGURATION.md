<!-- generated-by: gsd-doc-writer -->
# Configuration

IronHermes uses two configuration files, both stored under its home directory (`~/.ironhermes/` by default):

- **`~/.ironhermes/config.yaml`** — primary YAML configuration for all agent behavior, providers, tools, and subsystems.
- **`~/.ironhermes/.env`** — environment variable overrides, primarily for API secrets.

The home directory location can be overridden with the `IRONHERMES_HOME` environment variable. When using named profiles (`hermes --profile <name>`), the home directory is automatically pivoted to `~/.ironhermes/profiles/<name>/`. Each profile therefore loads its own `config.yaml` and `.env`. A dispatcher-spawned Kanban worker assigned to profile `dev`, for example, loads `~/.ironhermes/profiles/dev/config.yaml` and `~/.ironhermes/profiles/dev/.env`; provider secrets required by that worker must exist in the profile-scoped `.env`.

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
| `PERPLEXITY_API_KEY` | Optional | API key for Perplexity. Also `web_answer`'s first-position provider (Phase 41.3) — see [Web Tools](#web-tools-toolsweb_search--toolsweb_answer--toolsweb_extract). |
| `DEEPSEEK_API_KEY` | Optional | API key for DeepSeek |
| `FIREWORKS_API_KEY` | Optional | API key for Fireworks AI |
| `OLLAMA_BASE_URL` | Optional | Base URL for Ollama server. Default: `http://localhost:11434` |
| `OLLAMA_API_KEY` | Optional | API key for Ollama (if required by your server) |

### Tool API Keys

| Variable | Required | Description |
|---|---|---|
| `FIRECRAWL_API_KEY` | Optional | API key for Firecrawl web scraping backend. Format: `fc-...`. First-position `web_extract` provider — see [Web Tools](#web-tools-toolsweb_search--toolsweb_answer--toolsweb_extract). |
| `EXA_API_KEY` | Optional | API key for Exa. Used by `web_search`, `web_answer`, and `web_extract` — see [Web Tools](#web-tools-toolsweb_search--toolsweb_answer--toolsweb_extract). |
| `TAVILY_API_KEY` | Optional | API key for Tavily. Used by `web_search` and `web_extract` — see [Web Tools](#web-tools-toolsweb_search--toolsweb_answer--toolsweb_extract). |
| `BRAVE_API_KEY` | Optional | API key for Brave Search. Used by `web_search` and `web_answer` — see [Web Tools](#web-tools-toolsweb_search--toolsweb_answer--toolsweb_extract). |
| `VENICE_API_KEY` | Default for generation | Venice.ai API key for any generation mode resolved to the **venice** backend — the Phase 47 default for all four modes (t2i/t2v/i2v/v2v). When a mode resolves to venice and this is unset, that tool is hidden from the LLM (zero startup cost). Resolved lazily at call time, never logged. Also requires the `web` toolset enabled for chat — see *Image & Video Generation*. |
| `FAL_KEY` | fal-backed generation modes | fal.ai API key for any generation mode resolved to the **fal** backend (`fal-ai/*` models, or a mode with `provider: fal`). When a mode resolves to fal and this is unset, that tool is hidden and no fal client is built. Resolved lazily at call time, never logged. Get one at <https://fal.ai/dashboard/keys>. Also requires the `web` toolset enabled for chat — see *Image & Video Generation*. |

None of `FIRECRAWL_API_KEY`/`EXA_API_KEY`/`TAVILY_API_KEY`/`BRAVE_API_KEY`/`PERPLEXITY_API_KEY` is ever **required** — `web_search` and `web_answer` both fall through to keyless DuckDuckGo when nothing else is configured (Phase 41.3 D-09), and setting one only promotes a higher-quality provider earlier in that tool's chain. See [Web Tools](#web-tools-toolsweb_search--toolsweb_answer--toolsweb_extract) for the full chain mechanics and [Tool Credentials](#tool-credentials-toolscredentials--env--config--vault) for where else (besides `.env`) these five keys can live.

### Voice / Speech-to-Text

Voice mode (`/voice`, the TUI record key, and the web mic button) transcribes audio with a cloud STT provider. Provider selection is key-presence based: with `stt.provider: auto`, the first provider whose API key is set wins (Groq preferred, then OpenAI). Keys are read from the environment only — never written to logs.

| Variable | Required | Default | Description |
|---|---|---|---|
| `GROQ_API_KEY` | Required for Groq STT | — | API key for Groq Whisper transcription (also used for the Groq LLM provider). Format: `gsk_...` |
| `VOICE_TOOLS_OPENAI_KEY` | Required for OpenAI STT | — | API key for OpenAI Whisper transcription. Kept distinct from `OPENAI_API_KEY` so STT can use a separate key/budget. Format: `sk-...` |
| `STT_GROQ_MODEL` | Optional | `whisper-large-v3-turbo` | Overrides `stt.groq.model` for the Groq provider |
| `STT_OPENAI_MODEL` | Optional | `whisper-1` | Overrides `stt.openai.model` for the OpenAI provider |
| `GROQ_BASE_URL` | Optional | `https://api.groq.com/openai/v1` | Override the Groq API base URL (testing / proxies) |
| `OPENAI_STT_BASE_URL` | Optional | `https://api.openai.com/v1` | Override the OpenAI STT base URL (testing / proxies) |
| `ELEVENLABS_API_KEY` | Required for ElevenLabs TTS | — | API key for ElevenLabs text-to-speech (free-mode spoken replies). Without it, `tts.provider: elevenlabs` reports unavailable and replies fall back to keyless Edge. |

Spoken replies use a TTS key (`ELEVENLABS_API_KEY` for ElevenLabs, `OPENAI_API_KEY` for OpenAI TTS; Edge is keyless), and the `open_mic` realtime voice path uses an OpenAI key resolved via `providers.openai.api_key_env`. The full voice-to-voice walkthrough — including which mode uses which voice — is in [`VOICE-TO-VOICE.md`](VOICE-TO-VOICE.md).

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
| `TERMINAL_BACKEND` | Optional | `local` | Exec backend: `local`, `docker`, or `ssh` |
| `TERMINAL_CWD` | Optional | `.` | Default working directory for agent tool operations |
| `TERMINAL_TIMEOUT` | Optional | `30` | Command execution timeout in seconds |
| `TERMINAL_DOCKER_IMAGE` | Optional | — | Docker sandbox image (when `TERMINAL_BACKEND=docker`) |
| `TERMINAL_ENV` | Optional | — | Comma-separated env var names to pass through to sandbox |

> The docker/ssh backend knobs (`container_runtime`, `image`, `forward_env`, `container.*`, `ssh.*`,
> `container_reap_after_secs`) are **YAML-only** — set them under the `terminal:` block in
> `config.yaml`. See [Terminal (`terminal:`)](#terminal-terminal) and `docs/MULTI-ENVIRONMENT-EXEC.md`.

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
| `IRONHERMES_CRON_MAX_PARALLEL` | Optional | `2` | Maximum number of non-workdir cron jobs to run concurrently per tick. When the env var is unset, resolution falls back to `cron.max_parallel` in `config.yaml` (default `2`). `0` = unbounded; `1` = serial. |

### IronHermes Home

| Variable | Required | Default | Description |
|---|---|---|---|
| `IRONHERMES_HOME` | Optional | `~/.ironhermes` | Override the default data and config directory |
| `IRONHERMES_SOURCE` | Optional | — | Path to the IronHermes project root. When set, `hermes setup` (full) copies skill files from `$IRONHERMES_SOURCE/skills/` and `$IRONHERMES_SOURCE/optional-skills/` into `$IRONHERMES_HOME/skills/`. Auto-detected in dev builds via binary path walk. |
| `IRONHERMES_WORKER_BIN` | Optional | `ironhermes` via `PATH` | Absolute path to the executable used when the Kanban dispatcher spawns worker processes. Set this in the dispatcher's `.env` when running a development build that is not installed on `PATH`, for example `/Users/me/code/ironhermes/target/debug/ironhermes`. The value is forwarded through the worker's scrubbed environment for recursive spawns. |

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
| `kanban` | Dispatcher, concurrency, retry, and default worker workspace settings |
| `web` | Web scraping backend and request settings |
| `exec` | Python code execution sandbox |
| `gateway` | Messaging platform adapters (Telegram, etc.) |
| `cron` | Scheduled job settings |
| `memory` | Memory provider selection |
| `compression` | Context compression tuning |
| `skills` | Skills subsystem enable/disable and scan paths |
| `delegation` | Subagent delegation limits (renamed from `subagent` in Phase 32.2 — see [Delegation](#delegation-delegation)) |
| `rate_limit` | Per-user inbound rate limiting |
| `batch` | Batch processing worker settings |
| `security` | Secret redaction in logs |
| `providers` | Per-provider API key and endpoint overrides |
| `custom_providers` | User-defined OpenAI-compatible endpoints |
| `tools` | Per-toolset enable/disable, tool execution timeout (+ per-tool overrides), web-tool provider chains (`web_search`/`web_answer`/`web_extract`), and the config tier of tool credentials — see [Web Tools](#web-tools-toolsweb_search--toolsweb_answer--toolsweb_extract) |
| `stt` | Speech-to-text provider selection and per-provider model overrides |
| `tts` | Text-to-speech provider selection and per-provider voice/format overrides |
| `voice` | Voice-mode interaction: record key, VAD silence detection, barge-in mode, wake word, realtime tuning |
| `autonomous` | Autonomous (yolo) mode — skip dangerous-command approval prompts |
| `concurrency` | Per-session and process-wide caps for concurrent in-flight agent turns (Phase 39.1) |
| `auxiliary` | Auxiliary model routing for helper tasks |
| `browser` | Browser automation settings |
| `extract` | Web extraction (web_extract tool) tuning |
| `image_gen` | Text→image (`image_gen`): per-mode `{provider, model}` (venice default), per-session cap, poll timeout |
| `video_gen` | Text/image/video→video (`video_generate`/`video_animate`/`video_to_video`): per-mode `{provider, model}`, caps, resolution/aspect, progress ping |
| `generation` | Cross-surface generation spend guardrails (session pool + per-child cap + per-surface enable map) — delegate/kanban descendants only |
| `audio_cache` | Audio cache lifecycle policy (max age, sweep interval) |
| `mcp_servers` | MCP server configurations (raw YAML, parsed by ironhermes-mcp) |
| `vault` | Optional operator secret store for provider API keys — consulted as a last-resort fallback in `ProviderResolver` resolution (Phase 46.8). See [Vault](#vault-vault) |

---

## Required vs Optional Settings

The following settings cause startup validation to fail and re-launch the setup wizard (`hermes setup model`) if absent or empty:

| Setting | Validation Rule |
|---|---|
| `providers.<main-provider>.api_key_env` | Required — must reference a non-empty env var name matching `[A-Z][A-Z0-9_]*`. **Auto-backfilled by `hermes setup`** when the matching env var exists in `.env` or process env but the config entry is absent. |
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
| `agent.max_iterations` | `50` | Canonical per-turn cap — sizes both the `AgentLoop` iteration limit and the `BudgetHandle` it builds. Lowered from 90 by the runaway-delegation guard (commit `aaa1b0ac`); pairs with the consecutive-failure circuit breaker in `AgentLoop`. Read by `AgentRuntime::from_config` (Phase 28.1) |
| `agent.max_turns` | _(deprecated alias)_ | Deprecated in Phase 28.1. If present, `AgentConfig::normalize()` folds it onto `max_iterations` and emits a `warn!`. Remove from new configs |
| `agent.context_compression` | `0.5` | Context compression ratio |
| `agent.tool_delay_secs` | `1.0` | Delay between tool calls in seconds |
| `agent.context_engine` | `summarizing` | Context engine: `summarizing` or `local_prune` |
| `agent.compression_threshold` | `0.5` | Fraction of context_length at which compression triggers |
| `agent.system_message` | `""` | Optional injected system message (empty = omitted) |

### Terminal (`terminal:`)

Selects and configures the exec backend for the `terminal` and `execute_code` tools. Every key is
optional (`serde(default)`), so a minimal `terminal:` block keeps `backend: local` and behaves
exactly as before. Backend selection is **config-only** (D-06) — no LLM/tool argument can change it.
The `container.*` and `ssh.*` blocks are only consulted for `backend: docker` / `backend: ssh`
respectively; an unavailable/misconfigured backend **hard-errors** rather than silently falling back
to local (D-05). See the operator guide `docs/MULTI-ENVIRONMENT-EXEC.md` for the full walkthrough.

| Key | Default | Description |
|---|---|---|
| `terminal.backend` | `local` | Exec backend: `local`, `docker`, or `ssh` |
| `terminal.cwd` | `.` | Default working directory for tool operations |
| `terminal.timeout` | `30` | Command execution timeout in seconds |
| `terminal.terminal_env_allowlist` | `[]` | Env var names passed through to the **local** terminal subprocess, on top of the base safe-env set (Phase 42 D-05) |
| `terminal.container_runtime` | `docker` | Container CLI for the `docker` backend: `docker` or `podman`. Explicit, never auto-detected by probing PATH (D-07) |
| `terminal.image` | `debian:stable-slim` | Base image for the `docker` backend's persistent container |
| `terminal.forward_env` | `[]` | Credential/env allowlist forwarded **across the docker/ssh backend boundary** (D-09). Empty = nothing secret crosses. Distinct from `terminal_env_allowlist`, which is local-only |
| `terminal.container_reap_after_secs` | `86400` | Orphan-reaper lifetime (seconds); the boot-time reaper GCs labeled containers idle longer than 2× this (D-02) |
| `terminal.container.cpu` | `1.0` | `docker` backend CPU limit (fractional cores) |
| `terminal.container.memory_mib` | `5120` | `docker` backend memory limit in MiB (5 GiB) |
| `terminal.container.disk_mib` | `51200` | `docker` backend workspace disk limit in MiB (50 GiB) |
| `terminal.container.pids_limit` | `256` | `docker` backend `--pids-limit` process cap |
| `terminal.container.persistent` | `true` | `true` = bind-mount `/workspace` (survives container recreation); `false` = ephemeral tmpfs |
| `terminal.container.network` | `false` | `false` → `--network=none` (security-hardened default, D-09); `true` enables container networking |
| `terminal.ssh` | `null` | SSH backend connection block; `null` means the `ssh` backend cannot be constructed (hard-errors per D-05) |
| `terminal.ssh.host` | — | SSH host to connect to (required for `backend: ssh`) |
| `terminal.ssh.user` | — | SSH user to connect as (required for `backend: ssh`) |
| `terminal.ssh.port` | `22` | SSH port |
| `terminal.ssh.key_path` | `null` | Path to an SSH private key; `null` uses the ssh CLI's default identity resolution (agent, `~/.ssh/id_*`) |

> **Security note (D-08/D-10):** on every backend including `local`, `terminal` and `execute_code`
> now pass through the guardrail + AuditLog, and remote/credential-forwarding runs are forced through
> the approval gate. The web/desktop surface uses a fail-closed **deny** posture (no interactive
> approval UX yet). `yolo: true` relaxes `Allow`-tier approval but never bypasses the Tier-2 block or
> the audit floor.

### Kanban (`kanban:`)

| Key | Default | Description |
|---|---|---|
| `kanban.dispatch_in_gateway` | `true` | Runs the dispatcher inside the gateway process. |
| `kanban.dispatch_interval_seconds` | `60` | Seconds between dispatcher ticks. |
| `kanban.max_in_progress` | `8` | Maximum concurrently running tasks; `null` or `0` means unlimited. |
| `kanban.failure_limit` | `2` | Consecutive non-successful attempts before the circuit breaker blocks a task. |
| `kanban.default_workdir` | `null` | Absolute directory used when a task has no explicit workspace. External directories must already exist; paths under the managed Kanban scratch root are created as needed. Explicit task workspace wins; when unset, the dispatcher creates a task-specific scratch workspace. |

```yaml
kanban:
  dispatch_in_gateway: true
  default_workdir: /Users/me/code/active-project
```

Do not use `~`, `$IRONHERMES_KANBAN_WORKSPACE`, or another unexpanded shell expression in `default_workdir`. Worker workspace resolution requires an absolute path. The dispatcher canonicalizes the selected path and injects that exact value as `IRONHERMES_KANBAN_WORKSPACE`; users do not set this per-task runtime variable themselves.

Workspace precedence is:

1. Explicit task workspace (`--workspace`, `dir:<absolute-path>`, or the task record).
2. `kanban.default_workdir` from `config.yaml`.
3. The task-specific scratch directory under the Kanban workspaces root.

Both the gateway-embedded dispatcher and the CLI `hermes kanban dispatch` / deprecated `hermes kanban daemon --force` paths load this `kanban:` block from `config.yaml`.

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

### Speech-to-Text (`stt:`)

| Key | Default | Description |
|---|---|---|
| `stt.provider` | `auto` | Active STT provider: `auto`, `groq`, or `openai`. `auto` selects the first provider whose API key is set (Groq preferred, then OpenAI). |
| `stt.groq.model` | `whisper-large-v3-turbo` | Groq Whisper model. Overridden by `STT_GROQ_MODEL`. |
| `stt.openai.model` | `whisper-1` | OpenAI Whisper model. Overridden by `STT_OPENAI_MODEL`. |

Set the matching API key (`GROQ_API_KEY` and/or `VOICE_TOOLS_OPENAI_KEY`) in `.env`; see [Voice / Speech-to-Text](#voice--speech-to-text) above. With no key set, voice mode reports `provider: none` and transcription is disabled.

### Voice Mode (`voice:`)

| Key | Default | Description |
|---|---|---|
| `voice.record_key` | `ctrl+b` | TUI key binding to start/stop recording (D-08) |
| `voice.silence_threshold` | `200` | RMS energy below which audio counts as silence (D-09). **Lower this (e.g. `60`) if recording never ends** — some mics (built-in MacBook, low gain) run below 200 RMS even during speech, so the default treats your speech as silence. Run with `RUST_LOG=ironhermes_tools=info` and watch the `max_rms=…` line to pick a value below your speech level but above your silence floor. |
| `voice.silence_duration` | `3.0` | Seconds of continuous silence that ends a recording (D-09) |
| `voice.auto_tts` | `false` | When `true`, **every** agent reply is spoken back (the `/voice tts` "All mode" — toggle live with `/voice tts`). Independent of `silence_threshold`/STT. |
| `voice.beep_enabled` | `true` | Play an audio beep when recording starts/stops |
| `voice.max_recording_seconds` | `120` | Maximum recording length before a forced stop |
| `voice.barge_in_mode` | `push_to_interrupt` | Barge-in behavior during agent TTS playback. `push_to_interrupt` — turn-based, user presses the record key to interrupt (CLI/TUI + web, default). `open_mic` — full-duplex realtime voice over OpenAI Realtime API + WebRTC (web/orb only, Phase 36.17.12); requires `providers.openai.api_key_env` to resolve. `half_duplex` — deferred, not wired. |
| `voice.wake_word.enabled` | `false` | Enable wake-word gating (case-insensitive "contains" match). **Applies to turn-based mode only** — disabled in `open_mic` mode (the UI greys it out with a hint). |
| `voice.wake_word.phrase` | `hey hermes` | Wake phrase to match |
| `voice.web_silence_threshold_rms` | `5.0` | Web Audio (browser) RMS silence threshold. **DISTINCT from `silence_threshold`** (native PCM amplitude scale). Web VAD path only. Lower if the browser never detects end-of-speech; raise if it cuts off too early. |
| `voice.realtime_model` | `gpt-realtime` | OpenAI Realtime API model for the open-mic WebRTC path. Whitelist-validated; unlisted values trigger the D-07 graceful fallback to turn-based voice. |
| `voice.realtime_voice` | `shimmer` | Realtime agent voice. One of: `alloy`, `shimmer`, `echo`, `verse`, `ash`, `ballad`, `coral`, `sage`. Whitelist-validated; unlisted values trigger the D-07 fallback. |
| `voice.realtime_transcription_model` | `gpt-4o-mini-transcribe` | Input-audio transcription model for the realtime path. One of: `gpt-4o-mini-transcribe`, `gpt-4o-transcribe`, `whisper-1`, `off` (disables transcription). |
| `voice.realtime_noise_reduction` | `far_field` | Realtime mic noise-reduction profile applied before VAD and the model. `far_field` (laptop/built-in mic, most aggressive), `near_field` (headset/earbuds), `off` (disable). |
| `voice.realtime_vad_mode` | `semantic_vad` | Realtime turn-detection mode. `semantic_vad` — model-based, robust to background noise (default). `server_vad` — energy-based, lower latency; uses the three keys below. |
| `voice.realtime_vad_threshold` | `0.5` | `server_vad` activation threshold (0.0–1.0). Higher = less sensitive. Only applies when `realtime_vad_mode: server_vad`. |
| `voice.realtime_vad_silence_ms` | `500` | `server_vad` trailing-silence in ms that ends a turn. Only applies when `realtime_vad_mode: server_vad`. |
| `voice.realtime_vad_prefix_ms` | `300` | `server_vad` prefix padding in ms of audio kept before detected speech. Only applies when `realtime_vad_mode: server_vad`. |

**Phase 39.3 — open_mic is a full Hermes agent surface.** With `voice.barge_in_mode: open_mic`, the realtime voice session is no longer a plain conversational model — it runs with the full Hermes identity and skills (same `PromptBuilder` output as text chat), exposes the complete `ToolRegistry` as session tools, and routes function calls through the same approval/yolo/DEFCON gate as all other surfaces. A visual Approve/Deny card appears in the orb overlay while the session keeps conversing; background tool turns deliver a verbal acknowledgment followed by an in-flight "working…" badge, and the result is voiced and written to transcript/trajectory on completion.

> **Wake-word limitation:** `voice.wake_word` applies to turn-based mode only (`push_to_interrupt`). In `open_mic` mode the wake-word UI control is greyed out with an explanatory hint; the setting has no effect on the realtime session.

**Spoken replies — three modes** (set live with `/voice`, no config needed):

| Command | Mode | Speaks the reply when… |
|---|---|---|
| `/voice off` | Off | never |
| `/voice on` | Voice-Only | the turn's input was voice (`Ctrl+B`); typed turns stay text-only |
| `/voice tts` | All | always (typed or voice) — sets `voice.auto_tts` |

Spoken replies synthesize with the [`tts.provider`](#text-to-speech-tts) provider (default **Edge**, keyless) and fall back to Edge when the configured provider is unavailable. The reply text is cleaned before synthesis — fenced code blocks become "(code omitted)", markdown markers/links are stripped, and output is capped (~600 chars) so the agent reads listenable prose, not code.

Example — enable voice mode with Groq STT and spoken replies:

```yaml
stt:
  provider: auto              # auto | groq | openai
  groq:
    model: whisper-large-v3-turbo
  openai:
    model: whisper-1

voice:
  record_key: ctrl+b
  silence_threshold: 200
  silence_duration: 3.0
  auto_tts: false
  beep_enabled: true
  max_recording_seconds: 120
```

Both blocks are optional — omitting them applies every default above, so pre-existing `config.yaml` files keep working unchanged. Check the live state any time with `/voice status`.

### Text-to-Speech (`tts:`)

Controls how spoken replies (and the agent's `text_to_speech` tool) synthesize audio.

| Key | Default | Description |
|---|---|---|
| `tts.provider` | `edge` | Global active provider: `edge` (Microsoft Edge TTS — **keyless**), `elevenlabs` (needs `ELEVENLABS_API_KEY`), or `openai` (needs `OPENAI_API_KEY`). Per-avatar overrides under [`identities:`](#identities-identities) win over this. Falls back to Edge when the configured provider is unavailable. |
| `tts.ffmpeg_path` | `null` | Path to `ffmpeg` for MP3→Opus conversion (Telegram voice bubbles). `null` auto-detects `ffmpeg` on `PATH`. |
| `tts.edge.voice` | `en-US-AriaNeural` | Edge TTS voice name |
| `tts.edge.output_format` | `mp3` | Edge audio output format |
| `tts.elevenlabs.voice_id` | `pNInz6obpgDQGcFmaJgB` | ElevenLabs voice ID (Adam). The voice ID from your ElevenLabs library. |
| `tts.elevenlabs.model_id` | `eleven_multilingual_v2` | ElevenLabs model ID |
| `tts.elevenlabs.output_format` | `mp3` | ElevenLabs audio output format (v1 emits MP3) |
| `tts.openai.model` | `tts-1` | OpenAI TTS model |
| `tts.openai.voice` | `alloy` | OpenAI TTS voice: alloy, echo, fable, onyx, nova, shimmer |
| `tts.openai.format` | `mp3` | OpenAI TTS output format |

Edge requires no key and works out of the box. ElevenLabs requires `ELEVENLABS_API_KEY` and OpenAI TTS requires `OPENAI_API_KEY`; without the key the provider reports unavailable and spoken replies fall back to Edge.

This `tts:` block is the **global** voice. Each orb/avatar can override it per-identity (see [Identities](#identities-identities)). **ElevenLabs (and any `tts:` provider) is only used in free mode** (`voice.barge_in_mode: push_to_interrupt`); `open_mic` realtime uses OpenAI realtime voices instead. Full walkthrough: [`VOICE-TO-VOICE.md`](VOICE-TO-VOICE.md).

```yaml
tts:
  provider: elevenlabs        # edge | elevenlabs | openai
  elevenlabs:
    voice_id: pNInz6obpgDQGcFmaJgB
    model_id: eleven_multilingual_v2
```

### Identities (`identities:`)

Phase 40.5. Per-avatar **voice** (free-mode TTS + realtime voice) and **orb
appearance**. The LLM and STT stay global (an identity is a voice/look, not a
separate agent). Records live in `config.yaml`; the *active* identity is selected
in the UI (orb/avatar picker + Voice Settings "Applies to") and stored in the
browser. Voice/identity changes take effect on the **next turn** — no restart.

This section is **seeded automatically** on first run (`default_seed_identities`)
and missing shipped personas are back-filled into a partial section, so you rarely
hand-write it. Override only what you want to change.

| Key | Type / values | Description |
|---|---|---|
| `identities.<slug>.display_name` | string | Shown in the "Applies to" selector |
| `identities.<slug>.appearance.style` | `classic`\|`bloom`\|`ascii`\|`network` | Orb render mode (`null` for head avatars / inherit registry) |
| `identities.<slug>.appearance.base_hue` | `0`–`360` | Idle hue; listening/thinking/speaking shift relative to it |
| `identities.<slug>.appearance.size` | `0.5`–`2.0` | Orb scale factor |
| `identities.<slug>.appearance.glow` | `0.0`–`1.0` | Glow/bloom intensity |
| `identities.<slug>.voice.free_mode_tts_provider` | `edge`\|`openai`\|`elevenlabs`\|`null` | Free-mode TTS provider override. `null` = inherit global `tts.provider`. |
| `identities.<slug>.voice.free_mode_tts_voice` | string\|`null` | Free-mode voice override. For `elevenlabs` this is a voice ID. `null` = inherit global. |
| `identities.<slug>.voice.realtime_voice` | string\|`null` | OpenAI realtime voice (`open_mic` only): alloy, shimmer, echo, verse, ash, ballad, coral, sage. `null` = inherit global `voice.realtime_voice`. |

**Resolution** (`Config::effective_tts_config_for_identity`): the active identity's
non-null fields override the global config; null/absent inherit it. A
`free_mode_tts_voice` flows into the matching provider's voice slot —
`elevenlabs → elevenlabs.voice_id`, `openai → openai.voice`, `edge → edge.voice`.
To give an avatar a specific ElevenLabs voice, set **both** `free_mode_tts_provider:
elevenlabs` **and** `free_mode_tts_voice: <voice_id>`.

```yaml
identities:
  orb_bloom:                    # seeded: ElevenLabs Adam in free mode, shimmer in realtime
    display_name: Bloom
    appearance: { style: bloom, base_hue: 280, size: 1.0, glow: 0.8 }
    voice:
      free_mode_tts_provider: elevenlabs
      free_mode_tts_voice: pNInz6obpgDQGcFmaJgB
      realtime_voice: shimmer
  orb_classic:                  # all-null voice -> inherits the global tts:/voice: config
    display_name: Classic
    appearance: { style: classic, base_hue: 186, size: 1.0, glow: 0.5 }
    voice: { free_mode_tts_provider: null, free_mode_tts_voice: null, realtime_voice: null }
```

### Memory (`memory:`)

| Key | Default | Description |
|---|---|---|
| `memory.provider` | `file` | Provider: `file`, `sqlite`, `grafeo`, or `duckdb` |
| `memory.memory_enabled` | `true` | Enable/disable the memory subsystem entirely |
| `memory.user_profile_enabled` | `true` | Enable/disable the USER.md profile store |
| `memory.mirror_provider` | `null` | Optional write-only mirror provider |
| `memory.nudge_interval` | `10` | Turns between periodic memory-review nudges. `0` disables the nudge entirely. See [Periodic Memory Review Nudge](#periodic-memory-review-nudge) below. |
| `memory.skill_creation_guidance` | `true` | When `true` AND the `skill_manage` tool is registered, the system prompt includes the "Skill Creation (Learning Loop)" trigger block that tells the agent when to author a `SKILL.md`. Set to `false` in YAML to suppress the block (e.g. for child agents or restricted deployments). See [Autonomous Skill Creation](#autonomous-skill-creation-learning-toolset) below. |
| `memory.recall_min_score` | `0.0000072` | Minimum bm25 relevance score (negated FTS5 bm25, higher = more relevant) for `memory_recall` results from the sqlite provider; below-floor matches are dropped so off-topic memories are never recalled (Phase 47.5 D-03). The default is calibrated on a small test fixture and bm25 scores are corpus-relative — raise this if irrelevant memories still surface, lower it if legitimate recall goes quiet. No-op for the duckdb provider, whose recall is substring match with synthetic scores. |

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

#### Autonomous Skill Creation (Learning Toolset)

Phase 33 introduces the **`learning` toolset** — a single tool, `skill_manage`,
that lets the agent author and curate its own skills (`SKILL.md` files) at
runtime. Combined with the `skill_creation_guidance` trigger block in the
system prompt (see above), this delivers the autonomous skill-creation loop:
the agent recognises when a workflow is worth documenting, then writes a
durable skill it can find later via the existing skill-scanner.

**What the agent decides on its own.** The trigger block (above the user
prompt at every session freeze) instructs the agent to author a `SKILL.md`
when **any** of these signal a non-trivial workflow:

- It made 5 or more tool calls to complete the task
- It recovered from a tool error or unexpected result mid-task
- The user corrected its approach mid-task
- It discovered a non-obvious workflow that worked well

You can verify the block is live with:

```bash
grep "## Skill Creation (Learning Loop)" <(hermes config show prompt 2>/dev/null) || true
# or directly from source:
grep -A3 "^const SKILL_CREATION_GUIDANCE" crates/ironhermes-agent/src/prompt_builder.rs
```

**The `skill_manage` tool — 6 JSON-schema actions** (from
`crates/ironhermes-tools/src/skill_manage.rs`):

| Action | Purpose | Notes |
|---|---|---|
| `create` | Write a new `SKILL.md` with `Self-created` trust_tier | Two-level path: `$HERMES_HOME/skills/<category>/<slug>/SKILL.md`. Frontmatter includes `platforms` and `metadata.hermes.{tags, category, trust_tier}`. Skill name validated cross-crate via `pub fn validate_skill_name`. |
| `patch` | Surgical edit: `content.replacen(old_string, new_string, 1)` | Returns JSON `{ "error": "not_found", ... }` when `old_string` isn't present. Prefer this for incremental skill improvement — pass only the changed substring, not the whole file. |
| `edit` | Full SKILL.md rewrite | Overwrites the entire file. Use only for major rewrites. |
| `delete` | Remove the whole skill directory | Canonical-path verified — must resolve under `$HERMES_HOME/skills/` or the call is rejected. |
| `write_file` | Write a companion file inside the skill dir (e.g. `references/api.md`, `scripts/helper.py`) | Path-traversal blocked: `..` segments and absolute paths rejected; runs the content-scan gate. |
| `remove_file` | Remove a companion file inside the skill dir | Same canonical-path verification as `delete`. |

**`Self-created` trust tier** (LEARN-04). The `SkillSource` enum gains a
fourth variant (`#[serde(rename = "Self-created")]`) alongside `Builtin`,
`Catalog`, and `Local`. Self-created skills are routed through a
**WARN-BUT-LOAD** branch in the scan enforcer — they are loaded into the
runtime registry but logged so you can spot a runaway loop in the
operator dashboard. Verify with:

```bash
grep -n "SelfCreated\|Self-created" crates/ironhermes-core/src/skills.rs | head -5
```

**Enabling / disabling the toolset.** `learning` is wired into every
registration surface — `KNOWN_TOOLSETS` (CLI), `toolset_members_map`
(toolset session), `ALL_TOOLSETS` (constants), and the
`app_runtime_factory` registration loop — so you can toggle it like any
other toolset. **Note:** profiles that were saved before Phase 33 carry an
explicit `tools.toolsets:` map that does NOT mention `learning`; the
`with_default_toolsets_merged()` migration adds it as **enabled** (backward-compat:
upgrading users are not silently locked out of newly-added toolsets). Run
`hermes toolset disable learning` once if you want to opt out.

```bash
# Status check
hermes toolset list                # 'learning' will appear in the table
hermes toolset show learning       # members + registered tools + prerequisites

# Toggle persistently in the active profile config.yaml
hermes toolset enable learning
hermes toolset disable learning
```

When the toolset is disabled, `skill_manage` is not in the LLM-visible tool
list AND the prompt's skill-creation block is suppressed automatically
(the block is gated on `active_tools.contains("skill_manage")` regardless
of `skill_creation_guidance`).

**Suppressing only the prompt guidance** (e.g. child agents or restricted
deployments) while keeping the tool registered:

```yaml
memory:
  skill_creation_guidance: false   # tool still available, guidance suppressed
```

```bash
hermes config set memory.skill_creation_guidance false
```

**Verifying the feature is live:**

```bash
# 1. Workspace builds and invariants are locked
cargo test -p ironhermes-agent --test invariants_33    # 6/6 INV-33-* tests
cargo test -p ironhermes-tools --lib skill_manage      # 7/7 unit tests
                                                       #   - schema_actions (lists all 6)
                                                       #   - create_frontmatter, edit_overwrites
                                                       #   - patch, create_blocked_content
                                                       #   - path_traversal_rejected
                                                       #   - delete_removes_dir

# 2. CLI surfaces the new toolset
hermes toolset list | grep learning
hermes toolset show learning

# 3. Watch the agent author a skill in a CLI session — confirm the block is
# in the prompt and the agent calls skill_manage when a long workflow ends
RUST_LOG=ironhermes_tools::skill_manage=info hermes chat
```

### Compression (`compression:`)

| Key | Default | Description |
|---|---|---|
| `compression.protect_last_tokens` | `20000` | Tokens to protect at end of conversation |
| `compression.tool_pair_shift_tokens` | `500` | Token budget for tool-pair boundary shifting |
| `compression.protect_first_n` | `3` | Maximum number of leading system messages protected from compression (Phase 47.5: the first conversation pair is no longer pinned; `3` remains the default upper bound). |

### Gateway (`gateway:`)

| Key | Default | Description |
|---|---|---|
| `gateway.context_engine` | `local_prune` | Context engine for gateway sessions |
| `gateway.compression_threshold` | `0.85` | Compression threshold for gateway (fraction of context_length) |
| `gateway.persist_sessions` | `true` | Persist gateway session routing (`SessionKey → session_id`) + per-session voice mode to `state.db`. When `true`, an ongoing Telegram/Discord/Slack conversation **resumes its prior session across a restart** (same thread, full history rehydrated) instead of starting fresh. Set `false` to restore the legacy stateless behavior. |
| `gateway.platforms` | `{}` | Platform adapters map (empty = no platforms enabled) |

**Session persistence & resume (`persist_sessions`).** With the default `true`, the gateway records each chat's active session in a durable `gateway_routes` table (`state.db`, schema v10). On restart, the next inbound message from that chat looks up its route and resumes the same session — message history is rehydrated and the conversation continues. The per-chat voice mode (`/voice on|off|tts`) is stored in the same record, so it also survives restarts. Resume only applies while the prior session is still open (not ended/expired); otherwise a fresh session starts. Set `gateway.persist_sessions: false` to opt out and start fresh sessions on every restart.

The `gateway.platforms` map currently understands three keys: `telegram`, `discord`, `slack`. Each platform section shares the same `PlatformGatewayConfig` shape; per-platform fields are noted below. Missing or unconfigured sections **silently skip** at gateway startup — existing Telegram-only deployments are unchanged.

**Telegram platform defaults** (under `gateway.platforms.telegram:`):

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Master toggle (currently informational; presence of resolved token is the actual gate) |
| `token` | `null` | Bot token. Falls back to `TELEGRAM_BOT_TOKEN` env var |
| `whitelist` | `[]` | Allowed Telegram chat IDs (`Vec<i64>`). Empty = deny all (D-12) |
| `session_timeout_hours` | `24` | Session inactivity timeout in hours |
| `max_concurrent_runs` | `8` | Maximum concurrent agent runs |

**Discord platform** (under `gateway.platforms.discord:`) — added in Phase 34:

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Master toggle |
| `token` | `null` | Bot token. Falls back to `DISCORD_BOT_TOKEN` env var (Discord-specific — does NOT pick up `TELEGRAM_BOT_TOKEN`) |
| `whitelist` | `[]` | Allowed Discord user IDs (`Vec<i64>`). Empty = deny all |
| `session_timeout_hours` | `24` | Session inactivity timeout in hours |
| `max_concurrent_runs` | `8` | Maximum concurrent agent runs |

Requires the **MESSAGE_CONTENT** privileged gateway intent (toggled in the Discord developer portal — see [MULTI-PLATFORM-GATEWAY.md](MULTI-PLATFORM-GATEWAY.md)). Built on serenity 0.12.5.

**Slack platform** (under `gateway.platforms.slack:`) — added in Phase 34:

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Master toggle |
| `token` | `null` | Bot token `xoxb-…`. Falls back to `SLACK_BOT_TOKEN` env var |
| `app_token` | `null` | App-level token `xapp-…` for Socket Mode. Falls back to `SLACK_APP_TOKEN` env var. **Slack adapter is silently skipped unless BOTH `app_token` and `token` resolve** (Pitfall 2 — two-token shape) |
| `whitelist` | `[]` | Allowed Slack channel/user IDs (`Vec<i64>` — see caveat below). Empty = deny all |
| `session_timeout_hours` | `24` | Session inactivity timeout in hours |
| `max_concurrent_runs` | `8` | Maximum concurrent agent runs |

Uses slack-morphism 2.22.0 Socket Mode (WebSocket, no public HTTP endpoint required). Built on the `axum` feature flag which transitively activates `hyper-base`/`tokio-tungstenite`.

> **Slack whitelist caveat (deferred):** Slack channel IDs are alphanumeric (`C123ABC`, `D456DEF`) but the shared `PlatformGatewayConfig.whitelist` is typed `Vec<i64>` (Telegram-shaped). The adapter converts via `to_string()` at the boundary, so numeric entries you place in `whitelist` are compared as strings and will not match real Slack IDs. A schema upgrade to `Vec<String>` is tracked as a future config-schema improvement.

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

### Delegation (`delegation:`)

| Key | Default | Description |
|---|---|---|
| `delegation.child_timeout_seconds` | `300` | Timeout per child agent execution in seconds |
| `delegation.max_concurrent_children` | `3` | Maximum concurrent children per batch (oversize batches return a tool error) |
| `delegation.max_iterations` | `20` | Maximum LLM iterations per child agent (per-call `max_iterations` overrides; lowered from 50 to bound cost when a child loops on tool errors) |
| `delegation.max_spawn_depth` | `1` | Maximum spawn depth for `orchestrator`-role children (1 = flat, no nesting) |
| `delegation.orchestrator_enabled` | `true` | Global kill switch; when `false`, all children downgrade to `leaf` regardless of per-call `role` |
| `delegation.default_toolsets` | `["terminal", "file", "web"]` | Default toolset groups when none are specified per-call |
| `delegation.model` | `null` | Model override for children (null = inherit parent's model) |
| `delegation.provider` | `null` | Provider override for children (null = inherit parent's provider) |
| `delegation.base_url` | `null` | API base URL override for children (null = inherit parent's) |

> The legacy `subagent:` key, `max_subagents`, and `timeout_secs` were renamed in Phase 32.2. See [DELEGATION.md](DELEGATION.md) for the per-call `role` / `max_iterations` schema, the `/agents` tree view, and a full migration guide.

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
| `extract.per_url_timeout_secs` | `60` | Phase 41.3 D-16: per-URL deadline inside a multi-URL `web_extract` batch. A URL that exceeds this budget yields an in-array `extraction_timeout` error entry at its own index instead of hanging the whole call — the direct fix for a single bot-walled homepage sinking a 6-URL batch indefinitely. See [Web Tools](#web-tools-toolsweb_search--toolsweb_answer--toolsweb_extract) for how this relates to the tool-level timeout below. |

### Web Tools (`tools.web_search` / `tools.web_answer` / `tools.web_extract`)

Phase 41.3. Three tools share one operator surface: **`web_search`** (a ranked results list — title/url/snippet), **`web_answer`** (a synthesized, cited prose answer), and **`web_extract`** (full page content for one or more URLs). The model picks `web_search` vs. `web_answer` by intent — they are separate tools, not a `mode` switch on one tool.

**Provider chains (D-08).** Each tool tries its configured providers **in the order listed**, skipping any provider whose key is not configured and falling through to the next on a failed call. An unknown provider name is not silently ignored — it is a configuration error `hermes doctor` reports, naming both the bad value and the key path (`tools.web_search.chain`, etc.), so a typo is caught before a turn silently falls through the whole chain.

| Key | Default | Legal values |
|---|---|---|
| `tools.web_search.chain` | `[exa, brave, tavily, ddg]` | `exa`, `brave`, `tavily`, `ddg` |
| `tools.web_answer.chain` | `[perplexity, exa, brave, ddg]` | `perplexity`, `exa`, `brave`, `ddg` |
| `tools.web_extract.chain` | `[firecrawl, exa, tavily, local]` | `firecrawl`, `exa`, `tavily`, `local` |

```yaml
tools:
  web_search:
    chain: ["exa", "brave", "tavily", "ddg"]
  web_answer:
    chain: ["perplexity", "exa", "brave", "ddg"]
  web_extract:
    chain: ["firecrawl", "exa", "tavily", "local"]
```

**Both `web_search` and `web_answer` work with zero API keys.** Every chain's default order terminates in a keyless provider — `ddg` (DuckDuckGo) for `web_search`/`web_answer`, `local` (in-process HTML extraction) for `web_extract` — so neither tool is ever hidden or hard-fails on a fresh install (D-09). Configuring `EXA_API_KEY`/`TAVILY_API_KEY`/`BRAVE_API_KEY`/`PERPLEXITY_API_KEY`/`FIRECRAWL_API_KEY` only promotes a higher-quality provider earlier in the chain it belongs to — it is never a prerequisite for using the tool at all. `hermes doctor` reports this as an N-of-M count per tool, e.g. `web_search: 1/3 providers configured`, and explicitly notes when a `0/N` tool remains available via its keyless default (a `0/N` reading as "broken" would be wrong).

`hermes setup` asks about this once per tool's provider group — one "pick a provider" prompt listing the interchangeable options, not one prompt per key — and declining leaves the tool available exactly as above.

**Tool execution timeout (D-04/D-05/D-06/D-15).** Every tool call is bounded by a wall-clock timeout, resolved in this precedence order (highest first):

1. `tools.timeout_overrides[<tool_name>]` — operator, per-tool. **Wins even over a tool's own code-level opt-out.**
2. The tool's own declared budget (code-level, not operator-configurable).
3. `tools.timeout_secs` — operator, global default.
4. The built-in trait-default constant.

A configured value of **`0` or below disables the bound** for that tool — this applies both to a `tools.timeout_overrides` entry and to `tools.timeout_secs` itself. On expiry the tool's execution is hard-cancelled (the underlying process, if any, is reaped) and the model sees a normal tool-error result, not a hang.

| Key | Default | Description |
|---|---|---|
| `tools.timeout_secs` | `60` | Global default wall-clock bound (seconds) for any tool that does not declare its own budget. `<= 0` disables the bound globally for non-declaring tools. |
| `tools.timeout_overrides` | `{}` | Per-tool override map (`{tool_name: seconds}`). A value `<= 0` disables the bound for that specific tool, overriding even a code-level opt-out. |

```yaml
tools:
  timeout_secs: 60
  timeout_overrides:
    web_extract: 120   # a slow multi-URL batch gets more room
    # some_tool: 0     # disable the bound for some_tool entirely
```

`extract.per_url_timeout_secs` (above) is a *different, smaller-scoped* deadline — it bounds each URL **individually** inside one `web_extract` call, so one unresponsive site yields an `extraction_timeout` entry for just that URL while the rest of the batch keeps going. `tools.timeout_overrides[web_extract]` (or the tool's own code-level budget) is the *outer* ceiling on the whole call; it should stay large enough that a full batch completes inside it under the per-URL deadline, since it is a backstop that should never fire in ordinary operation.

### Tool Credentials (`tools.credentials` — env → config → vault)

Phase 41.3 D-18/D-19. The five web-tool provider keys (`FIRECRAWL_API_KEY`, `EXA_API_KEY`, `TAVILY_API_KEY`, `BRAVE_API_KEY`, `PERPLEXITY_API_KEY`) — and any future tool credential that adopts the same mechanism — can live in **three tiers**, resolved once at startup in this precedence order. An earlier tier's hit ends resolution for that key; a later tier is never even consulted once an earlier one is satisfied.

1. **Process environment** (`.env` or the real shell environment) — highest precedence, and the only tier resolved *live* (re-checked on every query, not cached).
2. **`tools.credentials.<CANONICAL_ENV_VAR_NAME>`** in `config.yaml` — the tier this section documents. Keyed by the exact canonical env-var name (`EXA_API_KEY`), never a `tools.`-prefixed variant, so one key name works identically across all three tiers.
3. **The vault** (see [Vault](#vault-vault)) — consulted last, only when `vault.enabled: true`, and keyed by that same canonical env-var name.

```yaml
tools:
  credentials: {}
  # credentials:
  #   EXA_API_KEY: "your-key-here"   # example only — do not commit a real key
```

| Key | Default | Description |
|---|---|---|
| `tools.credentials` | `{}` | The config tier of the precedence chain above. Empty by default. |

**This tier stores the secret in plaintext on disk**, in `config.yaml`. Anyone who can read that file can read a value stored here. The process environment or the vault (`vault.enabled: true`) is the preferred home for a real credential — this map exists for operators who cannot use either, and the tradeoff is theirs to make, not free.

**A sealed or unreachable vault stops the agent from starting.** This is deliberate, not a bug: when `vault.enabled: true`, tool-credential resolution treats a sealed, locked, or corrupt vault the same way `ProviderResolver`'s inference-key vault fallback already does (D-07) — a loud startup error, not a silently degraded, keyless agent. The accepted consequence is that an operator who enables the vault but never intended to use a vault-backed *tool* credential that session can still be stopped from booting by it. `hermes doctor` is the one place that condition can be diagnosed **without** booting the agent — it resolves the identical env → config → vault snapshot the runtime does, and on a sealed/unreachable vault it renders a failed check naming the backend and keeps going (never dies on the exact condition it exists to diagnose, and never prints any part of a credential — only env-var names, tiers, and counts).

### Kanban Worker Profile Credentials (`profiles/<name>/.env`)

A kanban worker profile keeps its provider key in its own `.env` at
`$IRONHERMES_HOME/profiles/<name>/.env`, mode `0600`. This is the **only** channel by
which a dispatched worker obtains a credential: workers are spawned with
`.env_clear()` plus a 7-variable allowlist that deliberately excludes every
`*_API_KEY` and `*_SECRET`, so the worker bootstraps its own key by reading that file.

**Values are single-quoted, and the writer verifies its own output.** Every value is
written strong-quoted, and the renderer parses its rendered bytes back through the real
`dotenvy` reader before anything reaches disk — a write whose round-trip does not
reproduce the exact input is **refused**, not written. This matters because `dotenvy`'s
*unquoted* grammar is not "the rest of the line": `${NAME}` is a substitution directive
resolved from the reading process's environment first, an unescaped space ends the
value, and a trailing `#` silently truncates it. Quoting makes all of those inert.

**If you write one of these files by hand, single-quote the values.** An unquoted
`${VAR}` will be substituted from whatever process reads the file next.

#### Checking for credential exposure

`ironhermes doctor` reports a **Profile credential exposure (CR-03 window)** check. It
exists because a defect fixed in phase 47.4 allowed an unquoted `${VAR}` in a profile
`.env` to be dereferenced against the server's own environment — which has the root
`.env` loaded — and the resulting plaintext root credential to be persisted into that
profile's file. Profiles written before that fix may still carry the result.

The check reports two things, by **key name only — never a value**:

| Reported | Meaning |
|---|---|
| `value contains a live ${...} substitution` | A raw, unquoted `${...}` is still on disk. It re-resolves on *every* read, so a dispatched worker resolves the referenced credential at runtime. Still live. |
| `value is identical to the root .env's <KEY>` | This profile key's value matches a root key stored under a **different** name — the fingerprint of a substitution that already landed on disk. |

A key inherited from the root `.env` under the **same** name is *not* flagged. That is
normal inheritance, not exposure.

```
  [EXPOSED] Profile credential exposure (CR-03 window)
  profile "worker-a":
      OPENROUTER_API_KEY: value is identical to the root .env's ANTHROPIC_API_KEY
                          (different name — consistent with a CR-03 dereference)

  ROTATE the affected root credentials. Deleting or editing these files does NOT
  remediate a credential that has already been disclosed to disk.
```

**On a hit, rotate the affected root credential.** Editing or deleting the profile
`.env` does not remediate anything — the secret was already written to disk in
plaintext, and must be assumed disclosed. Re-issue it at the provider, then update the
root `.env` and any profile that legitimately needs it.

**A clean result means "no fingerprint found", not "you were not compromised."** The
check compares against the root `.env` only, and a profile with a still-live `${...}`
is reported under that heading and skipped for the value comparison (reading it would
itself resolve the substitution) — fix the live vector and re-run to get the second
pass.

### Autonomous Mode (`autonomous:`)

| Key | Default | Description |
|---|---|---|
| `autonomous.yolo` | `false` | When `true`, skip dangerous-command approval prompts. Budget-100% / fatal-error / user-interrupt remain unskippable (G-01/G-04/G-09). The CLI `--yolo` flag overrides this config value when both are set. |

```yaml
autonomous:
  yolo: false   # set true to suppress approval prompts
```

### Concurrency (`concurrency:`)

| Key | Default | Description |
|---|---|---|
| `concurrency.session_turn_cap` | `3` | Maximum concurrent in-flight turns per session (D-03). Messages beyond this cap queue in FIFO order. |
| `concurrency.global_turn_ceiling` | `32` | Process-wide maximum concurrent turns across all sessions and surfaces (D-04). Protects the host regardless of active conversation count. |

Pre-39.1 configs parse cleanly via `#[serde(default)]` — omitting this block applies the defaults above.

```yaml
concurrency:
  # session_turn_cap: 3
  # global_turn_ceiling: 32
```

### Image & Video Generation (`image_gen:` / `video_gen:` / `generation:`)

**Phase 47 made generation multi-provider.** The generation tools — `image_gen` (text→image), `video_generate` (t2v), `video_animate` (i2v), and the net-new `video_to_video` (v2v) — are provider- and model-configurable **per mode**. Two backends ship: **venice.ai (the default)** and **fal.ai**.

**A tool is exposed to the LLM only when BOTH hold:** (1) the RESOLVED backend's key is set — `VENICE_API_KEY` for venice modes, `FAL_KEY` for fal modes (see [Tool API Keys](#tool-api-keys)); and (2) the surface is enabled — for chat, the `web` toolset must be on (`tools.toolsets.web.enabled: true`; the generation tools are members of the `web` toolset); for kanban/delegate see `generation.guardrails.surfaces`. All three blocks are optional and pre-existing configs parse cleanly.

**Provider resolution per mode:** `provider: "venice" | "fal" | null`. When `null`, the backend is inferred from the model id prefix (`fal-ai/*` → fal, else venice). The legacy flat keys (`default_model`, `default_t2v_model`, `default_i2v_model`) are kept for back-compat — set one **away** from its shipped fal default and it maps into that mode's `model` (with provider forced to prefix inference); leave it at the fal default and the venice per-mode default wins. (Practical consequence: a pre-Phase-47 config that never overrode these keys now resolves its four modes to the **venice** defaults below.)

#### `image_gen:`

| Key | Default | Description |
|---|---|---|
| `image_gen.default_model` | `fal-ai/flux/schnell` | Legacy flat fal model id (back-compat). LLM-overridable per call. |
| `image_gen.session_cap` | `20` | Per-chat-session cap on DIRECT chat generations. At the cap, returns a non-retried message and never calls the provider. Config-only. (Chat only — delegate/kanban descendants use `generation.guardrails`.) |
| `image_gen.timeout_secs` | `120` | Provider poll timeout (seconds) for a single queue job. Config-only. |
| `image_gen.t2i.provider` | `venice` | Backend for text→image: `venice` \| `fal` \| null(=infer from model prefix). |
| `image_gen.t2i.model` | `flux-2-pro` | Model id passed to the resolved backend. |
| `image_gen.steps` | `20` | Venice diffusion sampling-step count, sent on every venice image request. **Required for a finished image** — Venice does not apply the model's own `constraints.steps.default` when omitted, so too low a value returns an under-denoised "blob". Raise toward the model max (flux-2-pro / krea-2-turbo = 50) for more detail; `0` omits the field (Venice server-side fallback). Config-only; ignored by fal models. |
| `image_gen.safe_mode` | `false` | Venice `safe_mode` — `true` blurs adult-flagged output, `false` disables the blur (sensible for a single-user host). Config-only; Venice-only (fal ignores it). |

#### `video_gen:` (t2v / i2v / net-new v2v)

| Key | Default | Description |
|---|---|---|
| `video_gen.default_t2v_model` | `fal-ai/ltx-2.3/text-to-video` | Legacy flat fal t2v model (back-compat). |
| `video_gen.default_i2v_model` | `fal-ai/ltx-2.3/image-to-video` | Legacy flat fal i2v model (back-compat). |
| `video_gen.session_cap` | `5` | Per-session cap on DIRECT chat video generations (paid; lower than image). Config-only. |
| `video_gen.timeout_secs` | `300` | Provider poll timeout (seconds) — video is slower than image. Config-only. |
| `video_gen.max_inline_bytes` | `52428800` | Max inline delivery size (bytes); 50 MiB Telegram sendVideo cap. |
| `video_gen.default_duration_secs` | `6` | Default clip duration (seconds). |
| `video_gen.t2v` / `.i2v` / `.v2v` | `venice` / `wan-2-7-{text,image,video}-to-video` | Per-mode `{provider, model}`. `v2v` is net-new (no legacy key). |
| `video_gen.resolution` | `720p` | Fixed-from-config video resolution (D-12). |
| `video_gen.aspect_ratio` | `16:9` | Fixed-from-config aspect ratio (D-12). |
| `video_gen.progress_ping_secs` | `30` | Periodic "still working…" ping cadence (seconds) during the async poll. `0` = off. |

#### `generation.guardrails:` (cross-surface spend policy, D-07/D-08)

**Applies ONLY to delegate/kanban descendants** — direct chat stays governed by `image_gen.session_cap` / `video_gen.session_cap`. A descendant generation decrements BOTH its own `per_child_cap` allowance AND the shared `session_pool`. Cross-process safe: in-process delegate children share the parent's counter; kanban swarms account via `kanban.db` keyed by the root task id.

| Key | Default | Description |
|---|---|---|
| `generation.guardrails.session_pool` | `20` | Shared aggregate cap across ALL delegate/kanban descendants of a root. `0` = block immediately. |
| `generation.guardrails.per_child_cap` | `3` | Per-child sub-cap so one descendant can't drain the whole pool. `0` = block. |
| `generation.guardrails.surfaces.chat` | `true` | Direct chat generation (never subject to `per_child_cap`). |
| `generation.guardrails.surfaces.kanban` | `true` | Regular (non-goal-mode) kanban worker generation. |
| `generation.guardrails.surfaces.kanban_goal_mode` | `false` | Kanban goal-mode worker generation (opt-in). |
| `generation.guardrails.surfaces.delegate` | `false` | `delegate_task` children — the `"generation"` toolset group, opt-in, fail-closed. |

```yaml
image_gen:
  t2i: { provider: "venice", model: "flux-2-pro" }   # or { provider: fal, model: fal-ai/flux/schnell }
video_gen:
  t2v: { provider: "venice", model: "wan-2-7-text-to-video" }
  i2v: { provider: "venice", model: "wan-2-7-image-to-video" }
  v2v: { provider: "venice", model: "wan-2-7-video-to-video" }
  # resolution: "720p"
  # aspect_ratio: "16:9"
generation:
  guardrails:
    # session_pool: 20
    # per_child_cap: 3
    surfaces:
      delegate: false   # set true to opt-in generation for delegate_task children

# generation tools live in the `web` toolset — enable it to expose them in chat:
tools:
  toolsets:
    web:
      enabled: true
```

### Audio Cache (`audio_cache:`)

| Key | Default | Description |
|---|---|---|
| `audio_cache.max_age_days` | `7` | Maximum age in days for cached audio files under `$IRONHERMES_HOME/audio_cache/`. Files older than this are removed by `gc_sweep_audio_cache` on startup and on every periodic sweep. |
| `audio_cache.sweep_interval_secs` | `86400` | Periodic GC sweep interval in seconds (default: daily). |

```yaml
audio_cache:
  # max_age_days: 7
  # sweep_interval_secs: 86400
```

### Tools (`tools:`)

Toolsets enabled by default via `ToolsConfig::default()`: `memory`, `session`, `agent`, `skills`, `robotics`, `learning`. All known toolsets are additionally ensured present via `with_default_toolsets_merged()`, which iterates over `crate::constants::ALL_TOOLSETS`.

Toolsets disabled by default: `web`, `code`, `browser`

| Toolset | Members | Notes |
|---|---|---|
| `memory` | `memory` | Persistent memory tool (`MEMORY.md` / `USER.md`) |
| `session` | `session_search`, `session_recent` | Search the current/past session transcripts |
| `agent` | `delegate_task` | Spawn subagents |
| `skills` | discovery + invocation tools | Read existing skills from `skills/` paths |
| `robotics` | hexapod / robot tools | Gates further on `HEXAPOD_IP` env var |
| `learning` | `skill_manage` | **Phase 33** — autonomous skill creation (see [Autonomous Skill Creation](#autonomous-skill-creation-learning-toolset)) |
| `web` | `web_read`, `web_extract` | Opt-in; web scraping |
| `code` | `execute_code` | Opt-in; Python sandbox |
| `browser` | `browser_*` | Opt-in; headed browser automation |

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
    learning:
      enabled: true   # Phase 33 — autonomous skill creation
    web:
      enabled: false   # opt-in required
    code:
      enabled: false   # opt-in required
    browser:
      enabled: false   # opt-in required
```

### Vault (`vault:`)

Phase 46.8. An optional operator secret store for provider API keys, consulted as the **last** step in `ProviderResolver`'s key-resolution chain — after `api_key_env`, the deprecated inline `api_key` literal, legacy provider-specific env vars, and the deprecated `model.api_key`. The vault is never consulted for a provider whose key already resolved via one of those four; it only fills gaps. Disabled by default — omitting the `vault:` block entirely (or leaving `enabled: false`) is **byte-for-byte the same behavior as pre-46.8 configs**.

| Key | Default | Description |
|---|---|---|
| `vault.enabled` | `false` | Master switch. When `true`, `ProviderResolver::apply_vault_fallback` consults the configured `backend` for any provider endpoint still missing an `api_key` after the existing 4-priority chain. |
| `vault.backend` | `env-var` | Secret-store backend: `env-var` (always-on diagnostic store, no build feature required) or `rusty-vault` (embedded encrypted vault; requires the crate's `rusty-vault` cargo feature). |
| `vault.rusty_vault.data_dir` | `""` (empty) | On-disk directory for the embedded RustyVault store. Empty is a sentinel resolved at runtime to `$IRONHERMES_HOME/vault` (i.e. `~/.ironhermes/vault` by default); set an explicit absolute path to override. Only consulted when `backend: rusty-vault`. |
| `vault.rusty_vault.unseal_mode` | `keyfile` | Unseal strategy for the RustyVault backend: `keyfile` (a `0600` keyfile written beside the data dir auto-unseals on every open — no prompt) or `passphrase` (prompts for the unseal key on `vault unlock`, masked, never accepted as an argv token). |

```yaml
vault:
  enabled: true
  backend: rusty-vault        # env-var | rusty-vault
  rusty_vault:
    # data_dir: ""            # empty = $IRONHERMES_HOME/vault
    unseal_mode: keyfile      # keyfile | passphrase
```

**`env-var` backend.** The default backend. It is a **read-only diagnostic store** — `get_secret` reads directly from the process environment (no new `IRONHERMES_*` naming scheme is invented; the real per-provider env-var resolution already lives in `ProviderResolver`). Write operations (`vault set`, `vault init`, `migrate`'s vault-write step) hard-error against this backend ("EnvVarStore is read-only diagnostic"). Use it only to exercise the vault fallback path end-to-end without standing up a real store.

**`rusty-vault` backend requires a build feature.** Selecting `backend: rusty-vault` without compiling the binary with `--features rusty-vault` causes every vault operation to hard-error naming the missing feature, rather than silently falling back to `env-var`:

```bash
cargo build --release --features rusty-vault -p ironhermes-cli
```

<!-- VERIFY: exact crate/workspace flags your build/CI pipeline uses to enable the rusty-vault feature -->

**Key resolution precedence.** With `vault.enabled: true` and a healthy (unsealed) backend, a provider's API key resolves in this order — the vault only ever fills a gap, never overrides an earlier hit:

1. `providers.<name>.api_key_env` (env var referenced by name)
2. Deprecated inline `providers.<name>.api_key` literal
3. Legacy provider-specific env vars (e.g. `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`)
4. Deprecated `model.api_key`
5. **Vault** — `store.get_secret(<provider-name>)` against the configured `backend`

A sealed or unreachable `rusty-vault` backend hard-errors loudly at this step rather than silently leaving the key unresolved; a healthy vault that simply lacks the key is not an error — resolution falls through to the existing "no API key configured" error.

**`ironhermes vault` CLI subcommands** (operator-only — no chat/agent tool ever reaches the vault):

| Command | Description |
|---|---|
| `ironhermes vault init` | Create a fresh encrypted vault at `vault.rusty_vault.data_dir` (rusty-vault backend only; requires the `rusty-vault` feature). Writes a `0600` keyfile holding the unseal key + root token beside the data dir. Key material is never printed. |
| `ironhermes vault unlock` | Unseal an initialized vault. No-op status confirmation under `unseal_mode: keyfile` (auto-unseals on open); prompts for the unseal key (masked) under `unseal_mode: passphrase`. |
| `ironhermes vault set <key>` | Store (create or overwrite) a secret under `<key>` (the provider name, e.g. `openrouter`). The value is always read from a masked TTY prompt or piped stdin — never a bare argv token. |
| `ironhermes vault list [--prefix <p>]` | List secret key **names** only, sorted, optionally prefix-filtered. Never prints values. |
| `ironhermes vault migrate` | Import provider API keys out of `$IRONHERMES_HOME/.env` into the vault. Writes a `0600` timestamped backup of the full original `.env` first, then vault-writes each matched provider key, then scrubs from `.env` only the keys that were written successfully — a failed vault-write leaves its `.env` line untouched. Scope is provider API keys only; platform/gateway/telegram tokens and profile-scoped `.env` values are never touched. |

Every `vault` subcommand appends one audit entry (`vault_init`/`vault_unlock`/`vault_set`/`vault_list`/`vault_migrate`) recording only `{"success": bool}` — never a secret value.

> **Full-host migration:** `vault migrate` covers `.env` only — it never touches inline
> `api_key:` literals in `config.yaml`, which are precedence #1 and mask the vault while
> they exist. `scripts/deploy/vault-migrate.sh` wraps the whole hardening flow (config
> `vault:` block → `init` → `migrate` → move + null inline provider keys → verify →
> backup cleanup); step-by-step in [DEPLOYMENT.md](DEPLOYMENT.md#vault-migratesh--what-it-does-step-by-step).

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

### Per-Provider Extra Request Options (PROV-11..PROV-14)

Each provider entry under `providers:` may set `extra_request_options` — a free-form map merged into the outgoing chat-completions request body at the wire level. This lets you tune small-model knobs (Ollama `num_ctx`, vLLM `top_k`, OpenRouter `provider.order`) without code changes.

Per-model overrides under `providers.<name>.models.<model>.extra_request_options` win on a per-key basis (provider-level keys are kept; the per-model entry adds or overrides individual keys).

```yaml
providers:
  ollama:
    base_url: "http://localhost:11434/v1"
    api_mode: chat_completions
    # Provider-level defaults — applied to every request to this provider
    extra_request_options:
      num_ctx: 8192          # default 8K context window
      num_predict: 512
    # Per-model overrides — merged per-key on top of provider-level defaults
    models:
      "llama3.1:8b":
        extra_request_options:
          num_ctx: 32768     # bump just this model to 32K
      "qwen2.5-coder:7b":
        extra_request_options:
          top_k: 40

  vllm:
    base_url: "http://localhost:8000/v1"
    api_mode: chat_completions
    extra_request_options:
      top_k: 40
      top_p: 0.95

  openrouter:
    api_key_env: OPENROUTER_API_KEY
    extra_request_options:
      provider:
        order: ["anthropic", "openai"]    # OpenRouter routing preference
```

**Merge semantics:** `resolve_extras(providers, provider_name, model_name)` clones the provider-level map, then inserts each entry from the per-model map on top. Provider-level keys that are not overridden remain.

**Caller wins:** Code paths that explicitly set fields on `ChatRequest` (e.g. `extra` arg supplied by `call_llm`) still override anything sourced from config — `extra_request_options` is the floor, not a hard override.

**Reserved-key caveat (T-36.15-09 open):** Keys that collide with named `ChatRequest` fields (`model`, `messages`, `stream`, `temperature`, `max_tokens`, `tools`) currently shadow the named field via `#[serde(flatten)]`. A blocklist filter is planned in a follow-up phase. Treat reserved keys as undefined behavior for now.

**CLI surface:** `hermes config show` exposes the canonical extras keys (`num_ctx`, `num_predict`, `top_k`, `top_p`, `provider.order`); arbitrary additional keys are allowed but not listed in the schema.

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
