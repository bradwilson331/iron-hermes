---
phase: 08-code-execution
reviewed: 2026-04-10T12:00:00Z
depth: standard
files_reviewed: 13
files_reviewed_list:
  - Cargo.toml
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-core/src/config.rs
  - crates/ironhermes-core/src/lib.rs
  - crates/ironhermes-exec/Cargo.toml
  - crates/ironhermes-exec/src/hermes_tools.py
  - crates/ironhermes-exec/src/lib.rs
  - crates/ironhermes-exec/src/rpc_server.rs
  - crates/ironhermes-exec/src/sandbox.rs
  - crates/ironhermes-tools/Cargo.toml
  - crates/ironhermes-tools/src/execute_code.rs
  - crates/ironhermes-tools/src/lib.rs
  - crates/ironhermes-tools/src/registry.rs
findings:
  critical: 1
  warning: 4
  info: 2
  total: 7
status: issues_found
---

# Phase 8: Code Review Report

**Reviewed:** 2026-04-10T12:00:00Z
**Depth:** standard
**Files Reviewed:** 13
**Status:** issues_found

## Summary

Reviewed the code execution sandbox subsystem (ironhermes-exec) and its integration into the tool registry (ironhermes-tools). The architecture is well-designed: env stripping, RPC tool allowlist, output truncation, timeout enforcement, and structural recursion prevention via separate registries are all sound patterns. Test coverage is good with targeted tests for key security invariants.

Found one critical issue (sandbox timeout loses buffered output, producing misleading empty results), four warnings (unbounded recv buffer in Python bridge, call counter overflow edge case, PATH passthrough reduces sandbox isolation, discarded buffer data), and two informational items.

## Critical Issues

### CR-01: Sandbox timeout branch silently discards partial stdout/stderr

**File:** `crates/ironhermes-exec/src/sandbox.rs:136-148`
**Issue:** When the sandbox times out, stdout and stderr are returned as empty strings. However, the child process may have already written substantial output before the timeout fired. The stdout/stderr byte vectors were moved into the `tokio::time::timeout` async block (lines 100-118) and are inaccessible in the `Err(_elapsed)` timeout branch (line 136). This means the agent receives no diagnostic information about what the script did before it was killed, making debugging impossible and potentially hiding error messages.

**Fix:** Use shared buffers (e.g., `Arc<Mutex<Vec<u8>>>`) that can be read from both the success and timeout branches, or restructure to drain available output before reporting the timeout:

```rust
// Use shared buffers accessible from both branches
let stdout_buf = Arc::new(tokio::sync::Mutex::new(Vec::new()));
let stderr_buf = Arc::new(tokio::sync::Mutex::new(Vec::new()));

let stdout_buf_clone = Arc::clone(&stdout_buf);
let stderr_buf_clone = Arc::clone(&stderr_buf);

let result = tokio::time::timeout(timeout_duration, async {
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout_handle.read_to_end(&mut buf).await.ok();
        *stdout_buf_clone.lock().await = buf;
    });
    // ... similar for stderr ...
    let status = child.wait().await;
    stdout_task.await.ok();
    stderr_task.await.ok();
    status
})
.await;

match result {
    Err(_elapsed) => {
        // Can still read partial output from shared buffers
        let partial_stdout = stdout_buf.lock().await;
        let partial_stderr = stderr_buf.lock().await;
        // ...
    }
    // ...
}
```

## Warnings

### WR-01: Python RPC client has unbounded recv buffer (potential OOM)

**File:** `crates/ironhermes-exec/src/hermes_tools.py:55-59`
**Issue:** The `_call` function accumulates bytes in `buf` without any size limit until a newline is found. If the RPC server (or a bug) sends large amounts of data without a newline delimiter, the Python process could exhaust memory. While the RPC server is controlled code, defense-in-depth is appropriate for a sandbox bridge.

**Fix:** Add a maximum buffer size check:

```python
MAX_RESPONSE_BYTES = 10 * 1024 * 1024  # 10 MB

buf = b""
while b"\n" not in buf:
    chunk = s.recv(65536)
    if not chunk:
        raise IOError("RPC connection closed unexpectedly")
    buf += chunk
    if len(buf) > MAX_RESPONSE_BYTES:
        raise IOError("RPC response exceeded maximum size")
```

### WR-02: RPC call counter wraps on overflow allowing bypass

**File:** `crates/ironhermes-exec/src/rpc_server.rs:103-104`
**Issue:** `fetch_add(1, Ordering::SeqCst)` always increments the counter, even when the limit is already exceeded (line 104). If a script makes enough blocked calls (~4 billion), the `AtomicU32` wraps to 0, re-enabling calls. While extremely unlikely in a single execution, it is a correctness issue that violates the call-limit invariant.

**Fix:** Use `compare_exchange` or check-then-increment to avoid incrementing past the limit:

```rust
loop {
    let current = self.call_count.load(Ordering::SeqCst);
    if current >= self.max_calls {
        warn!("RPC call limit exceeded ({} calls)", self.max_calls);
        return Self::error_response(
            id, -32000,
            &format!("RPC call limit exceeded ({} calls)", self.max_calls),
        );
    }
    if self.call_count.compare_exchange(
        current, current + 1, Ordering::SeqCst, Ordering::SeqCst
    ).is_ok() {
        break;
    }
}
```

### WR-03: Full host PATH passed to sandbox reduces isolation

**File:** `crates/ironhermes-exec/src/sandbox.rs:76`
**Issue:** The sandbox passes through the host's complete `PATH` environment variable. This means sandboxed Python scripts can execute any binary on the host system (e.g., `os.system("curl ...")`, `subprocess.run(["rm", "-rf", ...])` etc.). While `terminal` and `execute_code` tools are blocked at the RPC layer, the Python script itself can use `os.system`, `subprocess`, or `os.exec*` directly to run arbitrary host commands. This significantly weakens the sandbox's isolation guarantees.

**Fix:** Restrict PATH to a minimal set of known-safe directories:

```rust
.env("PATH", "/usr/bin:/usr/local/bin")
```

Or even better, construct a minimal PATH containing only the Python interpreter's directory.

### WR-04: Python RPC client discards post-newline buffer data

**File:** `crates/ironhermes-exec/src/hermes_tools.py:62`
**Issue:** `buf.split(b"\n", 1)[0]` extracts the first line but discards any bytes after the newline. Since `buf` is a local variable and the socket has already consumed those bytes from the kernel buffer, if the server ever sends data ahead of the client's next request (e.g., due to a race or pipelining), subsequent `_call` invocations will read partial/corrupt data. The current server implementation sends exactly one response per request, so this is not triggered today, but it is fragile and would break silently if the protocol evolves.

**Fix:** Preserve the remainder in a module-level buffer:

```python
_recv_buf = b""

def _call(method, params):
    global _request_id, _recv_buf
    _request_id += 1
    s = _connect()
    req = json.dumps({...})
    s.sendall((req + "\n").encode("utf-8"))

    while b"\n" not in _recv_buf:
        chunk = s.recv(65536)
        if not chunk:
            raise IOError("RPC connection closed unexpectedly")
        _recv_buf += chunk

    line, _recv_buf = _recv_buf.split(b"\n", 1)
    resp = json.loads(line)
    # ...
```

## Info

### IN-01: Unused binding `_text` suggests missing non-streaming output

**File:** `crates/ironhermes-cli/src/main.rs:332`
**Issue:** The variable is bound as `_text` (prefixed underscore to suppress unused warning) but the response text is never printed. In streaming mode the output has already been printed incrementally, but in non-streaming mode (if streaming were disabled), the response would be silently dropped. The current default is `stream: true`, so this is not a bug today, but the dead binding suggests incomplete handling.

**Fix:** Either print the text when not in streaming mode, or add a comment explaining why it is intentionally discarded:

```rust
// Streaming already printed the text incrementally; just add newline separator
if let Some(_) = response {
    println!();
}
```

### IN-02: Duplicate guardrail iteration logic in registry

**File:** `crates/ironhermes-tools/src/registry.rs:86-109` and `crates/ironhermes-tools/src/registry.rs:166-188`
**Issue:** `check_guardrails` and `dispatch_with_hook` both iterate over guardrails with identical match logic. If guardrail behavior changes (e.g., adding a new decision variant), both code paths must be updated in lockstep. This is a maintenance hazard.

**Fix:** Have `dispatch_with_hook` call `check_guardrails` internally rather than reimplementing the loop:

```rust
pub async fn dispatch_with_hook<F>(
    &self, name: &str, args: serde_json::Value, post_guardrail_hook: Option<F>,
) -> anyhow::Result<String>
where F: FnOnce(&str, &str),
{
    match self.check_guardrails(name, &args) {
        GuardrailDecision::Block { reason } => {
            let error_msg = format_guardrail_error(name, &reason, /* ... */);
            return Err(anyhow::anyhow!("{}", error_msg));
        }
        _ => {} // Allow and Warn both proceed
    }
    // fire hook, then execute
}
```

Note: This would lose the per-guardrail error formatting (guardrail name). The refactor would need `check_guardrails` to return the blocking guardrail's name as well.

---

_Reviewed: 2026-04-10T12:00:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
