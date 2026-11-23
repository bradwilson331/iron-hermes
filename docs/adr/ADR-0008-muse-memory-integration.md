---
type: adr
id: ADR-0008
title: Muse Memory Integration — Semantic Search, Auto-Consolidation, and Forgetting Pipeline (SQLite + USearch)
status: Accepted
date: 2026-10-06
owners: ["@operator"]
links:
  - ./ADR-0005-security-audit-findings.md
  - ./ADR-0006-system-one-risk-classifier.md
  - ./ADR-0007-skill-manifest-model.md
  - https://github.com/muse-ai/muse (reference architecture)
---

# ADR-0008: Muse Memory Integration — Semantic Search, Auto-Consolidation, and Forgetting Pipeline

- **Status:** Accepted (amended by Phase 58.2, 2026-11)
- **Date:** 2026-10-06
- **Deciders:** operator (@twilson)
- **Supersedes:** none · **Superseded by:** none
- **Origin:** Evaluation of Muse memory architecture (Oct 2026). Muse implements a 7-stage memory pipeline: live files → hourly consolidation → derived memory → vector index (384-dim embeddings) → nightly dreams → semantic recall → response guidance. Includes first-class forgetting workflow. **This ADR adapts Muse for IronHermes using SQLite + USearch (embedded, zero external deps) instead of Postgres + pgvector.**

---

## Context

IronHermes memory today is a **two-bucket in-prompt system**:

| Bucket | Limit | Content | Operations |
|--------|-------|---------|------------|
| `memory` | 2,200 chars | General facts, preferences, commitments | Manual `add`/`replace`/`remove` |
| `user` | 1,375 chars | User profile (hardware, interests, patterns) | Manual `add`/`replace`/`remove` |

**What works:**
- Zero dependencies — memory lives in the system prompt
- Manual curation keeps signal high
- Simple mental model for the operator

**What doesn't work:**
- **Hard limits** — 3,575 chars total is insufficient for long-term agent memory
- **No search** — agent cannot query "what did the user say about X?" without manual recall
- **No consolidation** — no automatic extraction of claims from conversations
- **No citations** — no audit trail for where a fact came from
- **No forgetting** — manual `remove` with `old_text` match is fragile
- **No self-improvement** — no nightly review, no alignment synthesis
- **No embeddings** — no semantic similarity, no "related facts" discovery

**Muse's architecture (reference):**

Muse implements a 7-stage pipeline:

1. **Live memory files** — `~/MEMORY.md` (curated facts) + `~/memory/YYYY-MM-DD.md` (daily logs). Agent writes immediately during conversation.
2. **Memory consolidation** (hourly cron) — Extract claims from chat, check against existing claims, promote durable details, let newer claims supersede older ones.
3. **Derived memory** — Source-cited markdown in `~/memory/bank/` (world, experience, opinions, reflections) + `~/memory/people/` & `~/memory/groups/`. Every line cites source file + line number.
4. **Vector memory index** — `memory.entries` (chunks with `memory://` URLs), `memory.embeddings` (384-dim vectors), `memory.claims` (evidence, confidence, supersedes links).
5. **Nightly dreams** (nightly cron) — `~/dreams/YYYY-MM-DD.md` reviews recent interactions as prose. `ALIGNMENT_SYNTHESIS.md` holds standing guidance (preferences, boundaries, friction). Companion YAML files track alignment state, repair threads, progression history. `prompt_hoisted: false` — dream prose isn't injected into context.
6. **Recall and inspect** (later sessions) — `memory_search` returns claim IDs; `memory_explain` returns evidence + source citations.
7. **Guide the next response** — Synthesis + relevant memory applied to the next conversation.

**Forgetting workflow:** Request → Stage claim IDs → Retract linked material (claims, banks, people, dreams, synthesis) → Rebuild the index. Keeps later jobs from restoring removed facts.

**Key design choices:**
- Adaptation via files/records/instructions only — **weights unchanged**
- 384-dim embeddings (small, fast — likely `all-MiniLM-L6-v2`)
- Claim-based memory with explicit supersession (newer claims override older)
- "Dreams" as nightly alignment synthesis (similar to Claude's memory consolidation, but with explicit YAML state tracking)
- Forgetting is a first-class workflow, not just deletion

**The gap:** IronHermes has no semantic search, no auto-consolidation, no citations, no forgetting pipeline. The 2,200-char limit is a hard blocker for any real long-term memory.

---

## Decision

Adopt a **hybrid two-tier memory system**:

1. **Tier 1 (fast, in-prompt):** Keep current `memory`/`user` buckets for simple facts and user profile. Zero deps, always available.
2. **Tier 2 (rich, searchable):** Add Muse-style semantic memory with **SQLite + USearch** (embedded, zero external deps) for long-term, citable, searchable memory.

### Tier 1 — In-Prompt Memory (unchanged)

| Bucket | Limit | Content |
|--------|-------|---------|
| `memory` | 2,200 chars | High-signal facts, immediate preferences |
| `user` | 1,375 chars | User profile (hardware, interests, patterns) |

**Operations:** `add`, `replace`, `remove` (existing).

**Contract:** Tier 1 is for facts that must always be in context. No search, no consolidation. Manual curation.

### Tier 2 — Semantic Memory (new)

**Storage:**
- **Markdown files:** `~/.ironhermes/memory/live/YYYY-MM-DD.md` (daily logs), `~/.ironhermes/memory/bank/*.md` (derived: world, experience, opinions, reflections), `~/.ironhermes/memory/people/*.md`, `~/.ironhermes/memory/groups/*.md`
- **SQLite DB:** `~/.ironhermes/memory/memory.db` — `entries` (chunks with `memory://` URLs), `claims` (evidence, confidence, supersedes links), `embeddings` (384-dim vectors stored as BLOB)
- **USearch index:** `~/.ironhermes/memory/index.usearch` — HNSW index for semantic search, stored as a single binary file (~2MB base + ~4 bytes per vector)

**Operations:**
- `memory_search(query, limit)` — semantic search over Tier 2, returns claim IDs + snippets
- `memory_explain(claim_id)` — return full evidence + source citations for a claim
- `memory_consolidate()` — manual trigger for hourly consolidation (also runs hourly via cron)
- `memory_forget(claim_id)` — initiate forgetting workflow

**Automation:**
- **Hourly consolidation cron** — extract claims from recent chats, update index, supersede old claims
- **Nightly dreams cron** — review recent interactions, update `ALIGNMENT_SYNTHESIS.md`, track alignment state

**Embeddings:** 384-dim vectors via `all-MiniLM-L6-v2` (local, ~80MB, CPU-friendly) or API fallback (OpenAI `text-embedding-3-small`).

**Forgetting workflow:**
1. `memory_forget(claim_id)` → stage claim ID in `~/.ironhermes/memory/forget/pending.json`
2. Nightly job retracts linked material (claims, banks, people, dreams, synthesis)
3. Rebuild index (keep later jobs from restoring removed facts)

### Artifact IDs

- Memory entry: `memory://live/2026-10-06.md#L42`
- Claim: `claim:abc123`
- Derived fact: `memory://bank/world.md#L15`

### Search Contract

`memory_search(query, limit=10)` returns:
```json
{
  "results": [
    {
      "claim_id": "abc123",
      "text": "User prefers concise Telegram replies",
      "source": "memory://bank/world.md#L15",
      "confidence": 0.95,
      "created_at": "2026-10-06T12:00:00Z"
    }
  ]
}
```

`memory_explain(claim_id)` returns:
```json
{
  "claim_id": "abc123",
  "text": "User prefers concise Telegram replies",
  "evidence": [
    {
      "source": "memory://live/2026-10-06.md#L42",
      "quote": "User: keep it short, I'm on mobile",
      "timestamp": "2026-10-06T12:00:00Z"
    }
  ],
  "supersedes": ["claim:xyz789"],
  "confidence": 0.95
}
```

### Consolidation Contract

Hourly consolidation extracts claims from recent chats:
1. Read last 1 hour of chat messages
2. Extract factual claims (preferences, facts, commitments)
3. For each claim:
   - Check if similar claim exists (semantic similarity > 0.85)
   - If exists: supersede old claim, link as evidence
   - If new: create claim, assign confidence based on source (direct statement = 0.95, inferred = 0.7)
4. Promote durable claims to `memory://bank/` (derived memory)
5. Keep transient claims in `memory://live/` (daily logs)

### Dreams Contract

Nightly dreams (2 AM local):
1. Read last 24 hours of `memory://live/` entries
2. Write prose summary to `~/.ironhermes/dreams/YYYY-MM-DD.md`
3. Update `ALIGNMENT_SYNTHESIS.md` (standing guidance, preferences, boundaries, friction)
4. Update companion YAML files:
   - `ALIGNMENT_STATE.yaml` — current alignment status
   - `REPAIR_THREADS.yaml` — ongoing friction points
   - `PROGRESSION_HISTORY.yaml` — how agent behavior has evolved

**Contract:** Dream prose is **not** injected into prompt (`prompt_hoisted: false`). Only `ALIGNMENT_SYNTHESIS.md` is read at session start.

---

## Alternatives Considered

### A. Full Muse adoption (replace Tier 1 entirely)
**Rejected.** Tier 1 (in-prompt) is fast, zero-deps, and works for simple facts. Replacing it with Tier 2 (Postgres + embeddings) adds latency and complexity for facts that are always relevant.

### B. Tier 2 only (no Tier 1)
**Rejected.** Same as A. The two-tier system lets Tier 1 handle "always in context" facts and Tier 2 handle "search when needed" facts.

### C. SQLite + USearch (CHOSEN)
**Selected.** SQLite for metadata + USearch for vector search eliminates Postgres dependency entirely. USearch is a pure Rust crate (~2MB), HNSW algorithm, SIMD-optimized, supports f32/f16/i8 vectors. Zero external deps, no Docker, no managed DB. Trade-off: no SQL-based vector queries (pure vector index), so metadata stays in SQLite, vectors in USearch.

### D. API embeddings only (no local model)
**Rejected.** Local `all-MiniLM-L6-v2` is ~80MB, runs on CPU, and avoids API costs/latency. API fallback (OpenAI) is available for higher quality if needed.

### E. Skip consolidation (manual curation only)
**Rejected.** Manual curation doesn't scale. Hourly consolidation is the key to long-term memory without operator overhead.

---

## Consequences

### Positive

- **No more 2,200-char limit** — actual long-term memory via Tier 2
- **Semantic search** — "what did the user say about X?" without manual recall
- **Auto-consolidation** — agent curates its own memory hourly
- **Source citations** — audit trail for every claim (privacy, debugging)
- **Forgetting workflow** — GDPR/privacy compliance built-in
- **Nightly dreams** — alignment synthesis, self-improvement without weight changes
- **Two-tier system** — Tier 1 for fast/always-in-context, Tier 2 for rich/searchable

### Negative

- **Embedding model** — local (80MB) or API cost (~$0.02/1k tokens for OpenAI)
- **Hourly + nightly crons** — two new scheduled jobs
- **Rust implementation** — Muse is Python; IronHermes is Rust. Must port or wrap.
- **Migration effort** — current `memory`/`user` entries → Tier 2 format. Estimated 1-2 hours.
- **Complexity** — 7-stage pipeline is more complex than current 2-bucket system
- **Dual storage** — SQLite for metadata + USearch for vectors (not a single unified store like Postgres+pgvector)

### Neutral

- Tier 1 (in-prompt) is unchanged. Existing `memory` tool works as before.
- Tier 2 is additive. Operator can ignore it until needed.
- **Zero external deps** — SQLite + USearch are embedded, no Docker, no managed DB, no external service.
- USearch index is a single binary file (~2MB base + ~4 bytes per vector). Easy to back up, portable.

---

## Implementation Sketch

### Phase 1 — SQLite + USearch + Embeddings (1 week)

- **SQLite setup:** `~/.ironhermes/memory/memory.db` — schema: `entries` (id, claim_id, text, source_url, created_at), `claims` (id, text, confidence, supersedes_claim_id, created_at). ~150 LoC (rusqlite migrations).
- **USearch index:** `~/.ironhermes/memory/index.usearch` — HNSW index, 384-dim, cosine distance. Pure Rust crate (`usearch = "2.26"`), zero external deps. ~100 LoC.
- **Embedding model:** Local `all-MiniLM-L6-v2` via `rust-bert` or API fallback (OpenAI). ~150 LoC.
- **`memory_search` tool:** Embed query → USearch ANN search → join with SQLite metadata. ~200 LoC.
- **`memory_explain` tool:** Fetch full evidence from SQLite by claim ID. ~100 LoC.

**Total:** ~700 LoC, 1 week. **Zero external deps.**

### Phase 2 — Live Memory Files + Consolidation (1 week)

- **Live memory files:** `~/.ironhermes/memory/live/YYYY-MM-DD.md`. Agent writes immediately during conversation. ~100 LoC.
- **Hourly consolidation cron:** Extract claims from recent chats, update index, supersede old claims. ~300 LoC.
- **Derived memory:** `~/.ironhermes/memory/bank/*.md` (world, experience, opinions, reflections). ~150 LoC.

**Total:** ~550 LoC, 1 week.

### Phase 3 — Nightly Dreams (3 days)

- **Dreams cron:** Review recent interactions, write prose summary, update `ALIGNMENT_SYNTHESIS.md`. ~250 LoC.
- **Companion YAML files:** `ALIGNMENT_STATE.yaml`, `REPAIR_THREADS.yaml`, `PROGRESSION_HISTORY.yaml`. ~100 LoC.

**Total:** ~350 LoC, 3 days.

### Phase 4 — Forgetting Pipeline (2 days)

- **`memory_forget` tool:** Stage claim ID for deletion. ~50 LoC.
- **Nightly forget job:** Retract linked material, rebuild index. ~150 LoC.

**Total:** ~200 LoC, 2 days.

### Phase 5 — Migration + Integration (3 days)

- **Migration script:** Current `memory`/`user` entries → Tier 2 format. ~100 LoC.
- **Session start integration:** Read `ALIGNMENT_SYNTHESIS.md` + relevant Tier 2 entries at session start. ~150 LoC.
- **Documentation:** Migration guide, operator docs. ~50 LoC.

**Total:** ~300 LoC, 3 days.

### Total Estimated Effort

- **Code:** ~2,050 LoC across 10-12 modules
- **Time:** 4-5 weeks (1 developer)
- **Migration:** 1-2 hours to convert existing memory entries

---

## Rollout Plan

1. **Phase 1 (Week 1):** SQLite + USearch + embeddings + `memory_search`/`memory_explain` tools. No behavior change. Operator can manually search Tier 2.
2. **Phase 2 (Week 2):** Live memory files + hourly consolidation. Agent starts writing to Tier 2 automatically.
3. **Phase 3 (Week 3):** Nightly dreams. Agent starts self-improving via alignment synthesis.
4. **Phase 4 (Week 4):** Forgetting pipeline. Operator can initiate forgetting; nightly job retracts material.
5. **Phase 5 (Week 5):** Migration + integration. Existing memory entries moved to Tier 2. Session start reads Tier 2 synthesis.

**Feature flags:**

- `memory.tier2_enabled` (default: false) — enable Tier 2 semantic memory
- `memory.consolidation_enabled` (default: false) — enable hourly consolidation
- `memory.dreams_enabled` (default: false) — enable nightly dreams
- `memory.forgetting_enabled` (default: false) — enable forgetting pipeline

**Rollback:** Disable feature flags to revert to Tier 1 only. Tier 2 data preserved.

---

## References

- Muse memory architecture (reference implementation) — 7-stage pipeline with vector index + embeddings
- `all-MiniLM-L6-v2` — 384-dim embedding model, ~80MB, CPU-friendly
- **USearch** — pure Rust HNSW vector search crate (`usearch = "2.26"`), ~2MB, zero external deps, SIMD-optimized, supports f32/f16/i8 vectors
- **SQLite** (`rusqlite`) — embedded metadata store, zero external deps
- **LanceDB** — alternative embedded vector DB (rejected: larger binary, more complex API, overkill for this use case)
- ADR-0007 (this repo): skill manifest model (scoped persistence per skill)
- `~/.ironhermes/memory/` — current memory directory (Tier 1 only)
- `crates/ironhermes-memory/` — current memory implementation (in-prompt only)

---

## Amendments (Phase 58.2)

Accepted and implemented in Phase 58.2 with the following changes to the design above. Operator documentation: `docs/MEMORY-TIER2.md`.

- **Layout (D-11).** Tier 2 data is per profile under `<home>/tier2/` (`tier2.db`, `index.usearch`, `live/`, `bank/`, `people/`, `groups/`, `dreams/`, `forget/pending.json`, `models/`), where `<home>` follows `IRONHERMES_HOME`. This replaces `~/.ironhermes/memory/` and `memory.db` (which the `memory-sqlite` provider already owns).
- **Crate.** The implementation is the new crate `ironhermes-tier2`, with its trait in `ironhermes-core::tier2`. The `crates/ironhermes-memory/` named in the References does not exist.
- **USearch is C++17, not pure Rust.** The `usearch` crate compiles C++17 through `cxx`, so builds need a C++ toolchain and the runtime image needs `libstdc++6`. A brute-force cosine index over the SQLite vectors is the fallback and the test twin; `index.usearch` is a derived cache.
- **Embeddings (D-04, D-05, D-06, D-26).** `fastembed` (ONNX Runtime, default-on cargo feature `builtin-embedder`) replaces `rust-bert`; the model downloads on first use into `<home>/tier2/models`. An OpenAI-compatible endpoint (`memory.embeddings.{base_url, model, api_key_env}`, key env-only) is optional and overrides the built-in. Vectors are fixed at 384 dimensions. With no embedder, `memory_search` degrades to labelled FTS5 keyword search.
- **Scheduling (D-15, D-24).** Hourly consolidation and nightly (02:00 local) dreams are loops hosted by the gateway process for its own home, not cron jobs; the CLI hosts no scheduler.
- **Roles (D-16).** Claim extraction and dreams run on a new `model.roles.memory` role (cascade: role, then `auxiliary`, then the main model).
- **Stage order.** Delivered in the order 1, 2, 3, 4, 5 of the Implementation Sketch, in one phase, behind the four flags (all default false).
- **Safety (D-17, D-19).** Claims come only from `user` and `assistant` messages, every claim and quote passes the MEM-05 scanner, and rejections are audited by `kind`. `ALIGNMENT_SYNTHESIS.md` is injected as a capped (1,500 chars), session-frozen block; dream prose is never injected.
- **Tools.** `memory_search` and `memory_explain` are read-only (also available in `execute_code` sandboxes and delegated children); `memory_consolidate` and `memory_forget` are registered for the main agent only. A Memory > Tier 2 tab in the web UI covers search, explain, Forget and Undo.
- **Forgetting: stage and cancel shipped; the purge is deferred (D-27).** `memory_forget` stages a claim, which is hidden from every read surface at once (`forget/pending.json`), and a staged forget can be cancelled by tool or UI (D-21, D-23). The hard delete with a hash tombstone and the removal of the originating line from `MEMORY.md` / `USER.md` (D-20, D-22) are **not implemented**: staged text remains on disk until a purge is designed and shipped. The "Forgetting workflow" steps 2 and 3 and the "GDPR/privacy compliance built-in" consequence above therefore do not hold yet.
- **Known limits.** With the `sqlite` Tier 1 provider, in-session Tier 1 writes are indexed into Tier 2 only at the next process start. First searches on a fresh install run in keyword mode until the built-in model has downloaded. Forgetting could never reach `state.db` / `session_search`, backups or filesystem free blocks.
