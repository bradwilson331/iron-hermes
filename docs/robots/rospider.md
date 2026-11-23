<!-- generated-by: gsd-doc-writer -->
# ROSPider (Hiwonder) Operator Guide

## Overview

IronHermes talks to the ROSPider only through **rosbridge_suite over a WebSocket** (D-01) —
there is no ROS client library, DDS, or Zenoh dependency anywhere in the ironhermes build, and
no ironhermes code is ever deployed to the robot (D-18). ironhermes speaks the rosbridge v2 JSON
protocol to whatever rosbridge instance is reachable at the configured URL; the Jetson only ever
runs standard, unmodified ROS 2 packages plus the launch file shipped alongside this doc.

Two supported topologies (D-02), switched with a URL change and nothing else:

- **Operator machine on the LAN, robot separate** — ironhermes runs on your laptop/desktop,
  the ROSPider's Jetson Orin Nano is reachable over the LAN: `ws://<jetson-ip>:9090`.
- **ironhermes running on the Jetson itself** — same host as rosbridge: `ws://localhost:9090`.

Remote control from a VPS/cloud gateway (e.g. the sliplane deployment) is **out of scope** —
rosbridge has no authentication, and reaching it over the open internet is never supported by
this integration.

## Install rosbridge

On the Jetson (Ubuntu 22.04 / ROS 2 Humble):

```bash
sudo apt update && sudo apt install ros-humble-rosbridge-suite ros-humble-rosapi
source /opt/ros/humble/setup.bash
```

## Verify rosbridge supports actions

D-06: ironhermes's `send_action_goal` / `cancel_action_goal` operations require **rosbridge_suite
2.0.0 or newer** (the current Humble apt release is 2.0.8, well past that line). Check what you
have:

```bash
dpkg -s ros-humble-rosbridge-server | grep Version
apt-cache policy ros-humble-rosbridge-suite
```

If the reported version is `1.x`:

```bash
sudo apt update && sudo apt install --only-upgrade ros-humble-rosbridge-suite
```

If your mirror still serves a `1.x` build after upgrading, build rosbridge_suite from source
from a `2.0.x` release tag in a colcon overlay workspace:

```bash
mkdir -p ~/rosbridge_ws/src && cd ~/rosbridge_ws/src
git clone --branch 2.0.8 https://github.com/RobotWebTools/rosbridge_suite.git
cd ~/rosbridge_ws
rosdep install --from-paths src --ignore-src -r -y
colcon build --symlink-install
source install/setup.bash
```

**Symptom of an old rosbridge:** the `ros2` tool answers a goal action (`send_goal` /
`cancel_goal`) with a message saying the robot's rosbridge does not support ROS 2 action
operations.

## Launch rosbridge alongside the ROSPider bringup

Terminal 1 — the robot's normal bringup:

```bash
ros2 launch bringup bringup.launch.py
```

Terminal 2 — rosbridge + rosapi, using the launch file shipped next to this doc
(`ironhermes_rosbridge.launch.py`):

```bash
# on-robot topology (ironhermes runs on the Jetson): default bind 127.0.0.1
ros2 launch /path/to/ironhermes_rosbridge.launch.py

# LAN topology (ironhermes runs on the operator machine): bind the interface the
# operator machine can reach — never bind all interfaces
ros2 launch /path/to/ironhermes_rosbridge.launch.py address:=<jetson-lan-ip>
```

Confirm it's up:

```bash
ros2 node list
# expect: /rosbridge_websocket and /rosapi
```

## Network exposure

**rosbridge has no authentication.** Anyone who can reach port 9090 can command the robot —
this is a hard physical-safety boundary, not a convenience setting.

- Keep the default `127.0.0.1` bind when ironhermes runs on the Jetson itself.
- On the LAN, allow only the operator machine's IP and deny everything else:
  ```bash
  sudo ufw allow from <operator-ip> to any port 9090 proto tcp
  sudo ufw deny 9090/tcp
  ```
- Use a trusted network only.
- **Never port-forward port 9090** to the internet.

## Topics, services and types ironhermes expects

D-03: names and types derived directly from the ROSPider `Jetson_Nano_ROS2` source (README does
not document them). Confidence reflects how directly each name was confirmed in source —
`MEDIUM`/`LOW` entries should be reconciled against your actual robot (see next section) before
you rely on them.

| rospider verb | name | type | direction | derived from (ROSPider Jetson_Nano_ROS2 source) | confidence |
|---|---|---|---|---|---|
| walk / turn / stop | `/controller/cmd_vel` | `geometry_msgs/msg/Twist` | write | `src/driver/controller/controller/move_controller.py` (`~/cmd_vel` on node `controller`) | HIGH |
| walk / turn / stop (rest stop) | `/controller/traveling` | `kinematics_msgs/msg/Traveling` (`gait: 0` = stop: finish the current step, then hold pose over `time` s) | write | `src/driver/controller/controller/move_controller.py` (`~/traveling`, `set_traveling_callback`) | HIGH (confirmed on the live robot's installed source, 2026-09-24) |
| set_pose / body_height | `/controller/set_pose_euler` | `kinematics_msgs/msg/Pose` | write | same file (`~/set_pose_euler`) | MEDIUM |
| gait | `/step_controller/cmd_param` | `interfaces/msg/CmdParam` | write | `step_controller.py` and `app/app/object_tracking.py` | HIGH |
| builtin_pose | `/controller/set_pose_1` | `kinematics_msgs/srv/SetPose1` | service | `move_controller.py` (`~/set_pose_1`) | MEDIUM |
| camera_pan | `/servo_controller` | `servo_controller_msgs/msg/ServosPosition` (servo id 19, pulses 200-800, centre 500) | write | `example/.../face_track.py` | HIGH on the reference robot (2026-10-05 UAT): id 19 confirmed, positive `degrees` pans the camera left; confirm on your unit |
| camera_snapshot | `/depth_cam/rgb/image_compressed` `sensor_msgs/msg/CompressedImage` (USB camera only), fallback `/depth_cam/rgb/image_raw` `sensor_msgs/msg/Image` | read | `peripherals/launch/include/usb_cam.launch.py` and `aurora930.launch.py` | MEDIUM/HIGH |
| read_imu | `/imu` | `sensor_msgs/msg/Imu` (madgwick-filtered; raw on `/ros_robot_controller/imu_raw`) | read | `peripherals/launch/imu_filter.launch.py` | MEDIUM |
| read_battery | `/ros_robot_controller/battery` | `std_msgs/msg/UInt16` (millivolts) | read | `ros_robot_controller_node.py` | HIGH |
| read_odom | `/odom` | `nav_msgs/msg/Odometry` | read | `controller.launch.py` (EKF remap), published by `ekf_filter_node` | MEDIUM — name and type confirmed live (2026-10-05); x/y need a robot-side odometry source, see [Odometry needs a robot-side source](#odometry-needs-a-robot-side-source) |
| lidar_summary | `/scan` | `sensor_msgs/msg/LaserScan` | read | `peripherals/launch/lidar.launch.py` | HIGH |
| optional generic navigation | `/navigate_to_pose` | `nav2_msgs/action/NavigateToPose` | action | Nav2 convention | LOW |

A non-default `namespace` launch argument prefixes every name above.

On the reference ROSPider (2026-10-05 UAT, item 19), positive `degrees` pans the camera left
(negative pans right). Confirm this on your unit — servo mounting can differ by robot. The
tool's own reply still reminds you to confirm the direction on the physical robot.

## Reconcile names on your robot

Run on the Jetson:

```bash
ros2 topic list -t
ros2 service list -t
```

— or call the `ros2` tool's `list_topics` action through ironhermes itself. Compare the output
against the table above. If any name differs, correct it by setting the matching
`ros2.rospider.*` key in `config.yaml`. **No code change is ever needed** — these are the
thirteen exact keys `RospiderConfig` reads:

- `ros2.rospider.cmd_vel_topic`
- `ros2.rospider.traveling_topic`
- `ros2.rospider.pose_topic`
- `ros2.rospider.gait_topic`
- `ros2.rospider.builtin_pose_service`
- `ros2.rospider.servo_topic`
- `ros2.rospider.camera_pan_servo_id`
- `ros2.rospider.camera_compressed_topic`
- `ros2.rospider.camera_raw_topic`
- `ros2.rospider.imu_topic`
- `ros2.rospider.battery_topic`
- `ros2.rospider.odom_topic`
- `ros2.rospider.scan_topic`

## Odometry needs a robot-side source

**Where `/odom` comes from.** `/odom` is published by `ekf_filter_node` (robot_localization),
configured by `~/ros2_ws/src/driver/controller/config/ekf.yaml` on the Jetson. Its only
linear-velocity input is `odom0: odom_rf2o`; `imu0` feeds yaw only — the IMU never contributes
to x/y, so falling back to IMU readings can never give you position.

**Why it was empty on stock bringup.** Nothing published `/odom_rf2o`:

- `rf2o_laser_odometry_node` is installed under
  `~/third_party/third_party_ws/install/rf2o_laser_odometry` but no stock launch file started
  it.
- The controller's own `odom_publisher_node`, which publishes `/odom_raw` (leg/kinematic dead
  reckoning from commanded velocity), is commented out of `odom_publisher.launch.py`.

**Symptom.** `read_odom` answers with its own error text, a code span starting with
`no message on /odom within` (the window, then the `list_topics` hint). Alternatively `/odom`
publishes, but `x`/`y` stay at 0.0 after a walk.

**It is a robot-side setup gap, not an ironhermes defect.** The topic, type and sample window
were verified live on 2026-10-05.

**Fix applied on the reference robot (operator, D-18 — no ironhermes code).**
`rf2o_laser_odometry` was chosen over `odom_publisher_node`: `odom_publisher_node` integrates
the commanded `cmd_vel` (dead reckoning), so it would report motion even on a stand or with leg
slip — making item 17 pass without the robot having actually moved. `rf2o_laser_odometry`
instead measures motion from `/scan`, matching the vendor's own design (the vendor's
`slam/launch/include/robot.launch.py` already includes `rf2o_laser_odometry.launch.py`).
`ekf.yaml`'s `odom0: odom_rf2o` stays unchanged. The fix: include
`controller/launch/rf2o_laser_odometry.launch.py` from `bringup.launch.py` (defaults `/scan` ->
`/odom_rf2o`), right after the lidar launch, then restart bringup. `rf2o` needs the lidar
powered and publishing `/scan`.

**Gotcha.** The robot uses `ROS_DOMAIN_ID=10` (set in `~/ros2_ws/.typerc`) — any `ros2` CLI run
over SSH must source it first, or the ROS graph looks empty.

**Confirm on the Jetson,** in the bringup's environment (same `ROS_DOMAIN_ID`):

- `ros2 topic info -v /odom_rf2o` shows a publisher;
- `ros2 topic hz /odom` is steady;
- `ros2 topic echo --once /odom` shows `x` growing after a short walk, and staying near 0 at
  rest.

**Caveat — not yet confirmed.** A standalone test of `rf2o` saw the EKF's `/odom` jump to
`x` ≈ 8.7 m while the robot was stationary, then creep slowly — a likely first-frame velocity
spike integrated by the EKF. Confirm `/odom` stays near 0 at rest after a full bringup restart
before trusting item 17's result.

Then rerun `rospider-uat.md` item 17 — via `/gsd-verify-work 49.8`.

## Motion safety

The ROSPider controller clamps `cmd_vel` to `linear.x` ±0.12 m/s, `linear.y` ±0.10 m/s,
`angular.z` ±0.6 rad/s, and has no command timeout of its own. Every Twist published on
`/controller/cmd_vel` — including a **zero** Twist — starts a new, unbounded step generator on the
robot (`step_controller.cmd_vel`'s zero-velocity guard is commented out on the installed
firmware), so a zero Twist alone leaves the ROSPider stepping (marching) in place indefinitely,
draining the battery. The only real stop is `kinematics_msgs/msg/Traveling` with `gait: 0` on
`/controller/traveling`: `move_controller.set_traveling_callback` routes it to
`step_controller.stop_running()`, which finishes the current step and then holds pose over
`time` seconds. Because of this, ironhermes:

- Always sends **timed motion** (capped at 5 s) that ends with the zero-Twist burst and then the
  gait-0 rest stop on `/controller/traveling` (D-09, G-49.8-2b).
- **Stops (zero Twist, then the gait-0 rest stop) and cancels goals** whenever a session ends, and
  on `stop`, which sends the rest stop even when nothing was driven in the current process (D-10).
- **Never interleaves two stop sequences.**
  - A `stop` or session end that interrupts a walk/turn first waits, up to 1.5 s, for that
    walk's own zero-Twist-then-gait-0 stop to finish. It then adds only the gait-0 rest stop
    for that topic, so no zero Twist follows the gait-0 message.
  - Stop requests that arrive together, such as the session-end hooks at TUI or CLI exit,
    run one at a time. A request that finds an identical stop just completed, with nothing
    driven since, is answered by that stop without resending.
  - If the interrupted walk's own stop does not finish within 1.5 s (for example after a
    bridge drop mid-stop), the full zero-Twist-then-gait-0 stop is sent anyway. The stop is
    never skipped, and the reply says the interrupted motion did not finish its own stop.
- Never relaxes any of the above under `--yolo` (D-11).

Manual emergency stop, from any machine that can reach rosbridge — run both commands, **in this
order**, on the Jetson in a terminal that has sourced the ROSPider workspace
(`source ~/ros2_ws/install/setup.bash`) so the `kinematics_msgs` type resolves:

```bash
ros2 topic pub --once /controller/cmd_vel geometry_msgs/msg/Twist "{}"
ros2 topic pub --once /controller/traveling kinematics_msgs/msg/Traveling "{gait: 0, time: 1.0}"
```

**The `Traveling` `gait: 0` message must be the last one sent.** A zero Twist published after it
restarts the in-place march.

## ironhermes configuration

Minimal `config.yaml`:

```yaml
ros2:
  bridge_url: "ws://<jetson-ip-or-localhost>:9090"
  publish:
    - name: "/controller/cmd_vel"
      type: "geometry_msgs/msg/Twist"
      limits:
        "linear.x": [-0.12, 0.12]
```

Or set the env override instead of (or in addition to) `bridge_url`:

```bash
export IRONHERMES_ROS2_BRIDGE_URL="ws://<jetson-ip-or-localhost>:9090"
```

See the [`ros2:` section of docs/CONFIGURATION.md](../CONFIGURATION.md#ros-2-ros2) for every key
and its default.

## Troubleshooting

- **Tool not visible in the agent's tool list** — no bridge URL resolved. Set `ros2.bridge_url`
  in `config.yaml` or `IRONHERMES_ROS2_BRIDGE_URL`.
- **Connection refused** — rosbridge isn't running, or it's bound to `127.0.0.1` while you're
  connecting from the LAN. Re-launch with `address:=<jetson-lan-ip>`.
- **"…is not permitted…" answers** — the topic/service/action name isn't declared in your
  `ros2:` config allowlist (D-04, deny-by-default for writes). Add it.
- **Action operations unsupported** — old rosbridge; see "Verify rosbridge supports actions"
  above.
