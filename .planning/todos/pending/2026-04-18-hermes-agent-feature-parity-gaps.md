---
title: "hermes-agent feature parity gaps"
area: parity
created: 2026-04-18
source: hermes-agent/AGENTS.md vs IronHermes codebase
priority: high
---

## Overview

Comparison of hermes-agent (Python) AGENTS.md against IronHermes (Rust) codebase.
Items marked ✅ exist in Rust. Items marked ❌ are missing or stubbed.

## Already Implemented ✅

- Agent loop (ironhermes-agent/agent_loop.rs)
- Tool registry + dispatch (ironhermes-tools/registry.rs)
- File tools, web tools, terminal tool, code execution, delegate/subagent, memory tool, skills tool, cron tool
- Prompt builder (10-layer), context compressor, context engine, context loader, subdir discovery
- Personality system (SOUL.md, presets)
- Session search (FTS5)
- Telegram gateway (adapter, session, rate limiter, multimodal, streaming)
- Slash command router (Phase 21.1 — CommandRouter, 49 commands, three-stage resolve)
- CLI TUI (status line, knight rider, pills, activity, keybindings, extensions, double ctrl-c)
- Batch processing (ShareGPT, checkpointing, quality filters)
- Hooks system (ironhermes-hooks crate)
- Cron scheduler (ironhermes-cron crate)
- Skills framework (ironhermes-core/skills.rs, ironhermes-hub)
- SSRF protection, config system, memory providers (SQLite, Grafeo), state DB

## Missing Features ❌

### High Priority (core functionality gaps)

1. **Prompt caching** (Python: `agent/prompt_caching.py`)
   - Anthropic `cache_control` breakpoints, `system_and_3` strategy
   - Cached/ephemeral separation to reduce API costs
   - No equivalent in ironhermes-agent

2. **MCP client tool** (Python: `tools/mcp_tool.py` ~1050 lines)
   - Model Context Protocol client for external tool servers
   - No equivalent in ironhermes-tools

3. **Model metadata & models.dev registry** (Python: `agent/model_metadata.py`, `agent/models_dev.py`)
   - Model context lengths, token estimation, provider-aware context
   - No equivalent in ironhermes-agent

4. **Model switch pipeline** (Python: `hermes_cli/model_switch.py`)
   - Runtime /model switching shared across CLI + gateway
   - Currently a TODO stub in slash command handlers

5. **Browser tool** (Python: `tools/browser_tool.py`)
   - Browserbase browser automation
   - No equivalent in ironhermes-tools

6. **Dangerous command approval flow** (Python: `tools/approval.py`)
   - Gateway approval queue (/approve, /deny) — currently TODO stubs
   - CLI sudo/approval callbacks (Python: `hermes_cli/callbacks.py`)

### Medium Priority (UX & operations)

7. **Skin/theme engine** (Python: `hermes_cli/skin_engine.py`)
   - Data-driven CLI theming (4 built-in skins, user YAML skins)
   - Customizes banner, spinner, colors, branding, tool prefix
   - /skin command is a TODO stub

8. **Profile system** (Python: multi-instance `HERMES_HOME` isolation)
   - `_apply_profile_override()`, `get_hermes_home()` scoping
   - 119+ references in Python; IronHermes has `constants::get_hermes_home()` but no profile switching

9. **Setup wizard** (Python: `hermes_cli/setup.py`)
   - Interactive setup for API keys, providers, config
   - IronHermes has `memory_setup.rs` only — no general setup wizard

10. **Tools config CLI** (Python: `hermes_cli/tools_config.py`)
    - `hermes tools` — enable/disable tools per platform with curses menu
    - /tools command is a TODO stub

11. **Skills config CLI** (Python: `hermes_cli/skills_config.py`)
    - `hermes skills` — enable/disable skills per platform
    - Not implemented

12. **Auxiliary LLM client** (Python: `agent/auxiliary_client.py`)
    - Separate client for vision, summarization tasks
    - Not implemented

13. **Config migration** (Python: `_config_version` in `hermes_cli/config.py`)
    - Version-bumped config migration for existing users
    - Not implemented

### Lower Priority (nice-to-have)

14. **Background process registry** (Python: `tools/process_registry.py`)
    - Background process management, gateway notifications
    - `display.background_process_notifications` config
    - Not implemented

15. **SlashCommandCompleter** (Python: `hermes_cli/commands.py`)
    - Tab-autocomplete for slash commands in prompt_toolkit
    - Not implemented (rustyline may support this differently)

16. **Telegram BotCommand menu** (Python: `telegram_bot_commands()`)
    - Registers commands with Telegram for the / menu
    - May not be implemented in gateway

17. **Trajectory saving** (Python: `agent/trajectory.py`)
    - Conversation logging for debugging/training
    - Not implemented

18. **Auth system** (Python: `hermes_cli/auth.py`)
    - Provider credential resolution, multi-provider auth
    - Not implemented as dedicated module

19. **Additional gateway platforms** (Python: Discord, Slack, WhatsApp, HomeAssistant, Signal)
    - Only Telegram adapter exists
    - Out of scope per PROJECT.md but tracked for awareness

20. **ACP adapter** (Python: `acp_adapter/`)
    - VS Code / Zed / JetBrains integration
    - Phase 22.2 planned (TBD)

## Existing Todos That Overlap

- `cli-feature-parity.md` — overlaps with items 7, 9, 10, 11, 15
- `configuration-setup-wizard-improvements.md` — overlaps with item 9
- `slash-command-integration-skill-13.md` — DONE (Phase 21.1)
- `tool-registry-improvements.md` — overlaps with item 10
