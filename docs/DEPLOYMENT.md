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

---

## Build Pipeline

The CI workflow (`.github/workflows/ci.yml`) runs on every push or pull request to `develop` and `main`. There is no automated deploy step — deployment is a manual action after CI passes.

**CI jobs (all run on `ubuntu-latest`):**

1. **Phase 21.7 CI gates** — runs `scripts/ci-gates.sh` (static-grep and cargo-test invariant checks)
2. **insta snapshots up-to-date** — `cargo insta test --unreferenced=reject --workspace`
3. **cargo test --workspace** — `cargo test --workspace --all-features`
4. **cargo fmt + clippy** — `cargo fmt --all -- --check` then `cargo clippy --workspace --all-targets --all-features -- -D warnings`

There is no automated publish or release pipeline detected in the repository. <!-- VERIFY: GitHub Releases binary publishing process (referenced by install.sh but no workflow found) -->

---

## Native Install

The `install.sh` script handles end-to-end native installation:

```bash
# Install via curl-pipe (sets up ~/.local/bin/ironhermes and ~/.ironhermes/)
curl -fsSL https://raw.githubusercontent.com/bradwilson331/ironhermes/main/install.sh | bash
```

<!-- VERIFY: GitHub Releases URL and binary naming convention used by install.sh -->

The installer:
1. Detects OS (`linux` or `macos`) and architecture (`x86_64` or `aarch64`)
2. Downloads a prebuilt binary from GitHub Releases, or falls back to `cargo install`
3. Installs the binary to `~/.local/bin/ironhermes`
4. Scaffolds `~/.ironhermes/` with `config.yaml`, `.env`, and directory structure
5. Copies `cli-config.yaml.example` → `~/.ironhermes/config.yaml` and `.env.example` → `~/.ironhermes/.env`

After install, seed your API keys in `~/.ironhermes/.env` and set the model provider in `~/.ironhermes/config.yaml`. See [CONFIGURATION.md](CONFIGURATION.md) for the full variable reference.

---

## Docker Deployment

The `Dockerfile` uses a three-stage build:

- **Stage 0 (`gosu_source`)** — pulls `gosu` 1.19 from `tianon/gosu` for privilege dropping
- **Stage 1 (`builder`)** — `rust:latest`; compiles `ironhermes` release binary with workspace layer caching
- **Stage 2 (`runtime`)** — `debian:bookworm-slim`; installs `python3`, `ca-certificates`, `procps`; runs as UID 10000 (`ironhermes` user)

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

## Environment Setup

Refer to [CONFIGURATION.md](CONFIGURATION.md) for the complete environment variable reference. The minimum required variables for a functioning deployment are:

| Variable | Required for |
|---|---|
| `OPENROUTER_API_KEY` (or `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`) | LLM provider — at least one is required |
| `TELEGRAM_BOT_TOKEN` | Telegram gateway mode |
| `TELEGRAM_ALLOWED_USERS` | Restrict gateway access to specific chat IDs |

All variables are read from `~/.ironhermes/.env` (native) or the container environment (Docker). Never pass secrets as positional arguments or embed them in `config.yaml`.

---

## Rollback Procedure

There is no automated rollback pipeline. To revert to a previous version:

**Native install:**
1. Identify the previous binary version from GitHub Releases <!-- VERIFY: GitHub Releases URL -->
2. Download the matching binary for your platform
3. Replace `~/.local/bin/ironhermes` with the previous binary
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
