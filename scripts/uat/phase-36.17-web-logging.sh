#!/usr/bin/env bash
# Phase 36.17 — iron_hermes_ui web-logging UAT.
#
# Behavioral verification that the post-Wave-2 codebase wires two daily-rolling
# tracing-appender file streams into $IRONHERMES_HOME/logs/ AND mounts a
# tower_http TraceLayer that actually emits at INFO. Designed to be run
# locally on a dev machine after Plans 36.17-01..03 have been applied.
#
# Decisions covered (from .planning/phases/36.17-.../36.17-CONTEXT.md):
#   D-01  — Two daily-rolling files under $IRONHERMES_HOME/logs/
#   D-02  — Daily-rolling appenders, non-blocking, WorkerGuards held in main()
#   D-03  — File output is ANSI-free
#   D-04  — Default app filter: ironhermes=info,iron_hermes_ui=info
#   D-05  — Access log filter: tower_http::trace=info (per-layer, fixed)
#   D-09  — HTTP access lines route ONLY to web-access.log (not web.log, not stdout)
#   D-10  — TraceLayer::new_for_http() with on_request/on_response at INFO (Q2 fix)
#   D-11  — Basic access log only (no headers/bodies/request-id)
#   D-15  — Uses ironhermes_core::constants::get_hermes_home() to resolve $IRONHERMES_HOME
#   D-16  — Creates $IRONHERMES_HOME/logs/ at startup
#
# The single highest-leverage gate is Gate 3 (D-05/D-10 Q2): without explicit
# DefaultOnRequest::new().level(Level::INFO) and DefaultOnResponse::new().level(Level::INFO)
# the TraceLayer defaults emit at DEBUG, the tower_http::trace=info per-layer
# filter drops them, and web-access.log exists but is empty. A file-existence
# check alone would not catch this — Gate 3 grep'ing for the literal
# "tower_http::trace" target string is the regression net.
#
# Exit 0 on all-pass; non-zero on any fail. Mirrors scripts/ci-gates.sh shell
# conventions (shebang, set -euo pipefail, SCRIPT_DIR/WORKSPACE_ROOT, gate
# echo pattern, GATE FAIL exit pattern).

set -euo pipefail

# Run from the workspace root (directory containing Cargo.toml / crates/).
# Resolve relative to this script so `bash scripts/uat/phase-36.17-web-logging.sh`
# from any cwd still finds the right path.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${WORKSPACE_ROOT}"

echo "==> Phase 36.17 web-logging UAT (workspace: ${WORKSPACE_ROOT})"
echo

# -----------------------------------------------------------------------------
# Setup: temp $IRONHERMES_HOME and trap-based cleanup.
#
# A fresh tmp dir per run isolates this UAT from any developer state under
# ~/.ironhermes. The trap covers:
#   - normal exit (assertions all passed)
#   - set -e abort (assertion failed mid-script)
#   - SIGINT (user ctrl-c'd the script while server was still running)
# Background server is killed with SIGTERM so both WorkerGuard's Drop impls
# run (which call shutdown_signal.notify_one() + handle.join() — synchronous
# flush). The /tmp/phase-36.17-*.log files are INTENTIONALLY preserved for
# post-mortem diagnosis on failure (see Pitfall 5 / RESEARCH.md Q5).
# -----------------------------------------------------------------------------
IRONHERMES_HOME="$(mktemp -d)"
export IRONHERMES_HOME
SERVER_PID=""
SERVER_STDOUT="/tmp/phase-36.17-stdout.log"
SERVER_STDERR="/tmp/phase-36.17-stderr.log"

cleanup() {
    # Kill the server first (gracefully) so WorkerGuards flush, then wait,
    # then remove the tmp home. Each step is best-effort — if the server
    # already exited we silently swallow the kill error so the trap still
    # cleans up the tmp dir.
    if [ -n "${SERVER_PID}" ]; then
        kill -TERM "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    rm -rf "${IRONHERMES_HOME}"
}
trap cleanup EXIT

echo "==> Using IRONHERMES_HOME=${IRONHERMES_HOME}"
echo

# -----------------------------------------------------------------------------
# Port precheck.
#
# iron_hermes_ui binds 127.0.0.1:8080 by default via
# dioxus::cli_config::fullstack_address_or_localhost(). If that port is
# already in use (another server, a stuck previous run), abort early with a
# clear message rather than letting `cargo run` race-bind a different
# ephemeral port we can't predict for curl.
# -----------------------------------------------------------------------------
echo "--> Precheck: port 8080 is free"
if lsof -iTCP:8080 -sTCP:LISTEN >/dev/null 2>&1; then
    echo "    GATE FAIL (precheck): TCP port 8080 is already in use."
    echo "    Free the port (e.g. \`lsof -iTCP:8080 -sTCP:LISTEN\` then kill the offender) and re-run."
    exit 1
fi
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Build first so step "start the server" has deterministic timing.
# A first-run compile inside the background process can take minutes on a
# cold target/ — that would race the 30-iteration bind poll below.
# -----------------------------------------------------------------------------
echo "==> Building iron_hermes_ui --features server (separate step so server start is fast)"
cargo build -p iron_hermes_ui --features server
echo

# -----------------------------------------------------------------------------
# Satisfy dioxus-server's public-dir precondition.
#
# dioxus-server (0.7.x) panics at startup if target/debug/public/ does not
# exist (serve_dir_cached calls fs::read_dir and unwrap_or_else-panics on
# ENOENT). In normal dev workflow `dx build` / `dx serve` produces this
# directory with the bundled client assets. This UAT bypasses the Dioxus CLI
# (it boots the bin via `cargo run` so TraceLayer behavior is unobscured by
# the dx-serve dev proxy). An empty public/ is enough — TraceLayer is a
# router-level middleware that fires on every incoming request BEFORE any
# SSR/static-asset handler runs, so the access-log entry happens regardless
# of whether the SSR handler returns 200 or 500.
# -----------------------------------------------------------------------------
echo "==> Ensuring target/debug/public/ exists (dioxus-server precondition)"
mkdir -p target/debug/public
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Start the server in the background. Redirect stdout + stderr to /tmp files
# so we can (a) keep this script's own output uncluttered, (b) post-mortem
# the server's tracing output if a gate fails, (c) grep stdout for Gate 5's
# "access events did not leak to stdout" assertion.
# -----------------------------------------------------------------------------
echo "==> Starting server (stdout: ${SERVER_STDOUT}, stderr: ${SERVER_STDERR})"
cargo run -p iron_hermes_ui --features server >"${SERVER_STDOUT}" 2>"${SERVER_STDERR}" &
SERVER_PID=$!
echo "    pid=${SERVER_PID}"
echo

# -----------------------------------------------------------------------------
# Wait for the server to bind. Up to 30 iterations of 1s each. We probe with
# curl's exit code, NOT the HTTP status — a 404 still proves the server is
# listening and the TraceLayer is in the request path. curl -fsS would fail
# on 4xx, so we drop -f here.
# -----------------------------------------------------------------------------
echo "==> Waiting for server to bind 127.0.0.1:8080 (max 30s)"
BIND_OK=0
for _ in $(seq 1 30); do
    if curl -sS http://127.0.0.1:8080/ >/dev/null 2>&1; then
        BIND_OK=1
        break
    fi
    sleep 1
done

if [ "${BIND_OK}" -ne 1 ]; then
    echo "    GATE FAIL (bind): server did not accept a connection on 127.0.0.1:8080 within 30s."
    echo "    Server stderr (tail):"
    tail -n 50 "${SERVER_STDERR}" || true
    exit 1
fi
echo "    OK (bound)"
echo

# -----------------------------------------------------------------------------
# Send one real HTTP request to trigger TraceLayer. We tolerate non-2xx via
# `|| true` — the goal is to produce one on_request + on_response event in
# the tower_http::trace target stream, not to assert that the route returns
# 200. A 404 is fine; what matters is that the middleware emitted log events.
# -----------------------------------------------------------------------------
echo "==> Sending one request to trigger TraceLayer"
curl -sS http://127.0.0.1:8080/ >/dev/null 2>&1 || true
echo "    OK (request sent)"
echo

# -----------------------------------------------------------------------------
# Graceful shutdown so WorkerGuards drop and flush. SIGTERM triggers axum's
# shutdown path, which lets `main()` fall through scope end where the two
# `_web_log_guard` / `_access_log_guard` locals are dropped — each guard's
# Drop impl synchronously waits for its writer thread to drain.
# `wait $SERVER_PID` blocks until the child actually exits so we don't race
# the file assertions below.
# -----------------------------------------------------------------------------
echo "==> Stopping server (SIGTERM) and waiting for guards to flush"
kill -TERM "${SERVER_PID}" 2>/dev/null || true
wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""  # cleanup trap is now a no-op for the process
echo "    OK (server exited)"
echo

# -----------------------------------------------------------------------------
# Compute expected log paths. tracing-appender::rolling::daily uses a
# YYYY-MM-DD suffix in UTC. We use `date -u +%Y-%m-%d` to match.
# -----------------------------------------------------------------------------
LOG_DIR="${IRONHERMES_HOME}/logs"
DATE="$(date -u +%Y-%m-%d)"
WEB_LOG="${LOG_DIR}/web.log.${DATE}"
ACCESS_LOG="${LOG_DIR}/web-access.log.${DATE}"

echo "==> Log files expected:"
echo "    ${WEB_LOG}"
echo "    ${ACCESS_LOG}"
echo

# -----------------------------------------------------------------------------
# Gate 1 (D-01 / D-02 / D-15 / D-16): both daily-rolling files exist.
#
# This is the floor — proves get_hermes_home() resolved, create_dir_all
# succeeded, and tracing_appender::rolling::daily created the date-suffixed
# files. A failure here means the install fn never ran or fell back to the
# (None, None) no-file-layer arm.
# -----------------------------------------------------------------------------
echo "--> Gate 1 (D-01/D-02/D-15/D-16): both log files exist"
if [ ! -f "${WEB_LOG}" ]; then
    echo "    GATE FAIL (D-01/D-02/D-15/D-16): ${WEB_LOG} is missing — check install_web_logger_subscriber() ran and create_dir_all(${LOG_DIR}) succeeded."
    exit 1
fi
if [ ! -f "${ACCESS_LOG}" ]; then
    echo "    GATE FAIL (D-01/D-02/D-15/D-16): ${ACCESS_LOG} is missing — check install_web_logger_subscriber() ran and create_dir_all(${LOG_DIR}) succeeded."
    exit 1
fi
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 2 (D-04): web.log carries at least one INFO line.
#
# Proves the app filter (`ironhermes=info,iron_hermes_ui=info`) is wired and
# at least one INFO event from the ironhermes or iron_hermes_ui targets made
# it through the file layer. If this gate fails but Gate 1 passed, the
# subscriber installed but the filter is excluding everything we expected.
# -----------------------------------------------------------------------------
echo "--> Gate 2 (D-04): web.log has at least one INFO line"
if ! grep -q "INFO" "${WEB_LOG}"; then
    echo "    GATE FAIL (D-04): ${WEB_LOG} contains no 'INFO' marker — verify the app EnvFilter default 'ironhermes=info,iron_hermes_ui=info' is wired on the web file layer."
    exit 1
fi
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 3 (D-05 / D-10 / Q2 risk): web-access.log is non-empty AND carries
# the tower_http::trace target string.
#
# THIS IS THE Q2 RISK GATE.
#
# tower_http::trace::{DefaultOnRequest, DefaultOnResponse} default to
# Level::DEBUG, NOT Level::INFO (confirmed at
# docs.rs/tower-http/0.6.8/tower_http/trace/struct.DefaultOnRequest.html —
# "Defaults to Level::DEBUG"). The per-layer access filter is locked at
# `tower_http::trace=info` per D-05. If main.rs mounts
# `.layer(TraceLayer::new_for_http())` with no `.on_request(...)` /
# `.on_response(...)` customisation, the request/response events emit at
# DEBUG and the access filter SILENTLY DROPS THEM ALL. web-access.log
# exists (Gate 1 passes) but is empty. Only an explicit
#
#   TraceLayer::new_for_http()
#       .on_request(DefaultOnRequest::new().level(Level::INFO))
#       .on_response(DefaultOnResponse::new().level(Level::INFO))
#
# makes the access filter actually match what's emitted. See RESEARCH.md Q2.
# -----------------------------------------------------------------------------
echo "--> Gate 3 (D-05/D-10 Q2 risk): web-access.log contains tower_http::trace events"
if ! grep -q "tower_http::trace" "${ACCESS_LOG}"; then
    echo "    GATE FAIL (D-05/D-10 Q2): web-access.log is empty or missing tower_http::trace target — verify main.rs has DefaultOnRequest::new().level(Level::INFO) and DefaultOnResponse::new().level(Level::INFO) per RESEARCH.md Q2"
    exit 1
fi
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 4 (D-03): neither file contains ANSI escape sequences.
#
# `.with_ansi(false)` on each file layer is what enforces this — without it,
# the fmt layer would emit colored ESC[...] sequences (the same ones the
# console layer keeps for human readability). We use bash ANSI-C quoting
# ($'\x1b[') for the ESC byte; this is portable across BSD grep (macOS) and
# GNU grep (Linux). `grep -P` is NOT portable on macOS.
# -----------------------------------------------------------------------------
echo "--> Gate 4 (D-03): neither log file contains ANSI escape sequences"
if grep -q $'\x1b\[' "${WEB_LOG}"; then
    echo "    GATE FAIL (D-03): ${WEB_LOG} contains ANSI escapes — verify the web file layer carries .with_ansi(false)."
    exit 1
fi
if grep -q $'\x1b\[' "${ACCESS_LOG}"; then
    echo "    GATE FAIL (D-03): ${ACCESS_LOG} contains ANSI escapes — verify the access file layer carries .with_ansi(false)."
    exit 1
fi
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 5 (D-09): access events route ONLY to web-access.log.
#
# Two sub-assertions:
#   5a) web.log MUST NOT contain `tower_http::trace::on_request` — proves
#       the access filter doesn't bleed into the app stream.
#   5b) The server stdout file MUST NOT contain `tower_http::trace::on_request`
#       — proves D-09 ("HTTP access lines do NOT print to stdout") under the
#       composed registry. The console layer's filter
#       (`ironhermes=info,iron_hermes_ui=info`) is what suppresses it on the
#       console side.
# -----------------------------------------------------------------------------
echo "--> Gate 5 (D-09): access events do not leak into web.log or server stdout"
if grep -q "tower_http::trace::on_request" "${WEB_LOG}"; then
    echo "    GATE FAIL (D-09): ${WEB_LOG} contains tower_http::trace::on_request — separation-of-concerns violated; the web file layer is matching access events. Check that the access filter 'tower_http::trace=info' is attached only to the access file layer."
    exit 1
fi
if grep -q "tower_http::trace::on_request" "${SERVER_STDOUT}"; then
    echo "    GATE FAIL (D-09): server stdout contains tower_http::trace::on_request — the console layer's filter should NOT match the tower_http::trace target. Check the console layer's EnvFilter."
    exit 1
fi
echo "    OK"
echo

echo "==> Phase 36.17 web-logging UAT: all green"
