# Feature Landscape: IronHermes v1.1 Automation

**Domain:** AI agent automation capabilities — scheduled tasks, subagent delegation, code execution, event hooks, batch processing
**Researched:** 2026-04-07
**Milestone:** v1.1 Automation (subsequent milestone, existing foundation in place)

---

## Existing Foundation (Already Shipped)

These are not in scope but inform dependencies for new features:

| Capability | Crate | Relevant to New Features |
|---|---|---|
| Cron scheduler | ironhermes-cron | Scheduled tasks extend this directly |
| Tool registry | ironhermes-tools | Subagent delegation, code execution RPC |
| Terminal tool | ironhermes-tools/terminal.rs | Code execution child process |
| Agent loop | ironhermes-agent | Subagent delegation re-enters this loop |
| Telegram gateway | ironhermes-gateway | Multi-platform delivery for scheduled tasks |
| Memory subsystem | ironhermes-tools/memory_tool.rs | Subagent context isolation |

---

## Table Stakes

Features that any agent framework with automation must have. Missing = product feels unfinished relative to the Python original.

| Feature | Why Expected | Complexity | Depends On |
|---|---|---|---|
| Cron expression scheduling | Every scheduler supports cron syntax; already partially built | Low — existing cron crate already handles this | ironhermes-cron |
| Natural language schedule parsing | Users say "every morning at 9am" not "0 9 * * *"; hermes-agent Python has this | Medium — needs an NLP-to-cron translation step (LLM call or a dedicated parser crate like `cron-parser` + LLM interpretation) | ironhermes-cron, LLM client |
| Scheduled task pause/resume/edit | Without this, users delete and recreate tasks; bad UX | Low — cron persistence already exists; add `status` field | ironhermes-cron |
| Subagent spawning with isolated context | Core of the delegate_task tool; hermes-agent supports up to 3 concurrent | High — requires spinning up a fresh agent loop with scoped tool access and enforced concurrency limits | ironhermes-agent, tokio semaphore |
| Restricted toolsets for subagents | Without restriction, subagents have full access — defeats isolation purpose | Medium — tool registry already trait-based; pass a filtered subset at construction | ironhermes-tools/registry.rs |
| Python code execution in child process | execute_code is a meaningful capability reduction without sandbox; simple exec is insufficient | High — requires Unix domain socket RPC, credential stripping, stdout/timeout limits | ironhermes-tools, tokio |
| Event logging hooks | Operators need an audit trail; every production agent framework logs lifecycle events | Low — add hook trait, wire into gateway message receive/send paths | ironhermes-gateway |
| Batch prompt runner | Training data generation is the primary use case; batch without parallelism is just a loop | Medium — JSONL input, tokio parallel workers, ShareGPT output format | ironhermes-agent, file I/O |

---

## Differentiators

Features that go beyond what users expect — they add real value and separate IronHermes from naive re-implementations.

| Feature | Value Proposition | Complexity | Depends On |
|---|---|---|---|
| Skill attachment to scheduled tasks | Tasks can invoke named skills (pre-defined tool sequences) rather than free-form prompts — makes recurring jobs reliable and inspectable | Medium — requires a skill registry concept; simpler if skills are just named tool sequences in config | ironhermes-cron, tool registry |
| Multi-platform delivery for task output | Task results route to Telegram, CLI, or webhook — not just stdout | Medium — delivery abstraction layer; Telegram is already built | ironhermes-gateway |
| Code execution RPC tool passthrough | Python scripts call agent tools (web_search, read_file etc.) via socket, not shelling out — results are identical to normal tool calls | High — Unix socket, JSON serialization, parent-side dispatcher, child-side stub generation | ironhermes-tools, tokio |
| Subagent terminal isolation | Each subagent gets its own terminal session scope — prevents command history/state bleed between concurrent subagents | Medium — terminal tool needs session scoping; currently a single global terminal | ironhermes-tools/terminal.rs |
| Batch checkpointing for fault tolerance | Long batch jobs survive restarts by tracking completed entries by content hash, not position | Medium — stateful progress file, content comparison on resume | ironhermes-state or standalone file |
| ShareGPT-format trajectory output | Batch output is immediately HuggingFace-compatible for fine-tuning — not just raw text | Low-Medium — JSON schema output, "from"/"value" structure with human/assistant/tool roles | batch runner |
| Guardrail plugin hooks | Tool interception hooks let operators block dangerous calls before dispatch (e.g., block terminal in untrusted contexts) | Medium — hook trait needs pre-dispatch interception point, not just post-event logging | ironhermes-tools/registry.rs |
| Automatic quality filtering in batch | Discard trajectories without reasoning coverage or with hallucinated tool names — raises dataset quality without manual review | Medium — post-run heuristic pass; tool name validation against registry | batch runner, tool registry |

---

## Anti-Features

Features to explicitly avoid in this milestone.

| Anti-Feature | Why Avoid | What to Do Instead |
|---|---|---|
| Dynamic plugin loading (.so/.dylib at runtime) | Massive safety surface, complex linking, defeats single-binary goal — PROJECT.md explicitly lists "Plugin/extension system" as out of scope | Compile-in all hooks; use trait objects for extensibility within the binary |
| Per-prompt container images for batch | Hermes-agent Python supports this for benchmarking; enormous operational complexity (Docker daemon dependency, image pull latency) | Use process isolation with credential stripping; containers are a future milestone if needed |
| Discord/Slack delivery for scheduled task output | Out of scope in PROJECT.md until Telegram is solid | Wire delivery abstraction to Telegram only; leave trait open for future adapters |
| Multi-user subagent context separation | Single-operator deployment; multi-user auth is explicitly out of scope | One operator namespace; subagent isolation is within a single operator's context |
| Persistent subagent state across sessions | Subagents are ephemeral work units, not persistent agents — storing state adds complexity with unclear benefit | Subagents write results to files or memory if persistence is needed; the parent agent handles continuity |
| Interactive subagent communication | Subagents receive a task and return a result; bidirectional interactive sessions would require a second message bus | Parent polls result on completion; no mid-task steering in v1.1 |

---

## Feature Dependencies

```
Natural language scheduling → Cron scheduler (existing)
Skill attachment → Natural language scheduling + Skill registry (new concept)
Multi-platform delivery → Telegram gateway (existing) + Delivery abstraction (new)

Subagent delegation → Agent loop (existing) + Tool registry filtering (existing, extend)
Subagent terminal isolation → Terminal tool (existing, add session scope)
Concurrency limit (3) → tokio::sync::Semaphore (new, straightforward)

Code execution (child process) → Terminal tool (existing) + Unix socket RPC (new)
Code execution credential stripping → Environment handling (new, in execute_code)
Code execution tool passthrough → Tool registry dispatch (existing) + RPC layer (new)

Event logging hooks → Gateway message paths (existing, add hook trait)
Guardrail hooks → Tool registry dispatch (existing, add pre-dispatch intercept)
Webhook delivery → Event hooks + HTTP client (reqwest, already available via web tools)

Batch runner → Agent loop (existing) + tokio workers (new)
Batch checkpointing → File I/O (existing write_file tool) + progress state file (new)
ShareGPT output → Batch runner + JSON serialization (serde_json, already in workspace)
Quality filtering → Batch runner + tool registry (for name validation)
```

---

## MVP Recommendation

For v1.1, prioritize in this order based on dependency chain and risk:

**Phase 1 — Scheduled Tasks (extend existing cron)**
Build on the shipped cron crate. Natural language parsing is the only genuinely new mechanism; delivery and skill attachment sit on top. Low blast radius if natural language parsing is imperfect — fall back to cron expressions.

**Phase 2 — Event Hooks (low complexity, high operational value)**
Hook trait is additive and non-breaking. Do this before code execution or subagents because hooks provide the observability needed to debug those higher-complexity features.

**Phase 3 — Code Execution (isolated, high complexity)**
Unix socket RPC is the hardest new mechanism. Build and test independently of subagents — they share the child-process pattern but are architecturally separate.

**Phase 4 — Subagent Delegation (re-uses agent loop)**
Requires the agent loop to be re-entrant and the tool registry to support filtering. Event hooks should already be in place to monitor subagent activity.

**Phase 5 — Batch Processing (parallelizes existing loop)**
Lowest risk because it re-uses the full agent loop. Complexity is in checkpointing and output formatting, not agent behavior. Do last so the agent loop is stable.

**Defer:**
- Skill registry as a standalone concept — in v1.1, skills can be named prompts in config; a full registry is a v1.2 concern
- Webhook delivery for event hooks — log to file and Telegram first; HTTP webhook is a follow-on

---

## Complexity Summary

| Feature | Complexity | Primary Risk |
|---|---|---|
| Natural language schedule parsing | Medium | LLM accuracy for edge-case time expressions; need validation against cron parser |
| Scheduled task pause/resume/edit | Low | Schema migration for existing cron persistence files |
| Subagent spawning + concurrency limit | High | Re-entrancy in agent loop; tokio task cancellation on timeout |
| Restricted toolsets for subagents | Medium | Registry API design — must not break existing tool dispatch |
| Code execution child process + RPC | High | Unix socket lifecycle, partial reads, credential stripping correctness |
| Event logging hooks | Low | None — pure addition |
| Guardrail hooks (pre-dispatch intercept) | Medium | Hook ordering, error propagation (should a hook rejection be an error or silent drop?) |
| Batch runner with parallelism | Medium | Checkpointing correctness on partial failure; ShareGPT schema validation |
| Quality filtering for batch | Medium | Heuristics are inherently imprecise; need configurable thresholds |

---

## Sources

- Hermes-agent overview: https://hermes-agent.nousresearch.com/docs/user-guide/features/overview (HIGH confidence — official docs, fetched directly)
- Code execution details: https://hermes-agent.nousresearch.com/docs/user-guide/features/code-execution (HIGH confidence — official docs, fetched directly)
- Batch processing details: https://hermes-agent.nousresearch.com/docs/user-guide/features/batch-processing (HIGH confidence — official docs, fetched directly)
- IronHermes PROJECT.md: /Users/twilson/code/ironhermes/.planning/PROJECT.md (HIGH confidence — source of truth for existing features and constraints)
- IronHermes codebase structure: direct inspection of /Users/twilson/code/ironhermes/crates/ (HIGH confidence)
- Scheduled tasks and event hooks detail pages: 404 — fell back to overview summary and PROJECT.md scope description (MEDIUM confidence for those two features)
