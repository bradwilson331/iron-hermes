---
name: hexapod
description: Protocol reference and action guide for the Freenove hexapod robot — wire formats, parameter ranges, blocked commands, and calibration constants for all 15 hexapod_tcp actions. Companion hexapod_video tool provides single-frame JPEG capture via video port 8002.
version: 1.0.0
metadata:
  hermes:
    requires_toolsets: [robotics]
    tags: [robotics, hexapod, freenove, tcp, video]
---

# Hexapod Robot — Protocol Reference

## Overview

The `hexapod_tcp` tool sends TCP commands to a Freenove hexapod robot at `HEXAPOD_IP:5002`. One fresh TCP connection is opened per call — there is no persistent socket. Fire-and-forget actions return `"OK"` when the command was delivered. Sensor actions (read_battery, read_distance) return a formatted string with the reading. All errors surface as `Ok("Error: ...")` strings visible inline in the tool result — they never raise exceptions.

## Connection

| Setting | Value |
|---------|-------|
| Env var | `HEXAPOD_IP` (required — robot's IP address) |
| Port | `5002` |
| Protocol | Raw TCP, newline-terminated commands (`\n`) |
| Pattern | One connection per call, no persistent socket |

`HEXAPOD_IP` is read at call time, not at session start. If it is not set, every action returns `"Error: HEXAPOD_IP env var not set — cannot connect to robot"` without attempting any network connection.

## Actions and Wire Format

| Action | Parameters | Wire Command | Returns | Notes |
|--------|------------|-------------|---------|-------|
| walk | direction, speed | `CMD_MOVE#1#{x}#{y}#{speed}#0\n` | `"OK"` | direction sets x/y; see direction table below |
| stop | — | `CMD_MOVE#0#0#0#0#0\n` | `"OK"` | halts motion, returns to neutral stance |
| read_battery | — | `CMD_POWER\n` | `"Battery: v1V / v2V (OK\|LOW)"` | response: `CMD_POWER#v1#v2\n`; LOW if v1 < 5.5V or v2 < 6.0V |
| read_distance | — | `CMD_SONIC\n` | `"Distance: dist cm"` | response: `CMD_SONIC#dist\n` |
| relax_servos | — | `CMD_RELAX\n` | `"OK"` | toggle — two calls re-enable servos; use instead of CMD_SERVOPOWER |
| rotate | degrees | `CMD_MOVE#1#0#0#5#±10\n` | `"OK"` | sleeps `abs(degrees)*20ms` then sends stop; positive=clockwise, negative=counterclockwise |
| head_pan | angle | `CMD_HEAD#0#{angle}\n` | `"OK"` | angle clamped to ±90° |
| head_tilt | angle | `CMD_HEAD#1#{angle}\n` | `"OK"` | angle clamped to ±90° |
| buzzer_on | — | `CMD_BUZZER#1\n` | `"OK"` | — |
| buzzer_off | — | `CMD_BUZZER#0\n` | `"OK"` | — |
| led | r, g, b | `CMD_LED#{r}#{g}#{b}\n` | `"OK"` | sets all 8 LEDs to solid color; each channel clamped to 0–255 |
| led_off | — | `CMD_LED#0#0#0\n` | `"OK"` | color channel, all zeros — fast ledIndex path; see note |
| stream_distance | samples | `CMD_SONIC\n` × N with 200ms sleep | `"Distances: [d1, d2, ...] cm \| min=M max=X avg=A.B"` | samples clamped to [1, 20]; 20 × 200ms = 4s max |
| camera_pan | x | `CMD_CAMERA#{x}#90\n` | `"OK"` | x clamped to 50–180; y defaults to midpoint 90 (CMD_CAMERA sets both axes) |
| camera_tilt | y | `CMD_CAMERA#115#{y}\n` | `"OK"` | y clamped to 0–180; x defaults to midpoint 115 (CMD_CAMERA sets both axes) |

### Walk Direction Breakdown

| direction | x | y | resulting wire (speed=5) |
|-----------|---|---|--------------------------|
| forward | 0 | +25 | `CMD_MOVE#1#0#25#5#0\n` |
| backward | 0 | -25 | `CMD_MOVE#1#0#-25#5#0\n` |
| left | -25 | 0 | `CMD_MOVE#1#-25#0#5#0\n` |
| right | +25 | 0 | `CMD_MOVE#1#25#0#5#0\n` |

### Rotate Sequence

rotate sends `CMD_MOVE#1#0#0#5#10\n` (clockwise) or `CMD_MOVE#1#0#0#5#-10\n` (counterclockwise), sleeps for `abs(degrees) * ROTATE_MS_PER_DEGREE` milliseconds, then sends `CMD_MOVE#0#0#0#0#0\n` to stop. The stop command is best-effort — if it fails the tool returns a warning string but does not fail.

### LED Off — Correct Command and Why

`led_off` sends `CMD_LED#0#0#0\n` (color channel, all three fields zero). This routes through the server's `ledIndex` path, which writes all 8 pixels in microseconds and cannot be interrupted.

**Do NOT use `CMD_LED_MOD#0\n`.** That command triggers a 350 ms pixel-by-pixel `colorWipe` in a background thread. The server uses `ctypes`-based `stop_thread` (async `SystemExit` raise) to cancel the previous LED thread when a new command arrives. Raising mid-`colorWipe` leaves the strip in a partially-written state — LEDs do not turn off. Confirmed on physical hardware: `CMD_LED_MOD#0` returns `"OK"` (TCP write succeeded) but LEDs remain lit.

`led {r=0, g=0, b=0}` and `led_off` produce identical wire output (`CMD_LED#0#0#0\n`) and are interchangeable.

### Camera Gimbal Joint-Axis Behavior

`CMD_CAMERA#x#y\n` always sets BOTH the camera's pan and tilt servos in a single command. Because `camera_pan` and `camera_tilt` are exposed as separate single-axis actions, the unused axis is sent at its protocol midpoint: `camera_pan` sends `y=90` (midpoint of 0–180), and `camera_tilt` sends `x=115` (midpoint of 50–180). These midpoints produce a neutral camera position and are within the server's enforced ranges.

The camera gimbal is independent of the head servos: `CMD_HEAD` controls the robot's head, `CMD_CAMERA` controls the camera mount. Both can be positioned independently.

## Parameter Ranges and Clamping

All clamping is silent — out-of-range values are adjusted automatically, not rejected.

| Parameter | Action | Range | Clamping |
|-----------|--------|-------|----------|
| speed | walk | 2–10 | clamped to [2, 10] |
| degrees | rotate | negative allowed; ±3600 max | capped at ±3600 to prevent runaway rotation |
| angle | head_pan, head_tilt | ±90° | clamped to [-90, 90]; HEAD_PAN_MAX = HEAD_TILT_MAX = 90 |
| r, g, b | led | 0–255 per channel | each channel clamped independently to [0, 255] |
| samples | stream_distance | 1–20 | clamped to [1, 20]; 20 × 200ms = 4s max |
| x | camera_pan | 50–180 | clamped to [50, 180]; CAMERA_PAN_MIN / CAMERA_PAN_MAX constants |
| y | camera_tilt | 0–180 | clamped to [0, 180]; CAMERA_TILT_MIN / CAMERA_TILT_MAX constants |

**Calibration constant:** `ROTATE_MS_PER_DEGREE = 20` — tune this on the real robot after live testing if rotation distance is inaccurate.

## Blocked Commands

The following are explicitly blocked by `hexapod_tcp`. Attempting to call them returns `"Action '...' is blocked — not permitted via hexapod_tcp. Never send this command."` without any network connection.

- **CMD_CALIBRATION** (`calibration` action): moves servos to uncalibrated positions; risk of hardware damage
- **CMD_SERVOPOWER** (`servo_power` action): cuts servo power mid-stance causing the robot to drop; use `relax_servos` instead — it is the safe alternative
- **CMD_LED_MOD (all modes, including #0)**: all `CMD_LED_MOD` commands are blocked. Modes 2–5 (`chase`, `blink`, `breathing`, `rainbow`) are async animations that interfere with solid-color control. Mode 0 (`CMD_LED_MOD#0`) appears to be the off path but triggers a 350 ms `colorWipe` that is reliably corrupted by `stop_thread` — LEDs stay lit. Use `led_off` (`CMD_LED#0#0#0\n`) instead.

Never attempt to work around these blocks by constructing raw TCP commands manually.

## Error Strings

All errors are returned as `Ok("Error: ...")` — visible inline in the tool result, never raised as exceptions.

| Condition | Error string |
|-----------|-------------|
| `HEXAPOD_IP` not set | `"Error: HEXAPOD_IP env var not set — cannot connect to robot"` |
| Connection failed | `"Error: cannot connect to robot at {ip}:5002 — is HEXAPOD_IP set and the robot powered on?"` |
| Read timeout (sensor commands) | `"Error: read timed out after 3s waiting for robot response"` |
| Unexpected sensor response | `"Error: unexpected battery response from robot: ..."` |

## Session End Behavior

When the IronHermes session ends, `hexapod_tcp` automatically sends `stop` then `relax_servos` in a best-effort shutdown (errors silently swallowed). The robot will halt and relax even if the agent did not explicitly call `stop`. This means you do not need to add a stop call at the end of every session — it happens automatically.

---

## hexapod_video Tool

The `hexapod_video` tool captures a single JPEG frame from the robot's camera and returns it as a base64 data URI for multimodal vision analysis. Use it to give the agent visual awareness of the robot's surroundings — point the camera with `camera_pan`/`camera_tilt` first, then call `capture_frame`.

### Connection

| Setting | Value |
|---------|-------|
| Env var | `HEXAPOD_IP` (shared with hexapod_tcp) |
| Port | `8002` |
| Protocol | 4-byte little-endian frame length + JPEG bytes |
| Pattern | One connection per call, no persistent socket |

### Actions

| Action | Parameters | Returns | Notes |
|--------|------------|---------|-------|
| capture_frame | — | `"data:image/jpeg;base64,<data>"` | Connects to port 8002, reads one JPEG frame, disconnects |

### Error Strings

| Condition | Error string |
|-----------|-------------|
| `HEXAPOD_IP` not set | `"Error: HEXAPOD_IP env var not set — cannot connect to robot"` |
| Port 8002 busy (another client connected) | `"Error: video port 8002 is busy — another client is connected. Disconnect the other client and retry."` |
| Read timeout / camera not streaming | `"Error: read timed out after 5s waiting for video frame — camera may not be streaming"` |
| Frame read error | `"Error: cannot read video frame from robot at {addr} — camera may not be running"` |

### Usage Pattern

Point the camera before capturing:

```
camera_pan(x=115)    → "OK"
camera_tilt(y=90)    → "OK"
capture_frame()      → "data:image/jpeg;base64,/9j/..."
```

Pass the data URI directly as an image in the next multimodal call. The desktop PyQt5 client (`ui_client.py`) occupies port 8002 when running — disconnect it before calling `capture_frame`.
