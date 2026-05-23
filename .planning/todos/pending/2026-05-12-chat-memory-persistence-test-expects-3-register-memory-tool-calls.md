---
created: 2026-05-12T15:30:00.000Z
title: chat_memory_persistence test expects >=3 register_memory_tool calls but only sees 2
area: testing
files:
  - crates/ironhermes-cli/tests/chat_memory_persistence.rs
---

## Problem

`cargo test -p ironhermes-cli --test chat_memory_persistence` fails on
`run_chat_and_run_single_both_wire_memory_manager`:

```
expected >=3 register_memory_tool calls (gateway rpc + gateway main + chat + single); got 2
crates/ironhermes-cli/tests/chat_memory_persistence.rs:135
```

The test was authored against a world where the gateway path (`gateway rpc` +
`gateway main`) plus the `chat` and `single` run paths each invoke
`register_memory_tool`. With the current embedded-agent architecture (agent runs
in the UI server; no separate gateway/RPC process), the gateway-side
registrations no longer happen, so only 2 calls are observed instead of >=3.

Verified pre-existing: this test fails identically on commit `b05e7951`, i.e.
before phase 27.1.4.1.1 — it is **not** a regression from the transport-error
fallback work. Surfaced by phase 27.1.4.1.1's post-merge `cargo test` gate.

## Solution

Decide which is correct and align the other:

- If the embedded-agent architecture is the intended end state, update the test
  to expect the 2 registrations that actually occur (`chat` + `single`) and drop
  the `gateway rpc` / `gateway main` expectations — or split into two tests.
- If the gateway path is still supposed to register the memory tool, fix the
  wiring so `register_memory_tool` is invoked there.

Either way, also re-check `memory_persists_across_invocations_with_file_provider`
in the same file (currently passing) so the suite stays green.
