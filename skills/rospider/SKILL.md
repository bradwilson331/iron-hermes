---
name: rospider
description: Usage guide for the Hiwonder ROSPider hexapod — every rospider verb and ros2 action, the safety semantics that cannot be relaxed, and what a refusal answer means.
version: 1.0.0
metadata:
  hermes:
    requires_toolsets: [ros2]
    tags: [robotics, ros2, rospider, hexapod, rosbridge]
---

# ROSPider — Agent Usage Guide

## Overview

Two tools ride the same persistent rosbridge session:

- **`rospider`** — high-level, typed verbs for the Hiwonder ROSPider hexapod. Prefer this tool
  for anything the ROSPider driver exposes. No verb ever accepts a raw message body — every
  parameter is typed and clamped before anything is sent.
- **`ros2`** — the generic tool for anything else: any other robot, any other topic/service/
  action, or introspection (`list_topics`, `message_details`, and friends).

Both talk to the robot over one rosbridge WebSocket connection per process (`ros2.bridge_url`
or `IRONHERMES_ROS2_BRIDGE_URL`). If neither is configured, both tools are hidden.

## Safety you can rely on

- **Writes are allowlisted.** `ros2` only publishes/calls a service/sends a goal for a name the
  operator declared under `ros2.publish` / `ros2.services` / `ros2.actions` in config.yaml,
  together with its exact type. `rospider`'s own verbs target their own `ros2.rospider.*`
  config keys, which never widen that allowlist.
- **Every physical parameter is clamped, never rejected** — velocity, duration, pose angles,
  body height, servo pulses. The result always names every clamp it applied
  (`Clamped: linear.x: 0.5 -> 0.1`).
- **Motion is always timed and auto-stops.** Any walk/turn runs for at most 5 seconds and then
  the tool itself sends the zero-velocity stop followed by the gait-0 rest stop that brings the
  legs to rest — you never need to remember to stop the robot.
- **`stop` halts everything immediately** — every motion topic either tool has driven, sends the
  gait-0 rest stop (even when nothing was driven this session — the robot may still be marching
  from an earlier process), and cancels every unfinished action goal.
- **The session end always stops the robot**, on every surface (CLI, gateway, web chat,
  voice2voice, cron/kanban) — closing a chat, a Telegram `/new`, a cron job finishing, or a tab
  closing all trigger the same stop (zero Twist, then the gait-0 rest stop for any ROSPider motion
  topic driven).
- **On the ROSPider, a zero Twist alone does not stop the robot** — it leaves the legs stepping
  in place. Never try to stop it by publishing a zero Twist through `ros2`; use `rospider` `stop`.
- **`--yolo` / `autonomous.yolo` relaxes NONE of the above.** The allowlist, every cap, and the
  auto-stop are physical safety, not a budget guard — never try to work around a clamp or a
  refusal by retrying, rephrasing, or asking for yolo mode.

## rospider verbs

| Verb | Parameters | Ranges | Returns |
|---|---|---|---|
| `walk` | `direction` (forward\|backward\|left\|right, default forward), `speed` (m/s, default 0.05), `duration_secs` | speed further clamped by `ros2.caps.max_linear_velocity`; duration clamped by `ros2.caps.max_motion_duration_secs` (hard ceiling 5s) | Motion summary + clamps; always ends with the zero-Twist stop and the gait-0 rest stop |
| `turn` | `direction` (left\|right, default left), `rate` (rad/s, default 0.3), `duration_secs` | rate clamped by `ros2.caps.max_angular_velocity`; duration as above | Motion summary + clamps; always ends with the zero-Twist stop and the gait-0 rest stop |
| `stop` | — | — | Zeroes every motion topic, sends the gait-0 rest stop (even when nothing was driven), and cancels every unfinished goal |
| `set_pose` | `roll`, `pitch` (rad), `yaw` (rad), `x`, `y` (mm offset), `height` (mm) | roll/pitch ±0.2, yaw ±0.4, x/y ±20mm, height 100–160mm (default 120) | Publish confirmation + clamps |
| `body_height` | `height` (mm) | 100–160mm, default 120 | Publish confirmation + clamps |
| `gait` | `gait` (ripple\|tripod, default tripod), `step_height` (mm), `period` (s), `pose` | step_height 10–40mm (default 20), period 0.5–2.0s (default 1.0); an unknown `pose` name is refused before any I/O | Publish confirmation + clamps |
| `builtin_pose` | `pose`, `duration` (s) | `pose` one of `DEFAULT_POSE`, `DEFAULT_POSE_M`, `NARROW_POSE`, `SLAM_POSE`, `HOLE_POSE`, `SHRINK_POSE`, `SIDE_SHIFT_POSE`; duration clamped 0.5–3.0s (default 1.0) | Service result/message |
| `camera_pan` | `degrees`, `duration` | degrees clamped ±72° (0 = centre); refuses servo id 0 or any leg servo (ids 1–18) before touching config or the network; positive `degrees` pans the camera left (reference robot; negative = right) | Publish confirmation, or a refusal |
| `read_imu` | — | one bounded sample | Roll/pitch/yaw one-liner |
| `read_battery` | — | one bounded sample | Voltage one-liner |
| `read_odom` | — | one bounded sample | Position/orientation one-liner. A failed read (no message on the odom topic) or `x`/`y` stuck at 0 after walking means the robot-side odometry source is not running — say so, point to `docs/robots/rospider.md#odometry-needs-a-robot-side-source`, and never report IMU readings as a position |
| `lidar_summary` | — | one bounded sample | Nearest distance + per-sector summary, never raw ranges |
| `camera_snapshot` | `prompt` (what to look for) | compressed topic first, falls back to raw on timeout | A vision-model description of the frame — the frame itself is never returned |

## ros2 actions (generic tool)

| Action | Purpose |
|---|---|
| `list_topics` | List every topic and type the robot's rosapi reports |
| `subscribe` | Bounded sample: subscribe, collect up to `max_messages` or until `timeout_secs`, always unsubscribe |
| `list_services` / `list_nodes` / `list_actions` | rosapi introspection — no parameters |
| `topic_type` / `service_type` / `message_details` | rosapi type lookups |
| `publish` | Publish one message to a declared topic; add `duration_secs` for a motion topic |
| `call_service` | Call a declared ROS 2 service |
| `send_goal` | Send a declared ROS 2 action goal (e.g. Nav2 `NavigateToPose`) |
| `goal_status` | Poll a goal's status/feedback/result, with `wait_secs` up to 20 |
| `cancel_goal` | Cancel a goal and wait for it to report `CANCELED` |
| `stop` | Zero every motion topic, replay the gait-0 rest stop for any ROSPider motion topic driven, and cancel every unfinished goal |

## Recipes

- **Cover distance:** `rospider` motion is capped at ~5s per call. To go further, chain several
  `walk` calls back to back, and check `lidar_summary` between each one so you never walk the
  robot into something you can't see.
- **Look around:** point the camera with `camera_pan`, then call `camera_snapshot` to get a
  description of what's in view. Repeat at a few angles to build a picture of the surroundings.
- **Before a long task:** check `read_battery` first — do not start an extended motion sequence
  on a low battery.
- **Nav2 goals:** use `send_goal` (via the generic `ros2` tool) for `NavigateToPose`-style
  navigation, poll with `goal_status wait_secs`, and use `cancel_goal` or `rospider stop` to
  abort if something looks wrong.

## What answers mean

- **`... is not permitted ...`** — the operator has not declared that topic/service/action name
  (and matching type) in `ros2:` config. This is final for this session — do not retry the same
  call. Tell the operator what name/type you need declared.
- **`Clamped: ...`** — a parameter you asked for was outside the safe range and was adjusted,
  not rejected. The motion or write still happened, at the clamped value.
- **`Error: no message on ... within ...s`** — a read (subscribe, sensor read, camera snapshot)
  got no data in its window. The topic name may be wrong, or nothing is publishing yet — check
  with `list_topics` before retrying.
- **Action ops unsupported** — the robot's rosbridge is too old (rosbridge_suite < 2.0.0) to
  support `send_goal`/`cancel_goal`. This is an operator-side upgrade, not something you can
  work around.
