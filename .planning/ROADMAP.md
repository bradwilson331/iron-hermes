# IronHermes Roadmap

> Phase-indexed roadmap. Each phase links to a phase directory under `.planning/phases/`.

---

## Active

### Phase 20: Memory Provider Plugin Contract

**Status:** Planned
**Goal:** Bring the Rust `MemoryProvider` trait to API parity with the hermes-agent Python plugin contract (enriched hook surface, `ConfigField` schema, `MemoryManager` layer with write-only mirror) — without introducing runtime plugin discovery, per PROJECT.md:52. Migrate `initialize` signature (breaking) across all three external provider crates. Fold in two pending todos: factory `load_from_disk` regression (Fix 1) and chat-mode memory wiring (Fix 2).

**Requirements:** MEM-07, MEM-08, MEM-09, MEM-10, MEM-11, MEM-12

**Plans:** 2/4 plans executed

Plans:
- [x] 20-01-trait-enrichment-and-factory-fix-PLAN.md — Enrich MemoryProvider trait (defaulted new hooks + required `name()`), introduce `ConfigField`/`MemoryAction` in `ironhermes-core/src/config_schema.rs`, delete `MemoryProviderConfig`, migrate all three provider crates + file `MemoryStore` to new `initialize(session_id, hermes_home, &Value)` signature, make factory async with `load_from_disk` for every provider + `is_available` fallback, round-trip regression test (Fix 1)
- [x] 20-02-memory-manager-and-wiring-PLAN.md — Create `crates/ironhermes-agent/src/memory/manager.rs` (`MemoryManager` wrapping primary + optional write-only mirror, 5s timeout, swallow-on-error, reserved-name guard), rewire `MemoryTool` / `agent_loop.queue_prefetch` / `context_engine.on_pre_compress` / `prompt_builder.system_prompt_block`, add hook-ordering contract test
- [ ] 20-03-setup-wizard-and-chat-wiring-PLAN.md — `hermes memory setup` CLI wizard (minimal per D-08; POSIX-safe .env appends, deny-list, `RedactedValue`), wire `MemoryManager` into `run_chat` / `run_single` (Fix 2), cross-invocation persistence regression test
- [ ] 20-04-provider-hook-adoption-PLAN.md — Each provider (file/sqlite/duckdb/grafeo) overrides `name()` + `get_config_schema()` with real fields; per-provider config-schema unit tests; sqlite mirror fixture proving `on_memory_write` end-to-end through MemoryManager

**Wave structure:**
- Wave 1: 20-01 (trait + factory + provider migration — autonomous)
- Wave 2: 20-02 (MemoryManager + wiring — depends on 20-01, autonomous)
- Wave 3: 20-03 and 20-04 in parallel (depends on 20-02, both autonomous)

**Phase directory:** `.planning/phases/20-memory-provider-plugin-contract/`
