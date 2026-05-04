# Testing Patterns

**Analysis Date:** 2026-05-02

## Test Framework

**Runner:**
- None configured — no test infrastructure present in `Cargo.toml`
- Rust's built-in `cargo test` is available but no test modules exist in `src/`

**Assertion Library:**
- Rust standard `assert!`, `assert_eq!`, `assert_ne!` macros (built-in, no dependency needed)

**Run Commands:**
```bash
cargo test              # Run all tests
cargo test -- --nocapture  # Run tests with stdout output
cargo test <name>       # Run a specific test by name
```

## Test File Organization

**Location:**
- No tests exist yet
- Rust convention: unit tests in `#[cfg(test)]` modules at the bottom of the file being tested
- Integration tests go in a top-level `tests/` directory (not yet created)

**Naming:**
- Test modules: `mod tests { ... }` inside `#[cfg(test)]`
- Test functions: `fn test_<thing_being_tested>()` with `#[test]` attribute

**Structure:**
```
src/
├── main.rs          # #[cfg(test)] mod tests at bottom (when added)
tests/               # Integration tests (not yet created)
```

## Test Structure

**Suite Organization:**
```rust
// Pattern to follow when adding tests to src/main.rs or any module:
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_something() {
        // arrange
        // act
        // assert
    }
}
```

**Patterns:**
- Setup: inline or via helper functions within the test module
- Teardown: Rust's ownership model handles cleanup automatically
- Assertion: `assert_eq!(actual, expected)` with descriptive failure messages

## Mocking

**Framework:** None configured

**Patterns:**
- No mocking infrastructure present
- For Dioxus component testing, the `dioxus-testing` crate (when added) provides component test utilities
- For server function testing, use dependency injection patterns to swap implementations

**What to Mock:**
- HTTP clients / external API calls in server functions
- Browser APIs (localStorage, etc.) when testing fullstack code

**What NOT to Mock:**
- Signal state — test components by rendering them with `dioxus-testing`
- Pure functions — test directly with real inputs

## Fixtures and Factories

**Test Data:**
```rust
// No fixtures exist yet. When adding, use helper functions:
fn make_test_component_props() -> MyProps {
    MyProps {
        value: "test".to_string(),
    }
}
```

**Location:**
- Shared test helpers: `tests/helpers/mod.rs` (to be created)
- Module-level helpers: inside `#[cfg(test)] mod tests` block in the relevant source file

## Coverage

**Requirements:** None enforced — no coverage tooling configured

**View Coverage:**
```bash
# Install tarpaulin for coverage (when needed):
cargo install cargo-tarpaulin
cargo tarpaulin --out Html
```

## Test Types

**Unit Tests:**
- Scope: Pure logic functions, data transformations, route parsing
- Approach: `#[cfg(test)]` blocks within each source file
- Not applicable for components until `dioxus-testing` is added

**Integration Tests:**
- Scope: Full request/response cycles for server functions (when fullstack features are added)
- Approach: `tests/` directory with `#[tokio::test]` for async tests
- Not yet created

**E2E Tests:**
- Not configured
- Playwright or Selenium could be added for browser-level testing of the web target

## Common Patterns

**Async Testing:**
```rust
// When async tests are needed (e.g., testing use_resource or server functions):
#[tokio::test]
async fn test_async_operation() {
    let result = some_async_fn().await;
    assert_eq!(result, expected);
}
// Requires: tokio = { version = "1", features = ["rt", "macros"] } in [dev-dependencies]
```

**Error Testing:**
```rust
#[test]
fn test_error_case() {
    let result = function_that_may_fail();
    assert!(result.is_err());
    // Or for specific error messages:
    assert_eq!(result.unwrap_err().to_string(), "expected error message");
}
```

## Dioxus-Specific Testing Notes

- Dioxus 0.7 component testing requires the `dioxus-testing` crate (not yet in `Cargo.toml`)
- Add to `[dev-dependencies]` when component tests are needed:
  ```toml
  [dev-dependencies]
  dioxus-testing = "0.7"
  ```
- Do NOT use `cx`, `Scope`, or `use_state` in any test code — Dioxus 0.7 APIs only
- Signal reads/writes in tests must not be held across `.await` points (same clippy rule as production code — see `clippy.toml`)

## Current State

No tests exist in this codebase. The project is a scaffold (`src/main.rs` only). Testing infrastructure should be established before feature development begins. Priority additions:

1. Add `[dev-dependencies]` section to `Cargo.toml` with `dioxus-testing` when component tests are needed
2. Add `tokio` dev-dependency when async server function tests are needed
3. Create `tests/` directory for integration tests when server functions are added

---

*Testing analysis: 2026-05-02*
