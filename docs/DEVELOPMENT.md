<!-- generated-by: gsd-doc-writer -->
# Development

## Local Setup

IronHermes is a Cargo workspace written in Rust (Edition 2024). The web UI crate (`iron_hermes_ui`) additionally requires the Dioxus CLI (`dx`).

### Prerequisites

- Rust stable toolchain (managed via `rustup`) — includes `cargo`, `rustfmt`, and `clippy`
- `cargo-insta` for snapshot testing: `cargo install cargo-insta --locked`
- Dioxus CLI for the web UI crate: `curl -sSL http://dioxus.dev/install.sh | sh`

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

# Web UI only (requires dx CLI)
cd crates/iron_hermes_ui
dx serve          # hot-reload dev server at http://localhost:8080
```

---

## Build Commands

| Command | Description |
|---------|-------------|
| `cargo build` | Compile all default workspace crates (debug) |
| `cargo build --release` | Compile optimized release binary |
| `cargo run --bin ironhermes` | Run interactive REPL |
| `cargo run --bin ironhermes -- -e "<prompt>"` | Run a single prompt non-interactively |
| `cargo run --bin ironhermes -- status` | Show agent/session status |
| `cargo run --bin ironhermes -- doctor` | Validate configuration |
| `cargo test --workspace --all-features` | Run full test suite |
| `cargo fmt --all` | Format all crates |
| `cargo fmt --all -- --check` | Check formatting without modifying files (CI) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Run linter (CI-strict mode) |
| `cargo insta test --unreferenced=reject --workspace` | Run snapshot tests, rejecting orphaned snapshots |
| `bash scripts/ci-gates.sh` | Run Phase 21.7 static-analysis + cargo-test CI gates locally |
| `dx serve` | Dioxus CLI dev server for `iron_hermes_ui` (WASM, hot reload) |
| `dx serve --platform desktop` | Run `iron_hermes_ui` as a native desktop window |

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
- CI must pass all four jobs before merge:
  1. **Phase 21.7 CI gates** — runs `bash scripts/ci-gates.sh` (static-grep + targeted `cargo test` gates for E-05, E-08, E-09, D-12 invariants).
  2. **insta snapshots up-to-date** — `cargo insta test --unreferenced=reject --workspace` rejects orphaned `.snap` files and ensures committed snapshots match current code.
  3. **cargo test --workspace** — full workspace test suite with all features enabled.
  4. **cargo fmt + clippy** — formatting check and lint in `-D warnings` mode.
- No PR template is present in this repository. Include a description of what changed and why.
- Snapshot changes (`*.snap` files under `crates/*/tests/snapshots/`) must be reviewed — run `cargo insta review` locally before pushing if snapshots changed.

---

## CI Gates (`scripts/ci-gates.sh`)

The `scripts/ci-gates.sh` script can be run locally at any time from the workspace root:

```bash
bash scripts/ci-gates.sh
```

It enforces four invariants:

| Gate | ID | Description |
|------|----|-------------|
| 1 | E-05 | `BudgetHandle` must use only `SeqCst` ordering — no `Ordering::Relaxed` |
| 2 | E-08 | Transcript writer path must not `.unwrap()` or `.expect()` — write errors are fire-and-forget |
| 3 | E-09 | Three-site wiring parity for `AgentSubagentRunner::new`, `register_delegate_task_tool`, `register_execute_code_tool_with_*` |
| 4 | D-12 | Gateway and `main.rs` must not read a per-request `yolo` field — `--yolo` is a process-scoped flag only |

Gates 1–3 run as `cargo test` targets; gate 4 is a static `grep`. All four must pass for CI to go green.
