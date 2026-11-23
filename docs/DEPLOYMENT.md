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

## Container Deployment (Podman / Docker)

> **Step-by-step build/run walkthrough (prerequisites, rootless notes,
> systemd user service, troubleshooting): [CONTAINER.md](CONTAINER.md).**
> This section is the reference: build stages, environment variables, volumes.

> **The default run is [`docker-compose.yml`](../docker-compose.yml)** (phase
> 54.11): read-only root filesystem, noexec tmpfs, minimal capabilities,
> `no-new-privileges` and resource limits. `docker compose up -d` (or
> `podman compose up -d`) is the documented way to run the container; the plain
> `podman run` / `docker run` commands below are the unhardened alternative.
> What each control does and what it breaks: [CONTAINER.md](CONTAINER.md).

The `Dockerfile` uses a multi-stage build (OCI format — buildable and runnable with both **Podman** and Docker):

- **Stage 1 (`builder`)** — `rust:1.96-bookworm` (pinned; edition 2024 requires rustc ≥ 1.85); installs `pkg-config` + `libasound2-dev` (ALSA for the `cpal`/`rodio` audio deps in `ironhermes-tools`) and `libssl-dev` + `perl`; compiles `ironhermes` release binary (`cargo build --release --bin ironhermes`, no `--features` flags) with workspace layer caching
- **Stage 2 (`runtime`)** — `debian:bookworm-slim`; installs `python3`, `ca-certificates`, `procps`, `libasound2` (ALSA runtime); runs as UID 10000 (`ironhermes` user)

The build context also copies `skills/` and `assets/` (not just `crates/`/`providers/`): two crates embed files from those directories at compile time via `include_str!` (`ironhermes-kanban` embeds `skills/kanban-worker` + `skills/kanban-orchestrator` SKILL.md; `iron_hermes_ui` embeds `assets/site.css`). Omitting them fails the release build.

The shipped `Dockerfile` does not build with the `rusty-vault` cargo feature. To run the container with the RustyVault secret backend available, add `--features rusty-vault` to the `cargo build` line in a local copy of the Dockerfile (or maintain a custom build stage) before building the image.

```bash
# Build the image (podman or docker — the OCI format works with both)
podman build -t ironhermes .
#   equivalently: docker build -t ironhermes .

# STEP 1 — start once, on the image's default loopback bind, to get a password.
# A provider API key is required: iron_hermes_ui refuses to start when the main
# provider has no resolvable key, rather than coming up and failing every turn.
# The var name follows providers.<main>.api_key_env in config.yaml
# (OPENROUTER_API_KEY by default, since model.provider defaults to openrouter).
podman run -d \
  --name ironhermes \
  -v ironhermes-data:/opt/data \
  -p 8080:8080 \
  -e OPENROUTER_API_KEY=sk-or-... \
  ironhermes

# STEP 2 — read the generated password (printed once, hash stored in the volume)
podman logs ironhermes

# STEP 3 — expose it, explicitly. -e IP=0.0.0.0 is the opt-in to being reachable.
podman rm -f ironhermes
podman run -d \
  --name ironhermes \
  -v ironhermes-data:/opt/data \
  -p 8080:8080 \
  -e IP=0.0.0.0 \
  -e OPENROUTER_API_KEY=sk-or-... \
  ironhermes
```

**Why two steps.** The image sets `ENV IP=127.0.0.1`, so the first start binds
loopback *inside* the container. A loopback listener ignores `-p 8080:8080`
(port publishing forwards to the container's external interface), so the UI is
genuinely unreachable until step 3 — which is the point: a freshly generated
password is never on the network. Exposure is always an explicit act.

**Bringing your own password.** Mint a hash first — the image already contains
the CLI:

```bash
podman run --rm -it --entrypoint ironhermes ironhermes web set-password
```

Pass it as `-e IRONHERMES_WEB_PASSWORD_HASH='<hash>'` (**single-quote it**;
argon2id strings contain `$`). Nothing is generated or printed when it is set,
and you can pass `-e IP=0.0.0.0` on the very first run.

> **`-e IP=0.0.0.0` with no password hash is a hard refusal**, not a downgrade
> to loopback — the container exits within seconds, and under
> `--restart=always` that presents as a restart loop. The same is true when the
> provider key is missing. Both guards are deliberate: failing fast beats
> serving a broken agent, and silently binding loopback when you asked for
> `0.0.0.0` would leave you believing you were exposed when you were not.

**Volume:** `/opt/data` is the container's `IRONHERMES_HOME`. Mount a named volume here to persist sessions, memories, config, and logs across container restarts.

**Port:** `8080` is exposed; the gateway HTTP endpoint listens here.

### Container Environment Variables

The image ships two entrypoints and `Dockerfile:154` selects the web one by
default:

| Entrypoint | Execs | When |
|---|---|---|
| `docker/web-entrypoint.sh` | `iron_hermes_ui` (fullstack web server, `0.0.0.0:8080`; sole determinant of container health/lifecycle) + `ironhermes gateway --non-interactive` (background, best-effort), both supervised under `tini` as PID 1 so `stop` shuts them down gracefully | **Default** — the image's `ENTRYPOINT` |
| `docker/entrypoint.sh` | `ironhermes` CLI with your args | Override with `--entrypoint` for management/one-shot use |

Both seed config templates on first run (**only if absent**, so your edits
survive container recreation), drop privileges from root to the `ironhermes`
user (UID 10000), and respect the following runtime overrides:

| Variable | Description |
|---|---|
| `IRONHERMES_HOME` | Data directory inside the container. Default: `/opt/data` |
| `IRONHERMES_UID` | Override the runtime UID (for volume ownership compatibility with host) |
| `IRONHERMES_GID` | Override the runtime GID |
| `IRONHERMES_GATEWAY` | Launch `ironhermes gateway` in the background alongside the web UI. Default: `1`. Set to `0`/`false`/`no`/`off` to run the web UI alone. |

Pass provider API keys and gateway tokens via `podman run -e` (or `docker run -e`)
or a `--env-file`. Name the variable via `providers.<name>.api_key_env` in
`config.yaml` — that is the supported form, and it keeps the secret in the
environment rather than in the config file:

```yaml
providers:
  openrouter:
    api_key_env: OPENROUTER_API_KEY    # secret stays in .env / -e
```

Inline `providers.<name>.api_key` literals still resolve (they are precedence
#1), but they are deprecated: they put the secret in `config.yaml`, they mask
the vault backend, and the CLI's startup "runnable LLM" probe does not see them
— so an interactive `hermes chat` on a config using only an inline literal
re-launches the setup wizard on every start even though the config works. Use
`api_key_env`.


```bash
podman run -d \
  --name ironhermes \
  -v ironhermes-data:/opt/data \
  -p 8080:8080 \
  -e OPENROUTER_API_KEY=sk-or-... \
  -e TELEGRAM_BOT_TOKEN=... \
  ironhermes
```

### Cloudflare Tunnel

Cloudflare Tunnel is the supported way to put a hosted IronHermes on a public
hostname without opening an inbound port. `docker-compose.yml` carries an
optional `cloudflare` profile: a digest-pinned `cloudflared` connector (outbound
connection only, no published port, read-only, all capabilities dropped,
`no-new-privileges`) in front of the hardened `ironhermes` service. Without the
profile, compose starts only `ironhermes`.

1. In the Cloudflare Zero Trust dashboard create a **remotely managed tunnel**
   and copy its token.
2. Add a **public hostname** to the tunnel whose service is
   `http://ironhermes:8080` (the compose service name and the container port;
   `cloudflared` reaches it over the compose network).
3. Put the token in the compose project's `.env` file, mode `0600`. It is a
   bearer credential: never put it in `config.yaml`, and never commit the file.

   ```bash
   umask 077
   printf 'CLOUDFLARE_TUNNEL_TOKEN=%s\n' '<token from the dashboard>' >> .env
   ```
4. Set the three settings the exposed UI requires (the same fail-closed guards
   as any non-loopback bind, see [CONTAINER.md](CONTAINER.md)):

   | Setting | Value |
   |---|---|
   | `IRONHERMES_IP` | `0.0.0.0`, so `cloudflared` can reach the UI from another container |
   | `IRONHERMES_WEB_PASSWORD_HASH` | an argon2id hash (`ironhermes web set-password`), single-quoted |
   | `IRONHERMES_WEB_ALLOWED_ORIGINS` | `https://<tunnel hostname>`, the URL browsers use |

5. Start both services:

   ```bash
   docker compose --profile cloudflare up -d
   ```

   `cloudflared` waits for the `ironhermes` healthcheck (`depends_on` with
   `service_healthy`), so a refused bind configuration shows up as a failed
   `ironhermes` service, not a tunnel that points at nothing.
6. Check the **WebSocket path** from a browser on the public hostname: log in,
   open a chat and confirm the stream updates live. The web UI and the kanban
   board both use WebSockets; Cloudflare proxies them, but an origin mismatch
   is rejected with a 403 on upgrade. (Not yet verified live: the Cloudflare
   Tunnel and WebSocket checks in plan 54.11-06 were deferred by the operator on
   2026-11-03.)

The `ironhermes` host port stays bound to `127.0.0.1`; the tunnel does not use
it. `cloudflared` is stateless, so it has no volume.

**Why not Cloudflare Containers.** IronHermes keeps SQLite state and runs
always-on schedulers (cron, kanban, notifier). Cloudflare Containers has an
ephemeral disk, `sleepAfter` shutdowns that would stop those schedulers
(instances sleep after about 10 minutes idle), is amd64-only, and is Beta.
A tunnel in front of a container with a persistent volume avoids all four.

### Sliplane

Sliplane runs the **`-dev-tools` image** (it carries Node, npm and `wrangler`
for site deploys). Deploy it as a service with a volume mounted at `/opt/data`
and set:

| Variable | Value | Why |
|---|---|---|
| `SLIPLANE_USER_ID` | `10000` | Sliplane pre-owns the volume as this UID |
| `SLIPLANE_GROUP_ID` | `10000` | and this GID, matching the image's `ironhermes` user |

Sliplane gives you a container with no custom runtime flags, so what applies
there splits in two:

- **In-image enforcement, applies with no flags:** the entrypoint drops to uid
  10000 with `setpriv`, sets `no_new_privs`, empties the bounding set where the
  runtime allows `CAP_SETPCAP`, and fails closed (the container exits rather
  than running as root).
- **Runtime-only controls:** the read-only root filesystem, the `noexec`
  tmpfs and the pids/memory/cpu limits come from `docker-compose.yml` and
  depend on what Sliplane's runtime accepts. The Sliplane probe has **not been
  run** (see below), so treat them as **not enforced** on Sliplane.

#### Sliplane probe results

D-02 decision: option-a

Sliplane uses the single `-dev-tools` image. There is no `docker/sliplane.Dockerfile`
and no Sliplane variant to keep in step. The operator chose option-a on
2026-11-03 together with deferring the probe and gave no stated reason. For
context only, the plan listed these pros for it: no drift between two
Dockerfiles, the in-image enforcement already applies on Sliplane, and env-only
differences are set in Sliplane's own settings. Revisit the decision if a later
probe shows Sliplane needs a default that its settings cannot express.

**Not probed (deferred 2026-11-03).** Nobody has observed which compose keys
Sliplane's docker-run/Compose import accepts, silently drops or rejects, and
production Sliplane has not been switched to the hardened `-dev-tools` digest or
checked with the in-container probe. The status of each runtime-only control on
Sliplane is therefore **unknown**:

| Compose key | Control | Status on Sliplane |
|---|---|---|
| `read_only` | read-only root filesystem | unknown, residual risk |
| `tmpfs` (`/tmp`, `/var/tmp`, `/dev/shm`, noexec) | noexec scratch space | unknown, residual risk |
| `shm_size` | Chromium shared memory (Docker default 64 MiB) | unknown, residual risk |
| `cap_drop` / `cap_add` | capability set at container start | unknown, residual risk (the in-image bounding-set drop below still applies) |
| `security_opt: no-new-privileges` | runtime `no_new_privs` | unknown, residual risk (the in-image `no_new_privs` below still applies) |
| `pids_limit`, `mem_limit`/`memswap_limit`, `cpus` | fork-bomb, memory and CPU limits | unknown, residual risk |

Treat every row as a residual risk until a probe says otherwise: on Sliplane a
compromised agent may be able to write to the root filesystem, execute from
tmpfs, or exhaust the host's pids, memory or CPU.

**Enforced in-image regardless of what Sliplane applies.** The entrypoint drops
to uid 10000 with `setpriv`, sets `no_new_privs` and empties the bounding set. The
`sliplane_equivalent` case in `scripts/tests/container_hardened_smoke.sh` runs
the `-dev-tools` image with no runtime flags and asserts `APP_UID=10000`,
`APP_NONEWPRIVS=1`, `APP_CAPBND=0000000000000000` and `GET /` 200 (plan
54.11-07 Task 1, passed on podman). This is a local proof; it was not observed
on Sliplane itself.

**Chromium sandbox.** On rootful Linux Docker with Docker's default seccomp
profile and no flags, Chromium's own sandbox crashed (plan 54.11-06 Task 2). Sliplane
runs on Docker-class infrastructure, so keep `IRONHERMES_BROWSER_NO_SANDBOX=1`,
which is already the image default. Do not set it to `0` on Sliplane unless a
probe shows the sandbox works there.

**How to re-probe.** Run this after a Sliplane change, or to replace the
"unknown" rows above with facts:

1. Publish a pre-release per `docs/DOCKER-PUBLISH.md` ("Cutting a release"),
   confirm the docker-publish gates and `cosign verify` on the `-dev-tools`
   digest, and give Sliplane the digest, never a moving tag.
2. On a scratch Sliplane service, paste the `ironhermes` service from
   `docker-compose.yml` (with `image:` set to that digest) into the
   docker-run/Compose import. Note which keys it accepted, dropped or rejected.
3. In the service's exec console run:

   ```sh
   bash /opt/ironhermes/docker/hardening-probe.sh
   grep -E 'NoNewPrivs|CapBnd' /proc/1/status
   touch /usr/local/ih-x
   df -k /dev/shm
   cat /sys/fs/cgroup/pids.max /sys/fs/cgroup/memory.max
   ```

   Then, as uid 10000, run `chromium --headless --dump-dom about:blank` once
   without and once with `--no-sandbox`. Read the app PID values in the probe
   output (`APP_UID`, `APP_NONEWPRIVS`, `APP_CAPBND`, `ROOTFS_RO`,
   `TMP_NOEXEC`, `SHM_SIZE_KB`); PID 1 is tini running as root, so its own
   `CapBnd`/`NoNewPrivs` are not the signal. `ROOTFS_RO` is only `yes` if
   `touch /usr/local/ih-x` fails as root. `max` in `pids.max`/`memory.max` means
   no limit.
4. Switch production to the digest with `SLIPLANE_USER_ID=10000` and
   `SLIPLANE_GROUP_ID=10000`, keeping the existing env, and confirm the `/`
   health check and the same probe output on the production container. Then
   replace the table above with the observed outcomes.


---

## Combined Gateway and Web Update

`scripts/deploy/update.sh` replaces the four-step manual redeploy dance
(`cargo build --release` → `install.sh` → `web-build.sh` → `web-install.sh`)
with a single command. Run it with no arguments to rebuild and redeploy
**both** the gateway binary and the web bundle, restarting both services:

```bash
scripts/deploy/update.sh
```

### Flags

| Flag | Effect |
|---|---|
| (none) | Build, install, and restart both the gateway and the web UI |
| `--gateway-only` | Restrict the run to the gateway. Mutually exclusive with `--web-only` (exit 2) |
| `--web-only` | Restrict the run to the web UI. Mutually exclusive with `--gateway-only` and `--cron` (exit 2) |
| `--skip-build` | Deploy the artifacts already on disk; skip both build steps |
| `--no-start` | Forwarded to both installers (register without starting); also skips the post-restart health probe |
| `--force` | Forwarded to both installers (overwrite existing service registration) |
| `--skip-wasm-check` | Forwarded to `web-build.sh` — skip the wasm32 type-check gate |
| `--cron` | Forwarded to `install.sh` — install the gateway watchdog cron entry instead of a native service. Gateway-only deployment model, so it cannot be combined with `--web-only` |
| `--dry-run` | Print every step that would run, prefixed `[update] DRY-RUN: `, and mutate nothing |
| `-h`, `--help` | Print usage and exit |

### Ordering guarantee

The script runs four strictly ordered phases: **preflight → build → install +
restart → health probe**. Both components are *built* before either one is
*installed*, so a failed build (gateway or web) aborts the whole run before
any installer runs and before any live service is touched — a broken build
can never take down a running service. Within the install + restart phase,
the gateway installs first so `install.sh`'s `ironhermes doctor` sanity check
runs before the web bundle is staged.

`update.sh` performs no restart logic of its own — the actual `systemctl
--user restart` / `launchctl kickstart -k` calls live in the delegated
`install.sh` and `web-install.sh` scripts (see
[Gateway Service Setup](#gateway-service-setup) and
[Web UI Deployment](#web-ui-deployment-iron_hermes_ui) below for what those
restarts do on each platform).

### Health probe

Unless skipped by `--no-start` or `--dry-run`, after the restart phase the
script probes each component it just restarted (`systemctl --user is-active`
on Linux, `launchctl print` on macOS, or a `kill -0` against the recorded pid
for a `--cron` gateway), retrying for up to 10 seconds to absorb normal
startup latency. If any component is not running afterward, the script
prints the matching log-inspection command
(`journalctl --user -u <unit> -n 50` on Linux, `tail -n 50
~/.ironhermes/logs/<name>.err.log` on macOS — except for `gateway`, which
prints `~/.ironhermes/logs/gateway.log`; see
["The gateway log"](#the-gateway-log) above) and exits non-zero — a deploy
that leaves a service down is never reported as a success.

### No debug web build

`update.sh` deliberately does not offer a debug/non-release web build option.
`web-install.sh` always stages `target/dx/iron_hermes_ui/release/web`, so a
debug bundle would be silently ignored in favor of a stale release bundle.

---

## Gateway Service Setup

The Telegram gateway runs as a persistent background process (`ironhermes gateway`). Use the platform-appropriate service manager.

### The gateway log

There is exactly **one** canonical gateway log file: `~/.ironhermes/logs/gateway.log`. `~/.ironhermes/gateway.log` (no `logs/` segment) is a compatibility symlink to it, installed by `scripts/deploy/install.sh`. All three supervision mechanisms — the launchd plist, the systemd `--user` unit, and the cron watchdog — append to the same file. It is created mode `0600` before any supervisor first opens it, because gateway startup errors can quote provider keys. `ironhermes doctor` prints the resolved path and whether it is current or stale.

Every other log-path mention on this page for the gateway refers back to this file; the systemd and launchd subsections below say what they still write elsewhere for the case where that matters.

> **Pitfall: a log that "found nothing" may mean the wrong supervisor.** 48.3 was opened because an operator grepped `~/.ironhermes/gateway.log`, found nothing, and concluded a code path had never fired — when in fact the live gateway was supervised by systemd with no `StandardOutput=` directive, so its output went to `journalctl` instead. Before concluding a code path is silent, confirm which supervisor is actually running the process (`systemctl --user status <unit>` on Linux, `launchctl print gui/$UID/<label>` on macOS) and that its output destination matches the file you're grepping.

### macOS — LaunchAgent

The installer copies the plist template to `~/Library/LaunchAgents/com.ironhermes.gateway.plist` with `__HOME__` substituted. To manage manually:

```bash
# Load and start
launchctl load ~/Library/LaunchAgents/com.ironhermes.gateway.plist

# Stop and unload
launchctl bootout gui/$UID/com.ironhermes.gateway

# View logs — see "The gateway log" above
tail -f ~/.ironhermes/logs/gateway.log
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

# View application output — see "The gateway log" above
tail -f ~/.ironhermes/logs/gateway.log

# View unit lifecycle only (start/stop/restart/crash) — the unit's
# StandardOutput=/StandardError= directives route application output to
# the file above, so the journal no longer carries it for this unit.
journalctl --user -u ironhermes-gateway -f
```

On headless servers with no graphical login session, enable linger so the user service persists after logout:

```bash
loginctl enable-linger $USER
```

The unit restarts automatically (`Restart=always`, `RestartSec=5`), capped at 5 starts per 60-second window.

### Memory cap and agent job scopes (Linux)

Phase 58.1 added a containment cap to both agent-hosting units (`ironhermes-gateway.service` and `ironhermes-web.service`) after a child process OOM took a whole unit down. Both ship:

| Key | Value | Effect |
|-----|-------|--------|
| `MemoryHigh` | `6G` | Kernel throttles the unit's cgroup before it reaches the hard limit |
| `MemoryMax` | `8G` | Hard limit; the kernel OOM-kills inside the unit's cgroup |
| `OOMPolicy` | `continue` | An OOM-killed child no longer fails the whole unit |
| `KillMode` | `control-group` | On stop or crash, every process left in the unit's cgroup is killed (setsid escapees included) |

The cap is only sensible because agent workloads that outlive a turn do not run inside the unit's cgroup: the gateway itself is about 137 MB, so 8G means "gateway bug", not "a build job is running". Long-running workloads live in their own scopes (below) and are not counted against, or killed with, the unit.

**Tuning.** `install.sh` / `update.sh` / `web-install.sh` overwrite the installed unit, so put overrides in a drop-in, which survives reinstall:

```bash
systemctl --user edit ironhermes-gateway   # or: ironhermes-web
# in the editor:
# [Service]
# MemoryHigh=12G
# MemoryMax=16G
```

**Requirements.** The limits are enforced by cgroup v2 with the memory controller delegated to the user manager, and job scopes live under the user manager, which dies with your last session unless linger is on:

```bash
stat -fc %T /sys/fs/cgroup        # must print: cgroup2fs
loginctl enable-linger $USER
systemctl --user show ironhermes-gateway -p MemoryMax -p MemoryHigh -p OOMPolicy -p KillMode
```

On a host that fails either requirement the keys are accepted but the cap may be a silent no-op. Confirm that the shown values match the table above and that `memory.max` in the unit's cgroup is non-`max`; this document does not claim the cap has been verified on any particular host.

**Agent job scopes.** A terminal command that detaches (`nohup`, `setsid`, `disown`, or a trailing `&`) runs in `ironhermes-job-<id>.scope` under `ironhermes-jobs.slice`. It survives gateway restarts. List and stop:

```bash
systemctl --user list-units 'ironhermes-job-*'
systemctl --user stop ironhermes-job-<id>.scope
```

Limits of the scope placement:

- Detection is a textual match on the command, not a shell parser. A false positive (for example `grep -r nohup .`) still runs in a scope; if that command later hits its timeout, only the foreground shell is killed and anything it started stays in the scope until you stop it (the tool output says so).
- `background=true` launches go through the process registry and are **not** scoped. They stay in the gateway's cgroup, count against its memory cap, and die with the gateway on restart. Use `nohup`/`setsid` (with output redirected to a file) for anything that must outlive the gateway.
- If `systemd-run` fails its availability probe, scoping is skipped and the probe is retried after 60 seconds, so a transient failure does not disable it until restart.

**Optional aggregate cap for all jobs.** To bound the total memory of every job scope together:

```bash
systemctl --user set-property ironhermes-jobs.slice MemoryMax=<size>
```

This is unverified on every host (research assumption A4): check `systemctl --user show ironhermes-jobs.slice -p MemoryMax` after setting it.

**Kill switch.** Set `IRONHERMES_JOB_SCOPE=off` in `~/.ironhermes/.env` and restart the service to restore the pre-58.1 launch path (no scopes; detached workloads run as children of the unit and are subject to its cap and `KillMode`).

**macOS and containers.** There are no scopes and no memory cap: launchd has no equivalent of a systemd memory hard limit or transient scopes, and a container runtime applies its own limits. Behaviour there is exactly as before 58.1.

### Cron Watchdog (Fallback)

For systems without launchd or systemd, a cron-driven watchdog checks the gateway PID every minute and relaunches if it has died:

```bash
# Add to crontab
(crontab -l 2>/dev/null; echo "* * * * * $HOME/.ironhermes/scripts/gateway-watchdog.sh >/dev/null 2>&1 # ironhermes-gateway-watchdog") | crontab -
```

The watchdog reads `~/.ironhermes/gateway.pid`, probes with `kill -0`, and re-launches via `gateway-run.sh` if the process is gone. Its own log lines, and everything the relaunched gateway prints, are appended to the same canonical file described in "The gateway log" above — created mode `0600` before the watchdog's first append.

---

## Web UI Deployment (`iron_hermes_ui`)

### Security — read this first

Authentication is **opt-in by configuration**. With no `web_ui.auth.password_hash` set, behavior is identical to the pre-47.3 posture: no login, loopback-only bind is the operating assumption, and `security.web_config_write_enabled` / `vault.enabled` remain feature toggles (gating *what* the server will do), not authentication (gating *who* is allowed to ask it). Configure a hash and the boundary flips on: every request — server functions, the chat WebSocket, `/artifacts/{id}`, `/chat-attachments/{session_id}/{id}` — requires a valid session cookie, on every bind address.

**A non-loopback bind with no configured hash now refuses to start.** This is a hard startup check in the binary itself (`bind_guard_allows` in `main.rs`, called before `TcpListener::bind`), not only a shell-script warning — so launching via systemd, a LaunchAgent, or the binary directly is covered too, not just `scripts/deploy/web-run.sh`. The refusal message:

> refusing to bind `<address>`: this is a non-loopback address and no `web_ui.auth.password_hash` is configured. Set one via `ironhermes web set-password` (paste the printed hash into `web_ui.auth.password_hash` in config.yaml), or bind to a loopback address (127.0.0.1 / ::1) instead.

A second, independent startup guard covers origin protection rather than credentials — see [Cross-site origin allowlist (`web_ui.allowed_origins`)](#cross-site-origin-allowlist-web_uiallowed_origins) below.

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
| `cookie_secure` | `false` | Adds the `Secure` cookie attribute — **set `true` whenever TLS terminates in front of this server**, including at a reverse proxy or load balancer (see below) |
| `session_ttl_hours` | `168` (7 days) | Absolute session lifetime from creation |
| `idle_timeout_hours` | `24` | Sliding idle timeout, refreshed on authenticated requests |

**Operational facts:**

- Sessions are held **in-memory**, so every server restart forces a re-login by design — this is intentional, not a bug to report.
- Logout (`POST /auth/logout`, the `/logout` command-palette item) is a **true revocation** — the session token is deleted server-side, not just expired client-side.
- Lost password: run `ironhermes web set-password` again and restart with the new hash; there is no recovery flow.
- Single operator only — no roles, no multi-user, no OIDC.

**What is still true and must not be softened:** there is no built-in TLS. On plain-LAN HTTP the login password and the session cookie are sniffable by anything on the same network segment, and `cookie_secure` should stay `false` until a reverse proxy or load balancer is terminating TLS in front of this server — once one is, `cookie_secure` should be set `true` to protect the session cookie. Tailscale (or an equivalent WireGuard overlay) is the assumed transport-confidentiality layer for LAN/tailnet use — it encrypts the link itself, which is why plain `cookie_secure: false` HTTP is acceptable there but not on an open LAN. A reverse proxy is still the recommendation for any public-internet posture; it is no longer a hard requirement for LAN/tailnet use now that the server enforces its own fail-closed bind guard. Leaving `cookie_secure` at `false` behind a TLS-terminating proxy is what produced Phase 49.1.1's WebSocket outage — the flag was never the blocker the earlier guidance implied.

Because of the bind guard above, every script below still defaults to a loopback (`127.0.0.1`) bind and refuses any other address unless you explicitly set `IRONHERMES_WEB_ALLOW_PUBLIC_BIND=1` — that shell-script gate is now an earlier, friendlier warning layered in front of the binary's own real guarantee, not the only line of defense.

### Cross-site origin allowlist (`web_ui.allowed_origins`)

`web_ui.allowed_origins` (config.yaml, a **sibling** of `web_ui.auth` on the `web_ui` block — deliberately not nested under `web_ui.auth`) is the deployment's public identity, which authentication merely consumes. It is also the intended future anchor for the OAuth callback's own origin resolution. It controls which browser `Origin` values are accepted on the chat and kanban WebSocket upgrades.

```yaml
web_ui:
  allowed_origins: ["https://iron-hermes.sliplane.app"]
```

**Env override:** `IRONHERMES_WEB_ALLOWED_ORIGINS`, comma-separated for multiple origins. Set it in `~/.ironhermes/.env` (loaded at startup) or via a container/dashboard environment variable — the same two mechanisms already used for `IRONHERMES_WEB_PASSWORD_HASH`, for platforms that expose only env vars.

**Precedence:** `config.yaml` wins whenever `web_ui.allowed_origins` is present and non-empty; the env var applies only when it is absent or empty. This is the same layering `web_ui.auth.password_hash` already uses (`config.yaml` → env → vault), so the two keys behave alike. **A consequence worth knowing during an incident:** if a deployment has a non-empty `allowed_origins` in `config.yaml`, setting `IRONHERMES_WEB_ALLOWED_ORIGINS` will *not* override it — edit or clear the `config.yaml` value instead.

```bash
IRONHERMES_WEB_ALLOWED_ORIGINS='https://iron-hermes.sliplane.app'
```

**When configured:** the incoming `Origin` header on a WebSocket upgrade to `/api/ws/chat` or `/api/ws/kanban` is compared against the list, and the request's own `Host` header is ignored entirely.

**When unset (the default):** the expected origin is still derived from `Host`, with the scheme taken from the first comma-separated `X-Forwarded-Proto` value when the proxy supplies one, falling back to `web_ui.auth.cookie_secure` otherwise. This keeps loopback and LAN deployments zero-config.

**A WebSocket upgrade with no `Origin` header is rejected on both of those paths, in every configuration.** This reverses the previous rule, but browsers always send `Origin` on an upgrade request, so no real client is affected.

**The `["*"]` wildcard** is the typed escape hatch for a trusted LAN:

```yaml
web_ui:
  allowed_origins: ["*"]
```

This is **not** "disable the check": any *present* origin is accepted, but a *missing* one is still rejected. That makes it weaker than an explicit allowlist and strictly stronger than turning origin checking off.

**A non-loopback bind with no configured origins refuses to start**, the same way a non-loopback bind with no `web_ui.auth.password_hash` already does (see above). The refusal message:

> FATAL: Refusing to bind non-loopback address 0.0.0.0 without origin protection.
> To secure this deployment, configure `web_ui.allowed_origins` with your public URL(s).
> Example (config.yaml):
>   web_ui:
>     allowed_origins: ["https://iron-hermes.sliplane.app"]
>
> Or set IRONHERMES_WEB_ALLOWED_ORIGINS='https://iron-hermes.sliplane.app'
> (comma-separated for multiple) — use this on platforms that only expose env vars.
>
> Alternatively, to accept any browser origin on a trusted LAN, use:
>   web_ui:
>     allowed_origins: ["*"]

This is a second, independent startup guard from the credential bind guard above — the two exist for two distinct causes with two distinct messages and are not merged.

**Validation:** each configured entry must be a bare `scheme://host[:port]` origin with no path, query, fragment, or userinfo. A malformed entry is a hard startup error. Entries are checked by `ironhermes_mcp::security::validate_web_redirect_base` — the same validator the MCP redirect-origin key below already uses, so there is one rule for both keys.

### MCP OAuth redirect origin (`mcp_oauth.web_redirect_base_url`)

`mcp_oauth.web_redirect_base_url` (config.yaml, sibling to `mcp_oauth.issuer_allowlist`) is the **public origin** an MCP authorization server redirects the browser back to when the web UI drives an MCP CONNECT authorization (`McpManager::begin_oauth`/`complete_oauth`). It is combined with the fixed `/oauth/mcp/callback` path to build the `redirect_uri` argument passed into that flow — an operator never sets the callback path itself, only the origin it hangs off of.

**When it is required:** any deployment where the browser's own origin differs from what the process can infer from the address it binds to — a reverse proxy that rewrites `Host`, a TLS terminator in front of a plain-HTTP backend, or a container/VPS published on a different hostname or port than the bind address above. Set it to the exact origin operators' browsers see, e.g. `https://hermes.example.com`.

**When it is unset (the default):** the browser's own request origin is used to build the redirect URI instead, subject to the same validation rules below. This is correct for the common case (no reverse proxy, or a proxy that preserves `Host` faithfully) and requires no configuration.

**Validation:** the value is checked by `ironhermes_mcp::security::validate_web_redirect_base` — a pure function also callable directly from `iron_hermes_ui` — before it is used for anything. It must be an absolute `http` or `https` origin with **no** userinfo, path beyond a bare `/`, query string, or fragment (and no longer than 255 bytes). A malformed value is rejected with a fixed error message that never echoes the input back, matching this crate's existing credential-handling discipline (see `security::sanitize_error` above). This key is orthogonal to the `web_ui.auth.*` table above — it does not gate login, only where an MCP authorization server is told to send its browser redirect.

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
| `OPENROUTER_API_KEY` (or `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`, matching `model.provider`) | LLM provider — required for the main provider (see `providers.<name>.api_key_env` to point at a differently-named variable). `iron_hermes_ui` fails closed on this: with no resolvable key and a non-loopback provider `base_url`, the web server exits non-zero at startup instead of coming up and burning a retry storm on the first turn. |
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
- **Gateway log** — `~/.ironhermes/logs/gateway.log` (see ["The gateway log"](#the-gateway-log) above); `gateway.out.log` and `gateway.err.log` are pre-48.3 files that no longer receive writes — if you still have them on disk, that's why they stopped growing
- **systemd journal** — `journalctl --user -u ironhermes-gateway -f` (Linux; unit lifecycle only as of 48.3, not application output)
- **PID file** — `~/.ironhermes/gateway.pid` (3-line YAML; readable by the watchdog and external health checks)

<!-- VERIFY: any external monitoring or alerting integration beyond file-based logs -->
