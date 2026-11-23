#!/bin/bash
# =============================================================================
# docker/priv-drop.sh -- shared privilege-drop library (phase 54.11, D-02/D-06)
# =============================================================================
# SOURCED, never executed: it has no top-level side effects. Consumers:
#   - docker/web-entrypoint.sh and docker/entrypoint.sh source it at
#     /opt/ironhermes/docker/priv-drop.sh while still root and call
#     ih_drop_privileges. Both FAIL CLOSED: a missing library, an invalid
#     IRONHERMES_UID/GID or a drop that cannot happen exits non-zero. Neither
#     entrypoint ever continues as root.
#   - the same entrypoints source it again after the drop and call
#     ih_volume_env (D-16).
#
# The image has no USER directive (D-05); the entrypoint starts as root and
# drops here, so the drop is enforced in-image and holds with NO runtime flags
# (the Sliplane case). Order is validate, chown, then setpriv.
#
# A remapped IRONHERMES_UID/GID (D-14) never touches /etc/passwd: the
# shadow-utils edit tools cannot lock it under --read-only. Instead a patched copy of passwd/group goes
# to /run/ih-nss and libnss-wrapper (a fixed soname, never taken from the
# environment) makes the `ironhermes` name resolve for the dropped process.
#
# Portable to bash 3.2 syntax (no mapfile, no associative arrays).
# =============================================================================

# >>> ih_nss_remap
# ih_nss_remap UID GID HOME -- write /run/ih-nss/{passwd,group} (root-owned,
# 0644 in a 0755 dir) with the `ironhermes` entry moved to UID:GID. Called as
# root BEFORE the drop so the app uid cannot rewrite its own identity files.
ih_nss_remap() {
    local uid="$1" gid="$2" home="$3" d=/run/ih-nss
    ls /usr/lib/*/libnss_wrapper.so >/dev/null 2>&1 || {
        echo "Error: libnss_wrapper.so not installed -- cannot honour IRONHERMES_UID/GID=$uid/$gid" >&2
        return 1
    }
    if ! mkdir -p "$d" || ! chmod 0755 "$d"; then
        echo "Error: cannot create $d (is /run writable?) -- cannot honour IRONHERMES_UID/GID=$uid/$gid" >&2
        return 1
    fi
    if ! awk -F: -v u="$uid" -v g="$gid" -v h="$home" \
            '$1 == "ironhermes" { print "ironhermes:x:" u ":" g "::" h ":/bin/bash"; next } { print }' \
            /etc/passwd >"$d/passwd" ||
        ! awk -F: -v g="$gid" \
            '$1 == "ironhermes" { print "ironhermes:x:" g ":"; next } { print }' \
            /etc/group >"$d/group" ||
        ! chmod 0644 "$d/passwd" "$d/group"; then
        echo "Error: cannot write $d/passwd and $d/group -- cannot honour IRONHERMES_UID/GID=$uid/$gid" >&2
        return 1
    fi
}
# <<< ih_nss_remap

# >>> ih_drop_privileges
# ih_drop_privileges ARGV... -- as root: validate IRONHERMES_UID/GID, chown the
# data volume, then exec ARGV as that uid with no_new_privs and an empty
# inheritable set (and an empty bounding set when CAP_SETPCAP allows it).
# Only returns on failure (return 1); success replaces the process.
ih_drop_privileges() {
    local uid="${IRONHERMES_UID:-10000}" gid="${IRONHERMES_GID:-10000}"
    local home="${IRONHERMES_HOME:-/opt/data}"

    case "$uid" in
        ''|*[!0-9]*) echo "Error: IRONHERMES_UID must be a non-zero number, got: $uid" >&2; return 1 ;;
    esac
    case "$gid" in
        ''|*[!0-9]*) echo "Error: IRONHERMES_GID must be a non-zero number, got: $gid" >&2; return 1 ;;
    esac
    if [ "$((10#$uid))" -eq 0 ]; then
        echo "Error: IRONHERMES_UID must be a non-zero number, got: $uid" >&2
        return 1
    fi
    if [ "$((10#$gid))" -eq 0 ]; then
        echo "Error: IRONHERMES_GID must be a non-zero number, got: $gid" >&2
        return 1
    fi
    uid=$((10#$uid))
    gid=$((10#$gid))

    command -v setpriv >/dev/null 2>&1 || {
        echo "Error: setpriv not found -- refusing to run as root" >&2
        return 1
    }

    chown -R "$uid:$gid" "$home" 2>/dev/null || \
        echo "Warning: chown failed (rootless?) -- continuing" >&2

    local groups_flag="--clear-groups"
    [ "$uid" -eq 10000 ] && groups_flag="--init-groups"

    # Empty the bounding set only when CAP_SETPCAP is held (no runtime flags,
    # or SETPCAP added back); under the compose cap set the strict form fails
    # and the app keeps the bounding set it was started with.
    local bound_flag=""
    if setpriv --bounding-set=-all --inh-caps=-all true >/dev/null 2>&1; then
        bound_flag="--bounding-set=-all"
    fi

    # Name resolution for a remapped uid (D-14). The account database is never edited.
    local nss_env=()
    if [ "$uid" -ne 10000 ] || [ "$gid" -ne 10000 ]; then
        ih_nss_remap "$uid" "$gid" "$home" || return 1
        nss_env=(LD_PRELOAD=libnss_wrapper.so NSS_WRAPPER_PASSWD=/run/ih-nss/passwd NSS_WRAPPER_GROUP=/run/ih-nss/group)
    fi

    # shellcheck disable=SC2086  # bound_flag is empty or one flag
    exec setpriv --reuid="$uid" --regid="$gid" "$groups_flag" $bound_flag \
        --inh-caps=-all --no-new-privs \
        env HOME="$home" USER=ironhermes LOGNAME=ironhermes ${nss_env[@]+"${nss_env[@]}"} "$@"
}
# <<< ih_drop_privileges

# >>> ih_volume_env
# ih_volume_env -- export the volume-scoped install locations (D-16) so user
# installs land on /opt/data instead of the read-only rootfs: npm globals and
# the Python user base under $IRONHERMES_HOME/.local (a pre-set value wins),
# pip in --user mode, and that tree's bin/ first on PATH.
#
# NOTE for the skills work (54.12): a venv-scoped `pip install` must run with
# PIP_USER unset -- pip refuses --user inside a virtualenv.
ih_volume_env() {
    local home="${IRONHERMES_HOME:-/opt/data}"
    export NPM_CONFIG_PREFIX="${NPM_CONFIG_PREFIX:-$home/.local}"
    export PYTHONUSERBASE="${PYTHONUSERBASE:-$home/.local}"
    export PIP_USER=1
    case "$PATH" in
        "$home/.local/bin:"*) ;;
        *) export PATH="$home/.local/bin:$PATH" ;;
    esac
}
# <<< ih_volume_env
