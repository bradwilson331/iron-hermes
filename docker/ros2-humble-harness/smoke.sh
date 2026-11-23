#!/usr/bin/env bash
# Phase 49.8 D-17 (local layer): one-command smoke test against a real,
# apt-built Humble rosbridge — proves what the CI mock server cannot (D-06
# action ops on the actual rosbridge build). Opt-in, never part of the CI
# gate; loopback-only host exposure (T-49.8-24).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

cleanup() {
  echo "Tearing down docker/ros2-humble-harness..."
  # Always run from SCRIPT_DIR regardless of where the main body's own `cd`
  # left the shell — this trap fires from whatever directory is current at
  # EXIT, and running `docker compose down` from the wrong directory (e.g.
  # the repo root, which the cargo-test step below `cd`s into) silently
  # fails to find this harness's docker-compose.yml.
  (cd "$SCRIPT_DIR" && docker compose down)
}
trap cleanup EXIT

echo "Building and starting the ros2-humble-harness container..."
docker compose up -d --build

echo "Waiting up to 180s for 127.0.0.1:9090 to accept connections..."
waited=0
until (exec 3<>"/dev/tcp/127.0.0.1/9090") 2>/dev/null; do
  exec 3>&- 2>/dev/null || true
  waited=$((waited + 1))
  if [ "$waited" -ge 180 ]; then
    echo "ERROR: rosbridge did not come up on 127.0.0.1:9090 within 180s" >&2
    docker compose logs
    exit 1
  fi
  sleep 1
done
exec 3>&- 2>/dev/null || true
echo "Port 127.0.0.1:9090 accepted a connection after ${waited}s."

# The port can accept a raw TCP connection slightly before the rosbridge
# websocket server inside is actually ready to complete a handshake — the
# `ros2 launch` node bringup is still finishing at that instant, and an
# immediate real client connect can get "connection reset by peer"
# (confirmed live: 49.8-10). A bare "did the port open" check races that.
# Give the handshake a couple more seconds to become ready.
sleep 3
echo "rosbridge is up."

echo "Installed rosbridge version (from the container):"
docker compose exec -T ros2-humble-harness bash -lc \
  "source /opt/ros/humble/setup.bash && dpkg -s ros-humble-rosbridge-server | grep Version"

REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo "Running the live smoke test against ws://127.0.0.1:9090 ..."
(
  cd "$REPO_ROOT"
  IRONHERMES_ROS2_LIVE_URL=ws://127.0.0.1:9090 \
    cargo test -p ironhermes-tools --test ros2_live_bridge -- --ignored --test-threads=1 --nocapture
)
