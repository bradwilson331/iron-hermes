# Phase 1: Hygiene - Research

**Researched:** 2026-05-02
**Domain:** Rust/Cargo project configuration, Dioxus 0.7 CLI tooling, Tailwind v4 integration, module structure
**Confidence:** HIGH

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| HYG-01 | `Cargo.toml` declares `dioxus = { version = "=0.7.1", features = ["web", "desktop", "mobile"] }` with package-level feature indirection cleaned up | Dioxus 0.7 docs confirm feature indirection pattern; crates.io confirms 0.7.1 is not yanked |
| HYG-02 | `Cargo.lock` is generated and committed | Cargo official docs: commit Cargo.lock for binary crates; currently missing from repo |
| HYG-03 | `Dioxus.toml` configures `tailwind_input` and `tailwind_output` under `[application]`; root `tailwind.css` is single source of truth | Verified via Dioxus CLI schema.json: both keys live under `[application]` |
| HYG-04 | `src/` split into module hierarchy: `main.rs`, `app.rs`, `components/mod.rs`, `state.rs`, `platform/mod.rs` | Dioxus official tutorial shows `components/mod.rs` pattern; platform gating via Cargo features is standard |
| HYG-05 | `.gitignore` excludes `**/.DS_Store` recursively and `warp2ironhermes-handoff.zip` | Git glob semantics: `**/.DS_Store` is the correct recursive pattern; current `.gitignore` only has `.DS_Store` (root-level only) |
</phase_requirements>

---

## Summary

Phase 1 is a pure configuration and scaffolding phase with no new UI or logic. Every change is a file edit or file creation — no algorithm design, no component logic, no reactive state. The five requirements map to four distinct work areas: (1) fix `Cargo.toml` dependency declaration, (2) generate and commit `Cargo.lock`, (3) add Tailwind keys to `Dioxus.toml`, and (4) split `src/main.rs` into a module tree. A fifth sub-task is a one-line `.gitignore` fix.

The largest risk in this phase is the Dioxus feature model. Dioxus 0.7 treats `web`, `desktop`, and `mobile` as mutually exclusive at **build time** (the `dx` CLI selects which feature to enable per invocation), but they can all be declared in `Cargo.toml`'s `[features]` table simultaneously without conflict — the feature indirection pattern (`web = ["dioxus/web"]`) is explicitly shown in official docs and is the correct approach. The current `Cargo.toml` is almost correct but declares `features = []` on the dioxus dependency itself rather than using the indirection table; HYG-01 requires cleaning this up.

The installed `dx` CLI is version 0.7.3 while the project pins `dioxus = "=0.7.1"`. This is normal and expected — `dx` is a build tool, not a library dependency. The CLI is forward-compatible with older library versions. No action needed on the CLI version.

**Primary recommendation:** Execute all five requirements as sequential file edits in a single commit wave. No external tooling beyond the existing Rust toolchain and `dx` CLI is required. The module split (HYG-04) is the only task with non-trivial Rust surface area — moving `App` and `Hero` out of `main.rs` requires correct `pub use` re-exports and `mod` declarations.

---

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Cargo dependency management | Build system | — | `Cargo.toml` / `Cargo.lock` are build artifacts, not runtime |
| Tailwind CSS compilation | Build tool (`dx` CLI) | — | `dx serve` invokes Tailwind CLI automatically when `tailwind_input` is set |
| Module organization | Rust source tree | — | Rust's `mod` system; no runtime component |
| Asset pipeline | Dioxus `asset!()` macro | Build tool | Compile-time path resolution, served by `dx` |
| `.gitignore` hygiene | VCS config | — | Pure git configuration |

---

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| dioxus | =0.7.1 (pinned) | UI framework, reactivity, asset pipeline | Project requirement; 0.7.1 is not yanked [VERIFIED: crates.io] |
| dx CLI | 0.7.3 (installed) | Build, serve, hot-reload, Tailwind watcher | Official Dioxus CLI; forward-compatible with 0.7.1 lib [VERIFIED: dx --version] |
| Tailwind CSS v4 | managed by dx | Utility CSS (available but not primary styling strategy) | dx auto-manages when `tailwind.css` present [CITED: dioxuslabs.com/learn/0.7] |

### No Additional Dependencies Needed
Phase 1 requires zero new crate additions. All work is configuration and file organization.

**Version verification:**
- `dioxus 0.7.1`: yanked=False [VERIFIED: crates.io API]
- `dioxus 0.7.7`: latest available — project intentionally pins to 0.7.1
- `dx 0.7.3`: installed and working [VERIFIED: `dx --version`]

---

## Architecture Patterns

### System Architecture Diagram

```
Cargo.toml (feature flags)
    │
    ├── [features] web    → dioxus/web   ─┐
    ├── [features] desktop → dioxus/desktop ├─ dx CLI selects ONE at build time
    └── [features] mobile  → dioxus/mobile ─┘
                                           │
                                    dx serve --platform web
                                           │
                              Dioxus.toml [application]
                              tailwind_input  → tailwind.css (project root)
                              tailwind_output → assets/tailwind.css
                                           │
                                    Tailwind CLI (managed by dx)
                                           │
                              assets/tailwind.css (compiled output)
```

### Recommended Project Structure (post-HYG-04)

```
src/
├── main.rs          # fn main() { dioxus::launch(App); } + asset consts
├── app.rs           # #[component] fn App() — root, injects CSS links
├── components/
│   └── mod.rs       # pub mod declarations + pub use re-exports
└── state.rs         # (empty stub) — future Signal<AppState>
platform/            # NOT under src/ — gated via #[cfg(feature)]
```

Wait — the requirements spec says `src/platform/`. Platform-gated code should live under `src/platform/` (not a top-level `platform/`), consistent with the Rust module system. The `platform/mod.rs` stub is empty for Phase 1 but establishes the module boundary.

Final target structure for HYG-04:
```
src/
├── main.rs           # entry point, asset consts, mod declarations
├── app.rs            # App component
├── components/
│   └── mod.rs        # re-exports; Hero moves here
├── state.rs          # empty stub
└── platform/
    └── mod.rs        # empty stub; future cfg(feature) gating goes here
```

### Pattern 1: Feature Indirection (HYG-01)

**What:** Declare platform features in `[features]` table that activate dioxus sub-features. The `dioxus` dependency itself has `features = []` — the dx CLI injects the right feature flag at build time.

**When to use:** Every Dioxus multi-platform project.

**Correct form:**
```toml
# Source: https://dioxuslabs.com/learn/0.7/tutorial/new_app
[dependencies]
dioxus = { version = "=0.7.1", features = [] }

[features]
default = ["web"]
web = ["dioxus/web"]
desktop = ["dioxus/desktop"]
mobile = ["dioxus/mobile"]
```

**Current (broken) form in this project:**
```toml
dioxus = { version = "0.7.1", features = [] }   # missing pin; features=[] is OK
[features]
default = ["web"]
web = ["dioxus/web"]
desktop = ["dioxus/desktop"]
mobile = ["dioxus/mobile"]
```

The current form is actually mostly correct — the `features = []` on the dependency is fine, the dx CLI enables the right sub-feature. The main fix is adding the `=` version pin per HYG-01.

### Pattern 2: Dioxus.toml Tailwind Configuration (HYG-03)

**What:** `tailwind_input` and `tailwind_output` keys under `[application]` section tell the `dx` CLI where to find the Tailwind source and where to write the compiled output.

**Correct form:**
```toml
# Source: Dioxus CLI schema.json (packages/cli/schema.json)
[application]
tailwind_input = "tailwind.css"
tailwind_output = "assets/tailwind.css"
```

When these keys are set, `dx serve` automatically:
1. Detects that a `tailwind.css` source file exists at the configured path
2. Runs the Tailwind CLI watcher in the background
3. Writes compiled CSS to `assets/tailwind.css`
4. Hot-reloads the browser when CSS changes

**If keys are absent:** `dx` still auto-detects `tailwind.css` in the project root as the default, but setting explicit keys is required per HYG-03.

### Pattern 3: Module Split (HYG-04)

**What:** Rust's standard `mod`/`pub use` pattern for splitting a monolithic `main.rs` into focused modules.

**When to use:** When a single file exceeds a single responsibility — here, `main.rs` currently contains entry point, root component, and a component (`Hero`) that will be replaced entirely.

**Example — src/main.rs after split:**
```rust
// Source: https://dioxuslabs.com/learn/0.7/tutorial/routing
use dioxus::prelude::*;

mod app;
mod components;
mod state;
mod platform;

use app::App;

const FAVICON: Asset = asset!("/assets/favicon.ico");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

fn main() {
    dioxus::launch(App);
}
```

**Example — src/components/mod.rs:**
```rust
// Source: https://dioxuslabs.com/learn/0.7/tutorial/routing
mod hero;

pub use hero::*;
```

### Anti-Patterns to Avoid

- **Putting `features = ["web", "desktop", "mobile"]` directly on the dioxus dependency:** This would attempt to enable all three platform renderers simultaneously in a single compilation, which the linker will reject. The feature indirection table is essential. [CITED: dioxuslabs.com/learn/0.7]
- **Using `mod.rs` vs named files:** Both are valid Rust; the project should use `mod.rs` for directories (e.g., `components/mod.rs`) per the existing codebase convention shown in the official Dioxus tutorial.
- **Forgetting `pub use` in mod.rs:** Components in sub-files are private by default. Use `pub use module::*` or explicit `pub use` to make them visible to parent modules.
- **Leaving `Cargo.lock` in `.gitignore`:** The current `.gitignore` does not explicitly exclude `Cargo.lock`, but it is also not committed (absent from the tree). For a binary crate, Cargo's official stance is to commit it. [CITED: doc.rust-lang.org/cargo/faq]

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Tailwind CSS compilation | Custom build script or `build.rs` | `dx serve` with `tailwind_input`/`tailwind_output` in `Dioxus.toml` | dx manages Tailwind CLI lifecycle automatically |
| Feature-gated compilation | `#[cfg(all(feature="web", feature="desktop"))]` guards everywhere | Cargo feature indirection + dx platform selection | dx selects exactly one platform feature at build time |
| Asset path resolution | Hardcoded strings | `asset!("/assets/...")` macro | Compile-time path verification + Dioxus asset pipeline hashing |

---

## Common Pitfalls

### Pitfall 1: Version Pin Syntax

**What goes wrong:** Writing `version = "0.7.1"` allows Cargo to upgrade to `0.7.x` on next `cargo update`. Writing `version = "=0.7.1"` pins exactly to that patch.

**Why it happens:** Cargo SemVer by default allows `^0.7.1` (compatible upgrades). The `=` prefix disables this.

**How to avoid:** Use `dioxus = { version = "=0.7.1", ... }` exactly as specified in HYG-01.

**Warning signs:** `Cargo.lock` shows `dioxus 0.7.2` or higher despite specifying `0.7.1` in `Cargo.toml`.

### Pitfall 2: Tailwind_input Key Location

**What goes wrong:** Placing `tailwind_input` under `[web.app]` or `[web.resource]` instead of `[application]`.

**Why it happens:** The Dioxus.toml schema is not well-documented in the narrative docs; most examples only show `[web.resource] style = []` for adding pre-compiled CSS, not for the tailwind watcher config.

**How to avoid:** Both keys must be under `[application]`. The `dx` CLI reads them from `ApplicationConfig`. [VERIFIED: Dioxus CLI schema.json]

**Warning signs:** `dx serve` does not regenerate `assets/tailwind.css` when `tailwind.css` is edited.

### Pitfall 3: DS_Store Pattern Scope

**What goes wrong:** Using `.DS_Store` (no leading `**/`) in `.gitignore` only blocks the root-level file, not nested ones in `assets/`, `src/`, etc.

**Why it happens:** Git glob patterns without `**/` prefix are anchored to the `.gitignore` location.

**How to avoid:** Use `**/.DS_Store` for recursive matching. The current `.gitignore` has `.DS_Store` (without `**/`) — this is the bug to fix. [ASSUMED: git glob semantics from training knowledge; standard macOS gitignore practice]

**Warning signs:** `git status` shows `.DS_Store` files in subdirectories.

### Pitfall 4: Module Visibility After Split

**What goes wrong:** After splitting `Hero` to `src/components/hero.rs`, the component is not accessible from `app.rs` because `mod hero` is private inside `components/mod.rs`.

**Why it happens:** Rust modules are private by default.

**How to avoid:** In `components/mod.rs` use `pub mod hero;` and `pub use hero::Hero;`. In `app.rs` use `use crate::components::Hero;`.

**Warning signs:** Compile error: `error[E0603]: function `Hero` is private`.

### Pitfall 5: dx CLI Version vs Library Pin

**What goes wrong:** Assuming the installed `dx` CLI version must match the `dioxus` library version.

**Why it happens:** Both are versioned `0.7.x` which implies tight coupling.

**How to avoid:** The `dx` CLI (build tool) and `dioxus` crate (library) are independently versioned. `dx 0.7.3` is fully capable of building a project that depends on `dioxus = "=0.7.1"`. No special action required. [VERIFIED: dx --version shows 0.7.3; dioxus 0.7.1 is not yanked]

---

## Code Examples

### HYG-01: Correct Cargo.toml

```toml
# Source: https://dioxuslabs.com/learn/0.7/tutorial/new_app
[dependencies]
dioxus = { version = "=0.7.1", features = [] }

[features]
default = ["web"]
web = ["dioxus/web"]
desktop = ["dioxus/desktop"]
mobile = ["dioxus/mobile"]
```

### HYG-03: Correct Dioxus.toml

```toml
# Source: Dioxus CLI schema.json [application] section
[application]
tailwind_input = "tailwind.css"
tailwind_output = "assets/tailwind.css"

[web.app]
title = "iron_hermes_ui"

[web.resource]
style = []
script = []

[web.resource.dev]
script = []
```

### HYG-04: src/main.rs after split

```rust
// Source pattern: https://dioxuslabs.com/learn/0.7/tutorial/routing
// Asset constants live in the modules that consume them (see PATTERNS.md):
//   - FAVICON / MAIN_CSS / TAILWIND_CSS → src/app.rs
//   - HEADER_SVG → src/components/hero.rs
use dioxus::prelude::*;

mod app;
mod components;
mod state;
mod platform;

use app::App;

fn main() {
    dioxus::launch(App);
}
```

### HYG-04: src/app.rs

```rust
use dioxus::prelude::*;
use crate::components::Hero;

// Asset constants are declared in main.rs and passed or re-declared as needed.
// For CSS injection, App needs access to the asset handles.
// Re-declare or import from main module.
const FAVICON: Asset = asset!("/assets/favicon.ico");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
pub fn App() -> Element {
    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        Hero {}
    }
}
```

Note: `asset!()` macro can be called in any module — it resolves paths relative to the project root at compile time. Each module that needs an asset constant should declare it locally.

### HYG-04: src/components/mod.rs

```rust
// Source pattern: https://dioxuslabs.com/learn/0.7/tutorial/routing
mod hero;

pub use hero::Hero;
```

### HYG-05: .gitignore additions

```gitignore
# Add to existing .gitignore
**/.DS_Store
warp2ironhermes-handoff.zip
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Manual Tailwind CLI watch in separate terminal | `dx serve` auto-manages Tailwind watcher | Dioxus 0.7 | Single command to serve + compile Tailwind |
| `cx: Scope` component parameter | `#[component]` fn with no scope | Dioxus 0.6 → 0.7 | Existing code already uses 0.7 style |
| `dioxus/features = ["web"]` hardcoded on dep | Feature indirection table in `[features]` | Dioxus 0.5+ | `dx` selects platform; code uses `#[cfg(feature="...")]` |

**Deprecated/outdated:**
- `cx`, `Scope`, `use_state`: Removed in Dioxus 0.7 — project already avoids these
- `tailwind.css` in `[web.resource] style = []`: This includes a pre-compiled file in the HTML, not the same as triggering the Tailwind watcher. The watcher is triggered by `tailwind_input` in `[application]`.

---

## Runtime State Inventory

> This is a greenfield scaffold phase (no rename/refactor). Omit.

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `**/.DS_Store` is the correct recursive gitignore pattern vs `.DS_Store` | Common Pitfalls, Code Examples | If wrong, DS_Store files in subdirs still leak into git. Easily verified: `git check-ignore -v some/subdir/.DS_Store` |
| A2 | `dx 0.7.3` is fully forward-compatible with `dioxus = "=0.7.1"` (no breaking CLI↔lib contract) | Standard Stack, Pitfall 5 | If wrong, `dx serve` may fail with ABI errors; mitigation: install `dx 0.7.1` via `cargo install dioxus-cli --version 0.7.1 --locked` |
| A3 | Asset constants (`asset!()`) can be declared in any module, not just `main.rs` | Code Examples | If wrong, asset constants in `app.rs` will fail to compile; mitigation: declare all constants in `main.rs` and pass via props or use `pub const` |

**If this table is empty:** Not empty — three low-risk assumptions flagged above.

---

## Open Questions (RESOLVED)

1. **Where exactly to declare `Asset` constants after the module split?**
   - What we know: `asset!()` macro works anywhere in the codebase based on Dioxus docs
   - What's unclear: Whether there is a convention preference (all in `main.rs` vs in the module that uses them)
   - **RESOLVED:** Declare each constant in the module closest to its use. `FAVICON`, `MAIN_CSS`, and `TAILWIND_CSS` move into `src/app.rs`; `HEADER_SVG` moves into `src/components/hero.rs`. `src/main.rs` keeps no asset constants. (Captured in PATTERNS.md and 01-02-PLAN.md.)

2. **Should `state.rs` and `platform/mod.rs` be empty stubs or have placeholder content?**
   - What we know: HYG-04 requires these files exist; they have no content in Phase 1
   - What's unclear: Whether Rust will warn on empty modules
   - **RESOLVED:** Add a single `// Phase placeholder — implementation begins in Phase N` comment line to each. Suppresses dead-code warnings and documents intent. (Captured in PATTERNS.md and 01-02-PLAN.md.)

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain | All Cargo operations | ✓ | rustc 1.94.0 | — |
| Cargo | Dependency mgmt, Cargo.lock | ✓ | 1.94.0 | — |
| dx CLI | `dx serve`, Tailwind watcher | ✓ | 0.7.3 | — |
| Node.js | Tailwind CLI (dx bundles it) | ✓ | 24.14.0 | dx manages tailwind internally |
| tailwindcss CLI | CSS compilation | managed by dx | — | dx auto-downloads |
| git | HYG-02, HYG-05 | ✓ (repo exists) | — | — |

**Missing dependencies with no fallback:** None.

**Note:** `tailwindcss` is not in PATH directly, but `dx serve` manages the Tailwind CLI automatically when `tailwind_input` is configured. No separate Tailwind installation is required.

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | None configured (acknowledged in REQUIREMENTS.md as out-of-scope for v1) |
| Config file | none |
| Quick run command | `cargo build --features web` |
| Full suite command | `cargo build --features web && cargo build --features desktop` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| HYG-01 | Cargo.toml has pinned dioxus dep with correct features | smoke | `grep '=0.7.1' Cargo.toml` | ✅ Cargo.toml |
| HYG-02 | Cargo.lock committed and present | smoke | `git show HEAD:Cargo.lock` | ❌ Wave 0: generate via `cargo build` |
| HYG-03 | Dioxus.toml has tailwind_input/output | smoke | `grep 'tailwind_input' Dioxus.toml` | ✅ Dioxus.toml |
| HYG-04 | src/ module tree compiles | compile | `cargo build --features web` | ❌ Wave 0: create files |
| HYG-05 | .gitignore has correct DS_Store pattern | smoke | `git check-ignore -v .DS_Store` | ✅ .gitignore |

### Sampling Rate
- **Per task commit:** `cargo build --features web`
- **Per wave merge:** `cargo build --features web && cargo build --features desktop`
- **Phase gate:** All three platform builds green + `Cargo.lock` committed before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `src/app.rs` — covers HYG-04 (App component)
- [ ] `src/components/mod.rs` — covers HYG-04 (components module)
- [ ] `src/components/hero.rs` — covers HYG-04 (Hero component, moved from main.rs)
- [ ] `src/state.rs` — covers HYG-04 (state stub)
- [ ] `src/platform/mod.rs` — covers HYG-04 (platform stub)
- [ ] `Cargo.lock` — generated by `cargo build`, then `git add Cargo.lock`

---

## Security Domain

Phase 1 contains no network calls, no user input handling, no secrets, no authentication, no cryptography, and no server endpoints. ASVS categories V2–V6 do not apply. This phase is pure build configuration and file structure — there is no security attack surface introduced.

---

## Sources

### Primary (HIGH confidence)
- [/websites/dioxuslabs_learn via Context7] — Cargo.toml feature indirection pattern, module structure, tailwind hot-reload behavior
- [dioxuslabs.com/learn/0.7/tutorial/new_app] — Canonical project structure and feature declaration
- [dioxuslabs.com/learn/0.7/tutorial/routing] — Module split pattern with `components/mod.rs`
- [dioxuslabs.com/learn/0.7/essentials/ui/hotreload] — `tailwind_input`/`tailwind_output` key names confirmed
- [Dioxus CLI schema.json] — `tailwind_input` and `tailwind_output` are in `[application]` section
- [crates.io API: dioxus/0.7.1] — Version 0.7.1 is not yanked, confirmed available

### Secondary (MEDIUM confidence)
- [doc.rust-lang.org/cargo/faq — Cargo.lock in version control] — Guidance to commit Cargo.lock for reproducible builds
- [dx --version output] — dx 0.7.3 installed and working

### Tertiary (LOW confidence / ASSUMED)
- Git glob pattern `**/.DS_Store` semantics — based on training knowledge of git gitignore spec; standard macOS practice; not verified against git documentation in this session

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — crates.io + Context7 verified
- Architecture: HIGH — official Dioxus 0.7 docs via Context7
- Tailwind Dioxus.toml keys: HIGH — confirmed via CLI schema.json + hotreload docs
- Pitfalls: HIGH for Cargo/Dioxus-specific; MEDIUM for git DS_Store pattern (assumption)
- Module split: HIGH — direct from official Dioxus tutorial

**Research date:** 2026-05-02
**Valid until:** 2026-06-01 (stable domain; Dioxus 0.7.x patch releases unlikely to change config schema)
