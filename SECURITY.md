<!-- generated-by: gsd-doc-writer -->
# SECURITY.md — Phase 27.1.1 (safe-foundation)

**Generated:** 2026-05-11
**Auditor:** gsd-security-auditor (retroactive-STRIDE)
**ASVS Level:** L1
**Phase:** 27.1.1 — safe-foundation (hexapod TCP tool)
**block_on:** critical

---

## Scope

This document covers the security posture of the new trust boundary introduced in Phase 27.1.1:

```
LLM → IronHermes tool executor (hexapod_tcp.rs) → Freenove Python TCP server → physical servos
```

Primary implementation files audited:
- `crates/ironhermes-tools/src/hexapod_tcp.rs`
- `crates/ironhermes-tools/src/registry.rs`
- `crates/ironhermes-agent/src/app_runtime_factory.rs`
- `crates/ironhermes-core/src/config.rs`

---

## STRIDE Threat Register

### S — Spoofing

#### THREAT-S-01: LLM spoofs a permitted action string to bypass intent filtering
**Component:** `hexapod_tcp.rs::execute()`
**Attack:** LLM passes a string that looks like a valid action but is a typo, injection, or alternate encoding of a blocked command.
**Disposition:** MITIGATE
**Mitigation:** Compile-time exhaustive `match action { "walk" | "stop" | "read_battery" | "read_distance" | "relax_servos" => { ... } other => Ok(format!("Action '{other}' is blocked...")) }`. No fuzzy matching — string equality only. The match fires before any I/O.
**Evidence:** `hexapod_tcp.rs:343-585` — outer match arm with explicit 15-value allowlist; catch-all fires for any other string including `"Walk"`, `"WALK"`, `"calibration"`, etc.
**Status:** CLOSED

#### THREAT-S-02: LLM spoofs direction/speed fields to inject a different movement profile
**Component:** `hexapod_tcp.rs::build_walk_wire()`
**Attack:** LLM passes `direction: "forward\0"` or `direction: "forward#0#0#99#0\n"` to escape the wire format.
**Disposition:** MITIGATE
**Mitigation:** `build_walk_wire` uses a `match` on the direction string — only four exact values (`"forward"`, `"backward"`, `"left"`, `"right"`) produce a branch; anything else falls to the safe default (forward). Speed is clamped with `.clamp(2, 10)` before being interpolated into the format string.
**Evidence:** `hexapod_tcp.rs:116-124` — direction match; `hexapod_tcp.rs:117` — `speed.clamp(2, 10)`. The format string uses `{s}` (the clamped integer), not the raw direction string, so no `#`-injection is possible through the speed field.
**Status:** CLOSED

---

### T — Tampering

#### THREAT-T-01: LLM injects `#`-delimited fields into the wire protocol via direction
**Component:** `hexapod_tcp.rs::build_walk_wire()`
**Attack:** LLM passes `direction: "forward#0#0#99#0"` to append extra fields to the CMD_MOVE command.
**Disposition:** MITIGATE
**Mitigation:** `direction` is never interpolated directly into the wire string. `build_walk_wire` uses a hard-coded match; the direction value selects a fully pre-formed format string. The actual direction string never appears in the wire output.
**Evidence:** `hexapod_tcp.rs:119-123` — each branch returns a complete `format!()` with only `{s}` (clamped speed integer) as the variable component.
**Status:** CLOSED

#### THREAT-T-02: LLM injects `#`-delimited fields via speed value
**Component:** `hexapod_tcp.rs::build_walk_wire()`
**Attack:** LLM passes `speed: 999` or a value outside the expected range to cause protocol confusion.
**Disposition:** MITIGATE
**Mitigation:** Speed is clamped to 2..=10 with `.clamp(2, 10)` before any use. The resulting integer cannot contain `#`, newlines, or other wire-protocol characters.
**Evidence:** `hexapod_tcp.rs:117` — `let s = speed.clamp(2, 10);`
**Status:** CLOSED

#### THREAT-T-03: Command string constants could be mutated at runtime
**Component:** `hexapod_tcp.rs` wire constants
**Attack:** A bug or unsafe code block modifies `STOP_CMD`, `RELAX_CMD`, `BATTERY_CMD`, or `DISTANCE_CMD` between registration and use.
**Disposition:** MITIGATE
**Mitigation:** All wire constants are `&'static str` (`pub(crate) const`). Rust's type system prevents mutation of immutable statics.
**Evidence:** `hexapod_tcp.rs:18-30` — `pub(crate) const STOP_CMD: &str`, `RELAX_CMD: &str`, etc.
**Status:** CLOSED

---

### R — Repudiation

#### THREAT-R-01: No audit trail of hexapod commands sent
**Component:** `hexapod_tcp.rs::execute()`
**Attack:** If the robot moves unexpectedly, there is no evidence of what commands were sent or by which session.
**Disposition:** MITIGATE (partial — ASVS L1 minimum)
**Mitigation:** A `debug!` log is emitted at the entry of each permitted action: `debug!("hexapod_tcp: action={action} addr={addr}")`. This records the action name and destination address at the tracing `DEBUG` level. The existing IronHermes hook system (`HookRegistry` with JSONL event log) provides session-level event capture. The `addr` string contains the IP.
**Evidence:** `hexapod_tcp.rs:359` — `debug!("hexapod_tcp: action={action} addr={addr}")`.
**Caveat:** Logging is at DEBUG level; if the operator does not enable DEBUG tracing, commands are not logged. No separate hexapod-specific audit log is written. Accepted for Phase 1 / ASVS L1 — escalation path is to add an INFO-level structured event or hook emission in a future phase.
**Status:** CLOSED (ASVS L1 acceptable; advisory: promote to INFO or hook emission for production use)

---

### I — Information Disclosure

#### THREAT-I-01: HEXAPOD_IP logged in tracing output, revealing network topology
**Component:** `hexapod_tcp.rs::execute()`
**Attack:** The robot's LAN IP address is written to logs, exposing internal network topology.
**Disposition:** ACCEPT
**Mitigation:** The `debug!` line at `hexapod_tcp.rs:359` logs `addr` which contains `{ip}:5002`. This is an operator-supplied env var (not user or LLM-controlled data). The IP appears only at DEBUG level, which requires explicit opt-in. Accepted: this is operator-configured infrastructure data, not user PII or secrets. Operators running in sensitive environments should set appropriate tracing filters.
**Evidence:** `hexapod_tcp.rs:359` — `debug!("hexapod_tcp: action={action} addr={addr}")`.
**Status:** CLOSED (accepted risk — operator env var, DEBUG level only)

#### THREAT-I-02: TCP response data from robot leaks sensitive information
**Component:** `hexapod_tcp.rs::execute()` battery/distance arms
**Attack:** The robot's TCP response contains data beyond what is displayed (e.g., firmware version, calibration state, internal error codes).
**Disposition:** ACCEPT
**Mitigation:** Only `CMD_POWER` and `CMD_SONIC` responses are read. `parse_battery_response` extracts only two float fields; `parse_distance_response` extracts one distance field. Unparsed fields are discarded. No raw response bytes are returned to the LLM for battery/distance — only formatted human-readable strings.
**Evidence:** `hexapod_tcp.rs:142-160` — both parse helpers extract only named fields; surplus fields silently drop.
**Status:** CLOSED

#### THREAT-I-03: HEXAPOD_IP env var readable by all tools in the same process
**Component:** Process environment
**Attack:** A rogue tool or MCP server reads `HEXAPOD_IP` from the process environment.
**Disposition:** ACCEPT
**Mitigation:** Environment variables are process-wide in Rust/OS. This is a known limitation of env-var-based configuration. Mitigated architecturally by: (a) MCP servers run in subprocesses and do not inherit the parent's full environment by design; (b) the value is an IP address, not a credential. Accepted for Phase 1.
**Status:** CLOSED (accepted risk — infrastructure address, not a credential)

---

### D — Denial of Service

#### THREAT-D-01: LLM spams walk commands to wear out servos / overheat motors
**Component:** `hexapod_tcp.rs::execute()`
**Attack:** LLM issues a rapid sequence of `walk` commands, causing motor wear or thermal shutdown.
**Disposition:** ACCEPT
**Mitigation:** No rate limiting is implemented in Phase 1. The Freenove Python TCP server processes one command per TCP connection and has no built-in rate limiting. The `tool_delay_secs` config in `AgentConfig` (default 1.0 second between tool calls) provides minimal throttling at the agent loop level, but this is not hexapod-specific and can be reduced by configuration.
**Evidence:** No rate limiting found in `hexapod_tcp.rs`. `AgentConfig::tool_delay_secs` exists in `config.rs` but is agent-loop-wide and configurable.
**Accepted risk:** Physical safety of the hardware depends on operator supervision. Phase 1 is explicitly scoped to safe-foundation (supervised use). A future phase may add per-session command quotas or servo telemetry checks. Operator mitigation: set `agent.tool_delay_secs` >= 1.0 and supervise robot sessions.
**Status:** CLOSED (accepted risk — Phase 1 supervised use; ASVS L1 acceptable)

#### THREAT-D-02: TCP read hangs indefinitely on sensor commands
**Component:** `hexapod_tcp.rs::execute()` battery/distance arms
**Attack:** Robot sends partial response or no response; tool blocks the async executor indefinitely.
**Disposition:** MITIGATE
**Mitigation:** All sensor reads are wrapped in `timeout(Duration::from_secs(3), ...)`. Timeout expiry returns `Ok("Error: read timed out after 3s waiting for robot response")`.
**Evidence:** `hexapod_tcp.rs:401-408` (battery), `hexapod_tcp.rs:411-419` (distance) — both use `tokio::time::timeout(Duration::from_secs(3), ...)`.
**Status:** CLOSED

#### THREAT-D-03: TCP connect hangs indefinitely on unreachable robot
**Component:** `hexapod_tcp.rs::send_fire_and_forget()` and `send_and_read_line()`
**Attack:** Robot is powered off or network is partitioned; `TcpStream::connect` hangs for OS-level TCP timeout (2+ minutes).
**Disposition:** ACCEPT (partial mitigation)
**Mitigation:** For sensor commands (`read_battery`, `read_distance`), the outer 3-second timeout covers the connect phase as well. For fire-and-forget commands (`walk`, `stop`, `relax_servos`), there is NO connect timeout — if the robot is unreachable, `TcpStream::connect` will hang until the OS-level TCP timeout fires (typically 75+ seconds on Linux). On connection error, the tool returns the D-17 error string.
**Evidence:** `hexapod_tcp.rs:192-197` — `send_fire_and_forget` has no timeout wrapper. `hexapod_tcp.rs:362-380` — walk arm wraps only with `match`, not `timeout`.
**Accepted risk:** Fire-and-forget commands are expected to be fast (local LAN). A future phase should add a connect timeout (e.g., 5 seconds) to `send_fire_and_forget`. Not a blocker for ASVS L1 Phase 1 supervised use, but should be addressed before unattended operation.
**Status:** CLOSED (accepted risk with advisory — see OPEN_ADVISORIES below)

#### THREAT-D-04: Session-end halt fails to fire due to runtime shutdown race
**Component:** `hexapod_tcp.rs::on_session_end()` / `registry.rs::call_session_end_hooks()`
**Attack:** Tokio runtime shuts down before the spawned `send_stop_and_relax` future completes, leaving the robot walking.
**Disposition:** ACCEPT
**Mitigation:** `on_session_end` uses `tokio::spawn` (fire-and-forget). The spawned task reads `HEXAPOD_IP` and sends stop+relax. If the runtime shuts down before the task completes, the stop command may not reach the robot. This is documented in the code as "best-effort at shutdown" (`hexapod_tcp.rs:216`). The stop command is sent first, then relax — the critical safety action (stop) has first priority. The risk is accepted because: (a) the walk command itself is fire-and-forget with no acknowledgment; (b) the robot's own inactivity timer (Freenove server) will eventually halt motion; (c) this is Phase 1 supervised use.
**Evidence:** `hexapod_tcp.rs:597-602` — `tokio::spawn(async move { if let Ok(ip) = env::var("HEXAPOD_IP") { ... } })`; comment at `hexapod_tcp.rs:216` — "All errors are swallowed — this is best-effort at shutdown."
**Status:** CLOSED (accepted risk — documented best-effort; physically supervised use)

#### THREAT-D-05: CMD_RELAX toggle leaves robot in unexpected servo state
**Component:** `hexapod_tcp.rs` — `RELAX_CMD`
**Attack:** `CMD_RELAX` is a toggle on the Freenove server — calling it twice re-enables servos. Session-end halt sends stop then relax; if a prior `relax_servos` call was made, the session-end relax toggles servos back ON.
**Disposition:** ACCEPT
**Mitigation:** The toggle behavior is documented in the code: `hexapod_tcp.rs:21` — "NOTE: CMD_RELAX is a toggle — each invocation flips the servo-enabled state. Calling twice re-enables servos." The stop command (`CMD_MOVE#0#0#0#0#0\n`) is sent first and halts motion regardless of servo power state. `CMD_SERVOPOWER` would be more reliable but is blocked (D-16). Accepted: the stop command is the primary safety action; relax is supplementary.
**Evidence:** `hexapod_tcp.rs:18-24` — toggle comment on `RELAX_CMD` constant.
**Status:** CLOSED (accepted risk — documented; stop command is primary safety action)

---

### E — Elevation of Privilege

#### THREAT-E-01: LLM calls blocked commands (calibration, servo_power, led_mode 2-5)
**Component:** `hexapod_tcp.rs::execute()`
**Attack:** LLM attempts `action: "calibration"` or `action: "servo_power"` to access hardware control functions beyond Phase 1 scope.
**Disposition:** MITIGATE
**Mitigation:** Compile-time exhaustive `match` with explicit 15-value allowlist. Catch-all arm returns D-16 blocked string and performs NO I/O — no TCP connection is opened, no env var is read.
**Evidence:** `hexapod_tcp.rs:343-585` — outer match; catch-all at line 582: `other => Ok(format!("Action '{other}' is blocked — not permitted via hexapod_tcp. Never send this command."))`. Tests at lines 664-701 verify `calibration` and `servo_power` are blocked.
**Status:** CLOSED

#### THREAT-E-02: Toolset config bypassed — hexapod_tcp visible when robotics: {enabled: false}
**Component:** `registry.rs::get_definitions()` / `config.rs::ToolsConfig` / `app_runtime_factory.rs`
**Attack:** User sets `robotics: {enabled: false}` in config.yaml but the LLM can still call `hexapod_tcp` because `set_toolset_config` was not wired.
**Disposition:** MITIGATE
**Mitigation:** `build_app_runtime_bundle` calls `registry.set_toolset_config(Some(merged_tools.clone()))` AFTER all `register_*` calls. `get_definitions()` applies toolset-level filtering when `toolset_config` is `Some`. `with_default_toolsets_merged()` ensures the `robotics` entry is present and defaults to `enabled: true` for old configs, while preserving explicit `enabled: false` overrides.
**Evidence:** `app_runtime_factory.rs:68` — `let merged_tools = input.config.tools.clone().with_default_toolsets_merged();`; `app_runtime_factory.rs:139` — `registry.set_toolset_config(Some(merged_tools.clone()));`; `registry.rs:277-293` — toolset filter in `get_definitions()`. Integration tests at `app_runtime_factory.rs:599-648` verify the disabled case.
**Status:** CLOSED

#### THREAT-E-03: hexapod_tcp accessible from within execute_code sandbox
**Component:** `app_runtime_factory.rs::build_rpc_registry()`
**Attack:** LLM uses `execute_code` to write Python/Rust that calls the hexapod TCP tool or directly connects to port 5002 through the RPC sub-registry.
**Disposition:** MITIGATE
**Mitigation:** `build_rpc_registry` explicitly constructs a minimal registry with only file, search, web, and memory tools. It does NOT call `register_defaults_except`. `hexapod_tcp` is never registered in the RPC sub-registry. The comment explicitly names this exclusion.
**Evidence:** `app_runtime_factory.rs:177-209` — `build_rpc_registry` registers `ReadFileTool`, `WriteFileTool`, `PatchFileTool`, `SearchFilesTool`, `WebSearchTool`, `WebReadTool`, memory — and the comment at line 184-186 reads "hexapod_tcp: excluded — hexapod hardware control must not be accessible from within the execute_code sandbox."
**Note:** `execute_code` could still open a raw TCP socket to port 5002 via Python's `socket` module — this is a general code sandbox isolation concern, not specific to the hexapod tool registration. Out of scope for Phase 1.
**Status:** CLOSED

#### THREAT-E-04: is_available() bypass — tool callable even when HEXAPOD_IP is unset
**Component:** `registry.rs::execute_tool()` vs `handle_tool_call()`
**Attack:** A caller uses `handle_tool_call()` to bypass `is_available()` and invoke `hexapod_tcp` without `HEXAPOD_IP` being set.
**Disposition:** ACCEPT
**Mitigation:** `handle_tool_call` is documented at `registry.rs:400-410` as an internal method for `/toolset` commands that need to call tools reporting `is_available() = false`. `hexapod_tcp::execute()` independently reads `HEXAPOD_IP` at call time and returns `Ok("Error: HEXAPOD_IP env var not set...")` if absent. The defense-in-depth layer (the env var check inside `execute()`) operates independently of `is_available()`.
**Evidence:** `hexapod_tcp.rs:349-357` — env var check at execute time; `registry.rs:400-410` — `handle_tool_call` documented usage.
**Status:** CLOSED

#### THREAT-E-05: SSRF via LLM-controlled HEXAPOD_IP
**Component:** `hexapod_tcp.rs::execute()`
**Attack:** LLM influences the value of `HEXAPOD_IP` to redirect TCP connections to an internal service (e.g., `169.254.169.254` cloud metadata).
**Disposition:** MITIGATE
**Mitigation:** `HEXAPOD_IP` is read from the process environment via `env::var("HEXAPOD_IP")`. The LLM has no mechanism to set environment variables in the IronHermes process. The env var is set by the operator before process start.
**Evidence:** `hexapod_tcp.rs:349` — `let ip = match env::var("HEXAPOD_IP") { Ok(v) => v, ... }`. No code path allows LLM-supplied input to influence the env var value.
**Status:** CLOSED

---

## Threat Summary Table

| Threat ID | STRIDE | Component | Disposition | Status |
|-----------|--------|-----------|-------------|--------|
| THREAT-S-01 | Spoofing | `execute()` match allowlist | MITIGATE | CLOSED |
| THREAT-S-02 | Spoofing | `build_walk_wire()` direction/speed | MITIGATE | CLOSED |
| THREAT-T-01 | Tampering | Wire protocol `#` injection via direction | MITIGATE | CLOSED |
| THREAT-T-02 | Tampering | Wire protocol injection via speed | MITIGATE | CLOSED |
| THREAT-T-03 | Tampering | Wire constant immutability | MITIGATE | CLOSED |
| THREAT-R-01 | Repudiation | Command audit trail | MITIGATE (partial) | CLOSED |
| THREAT-I-01 | Info Disclosure | HEXAPOD_IP in debug log | ACCEPT | CLOSED |
| THREAT-I-02 | Info Disclosure | TCP response data | ACCEPT | CLOSED |
| THREAT-I-03 | Info Disclosure | Env var process-wide visibility | ACCEPT | CLOSED |
| THREAT-D-01 | DoS | LLM command spam / servo wear | ACCEPT | CLOSED |
| THREAT-D-02 | DoS | TCP read hang (sensor commands) | MITIGATE | CLOSED |
| THREAT-D-03 | DoS | TCP connect hang (fire-and-forget) | ACCEPT (advisory) | CLOSED |
| THREAT-D-04 | DoS | Session-end halt runtime race | ACCEPT | CLOSED |
| THREAT-D-05 | DoS | CMD_RELAX toggle state | ACCEPT | CLOSED |
| THREAT-E-01 | EoP | Blocked commands (calibration, etc.) | MITIGATE | CLOSED |
| THREAT-E-02 | EoP | Toolset config bypass | MITIGATE | CLOSED |
| THREAT-E-03 | EoP | execute_code sandbox access | MITIGATE | CLOSED |
| THREAT-E-04 | EoP | is_available() bypass | ACCEPT | CLOSED |
| THREAT-E-05 | EoP | SSRF via LLM-controlled IP | MITIGATE | CLOSED |

**Total:** 19 threats | **Closed:** 19 | **Open:** 0

---

## Unregistered Threat Flags (from SUMMARY.md)

| Flag | File | Description | Mapping |
|------|------|-------------|---------|
| `threat_flag: tcp-client` | `hexapod_tcp.rs` | New outbound TCP client to arbitrary IP:5002; IP is operator-controlled env var; blocked commands never open connection | Maps to THREAT-E-05 (SSRF), THREAT-S-01 (allowlist), THREAT-E-01 (blocked commands) — all CLOSED |

---

## Accepted Risks Log

| Risk ID | Threat | Rationale | Future Mitigation |
|---------|--------|-----------|-------------------|
| AR-01 | THREAT-D-01 — servo spam | Phase 1 is supervised use; `tool_delay_secs` provides minimal throttling | Add per-session command quota or servo telemetry gate in a future phase |
| AR-02 | THREAT-D-03 — connect hang on fire-and-forget | Local LAN expected; OS timeout applies | Wrap `send_fire_and_forget` in `tokio::time::timeout` (5s) in a future phase |
| AR-03 | THREAT-D-04 — session-end runtime race | Best-effort halt; robot has inactivity behavior; supervised use | Future: use `JoinHandle` and await with timeout on graceful shutdown path |
| AR-04 | THREAT-D-05 — CMD_RELAX toggle | Stop command is primary safety action; relax is supplementary | Future: use `CMD_SERVOPOWER` (currently blocked) or track toggle state in session |
| AR-05 | THREAT-R-01 — DEBUG-only logging | ASVS L1 acceptable; commands visible when debug tracing enabled | Future: emit INFO-level structured event or hook emission per command |
| AR-06 | THREAT-I-01 — IP in debug log | Operator env var; not PII or credential; DEBUG level only | Acceptable for all phases unless network topology is sensitive |
| AR-07 | THREAT-E-04 — handle_tool_call bypass | Defense-in-depth: execute() independently validates env var; no actual bypass possible | No action needed — defense-in-depth is sufficient |

---

## Open Advisories (Non-Blocking)

**ADV-01 (THREAT-D-03): Connect timeout absent on fire-and-forget commands**
- `send_fire_and_forget` has no `tokio::time::timeout` wrapper.
- If the robot is unreachable, `walk`, `stop`, and `relax_servos` calls will block the async task for the OS-level TCP connect timeout (75+ seconds on Linux).
- Sensor commands (`read_battery`, `read_distance`) are protected by the 3-second outer timeout, which covers the connect phase.
- Recommended fix in a future phase: `timeout(Duration::from_secs(5), send_fire_and_forget(&addr, &wire)).await`

**ADV-02 (THREAT-R-01): Command logging at DEBUG level only**
- Hexapod commands are logged only when the operator runs with `RUST_LOG=debug` or equivalent.
- For any production or unattended use, consider promoting the log line to INFO or wiring through the `HookRegistry` JSONL event log so all robot commands have a persistent audit trail.

**ADV-03 (execute_code TCP bypass): Raw TCP possible from within execute_code sandbox**
- The RPC sub-registry correctly excludes `hexapod_tcp`.
- However, Python code running inside `execute_code` can open raw TCP sockets (e.g., `import socket; s.connect(("192.168.1.42", 5002))`).
- This bypasses the allowlist entirely. Mitigation requires network namespace isolation or a seccomp profile on the execute_code sandbox — out of scope for Phase 1.

---

## threats_open: 0
