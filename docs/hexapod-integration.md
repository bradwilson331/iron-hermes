<!-- generated-by: gsd-doc-writer -->
# Hexapod Integration

## Overview

IronHermes integrates with the Freenove Big Hexapod Robot Kit for Raspberry Pi through two
purpose-built tools in the `ironhermes-tools` crate: `hexapod_tcp` and `hexapod_video`. Both
tools are live in production. The agent (running Gemma 4 or any compatible model) calls these
tools through the standard `ToolRegistry` → `AgentLoop` pipeline — no separate robot process,
gateway, or driver layer exists. The robot is a peer on the local network, reached by raw TCP
at a user-supplied IP address.

The integration is complete through Phase 27.1.4. All 15 `hexapod_tcp` actions and the
`hexapod_video` `capture_frame` action are implemented, tested, and documented in
`skills/hexapod/SKILL.md`.

---

## Existing Tool Surface

### `hexapod_tcp` — Robot Control

**File:** `crates/ironhermes-tools/src/hexapod_tcp.rs`
**Toolset:** `robotics`
**Prerequisite:** `HEXAPOD_IP` environment variable (robot's IP address)

Sends TCP commands to the Freenove server on port `5002`. Every call opens a fresh TCP
connection — there is no persistent socket. The tool is stateless; `HEXAPOD_IP` is read at
call time, not at session start.

| Action | Description |
|--------|-------------|
| `walk` | Move in a direction (`forward`, `backward`, `left`, `right`) at a given speed (2–10) |
| `stop` | Halt motion and return to neutral stance |
| `rotate` | Spin in place by a signed degree count; positive = clockwise |
| `head_pan` | Pan the robot's head servo (−90 to +90°) |
| `head_tilt` | Tilt the robot's head servo (−90 to +90°) |
| `camera_pan` | Pan the camera gimbal (x: 50–180) |
| `camera_tilt` | Tilt the camera gimbal (y: 0–180) |
| `read_battery` | Query battery voltages; returns `"Battery: v1V / v2V (OK|LOW)"` |
| `read_distance` | Single ultrasonic distance reading |
| `stream_distance` | Poll distance N times (1–20, max 4 s) with min/max/avg summary |
| `relax_servos` | Toggle servo-enabled state (safe power-save; use instead of `servo_power`) |
| `buzzer_on` | Activate onboard buzzer |
| `buzzer_off` | Deactivate onboard buzzer |
| `led` | Set all 8 LEDs to a solid RGB color (0–255 per channel) |
| `led_off` | Turn LEDs off via the server's dedicated mode-0 path |

**Blocked actions** (return an error string without any network attempt):
- `calibration` — moves servos to uncalibrated positions; risk of hardware damage
- `servo_power` — cuts servo power mid-stance; use `relax_servos` instead
- `chase`, `blink`, `breathing`, `rainbow` (LED animation modes) — async interference with solid-color control

**Session-end safety halt:** `HexapodTcpTool::on_session_end()` fires when the IronHermes
session closes. It spawns an async task that sends `CMD_MOVE#0#0#0#0#0\n` (stop) then
`CMD_RELAX\n` (relax servos) in best-effort fashion. The robot halts automatically even if the
agent did not call `stop` explicitly.

### `hexapod_video` — Camera Frame Capture

**File:** `crates/ironhermes-tools/src/hexapod_video.rs`
**Toolset:** `robotics`
**Prerequisite:** `HEXAPOD_IP` environment variable (shared with `hexapod_tcp`)

Connects to the Freenove video server on port `8002`, reads one JPEG frame using the
4-byte little-endian length-prefix protocol, and returns a `data:image/jpeg;base64,<data>`
data URI. The model can include this URI directly in the next multimodal call for visual
reasoning about the robot's surroundings.

| Action | Parameters | Returns |
|--------|-----------|---------|
| `capture_frame` | none | `data:image/jpeg;base64,<jpeg>` |

Port 8002 is single-client. If the desktop PyQt5 client (`ui_client.py`) is connected, the
tool returns `"Error: video port 8002 is busy — another client is connected."` Disconnect the
desktop client before calling `capture_frame`.

**Typical usage pattern:**

```
hexapod_tcp { action: "camera_pan",  x: 115 }  → "OK"
hexapod_tcp { action: "camera_tilt", y: 90  }  → "OK"
hexapod_video { action: "capture_frame" }       → "data:image/jpeg;base64,/9j/..."
```

---

## How Tools Reach the Agent Loop

Both hexapod tools follow the standard IronHermes tool pipeline. No special code paths exist
for robotics.

```
HEXAPOD_IP (env) ──────────────────────────────────────────┐
                                                            │
User / Telegram / Web UI                                    │
        │                                                   │
        ▼                                                   │
   AgentLoop (ironhermes-agent)                             │
        │  sends conversation + tool definitions            │
        ▼                                                   │
   LLM (Gemma 4 / Anthropic / OpenAI-compatible)           │
        │  returns tool_call { name, args }                 │
        ▼                                                   │
   ToolRegistry::execute_tool (ironhermes-tools)            │
        │  is_available() check ──────────────────────── reads HEXAPOD_IP
        │  toolset "robotics" enabled check                 │
        ▼                                                   │
   HexapodTcpTool::execute()  OR  HexapodVideoTool::execute()
        │  fresh TcpStream to HEXAPOD_IP:5002 or :8002      │
        ▼                                                   │
   Freenove robot (Raspberry Pi server.py)                  │
        │  raw TCP response                                 │
        ▼                                                   │
   ToolResult string → back to AgentLoop → next LLM turn
```

### Toolset and Availability Gating

The `robotics` toolset is in `DEFAULT_TOOLSETS` (`crates/ironhermes-core/src/constants.rs`),
so it is enabled on every fresh configuration without requiring an explicit opt-in. The actual
gate is `is_available()`, which walks the tool's `prerequisites()` list:

- If `HEXAPOD_IP` is **not set**: both tools report `is_available() = false` and are excluded
  from `ToolRegistry::get_definitions()`. The LLM never sees them, and no TCP is attempted.
- If `HEXAPOD_IP` **is set**: both tools are included in the tool definitions sent to the LLM
  and become callable.

This means starting IronHermes with `HEXAPOD_IP=192.168.1.42` is the only configuration
change needed to activate the full hexapod tool surface. No config file edits are required.

### Session Lifecycle

`ToolRegistry::call_session_end_hooks()` iterates all registered tools and calls `on_session_end()`.
`HexapodTcpTool` overrides this hook to fire the stop-and-relax sequence. `HexapodVideoTool`
uses the default no-op implementation (no shutdown behavior needed for the video port).

---

## Configuration

| Variable | Required | Description |
|----------|----------|-------------|
| `HEXAPOD_IP` | Yes (to activate) | IP address of the robot on the local network (e.g., `192.168.1.42`). Absent = tools hidden from LLM. |

No other configuration is needed. Port numbers are hardcoded to match the Freenove server:
`5002` for commands, `8002` for video.

The `robotics` toolset can be explicitly disabled in the IronHermes config file if needed:

```toml
[tools.toolsets.robotics]
enabled = false
```

---

## Wire Protocol Reference

The Freenove server (`Code/Server/server.py` on the robot's Raspberry Pi) speaks a simple
text protocol over raw TCP.

**Command port 5002 — `hexapod_tcp`:**
- Wire format: UTF-8 text, `#`-separated fields, `\n`-terminated
- Example: `CMD_MOVE#1#0#25#5#0\n` (walk forward at speed 5)
- Fire-and-forget commands send and close; no response is read
- Sensor commands (`CMD_POWER`, `CMD_SONIC`) send and then read one `\n`-terminated response line within a 3-second timeout

**Video port 8002 — `hexapod_video`:**
- Wire format: 4-byte little-endian `u32` frame length, then that many JPEG bytes
- One frame per connection; the tool connects, reads one frame, and disconnects
- 5-second read timeout; maximum frame size 2 MB (guard against malformed length prefix)

All clamping is silent. Out-of-range parameter values are adjusted to their nearest valid bound
before the wire command is built — they are never rejected with an error.

---

## Skill Documentation

The authoritative protocol reference for agent use is `skills/hexapod/SKILL.md`. It documents:
- All 15 `hexapod_tcp` actions with wire format, parameter ranges, and clamping rules
- `hexapod_video` connection details and usage pattern
- Camera gimbal joint-axis behavior (why `camera_pan` sends `y=90` and `camera_tilt` sends `x=115`)
- LED off vs. LED color-zero distinction (`CMD_LED#0\n` vs. `CMD_LED#0#0#0\n`)
- Rotate timing constant (`ROTATE_MS_PER_DEGREE = 20`) and calibration note
- Session-end safety halt behavior

The skill is loaded by the agent when the `hexapod` skill is active, giving the LLM the full
protocol reference inline in its context window.

---

## Adding a New Hexapod Action

To add a new robot action to `hexapod_tcp`:

1. **Add the wire constant** at the top of `hexapod_tcp.rs` alongside existing constants.
2. **Extend the action enum** in the JSON schema (`schema()` method) — add the new action name
   to the `"enum"` array.
3. **Add the action to the outer allowlist match arm** (`"walk" | "stop" | ... | "new_action"`).
   The compiler enforces exhaustiveness on the inner match.
4. **Implement the inner match arm** following the fire-and-forget or send-and-read pattern.
5. **Update `DESCRIPTION`** — the tool description string is what the LLM reads. Keep it
   accurate.
6. **Add a unit test** — all existing tests run without a live robot. Pure-function helpers
   (`build_walk_wire`, `parse_battery_response`, etc.) are the preferred test surface.
7. **Update `skills/hexapod/SKILL.md`** — add a row to the actions table and document any
   parameter ranges or clamping behavior.

For a new tool type (e.g., a second robot or a different hardware interface):

1. Create `crates/ironhermes-tools/src/<new_tool>.rs` implementing the `Tool` trait.
2. Declare `pub mod <new_tool>;` in `crates/ironhermes-tools/src/lib.rs`.
3. Register with `register_unless_skipped!(Box::new(<NewTool>), "<tool_name>");` in
   `registry.rs`'s `register_defaults()`.
4. Set `fn toolset(&self) -> &str` to `"robotics"` (or a new toolset name added to
   `ALL_TOOLSETS` in `constants.rs`).
5. Declare any `Prerequisite` entries (env vars, config fields) in `fn prerequisites()`.
   `is_available()` hides the tool automatically when required prerequisites are absent.

---

## Planned Work (Phase 27.x Roadmap)

The following capabilities are deferred to future phases:

- **Background video streaming** (`start_stream` / `stop_stream`) — continuous frames to a
  ring buffer; not needed for single-frame navigation decisions
- **`CMD_LED_MOD` animated modes** (chase, blink, breathing, rainbow) — blocked pending an
  async-safe LED control design
- **DEFCON-tiered security for symlink bypass in `skills.rs`** (CR-01) — deferred to a future
  DEFCON phase, unrelated to hexapod
- **Video stream + vision at scale** — Phase 27.1.4 delivers single-frame capture; multi-frame
  or real-time streaming is a future milestone
