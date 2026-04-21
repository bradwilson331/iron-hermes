# =============================================================================
# IronHermes — Multi-stage Docker Build
# =============================================================================
# Build: docker build -t ironhermes .
# Run:   docker run -v ironhermes-data:/opt/data ironhermes
# =============================================================================

# --- Stage 0: gosu for privilege dropping ---
FROM tianon/gosu:1.19 AS gosu_source

# --- Stage 1: Rust build ---
FROM rust:latest AS builder
WORKDIR /build

# Copy dependency manifests first for layer caching.
# Changes to source code won't invalidate the dependency compile cache.
COPY Cargo.toml Cargo.lock ./

# Copy workspace crate manifests (creates directory structure for cargo)
COPY crates/ironhermes-core/Cargo.toml crates/ironhermes-core/Cargo.toml
COPY crates/ironhermes-state/Cargo.toml crates/ironhermes-state/Cargo.toml
COPY crates/ironhermes-tools/Cargo.toml crates/ironhermes-tools/Cargo.toml
COPY crates/ironhermes-agent/Cargo.toml crates/ironhermes-agent/Cargo.toml
COPY crates/ironhermes-cli/Cargo.toml crates/ironhermes-cli/Cargo.toml
COPY crates/ironhermes-gateway/Cargo.toml crates/ironhermes-gateway/Cargo.toml
COPY crates/ironhermes-cron/Cargo.toml crates/ironhermes-cron/Cargo.toml
COPY crates/ironhermes-hooks/Cargo.toml crates/ironhermes-hooks/Cargo.toml
COPY crates/ironhermes-exec/Cargo.toml crates/ironhermes-exec/Cargo.toml
COPY crates/ironhermes-hub/Cargo.toml crates/ironhermes-hub/Cargo.toml
COPY providers/memory-sqlite/Cargo.toml providers/memory-sqlite/Cargo.toml
COPY providers/memory-grafeo/Cargo.toml providers/memory-grafeo/Cargo.toml
COPY providers/memory-duckdb/Cargo.toml providers/memory-duckdb/Cargo.toml

# Copy full source
COPY crates/ crates/
COPY providers/ providers/

# Build release binary
RUN cargo build --release --bin ironhermes

# --- Stage 2: Minimal runtime ---
FROM debian:bookworm-slim AS runtime

# Install minimal runtime dependencies:
# - python3: required for execute_code sandbox (D-08)
# - ca-certificates: HTTPS for API calls
# - procps: ps command for process management
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        python3 \
        ca-certificates \
        procps && \
    rm -rf /var/lib/apt/lists/*

# Copy gosu from dedicated stage (not apt — avoids security repo dependency)
COPY --chmod=0755 --from=gosu_source /gosu /usr/local/bin/gosu

# Create non-root runtime user (D-10: UID 10000, home at /opt/data)
RUN useradd -u 10000 -m -d /opt/data ironhermes

# Copy compiled binary from builder
COPY --from=builder /build/target/release/ironhermes /usr/local/bin/ironhermes

# Copy templates and entrypoint into install directory
COPY --chown=ironhermes:ironhermes .env.example cli-config.yaml.example /opt/ironhermes/
COPY --chown=ironhermes:ironhermes docker/ /opt/ironhermes/docker/

WORKDIR /opt/ironhermes

# D-11: IRONHERMES_HOME=/opt/data as persistent volume
ENV PYTHONUNBUFFERED=1
ENV IRONHERMES_HOME=/opt/data

VOLUME ["/opt/data"]

EXPOSE 8080

ENTRYPOINT ["/opt/ironhermes/docker/entrypoint.sh"]
