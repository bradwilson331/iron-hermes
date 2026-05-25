# Phase 14: Context Files & SOUL.md - Context

**Gathered:** 2026-04-12
**Status:** Ready for planning

<domain>
## Phase Boundary

The agent loads its project context and identity from the filesystem using hermes-agent's priority chain (.hermes.md > AGENTS.md > CLAUDE.md > .cursorrules), with security scanning, truncation, YAML frontmatter stripping, git-root walking for .hermes.md, and progressive subdirectory discovery as the agent navigates via tool calls. SOUL.md loads from HERMES_HOME as the agent's durable identity with a hardcoded default fallback.

</domain>

<decisions>
## Implementation Decisions

### .hermes.md walk behavior
- **D-01:** .hermes.md walks upward from CWD — first match wins. Walk stops at git root if found, otherwise stops at $HOME. Only one .hermes.md is loaded (no merging).
- **D-02:** YAML frontmatter (between `---` markers) is stripped from .hermes.md before injection into the system prompt. Frontmatter is reserved for future config overrides per CTX-07. Content after stripping is scanned and truncated as normal.
- **D-03:** If no git root is found, walk stops at $HOME (not filesystem root). Prevents loading context files from system directories.

### Subdirectory discovery
- **D-04:** Context files discovered in subdirectories are injected into tool results. When a file-access tool (read_file, write_file, list_directory, etc.) touches a new directory, discovered context is appended to that tool's result output.
- **D-05:** Only file-access tools trigger subdirectory discovery. Other tools (web scraping, memory, etc.) do not trigger discovery even if they contain path-like arguments.
- **D-06:** Walk direction is upward from the accessed file's directory, checking up to 5 parent directories. Each directory is checked at most once per session (tracked via a visited-dirs set).
- **D-07:** Subdirectory discovery checks the full priority chain (.hermes.md > AGENTS.md > CLAUDE.md > .cursorrules), not just .hermes.md. First match in each new directory wins.

### Priority chain
- **D-08:** Context file name matching is case-sensitive. Only exact names match: `.hermes.md`, `AGENTS.md`, `CLAUDE.md`, `.cursorrules`. Drop the current lowercase variants (`agents.md`, `claude.md`) from the candidate list.
- **D-09:** AGENTS.md in HERMES_HOME and AGENTS.md in CWD serve separate roles and both load. HERMES_HOME/AGENTS.md is global agent configuration (always loaded as a separate prompt layer). CWD/AGENTS.md is project context (part of the priority chain, only loads if .hermes.md not found first). Two different purposes, both injected.

### SOUL.md identity system
- **D-10:** When `skip_context_files` is set (subagent delegation), SOUL.md is NOT loaded. The agent uses DEFAULT_AGENT_IDENTITY instead. Project context and AGENTS.md from HERMES_HOME are also skipped. Subagents get a clean, focused identity.
- **D-11:** SOUL.md content is injected raw (after scan + truncate) as the first layer of the system prompt. No header wrapping — it IS the identity.
- **D-12:** DEFAULT_AGENT_IDENTITY remains a hardcoded `const &str` in prompt_builder.rs. No file loading or `include_str!` needed.

### Claude's Discretion
- How to implement the visited-dirs set (HashSet<PathBuf> on the session/agent, or a shared Arc structure)
- How file-access tools detect "new directory" and trigger discovery (interceptor pattern vs. per-tool check)
- YAML frontmatter parsing approach (regex vs. dedicated parser like `gray_matter`)
- How subdirectory context injection is formatted within tool results
- Whether to add a `ContextLoader` struct/trait or extend the existing `PromptBuilder` for the walk logic
- Git root detection method (walk up looking for `.git` directory)

### Folded Todos
None.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Context file requirements
- `.planning/REQUIREMENTS.md` — CTX-01 (priority chain), CTX-02 (.hermes.md walks CWD to git root), CTX-03 (progressive subdirectory discovery), CTX-04 (once-per-dir, 5 parent cap), CTX-05 (security scanning), CTX-06 (truncation 70/20), CTX-07 (frontmatter stripping)

### Existing implementation (primary code references)
- `crates/ironhermes-agent/src/prompt_builder.rs` — Current PromptBuilder with `load_context()`, `load_soul_md()`, `load_project_context()`, `load_agents_md()`. Priority chain partially implemented (CWD only, no git root walk, no frontmatter stripping, no subdirectory discovery).
- `crates/ironhermes-core/src/context_scanner.rs` — `scan_context_content()` (threat pattern matching, invisible unicode detection) and `truncate_content()` (70/20 head/tail at 20K chars). Both fully functional, no changes needed.
- `crates/ironhermes-core/src/lib.rs` — Exports `scan_context_content`, `truncate_content`, `CONTEXT_FILE_MAX_CHARS`, `get_hermes_home()`

### Integration points
- `crates/ironhermes-tools/src/file_tools.rs` — File-access tools that need to trigger subdirectory discovery
- `crates/ironhermes-agent/src/agent_loop.rs` — Agent loop where tool results flow and context injection would happen
- `crates/ironhermes-gateway/src/handler.rs` — Gateway handler that constructs PromptBuilder
- `crates/ironhermes-cli/src/main.rs` — CLI entry point that constructs PromptBuilder

### Architecture
- `.planning/codebase/ARCH.md` — Crate dependency graph, key abstractions
- `.planning/ROADMAP.md` — Phase 14 success criteria, downstream dependencies

### Prior phase context
- `.planning/phases/11-memory-provider-trait/11-CONTEXT.md` — Established async_trait + Send + Sync pattern, `scan_context_content()` usage in memory scanning
- `.planning/phases/12-provider-resolution/12-CONTEXT.md` — Frozen-snapshot pattern (context loaded once at session start)
- `.planning/phases/13-session-storage/13-CONTEXT.md` — Write-through cache pattern, `spawn_blocking` bridge

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `PromptBuilder` (`ironhermes-agent/src/prompt_builder.rs`): Already loads SOUL.md, AGENTS.md (from HERMES_HOME), and project context (from CWD) with partial priority chain. Needs extension for git-root walk, frontmatter stripping, and subdirectory discovery.
- `scan_context_content()` (`ironhermes-core/src/context_scanner.rs`): Fully functional threat scanning — use as-is for all context files including discovered subdirectory files.
- `truncate_content()` (`ironhermes-core/src/context_scanner.rs`): 70/20 head/tail truncation at 20K chars — use as-is.
- `get_hermes_home()` (`ironhermes-core`): Returns HERMES_HOME path — used for SOUL.md and AGENTS.md loading.

### Established Patterns
- Frozen-snapshot: context loaded once at `load_context()` call, mid-session file edits don't change the prompt (Phase 12)
- Security scanning applied to all user-controlled content before prompt injection (Phase 11)
- `Arc<Mutex<>>` sharing for stateful session resources (visited-dirs tracking would follow this)

### Integration Points
- `PromptBuilder::load_context(cwd)` — entry point for initial context loading, needs git-root walk extension
- File tools in `ironhermes-tools/src/file_tools.rs` — need to trigger subdirectory discovery post-execution
- `AgentLoop` tool dispatch — where discovered context would be appended to tool results
- `PromptBuilder::new()` / construction sites in `main.rs` and `handler.rs` — may need `skip_context_files` parameter

</code_context>

<specifics>
## Specific Ideas

- The current code already has lowercase variants (`agents.md`, `claude.md`) in the priority chain candidates — these should be removed per D-08 (case-sensitive matching).
- hermes-agent's architecture is the reference implementation — port the context file discovery behavior faithfully.
- Subdirectory discovery is a session-scoped concern (visited-dirs tracking lives on the session), while initial context loading is a one-time setup concern. These may be separate code paths.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

### Reviewed Todos (not folded)
- "Add setup wizard and config scaffolding for gateway testing" — belongs in Phase 23 (Configuration & Setup Wizard). Already reviewed and deferred in Phases 12 and 13.

</deferred>

---

*Phase: 14-context-files-soul-md*
*Context gathered: 2026-04-12*
