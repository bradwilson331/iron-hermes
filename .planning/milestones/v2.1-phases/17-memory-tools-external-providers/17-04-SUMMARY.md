---
phase: 17-memory-tools-external-providers
plan: 04
status: complete
commit: 83e24ec
---

# Plan 17-04 Summary: Grafeo Memory Provider

## What was built

Grafeo embedded graph database memory provider as a feature-gated crate at `providers/memory-grafeo/`.

### Task 1: Grafeo memory provider crate with graph-based storage
- Created `providers/memory-grafeo/` crate with `Cargo.toml`, `src/lib.rs`, `src/schema.rs`
- `GrafeoMemoryProvider` implements `MemoryProvider` trait using Grafeo LPG graph
- Memory entries stored as nodes with `content`, `target`, `created_at` properties
- `NODE_LABEL = "MemoryEntry"` with property indexes for fast lookups
- Security scanning via `scan_context_content()` on every write (T-17-08)
- Capacity enforcement: 2200 chars for Memory, 1375 for User (T-17-09)
- Frozen-snapshot pattern: `load_from_disk()` captures snapshot, mutations don't update it
- Substring matching for replace/remove with ambiguity detection
- Persistence survives database reopen
- 16 tests passing

### Task 2: Factory integration and feature gates
- Added `providers/memory-grafeo` to workspace members in root `Cargo.toml`
- Added `memory-grafeo` optional dependency and feature gate in `ironhermes-agent`
- Added `#[cfg(feature = "memory-grafeo")]` match arm in `factory.rs`
- Updated "Available providers" error message to include grafeo

## Verification
- `cargo test -p memory-grafeo` — 16 tests pass
- `cargo check -p ironhermes-agent --features memory-grafeo` — compiles
- `cargo check -p ironhermes-agent` — compiles (default, no features)

## Requirements covered
- MEM-10: Graph database memory provider
