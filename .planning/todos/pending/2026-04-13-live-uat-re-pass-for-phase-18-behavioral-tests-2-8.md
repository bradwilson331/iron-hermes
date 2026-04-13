---
created: 2026-04-13T02:56:41.555Z
title: Live UAT re-pass for Phase 18 behavioral tests 2-8
area: testing
files:
  - .planning/phases/18-context-compression/18-UAT.md
  - .planning/phases/18-context-compression/18-VERIFICATION.md
  - crates/ironhermes-agent/src/agent_wiring.rs
  - crates/ironhermes-gateway/src/runner.rs
---

## Problem

Phase 18 passed structural verification (7/7 requirements shipped, gap closures 18-08 and 18-09 merged to develop at `620005e` and `cea4519`), but UAT tests 2–8 describe runtime behavioral contracts that cannot be confirmed by `cargo test` alone. Specifically:

- Test 2: agent `compression_threshold` triggers `pre_chat_compress` engine branch at 50% (not the legacy `compressor` fallback)
- Test 3: gateway `compression_threshold` triggers `maybe_compress_gateway` at 85%
- Test 4: three-channel pressure warning fires at 85% of engine threshold with per-session cooldown
- Test 5: tool-pair atomicity holds under live compression
- Test 6: aux-model fallback works when `build_role_client("compression")` returns None
- Test 7: memory-flush ordering via `context:pre_compress` hook
- Test 8: SystemMessage slot (durable slot 2) inspection

These need a live run against the agent + gateway with a large context to confirm.

## Solution

Run the UAT scenarios end-to-end against `ironhermes-cli` and `ironhermes-gateway`:
1. Configure a test profile with low `compression_threshold` values (e.g., 1000 tokens) to force threshold crossings on short conversations
2. Execute each UAT test in 18-UAT.md, capturing tracing output
3. Verify tracing events show engine branch (not `compressor` fallback) and correct threshold attribution
4. Update `18-VERIFICATION.md` with live results; if any test fails, file a new gap closure plan rather than reopening 18-08/09
