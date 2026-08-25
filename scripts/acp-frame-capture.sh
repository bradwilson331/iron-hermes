#!/usr/bin/env bash
# scripts/acp-frame-capture.sh
#
# Phase 47.7-06 — transparent stdio wrapper for `ironhermes acp`, meant to be
# pointed at via buzz-acp's BUZZ_ACP_AGENT_COMMAND. Tees both directions of
# the JSON-RPC conversation to disk (request frames from buzz-acp -> agent,
# response frames from agent -> buzz-acp) and then hands off to the real
# agent byte-identically.
#
# Contract: the bytes reaching the agent and the bytes reaching buzz-acp
# must be identical to an unwrapped run. This wrapper never writes a single
# byte to stdout itself — all diagnostics go to stderr, which tracing
# already owns on this path (see docs/ACP.md "Debugging").
#
# Env vars:
#   IRONHERMES_ACP_BIN         - path to the real ironhermes binary
#                                 (default: "ironhermes", resolved via PATH)
#   IRONHERMES_ACP_CAPTURE_DIR - directory for capture files
#                                 (default: $IRONHERMES_HOME/acp-captures,
#                                 or $HOME/.ironhermes/acp-captures)
#
# All positional arguments are forwarded to the agent unchanged, so the
# documented `--profile,<name>,acp` BUZZ_ACP_AGENT_ARGS form still applies
# verbatim (see docs/ACP.md "Buzz as ACP client" > Configuration).

set -u
set -o pipefail

agent_bin="${IRONHERMES_ACP_BIN:-ironhermes}"
capture_dir="${IRONHERMES_ACP_CAPTURE_DIR:-${IRONHERMES_HOME:-$HOME/.ironhermes}/acp-captures}"

# Set the umask BEFORE creating anything. The capture files hold full live-session
# prompt text, tool args and tool output; creating them under the ambient umask
# (0644 on a default `umask 022`) and chmod-ing afterwards leaves a window in which
# they are world-readable.
umask 077

if ! mkdir -p "$capture_dir" 2>/dev/null; then
  printf 'acp-frame-capture: failed to create capture dir %s\n' "$capture_dir" >&2
  exit 1
fi
# `mkdir -p` on a pre-existing directory succeeds WITHOUT altering its mode, and this
# script does not `set -e`, so an unchecked chmod would let a failure here (a directory
# the operator does not own, a read-only mount, an ACL edge) pass silently and capture
# live session text into a directory whose permissions were never verified. The 0700
# directory is the outer containment, so refuse rather than proceed.
if ! chmod 700 "$capture_dir"; then
  printf 'acp-frame-capture: refusing to capture into %s (cannot set mode 700)\n' \
    "$capture_dir" >&2
  exit 1
fi

timestamp="$(date -u +%Y%m%dT%H%M%S)"
req_file="$capture_dir/${timestamp}-$$.request.ndjson"
resp_file="$capture_dir/${timestamp}-$$.response.ndjson"

if ! : > "$req_file" || ! : > "$resp_file"; then
  printf 'acp-frame-capture: failed to create capture files under %s\n' "$capture_dir" >&2
  exit 1
fi
# The umask above already creates both at 0600. This covers the one case it cannot:
# truncating a file that somehow already existed keeps that file's original mode.
if ! chmod 600 "$req_file" "$resp_file"; then
  printf 'acp-frame-capture: refusing to capture (cannot set mode 600 on capture files)\n' >&2
  exit 1
fi

printf 'acp-frame-capture: agent=%s request=%s response=%s\n' \
  "$agent_bin" "$req_file" "$resp_file" >&2

# One three-stage pipeline — request tee, agent, response tee.
#
# This deliberately does NOT use process substitution (`exec 3> >(tee ...)`). Bash does
# not wait for process-substitution children, so the wrapper would reach `exit` while the
# tee holding a duplicate of its stdout was still draining: the agent's trailing response
# frames are then still in flight when the wrapper's exit status is reaped. A client that
# treats child exit as end-of-stream, or that drops its stdout pipe on exit (the common
# `Child::wait`-then-drop shape), loses whatever had not yet been flushed. That breaks the
# contract at the top of this file — unwrapped, the agent's last byte is written before
# the agent exits, and a parent process is entitled to assume that ordering.
#
# Bash waits for a foreground pipeline in full, so by the time `status=` runs below, both
# tees have exited and everything they held has reached stdout and disk. Making the
# request tee a pipeline member too means it dies of SIGPIPE when the agent exits, rather
# than being orphaned blocked on a read of the wrapper's stdin.
#
# `$!` + `wait` would also work but only on bash 4.4+, which sets `$!` for process
# substitutions; macOS still ships bash 3.2.
tee -a "$req_file" | "$agent_bin" "$@" | tee -a "$resp_file"
# Must be the very next command — any other command resets PIPESTATUS. Index 1 is the
# agent itself; index 0/2 are the tees, whose status is not ours to propagate.
status=${PIPESTATUS[1]}

exit "$status"
