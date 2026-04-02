# Deferred Items — Phase 01: Context File Loading

## Pre-existing Clippy Warnings (Out of Scope)

These clippy issues existed before plan 01-01 and are in files not modified by this plan.
Run `cargo clippy -p ironhermes-agent -- -D warnings` to see them.

### crates/ironhermes-agent/src/client.rs
- `type_complexity`: complex tuple type in `deltas` param (line 227) — refactor to named type
- `collapsible_if`: two nested if-let blocks (lines 236, 241) — collapse with `&&`

### crates/ironhermes-agent/src/agent_loop.rs
- `collapsible_if`: nested if-let (line 144) — collapse with `&&`
- `type_complexity`: complex Vec tuple type (line 213) — refactor to named type

### crates/ironhermes-agent/src/context_compressor.rs
- `ptr_arg`: `&mut Vec<ChatMessage>` should be `&mut [ChatMessage]` (line 112)
- `needless_range_loop`: for loop over index only (line 131) — use `.iter_mut()`
- `collapsible_if`: nested if (line 132) — collapse with `&&`

### crates/ironhermes-core/src/config.rs
- `derivable_impls`: manual Default impls that could be derived (lines 18, 114, 132)

### crates/ironhermes-core/src/constants.rs
- `collapsible_if`: nested if-let (line 34) — collapse with `&&`

*Discovered during: 01-01 plan verification pass*
*Status: Deferred — fix in a dedicated cleanup plan*
