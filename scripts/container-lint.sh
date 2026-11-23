#!/usr/bin/env bash
# Container lint gate (Phase 54.11, D-04 #10 / D-17). Gate 11 of scripts/ci-gates.sh.
#
#   bash scripts/container-lint.sh              static checks on the repo
#   bash scripts/container-lint.sh --image REF  Dockle on a built image
#
# Prints `OK <check>` / `FAIL <check>: <why>` / `SKIP <check> (<why>)` and exits
# 1 on any FAIL. A missing linter or container runtime is a FAIL when CI is set
# and a SKIP otherwise. hadolint and dockle run from the local binary when it is
# on PATH, else from the digest-pinned tool images below via podman or docker.
#
# Env: IH_LINT_RT forces the container runtime (podman|docker).

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${ROOT}" || exit 1

# Tool images, multi-arch INDEX digests (amd64 + arm64) resolved 2026-11-01.
HADOLINT_IMAGE="docker.io/hadolint/hadolint:v2.12.0-debian@sha256:27173fe25e062448490a32de410c08491c626a0bef360aa2ce5d5bdd9384b50d"
DOCKLE_IMAGE="docker.io/goodwithtech/dockle:v0.4.14@sha256:68f7473909b49013f97984e9917fb7edd0c440bf15e38f41449860f8a2680d51"

DOCKERFILES=(Dockerfile docker/dev-tools.Dockerfile docker/ros2-humble-harness/Dockerfile)
FAILED=0
TMP="$(mktemp -d "${TMPDIR:-/tmp}/container-lint.XXXXXX")"
trap 'rm -rf "${TMP}"' EXIT

indent() { sed 's/^/    /'; }
ok()   { echo "OK $1"; }
fail() { echo "FAIL $1: $2"; FAILED=1; }
skip() { echo "SKIP $1 ($2)"; }
# Missing tool: hard failure in CI, soft skip on a workstation.
missing() {
    if [ -n "${CI:-}" ]; then fail "$1" "$2"; else skip "$1" "$2"; fi
}

# Prints the usable container runtime, or nothing.
pick_rt() {
    local rt
    for rt in ${IH_LINT_RT:-docker podman}; do
        if command -v "${rt}" >/dev/null 2>&1 && "${rt}" info >/dev/null 2>&1; then
            echo "${rt}"
            return 0
        fi
    done
    return 1
}

# ---- (b) pins ---------------------------------------------------------------
# Every FROM must be @sha256:<64 hex>, a ${ARG} whose default is digest-pinned
# (or that has no default, so a missing build arg fails the build), or a prior
# stage name. Prints one line per offending FROM; returns 1 if any.
check_pins() {
    local file="$1" bad=0 stages=" " line ref name default
    while IFS= read -r line; do
        # Drop `FROM`, any --flag=value options, keep ref and optional `AS name`.
        # shellcheck disable=SC2086 # word splitting is the point
        set -- ${line#FROM }
        while [ $# -gt 0 ] && [ "${1#--}" != "$1" ]; do shift; done
        ref="${1:-}"
        name=""
        if [ "${2:-}" != "" ] && [ "$(echo "$2" | tr '[:upper:]' '[:lower:]')" = "as" ]; then
            name="$(echo "${3:-}" | tr '[:upper:]' '[:lower:]')"
        fi
        if echo "${ref}" | grep -qE '@sha256:[0-9a-f]{64}$'; then
            :
        elif [ "${ref}" = "scratch" ]; then
            :
        elif echo "${stages}" | grep -qF " $(echo "${ref}" | tr '[:upper:]' '[:lower:]') "; then
            :
        elif echo "${ref}" | grep -qE '^\$\{?[A-Za-z_][A-Za-z0-9_]*\}?$'; then
            local arg
            arg="${ref//[\$\{\}]/}"
            if grep -qE "^ARG ${arg}=" "${file}"; then
                default="$(grep -E "^ARG ${arg}=" "${file}" | head -n1 | cut -d= -f2-)"
                if ! echo "${default}" | grep -qE '@sha256:[0-9a-f]{64}$'; then
                    echo "${file}: ARG ${arg} default is not digest-pinned: ${default}"
                    bad=1
                fi
            fi
        else
            echo "${file}: unpinned FROM: ${line}"
            bad=1
        fi
        [ -n "${name}" ] && stages="${stages}${name} "
    done < <(grep -E '^FROM ' "${file}")
    return "${bad}"
}

static_hadolint() {
    local out rc rt
    if command -v hadolint >/dev/null 2>&1; then
        out="$(hadolint --config .hadolint.yaml "${DOCKERFILES[@]}" 2>&1)" && rc=0 || rc=$?
    elif rt="$(pick_rt)"; then
        out="$("${rt}" run --rm -v "${ROOT}:/work:ro" -w /work "${HADOLINT_IMAGE}" \
            hadolint --config .hadolint.yaml "${DOCKERFILES[@]}" 2>&1)" && rc=0 || rc=$?
    else
        missing hadolint "no hadolint binary and no podman/docker"
        return
    fi
    if [ "${rc}" -eq 0 ]; then ok hadolint; else fail hadolint "exit ${rc}"; echo "${out}" | indent; fi
}

static_pins() {
    local f out bad=0
    for f in "${DOCKERFILES[@]}"; do
        out="$(check_pins "${f}")" || { bad=1; echo "${out}" | indent; }
    done
    if [ "${bad}" -eq 0 ]; then ok pins; else fail pins "unpinned FROM above"; fi

    # Positive control: the checker must reject an unpinned FROM, else the
    # check above proves nothing.
    printf 'FROM docker.io/library/debian:trixie-slim\nRUN true\n' >"${TMP}/Unpinned.Dockerfile"
    if check_pins "${TMP}/Unpinned.Dockerfile" >/dev/null; then
        fail pin_control "check_pins accepted an unpinned FROM"
    else
        ok pin_control
    fi
}

static_healthcheck() {
    local n line
    n="$(grep -cE '^HEALTHCHECK ' Dockerfile)"
    line="$(grep -E '^HEALTHCHECK ' Dockerfile | head -n1)"
    if [ "${n}" -ne 1 ]; then
        fail healthcheck "expected exactly one HEALTHCHECK, found ${n}"
    elif ! echo "${line}" | grep -qF '127.0.0.1'; then
        fail healthcheck "target must be 127.0.0.1"
    elif echo "${line}" | grep -qF '/health'; then
        fail healthcheck "must probe /, never /health (only / is unauthenticated)"
    else
        ok healthcheck
    fi
}

static_labels() {
    local k miss=""
    for k in title source licenses; do
        grep -qE "org\.opencontainers\.image\.${k}=" Dockerfile || miss="${miss} ${k}"
    done
    if [ -z "${miss}" ]; then ok labels; else fail labels "missing OCI label(s):${miss}"; fi
}

static_compose() {
    local out rc
    if ! command -v ruby >/dev/null 2>&1; then
        missing compose "ruby not found"
        return
    fi
    out="$(ruby -ryaml -e '
        svcs = YAML.load_file(ARGV[0]).fetch("services")
        errs = []
        svcs.each do |n, s|
          (s["volumes"] || []).each do |v|
            src = v.is_a?(Hash) ? "#{v["source"]}:#{v["target"]}" : v.to_s
            errs << "#{n}: runtime socket mount #{src}" if src =~ /\.sock\b|\/var\/run\/(docker|podman)/
          end
        end
        ih = svcs["ironhermes"] or abort("no ironhermes service")
        errs << "ironhermes: read_only must be true" unless ih["read_only"] == true
        errs << "ironhermes: cap_drop must include ALL" unless (ih["cap_drop"] || []).include?("ALL")
        errs << "ironhermes: security_opt must include no-new-privileges" unless (ih["security_opt"] || []).any? { |o| o.to_s.start_with?("no-new-privileges") }
        puts errs
        exit(errs.empty? ? 0 : 1)
    ' docker-compose.yml 2>&1)" && rc=0 || rc=$?
    if [ "${rc}" -eq 0 ]; then ok compose; else fail compose "see below"; echo "${out}" | indent; fi
}

static_shellcheck() {
    local files=() f
    for f in docker/*.sh scripts/container-lint.sh scripts/tests/container_hardened_smoke.sh; do
        [ -f "${f}" ] && files+=("${f}")
    done
    if ! command -v shellcheck >/dev/null 2>&1; then
        missing shellcheck "shellcheck not found"
        return
    fi
    if shellcheck "${files[@]}"; then ok shellcheck; else fail shellcheck "findings above"; fi
}

# ---- (g) D-17 tmp-exec guard ------------------------------------------------
# No Command::new(...) whose argument names a temp dir or a literal /tmp path in
# non-test Rust. shortcut: single-line match only, so a path built on an earlier
# line (`let p = temp_dir().join(..); Command::new(&p)`) is not seen; upgrade to
# a syn-based check if that pattern ever shows up.
TMP_EXEC_RE='Command::new\([^)]*(temp_dir|"/tmp)'
tmp_exec_hits() {
    # Non-test = not under a tests/ dir and before the first #[cfg(test)] line.
    local f
    grep -rlE "${TMP_EXEC_RE}" --include='*.rs' crates providers 2>/dev/null | grep -v '/tests/' | while IFS= read -r f; do
        awk -v F="${f}" -v RE="${TMP_EXEC_RE}" '/#\[cfg\(test\)\]/ { exit } $0 ~ RE { print F ":" NR ": " $0 }' "${f}"
    done
}

static_tmp_exec_guard() {
    local hits
    # Positive control: the regex must match a known violating sample.
    if ! echo 'Command::new(std::env::temp_dir().join("x"))' | grep -qE "${TMP_EXEC_RE}" \
        || ! echo 'Command::new("/tmp/payload")' | grep -qE "${TMP_EXEC_RE}"; then
        fail tmp_exec_guard "positive control: regex did not match a violating sample"
        return
    fi
    hits="$(tmp_exec_hits)"
    if [ -z "${hits}" ]; then ok tmp_exec_guard; else fail tmp_exec_guard "executes from a temp dir (D-17)"; echo "${hits}" | indent; fi
}

# ---- --image: Dockle --------------------------------------------------------
image_dockle() {
    local ref="$1" ids=() id out rc rt
    while IFS= read -r id; do
        ids+=(-i "${id}")
    done < <(grep -vE '^[[:space:]]*(#|$)' .dockleignore | awk '{print $1}')

    if command -v dockle >/dev/null 2>&1; then
        out="$(dockle --exit-code 1 --exit-level warn ${ids[@]+"${ids[@]}"} "${ref}" 2>&1)" && rc=0 || rc=$?
    elif rt="$(pick_rt)"; then
        if ! "${rt}" save -o "${TMP}/image.tar" "${ref}" 2>"${TMP}/save.err"; then
            fail dockle "cannot save ${ref}: $(cat "${TMP}/save.err")"
            return
        fi
        out="$("${rt}" run --rm -v "${TMP}/image.tar:/image.tar:ro" "${DOCKLE_IMAGE}" \
            --input /image.tar --exit-code 1 --exit-level warn ${ids[@]+"${ids[@]}"} 2>&1)" && rc=0 || rc=$?
    else
        missing dockle "no dockle binary and no podman/docker"
        return
    fi
    if [ "${rc}" -eq 0 ]; then ok dockle; else fail dockle "exit ${rc}"; fi
    echo "${out}" | indent
}

if [ "${1:-}" = "--image" ]; then
    [ -n "${2:-}" ] || { echo "usage: $0 --image REF" >&2; exit 2; }
    image_dockle "$2"
else
    static_hadolint
    static_pins
    static_healthcheck
    static_labels
    static_compose
    static_shellcheck
    static_tmp_exec_guard
fi
exit "${FAILED}"
