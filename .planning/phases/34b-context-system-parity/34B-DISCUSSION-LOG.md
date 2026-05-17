# Phase 34b: Context-System Parity - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-05-16
**Phase:** 34b-context-system-parity
**Areas discussed:** @url: expansion depth, allowed_root default, ContextEngine trait shape

---

## @url: expansion depth

| Option | Description | Selected |
|--------|-------------|----------|
| HTTP-only (no LLM processing) | Fast, no extra cost. Raw markdown/text injected as-is. | |
| LLM-processed (mirrors Python) | Fetches then runs a cleaning LLM call. Polished markdown output. Slower + costs extra tokens. Matches Python behavior exactly. | ✓ |
| Configurable (default HTTP-only) | Default HTTP-only; allow use_llm_processing: true in config for power users. | |

**User's choice:** LLM-processed (mirrors Python)
**Notes:** None.

---

### @url: error handling

| Option | Description | Selected |
|--------|-------------|----------|
| Fall back to raw HTTP content | Inject unprocessed fetch result with a warning. Agent still gets the content. | ✓ |
| Surface error to user, skip expansion | Tell user @url: couldn't be expanded, preserve original message. | |
| You decide | Claude picks the safest option at implementation time. | |

**User's choice:** Fall back to raw HTTP content
**Notes:** Warning surfaces in the `--- Context Warnings ---` block. Agent still gets content, just unpolished.

---

## allowed_root default

| Option | Description | Selected |
|--------|-------------|----------|
| cwd (mirrors Python) | Only paths inside the working directory expand. Tightest blast radius. | ✓ |
| $HOME | Any file under the user's home directory. More convenient, larger blast radius. | |

**User's choice:** cwd (mirrors Python)
**Notes:** None.

---

### allowed_root configurability

| Option | Description | Selected |
|--------|-------------|----------|
| Fixed to cwd — no config escape hatch | Simpler, smaller attack surface. | ✓ |
| Configurable in cli-config.yaml | Add context_refs.allowed_root for power users. | |
| You decide | Claude picks the simpler path. | |

**User's choice:** Fixed to cwd — no config escape hatch
**Notes:** Sensitive-path blocklist is a second independent defense layer.

---

### Which cwd to use

| Option | Description | Selected |
|--------|-------------|----------|
| Process cwd at startup (std::env::current_dir) | Wherever hermes was launched from. | |
| TerminalConfig.cwd if set, else process cwd | Honors the configured cwd from cli-config.yaml. Falls back to process cwd if not set. | ✓ |

**User's choice:** TerminalConfig.cwd if set, else process cwd
**Notes:** Consistent with how the terminal tool already resolves its working directory.

---

## ContextEngine trait shape

| Option | Description | Selected |
|--------|-------------|----------|
| Additive on existing ContextEngine trait | Add hooks with default no-op impls. Mirrors Python's single ABC. No breaking changes. | ✓ |
| Separate ContextEngineLifecycle trait | Cleaner conceptual separation, but Rust trait objects don't compose cleanly — more boilerplate. | |

**User's choice:** Additive on existing ContextEngine trait
**Notes:** `check_pressure` default no-op already demonstrates this pattern in the codebase.

---

### update_from_response / update_model wiring

| Option | Description | Selected |
|--------|-------------|----------|
| Just define on the trait (stub for LCM) | Add no-ops now; wire call sites when LCM actually uses them. | |
| Wire at call sites now | Call after every LLM response and on model changes. Full Python parity. | ✓ |

**User's choice:** Wire at call sites now
**Notes:** Full Python parity; `update_from_response` called after every `AgentLoop::run`, `update_model` on model changes.

---

## Claude's Discretion

- Exact type for `update_from_response` usage parameter — `AggregatedUsage` (already in agent_loop.rs) or a new `UsageReport` alias
- `has_content_to_compress` default returns `true`; implementors override if they develop a cheaper early-exit check
- Exact position of `preprocess_context_references_async` in each surface's call path

## Deferred Ideas

None raised during discussion — all deferred items carried over from the 34b draft plan (focus_topic, LCM tools, MemoryProvider lifecycle hooks, only-one-external-provider guard).
