#!/usr/bin/env bash
# Phase 58.1 — live UAT on spark-e309 (Linux, cgroup v2, systemd --user with linger).
#
# Proves what fakes and static checks on a Mac cannot (D-03):
#   preflight   D-08/D-14  unit caps (MemoryHigh/MemoryMax/OOMPolicy/KillMode) as systemd
#                          reports them AND as the kernel enforces them; scope probe works
#   scope       D-02/D-04  an agent `setsid nohup sleep 4242 & disown` lands in its own
#                          ironhermes-jobs.slice/ironhermes-job-*.scope, not the gateway cgroup
#   restart     D-06/D-11  gateway restart: scoped job survives, in-unit background job dies;
#                          `systemctl --user stop <scope>` removes the scoped job
#   rss-before  D-04/D-05  record gateway MainPID / RSS / NRestarts before a delegate fan-out
#   rss-after              assert no restart, RSS <= RSS_LIMIT_KB, memory.current < MemoryHigh,
#                          and plan 02's "delegate batch collected" line reached gateway.log
#
# Stage order: preflight -> scope -> restart -> rss-before -> (fan-out by hand) -> rss-after.
# Exit codes: 0 all checks passed, 1 a check failed, 2 bad usage or not a spark-style host.
# Env: RSS_LIMIT_KB (default 524288 = 512 MiB, ~3.7x the 137 MB measured in D-10).
#
# No `set -e`: every check must run and report. Prints PIDs, cgroup paths, systemd
# property values and RSS only — never env, .env or config contents.
set -uo pipefail

usage() {
  echo "usage: $0 {preflight|scope|restart|rss-before|rss-after}" >&2
  exit 2
}

# Validate the subcommand FIRST so a typo exits 2 on every platform.
case "${1:-}" in
  preflight|scope|restart|rss-before|rss-after) ;;
  *) usage ;;
esac
STAGE="$1"

if [ "$(uname -s)" != "Linux" ] || ! command -v systemctl >/dev/null 2>&1 \
   || ! systemctl --user show-environment >/dev/null 2>&1; then
  echo "!! spark-only UAT: needs Linux with a reachable systemd --user manager" >&2
  exit 2
fi

FAIL=0
GW=ironhermes-gateway
WEB=ironhermes-web
STATE_FILE="${TMPDIR:-/tmp}/58.1-uat-rss.state"
GW_LOG="$HOME/.ironhermes/logs/gateway.log"
MEM_HIGH=6442450944
MEM_MAX=8589934592

ok()  { echo "✓ $*"; }
bad() { echo "!! $*"; FAIL=1; }

gw_pid() { systemctl --user show -p MainPID --value "$GW"; }
# cgroup v2 is a single "0::/path" line; take the third colon-field onward.
cg_of() { cut -d: -f3- "/proc/$1/cgroup" 2>/dev/null; }
# prop UNIT NAME -> value
prop() { systemctl --user show "$1" -p "$2" --value 2>/dev/null; }
# wait_for PATTERN SECONDS -> newest matching PID on stdout, empty + rc 1 on timeout
wait_for() {
  local pat="$1" secs="$2" waited=0 pid
  while [ "$waited" -lt "$secs" ]; do
    pid=$(pgrep -nxf "$pat" 2>/dev/null)
    if [ -n "$pid" ]; then echo "$pid"; return 0; fi
    sleep 2; waited=$((waited + 2))
  done
  return 1
}
# newest PID of an exact command line, or empty
pid_of() { pgrep -nxf "$1" 2>/dev/null; }
# no_stale PATTERN: fail unless NO process already matches. wait_for returns the newest
# match, so a leftover (or hand-started) one would be mistaken for the agent's launch.
no_stale() {
  local stale
  stale=$(pgrep -xf "$1" 2>/dev/null | tr '\n' ' ')
  if [ -n "$stale" ]; then
    bad "stale '$1' already running (PIDs: $stale); stop it (pkill -xf '$1') and rerun this stage"
    return 1
  fi
}

check_unit() {
  local unit="$1" want val cg
  for pair in "MemoryHigh=$MEM_HIGH" "MemoryMax=$MEM_MAX" "OOMPolicy=continue" "KillMode=control-group"; do
    local name="${pair%%=*}"; want="${pair#*=}"
    val=$(prop "$unit" "$name")
    if [ "$val" = "$want" ]; then ok "$unit $name=$val"
    else bad "D-08/D-14: $unit $name is '$val', expected '$want'"; fi
  done
  if [ "$(prop "$unit" ActiveState)" = "active" ]; then
    cg=$(prop "$unit" ControlGroup)
    val=$(cat "/sys/fs/cgroup${cg}/memory.max" 2>/dev/null)
    if [ "$val" = "$MEM_MAX" ]; then ok "$unit kernel memory.max=$val (enforced, not just configured)"
    else bad "D-14: $unit cgroup memory.max is '$val', expected '$MEM_MAX' (cap is a silent no-op)"; fi
    val=$(cat "/sys/fs/cgroup${cg}/memory.current" 2>/dev/null)
    if [ -n "$val" ]; then ok "$unit memory.current=$((val / 1048576)) MiB (input for retuning 6G/8G)"
    else bad "D-14: $unit memory.current unreadable at /sys/fs/cgroup${cg}"; fi
  else
    bad "D-08: $unit is not active; start it before the UAT"
  fi
}

stage_preflight() {
  local t out
  t=$(stat -fc %T /sys/fs/cgroup 2>/dev/null)
  if [ "$t" = "cgroup2fs" ]; then ok "cgroup v2 (cgroup2fs)"
  else bad "D-08: /sys/fs/cgroup is '$t', expected cgroup2fs"; fi

  t=$(loginctl show-user "$USER" -p Linger --value 2>/dev/null)
  if [ "$t" = "yes" ]; then ok "linger enabled for $USER"
  else bad "D-03: loginctl Linger is '$t' for $USER; run: sudo loginctl enable-linger $USER"; fi

  check_unit "$GW"
  if systemctl --user cat "$WEB" >/dev/null 2>&1; then check_unit "$WEB"
  else echo "– $WEB not installed (skipped)"; fi

  if systemd-run --user --scope --quiet --collect -- true; then ok "systemd-run --user --scope probe exits 0"
  else bad "D-02: systemd-run --user --scope probe failed; the gateway would fall back to no scope"; fi
  out=$(systemd-run --user --scope --quiet --collect -- echo scope-probe 2>&1)
  if [ "$out" = "scope-probe" ]; then ok "--quiet keeps 'Running scope as unit' out of tool output"
  else bad "D-02 (A2): scope output is '$out', expected exactly 'scope-probe'"; fi
  # The gateway probes with a scrubbed env (env_clear + safe env incl. XDG_*), not this shell's.
  if env -i PATH="$PATH" XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}" \
       systemd-run --user --scope --quiet --collect -- true; then
    ok "systemd-run probe also exits 0 with a scrubbed env (PATH + XDG_RUNTIME_DIR only)"
  else bad "D-02: probe fails under a scrubbed env; the gateway would not scope (needs XDG_RUNTIME_DIR in its env)"; fi
}

stage_scope() {
  local gpid pid gcg pcg
  gpid=$(gw_pid)
  no_stale 'sleep 4242' || return
  echo "Send the gateway bot (Telegram) this message:"
  echo "  Run this exact terminal command and show me its full output: setsid nohup sleep 4242 >/dev/null 2>&1 < /dev/null & disown; echo launched"
  echo "Waiting up to 300 s for 'sleep 4242'..."
  pid=$(wait_for 'sleep 4242' 300) || { bad "D-04: 'sleep 4242' never appeared within 300 s"; return; }
  gcg=$(cg_of "$gpid"); pcg=$(cg_of "$pid")
  echo "  gateway cgroup: $gcg"
  echo "  job     cgroup: $pcg"
  if [ "$pcg" = "$gcg" ]; then
    bad "D-04: RED: workload is in the gateway cgroup (pre-58.1 behaviour)"
  else ok "job cgroup differs from the gateway cgroup"; fi
  case "$pcg" in *ironhermes-jobs.slice*) ok "job is under ironhermes-jobs.slice" ;;
    *) bad "D-04: job cgroup lacks ironhermes-jobs.slice" ;; esac
  case "$pcg" in */ironhermes-job-*) ok "job is in an ironhermes-job-* scope" ;;
    *) bad "D-04: job cgroup lacks /ironhermes-job-" ;; esac
  if [ -n "$(systemctl --user list-units --no-legend 'ironhermes-job-*' 2>/dev/null)" ]; then
    ok "systemctl --user lists an ironhermes-job-* unit"
  else bad "D-04: no ironhermes-job-* unit listed"; fi
  echo "Operator: confirm the bot's reply contains '[ironhermes] detached into ironhermes-job-' and no 'Running scope as unit'."
}

stage_restart() {
  local pid4242 pid4343 gpid_old gpid_new gcg c4242 scope
  pid4242=$(pid_of 'sleep 4242')
  if [ -z "$pid4242" ]; then bad "D-06: no running 'sleep 4242'; run the 'scope' stage first"; return; fi
  # Survival only means something if the job starts in its own scope.
  case "$(cg_of "$pid4242")" in */ironhermes-job-*) ;;
    *) bad "D-11: sleep 4242 is in '$(cg_of "$pid4242")', not an ironhermes-job scope; rerun 'scope'"; return ;; esac
  gpid_old=$(gw_pid)
  gcg=$(cg_of "$gpid_old")
  no_stale 'sleep 4343' || return
  # The marker is placed by the user manager, not the bot: gateway `terminal` background=true is
  # unreliable (todo 2026-11-04-gateway-terminal-background-flag-unavailable), and an SSH shell
  # cannot move a PID into the gateway cgroup (cgroup v2 needs write on the root-owned common
  # ancestor). A transient service starts inside user@.service, so the move stays in that subtree.
  echo "In a second shell, place the in-unit marker:"
  echo "  systemd-run --user --unit=uat-4343 --collect --quiet bash -c 'echo \$\$ > /sys/fs/cgroup${gcg}/cgroup.procs && exec sleep 4343'"
  echo "Waiting up to 300 s for 'sleep 4343'..."
  pid4343=$(wait_for 'sleep 4343' 300) || { bad "D-06: 'sleep 4343' never appeared within 300 s"; return; }
  if [ "$(cg_of "$pid4343")" = "$gcg" ]; then ok "in-unit sleep 4343 shares the gateway cgroup (short-lived marker)"
  else
    bad "D-06: sleep 4343 cgroup is '$(cg_of "$pid4343")', expected the gateway's '$gcg'; not restarting"
    return
  fi

  systemctl --user restart "$GW"
  sleep 8
  gpid_new=$(gw_pid)
  if [ "$(prop "$GW" ActiveState)" = "active" ] && [ -n "$gpid_new" ] && [ "$gpid_new" != "0" ] \
     && [ "$gpid_new" != "$gpid_old" ]; then ok "gateway active with new MainPID ($gpid_old -> $gpid_new)"
  else bad "D-06: gateway not active with a new MainPID after restart ($gpid_old -> $gpid_new)"; fi

  if kill -0 "$pid4242" 2>/dev/null; then ok "scoped sleep 4242 survived the restart (D-11)"
  else bad "D-11: scoped sleep 4242 died with the gateway restart"; fi
  c4242=$(cg_of "$pid4242")
  case "$c4242" in *ironhermes-job-*) ok "sleep 4242 is still in its job scope" ;;
    *) bad "D-11: sleep 4242 cgroup is '$c4242', not an ironhermes-job scope" ;; esac
  if kill -0 "$pid4343" 2>/dev/null; then bad "D-06: in-unit sleep 4343 survived the restart (KillMode not killing the unit)"
  else ok "in-unit sleep 4343 was killed with the unit (D-06)"; fi

  scope=$(printf '%s' "$c4242" | tr '/' '\n' | grep -E '^ironhermes-job-.*\.scope$' | head -n1)
  if [ -z "$scope" ]; then bad "D-06: cannot derive a job scope name from '$c4242'"; return; fi
  systemctl --user stop "$scope"
  sleep 2
  if kill -0 "$pid4242" 2>/dev/null; then bad "D-06: sleep 4242 still alive after 'systemctl --user stop $scope'"
  else ok "systemctl --user stop $scope removed the scoped job"; fi
  if systemctl --user list-units --no-legend 'ironhermes-job-*' 2>/dev/null | grep -qF "$scope"; then
    bad "D-06: $scope still listed after stop"
  else ok "$scope no longer listed"; fi
}

stage_rss_before() {
  local pid rss nr off
  pid=$(gw_pid)
  if [ -z "$pid" ] || [ "$pid" = "0" ]; then bad "D-05: $GW has no MainPID"; return; fi
  rss=$(ps -o rss= -p "$pid" | tr -d ' ')
  nr=$(prop "$GW" NRestarts)
  # gateway.log is append-only: remember where it ends so rss-after only reads this run's lines.
  off=$(wc -c < "$GW_LOG" 2>/dev/null | tr -d ' ')
  printf 'pid=%s\nrss_kb=%s\nnrestarts=%s\nlog_off=%s\n' "$pid" "$rss" "$nr" "${off:-0}" > "$STATE_FILE"
  ok "recorded MainPID=$pid RSS=${rss} KiB NRestarts=$nr log_off=${off:-0} in $STATE_FILE"
  echo "Now: on Telegram ask for a 3-task delegate fan-out, and send one message in web chat."
  echo "Wait until BOTH replies arrive (watch the web UI for D-07), then run: $0 rss-after"
}

stage_rss_after() {
  local pid0 rss0 nr0 pid1 rss1 nr1 limit cg cur off size
  if [ ! -f "$STATE_FILE" ]; then bad "D-05: $STATE_FILE missing; run rss-before first"; return; fi
  pid0=$(sed -n 's/^pid=//p' "$STATE_FILE"); rss0=$(sed -n 's/^rss_kb=//p' "$STATE_FILE")
  nr0=$(sed -n 's/^nrestarts=//p' "$STATE_FILE")
  off=$(sed -n 's/^log_off=//p' "$STATE_FILE"); off="${off:-0}"
  pid1=$(gw_pid); nr1=$(prop "$GW" NRestarts)
  if [ "$pid1" = "$pid0" ]; then ok "MainPID unchanged ($pid1)"
  else bad "D-05: MainPID changed $pid0 -> $pid1 (restart or crash mid-fan-out)"; fi
  if [ "$nr1" = "$nr0" ]; then ok "NRestarts unchanged ($nr1)"
  else bad "D-05: NRestarts changed $nr0 -> $nr1"; fi

  rss1=$(ps -o rss= -p "$pid1" 2>/dev/null | tr -d ' ')
  limit="${RSS_LIMIT_KB:-524288}"
  if [ -n "$rss1" ] && [ "$rss1" -le "$limit" ]; then
    ok "RSS before=${rss0} KiB after=${rss1} KiB delta=$((rss1 - rss0)) KiB (limit ${limit} KiB)"
  else bad "D-04: gateway RSS '${rss1}' KiB exceeds ${limit} KiB (before=${rss0} KiB)"; fi

  cg=$(prop "$GW" ControlGroup)
  cur=$(cat "/sys/fs/cgroup${cg}/memory.current" 2>/dev/null)
  if [ -n "$cur" ] && [ "$cur" -lt "$MEM_HIGH" ]; then ok "unit memory.current=$((cur / 1048576)) MiB < MemoryHigh"
  else bad "D-08: unit memory.current '${cur}' is not below MemoryHigh=$MEM_HIGH"; fi

  # Only lines appended since rss-before count; a log rotated smaller than the offset is read whole.
  size=$(wc -c < "$GW_LOG" 2>/dev/null | tr -d ' ')
  if [ "${size:-0}" -lt "$off" ]; then off=0; fi
  if tail -c +$((off + 1)) "$GW_LOG" 2>/dev/null | grep -qF "delegate batch collected"; then
    ok "gateway.log gained 'delegate batch collected' since rss-before (plan 02 evidence)"
  else bad "D-05: no new 'delegate batch collected' in $GW_LOG since rss-before (this fan-out never logged)"; fi
  echo "Operator: confirm both the Telegram and web replies arrived (D-05) and note whether the web UI was responsive (D-07)."
}

case "$STAGE" in
  preflight) stage_preflight ;;
  scope) stage_scope ;;
  restart) stage_restart ;;
  rss-before) stage_rss_before ;;
  rss-after) stage_rss_after ;;
esac

exit "$FAIL"
