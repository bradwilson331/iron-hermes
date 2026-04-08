# Phase 3: Self-Improvement + Security - Context

**Gathered:** 2026-04-07
**Status:** Ready for planning

<domain>
## Phase Boundary

Agent can safely read, edit, and extend its own context files (SOUL.md, AGENTS.md) and maintain a persistent memory of facts (MEMORY.md, USER.md), with security scanning that prevents prompt injection or self-destructive modifications. Also includes SSRF validation (prerequisite for Phase 4 web tools) and inbound Telegram rate limiting.

</domain>

<decisions>
## Implementation Decisions

### Self-edit guardrails
- **D-01:** Block writes entirely when threat patterns are detected — write_file/patch returns an error, file is not modified. Matches hermes-agent behavior.
- **D-02:** Security scanning applies only to context files (SOUL.md, AGENTS.md, MEMORY.md, USER.md) — files that get injected into the system prompt. Regular write_file/patch to other files remains unscanned.
- **D-03:** All context files are writable by the agent — no read-only restrictions. Self-modification is the core feature.
- **D-04:** Move `context_scanner.rs` from `ironhermes-agent` to `ironhermes-core` so both agent (prompt loading) and tools (write blocking) can use it. Core is the shared leaf crate.

### Memory subsystem
- **D-05:** Two memory stores matching hermes-agent: MEMORY.md (2,200 char limit, agent's personal notes) + USER.md (1,375 char limit, user profile). Stored in `~/.ironhermes/memories/`.
- **D-06:** Entry delimiter: `§` (section sign) with `\n§\n` as the full delimiter. Entries can be multiline.
- **D-07:** File locking via `fs2` crate (advisory flock) for read-modify-write safety across concurrent sessions. Separate `.lock` file per memory file.
- **D-08:** Atomic file I/O: tempfile + `fs::rename` pattern (matching `ironhermes-cron`). Writers always produce a complete file; readers never see partial state.
- **D-09:** No `read` action on the memory tool — memory is injected into the system prompt as a frozen snapshot at session start. Tool actions: add, replace, remove.
- **D-10:** Replace/remove use short unique substring matching via `old_text` parameter — no IDs, no full-text match required. Multiple-match returns an error asking for more specificity.
- **D-11:** MemoryStore lives in `ironhermes-core` so both agent (prompt injection) and tools (memory tool) can depend on it without circular deps.
- **D-12:** Frozen-snapshot pattern: system prompt injection captured once at `load_from_disk()`, never mutated mid-session. Tool responses reflect live state. Disk writes are immediate.
- **D-13:** Memory content scanned for injection/exfiltration before accepting (same threat patterns as context file scanning). Blocked entries return an error, not persisted.
- **D-14:** Duplicate prevention: exact duplicate entries are rejected with a "no duplicate added" message.
- **D-15:** Capacity overflow: adding an entry that would exceed the char limit returns an error with current usage and entries, prompting the agent to replace/remove first.

### SSRF validation
- **D-16:** Direct port of Python `url_safety.py`: resolve hostname via DNS, check against private IP ranges (is_private, is_loopback, is_link_local, CGNAT 100.64.0.0/10, metadata hostnames), fail closed on errors.
- **D-17:** DNS rebinding is a documented known limitation (TOCTOU between resolution and connection) — same as Python.
- **D-18:** Blocked hostnames: `metadata.google.internal`, `metadata.goog`.
- **D-19:** SSRF validator lives in `ironhermes-core` for shared access by web tools and any future HTTP-making code.

### Rate limiting
- **D-20:** Per-user (Telegram user_id) inbound rate limiting on message processing.
- **D-21:** Excess messages silently dropped — consistent with unauthorized-user pattern (D-11 from Phase 2) where the bot appears offline.
- **D-22:** Configurable in `config.yaml`: `rate_limit.messages_per_minute` (default 10), `rate_limit.burst_size` (default 3).

### Claude's Discretion
- Token bucket vs sliding window algorithm for rate limiting
- Exact threat pattern set for memory scanning (can extend beyond context_scanner's existing 10)
- Memory tool schema description wording (behavioral guidance for the LLM)
- Whether to add `fsync` before rename in atomic writes (cron doesn't, Python hermes-agent does)
- MemoryStore deduplication strategy details (preserve order, keep first occurrence)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements and architecture
- `.planning/REQUIREMENTS.md` §Self-Improvement — SELF-01 through SELF-06
- `.planning/REQUIREMENTS.md` §Security — SEC-01 through SEC-03
- `.planning/ROADMAP.md` §Phase 3 — Key technical decisions, success criteria
- `.planning/codebase/ARCH.md` — Crate dependency graph, module structure, concurrency model

### Python reference implementation
- `/Users/twilson/code/hermes-agent/tools/memory_tool.py` — MemoryStore class, memory tool schema, entry format, scanning, atomic writes, file locking. **Primary reference for memory subsystem port.**
- `/Users/twilson/code/hermes-agent/tools/url_safety.py` — SSRF validation: is_safe_url(), _is_blocked_ip(), blocked hostnames, CGNAT handling. **Primary reference for SEC-01 port.**
- `/Users/twilson/code/hermes-agent/website/docs/user-guide/features/memory.md` — User-facing memory documentation (format, capacity, frozen snapshot pattern)

### Existing IronHermes code
- `crates/ironhermes-agent/src/context_scanner.rs` — THREAT_PATTERNS RegexSet, scan_context_content(), truncate_content(). **Must be moved to ironhermes-core per D-04.**
- `crates/ironhermes-agent/src/prompt_builder.rs` — PromptBuilder that loads context files, will need memory injection
- `crates/ironhermes-tools/src/file_tools.rs` — WriteFileTool, PatchFileTool that need scanning integration for context files
- `crates/ironhermes-cron/src/lib.rs` — Atomic write pattern (temp file + rename) to replicate
- `crates/ironhermes-gateway/src/handler.rs` — with_rate_limit_retry (outbound 429 handling), rate limiting integration point
- `crates/ironhermes-core/src/config.rs` — Config struct to extend with rate_limit fields

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `context_scanner.rs`: THREAT_PATTERNS RegexSet with 10 patterns + invisible Unicode detection — extend for memory scanning, move to core
- `ironhermes-cron` atomic write: temp file + `fs::rename` pattern — replicate for MemoryStore
- `with_rate_limit_retry`: outbound rate limit handling — rate limiting module can sit alongside in gateway
- `ToolRegistry` + `Tool` trait: register new `MemoryTool` via `register_defaults()`
- `ToolSchema::new()`: schema definition pattern for the memory tool

### Established Patterns
- `LazyLock<RegexSet>` for compiled regex patterns (context_scanner)
- `Arc<ToolRegistry>` shared across agent runs — MemoryStore needs similar Arc wrapping
- Tool results as `String` (JSON) — memory tool returns JSON matching Python's dict responses
- Config loaded from YAML at `~/.ironhermes/` — rate limit config fits here

### Integration Points
- `PromptBuilder::build_system_message()` — needs to inject memory snapshot after context files
- `WriteFileTool::execute()` / `PatchFileTool::execute()` — need to check if target is a context file and run scan before writing
- `register_defaults()` in `registry.rs` — register MemoryTool
- `GatewayRunner` or handler dispatch — rate limiter check before queuing messages
- `Config` struct — add `rate_limit` and `memory` sections

</code_context>

<specifics>
## Specific Ideas

- Memory format matches hermes-agent exactly: section-sign delimited, char-bounded, two-store split
- The Python MemoryStore is the authoritative reference for behavior — port the logic, adapt for Rust idioms
- SSRF validator is a near-direct port — Python's `ipaddress` maps cleanly to Rust's `std::net::IpAddr`
- Silent drop for rate-limited messages matches the "bot appears offline" pattern from Phase 2's unauthorized user handling

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 03-self-improvement-security*
*Context gathered: 2026-04-07*
