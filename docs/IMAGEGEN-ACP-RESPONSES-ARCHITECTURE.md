<!-- Porting reference for IronHermes Phases 36.3.2/36.3.3 (image/video gen), 36.8 (ACP), Responses API. -->
# hermes-agent: Image/Video Gen · ACP · Responses API — Architecture & Porting Spec

**Date:** 2026-06-14
**Source:** `hermes-agent` working tree (`agent/`, `tools/`, `plugins/`, `acp_adapter/`)
**Purpose:** Reference for three IronHermes parity gaps from `PARITY-UPDATE.md`:
image/video generation (Phases 36.3.2 / 36.3.3), the ACP adapter (Phase 36.8), and the
Responses API (Codex mode). Companion to `docs/EXEC-BACKENDS-ARCHITECTURE.md`.

Descriptive (what the reference does) + concrete quoted code + a Rust port sketch per area.

> **TL;DR on your question — does the Responses API include webhooks?** **No.** hermes-agent's
> Responses path is synchronous streaming SSE with `store=False` (stateless); there is no
> background mode, no poll-by-id, and no webhook callback. Details + the general-OpenAI nuance
> in **Part C §C.6**.

---

# Part A — Image & Video Generation

A **pluggable-provider** subsystem. One agent tool per modality (`image_generate`,
`video_generate`) dispatches to the **active provider**, selected by config. Providers are
plugins that self-register at import time. fal.ai is the reference backend; others
(openai, xai, krea) follow the same ABC.

```
tools/image_generation_tool.py ──► agent/image_gen_registry.py ──► ImageGenProvider (active)
tools/video_generation_tool.py ──► agent/video_gen_registry.py ──► VideoGenProvider (active)
                                                                       │
plugins/{image,video}_gen/<name>/__init__.py  register(ctx) ──────────┘  (fal, openai, xai, krea)
```

## A.1 Provider ABC

`agent/image_gen_provider.py:51` — subclasses implement only `name` + `generate()`; everything
else has defaults.

```python
class ImageGenProvider(abc.ABC):
    @property
    @abc.abstractmethod
    def name(self) -> str: ...                       # "fal", "openai", "xai" — config key
    @property
    def display_name(self) -> str: return self.name.title()
    def is_available(self) -> bool: return True       # typically checks for an API key
    def list_models(self) -> List[Dict]: return []    # catalog for the `hermes tools` picker
    def get_setup_schema(self) -> Dict: ...           # picker metadata + env_vars to prompt for
    def default_model(self) -> Optional[str]: ...

    @abc.abstractmethod
    def generate(self, prompt: str, aspect_ratio: str = "landscape", **kwargs) -> Dict[str, Any]:
        """Return success_response(...) or error_response(...). Ignore unknown kwargs."""
```

`VideoGenProvider` (`agent/video_gen_provider.py:75`) is the same shape plus a
`capabilities()` method and a richer `generate(prompt, *, model, image_url, duration,
aspect_ratio, resolution, negative_prompt, audio, seed, **kwargs)`.

**Uniform response contract** (`image_gen_provider.py:276`) — every provider returns a plain dict
the tool JSON-serializes:

```python
def success_response(*, image, model, prompt, aspect_ratio, provider, extra=None) -> Dict:
    return {"success": True, "image": image, "model": model, "prompt": prompt,
            "aspect_ratio": aspect_ratio, "provider": provider, **(extra or {})}
# image := an HTTP URL OR an absolute file path.  Video uses {"video": url, ...}.
def error_response(*, error, error_type="provider_error", provider="", ...) -> Dict: ...
```

## A.2 Registry & active-provider selection

`agent/image_gen_registry.py` — a name→provider dict, import-time registration, config-driven
active selection with fallback:

```python
def register_provider(provider: ImageGenProvider) -> None:    # idempotent, last-writer-wins
    _providers[provider.name] = provider

def get_active_provider() -> Optional[ImageGenProvider]:
    configured = config["image_gen"]["provider"]              # explicit wins (even if !is_available
                                                              #   → precise "X_API_KEY not set" error)
    if configured and configured in _providers: return _providers[configured]
    available = [p for p in _providers.values() if p.is_available()]
    if len(available) == 1: return available[0]               # single-provider shortcut
    if "fal" in _providers and _providers["fal"].is_available(): return _providers["fal"]  # legacy default
    return None
```

Plugin entry point (`plugins/video_gen/fal/__init__.py:618`):

```python
def register(ctx) -> None:
    ctx.register_video_gen_provider(FALVideoGenProvider())
```

Plugins live in `plugins/{image,video}_gen/<name>/` (built-in, auto-loaded) or
`~/.hermes/plugins/...` (user, opt-in) with a `plugin.yaml` manifest + `__init__.py`.

## A.3 The agent-facing tools — minimal stable schema, dynamic enums

The schema exposed to the **LLM** is deliberately minimal and stable so the tool surface doesn't
churn as backends change.

- **`image_generate`** (`tools/image_generation_tool.py:1021`): exposes only `prompt` +
  `aspect_ratio` (enum `landscape|square|portrait`). The provider internally translates
  `aspect_ratio` → its native size spec (preset enum like `landscape_16_9`, an aspect enum like
  `16:9`, or GPT's literal size string — `size_style` per model, `:517`/`:540`).
- **`video_generate`** (`tools/video_generation_tool.py:100`): `prompt`, `image_url`, `duration`,
  `aspect_ratio` (enum), `resolution` (enum), `negative_prompt`, `audio`, `seed`. Has an optional
  **dynamic schema** (`:398`) that reflects the *active backend's actual capabilities* — the
  family catalog's real resolution/duration enums and audio/negative flags — so the agent can't
  request unsupported combos.

Dispatch is: resolve active provider → call `provider.generate(...)` → JSON-serialize the result.
If no provider/credentials: a helpful error pointing at `hermes tools`.

## A.4 Async generation (the fal queue) — submit + blocking get

Image gen is usually one-shot; **video gen is a queue job**. fal exposes a submit/poll queue API;
the SDK's blocking `.get()` hides the polling (`plugins/video_gen/fal/__init__.py:361`):

```python
def _submit_fal_video_request(endpoint, arguments):
    request_headers = {"x-idempotency-key": str(uuid.uuid4())}
    managed_gateway = _resolve_managed_fal_video_gateway()
    if managed_gateway is None:
        return _fal_client.submit(endpoint, arguments=arguments, headers=request_headers)  # handle
    return _get_managed_fal_video_client(managed_gateway).submit(endpoint, ...)
# caller:
handle = _submit_fal_video_request(endpoint, payload)
result = handle.get()            # BLOCKS until the queue job completes (poll loop inside the SDK)
url    = result["video"]["url"]  # fal CDN URL
```

**Auto-routing** t2v vs i2v by `image_url` presence; **capability-filtered payload** drops keys a
family doesn't declare (`_build_payload`, `:237`); per-family `image_param_key`/`duration_suffix`
quirks. The family catalog (`FAL_FAMILIES`, `:67`) holds endpoints + capability sheets for
pixverse/veo3.1/seedance/kling/ltx/happy-horse.

**Two transports per provider:** direct (`FAL_KEY`) or the **managed Nous gateway**
(`_ManagedFalSyncClient` against the `fal-queue` proxy) when the user has no key but a Nous
subscription. Same pattern as Modal direct-vs-managed in the exec doc.

## A.5 Artifact handling

- **Image:** providers return either inline base64 → `save_b64_image()` or an *ephemeral* URL →
  `save_url_image()` (downloads it because xAI/OpenAI URLs expire). Both write to
  **`$HERMES_HOME/cache/images/<prefix>_<YYYYMMDD_HHMMSS>_<uuid8>.<ext>`** with a 25 MB cap and
  content-type→extension inference (`image_gen_provider.py:174`/`:207`).
- **Video:** returns the fal **CDN URL**; the gateway downloads + delivers it (no local cache by
  default). Delivery to chat platforms happens via the gateway's media extraction
  (`send_image_file`/`send_video`), not inside the gen tool.

## A.6 Config / credentials

`FAL_KEY` (fal), `OPENAI_API_KEY`, `XAI_API_KEY`, etc. — declared per provider in
`get_setup_schema().env_vars`. SDKs (`fal-client`) are **lazy-installed** on first use
(double-checked-lock cache, `:297`). Model selection precedence (fal video, `:210`):
`model=` arg → `FAL_VIDEO_MODEL` env → `video_gen.fal.model` → `video_gen.model` → `DEFAULT_MODEL`.

## A.7 Rust design for IronHermes

```rust
#[async_trait]
trait ImageGenProvider: Send + Sync {
    fn name(&self) -> &str;
    fn is_available(&self) -> bool { true }
    fn list_models(&self) -> Vec<ModelInfo> { vec![] }
    async fn generate(&self, prompt: &str, aspect_ratio: AspectRatio, opts: GenOpts) -> GenResult;
}
#[async_trait]
trait VideoGenProvider: Send + Sync { /* + capabilities(), richer generate() */ }

// Registry: OnceCell<RwLock<HashMap<String, Arc<dyn ImageGenProvider>>>>; config key picks active.
struct GenResult { success: bool, media: Option<String> /*url|path*/, model: String, provider: String, error: Option<String> }
```

- fal has **no Rust SDK** → drive the fal **queue REST API** directly with `reqwest`:
  `POST {endpoint}` → `{request_id, status_url, response_url}` → poll `status_url` until
  `COMPLETED` → `GET response_url`. (This is exactly what the Python SDK's `.submit().get()` wraps.)
- Reuse the `tts`/`stt` trait+registry pattern already in `ironhermes-core` (`tts.rs`/`stt.rs`)
  and `ironhermes-tools` — image/video gen is the same shape (trait → registry → tool).
- Artifact dir: `~/.ironhermes/cache/images/`; download ephemeral URLs locally; size-cap.
- **Checklist:** (1) `ImageGenProvider`/`VideoGenProvider` traits + uniform result; (2) registry +
  config selection w/ fallback; (3) fal queue REST client (submit→poll→get); (4) the two agent
  tools w/ minimal schema + capability-driven video enums; (5) artifact save + size cap;
  (6) managed-gateway transport (defer if no Nous proxy); (7) auto-route t2v/i2v.

---

# Part B — ACP adapter (Agent Client Protocol)

ACP is the protocol IDEs (Zed, VS Code, JetBrains) speak to an agent. hermes-agent ships a
**stdio JSON-RPC server** (`hermes-acp`) built on the Python `acp` / `agent-client-protocol`
library. One Hermes `AIAgent` core drives each ACP session turn; streaming flows back as
`session/update` notifications.

## B.1 Transport & entry point

`acp_adapter/entry.py:257` — stdio JSON-RPC; **stdout is reserved for protocol**, all logs to
stderr:

```python
agent = HermesACPAgent()
asyncio.run(acp.run_agent(agent, use_unstable_protocol=True))   # reads stdin / writes stdout (JSON-RPC)
```

Console script `hermes-acp = "acp_adapter.entry:main"` (extra `acp`,
`agent-client-protocol==0.9.0`). `hermes_bootstrap` must be the first import (UTF-8 stdio on
Windows). Unknown liveness-probe methods (`ping`/`health`) get a clean JSON-RPC `-32601` with
stderr noise suppressed (`:43`).

## B.2 JSON-RPC method surface

`HermesACPAgent(acp.Agent)` (`server.py:446`) implements:

| ACP method | Handler | Role |
| --- | --- | --- |
| `initialize` | `initialize()` `:861` | negotiate protocol version + advertise capabilities |
| `authenticate` | `authenticate()` `:895` | provider/model auth method handling |
| `session/new` | `new_session()` `:1109` | create a session (+ schedule commands/usage updates) |
| `session/load` | `load_session()` `:1129` | **replay history via `session/update` before responding** |
| (resume) | `:1186` | resume a prior session |
| `session/cancel` | `cancel()` `:1211` | interrupt the running turn |
| `session/prompt` | `prompt()` `:1292` | run a Hermes turn, stream results back |

## B.3 Capabilities / manifest

`initialize()` (`server.py:881`) advertises:

```python
InitializeResponse(
    protocol_version=acp.PROTOCOL_VERSION,
    agent_capabilities=AgentCapabilities(
        load_session=True,
        prompt_capabilities=PromptCapabilities(image=True),
        session_capabilities=SessionCapabilities(...),
    ),
)
```

`acp_registry/agent.json` is the published manifest (id `hermes-agent`, `uvx` distribution
`hermes-agent[acp]`, `args: ["hermes-acp"]`).

## B.4 Session lifecycle & the prompt turn

`prompt()` (`server.py:1292`) is the heart. Flow:

1. Resolve `SessionState`; extract text + multimodal content from the ACP content blocks
   (`TextContentBlock | ImageContentBlock | AudioContentBlock | ResourceContentBlock | ...`).
2. Intercept **slash commands** locally (no LLM) (`:1356`); `/steer` handling for interrupt
   salvage (`:1331`).
3. **Concurrency:** if a turn is already running, **queue** the new prompt instead of racing two
   AIAgent loops on the same history (`:1369`).
4. Wire callbacks that translate Hermes core events → ACP `session/update` notifications, then run
   the `AIAgent`:

```python
tool_progress_cb = make_tool_progress_cb(conn, session_id, loop, tool_call_ids, tool_call_meta, ...)
reasoning_cb     = make_thinking_cb(conn, session_id, loop)        # → agent_thought_chunk
message_cb       = make_message_cb(conn, session_id, loop)         # → agent_message_chunk
approval_cb      = make_approval_callback(conn.request_permission, loop, session_id)
edit_approval    = make_acp_edit_approval_requester(conn.request_permission, loop, session_id, ...)
agent.tool_progress_callback = tool_progress_cb
# ... run agent turn; emit conn.session_update(session_id, update) for each event ...
return PromptResponse(stop_reason="end_turn")   # or "refusal"/"cancelled"
```

**Streaming update types** sent via `conn.session_update(session_id, update)`:
`agent_message_chunk`, `user_message_chunk`, `agent_thought_chunk`, tool-call start/progress,
`usage_update` (`:661`), `session_info_update` (title/`:733`), `available_commands_update`.
`session/load` must **replay prior history** as updates *before* returning (`:1142`).

## B.5 Tool exposure & translation

`acp_adapter/tools.py` maps Hermes tool invocations → ACP tool-call shapes
(`ToolCallStart` / `ToolCallProgress`, statuses `pending|in_progress|completed|failed`):

```python
def build_tool_start(tool_call_id, ...) -> ToolCallStart:
    return acp.start_tool_call(tool_call_id, title, kind=kind, content=content, locations=locations)
# get_tool_kind(name) → ToolKind  (edit/read/execute/search/...)
# build_tool_title(name, args)    → human title shown in the IDE
# ToolCallLocation                → file path + line, so the IDE can focus the edited file
```

Edit/browser/media results get custom formatting (`_format_edit_result`, `:667`;
`_format_browser_result`, `:690`) so diffs and previews render natively in the editor.

## B.6 Permissions & edit approval

`acp_adapter/permissions.py` builds the `session/request_permission` option set:

```python
PermissionOption(option_id="allow_once",    kind="allow_once",   name="Allow once")
PermissionOption(option_id="allow_session", kind="allow_always", name="Allow for session")
PermissionOption(option_id="allow_always",  kind="allow_always", name="Allow always")   # if allow_permanent
PermissionOption(option_id="deny",          kind="reject_once",  name="Deny")
# + reject_always when the client supports it
```

`make_approval_callback()` (`:107`) bridges Hermes' synchronous approval gate to the async
`conn.request_permission` RPC; the chosen `option_id` maps back to a Hermes allow/deny decision
(`_map_outcome_to_hermes`, `:95`). File edits route through `edit_approval.py`
(`make_acp_edit_approval_requester`) with an auto-approve policy getter.

## B.7 Rust design for IronHermes (Phase 36.8)

- **No official Rust ACP crate** is assumed — but ACP is plain **JSON-RPC 2.0 over stdio**, so a
  thin handler is straightforward: a stdin reader → dispatch on `method` → write responses +
  `session/update` notifications to stdout. Reuse the JSON-RPC machinery from `ironhermes-mcp`
  (it already does stdio JSON-RPC for MCP) as the transport scaffold.
- Map each method to an async handler; drive the existing IronHermes agent loop per `session/prompt`,
  forwarding its streaming events (text delta, tool start/done, reasoning) as `session/update`.
- The **callback-translation layer** is the bulk of the work: agent event → ACP update shape, and
  IronHermes tool-call lifecycle → `tool_call`/`tool_call_update` with `ToolKind` + file locations.
- Wire `request_permission` to IronHermes' existing approval/consent path; map option ids.
- **Checklist:** (1) stdio JSON-RPC server + `initialize` capabilities; (2) `session/new|load|cancel`;
  (3) `session/prompt` driving the agent loop; (4) event→`session/update` translation
  (message/thought/tool-call/usage); (5) tool kind/title/location mapping; (6) `request_permission`
  + edit approval; (7) `authenticate` + manifest `agent.json`. Build against Zed (the most common
  ACP client) for conformance.

---

# Part C — Responses API (Codex mode)

The OpenAI **Responses API** (`/v1/responses`) is a distinct surface from Chat Completions, used by
OpenAI Codex, xAI/SuperGrok, GitHub Models, and other Responses-compatible relays. hermes-agent's
adapter is `agent/codex_responses_adapter.py` (format conversion) + `agent/codex_runtime.py`
(streaming) + `agent/transports/codex*.py` (dispatch). It is selected via `ApiMode::CodexResponses`.

## C.1 Stateless by contract: `store=False`

The single most important design fact. hermes-agent **never** persists responses server-side
(`codex_responses_adapter.py:835`):

```python
store = api_kwargs.get("store", False)
if store is not False:
    raise ValueError("Codex Responses contract requires 'store' to be false.")
normalized = {"model": model, "instructions": instructions, "input": normalized_input, "store": False, ...}
```

The full allow-list of request keys (`:839`): `model, instructions, input, tools, store, reasoning,
include, max_output_tokens, temperature, tool_choice, parallel_tool_calls, prompt_cache_key,
service_tier, extra_headers, extra_body, timeout` (+ `stream`). **No `background`, no `previous_response_id`,
no webhook key.** Because nothing is stored, there is no server-side conversation to reference by id
and nothing to call back about asynchronously — full history is re-sent every turn.

## C.2 Streaming SSE consumption (no SDK helper)

`codex_runtime.py:380` `_consume_codex_event_stream` consumes the **raw** SSE event iterable from
`client.responses.create(stream=True)` and assembles the result itself — deliberately avoiding the
SDK's `responses.stream()` helper (which crashed when `response.completed.response.output` drifted
to `null`):

```python
_TERMINAL_EVENT_TYPES = {"response.completed", "response.incomplete", "response.failed"}
for event in event_iter:
    # response.output_text.delta        → on_text_delta (suppressed once a function_call appears)
    # response.reasoning.*.delta        → on_reasoning_delta
    # response.output_item.done         → collect output items (tool calls, message)
    # response.completed/incomplete/failed → terminal: read usage/status/id ONLY
```

Result is reconstructed from `response.output_item.done` + text deltas — **never** from the terminal
event's `output` field (which may be `null`/`[]`/missing). Errors arrive as `type=error` SSE frames
→ `_StreamErrorEvent` (`:365`).

## C.3 The hard part: encrypted reasoning replay

Because there's no server-side state, **reasoning continuity across turns** is achieved by replaying
`encrypted_content` reasoning blobs that the API minted on prior turns
(`codex_responses_adapter.py:293`). This is the main porting challenge.

```python
# request: include=["reasoning.encrypted_content"]  → API returns encrypted reasoning items
# next turn: replay them as input items so the model keeps a coherent reasoning chain
codex_reasoning = msg.get("codex_reasoning_items")          # stored on the assistant message
for ri in codex_reasoning:
    if ri.get("encrypted_content"):
        # 1) cross-issuer guard: a blob is decryptable ONLY by the endpoint that minted it.
        if current_issuer_kind and ri.get("_issuer_kind") not in (None, current_issuer_kind):
            continue   # dropping it; replaying a Codex blob against xAI → HTTP 400 invalid_encrypted_content
        # 2) strip "id" (store=False ⇒ API can't resolve items by id ⇒ 404) and the internal "_issuer_kind"
        replay_item = {k: v for k, v in ri.items() if k not in ("id", "_issuer_kind")}
        items.append(replay_item)
```

Two guards (`:306`): a **session-wide kill switch** (`replay_encrypted_reasoning=False`, flipped when
a relay returns HTTP 400 `invalid_encrypted_content`, which also strips cached items) and a
**per-item cross-issuer filter** (`current_issuer_kind`) for when a session switches providers
mid-conversation. Assistant `message` items are also replayed verbatim (with `phase`) for prefix-cache
hits (`:406`).

## C.4 Input conversion (chat ⇄ Responses shapes)

`codex_responses_adapter.py` converts internal chat-style messages → Responses input items:
- content parts: `input_text` (user) / `output_text` (assistant) / `input_image` (`:80`);
  `input_text` is rejected inside assistant messages (`:89`).
- tool calls → `function_call` items with `call_id` + an `fc_`-prefixed id (`:215`); tool results →
  `function_call_output` items (`:529`). Tool schemas → Responses `{"type":"function", "name", ...}`
  (`:245`, slash-enum sanitization for xAI at `:934`).

## C.5 Dispatch & transports

`agent/transports/codex.py` builds the request (`store:False`, `:148`) and calls the endpoint;
`agent/transports/codex_app_server*.py` handle the consumer Codex backend
(`chatgpt.com/backend-api/codex`). `codex_runtime.py` also has a "background review fork" (`:297`) —
**note: this is a Hermes-internal parallel-agent review, NOT the OpenAI background API.**

## C.6 Webhooks — the direct answer

**In hermes-agent: no.** The Responses adapter is synchronous streaming, `store=False`, no
`background`, no poll-by-id, no callback URL. The request never asks the server to call anything back;
it holds the HTTP stream open and reads SSE frames to `response.completed`.

**In the OpenAI Responses API generally:** webhooks *do* exist as a separate, account-level platform
feature, tied to **background mode** (`background: true` returns immediately; you then either
`GET /v1/responses/{id}` to poll, or receive `response.completed` / `response.failed` events at a
webhook endpoint configured in your OpenAI account — with their own signing secret). hermes-agent
deliberately uses none of it: `store=False` precludes the persisted-response/poll model, and reasoning
continuity is solved by **encrypted-content replay** (§C.3) instead of server-side state. So for the
IronHermes port, **webhooks are out of scope** for Responses parity — a streaming SSE client with
`store=False` + encrypted-reasoning replay is the whole job.

## C.7 Rust design for IronHermes

- IronHermes already has SSE streaming + an `ApiMode::CodexResponses` stub that returns "not yet
  implemented" (per `PARITY-UPDATE.md` §2). The port is: build the Responses request body
  (allow-listed keys, `store:false`), `POST /v1/responses` with `stream:true`, and consume the SSE
  stream into output items + text deltas, watching for `response.completed|incomplete|failed`.
- **Reasoning replay is mandatory for multi-turn coherence.** Store `encrypted_content` reasoning
  items + an issuer stamp on each assistant turn; replay them (minus `id`, minus stamp) on the next
  request with `include=["reasoning.encrypted_content"]`; implement the kill-switch on HTTP 400
  `invalid_encrypted_content` and the cross-issuer drop.
- Reuse the existing `reqwest` SSE plumbing (`ironhermes-agent/src/client.rs`). No webhook server,
  no background polling.
- **Checklist:** (1) request builder (allow-listed keys, `store:false`, tools→function shape);
  (2) chat→Responses input conversion (input_text/output_text/input_image, function_call/output, `fc_` ids);
  (3) SSE consumer assembling from `output_item.done` + `output_text.delta`, terminal frames;
  (4) encrypted-reasoning replay + kill switch + cross-issuer guard; (5) error-frame handling.

---

*Derived from `hermes-agent` on 2026-06-14. Companion to `docs/EXEC-BACKENDS-ARCHITECTURE.md` and
`PARITY-UPDATE.md` (§1 tools, §2 providers/Responses, §3 AI sub-backends, §4 surfaces/ACP).*
