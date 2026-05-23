## Task Delegation

When spawning subagents, use the cheapest model that can handle the task:
- Haiku: bulk mechanical tasks - no judgment needed
- Sonnet: scoped research, code exploration, synthesis
- Opus: only for real planning or tradeoff decisions

Spawn rules:
- Haiku cannot spawn subagents. If it needs to, return to parent.
- Max spawn depth: 2
- Subagents escalate to parent, never self-escalate model tier

## Preferred Tools
- Public pages → WebFetch (free, text-only)
- Dynamic pages / auth walls → agent-browser CLI 
- PDFs → pdftotext (not Read tool)
- Repeated fetch patterns → wrap as reusable tool
