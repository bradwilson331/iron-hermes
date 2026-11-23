#!/bin/bash
# =============================================================================
# docker/hardening-probe.sh -- in-container hardening probe (phase 54.11)
# =============================================================================
# Run with `bash` inside a running container as any uid; read-only, it never
# modifies the container. Prints KEY=VALUE lines for the iron_hermes_ui process
# and the mounts around it. Reused by scripts/tests/container_hardened_smoke.sh
# and by the Linux / Sliplane UAT (audit Appendix A).
#
# Keys: APP_PID APP_UID APP_GID APP_CAPEFF APP_CAPBND APP_NONEWPRIVS ROOTFS_RO
#       TMP_NOEXEC VARTMP_NOEXEC SHM_NOEXEC SHM_SIZE_KB HTTP_ROOT
#       VOLUME_FOREIGN_FILES APP_ENV_<NAME>
# =============================================================================
set -u

app_pid=""
for p in /proc/[0-9]*; do
    a0=$(tr '\0' '\n' <"$p/cmdline" 2>/dev/null | head -n 1)
    case "$a0" in
        */iron_hermes_ui) app_pid="${p#/proc/}"; break ;;
    esac
done
echo "APP_PID=${app_pid:-none}"

status_field() { # status_field KEY -> first value of /proc/PID/status KEY
    awk -v k="$1:" '$1 == k {print $2; exit}' "/proc/$app_pid/status" 2>/dev/null
}

if [ -n "$app_pid" ]; then
    echo "APP_UID=$(status_field Uid)"
    echo "APP_GID=$(status_field Gid)"
    echo "APP_CAPEFF=$(status_field CapEff)"
    echo "APP_CAPBND=$(status_field CapBnd)"
    echo "APP_NONEWPRIVS=$(status_field NoNewPrivs)"
fi

# mount_has_opt MOUNTPOINT OPT -> yes|no from /proc/mounts options
mount_has_opt() {
    awk -v m="$1" -v o="$2" '
        $2 == m { n = split($4, a, ","); for (i = 1; i <= n; i++) if (a[i] == o) f = 1; seen = 1 }
        END { print (seen && f) ? "yes" : "no" }' /proc/mounts
}
echo "ROOTFS_RO=$(mount_has_opt / ro)"
echo "TMP_NOEXEC=$(mount_has_opt /tmp noexec)"
echo "VARTMP_NOEXEC=$(mount_has_opt /var/tmp noexec)"
echo "SHM_NOEXEC=$(mount_has_opt /dev/shm noexec)"
echo "SHM_SIZE_KB=$(df -Pk /dev/shm 2>/dev/null | awk 'NR == 2 {print $2}')"
echo "HTTP_ROOT=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${PORT:-8080}/" 2>/dev/null)"

if [ -n "$app_pid" ]; then
    echo "VOLUME_FOREIGN_FILES=$(find "${IRONHERMES_HOME:-/opt/data}" ! -uid "$(status_field Uid)" 2>/dev/null | wc -l | tr -d ' ')"
fi

# Read environ once: after setpriv changes uid the process is non-dumpable, so the
# open is denied even when `[ -r ]` passes. A failed read is "unreadable", never "unset".
app_env=""
env_ok=0
if [ -n "$app_pid" ] && app_env=$( (tr '\0' '\n' <"/proc/$app_pid/environ") 2>/dev/null); then
    env_ok=1
fi
for name in HOME PATH NPM_CONFIG_PREFIX PYTHONUSERBASE PIP_USER LD_PRELOAD NSS_WRAPPER_PASSWD NSS_WRAPPER_GROUP IRONHERMES_BROWSER_NO_SANDBOX; do
    if [ "$env_ok" -ne 1 ]; then
        echo "APP_ENV_$name=unreadable"
        continue
    fi
    val=$(printf '%s\n' "$app_env" | sed -n "s/^$name=//p" | head -n 1)
    if printf '%s\n' "$app_env" | grep -q "^$name="; then
        echo "APP_ENV_$name=$val"
    else
        echo "APP_ENV_$name=unset"
    fi
done
