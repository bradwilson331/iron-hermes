---
created: 2026-04-17T03:11:41.262Z
title: CLI feature parity
area: cli
files: []
---

## Problem

execute_code, hooks, and guardrails are currently gateway-only (architectural decision from v1.1, marked "Revisit" in PROJECT.md). CLI mode needs feature parity so the interactive REPL can use code execution, hook lifecycle events, and guardrail tool interception. Also needs CLI extension hooks for TUI widgets/keybindings. ACP adapter (JSON-RPC stdio server) for VS Code/Zed/JetBrains integration.

Covers requirements: CLI-01 through CLI-08.

## Solution

Wire execute_code tool registration, hook dispatcher, and guardrail middleware into CLI's run_chat path. Implement ACP adapter as separate entry point with SessionManager, event bridge, permission bridge, and tool rendering.
