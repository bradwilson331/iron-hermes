<!-- generated-by: gsd-doc-writer -->
# ROS 2 Integration — Operator & Developer Reference

Phase 49.8. IronHermes talks to a ROS 2 (Humble) robot through two tools — the generic `ros2`
tool and the high-level `rospider` tool for the Hiwonder ROSPider — both riding one persistent
**rosbridge_suite** WebSocket connection per process. There is no ROS client library, DDS, or
Zenoh dependency anywhere in the ironhermes build (D-01).

For the ROSPider-specific setup walkthrough (install, version check, launch, network exposure,
name reconciliation), see [`docs/robots/rospider.md`](robots/rospider.md). For the config
schema, see [`docs/CONFIGURATION.md`](CONFIGURATION.md#ros-2-ros2). This document is the
operator/developer reference for the tools themselves — what they do, how they're safe, and
their known limits.

## Architecture

IronHermes speaks the **rosbridge v2 JSON protocol** over a single, persistent WebSocket per
process (`ws://<robot>:9090` by default) — every op (`advertise`, `publish`, `subscribe`,
`call_service`, `send_action_goal`, `cancel_action_goal`, `unsubscribe`) is a JSON frame on that
one connection. The connection is persistent, not fresh-per-call, because timed motion, bounded
subscriptions, and action-goal polling/cancel all need the same socket (or the same goal handle)
to stay valid across multiple tool calls within a session — a fresh-per-call connection would
lose in-flight goal state between a `send_goal` and its later `goal_status`/`cancel_goal`.

Two supported topologies, switched with a URL change and nothing else (D-02):

- Operator machine on the LAN, robot separate: `ws://<robot-ip>:9090`.
- ironhermes running on the robot computer itself: `ws://localhost:9090`.

```
ironhermes agent process                          Robot (ROS 2 Humble)
┌──────────────────────────┐   rosbridge JSON/WS   ┌─────────────────────────┐
│ ros2 tool (generic)       │ ───────────────────▶ │ rosbridge_server (:9090) │
│ rospider tool (high-level)│ ◀─────────────────── │   + rosapi               │
│   both share one          │                       │        │                 │
│   Ros2BridgeClient        │                       │        ▼                 │
│   connection (this proc)  │                       │  ROS 2 graph             │
└──────────────────────────┘                       └─────────────────────────┘
```

## Tool reference

### `ros2` (generic)

| Action | Parameters | Notes |
|---|---|---|
| `list_topics` | — | Every topic and type the robot's rosapi reports |
| `subscribe` | `topic`, `type` (optional, resolved via rosapi), `max_messages`, `timeout_secs`, `prompt` | Bounded sample (D-12): subscribes, collects up to `max_messages` or until `timeout_secs`, then ALWAYS unsubscribes — never a background subscription |
| `list_services` / `list_nodes` / `list_actions` | — | rosapi introspection, no parameters |
| `topic_type` | `topic` | rosapi type lookup |
| `service_type` | `service` | rosapi type lookup |
| `message_details` | `msg_type_name` | rosapi message/service/action field layout |
| `publish` | `topic`, `type`, `msg`, `duration_secs` (motion topics) | Allowlisted (D-04) — see Safety model |
| `call_service` | `service`, `type`, `args`, `timeout_secs` (clamped 0.5–10) | Allowlisted (D-04) |
| `send_goal` | `action_name`, `type`, `goal` | Allowlisted (D-04); returns a goal id |
| `goal_status` | `goal_id`, `wait_secs` (clamped 0–20) | Polls status/feedback/result |
| `cancel_goal` | `goal_id` | Cancels and waits for a terminal status |
| `stop` | — | Zeroes every motion topic this session drove and cancels every unfinished goal; also replays the ROSPider gait-0 rest stop for any ROSPider motion topic driven, and runs automatically at session end |

Reads (`list_topics`, `subscribe`, every introspection action) may target **any name** — no
declaration required (D-04). Writes (`publish`, `call_service`, `send_goal`) are permitted
**only** for names the operator declared under `ros2.publish` / `ros2.services` / `ros2.actions`
in config, together with the exact type.

### `rospider` (Hiwonder ROSPider)

| Action | Parameters | Notes |
|---|---|---|
| `walk` | `direction`, `speed`, `duration_secs` | Timed, capped `cmd_vel` motion; always ends with zero Twists, then the gait-0 rest stop |
| `turn` | `direction`, `rate`, `duration_secs` | Timed, capped `cmd_vel` motion; always ends with zero Twists, then the gait-0 rest stop |
| `stop` | — | Zeroes every motion topic (either tool), sends the gait-0 rest stop (always, even when nothing was driven), cancels every goal |
| `set_pose` | `roll`, `pitch`, `yaw`, `x`, `y`, `height` | Body pose, clamped |
| `body_height` | `height` | Body height only, clamped |
| `gait` | `gait`, `step_height`, `period`, `pose` | Gait selection, clamped |
| `builtin_pose` | `pose`, `duration` | One of the seven built-in poses |
| `camera_pan` | `degrees`, `duration` | Refuses leg-servo ids before any I/O |
| `read_imu` | — | One bounded sample |
| `read_battery` | — | One bounded sample |
| `read_odom` | — | One bounded sample |
| `lidar_summary` | — | One bounded sample, summarized |
| `camera_snapshot` | `prompt` | Compressed topic first, raw fallback |

## Configuration

The full `ros2:` schema (bridge URL, allowlists, caps, and every `ros2.rospider.*` default) is
documented in [`docs/CONFIGURATION.md`](CONFIGURATION.md#ros-2-ros2). The ROSPider's own
default topic/service names and their source-derived confidence are in
[`docs/robots/rospider.md`](robots/rospider.md).

## Safety model

Writes are **deny-by-default** (D-04) — the inverse of `browser.allowed_domains`'s
allow-all-when-empty default. A topic/service/action must be declared, with its exact type,
before `ros2` will write to it; `rospider`'s own verbs target their own `ros2.rospider.*` keys,
which never widen that allowlist. Every physical parameter is clamped, never rejected, and the
result always names every clamp applied.

| Cap | Default | Enforces |
|---|---|---|
| `ros2.caps.max_linear_velocity` | 0.10 m/s | Hard ceiling on any published linear velocity. An explicit `0` disables linear motion; the code ceiling is 0.5 m/s |
| `ros2.caps.max_angular_velocity` | 0.5 rad/s | Hard ceiling on any published angular velocity. An explicit `0` disables angular motion; the code ceiling is 1.5 rad/s |
| `ros2.caps.max_motion_duration_secs` | 5.0 s | Hard ceiling on timed-motion duration; auto-stop fires at expiry (D-09). A value `<= 0` falls back to the default |
| `ros2.caps.motion_publish_hz` | 10.0 | Republish rate during timed motion, clamped into 2-20 Hz. A value `<= 0` falls back to the default |
| `ros2.caps.max_subscribe_messages` | 20 | Hard ceiling on messages collected per bounded-sample subscribe (D-12). An explicit `0` disables sampling — the ros2 subscribe action and every rospider sensor/camera read answer `is not permitted` without connecting |
| `ros2.caps.max_subscribe_secs` | 10.0 s | Hard ceiling on a bounded-sample subscribe window. A value `<= 0` falls back to the default |
| `ros2.caps.max_goal_secs` | 120.0 s | Watchdog ceiling on an unpolled action goal before it is cancelled. A value `<= 0` falls back to the default |

An operator-configured value can only ever narrow these ceilings, never widen them. An explicit
`0` on a velocity or subscribe-count cap is the strictest setting and is honored, not treated as
unset.
`--yolo`/`autonomous.yolo` relaxes **none** of the above (D-11) — the allowlist, every cap, and
the auto-stop are physical safety, treated like a budget hard stop, not an approval prompt.

## Surfaces and session-end semantics

Both tools are available on every surface: CLI, local gateway + web UI chat, voice2voice, and
cron/kanban agents (D-16). There is no weaker path — unattended and voice-driven motion rely
entirely on the allowlist/caps/auto-stop enforcement above.

Every real session boundary awaits a bounded (12s) `end_session_stop` — publishing zero velocity
on every motion topic driven, then the gait-0 rest stop for ROSPider motion topics, and
cancelling every unfinished goal. Concurrent stops run one at a time. A stop that interrupts an
in-flight motion first waits, bounded by `PREEMPT_WAIT_MS` (1.5 s), for that motion's own stop
sequence, so the gait-0 rest stop is the last message on that topic. If the motion does not
finish in time, the full stop is still sent, and a motion command that races a stop's interrupt
wait now waits for that stop to finish before installing, so the two can never publish
unsynchronized: the new velocity is never installed until the stop's zero Twists, gait-0 rest
stops, and goal cancels have all landed, and a motion started mid-stop can never interleave with
(or be mistaken for) the drained motion's trailing frames.

The (12s) bound is sized, term by term, for the worst degraded stop the code can take — the
interrupt wait (`PREEMPT_WAIT_MS`, 1.5 s), one bounded reconnect (3 s), each driven motion
topic's zero-Twist-then-gait-0 sequence (`STOP_SEQUENCE_MS`, 0.5 s per motion topic), one
goal-cancel phase covering every unfinished goal at once (`GOAL_CANCEL_PHASE_MS`, 2.5 s —
independent of how many goals are open), plus 1 s of headroom for socket writes and scheduling —
for the largest number of motion topics a process can drive. A test pins the derivation against
the raw constants for every count that ceiling allows.

A process drives at most 8 distinct motion topics (`MAX_MOTION_TOPICS`), whatever `config.yaml`
declares. A timed motion on a ninth distinct topic is refused with an `is not permitted` reply
before anything is connected, advertised or published; re-driving a topic already in use is
unaffected. The count is per process and never resets while it runs, including across
`config.yaml` edits.

A goal counts as cancelled only when its own terminal status arrives. A refused, merely accepted
(its goals still CANCELING) or unacknowledged cancel is reported as a goal failure
("goal cancel not confirmed"), and the next stop retries it; an action server that reports no
cancelable goal is reported as such, never as cancelled and never as a failure.

Every stop runs in its own task that holds the stop serialization (and the in-flight indicator)
until it is done. If the waiting surface gives up at the bound, the stop keeps going — its
gait-0 rest stops and goal cancels still go out while the process keeps running, and any later
stop or motion waits for it.

**Shutdown timing.** The gateway's graceful shutdown awaits `end_session_stop` as its first
step, and the bound plus a 2 s margin stays inside both deploy units' 15 s `TimeoutStopSec`,
which a test pins — so systemd cannot kill the gateway during that awaited stop. The MCP
teardown and task drain that follow are not covered by that budget, and the web service's
graceful shutdown does not await a session-end stop at all.

| Surface | Session-end trigger |
|---|---|
| CLI / TUI | Process exit (`run_single`/`run_chat`) — this is also the kanban worker's `chat -q` path. TUI exit: a second ctrl+c while a turn runs, `/quit`, the terminal input ending, SIGTERM, SIGHUP (a closed terminal window), or a panic or error in the TUI loop (G-49.8-21) |
| Gateway | `/new`, `/start`, the idle-expiry sweep, and graceful shutdown |
| Web chat | WebSocket teardown (every break path — close frame, recv error, keepalive failure) |
| voice2voice | `realtime_session_end`, a cancelled heartbeat, and prune when a session is dropped |
| ACP | `SessionManager::close` |
| cron/kanban | Job end, when the job touched `ros2`/`rospider` (or errored — treated as touched, conservatively) |

The TUI stops the agent runtime's tool registry — the one its turns drive, not the TUI-local
registry built for slash commands (G-49.8-21).

An action goal survives turns until session end, its own watchdog (120s default, 300s hard
ceiling), or an explicit `cancel_goal` — a session end on **one** surface of a process stops
robot work started from **another** surface of that same process, because the stop applies to
the shared `Ros2Session`, not to a per-surface handle.

## Camera frames

`sensor_msgs/Image` and `CompressedImage` topics (including `rospider camera_snapshot`) are
**described by the vision route, never returned as a raw frame or base64 payload** (D-13). The
tool internally calls a vision-capable model and returns a text description as the result — the
same pattern `browser_vision` uses, chosen specifically because IronHermes has no code path that
converts a `data:image/...;base64,` tool-result string into an actual image content block for
the next turn (see `49.8-RESEARCH.md` Pitfall 2 and the `49.8-03` spike). `LaserScan` and
`PointCloud2` come back summarized (nearest distance, per-sector distances, point counts) —
never raw arrays.

## Testing layers

- **CI mock** — an in-process mock rosbridge WebSocket server (Rust tests) covers protocol, the
  allowlist, caps, timed-motion/auto-stop, session-end stop, and goal cancel. Always runs in the
  default `cargo test` gate.
- **Local Docker harness** — `docker/ros2-humble-harness/` builds an official `ros:humble-ros-base`
  image with apt `rosbridge_suite`/`rosapi`/`turtlesim`, exposed on `127.0.0.1:9090` only, and
  runs the `#[ignore]`d `ros2_live_bridge_smoke` test against it. Opt-in, never part of the CI
  gate — proves the apt Humble rosbridge build's action ops end to end (D-06), which the mock
  server cannot.
- **Live UAT** — [`docs/robots/rospider-uat.md`](robots/rospider-uat.md), the end-of-phase human
  check against the physical ROSPider.

## Known limitations

- **`ws://` only** — no `wss://`/TLS support. Keep rosbridge on a trusted network.
- **rosbridge is unauthenticated.** Anyone who can reach the port can command the robot — bind
  to `127.0.0.1` or firewall to the operator's IP; never port-forward it to the internet.
- **There is no cmd_vel timeout** of its own on ROSPider firmware, and — unlike a normal
  cmd_vel-only robot — a zero Twist is not a stop: it leaves the ROSPider stepping in place
  indefinitely. IronHermes ends every motion, `stop`, and session end with the gait-0 rest stop
  (`kinematics_msgs/msg/Traveling` on `ros2.rospider.traveling_topic`) — the real stop on ROSPider
  firmware (D-09/D-10, G-49.8-2b). There is still no hardware-side safety net if ironhermes itself
  crashes mid-motion without running its stop path.
- **Names need on-robot reconciliation.** `docs/robots/rospider.md`'s topic/service table is
  derived from ROSPider source, not all live-confirmed; confirm against your actual robot and
  correct any `ros2.rospider.*` key that differs.
- **Goal cancels are confirmed by the goal's own terminal status, not assumed.** After a
  reconnect the goal's own status is no longer observable, so a cancel-all that the action
  server accepts is reported as not yet confirmed and retried by the next stop until the server
  reports no cancelable goal. On a rosbridge without action ops (`ACTION_OPS_UNSUPPORTED`,
  rosbridge_suite < 2.0.0), the goal's own watchdog is the remaining backstop.
- **Degraded stop ordering after a mid-stop bridge drop.** If an interrupted motion's own stop
  has not finished within `PREEMPT_WAIT_MS`, ironhermes sends its full stop anyway, but cannot
  order it against the straggling motion's frames, which may be on a reconnected connection.
  Every sender still ends with the gait-0 rest stop, and the stop reply names the timeout.
- **Idle session end on a ROSPider.** When the walk already finished before the session ended,
  the session-end stop still publishes zero Twists before the gait-0 rest stop, because D-10
  requires zero cmd_vel. On ROSPider firmware that can start one brief step until the rest stop
  lands about 0.3 s later.
- **A stop that outlives the bound.** When a stop has not finished within the bound (a stalled
  socket write, or a stop queued behind another stop), the surface gets the reply below and
  moves on:
  `ros2 session-end stop timed out — unfinished stop work continues only while this process keeps running`
  At CLI/TUI exit or gateway shutdown the rest of that stop is not sent; nothing is recorded as
  clean, so the next stop — in this process or a new one — is a full stop.
- **Motion-topic ceiling.** The count of distinct motion topics is per process and never resets
  while it runs, including across `config.yaml` edits. A deployment that needs more than
  `MAX_MOTION_TOPICS` must restart the process, or raise the constant together with
  `ROS2_SESSION_END_TIMEOUT_SECS` and both units' `TimeoutStopSec`; the budget test fails if they
  no longer fit.
- **TUI ctrl+c cancels the turn, not a running motion.** A single ctrl+c cancels the agent's
  turn but not a tool call already executing, so a timed walk or turn continues to its own
  auto-stop at the end of its requested duration. To stop it sooner, press ctrl+c again (exits
  the TUI and runs the session-end stop), type `/quit`, or ask the agent to stop (`rospider
  stop`). SIGKILL (`kill -9`) or a power loss runs no stop at all. Keep the separate
  emergency-stop command from `docs/robots/rospider.md` ready.
