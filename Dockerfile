# =============================================================================
# IronHermes — Multi-stage OCI Build (Podman / Docker compatible)
# =============================================================================
# Build: podman build -t ironhermes .
# Run:   podman run -v ironhermes-data:/opt/data ironhermes
# =============================================================================

# --- Stage 0: gosu for privilege dropping ---
FROM docker.io/tianon/gosu:1.17 AS gosu_source

# --- Stage 1: Rust build ---
# Edition 2024 requires rustc >= 1.85; pin for reproducibility.
FROM docker.io/library/rust:1.96-bookworm AS builder
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
#   (iron_hermes_ui's site.css lives inside crates/iron_hermes_ui/assets/,
#    already covered by `COPY crates/ crates/` — the repo has no root assets/)
COPY skills/ skills/

# Build release binary
RUN cargo build --release --bin ironhermes

# --- Stage 2: Minimal runtime ---
FROM docker.io/library/debian:bookworm-slim AS runtime

# Runtime deps:
# - python3: execute_code sandbox
# - ca-certificates: HTTPS for API calls
# - procps: ps for process management
# - libasound2: ALSA runtime for cpal/rodio audio
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        python3 \
        ca-certificates \
        procps \
        libasound2 && \
    rm -rf /var/lib/apt/lists/*

# gosu from dedicated stage (avoids apt security-repo dependency)
COPY --chmod=0755 --from=gosu_source /gosu /usr/local/bin/gosu

# Non-root runtime user (UID 10000, home at /opt/data)
RUN useradd -u 10000 -m -d /opt/data ironhermes

# Compiled binary
COPY --from=builder /build/target/release/ironhermes /usr/local/bin/ironhermes

# Templates and entrypoint
COPY --chown=ironhermes:ironhermes env.example /opt/ironhermes/.env.example
COPY --chown=ironhermes:ironhermes cli-config.yaml.example /opt/ironhermes/cli-config.yaml.example
COPY --chown=ironhermes:ironhermes docker/ /opt/ironhermes/docker/

WORKDIR /opt/ironhermes

ENV PYTHONUNBUFFERED=1
ENV IRONHERMES_HOME=/opt/data

VOLUME ["/opt/data"]

EXPOSE 8080

ENTRYPOINT ["/opt/ironhermes/docker/entrypoint.sh"]
