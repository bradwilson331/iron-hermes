---
name: hexapod
description: Protocol reference and action guide for the Freenove hexapod robot — wire formats, parameter ranges, blocked commands, and calibration constants for all 12 hexapod_tcp actions.
version: 1.0.0
metadata:
  hermes:
    requires_toolsets: [robotics]
    tags: [robotics, hexapod, freenove, tcp]
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
| led_off | — | `CMD_LED#0\n` | `"OK"` | mode-0 server off path — NOT `CMD_LED#0#0#0\n` (see note) |

### Walk Direction Breakdown

| direction | x | y | resulting wire (speed=5) |
|-----------|---|---|--------------------------|
| forward | 0 | +25 | `CMD_MOVE#1#0#25#5#0\n` |
| backward | 0 | -25 | `CMD_MOVE#1#0#-25#5#0\n` |
| left | -25 | 0 | `CMD_MOVE#1#-25#0#5#0\n` |
| right | +25 | 0 | `CMD_MOVE#1#25#0#5#0\n` |

### Rotate Sequence

rotate sends `CMD_MOVE#1#0#0#5#10\n` (clockwise) or `CMD_MOVE#1#0#0#5#-10\n` (counterclockwise), sleeps for `abs(degrees) * ROTATE_MS_PER_DEGREE` milliseconds, then sends `CMD_MOVE#0#0#0#0#0\n` to stop. The stop command is best-effort — if it fails the tool returns a warning string but does not fail.

### LED Off vs LED Color-Zero

`led_off` sends `CMD_LED#0\n` which sets the server's internal `led_mode` to 0, triggering `color_wipe([0,0,0])` — the dedicated off path. Do NOT call `led` with `r=0, g=0, b=0` as a substitute: `CMD_LED#0#0#0\n` sets the color to black without activating the server's off path.

## Parameter Ranges and Clamping

All clamping is silent — out-of-range values are adjusted automatically, not rejected.

| Parameter | Action | Range | Clamping |
|-----------|--------|-------|----------|
| speed | walk | 2–10 | clamped to [2, 10] |
| degrees | rotate | negative allowed; ±3600 max | capped at ±3600 to prevent runaway rotation |
| angle | head_pan, head_tilt | ±90° | clamped to [-90, 90]; HEAD_PAN_MAX = HEAD_TILT_MAX = 90 |
| r, g, b | led | 0–255 per channel | each channel clamped independently to [0, 255] |

**Calibration constant:** `ROTATE_MS_PER_DEGREE = 20` — tune this on the real robot after live testing if rotation distance is inaccurate.

## Blocked Commands

The following are explicitly blocked by `hexapod_tcp`. Attempting to call them returns `"Action '...' is blocked — not permitted via hexapod_tcp. Never send this command."` without any network connection.

- **CMD_CALIBRATION** (`calibration` action): moves servos to uncalibrated positions; risk of hardware damage
- **CMD_SERVOPOWER** (`servo_power` action): cuts servo power mid-stance causing the robot to drop; use `relax_servos` instead — it is the safe alternative
- **CMD_LED_MOD modes 2–5** (`chase`, `blink`, `breathing`, `rainbow`): asynchronous LED animations that may interfere with solid-color LED control; blocked pending async-safe design

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
