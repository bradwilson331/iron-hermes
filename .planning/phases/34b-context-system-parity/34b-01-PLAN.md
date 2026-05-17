---
phase: 34b-context-system-parity
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - crates/ironhermes-agent/src/context_refs.rs
  - crates/ironhermes-agent/src/lib.rs
  - crates/ironhermes-agent/Cargo.toml
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-gateway/src/handler.rs
  - crates/iron_hermes_ui/src/server/state.rs
autonomous: true
requirements:
  - CTX-REF-01
  - CTX-REF-02
tags:
  - context-references
  - parser
  - security
  - 3-surface-wiring

must_haves:
  truths:
    - "A chat message containing `@file:README.md` causes the agent to receive the README contents inside a `--- Attached Context ---` footer; the inline message no longer contains the `@file:` token."
    - "A chat message containing `@file:.ssh/id_rsa` causes a `--- Context Warnings ---` block with the sensitive-credential message; the file is NOT read."
    - "A chat message whose expansion would exceed 50% of the configured context window returns `blocked=true`, the original message unchanged, and a single hard-limit warning."
    - "All three surfaces (CLI run_chat, gateway handle_with_multimodal, web UI run_web_turn) call preprocess_context_references_async on the user message before AgentLoop::run."
    - "@url: expansion calls WebExtractTool with use_llm_processing=true; falls back to use_llm_processing=false on error and surfaces a warning."
  artifacts:
    - path: "crates/ironhermes-agent/src/context_refs.rs"
      provides: "Reference parser + expander + blocklist + budget enforcement + preprocess_context_references_async"
      min_lines: 400
    - path: "crates/ironhermes-agent/src/lib.rs"
      provides: "pub mod context_refs registration"
      contains: "pub mod context_refs"
    - path: "crates/ironhermes-cli/src/main.rs"
      provides: "CLI surface preprocessor call site"
      contains: "context_refs::preprocess_context_references_async"
    - path: "crates/ironhermes-gateway/src/handler.rs"
      provides: "Gateway surface preprocessor call site"
      contains: "context_refs::preprocess_context_references_async"
    - path: "crates/iron_hermes_ui/src/server/state.rs"
      provides: "Web UI surface preprocessor call site"
      contains: "context_refs::preprocess_context_references_async"
  key_links:
    - from: "crates/ironhermes-agent/src/context_refs.rs"
      to: "crates/ironhermes-tools/src/web_extract.rs"
      via: "Arc<WebExtractTool>::execute(serde_json::Value)"
      pattern: "use_llm_processing"
    - from: "crates/ironhermes-cli/src/main.rs"
      to: "context_refs::preprocess_context_references_async"
      via: "await before run_agent_turn"
      pattern: "preprocess_context_references_async"
    - from: "crates/ironhermes-gateway/src/handler.rs"
      to: "context_refs::preprocess_context_references_async"
      via: "await before agent.run(messages)"
      pattern: "preprocess_context_references_async"
    - from: "crates/iron_hermes_ui/src/server/state.rs"
      to: "context_refs::preprocess_context_references_async"
      via: "await before agent.run(messages)"
      pattern: "preprocess_context_references_async"
---

<objective>
Port Python's `context_references.py` to Rust as a new `context_refs` module in `ironhermes-agent`, then wire it at all three agent-execution surfaces so user messages with `@file:`, `@folder:`, `@diff`, `@staged`, `@git:N`, or `@url:` tokens are expanded into attached-context blocks before the agent loop runs.

Purpose: bring user-facing `@`-reference UX to parity with hermes-agent. Today, a user typing `@file:README.md` sends the literal string; after this plan, the agent receives the file contents.

Output: a new module file with parser + expander + sensitive-path blocklist + 50%/25% token budget enforcement + 14+ unit tests, plus three call-site edits that thread the preprocessor before the agent dispatch.

Security: sensitive-path blocklist (D-03/D-04, T-34b-01-PATH), token-budget hard reject (T-34b-01-DOS), WebExtractTool-only fetch for @url (T-34b-01-SSRF), no shell expansion for git subcommands (T-34b-01-SHELL).
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/STATE.md
@.planning/phases/34b-context-system-parity/34B-CONTEXT.md
@.planning/phases/34b-context-system-parity/34B-RESEARCH.md
@.planning/phases/34b-context-system-parity/34B-PATTERNS.md

# Python reference implementation (canonical port target)
@../hermes-agent/agent/context_references.py

# Rust analogs (read first for patterns)
@crates/ironhermes-agent/src/nudge.rs
@crates/ironhermes-tools/src/web_extract.rs

# Surfaces being modified
@crates/ironhermes-agent/src/lib.rs
@crates/ironhermes-cli/src/main.rs
@crates/ironhermes-gateway/src/handler.rs
@crates/iron_hermes_ui/src/server/state.rs

<interfaces>
<!-- Key types and contracts the executor needs. Extracted from codebase. -->
<!-- Executor should use these directly — no codebase exploration needed. -->

From crates/ironhermes-tools/src/web_extract.rs (already wired in all 3 surfaces):
- `WebExtractTool::execute(args: serde_json::Value) -> anyhow::Result<String>` where args is
  `{ "urls": [String], "use_llm_processing": bool, "format": "markdown" }`. Returns a JSON string
  whose payload is an `ExtractionResult` (`{ success, url, content, title, error, ... }`).
- The tool is constructed inside `AppRuntimeBundle::build` and exposed via the existing
  `Arc<dyn ToolRegistry>` wired into each surface; the call site for this plan accepts an
  `Option<Arc<WebExtractTool>>` directly, threaded from the same construction site.

From crates/ironhermes-core/src/constants.rs:61:
- `pub fn get_hermes_home() -> PathBuf` — reads `IRONHERMES_HOME` env var; this is the
  Rust equivalent of Python's `hermes_constants.get_hermes_home()` (RESEARCH §1.2 A2 closed).

From crates/ironhermes-core/src/config.rs (`TerminalConfig` under `AgentConfig.terminal`):
- `pub cwd: Option<PathBuf>` — resolves `allowed_root` per D-05 (TerminalConfig.cwd first,
  else `std::env::current_dir()`).

From crates/ironhermes-agent/src/context_compressor.rs:
- `pub fn estimate_tokens(text: &str) -> usize` — tiktoken-backed; used for budget enforcement.

From crates/ironhermes-core/src/types.rs:
- `pub struct ChatMessage { pub role: Role, pub content: Option<MessageContent>, ... }` — user
  message constructed by surface; preprocessor returns a `String` that the surface assigns into
  the last user message before agent dispatch.

From crates/ironhermes-agent/src/nudge.rs (module-structure analog):
- `//!` doc header pattern; `use anyhow::Result;` + `use tokio::process::Command;` imports;
  fire-and-forget `tokio::spawn` is NOT used here — the preprocessor runs inline (awaited).

Rust standard library (already used elsewhere in the workspace; no new crates required):
- `std::sync::LazyLock<Regex>` — used in `crates/ironhermes-core/src/ssrf.rs` and
  `skills.rs`. This replaces `once_cell::sync::Lazy`; do NOT add `once_cell` as a new dep
  (research §10 outdated assumption — `once_cell` is NOT a workspace dep; `LazyLock` stable
  since Rust 1.80 IS in use).
</interfaces>

</context>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Create context_refs module — types, parser, sensitive-path blocklist (CTX-REF-01)</name>
  <files>crates/ironhermes-agent/src/context_refs.rs, crates/ironhermes-agent/src/lib.rs, crates/ironhermes-agent/Cargo.toml</files>
  <read_first>
    - ../hermes-agent/agent/context_references.py (canonical port target — full file)
    - crates/ironhermes-agent/src/nudge.rs (module-structure analog: doc header, imports, anyhow::Result error pattern)
    - crates/ironhermes-core/src/ssrf.rs (LazyLock<HashSet<&'static str>> pattern at line 19)
    - crates/ironhermes-core/src/skills.rs (LazyLock<Regex> pattern at line 23)
    - crates/ironhermes-core/src/constants.rs (get_hermes_home() at line 61)
    - crates/ironhermes-agent/src/lib.rs (module list — alphabetical insertion between context_loader and engine_factory)
    - crates/ironhermes-agent/Cargo.toml (verify regex workspace dep at line 38; confirm no once_cell dep)
  </read_first>
  <behavior>
    - parse_context_references("@diff") returns one ContextReference with kind=Diff, target="", raw="@diff".
    - parse_context_references("@staged") returns one ContextReference with kind=Staged.
    - parse_context_references("@file:src/main.rs") returns one ContextReference with kind=File, target="src/main.rs", line_start=None, line_end=None.
    - parse_context_references("@file:src/main.rs:10-25") returns line_start=Some(10), line_end=Some(25).
    - parse_context_references("@file:src/main.rs:42") returns line_start=Some(42), line_end=Some(42).
    - parse_context_references("@file:`path with spaces.rs`:1-10") strips backticks; target="path with spaces.rs"; line_start=Some(1).
    - parse_context_references("@file:foo.rs,") strips trailing comma; target="foo.rs".
    - parse_context_references("see https://example.com/@diff for") returns NO match (negative-lookbehind via post-match position check: prev char is "/" → reject).
    - parse_context_references("contact@example.com") returns NO match (prev char is alphanumeric → reject).
    - parse_context_references("@folder:src/") returns kind=Folder, target="src/".
    - parse_context_references("@git:5") returns kind=Git, target="5".
    - parse_context_references("@git:99") returns kind=Git, target clamped to "10" at expansion time (parser keeps raw "99"; clamping happens in Task 2 expander).
    - parse_context_references("@url:https://example.com") returns kind=Url, target="https://example.com".
    - is_sensitive_path() returns Err with "path is a sensitive credential file..." for every entry in SENSITIVE_HOME_FILES.
    - is_sensitive_path() returns Err for any path under SENSITIVE_HOME_DIRS prefix.
    - is_sensitive_path() returns Err for HERMES_HOME/.env (exact file) and HERMES_HOME/skills/.hub/ (prefix).
  </behavior>
  <action>
    Add `crates/ironhermes-agent/src/context_refs.rs` with module doc-header following the nudge.rs pattern (`//! Phase 34b Plan 01: @-reference parser ...`). Add `pub mod context_refs;` to `crates/ironhermes-agent/src/lib.rs` in alphabetical order between existing `pub mod context_loader;` and `pub mod engine_factory;` lines. Do NOT add an `pub use` re-export from lib.rs — callers use the fully-qualified path `ironhermes_agent::context_refs::*`.

    Use `std::sync::LazyLock<regex::Regex>` for the static `REFERENCE_PATTERN` (NOT `once_cell::sync::Lazy` — `once_cell` is not a workspace dep; `LazyLock` is the established codebase pattern from ssrf.rs and skills.rs). Verify `regex = { workspace = true }` is already in `crates/ironhermes-agent/Cargo.toml` line 38 — no Cargo.toml edit needed unless the dep is missing. If missing, add it as `regex = { workspace = true }`.

    Define `pub enum RefKind { Diff, Staged, File, Folder, Git, Url }` with `#[derive(Debug, Clone, PartialEq, Eq)]`. Define `pub struct ContextReference` with public fields `raw: String, kind: RefKind, target: String, start: usize, end: usize, line_start: Option<u32>, line_end: Option<u32>` and `#[derive(Debug, Clone)]`. Define `pub struct ContextReferenceResult` with public fields `message: String, original_message: String, references: Vec<ContextReference>, warnings: Vec<String>, injected_tokens: usize, expanded: bool, blocked: bool` and `#[derive(Debug, Clone, Default)]`. Add `impl ContextReferenceResult { pub fn passthrough(message: String) -> Self }` returning a result with `message == original_message == message` and `expanded=false, blocked=false`.

    Define the `REFERENCE_PATTERN` regex matching Python's pattern from context_references.py lines 17-19. Use this Rust regex string (anchored without lookbehind — lookbehind enforced by post-match check below):

    `@(?:(?P<simple>diff|staged)\b|(?P<kind>file|folder|git|url):(?P<value>(?:` + backtick + `[^` + backtick + `\n]+` + backtick + `|"[^"\n]+"|\x27[^\x27\n]+\x27)(?::\d+(?:-\d+)?)?|\S+))`

    Implement `pub fn parse_context_references(message: &str) -> Vec<ContextReference>` that iterates `REFERENCE_PATTERN.captures_iter(message)`. For each match, enforce Python's negative-lookbehind `(?<![\w/])` manually: if `m.start() > 0`, inspect `message[..m.start()].chars().last().unwrap()`; if it is `is_alphanumeric() || == '_' || == '/'`, continue (skip the match). Strip trailing punctuation from the matched value: characters in the set `,.;!?` and unbalanced `)]}`. Quoted-value handling: if the value starts with backtick/double-quote/single-quote, strip the outer pair before parsing line range. Line range parsing: if remainder after target contains `:N` or `:N-M`, populate `line_start` / `line_end` (single `:N` → both equal). Return `Vec<ContextReference>` in match order.

    Define `SENSITIVE_HOME_DIRS: &[&str]` = `[".ssh", ".aws", ".gnupg", ".kube", ".docker", ".azure", ".config/gh"]`. Define `SENSITIVE_HOME_FILES: &[&str]` = `[".ssh/authorized_keys", ".ssh/id_rsa", ".ssh/id_ed25519", ".ssh/config", ".bashrc", ".zshrc", ".profile", ".bash_profile", ".zprofile", ".netrc", ".pgpass", ".npmrc", ".pypirc"]`. Implement `pub fn is_sensitive_path(path: &Path, home: &Path, hermes_home: &Path) -> Result<(), String>`: return `Err("...")` if the canonicalized path matches an entry under home OR is `hermes_home.join(".env")` OR has prefix `hermes_home.join("skills/.hub/")`. Error messages: `"path is a sensitive credential file and cannot be attached"` for exact files and `"path is a sensitive credential or internal Hermes path and cannot be attached"` for directory prefix matches (per RESEARCH §1.2). Use `get_hermes_home()` from `ironhermes_core::constants` for the hermes_home argument resolution in callers; the function itself takes `hermes_home: &Path` to remain testable.

    Test module in `#[cfg(test)] mod tests` at the bottom of context_refs.rs. Minimum 8 tests in this task:
    - `test_parse_simple_diff`, `test_parse_simple_staged`, `test_parse_kind_value` (file/folder/git/url variants)
    - `test_parse_quoted_path`, `test_parse_line_range`, `test_parse_trailing_punctuation`
    - `test_lookbehind_rejects_url_path` (asserts `https://x.com/@diff` does NOT match)
    - `test_sensitive_path_blocklist` (parameterised: iterates SENSITIVE_HOME_FILES and SENSITIVE_HOME_DIRS, asserts each returns Err with expected message). Use `tempfile::TempDir` to create a fake home/hermes_home so paths exist.
  </action>
  <verify>
    <automated>cargo build -p ironhermes-agent && cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast 2>&1 | tee /tmp/34b-01-task1.log; grep -E "test result|FAILED|passed" /tmp/34b-01-task1.log; grep -q "pub mod context_refs" crates/ironhermes-agent/src/lib.rs && echo "lib.rs export OK"; grep -q "LazyLock" crates/ironhermes-agent/src/context_refs.rs && echo "uses LazyLock (not once_cell) OK"; ! grep -q "once_cell" crates/ironhermes-agent/Cargo.toml && echo "no once_cell dep OK"</automated>
  </verify>
  <acceptance_criteria>
    - `crates/ironhermes-agent/src/context_refs.rs` exists
    - `grep -c "pub fn parse_context_references" crates/ironhermes-agent/src/context_refs.rs` returns 1
    - `grep -c "pub enum RefKind" crates/ironhermes-agent/src/context_refs.rs` returns 1
    - `grep -c "pub struct ContextReference" crates/ironhermes-agent/src/context_refs.rs` returns 2 (struct ContextReference + struct ContextReferenceResult)
    - `grep -c "SENSITIVE_HOME_DIRS\|SENSITIVE_HOME_FILES" crates/ironhermes-agent/src/context_refs.rs` returns at least 2 (definitions) and matches at use sites
    - `grep -v '^//' crates/ironhermes-agent/src/context_refs.rs | grep -c "once_cell"` returns 0 (must use LazyLock)
    - `grep -c "^pub mod context_refs;" crates/ironhermes-agent/src/lib.rs` returns 1
    - `cargo build -p ironhermes-agent` exits 0
    - `cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast` exits 0
    - test output contains at least 8 passing tests with names: `test_parse_simple_diff`, `test_parse_simple_staged`, `test_parse_kind_value`, `test_parse_quoted_path`, `test_parse_line_range`, `test_parse_trailing_punctuation`, `test_lookbehind_rejects_url_path`, `test_sensitive_path_blocklist`
  </acceptance_criteria>
  <done>The parser, types, and blocklist exist as a self-contained module with passing tests. No expansion or async surface yet — Task 2 adds that. The module compiles, builds cleanly, and is registered in lib.rs.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: Expander, budget enforcement, async preprocessor entry point (CTX-REF-02)</name>
  <files>crates/ironhermes-agent/src/context_refs.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/context_refs.rs (current state from Task 1)
    - ../hermes-agent/agent/context_references.py (expand_context_references, preprocess_context_references_async — output format, budget logic, @folder rg fallback)
    - crates/ironhermes-tools/src/web_extract.rs (lines 174-195: WebExtractTool::execute signature, ExtractionResult JSON shape)
    - crates/ironhermes-agent/src/context_compressor.rs (lines 1-50: estimate_tokens import path)
  </read_first>
  <behavior>
    - expand_file(path with 50 lines, line_start=10, line_end=25) returns a block formatted `📄 @file:path:10-25 (N tokens)\n```{lang}\n{slice lines 10..=25}\n```` where lang is inferred from extension.
    - expand_file on a binary file returns Err that the preprocessor surfaces as a warning (no panic, no expansion).
    - expand_file outside allowed_root returns Err with "path escapes allowed root" message; the preprocessor surfaces it as a warning.
    - expand_folder(temp dir with 2 files) returns `📁 @folder:... (N tokens)\n{listing}` where listing has each file on its own line with line count, ≤ 200 entries, truncation marker "- ..." appended if hit.
    - expand_git_log(N=3) calls `git log -3 -p` via tokio::process::Command; expand_diff calls `git diff`; expand_staged calls `git diff --staged`. Output formatted as `🧾 {label} (N tokens)\n```diff\n{output}\n```` (label per Python: "git diff", "git diff --staged", "git log -N").
    - expand_url(url, Some(tool)) calls tool.execute with use_llm_processing=true. On Ok(json_str), parses `content` field; returns `🌐 @url:{url} (N tokens)\n{content}`. On Err(_), retries once with use_llm_processing=false and pushes a warning "@url:{url}: LLM processing failed ({e}), using raw content". If second call also fails, returns Err for the preprocessor to surface as a warning.
    - expand_url(url, None) returns Err("@url: WebExtractTool not configured") which the preprocessor turns into a warning.
    - preprocess_context_references_async("@file:foo.rs blocked content", ctx_len=1000, allowed_root=cwd, tool=None) where foo.rs is .ssh/id_rsa-equivalent returns ContextReferenceResult with blocked=false, expanded=false, warnings=[sensitive-path message], message includes "--- Context Warnings ---" but NO "--- Attached Context ---" block.
    - preprocess_context_references_async with a 1000-token context and a folder expansion that injects 600 tokens (>50% hard limit=500) returns ContextReferenceResult with blocked=true, expanded=false, message==original_message, warnings=["@ context injection refused: 600 tokens exceeds the 50% hard limit (500)."].
    - preprocess_context_references_async with a 1000-token context and a 300-token injection (>25% soft limit=250, <50% hard=500) returns blocked=false, expanded=true, warnings contains "@ context injection warning: 300 tokens exceeds the 25% soft limit (250).".
    - Output assembly order: 1) stripped message (refs removed), 2) `\n\n--- Context Warnings ---\n` + each warning prefixed with "- ", 3) `\n\n--- Attached Context ---\n\n` + blocks joined by "\n\n", 4) trimmed.
  </behavior>
  <action>
    Extend `crates/ironhermes-agent/src/context_refs.rs` with expansion implementations.

    Add `infer_lang_from_ext(path: &Path) -> &'static str` mapping extensions to fence languages: `.rs`→"rust", `.py`→"python", `.ts`→"typescript", `.js`→"javascript", `.go`→"go", `.toml`→"toml", `.yaml`/`.yml`→"yaml", `.json`→"json", `.md`→"markdown", `.sh`→"bash", default→"". Mirror Python's mapping where present.

    Add `fn is_likely_binary(bytes: &[u8]) -> bool` — checks first 4096 bytes for null byte (`bytes.iter().take(4096).any(|&b| b == 0)`). Per RESEARCH §1.6 A3, this null-byte check is the Rust approach; no `infer` crate dep.

    Add async expansion helpers, each returning `anyhow::Result<String>` (block body for success; Err captured as warning by caller):
    - `async fn expand_file(reference: &ContextReference, allowed_root: &Path, home: &Path, hermes_home: &Path) -> Result<String>` — canonicalize path, call `is_sensitive_path`, check it has `allowed_root` prefix, read file via `tokio::fs::read`, run `is_likely_binary`, slice by line range if present, build the `📄` block with token-count estimate via `crate::context_compressor::estimate_tokens(content)`.
    - `async fn expand_folder(reference: &ContextReference, allowed_root: &Path) -> Result<String>` — canonicalize, prefix-check vs allowed_root, try `tokio::process::Command::new("rg").args(["--files", path]).output().await`; on success, take stdout. On failure or rg-absent (output.status non-zero OR spawn error), fall back to manual recursion via `tokio::fs::read_dir` skipping hidden dirs (`.` prefix) and `__pycache__`. Cap at 200 entries; if cap hit, append "- ..." line. Build the `📁` block.
    - `async fn expand_diff() -> Result<String>` — run `tokio::process::Command::new("git").args(["diff"]).output().await`, take stdout. Label "git diff".
    - `async fn expand_staged() -> Result<String>` — same with `git diff --staged`. Label "git diff --staged".
    - `async fn expand_git_log(n: u32) -> Result<String>` — clamp `n` to range 1..=10; run `git log -{n} -p`. Label `git log -{n}`. Pass `n` as a separate arg (NOT as a format-string injected into a shell — `tokio::process::Command` does not invoke a shell, T-34b-01-SHELL).
    - `async fn expand_url(reference: &ContextReference, tool: Option<Arc<WebExtractTool>>) -> Result<(String, Vec<String>)>` — returns `(block, warnings)` because the LLM-fallback flow needs to emit a warning. Build args JSON per D-01: `serde_json::json!({"urls": [reference.target], "use_llm_processing": true, "format": "markdown"})`. Call `tool.execute(args).await`; on Ok, parse the JSON response, extract `content`/`title`, build the `🌐 @url:{url} ({N} tokens)\n{content}` block, return `(block, vec![])`. On Err(e), retry with `use_llm_processing: false`, push the LLM-failure warning, return `(block, warnings)`. If both fail, return Err.

    Add the main entry point: `pub async fn preprocess_context_references_async(message: &str, context_length: usize, allowed_root: &Path, web_extract_tool: Option<Arc<WebExtractTool>>) -> Result<ContextReferenceResult>`.

    Body:
    1. `let references = parse_context_references(message);` — if empty, return `ContextReferenceResult::passthrough(message.to_string())` immediately.
    2. Resolve `home: PathBuf = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))` and `hermes_home: PathBuf = ironhermes_core::constants::get_hermes_home();`. If `dirs` is not already in workspace deps, use `std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))` instead — verify in Cargo.toml.
    3. Iterate references, dispatch on `RefKind`, accumulate `(block, warnings)` per reference; per-reference errors → push to warnings, no abort.
    4. Strip references from inline message: walk references in reverse order (so byte offsets stay valid) and remove `message[ref.start..ref.end]` from a `String::from(message)`. Normalize whitespace via two regex passes: `\s{2,}` → " ", then `\s+([,.;:!?])` → "$1". Trim final result. This is the `stripped` value.
    5. Compute `injected_tokens` by summing `estimate_tokens(block_body)` for every successful block.
    6. `let hard_limit = (context_length / 2).max(1); let soft_limit = (context_length / 4).max(1);`.
    7. If `injected_tokens > hard_limit`: return `ContextReferenceResult { message: message.to_string(), original_message: message.to_string(), references, warnings: vec![format!("@ context injection refused: {} tokens exceeds the 50% hard limit ({}).", injected_tokens, hard_limit)], injected_tokens, expanded: false, blocked: true }`.
    8. If `injected_tokens > soft_limit`: push the soft-limit warning to the accumulated warnings list.
    9. Assemble final `message` per RESEARCH §1.4 order: start with `stripped`, append `\n\n--- Context Warnings ---\n` + each `format!("- {}\n", w)` if warnings non-empty, append `\n\n--- Attached Context ---\n\n` + `blocks.join("\n\n")` if blocks non-empty, then `.trim().to_string()`.
    10. Return `ContextReferenceResult { message, original_message: message.to_string(), references, warnings, injected_tokens, expanded: !blocks.is_empty(), blocked: false }`.

    Add at least 6 more tests (total ≥ 14):
    - `test_expand_file_full` — tempdir, write 10-line file, expand without range → block contains all 10 lines with `📄` prefix and `(N tokens)`.
    - `test_expand_file_with_range` — same file, range 2-4 → block contains exactly lines 2..=4.
    - `test_expand_folder` — tempdir with 2 files → listing block.
    - `test_hard_limit_blocks_all` — context_length=100, mock expansion producing 51+ tokens → blocked=true, message==original_message.
    - `test_soft_limit_warns` — context_length=200, expansion producing 51-100 tokens → expanded=true, warnings non-empty.
    - `test_output_format_assembly` — message with mix of warnings + blocks → asserts `--- Context Warnings ---` before `--- Attached Context ---` in output, both present, original tokens stripped from inline text.

    For tests that require WebExtractTool, do NOT instantiate one (it requires SummarizationClientHandle wiring). Test `@url:` indirectly by passing `tool=None` and asserting the warning "@url: WebExtractTool not configured" is surfaced — wiring with a real tool is exercised at the surface integration sites in Task 3.

    No fenced code blocks here — code lives in the file produced by this task.
  </action>
  <verify>
    <automated>cargo build -p ironhermes-agent && cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast 2>&1 | tee /tmp/34b-01-task2.log; grep -E "test result|FAILED" /tmp/34b-01-task2.log; TEST_COUNT=$(grep -v '^//' /tmp/34b-01-task2.log | grep -cE "^test context_refs::tests::test_" || echo 0); echo "Test count: $TEST_COUNT (must be >= 14)"; [ "$TEST_COUNT" -ge 14 ] && echo "TEST COUNT OK" || echo "TEST COUNT INSUFFICIENT"</automated>
  </verify>
  <acceptance_criteria>
    - `grep -c "pub async fn preprocess_context_references_async" crates/ironhermes-agent/src/context_refs.rs` returns 1
    - `grep -c "async fn expand_file\|async fn expand_folder\|async fn expand_diff\|async fn expand_staged\|async fn expand_git_log\|async fn expand_url" crates/ironhermes-agent/src/context_refs.rs` returns 6
    - `grep -c "tokio::process::Command::new(\"git\")\|tokio::process::Command::new(\"rg\")" crates/ironhermes-agent/src/context_refs.rs` returns at least 4 (git diff, git diff --staged, git log, rg --files)
    - `grep -c "use_llm_processing.*true\|use_llm_processing.*false" crates/ironhermes-agent/src/context_refs.rs` returns at least 2 (LLM path + fallback)
    - `grep -c "context injection refused\|context injection warning" crates/ironhermes-agent/src/context_refs.rs` returns at least 2 (hard + soft messages)
    - `grep -c "--- Context Warnings ---\|--- Attached Context ---" crates/ironhermes-agent/src/context_refs.rs` returns at least 2 (footer constants)
    - `cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast` exits 0 with at least 14 tests passing
    - Test names include all 6 new ones: `test_expand_file_full`, `test_expand_file_with_range`, `test_expand_folder`, `test_hard_limit_blocks_all`, `test_soft_limit_warns`, `test_output_format_assembly`
  </acceptance_criteria>
  <done>The context_refs module is feature-complete: parses all 6 token types, expands each via async helpers, enforces sensitive-path blocklist and 50%/25% token budget, and emits the canonical Python-parity output format. 14+ tests pass. The module is ready for surface wiring in Task 3.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 3: Wire preprocess_context_references_async into all 3 agent surfaces</name>
  <files>crates/ironhermes-cli/src/main.rs, crates/ironhermes-gateway/src/handler.rs, crates/iron_hermes_ui/src/server/state.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/context_refs.rs (the module produced by Tasks 1-2; entry point signature)
    - crates/ironhermes-cli/src/main.rs (specifically lines 1750-1800 around the user-message push and run_agent_turn dispatch; also REPL loop init around line 2350-2400)
    - crates/ironhermes-gateway/src/handler.rs (specifically the run_agent function around line 999-1040 where agent.run is called)
    - crates/iron_hermes_ui/src/server/state.rs (run_web_turn function around line 144-170; ensure_web_session around 123)
    - crates/ironhermes-agent/src/agent_loop.rs (lines 94-107 for AggregatedUsage and AgentResult.total_usage shape)
    - crates/ironhermes-tools/src/web_extract.rs (verify WebExtractTool is accessible from AppRuntimeBundle — already wired in all 3 surfaces)
  </read_first>
  <behavior>
    - When `hermes chat` is invoked and the user types `summarize @file:Cargo.toml`, the message that reaches `AgentLoop::run` has `@file:Cargo.toml` stripped from the inline text and a `--- Attached Context ---\n\n📄 @file:Cargo.toml (N tokens)\n` block appended.
    - When the gateway receives a Telegram message `@file:README.md what is this`, the agent_loop sees the README contents in the message body before invoking the LLM.
    - When the Web UI receives a WebSocket `ChatRequest` whose `message` contains `@file:src/main.rs:1-10`, the agent receives the first 10 lines of src/main.rs as attached context.
    - At each surface, if preprocess_context_references_async returns blocked=true, the user-facing reply or the next streamed delta surfaces the hard-limit warning (the surface inspects `result.blocked` and logs it; the agent still receives `result.message` which equals the original).
    - At each surface, when `result.warnings` is non-empty, the warnings are emitted via `tracing::warn!(warning = %w, surface = "cli"|"gateway"|"web", "@-ref preprocessing emitted warning")` (do not also print to stdout — keep the user-message body itself as the carrier).
    - All three surfaces resolve `allowed_root` per D-05: `config.agent.terminal.cwd` if set, else `std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))`.
    - All three surfaces pass the WebExtractTool (Option<Arc<WebExtractTool>>) constructed during AppRuntimeBundle build.
    - When preprocess_context_references_async itself returns Err (catastrophic failure — should be impossible since per-ref errors are warnings), the surface logs `tracing::warn!(error = %e, "context_refs preprocessing failed")` and falls back to ContextReferenceResult::passthrough so the agent still receives the original message.
  </behavior>
  <action>
    For each of the three surfaces, insert exactly one call to `ironhermes_agent::context_refs::preprocess_context_references_async` immediately before the existing `AgentLoop::run` (or surface-equivalent) call, and replace the user-message text in the message vector with `result.message`. Pass through `result.warnings` to `tracing::warn!`.

    CLI — `crates/ironhermes-cli/src/main.rs`:

    Locate the `run_chat` function (around line 1070). Find the existing `messages.push(user_msg)` call site (around line 1764, per PATTERNS §CLI wiring) and the subsequent `Box::pin(run_agent_turn(...))` dispatch (around line 1774). Between these two, insert:
    - Compute `allowed_root: PathBuf = config.agent.terminal.cwd.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))`.
    - Resolve `context_length` from the same source used today for compression threshold (look for an existing `context_length` binding in scope or compute via `endpoint.context_length()`).
    - Resolve `web_extract_tool: Option<Arc<WebExtractTool>>` — find where AppRuntimeBundle wires WebExtractTool into the tool registry and thread an Arc<WebExtractTool> binding through `build_app_runtime_bundle`. If WebExtractTool is constructed inline in run_chat, capture the `Arc` immediately after construction. Pass `None` only if web_extract is statically disabled by config; otherwise pass `Some(arc.clone())`.
    - Call `let ref_result = ironhermes_agent::context_refs::preprocess_context_references_async(&input, context_length, &allowed_root, web_extract_tool.clone()).await.unwrap_or_else(|e| { tracing::warn!(error = %e, surface = "cli", "context_refs preprocessing failed"); ironhermes_agent::context_refs::ContextReferenceResult::passthrough(input.clone()) });`.
    - If `ref_result.expanded || ref_result.blocked`: replace the last user message: `if let Some(last) = messages.last_mut() { *last = ChatMessage::user(&ref_result.message); }`. Use `ChatMessage::user` if available, or construct via existing pattern.
    - Iterate `ref_result.warnings` and call `tracing::warn!(warning = %w, surface = "cli", "@-ref preprocessing emitted warning")` per entry. Do NOT also println — the message body itself carries `--- Context Warnings ---` for the agent.

    Gateway — `crates/ironhermes-gateway/src/handler.rs`:

    Locate the `run_agent` function. Find the existing `let messages_for_nudge = messages.clone();` (around line 1023, per PATTERNS) and the immediately following `let agent_result = agent.run(messages).await;` (line 1024). Between these, insert:
    - Compute `allowed_root: PathBuf` from `self.config.agent.terminal.cwd` (or the equivalent reachable from `self.config`) with the same fallback chain as CLI.
    - Resolve `context_length` from the same source used by the existing gateway engine wiring (look near `set_gateway_engine` call around line 275).
    - Resolve `web_extract_tool: Option<Arc<WebExtractTool>>` — if GatewayHandler does not already hold an `Arc<WebExtractTool>`, add a field `web_extract_tool: Option<Arc<WebExtractTool>>` to the GatewayHandler struct (search for existing handler fields like `context_engine: Option<Arc<dyn ContextEngine>>` at line 94 as the template). Wire it in the handler constructor from the AppRuntimeBundle.
    - Extract the user message text from the most recent `messages.last()` (it should be the User-role message just built; if multiple, the last User entry).
    - Call `preprocess_context_references_async` with the user text. Replace `messages.last_mut()`'s content with `ref_result.message` if `expanded || blocked`.
    - Log warnings via `tracing::warn!(surface = "gateway", ...)`.

    Web UI — `crates/iron_hermes_ui/src/server/state.rs`:

    Locate the `run_web_turn` function (around line 144). Find the existing `messages.clone()` snapshot for nudge (around line 156) and the subsequent `let result = agent.run(messages).await?;` (line 161). Between them, insert the same preprocessing pattern:
    - Compute `allowed_root` from `self.config.agent.terminal.cwd` or `std::env::current_dir()`.
    - Resolve `context_length` from the same source the rest of state.rs uses.
    - Add `web_extract_tool: Option<Arc<WebExtractTool>>` field to `AppState` if not already present (search for existing `nudge_turns: Arc<...>` field around line 40 as the template). Wire it in `AppState::new` from `build_app_runtime_bundle`.
    - Call `preprocess_context_references_async`, replace the last user message in `messages` with `ref_result.message`, log warnings.

    All three surfaces use the SAME function signature call shape — copy-paste with surface name differing only in the `surface = "..."` tracing tag.

    Verification helper: after the three insertions, the static grep `grep -rn "preprocess_context_references_async" crates/ironhermes-cli/src/main.rs crates/ironhermes-gateway/src/handler.rs crates/iron_hermes_ui/src/server/state.rs` returns exactly 3 matches (one per surface).

    No code blocks in this action — implementation lives in the three source files. No new test file in this task; live verification via `cargo build --workspace` + a manual smoke step in `<verify>`.
  </action>
  <verify>
    <automated>cargo build --workspace 2>&1 | tee /tmp/34b-01-task3.log; grep -E "error\[|^error:" /tmp/34b-01-task3.log && echo "BUILD ERRORS" || echo "BUILD OK"; CALL_COUNT=$(grep -rn "preprocess_context_references_async" crates/ironhermes-cli/src/main.rs crates/ironhermes-gateway/src/handler.rs crates/iron_hermes_ui/src/server/state.rs | grep -v "^Binary" | grep -vc "^[^:]*://"); echo "Surface call count: $CALL_COUNT (must be exactly 3)"; [ "$CALL_COUNT" -eq 3 ] && echo "WIRING COUNT OK" || echo "WIRING COUNT WRONG"</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build --workspace` exits 0
    - `grep -c "preprocess_context_references_async" crates/ironhermes-cli/src/main.rs` returns 1
    - `grep -c "preprocess_context_references_async" crates/ironhermes-gateway/src/handler.rs` returns 1
    - `grep -c "preprocess_context_references_async" crates/iron_hermes_ui/src/server/state.rs` returns 1
    - `grep -c "surface = \"cli\"" crates/ironhermes-cli/src/main.rs` returns at least 1
    - `grep -c "surface = \"gateway\"" crates/ironhermes-gateway/src/handler.rs` returns at least 1
    - `grep -c "surface = \"web\"" crates/iron_hermes_ui/src/server/state.rs` returns at least 1
    - `cargo test -p ironhermes-agent --lib context_refs::tests` still exits 0 (Tasks 1-2 tests preserved)
    - Phase 32 regression: `cargo test -p ironhermes-agent --lib nudge::tests` exits 0 (6/6)
    - Phase 34a regression: `cargo test -p ironhermes-agent --lib memory_context::tests streaming_scrubber::tests` exits 0
  </acceptance_criteria>
  <done>All three surfaces invoke preprocess_context_references_async on every user message before agent dispatch. The workspace builds cleanly. Context-refs unit tests still pass. Phase 32 + 34a regression gates green. The `@`-reference UX is live end-to-end.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| user-input → preprocessor | Untrusted message text crosses into a parser + path resolver + subprocess launcher. |
| preprocessor → filesystem | Parsed `@file:`/`@folder:` paths resolve to disk reads; must stay inside `allowed_root` and outside SENSITIVE_PATHS. |
| preprocessor → git subprocess | `@diff`, `@staged`, `@git:N` invoke `git` via `tokio::process::Command` with args (no shell). |
| preprocessor → WebExtractTool | `@url:` content is fetched via the existing tool which has its own SSRF gates. |
| agent context window | Hard 50% / soft 25% budget caps prevent malicious refs from displacing the conversation. |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-34b-01-PATH | Tampering / Information Disclosure | `expand_file` / `expand_folder` path resolution | mitigate | Canonicalize via `tokio::fs::canonicalize`; verify result has `allowed_root` prefix; reject before any read. SENSITIVE_PATHS blocklist is the second independent layer (D-04). |
| T-34b-01-SYMLINK | Tampering | `expand_file` symlink-escape | mitigate | `tokio::fs::canonicalize` resolves symlinks; prefix check against `allowed_root` runs on the canonical path. A symlinked `.ssh/id_rsa` inside workspace canonicalizes to the real path, triggering the blocklist. |
| T-34b-01-DOS | Denial of Service | budget enforcement | mitigate | Hard reject at 50% context window; soft warn at 25%; budget computed via tiktoken `estimate_tokens` (RESEARCH §1.3); folder listing capped at 200 entries; per-file size implicitly bounded by content tokenization happening before block assembly. |
| T-34b-01-SSRF | Information Disclosure (network) | `@url:` fetch | transfer (to WebExtractTool) | All HTTP traffic flows through `WebExtractTool::execute` (Phase 25.2). Direct HTTP from `context_refs.rs` is forbidden — only the tool reference is callable. WebExtractTool has its own SSRF gates (private-IP blocklist, allowlist host check). |
| T-34b-01-SHELL | Injection | git subcommands + rg | mitigate | `tokio::process::Command::new("git").args([...])` — no shell, no format-string injection. `n` for `@git:N` is parsed as `u32` from regex match, clamped to 1..=10 BEFORE the args vector is built. Path arguments are passed as separate strings, never interpolated. |
| T-34b-01-BIN | Information Disclosure | binary-file content leakage | mitigate | `is_likely_binary` null-byte scan on first 4096 bytes; binary files refused with warning, no content injected. |
| T-34b-01-SC | Tampering | npm/pip/cargo installs | accept | No new external packages introduced. `regex` and `tokio` are existing workspace deps; `LazyLock` is std. Package legitimacy gate not applicable. |

## Residual Risk

- Token budget uses tiktoken BPE in Rust vs `chars/4` in Python — limits enforce at slightly different thresholds (RESEARCH §8.4). Acceptable: limits are safety margins, not exact.
- `dirs::home_dir()` may return None on unusual systems; fallback to `/` makes the home blocklist a no-op for that process. Tradeoff acceptable for niche environments; primary defense is `allowed_root`.
</threat_model>

<verification>
After all three tasks complete:

```bash
# Unit tests for new module
cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast

# Full workspace builds
cargo build --workspace

# Cross-phase regression gates
cargo test -p ironhermes-agent --lib nudge::tests                                 # Phase 32
cargo test -p ironhermes-agent --test invariants_33                               # Phase 33
cargo test -p ironhermes-agent --lib memory_context::tests streaming_scrubber::tests   # Phase 34a
cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load                # D-12

# 3-surface wiring static-grep gate
grep -rn "preprocess_context_references_async" \
  crates/ironhermes-cli/src/main.rs \
  crates/ironhermes-gateway/src/handler.rs \
  crates/iron_hermes_ui/src/server/state.rs | wc -l
# Expected: 3
```
</verification>

<success_criteria>
1. `crates/ironhermes-agent/src/context_refs.rs` exists, is registered in `lib.rs`, exports `parse_context_references`, `preprocess_context_references_async`, `ContextReference`, `ContextReferenceResult`, and `RefKind` (CTX-REF-01).
2. Regex parser handles all 6 token types (`@diff`, `@staged`, `@file:`, `@folder:`, `@git:`, `@url:`) with quoted values, line ranges, trailing-punctuation stripping, and negative-lookbehind via post-match check (CTX-REF-01).
3. Sensitive-path blocklist rejects every entry in `SENSITIVE_HOME_DIRS` and `SENSITIVE_HOME_FILES` plus `$HERMES_HOME/.env` and `$HERMES_HOME/skills/.hub/` (CTX-REF-02).
4. Budget enforcement: hard 50% reject returns blocked=true with original_message; soft 25% adds warning but expands (CTX-REF-02).
5. `@url:` expansion calls `WebExtractTool::execute` with `use_llm_processing: true` (D-01) and falls back to false on failure with a warning (D-02) (CTX-REF-02).
6. 14+ unit tests pass in `context_refs::tests`.
7. All three surfaces (CLI/gateway/web) invoke the preprocessor before agent dispatch — static-grep returns exactly 3 matches.
8. Cross-phase regressions stay green: nudge::tests, invariants_33, memory_context::tests, streaming_scrubber::tests, test_snapshot_frozen_after_load.
</success_criteria>

<output>
Create `.planning/phases/34b-context-system-parity/34b-01-SUMMARY.md` when done, including:
- New file size + line count for `context_refs.rs`
- Final unit test count and pass result
- Resolution notes for any [ASSUMED] items from RESEARCH (Web UI on_session_reset trigger NOT addressed here — that lives in 34b-02; `dirs` crate or `std::env::var("HOME")` decision)
- Token-budget threshold observations from live testing (if any)
- Any deviations from D-01/D-02/D-04/D-05 with rationale
</output>
