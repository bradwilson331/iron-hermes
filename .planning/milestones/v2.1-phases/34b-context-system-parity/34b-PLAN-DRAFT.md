---
phase: "34b"
title: "Context-System Parity (@-references + ContextEngine lifecycle + Compressor reset)"
slot_note: |
  Phase 34b follows Phase 34a (memory_manager.py parity). Covers parity with:
    - hermes-agent/agent/context_engine.py     (ContextEngine ABC lifecycle hooks)
    - hermes-agent/agent/context_compressor.py (counter reset on /reset, SUMMARY_PREFIX memory authority reminder)
    - hermes-agent/agent/context_references.py (@file: @folder: @diff @staged @git:N @url: expansion)
  Existing roadmap Phase 34 (webchat + Discord/Slack) is unrelated; the user
  picks whether to promote 34a/34b ahead of it or interleave.
status: draft
depends_on: ["34a"]
requirements: "Defined in /gsd-discuss-phase 34b"
references:
  python_sources:
    - "../hermes-agent/agent/context_engine.py"
    - "../hermes-agent/agent/context_compressor.py"
    - "../hermes-agent/agent/context_references.py"
  rust_baseline:
    - "crates/ironhermes-agent/src/context_engine.rs"
    - "crates/ironhermes-agent/src/context_compressor.rs"
    - "crates/ironhermes-agent/src/summarizing_engine.rs"
    - "crates/ironhermes-agent/src/agent_loop.rs"
    - "crates/ironhermes-cli/src/main.rs"
    - "crates/ironhermes-gateway/src/handler.rs"
    - "crates/iron_hermes_ui/src/server/state.rs"
---

<objective>
Close the parity gap with the three context-system modules in hermes-agent:

1. **`@`-reference expansion** (`context_references.py`) — users today can paste
   raw file paths into a message, but the agent only sees the literal string.
   In Python, `@file:`, `@folder:`, `@diff`, `@staged`, `@git:N`, `@url:`
   tokens are parsed pre-turn and replaced with bounded attached-context
   blocks (file slices, folder listings, git diffs, URL fetches) wrapped in a
   `--- Attached Context ---` footer. Sensitive paths (`.ssh/`, `.aws/`,
   `.env`, etc.) are rejected. Two limits enforce safety: 50% of context
   window = hard reject, 25% = warning.

2. **`ContextEngine` lifecycle hook parity** (`context_engine.py`) — Python's
   ABC exposes `on_session_start`, `on_session_reset`, `update_from_response`,
   `update_model`, `has_content_to_compress`. Rust's trait covers only the
   compress / pressure-check surface. The CLI / gateway / web UI today never
   call session-boundary hooks on the engine — `compression_count` and token
   counters accumulate across `/new` and `/reset`.

3. **`ContextCompressor` counter reset + memory-authority reminder**
   (`context_compressor.py`) — Python's `on_session_reset` clears
   `compression_count` and token counters. Python's `SUMMARY_PREFIX`
   explicitly tells the model *"Your persistent memory (MEMORY.md, USER.md)
   in the system prompt is ALWAYS authoritative — never ignore or
   deprioritize memory content due to this compaction note."* Rust's
   compaction header should include the same reminder to keep the model
   anchored to live memory after compression.
</objective>

<background>
This phase is non-load-bearing for the read-side mid-session recall problem
(Phase 34a solves that). It closes the remaining ergonomic + safety gaps
that show up in day-to-day use:

- `@file:foo.rs:10-25` in a chat message currently sends the literal string.
- Compression counter doesn't reset on `/new`, so display metrics drift.
- Compaction summary doesn't re-anchor the model to MEMORY.md / USER.md
  authority, which can let summarized-context drift outweigh live memory.

`@`-reference expansion has security implications (sensitive-path blocklist,
budget enforcement). Treat the implementation as a security-relevant module.
</background>

<parity_matrix>
### A. `context_references.py`

| Python feature                                          | Rust today | Plan |
|---------------------------------------------------------|-----------|------|
| Regex parser (`@diff` / `@staged` / `@file:` / `@folder:` / `@git:N` / `@url:`) | none | **34b-01** |
| Quoted-path support (`@file:"path with spaces":12-20`)  | none      | **34b-01** |
| Line-range slicing (`@file:foo.rs:10-25`)               | none      | **34b-01** |
| File / folder / diff / staged / git / url expansion     | none (infrastructure exists: `web_extract` for url, `ripgrep` for folder listing) | **34b-01** |
| Sensitive-path blocklist (`.ssh/`, `.aws/`, `.env`, ...)| none      | **34b-01** |
| 50% hard / 25% soft token-budget enforcement            | none      | **34b-01** |
| `ContextWarnings` block surfaced to user                | none      | **34b-01** |
| `allowed_root` workspace scoping                        | none      | **34b-01** |

### B. `context_engine.py`

| Python ABC member                                       | Rust today (`context_engine.rs`)               | Plan |
|---------------------------------------------------------|------------------------------------------------|------|
| `name` property                                         | `mode() -> CompressionMode`                    | ✅ parity (different shape) |
| `compress(messages, current_tokens, focus_topic)`       | `compress(messages, ...)` async                | ⚠ missing `focus_topic`; defer to LCM phase |
| `should_compress(prompt_tokens)`                        | `threshold()` + `check_pressure(stats)`        | ✅ functional parity |
| `should_compress_preflight(messages)`                   | `check_pressure(ContextStats)`                 | ✅ functional parity |
| `has_content_to_compress(messages)`                     | none                                           | **34b-02** |
| `on_session_start(session_id, **kwargs)`                | none                                           | **34b-02** |
| `on_session_end(session_id, messages)`                  | `MemoryManager::on_session_end` (memory only)  | **34b-02** (add to engine trait) |
| `on_session_reset()`                                    | none                                           | **34b-02** |
| `update_from_response(usage)`                           | usage tracked outside trait                    | **34b-02** |
| `update_model(model, ctx_len, base_url, ...)`           | none                                           | **34b-02** |
| `get_tool_schemas()` / `handle_tool_call()` (`lcm_grep`)| none                                           | deferred (LCM phase) |
| Token-state fields (`last_prompt_tokens`, `compression_count`, ...) | external `PressureTracker`         | ⚠ functional; consider promoting to trait in LCM phase |

### C. `context_compressor.py`

| Python feature                                          | Rust today                                     | Plan |
|---------------------------------------------------------|------------------------------------------------|------|
| `SUMMARY_PREFIX` header                                 | exists (`[CONTEXT COMPACTION — REFERENCE ONLY] ...`) | ✅ parity |
| Memory-authority reminder in summary preamble           | **needs verification** — check current Rust header text | **34b-02** (verify + patch if missing) |
| Iterative summary updates (preserve info across compactions) | exists                                    | ✅ |
| Token-budget tail protection                            | exists (`protect_first_n`, `protect_last_tokens`) | ✅ |
| Tool output pruning before summarization                | exists                                         | ✅ |
| `on_session_reset()` clears counters                    | none                                           | **34b-02** |
| `focus_topic` argument (user-guided compression)        | none                                           | deferred (LCM-adjacent) |
</parity_matrix>

<success_criteria>
What MUST be true at phase completion:

### 34b-01 acceptance:
1. **`crates/ironhermes-agent/src/context_refs.rs`** is created and exports:
   - `pub fn parse_context_references(message: &str) -> Vec<ContextReference>`
   - `pub async fn preprocess_context_references_async(message: &str, cwd: &Path, context_length: usize, url_fetcher: Option<UrlFetcher>, allowed_root: Option<&Path>) -> ContextReferenceResult`
   - `pub struct ContextReference { raw, kind, target, start, end, line_start, line_end }`
   - `pub struct ContextReferenceResult { message, original_message, references, warnings, injected_tokens, expanded, blocked }`

2. **Regex matches Python byte-for-byte:**
   - `@diff` and `@staged` (no value)
   - `@file:<value>[:start[-end]]`
   - `@folder:<value>`
   - `@git:<N>` (1–10 commits)
   - `@url:<value>`
   - Quoted values: `` `path` `` `"path"` `'path'`
   - Trailing-punctuation stripping (`,.;!?` and balanced-paren-aware)

3. **Sensitive-path blocklist** rejects expansion for every entry: `.ssh/`,
   `.aws/`, `.gnupg/`, `.kube/`, `.docker/`, `.azure/`, `.config/gh/` (dirs),
   `.ssh/authorized_keys`, `.ssh/id_rsa`, `.ssh/id_ed25519`, `.ssh/config`,
   `.bashrc`, `.zshrc`, `.profile`, `.bash_profile`, `.zprofile`, `.netrc`,
   `.pgpass`, `.npmrc`, `.pypirc` (files), `$HERMES_HOME/.env`,
   `$HERMES_HOME/skills/.hub/`. Returns a structured `"path is a sensitive
   credential file and cannot be attached"` warning in the result; original
   message preserved when ALL refs are blocked.

4. **Budget enforcement:**
   - `hard_limit = context_length * 0.50` — exceeded → `result.blocked = true`,
     all expansions stripped, single warning surfaced, `result.message` ==
     `result.original_message`.
   - `soft_limit = context_length * 0.25` — exceeded → warning surfaced,
     expansions still applied.

5. **Output format mirrors Python:**
   - Refs stripped from inline message text.
   - Warnings prepended as `--- Context Warnings ---\n- {warning1}\n- {warning2}`.
   - Expansions appended as `--- Attached Context ---\n\n📄 @file:foo.rs (N tokens)\n```rust\n{slice}\n```\n\n🧾 git diff (N tokens)\n```diff\n{output}\n````.

6. **Three-surface wiring** — every user message in CLI `run_chat`, gateway
   `handle_with_multimodal`, and web UI `run_web_turn` runs through
   `preprocess_context_references_async` BEFORE going to the agent loop.
   `result.message` replaces the user-facing text; `result.warnings` get
   logged. When `result.blocked == true`, the user sees the warning block.

7. **Test count:** minimum 14 unit tests in `context_refs::tests` —
   - 6 parser tests (simple, kind:value, quoted, line range, trailing punct, multiple refs)
   - 5 expander tests (file, file-with-range, folder listing, diff, url stub)
   - 1 hard-limit budget test
   - 1 soft-limit warning test
   - 1 sensitive-path blocklist test (parameterised over every entry)

### 34b-02 acceptance:
8. **`ContextEngine` trait gains 5 lifecycle hooks** with default no-op
   bodies (existing implementors don't need to change):
   - `fn on_session_start(&self, session_id: &str)` (default no-op)
   - `fn on_session_reset(&self)` (default no-op)
   - `fn update_from_response(&self, usage: &UsageReport)` (default no-op)
   - `fn update_model(&self, model: &str, context_length: usize, base_url: Option<&str>)` (default no-op)
   - `fn has_content_to_compress(&self, messages: &[ChatMessage]) -> bool { true }`

9. **`ContextCompressor` and `SummarizingEngine` override `on_session_reset`**
   to clear `last_prompt_tokens`, `last_completion_tokens`,
   `last_total_tokens`, `compression_count`, and any internal pressure-tracker
   state. Unit test verifies the clear.

10. **3-surface session-boundary wiring:**
    - CLI `run_chat`: call `engine.on_session_start(&session_id)` at REPL
      start; call `engine.on_session_reset()` from `/new` and `/reset`
      command handlers.
    - Gateway `handle_with_multimodal`: call `on_session_start` when a new
      `SessionKey` is allocated; `on_session_reset` from `/reset` slash.
    - Web UI `run_web_turn`: call `on_session_start` on WebSocket connect;
      `on_session_reset` on new-chat request.

11. **Memory-authority reminder in compaction summary.** Read the current
    Rust compaction header text in `context_compressor.rs`. If it does NOT
    include the Python equivalent of *"Your persistent memory (MEMORY.md,
    USER.md) in the system prompt is ALWAYS authoritative — never ignore or
    deprioritize memory content due to this compaction note"*, patch the
    header to include it. Add a unit test asserting the header contains both
    "MEMORY.md" and "ALWAYS authoritative" (or the agreed equivalent).

### Cross-phase regression:
12. Phase 32 `nudge::tests`: 6/6
13. Phase 33 `invariants_33`: 6/6
14. Phase 34a `memory_context::tests` + `streaming_scrubber::tests`: passing
15. D-12: `test_snapshot_frozen_after_load` still green
</success_criteria>

<plans>
Two plans, parallel waves (34b-01 and 34b-02 share no files).

<plan id="34b-01" wave="1" depends_on="['34a']">
**Title:** `@`-reference expansion module + sensitive-path blocklist + budget enforcement + 3-surface wiring

**Files modified:**
- `crates/ironhermes-agent/src/context_refs.rs` — NEW; parser + expander + blocklist + budget logic + 14 unit tests.
- `crates/ironhermes-agent/src/lib.rs` — `pub mod context_refs`.
- `crates/ironhermes-cli/src/main.rs` — call `preprocess_context_references_async` on each user message in `run_chat` before agent dispatch.
- `crates/ironhermes-gateway/src/handler.rs` — same call in `handle_with_multimodal`.
- `crates/iron_hermes_ui/src/server/state.rs` — same call in `run_web_turn`.

**Acceptance:** see #1–7 above.
</plan>

<plan id="34b-02" wave="1" depends_on="['34a']">
**Title:** ContextEngine lifecycle hook parity + ContextCompressor reset + memory-authority reminder

**Files modified:**
- `crates/ironhermes-agent/src/context_engine.rs` — add 5 lifecycle methods to the trait with default impls.
- `crates/ironhermes-agent/src/context_compressor.rs` — override `on_session_reset`; verify + patch memory-authority reminder in compaction header.
- `crates/ironhermes-agent/src/summarizing_engine.rs` — override `on_session_reset`.
- `crates/ironhermes-cli/src/main.rs` — wire `on_session_start` / `on_session_reset` into REPL start, `/new`, `/reset`.
- `crates/ironhermes-gateway/src/handler.rs` — wire into session allocation + `/reset` slash.
- `crates/iron_hermes_ui/src/server/state.rs` — wire into WebSocket connect + new-chat.

**Acceptance:** see #8–11 above.
</plan>
</plans>

<verification_recipe>
```bash
cargo build --workspace

# 34b-01 unit tests
cargo test -p ironhermes-agent --lib context_refs::tests          # 14+/14+

# 34b-02 unit tests
cargo test -p ironhermes-agent --lib context_engine::tests
cargo test -p ironhermes-agent --lib context_compressor::tests

# Cross-phase gates
cargo test -p ironhermes-agent --lib memory_context::tests        # 34a
cargo test -p ironhermes-agent --lib streaming_scrubber::tests    # 34a
cargo test -p ironhermes-agent --lib nudge::tests                 # 32
cargo test -p ironhermes-agent --test invariants_33               # 33
cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load   # D-12

# Live: @-reference expansion
hermes chat
# > summarize @file:README.md
# Expected: agent receives expanded README; user-visible message has refs stripped.
# > read @file:.ssh/id_rsa
# Expected: "--- Context Warnings ---\n- path is a sensitive credential file..."
# > overload context with @folder:.
# Expected at >50% budget: "@ context injection refused: N tokens exceeds the 50% hard limit (M)."

# Live: session reset clears counters
hermes chat
# > /reset
# Then check `hermes status` or whatever surfaces compression_count — should be 0.
```
</verification_recipe>

<deferred>
- `focus_topic` argument on `compress(...)` (user-guided `/compress <focus>`) — adjacent to LCM phase
- `ContextEngine.get_tool_schemas()` / `handle_tool_call()` for LCM tools (`lcm_grep`, `lcm_describe`, `lcm_expand`) — LCM phase
- Promoting `last_prompt_tokens` / `compression_count` / `context_length` from external `PressureTracker` to trait-level fields — LCM phase (may need a new associated type)
- Multi-provider teardown order (Python iterates providers reverse on shutdown) — when a second external provider lands
- `MemoryProvider.on_turn_start` / `on_session_switch` / `on_delegation` — separate memory-lifecycle phase
- "Only one external memory provider" registration guard — same memory-lifecycle phase
- `MemoryProvider.on_pre_compress` returns text contribution — same
</deferred>

<open_questions_for_discuss_phase>
1. **URL fetcher** for `@url:`. Python uses `web_extract_tool` with markdown
   format + LLM processing. Rust has `web_extract` tool too — should the
   `@url:` expansion call into the tool directly (synchronous, possibly with
   LLM call) or via a lighter HTTP-only path? LLM processing makes `@url:`
   slow on every turn but produces cleaner content. Pick a default.

2. **`allowed_root` default.** Python defaults to `cwd` so `@file:`
   references can't escape the active workspace unless a caller widens the
   root. Mirror that in Rust, or widen to `$HOME` by default (more
   convenient, larger blast radius)? Recommend keeping cwd.

3. **`ContextEngine` trait method placement.** The 5 new lifecycle methods
   (start/reset/update_from_response/update_model/has_content_to_compress)
   can go on the existing `ContextEngine` trait (additive default impls) or
   on a separate `ContextEngineLifecycle` trait. The Python equivalent has
   them on the same ABC; recommend the additive approach.

4. **Memory-authority reminder.** Verify the current Rust compaction header
   text in `context_compressor.rs` SUMMARY_PREFIX equivalent first — if it
   already includes the reminder, drop that part of Plan 34b-02. Cheap to
   check pre-discuss.

5. **Compression integration with Phase 34a synthetic-system-messages.** The
   recall-context messages injected per-turn (34a) are ephemeral. Compressor
   should skip them (don't summarize them; the next turn re-injects fresh
   recall). Need a way to tag them — ties back to the 34a discuss-phase
   question about ChatMessage metadata.
</open_questions_for_discuss_phase>
