# Project Retrospective

*A living document updated after each milestone. Lessons feed forward into future planning.*

## Milestone: v1.0 — MVP

**Shipped:** 2026-04-08
**Phases:** 4 | **Plans:** 11

### What Was Built
- Context file loading with priority-chain assembly and injection scanning
- Telegram gateway with streaming, concurrent users, graceful shutdown
- Self-improvement with security scanning and bounded memory subsystem
- Web scraping via Firecrawl with SSRF protection and local fallback

### What Worked
- Cargo workspace structure mapped cleanly from Python module layout
- TDD approach caught issues early in context loading and cron
- SQLite with WAL mode and FTS5 provided solid persistence foundation

### What Was Inefficient
- Phase 2 plan 05 (multimodal input) deferred — scope was too large for initial gateway
- anyhow everywhere means error types aren't self-documenting

### Patterns Established
- 7-crate workspace: core, state, tools, agent, cli, gateway, cron
- Config via YAML + .env at ~/.ironhermes/
- Security scanning on all context file writes

### Key Lessons
1. Get the core agent loop rock-solid before adding features — it's the foundation everything builds on
2. Gateway-only vs CLI feature parity is a conscious architectural decision worth documenting early

---

## Milestone: v1.1 — Automation

**Shipped:** 2026-04-11
**Phases:** 12 | **Plans:** 34 | **Commits:** 204

### What Was Built
- Scheduled tasks with natural language parsing (cron/interval/once), skill attachment, multi-platform delivery
- Event hooks with JSONL lifecycle logging, guardrail tool interception, webhook forwarding with HMAC + retry
- Skills system with progressive disclosure, agentskills.io compatibility, allowed_tools enforcement
- Python code execution sandbox with JSON-RPC tool bridge, env stripping, resource limits
- Subagent delegation with isolated context, semaphore concurrency, batch mode, cancellation propagation
- Batch processing with parallel ShareGPT output, content-hash checkpointing, 4-criteria quality filtering

### What Worked
- Phase ordering (SCHED → HOOK → SKILL → EXEC → AGENT → BATCH) — hooks early meant observability was ready for later phases
- Gap analysis phases (07.1, 07.2, 07.3, 07.4, 07.5) caught integration issues before they compounded
- Milestone audit identified the active_skills Arc mismatch before it shipped as a silent bug
- TDD with wave-0 test scaffolding in code execution and subagent phases

### What Was Inefficient
- Multiple gap closure phases (07.1-07.5, 10.1) — suggests initial phase scoping could be tighter
- SUMMARY.md frontmatter format inconsistency made automated extraction unreliable
- Nyquist validation scaffolds created but never formally signed off for most phases
- Some VERIFICATION.md files left in "human_needed" state without follow-through

### Patterns Established
- New crates (ironhermes-hooks, ironhermes-exec) for cleanly separable subsystems
- Arc<Mutex<Vec<T>>> shared state pattern for cross-component communication (skills, hooks)
- Setter injection pattern (set_hook_registry, set_skill_registry, set_active_skills) for optional Arc fields
- RegistryDispatch adapter pattern for bridging tool registries across crate boundaries
- Pattern-based env exclusion over allowlist for sandbox security

### Key Lessons
1. Integration testing across crate boundaries is critical — the active_skills Arc mismatch was a wiring bug, not a logic bug
2. Gap analysis phases are valuable but should be budgeted upfront rather than discovered mid-milestone
3. Subphase numbering (07.1, 07.2, etc.) scales well for inserted work without disrupting the roadmap
4. Gateway-only feature registration is a reasonable architectural boundary but should be documented as a conscious constraint

### Cost Observations
- Model mix: primarily opus for architecture/planning, sonnet for execution
- 204 commits across 3 days (Apr 8-11)
- Notable: 12 phases in 3 days — high velocity enabled by clear phase scoping and TDD

---

## Cross-Milestone Trends

### Process Evolution

| Milestone | Phases | Plans | Key Change |
|-----------|--------|-------|------------|
| v1.0 | 4 | 11 | Established workspace structure, core patterns |
| v1.1 | 12 | 34 | Added gap analysis phases, milestone audits, subphase numbering |

### Cumulative Quality

| Milestone | Tests | Key Quality Gate |
|-----------|-------|-----------------|
| v1.0 | 31 | Manual testing of Telegram gateway |
| v1.1 | 382+ | Milestone audit with cross-phase integration verification |

### Top Lessons (Verified Across Milestones)

1. Get the foundation right first — both milestones benefited from solid core patterns before feature expansion
2. Integration wiring bugs are the most dangerous class — unit tests pass but features don't connect
3. Gap analysis and audit phases pay for themselves in prevented rework
