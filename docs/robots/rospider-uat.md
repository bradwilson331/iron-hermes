<!-- generated-by: gsd-doc-writer -->
# ROSPider Live Acceptance Checklist

Phase 49.8 D-17 — the phase's **final validation layer**. The CI mock server proves protocol
logic; `docker/ros2-humble-harness/smoke.sh` proves the apt Humble rosbridge build's action ops
(D-06); this checklist is the only layer that proves the source-derived names in
[`docs/robots/rospider.md`](rospider.md) and the physical stop actually hold on the real robot
(D-03, D-17).

Run every item below against the physical ROSPider, with rosbridge launched per
[`docs/robots/rospider.md`](rospider.md) and ironhermes configured with a working
`ros2.bridge_url`. Record results — including any corrected `ros2.rospider.*` names — in
`49.8-UAT.md`.

## Setup

1. **rosbridge version** — `dpkg -s ros-humble-rosbridge-server | grep Version` on the Jetson.
   **Expect:** `2.0.0` or newer (D-06). An older version means action ops (`send_goal`,
   `cancel_goal`) will not work — upgrade before continuing.
2. **Names reconciled** — run `ros2 topic list -t` / `ros2 service list -t` on the Jetson (or the
   `ros2` tool's `list_topics`) and compare against `docs/robots/rospider.md`'s table. **Expect:**
   every name matches, or you've corrected the matching `ros2.rospider.*` key in `config.yaml`.
3. **`ros2.bridge_url` set** — confirm the `ros2`/`rospider` tools are visible (not hidden) in
   your agent's tool list. **Expect:** both tools appear; if not, the bridge URL didn't resolve.
   **Note:** after rebuilding ironhermes, redeploy the binaries before testing this checklist —
   `scripts/deploy/install.sh` for the gateway, then `web-build.sh` followed by `web-install.sh`
   for the web UI. The interrupt and session-end retest (items 8 and 23-27) needs a build that
   includes plans 49.8-18 through 49.8-22 — redeploy the gateway, web UI **and CLI** binaries
   first.
4. **Emergency-stop command ready** — have both commands below ready to paste in a separate
   terminal before you start, in a shell that has sourced the ROSPider workspace
   (`source ~/ros2_ws/install/setup.bash`) so the `kinematics_msgs` type resolves, and run **in
   this order**:
   ```bash
   ros2 topic pub --once /controller/cmd_vel geometry_msgs/msg/Twist "{}"
   ros2 topic pub --once /controller/traveling kinematics_msgs/msg/Traveling "{gait: 0, time: 1.0}"
   ```
   **The `Traveling` `gait: 0` message must be the last one sent** — a zero Twist published after
   it restarts the in-place march.
5. **First motion tests on a stand** — put the ROSPider on a stand (legs clear of the ground, or
   otherwise unable to walk away) for every item below until you've confirmed `stop` and the
   emergency-stop command both work.

## Motion

6. **Walk forward** — ask the agent to walk forward for 2 seconds. **Expect:** the robot walks,
   then stops **by itself** — you should not need to say "stop". Its legs come to rest within
   about 2 seconds of the walk ending, as the current step finishes and it holds pose; the legs
   must not keep stepping in place. The reply mentions the gait-0 rest stop. Then ask for a
   second 1-second walk: it must walk and come to rest again, confirming gait 0 does not latch
   the robot stopped.
7. **Turn left** — ask the agent to turn left for 2 seconds. **Expect:** the same auto-stop
   behavior as item 6 — the legs come to rest, no stepping in place.
8. **Interrupted walk** — ask for a 5-second walk, then say "stop" partway through. **Expect:**
   the robot stops walking promptly and its legs come to rest within about 1.5 seconds of the
   stop command (the current step finishes first), with no stepping in place. The stop reply must
   not say the interrupted motion did not finish its own stop. Then, with the robot idle, say
   "stop" again. **Expect:** the reply reports the gait-0 rest stop sent, even though there is
   nothing else to stop.
9. **Speed clamp** — ask for a walk at 1.0 m/s. **Expect:** the reply names the clamp to the
   configured `ros2.caps.max_linear_velocity` (default 0.10 m/s) — the robot never actually
   reaches 1.0 m/s.

## Body and gait

10. **set_pose** — ask for `set_pose` with `height=140`, `roll=0.1`. **Expect:** the body tilts
    and raises accordingly, within the documented clamps (roll ±0.2, height 100–160mm).
11. **body_height** — ask for `body_height=120`. **Expect:** the body height changes; the reply
    confirms 120mm (the default, so no clamp note expected).
12. **gait ripple** — ask to switch gait to `ripple`. **Expect:** the walking gait pattern
    visibly changes on the next walk.
13. **gait tripod** — ask to switch back to `tripod`. **Expect:** the gait pattern changes back.
14. **builtin_pose** — ask for `builtin_pose` with `pose=DEFAULT_POSE`. **Expect:** the robot
    moves to its default standing pose.

## Sensors and camera

15. **Battery** — ask to check the battery. **Expect:** a plausible voltage reading (not an
    error, not an obviously wrong value like 0 or negative).
16. **IMU** — ask to read the IMU, then physically tilt the robot by hand. **Expect:** the
    reported roll changes to reflect the tilt.
17. **Odometry** — ask to read odometry, walk the robot forward, then read odometry again.
    **Expect:** the reported `x` position has grown. **Prerequisite:** a robot-side odometry source
    must be running (see `docs/robots/rospider.md`, "Odometry needs a robot-side source"). Without
    it, `read_odom` reports no message or `x` stays at 0 — record that as a robot-side setup gap,
    not an ironhermes failure.
18. **Lidar** — place a box or other obstacle directly in front of the robot, then ask for a
    lidar summary. **Expect:** the reported front-sector distance roughly matches the box's
    actual distance.
19. **Camera pan** — ask to pan the camera 30 degrees. **Expect:** the camera physically moves.
    **Record which direction** (left or right) a positive `degrees` value actually moved it —
    the reference robot panned left for a positive value on 2026-10-05; record whether your
    unit matches.
20. **Camera snapshot** — ask for a camera snapshot. **Expect:** a text description that
    plausibly matches what the camera is actually pointed at.

## Safety

21. **Yolo does not relax caps** — start a session with `--yolo` and repeat the speed-clamp test
    (item 9). **Expect:** the walk is still clamped to the same velocity — yolo changes nothing
    about the physical caps.
22. **Undeclared write refused** — ask the `ros2` tool to publish to a topic you have NOT
    declared in `ros2.publish` (e.g. an arbitrary made-up topic name). **Expect:** an
    `is not permitted` reply, and the robot does not move at all.

## Session end on every surface

D-16 — start a Nav2 goal (or any long-running action) or a walk, then confirm each surface stops
it. If a walk is still running when the session ends, the legs must come to rest with no extra
step. If the walk had already finished on its own, one brief step before the legs settle is
expected (the session-end zero Twist precedes the gait-0 rest stop, D-10) — continuous stepping
in place is a fail either way:

23. **Telegram `/new`** — start a motion or goal from Telegram, then send `/new`. **Expect:** the
    robot stops with its legs at rest (no stepping in place) / the goal is cancelled. If the
    reply or the gateway log reports "goal cancel not confirmed", the goal was NOT cancelled —
    record that as a fail.
24. **Web chat tab close** — start a motion or goal from the web UI, then close the browser tab.
    **Expect:** the robot stops with its legs at rest (no stepping in place) / the goal is
    cancelled. If the reply or the gateway log reports "goal cancel not confirmed", the goal was
    NOT cancelled — record that as a fail.
25. **Cron job** — schedule a cron job that walks for 1 second, let it run. **Expect:** the robot
    stops with its legs at rest (no stepping in place) when the job completes (it should already
    stop on its own via the timed-motion auto-stop, but confirm no lingering motion after the
    job's session ends).
26. **TUI exit** — start a 5-second walk from the TUI, then exit the TUI while it is still
    walking: press ctrl+c twice (the first press cancels the turn, the second exits) or type
    `/quit`. **Expect:** the robot stops with its legs at rest (no stepping in place). Optional
    repeat: end the TUI with `kill -TERM <pid>` from another terminal; the same expectation
    applies.
27. **CLI exit** — start a motion from a CLI single-shot invocation, let the process exit.
    **Expect:** the robot stops with its legs at rest (no stepping in place).

## Voice

28. **Voice2voice motion** — say "walk forward" over voice2voice. **Expect:** without `--yolo`,
    an approval prompt appears before anything moves; once approved (or immediately, under
    `--yolo`), a timed walk happens and stops on its own.

## Optional (bridge-drop check)

29. **Mid-walk bridge drop** *(optional, stand only)* — start a walk, then stop the rosbridge
    process mid-motion. **Warning: the ROSPider controller keeps executing the last velocity it
    received when the bridge disappears — it has no command timeout of its own.** Have the
    manual emergency-stop command (Setup, item 4) ready on a SEPARATE, already-running rosbridge
    connection before you try this, and only attempt it with the robot on a stand.

## Record

Record every result — pass/fail per item, whether the legs came to rest on every stop path, which
interrupt paths were tested while the robot was still walking, and any corrected
`ros2.rospider.*` names or the observed `camera_pan` direction — in `49.8-UAT.md`. For items
23-27, also record the logged session-end stop report (for example from
`journalctl -u ironhermes-gateway`). A report starting
`ros2 session-end stop timed out` is a fail.
