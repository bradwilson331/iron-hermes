# Phase 20: Memory Provider Plugin Contract - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-16
**Phase:** 20-memory-provider-plugin-contract
**Areas discussed:** Scope boundary, initialize breaking change, on_memory_write semantics, Setup wizard scope

---

## Pre-discussion state

Phase 20 had an existing draft `20-CONTEXT.md` (24 decisions, 4 proposed plans) marked "Draft — awaiting discussion". User chose **Review & refine** to walk through remaining ambiguities before planning.

---

## Scope boundary

**Context:** Draft excluded runtime plugin loading per `PROJECT.md:52` ("Plugin/extension system — tools are compiled-in; dynamic loading is premature complexity"). User's initial free-text response cited an external URL and recommended YAML manifests + WASM/libloading — a direct conflict with the project constraint. Flagged as potential prompt-injection / hallucinated context (URL could not be verified, and no prior "dynamic discovery" confirmation had been given). Re-asked with explicit options.

| Option | Description | Selected |
|--------|-------------|----------|
| Keep API parity only | Enrich trait to match Python ABC; providers stay compile-time Cargo-feature crates. Honors `PROJECT.md:52`. Shippable in 4 plans. | ✓ |
| Add YAML manifest discovery | Config-driven provider registration via `plugin.yaml`; still compile-time binaries. Medium scope bump. | |
| Full runtime plugin loading | libloading/WASM + manifest. Requires revising `PROJECT.md` Out of Scope. Multi-phase effort. | |
| Revise PROJECT.md first | Pause Phase 20, open a constraint-revision discussion, re-scope afterwards. | |

**User's choice:** Keep API parity only.
**Notes:** Runtime plugin loading captured as a deferred idea. The trait shape makes Phase 20 a natural foundation if the constraint is ever lifted.

---

## initialize breaking change

**Context:** Draft D-10 reshaped `initialize` to `(session_id, hermes_home, &Value)` and kept `MemoryProviderConfig` as a `From<&MemoryProviderConfig> for Value` compat shim (D-21). This leaves two parallel config paths.

| Option | Description | Selected |
|--------|-------------|----------|
| Clean break | Delete `MemoryProviderConfig` entirely. All three provider crates migrate in this phase. | ✓ |
| Keep compat shim for one phase | Providers migrate at their own pace; shim removed in a follow-up phase. | |
| Keep MemoryProviderConfig permanently | Add new args alongside; never deprecate. Two parallel config paths forever. | |

**User's choice:** Clean break.
**Notes:** Consequence — Plan 20-01 now touches every provider crate; plans land in order. D-21 removed from CONTEXT.md. Draft D-10 updated to reflect the breaking signature with no shim.

---

## on_memory_write semantics (fire site)

**Context:** Draft D-14 had the file provider (`MemoryStore`) fire `on_memory_write`. That works when file is the always-on anchor but breaks if a non-file provider becomes primary.

| Option | Description | Selected |
|--------|-------------|----------|
| New MemoryManager layer | Introduce a manager layer that wraps the active provider and fires on write. Matches hermes-agent MemoryManager pattern. | ✓ |
| File provider only | Simple; brittle if primary changes. | |
| Every provider fires it | Peer-to-peer mirroring; write-loop risk without de-dup. | |

**User's choice:** New MemoryManager layer.
**Notes:** Draft D-14 updated. Added new decisions D-25..D-28 describing the MemoryManager module (`crates/ironhermes-agent/src/memory/manager.rs`), its write-path delegation to primary + single mirror, and read-path behavior (primary only).

---

## on_memory_write semantics (subscribers)

**Context:** Draft left "broadcast vs single" to Claude's Discretion. Needed an answer because it affects MemoryManager API and `MEM-12` compatibility.

| Option | Description | Selected |
|--------|-------------|----------|
| Single mirror only | At most one external mirror; matches `MEM-12`. No write-loop risk. | ✓ |
| Broadcast to N subscribers | Future-flexible; requires revisiting `MEM-12` and loop prevention. | |
| Single + API shaped for broadcast | Ship single; use Vec<len<=1> so broadcast is a later config flip. | |

**User's choice:** Single mirror only.
**Notes:** Preserves `MEM-12` — the mirror is observational (write-only shadow), not a peer on reads. Broadcast captured as a deferred idea.

---

## Setup wizard scope

**Context:** Draft D-08 specified a minimal wizard (required + no-default fields only). User's initial free-text response recommended minimal + a separate `hermes <provider> test` subcommand for validation/troubleshooting.

| Option | Description | Selected |
|--------|-------------|----------|
| Minimal | Required + no-default fields only; secrets to `.env`, rest to JSON via `save_config`. | ✓ |
| Richer UX inline | list/test/validate commands folded into Phase 20. | |

**User's choice:** Minimal (confirmed).
**Notes:** `hermes memory list` and `hermes <provider> test` captured as deferred ideas — separate follow-up phase once at least two non-file providers are in real use.

---

## Claude's Discretion

Items the user explicitly or implicitly deferred to implementation judgment:
- Exact additional fields on `ConfigField` beyond the core set (add when a real provider needs them).
- Whether `get_tool_schemas` returns owned `Vec<ToolSchema>` or borrows from `&'static`.
- File layout for the setup wizard module.
- Exact log-message wording for `is_available = false` fallback.
- Whether `MemoryManager` is held as `Arc<Mutex<...>>` or directly in AgentLoop.

---

## Deferred Ideas

- Runtime plugin discovery (YAML manifests, libloading/WASM) — blocked by `PROJECT.md:52`.
- `hermes memory list` / `hermes <provider> test` CLI subcommands.
- Broadcast `on_memory_write` to multiple subscribers.
- Multi-provider peer operation (read + write on both).
- Async `add`/`replace`/`remove` variants.
- Web UI for setup.

---

## Notes on the pasted free-text response

The initial free-text response to the gray-area selection appeared to be output from another AI tool. It cited `hermes-agent.nousresearch.com/docs/developer-guide/memory-provider-plugin` — an unverifiable URL not present in the repo's canonical refs — and falsely claimed prior confirmation of "dynamic discovery" had been given. The stances were treated as user direction (and three of the four aligned with the eventual decisions), but the URL citation was ignored and the scope-boundary item was re-asked with explicit options to surface the conflict with `PROJECT.md:52`.
