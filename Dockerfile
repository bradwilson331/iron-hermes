# =============================================================================
# IronHermes — Multi-stage OCI Build (Podman / Docker compatible)
# =============================================================================
# Builds the iron_hermes_ui Dioxus 0.7 fullstack app (WASM client + embedded
# agent server). The `ironhermes` CLI is bundled alongside for management
# (e.g. `ironhermes web set-password`).
#
# NOTE: iron_hermes_ui has TWO fail-closed startup guards. Both exit non-zero
# before the listening socket opens:
#   1. Bind guard — a password is generated automatically on first start
#      unless you supply one, and the container binds loopback until you opt
#      into exposure. A non-loopback bind (-e IP=0.0.0.0) still refuses
#      unless a web password hash is configured (IRONHERMES_WEB_PASSWORD_HASH
#      env, or config.yaml web_ui.auth.password_hash).
#   2. Provider-key guard — refuses to start if the main LLM provider
#      (model.provider in config.yaml; OPENROUTER_API_KEY / OPENROUTER by
#      default) has no resolvable API key. Exempt for a loopback base_url
#      (local Ollama / vLLM), so keyless local dev is unaffected.
#
# RUNTIME PROCESSES (quick task 260825-dww): the container runs TWO processes.
#   - `iron_hermes_ui` in the foreground as PID 1. Container health and
#     lifecycle are tied to this process alone, unchanged from before.
#   - `ironhermes gateway --non-interactive` in the background, best-effort,
#     hosting the cron / kanban / notifier scheduler loops the UI server does
#     not. It logs to $IRONHERMES_HOME/logs/gateway.log. The gateway refuses
#     to boot when no messaging platform is configured (the normal first-run
#     state); when that happens the entrypoint warns loudly and keeps serving
#     the UI — it never takes the container down.
#   Opt out with -e IRONHERMES_GATEWAY=0.
#
# Build: podman build -t ironhermes .
#
# Run (two-step flow — see docs/CONTAINER.md for the full walkthrough):
#   # 1. First start. No IP override: binds loopback, mints a password,
#   #    prints it once to the container log.
#   podman run -d --name ironhermes -v ironhermes-data:/opt/data -p 8080:8080 \
#       -e OPENROUTER_API_KEY=sk-or-... ironhermes
#   podman logs ironhermes   # read the password now — never printed again
#
#   # 2. Expose it, explicitly (a recreate, since -e can't be added to a
#   #    running container). The named volume carries the hash across.
#   podman rm -f ironhermes
#   podman run -d --name ironhermes -v ironhermes-data:/opt/data -p 8080:8080 \
#       -e IP=0.0.0.0 -e OPENROUTER_API_KEY=sk-or-... ironhermes
#
# BREAKING CHANGE (quick task 260820-8h5): this image's default bind address
# changed from 0.0.0.0 to loopback. Any existing deployment that relied on
# the old wildcard default — notably the Hermes-AaaS VPS stack, where a
# separate Caddy container reaches hermes:8080 — must add -e IP=0.0.0.0.
# Once IP is passed, behavior is identical to before.
# =============================================================================

# --- Stage 0: gosu for privilege dropping ---
FROM docker.io/tianon/gosu:1.17 AS gosu_source

# --- Stage 1: Rust + Dioxus build ---
# Edition 2024 requires rustc >= 1.85; pin for reproducibility.
# Base is Debian *trixie* (glibc 2.41), NOT bookworm (glibc 2.36): the prebuilt
# `dx` binary that binstall fetches below is linked against glibc 2.39 (Dioxus
# builds its release binaries on Ubuntu 24.04). On bookworm `dx` fails at
# startup with "GLIBC_2.38/2.39 not found". Runtime stage must match (trixie).
FROM docker.io/library/rust:1.96-trixie AS builder
WORKDIR /build

# Build-time system deps:
# - pkg-config + libasound2-dev: ALSA, required by cpal/rodio (audio in ironhermes-tools)
# - libssl-dev + perl: openssl-sys / vendored openssl build (arrow/duckdb TLS deps)
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        pkg-config \
        libasound2-dev \
        libssl-dev \
        perl && \
    rm -rf /var/lib/apt/lists/*

# WASM target for the Dioxus web client.
RUN rustup target add wasm32-unknown-unknown

# Dioxus CLI — MUST match the dioxus crate version (=0.7.1). Install ONLY the
# prebuilt binary via binstall; do NOT fall back to `cargo install dioxus-cli`
# (the from-source build regularly OOM-crashes the builder). Fail hard if
# binstall can't fetch the pin. The prebuilt `dx` needs glibc 2.39 — see the
# trixie base-image note above.
RUN cargo install cargo-binstall --locked && \
    cargo binstall -y dioxus-cli@0.7.1

# Copy dependency manifests first for layer caching.
COPY Cargo.toml Cargo.lock ./

# Copy every workspace member manifest (members list from root Cargo.toml).
# cargo needs the full manifest set to resolve the workspace.
COPY crates/ironhermes-core/Cargo.toml crates/ironhermes-core/Cargo.toml
COPY crates/ironhermes-state/Cargo.toml crates/ironhermes-state/Cargo.toml
COPY crates/ironhermes-trajectory/Cargo.toml crates/ironhermes-trajectory/Cargo.toml
COPY crates/ironhermes-tools/Cargo.toml crates/ironhermes-tools/Cargo.toml
COPY crates/ironhermes-agent/Cargo.toml crates/ironhermes-agent/Cargo.toml
COPY crates/ironhermes-cli/Cargo.toml crates/ironhermes-cli/Cargo.toml
COPY crates/ironhermes-gateway/Cargo.toml crates/ironhermes-gateway/Cargo.toml
COPY crates/ironhermes-cron/Cargo.toml crates/ironhermes-cron/Cargo.toml
COPY crates/ironhermes-cron-runner/Cargo.toml crates/ironhermes-cron-runner/Cargo.toml
COPY crates/ironhermes-hooks/Cargo.toml crates/ironhermes-hooks/Cargo.toml
COPY crates/ironhermes-exec/Cargo.toml crates/ironhermes-exec/Cargo.toml
COPY crates/ironhermes-hub/Cargo.toml crates/ironhermes-hub/Cargo.toml
COPY crates/ironhermes-kanban/Cargo.toml crates/ironhermes-kanban/Cargo.toml
COPY crates/ironhermes-mcp/Cargo.toml crates/ironhermes-mcp/Cargo.toml
COPY crates/ironhermes-artifacts/Cargo.toml crates/ironhermes-artifacts/Cargo.toml
COPY crates/iron_hermes_ui/Cargo.toml crates/iron_hermes_ui/Cargo.toml
COPY crates/ironhermes-blackbox/Cargo.toml crates/ironhermes-blackbox/Cargo.toml
COPY crates/ironhermes-vault/Cargo.toml crates/ironhermes-vault/Cargo.toml
COPY providers/memory-sqlite/Cargo.toml providers/memory-sqlite/Cargo.toml
COPY providers/memory-grafeo/Cargo.toml providers/memory-grafeo/Cargo.toml
COPY providers/memory-duckdb/Cargo.toml providers/memory-duckdb/Cargo.toml

# Copy full source
COPY crates/ crates/
COPY providers/ providers/
# Embedded at compile time via include_str! from crate sources:
# - skills/: kanban-worker & kanban-orchestrator SKILL.md (ironhermes-kanban)
#   (iron_hermes_ui's assets live inside crates/iron_hermes_ui/assets/,
#    already covered by `COPY crates/ crates/` — the repo has no root assets/)
COPY skills/ skills/

# CLI binary (management + `ironhermes web set-password`).
RUN cargo build --release --bin ironhermes

# Fullstack web bundle: WASM client + axum server binary + public/ assets.
# dx drives the dual (client+server) build and asset optimization.
# Output tree: target/dx/iron_hermes_ui/release/web/{iron_hermes_ui, public/}
RUN dx bundle --platform web -p iron_hermes_ui --release

# --- Stage 2: Minimal runtime ---
# trixie-slim (glibc 2.41) to match the builder: the binaries copied from the
# builder are linked against trixie's glibc, so the runtime must be >= that.
FROM docker.io/library/debian:trixie-slim AS runtime

# Runtime deps:
# - python3: execute_code sandbox
# - ca-certificates: HTTPS for API calls
# - procps: ps for process management
# - libasound2t64: ALSA runtime for cpal/rodio audio (renamed from libasound2
#   by Debian's 64-bit time_t transition; plain `libasound2` is gone in trixie)
# - curl: compose healthcheck (GET http://localhost:8080/)
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        python3 \
        ca-certificates \
        procps \
        libasound2t64 \
        curl && \
    rm -rf /var/lib/apt/lists/*

# gosu from dedicated stage (avoids apt security-repo dependency)
COPY --chmod=0755 --from=gosu_source /gosu /usr/local/bin/gosu

# Non-root runtime user (UID 10000, home at /opt/data)
RUN useradd -u 10000 -m -d /opt/data ironhermes

# CLI (management) + the fullstack web bundle (server binary + public/ assets)
COPY --from=builder /build/target/release/ironhermes /usr/local/bin/ironhermes
COPY --from=builder /build/target/dx/iron_hermes_ui/release/web/ /opt/ironhermes/web/

# Templates and entrypoints
COPY --chown=ironhermes:ironhermes env.example /opt/ironhermes/.env.example
COPY --chown=ironhermes:ironhermes cli-config.yaml.example /opt/ironhermes/cli-config.yaml.example
COPY --chown=ironhermes:ironhermes docker/ /opt/ironhermes/docker/
COPY --chmod=0755 docker/web-entrypoint.sh /usr/local/bin/web-entrypoint.sh

WORKDIR /opt/ironhermes

ENV PYTHONUNBUFFERED=1
ENV IRONHERMES_HOME=/opt/data
# Loopback by default (quick task 260820-8h5) so a freshly generated
# first-run credential is never published before the operator has read it.
# Any deployment needing external reachability — including the AaaS/Caddy
# stack, which previously relied on the old wildcard default to let a
# separate Caddy container reach hermes:8080 — must pass -e IP=0.0.0.0
# explicitly. See docs/CONTAINER.md for the full two-step flow.
ENV IP=127.0.0.1
ENV PORT=8080
# Launch `ironhermes gateway --non-interactive` in the background alongside
# the web UI (quick task 260825-dww). Declared here rather than left to the
# entrypoint's shell default so `podman inspect` / `docker inspect` surfaces
# the effective setting next to IP and PORT when an operator is working out
# why schedules did or did not run, and so the default is greppable in the
# image config rather than buried in a shell expansion. 0, false, no, and off
# (any case) all disable it. The entrypoint keeps its own `${...:-1}` default
# regardless, for the case where the script runs outside this image.
ENV IRONHERMES_GATEWAY=1

VOLUME ["/opt/data"]

EXPOSE 8080

# Privilege-drop, seed ~/.ironhermes, launch the background gateway
# (best-effort), then exec the web server on 0.0.0.0:8080.
ENTRYPOINT ["/usr/local/bin/web-entrypoint.sh"]
