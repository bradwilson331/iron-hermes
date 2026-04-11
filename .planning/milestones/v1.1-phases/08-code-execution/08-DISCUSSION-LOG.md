# Phase 8: Code Execution - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-10
**Phase:** 08-code-execution
**Areas discussed:** Sandbox strategy, JSON-RPC bridge, Resource limits

---

## Sandbox Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Allowlist env vars | Clean env, only pass safe vars (PATH, HOME, PYTHONPATH, LANG) | ✓ |
| Denylist env vars | Inherit parent env, strip known secret patterns | |
| You decide | Claude picks safest approach | |

**User's choice:** Allowlist env vars
**Notes:** Matches EXEC-03 intent exactly. Everything not explicitly allowed is excluded.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Secrets only | Strip env secrets + resource limits, no FS/network restrictions | ✓ |
| Filesystem + network restrictions | OS-level sandboxing (sandbox-exec, seccomp) | |
| Temp directory jail | Run in temp dir, can still access FS via absolute paths | |

**User's choice:** Secrets only
**Notes:** Keeps complexity low for v1.1. Matches hermes-agent's Python approach.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Search PATH | Use `python3` from system PATH | |
| Configurable in config.yaml | `exec.python_path` setting with `python3` default | ✓ |
| You decide | Claude picks | |

**User's choice:** Configurable in config.yaml
**Notes:** Lets users point to a venv or specific interpreter.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Python only | Implicit Python, no language parameter | ✓ |
| Language parameter | Accept `language` param, default python | |

**User's choice:** Python only
**Notes:** EXEC-01 says Python. Other languages can be added later.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Minimal context | CWD + IRONHERMES_SESSION_ID + IRONHERMES_RPC_ADDR | ✓ |
| Rich context | session_id, chat_id, platform, CWD via env vars | |
| No context | Clean slate, only code + RPC bridge | |

**User's choice:** Minimal context
**Notes:** No chat_id or platform info leaks into sandbox.

---

## JSON-RPC Bridge

| Option | Description | Selected |
|--------|-------------|----------|
| Unix domain socket | Temp UDS per execution, path via env var | ✓ |
| TCP localhost | Ephemeral port on 127.0.0.1 | |
| Stdin/stdout pipes | JSON-RPC over stdin/stdout | |

**User's choice:** Unix domain socket
**Notes:** No port conflicts, no network exposure, fast IPC. macOS + Linux native.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Safe subset | read_file, write_file, patch, search_files, web_search, web_read, memory | ✓ |
| All except execute_code | Full agent capability minus recursion | |
| Configurable allowlist | exec.allowed_tools in config.yaml | |

**User's choice:** Safe subset
**Notes:** terminal excluded (defeats isolation), execute_code excluded (prevents recursion).

---

| Option | Description | Selected |
|--------|-------------|----------|
| Bundled helper module | hermes_tools.py written to temp dir, added to PYTHONPATH | ✓ |
| Inline in script preamble | Prepend RPC client code to every script | |
| Require pip install | hermes-tools PyPI package | |

**User's choice:** Bundled helper module
**Notes:** Zero pip install needed. Scripts do `from hermes_tools import web_search`.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Synchronous | `result = web_search("query")` blocks until Rust returns | ✓ |
| Async (asyncio) | `result = await web_search("query")` | |

**User's choice:** Synchronous
**Notes:** Simple for script authors. Rust handles async internally.

---

| Option | Description | Selected |
|--------|-------------|----------|
| JSON-RPC 2.0 | Standard protocol, newline-delimited over UDS | ✓ |
| Simple request/response | Custom lightweight format | |

**User's choice:** JSON-RPC 2.0
**Notes:** Well-documented, matches EXEC-02 specification.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Auto-shutdown on exit | RPC server tied to child process lifetime | ✓ |
| Grace period | Keep server alive briefly after process exit | |

**User's choice:** Auto-shutdown on exit
**Notes:** Clean, no orphaned listeners.

---

## Resource Limits

| Option | Description | Selected |
|--------|-------------|----------|
| tokio::time::timeout + kill | Wrap wait in 300s timeout, SIGKILL on expiry | ✓ |
| SIGTERM then SIGKILL | Graceful shutdown attempt before hard kill | |
| You decide | Claude picks | |

**User's choice:** tokio::time::timeout + kill
**Notes:** Same proven pattern as TerminalTool.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Server-side counter | Rust RPC server tracks count, returns error after 50 | ✓ |
| Python-side counter | hermes_tools.py tracks count | |
| Both sides | Enforce on Rust and Python | |

**User's choice:** Server-side counter
**Notes:** No way to bypass from Python side.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Truncate with notice | Keep first 50KB, append truncation notice | ✓ |
| Kill on overflow | Kill process when stdout hits 50KB | |
| Tail truncation | Keep last 50KB instead of first | |

**User's choice:** Truncate with notice
**Notes:** Same pattern as TerminalTool. Script continues running.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Separate fields | stdout and stderr as distinct fields in tool result | ✓ |
| Combined like TerminalTool | Merge into one string | |
| You decide | Claude picks | |

**User's choice:** Separate fields
**Notes:** Lets LLM distinguish normal output from errors.

---

## Claude's Discretion

- Crate structure (ironhermes-exec vs ironhermes-tools split)
- hermes_tools.py embedding strategy (include_str! vs runtime generation)
- Process group management details for SIGKILL cleanup
- UDS path format / temp dir naming
- Number of plans

## Deferred Ideas

None — discussion stayed within phase scope.
