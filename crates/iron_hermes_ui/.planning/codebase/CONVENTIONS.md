# Coding Conventions

**Analysis Date:** 2026-05-02

## Naming Patterns

**Files:**
- Snake_case for Rust source files: `main.rs`
- Modules named after their primary responsibility
- Asset constants in SCREAMING_SNAKE_CASE at module top: `const FAVICON: Asset = asset!(...)`

**Functions:**
- `snake_case` for regular functions per Rust standard: `fn main()`
- Component functions use `PascalCase` (required by Dioxus `#[component]` macro): `fn App()`, `fn Hero()`
- Event handler closures use `move |event_name|` pattern: `move |e| { ... }`

**Variables:**
- `snake_case` for all local bindings: `let mut count = use_signal(|| 0)`
- Signal variables named after the data they hold: `value`, `count`, `theme`
- Asset constants declared at module scope with `const`: `const MAIN_CSS: Asset = asset!(...)`

**Types:**
- `PascalCase` for structs, enums, and type aliases per Rust standard
- Route enums derive `Routable, Clone, PartialEq`
- Props structs implied by `#[component]` function arguments — no explicit struct needed

## Code Style

**Formatting:**
- `rustfmt` (standard Rust formatter via `cargo fmt`)
- 4-space indentation in Rust source
- RSX macro blocks indented consistently within `rsx! { ... }`

**Linting:**
- `clippy` with custom `clippy.toml` at project root
- Key enforced rule: do NOT hold `GenerationalRef`, `GenerationalRefMut`, or `dioxus_signals::WriteLock` across `.await` points — this causes borrow panics at runtime
- Config: `/Users/twilson/code/iron_hermes_ui/clippy.toml`

## Import Organization

**Order:**
1. `use dioxus::prelude::*;` — always first, brings all Dioxus primitives into scope
2. Standard library imports (`use std::...`)
3. Third-party crate imports
4. Local module imports (`use crate::...`)

**Path Aliases:**
- None configured — imports use full crate paths

**Glob imports:**
- `use dioxus::prelude::*` is standard and expected — do not enumerate individual Dioxus items

## Error Handling

**Patterns:**
- No error handling crates detected (`anyhow`, `thiserror` not present in `Cargo.toml`)
- Server functions (when added) must return `Result<T, ServerFnError>` — this is Dioxus fullstack's required error type
- Component functions return `Element` — panics are the failure mode for component logic errors
- Async resources via `use_resource` return `Option<T>` — `None` represents loading state, errors are surfaced as `None` or via `use_resource`'s error handling

**Do not use:**
- `.unwrap()` in components where avoidable — prefer `match` or `if let` on `Option`/`Result`
- Holding signal read/write locks (`.read()` / `.write()`) across `.await` points — clippy enforces this

## Logging

**Framework:** Not configured (no `log`, `tracing`, or `console_log` crates present)

**Patterns:**
- Use `web_sys::console::log_1` for browser-side debug logging when needed (standard for Dioxus web targets)
- No structured logging infrastructure currently present

## Comments

**When to Comment:**
- Document non-obvious component behavior with `///` doc comments
- AGENTS.md serves as the authoritative Dioxus 0.7 API reference — consult before adding new patterns

**JSDoc/TSDoc:**
- Not applicable (Rust project)
- Use `///` for public API documentation, `//` for inline implementation notes

## Function Design

**Size:** Components should be focused on a single UI concern. Split large RSX trees into sub-components.

**Parameters:**
- Component props are declared as function arguments, not a separate struct
- Props must implement `PartialEq + Clone`
- Use `Signal<T>` for mutable props passed down to children: `fn Input(mut value: Signal<String>) -> Element`
- Use `ReadOnlySignal<T>` for read-only reactive props

**Return Values:**
- All components return `Element`
- All server functions return `Result<T, ServerFnError>`

## Module Design

**Exports:**
- `pub fn` for components intended to be used across modules: `pub fn Hero() -> Element`
- Private components (internal to a module) use `fn` without `pub`

**Barrel Files:**
- Not used in current codebase (single `main.rs` file)
- When modules are added, use `mod.rs` or named module files — re-export public components via `pub use`

## Dioxus 0.7 Specific Conventions

**Components:**
- Always annotate with `#[component]` macro
- Function name must start with capital letter (PascalCase)
- Do NOT use `cx`, `Scope`, or `use_state` — these are Dioxus 0.6 APIs and will not compile

**State:**
- Local state: `use_signal(|| initial_value)`
- Derived state: `use_memo(move || expression)`
- Async state: `use_resource(move || async move { ... })`
- Shared state: `use_context_provider(|| value)` / `use_context::<Type>()`

**RSX:**
- Prefer `for` loops directly in RSX over `.map()` iterator chains when possible
- Wrap iterator expressions in braces: `{(0..5).map(|i| rsx! { ... })}`
- Conditional rendering uses `if condition { ... }` directly in RSX
- Conditional attributes: `attr: if condition { "value" }`

**Assets:**
- Declare asset constants at module top with `const NAME: Asset = asset!("/assets/file.ext")`
- All asset paths start with `/assets/` and are relative to project root
- Inject stylesheets via `document::Link { rel: "stylesheet", href: CONST_NAME }`

---

*Convention analysis: 2026-05-02*
