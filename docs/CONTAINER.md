# Container Guide — Podman Build & Run

Step-by-step instructions for building and running the IronHermes container
image with **Podman** (recommended) or Docker. The shipped `Dockerfile` is a
multi-stage OCI build, so the same file works with both engines.

> **Quick reference — first run is two steps.** Build, start once to get a
> generated password, then restart with `-e IP=0.0.0.0` to expose it:
>
> ```bash
> podman build -t ironhermes .
> podman run -d --name ironhermes -v ironhermes-data:/opt/data -p 8080:8080 -e OPENROUTER_API_KEY=sk-or-... ironhermes
> podman logs ironhermes        # copy the FIRST-RUN WEB PASSWORD
> podman rm -f ironhermes
> podman run -d --name ironhermes -v ironhermes-data:/opt/data -p 8080:8080 -e IP=0.0.0.0 -e OPENROUTER_API_KEY=sk-or-... ironhermes
> ```
>
> The first start binds loopback **inside** the container on purpose, so a
> freshly generated password is never reachable from the network. See
> [§3 First run](#first-run-generate-a-password-then-expose). The sections
> below explain each step, the prerequisites, rootless notes, and what to do
> when something fails.

---

## 1. Prerequisites

| Requirement | Version | Notes |
|---|---|---|
| Podman | ≥ 4.x (validated on 4.9.3, rootless) | Rootless mode works — no daemon or root needed |
| Docker | any recent (24+) | Drop-in alternative; swap `podman` → `docker` in every command |
| Disk space | ~10 GB free during build | The `rust:1.96-bookworm` builder stage is ~1.6 GB and the Rust release build produces a large intermediate layer; the **final image is ~231 MB** |
| Build context | the repo root | `skills/` and `assets/` must be present — see §2 |

No other host dependencies: the image builds its own toolchain and the runtime
stage only needs a container engine.

**Rootless Podman.** No special setup is required — all commands below work
rootless. The entrypoint's `chown` of `/opt/data` may warn
(`Warning: chown failed (rootless container?) — continuing anyway`); this is
expected and harmless, since named volumes in rootless Podman are already
owned by your mapped UID.

## 2. Build the image

From the **repository root** (the directory containing `Cargo.toml`):

```bash
podman build -t ironhermes .
#   equivalently: docker build -t ironhermes .
```

The build takes roughly **10–25 minutes** on a warm machine (Rust release
profile; dependency layers are cached on rebuilds).

### What the build does

Three stages (see `docs/DEPLOYMENT.md` §Container Deployment for the full
reference):

1. **`gosu_source`** — fetches `tianon/gosu:1.17` for privilege dropping.
2. **`builder`** — `rust:1.96-bookworm`; installs ALSA/OpenSSL build deps and
   compiles `cargo build --release --bin ironhermes`.
3. **`runtime`** — `debian:bookworm-slim` + `python3`, `ca-certificates`,
   `procps`, `libasound2`; runs as UID 10000.

### Build-context gotchas

- **`skills/` and `assets/` are required in the build context.** Two crates
  embed files from these directories at compile time via `include_str!`
  (`ironhermes-kanban` embeds the kanban SKILL.md files; `iron_hermes_ui`
  embeds `assets/site.css`). If you build from a tarball or a minimal checkout
  that omits them, the release build **fails at compile time**. Building from
  the repo root with the shipped `.dockerignore` is always correct.
- **`.dockerignore` excludes secrets** (`.env`, `*.pem`, `*.key`) and
  `target/` — never remove those exclusions.
- The build needs network access to pull base images and crates from
  `docker.io` / `crates.io`. On a host that requires a proxy or a registry
  mirror, configure `containers-registries.conf` (Podman) or the Docker
  daemon mirror before building.

### Verify the build

```bash
podman images localhost/ironhermes
# REPOSITORY              TAG    SIZE
# localhost/ironhermes    latest ~231 MB

podman run --rm ironhermes --help      # exercises the entrypoint, exits 0
podman run --rm ironhermes version     # prints the version, exits 0
```

`--help`/`version` run the real entrypoint first: it starts as root, seeds
config templates into `/opt/data` (an anonymous volume here, discarded with
`--rm`), drops to UID 10000 via `gosu`, then execs the binary. Exit code 0
from both means the image is healthy.

## 3. Run the container

### Minimal: CLI chat inside the container

```bash
podman run --rm -it \
  -e OPENROUTER_API_KEY=sk-or-... \
  ironhermes chat
```

(No volume → nothing persists; quit and everything is gone.)

### Standard: persistent data + gateway port

```bash
podman run -d \
  --name ironhermes \
  -v ironhermes-data:/opt/data \
  -p 8080:8080 \
  --env-file ~/.ironhermes/.env \
  ironhermes gateway
```

- **`-v ironhermes-data:/opt/data`** — named volume mounted at the container's
  `IRONHERMES_HOME`. Everything that matters lives here: sessions, memories,
  cron state, logs, `config.yaml`, and the seeded `.env`. **Always mount a
  volume** for anything but a throwaway smoke test.
- **`-p 8080:8080`** — gateway HTTP endpoint (only needed if you run
  `gateway`; the `EXPOSE 8080` in the Dockerfile publishes nothing by itself).
- **`--env-file`** — the easiest way to pass provider keys
  (`OPENROUTER_API_KEY`, `TELEGRAM_BOT_TOKEN`, …). Alternatively use
  `-e KEY=value` per variable. The entrypoint also seeds `/opt/data/.env`
  from the built-in template on first run and the binary reads it, so keys
  baked into the volume persist across recreations.
- **Command** — anything after the image name is passed to the `ironhermes`
  binary (`gateway`, `chat`, `cron list`, …). With no command you get the
  default CLI behavior; `podman run --rm ironhermes --help` lists them all.

### First run: generate a password, then expose

The image binds **`127.0.0.1` by default** (`ENV IP=127.0.0.1`). Exposing the
UI on the network is always something you ask for explicitly, never something
that happens on your behalf.

**Step 1 — start it once.** No `IP` override:

```bash
podman run -d --name ironhermes \
  -v ironhermes-data:/opt/data -p 8080:8080 \
  -e OPENROUTER_API_KEY=sk-or-... \
  ironhermes
```

With no web password hash configured, the container generates a random one,
stores its argon2id hash in `/opt/data/config.yaml`, and prints the plaintext
**once**:

```
============================================
 FIRST-RUN WEB PASSWORD (shown once)

   k7mQ-2xVn-8pLd-Rw3f

 Stored as an argon2id hash in
 /opt/data/config.yaml. Change it with:
   ironhermes web set-password
============================================
```

**Step 2 — read it.** `podman logs ironhermes`. Copy the password now; it is
never printed again, and only the hash is stored.

> **The UI is not reachable yet, and that is deliberate.** A listener bound to
> `127.0.0.1` *inside* a container ignores `-p 8080:8080` — port publishing
> forwards to the container's external interface, which nothing is listening
> on. So `curl http://127.0.0.1:8080/` on your host will fail at this stage.
> This is what keeps a freshly generated credential off the network.

**Step 3 — expose it, explicitly.** Recreate with `-e IP=0.0.0.0`:

```bash
podman rm -f ironhermes
podman run -d --name ironhermes \
  -v ironhermes-data:/opt/data -p 8080:8080 \
  -e IP=0.0.0.0 \
  -e OPENROUTER_API_KEY=sk-or-... \
  ironhermes
```

The hash now lives in the volume's `config.yaml`, so the bind guard is
satisfied and the server comes up on `0.0.0.0:8080`. Log in with the password
from step 2, then change it: `ironhermes web set-password`.

#### Bringing your own password instead

To skip generation entirely, mint a hash up front and pass it in — the image
already contains the CLI, so no separate install is needed:

```bash
podman run --rm -it --entrypoint ironhermes ironhermes web set-password
```

Then pass `-e IRONHERMES_WEB_PASSWORD_HASH='<hash>'` on the run command
(**single-quote it** — argon2id strings contain `$`, which your shell will
otherwise expand). When that variable is set, nothing is generated and nothing
is printed, and you can go straight to `-e IP=0.0.0.0` on the first run.

#### Asking for exposure without a password still fails

Passing `-e IP=0.0.0.0` with no hash configured — not in the environment, not
in `config.yaml` — is a **hard refusal**, not a silent downgrade to loopback.
You asked to be reachable; being quietly bound to loopback instead would leave
you believing you were exposed when you were not.

### What the entrypoint does on first run

1. Starts as root; optionally remaps UID/GID (see §4).
2. `chown`s `/opt/data` to the `ironhermes` user (skipped silently in rootless).
3. Drops privileges to UID 10000 via `gosu` (never runs the agent as root).
4. Creates `cron/ sessions/ logs/ hooks/ memories/ skills/ workspace/` under
   `/opt/data`.
5. Seeds `.env`, `config.yaml`, and `SOUL.md` from templates **only if they
   don't already exist** — your edits survive container recreation.
6. `chmod 600` on `.env`, then `exec ironhermes <your args>`.

### The background gateway (quick task 260825-dww)

The image runs **two** processes. `iron_hermes_ui` is the foreground process
— container health, restarts, and the `podman stop` lifecycle all follow it
alone, unchanged from before. Alongside it, the entrypoint also launches
`ironhermes gateway --non-interactive` in the background, best-effort: it
hosts the cron / kanban / notifier schedule loops the UI server does not run
itself.

Its combined stdout+stderr goes to `/opt/data/logs/gateway.log` (mode `600`,
on the persisted volume). It refuses to boot with **zero configured
messaging platforms** — the normal state of a fresh container — and when
that happens the entrypoint prints a loud warning and the UI keeps serving
regardless; the container never dies because the gateway didn't start.

Set `-e IRONHERMES_GATEWAY=0` to opt out and run the web UI alone.

When schedules aren't firing, this is the one-liner you need:

```bash
podman exec ironhermes cat /opt/data/logs/gateway.log
```

Two operational facts worth knowing before you hit them the hard way:

- **Shared state.** Both processes read and write the same `/opt/data`,
  including the same SQLite state DB. Concurrent writes rely on SQLite's own
  locking and a busy-timeout retry — this is a known constraint of the
  current design, not a supported multi-writer architecture.
- **No graceful gateway shutdown.** `podman stop` signals PID 1 (the UI)
  only. The gateway is a background child, so it is torn down by PID
  namespace teardown when the container exits, not by a SIGTERM it can
  react to. It can never survive the container or be orphaned onto the
  host, but it also never shuts down cleanly — which is why the entrypoint
  clears a leftover `gateway.pid` on the *next* container start rather than
  relying on the gateway's own cleanup.

### Lifecycle commands

```bash
podman logs -f ironhermes        # follow logs
podman exec -it ironhermes ironhermes status   # poke a running container
podman restart ironhermes        # restart (data persists in the volume)
podman stop ironhermes && podman rm ironhermes   # recreate from scratch
```

The volume survives `podman rm`; delete it explicitly with
`podman volume rm ironhermes-data` only if you want a full reset.

### Run as a systemd user service (rootless auto-start)

Podman can generate a user unit so the container starts at login and restarts
on failure:

```bash
mkdir -p ~/.config/systemd/user
podman generate systemd --new --name ironhermes \
  > ~/.config/systemd/user/ironhermes.service
systemctl --user daemon-reload
systemctl --user enable --now ironhermes
loginctl enable-linger "$USER"   # start without an active login session
```

## 4. Environment variables

### Entrypoint-level (container behavior)

| Variable | Default | Purpose |
|---|---|---|
| `IRONHERMES_HOME` | `/opt/data` | Data directory inside the container |
| `IRONHERMES_UID` | `10000` | Remap the runtime UID (useful when bind-mounting a host directory so ownership matches your host user) |
| `IRONHERMES_GID` | `10000` | Remap the runtime GID |
| `IRONHERMES_GATEWAY` | `1` | Launch `ironhermes gateway` in the background alongside the web UI. Set to `0`/`false`/`no`/`off` to run the web UI alone. |

Example — bind-mount a host directory you own instead of a named volume:

```bash
podman run -d --name ironhermes \
  -v ~/ironhermes-data:/opt/data:Z \
  -e IRONHERMES_UID=$(id -u) -e IRONHERMES_GID=$(id -g) \
  ironhermes gateway
```

(`:Z` relabels for SELinux hosts; drop it where SELinux isn't enforcing.)

### Application-level (agent behavior)

Provider keys and platform tokens — most commonly:

- `OPENROUTER_API_KEY` — **required** for the web image. `model.provider`
  defaults to `openrouter`, and `iron_hermes_ui` has a fail-closed
  provider-key startup guard: if the main provider has no resolvable API key
  (and its `base_url` isn't a loopback local model server), the server exits
  non-zero within seconds instead of coming up and burning a retry storm on
  the first turn. If you've pointed `model.provider` at a different
  provider, set `providers.<that-provider>.api_key_env` in `config.yaml` to
  name the env var it should read instead.
- **No fallback provider is configured by default.** The seeded `config.yaml`
  is API-key-only: `providers.openrouter.fallback_providers` ships commented
  out, as does the example `ollama:` block. This is deliberate — a fallback
  naming a host that isn't running turns a clear auth error ("your key is
  invalid") into a misleading connection-refused against a dead port, which
  sends diagnosis in the wrong direction. Enable a fallback only if it
  genuinely runs somewhere this container can reach; uncomment **both** the
  `fallback_providers` line and the provider block it names.

  Note this applies to **newly seeded** volumes. Step 5 of §"First run" seeds
  `config.yaml` only when absent, so a server provisioned before this change
  keeps its existing file with the old live `fallback_providers: ["ollama"]`.
  To update one in place: `podman exec -it ironhermes sh`, comment out that
  line in `/opt/data/config.yaml`, and restart — or delete the file and let
  the entrypoint re-seed it.

- `TELEGRAM_BOT_TOKEN` — Telegram gateway listener
- plus tool API keys; see `env.example` (shipped in the repo) and
  `docs/CONFIGURATION.md` for the full reference.

Precedence: real environment (`-e` / `--env-file`) beats the seeded
`/opt/data/.env`. Keys baked into the volume's `.env` persist across
container recreations; keys passed with `-e` do not (unless you pass them
again).

## 5. Volumes and ports

| Mount | Why |
|---|---|
| `ironhermes-data:/opt/data` (named volume, **recommended**) | Persists sessions, memories, cron state, logs, seeded config. Managed by Podman; correct ownership in rootless mode out of the box. |
| `/host/path:/opt/data:Z` (bind mount) | Direct host access to the data files. Pair with `IRONHERMES_UID/GID=$(id -u)/$(id -g)` so file ownership maps to your user. |

| Port | Why |
|---|---|
| `8080` | The `iron_hermes_ui` web server, bound per the `IP`/`PORT` env (default image `ENTRYPOINT`). Publish it (`-p 8080:8080`) to reach the UI. The background gateway (see §3) does not itself bind this port. |

## 6. Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `error: failed to load manifest for workspace member ...` during build | Build context is missing a crate manifest or was built from a partial checkout. Build from the repo root with the shipped `.dockerignore`. |
| `include_str!` / "No such file or directory" compile error mentioning `skills/` or `assets/` | Those directories are embedded at compile time — restore them to the build context (see §2). |
| `alsa-sys` / `pkg-config` error | Out-of-date Dockerfile predating the ALSA deps — rebuild from current `main`. |
| `Warning: chown failed (rootless container?)` on start | Normal under rootless Podman; the named volume is already owned correctly. Harmless. |
| Permission denied writing to `/opt/data` with a **bind mount** | UID mismatch — pass `-e IRONHERMES_UID=$(id -u) -e IRONHERMES_GID=$(id -g)` (§4). |
| Container starts but the LLM calls fail with auth errors | No provider key in scope. Pass `--env-file`/`-e`, or `podman exec -it ironhermes sh` and edit `/opt/data/.env` (it persists). |
| `podman build` fails pulling base images | Registry mirror/proxy needed — configure `/etc/containers/registries.conf` or `~/.config/containers/registries.conf`. |
| Nothing listens on 8080 | `iron_hermes_ui` has two fail-closed startup guards (see the Dockerfile header) — the bind guard (no password hash configured for a non-loopback `IP`) and the provider-key guard (no resolvable API key for `model.provider`). Either refuses the socket before it opens. Check `podman logs ironhermes` for which guard tripped. |
| Cron / kanban schedules never fire | The background gateway refused to boot — almost always because zero messaging platforms are configured (§3). Check `podman exec ironhermes cat /opt/data/logs/gateway.log` for the exact reason, configure a platform (e.g. `TELEGRAM_BOT_TOKEN`), or accept it and silence the warning with `-e IRONHERMES_GATEWAY=0`. |

Rebuilding after pulling new code is just `podman build -t ironhermes .`
again — cargo's dependency layers are cached, so only changed crates
recompile.

---

*Validated with Podman 4.9.3 (rootless, OCI format) producing a 231 MB image;
`podman run --rm ironhermes --help` / `version` smoke tests exit 0, and a
named-volume run seeds `/opt/data` with `config.yaml`, mode-600 `.env`,
`SOUL.md`, and the data subdirectories as the `ironhermes` user (UID 10000).*
