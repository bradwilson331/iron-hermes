#!/usr/bin/env bash
# =============================================================================
# scripts/tests/container_hardened_smoke.sh -- container smoke harness for the
# hardened default run (phase 54.11, plan 02)
# =============================================================================
# Boots the real image under the profile defined in docker-compose.yml (the run
# flags are DERIVED from that file, never copied) and asserts the privilege,
# mount and environment properties with docker/hardening-probe.sh.
#
# Usage:
#   bash scripts/tests/container_hardened_smoke.sh            # every case
#   bash scripts/tests/container_hardened_smoke.sh <case>...  # exactly those
#
# Prints `PASS <case>` / `FAIL <case>` / `SKIP <case> (<reason>)`; exits 1 on
# any FAIL or unknown case.
#
# Env:
#   IH_SMOKE_RT                 container runtime (default: podman if present, else docker)
#   IRONHERMES_IMAGE            image under test (default localhost/ironhermes:54.11)
#   IRONHERMES_DEVTOOLS_IMAGE   dev-tools image (default localhost/ironhermes:54.11-dev-tools)
#
# Needs `ruby` or `python3` + PyYAML to derive the flags; neither present is a FAIL, not a SKIP.
# No `set -e`: grep -c style counts legitimately exit non-zero.
# SC2329: case_*/cleanup are invoked indirectly (declare -F, trap).
# SC2016: the ruby program is deliberately single-quoted.
# shellcheck disable=SC2329,SC2016
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "$REPO_ROOT" || exit 1

if [ -z "${IH_SMOKE_RT:-}" ]; then
    if command -v podman >/dev/null 2>&1; then IH_SMOKE_RT=podman; else IH_SMOKE_RT=docker; fi
fi
RT="$IH_SMOKE_RT"
IMAGE="${IRONHERMES_IMAGE:-localhost/ironhermes:54.11}"
DEVTOOLS_IMAGE="${IRONHERMES_DEVTOOLS_IMAGE:-localhost/ironhermes:54.11-dev-tools}"
export IRONHERMES_IMAGE="$IMAGE"
echo "INFO runtime=$RT"

ALL_CASES="hardened_boot strict_bounding_set fail_closed_drop compose_up uid_remap uid_invalid_fails_closed volume_env noexec_tmp healthcheck config_template_seed_once gateway_and_web_together cli_entrypoint_drop chromium_hardened devtools_hardened_boot tirith_hardened wrangler_hardened sliplane_equivalent"

PREFIX="ih-smoke-$$"
PROJECT="ihsmoke$$"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ih-smoke.XXXXXX")"
DUMMY_KEY="sk-or-smoke-not-a-real-key"
FAILS=0

cleanup() {
    local n
    for n in $("$RT" ps -a --filter "name=${PREFIX}-" --format '{{.Names}}' 2>/dev/null); do
        "$RT" rm -f "$n" >/dev/null 2>&1
    done
    for n in $("$RT" volume ls --filter "name=${PREFIX}-" --format '{{.Name}}' 2>/dev/null); do
        "$RT" volume rm -f "$n" >/dev/null 2>&1
    done
    if "$RT" compose version >/dev/null 2>&1; then
        "$RT" compose -p "$PROJECT" -f "$REPO_ROOT/docker-compose.yml" down -v >/dev/null 2>&1
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

die() {
    echo "$*" >&2
    exit 1
}

# hardened_flags -- print, one per line, the `run` flags equivalent to the
# `ironhermes` service in docker-compose.yml. ${NAME:-default} is resolved from
# this shell's environment; a null environment entry passes through when set.
hardened_flags() {
    if ! command -v ruby >/dev/null 2>&1 || [ -n "${IH_SMOKE_FORCE_PY:-}" ]; then
        # Linux hosts often lack ruby; python3 + PyYAML is the fallback (same output).
        python3 -c 'import yaml' 2>/dev/null || { echo "neither ruby nor python3+PyYAML found: cannot derive flags from docker-compose.yml" >&2; return 1; }
        python3 -c '
import os, re, sys, yaml
svc = yaml.safe_load(open(sys.argv[1]))["services"]["ironhermes"]
def sub(s):
    return re.sub(r"\$\{(\w+)(?::-([^}]*))?\}",
                  lambda m: (os.environ.get(m.group(1)) or m.group(2) or ""), str(s))
def s(v):
    return ("true" if v else "false") if isinstance(v, bool) else str(v)
out = []
if svc.get("read_only"): out.append("--read-only")
for t in svc.get("tmpfs") or []: out += ["--tmpfs", s(t)]
if svc.get("shm_size"): out += ["--shm-size", s(svc["shm_size"])]
for c in svc.get("cap_drop") or []: out.append("--cap-drop=%s" % c)
for c in svc.get("cap_add") or []: out.append("--cap-add=%s" % c)
for o in svc.get("security_opt") or []: out.append("--security-opt=%s" % o)
for k, f in (("pids_limit", "--pids-limit"), ("mem_limit", "--memory"),
             ("memswap_limit", "--memory-swap"), ("cpus", "--cpus")):
    if svc.get(k): out += [f, s(svc[k])]
for k, v in (svc.get("environment") or {}).items():
    if v is None:
        if k in os.environ: out += ["-e", "%s=%s" % (k, os.environ[k])]
    else:
        out += ["-e", "%s=%s" % (k, sub(s(v)))]
print("\n".join(out))
' "$REPO_ROOT/docker-compose.yml"
        return
    fi
    ruby -ryaml -e '
      svc = YAML.load_file(ARGV[0])["services"]["ironhermes"]
      sub = lambda { |s| s.to_s.gsub(/\$\{(\w+)(?::-([^}]*))?\}/) { (ENV[$1].to_s.empty? ? $2 : ENV[$1]).to_s } }
      out = []
      out << "--read-only" if svc["read_only"]
      (svc["tmpfs"] || []).each { |t| out << "--tmpfs"; out << t }
      out += ["--shm-size", svc["shm_size"].to_s] if svc["shm_size"]
      (svc["cap_drop"] || []).each { |c| out << "--cap-drop=#{c}" }
      (svc["cap_add"] || []).each { |c| out << "--cap-add=#{c}" }
      (svc["security_opt"] || []).each { |o| out << "--security-opt=#{o}" }
      out += ["--pids-limit", svc["pids_limit"].to_s] if svc["pids_limit"]
      out += ["--memory", svc["mem_limit"].to_s] if svc["mem_limit"]
      out += ["--memory-swap", svc["memswap_limit"].to_s] if svc["memswap_limit"]
      out += ["--cpus", svc["cpus"].to_s] if svc["cpus"]
      (svc["environment"] || {}).each do |k, v|
        if v.nil?
          out += ["-e", "#{k}=#{ENV[k]}"] if ENV.key?(k)
        else
          out += ["-e", "#{k}=#{sub.call(v)}"]
        end
      end
      puts out
    ' "$REPO_ROOT/docker-compose.yml"
}

# load_flags -- fill the global array HFLAGS from hardened_flags.
HFLAGS=()
load_flags() {
    local out line
    HFLAGS=()
    out="$(hardened_flags)" || return 1
    while IFS= read -r line; do
        HFLAGS+=("$line")
    done <<<"$out"
}

# start NAME VOL [run flags...] -- detached container on a named volume.
start() {
    local name="$1" vol="$2"
    shift 2
    "$RT" run -d --name "${PREFIX}-${name}" -v "${PREFIX}-${name}-${vol}:/opt/data" \
        -e "OPENROUTER_API_KEY=${DUMMY_KEY}" "$@" "${CASE_IMAGE:-$IMAGE}" >/dev/null
}

# info KEY=VALUE -- an informational line, printed after the case verdict (fd 4).
info() { echo "INFO $1" >&4; }

# chromium_dump CONTAINER [flags...] -- headless about:blank as the app uid; 0 if it rendered.
chromium_dump() {
    local c="$1" out
    shift
    out="$("$RT" exec --user 10000 -e HOME=/opt/data "$c" timeout 90 chromium --headless --disable-gpu "$@" --dump-dom about:blank 2>&1)" || return 1
    grep -q '<html' <<<"$out"
}

cname() { echo "${PREFIX}-$1"; }

# wait_ready CONTAINER -- poll GET / for up to 90 s; dump logs on timeout.
wait_ready() {
    local c="$1" i=0 state
    while [ "$i" -lt 90 ]; do
        state="$("$RT" inspect -f '{{.State.Status}}' "$c" 2>/dev/null)"
        if [ "$state" = "exited" ] || [ "$state" = "dead" ]; then
            echo "container $c exited before becoming ready" >&2
            "$RT" logs "$c" 2>&1 | tail -n 40 >&2
            return 1
        fi
        if "$RT" exec "$c" curl -fsS -o /dev/null http://127.0.0.1:8080/ >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
        i=$((i + 1))
    done
    echo "container $c not ready after 90s" >&2
    "$RT" logs "$c" 2>&1 | tail -n 60 >&2
    return 1
}

# probe CONTAINER UID[:GID] -- run the in-image probe as that user.
probe() {
    "$RT" exec --user "$2" "$1" bash /opt/ironhermes/docker/hardening-probe.sh
}

# expect PROBE_OUTPUT KEY=VALUE...
expect() {
    local out="$1" kv
    shift
    for kv in "$@"; do
        grep -qxF -- "$kv" <<<"$out" || {
            echo "expected $kv; probe said:" >&2
            printf '%s\n' "$out" >&2
            return 1
        }
    done
}

# wait_exit CONTAINER SECONDS -- wait for the container to exit; print its code.
wait_exit() {
    local c="$1" max="$2" i=0 state
    while [ "$i" -lt "$max" ]; do
        state="$("$RT" inspect -f '{{.State.Status}}' "$c" 2>/dev/null)"
        if [ "$state" = "exited" ] || [ "$state" = "dead" ]; then
            "$RT" inspect -f '{{.State.ExitCode}}' "$c"
            return 0
        fi
        sleep 1
        i=$((i + 1))
    done
    return 1
}

# --- Cases -------------------------------------------------------------------
hardened_expectations() { # hardened_expectations PROBE_OUTPUT
    expect "$1" APP_UID=10000 APP_CAPEFF=0000000000000000 APP_CAPBND=00000000000000c3 \
        APP_NONEWPRIVS=1 ROOTFS_RO=yes TMP_NOEXEC=yes VARTMP_NOEXEC=yes SHM_NOEXEC=yes \
        HTTP_ROOT=200 VOLUME_FOREIGN_FILES=0 APP_ENV_HOME=/opt/data
}

case_hardened_boot() {
    load_flags || die "could not derive flags"
    start hb data "${HFLAGS[@]}" || die "run failed"
    wait_ready "$(cname hb)" || die "not ready"
    hardened_expectations "$(probe "$(cname hb)" 10000)"
}

case_strict_bounding_set() {
    start sb data || die "run failed"
    wait_ready "$(cname sb)" || die "not ready"
    expect "$(probe "$(cname sb)" 10000)" APP_CAPBND=0000000000000000 APP_NONEWPRIVS=1 \
        APP_UID=10000 HTTP_ROOT=200
}

case_fail_closed_drop() {
    local code logs
    start fc data --cap-drop=ALL || die "run failed"
    code="$(wait_exit "$(cname fc)" 60)" || die "container did not exit within 60s"
    [ "$code" != "0" ] || die "container exited 0 with the drop impossible"
    logs="$("$RT" logs "$(cname fc)" 2>&1)"
    ! grep -qiE 'listening|server bound|Gateway started in background' <<<"$logs" \
        || die "web server / gateway started although the drop failed"
}

case_compose_up() {
    local port out cid
    "$RT" compose version >/dev/null 2>&1 || { echo "SKIP compose_up (no compose provider)" >&3; return 0; }
    port=$((20000 + $$ % 20000))
    IRONHERMES_PORT="$port" OPENROUTER_API_KEY="$DUMMY_KEY" "$RT" compose -p "$PROJECT" \
        -f "$REPO_ROOT/docker-compose.yml" up -d >/dev/null || die "compose up failed"
    cid="$("$RT" compose -p "$PROJECT" -f "$REPO_ROOT/docker-compose.yml" ps -q ironhermes)"
    [ -n "$cid" ] || die "no compose container"
    wait_ready "$cid" || die "not ready"
    out="$(probe "$cid" 10000)"
    hardened_expectations "$out"
}

case_uid_remap() {
    local c nss
    c="$(cname ur)"
    load_flags || die "could not derive flags"
    start ur data "${HFLAGS[@]}" -e IRONHERMES_UID=12345 -e IRONHERMES_GID=12345 || die "run failed"
    wait_ready "$c" || die "not ready"
    expect "$(probe "$c" 12345:12345)" APP_UID=12345 VOLUME_FOREIGN_FILES=0 APP_ENV_LD_PRELOAD=libnss_wrapper.so
    "$RT" exec "$c" grep -q '^ironhermes:x:12345:12345:' /run/ih-nss/passwd || die "remapped passwd line missing"
    nss="$("$RT" exec --user 12345:12345 -e LD_PRELOAD=libnss_wrapper.so \
        -e NSS_WRAPPER_PASSWD=/run/ih-nss/passwd -e NSS_WRAPPER_GROUP=/run/ih-nss/group "$c" \
        python3 -c 'import getpass; print(getpass.getuser())')"
    [ "$nss" = "ironhermes" ] || die "name lookup for uid 12345 gave '$nss'"
}

case_uid_invalid_fails_closed() {
    local v code
    load_flags || die "could not derive flags"
    for v in abc 0; do
        start "ui$v" "data$v" "${HFLAGS[@]}" -e "IRONHERMES_UID=$v" || die "run failed"
        code="$(wait_exit "$(cname "ui$v")" 60)" || die "IRONHERMES_UID=$v: no exit within 60s"
        [ "$code" != "0" ] || die "IRONHERMES_UID=$v exited 0"
        logs="$("$RT" logs "$(cname "ui$v")" 2>&1)"
        grep -q 'must be a non-zero number' <<<"$logs" \
            || die "IRONHERMES_UID=$v: error message missing"
    done
}

case_volume_env() {
    local c out
    c="$(cname ve)"
    load_flags || die "could not derive flags"
    start ve data "${HFLAGS[@]}" || die "run failed"
    wait_ready "$c" || die "not ready"
    out="$(probe "$c" 10000)"
    expect "$out" APP_ENV_HOME=/opt/data APP_ENV_NPM_CONFIG_PREFIX=/opt/data/.local \
        APP_ENV_PYTHONUSERBASE=/opt/data/.local APP_ENV_PIP_USER=1
    printf '%s\n' "$out" | grep -q '^APP_ENV_PATH=/opt/data/.local/bin:' || die "PATH does not start with /opt/data/.local/bin"
}

case_noexec_tmp() {
    local c out
    c="$(cname nt)"
    load_flags || die "could not derive flags"
    start nt data "${HFLAGS[@]}" || die "run failed"
    wait_ready "$c" || die "not ready"
    out="$("$RT" exec --user 10000 "$c" sh -c \
        'printf "#!/bin/sh\necho hi\n" >/tmp/s.sh; chmod +x /tmp/s.sh; /tmp/s.sh 2>&1; echo "print(42)" >/tmp/s.py; python3 /tmp/s.py')"
    printf '%s\n' "$out" | grep -q 'Permission denied' || die "direct exec on /tmp was not blocked: $out"
    printf '%s\n' "$out" | grep -qx '42' || die "python3 /tmp/s.py did not print 42: $out"
}

case_healthcheck() {
    local c test
    c="$(cname hc)"
    test="$("$RT" image inspect "$IMAGE" --format '{{json .Config.Healthcheck.Test}}')"
    printf '%s' "$test" | grep -q '127.0.0.1' || die "healthcheck does not target 127.0.0.1: $test"
    ! printf '%s' "$test" | grep -q '/health' || die "healthcheck targets /health: $test"
    load_flags || die "could not derive flags"
    start hc data "${HFLAGS[@]}" || die "run failed"
    wait_ready "$c" || die "not ready"
    if [ "$RT" = "podman" ]; then
        "$RT" healthcheck run "$c" || die "podman healthcheck run failed"
    else
        local i=0 s=""
        while [ "$i" -lt 90 ]; do
            s="$("$RT" inspect -f '{{.State.Health.Status}}' "$c" 2>/dev/null)"
            [ "$s" = "healthy" ] && return 0
            sleep 1
            i=$((i + 1))
        done
        die "health state is '$s', not healthy"
    fi
}

case_config_template_seed_once() {
    local c tpl="$WORK/template.yaml"
    c="$(cname ct)"
    load_flags || die "could not derive flags"
    { cat "$REPO_ROOT/cli-config.yaml.example"; echo "# ih-smoke-template-v1"; } >"$tpl"
    chmod 644 "$tpl"
    start ct data "${HFLAGS[@]}" -v "$tpl:/etc/ih-template.yaml:ro" -e IRONHERMES_CONFIG_TEMPLATE=/etc/ih-template.yaml \
        || die "run failed"
    wait_ready "$c" || die "not ready"
    "$RT" exec "$c" grep -q 'ih-smoke-template-v1' /opt/data/config.yaml || die "config.yaml not seeded from the template"
    "$RT" rm -f "$c" >/dev/null
    { cat "$REPO_ROOT/cli-config.yaml.example"; echo "# ih-smoke-template-v2"; } >"$tpl"
    start ct data "${HFLAGS[@]}" -v "$tpl:/etc/ih-template.yaml:ro" -e IRONHERMES_CONFIG_TEMPLATE=/etc/ih-template.yaml \
        || die "second run failed"
    wait_ready "$c" || die "second boot not ready"
    "$RT" exec "$c" grep -q 'ih-smoke-template-v1' /opt/data/config.yaml || die "v1 seed was lost on the second boot"
    ! "$RT" exec "$c" grep -q 'ih-smoke-template-v2' /opt/data/config.yaml || die "second boot re-seeded config.yaml from v2"
}

case_gateway_and_web_together() {
    local c
    c="$(cname gw)"
    load_flags || die "could not derive flags"
    start gw data "${HFLAGS[@]}" || die "run failed"
    wait_ready "$c" || die "not ready"
    logs="$("$RT" logs "$c" 2>&1)"
    grep -q 'Gateway started in background' <<<"$logs" || die "gateway launch line missing"
    expect "$(probe "$c" 10000)" HTTP_ROOT=200
    [ "$("$RT" exec "$c" stat -c %u /opt/data/logs/gateway.log)" = "10000" ] || die "gateway.log is not owned by the app uid"
}

case_cli_entrypoint_drop() {
    local v="${PREFIX}-cli"
    load_flags || die "could not derive flags"
    "$RT" run --rm --name "${v}-ok" -v "${v}-data:/opt/data" "${HFLAGS[@]}" \
        --entrypoint /usr/bin/tini "$IMAGE" -- /opt/ironhermes/docker/entrypoint.sh version >/dev/null 2>&1 \
        || die "CLI entrypoint failed under the hardened flags"
    if "$RT" run --rm --name "${v}-bad" -v "${v}-data:/opt/data" --cap-drop=ALL \
        --entrypoint /usr/bin/tini "$IMAGE" -- /opt/ironhermes/docker/entrypoint.sh version >/dev/null 2>&1; then
        die "CLI entrypoint succeeded with the drop impossible"
    fi
}

# --- Plan 03: browser, sidecar, wrangler -------------------------------------
# image_up NAME IMAGE -- hardened container from IMAGE, ready, on its own volume.
image_up() {
    CASE_IMAGE="$2"
    load_flags || die "could not derive flags"
    start "$1" data "${HFLAGS[@]}" || die "run failed ($2)"
    wait_ready "$(cname "$1")" || die "not ready ($2)"
}

case_chromium_hardened() {
    local n=0 img c out kb
    for img in "$IMAGE" "$DEVTOOLS_IMAGE"; do
        n=$((n + 1))
        c="$(cname "ch$n")"
        image_up "ch$n" "$img"
        out="$(probe "$c" 10000)"
        expect "$out" APP_ENV_IRONHERMES_BROWSER_NO_SANDBOX=1
        kb="$(sed -n 's/^SHM_SIZE_KB=//p' <<<"$out")"
        [ "${kb:-0}" -ge 65536 ] 2>/dev/null || die "$img: /dev/shm is ${kb:-?} KiB, need >= 65536"
        chromium_dump "$c" --no-sandbox || die "$img: hardened chromium --no-sandbox did not render about:blank"
        if [ "$n" = 1 ]; then
            if chromium_dump "$c"; then info "chromium_sandbox_hardened=ok"; else info "chromium_sandbox_hardened=crash"; fi
        fi
    done
}

case_devtools_hardened_boot() {
    image_up dh "$DEVTOOLS_IMAGE"
    hardened_expectations "$(probe "$(cname dh)" 10000)"
}

case_tirith_hardened() {
    local n=0 img c ver out home=/opt/data/security/tirith-home
    for img in "$IMAGE" "$DEVTOOLS_IMAGE"; do
        n=$((n + 1))
        c="$(cname "ti$n")"
        image_up "ti$n" "$img"
        ver="$("$RT" exec --user 10000 "$c" ironhermes-tirith --version)"
        grep -q 'tirith-core 0.4.2' <<<"$ver" || die "$img: tirith --version said '$ver'"
        echo "tirith --version ($img): $ver"
        "$RT" exec --user 10000 "$c" sh -c "mkdir -p /opt/data/security && mkdir -m 0700 -p $home" || die "$img: cannot create tirith home"
        [ "$("$RT" exec "$c" stat -c '%a %u' "$home")" = "700 10000" ] || die "$img: tirith home is not 0700 owned by 10000"
        out="$("$RT" exec -i --user 10000 "$c" sh -c \
            "printf '%s\n' '{\"id\":1,\"op\":\"version\"}' '{\"id\":2,\"op\":\"ping\"}' | env -i HOME=$home PATH=/usr/bin:/bin /usr/local/bin/ironhermes-tirith")" \
            || die "$img: sidecar round trip failed"
        [ "$(grep -c . <<<"$out")" = 2 ] || die "$img: expected 2 response lines, got: $out"
        { grep -q '"id":1' <<<"$out" && grep -q '"id":2' <<<"$out"; } || die "$img: response ids missing: $out"
    done
}

case_wrangler_hardened() {
    local c pathv w=/opt/data/ih-smoke-worker
    c="$(cname wr)"
    image_up wr "$DEVTOOLS_IMAGE"
    pathv="$(sed -n 's/^APP_ENV_PATH=//p' <<<"$(probe "$c" 10000)")"
    wr() {
        "$RT" exec --user 10000 -e HOME=/opt/data -e "PATH=$pathv" -e NPM_CONFIG_PREFIX=/opt/data/.local \
            -e WRANGLER_SEND_METRICS=false -w "$w" "$c" "$@"
    }
    "$RT" exec --user 10000 "$c" mkdir -p "$w" || die "cannot create worker dir"
    "$RT" exec --user 10000 "$c" sh -c "printf 'name = \"ih-smoke\"\nmain = \"index.js\"\ncompatibility_date = \"2025-01-01\"\n' >$w/wrangler.toml; printf 'export default { async fetch() { return new Response(\"ok\"); } };\n' >$w/index.js"
    wr wrangler --version | grep -q '4.140.0' || die "wrangler is not 4.140.0"
    [ "$(wr npm config get prefix)" = "/opt/data/.local" ] || die "npm prefix is not /opt/data/.local"
    wr wrangler deploy --dry-run --outdir /tmp/ih-wr-out || die "wrangler deploy --dry-run failed"
}

case_sliplane_equivalent() {
    local c out
    c="$(cname sl)"
    CASE_IMAGE="$DEVTOOLS_IMAGE"
    start sl data || die "run failed"
    wait_ready "$c" || die "not ready"
    out="$(probe "$c" 10000)"
    expect "$out" APP_CAPBND=0000000000000000 APP_NONEWPRIVS=1 APP_UID=10000 HTTP_ROOT=200
    # CR-01: root sources these on every start, so the app uid must not be able to
    # rewrite them (or add files beside them) -- root-owned, not writable by 10000.
    local f
    for f in priv-drop.sh entrypoint.sh web-entrypoint.sh seed-bundled.sh; do
        [ "$("$RT" exec "$c" stat -c %u "/opt/ironhermes/docker/$f")" = "0" ] \
            || die "/opt/ironhermes/docker/$f is not root-owned"
        if "$RT" exec --user 10000 "$c" sh -c ": >> /opt/ironhermes/docker/$f" 2>/dev/null; then
            die "uid 10000 can write /opt/ironhermes/docker/$f"
        fi
    done
    if "$RT" exec --user 10000 "$c" touch /opt/ironhermes/docker/.ih-smoke-new 2>/dev/null; then
        die "uid 10000 can create files in /opt/ironhermes/docker"
    fi
    if chromium_dump "$c" --no-sandbox; then info "chromium_nosandbox_noflags=ok"; else info "chromium_nosandbox_noflags=crash"; fi
    if chromium_dump "$c"; then info "chromium_sandbox_noflags=ok"; else info "chromium_sandbox_noflags=crash"; fi
}

# --- Runner ------------------------------------------------------------------
# fd 3 carries a case's own SKIP line past the output capture.
run_case() {
    local name="$1" out skip infof
    if ! declare -F "case_$name" >/dev/null 2>&1; then
        echo "FAIL unknown-case $name"
        FAILS=$((FAILS + 1))
        return 0
    fi
    out="$WORK/out.$name"
    skip="$WORK/skip.$name"
    infof="$WORK/info.$name"
    if ( "case_$name" ) 3>"$skip" 4>"$infof" >"$out" 2>&1; then
        if [ -s "$skip" ]; then
            cat "$skip"
        else
            echo "PASS $name"
        fi
    else
        echo "FAIL $name"
        sed 's/^/    /' "$out"
        FAILS=$((FAILS + 1))
    fi
    if [ -s "$infof" ]; then cat "$infof"; fi
}

if [ "$#" -ge 1 ]; then
    for c in "$@"; do run_case "$c"; done
else
    for c in $ALL_CASES; do run_case "$c"; done
fi

if [ "$FAILS" -gt 0 ]; then
    echo "FAILED: $FAILS case(s)"
    exit 1
fi
exit 0
