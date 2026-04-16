# IronHermes Roadmap

> Phase-indexed roadmap. Each phase links to a phase directory under `.planning/phases/`.

---

## Active

### Phase 20: Memory Provider Plugin Contract

**Status:** Planned
**Goal:** Bring the Rust `MemoryProvider` trait to API parity with the hermes-agent Python plugin contract (enriched hook surface, `ConfigField` schema, `MemoryManager` layer with write-only mirror) — without introducing runtime plugin discovery, per PROJECT.md:52. Migrate `initialize` signature (breaking) across all three external provider crates. Fold in two pending todos: factory `load_from_disk` regression (Fix 1) and chat-mode memory wiring (Fix 2).

**Requirements:** MEM-07, MEM-08, MEM-09, MEM-10, MEM-11, MEM-12

**Plans:** 4/4 plans complete

Plans:
- [x] 20-01-trait-enrichment-and-factory-fix-PLAN.md — Enrich MemoryProvider trait (defaulted new hooks + required `name()`), introduce `ConfigField`/`MemoryAction` in `ironhermes-core/src/config_schema.rs`, delete `MemoryProviderConfig`, migrate all three provider crates + file `MemoryStore` to new `initialize(session_id, hermes_home, &Value)` signature, make factory async with `load_from_disk` for every provider + `is_available` fallback, round-trip regression test (Fix 1)
- [x] 20-02-memory-manager-and-wiring-PLAN.md — Create `crates/ironhermes-agent/src/memory/manager.rs` (`MemoryManager` wrapping primary + optional write-only mirror, 5s timeout, swallow-on-error, reserved-name guard), rewire `MemoryTool` / `agent_loop.queue_prefetch` / `context_engine.on_pre_compress` / `prompt_builder.system_prompt_block`, add hook-ordering contract test
- [x] 20-03-setup-wizard-and-chat-wiring-PLAN.md — `hermes memory setup` CLI wizard (minimal per D-08; POSIX-safe .env appends, deny-list, `RedactedValue`), wire `MemoryManager` into `run_chat` / `run_single` (Fix 2), cross-invocation persistence regression test
- [x] 20-04-provider-hook-adoption-PLAN.md — Each provider (file/sqlite/duckdb/grafeo) overrides `name()` + `get_config_schema()` with real fields; per-provider config-schema unit tests; sqlite mirror fixture proving `on_memory_write` end-to-end through MemoryManager

**Wave structure:**
- Wave 1: 20-01 (trait + factory + provider migration — autonomous)
- Wave 2: 20-02 (MemoryManager + wiring — depends on 20-01, autonomous)
- Wave 3: 20-03 and 20-04 in parallel (depends on 20-02, both autonomous)

**Phase directory:** `.planning/phases/20-memory-provider-plugin-contract/`

---

### Phase 21: Commandline UI update — polish CLI UX including graceful double ctrl-c handling in agent mode (first interrupt cancels in-flight turn/stream and returns to prompt; second exits cleanly)

**Goal:** Polish `crates/ironhermes-cli/` REPL UX on existing deps (crossterm/rustyline/colored/tokio — no new crates per D-18): render a persistent dot-separated pill status line at the bottom (mode · model · provider · tokens/limit · hint, alternating cyan/magenta/green/yellow/dimmed), animate a 10-cell Knight Rider scanner during in-flight turns/tools, and implement graceful double ctrl-c where the first press cancels the in-flight turn (preserving conversation history) and the second press within 1.5s persists the session as "interrupted" and exits cleanly. Rolls in todo (2026-04-13). CONTEXT.md decisions D-01..D-22 serve as requirements for this phase (no REQ-IDs map).

**Requirements:** (none — D-01..D-22 from 21-CONTEXT.md are the requirements)
**Depends on:** Phase 20
**Plans:** 2/3 plans executed

Plans:
- [x] 21-01-tui-scaffold-and-pure-cores-PLAN.md — Scaffold `crates/ironhermes-cli/src/tui/` module tree (mod.rs, activity.rs, pills.rs, knight_rider.rs, double_ctrl_c.rs, status_line.rs). Implement all pure-function cores with full unit tests: pill color rotation (D-04), knight-rider triangle-wave frame generator (D-06/D-07), double-ctrl-c state machine (D-10..D-14), status-line pure renderer (D-03/D-05). No main.rs wiring yet — zero runtime behavior change.
- [x] 21-02-activity-watch-and-render-task-PLAN.md — Build the rendering I/O layer: `TuiHandle` owning two `tokio::sync::watch` channels (ActivityState + StatusLineState) and a 100ms-tick render task that writes to stderr via crossterm absolute cursor positioning with Hide/Show flicker guards (D-15/D-16/D-17). Auto-detects non-tty stderr and no-ops (Open Q5). Re-queries `size()` each tick for SIGWINCH tolerance. Not yet wired into main.rs.
- [ ] 21-03-run-chat-integration-and-double-ctrl-c-PLAN.md — Wire TuiHandle into `run_chat` (streaming + tool-progress callbacks publish ActivityState; remove old `\r Running: …` clutter per D-08). Install `tokio::signal::ctrl_c` in a `tokio::select!` around the agent future (D-10). Parent CancellationToken lives the session; per-turn children via `.child_token()` (RESEARCH §Pitfall 2). Wire DoubleCtrlCState (D-11, D-12, D-13). Preserve rustyline-Interrupted branch (D-14). 3rd-ctrl-c-within-3s emergency escape (RESEARCH §Pitfall 7). Static-grep regression tests for INV-1..INV-6. Manual VALIDATION.md walkthrough (D-22). Move rolled-in todo to completed/.

**Wave structure:**
- Wave 1: 21-01 (pure-function cores — autonomous)
- Wave 2: 21-02 (TuiHandle + render task — depends on 21-01, autonomous)
- Wave 3: 21-03 (main.rs integration + manual QA — depends on 21-01 and 21-02, NOT fully autonomous: final task is `checkpoint:human-verify`)

**Rolls in todo:** [cli] Double ctrl-c in agent mode ends process and thread (2026-04-13) — see `.planning/todos/pending/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md`

**Phase directory:** `.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/`
