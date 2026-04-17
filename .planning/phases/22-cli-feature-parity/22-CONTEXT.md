# Phase 22: CLI Feature Parity - Context

**Gathered:** 2026-04-16
**Status:** Ready for planning

<domain>
## Phase Boundary

Bring the CLI interactive mode (`run_chat`) and one-shot mode (`run_single`) to **full tool-level parity** with the gateway: register execute_code, guardrails, HookRegistry (with JSONL event logging), skills_tool, and cron_tool — the same tool surface the gateway has.

**In scope (Phase 22):**
- CLI-01: Wire execute_code tool into `run_chat` and `run_single` (with active_skills for sandbox env pass-through)
- Wire `BlocklistGuardrail` + `error_detail` from `HooksConfig` into both CLI paths
- Wire `HookRegistry` with JSONL event log listener into both CLI paths
- Register `skills_tool` (the interactive tool, not just PromptBuilder skill loading) in both paths
- Register `cron_tool` (JobStore) in both paths
- Fire same lifecycle events as gateway: session:start/end, agent:start/step/end, tool:called/completed, command:*. Only `gateway:startup` is skipped.
- JSONL event logging enabled by default when configured; webhook forwarding remains opt-in

**Out of scope (split to separate phases):**
- CLI-02 (TUI extension hooks) → Phase 22.1
- CLI-03..08 (ACP adapter) → Phase 22.2
- New CLI subcommands beyond what's already implemented (hermes sessions, hermes config, hermes tools, etc.)
- Slash command integration (SKILL-12/13/14 → separate phase)

</domain>

<decisions>
## Implementation Decisions

### Phase splitting
- **D-01:** Three-way split. Phase 22 = CLI-01 (tool parity only). Phase 22.1 = CLI-02 (TUI extension hooks: `_get_extra_tui_widgets()`, `_register_extra_tui_keybindings()`, `_build_tui_layout_children()`, `process_command()`, `_build_tui_style_dict()`). Phase 22.2 = CLI-03..08 (ACP adapter with `ironhermes-acp` crate, Agent Protocol, VS Code first). ROADMAP.md must be updated to reflect this split.

### Tool parity scope
- **D-02:** Full parity — wire ALL tools the gateway has into CLI: execute_code, guardrails, hooks, skills_tool, cron_tool. Not just the CLI-01 trio.
- **D-03:** Both `run_chat` AND `run_single` get the full tool surface. A user running `ironhermes -e 'run my script'` expects execute_code to work.
- **D-04:** The RPC dispatch registry (sandbox-safe tools for execute_code) is constructed the same way as in `run_gateway`: file tools + web tools + memory tool — no terminal, no execute_code in the RPC registry itself.

### Hook lifecycle in CLI
- **D-05:** CLI fires the same lifecycle events as gateway: `session:start`, `session:end`, `agent:start`, `agent:step`, `agent:end`, `tool:called`, `tool:completed`, `command:*`. Only `gateway:startup` is skipped (CLI is not a long-running service).
- **D-06:** JSONL event logging is the default when `hooks_config.event_log.enabled` is true. This provides a persistent, searchable audit trail of every tool call and agent step.
- **D-07:** Webhook forwarding is **opt-in**, not default for CLI. Webhooks require template mapping (`{dot.notation}`), HMAC validation, and external platform credentials — they're event-driven triggers, not appropriate as CLI defaults. If `hooks_config.webhooks` has entries, they're registered — same as gateway — but the config drives whether they're active.

### Wiring pattern
- **D-08:** Follow the gateway's wiring pattern in `run_gateway` (lines 800-900 of main.rs) as the reference implementation. The CLI paths replicate the same sequence: build_registry → register tools → load hooks config → add guardrails → build HookRegistry → register listeners.
- **D-09:** The `attach_context_engine` call in `run_single` currently passes `None` for the hook registry parameter. Phase 22 changes this to pass the actual `HookRegistry` so context compression events fire hooks.

### ACP decisions (for Phase 22.2 context)
- **D-10:** New crate `ironhermes-acp` — separate from CLI, clean dependency boundary. Mirrors `ironhermes-gateway` as a top-level consumer crate.
- **D-11:** Target Agent Protocol (agentprotocol.ai) — the open standard VS Code Copilot, Zed, and JetBrains are converging on. JSON-RPC over stdio.
- **D-12:** VS Code first. Ship one editor integration well before expanding to Zed and JetBrains.

### Claude's Discretion
- Exact placement of hook `emit()` calls within run_chat/run_single (before or after state_store writes)
- Whether to extract a shared `wire_tools()` helper that both `run_chat`, `run_single`, and `run_gateway` call, or keep the wiring inline
- Whether the cron_tool in CLI should be limited (e.g., no `tick` in non-gateway mode) or full-featured

### Folded Todos
- "CLI feature parity" (2026-04-17) — execute_code, hooks, guardrails in CLI mode. Requirements CLI-01..08. Folded as the primary scope of this phase.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase 22 primary targets
- `crates/ironhermes-cli/src/main.rs` — `run_chat` (line ~377), `run_single` (line ~261), `run_gateway` (line ~786, reference implementation for wiring pattern), `build_registry` (line ~942)
- `crates/ironhermes-cli/src/main.rs` lines 800-900 — Gateway tool wiring: execute_code, skills_tool, cron_tool, guardrails, HookRegistry, webhook listeners. This is the pattern to replicate in CLI paths.

### Hook system
- `crates/ironhermes-hooks/` — `HooksConfig`, `HookRegistry`, `BlocklistGuardrail`, `create_jsonl_listener`, `create_webhook_listener`, `RetryQueue`, `drain_retry_queue`, `format_guardrail_error`
- `crates/ironhermes-agent/src/agent_loop.rs` lines 772-787 — Guardrail check in agent loop (already fires for any registry with guardrails)

### Execute code
- `crates/ironhermes-exec/` — Execute code sandbox, RPC server
- `crates/ironhermes-tools/src/execute_code.rs` — ExecuteCodeTool registration via `register_execute_code_tool_with_active_skills`

### Skills tool
- `crates/ironhermes-tools/src/skills_tool.rs` — SkillsTool registration via `register_skills_tool`
- `crates/ironhermes-core/src/skills.rs` — SkillRegistry

### Cron tool
- `crates/ironhermes-cron/` — JobStore
- Registration via `register_cronjob_tool` in main.rs

### External reference (user-provided)
- hermes-agent CLI Commands Reference — comprehensive list of all hermes-agent CLI commands, flags, and subcommands. Establishes the target CLI surface for feature parity assessment.
- hermes-agent webhook documentation — webhook subscribe structure, HMAC security, template mapping, state persistence patterns.

### Prior phase context
- `.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md` — TUI module decisions (D-15..D-18), no new deps constraint
- `.planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md` — MemoryManager wiring pattern, Fix 2 (chat-mode memory wiring already done)
- `.planning/phases/19-skills-framework/19-CONTEXT.md` — Skills tool activation, env var pass-through, security scanning

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `run_gateway` (main.rs:786-920) — Complete reference implementation for wiring all tools, guardrails, hooks, webhooks. Phase 22 replicates this pattern in `run_chat` and `run_single`.
- `HookRegistry` + `HooksConfig` (ironhermes-hooks) — Already built and working in gateway. Just needs to be constructed and passed in CLI paths.
- `BlocklistGuardrail` (ironhermes-hooks) — Already integrated in agent_loop.rs guardrail check. Just needs to be added to CLI's ToolRegistry.
- `register_execute_code_tool_with_active_skills` — Existing method on ToolRegistry. Needs active_skills Arc + RPC registry + ExecConfig.
- `register_skills_tool` — Existing method on ToolRegistry. Needs SkillRegistry + active_skills + credential_dir + config map.
- `register_cronjob_tool` — Existing method on ToolRegistry. Needs JobStore.

### Established Patterns
- Gateway wiring sequence: defaults → memory → delegate_task → cron → skills → execute_code → guardrails → Arc::new(registry) → HookRegistry → listeners
- `attach_context_engine` accepts optional `hook_registry` parameter — currently `None` in CLI, needs to be wired
- `MemoryManager` wiring in CLI already done (Phase 20 Fix 2) — same pattern for other tools

### Integration Points
- `run_chat` line 395: `build_registry()` — currently just `register_defaults()`. Add all missing tool registrations.
- `run_single` line 271: Same — add all missing tool registrations.
- `run_agent_turn` line 750: `attach_context_engine` passes `None` for hook registry. Pass actual registry.
- `run_single` line 346: Same — pass hook registry.

</code_context>

<specifics>
## Specific Ideas

- The wiring is largely copy-paste from `run_gateway` with minor adjustments (no gateway-specific config like token overrides, no GatewayRunner construction). The main work is extracting the right tool setup into both CLI paths.
- Consider extracting a shared `wire_full_toolset()` helper to avoid three copies of the same wiring code (run_chat, run_single, run_gateway). Claude's discretion on whether this refactor is worth it.
- The `run_single` path needs the same `active_skills` Arc that `run_chat` uses for execute_code sandbox env pass-through.
- Hook lifecycle events in CLI: `session:start` fires when the REPL starts (or when run_single begins), `session:end` fires on `/quit` or completion, `agent:start/step/end` fire around each `run_agent_turn` call.

</specifics>

<deferred>
## Deferred Ideas

- **Phase 22.1: TUI extension hooks (CLI-02)** — Rust trait or callback system for `_get_extra_tui_widgets()`, `_register_extra_tui_keybindings()`, `_build_tui_layout_children()`, `process_command()`, `_build_tui_style_dict()`. Needs design discussion for Rust equivalent of Python's subclassable CLI hooks.
- **Phase 22.2: ACP adapter (CLI-03..08)** — New `ironhermes-acp` crate. Agent Protocol (agentprotocol.ai). JSON-RPC over stdio. SessionManager, event bridge, permission bridge, tool rendering. VS Code first, then Zed and JetBrains.
- **Additional CLI subcommands** — hermes sessions, hermes config, hermes tools, hermes auth, hermes logs, hermes insights, hermes plugins, hermes mcp, etc. These are hermes-agent CLI parity beyond the current v2.0 requirements. Capture for v2.1+ roadmap.
- **Shared tool wiring helper** — Extract common wiring from run_chat/run_single/run_gateway into a reusable function. Claude's discretion in Phase 22 planning.

### Reviewed Todos (not folded)
- "Configuration and setup wizard improvements" (2026-04-17) — Phase 23 scope (CFG-01..04), not Phase 22.
- "Slash command integration SKILL-13" (2026-04-17) — Separate phase, not Phase 22 scope.
- "Add setup wizard and config scaffolding for gateway testing" (2026-04-02) — Gateway setup, not CLI tool parity.

</deferred>

---

*Phase: 22-cli-feature-parity*
*Context gathered: 2026-04-16*
