#!/bin/bash
# =============================================================================
# IronHermes AaaS web-server entrypoint
# Mirrors docker/entrypoint.sh (privilege drop + template seeding) but execs the
# iron_hermes_ui fullstack server instead of the `ironhermes` CLI.
# Binds 0.0.0.0:8080 (IP/PORT env, set in the Dockerfile) — requires a web
# password hash (IRONHERMES_WEB_PASSWORD_HASH / config.yaml) or the app's
# fail-closed bind guard refuses to start.
#
# Runs TWO processes (quick task 260825-dww): `ironhermes gateway
# --non-interactive` in the background, best-effort, hosting the cron / kanban
# / notifier scheduler loops the UI server does not; and `iron_hermes_ui` in
# the foreground as PID 1, which alone determines container health/lifecycle.
# Toggle the background gateway with IRONHERMES_GATEWAY (0/false/no/off to
# disable; default on).
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

# ─── Post-exec lifecycle note ───
# `exec` below replaces this shell in place, so the backgrounded gateway (if
# any) becomes a child of the new PID 1 (iron_hermes_ui). Consequences,
# accepted per D-01:
#   - `podman stop` / `docker stop` signals PID 1 only; the gateway never
#     gets a SIGTERM. When PID 1 exits, the kernel SIGKILLs every remaining
#     process in the container's PID namespace and tears it down — so the
#     gateway can never outlive the container or be orphaned onto the host,
#     but it also never shuts down gracefully.
#   - Because the shutdown is ungraceful, PidLockGuard::Drop never runs and
#     gateway.pid survives on the persisted volume — which is exactly why
#     the block above cleans up a leftover pidfile on the NEXT start rather
#     than trusting Drop.
#   - An early-exiting gateway lingers as a harmless zombie (one process-
#     table entry, no resource cost) for the container's lifetime, since PID
#     1 here does not reap unrelated children. Run with `--init` to reap it.
# The fullstack server resolves assets relative to the bundle dir.
cd "$INSTALL_DIR/web"
exec ./iron_hermes_ui
