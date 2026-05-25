# Phase 22: CLI Feature Parity - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-16
**Phase:** 22-cli-feature-parity
**Areas discussed:** Phase scope / splitting, Tool parity wiring, Hook lifecycle in CLI, ACP adapter design

---

## Phase Scope / Splitting

| Option | Description | Selected |
|--------|-------------|----------|
| Split ACP to Phase 22.1 | Phase 22 = CLI-01 + CLI-02. Phase 22.1 = ACP (CLI-03..08). | |
| Everything in Phase 22 | All 8 requirements in one phase. | |
| Three-way split | Phase 22 = CLI-01. Phase 22.1 = CLI-02. Phase 22.2 = CLI-03..08. | ✓ |

**User's choice:** Three-way split
**Notes:** Maximum granularity. CLI-01 (tool parity) is straightforward wiring. CLI-02 (TUI extension hooks) needs its own design discussion. CLI-03..08 (ACP) is greenfield and architecturally distinct.

---

## Tool Parity Wiring

### Q1: Tool scope beyond CLI-01

| Option | Description | Selected |
|--------|-------------|----------|
| Full parity | Wire ALL missing: execute_code, guardrails, hooks, skills_tool, cron_tool | ✓ |
| CLI-01 only | Just execute_code, guardrails, HookRegistry | |
| CLI-01 + skills_tool | execute_code, guardrails, hooks, plus skills_tool. Cron stays gateway-only. | |

**User's choice:** Full parity
**Notes:** Same tool surface in CLI as gateway.

### Q2: run_single scope

| Option | Description | Selected |
|--------|-------------|----------|
| Both run_chat and run_single | One-shot mode should have the same capabilities | ✓ |
| run_chat only | run_single stays minimal | |

**User's choice:** Both run_chat and run_single
**Notes:** A user running `ironhermes -e 'run my script'` expects execute_code to work.

---

## Hook Lifecycle in CLI

### Q1: Which events to fire

| Option | Description | Selected |
|--------|-------------|----------|
| Same events as gateway | All lifecycle events except gateway:startup | ✓ |
| Tool events only | Only tool:called and tool:completed | |
| You decide | Claude's discretion | |

**User's choice:** Same events as gateway
**Notes:** User provided detailed context about hermes-agent webhook event structure: template-based system with {dot.notation} payload mapping, HMAC security, state persistence in ~/.hermes/webhook_subscriptions.json.

### Q2: Webhook forwarding default

| Option | Description | Selected |
|--------|-------------|----------|
| Both JSONL + webhooks | If hooks.yaml configures webhooks, CLI forwards same as gateway | |
| JSONL only, webhooks opt-in | CLI gets JSONL event logging; webhook forwarding is gateway-only unless configured | ✓ |
| You decide | Claude's discretion | |

**User's choice:** JSONL only, webhooks opt-in
**Notes:** User provided detailed architectural reasoning: JSONL is non-blocking, local-only, auditable. Webhook forwarding increases attack surface, requires payload template mapping, depends on external platform credentials. Recommendation: JSONL for all sessions; webhooks specifically for event-driven tasks (e.g., GitHub PR notifications).

---

## ACP Adapter Design

### Q1: Crate structure

| Option | Description | Selected |
|--------|-------------|----------|
| New crate: ironhermes-acp | Separate crate like ironhermes-gateway | ✓ |
| Module in ironhermes-cli | Submodule of CLI binary | |
| You decide | Claude's discretion | |

**User's choice:** New crate: ironhermes-acp
**Notes:** Clean dependency boundary. Can be compiled independently. Matches hermes-agent pattern.

### Q2: Protocol standard

| Option | Description | Selected |
|--------|-------------|----------|
| Agent Protocol (agentprotocol.ai) | Open standard for VS Code, Zed, JetBrains | ✓ |
| Custom JSON-RPC | Roll our own tailored protocol | |
| You decide | Research during Phase 22.2 | |

**User's choice:** Agent Protocol (agentprotocol.ai)
**Notes:** The standard VS Code Copilot, Zed, and JetBrains are converging on.

### Q3: Target editors

| Option | Description | Selected |
|--------|-------------|----------|
| VS Code first | Largest market share, well-documented protocol | ✓ |
| VS Code + Zed | Both support ACP stdio | |
| All three | VS Code + Zed + JetBrains | |

**User's choice:** VS Code first
**Notes:** Ship one editor integration well before expanding.

---

## Claude's Discretion

- Exact placement of hook emit() calls within run_chat/run_single
- Whether to extract shared wire_tools() helper vs inline wiring
- Whether cron_tool in CLI should be limited or full-featured

## Deferred Ideas

- Phase 22.1: TUI extension hooks (CLI-02) — needs Rust-equivalent design for Python's subclassable CLI hooks
- Phase 22.2: ACP adapter (CLI-03..08) — new ironhermes-acp crate, Agent Protocol, VS Code first
- Additional CLI subcommands (sessions, config, tools, auth, logs, insights, plugins, mcp) — v2.1+ roadmap
