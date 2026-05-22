---
phase: 34b-context-system-parity
plan: 01
type: execute
wave: 1
depends_on: [34b-00]
files_modified:
  - crates/ironhermes-agent/src/context_refs.rs
  - crates/ironhermes-agent/src/agent_runtime.rs
  - crates/ironhermes-agent/src/agent_loop.rs
  - crates/ironhermes-agent/tests/invariants_34b.rs
autonomous: true
requirements: [CTX-REF-01, CTX-REF-02]
must_haves:
  truths:
    - "A user message containing @file:/@folder:/@diff/@staged/@git:N/@url: is expanded into a `--- Attached Context ---` footer and the refs are stripped from the inline text — once, centrally, inside AgentRuntime::run_turn"
    - "Expansion happens BEFORE attach_context_engine/agent.run inside run_turn, over TurnRequest.messages' latest user message"
    - "Sensitive credential paths (.ssh/, .aws/, .env, etc.) are rejected with a structured warning; original message preserved when ALL refs blocked"
    - "Injected tokens > 50% context_length blocks all expansion; > 25% warns but expands"
    - "Expansion warnings reach all 3 surfaces via AgentResult.context_warnings so the `--- Context Warnings ---` block can render"
    - "git/rg subprocesses for @diff/@staged/@git:N/@folder: are invoked argv-style (Command::arg) with no shell and no string interpolation; @git:N validated as u32 in [1,10]"
  artifacts:
    - path: crates/ironhermes-agent/src/context_refs.rs
      provides: "Parser + expander + sensitive-path blocklist + budget enforcement + argv-only subprocess expansion + 14 unit tests"
      exports: ["parse_context_references", "preprocess_context_references_async", "ContextReference", "ContextReferenceResult"]
      min_lines: 300
    - path: crates/ironhermes-agent/src/agent_runtime.rs
      provides: "Central @-ref preprocessing inside run_turn before attach_context_engine; warnings threaded onto AgentResult"
      contains: "preprocess_context_references_async"
    - path: crates/ironhermes-agent/src/agent_loop.rs
      provides: "AgentResult.context_warnings carrier field for expansion warnings"
      contains: "context_warnings"
  key_links:
    - from: crates/ironhermes-agent/src/agent_runtime.rs
      to: crates/ironhermes-agent/src/context_refs.rs
      via: "preprocess_context_references_async call in run_turn before attach_context_engine"
      pattern: "preprocess_context_references_async"
    - from: crates/ironhermes-agent/src/agent_runtime.rs
      to: crates/ironhermes-agent/src/agent_loop.rs
      via: "AgentResult.context_warnings populated from ContextReferenceResult.warnings"
      pattern: "context_warnings"
---

<objective>
Port `../hermes-agent/agent/context_references.py` to a new Rust module
`context_refs.rs`, and wire it CENTRALLY into `AgentRuntime::run_turn` (D-09 /
D-11) — NOT per-surface. The OLD draft (call preprocess 3× per surface before
`AgentLoop::run`) is superseded: all three surfaces (CLI `run_chat`, gateway
`run_agent`, web `run_web_turn`) already delegate to `run_turn`, which owns the
resolver/`context_length`, `attach_context_engine`, and the `AgentResult`.

Preprocessing runs once in `run_turn` over the latest user message in
`TurnRequest.messages`, before `attach_context_engine`/`agent.run`. Because the
surfaces no longer see the message between raw input and dispatch, expansion
WARNINGS need a return path: this plan adds a `context_warnings: Vec<String>`
field to `AgentResult` (the carrier, per D-11) AND logs them centrally. Surfaces
read `result.context_warnings` to render the `--- Context Warnings ---` block.

Purpose: users get `@file:/@folder:/@diff/@staged/@git:N/@url:` expansion with a
sensitive-path blocklist and 50%/25% token budget — a security-relevant module
that runs git/rg subprocesses on user-supplied values (argv-only, no shell).
Output: new `context_refs.rs` (~14 unit tests), run_turn wiring, AgentResult carrier.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/STATE.md
@.planning/phases/34b-context-system-parity/34B-CONTEXT.md
@.planning/phases/34b-context-system-parity/34B-PATTERNS.md
@.planning/phases/34b-context-system-parity/34b-PLAN-DRAFT.md

<interfaces>
From crates/ironhermes-agent/src/agent_runtime.rs — the single per-turn chokepoint:
```rust
pub struct TurnRequest {
    pub messages: Vec<ChatMessage>,
    pub session_id: String,
    // ... compression_count: usize  (state-threading precedent)
}
pub async fn run_turn(&self, req: TurnRequest) -> Result<AgentResult> {
    self.budget.reset();
    let context_length = self.resolver.resolve_for_main().context_length();
    let mut agent = AgentLoop::new(...)...;
    // ... per-turn wiring ...
    agent = attach_context_engine(agent, &self.config, &self.resolver, req.session_id, ...);
    agent.run(req.messages).await
}
```
Note: `self.config` is `Arc<Config>`; `config.agent` carries `TerminalConfig`-style
cwd config — confirm the exact path to the agent cwd in config.rs at implementation time (D-05).

HERMES_HOME resolution (resolves the RESEARCH Open Question): use
`ironhermes_core::constants::get_hermes_home()` — reads `IRONHERMES_HOME` env if
set and non-empty, else `dirs::home_dir().join(".ironhermes")`. This is the
`$HERMES_HOME` base for the `$HERMES_HOME/.env` and `$HERMES_HOME/skills/.hub/`
blocklist entries; the user's `$HOME` (dirs::home_dir()) is the base for the
`.ssh`/`.aws`/dotfile entries.

From crates/ironhermes-agent/src/agent_loop.rs — the result type to extend:
```rust
pub struct AgentResult {
    pub messages: Vec<ChatMessage>,
    pub appended: Vec<ChatMessage>,
    pub turns_used: usize,
    pub finished_naturally: bool,
    pub final_response: Option<String>,
    pub total_usage: AggregatedUsage,
    pub compression_count_after: usize,
    pub stop_reason: StopReason,
}
impl AgentResult { pub fn budget_exhausted(messages, turns_used) -> Self { ... } }
```
A new `pub context_warnings: Vec<String>` field must be added here AND defaulted
in `budget_exhausted` and every `Ok(AgentResult { .. })` construction site in
agent_loop.rs (run() returns AgentResult at ~:885, ~:914, ~:1032, ~:1165).

From crates/ironhermes-tools/src/web_extract.rs — `WebExtractTool` with
`use_llm_processing: bool` (call `true` for @url:, retry `false` on LLM failure, D-01/D-02).

Subprocess discipline (BLOCKER-3 / CWE-78): @diff/@staged/@git:N use `git`, and
@folder: listing uses `rg`/directory walk. ALL such calls MUST use
`tokio::process::Command::new("git")` (or `rg`) with `.arg(...)` per argument —
never `sh -c`, never a shell, never an interpolated command string. @git:N is
parsed and validated as a `u32` in `[1,10]` BEFORE being formatted into a `-N`
arg. @folder:/@file: targets are passed as their own `.arg(path)` (the path is a
separate argv element, not interpolated into a flag string).

From ../hermes-agent/agent/context_references.py — the canonical port target:
- `REFERENCE_PATTERN` regex (line ~16), `TRAILING_PUNCTUATION` (~19)
- `_SENSITIVE_HOME_DIRS`/`_SENSITIVE_HOME_FILES`/`_SENSITIVE_HERMES_DIRS` (~21-37)
- `parse_context_references` (~62), `preprocess_context_references_async` (~132)
- budget: `hard_limit = context_length * 0.50`, `soft_limit = context_length * 0.25` (~167-168)
- output assembly: `--- Context Warnings ---` then `--- Attached Context ---` (~191-193)
- `_resolve_path` allowed_root scoping (~329), blocklist sets (~347-350)
</interfaces>
</context>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Parser + types + sensitive-path blocklist in context_refs.rs</name>
  <files>crates/ironhermes-agent/src/context_refs.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/context_refs.rs (the Wave-0 stub being grown in place)
    - ../hermes-agent/agent/context_references.py (REFERENCE_PATTERN, TRAILING_PUNCTUATION, the three _SENSITIVE_* tuples, parse_context_references, _strip_trailing_punctuation, _strip_reference_wrappers, _parse_file_reference_value, _resolve_path)
    - crates/ironhermes-core/src/constants.rs (get_hermes_home — the $HERMES_HOME base for the two HERMES blocklist entries)
    - crates/ironhermes-agent/src/context_loader.rs (existing module idioms in this crate)
  </read_first>
  <behavior>
    - Test simple: parse "see @diff and @staged" → two refs, kinds "diff"/"staged", empty targets.
    - Test kind:value: parse "@file:src/foo.rs" → ContextReference{ kind:"file", target:"src/foo.rs" }.
    - Test quoted: parse `@file:"path with spaces.rs":12-20` → target "path with spaces.rs", line_start 12, line_end 20.
    - Test line range: parse "@file:foo.rs:10-25" → line_start 10, line_end 25; "@file:foo.rs:10" → line_start 10, line_end None.
    - Test trailing punctuation: parse "look at @file:foo.rs." → target "foo.rs" (trailing '.' stripped, balanced-paren-aware).
    - Test multiple refs: parse a message with @file:, @folder:, @url: → three refs in source order with correct start/end byte offsets.
    - Test sensitive blocklist (parameterised over EVERY entry: .ssh/, .aws/, .gnupg/, .kube/, .docker/, .azure/, .config/gh/, .ssh/authorized_keys, .ssh/id_rsa, .ssh/id_ed25519, .ssh/config, .bashrc, .zshrc, .profile, .bash_profile, .zprofile, .netrc, .pgpass, .npmrc, .pypirc, $HERMES_HOME/.env, $HERMES_HOME/skills/.hub/): the blocklist predicate returns true → expansion rejected with warning "path is a sensitive credential file and cannot be attached".
  </behavior>
  <action>
    Grow the Wave-0 `context_refs.rs` stub into the parser + types layer.
    Define `pub struct ContextReference { raw, kind, target, start, end,
    line_start: Option<usize>, line_end: Option<usize> }` and `pub struct
    ContextReferenceResult { message, original_message, references, warnings:
    Vec<String>, injected_tokens: usize, expanded: bool, blocked: bool }`,
    matching Python's dataclasses field-for-field. Implement `pub fn
    parse_context_references(message: &str) -> Vec<ContextReference>` porting
    `REFERENCE_PATTERN` (use the `regex` crate; confirm it is already a
    dependency of ironhermes-agent before adding), trailing-punctuation
    stripping, quoted-value unwrapping (backtick/double/single), and
    file-reference `:start[-end]` parsing — byte-for-byte with Python. Add the
    three sensitive-path constant lists and a `fn is_sensitive_path(resolved:
    &Path, home: &Path, hermes_home: &Path) -> bool` predicate (use
    get_hermes_home() for the HERMES base) plus a
    `fn resolve_within_root(cwd, target, allowed_root) -> Option<PathBuf>`
    helper enforcing `allowed_root` containment (D-03/D-04 — fixed to cwd, no
    escape hatch). Implement the 6 parser unit tests + the 1 parameterised
    blocklist test from <behavior> in `mod tests`. Do NOT implement expansion
    or budget yet (Task 2).
  </action>
  <verify>
    <automated>cargo test -p ironhermes-agent --lib context_refs::tests 2>&1 | tail -15</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build -p ironhermes-agent` succeeds.
    - `parse_context_references` and the two structs are `pub` and exported.
    - At least 7 tests pass in `context_refs::tests` (6 parser + 1 blocklist).
    - Blocklist test covers every entry listed in <behavior> (parameterised).
    - `grep -c 'pub fn parse_context_references' crates/ironhermes-agent/src/context_refs.rs` returns 1.
  </acceptance_criteria>
  <done>Parser, types, and sensitive-path blocklist match Python; 7+ tests green.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: Expander + budget enforcement + preprocess_context_references_async (argv-only subprocesses)</name>
  <files>crates/ironhermes-agent/src/context_refs.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/context_refs.rs (Task-1 parser layer to build on)
    - ../hermes-agent/agent/context_references.py (preprocess_context_references_async, _expand_file_reference, _expand_folder_reference, diff/staged/git expansion, url expansion, hard/soft budget logic, output assembly)
    - crates/ironhermes-tools/src/web_extract.rs (WebExtractTool, use_llm_processing flag — for @url: per D-01/D-02)
    - crates/ironhermes-agent/src/context_compressor.rs (estimate_messages_tokens / token estimation helper to size injected_tokens consistently with the rest of the agent)
  </read_first>
  <behavior>
    - Test expand file (full): @file: a temp file with known content → block "📄 @file:<name> (N tokens)" + fenced slice; refs stripped from inline message.
    - Test expand file (range): @file:foo:2-3 → only lines 2-3 in the slice.
    - Test expand folder: @folder: a temp dir → listing block; no file contents.
    - Test expand diff: @diff with a stubbed/temp git repo (or inject a command runner) → "🧾 git diff (N tokens)" fenced block.
    - Test url stub: @url: with an injected `url_fetcher` returning fixed markdown → "🌐 @url:<url> (N tokens)" block; on fetcher error, a warning is added and content still attached if a raw fallback is provided (D-02), else ref dropped with warning (no silent drop).
    - Test hard limit: injected_tokens > context_length*0.50 → result.blocked == true, result.message == result.original_message, single warning "@ context injection refused: N tokens exceeds the 50% hard limit (M).".
    - Test soft limit: injected_tokens in (25%, 50%] → warning "@ context injection warning: N tokens exceeds the 25% soft limit (M)." AND expansions still applied (blocked == false).
    - Test git:N validation: @git:0 and @git:11 (out of [1,10]) are rejected/clamped with a warning; @git:3 maps to a `git log -3`-style argv with "3" as a separate validated arg (assert via the injected command runner that args are passed argv-style, no shell string).
  </behavior>
  <action>
    Implement `pub async fn preprocess_context_references_async(message: &str,
    cwd: &Path, context_length: usize, url_fetcher: Option<UrlFetcher>,
    allowed_root: Option<&Path>) -> ContextReferenceResult` (define `UrlFetcher`
    as a boxed async fn type or a small trait). Expand each kind: file (with
    optional line range + sensitive-path rejection), folder (listing), diff,
    staged, git:N, url (via injected url_fetcher; production wires
    `WebExtractTool` with `use_llm_processing: true`, falling back to raw on
    failure per D-02 with a surfaced warning). SUBPROCESS DISCIPLINE (CWE-78,
    BLOCKER-3): all git/rg invocations use `tokio::process::Command::new("git"|"rg")`
    with one `.arg(...)` per argument — NEVER `sh -c`, NEVER a shell, NEVER an
    interpolated command string. Parse @git:N into a `u32` and validate it is in
    `[1,10]` BEFORE constructing the command (out-of-range → warning, no
    command run); pass the validated count and any @folder:/@file: path as their
    own separate `.arg()` elements. Compute `injected_tokens` using the crate's
    existing token estimator. Enforce `hard_limit = context_length * 0.50`
    (exceeded → blocked, all expansions stripped, message reverts to original,
    single warning) and `soft_limit = context_length * 0.25` (exceeded →
    warning, expansions applied). Assemble output exactly as Python: strip refs
    from inline text, prepend `--- Context Warnings ---\n- {w}` lines, append
    `--- Attached Context ---\n\n{blocks}`. Implement the 5 expander tests +
    hard-limit + soft-limit + git:N-validation tests from <behavior> (use
    injected fakes for git/url so tests are hermetic). Total
    `context_refs::tests` must reach >= 14.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-agent --lib context_refs::tests 2>&1 | tail -20</automated>
  </verify>
  <acceptance_criteria>
    - `preprocess_context_references_async` is `pub` and exported.
    - `context_refs::tests` has >= 14 passing tests (6 parser + 5 expander + hard + soft + blocklist + git:N validation).
    - Hard-limit test asserts `blocked == true` AND `message == original_message`.
    - Soft-limit test asserts a warning is present AND `blocked == false`.
    - @url: fetcher-error path adds a warning and never silently drops the ref.
    - NO-SHELL gate (CWE-78): the expansion module contains no `sh -c`, no `.sh(`, and no shell invocation — `grep -nE 'sh -c|/bin/sh|Command::new\("sh"\)|Command::new\("bash"\)' crates/ironhermes-agent/src/context_refs.rs` returns 0 matches; AND `grep -c '\.arg(' crates/ironhermes-agent/src/context_refs.rs` returns >= 1 (argv-style invocation present).
    - @git:N is validated as u32 in [1,10] before command construction (git:N-validation test green).
    - `grep -c 'pub async fn preprocess_context_references_async' crates/ironhermes-agent/src/context_refs.rs` returns 1.
  </acceptance_criteria>
  <done>Expander + budget + argv-only subprocesses complete; 14+ tests green; no shell invocation in the expansion path; output format mirrors Python.</done>
</task>

<task type="auto">
  <name>Task 3: Centralize @-ref preprocessing in run_turn + AgentResult.context_warnings carrier (D-09/D-11)</name>
  <files>crates/ironhermes-agent/src/agent_runtime.rs, crates/ironhermes-agent/src/agent_loop.rs, crates/ironhermes-agent/tests/invariants_34b.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/agent_runtime.rs (run_turn ~:205; context_length resolved at ~:209; attach_context_engine call ~:249; TurnRequest at ~:92 — compression_count at ~:108 is the threading precedent)
    - crates/ironhermes-agent/src/agent_loop.rs (AgentResult struct ~:41; budget_exhausted ~:80; the four `Ok(AgentResult { .. })` sites in run() at ~:885/~:914/~:1032/~:1165)
    - crates/ironhermes-agent/src/context_refs.rs (the preprocess fn from Task 2)
    - crates/ironhermes-tools/src/web_extract.rs (to construct the production UrlFetcher closure around WebExtractTool, D-01/D-02)
    - crates/ironhermes-core/src/config.rs (resolve the agent cwd for allowed_root per D-05 — TerminalConfig.cwd if set, else current_dir at startup)
    - crates/ironhermes-agent/tests/invariants_34b.rs (the Wave-0 placeholder to replace with the centralization source guard)
  </read_first>
  <action>
    Add `pub context_warnings: Vec<String>` to `AgentResult` in agent_loop.rs;
    default it to `Vec::new()` in `budget_exhausted` and in every
    `Ok(AgentResult { .. })` construction inside `run()`. In
    `agent_runtime.rs::run_turn`, AFTER resolving `context_length` and BEFORE
    `attach_context_engine`/`agent.run`, mutate `req.messages`: find the LATEST
    user message, run `preprocess_context_references_async` over its text with
    `cwd`/`allowed_root` resolved per D-05, the resolved `context_length`, and a
    production `UrlFetcher` built from `WebExtractTool` (use_llm_processing true,
    raw fallback per D-02). Replace that message's text with `result.message`.
    Collect `result.warnings` to (1) log centrally via `tracing::warn!` and (2)
    return on the final `AgentResult.context_warnings` (the D-11 carrier) — the
    run() result is the value returned by run_turn, so assign warnings onto it
    before returning (e.g. `let mut out = agent.run(req.messages).await?;
    out.context_warnings = collected; Ok(out)`). Do NOT call preprocess in any
    surface. Replace the `invariants_34b.rs` placeholder with a source-guard
    test asserting via `include_str!` that (a) `agent_runtime.rs` contains
    `preprocess_context_references_async` and the call appears BEFORE
    `attach_context_engine(`, and (b) handler.rs / state.rs / main.rs do NOT
    each call `preprocess_context_references_async` (centralization invariant —
    grep on those three sources returns 0).
  </action>
  <verify>
    <automated>cargo build --workspace && cargo test -p ironhermes-agent --test invariants_34b 2>&1 | tail -10</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build --workspace` succeeds.
    - `AgentResult` has a `context_warnings: Vec<String>` field; all construction sites compile.
    - In agent_runtime.rs source, the byte offset of `preprocess_context_references_async` is LESS than the byte offset of `attach_context_engine(` (centralization-before-engine invariant, asserted in invariants_34b).
    - `grep -c preprocess_context_references_async crates/ironhermes-cli/src/main.rs crates/ironhermes-gateway/src/handler.rs crates/iron_hermes_ui/src/server/state.rs` sums to 0 (no per-surface calls).
    - `grep -c preprocess_context_references_async crates/ironhermes-agent/src/agent_runtime.rs` returns >= 1.
    - invariants_34b test is no longer `#[ignore]` and passes.
  </acceptance_criteria>
  <done>@-ref preprocessing runs once in run_turn before attach_context_engine; warnings carried on AgentResult; centralization proven by invariants_34b.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| user message text → @-ref expander | Untrusted chat input selects filesystem paths, git ranges, and URLs to read and inject into the model context. |
| @file:/@folder: target → local filesystem | Path traversal could exfiltrate credentials outside the workspace. |
| @diff/@staged/@git:N/@folder: target → git/rg subprocess | User-supplied values become subprocess arguments; shell interpolation would allow command injection (CWE-78). |
| @url: target → remote host | SSRF + remote content injection into the prompt. |
| expanded blocks → context window | Token-budget DoS (oversized injection crowding out real context). |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-34b-01-PATH | Information Disclosure | `@file:`/`@folder:` path resolution in context_refs.rs | mitigate | `resolve_within_root` enforces `allowed_root` (fixed to cwd, no escape hatch, D-03/D-04); references resolving outside cwd are rejected with a warning. |
| T-34b-01-SC | Information Disclosure | sensitive-path blocklist | mitigate | `is_sensitive_path` rejects every entry in the three _SENSITIVE_* lists (.ssh/.aws/.env/etc.); parameterised blocklist test covers all entries; original message preserved when ALL refs blocked. |
| T-34b-01-SHELL | Elevation of Privilege / Tampering (CWE-78 command injection) | git/rg subprocess invocation for @diff/@staged/@git:N/@folder: | mitigate | ALL subprocess calls use `tokio::process::Command::new("git"\|"rg")` with `.arg()` per argument — no `sh -c`, no shell, no interpolated command string. @git:N validated as a u32 in [1,10] before command construction; @folder:/@file: paths passed as separate argv elements. NO-SHELL grep gate in Task 2 acceptance asserts the module contains no shell invocation and uses `.arg(`. This is a HIGH-severity ASVS-L1 gate. |
| T-34b-01-SSRF | Spoofing/Tampering | `@url:` fetch via WebExtractTool | mitigate | URL fetch goes through the existing WebExtractTool (which carries the project's URL-fetch policy); on LLM-processing failure fall back to raw with a surfaced warning (D-02) — never silently drop, never bypass the tool. |
| T-34b-01-DOS | Denial of Service | injected-token budget | mitigate | hard_limit = 50% context_length → blocked, all expansion stripped, message reverts to original; soft_limit = 25% → warning. Budget computed with the crate's token estimator. |
| T-34b-01-INJECT | Tampering | expanded blocks become model context | accept | Attached content is fenced and labeled `--- Attached Context ---`; the model treats it as reference. Per-content sanitization beyond fencing is out of scope for L1 and matches Python parity. |
| T-34b-01-SC-PKG | Tampering | `regex` crate dependency (if newly added) | mitigate | `regex` is a first-party rust-lang maintained crate; confirm it is already a transitive/direct dep of ironhermes-agent before adding. If a new install is required, verify on crates.io/crates/regex before adding to Cargo.toml. |
</threat_model>

<verification>
```bash
cargo build --workspace
cargo test -p ironhermes-agent --lib context_refs::tests 2>&1 | tail -20   # 14+/14+
cargo test -p ironhermes-agent --test invariants_34b 2>&1 | tail -10
# Centralization invariant — must sum to 0:
grep -c preprocess_context_references_async crates/ironhermes-cli/src/main.rs crates/ironhermes-gateway/src/handler.rs crates/iron_hermes_ui/src/server/state.rs
# CWE-78 no-shell gate — must be 0 matches:
grep -nE 'sh -c|/bin/sh|Command::new\("sh"\)|Command::new\("bash"\)' crates/ironhermes-agent/src/context_refs.rs
# Regression gates:
cargo test -p ironhermes-agent --lib memory_context::tests streaming_scrubber::tests
cargo test -p ironhermes-agent --test invariants_33
```
</verification>

<success_criteria>
- context_refs.rs ports the Python parser/expander/blocklist/budget with 14+ tests green.
- @-ref preprocessing runs ONCE inside run_turn before attach_context_engine (not per-surface).
- AgentResult.context_warnings carries expansion warnings to all 3 surfaces.
- Sensitive paths rejected; 50% hard / 25% soft budget enforced; @url via WebExtractTool with raw fallback.
- git/rg subprocesses are argv-only (no shell, CWE-78 mitigated); @git:N validated u32 in [1,10].
- invariants_34b proves centralization and run_turn ordering.
</success_criteria>

<output>
Create `.planning/phases/34b-context-system-parity/34B-01-SUMMARY.md` when done.
</output>
