<!-- Porting reference for IronHermes Phase 36.3.12 (multi-environment exec). -->
# hermes-agent Exec Backends — Architecture & Porting Spec

**Date:** 2026-06-14
**Source:** `hermes-agent/tools/environments/` (mapped from working tree)
**Purpose:** Reference for building IronHermes `ironhermes-exec` multi-environment parity
(local Python-sandbox → **docker / ssh / modal / daytona / singularity**), Phase 36.3.12.

> **Ported to IronHermes (Phase 36.3.12):** Local/Docker/SSH shipped, implemented in
> `ironhermes-exec::backend` (`Environment` trait + `Session`/`Executor` core in
> `backend/mod.rs`/`session.rs`/`executor.rs`, `LocalEnvironment` in `backend/local.rs`,
> `DockerEnvironment` in `backend/docker.rs`, `SshEnvironment`/`FileSyncManager` in
> `backend/ssh.rs`/`file_sync.rs`), selected via `create_environment()` from the
> `terminal.*` config surface (`crates/ironhermes-core/src/config.rs`). Modal, Daytona,
> Singularity, and any native-SDK backend described below remain **deferred** — not
> ported this phase. See the operator-facing guide at `docs/MULTI-ENVIRONMENT-EXEC.md`
> for backend selection, the `docker`/`podman` runtime knob, `forward_env`, the D-05
> hard-error contract, and the D-10 gating behavior change.

This documents **how the Python reference actually works**, with concrete code, then maps it
to a Rust design. It is descriptive (what exists), not prescriptive about Rust internals —
the port should adapt the *model*, not transliterate the Python.

---

## 0. The One Big Idea: unified spawn-per-call

Every backend shares **one execution model**, defined once in `base.py` and never overridden:

> **Each `execute()` spawns a fresh `bash -c` process.** There is no long-lived interactive
> shell. Cross-command state (env vars, functions, aliases, `cwd`) is preserved by
> **capturing a snapshot once**, then **re-sourcing it before every command** and
> **re-dumping it after**. CWD travels via an in-band stdout marker (remote) or a temp file (local).

`base.py:1`:
```text
Unified spawn-per-call model: every command spawns a fresh ``bash -c`` process.
A session snapshot (env vars, functions, aliases) is captured once at init and
re-sourced before each command. CWD persists via in-band stdout markers (remote)
or a temp file (local).
```

This is the single most important thing to get right in the port. It means a backend is
almost entirely defined by **two operations**:

1. `_run_bash(cmd_string) -> ProcessHandle` — "run this bash string in my environment, give me
   a handle to poll."
2. `cleanup()` — "release my resources."

Everything else (snapshotting, cwd tracking, timeout, interrupt, output draining, sudo
rewriting) lives in the base class and is **backend-agnostic**. Docker differs from SSH differs
from Modal **only** in how those two primitives are implemented.

---

## 1. The `BaseEnvironment` contract

`tools/environments/base.py:288` — the ABC. Subclasses implement `_run_bash` + `cleanup`; the
base provides `execute`, `init_session`, `_wrap_command`, `_wait_for_process`, CWD extraction.

```python
class BaseEnvironment(ABC):
    _stdin_mode: str = "pipe"      # "pipe" or "heredoc" (SDK backends use heredoc)
    _snapshot_timeout: int = 30    # override for slow cold-starts (Modal=60)

    def __init__(self, cwd, timeout, env=None):
        self.cwd = cwd; self.timeout = timeout; self.env = env or {}
        self._session_id   = uuid.uuid4().hex[:12]
        self._snapshot_path = f"/tmp/hermes-snap-{self._session_id}.sh"
        self._cwd_file      = f"/tmp/hermes-cwd-{self._session_id}.txt"
        self._cwd_marker    = f"__HERMES_CWD_{self._session_id}__"
        self._snapshot_ready = False

    def _run_bash(self, cmd_string, *, login=False, timeout=120, stdin_data=None) -> ProcessHandle: ...
    @abstractmethod
    def cleanup(self): ...
```

### 1.1 `ProcessHandle` — the poll/kill/wait duck type

`base.py:187`. Real subprocesses (`subprocess.Popen`) satisfy this natively; SDK backends
(Modal, Daytona) return a `_ThreadedProcessHandle` adapter (see §5.3).

```python
class ProcessHandle(Protocol):
    def poll(self) -> int | None: ...
    def kill(self) -> None: ...
    def wait(self, timeout: float | None = None) -> int: ...
    @property
    def stdout(self) -> IO[str] | None: ...
    @property
    def returncode(self) -> int | None: ...
```

> **Rust mapping:** this Protocol becomes a trait object the executor polls. In async Rust you
> likely don't need the poll/drain machinery at all — a `tokio::process::Child` (local/docker/ssh)
> or an async SDK future (modal) can be `.await`ed directly with `tokio::time::timeout` and a
> `select!` against a cancellation token. The Python poll loop exists only because it's sync.

### 1.2 The unified `execute()` flow

`base.py:829`. This is the call every tool invocation funnels through. **Port this verbatim
in spirit** — the ordering matters.

```python
def execute(self, command, cwd="", *, timeout=None, stdin_data=None,
            rewrite_compound_background=True) -> dict:
    self._before_execute()                                  # (remote: trigger file sync)
    exec_command, sudo_stdin = self._prepare_command(command)   # sudo password injection
    if rewrite_compound_background:
        exec_command = _rewrite_compound_background(exec_command)  # `A && B &` trap guard
    effective_timeout = timeout or self.timeout
    effective_cwd     = cwd or self.cwd
    # ... merge sudo_stdin + caller stdin ...
    if effective_stdin and self._stdin_mode == "heredoc":
        exec_command = self._embed_stdin_heredoc(exec_command, effective_stdin)
        effective_stdin = None
    wrapped = self._wrap_command(exec_command, effective_cwd)   # the magic — see §2
    login = not self._snapshot_ready                            # fall back to bash -l if snapshot failed
    proc   = self._run_bash(wrapped, login=login, timeout=effective_timeout, stdin_data=effective_stdin)
    result = self._wait_for_process(proc, timeout=effective_timeout)
    self._update_cwd(result)                                   # parse + strip cwd marker
    return result                                              # {"output": str, "returncode": int}
```

Return contract everywhere: `{"output": <combined stdout+stderr>, "returncode": <int>}`.
Note **stderr is merged into stdout** (`_popen_bash` sets `stderr=subprocess.STDOUT`).

---

## 2. Session state without a persistent shell (the clever part)

Because each command is a fresh `bash -c`, env/alias/function/cwd state would normally be lost.
The base reconstructs it with a snapshot file living **inside the target environment** (`/tmp`).

### 2.1 Snapshot capture — `init_session()` (`base.py:351`)

Run **once** after the backend is constructed, as a login shell:

```python
bootstrap = (
    f"export -p > {snap}\n"                       # all exported env vars
    f"declare -f | grep -vE '^_[^_]' >> {snap}\n" # shell functions (filtered)
    f"alias -p >> {snap}\n"                       # aliases
    f"echo 'shopt -s expand_aliases' >> {snap}\n"
    f"echo 'set +e' >> {snap}\n"
    f"echo 'set +u' >> {snap}\n"
    f"builtin cd {cwd} 2>/dev/null || true\n"
    f"pwd -P > {cwd_file} 2>/dev/null || true\n"
    f"printf '\\n{marker}%s{marker}\\n' \"$(pwd -P)\"\n"
)
proc = self._run_bash(bootstrap, login=True, timeout=self._snapshot_timeout)
```

If this fails, `_snapshot_ready = False` and the base falls back to running every command with
`bash -l` (login shell loads the user's profile each time — correct but slower).

### 2.2 Per-command wrapping — `_wrap_command()` (`base.py:417`)

Every command is wrapped into a script that **sources the snapshot, cds, runs, re-dumps env,
and emits the cwd marker**:

```python
parts = []
if self._snapshot_ready:
    parts.append(f"source {snap} >/dev/null 2>&1 || true")   # restore prior state
parts.append(f"builtin cd -- {quoted_cwd} || exit 126")      # apply cwd
parts.append(f"eval '{escaped}'")                            # the user's command
parts.append("__hermes_ec=$?")                               # capture exit code
if self._snapshot_ready:
    parts.append(f"export -p > {snap} 2>/dev/null || true")  # persist env mutations for next call
parts.append(f"pwd -P > {cwd_file} 2>/dev/null || true")     # local reads this file
parts.append(f"printf '\\n{marker}%s{marker}\\n' \"$(pwd -P)\"")  # remote parses this from stdout
parts.append("exit $__hermes_ec")
return "\n".join(parts)
```

### 2.3 CWD extraction — `_extract_cwd_from_output()` (`base.py:777`)

Remote backends parse `__HERMES_CWD_<session>__<path>__HERMES_CWD_<session>__` out of stdout,
update `self.cwd`, and **strip the marker line** (including the injected leading `\n`) so the
user never sees it. Local overrides `_update_cwd` to read the temp file instead.

> **Rust mapping:** keep this exact mechanism — it is transport-independent and is what makes
> `cd foo` persist across tool calls. A unit-testable `wrap_command(cmd, cwd, session) -> String`
> + `extract_cwd(output, marker) -> (clean_output, Option<cwd>)` pair ports cleanly. Marker
> format must match byte-for-byte if you ever want a Rust executor to drive a shared `/tmp`
> snapshot, but since IronHermes owns both ends you can choose your own marker.

---

## 3. Backend selection — the factory

`tools/terminal_tool.py:1205` `_create_environment(env_type, image, cwd, timeout, ssh_config,
container_config, ...)`. `env_type` comes from the `TERMINAL_ENV` config (default `"local"`).
`container_config` carries the resource knobs:

```python
cpu        = cc.get("container_cpu", 1)
memory     = cc.get("container_memory", 5120)     # MiB
disk       = cc.get("container_disk", 51200)      # MiB
persistent = cc.get("container_persistent", True)

if   env_type == "local":       return LocalEnvironment(cwd, timeout)
elif env_type == "docker":      return DockerEnvironment(image, cwd, timeout, cpu, memory, disk, ...)
elif env_type == "singularity": return SingularityEnvironment(...)
elif env_type == "modal":       # direct vs managed split — see §6.3
elif env_type == "daytona":     return DaytonaEnvironment(...)
elif env_type == "ssh":         return SSHEnvironment(host, user, port, key_path, cwd, timeout)
else: raise ValueError(...)
```

> **Rust mapping:** `enum ExecBackend { Local, Docker, Ssh, Modal, Daytona, Singularity }` +
> `fn create_environment(cfg) -> Box<dyn Environment>`. `task_id` keys snapshot/container reuse.

A useful split for the port: **bind-mount backends** (Docker, Singularity, Local) see the host
FS live and need **no file sync**; **remote backends** (SSH, Modal, Daytona) need the
`FileSyncManager` (§7). This is declared by overriding `_before_execute()`.

---

## 4. Docker backend (`docker.py`, 1312 LOC)

The biggest and most production-hardened backend. It **shells out to the `docker` CLI** (not the
SDK), which keeps the dependency surface to "a `docker` binary on PATH."

### 4.1 Lifecycle: `docker run -d ... sleep infinity`, then `docker exec` per command

The container is a **long-lived idle box**; commands are one-shot `docker exec`. `docker.py:862`:

```python
container_name = f"hermes-{uuid.uuid4().hex[:8]}"
init_args = [] if image_uses_s6_init else ["--init"]      # tini PID1 reaps zombies
run_cmd = [
    docker_exe, "run", "-d", *init_args,
    "--name", container_name,
    "--label", "hermes-agent=1",                          # reaper + reuse discovery
    "--label", f"hermes-task-id={task_label}",
    "--label", f"hermes-profile={profile_name}",
    "-w", cwd,
    *all_run_args,                                        # security + resources + mounts + env
    image,
    "sleep", "infinity",                                  # no fixed lifetime
]
result = subprocess.run(run_cmd, capture_output=True, text=True, timeout=120, check=True)
self._container_id = result.stdout.strip()
```

Command execution — `_run_bash()` (`docker.py:943`):

```python
def _run_bash(self, cmd_string, *, login=False, timeout=120, stdin_data=None):
    cmd = [self._docker_exe, "exec"]
    if stdin_data is not None: cmd.append("-i")
    if login: cmd.extend(self._init_env_args)            # -e KEY=VAL only on init_session
    cmd.append(self._container_id)
    cmd.extend(["bash", "-l", "-c", cmd_string] if login else ["bash", "-c", cmd_string])
    return _popen_bash(cmd, stdin_data)
```

> Note: host env vars are injected via `-e` **only during `init_session`** so `export -p`
> captures them into the snapshot; subsequent `docker exec`s carry no `-e` (they `source` the
> snapshot). `_build_init_env_args` (`docker.py:911`) filters secrets through a blocklist unless
> explicitly forwarded via `docker_forward_env`.

### 4.2 Cross-process container reuse (the "ONE long-lived container" contract)

`docker.py:828`. Before starting fresh, look for an existing container by **label** matching
`(task_id, profile)`; attach to it (`docker start` if stopped) instead of creating a new one.
Reuse matches labels **only** — not image/mounts/resources.

```python
if persist_across_processes:
    existing = self._find_reusable_container(task_label, profile_name)
    if existing is not None:
        container_id, state = existing
        self._container_id = container_id
        if state != "running":
            subprocess.run([docker_exe, "start", container_id], check=True, timeout=30)
        reused = True
```

### 4.3 "No such container" recovery

`docker.py:970`. If a `docker exec` returns `No such container` / `is not running` (container
was `docker rm`'d out of band), `_recreate_container()` (`:980`) re-attaches by label or starts a
fresh one with the saved `image` + `all_run_args`, then retries. **Port this** — long-lived
agents hit it.

### 4.4 Filesystem: bind-mount (persistent) vs tmpfs (ephemeral)

`docker.py:575`–`630`. Persistence is "preserve `/workspace` and `/root` across runs" via host
bind mounts under `TERMINAL_SANDBOX_DIR` (default `~/.hermes/sandboxes/`); non-persistent uses
size-capped tmpfs:

```python
# persistent: -v {sandbox}/workspace:/workspace   (+ tmpfs /home,/root size 1g)
# ephemeral:  --tmpfs /workspace:rw,exec,size=10g  --tmpfs /home,/root,...
```

Credentials/skills/cache are mounted **read-only** (`:ro`) from the host (`docker.py:648`–`715`),
so Docker needs **no FileSyncManager** — the host FS is directly visible.

### 4.5 Security model (`docker.py:327`, `_build_security_args`)

Drop-all-then-add-back; the container is the boundary:

```python
_BASE_SECURITY_ARGS = [
    "--cap-drop", "ALL",
    "--cap-add", "DAC_OVERRIDE", "--cap-add", "CHOWN", "--cap-add", "FOWNER",
    "--security-opt", "no-new-privileges",
    "--pids-limit", "256",
    "--tmpfs", "/tmp:rw,nosuid,size=512m",
    "--tmpfs", "/var/tmp:rw,noexec,nosuid,size=256m",
]
# + SETUID/SETGID only when container starts as root and its init drops privilege
# + --network=none when network is disabled
# Resource caps: --cpus, --memory {m}m
```

s6-overlay images (the bundled `hermes-agent` image) need special handling: skip `--init`
(they own PID 1) and mount `/run` with `exec` instead of `noexec` (`docker.py:748`).

### 4.6 Cleanup & orphan reaping

`docker.py:1180`. In persist mode (default) cleanup is a **no-op** — the container keeps running
so background processes (`npm run dev`, watchers) survive `/quit`. Reclamation is handled by
`reap_orphan_containers()` at next startup: any labeled container untouched for
`2 × lifetime_seconds` is `docker rm -f`'d. Non-persist mode does `docker stop -t 10` + `rm -f`
on every cleanup. Cleanup runs on a **daemon thread** with an atexit hook waiting ≤15s.

---

## 5. SSH backend (`ssh.py`, 375 LOC)

Run commands on a remote host. Smallest backend — a good first port target.

### 5.1 Connection: OpenSSH ControlMaster multiplexing

`ssh.py:83`. One persistent master connection; every command/scp reuses the socket (no
re-handshake per call). Socket name is a sha256 of `user@host:port` to stay under macOS's
104-byte `sun_path` limit (`ssh.py:63`).

```python
def _build_ssh_command(self, extra_args=None):
    cmd = ["ssh",
        "-o", f"ControlPath={self.control_socket}",
        "-o", "ControlMaster=auto",
        "-o", "ControlPersist=300",
        "-o", "BatchMode=yes",
        "-o", "StrictHostKeyChecking=accept-new",
        "-o", "ConnectTimeout=10"]
    if self.port != 22: cmd += ["-p", str(self.port)]
    if self.key_path:   cmd += ["-i", self.key_path]
    cmd += (extra_args or [])
    cmd.append(f"{self.user}@{self.host}")
    return cmd
```

Command execution — `_run_bash()` (`ssh.py:343`): the wrapped script is **`shlex.quote`d** and
passed to a remote bash:

```python
cmd = self._build_ssh_command()
cmd += ["bash", "-l", "-c", shlex.quote(cmd_string)] if login else \
       ["bash", "-c", shlex.quote(cmd_string)]
return _popen_bash(cmd, stdin_data)
```

### 5.2 File sync: tar-over-SSH

`ssh.py:188` `_ssh_bulk_upload` pipes a single `tar c | ssh … tar x` stream instead of N scp
calls (~580 files → one TCP stream). Uses a symlink staging dir to avoid `--transform` fragility,
and `--no-overwrite-dir` so sshd `StrictModes` isn't broken by a umask-widened home dir:

```python
tar_cmd = ["tar", "-chf", "-", "-C", staging, "."]
ssh_cmd = self._build_ssh_command() + [f"tar xf - --no-overwrite-dir -C {shlex.quote(base)}"]
tar_proc = subprocess.Popen(tar_cmd, stdout=subprocess.PIPE, ...)
ssh_proc = subprocess.Popen(ssh_cmd, stdin=tar_proc.stdout, ...)
```

Download mirrors it (`tar cf - -C / <base>` → local file). Single-file upload falls back to
`scp` over the ControlMaster socket (`ssh.py:159`). `cleanup()` (`ssh.py:355`) syncs files back,
then `ssh -O exit` to drop the master + unlinks the socket.

> `_before_execute()` (`ssh.py:335`) calls `self._sync_manager.sync()` — rate-limited push of
> changed `~/.hermes` files before each command.

---

## 6. Modal backend (serverless) — two modes

Modal is cloud serverless. There are **two implementations** selected by `terminal.modal_mode`:
**direct** (native Modal SDK, your credentials) and **managed** (REST against the Nous tool
gateway, no Modal creds needed). Both subclass `BaseEnvironment` and present identical
`execute`/`cleanup`.

### 6.1 Direct mode (`modal.py`, 478 LOC) — native SDK

Uses `Sandbox.create("sleep", "infinity")` + `sandbox.exec("bash","-c",cmd)`. Because the SDK is
async and the agent loop is sync, all SDK calls run on a dedicated **background event-loop thread**
(`_AsyncWorker`, `modal.py:127`), and `_run_bash` returns a `_ThreadedProcessHandle`.

Sandbox creation (`modal.py:241`):
```python
async def _create_sandbox(image_spec):
    app = await modal.App.lookup.aio("hermes-agent", create_if_missing=True)
    sandbox = await modal.Sandbox.create.aio(
        "sleep", "infinity", image=image_spec, app=app,
        timeout=int(kwargs.pop("timeout", 3600)), **kwargs)   # cpu/memory/ephemeral_disk
    return app, sandbox
```

Exec (`modal.py:408`):
```python
def _run_bash(self, cmd_string, *, login=False, timeout=120, stdin_data=None):
    def cancel(): worker.run_coroutine(sandbox.terminate.aio(), timeout=15)   # interrupt support
    def exec_fn():
        async def _do():
            args = ["bash", "-l", "-c", cmd_string] if login else ["bash", "-c", cmd_string]
            process = await sandbox.exec.aio(*args, timeout=timeout)
            stdout = await process.stdout.read.aio()
            stderr = await process.stderr.read.aio()
            exit_code = await process.wait.aio()
            return (stdout + ("\n"+stderr if stderr else "")), exit_code
        return worker.run_coroutine(_do(), timeout=timeout + 30)
    return _ThreadedProcessHandle(exec_fn, cancel_fn=cancel)
```

`_stdin_mode = "heredoc"` — stdin is embedded into the command as a heredoc (`base.py:474`)
because SDK exec has no clean stdin pipe in this path.

### 6.2 Direct mode: filesystem snapshots for persistence

`modal.py:442` `cleanup()`: on persistent mode, `sandbox.snapshot_filesystem()` → store the
image id in `~/.hermes/modal_snapshots.json` keyed by `task_id`; next construction restores
from that snapshot (`modal.py:241`, falling back to the base image if restore fails). File sync
in/out uses **base64-over-stdin** and **gzip-tar-over-stdin** (`_modal_bulk_upload`, `modal.py:325`)
to dodge the SDK's 64 KB exec-arg limit.

### 6.3 Managed mode (`managed_modal.py`, 282 LOC) — REST poll loop

No Modal SDK; talks to the Nous tool gateway over HTTPS. Selected in the factory
(`terminal_tool.py:1283`) when `modal_mode` resolves to `managed`. The exec model is
**start → poll → cancel** (`modal_utils.BaseModalExecutionEnvironment` drives the loop):

```python
# create:  POST /v1/sandboxes                 {image,cwd,cpu,memoryMiB,persistentFilesystem,logicalKey}
# start:   POST /v1/sandboxes/{id}/execs       {execId,command,cwd,timeoutMs,stdinData}
# poll:    GET  /v1/sandboxes/{id}/execs/{eid} -> status in {completed,failed,cancelled,timeout}
# cancel:  POST /v1/sandboxes/{id}/execs/{eid}/cancel
# teardown:POST /v1/sandboxes/{id}/terminate   {snapshotBeforeTerminate: persistent}
```

Auth is `Authorization: Bearer <nous_user_token>`; create is idempotent via
`x-idempotency-key`. Managed mode **refuses host credential-file passthrough**
(`managed_modal.py:214`) — use direct mode when skills need creds inside the sandbox.

> **Rust mapping:** managed mode is just an HTTP client + a poll loop → trivially portable with
> `reqwest`. Direct mode needs a Modal SDK; there is **no official Rust Modal SDK**, so the port
> options are (a) ship only managed mode first, (b) drive Modal's HTTP/gRPC API directly, or
> (c) shell to a `modal` CLI. Recommend **managed-first** for parity, direct deferred.

---

## 7. `FileSyncManager` — shared remote file sync (`file_sync.py`, 403 LOC)

Used by **SSH, Modal, Daytona only** (bind-mount backends skip it). Tracks local `~/.hermes`
changes by `(mtime, size)`, detects deletions, rate-limits (5s), and pushes transactionally.
Backends inject **transport callbacks**, so the manager is transport-agnostic:

```python
UploadFn       = Callable[[str, str], None]                  # (host, remote)
BulkUploadFn   = Callable[[list[tuple[str, str]]], None]
BulkDownloadFn = Callable[[Path], None]                      # writes a tar
DeleteFn       = Callable[[list[str]], None]
GetFilesFn     = Callable[[], list[tuple[str, str]]]

FileSyncManager(get_files_fn=lambda: iter_sync_files(remote_base),
                upload_fn=..., delete_fn=..., bulk_upload_fn=..., bulk_download_fn=...)
```

`iter_sync_files()` (`file_sync.py:50`) enumerates credentials + skills + cache as
`(host_path, remote_path)` pairs, remapping the hardcoded `/root/.hermes` base to the remote
user's home. Helpers `quoted_mkdir_command` / `quoted_rm_command` / `unique_parent_dirs` build
batched shell commands. `_before_execute()` triggers `sync()`; `cleanup()` triggers `sync_back()`.

> **Rust mapping:** `trait FileTransport { fn bulk_upload(&self, files); fn bulk_download(&self, dest); fn delete(&self, paths); }`
> + a backend-agnostic `FileSyncManager<T: FileTransport>` holding the mtime/size change map.

---

## 8. Process lifecycle: timeout, interrupt, output draining

`base.py:483` `_wait_for_process` — shared, not overridden. The sync Python implementation is
heavy (select()-based non-blocking drain, adaptive 5ms→200ms poll, interrupt checks via
`is_interrupted()`, 10s activity heartbeats so the gateway doesn't kill long commands). Key
behaviors to **preserve in the port**, even though the mechanism differs in async Rust:

| Concern | Python | Return code |
| --- | --- | --- |
| Normal exit | drain to EOF, return `{output, returncode}` | child rc |
| Timeout | `_kill_process` after `deadline` | **124** |
| Interrupt (user cancel) | `is_interrupted()` → kill | **130** |
| `cd` failure (bad dir) | wrapper `exit 126` | 126 |
| Backgrounded grandchild holding the pipe | stop draining ~300ms after bash exits (don't hang) | — |

Two non-obvious bugs the port must not reintroduce (`base.py:505`, `:719`):
- A backgrounded process (`cmd &`, `setsid … & disown`) inherits the stdout pipe; naive
  "read to EOF" hangs forever. → stop draining shortly after bash exits.
- Local subprocesses spawned with `os.setsid` (own process group) orphan to PID 1 if the agent
  is killed mid-command. → kill the **process group** on `KeyboardInterrupt`/`SystemExit`.

> In async Rust most of this collapses: `tokio::select!` over `child.wait()`, a
> `tokio::time::sleep(timeout)`, and a `CancellationToken`; stream stdout via
> `BufReader::lines()`. Map timeout→124, cancel→130 to preserve contract. For the
> backgrounded-pipe case, set a short read-drain deadline after the child exits.

---

## 9. Proposed Rust design for `ironhermes-exec`

```rust
// The two-primitive contract — everything else is shared.
#[async_trait]
trait Environment: Send + Sync {
    /// Spawn the wrapped bash string; return a handle the executor awaits.
    async fn run_bash(&self, cmd: &str, opts: RunOpts) -> Result<ExecResult>;
    /// Release resources (container/connection/sandbox).
    async fn cleanup(&mut self) -> Result<()>;
    /// Remote backends override to push file sync before each command.
    async fn before_execute(&self) -> Result<()> { Ok(()) }
    /// Default temp dir inside the env (Local may override).
    fn temp_dir(&self) -> &str { "/tmp" }
}

struct RunOpts { login: bool, timeout: Duration, stdin: Option<String> }
struct ExecResult { output: String, returncode: i32 }

// Shared, backend-agnostic — lives in the base/session module, NOT per backend:
struct Session { id: String, snapshot_path: String, cwd_file: String, cwd_marker: String, ready: bool }
impl Session {
    fn bootstrap_script(&self, cwd: &str) -> String { /* §2.1 */ }
    fn wrap_command(&self, cmd: &str, cwd: &str) -> String { /* §2.2 */ }
    fn extract_cwd(&self, output: &mut String) -> Option<String> { /* §2.3 */ }
}

async fn execute(env: &dyn Environment, sess: &mut Session, command: &str, cwd: &str,
                 timeout: Duration, cancel: CancellationToken) -> Result<ExecResult> {
    env.before_execute().await?;
    let wrapped = sess.wrap_command(command, cwd);
    let mut res = env.run_bash(&wrapped, RunOpts{ login: !sess.ready, timeout, stdin: None }).await?;
    if let Some(new_cwd) = sess.extract_cwd(&mut res.output) { /* persist cwd */ }
    Ok(res)
}
```

**Crate/dependency suggestions:**

| Backend | Approach | Rust crate |
| --- | --- | --- |
| Local | spawn `bash -c` (already exists as the Python-sandbox) | `tokio::process` |
| Docker | shell out to `docker` CLI (mirror Python) **or** SDK | `tokio::process` (CLI) or `bollard` (SDK) |
| SSH | `ssh`/`scp` CLI + ControlMaster, **or** native | CLI via `tokio::process`; or `openssh`/`russh` |
| Modal (managed) | REST start/poll/cancel | `reqwest` |
| Modal (direct) | no Rust SDK → defer, or HTTP/gRPC/CLI | — |
| Daytona | SDK/REST | `reqwest` |
| Singularity | `singularity exec` CLI | `tokio::process` |

> **Recommendation:** mirror the Python "shell out to the CLI" choice for Docker/SSH/Singularity
> first — it's the lowest-risk path to parity and matches the reference's exact semantics
> (labels, reuse, tar-over-ssh). Reserve native SDKs (`bollard`, `russh`) for a later hardening
> pass. For Modal, ship **managed** mode first.

---

## 10. Porting checklist (parity-ordered)

1. **Session core** (`Session`: bootstrap, `wrap_command`, `extract_cwd`) — backend-agnostic,
   fully unit-testable, unblocks everything. (`base.py:351/417/777`)
2. **Executor** (`execute` + timeout/cancel→124/130 + stdout draining + backgrounded-pipe guard).
   (`base.py:483/829`)
3. **Docker** (CLI: `run -d … sleep infinity` + `exec`, label-based reuse, "no such container"
   recovery, bind-mount vs tmpfs, cap-drop security, persist-mode no-op cleanup + orphan reaper).
   (`docker.py`)
4. **SSH** (ControlMaster, `bash -c shlex.quote`, tar-over-ssh sync, `ssh -O exit`). (`ssh.py`)
5. **FileSyncManager** (`trait FileTransport` + mtime/size change map) — shared by SSH/Modal/Daytona.
   (`file_sync.py`)
6. **Modal managed** (REST start/poll/cancel/terminate, bearer auth, idempotency key). (`managed_modal.py`)
7. **Daytona / Singularity / Modal-direct** — last; Modal-direct may stay deferred (no Rust SDK).
8. **Factory + config** (`TERMINAL_ENV`, resource knobs, ssh/container config). (`terminal_tool.py:1205`)

**Gotchas worth a test each:** cwd persistence across calls; env var set in cmd A visible in
cmd B (snapshot re-dump); timeout→124 / interrupt→130; `cd /nonexistent`→126; backgrounded
process doesn't hang the drain; Docker container reuse across process restarts; Docker
"No such container" auto-recreate; SSH socket path under 104 bytes; secrets NOT forwarded into
Docker unless explicitly allow-listed.

---

*Derived from `hermes-agent/tools/environments/` on 2026-06-14. Companion to `PARITY-UPDATE.md`
§6 (Execution Backends) and the `ironhermes-exec` crate.*
