# Tools & Toolsets Gap Research

**Date:** 2026-04-10
**Context:** Comparison of IronHermes tools against the Hermes reference spec, with solutions research for gaps and out-of-scope items.

---

## 1. Toolset Runtime Filtering (TOOL-01)

**Gap:** IronHermes has `toolset()` on the Tool trait but no mechanism to enable/disable toolsets at runtime.

### Recommended Solution

**Filter at dispatch time, not registration time.** Add `enabled_toolsets: Option<HashSet<String>>` to `ToolRegistry`. Filter in both `get_definitions()` (so disabled tools don't appear in LLM schema) and `dispatch_with_hook()` (belt-and-suspenders guard).

**Platform presets** in `config.yaml`:
```yaml
toolsets:
  enabled: ["web", "file", "system"]
  presets:
    minimal: ["file"]
    full: ["web", "file", "system", "memory", "code"]
```

**CLI flags:**
```
--toolsets web,terminal    # explicit list
--preset minimal           # named preset
```

Precedence: `--toolsets` > `--preset` > `config.yaml` > all enabled. Make `--toolsets` and `--preset` mutually exclusive via clap `conflicts_with`.

**Not a guardrail.** Toolset filtering is a capability gate (controls what the LLM sees), not a security check. Guardrails intercept calls the agent already chose to make.

**Effort:** Small — extend existing `ToolRegistry` and `Config`, add CLI args.

---

## 2. Background Process Management (TOOL-02)

**Gap:** Terminal tool runs commands synchronously. No background execution or process lifecycle management.

### Recommended Solution

Create `ProcessRegistry` with `ProcessEntry` structs:
```rust
pub struct ProcessEntry {
    child: tokio::process::Child,
    output_buf: Arc<RwLock<Vec<u8>>>,
    stdin: Option<ChildStdin>,
    session_id: Uuid,
}
```

- **Spawn:** `Command::new().spawn()` (not `.output()`), return session_id immediately
- **Output buffering:** Spawn tokio reader task per process, append to `Arc<RwLock<Vec<u8>>>`
- **stdin:** Take from child immediately, store in entry, write on demand
- **Lifecycle ops:** poll (`try_wait`), wait (`.wait().await`), log (read buffer), kill (`start_kill` + timeout + SIGKILL)
- **Cleanup:** `Drop` on ProcessRegistry calls `start_kill()` on all entries; async shutdown via `CancellationToken` for graceful cleanup

**PTY support:** `portable-pty` crate (maintained, macOS/Linux/Windows). It's synchronous — bridge to tokio with `spawn_blocking`. Only needed for interactive programs; plain piped process is the default.

**Effort:** Medium — new `ProcessRegistry` module + `ProcessTool` with 6 actions.

---

## 3. Terminal Backend Abstraction (TOOL-03, TOOL-04, TOOL-05)

**Gap:** Local-only terminal via `tokio::process::Command`. No docker, ssh, or other sandbox backends.

### Recommended Solution

**Trait design** — separate execution from lifecycle:
```rust
#[async_trait]
pub trait TerminalBackend: Send + Sync {
    async fn execute(&self, command: &str, env: &[(&str, &str)], timeout: Duration) -> Result<CommandOutput>;
}

#[async_trait]
pub trait ContainerLifecycle: Send + Sync {
    async fn start(&mut self, config: &ContainerConfig) -> Result<()>;
    async fn stop(&mut self) -> Result<()>;
}
```

Lifecycle is separate because SSH and Modal have no teardown; Docker and Daytona do.

### Docker Backend (TOOL-04)

**Crate:** `bollard` v0.17+ — the definitive async Docker crate. Native Rust, Hyper + Tokio, no C deps.

- `create_exec` + `start_exec` for command execution
- Security hardening via `HostConfig`: `readonly_rootfs`, `cap_drop: ["ALL"]`, `pids_limit`, `memory`, `security_opt: ["no-new-privileges:true"]`, `network_mode: "none"`
- Resource config: `nano_cpus`, `memory`, volume mounts for persistence
- Production-grade maturity, tracks Moby API schema v1.52+

### SSH Backend (TOOL-05)

**Recommended crate:** `russh` (pure Rust, native tokio) or `async-ssh2-russh` (higher-level wrapper with separate stdout/stderr streams).

- Key auth, password auth, agent forwarding
- Channel-per-command model, output streaming
- No container lifecycle needed

**Avoid:** `ssh2` (synchronous/libssh2 C bindings), `async-ssh2-tokio` (too simple for streaming).

### Other Backends (out of scope, but viable if needed)

| Backend | Approach | Crate/Method |
|---------|----------|-------------|
| Singularity | CLI wrap via `tokio::process::Command` calling `apptainer exec` | No native bindings exist |
| Modal | REST API via reqwest + sandbox connect tokens | No Rust SDK; REST is supported path |
| Daytona | `daytona-client` v0.5 crate (Rust, full REST API) | Workspace Toolbox API for exec |

**Effort:** Large — new crate or module, trait abstraction, bollard + russh integration, config extensions.

---

## 4. send_message Tool (TOOL-06)

**Gap:** Agent cannot initiate outbound messages. Cron delivery routing exists but isn't exposed as a tool.

### Recommended Solution

Wrap existing delivery infrastructure from `ironhermes-gateway/src/runner.rs` (delivery routing for cron output) into a standalone `SendMessageTool`. The delivery routing already supports Telegram chat, CLI stdout, and webhook URL targets.

```rust
pub struct SendMessageTool {
    delivery_router: Arc<DeliveryRouter>,
}
// Parameters: target (telegram/webhook/cli), message, optional chat_id/url
```

**Effort:** Small — the infrastructure already exists in cron delivery; just needs a Tool wrapper.

---

## 5. session_search Tool (TOOL-07)

**Gap:** No way to search past conversation history.

### Recommended Solution

Query the existing `SessionStore` (which already persists conversation history per chat_id). Add a search method that iterates stored messages, matching by keyword with optional date range filtering.

```rust
pub struct SessionSearchTool {
    session_store: Arc<RwLock<SessionStore>>,
}
// Parameters: query (keyword), date_from, date_to, limit
// Returns: matching messages with session_id, timestamp, role, content snippet
```

**Effort:** Small-medium — depends on SessionStore's current persistence format and indexing.

---

## 6. MCP Server Integration (TOOL-08)

**Gap:** No dynamic tool loading from MCP servers.

### Recommended Solution

**Use `rmcp` v1.3.0** — the official Anthropic/MCP org Rust crate. Tokio-native, handles JSON-RPC framing, initialize handshake, tools/list, tools/call, stdio + HTTP transports.

```toml
rmcp = { version = "1", features = ["client", "transport-child-process"] }
```

**Architecture:**
1. `McpServerConfig` in config.yaml (command, args, env per server)
2. On startup: spawn each server, run initialize + tools/list
3. Create `McpToolProxy` per discovered tool, implementing the `Tool` trait
4. Register into `ToolRegistry` with `toolset = "mcp-{server_name}"`

**Key design:**
```rust
pub struct McpToolProxy {
    server_name: String,
    tool_name: String,
    description: String,
    input_schema: serde_json::Value,
    client: Arc<McpClientHandle>,
}
```

The `Tool` trait's `schema()` returns an owned value (not `&'static`), so runtime schema construction from MCP's JSON Schema works cleanly.

**Config format** (adapted from Claude Desktop):
```yaml
mcp:
  servers:
    github:
      command: "npx"
      args: ["-y", "@modelcontextprotocol/server-github"]
      env:
        GITHUB_PERSONAL_ACCESS_TOKEN: "${GITHUB_TOKEN}"
```

**Security:**
- Namespace tools as `mcp-{server}-{tool}` to prevent collisions with built-in tools
- MCP tools go through the same guardrail system as native tools
- Filter env vars passed to server processes
- Supervisor loop with backoff for crashed servers; `is_available() -> false` until reconnected

**Effort:** Medium-large — new module, rmcp integration, config, lifecycle management, proxy trait impl.

---

## 7. Browser Automation — Built-in with Chrome for Testing

**Gap:** Entire browser category missing (browser_navigate, browser_snapshot, browser_vision).

### Recommended Solution: Built-in Browser with Managed Chrome

Follow the Hermes `agent-browser install` pattern — IronHermes manages its own Chrome installation via Google's Chrome for Testing (CfT) builds, driven natively through CDP from Rust.

#### Chrome for Testing (CfT)

CfT is Google's dedicated, non-auto-updating Chrome build for automation. No system Chrome dependency.

**Version API:** `https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json`

Returns platform-specific download URLs for `linux64`, `mac-arm64`, `mac-x64`, `win64`. Downloads are zip archives from Google Cloud Storage.

#### Install Flow — CLI Subcommand

`ironhermes browser install` would:
1. Detect platform via `std::env::consts::{OS, ARCH}` → `mac-arm64`, `linux64`, etc.
2. Fetch latest stable CfT version from the JSON API
3. Download the zip via reqwest streaming (`bytes_stream()`)
4. Extract to `~/.ironhermes/browser/chrome-for-testing/{version}/` via the `zip` crate (in `spawn_blocking`)
5. Verify the binary works (`--version` check)
6. Write active version to `~/.ironhermes/browser/active-version`

**Platform binary paths:**
- macOS: `chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`
- Linux: `chrome-linux64/chrome`
- Windows: `chrome-win64/chrome.exe`

#### CDP Driver — chromiumoxide

**Crate:** `chromiumoxide` v0.7 with `tokio-runtime` feature. Full async/tokio, auto-generated CDP typed bindings, mature.

**Launch pattern:**
```rust
let config = BrowserConfig::builder()
    .chrome_executable(binary_path)
    .arg("--headless=new")
    .arg("--no-sandbox")
    .arg("--disable-gpu")
    .arg("--disable-dev-shm-usage")
    .build()?;
let (browser, mut handler) = Browser::launch(config).await?;
```

The `--headless=new` flag is required for Chrome 112+ (CfT stable is 147+).

#### Browser Tool Actions

Seven tool actions for the `browser` toolset:

| Action | chromiumoxide API | Description |
|--------|-------------------|-------------|
| `navigate(url)` | `browser.new_page(url).await` | Navigate to URL, wait for load |
| `snapshot()` | `page.evaluate("document.body.innerText")` | Get page text for LLM context |
| `screenshot()` | `page.screenshot(ScreenshotParams)` | Capture viewport as base64 PNG for vision |
| `click(selector)` | `page.find_element(sel).await?.click()` | Click an element by CSS selector |
| `type(selector, text)` | `page.find_element(sel).await?.type_str(text)` | Type into an input field |
| `evaluate(js)` | `page.evaluate(js).await` | Run arbitrary JavaScript, return result |
| `wait(selector)` | `page.find_element(sel).await` | Wait for element to appear in DOM |

#### New Dependencies

```toml
chromiumoxide = { version = "0.7", features = ["tokio-runtime"] }
zip = "2"
dirs = "5"  # cross-platform home_dir()
```

`reqwest`, `base64`, `serde_json`, `tokio`, `futures-util` are already in the workspace.

#### Architecture

- **New crate:** `ironhermes-browser` — CfT installer, Chrome lifecycle, CDP wrapper
- **Tool:** `BrowserTool` in `ironhermes-tools` with 7 actions, toolset `"browser"`
- **State:** Browser instance shared via `Arc<Mutex<Option<Browser>>>` — lazily launched on first tool call, or pre-launched if `--toolsets` includes `browser`
- **Cleanup:** Browser process killed on agent shutdown via `CancellationToken`

#### Fallback Options (if built-in is not desired)

| Option | Notes |
|--------|-------|
| **Browser MCP server** via TOOL-08 | Playwright MCP, Browserbase MCP, or `chrome-debug-mcp` (Rust). Zero native browser code in IronHermes. |
| **Steel.dev** (browser-as-a-service) | Open-source, self-hostable REST API. No local Chrome needed. |
| **spider_chrome** | Actively maintained chromiumoxide fork with higher concurrency features. Drop-in replacement. |

**Effort:** Medium — new crate for installer + browser lifecycle, BrowserTool with 7 actions, CLI subcommand.

---

## 8. Media Tools (currently Out of Scope)

**Gap:** No vision_analyze, image_generate, or text_to_speech tools.

### Possible Solutions

All cloud APIs below are callable with IronHermes's existing `reqwest` + `base64` dependencies. **Zero new crate dependencies needed for the cloud path.**

#### Vision / Image Analysis
| Provider | API | Notes |
|----------|-----|-------|
| **Anthropic Claude** (recommended) | Messages API with `image` content block | Already configured in IronHermes; zero-friction path |
| OpenAI GPT-4o | Chat completions with `image_url` content | Base64 or URL reference |
| Google Gemini | generateContent with inline parts | More verbose schema |

#### Image Generation
| Provider | API | Notes |
|----------|-----|-------|
| **OpenAI gpt-image-1** (recommended) | `/v1/images/generations` | Simplest REST. DALL-E 2/3 deprecated May 2026 |
| Stability AI SD 3.5 | `/v1/generation/{engine}/text-to-image` | More params, base64 response |
| HuggingFace Inference | `POST /models/{model}` | Free tier, open models |

#### Text-to-Speech
| Provider | API | Notes |
|----------|-----|-------|
| **OpenAI TTS** (recommended) | `/v1/audio/speech` | Single endpoint, returns MP3 bytes |
| ElevenLabs | `/v1/text-to-speech/{voice_id}` | Best voice quality, streaming support |
| Google Cloud TTS | `text:synthesize` | Requires GCP auth overhead |

**Architecture:** Individual tools (not a single "media" toolset), matching IronHermes's one-tool-per-capability pattern. Output to temp files under `~/.ironhermes/media/`, return absolute path as tool result.

**Recommendation:** Start with `vision_analyze` using Anthropic Claude (already configured). Then `image_generate` via OpenAI, and `text_to_speech` via OpenAI TTS.

---

## 9. Home Assistant Integration (currently Out of Scope)

**Gap:** No ha_* tools for smart home control.

### Recommended Solution: MCP, not native tools

**Home Assistant shipped an official MCP server in HA 2025.2** at `/api/mcp` (Streamable HTTP transport). This means IronHermes can connect to HA via the planned TOOL-08 MCP integration — no HA-specific tool code needed.

```yaml
mcp:
  servers:
    homeassistant:
      transport: http
      url: "http://homeassistant.local:8123/api/mcp"
      headers:
        Authorization: "Bearer ${HA_TOKEN}"
```

Community MCP servers also exist: `tevonsb/homeassistant-mcp`, `homeassistant-ai/ha-mcp`.

**If MCP is unavailable** (HA < 2025.2): The REST API (`/api/states`, `/api/services/{domain}/{service}`) is callable via reqwest with a Bearer token. A thin `HaTool` wrapping direct REST calls would work but is less maintainable than the MCP path.

**Recommendation:** MCP-first. Build TOOL-08, point it at HA's MCP server. No native `ha_*` tools.

---

## 10. RL Training Tools (currently Out of Scope)

**Gap:** No rl_* tools for managing training runs.

### Assessment

The Rust ML/RL ecosystem is immature:
- **candle** (HuggingFace): Pure Rust, GPU support, but no built-in RL algorithms
- **tch-rs**: PyTorch bindings, pulls 2GB libtorch
- **border**: Rust RL library (DQN, SAC) on candle/tch backends — research-tier only

**Practical approach:** Use IronHermes's `execute_code` tool (Phase 8) to run Python RL scripts (PyTorch/Gym/SB3). The agent manages training via subprocess tool calls. IronHermes's batch processing (Phase 10) already produces ShareGPT-format trajectories — this IS the RL data pipeline.

**Recommendation:** Keep out of scope as native tools. Use `execute_code` for Python-side RL training. No native Rust RL stack is production-ready.

---

## Priority Matrix

| Item | Effort | Value | Dependencies | Recommendation |
|------|--------|-------|-------------|----------------|
| **TOOL-01** Toolset filtering | Small | High | None | v2 early — foundational |
| **TOOL-02** Process management | Medium | High | None | v2 early |
| **TOOL-08** MCP integration | Medium-large | Very high | None | v2 priority — unlocks HA, browser, and extensibility |
| **TOOL-06** send_message | Small | Medium | Delivery infra exists | v2 — quick win |
| **TOOL-07** session_search | Small-medium | Medium | SessionStore | v2 |
| **TOOL-03/04/05** Terminal backends | Large | High | TOOL-02 (process mgmt) | v2 later |
| Browser (built-in CfT) | Medium | High | chromiumoxide crate | v2 — native CDP with managed Chrome for Testing install |
| Media (out of scope) | Small per tool | Medium | reqwest (exists) | Revisit; vision_analyze is near-zero effort |
| HA (out of scope) | None | Medium | TOOL-08 | Free via MCP; no native code needed |
| RL (out of scope) | N/A | Low | execute_code (Phase 8) | Keep out of scope; use execute_code |

---

## Key Insight: MCP as the Integration Strategy

The single highest-leverage item is **TOOL-08 (MCP integration)**. Once IronHermes can connect to MCP servers, it gains:
- **Home Assistant** via HA's built-in MCP server (2025.2+)
- **Browser automation** via Playwright MCP, Browserbase MCP, or chrome-debug-mcp
- **File system access** via @modelcontextprotocol/server-filesystem
- **GitHub** via @modelcontextprotocol/server-github
- **Any future MCP-compatible service** with zero IronHermes code changes

This aligns with the project's anti-feature stance on dynamic plugin loading — MCP is a standard protocol, not arbitrary plugin code. The `rmcp` crate (official, tokio-native) makes the Rust implementation straightforward.

---

*Research sources: crates.io, docs.rs, MCP spec (modelcontextprotocol.io), Steel.dev docs, Home Assistant docs, bollard/russh/rmcp documentation, OpenAI/Anthropic/Stability API docs*
