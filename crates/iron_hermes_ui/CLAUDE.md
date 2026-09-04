<!-- GSD:project-start source:PROJECT.md -->
## Project

**IronHermes web UI (`iron_hermes_ui`)**

A pixel-perfect Dioxus 0.7 port of the Warp × IronHermes design prototype — a terminal-style shell with a block-based command stream, agent side panel, command palette, and an ANSI-derived design system. It also embeds the IronHermes agent server (see `src/server/`), so this crate is both the UI and the web backend.

**Core value:** visually indistinguishable from the React prototype in `warp2ironhermes/` — every block, panel, palette, scanner tick, and theme variant matches when rendered side by side.

### Constraints

- **Dioxus 0.7 only** — `cx`, `Scope`, and `use_state` are Dioxus 0.6 APIs, removed in 0.7, and will not compile. Use `use_signal`, `use_memo`, `use_resource`, `use_context_provider` / `use_context`. Components are `PascalCase` + `#[component]`.
- **Signal borrows must not span `.await`** — holding `GenerationalRef`, `GenerationalRefMut`, or `dioxus_signals::WriteLock` across an await point panics at runtime. Enforced by `crates/iron_hermes_ui/clippy.toml`.
- **`.peek()` does not subscribe** — use `.read()` in render (so `.set()` re-renders) and reserve `.peek()` for effects and closures.
- **`use_context_provider` must live at the `HermesApp` root** — providing a `*Ctx` in a child compiles fine and panics consumers at runtime.
- **Native builds are blind to wasm code** — `cargo build`/`clippy` never type-check `cfg(target_arch = "wasm32")`. Gate web work on
  `RUSTFLAGS='--cfg getrandom_backend="wasm_js"' cargo check --target wasm32-unknown-unknown -p iron_hermes_ui`.
- **This crate is excluded from workspace `default-members`** — every `dx` invocation must pass `--package iron_hermes_ui` and run from the workspace root. Bare `dx serve` resolves the wrong package; `cd`-ing into the crate directory panics in Dioxus's `find_main_package`.
- **Design fidelity is the primary failure mode.** Visual drift matters more than code elegance here.
- **CSS is ported as-is** — `warp-ih.css` and `colors_and_type.css`, no Tailwind conversion. These define `--w-bg-*` / `--accent-primary`; they must load unconditionally, never gated behind a feature.
- **`warp2ironhermes/` is read-only reference** — consulted, never compiled. Do not import from it or include it in any build asset path.
<!-- GSD:project-end -->

<!-- GSD:stack-start source:codebase/STACK.md -->
## Technology Stack

Derived from `Cargo.toml` and `Dioxus.toml` — read those rather than a copy that drifts.

Build, test, lint, and the wasm/`dx` gates are documented once in `docs/DEVELOPMENT.md`; deployment in `docs/DEPLOYMENT.md`.
<!-- GSD:stack-end -->

<!-- GSD:conventions-start source:CONVENTIONS.md -->
## Conventions

Standard Rust conventions apply (`rustfmt`, `snake_case`, `PascalCase` types). The Dioxus-specific rules that are NOT guessable are in the Constraints section above; the shared code-style rules live in `docs/DEVELOPMENT.md` § Code Style.

RSX specifics worth knowing:
- Prefer `for` loops in RSX over `.map()` chains; wrap iterator expressions in braces: `{(0..5).map(|i| rsx! { ... })}`.
- Conditional attributes: `attr: if condition { "value" }`.
- Asset constants at module top: `const NAME: Asset = asset!("/assets/file.ext")`; paths start with `/assets/`.
- Server functions return `Result<T, ServerFnError>`. Server fns taking arguments must be `#[server]`, not `#[get]`.
<!-- GSD:conventions-end -->

<!-- GSD:architecture-start source:ARCHITECTURE.md -->
## Architecture

Read `src/` directly — it is a full module tree (`app.rs`, `components/`, `server/`, `kanban/`, `platform/`, `mocks/`), not a single file. `HermesApp` (in `components/hermes_app/mod.rs`) is the crate's only root component — there is no compile-time branch selecting a different one.

### Design vocabulary (the prototype contract — not derivable from code)

- Split layout: scrollable terminal stream + agent side panel (right, 360px).
- Block types: `is-cmd` (user command), `is-out` (output), `is-ai` (Hermes reply), `is-ok` (success), `is-err` (error); each carries a 2px left accent stripe color-coded by type.
- Mode toggle: Shell (`❯`) vs Agent (`✦`), switched with `⌥+M`. Command palette on `⌘K`.
- Scanner: knight-rider 10-cell animation, 100ms tick, triangle-wave bounce, `░` `▒` `▓` `█`, auto-deactivates after ~1400ms.
- Colors are ANSI-palette-derived: `--accent-primary` cyan `#4ec9b0`, `--accent-secondary` magenta `#c678dd`, `--success` `#3fb950`, `--warn` `#d29922`, `--danger` `#f85149`, `--brand` `#f0883e`.
- Font: `"Ioskeley Mono"` → `"Berkeley Mono"` → `ui-monospace`. Everything is monospace (`--font-body: var(--font-mono)`).
- Zero border-radius on base elements (`--radius-0`); Warp blocks use `var(--w-radius-block): 6px`.
- Data attributes: `data-theme` `cyan|magenta|green|amber` · `data-density` `comfy|compact` · `data-block` `framed|flat|minimal` · `data-agent` `right|bottom|hidden`.

### Constraints

- Single-threaded WASM event loop in web mode — no `std::thread`.
- Web/desktop/mobile targets are mutually exclusive via Cargo features.
<!-- GSD:architecture-end -->

<!-- GSD:skills-start source:skills/ -->
## Project Skills

No project skills found. Add skills to any of: `.claude/skills/`, `.agents/skills/`, `.cursor/skills/`, `.github/skills/`, or `.codex/skills/` with a `SKILL.md` index file.
<!-- GSD:skills-end -->

<!-- GSD:workflow-start source:GSD defaults -->
## GSD Workflow Enforcement

Before using Edit, Write, or other file-changing tools, start work through a GSD command so planning artifacts and execution context stay in sync.

Use these entry points:
- `/gsd-quick` for small fixes, doc updates, and ad-hoc tasks
- `/gsd-debug` for investigation and bug fixing
- `/gsd-execute-phase` for planned phase work

Do not make direct repo edits outside a GSD workflow unless the user explicitly asks to bypass it.
<!-- GSD:workflow-end -->



<!-- GSD:profile-start -->
## Developer Profile

> Profile not yet configured. Run `/gsd-profile-user` to generate your developer profile.
> This section is managed by `generate-claude-profile` -- do not edit manually.
<!-- GSD:profile-end -->
