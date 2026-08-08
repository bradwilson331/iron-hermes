<!-- generated-by: gsd-doc-writer -->
# Deployment

IronHermes is a Rust binary that runs on Linux and macOS. It supports three deployment modes:

1. **Native install** — binary dropped into `~/.local/bin/` via the installer script, run interactively or as a background gateway service.
2. **Docker container** — multi-stage image producing a minimal Debian Bookworm runtime.
3. **Gateway service** — long-running `ironhermes gateway` process managed by launchd (macOS), systemd --user (Linux), or a cron watchdog (fallback).

---

## Deployment Targets

| Target | Config file | Notes |
|---|---|---|
| Native (macOS/Linux) | `install.sh` | Installs prebuilt binary from GitHub Releases; falls back to `cargo install` |
| Docker | `Dockerfile` | Multi-stage build; exposes port 8080; persists data to `/opt/data` volume |
| macOS LaunchAgent | `scripts/deploy/com.ironhermes.gateway.plist` | Managed by launchd; restarts on crash |
| Linux systemd --user | `scripts/deploy/ironhermes-gateway.service` | Managed by systemd user session; requires `loginctl enable-linger` on headless servers |
| Cron watchdog | `scripts/deploy/gateway-watchdog.sh` | Fallback for systems without launchd/systemd; checks PID every minute |
| Web UI (native web server) | `scripts/deploy/web-run.sh` | Runs the standalone `iron_hermes_ui` server binary; loopback-only by default |
| Web UI — Linux systemd --user | `scripts/deploy/ironhermes-web.service` | Managed by systemd user session; points at `web-run.sh` |
| Web UI — macOS LaunchAgent | `scripts/deploy/com.ironhermes.web.plist` | Managed by launchd; points at `web-run.sh` |

---

## Build Pipeline

The CI workflow (`.github/workflows/ci.yml`) runs on every push or pull request to `develop` and `main`.

**CI jobs (all run on `ubuntu-latest`):**

1. **Phase 21.7 CI gates** — runs `scripts/ci-gates.sh` (static-grep and cargo-test invariant checks)
2. **insta snapshots up-to-date** — `cargo insta test --unreferenced=reject -p ironhermes-cli --all-features` (scoped to the crate that owns every committed `.snap` file)
3. **cargo nextest run --workspace** — `cargo nextest run --no-fail-fast --workspace --all-features`, plus a separate `cargo test --workspace --all-features --doc` step (nextest does not run doctests)
4. **cargo fmt + clippy** — `cargo fmt --all -- --check` then `cargo clippy --workspace --all-targets --all-features -- -D warnings`
5. **cargo audit (supply-chain)** — `cargo audit` against the RustSec advisory database

None of the CI jobs build with the `rusty-vault` cargo feature enabled.

### Release Pipeline

A separate workflow (`.github/workflows/release.yml`) triggers on pushing a `v*` tag and builds/publishes prebuilt binaries — this is the automated release/deploy step:

- **Matrix targets:** macOS Apple Silicon (`aarch64-apple-darwin`), macOS Intel (`x86_64-apple-darwin`), Linux x86_64 (`x86_64-unknown-linux-gnu`), Linux aarch64 (`aarch64-unknown-linux-gnu`). Windows is not built yet (the codebase has ~56 Unix-only API call sites).
- **Build command:** `cargo build --release --bin ironhermes` — release builds intentionally omit `-D warnings`, and do **not** pass `--features rusty-vault`, so official release binaries ship with the vault feature disabled (env-var/file secret resolution only).
- **macOS signing:** ad-hoc `codesign -s - target/release/ironhermes` (no Apple Developer account provisioned yet; notarization is not performed).
- **Packaging:** each target is tarred as `ironhermes-<platform>.tar.gz` (e.g. `ironhermes-macos-aarch64.tar.gz`, `ironhermes-linux-x86_64.tar.gz`) — matches the artifact name `install.sh` requests from GitHub Releases.
- **Publish:** uploaded to the tag's GitHub Release via `softprops/action-gh-release`.

<!-- VERIFY: repository visibility (public vs private) — determines whether the curl-pipe install and `install.sh`'s GitHub Releases download work for external users -->

---

## Native Install

The `install.sh` script handles end-to-end native installation:

```bash
# Install via curl-pipe (sets up ~/.local/bin/ironhermes and ~/.ironhermes/)
curl -fsSL https://raw.githubusercontent.com/bradwilson331/iron-hermes/main/install.sh | bash
```

(Repository visibility affects whether this curl-pipe install works for external users — see the note under [Release Pipeline](#release-pipeline).)

The installer (`REPO_OWNER=bradwilson331`, `REPO_NAME=iron-hermes` — override via `IRONHERMES_REPO`):
1. Detects OS (`linux` or `macos`) and architecture (`x86_64` or `aarch64`)
2. Resolves the latest release tag from the GitHub API, downloads the matching `ironhermes-<os>-<arch>.tar.gz` from GitHub Releases, or falls back to `cargo install --git <repo> ironhermes`
3. Installs the binary to `~/.local/bin/ironhermes`
4. Scaffolds `~/.ironhermes/` with `config.yaml`, `.env`, and directory structure
5. Copies `cli-config.yaml.example` → `~/.ironhermes/config.yaml` and `.env.example` → `~/.ironhermes/.env`

Prebuilt binaries from GitHub Releases do **not** include the optional `rusty-vault` cargo feature (see [Release Pipeline](#release-pipeline)). To deploy a build with the RustyVault secret backend enabled, build from source instead: `cargo build --release --features rusty-vault -p ironhermes-cli`, then install the resulting `target/release/ironhermes` binary manually.

After install, seed your API keys in `~/.ironhermes/.env` and set the model provider in `~/.ironhermes/config.yaml`. See [CONFIGURATION.md](CONFIGURATION.md) for the full variable reference.

---

## Docker Deployment

The `Dockerfile` uses a three-stage build:

- **Stage 0 (`gosu_source`)** — pulls `gosu` 1.19 from `tianon/gosu` for privilege dropping
- **Stage 1 (`builder`)** — `rust:latest`; compiles `ironhermes` release binary (`cargo build --release --bin ironhermes`, no `--features` flags) with workspace layer caching
- **Stage 2 (`runtime`)** — `debian:bookworm-slim`; installs `python3`, `ca-certificates`, `procps`; runs as UID 10000 (`ironhermes` user)

The shipped `Dockerfile` does not build with the `rusty-vault` cargo feature. To run the container with the RustyVault secret backend available, add `--features rusty-vault` to the `cargo build` line in a local copy of the Dockerfile (or maintain a custom build stage) before building the image.

```bash
# Build the image
docker build -t ironhermes .

# Run with a named volume for persistent data
docker run -d \
  --name ironhermes \
  -v ironhermes-data:/opt/data \
  -p 8080:8080 \
  ironhermes
```

**Volume:** `/opt/data` is the container's `IRONHERMES_HOME`. Mount a named volume here to persist sessions, memories, config, and logs across container restarts.

**Port:** `8080` is exposed; the gateway HTTP endpoint listens here.

### Container Environment Variables

The entrypoint (`docker/entrypoint.sh`) seeds config templates on first run, drops privileges from root to the `ironhermes` user (UID 10000), and respects the following runtime overrides:

| Variable | Description |
|---|---|
| `IRONHERMES_HOME` | Data directory inside the container. Default: `/opt/data` |
| `IRONHERMES_UID` | Override the runtime UID (for volume ownership compatibility with host) |
| `IRONHERMES_GID` | Override the runtime GID |

Pass provider API keys and gateway tokens via `docker run -e` or a `--env-file`:

```bash
docker run -d \
  --name ironhermes \
  -v ironhermes-data:/opt/data \
  -p 8080:8080 \
  -e OPENROUTER_API_KEY=sk-or-... \
  -e TELEGRAM_BOT_TOKEN=... \
  ironhermes
```

---

## Gateway Service Setup

The Telegram gateway runs as a persistent background process (`ironhermes gateway`). Use the platform-appropriate service manager.

### macOS — LaunchAgent

The installer copies the plist template to `~/Library/LaunchAgents/com.ironhermes.gateway.plist` with `__HOME__` substituted. To manage manually:

```bash
# Load and start
launchctl load ~/Library/LaunchAgents/com.ironhermes.gateway.plist

# Stop and unload
launchctl bootout gui/$UID/com.ironhermes.gateway

# View logs
tail -f ~/.ironhermes/logs/gateway.out.log
tail -f ~/.ironhermes/logs/gateway.err.log
```

The LaunchAgent restarts the gateway on crash (`KeepAlive.Crashed=true`) but not on a clean exit. Restart storms are throttled: one restart per 30 seconds (`ThrottleInterval=30`).

### Linux — systemd --user

```bash
# Copy unit file
mkdir -p ~/.config/systemd/user/
cp scripts/deploy/ironhermes-gateway.service ~/.config/systemd/user/

# Enable and start
systemctl --user daemon-reload
systemctl --user enable --now ironhermes-gateway

# View logs
journalctl --user -u ironhermes-gateway -f
```

On headless servers with no graphical login session, enable linger so the user service persists after logout:

```bash
loginctl enable-linger $USER
```

The unit restarts automatically (`Restart=always`, `RestartSec=5`), capped at 5 starts per 60-second window.

### Cron Watchdog (Fallback)

For systems without launchd or systemd, a cron-driven watchdog checks the gateway PID every minute and relaunches if it has died:

```bash
# Add to crontab
(crontab -l 2>/dev/null; echo "* * * * * $HOME/.ironhermes/scripts/gateway-watchdog.sh >/dev/null 2>&1 # ironhermes-gateway-watchdog") | crontab -
```

The watchdog reads `~/.ironhermes/gateway.pid`, probes with `kill -0`, and re-launches via `gateway-run.sh` if the process is gone. Logs are appended to `~/.ironhermes/logs/gateway.log`.

---

## Web UI Deployment (`iron_hermes_ui`)

### Security — read this first

Authentication is **opt-in by configuration**. With no `web_ui.auth.password_hash` set, behavior is identical to the pre-47.3 posture: no login, loopback-only bind is the operating assumption, and `security.web_config_write_enabled` / `vault.enabled` remain feature toggles (gating *what* the server will do), not authentication (gating *who* is allowed to ask it). Configure a hash and the boundary flips on: every request — server functions, the chat WebSocket, `/artifacts/{id}`, `/chat-attachments/{session_id}/{id}` — requires a valid session cookie, on every bind address.

**A non-loopback bind with no configured hash now refuses to start.** This is a hard startup check in the binary itself (`bind_guard_allows` in `main.rs`, called before `TcpListener::bind`), not only a shell-script warning — so launching via systemd, a LaunchAgent, or the binary directly is covered too, not just `scripts/deploy/web-run.sh`. The refusal message:

> refusing to bind `<address>`: this is a non-loopback address and no `web_ui.auth.password_hash` is configured. Set one via `ironhermes web set-password` (paste the printed hash into `web_ui.auth.password_hash` in config.yaml), or bind to a loopback address (127.0.0.1 / ::1) instead.

**Generating the credential:**

```bash
ironhermes web set-password            # prompts twice (masked), prints an argon2id PHC string
ironhermes web set-password --vault    # same prompt, stores the hash in the vault SecretStore instead
```

The command never writes `config.yaml` itself — paste the printed PHC string into `web_ui.auth.password_hash` by hand (or use `--vault` and let the existing vault fallback resolve it at startup).

**The full `web_ui.auth.*` key reference:**

| Key | Default | Meaning |
|---|---|---|
| `password_hash` | unset | argon2id PHC string; auth is disabled while this is unset. Also resolvable via `IRONHERMES_WEB_PASSWORD_HASH` env or the vault key `web_ui/auth/password_hash` |
| `login_theme` | `basic` | Which of the five login treatments the server renders; an unrecognized slug falls back to `basic` |
| `cookie_secure` | `false` | Adds the `Secure` cookie attribute — **cannot be set `true` without a TLS story** (see below) |
| `session_ttl_hours` | `168` (7 days) | Absolute session lifetime from creation |
| `idle_timeout_hours` | `24` | Sliding idle timeout, refreshed on authenticated requests |

**Operational facts:**

- Sessions are held **in-memory**, so every server restart forces a re-login by design — this is intentional, not a bug to report.
- Logout (`POST /auth/logout`, the `/logout` command-palette item) is a **true revocation** — the session token is deleted server-side, not just expired client-side.
- Lost password: run `ironhermes web set-password` again and restart with the new hash; there is no recovery flow.
- Single operator only — no roles, no multi-user, no OIDC.

**What is still true and must not be softened:** there is no built-in TLS. On plain-LAN HTTP the login password and the session cookie are sniffable by anything on the same network segment, and `cookie_secure` cannot be set `true` without a reverse proxy or load balancer terminating TLS in front of this server. Tailscale (or an equivalent WireGuard overlay) is the assumed transport-confidentiality layer for LAN/tailnet use — it encrypts the link itself, which is why plain `cookie_secure: false` HTTP is acceptable there but not on an open LAN. A reverse proxy is still the recommendation for any public-internet posture; it is no longer a hard requirement for LAN/tailnet use now that the server enforces its own fail-closed bind guard.

Because of the bind guard above, every script below still defaults to a loopback (`127.0.0.1`) bind and refuses any other address unless you explicitly set `IRONHERMES_WEB_ALLOW_PUBLIC_BIND=1` — that shell-script gate is now an earlier, friendlier warning layered in front of the binary's own real guarantee, not the only line of defense.

### Build

```bash
scripts/deploy/web-build.sh              # release bundle
scripts/deploy/web-build.sh --debug      # debug bundle, fast iteration
scripts/deploy/web-build.sh --skip-wasm-check  # skip the wasm gate
```

Runs `dx bundle --platform web --release --package iron_hermes_ui` from the workspace root. `--package` (or `-p`) is mandatory on every `dx` invocation for this crate: `iron_hermes_ui` is excluded from `[workspace] default-members`, so a bare `dx bundle`/`dx serve` from the workspace root resolves the wrong package, and running `dx` from inside `crates/iron_hermes_ui/` panics in Dioxus's `find_main_package`.

Before bundling, the script runs the wasm32 type-check gate:

```bash
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' cargo check --target wasm32-unknown-unknown -p iron_hermes_ui
```

A native `cargo build`/`clippy` never type-checks `#[cfg(target_arch = "wasm32")]` code, so this is the only gate that catches wasm-only breakage before a bundle ships.

**Output layout:** `target/dx/iron_hermes_ui/release/web/` — the `iron_hermes_ui` server binary, a sibling `public/` directory, and `.manifest.json`.

### Run in place

```bash
scripts/deploy/web-run.sh
```

Resolves the built bundle, sources `~/.ironhermes/.env`, gates the bind address, exports `IP`/`PORT`, and `exec`s the server binary so OS signals reach it directly.

| Variable | Description |
|---|---|
| `IRONHERMES_WEB_BIND` | Bind address. Default: `127.0.0.1` |
| `IRONHERMES_WEB_PORT` | Bind port. Default: `8080` |
| `IRONHERMES_WEB_ALLOW_PUBLIC_BIND` | Set to `1` to allow a non-loopback `IRONHERMES_WEB_BIND` — prints a no-authentication exposure warning when used |
| `IRONHERMES_UI_BUNDLE_DIR` | Explicit bundle directory override (default: staged install, else the workspace release build) |
| `IRONHERMES_UI_BIN` | Explicit server binary override |
| `DIOXUS_PUBLIC_PATH` | Escape hatch if the binary is not sitting next to its bundle's `public/` |

The script exports `IP` and `PORT` because those are the exact environment variable names `dioxus_cli_config::fullstack_address_or_localhost()` reads (`main.rs` binds whatever that call resolves to).

### Install as a service

```bash
scripts/deploy/web-install.sh             # stage bundle, register + start service
scripts/deploy/web-install.sh --no-start  # register without starting
scripts/deploy/web-install.sh --force     # overwrite existing service registration
```

Stages the whole built bundle directory (binary + `public/` + manifest, as a unit — never just the binary) into `~/.ironhermes/web/`, stages `web-run.sh` into `~/.ironhermes/scripts/`, and registers the platform-appropriate service.

**macOS — LaunchAgent** (`scripts/deploy/com.ironhermes.web.plist`, label `com.ironhermes.web`):

```bash
launchctl print gui/$UID/com.ironhermes.web | grep -E 'state|pid|last exit'
tail -f ~/.ironhermes/logs/web.err.log

# Stop:    launchctl bootout gui/$UID/com.ironhermes.web
# Restart: launchctl kickstart -k gui/$UID/com.ironhermes.web
```

**Linux — systemd --user** (`scripts/deploy/ironhermes-web.service`):

```bash
systemctl --user status ironhermes-web
journalctl --user -u ironhermes-web -f

# Stop:    systemctl --user stop ironhermes-web
# Restart: systemctl --user restart ironhermes-web
```

On headless servers with no graphical login session, enable linger so the user service persists after logout: `loginctl enable-linger $USER`.

Logs: `~/.ironhermes/logs/web.out.log` and `~/.ironhermes/logs/web.err.log` (both platforms).

### Local / staging dev server

```bash
scripts/deploy/web-dev.sh
```

This is **not** the production path. It runs `dx serve --package iron_hermes_ui` with the identical loopback default and `IRONHERMES_WEB_ALLOW_PUBLIC_BIND` opt-in gate as `web-run.sh`, so the same rule applies in both places. Use it for local iteration only: `dx serve` proxies requests through the Dioxus CLI's own dev server, which imposes a WebSocket idle timeout — production always uses the standalone binary via `web-build.sh` + `web-run.sh` instead.

### The bundle must stay intact

`iron_hermes_ui` resolves static assets as `current_exe().parent()/public` — relative to the running binary's own directory, not the working directory. Never copy the server binary out of its bundle away from its sibling `public/` directory; if you must relocate it, set `DIOXUS_PUBLIC_PATH` explicitly.

### Not part of the automated release workflow

`.github/workflows/release.yml` builds only `cargo build --release --bin ironhermes` (the CLI/gateway binary). The web UI bundle is built and deployed manually via the scripts on this page — there is no CI/CD step that produces or publishes it.

---

## Environment Setup

Refer to [CONFIGURATION.md](CONFIGURATION.md) for the complete environment variable reference. The minimum required variables for a functioning deployment are:

| Variable | Required for |
|---|---|
| `OPENROUTER_API_KEY` (or `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`) | LLM provider — at least one is required |
| `TELEGRAM_BOT_TOKEN` | Telegram gateway mode |
| `TELEGRAM_ALLOWED_USERS` | Restrict gateway access to specific chat IDs |

All variables are read from `~/.ironhermes/.env` (native) or the container environment (Docker). Never pass secrets as positional arguments or embed them in `config.yaml`.

### Optional: RustyVault Secret Backend

By default, provider API keys are resolved from `~/.ironhermes/.env` via the always-on `env-var` backend — no extra provisioning is required. An optional, feature-gated RustyVault backend can be enabled as a last-resort fallback for a given deployment:

1. **Build with the feature compiled in.** The binary must be built with `--features rusty-vault` (e.g. `cargo build --release --features rusty-vault -p ironhermes-cli`). Neither the CI workflow, the release pipeline, nor the shipped `Dockerfile` build with this feature by default — see the notes above.
2. **Provision vault state per host.** The vault's on-disk data lives under `$IRONHERMES_HOME/vault` by default (i.e. `~/.ironhermes/vault` for a native install, or `/opt/data/vault` inside the Docker container) — set `vault.rusty_vault.data_dir` in `config.yaml` to override. With the default `unseal_mode: keyfile`, a `0600` keyfile holding the unseal key and root token is written beside the data directory and auto-unseals the vault on every open (no prompt) — this keyfile is sensitive and must be included in host backup/restore procedures alongside `~/.ironhermes/.env`.
3. **Enable, initialize, and migrate — use the wrapper script.** `scripts/deploy/vault-migrate.sh` performs the whole hardening flow in one pass (see the step-by-step below). Manual equivalent: set `vault.enabled: true` and `vault.backend: rusty-vault` in `config.yaml`, run `ironhermes vault init` once per host, then `ironhermes vault migrate` — but note the built-in `migrate` imports from `.env` **only**; it never touches inline `api_key:` literals in `config.yaml`, and those are precedence #1, so leaving even one in place means resolution never falls through to the vault at all.

#### `vault-migrate.sh` — what it does, step by step

```bash
scripts/deploy/vault-migrate.sh                 # interactive (confirms each change)
scripts/deploy/vault-migrate.sh --dry-run       # report what would change; change nothing
scripts/deploy/vault-migrate.sh --yes           # no prompts (provisioning/CI)
scripts/deploy/vault-migrate.sh --yes --purge-backups  # also delete plaintext backups after verify passes
```

1. **Preflight.** Resolves `IRONHERMES_HOME` (default `~/.ironhermes`) and the `ironhermes` binary (`$IRONHERMES_BIN` → `~/.local/bin/ironhermes` → `PATH` → `target/{release,debug}/ironhermes`); requires an existing `config.yaml`. The binary must be built with `--features rusty-vault` — if not, step 4 fails with a rebuild hint.
2. **Back up `config.yaml`** to a `0600` timestamped `config.yaml.pre-vault-<ts>.bak` before any edit.
3. **Ensure the `vault:` block.** Appends `vault: {enabled: true, backend: rusty-vault, rusty_vault: {unseal_mode: keyfile}}` if no block exists. If a block exists but has the wrong `backend`/`enabled`, the script stops and tells you to edit it manually rather than rewriting nested YAML.
4. **`ironhermes vault init`** — creates the encrypted store (default `$IRONHERMES_HOME/vault`) and its sibling `0600` unseal keyfile. Skipped when the keyfile already exists (idempotent re-runs).
5. **`ironhermes vault migrate`** (the built-in) — imports provider API keys from `$IRONHERMES_HOME/.env` into the vault: writes a `0600` timestamped backup of the full original `.env` first, vault-writes each matched key (the three legacy names plus every configured `providers.<name>.api_key_env`), then scrubs **only** the successfully-migrated lines; all other lines (comments, gateway/Telegram tokens) survive byte-for-byte. One audit entry per key, names only.
6. **Inline `providers.<name>.api_key` literals → vault.** The gap the built-in leaves: for each non-null inline key in the `providers:` block, the value is piped (never argv, never echoed) into `ironhermes vault set <provider>`, the write is confirmed via `vault list`, and only then is the config line rewritten to `api_key: null`. `api_key:` literals at deprecated locations outside the `providers:` block (e.g. legacy `model.api_key`) are reported with line numbers but not modified — those need operator judgment.
7. **Verify.** Prints `vault list` (key names only) and the `doctor` vault checks (enabled / backend / data-dir / sealed-state), then scans `.env` and the `providers:` block for residual plaintext keys. Any residue fails the verify.
8. **Plaintext backups.** The `.env` and `config.yaml` backups contain your original keys in plaintext (`0600`, but same-user readable — including by the agent's own tools). The script lists them with a deletion reminder, or deletes them itself with `--purge-backups` (only when verify passed).

After the script: restart long-running services (gateway, web UI server) so they resolve keys from the vault, and remember every binary that should read the vault needs the feature compiled in (`-p ironhermes-cli --features rusty-vault`; `-p iron_hermes_ui --features server,rusty-vault`).

See [CONFIGURATION.md](CONFIGURATION.md#vault-vault) for the full config reference and CLI subcommands.

---

## Rollback Procedure

There is no automated rollback pipeline. To revert to a previous version:

**Native install:**
1. Identify the previous release tag (e.g. `v1.2.3`) from the project's GitHub Releases page
2. Re-run the installer pinned to that version: `IRONHERMES_VERSION=v1.2.3 curl -fsSL https://raw.githubusercontent.com/bradwilson331/iron-hermes/main/install.sh | bash` — this downloads the matching `ironhermes-<os>-<arch>.tar.gz` from that release instead of `latest`
3. This replaces `~/.local/bin/ironhermes` with the previous binary automatically
4. Restart the gateway service: `systemctl --user restart ironhermes-gateway` (Linux) or reload the LaunchAgent (macOS)

**Docker:**
1. Pull or retag the previous image version
2. Stop and remove the running container: `docker stop ironhermes && docker rm ironhermes`
3. Start a new container with the previous image tag
4. The `/opt/data` volume is preserved — no data migration needed for a same-major rollback

**Configuration rollback:**
- `~/.ironhermes/config.yaml` and `~/.ironhermes/.env` are plain files; restore from a backup or version control snapshot
- The entrypoint only seeds templates when files are absent, so existing config is never overwritten by a redeploy

---

## Uninstall

```bash
# Remove native service (auto-detects macOS launchd or Linux systemd)
bash scripts/deploy/uninstall.sh

# Remove cron watchdog entry only
bash scripts/deploy/uninstall.sh --cron

# Remove service, cron entry, and staged scripts
bash scripts/deploy/uninstall.sh --all
```

Logs in `~/.ironhermes/logs/` are preserved by the uninstaller.

---

## Monitoring

No third-party monitoring library (Sentry, Datadog, New Relic, OpenTelemetry) was detected in the project dependencies. Runtime observability is available through:

- **Structured logs** — set `RUST_LOG=ironhermes=info` (or `debug`) in `~/.ironhermes/.env` to control log verbosity
- **Gateway logs** — `~/.ironhermes/logs/gateway.log`, `gateway.out.log`, `gateway.err.log`
- **systemd journal** — `journalctl --user -u ironhermes-gateway -f` (Linux)
- **PID file** — `~/.ironhermes/gateway.pid` (3-line YAML; readable by the watchdog and external health checks)

<!-- VERIFY: any external monitoring or alerting integration beyond file-based logs -->
