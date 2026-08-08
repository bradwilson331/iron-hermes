# Multi-Environment Exec — Operator Guide (Phase 36.3.12)

Phase 36.3.12 gives the `terminal` and `execute_code` tools a pluggable execution
backend: `local` (default, unchanged), `docker` (persistent, resource-limited
containers, `docker`/`podman` CLI), and `ssh` (a remote host over OpenSSH with
`ControlMaster` multiplexing). This doc covers backend selection, the
`docker`/`podman` runtime knob, credential handling, the D-05 hard-error
contract, and — most important for existing users — the D-10 gating behavior
change.

**Scope of this phase:** Local, Docker, and SSH backends ship. Modal, Daytona,
Singularity, and any native-SDK backend are **deferred** — see
[Deferred scope](#deferred-scope) below. This doc does not claim they exist.

---

## 1. Backend selection

All exec-backend configuration lives under the existing `terminal:` block in
`config.yaml`. Every new key is `#[serde(default)]`, so an existing
`config.yaml` with no `terminal:` section (or a minimal one) parses unchanged
and continues to run `backend: local` — this phase is additive-only for
existing deployments.

```yaml
terminal:
  # "local" (default) | "docker" | "ssh"
  backend: local

  # Only consulted when backend: docker. Explicit, no auto-detection —
  # auto-picking a runtime by probing PATH is a silent-fallback footgun (D-07).
  # "docker" (default) | "podman"
  container_runtime: docker

  # Base image for the docker backend's persistent container.
  image: debian:stable-slim

  # Credential/env allowlist forwarded across the docker/ssh backend boundary.
  # Default empty — nothing secret crosses unless a var name is opted in here.
  forward_env: []
  # forward_env: [KUBECONFIG, AWS_PROFILE]

  # Orphan-reaper "lifetime" knob (seconds). The boot-time reaper GCs
  # hermes-agent-labeled containers idle longer than 2x this value.
  container_reap_after_secs: 86400  # 24h

  # docker backend resource limits / persistence / networking.
  container:
    cpu: 1.0            # fractional cores
    memory_mib: 5120     # 5 GiB
    disk_mib: 51200      # 50 GiB (workspace bind-mount/tmpfs cap)
    pids_limit: 256
    persistent: true     # bind-mount /workspace (survives recreation) vs tmpfs
    network: false        # false -> --network=none (security-hardened default)

  # Only consulted when backend: ssh. None of these have meaningful defaults
  # (except port) — the ssh backend cannot be constructed without host+user.
  ssh:
    host: ""
    user: ""
    port: 22
    key_path: null   # null uses the ssh CLI's default identity resolution
```

- `backend` is **config-only** (D-06): it is populated exclusively by parsing
  `config.yaml` at startup. There is no LLM/tool-call argument path that can
  change which backend a session runs on.
- `container_runtime` is likewise explicit and config-only (D-07): setting
  `docker` vs `podman` selects the literal binary name invoked (`{runtime}
  ps`, `{runtime} exec`, ...) — it is never auto-detected by probing `PATH`.

## 2. The D-05 hard-error contract (no silent fallback)

**If the selected backend is unavailable at runtime, the tool call fails
loudly, naming the fix. It never silently falls back to `local`.** Silently
downgrading a sandboxed/remote session to local execution would destroy the
isolation the operator explicitly asked for.

Concrete cases:

- **`backend: docker` with the runtime binary absent or the daemon
  unreachable** — the tool call errors with a message naming the missing
  binary / daemon and suggesting either installing it or setting
  `terminal.backend: local`. This is exercised on this project's own dev
  machine today: Docker Desktop is absent, so the default `container_runtime:
  docker` hard-errors immediately unless overridden.
- **macOS + podman gotcha:** unlike Docker Desktop (which runs its Linux VM
  transparently), a fresh `podman` install on macOS does **not** start a VM
  automatically. `podman machine init && podman machine start` must be run
  first, or `terminal.container_runtime: podman` will hit the same D-05
  hard-error (a connection-refused-style failure from `podman version`).
- **`backend: ssh` with `terminal.ssh` unset, or `host`/`user` empty** — hard
  errors before any network I/O is attempted.
- **`backend: ssh` with an unreachable/refused host** — the `ControlMaster`
  availability probe (`ssh ... true`, `ConnectTimeout=10`) fails and the call
  errors within ~10s; it never falls through to a local shell.

If you hit a D-05 hard-error and want to keep working locally in the meantime,
explicitly set `terminal.backend: local` (or delete the `terminal.backend`
key, since `local` is the default) rather than relying on any fallback
behavior — there isn't one.

## 3. Behavior change (D-10): local `terminal`/`execute_code` are now gated + audited

**This is a deliberate, documented behavior change that affects every
existing deployment, not just new docker/ssh users.**

Before this phase, a local `terminal` tool call from the CLI/TUI and every
`execute_code` call ran **completely ungated** — no guardrail check, no
approval prompt, no audit trail entry. Phase 36.3.12 closes that gap: as of
this phase, **every** `terminal` and `execute_code` call — regardless of
backend, on every surface (CLI, TUI, the gateway, and the `iron_hermes_ui`
web/desktop UI) — is routed through a single guardrail → approval → audit
chokepoint (`execute_gated_command()` in `ironhermes-hooks::gated_exec`).
The web/desktop UI's approval step is fail-closed deny rather than an
interactive prompt — see §5 for the full posture and its rationale.

- **Guardrail classification** runs first: the existing two-tier dangerous-command
  denylist (Tier-2 = catastrophic, Tier-1 = risky-but-allowed-with-approval).
  A Tier-2 command is **blocked** before it is ever spawned, on every surface,
  even under `yolo` mode.
- **Every resolution is audited** — including a routine `Allow`-classified
  command. Previously only explicit approval prompts were logged; now the
  full history of terminal/execute_code activity lands in the audit trail.
- **Remote or credential-forwarding runs are always forced through the
  approval gate**, even if the command itself classifies as `Allow`: any
  command on the `ssh` backend, or any command whose config carries a
  non-empty `forward_env`, requires operator approval before it runs (unless
  `yolo` is set, in which case it still runs and is audited as a bypass).
- **`background=true` commands are gated too** — the gate wraps the entire
  tool call, including the background-dispatch branch, so a backgrounded
  command is only reached after passing the chokepoint.
- **`execute_code` is gate-only** (D-11): Python source is not pattern-matched
  against the shell-command denylist (`DANGEROUS_PATTERNS` targets shell
  syntax, not Python) — an opaque, empty classify-argument is passed, which
  resolves to `Allow` and is still audited. The Python code itself continues
  to run on the existing local `Sandbox`; `execute_code` does not (yet) route
  through the docker/ssh backends (see [Deferred scope](#deferred-scope)).

**What this means for you as an existing operator:** if you were relying on
local terminal/execute_code calls running silently with zero prompts, you
will now occasionally see an approval prompt for Tier-1-classified commands,
and every call (including benign ones) now appears in the audit log
(`audit.jsonl`). If this is disruptive, `yolo` mode still bypasses ordinary
approval prompts (Tier-1 and forced-remote/forward_env approval) — but it
**never** bypasses a Tier-2 block, and every bypass is still recorded in the
audit trail.

**The `yolo` caveat, stated precisely:** `yolo=true` skips the forced-approval
prompt that D-08 would otherwise require for remote (`ssh` backend) or
credential-forwarding (non-empty `forward_env`) runs — even on a command that
classifies as `Allow`. That is the one thing `yolo=true` changes. It does
**not** change anything else: it never relaxes the Tier-2 hard block (a
catastrophic command is blocked before any `yolo` check exists in the code
path, on every surface, with or without `yolo`), and it never skips the audit
entry — a yolo'd bypass runs and is recorded with `resolution = "bypass"`, so
it remains attributable after the fact even though it wasn't gated before the
fact. See `deferred-items.md`'s "Deferral 2" for the full rationale and the
named follow-up owner (the future tiered-DEFCON-strictness phase).

**Coverage note — all four surfaces are gated.** CLI, TUI, the gateway, and
the `iron_hermes_ui` web/desktop surface (`crates/iron_hermes_ui/src/server/state.rs`,
`run_web_turn`) are **all** routed through the same guardrail → approval →
audit chokepoint. The web/desktop surface's approval posture is **fail-closed
deny**, not an interactive prompt (see §5) — it has no approval-prompt UI, no
pending-approval awaiter registry, and no WebSocket round-trip for a
prompt/response yet, so any command requiring approval on that surface is
refused rather than shown to a human. `Allow`-classified commands (the common
case) run normally on every surface, including web/desktop, and are audited
there too.

## 4. D-09: scrub-by-default env + `forward_env` allowlist

Environment variables never cross into a `docker` container or an `ssh`
remote session by default. Both backends reuse the same
`build_terminal_safe_env()` helper (the base `SAFE_ENV_KEYS`/`XDG_*`/
`IRONHERMES_HOME` set, mirroring the existing local-sandbox `SECRET_PATTERNS`
scrub) — a host variable matching a secret-ish pattern (`*_KEY`, `*_TOKEN`,
`*_SECRET`, `*_PASSWORD`, `*_CREDENTIAL`, `*_PASSWD`, `*_AUTH`, ...) is never
forwarded unless its exact name is listed in `terminal.forward_env`.

- **docker:** allow-listed vars are injected via `-e KEY=VAL` on the
  container's first/login exec call.
- **ssh:** allow-listed vars are injected as explicit `export KEY=value` lines
  prepended to the remote `bash -c` script (not `SendEnv`/`AcceptEnv`, which
  would require matching `sshd_config` on a remote host this backend cannot
  assume control over).

Any command that carries a non-empty `forward_env` is additionally forced
through the D-10 approval gate — see §3.

## 5. Web/desktop approval posture: fail-closed deny

The `iron_hermes_ui` web/desktop surface (`run_web_turn`) is gated identically
to CLI/TUI/gateway — guardrail classification and audit both run — but its
`ApprovalGate` (`WebApprovalGate`) is **fail-closed**: any command requiring
approval (Tier-1 / `NeedsApproval`, or an `Allow`-classified command forced
into the approval path by D-08's remote/`forward_env` rule) is **denied
outright**, with no prompt shown to the operator.

This is a deliberate interim posture, not an oversight or a stub:

- The web/desktop surface has no approval-prompt UI component, no
  pending-approval awaiter registry, no WebSocket round-trip for a
  prompt/response, and no timeout policy today. Building that is a feature
  on the scale of the gateway's `/approve` chat bridge or the CLI's
  `[o/s/a/d]` interactive prompt.
- Fail-closed deny is strictly safer than the pre-Plan-12 status quo (zero
  checks — no guardrail, no approval, no audit at all) and strictly safer
  than auto-approving.
- Tier-2 commands were already unconditionally blocked on this surface
  regardless of this decision, and `Allow`-classified commands (ordinary use)
  are unaffected — they run normally and are now audited.

**What this means for you as a web/desktop operator:** if you issue a
Tier-1/`NeedsApproval` command, or a command that D-08 forces into the
approval path (remote backend or non-empty `forward_env`), from the web or
desktop UI, it is refused with a clear denial message — you will not see an
approval prompt on this surface. Use the CLI, TUI, or gateway if you need the
interactive approval flow. An interactive web approval UX is a tracked
follow-up with no phase currently scheduled (see `deferred-items.md`,
"Deferral 3").

## 6. Session persistence: `cd`/`export` do not persist across `terminal` calls

Each `terminal` tool call constructs a fresh session and starts from the
process's actual current directory (or an explicit `cwd` argument you pass),
not from wherever a previous call's `cd` left off. Concretely: if one
`terminal` call runs `cd /tmp && export FOO=bar`, the next `terminal` call —
even in the same conversation — does **not** see `FOO` set and is not running
from `/tmp`.

**Why:** this phase built a Session core capable of carrying cwd/env state
across calls, but the production `terminal` tool constructs a brand-new
`Session` on every foreground call, matching the pre-36.3.12 stateless-per-call
behavior. This is a decided, documented limitation, not a bug — see
`deferred-items.md`'s "Deferral 1" for the full rationale, including a
secondary consequence for the Docker backend's credential-env injection
timing (allow-listed values become visible in the host's `ps` output on every
exec rather than once at container init — the `forward_env` allowlist scrub
itself is unaffected).

**Workaround:** if you need a sequence of commands to share a working
directory or exported variables, chain them in a single `terminal` call
(e.g. `cd /some/dir && do-thing && do-other-thing`) rather than relying on
state carrying over between separate calls.

## 7. Deferred scope

The following are consciously fenced out of this phase — they are not
half-implemented, they simply do not exist yet:

- **Modal, Daytona, Singularity backends** and any other native-SDK backend
  (D-01 scope). Only `local`, `docker`, and `ssh` ship this phase.
- **Backend-routed `execute_code`** — `execute_code` is gated (§3) but its
  Python still runs on the existing local `Sandbox`, not on the selected
  `docker`/`ssh` backend.
- **Background-command backend routing** — `background=true` commands
  continue to run through the local `ProcessRegistry`, independent of
  `terminal.backend` (D-12); only the foreground path is routed through
  `create_environment`.
- **Kanban-worker exec** does not yet consult `terminal.backend`.
- **Interactive web/desktop approval UX** — the `iron_hermes_ui` surface is
  gated (guardrail + audit run, per §3) but its approval step is fail-closed
  deny rather than an interactive prompt; see §5.
- **Persistent Sessions in the production `terminal` tool** — `cd`/`export`
  do not persist across separate `terminal` calls; see §6.
- **Live persistent-container reuse across process restarts, a real SSH
  target, and the gateway operator-approval UX** are structurally implemented
  and unit-tested but require a human with a live daemon/host to exercise
  end-to-end (manual UAT — see `36.3.12-VALIDATION.md`).

---

*Phase: 36.3.12-multi-environment-exec-docker-ssh-modal-daytona-singularity*
