<!-- generated-by: gsd-doc-writer -->
# Development

## Local Setup

IronHermes is a Cargo workspace written in Rust (Edition 2024). The web UI crate (`iron_hermes_ui`) additionally requires the Dioxus CLI (`dx`).

### Prerequisites

- Rust stable toolchain (managed via `rustup`) — includes `cargo`, `rustfmt`, and `clippy`
- `cargo-nextest` for the workspace test suite (matches CI): `cargo install cargo-nextest --locked`
- `cargo-insta` for snapshot testing: `cargo install cargo-insta --locked`
- Dioxus CLI for the web UI crate: `cargo install dioxus-cli`

### Clone and configure

```bash
git clone <repository-url>
cd ironhermes

# Create the IronHermes home directory and copy configuration templates
mkdir -p ~/.ironhermes
cp env.example ~/.ironhermes/.env          # add at least one LLM provider API key
cp cli-config.yaml.example ~/.ironhermes/config.yaml
```

Edit `~/.ironhermes/.env` and uncomment the API key for your preferred provider (e.g., `OPENROUTER_API_KEY`). Edit `~/.ironhermes/config.yaml` to match — the `providers` block must include a `api_key_env` entry pointing at the env var you set, or the setup wizard will re-launch on every start.

### Build

```bash
# Development build (all default-member crates)
cargo build

# Release build
cargo build --release

# Web UI (requires the Dioxus CLI) — run from the workspace root, not the crate directory.
# `iron_hermes_ui` is excluded from [workspace] default-members, so bare `dx serve`/`dx build`
# from the root resolves to the wrong default package, and `cd`-ing into the crate directory
# panics in `find_main_package` (Dioxus workspace resolution). Always pass -p/--package explicitly:
dx serve --package iron_hermes_ui              # hot-reload dev server at http://localhost:8080

# Standalone binary (no dx-proxy WebSocket idle timeout — recommended over `dx serve` for real use)
dx bundle --platform web -p iron_hermes_ui
RUST_LOG=info ./target/dx/iron_hermes_ui/debug/web/iron_hermes_ui
```

**Auth dev-mode caveat (Phase 47.3):** `dx serve --package iron_hermes_ui` proxies every
request through the Dioxus CLI's own dev server. Run local development with no
`web_ui.auth.password_hash` configured, on the default loopback bind — the D-07 auth
boundary is inert by design in that configuration (identical to pre-47.3 behavior), so
there is nothing for the proxy to interfere with. The wasm type-check gate below is
unaffected either way.

---

## Build Commands

| Command | Description |
|---------|-------------|
| `cargo build` | Compile all default-member workspace crates (debug); `iron_hermes_ui` is excluded and not built |
| `cargo build --release` | Compile optimized release binary (default-members only) |
| `cargo run --bin ironhermes` | Run interactive REPL |
| `cargo run --bin ironhermes -- -e "<prompt>"` | Run a single prompt non-interactively |
| `cargo run --bin ironhermes -- status` | Show agent/session status |
| `cargo run --bin ironhermes -- doctor` | Validate configuration |
| `cargo nextest run --no-fail-fast --workspace --all-features` | Run the full workspace test suite (CI test runner) |
| `cargo test --workspace --all-features --doc` | Run doctests (nextest does not run these; CI runs them as a separate step) |
| `cargo fmt --all` | Format all crates |
| `cargo fmt --all -- --check` | Check formatting without modifying files (CI) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Run linter (CI-strict mode; `--all-features` is required for `iron_hermes_ui` to type-check its `server` feature code) |
| `cargo insta test --unreferenced=reject -p ironhermes-cli --all-features` | Run snapshot tests and reject orphaned `.snap` files (CI scopes this to `ironhermes-cli`, where all committed snapshots live) |
| `cargo audit` | Check `Cargo.lock` against the RustSec advisory database (CI supply-chain gate) |
| `bash scripts/ci-gates.sh` | Run Phase 21.7 static-analysis + cargo-test CI gates locally |
| `dx serve --package iron_hermes_ui` | Dioxus CLI dev server for `iron_hermes_ui` (WASM, hot reload); run from the workspace root |
| `dx serve --platform desktop --package iron_hermes_ui` | Run `iron_hermes_ui` as a native desktop window |
| `dx bundle --platform web -p iron_hermes_ui [--release]` | Build the standalone web binary at `target/dx/iron_hermes_ui/{debug,release}/web/iron_hermes_ui` |
| `RUSTFLAGS='--cfg getrandom_backend="wasm_js"' cargo check --target wasm32-unknown-unknown -p iron_hermes_ui` | Type-check the actual `wasm32` build of `iron_hermes_ui`; a native `cargo build`/`clippy` never compiles `#[cfg(target_arch = "wasm32")]` code, so this is the only gate that catches wasm-only breakage |

---

## Code Style

### Rust (all crates except `iron_hermes_ui`)

- **Formatter:** `rustfmt` — run with `cargo fmt --all`. CI enforces clean formatting via `cargo fmt --all -- --check`.
- **Linter:** `cargo clippy` — CI runs with `--workspace --all-targets --all-features -- -D warnings`. All clippy warnings are hard errors.
- **`RUSTFLAGS`:** `-D warnings` is set in CI (`ci.yml` `env` block), so any new warning breaks the build.

### `iron_hermes_ui` (Dioxus 0.7 web UI crate)

- Same `rustfmt` and `clippy` rules apply.
- Additional clippy rules are configured in `crates/iron_hermes_ui/clippy.toml`: signal borrows (`GenerationalRef`, `GenerationalRefMut`, `dioxus_signals::WriteLock`) must **not** be held across `.await` points — this causes runtime panics.
- Dioxus 0.7 component conventions:
  - Use `use_signal`, `use_memo`, `use_resource`, `use_context_provider` / `use_context`.
  - Do **not** use `cx`, `Scope`, or `use_state` — these are removed Dioxus 0.6 APIs.
  - Component functions must be `PascalCase` and annotated `#[component]`.

### Async→sync bridges: use `block_on_sync`, never `block_in_place`

`commands::handlers::dispatch` and the `CommandContext` handle traits
(`SubagentListSnapshot`, `ProcessRegistrySnapshotHandle`, `ToolsetSessionHandle`, …)
are **synchronous** by design, but their implementations guard state behind
`tokio::sync::RwLock`. Every such bridge must go through
`ironhermes_core::async_bridge::block_on_sync`.

**Do not use `tokio::task::block_in_place` in any code reachable from slash-command
dispatch or a handle trait.** It panics with
`"can call blocking only when running on the multi-threaded runtime"` whenever the
caller is on a current-thread runtime *or* inside a `tokio::task::LocalSet` — and
the Dioxus fullstack server polls **every websocket server-fn handler inside a
per-connection `LocalSet`**. The runtime underneath is multi-threaded, so
`Handle::runtime_flavor()` still reports `MultiThread`; there is no public tokio API
that reports the flag which actually governs this. You cannot detect the bad case at
runtime — you can only avoid the call.

This is a repeat offender. It broke `RegistrationGuard::drop` (Phase 26.7-06 UAT,
fixed in 26.7-07) and then broke `/agents` on the web UI (Phase 41.3 UAT) the moment
Web's `CommandContext` was wired past two handles and the remaining bridges became
reachable from a `LocalSet`. **A native build, the CLI, the TUI and the gateway all
pass while Web panics at runtime** — cargo cannot catch this for you.

The CLI TUI's own two `block_in_place` sites are acceptable: they only ever run on a
multi-thread runtime, never inside a `LocalSet`.

When adding a new bridge, mirror the regression test at
`crates/ironhermes-agent/tests/localset_sync_bridge.rs`, which drives the sync
methods from inside a real `LocalSet`. `block_on_sync`'s own unit tests cover all
four runtime contexts, write visibility, and panic propagation.

**Cost:** one short-lived OS thread per call, and the calling thread parks in
`join()` without announcing the block to the scheduler. That is fine for interactive,
low-frequency work (slash dispatch, toolset refresh). Do **not** reach for it on a
per-frame or per-token path — restructure the caller to be async instead.

### `CommandContext` handle wiring is per-surface

`build_core_context` populates only the **nine core handles**. Everything else
(`provider_resolver`, `mcp_manager`, `memory_manager`, `context_compressor`,
`personality_overlay`, `agent_loop`, `cron_store`, `history`) is attached
surface-by-surface with `with_*` builders, and each has a "not configured" guard that
returns informational text instead of failing loudly. A surface that forgets one gets
a **silently degraded slash command**, not a compile error.

Current state: the TUI wires all of them. Web wires the nine core handles plus
`provider_resolver`; the remaining seven are unwired, so those commands are inert
there. The gateway is wired more sparsely still. When adding a handle, check every
surface — and prefer putting the adapter in `ironhermes-core` next to the trait so it
can be shared (`ProviderResolverAdapter` is the model; it previously lived private to
the TUI, which is precisely why `/model` was dead everywhere else).

### Optional `rusty-vault` cargo feature

`ironhermes-vault` (and every crate that threads it through: `ironhermes-core`, `ironhermes-agent`, `ironhermes-cli`, `ironhermes-cron-runner`, `iron_hermes_ui`) exposes an optional `rusty-vault` feature that pulls in the vendored `rusty_vault = "=0.2.1"` dependency (and transitively `openssl`) to back a `RustyVaultStore` fallback for provider API key resolution. It is **off by default** — default builds carry zero vault-provider deps and use only the always-on `EnvVarStore`. Enable it explicitly when working on the vault integration:

```bash
cargo build --features rusty-vault
cargo clippy --workspace --all-targets --all-features -- -D warnings   # --all-features already covers this
```

The vendored `rusty_vault` crate debug-logs secret material (the raw master key and root token) at `debug` level, so a `rusty_vault=off` directive is load-bearing in every `tracing-subscriber` `EnvFilter` in the codebase — do not remove it, and never run with `RUST_LOG=debug` and `--features rusty-vault` together without it.

---

## Branch Conventions

No branch naming convention is formally documented in this repository. The CI pipeline triggers on pushes and pull requests targeting the `develop` and `main` branches.

Suggested practice (inferred from commit history):
- Feature branches: `feat/<description>`
- Bug fix branches: `fix/<description>`
- Default integration branch: `develop`

---

## PR Process

- Open pull requests against `develop` (the default integration branch).
- CI must pass all five jobs before merge:
  1. **Phase 21.7 CI gates** — runs `bash scripts/ci-gates.sh` (static-grep + targeted `cargo test` gates for E-05, E-08, E-09, D-12 invariants).
  2. **insta snapshots up-to-date** — `cargo insta test --unreferenced=reject -p ironhermes-cli --all-features` rejects orphaned `.snap` files in `ironhermes-cli` (every committed snapshot lives under `crates/ironhermes-cli/tests/snapshots`).
  3. **cargo nextest run --workspace** — full workspace test suite with all features enabled, plus a separate `cargo test --workspace --all-features --doc` step (nextest does not run doctests).
  4. **cargo fmt + clippy** — formatting check and lint in `-D warnings` mode (`cargo clippy --workspace --all-targets --all-features`).
  5. **cargo audit (supply-chain)** — `cargo audit` checks `Cargo.lock` against the RustSec advisory database.
- No PR template is present in this repository. Include a description of what changed and why.
- Snapshot changes (`*.snap` files under `crates/*/tests/snapshots/`) must be reviewed — run `cargo insta review` locally before pushing if snapshots changed.

---

## CI Gates (`scripts/ci-gates.sh`)

The `scripts/ci-gates.sh` script can be run locally at any time from the workspace root:

```bash
bash scripts/ci-gates.sh
```

It enforces six invariants:

| Gate | ID | Description |
|------|----|-------------|
| 1 | E-05 | `BudgetHandle` must use only `SeqCst` ordering — no `Ordering::Relaxed` |
| 2 | E-08 | Transcript writer path must not `.unwrap()` or `.expect()` — write errors are fire-and-forget |
| 3 | E-09 | Three-site wiring parity for `AgentSubagentRunner::new`, `register_delegate_task_tool`, `register_execute_code_tool_with_*` |
| 4 | D-12 | Gateway and `main.rs` must not read a per-request `yolo` field — `--yolo` is a process-scoped flag only |
| 5 | D-10 | `iron_hermes_ui`'s auth layer must register after every raw `.route(` call in `main.rs`, and `into_make_service_with_connect_info` must replace the bare `into_make_service()` |
| 6 | D-13a | The artifact iframe's `ARTIFACT_CSP` must retain `connect-src 'none'` and the viewer's sandbox must never gain `allow-same-origin` — the two containments ADR-001 (Phase 47.3) requires to stay that way |

Gates 1–3 run as `cargo test` targets; gates 4–6 are static `grep`/`awk` checks. All six must pass for CI to go green.
