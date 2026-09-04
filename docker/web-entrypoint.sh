#!/bin/bash
# =============================================================================
# IronHermes AaaS web-server entrypoint
# Mirrors docker/entrypoint.sh (privilege drop + template seeding) but runs the
# iron_hermes_ui fullstack server instead of the `ironhermes` CLI.
# Binds 0.0.0.0:8080 (IP/PORT env, set in the Dockerfile) — requires a web
# password hash (IRONHERMES_WEB_PASSWORD_HASH / config.yaml) or the app's
# fail-closed bind guard refuses to start.
#
# Runs TWO processes (quick task 260825-dww): `ironhermes gateway
# --non-interactive` in the background, best-effort, hosting the cron / kanban
# / notifier scheduler loops the UI server does not; and `iron_hermes_ui`,
# which alone determines container health/lifecycle. Toggle the background
# gateway with IRONHERMES_GATEWAY (0/false/no/off to disable; default on).
#
# This script is tini's direct child (see the image ENTRYPOINT), and it
# supervises rather than execs, so that the SIGTERM tini forwards on
# `podman stop` reaches BOTH processes and the gateway gets a bounded window
# (IRONHERMES_GATEWAY_STOP_TIMEOUT, default 5s) to shut down cleanly.
# =============================================================================
set -euo pipefail

IRONHERMES_HOME="${IRONHERMES_HOME:-/opt/data}"
INSTALL_DIR="/opt/ironhermes"

# ─── Privilege drop (start as root, switch to ironhermes) ───
if [ "$(id -u)" = "0" ]; then
    chown -R ironhermes:ironhermes "$IRONHERMES_HOME" 2>/dev/null || \
        echo "Warning: chown failed (rootless?) — continuing"
    exec gosu ironhermes "$0" "$@"
fi

# ─── Running as ironhermes user ───
mkdir -p "$IRONHERMES_HOME"/{cron,sessions,logs,hooks,memories,skills,workspace}

# Seed templates only if absent (preserve the .env the AaaS provisioner wrote,
# which carries IRONHERMES_WEB_PASSWORD_HASH + provider keys).
[ -f "$IRONHERMES_HOME/.env" ]        || cp "$INSTALL_DIR/.env.example" "$IRONHERMES_HOME/.env"
[ -f "$IRONHERMES_HOME/config.yaml" ] || cp "$INSTALL_DIR/cli-config.yaml.example" "$IRONHERMES_HOME/config.yaml"
[ -f "$IRONHERMES_HOME/SOUL.md" ]     || cp "$INSTALL_DIR/docker/SOUL.md" "$IRONHERMES_HOME/SOUL.md"
chmod 600 "$IRONHERMES_HOME/.env" 2>/dev/null || true

# ─── Skill seeding (image → volume, skip-if-exists) ───
# /opt/ironhermes/{skills,optional-skills} are the read-only copies baked into
# the image; the agent only ever scans the volume. `cp -rn` merges new
# categories and new skills in on an image upgrade while never overwriting an
# operator edit or a Hub install already on the volume.
#
# skills/ lands in $IRONHERMES_HOME/skills, a default search root, so those
# skills are live immediately. optional-skills/ lands in
# $IRONHERMES_HOME/optional-skills, which is NOT a default search root — opt in
# by adding it to skills.extra_paths in config.yaml, or import individual
# skills from it in the web UI (its Local Path quick-pick already offers this
# directory). Non-fatal: a seeding failure must never take the container down.
for _src in skills optional-skills; do
    if [ -d "$INSTALL_DIR/$_src" ]; then
        mkdir -p "$IRONHERMES_HOME/$_src"
        cp -rn "$INSTALL_DIR/$_src/." "$IRONHERMES_HOME/$_src/" 2>/dev/null || \
            echo "Warning: seeding $_src into $IRONHERMES_HOME failed — continuing" >&2
    fi
done

# First-run web credential provisioning (quick task 260820-8h5). Generates a
# random password and persists ONLY its argon2id hash into config.yaml, but
# only when no hash is resolvable from config.yaml/IRONHERMES_WEB_PASSWORD_HASH/
# vault, and only when IP names a loopback address (it reads the same IP
# variable, with the same parse semantics, that the server below resolves its
# bind address from — see init-password's own doc comment). No bind address
# is computed here: the image's ENV default (Dockerfile) is the entire
# mechanism, so an operator-supplied IP reaches dioxus_cli_config::server_ip()
# untouched either way. Non-fatal by design: aborting here would leave the
# container exiting on every start, and under `--restart=always` that is a
# genuine restart loop. If provisioning fails, the server still starts on the
# loopback default with auth disabled (the pre-47.3 posture), reachable via
# `podman exec` for repair.
if ! ironhermes web init-password; then
    echo "Warning: ironhermes web init-password failed — continuing with auth disabled" >&2
fi

# Seconds to wait for the gateway to finish after SIGTERM. Must stay under the
# runtime's own stop grace period (podman/docker default 10s) or PID 1 is
# SIGKILLed mid-drain. Non-numeric or empty values fall back to the default
# rather than breaking the arithmetic below.
GATEWAY_STOP_TIMEOUT="${IRONHERMES_GATEWAY_STOP_TIMEOUT:-5}"
case "$GATEWAY_STOP_TIMEOUT" in
    ''|*[!0-9]*) GATEWAY_STOP_TIMEOUT=5 ;;
esac

# Set by forward_shutdown so stop_gateway knows the gateway has already been
# signalled and does not send a second, redundant SIGTERM into an in-progress
# shutdown.
SHUTDOWN_SIGNALLED=0

# Stop the background gateway and wait (bounded) for it to exit. Its SIGTERM
# handler runs the ordinary shutdown path — MCP teardown, task drain, and
# PidLockGuard::Drop removing gateway.pid from the volume. Reaching the
# timeout is not an error: the container tears down either way, and the next
# start still reconciles a leftover pidfile.
#
# Reached by BOTH exit paths: an operator `podman stop` (where the trap fired
# first and SHUTDOWN_SIGNALLED is 1), and the web server exiting on its own —
# a crash or a failed startup guard — where nothing has signalled the gateway
# yet and the SIGTERM below is the only one it will get.
stop_gateway() {
    [ -n "${GW_PID:-}" ] || return 0
    kill -0 "$GW_PID" 2>/dev/null || return 0

    if [ "$SHUTDOWN_SIGNALLED" = "0" ]; then
        kill -TERM "$GW_PID" 2>/dev/null || true
    fi

    _waited=0
    while [ "$_waited" -lt "$GATEWAY_STOP_TIMEOUT" ] && kill -0 "$GW_PID" 2>/dev/null; do
        sleep 1
        _waited=$((_waited + 1))
    done

    if kill -0 "$GW_PID" 2>/dev/null; then
        echo "Gateway (pid $GW_PID) still running after ${GATEWAY_STOP_TIMEOUT}s — leaving it to container teardown" >&2
    else
        echo "Gateway shut down cleanly"
    fi
}

# SIGTERM/SIGINT handler. Signals the gateway FIRST — it has the longer
# shutdown path (bounded MCP teardown, task drain) — then the web server,
# so both drain concurrently instead of serially.
#
# Installed BEFORE the gateway launch below, not next to the web-server launch
# it also signals: the launch block contains a 2-second liveness probe, and a
# stop arriving inside that window would otherwise hit bash's default SIGTERM
# disposition and kill this shell with the gateway still running. Both PIDs are
# read with `:-` defaults so the handler is safe at any point in that window.
forward_shutdown() {
    SHUTDOWN_SIGNALLED=1
    if [ -n "${GW_PID:-}" ]; then
        kill -TERM "$GW_PID" 2>/dev/null || true
    fi
    if [ -n "${UI_PID:-}" ]; then
        kill -TERM "$UI_PID" 2>/dev/null || true
    fi
}
trap forward_shutdown TERM INT

# ─── Background gateway launch (quick task 260825-dww, D-01) ───
# Best-effort: `ironhermes gateway` hosts the cron / kanban / notifier
# scheduler loops the UI server does not. It refuses to boot when no
# messaging platform is configured — the normal first-run state — and that
# must NOT take the container down. IRONHERMES_GATEWAY defaults to on;
# 0/false/no/off (any case) disables it.
case "${IRONHERMES_GATEWAY:-1}" in
    0|[Ff][Aa][Ll][Ss][Ee]|[Nn][Oo]|[Oo][Ff][Ff])
        echo "IRONHERMES_GATEWAY disabled — starting web UI only"
        ;;
    *)
        # Clean up a leftover gateway.pid from a previous container life
        # UNLESS it names a still-live gateway process. Container PID
        # numbering is deterministic across restarts, so an unconditional
        # skip here would let a stale pidfile block every future launch with
        # a nonsensical "Gateway already running" (acquire_pid_lock has no
        # self-pid exemption). See design_rationale B.
        parsed="$(sed -n 's/^pid: *//p' "$IRONHERMES_HOME/gateway.pid" 2>/dev/null | head -n 1 || true)"
        case "$parsed" in
            ''|*[!0-9]*) parsed="" ;;
        esac
        if [ -n "$parsed" ] && [ -r "/proc/$parsed/cmdline" ]; then
            cmdline="$(tr '\0' ' ' < "/proc/$parsed/cmdline" 2>/dev/null || true)"
            case "$cmdline" in
                *gateway*)
                    echo "A live gateway (pid $parsed) already holds $IRONHERMES_HOME/gateway.pid — leaving it in place" >&2
                    ;;
                *)
                    echo "Removing leftover gateway.pid from a previous container run (pid $parsed is not a gateway)"
                    rm -f "$IRONHERMES_HOME/gateway.pid"
                    ;;
            esac
        elif [ -n "$parsed" ]; then
            echo "Removing leftover gateway.pid from a previous container run (pid $parsed is not a gateway)"
            rm -f "$IRONHERMES_HOME/gateway.pid"
        fi

        GATEWAY_LOG="$IRONHERMES_HOME/logs/gateway.log"
        # Gateway startup errors can quote provider keys / platform tokens,
        # and this file lands on a persisted volume (threat T-dww-01) —
        # lock it down before the first write, same as the .env chmod above.
        touch "$GATEWAY_LOG" 2>/dev/null || true
        chmod 600 "$GATEWAY_LOG" 2>/dev/null || true

        ironhermes gateway --non-interactive >>"$GATEWAY_LOG" 2>&1 &
        GW_PID=$!
        echo "Gateway started in background, pid=$GW_PID, log=$GATEWAY_LOG"

        # One-shot liveness probe (bounded, not a supervision loop — D-01).
        # `kill -0` alone is not sufficient: an already-exited gateway may
        # still be an unreaped zombie at this point, and zombies ARE
        # signalable. Read /proc/<pid>/stat's process-state field instead;
        # `Z` means it is already dead. An unreadable /proc entry is treated
        # as alive so a probe failure can never fabricate an alarm.
        sleep 2
        gw_down=0
        if ! kill -0 "$GW_PID" 2>/dev/null; then
            gw_down=1
        else
            gw_state="$(sed -e 's/^.*) //' -e 's/ .*//' "/proc/$GW_PID/stat" 2>/dev/null || true)"
            if [ "$gw_state" = "Z" ]; then
                gw_down=1
            fi
        fi

        if [ "$gw_down" = "1" ]; then
            {
                echo "=============================================="
                echo " WARNING: gateway exited within 2s of launch"
                echo ""
                echo " The web UI is unaffected and starting now."
                echo " Cron, kanban, and notifier schedules will NOT run"
                echo " until the gateway is restarted successfully."
                echo ""
                echo " Most likely cause: no messaging platform is"
                echo " configured (the gateway refuses to boot with zero"
                echo " usable platforms). Configure one, e.g."
                echo " -e TELEGRAM_BOT_TOKEN=..., or silence this by"
                echo " passing -e IRONHERMES_GATEWAY=0."
                echo ""
                echo " Full reason: $GATEWAY_LOG"
                echo "=============================================="
            } >&2
        fi
        ;;
esac

# ─── Supervised launch + graceful shutdown ───
# This script does NOT exec the web server. It stays alive as tini's direct
# child so it can receive the SIGTERM tini forwards on `podman stop` and shut
# BOTH processes down in order. `exec` here would replace this shell, leaving
# the gateway parented to an app server that forwards nothing — the signal
# would die there and the gateway would be SIGKILLed by PID-namespace
# teardown, which is precisely the ungraceful path this replaces.
#
# What is preserved from the exec-based design: container health and lifecycle
# still depend on `iron_hermes_ui` alone. If it exits for any reason, this
# script stops the gateway and exits with the web server's status, which tini
# propagates as the container's exit code. The gateway's health is still never
# consulted (D-01) — it remains best-effort.
#
# Zombie reaping is tini's job now, so an early-exiting gateway no longer
# lingers in the process table and `--init` is no longer needed.


# The fullstack server resolves assets relative to the bundle dir.
cd "$INSTALL_DIR/web"
./iron_hermes_ui &
UI_PID=$!

# `wait` is interrupted by a trapped signal and returns >128 without the child
# having exited, so re-wait until it really is gone. Guarded by `if` because
# `set -e` would otherwise abort on any non-zero web-server exit status — which
# we want to capture and propagate, not die on.
while :; do
    if wait "$UI_PID"; then
        UI_STATUS=0
    else
        UI_STATUS=$?
    fi
    if [ "$UI_STATUS" -gt 128 ] && kill -0 "$UI_PID" 2>/dev/null; then
        continue
    fi
    break
done

stop_gateway
exit "$UI_STATUS"
