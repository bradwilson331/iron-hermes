---
phase: 12-provider-resolution
plan: "01"
subsystem: ironhermes-core
tags: [provider-resolution, config, api-mode, rust]
dependency_graph:
  requires: []
  provides: [ProviderResolver, ResolvedEndpoint, ApiMode, ProviderConfig, CustomProviderConfig, ModelRoleConfig]
  affects: [ironhermes-core]
tech_stack:
  added: []
  patterns: [resolver-pattern, key-scoping, tdd]
key_files:
  created:
    - crates/ironhermes-core/src/provider.rs
  modified:
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-core/src/lib.rs
decisions:
  - "ApiMode enum placed in config.rs (not provider.rs) to avoid circular imports; provider.rs imports from config"
  - "is_provider_url_safe() helper instead of is_safe_url() for base_url validation — is_safe_url blocks localhost which is needed for local model servers"
  - "ApiMode re-exported from lib.rs via config (not via provider) to avoid duplicate export"
metrics:
  duration: "~20 minutes"
  completed: "2026-04-11"
  tasks_completed: 2
  files_changed: 3
---

# Phase 12 Plan 01: Provider Foundation Summary

ProviderResolver with scoped API keys, ApiMode enum, and extended Config — the resolution foundation for all subsequent provider plans.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Create provider.rs with ProviderResolver, ApiMode, ResolvedEndpoint | 3516e01 | crates/ironhermes-core/src/provider.rs, lib.rs |
| 2 | Extend Config with providers, custom_providers, and model.roles | 4be163c | crates/ironhermes-core/src/config.rs |

## What Was Built

**`crates/ironhermes-core/src/provider.rs`**
- `ApiMode` enum with three variants: `ChatCompletions`, `AnthropicMessages`, `CodexResponses` — defined in `config.rs` to avoid circular import
- `ResolvedEndpoint` struct pairing `base_url + api_key + api_mode + default_model + fallback_providers`; `Debug` impl redacts `api_key` (T-12-01)
- `ProviderResolver::build(&config)` — constructs lookup table from Config + env vars
  - Pre-populates three built-in providers: `openrouter` (ChatCompletions), `anthropic` (AnthropicMessages), `openai` (ChatCompletions)
  - Overlays `config.providers` entries on top of built-ins (override, not replace)
  - Adds `config.custom_providers` entries
  - Resolves API keys with precedence: config explicit > provider env var > `config.model.api_key` (main provider only)
  - Key scoping: `OPENROUTER_API_KEY` → openrouter only, `ANTHROPIC_API_KEY` → anthropic only, `OPENAI_API_KEY` → openai + custom
  - Validates `fallback_providers` reference known names (T-12-03)
  - Validates custom provider `base_url`: https or http://localhost/127.0.0.1 only (T-12-02)
- `resolve(&str)` — direct provider lookup
- `resolve_for_main()` — main provider endpoint (panics if missing; startup validation prevents this)
- `resolve_role(&str)` — role lookup; `provider="main"` falls through to main provider with role's model
- 13 unit tests covering all behaviors

**`crates/ironhermes-core/src/config.rs`**
- `ApiMode` enum added here to break circular dependency with provider.rs
- `ProviderConfig` — per-provider override fields, all `Option` with Default=all-None
- `CustomProviderConfig` — name + base_url (required) + optional key/mode/model
- `ModelRoleConfig` — provider name + optional model override
- `Config.providers: HashMap<String, ProviderConfig>` with `#[serde(default)]`
- `Config.custom_providers: Vec<CustomProviderConfig>` with `#[serde(default)]`
- `ModelConfig.roles: HashMap<String, ModelRoleConfig>` with `#[serde(default)]`
- 2 new tests: backward compat (empty maps on old YAML) + full provider section parse

## Test Results

- 13 provider tests passing
- 23 config tests passing (including 2 new provider-related)
- 114 total ironhermes-core tests passing
- `cargo build --workspace` exits 0 (no downstream breakage)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Avoided circular import by placing ApiMode in config.rs**
- **Found during:** Task 1 implementation
- **Issue:** Plan said to define ApiMode in provider.rs, but Config structs need ApiMode in config.rs, and provider.rs imports from config.rs — circular dependency
- **Fix:** Defined ApiMode in config.rs; provider.rs imports it from there. lib.rs exports ApiMode from config (not provider)
- **Files modified:** crates/ironhermes-core/src/config.rs, provider.rs, lib.rs

**2. [Rule 2 - Missing Critical Functionality] Custom is_provider_url_safe() instead of is_safe_url()**
- **Found during:** Task 1 test execution
- **Issue:** `is_safe_url()` from ssrf module blocks all loopback IPs (by design) — but local model servers like Ollama at `http://localhost:11434` are a legitimate use case per the plan
- **Fix:** Added `is_provider_url_safe()` helper that allows https (any host) or http (localhost/127.0.0.1 only)
- **Files modified:** crates/ironhermes-core/src/provider.rs

**3. [Rule 1 - Bug] Rust 2024: set_var/remove_var require unsafe block**
- **Found during:** Task 1 test compilation
- **Issue:** Rust 2024 edition treats `std::env::set_var` as unsafe
- **Fix:** Wrapped test env var calls in `unsafe {}` blocks
- **Files modified:** crates/ironhermes-core/src/provider.rs

## Known Stubs

None — all resolution logic is fully implemented.

## Threat Flags

None — threat mitigations T-12-01, T-12-02, T-12-03 from the plan's threat model are all implemented:
- T-12-01: Debug redacts api_key
- T-12-02: custom provider base_url validated by is_provider_url_safe()
- T-12-03: fallback_providers validated against known names at build() time

## Self-Check: PASSED

- crates/ironhermes-core/src/provider.rs: FOUND
- crates/ironhermes-core/src/config.rs: modified with providers/custom_providers/roles
- crates/ironhermes-core/src/lib.rs: updated with pub mod provider and re-exports
- Commits 3516e01 and 4be163c: FOUND
- 114 tests passing
