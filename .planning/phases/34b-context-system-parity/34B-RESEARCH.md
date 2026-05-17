# Phase 34b: Context-System Parity — Research

**Researched:** 2026-05-16
**Domain:** Rust async (tokio), trait extension, regex parsing, file I/O, process::Command, ContextEngine lifecycle
**Confidence:** HIGH

---

## Summary

Phase 34b has three deliverables that share no files and can be implemented in parallel waves.
The Python reference implementations are complete and well-understood; the Rust baseline is
equally clear. The main risk is correctness of the regex (must match Python byte-for-byte) and
interior-mutability discipline for the lifecycle hooks (hooks are `&self`, so counters need
`AtomicUsize` or `Mutex`-guarded fields).

**Primary recommendation:** Port the Python implementations directly. The Rust codebase's
existing patterns (default-no-op `check_pressure`, `Arc<dyn ContextEngine>`, tokio async)
provide clean templates for every new piece.

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** `@url:` uses LLM-processed expansion — call `WebExtractTool` with `use_llm_processing: true`. Mirrors Python's `web_extract_tool` behavior.
- **D-02:** If LLM processing fails, fall back to raw HTTP content and surface a warning in `--- Context Warnings ---`. Do NOT fail silently or drop the reference.
- **D-03:** Default `allowed_root` is `cwd`. `@file:` and `@folder:` cannot escape the workspace root.
- **D-04:** `allowed_root` is fixed to cwd — no config escape hatch.
- **D-05:** `allowed_root` resolves to `TerminalConfig.cwd` if set; otherwise `std::env::current_dir()`.
- **D-06:** The 5 lifecycle hooks go on the **existing `ContextEngine` trait** as additive default no-op impls. No breaking changes.
- **D-07:** `update_from_response` and `update_model` are wired at call sites in this phase (not deferred).
- **D-08:** `is_recall_context` stripping in compressor step 0 (34a D-03) requires no additional work in 34b.

### Claude's Discretion

- Exact type for `update_from_response` usage parameter — use `AggregatedUsage` or a new `UsageReport` alias; pick the cleanest fit.
- `has_content_to_compress` default impl returns `true`.
- Exact position of `preprocess_context_references_async` in each surface's call path — immediately before user message is handed to `AgentLoop::run`.

### Deferred Ideas (OUT OF SCOPE)

- `focus_topic` arg on `compress(...)`
- LCM engine tools (`lcm_grep`, `lcm_describe`, `lcm_expand`)
- Promoting `PressureTracker` fields to trait level
- `MemoryProvider.on_turn_start` / `on_session_switch` / `on_delegation`
- "Only one external memory provider" guard
- `MemoryProvider.on_pre_compress` returns text
- Multi-provider teardown order
</user_constraints>

---

## 1. Python Parity Gap (context_references.py)

### 1.1 Reference Token Types

The master regex in Python (`context_references.py` line 17-19):

```python
_QUOTED_REFERENCE_VALUE = r'(?:`[^`\n]+`|"[^"\n]+"|\'[^\'\n]+\')'
REFERENCE_PATTERN = re.compile(
    rf"(?<![\w/])@(?:(?P<simple>diff|staged)\b|(?P<kind>file|folder|git|url):(?P<value>{_QUOTED_REFERENCE_VALUE}(?::\d+(?:-\d+)?)?|\S+))"
)
```

Six token types:

| Token | Kind | Target / parsing |
|-------|------|-----------------|
| `@diff` | `simple="diff"` | No value; expands to `git diff` |
| `@staged` | `simple="staged"` | No value; expands to `git diff --staged` |
| `@file:<path>[:start[-end]]` | `kind="file"` | Path + optional line range (1-based inclusive) |
| `@folder:<path>` | `kind="folder"` | Directory listing via rg or os.walk |
| `@git:<N>` | `kind="git"` | N clamped to 1-10; expands to `git log -N -p` |
| `@url:<url>` | `kind="url"` | Fetched via web_extract_tool with use_llm_processing=true |

**Negative-lookbehind:** `(?<![\w/])` — token must not be preceded by a word char or `/`.
**Trailing punctuation stripped** from value: `,.;!?` and unbalanced `)`, `]`, `}`.
**Quoted values** supported: backtick, double-quote, single-quote wrappers stripped from target.
**Line range** for `@file:`: `_parse_file_reference_value` handles:
- Quoted: `` `path`:12-20 `` — regex match on quoted form first
- Unquoted: `path/to/file.rs:10-25` — colon-separated suffix
- Single line: `path:10` → line_start=10, line_end=10

### 1.2 Sensitive-Path Blocklist (exact)

[VERIFIED: context_references.py lines 21-37]

**Sensitive HOME dirs** (anything under these is blocked):
- `.ssh`, `.aws`, `.gnupg`, `.kube`, `.docker`, `.azure`, `.config/gh`

**Sensitive HOME files** (exact path match):
- `.ssh/authorized_keys`, `.ssh/id_rsa`, `.ssh/id_ed25519`, `.ssh/config`
- `.bashrc`, `.zshrc`, `.profile`, `.bash_profile`, `.zprofile`
- `.netrc`, `.pgpass`, `.npmrc`, `.pypirc`

**Sensitive HERMES_HOME paths:**
- `$HERMES_HOME/.env` (exact file)
- `$HERMES_HOME/skills/.hub/` (directory prefix)

Error message on block: `"path is a sensitive credential file and cannot be attached"` (exact files) or `"path is a sensitive credential or internal Hermes path and cannot be attached"` (dir prefix match).

**Rust equivalent for HERMES_HOME:** Use `ironhermes_core::get_hermes_home()` or `HERMES_HOME` env var (check what the codebase uses). [ASSUMED — confirm the Rust equivalent of `get_hermes_home()`]

### 1.3 Budget Enforcement

```
hard_limit = max(1, int(context_length * 0.50))
soft_limit = max(1, int(context_length * 0.25))
```

- `injected_tokens > hard_limit` → return immediately with `blocked=true`, `message == original_message`, single warning: `"@ context injection refused: {N} tokens exceeds the 50% hard limit ({M})."`
- `injected_tokens > soft_limit` (but not hard) → add warning, still apply expansions

Token estimation: Python uses `estimate_tokens_rough` (chars/4 roughly). Rust should use `crate::context_compressor::estimate_tokens(text)` which is backed by tiktoken BPE.

### 1.4 Output Format (exact)

Assembly order (from Python `preprocess_context_references_async` lines 188-193):
1. Start with `stripped` (original message with `@ref` tokens removed)
2. If warnings: append `\n\n--- Context Warnings ---\n` + joined `- {warning}` lines
3. If blocks: append `\n\n--- Attached Context ---\n\n` + blocks joined by `\n\n`
4. Call `.strip()` on the result

Block formats per reference type:
- `@file:`: `📄 {ref.raw} ({N} tokens)\n```{lang}\n{content}\n````
- `@folder:`: `📁 {ref.raw} ({N} tokens)\n{listing}`
- `@diff` / `@staged` / `@git:N`: `🧾 {label} ({N} tokens)\n```diff\n{content}\n````
- `@url:`: `🌐 {ref.raw} ({N} tokens)\n{markdown_content}`

After stripping ref tokens, Python normalizes whitespace: `re.sub(r"\s{2,}", " ", text)` then `re.sub(r"\s+([,.;:!?])", r"\1", text)`.

### 1.5 `@folder:` Listing

Python tries `rg --files <path>` first (10s timeout), falls back to `os.walk` skipping hidden dirs and `__pycache__`. Max 200 entries; appends `- ...` truncation marker. Format:
```
src/
  - main.rs (45 lines)
  - lib.rs (120 lines)
  - agent/
    - loop.rs (200 lines)
```

Rust equivalent: try `std::process::Command::new("rg")` with `--files`, fall back to `walkdir` or manual `fs::read_dir` recursion. Limit 200.

### 1.6 Binary File Detection

Python: check mimetype (skip if not text/* and not common source extension), then read first 4096 bytes and check for `\x00`. Rust: `infer` crate or manual null-byte check. [ASSUMED — confirm Rust approach]

---

## 2. Rust context_refs.rs Design

### 2.1 Async Runtime

The codebase uses **tokio** exclusively. `agent_loop.rs` imports `tokio::sync::{Mutex, RwLock}`. All async is `#[tokio::main]` / `#[async_trait]`. [VERIFIED: agent_loop.rs imports]

### 2.2 Struct/Enum Design

Mirror Python's dataclasses directly:

```rust
#[derive(Debug, Clone)]
pub struct ContextReference {
    pub raw: String,          // original matched text (e.g. "@file:foo.rs:10-25")
    pub kind: RefKind,        // enum
    pub target: String,       // stripped/unquoted value
    pub start: usize,         // byte offset in original message
    pub end: usize,           // byte offset in original message
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RefKind {
    Diff,
    Staged,
    File,
    Folder,
    Git,   // N stored as parsed u32 in target or separate field
    Url,
}

#[derive(Debug, Clone)]
pub struct ContextReferenceResult {
    pub message: String,
    pub original_message: String,
    pub references: Vec<ContextReference>,
    pub warnings: Vec<String>,
    pub injected_tokens: usize,
    pub expanded: bool,
    pub blocked: bool,
}
```

### 2.3 Regex

Use the `regex` crate (already in workspace — used in nudge.rs, transcript.rs). The Rust regex equivalent of the Python pattern:

```rust
static REFERENCE_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?<![[:word:]/])@(?:(?P<simple>diff|staged)\b|(?P<kind>file|folder|git|url):(?P<value>(?:`[^`\n]+`|"[^"\n]+"|\x27[^\x27\n]+\x27)(?::\d+(?:-\d+)?)?|\S+))"#
    ).unwrap()
});
```

**Note:** Rust's `regex` crate does not support lookbehinds. The `(?<![\w/])` lookbehind must be implemented as a post-match position check: verify `match.start() == 0 || !is_word_or_slash(input.as_bytes()[match.start()-1])`. [ASSUMED — verify regex crate lookbehind support; likely needs manual position check]

### 2.4 Error Type

Return `anyhow::Result` from the async preprocessing function (matches the rest of the codebase — agent_loop.rs, handler.rs all use `anyhow::Result`). Internal expansion errors per-reference are captured as warning strings, not hard errors.

### 2.5 `UrlFetcher` type

In Python: `Callable[[str], str | Awaitable[str]] | None`. In Rust, use a trait object:

```rust
pub type UrlFetcher = Arc<dyn Fn(String) -> BoxFuture<'static, anyhow::Result<String>> + Send + Sync>;
```

Or simpler: accept `Option<&WebExtractTool>` directly since D-01 says to call `WebExtractTool` with `use_llm_processing: true`. However, `WebExtractTool` requires `Arc<dyn SummarizationClientHandle>` at construction — passing it by reference from the call site is cleaner than the trait-object approach. [ASSUMED — planner should decide whether to pass `WebExtractTool` directly or use a `Box<dyn AsyncFn>`]

**WebExtractTool calling convention (VERIFIED: web_extract.rs lines 186-189, 229-231):**
The tool's `execute()` takes `serde_json::Value` args with `use_llm_processing: bool` as a JSON field. To call it for `@url:` expansion, construct args:
```rust
let args = serde_json::json!({
    "urls": [url],
    "use_llm_processing": true,
    "format": "markdown"
});
let result_json = web_extract_tool.execute(args).await?;
// parse ExtractionResult from JSON
```
For D-02 fallback: retry with `"use_llm_processing": false`.

---

## 3. ContextEngine Hook Signatures

### 3.1 Python ABC Signatures (exact)

From `context_engine.py`:

```python
def on_session_start(self, session_id: str, **kwargs) -> None: ...
def on_session_end(self, session_id: str, messages: List[Dict[str, Any]]) -> None: ...
def on_session_reset(self) -> None:  # default clears token counters
    self.last_prompt_tokens = 0
    self.last_completion_tokens = 0
    self.last_total_tokens = 0
    self.compression_count = 0

def update_from_response(self, usage: Dict[str, Any]) -> None: ...  # abstract
def update_model(self, model: str, context_length: int, base_url: str = "", ...) -> None: ...
def has_content_to_compress(self, messages: List[Dict[str, Any]]) -> bool: ...  # default True
```

### 3.2 Rust Trait Signatures

Add to the existing `ContextEngine` trait in `context_engine.rs` as default impls:

```rust
#[async_trait]
pub trait ContextEngine: Send + Sync + 'static {
    // ... existing methods ...

    /// Called when a new conversation session begins.
    fn on_session_start(&self, _session_id: &str) {}

    /// Called at real session end (CLI exit, /reset, gateway expiry).
    fn on_session_end(&self, _session_id: &str, _messages: &[ChatMessage]) {}

    /// Called on /new or /reset. Clear per-session counters.
    fn on_session_reset(&self) {}

    /// Called after every LLM turn with aggregated token usage.
    fn update_from_response(&self, _usage: &AggregatedUsage) {}

    /// Called when the user switches model or on fallback activation.
    fn update_model(&self, _model: &str, _context_length: usize, _base_url: Option<&str>) {}

    /// Quick check: is there content that can be compacted?
    fn has_content_to_compress(&self, _messages: &[ChatMessage]) -> bool { true }
}
```

**`AggregatedUsage` type:** Already defined in `agent_loop.rs` (lines 94-99) with fields `prompt_tokens: usize`, `completion_tokens: usize`, `total_tokens: usize`. This is the correct type for `update_from_response`. [VERIFIED: agent_loop.rs]

**Interior mutability required:** Because hooks are `&self` (not `&mut self`) but `on_session_reset` must clear counters, any fields cleared by `on_session_reset` must use `AtomicUsize` or be wrapped in `Mutex<T>`. The `check_pressure` pattern uses `Arc<PressureTracker>` with `Mutex<HashMap>` inside — same pattern applies here.

**`async_trait` not needed for hooks:** All 5 new hooks are synchronous (`fn`, not `async fn`). They do NOT need `#[async_trait]`. The existing `async fn compress` and `async fn check_pressure` already use `#[async_trait]`; the synchronous hooks coexist on the same trait.

---

## 4. Call Site Mapping (3 Surfaces)

### 4.1 ContextCompressor vs ContextEngine — clarification

There are TWO compression layers in the Rust codebase:

1. **`ContextCompressor`** (in `context_compressor.rs`): the lower-level struct used by `LocalPruningEngine`; holds `compression_count`, `protect_first_n`, `protect_last_tokens`. Its `compression_count` field is a bare `usize` (not atomic).

2. **`LocalPruningEngine` / `SummarizingEngine`** (in `context_engine.rs` / `summarizing_engine.rs`): implement `ContextEngine` trait; held as `Arc<dyn ContextEngine>` in the agent loop. These are the types that need `on_session_reset` overrides.

The lifecycle hooks go on the **trait** (`ContextEngine`). The counter fields that need clearing (`compression_count`, token counters) live inside the concrete structs, behind `Mutex` or `AtomicUsize` after this phase.

### 4.2 CLI Surface (`crates/ironhermes-cli/src/main.rs`)

**Function:** `run_chat` (line 1070)

| Hook | Where in `run_chat` | Notes |
|------|---------------------|-------|
| `on_session_start` | After `session_id` is generated (line 1110), before first turn | One-shot at REPL start |
| `on_session_reset` | `CommandResult::ClearSession` arm (line 1597): `messages.truncate(1)` | Call after truncate |
| `on_session_reset` | `CommandResult::ResetTerminal` arm (line 1602) does NOT clear messages; skip | No-op for visual reset |
| `preprocess_context_references_async` | Before each `AgentLoop::run` call — user message is in `messages` | Immediately before agent dispatch |
| `update_from_response` | After `AgentLoop::run` returns `AgentResult` — `result.total_usage` has the data | Call with `&result.total_usage` |
| `update_model` | When model changes (model switch command) | [ASSUMED — need to find the exact CLI model-switch handler] |

The `context_engine` is held as `Option<Arc<dyn ContextEngine>>` in `AgentLoop` (line 173 in agent_loop.rs). The engine reference is also accessible at the `run_chat` level via `ironhermes_agent::attach_context_engine` (line 2373). The hooks should be called on the same `Arc<dyn ContextEngine>` held by `run_chat`.

**`/new` vs `/reset` distinction:** In `run_chat`, `CommandResult::ClearSession` is the `/new` semantic (line 1597-1601: `messages.truncate(1)`). Both `/new` and `/reset` should call `on_session_reset`. The `CommandResult::ResetTerminal` arm is a visual-only TTY reset (line 1602-1608) and should NOT call `on_session_reset`.

### 4.3 Gateway Surface (`crates/ironhermes-gateway/src/handler.rs`)

**Function:** `handle_with_multimodal` (line 728); slash dispatch via `handle_slash_command` (line 364)

| Hook | Where | Notes |
|------|-------|-------|
| `on_session_start` | When a new `SessionKey` is first allocated — `CoreCommandResult::NewSession { .. }` arm (line 466) or the first `run_agent` call for an unseen key | Session store keyed by `SessionKey` |
| `on_session_reset` | `CoreCommandResult::NewSession { .. }` arm (line 466): `store.remove(&session_key)` — call before or after remove | `/new` handler |
| `on_session_reset` | `CoreCommandResult::ClearSession` arm (line 499): `session.clear()` | `/clear` / future commands |
| `preprocess_context_references_async` | In `run_agent` before `AgentLoop::run`, after `user_message` is built (line 799) | |
| `update_from_response` | After `AgentLoop::run` returns in `run_agent` | `session_id_str` is `"gw:<chat_id>:<sender_id>"` |

### 4.4 Web UI Surface (`crates/iron_hermes_ui/src/server/`)

**Functions:** `ensure_web_session` in `state.rs` (line 123); `run_web_turn` in `state.rs` (line 144); called from `api.rs` (line 130) and `ws.rs` (line 213)

| Hook | Where | Notes |
|------|-------|-------|
| `on_session_start` | In `ensure_web_session` after `create_session` succeeds (line 131-138) — this is called from `POST /api/sessions/create` (api.rs line 123) | Session key format: `"agent:main:web:dm:{uuid}"` |
| `on_session_reset` | Need to add a `POST /api/sessions/{id}/reset` or `new-chat` WebSocket message handler — does not currently exist | [ASSUMED — the new-chat trigger may need to be added; check ws.rs for a new-chat message type] |
| `preprocess_context_references_async` | In `run_web_turn` (state.rs line 144) before `agent.run(messages)` (line 161) | |
| `update_from_response` | In `run_web_turn` after `agent.run` returns (line 161); `agent_result.total_usage` is available | |

**Important:** The web UI's `on_session_reset` trigger is less obvious than CLI. The `ensure_web_session` function only creates sessions, not resets them. The planner needs to check ws.rs for a `new_chat` message type or add a reset endpoint. [ASSUMED]

---

## 5. Counter Reset Scope

### 5.1 Python `ContextCompressor.on_session_reset` (exact, lines 361-374)

```python
def on_session_reset(self) -> None:
    super().on_session_reset()                  # clears: last_prompt_tokens, last_completion_tokens,
                                                #         last_total_tokens, compression_count
    self._context_probed = False
    self._context_probe_persistable = False
    self._previous_summary = None
    self._last_summary_error = None
    self._last_summary_dropped_count = 0
    self._last_summary_fallback_used = False
    self._last_aux_model_failure_error = None
    self._last_aux_model_failure_model = None
    self._last_compression_savings_pct = 100.0
    self._ineffective_compression_count = 0
    self._summary_failure_cooldown_until = 0.0
```

### 5.2 Rust `ContextCompressor` Fields (what exists today)

The Rust `ContextCompressor` (context_compressor.rs, lines 39-45) has:
- `compression_count: usize` (bare field, not atomic)
- `protect_first_n: usize`
- `protect_last_tokens: usize`
- `threshold_percent: f64`
- `context_length: usize`

No `last_prompt_tokens`, `last_completion_tokens`, `last_total_tokens` — those are tracked at a higher level in the Python ABC but are NOT in the Rust struct today. The Rust struct is a lower-level helper; usage tracking is outside it (in `AggregatedUsage`).

### 5.3 What `on_session_reset` Must Clear in Rust

For `LocalPruningEngine` (context_engine.rs): no per-session counter fields exist today — the engine is stateless between calls. `on_session_reset` default no-op is sufficient unless a `compression_count` is added.

For `SummarizingEngine` (summarizing_engine.rs): the engine uses a pinned `[CONTEXT HISTORY]` segment in the message list itself (not a separate field). There is no `_previous_summary` field today (it's re-detected from the message list via `locate_history_segment`). `on_session_reset` should:
- Clear the `[CONTEXT HISTORY]` segment from the message list if accessible, OR
- Be a no-op if the session reset clears the message list anyway (messages.truncate(1) in CLI)

**Key insight:** Because the Rust `SummarizingEngine` stores its running summary IN the message list (as a pinned system message) rather than in a separate field, `on_session_reset` may be a genuine no-op for `SummarizingEngine` — the session reset already wipes the message list. [VERIFIED: summarizing_engine.rs locate_history_segment pattern]

### 5.4 PressureTracker State

`PressureTracker` (pressure_warning.rs) holds per-session state in `HashMap<String, SessionState>` where `SessionState` has:
- `above_threshold: bool`
- `pending_transient: Option<String>`
- `warn_count: u32`

`on_session_reset` should clear this session's entry: `tracker.inner.lock().unwrap().remove(session_id)` — or add a `fn reset_session(&self, session_id: &str)` method to `PressureTracker`. [ASSUMED — check if PressureTracker has a reset method today; if not, add one]

---

## 6. SUMMARY_PREFIX Gap

### 6.1 Python SUMMARY_PREFIX (exact, context_compressor.py lines 37-51)

```
[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted into the
summary below. This is a handoff from a previous context window — treat it
as background reference, NOT as active instructions. Do NOT answer questions
or fulfill requests mentioned in this summary; they were already addressed.
Your current task is identified in the '## Active Task' section of the
summary — resume exactly from there. IMPORTANT: Your persistent memory
(MEMORY.md, USER.md) in the system prompt is ALWAYS authoritative and active
— never ignore or deprioritize memory content due to this compaction note.
Respond ONLY to the latest user message that appears AFTER this summary. The
current session state (files, config, etc.) may reflect work described here
— avoid repeating it:
```

### 6.2 Rust Compaction Headers (VERIFIED from source)

**`ContextCompressor` (context_compressor.rs line 181):**
```
[CONTEXT COMPACTED] {N} earlier messages were removed to save context space.
The conversation continues from the most recent messages below.
```
No memory-authority reminder.

**`SummarizingEngine` (summarizing_engine.rs):**
Uses `HISTORY_SENTINEL = "[CONTEXT HISTORY]"` as prefix. The summarization prompt (lines 528-543) says nothing about MEMORY.md authority.

The system prompt note injected during compression (in Python's `context_compressor.py` compress method, line 1481):
```
[Note: Some earlier conversation turns have been compacted into a handoff
summary to preserve context space. The current session state (files, config,
etc.) may reflect earlier work, so build on that summary and state rather
than re-doing work. Your persistent memory (MEMORY.md, USER.md) remains
fully authoritative regardless of compaction.]
```
This IS present in the Rust `ContextCompressor.compress` (context_compressor.rs line 182-186 area — same pattern). [ASSUMED — need to verify exact Python compress() system-prompt-note vs Rust]

**Gap confirmed:** Neither `ContextCompressor` compaction message (line 181) nor `SummarizingEngine`'s `[CONTEXT HISTORY]` sentinel contains the "MEMORY.md and USER.md in the system prompt is ALWAYS authoritative" reminder.

### 6.3 What to Patch

Per CONTEXT.md `<specifics>` section, the exact text to verify/add in the `SummarizingEngine` compaction header:

> *"Your persistent memory (MEMORY.md, USER.md) in the system prompt is ALWAYS authoritative — never ignore or deprioritize memory content due to this compaction note."*

The planner must decide whether to:
1. Add the reminder to `make_history_message` (the `[CONTEXT HISTORY]` segment body prefix), OR
2. Add it to the LLM summarization prompt so the model includes it in the generated summary

Option 1 is simpler and more reliable — the reminder is in the message the model reads, not dependent on the summarizer's output. Match this to success criterion #11.

**Unit test:** `assert!(summary_prefix.contains("MEMORY.md") && summary_prefix.contains("ALWAYS authoritative"))`.

---

## 7. Test Strategy

### 7.1 Required Tests for 34b-01 (context_refs.rs)

Minimum 14 unit tests per success criteria. Map to actual test functions:

**Parser tests (6):**
1. `test_parse_simple_diff` — `@diff` parses to kind=Diff, target=""
2. `test_parse_simple_staged` — `@staged` parses to kind=Staged
3. `test_parse_kind_value` — `@file:src/main.rs` parses to kind=File, target="src/main.rs"
4. `test_parse_quoted_path` — `` @file:`path with spaces.rs`:1-10 `` → line_start=1, line_end=10
5. `test_parse_line_range` — `@file:foo.rs:10-25` → line_start=10, line_end=25
6. `test_parse_trailing_punctuation` — `@file:foo.rs,` strips comma from target

**Expander tests (5):**
7. `test_expand_file` — creates temp file, verifies `📄 @file:... (N tokens)` block returned
8. `test_expand_file_with_range` — slice of lines 2-4 only
9. `test_expand_folder` — temp dir with 2 files → listing block
10. `test_expand_diff` — calls `git diff` in a temp git repo (or mocks process)
11. `test_expand_url_stub` — passes a mock url_fetcher that returns "hello"; verifies `🌐` block

**Budget tests (2):**
12. `test_hard_limit_blocks_all` — context_length=100, expansion > 50 tokens → blocked=true, message unchanged
13. `test_soft_limit_warns` — expansion 26-50 tokens → warns, still applies

**Sensitive-path test (1):**
14. `test_sensitive_path_blocklist` — parameterized over all entries; each returns warning, no expansion

### 7.2 Required Tests for 34b-02 (context_engine.rs, context_compressor.rs)

**Counter reset test:**
```rust
#[test]
fn test_on_session_reset_clears_counters() {
    // Set compression_count, last_prompt_tokens etc.
    // Call on_session_reset()
    // Assert all are zero
}
```

**Header content test:**
```rust
#[test]
fn test_compaction_header_contains_memory_authority_reminder() {
    let header = /* get the [CONTEXT HISTORY] prefix or SUMMARY_PREFIX equivalent */;
    assert!(header.contains("MEMORY.md"), "header must mention MEMORY.md");
    assert!(header.contains("ALWAYS authoritative"), "header must include authority reminder");
}
```

### 7.3 Existing Test Patterns (for consistency)

From `context_engine.rs` tests (lines 322-637):
- Use `#[tokio::test]` for async tests
- Use `#[test]` for sync unit tests
- `build_large_message_vec(n)` helper pattern for test fixtures
- Test names: `snake_case`, descriptive verbs (`test_xxx`, not `xxx_test`)

From `nudge.rs` tests (line 155+): pure `#[test]` for deterministic logic, no async needed for parser tests.

---

## 8. Integration Risks

### 8.1 Regex Lookbehind (HIGH risk)

The `regex` crate does NOT support lookbehind assertions. Python's `(?<![\w/])` must be implemented as a manual position check after each match:

```rust
for m in REFERENCE_PATTERN.find_iter(message) {
    let start = m.start();
    if start > 0 {
        let prev = &message[..start];
        let last = prev.chars().last().unwrap();
        if last.is_alphanumeric() || last == '_' || last == '/' {
            continue; // skip — lookbehind would reject
        }
    }
    // process match
}
```

This is the primary parser fidelity risk.

### 8.2 Async Boundary in `@url:` Expansion (MEDIUM risk)

`WebExtractTool::execute` is `async`. The preprocessor is already `async fn preprocess_context_references_async`. No boundary issue — the tokio runtime is running at all three call sites (CLI uses `tokio::main`, gateway uses axum which is tokio, web UI uses actix/axum). The url fetcher can simply be `await`ed inside the async preprocessor.

For `@folder:` expansion with `rg` subprocess: use `tokio::process::Command` (not `std::process::Command`) to avoid blocking the tokio thread. Same for `@diff`, `@staged`, `@git:N`.

### 8.3 `&self` vs `&mut self` for Hook State (MEDIUM risk)

The `ContextEngine` trait requires `Send + Sync + 'static`. Hooks are `&self`. Any mutable state (counters to clear, session state) must use `AtomicUsize` or `Mutex`-guarded fields. The `PressureTracker` already demonstrates this pattern with `Arc<Mutex<HashMap>>`.

For `ContextCompressor` (the lower-level struct): its `compression_count` is a bare `usize` field. To support `on_session_reset` via `&self`, it needs to change to `AtomicUsize`. Since `ContextCompressor` is not `Arc<dyn ContextEngine>` (it's used by `LocalPruningEngine`), the `on_session_reset` override on `LocalPruningEngine` would clear the embedded `ContextCompressor` via `Mutex<ContextCompressor>` or rebuild it on reset.

Simplest path: `LocalPruningEngine`'s `on_session_reset` default no-op is fine (no persistent counter state — `compression_count` lives in the short-lived `ContextCompressor` created per `compress()` call). [VERIFIED: LocalPruningEngine::compress creates a new `ContextCompressor::new(...)` every time it compresses — line 252-256 of context_engine.rs]

### 8.4 Token Budget Estimation Consistency

Python uses `estimate_tokens_rough` (~chars/4). Rust uses tiktoken BPE (`estimate_tokens` in context_compressor.rs backed by `global_estimate_tokens`). The token counts will differ slightly, meaning the soft/hard limits will enforce at slightly different thresholds. This is acceptable — the limits are safety margins, not exact. [ASSUMED — acceptable divergence]

### 8.5 `WebExtractTool` Requires `SummarizationClientHandle`

`WebExtractTool::new` requires `Arc<dyn SummarizationClientHandle>` and `Arc<SkillRegistry>`. These are available at the surfaces where `@url:` expansion needs to run (they're wired in `AppRuntimeBundle`). The preprocessor will receive a reference to the `WebExtractTool` (or its JSON-callable equivalent) from the call site. The planner must ensure the tool reference is threaded through to `preprocess_context_references_async`. [ASSUMED — confirm how WebExtractTool is accessed from CLI run_chat]

### 8.6 `@folder:` Listing — `rg` Availability

The folder expander tries `rg` first, falls back to manual walk. `rg` is used elsewhere in the codebase (Phase 34a `session_search.rs`). The fallback walkdir is safe even if `rg` is absent.

### 8.7 `on_session_reset` in Web UI — Missing Trigger

The web UI's `run_web_turn` does not currently have a "new chat / reset" concept that clears messages. The `ensure_web_session` creates but never resets a session. A new-chat flow would need a WebSocket message type or REST endpoint that:
1. Clears the session messages in `StateStore`
2. Calls `engine.on_session_reset()`

This may be out of scope for 34b-02 if no new-chat UI exists yet. [ASSUMED — planner must determine if web UI exposes a new-chat action]

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | cargo test (built-in) |
| Config file | workspace `Cargo.toml` |
| Quick run — 34b-01 | `cargo test -p ironhermes-agent --lib context_refs::tests` |
| Quick run — 34b-02 | `cargo test -p ironhermes-agent --lib context_engine::tests context_compressor::tests` |
| Full suite | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req | Behavior | Test Type | Automated Command |
|-----|----------|-----------|-------------------|
| SC-1 | `context_refs.rs` exports correct types | compile | `cargo build -p ironhermes-agent` |
| SC-2 | Regex matches Python token patterns | unit | `cargo test -p ironhermes-agent --lib context_refs::tests::test_parse_*` |
| SC-3 | Sensitive-path blocklist | unit | `cargo test -p ironhermes-agent --lib context_refs::tests::test_sensitive_path_blocklist` |
| SC-4 | Budget enforcement (hard + soft) | unit | `cargo test -p ironhermes-agent --lib context_refs::tests::test_hard_limit_blocks_all` |
| SC-5 | Output format | unit | `cargo test -p ironhermes-agent --lib context_refs::tests::test_expand_file` |
| SC-6 | 3-surface wiring compiles | compile | `cargo build --workspace` |
| SC-7 | 14+ unit tests pass | unit | `cargo test -p ironhermes-agent --lib context_refs::tests` |
| SC-8 | 5 lifecycle hooks on trait | compile + unit | `cargo test -p ironhermes-agent --lib context_engine::tests` |
| SC-9 | `on_session_reset` clears counters | unit | `cargo test -p ironhermes-agent --lib context_compressor::tests::test_on_session_reset_clears_counters` |
| SC-11 | Header contains memory-authority reminder | unit | `cargo test -p ironhermes-agent --lib context_compressor::tests::test_compaction_header_contains_memory_authority` |
| SC-12-15 | Cross-phase regression | unit | `cargo test -p ironhermes-agent --lib memory_context::tests streaming_scrubber::tests nudge::tests` |

### Sampling Rate

- **Per task commit:** `cargo build --workspace` (confirms no compile breaks)
- **Per wave merge:** Full test commands above for 34b-01 and 34b-02
- **Phase gate:** `cargo test --workspace` green before `/gsd:verify-work`

### Wave 0 Gaps

- [ ] `crates/ironhermes-agent/src/context_refs.rs` — new file; 14+ unit tests inside `#[cfg(test)] mod tests`
- [ ] `context_engine::tests` module — needs new test functions for hook existence and counter reset
- [ ] `context_compressor::tests` — needs `test_on_session_reset_clears_counters` and `test_compaction_header_contains_memory_authority`

---

## Standard Stack

No new external crates required. All needed capabilities exist in the workspace:

| Capability | Existing Asset | Location |
|------------|---------------|----------|
| Regex parsing | `regex` crate | workspace dep (used in nudge.rs) |
| Async subprocess | `tokio::process::Command` | tokio workspace dep |
| File I/O | `std::fs` / `tokio::fs` | std |
| Token estimation | `estimate_tokens()` | `context_compressor.rs` |
| URL fetch | `WebExtractTool` | `ironhermes-tools/src/web_extract.rs` |
| `async_trait` | `async_trait` crate | workspace dep |
| `once_cell` / `lazy_static` | available | workspace dep (for static regex) |

---

## Environment Availability

Step 2.6: No external dependencies beyond the Rust workspace itself. The `rg` (ripgrep) binary is a soft dependency for `@folder:` expansion with graceful fallback to manual walk. `git` is required for `@diff`, `@staged`, `@git:N` but is standard in any dev environment.

| Dependency | Required By | Available | Fallback |
|------------|------------|-----------|----------|
| `git` binary | `@diff`, `@staged`, `@git:N` expansion | Expected present | Return warning: `"{ref.raw}: git command failed"` |
| `rg` binary | `@folder:` listing (fast path) | Optional | Manual `fs::read_dir` walk |
| tokio runtime | all async | Yes — workspace | — |
| WebExtractTool wiring | `@url:` expansion | Requires call-site plumbing | Fall back to raw HTTP per D-02 |

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Rust `regex` crate does not support lookbehind — manual position check needed | §2.3 | If lookbehind IS supported (via `fancy-regex`), the manual check is unnecessary but harmless |
| A2 | Rust equivalent of Python's `get_hermes_home()` — needs verification | §1.2 | Wrong function name causes compile error |
| A3 | Binary file detection uses null-byte check (Rust approach) | §1.5 | Different approach may produce different binary vs text classification |
| A4 | `WebExtractTool` is accessible from CLI `run_chat` call site | §2.5 | If not wired through `AppRuntimeBundle`, requires plumbing |
| A5 | Web UI does not currently have a new-chat trigger for `on_session_reset` | §4.4, §8.7 | If a reset endpoint exists, the hook wires there; if not, planner must decide scope |
| A6 | `LocalPruningEngine.on_session_reset` is a genuine no-op (compressor is recreated each call) | §5.3 | If a persistent compressor accumulates state, the no-op is wrong |
| A7 | Token budget divergence (tiktoken vs chars/4) is acceptable | §8.4 | If budgets are security-critical, the divergence needs to be addressed |
| A8 | `UrlFetcher` type design (trait object vs direct WebExtractTool ref) | §2.5 | Affects API surface of `preprocess_context_references_async` |

---

## Open Questions

1. **Where is `HERMES_HOME` resolved in Rust?**
   - What we know: Python uses `from hermes_constants import get_hermes_home()`
   - What's unclear: Rust equivalent — is it `std::env::var("HERMES_HOME")` or a crate function?
   - Recommendation: `grep -r "HERMES_HOME\|hermes_home" crates/` before implementing

2. **Does `PressureTracker` have a `reset_session` method?**
   - What we know: `PressureTracker` stores `HashMap<String, SessionState>`; no `remove` or `reset` method found
   - What's unclear: Whether adding `fn reset_session(&self, session_id: &str)` is the right API
   - Recommendation: Add the method in 34b-02 as part of `on_session_reset` implementation

3. **Web UI new-chat trigger for `on_session_reset`**
   - What we know: `ensure_web_session` creates sessions; `POST /api/sessions/create` is the entry point
   - What's unclear: Whether the web UI exposes a "new conversation" button that should trigger reset
   - Recommendation: Check ws.rs for a `new_chat` or `reset` WebSocket message type; scope accordingly

4. **`update_model` call site in CLI `run_chat`**
   - What we know: `update_model` should fire when the user switches models
   - What's unclear: Where exactly model switches happen in `run_chat` (model-switch slash command?)
   - Recommendation: `grep -n "model\|switch\|\"model\"" main.rs` to find the exact handler

---

## Sources

### Primary (HIGH confidence)
- `../hermes-agent/agent/context_references.py` — read in full; all token types, regex, blocklist, output format verified
- `../hermes-agent/agent/context_engine.py` — read in full; all 5 hook signatures verified
- `../hermes-agent/agent/context_compressor.py` — read in full; `SUMMARY_PREFIX` and `on_session_reset` verified
- `crates/ironhermes-agent/src/context_engine.rs` — read in full; existing trait shape, `LocalPruningEngine`, test patterns verified
- `crates/ironhermes-agent/src/context_compressor.rs` — read in full; field inventory verified
- `crates/ironhermes-agent/src/summarizing_engine.rs` — read in part; `HISTORY_SENTINEL`, build_summary_prompt, no memory-authority text confirmed
- `crates/ironhermes-agent/src/agent_loop.rs` — `AggregatedUsage` type verified (lines 94-99)
- `crates/ironhermes-tools/src/web_extract.rs` — `use_llm_processing` param verified (lines 186-189)
- `crates/ironhermes-agent/src/lib.rs` — module list verified
- `crates/ironhermes-core/src/config.rs` — `TerminalConfig.cwd` field verified (line 562)
- `crates/ironhermes-agent/src/pressure_warning.rs` — `PressureTracker` and `SessionState` fields verified

### Secondary (MEDIUM confidence)
- `crates/ironhermes-cli/src/main.rs` — call site mapping via grep; `run_chat` structure, session_id generation, CommandResult arms
- `crates/ironhermes-gateway/src/handler.rs` — call site mapping via grep; `handle_with_multimodal`, `handle_slash_command`, `NewSession` arm
- `crates/iron_hermes_ui/src/server/state.rs` — `run_web_turn`, `ensure_web_session` verified
- `crates/iron_hermes_ui/src/server/api.rs` — `create_session` endpoint verified (line 123-133)
- `crates/iron_hermes_ui/src/server/ws.rs` — `run_web_turn` call site confirmed (line 213)

---

## Metadata

**Confidence breakdown:**
- Python parity gap: HIGH — read full source, verified every detail
- Rust call-site mapping: MEDIUM — verified via grep + partial reads; exact line numbers for some sites are grep-derived
- SUMMARY_PREFIX gap: HIGH — confirmed absent by grep across summarizing_engine.rs
- Architecture patterns: HIGH — consistent with established Phase 18/32/34a patterns
- Web UI on_session_reset trigger: LOW — not found; likely needs new endpoint

**Research date:** 2026-05-16
**Valid until:** 2026-06-16 (30 days; stable domain)
