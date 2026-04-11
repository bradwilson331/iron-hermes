# Phase 8: Code Execution - Research

**Researched:** 2026-04-10
**Domain:** Rust child-process execution, Unix domain sockets, JSON-RPC 2.0, Python IPC bridge
**Confidence:** HIGH

## Summary

Phase 8 delivers `execute_code`: an agent tool that spawns a Python child process in an isolated environment, exposes a subset of agent tools over a JSON-RPC 2.0 bridge on a Unix domain socket, and enforces timeout/call/output limits. All four decisions are tightly specified in CONTEXT.md with no major ambiguities — this is a clean implementation phase.

The dominant structural model is `TerminalTool` in `terminal.rs`. The core execution pattern (`tokio::process::Command`, `tokio::time::timeout`, output truncation at 50 KB) translates directly. The new complexity layers are: (1) the Unix domain socket server that the Rust parent runs alongside the child process, and (2) the bundled `hermes_tools.py` module the child imports to reach the RPC bridge.

A new `ironhermes-exec` crate will hold the sandbox runtime and RPC server. `ExecuteCodeTool` in `ironhermes-tools` is the thin agent-facing wrapper. The split keeps tool registration simple and avoids circular crate dependencies.

**Primary recommendation:** Model the Rust side on `TerminalTool`, add `tokio::net::UnixListener` for the RPC server (already in `tokio::full`), embed `hermes_tools.py` with `include_str!`, and write it to a tempdir at execution time. No new external crates required — everything needed is already in the workspace dependency set.

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Sandbox Strategy**
- D-01: Allowlist env vars — start clean, pass only PATH, HOME, PYTHONPATH, LANG, IRONHERMES_SESSION_ID, IRONHERMES_RPC_ADDR. All API keys/secrets excluded by default. Satisfies EXEC-03.
- D-02: Secrets-only isolation — no filesystem/network restrictions. Python can read/write files and make HTTP requests. Keeps v1.1 complexity low.
- D-03: Python interpreter path configurable via `exec.python_path` in config.yaml, defaulting to `python3`.
- D-04: Python only — no language parameter. Tool is called `execute_code`.
- D-05: Minimal context passed to scripts — CWD as working directory, safe env vars only. No chat_id or platform info leaks.

**JSON-RPC Bridge**
- D-06: Unix domain socket per execution. Path passed via `IRONHERMES_RPC_ADDR`. Cleaned up on completion.
- D-07: Safe tool subset: `read_file`, `write_file`, `patch`, `search_files`, `web_search`, `web_read`, `memory`. `terminal` and `execute_code` excluded.
- D-08: Bundled `hermes_tools.py` — Rust parent writes it to a tempdir and adds to PYTHONPATH. Zero pip install.
- D-09: Synchronous RPC calls from Python. Rust handles async internally.
- D-10: JSON-RPC 2.0 protocol, newline-delimited over UDS.
- D-11: RPC server tied to child process lifetime. Auto-shutdown on exit/kill.

**Resource Limits**
- D-12: Timeout via `tokio::time::timeout` + SIGKILL. 300 seconds. Same pattern as TerminalTool.
- D-13: Server-side call counter. After 50 calls, return JSON-RPC error. Python helper raises `HermesCallLimitError`.
- D-14: Truncate stdout at 50 KB, append `[truncated: output exceeded 50KB limit]`. Script continues running.
- D-15: Separate stdout and stderr as distinct fields. Tool result format:
  ```
  [stdout]
  <script output>
  [stderr]
  <error output>
  [exit_code: 0]
  ```

### Claude's Discretion
- Crate structure: what lives in `ironhermes-exec` vs `ironhermes-tools` — planner decides the minimal split
- Whether `hermes_tools.py` is embedded as a `include_str!` constant or generated at runtime
- Process group management details for the SIGKILL cleanup
- UDS path format (temp dir naming convention)
- Number of plans — default to 3 per ROADMAP, but planner may adjust

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope.
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| EXEC-01 | Agent can execute Python scripts in an isolated child process via an `execute_code` tool | `ExecuteCodeTool` implements `Tool` trait; child process via `tokio::process::Command`; env stripping via explicit allowlist |
| EXEC-02 | Python scripts can call agent tools (web_search, read_file, etc.) via JSON-RPC over a socket | `tokio::net::UnixListener` RPC server in parent; `hermes_tools.py` in child connects via `IRONHERMES_RPC_ADDR`; JSON-RPC 2.0 newline-delimited protocol |
| EXEC-03 | Child process environment has API keys and secrets stripped for safety | `Command::env_clear()` + explicit allowlist insertion; verified by inspection test |
| EXEC-04 | Code execution enforces timeout (5 min), call limit (50), and stdout cap (50KB) | `tokio::time::timeout(300s)` + SIGKILL; server-side atomic counter; `stdout[..50_000]` truncation with notice |
</phase_requirements>

---

## Standard Stack

### Core (all already in workspace dependencies — no new external crates needed)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `tokio` | 1 (full) | `UnixListener`, `UnixStream`, `timeout`, `process::Command` | Already workspace dep; `tokio::net::UnixListener` is the idiomatic async UDS server [VERIFIED: codebase grep + TECH.md] |
| `tokio::process::Command` | (tokio 1) | Spawn Python child process with env control | Already used in `TerminalTool`; supports `env_clear()`, `envs()`, piped stdio [VERIFIED: terminal.rs] |
| `serde_json` | 1 | JSON-RPC message encoding/decoding | Already workspace dep; used everywhere [VERIFIED: Cargo.toml] |
| `tempfile` | 3 | Create temp directory for UDS socket path and `hermes_tools.py` | Already dev-dep in cron; promote to regular dep for ironhermes-exec [VERIFIED: Cargo.toml] |
| `uuid` | 1 (v4) | Generate unique socket filenames per execution | Already workspace dep [VERIFIED: Cargo.toml] |
| `anyhow` | 1 | Error handling throughout | Already workspace dep [VERIFIED: Cargo.toml] |
| `tracing` | 0.1 | Debug/info logging in exec crate | Already workspace dep [VERIFIED: Cargo.toml] |
| `async-trait` | 0.1 | Tool trait impl | Already workspace dep [VERIFIED: Cargo.toml] |

### No New External Dependencies Required

All required functionality exists in current workspace deps:
- Unix domain sockets: `tokio::net::UnixListener` / `UnixStream` (in `tokio::full`) [ASSUMED — standard tokio feature, not grepped]
- Process management: `tokio::process::Command` with `kill_on_drop(true)` and explicit SIGKILL
- Temp directories: `tempfile::TempDir` (already dev-dep in cron tests)
- JSON encoding: `serde_json`

**Installation (promote tempfile from dev-dep to regular dep in ironhermes-exec):**
```toml
# In crates/ironhermes-exec/Cargo.toml
[dependencies]
tokio = { workspace = true }
serde_json = { workspace = true }
serde = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
async-trait = { workspace = true }
uuid = { workspace = true }
tempfile = "3"

[workspace.dependencies]  # add to root Cargo.toml if promoting
tempfile = "3"
```

---

## Architecture Patterns

### Recommended Project Structure

```
crates/ironhermes-exec/
├── Cargo.toml
└── src/
    ├── lib.rs              # re-exports: Sandbox, SandboxResult, RpcServer
    ├── sandbox.rs          # Sandbox struct: spawn + wait + cleanup orchestration
    ├── rpc_server.rs       # UnixListener RPC server, call counter, tool dispatch
    └── hermes_tools.py     # embedded via include_str! — the Python helper module

crates/ironhermes-tools/src/
└── execute_code.rs         # ExecuteCodeTool: implements Tool trait, delegates to Sandbox
```

### Crate Dependency Addition

```
ironhermes-exec (new leaf — depends only on ironhermes-core + ironhermes-tools)
    ^
    |
ironhermes-tools (adds execute_code.rs, depends on ironhermes-exec)
```

**Concern:** `ironhermes-exec` needs to call tools via `ToolRegistry`, but `ToolRegistry` lives in `ironhermes-tools`. This creates a circular dependency if `ironhermes-exec` depends on `ironhermes-tools`.

**Resolution (Claude's discretion):** `ironhermes-exec` should NOT depend on `ironhermes-tools`. Instead, `ExecuteCodeTool` in `ironhermes-tools` holds the `Arc<ToolRegistry>` reference and passes a dispatch closure or `Arc<ToolRegistry>` into the `Sandbox` at call time. The `Sandbox` calls the tools via a trait object or `Arc<ToolRegistry>` injected by `ExecuteCodeTool`. This matches how `register_memory_tool`, `register_cronjob_tool`, and `register_skills_tool` pass shared state (see `registry.rs` lines 221-244). [VERIFIED: registry.rs]

Alternatively, `ironhermes-exec` takes a `Arc<dyn Fn(String, serde_json::Value) -> BoxFuture<Result<String>>>` — a dispatch callback injected by `ExecuteCodeTool`. Lighter coupling.

### Pattern 1: Sandbox Execution Flow

**What:** Rust creates temp dir, writes `hermes_tools.py`, creates UDS listener, spawns Python, serves RPC requests concurrently with waiting for Python exit, cleans up.

**When to use:** Every `execute_code` tool call.

```rust
// Source: [ASSUMED — based on tokio::net::UnixListener docs pattern]
pub async fn run(
    script: &str,
    python_path: &str,
    tool_dispatch: Arc<dyn ToolDispatch>,
) -> anyhow::Result<SandboxResult> {
    let dir = tempfile::TempDir::new()?;
    let socket_path = dir.path().join("rpc.sock");
    let helper_path = dir.path().join("hermes_tools.py");

    // Write embedded Python helper
    std::fs::write(&helper_path, HERMES_TOOLS_PY)?;

    // Write script to temp file
    let script_path = dir.path().join("script.py");
    std::fs::write(&script_path, script)?;

    // Start UDS RPC server
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let rpc_server = RpcServer::new(listener, tool_dispatch);
    let rpc_handle = tokio::spawn(rpc_server.serve());

    // Spawn Python child
    let mut child = tokio::process::Command::new(python_path)
        .arg(&script_path)
        .env_clear()
        .envs([
            ("PATH", std::env::var("PATH").unwrap_or_default()),
            ("HOME", std::env::var("HOME").unwrap_or_default()),
            ("LANG", std::env::var("LANG").unwrap_or_default()),
            ("PYTHONPATH", dir.path().to_str().unwrap()),
            ("IRONHERMES_RPC_ADDR", socket_path.to_str().unwrap()),
        ])
        .current_dir(&working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    // Enforce timeout
    let result = tokio::time::timeout(
        Duration::from_secs(300),
        child.wait_with_output(),
    ).await;
    // ... handle timeout, truncate, format result
}
```

### Pattern 2: RPC Server Concurrent with Child

**What:** The RPC server task runs concurrently with the child process wait using `tokio::select!` or by joining both futures.

**When to use:** Always — the Python script makes synchronous RPC calls that must be served while the main task waits for the process to exit.

```rust
// Source: [ASSUMED — standard tokio concurrency pattern]
// Two tasks: wait for child exit, and serve RPC requests
// Use tokio::select! to race them, abort RPC on child exit
tokio::select! {
    output = child.wait_with_output() => {
        rpc_handle.abort();
        // process output
    }
    _ = tokio::time::sleep(Duration::from_secs(300)) => {
        // timeout: kill child, abort RPC
        child.kill().await?;
        rpc_handle.abort();
    }
}
```

**Key insight:** The Python script blocks on each RPC call waiting for the socket response. The Rust parent must be actively serving those requests at the same time as it's waiting for the process. `tokio::spawn` for the RPC server is mandatory, not optional.

### Pattern 3: env_clear() + Allowlist

**What:** `Command::env_clear()` strips all environment variables inherited from the parent process, then explicitly re-add only safe vars.

```rust
// Source: [VERIFIED: tokio::process::Command docs pattern, terminal.rs shows base pattern]
Command::new(python_path)
    .env_clear()  // strip ALL env vars including API keys
    .env("PATH", ...)
    .env("HOME", ...)
    .env("LANG", ...)
    .env("PYTHONPATH", helper_dir)
    .env("IRONHERMES_SESSION_ID", session_id)
    .env("IRONHERMES_RPC_ADDR", socket_path)
```

**Verification approach for EXEC-03:** In the Python script, print `os.environ` and assert API key names are absent. This is the "verified by inspection" the success criterion requires.

### Pattern 4: hermes_tools.py Embedding

**What:** Embed the Python helper as a Rust string constant using `include_str!`.

**When to use:** Simplest approach — no runtime generation, no heap allocation, the file is part of the compiled binary.

```rust
// Source: [ASSUMED — standard Rust include_str! pattern]
const HERMES_TOOLS_PY: &str = include_str!("hermes_tools.py");
// then: std::fs::write(helper_path, HERMES_TOOLS_PY)?;
```

The file `crates/ironhermes-exec/src/hermes_tools.py` is adjacent to the Rust source. `include_str!` path is relative to the source file location.

### Pattern 5: JSON-RPC 2.0 over newline-delimited UDS

**What:** Each message is a JSON object terminated by `\n`. Python sends a request, Rust replies. One connection per execution (Python connects once, makes N calls, Python exits).

```python
# hermes_tools.py — Python side [ASSUMED — standard socket JSON pattern]
import socket, json, os

_sock = None

def _connect():
    global _sock
    if _sock is None:
        addr = os.environ["IRONHERMES_RPC_ADDR"]
        _sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        _sock.connect(addr)
    return _sock

def _call(method: str, params: dict) -> str:
    s = _connect()
    req = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    s.sendall((req + "\n").encode())
    buf = b""
    while b"\n" not in buf:
        chunk = s.recv(4096)
        if not chunk:
            raise IOError("RPC connection closed")
        buf += chunk
    line = buf.split(b"\n")[0]
    resp = json.loads(line)
    if "error" in resp:
        code = resp["error"].get("code")
        msg = resp["error"].get("message", "RPC error")
        if code == -32000:
            raise HermesCallLimitError(msg)
        raise HermesRpcError(msg)
    return resp["result"]

def web_search(query: str) -> str:
    return _call("web_search", {"query": query})

def read_file(path: str) -> str:
    return _call("read_file", {"path": path})
# ... etc for each allowed tool
```

### Pattern 6: Process Group Kill for Timeout

**What:** When the timeout fires, kill the entire process group so any child-of-child processes are also cleaned up.

**When to use:** Timeout path only.

```rust
// Source: [ASSUMED — Unix process group pattern]
// Before spawning: set process group = its own pid
// On timeout: kill(-pgid, SIGKILL) via libc or nix crate
// OR: use child.kill() which sends SIGKILL to the direct child
// For v1.1, child.kill() is sufficient — no process-group isolation needed
// since Python scripts don't typically spawn subprocesses
```

**Decision for planner (Claude's Discretion):** `child.kill()` (tokio) sends SIGKILL to the direct Python process. This is sufficient for v1.1 per D-12. Full process group kill (requires `libc` or `nix` crate) can be added if needed. Recommend `child.kill()` for simplicity.

### Anti-Patterns to Avoid

- **Letting RPC server own the `Arc<ToolRegistry>` directly:** Creates a crate dependency cycle. Inject a dispatch closure or `Arc<dyn ToolDispatch>` trait object instead.
- **Keeping UDS socket connection open across multiple scripts:** Each execution gets its own temp dir and socket. No connection reuse.
- **Truncating by byte index without UTF-8 boundary check:** `output[..50_000]` can split a multi-byte character. Use `String::from_utf8_lossy` and truncate to a char boundary. See `str::floor_char_boundary` (stable in Rust 1.73+) or iterate chars.
- **Writing `hermes_tools.py` to a fixed path:** Use a fresh tempdir per execution to prevent race conditions in concurrent agent runs.
- **Sending call limit errors back as `Err` from the tool dispatch:** The RPC layer should return a JSON-RPC error response (code `-32000`) — the Python helper converts that to a Python exception. The Rust tool itself only errors on protocol/IO failures.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Async child process spawning | Custom process management | `tokio::process::Command` | Battle-tested, integrates with tokio runtime, supports `kill_on_drop` [VERIFIED: terminal.rs] |
| Unix domain socket server | Raw `std::os::unix::net` | `tokio::net::UnixListener` | Async, non-blocking, integrates with tokio select [ASSUMED: standard tokio] |
| Temp directory with cleanup | Manual `mkdir` + cleanup | `tempfile::TempDir` | RAII cleanup on drop, handles cleanup even on panic [ASSUMED: standard tempfile] |
| JSON-RPC message framing | Custom binary protocol | Newline-delimited JSON | Simple, debuggable, standard — matches D-10 spec |
| Timeout + kill | Polling loop | `tokio::time::timeout` + `child.kill()` | Exactly the TerminalTool pattern, proven [VERIFIED: terminal.rs] |

**Key insight:** `TerminalTool` already solves 80% of the execution complexity. The new surface area is exclusively the UDS RPC bridge and env stripping.

---

## Common Pitfalls

### Pitfall 1: Deadlock Between Child Stdout and RPC Server

**What goes wrong:** Python script writes large amounts to stdout while simultaneously blocking on an RPC call. If stdout pipe buffer fills (typically 64 KB on Linux), Python blocks trying to write. The Rust parent is also blocked waiting for the RPC response. Classic deadlock.

**Why it happens:** The `wait_with_output()` convenience method reads ALL of stdout/stderr into memory after the process exits. But if the process is blocked on a full pipe buffer, it never exits.

**How to avoid:** Read stdout/stderr incrementally via async tasks, NOT with `wait_with_output()`. Spawn a `tokio::spawn` task to drain stdout/stderr into a buffer while the process runs. Alternatively, use `child.stdout.take()` to get the pipe handle and read it concurrently.

**Warning signs:** Script hangs indefinitely without timeout firing; script produces large stdout before making RPC calls.

**Recommended approach:**
```rust
// Source: [ASSUMED — standard async stdio draining pattern]
let stdout_handle = tokio::spawn(async move {
    let mut buf = Vec::new();
    stdout_reader.read_to_end(&mut buf).await?;
    Ok::<Vec<u8>, io::Error>(buf)
});
```

### Pitfall 2: UDS Socket Path Too Long

**What goes wrong:** Unix domain socket paths have a platform-specific maximum length (104 bytes on macOS, 108 on Linux). A tempdir path like `/var/folders/xx/abc123.../T/ironhermes-exec-<uuid>/rpc.sock` can exceed this on macOS.

**Why it happens:** macOS `sockaddr_un.sun_path` is 104 bytes total including null terminator.

**How to avoid:** Keep the socket path short. Use `/tmp/ih-<6-char-uuid>/rpc.sock` (roughly 28 bytes) rather than a system tempdir path. Or use `std::env::temp_dir()` and truncate the UUID to 8 chars.

**Warning signs:** `bind` returns `ENAMETOOLONG`; socket creation fails on macOS but works on Linux.

### Pitfall 3: Call Counter Race Between Concurrent RPC Requests

**What goes wrong:** If the Python script is somehow multi-threaded (or uses `asyncio`), two RPC requests could arrive simultaneously. The call counter check-and-increment is not atomic, allowing more than 50 calls through.

**Why it happens:** Two threads check `count < 50` simultaneously before either increments.

**How to avoid:** Use an atomic counter (`std::sync::atomic::AtomicU32`) for the call count, or hold it behind the RPC server's single `tokio::task` (which makes it single-threaded by default). Per D-09, Python calls are synchronous — a single connection handles requests sequentially, making this low risk for v1.1. Still, use `AtomicU32` for correctness.

### Pitfall 4: hermes_tools.py Socket Buffer Fragmentation

**What goes wrong:** `socket.recv(4096)` may return a partial JSON message if the response is larger than 4096 bytes (which is likely for large file reads). The Python helper tries to `json.loads` a partial response and crashes.

**Why it happens:** TCP/UDS `recv` returns whatever is in the kernel buffer — it does not guarantee complete messages.

**How to avoid:** In `hermes_tools.py`, accumulate into a buffer until `\n` is found before calling `json.loads`. The code example above in Pattern 5 demonstrates the correct buffering loop.

### Pitfall 5: UTF-8 Boundary Truncation

**What goes wrong:** `&output[..50_000]` panics or corrupts text if byte 50_000 falls inside a multi-byte UTF-8 character.

**How to avoid:** Use `String::from_utf8_lossy` to convert raw bytes, then truncate to a char boundary:
```rust
// Source: [ASSUMED — standard Rust string truncation pattern]
let s = String::from_utf8_lossy(&stdout_bytes);
let truncated = if s.len() > MAX_OUTPUT_LEN {
    let boundary = s.floor_char_boundary(MAX_OUTPUT_LEN); // Rust 1.73+
    &s[..boundary]
} else {
    &s
};
```

### Pitfall 6: PYTHONPATH Overwriting Existing Value

**What goes wrong:** If the user's `python3` environment already has a `PYTHONPATH` set and the env is cleared, then only the tempdir is in PYTHONPATH, which is correct. But if PYTHONPATH needs to include additional paths (e.g., the user's venv site-packages), those are lost.

**How to avoid:** Per D-02 and D-03, users point `exec.python_path` to their venv interpreter. The venv interpreter already knows its own site-packages via `sys.prefix` — no PYTHONPATH manipulation needed for venv packages. The PYTHONPATH we set is exclusively for `hermes_tools.py`. This is correct and intentional.

---

## Code Examples

Verified patterns from existing codebase and standard Rust async idioms:

### ExecuteCodeTool Registration Pattern (matches existing tools)

```rust
// Source: [VERIFIED: registry.rs lines 221-244]
// In ToolRegistry:
pub fn register_execute_code_tool(&mut self, registry: Arc<ToolRegistry>, config: ExecConfig) {
    use crate::execute_code::ExecuteCodeTool;
    self.register(Box::new(ExecuteCodeTool::new(registry, config)));
}
```

But since `ExecuteCodeTool` needs `Arc<ToolRegistry>` (itself), the pattern needs care.
The tool receives an `Arc<ToolRegistry>` for dispatching RPC-proxied tools. This arc must be created before registering `ExecuteCodeTool`. Pattern:

```rust
// Source: [ASSUMED — registry self-reference pattern]
let registry = Arc::new(ToolRegistry::new());
// ... register other tools ...
// Then register execute_code with a reference to the registry:
let exec_tool = ExecuteCodeTool::new(Arc::clone(&registry), exec_config);
// Can't register into Arc<ToolRegistry> — need Arc<Mutex<ToolRegistry>> or register before Arc-wrapping
```

**Resolution:** Register `execute_code` before wrapping in `Arc`, passing the exec config. The `ExecuteCodeTool` holds an `Arc<ToolRegistry>` that is populated after the fact via an `Arc<RwLock<Option<Arc<ToolRegistry>>>>` — or more simply, the tool receives a dispatch closure at registration time. The planner should decide the cleanest pattern; the simplest is to build the full `ToolRegistry`, wrap in `Arc`, then register `ExecuteCodeTool` via a separate mutable handle before the final `Arc` wrap. (The current codebase builds the registry mutably then wraps in Arc in `main.rs`.)

### TerminalTool Timeout Pattern (direct template)

```rust
// Source: [VERIFIED: terminal.rs lines 92-103]
let result = timeout(Duration::from_secs(timeout_secs), fut)
    .await
    .map_err(|_| anyhow::anyhow!("Command timed out after {}s", timeout_secs))??;

if result.len() > MAX_OUTPUT_LEN {
    let truncated = &result[..MAX_OUTPUT_LEN];
    Ok(format!("{}\n[truncated]", truncated))
} else {
    Ok(result)
}
```

For EXEC-04, replace the timeout message with EXEC-04-specific text and the truncation notice with `[truncated: output exceeded 50KB limit]`.

### ExecConfig Structure (follows SkillsConfig pattern)

```rust
// Source: [ASSUMED — modeled on SkillsConfig in config.rs lines 196-211]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecConfig {
    /// Path to the Python interpreter. Default: "python3". (D-03)
    pub python_path: String,
    /// Timeout in seconds. Default: 300 (5 minutes). (D-12)
    pub timeout_secs: u64,
    /// Maximum RPC calls per execution. Default: 50. (D-13)
    pub max_rpc_calls: u32,
    /// Maximum stdout bytes before truncation. Default: 51200 (50KB). (D-14)
    pub max_output_bytes: usize,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            python_path: "python3".to_string(),
            timeout_secs: 300,
            max_rpc_calls: 50,
            max_output_bytes: 50_000,
        }
    }
}
// Added to Config struct as: pub exec: ExecConfig,
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `std::process::Command` | `tokio::process::Command` | Tokio 0.2+ | Non-blocking child process; compatible with async runtime |
| `std::os::unix::net::UnixListener` | `tokio::net::UnixListener` | Tokio 1.0 | Async UDS accept/read/write without blocking thread |
| Polling for process exit | `child.wait_with_output().await` | Tokio 1.0 | Async wait, efficient |
| Separate `nix` crate for signals | `tokio::process::Child::kill()` | Tokio 1.x | `kill()` sends SIGKILL; sufficient for v1.1 without adding `nix` dep |

**Relevant to this phase:**
- `tempfile::TempDir` is the idiomatic RAII temp directory in Rust — no custom cleanup needed [ASSUMED]
- `include_str!()` is the standard way to embed text files in Rust binaries — works with `hermes_tools.py` as-is [ASSUMED]
- `tokio::net::UnixListener` supports async `accept()` which returns `tokio::net::UnixStream` — use `BufReader` + `lines()` for newline framing [ASSUMED]

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `tokio::net::UnixListener` is available in `tokio = { version = "1", features = ["full"] }` | Standard Stack | Would need to add `net` feature explicitly; low risk since `full` includes all features |
| A2 | `include_str!("hermes_tools.py")` works when the `.py` file is in `src/` alongside Rust source | Architecture Patterns — Pattern 4 | Would need to use `build.rs` or different embedding approach; low risk, this is standard |
| A3 | `tokio::process::Child::kill()` is sufficient for SIGKILL without the `nix` crate | Architecture Patterns — Pattern 6 | Would need `nix` or `libc` dep for process group kill; low risk since Python scripts typically don't spawn children |
| A4 | `tempfile::TempDir` drop behavior cleans up the socket file before Rust drops the `UnixListener` | Common Pitfalls | Potential ordering issue: listener should be dropped before TempDir; planner must ensure correct drop order |
| A5 | `str::floor_char_boundary` is stable in the project's Rust toolchain (Rust 1.73+) | Code Examples | If toolchain is older, need manual char boundary scan; TECH.md says edition 2024 requires 1.85+, so this is safe |
| A6 | Python 3 standard library `socket` module supports `AF_UNIX` on macOS | Architecture Patterns — Pattern 5 | macOS supports AF_UNIX since macOS 10.x; Python 3.9 (detected on this machine) includes it |

**If this table is empty:** All claims in this research were verified or cited — no user confirmation needed.
_(Table is not empty — 6 assumptions flagged above for planner awareness.)_

---

## Open Questions

1. **ExecuteCodeTool self-reference to ToolRegistry**
   - What we know: `ExecuteCodeTool` needs `Arc<ToolRegistry>` to dispatch RPC tool calls, but it is itself registered in `ToolRegistry`. The current registry is built mutably then wrapped in `Arc`.
   - What's unclear: Whether to use a dispatch closure, a `Arc<Mutex<Option<Arc<ToolRegistry>>>>` that's populated after construction, or build the registry in two passes.
   - Recommendation: Two-pass build — create `ToolRegistry`, populate it with all other tools, capture an `Arc<ToolRegistry>` for the exec tool, then add `ExecuteCodeTool` last via a secondary step. Since `ToolRegistry` stores `Box<dyn Tool>` in a `HashMap`, and the `HashMap` is mutably accessible before `Arc`-wrapping, this is straightforward. The planner should codify this in the build sequence task.

2. **Drop order: TempDir vs. UnixListener**
   - What we know: Rust drops in reverse declaration order. The `TempDir` must outlive the `UnixListener` (or else `bind` fails because the path is gone). But we also need the `TempDir` to be cleaned up after the listener is done.
   - What's unclear: Whether `TempDir` drop removing the socket file causes issues for the `UnixListener` on macOS vs. Linux.
   - Recommendation: Declare `listener` before `dir` so `dir` drops after `listener`. Or explicitly drop listener before dir. Planner should document drop order in the sandbox cleanup code.

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Python 3 | EXEC-01, EXEC-02 | Yes | 3.9.6 at `/usr/bin/python3` | Configurable via `exec.python_path` (D-03) |
| Unix domain sockets | EXEC-02 (D-06) | Yes | macOS 25.4 (Darwin) | No fallback needed — macOS/Linux both support UDS |
| `tokio::net::UnixListener` | EXEC-02 | Yes (tokio full) | tokio 1.x | No fallback needed |
| `tempfile` crate | Sandbox temp dirs | Available (dev-dep in cron) | 3.x | Promote from dev-dep to regular dep |

**Missing dependencies with no fallback:** None.

**Missing dependencies with fallback:** None — all required capabilities are available.

**Note:** Python 3.9.6 is the system Python on this machine. The default `python3` path works. Users on venvs will configure `exec.python_path` per D-03.

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in (`#[test]`, `#[tokio::test]`) |
| Config file | None — standard `cargo test` |
| Quick run command | `cargo test -p ironhermes-exec` |
| Full suite command | `cargo test` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| EXEC-01 | `execute_code` runs a Python script and returns stdout | unit/integration | `cargo test -p ironhermes-exec test_execute_simple_script` | No — Wave 0 |
| EXEC-02 | Python calls `web_search` via RPC and gets a mocked result | unit | `cargo test -p ironhermes-exec test_rpc_tool_call` | No — Wave 0 |
| EXEC-03 | Child env has no API keys — verified by script printing `os.environ` | unit | `cargo test -p ironhermes-exec test_env_stripping` | No — Wave 0 |
| EXEC-04a | Script exceeding 300s is killed and returns timeout error | unit | `cargo test -p ironhermes-exec test_timeout_kills_process` | No — Wave 0 |
| EXEC-04b | Script output > 50KB is truncated with notice | unit | `cargo test -p ironhermes-exec test_output_truncation` | No — Wave 0 |
| EXEC-04c | After 50 RPC calls, subsequent calls return call-limit error | unit | `cargo test -p ironhermes-exec test_call_limit` | No — Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-exec`
- **Per wave merge:** `cargo test`
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `crates/ironhermes-exec/src/lib.rs` — crate skeleton
- [ ] `crates/ironhermes-exec/src/sandbox.rs` — Sandbox struct
- [ ] `crates/ironhermes-exec/src/rpc_server.rs` — RPC server
- [ ] `crates/ironhermes-exec/src/hermes_tools.py` — embedded Python helper
- [ ] `crates/ironhermes-exec/Cargo.toml` — crate manifest
- [ ] `crates/ironhermes-tools/src/execute_code.rs` — ExecuteCodeTool
- [ ] Add `crates/ironhermes-exec` to root `Cargo.toml` workspace members
- [ ] Add `exec: ExecConfig` to `Config` struct in `ironhermes-core/src/config.rs`

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | No | N/A — no auth in exec path |
| V3 Session Management | No | N/A |
| V4 Access Control | Yes | Allowlist env vars (D-01); tool subset allowlist (D-07); `execute_code` excluded from child RPC |
| V5 Input Validation | Yes | Script content passed as string — no injection risk since it goes to a new process, not a shell. Script path is a temp file not user-controlled. |
| V6 Cryptography | No | N/A |

### Known Threat Patterns for Code Execution Sandbox

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Secret exfiltration via env var leak | Information Disclosure | `env_clear()` + explicit allowlist (D-01, EXEC-03) |
| RPC recursion (execute_code calling execute_code) | Elevation of Privilege | `execute_code` not in RPC tool subset (D-07) |
| `terminal` tool bypass via RPC | Elevation of Privilege | `terminal` not in RPC tool subset (D-07) |
| Resource exhaustion via infinite loop | Denial of Service | 300s timeout + SIGKILL (D-12) |
| Output flooding | Denial of Service | 50KB stdout cap (D-14) |
| RPC call flooding (expensive tool calls) | Denial of Service | 50 call limit server-side (D-13) |
| Script writing to sensitive paths via `write_file` | Tampering | Out of scope for v1.1 (D-02); existing `WriteFileTool` has no path restriction |
| UDS socket hijacking | Spoofing | Temp dir is in `/tmp` with process-unique path; risk is low in single-operator deployment |

**Security note on D-02:** The decision to allow unrestricted filesystem/network access from the Python sandbox is a deliberate scope reduction for v1.1. The guardrail hook system (Phase 6) does NOT automatically intercept RPC-proxied tool calls — the RPC server dispatches directly via `ToolRegistry`. This means `write_file` called from Python bypasses any guardrail hooks that would apply to direct agent tool calls. The planner should note this as a known limitation, documented but not fixed in Phase 8.

---

## Sources

### Primary (HIGH confidence)
- [VERIFIED: codebase] `crates/ironhermes-tools/src/terminal.rs` — TerminalTool execution pattern (timeout, truncation, tokio::process::Command)
- [VERIFIED: codebase] `crates/ironhermes-tools/src/registry.rs` — Tool trait, register pattern, shared-state registration methods
- [VERIFIED: codebase] `crates/ironhermes-core/src/config.rs` — Config struct pattern, SkillsConfig as ExecConfig template
- [VERIFIED: codebase] `Cargo.toml` — workspace dependency versions
- [VERIFIED: codebase] `.planning/codebase/ARCH.md` — crate dependency graph, concurrency model
- [VERIFIED: codebase] `.planning/codebase/TECH.md` — dependency versions, async patterns

### Secondary (MEDIUM confidence)
- [CITED: tokio docs] `tokio::process::Command` — env_clear(), kill_on_drop(), wait_with_output()
- [CITED: Rust stdlib] `include_str!` macro — compile-time file embedding
- [CITED: Rust stdlib] `str::floor_char_boundary` — stable since Rust 1.73

### Tertiary (LOW confidence / ASSUMED)
- tokio::net::UnixListener async UDS accept pattern — standard tokio, not explicitly verified via Context7
- tempfile::TempDir RAII behavior — standard crate, training knowledge only
- Python 3 `socket.AF_UNIX` availability on macOS — assumed from platform knowledge

---

## Metadata

**Confidence breakdown:**
- Standard Stack: HIGH — all deps verified in Cargo.toml; no new external crates needed
- Architecture: HIGH for TerminalTool template; MEDIUM for UDS/RPC patterns (standard but not verified via Context7)
- Pitfalls: HIGH — stdout deadlock and UDS path length are well-documented platform issues

**Research date:** 2026-04-10
**Valid until:** 2026-05-10 (stable tokio APIs, no fast-moving dependencies)
