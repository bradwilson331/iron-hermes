# Codebase Quality Analysis

**Analysis Date:** 2026-04-01

## Test Coverage

**Overall:** Very low. Only 2 of 7 crates have any tests at all.

**Crates with tests:**

| Crate | Test Count | What's Tested |
|-------|-----------|---------------|
| `ironhermes-cron` | 9 tests | `compute_next_run` (5/6/invalid field), `JobStore` roundtrip/remove/toggle/due/mark_run, `acquire_tick_lock` |
| `ironhermes-agent` | 2 tests | `estimate_tokens` basic arithmetic, `should_compress` threshold check |

**Test file locations:**
- `crates/ironhermes-cron/src/lib.rs` (line 279) -- inline `#[cfg(test)]` module
- `crates/ironhermes-agent/src/context_compressor.rs` (line 197) -- inline `#[cfg(test)]` module

**Crates with ZERO tests:**

| Crate | Risk | What Needs Testing |
|-------|------|--------------------|
| `ironhermes-core` | Medium | Config load/save, `resolve_api_key` logic, `resolve_base_url` fallback chain, serde round-trips for `ChatMessage`/`MessageContent` |
| `ironhermes-state` | **High** | SQLite schema init, migrations (v1-v6), `add_message`, `get_messages`, `search_messages` FTS, `update_session_stats`, session lifecycle |
| `ironhermes-tools` | **High** | `ToolRegistry.dispatch`, `ReadFileTool`/`WriteFileTool`/`PatchFileTool`/`SearchFilesTool` execute logic, `WebSearchTool` HTTP handling, `TerminalTool` timeout behavior |
| `ironhermes-agent` (agent_loop, client) | **High** | `AgentLoop.run` iteration logic, streaming assembly via `assemble_tool_calls_from_stream`, `LlmClient` HTTP error handling |
| `ironhermes-gateway` | Medium | `TelegramAdapter` polling/send/edit/delete, `SessionStore` get_or_create/remove, `GatewayRunner` lifecycle |
| `ironhermes-cli` | Low | CLI is thin orchestration; integration tests would be more useful |

**Test infrastructure:**
- No test framework beyond built-in `#[test]`
- No async test support (no `#[tokio::test]` anywhere)
- `tempfile` crate used in `ironhermes-cron` for test isolation -- good pattern
- No mocking framework -- will need one for `LlmClient` and HTTP calls
- No CI configuration detected (no `.github/workflows/`, no `Makefile`, no `justfile`)
- No coverage tooling configured

**Test patterns (from existing tests):**
```rust
// Good: temp directory isolation for filesystem tests
fn tmp_cron_dir() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let cron_dir = dir.path().join("cron");
    (dir, cron_dir)
}

// Tests use expect() for setup, assert!() / assert_eq!() for verification
#[test]
fn test_job_store_roundtrip() {
    let (_dir, cron_dir) = tmp_cron_dir();
    let mut store = JobStore::open(cron_dir.clone()).expect("store");
    // ...
    assert_eq!(store.list_jobs().len(), 1);
}
```

## Error Handling

**Strategy:** Dual-layer approach using both `thiserror` (typed errors) and `anyhow` (ad-hoc errors).

**Typed error enums:**

1. `HermesError` in `crates/ironhermes-core/src/error.rs`:
   - 12 variants covering Config, Api, Tool, State, Provider, ContextOverflow, MaxIterations, Io, Json, Http, NotFound, Unauthorized, Other
   - Includes `#[from]` conversions for `std::io::Error`, `serde_json::Error`, and `anyhow::Error`
   - Defines `pub type Result<T> = std::result::Result<T, HermesError>`

2. `StateError` in `crates/ironhermes-state/src/lib.rs`:
   - 4 variants: Sqlite, Json, SessionNotFound, Other
   - `#[from]` for `rusqlite::Error`, `serde_json::Error`, `anyhow::Error`
   - Defines `pub type Result<T, E = StateError> = std::result::Result<T, E>`

**Actual usage pattern:** Despite having typed errors, most code uses `anyhow::Result` directly:

| Crate | Error Strategy |
|-------|---------------|
| `ironhermes-core` | Defines `HermesError` but `config.rs` returns `anyhow::Result` |
| `ironhermes-state` | Uses own `StateError` consistently -- **best practice in the codebase** |
| `ironhermes-tools` | All tool `execute()` methods return `anyhow::Result<String>` |
| `ironhermes-agent` | `anyhow::Result` everywhere |
| `ironhermes-cli` | `anyhow::Result` everywhere |
| `ironhermes-gateway` | `anyhow::Result` everywhere |
| `ironhermes-cron` | `anyhow::Result` everywhere with good `.with_context()` usage |

**Concern:** `HermesError` is defined but largely unused outside its own module. Code that could benefit from typed error matching (e.g., retry on `Api` errors, handle `ContextOverflow` in agent loop) uses `anyhow` instead, losing type information.

**Context annotations:** Good usage of `.with_context()` in `ironhermes-cron` and `ironhermes-state`. Less consistent in `ironhermes-tools` which uses `.map_err(|e| anyhow::anyhow!(...))` instead of `.context()`.

**`unwrap()` usage (5 total):**
- `crates/ironhermes-cron/src/lib.rs:342,344,373` -- test code only (acceptable)
- `crates/ironhermes-cli/src/main.rs:67` -- `parse().unwrap()` on a static tracing directive (acceptable)
- `crates/ironhermes-agent/src/agent_loop.rs:158` -- `unwrap()` after an `is_some_and()` guard (safe but could use `if let`)

**No `unsafe` code** anywhere in the codebase.

## Documentation

**Doc comments (`///` and `//!`):** 131 total across 13 files.

**Module-level docs (`//!`):**
- `crates/ironhermes-state/src/lib.rs` -- good module-level doc explaining purpose and design constraints (sync rusqlite)

**Per-crate documentation quality:**

| Crate | Doc Quality | Notes |
|-------|------------|-------|
| `ironhermes-core` | Low | No doc comments on types in `types.rs` (ChatMessage, Role, ToolSchema, etc.) |
| `ironhermes-state` | **Good** | Module doc, doc comments on `StateStore`, all public methods documented |
| `ironhermes-tools` | Low | `Tool` trait and `ToolRegistry` undocumented; individual tools have JSON schema descriptions but no Rust doc comments |
| `ironhermes-agent` | Medium | `AgentLoop.run` has good doc comment explaining termination conditions; `LlmClient` methods have brief docs |
| `ironhermes-gateway` | Low | Trait `PlatformAdapter` and `MessageHandler` undocumented |
| `ironhermes-cron` | **Good** | `JobStore` methods all documented; `compute_next_run` and `acquire_tick_lock` well documented |
| `ironhermes-cli` | Low | No doc comments (CLI functions use `///` sparingly) |

**README:** `README.md` at project root is adequate -- covers architecture table, quick start, configuration, and tool list. No per-crate READMEs.

**Section comment style:** Code uses `// ===...===` banners in `types.rs` and `// ---...---` dividers in `state/lib.rs` and `cron/lib.rs` for section separation. Consistent within files.

## Code Style

**Formatting:** No `rustfmt.toml` or `.rustfmt.toml` detected. Default `rustfmt` settings assumed. Code appears consistently formatted.

**Linting:** No `clippy.toml`, no `#[allow(...)]`, `#[deny(...)]`, or `#[warn(...)]` attributes anywhere. No workspace-level lint configuration in `Cargo.toml`. Code should be run through `cargo clippy` to establish baseline.

**Rust edition:** 2024 (set in `[workspace.package]`).

**Naming conventions:**
- Files: `snake_case.rs` -- consistent
- Types/structs: `PascalCase` -- consistent
- Functions: `snake_case` -- consistent
- Constants: `SCREAMING_SNAKE_CASE` -- consistent
- Crate names: `ironhermes-{module}` kebab-case -- consistent

**Import organization:** No enforced order. General observed pattern:
1. `std` / standard library
2. External crates (`anyhow`, `serde`, `tokio`, etc.)
3. Workspace crates (`ironhermes_core`, etc.)
4. Local (`crate::`, `super::`)

**Builder pattern:** Used consistently for configuration objects:
- `AgentLoop::new().with_compression().with_streaming().with_tool_progress()`
- `PromptBuilder::new().with_identity().with_skills().with_context_files()`
- `SessionKey::new().with_user()`

**`impl Into<String>` for constructors:** Consistent use of `impl Into<String>` for string parameters across `LlmClient::new`, `PromptBuilder::new`, `JobStore.add_job`, `ChatMessage::system/user/assistant`, `GatewaySession::new`.

**Serde patterns:** Consistent use of `#[serde(skip_serializing_if = "Option::is_none")]` on optional fields, `#[serde(default)]` on config structs, `#[serde(flatten)]` for extension maps.

**File sizes:** All files are under 550 lines. Largest is `ironhermes-state/src/lib.rs` at 540 lines. No excessively large files.

**Single-file crates:** `ironhermes-state` and `ironhermes-cron` are each a single `lib.rs`. This is acceptable given their size (540 and 394 lines respectively), but `ironhermes-state` may benefit from splitting schema/migrations from query methods as it grows.

## Known Issues / TODOs

**No TODO/FIXME/HACK/XXX comments** found anywhere in the codebase. This is either very clean or indicates the project is young and debt hasn't been annotated yet.

**Identified concerns:**

1. **`HermesError` is dead code in practice**
   - Files: `crates/ironhermes-core/src/error.rs`
   - The typed error enum is well-designed but nearly all calling code uses `anyhow::Result` instead. Either commit to using `HermesError` at API boundaries or remove it to reduce confusion.

2. **No async test support**
   - The `ironhermes-agent`, `ironhermes-gateway`, and `ironhermes-tools` crates are heavily async but have zero async tests. Adding `tokio` test feature and `#[tokio::test]` is needed before meaningful test coverage can be written.

3. **`ironhermes-state` has no tests despite complex migration logic**
   - Files: `crates/ironhermes-state/src/lib.rs` (lines 249-281)
   - The `run_migrations` method silently ignores `ALTER TABLE` failures via `let _ =`. This is intentional (column may already exist) but fragile without tests proving the upgrade path works.

4. **Tool argument validation is repetitive**
   - Files: `crates/ironhermes-tools/src/file_tools.rs`, `crates/ironhermes-tools/src/terminal.rs`, `crates/ironhermes-tools/src/web_search.rs`
   - Every tool manually extracts and validates JSON parameters with repeated `.ok_or_else(|| anyhow::anyhow!("Missing required parameter: ..."))` patterns. A helper macro or extraction utility would reduce boilerplate.

5. **Streaming SSE parser is hand-rolled**
   - Files: `crates/ironhermes-agent/src/client.rs` (lines 144-211)
   - The SSE line parser in `chat_completion_stream` handles the common case but could fail on edge cases (multi-line data fields, retry fields). Consider using an SSE parsing crate.

6. **`assemble_tool_calls_from_stream` lacks tests**
   - Files: `crates/ironhermes-agent/src/client.rs` (lines 226-270)
   - This function has non-trivial logic (accumulating partial deltas by index) with no test coverage. Bugs here would cause silent tool call failures.

7. **Gateway `SessionStore` is in-memory only**
   - Files: `crates/ironhermes-gateway/src/session.rs`
   - Sessions are lost on restart. The `ironhermes-state` crate exists for persistence but is not wired into the gateway.

8. **No CI pipeline detected**
   - No `.github/workflows/`, `Makefile`, `justfile`, or similar. `cargo test`, `cargo clippy`, and `cargo fmt --check` should be automated.

9. **`Config` silently falls back to defaults**
   - Files: `crates/ironhermes-core/src/config.rs` (line 184), `crates/ironhermes-cli/src/main.rs` (line 107, 359)
   - `Config::load().unwrap_or_default()` means malformed YAML is silently ignored. The CLI `build_client` does this, so a user with a typo in config gets unexpected defaults with no warning.

10. **Context length hardcoded in CLI**
    - Files: `crates/ironhermes-cli/src/main.rs` (lines 217, 339)
    - `with_compression(128_000, ...)` ignores `config.model.context_length`. Should use `config.model.context_length.unwrap_or(DEFAULT_CONTEXT_LENGTH)`.

---

*Quality analysis: 2026-04-01*
