//! `RospiderTool` — high-level Hiwonder ROSPider verbs (Phase 49.8 D-05/D-15).
//!
//! Mirrors `Ros2Tool`'s shape exactly (same four fields, same `is_available`/
//! `prerequisites`/`credentials`/`timeout_secs`/`on_session_end`, same
//! `cap_result` wrapper around every return) but talks to the ROSPider's OWN
//! topics/service — `cfg.ros2.rospider.*` — never `ros2.publish` /
//! `ros2.services` / `ros2.actions` (D-14). Every verb builds its message
//! from typed, clamped parameters; no verb accepts a raw message body from
//! the model (D-08). Motion goes through the same [`super::motion::run_timed_motion`]
//! engine the generic tool uses, on the shared [`super::session::Ros2Session`]
//! (registered by [`crate::registry::ToolRegistry::register_ros2_tools`]
//! alongside [`super::ros2_tool::Ros2Tool`]).
//!
//! Camera reads reuse [`super::read_ops::describe_frame`] (49.8-06's D-13
//! vision seam) directly — the frame never appears in a tool result.

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::time::Duration;

use crate::registry::{Prerequisite, Tool};
use crate::ros2::motion::StopFollowup;
use crate::ros2::read_ops::{self, SubscribeRequest};
use crate::ros2::safety::{
    ClampNote, EffectiveCaps, clamp_subscribe_count, clamp_subscribe_window,
};
use crate::ros2::{
    BRIDGE_URL_ENV, ROS2_TOOL_TIMEOUT_SECS, ROS2_TOOLSET, Ros2ConfigSource, Ros2Session,
    validate_bridge_url,
};

// ---------------------------------------------------------------------------
// Cap numbers (49.8-09-PLAN.md "ROSPider verb facts" / "Artifacts this phase
// produces") — every value here is a hard code ceiling on top of a request
// this tool clamps, never rejects.
// ---------------------------------------------------------------------------

pub const ROSPIDER_DEFAULT_WALK_SPEED: f64 = 0.05;
pub const ROSPIDER_DEFAULT_TURN_RATE: f64 = 0.3;
pub const ROSPIDER_ROLL_PITCH_MAX: f64 = 0.2;
pub const ROSPIDER_YAW_MAX: f64 = 0.4;
pub const ROSPIDER_XY_OFFSET_MAX_MM: f64 = 20.0;
pub const ROSPIDER_BODY_HEIGHT_MIN_MM: f64 = 100.0;
pub const ROSPIDER_BODY_HEIGHT_MAX_MM: f64 = 160.0;
pub const ROSPIDER_BODY_HEIGHT_DEFAULT_MM: f64 = 120.0;
/// `(min, max)` mm.
pub const ROSPIDER_STEP_HEIGHT_RANGE_MM: (f64, f64) = (10.0, 40.0);
pub const ROSPIDER_STEP_HEIGHT_DEFAULT_MM: f64 = 20.0;
/// `(min, max)` seconds.
pub const ROSPIDER_GAIT_PERIOD_RANGE: (f64, f64) = (0.5, 2.0);
pub const ROSPIDER_GAIT_PERIOD_DEFAULT_SECS: f64 = 1.0;
/// The seven ROSPider built-in poses with a `_TRANSFORM` in `build_in_pose.py`
/// (source-derived, `docs/robots/rospider.md`).
pub const ROSPIDER_BUILTIN_POSES: &[&str] = &[
    "DEFAULT_POSE",
    "DEFAULT_POSE_M",
    "NARROW_POSE",
    "SLAM_POSE",
    "HOLE_POSE",
    "SHRINK_POSE",
    "SIDE_SHIFT_POSE",
];
pub const ROSPIDER_BUILTIN_POSE_DURATION_MIN_SECS: f64 = 0.5;
pub const ROSPIDER_BUILTIN_POSE_DURATION_MAX_SECS: f64 = 3.0;
pub const ROSPIDER_BUILTIN_POSE_DURATION_DEFAULT_SECS: f64 = 1.0;
pub const ROSPIDER_PAN_MAX_DEG: f64 = 72.0;
pub const ROSPIDER_PAN_CENTER_PULSE: i64 = 500;
/// `(min, max)` pulses.
pub const ROSPIDER_PAN_PULSE_RANGE: (i64, i64) = (200, 800);
pub const ROSPIDER_PULSES_PER_DEG: f64 = 1000.0 / 240.0;
pub const ROSPIDER_CAMERA_PAN_DURATION_MIN_SECS: f64 = 0.1;
pub const ROSPIDER_CAMERA_PAN_DURATION_MAX_SECS: f64 = 2.0;
pub const ROSPIDER_CAMERA_PAN_DURATION_DEFAULT_SECS: f64 = 0.5;
/// Hiwonder bus servo ids 1-18 drive the ROSPider's 18 leg joints (D-08) —
/// `camera_pan` refuses to command any of these, whatever
/// `ros2.rospider.camera_pan_servo_id` says.
pub const ROSPIDER_LEG_SERVO_IDS: std::ops::RangeInclusive<u16> = 1..=18;
/// Default bounded-sample window for every read verb (`read_imu`/
/// `read_battery`/`read_odom`/`lidar_summary`) — one message, up to this
/// many seconds by default. The EFFECTIVE window used by `sample_one` is
/// `clamp_subscribe_window(Some(SENSOR_WINDOW_SECS), caps)` — the same clamp
/// the generic `ros2` tool's `subscribe` action uses (VERIFICATION gap #4 /
/// WR-01) — so an operator's `ros2.caps.max_subscribe_secs` can only
/// SHORTEN this window, never widen it.
pub const SENSOR_WINDOW_SECS: f64 = 2.0;

const BUILTIN_POSE_SERVICE_TIMEOUT_SECS: f64 = 5.0;

// ---------------------------------------------------------------------------
// G-49.8-2b (Phase 49.8 Plan 13): the gait-0 rest stop.
// ---------------------------------------------------------------------------

/// `kinematics_msgs/msg/Traveling` — the ROSPider's real stop.
/// `move_controller.set_traveling_callback` routes `gait: 0` to
/// `step_controller.stop_running()`, which finishes the current step and
/// holds pose. A zero Twist on `cmd_vel_topic` alone starts a new,
/// unbounded step generator (its zero-velocity guard is commented out on
/// the robot), leaving the legs stepping in place forever.
pub const ROSPIDER_TRAVELING_TYPE: &str = "kinematics_msgs/msg/Traveling";
/// The `gait` value that means "stop" on `/controller/traveling`. `-2`
/// instead moves to `DEFAULT_POSE`, which this rest stop does not use.
pub const ROSPIDER_REST_STOP_GAIT: i64 = 0;
/// `set_traveling_callback`'s `set_pose(None, None, msg.time)` settles the
/// hold pose over this many seconds.
pub const ROSPIDER_REST_STOP_POSE_SECS: f64 = 1.0;

const DESCRIPTION: &str = "High-level verbs for the Hiwonder ROSPider hexapod, built on the same \
    rosbridge session and physical-safety engine as the generic ros2 tool — prefer this over ros2 \
    for the ROSPider so you rarely have to write message JSON by hand. Actions: walk (with \
    direction: forward|backward|left|right, speed m/s default 0.05, duration_secs — timed, capped, \
    always ends in the zero-velocity Twist stop followed by the gait-0 rest stop that actually \
    brings the ROSPider's legs to rest), turn (with direction: left|right, rate rad/s default 0.3, \
    duration_secs — same stop sequence as walk), stop (zeroes every motion topic, then sends the \
    gait-0 rest stop that actually brings the ROSPider's legs to rest — even when nothing was \
    driven in this session — and cancels every unfinished action goal; also runs automatically \
    when the session ends), set_pose (with roll/pitch/yaw in radians clamped \
    ±0.2/±0.2/±0.4, x/y mm offset clamped ±20, height mm body height clamped 100-160 default 120 — \
    every field clamped, never rejected), body_height (height mm only, same range), gait (with \
    gait: ripple|tripod default tripod, step_height mm clamped 10-40 default 20, period s clamped \
    0.5-2.0 default 1.0, pose: one of the seven ROSPider built-in poses default DEFAULT_POSE), \
    builtin_pose (with pose and duration s clamped 0.5-3.0 default 1.0 — moves to a named built-in \
    pose via the driver's own service), camera_pan (with degrees clamped ±72°, 0 = centre; \
    refuses to command a leg servo), read_imu, read_battery, read_odom, lidar_summary (one-line \
    readings from a bounded sample of up to 2s that never exceeds ros2.caps.max_subscribe_secs), \
    and camera_snapshot (with an optional prompt — describes \
    one camera frame via the vision model, preferring the compressed topic and falling back to \
    raw; the frame itself is never returned, only its description). Every write targets this \
    robot's own ros2.rospider.* config keys — never ros2.publish/services/actions, and never a \
    leg servo (Hiwonder bus servo ids 1-18). Never try to stop the ROSPider by publishing a zero \
    Twist through the ros2 tool — a zero Twist alone leaves it stepping in place; use rospider \
    stop. No verb accepts a raw message body from the model.";

/// The high-level `rospider` tool (D-15). Same shape as [`super::ros2_tool::Ros2Tool`]:
/// stateless per call except for the shared, persistent [`Ros2Session`].
pub struct RospiderTool {
    session: Arc<Ros2Session>,
    source: Ros2ConfigSource,
    creds: Arc<crate::credentials::ToolCredentials>,
    /// D-13: `camera_snapshot`'s vision route — reused directly from
    /// [`read_ops::describe_frame`], the same seam 49.8-06 built for the
    /// generic tool's camera-topic `subscribe`.
    vision: Option<Arc<dyn crate::browser_vision::VisionClientHandle>>,
}

impl RospiderTool {
    pub fn new(
        session: Arc<Ros2Session>,
        source: Ros2ConfigSource,
        creds: Arc<crate::credentials::ToolCredentials>,
        vision: Option<Arc<dyn crate::browser_vision::VisionClientHandle>>,
    ) -> Self {
        Self {
            session,
            source,
            creds,
            vision,
        }
    }

    // -----------------------------------------------------------------
    // Motion — walk / turn / stop
    // -----------------------------------------------------------------

    async fn do_walk(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let topic = match target_or_error("cmd_vel_topic", &cfg.ros2.rospider.cmd_vel_topic) {
            Ok(t) => t,
            Err(e) => return Ok(e),
        };

        let direction = args["direction"].as_str().unwrap_or("forward");
        let speed = args["speed"]
            .as_f64()
            .unwrap_or(ROSPIDER_DEFAULT_WALK_SPEED);
        let (lx, ly) = match direction {
            "forward" => (speed, 0.0),
            "backward" => (-speed, 0.0),
            "left" => (0.0, speed),
            "right" => (0.0, -speed),
            other => {
                return Ok(format!(
                    "Error: unknown walk direction '{other}' — expected forward, backward, left, or right"
                ));
            }
        };
        let twist = json!({
            "linear": {"x": lx, "y": ly, "z": 0.0},
            "angular": {"x": 0.0, "y": 0.0, "z": 0.0},
        });
        self.run_motion(&cfg, &topic, "geometry_msgs/msg/Twist", twist, args)
            .await
    }

    async fn do_turn(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let topic = match target_or_error("cmd_vel_topic", &cfg.ros2.rospider.cmd_vel_topic) {
            Ok(t) => t,
            Err(e) => return Ok(e),
        };

        let direction = args["direction"].as_str().unwrap_or("left");
        let rate = args["rate"].as_f64().unwrap_or(ROSPIDER_DEFAULT_TURN_RATE);
        let az = match direction {
            "left" => rate,
            "right" => -rate,
            other => {
                return Ok(format!(
                    "Error: unknown turn direction '{other}' — expected left or right"
                ));
            }
        };
        let twist = json!({
            "linear": {"x": 0.0, "y": 0.0, "z": 0.0},
            "angular": {"x": 0.0, "y": 0.0, "z": az},
        });
        self.run_motion(&cfg, &topic, "geometry_msgs/msg/Twist", twist, args)
            .await
    }

    /// Shared timed-motion path for `walk`/`turn` — clamps `twist` and the
    /// requested `duration_secs` against [`EffectiveCaps`] (same hard
    /// ceilings the generic tool's Twist-family publish uses), then hands off
    /// to [`super::motion::run_timed_motion`], which republishes at the
    /// capped rate for the capped duration and ALWAYS finishes with a
    /// zero-velocity stop — followed, on the ROSPider's own cmd_vel_topic,
    /// by the gait-0 rest stop ([`rospider_stop_followup`], G-49.8-2b): the
    /// real stop on ROSPider firmware, where a zero Twist alone leaves the
    /// legs stepping in place.
    async fn run_motion(
        &self,
        cfg: &ironhermes_core::Config,
        topic: &str,
        msg_type: &str,
        mut twist: serde_json::Value,
        args: &serde_json::Value,
    ) -> anyhow::Result<String> {
        use crate::ros2::motion::{MotionRequest, run_timed_motion};
        use crate::ros2::safety::{clamp_duration, clamp_twist};

        let caps = EffectiveCaps::from_config(&cfg.ros2.caps);
        let notes = clamp_twist(&mut twist, &caps, false);
        let applied_duration = clamp_duration(args["duration_secs"].as_f64(), &caps);

        let Some(url) = self.source.bridge_url(cfg) else {
            return Ok(format!(
                "Error: no rosbridge URL configured — set {BRIDGE_URL_ENV} or ros2.bridge_url in config.yaml"
            ));
        };
        if let Err(e) = validate_bridge_url(&url) {
            return Ok(format!("Error: {e}"));
        }

        tracing::info!(
            topic,
            msg_type,
            clamp_notes = notes.len(),
            duration_secs = applied_duration,
            "rospider: motion publish"
        );

        // G-49.8-2b: on the ROSPider's own cmd_vel_topic, every walk/turn
        // ends its zero-Twist burst with the gait-0 rest stop — the real
        // stop on ROSPider firmware, where a zero Twist alone leaves the
        // legs stepping in place. `None` only when ros2.rospider.traveling_topic
        // is empty (the operator's explicit opt-out).
        let stop_followup = rospider_stop_followup(cfg, topic);
        let followup_topic = stop_followup.as_ref().map(|f| f.topic.clone());

        let req = MotionRequest {
            topic: topic.to_string(),
            msg_type: msg_type.to_string(),
            twist,
            duration: Duration::from_secs_f64(applied_duration),
            hz: caps.publish_hz,
            stamped: false,
            stop_followup,
        };
        let outcome = run_timed_motion(self.session.clone(), url, req).await;
        // 49.8-19 (VERIFICATION gap #1, D-08/D-11): a refused motion topic
        // never touches yolo/approval — the ceiling reply is returned
        // verbatim, before any of the reply assembly below.
        if let Some(reply) = outcome.refused {
            return Ok(reply);
        }

        let mut out = format!(
            "rospider motion on {topic} [{msg_type}]: {} published, {:.2}s elapsed",
            outcome.published,
            outcome.elapsed.as_secs_f64()
        );
        if outcome.stop_sent {
            match (outcome.stop_followup_sent, &followup_topic) {
                (Some(true), Some(t)) => out.push_str(&format!(
                    ", stop sent (3 zero Twists, then gait-0 rest stop on {t})"
                )),
                _ => out.push_str(", stop sent (3 zero Twists)"),
            }
        }
        for note in &notes {
            out.push_str(&format!("\nClamped: {note}"));
        }
        if outcome.interrupted {
            out.push_str("\ninterrupted before its full duration elapsed");
        }
        if outcome.connection_lost {
            match outcome.reconnect_stop {
                Some(true) => out.push_str(&format!(
                    "\nError: bridge connection lost after {:.2}s; stop re-sent after reconnect",
                    outcome.elapsed.as_secs_f64()
                )),
                _ => out.push_str(
                    "\nError: bridge connection lost — reconnect failed — the robot may still be moving",
                ),
            }
        }
        match outcome.stop_followup_sent {
            Some(false) => out.push_str(&format!(
                "\nError: the gait-0 rest stop on {} was not confirmed sent — the ROSPider may keep \
                 stepping in place; run the rospider stop action or the manual emergency stop in \
                 docs/robots/rospider.md",
                followup_topic.as_deref().unwrap_or("(traveling topic)")
            )),
            None => out.push_str(
                "\nNote: gait-0 rest stop disabled (ros2.rospider.traveling_topic is empty) — on a \
                 ROSPider a zero Twist alone leaves the legs stepping in place",
            ),
            Some(true) => {}
        }
        Ok(out)
    }

    /// `stop` (D-10/D-15) — the on-demand form of the same stop
    /// `on_session_end` runs automatically: zeroes every motion topic
    /// (rospider's own AND any the generic tool has driven — they share one
    /// [`Ros2Session`]) and cancels every unfinished goal, via `stop_all`
    /// (which also replays the gait-0 rest stop for any ROSPider motion
    /// topic this process actually drove). On top of that, this send is
    /// UNCONDITIONAL, on purpose (G-49.8-2b): every explicit `stop` also
    /// sends the gait-0 rest stop directly, even when nothing was driven in
    /// this process — the robot may still be marching from a previous
    /// process that no longer exists, and `stop_all`'s replay alone can
    /// never cover that case (it only ever replays what THIS process
    /// recorded). `on_session_end` deliberately does NOT do this
    /// unconditional send — an idle session must still open no bridge
    /// connection.
    ///
    /// 49.8-16 (G-49.8-2b): this unconditional follow-up runs strictly AFTER
    /// `stop_all` has returned — and therefore released `stop_lock` — then
    /// `send_stop_followup` itself takes `stop_lock` for its own connect/
    /// advertise/burst. It is never coalesced (it is unconditional by
    /// design), and this ordering (never nested) is what keeps the two
    /// `stop_lock` acquisitions in this function sequential rather than a
    /// deadlock.
    async fn do_stop(&self) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let url = self.source.bridge_url(&cfg);
        let report = self.session.stop_all(url.as_deref()).await;

        match rospider_rest_stop(&cfg) {
            None => Ok(format!(
                "{report}; gait-0 rest stop disabled (ros2.rospider.traveling_topic is empty)"
            )),
            Some(f) => {
                let Some(u) = url.as_deref() else {
                    return Ok(format!(
                        "{report}; gait-0 rest stop not sent: no rosbridge URL configured"
                    ));
                };
                if let Err(e) = validate_bridge_url(u) {
                    return Ok(format!("{report}; gait-0 rest stop not sent: {e}"));
                }
                match self.session.send_stop_followup(u, &f).await {
                    Ok(()) => Ok(format!("{report}; gait-0 rest stop sent on {}", f.topic)),
                    Err(e) => Ok(format!(
                        "{report}\nError: gait-0 rest stop on {} failed — {e} — the ROSPider may \
                         keep stepping in place; use the manual emergency stop in \
                         docs/robots/rospider.md",
                        f.topic
                    )),
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // Body — set_pose / body_height / gait / builtin_pose
    // -----------------------------------------------------------------

    async fn do_set_pose(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let topic = match target_or_error("pose_topic", &cfg.ros2.rospider.pose_topic) {
            Ok(t) => t,
            Err(e) => return Ok(e),
        };

        let mut notes = Vec::new();
        let roll = clamp_field(
            &mut notes,
            "roll",
            args["roll"].as_f64().unwrap_or(0.0),
            -ROSPIDER_ROLL_PITCH_MAX,
            ROSPIDER_ROLL_PITCH_MAX,
        );
        let pitch = clamp_field(
            &mut notes,
            "pitch",
            args["pitch"].as_f64().unwrap_or(0.0),
            -ROSPIDER_ROLL_PITCH_MAX,
            ROSPIDER_ROLL_PITCH_MAX,
        );
        let yaw = clamp_field(
            &mut notes,
            "yaw",
            args["yaw"].as_f64().unwrap_or(0.0),
            -ROSPIDER_YAW_MAX,
            ROSPIDER_YAW_MAX,
        );
        let x = clamp_field(
            &mut notes,
            "x",
            args["x"].as_f64().unwrap_or(0.0),
            -ROSPIDER_XY_OFFSET_MAX_MM,
            ROSPIDER_XY_OFFSET_MAX_MM,
        );
        let y = clamp_field(
            &mut notes,
            "y",
            args["y"].as_f64().unwrap_or(0.0),
            -ROSPIDER_XY_OFFSET_MAX_MM,
            ROSPIDER_XY_OFFSET_MAX_MM,
        );
        let height = clamp_field(
            &mut notes,
            "height",
            args["height"]
                .as_f64()
                .unwrap_or(ROSPIDER_BODY_HEIGHT_DEFAULT_MM),
            ROSPIDER_BODY_HEIGHT_MIN_MM,
            ROSPIDER_BODY_HEIGHT_MAX_MM,
        );

        let msg = json!({
            "position": {"x": x, "y": y, "z": height},
            "orientation": {"roll": roll, "pitch": pitch, "yaw": yaw},
        });

        self.publish_once(&cfg, &topic, "kinematics_msgs/msg/Pose", msg, notes)
            .await
    }

    async fn do_body_height(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let topic = match target_or_error("pose_topic", &cfg.ros2.rospider.pose_topic) {
            Ok(t) => t,
            Err(e) => return Ok(e),
        };

        let mut notes = Vec::new();
        let height = clamp_field(
            &mut notes,
            "height",
            args["height"]
                .as_f64()
                .unwrap_or(ROSPIDER_BODY_HEIGHT_DEFAULT_MM),
            ROSPIDER_BODY_HEIGHT_MIN_MM,
            ROSPIDER_BODY_HEIGHT_MAX_MM,
        );

        let msg = json!({
            "position": {"x": 0.0, "y": 0.0, "z": height},
            "orientation": {"roll": 0.0, "pitch": 0.0, "yaw": 0.0},
        });

        self.publish_once(&cfg, &topic, "kinematics_msgs/msg/Pose", msg, notes)
            .await
    }

    async fn do_gait(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let topic = match target_or_error("gait_topic", &cfg.ros2.rospider.gait_topic) {
            Ok(t) => t,
            Err(e) => return Ok(e),
        };

        let gait_name = args["gait"].as_str().unwrap_or("tripod");
        let gait_num: i64 = match gait_name {
            "ripple" => 1,
            "tripod" => 2,
            other => {
                return Ok(format!(
                    "Error: unknown gait '{other}' — expected ripple or tripod"
                ));
            }
        };

        let pose = args["pose"].as_str().unwrap_or("DEFAULT_POSE");
        if !ROSPIDER_BUILTIN_POSES.contains(&pose) {
            return Ok(format!(
                "Error: unknown pose '{pose}' — expected one of {}",
                ROSPIDER_BUILTIN_POSES.join(", ")
            ));
        }

        let mut notes = Vec::new();
        let step_height = clamp_field(
            &mut notes,
            "step_height",
            args["step_height"]
                .as_f64()
                .unwrap_or(ROSPIDER_STEP_HEIGHT_DEFAULT_MM),
            ROSPIDER_STEP_HEIGHT_RANGE_MM.0,
            ROSPIDER_STEP_HEIGHT_RANGE_MM.1,
        );
        let period = clamp_field(
            &mut notes,
            "period",
            args["period"]
                .as_f64()
                .unwrap_or(ROSPIDER_GAIT_PERIOD_DEFAULT_SECS),
            ROSPIDER_GAIT_PERIOD_RANGE.0,
            ROSPIDER_GAIT_PERIOD_RANGE.1,
        );

        let msg = json!({
            "pose": pose,
            "gait": gait_num,
            "height": step_height.round() as i64,
            "period": period,
        });

        self.publish_once(&cfg, &topic, "interfaces/msg/CmdParam", msg, notes)
            .await
    }

    async fn do_builtin_pose(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let service = match target_or_error(
            "builtin_pose_service",
            &cfg.ros2.rospider.builtin_pose_service,
        ) {
            Ok(t) => t,
            Err(e) => return Ok(e),
        };

        let pose = args["pose"].as_str().unwrap_or("DEFAULT_POSE");
        if !ROSPIDER_BUILTIN_POSES.contains(&pose) {
            return Ok(format!(
                "Error: unknown pose '{pose}' — expected one of {}",
                ROSPIDER_BUILTIN_POSES.join(", ")
            ));
        }

        let duration = args["duration"]
            .as_f64()
            .unwrap_or(ROSPIDER_BUILTIN_POSE_DURATION_DEFAULT_SECS)
            .clamp(
                ROSPIDER_BUILTIN_POSE_DURATION_MIN_SECS,
                ROSPIDER_BUILTIN_POSE_DURATION_MAX_SECS,
            );

        let Some(url) = self.source.bridge_url(&cfg) else {
            return Ok(format!(
                "Error: no rosbridge URL configured — set {BRIDGE_URL_ENV} or ros2.bridge_url in config.yaml"
            ));
        };
        if let Err(e) = validate_bridge_url(&url) {
            return Ok(format!("Error: {e}"));
        }
        let client = match self.session.client_for(&url).await {
            Ok(c) => c,
            Err(e) => return Ok(format!("Error: cannot connect to rosbridge at {url} — {e}")),
        };

        let call_args = json!({"pose": pose, "duration": duration, "interrupt": true});
        tracing::info!(service = %service, pose, duration, "rospider: builtin_pose");
        let resp = match client
            .call_service(
                &service,
                Some("kinematics_msgs/srv/SetPose1"),
                call_args,
                Duration::from_secs_f64(BUILTIN_POSE_SERVICE_TIMEOUT_SECS),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(format!(
                    "Error: call_service '{service}' failed at {url} — {e}"
                ));
            }
        };
        let result = resp
            .values
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let message = resp
            .values
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        Ok(format!(
            "builtin_pose {pose}: result={result} message={message}"
        ))
    }

    /// Shared single-shot publish path for `set_pose`/`body_height`/`gait` —
    /// advertise-then-publish once, mirroring [`super::ros2_tool::Ros2Tool::do_publish`]'s
    /// non-motion branch (49.8-01's settle delay covers DDS discovery for a
    /// one-shot message).
    async fn publish_once(
        &self,
        cfg: &ironhermes_core::Config,
        topic: &str,
        msg_type: &str,
        msg: serde_json::Value,
        notes: Vec<ClampNote>,
    ) -> anyhow::Result<String> {
        let Some(url) = self.source.bridge_url(cfg) else {
            return Ok(format!(
                "Error: no rosbridge URL configured — set {BRIDGE_URL_ENV} or ros2.bridge_url in config.yaml"
            ));
        };
        if let Err(e) = validate_bridge_url(&url) {
            return Ok(format!("Error: {e}"));
        }
        let client = match self.session.client_for(&url).await {
            Ok(c) => c,
            Err(e) => return Ok(format!("Error: cannot connect to rosbridge at {url} — {e}")),
        };
        if let Err(e) = client.advertise(topic, msg_type).await {
            return Ok(format!(
                "Error: advertise failed for '{topic}' at {url} — {e}"
            ));
        }
        tracing::info!(
            topic,
            msg_type,
            clamp_notes = notes.len(),
            "rospider: publish"
        );
        if let Err(e) = client.publish(topic, msg).await {
            return Ok(format!(
                "Error: publish failed for '{topic}' at {url} — {e}"
            ));
        }

        let mut out = format!("Published 1 message to {topic} [{msg_type}].");
        for note in &notes {
            out.push_str(&format!("\nClamped: {note}"));
        }
        Ok(out)
    }

    // -----------------------------------------------------------------
    // Camera pan
    // -----------------------------------------------------------------

    async fn do_camera_pan(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let servo_id = cfg.ros2.rospider.camera_pan_servo_id;
        if servo_id == 0 || ROSPIDER_LEG_SERVO_IDS.contains(&servo_id) {
            return Ok(format!(
                "Error: servo id {servo_id} is a leg servo — refusing; set ros2.rospider.camera_pan_servo_id to the camera servo"
            ));
        }
        let topic = match target_or_error("servo_topic", &cfg.ros2.rospider.servo_topic) {
            Ok(t) => t,
            Err(e) => return Ok(e),
        };

        let mut notes = Vec::new();
        let degrees = clamp_field(
            &mut notes,
            "degrees",
            args["degrees"].as_f64().unwrap_or(0.0),
            -ROSPIDER_PAN_MAX_DEG,
            ROSPIDER_PAN_MAX_DEG,
        );
        let duration = args["duration"]
            .as_f64()
            .unwrap_or(ROSPIDER_CAMERA_PAN_DURATION_DEFAULT_SECS)
            .clamp(
                ROSPIDER_CAMERA_PAN_DURATION_MIN_SECS,
                ROSPIDER_CAMERA_PAN_DURATION_MAX_SECS,
            );

        let pulse_f = ROSPIDER_PAN_CENTER_PULSE as f64 + degrees * ROSPIDER_PULSES_PER_DEG;
        let pulse =
            (pulse_f.round() as i64).clamp(ROSPIDER_PAN_PULSE_RANGE.0, ROSPIDER_PAN_PULSE_RANGE.1);

        let msg = json!({
            "duration": duration,
            "position_unit": "pulse",
            "position": [{"id": servo_id, "position": pulse}],
        });

        let out = self
            .publish_once(
                &cfg,
                &topic,
                "servo_controller_msgs/msg/ServosPosition",
                msg,
                notes,
            )
            .await?;
        Ok(format!(
            "{out}\nNote: confirm the camera pan direction (left/right) on the physical robot before relying on it."
        ))
    }

    // -----------------------------------------------------------------
    // Sensors / camera reads (open per D-04 — no allowlist)
    // -----------------------------------------------------------------

    /// Samples exactly one message on `topic` (already-known `msg_type`),
    /// waiting up to the EFFECTIVE window —
    /// `clamp_subscribe_window(Some(SENSOR_WINDOW_SECS), caps)`, the same
    /// clamp the generic `ros2` tool's `subscribe` action uses (VERIFICATION
    /// gap #4 / WR-01) — so `ros2.caps.max_subscribe_secs` can only shorten
    /// [`SENSOR_WINDOW_SECS`], never widen it. `Err` (never prefixed with
    /// `"Error:"` — callers add that) names the topic and the effective
    /// window on a timeout, or the underlying bridge/connection failure
    /// otherwise.
    async fn sample_one(
        &self,
        cfg: &ironhermes_core::Config,
        topic: &str,
        msg_type: &str,
    ) -> Result<serde_json::Value, String> {
        // D-08 gap closure: an explicit ros2.caps.max_subscribe_messages: 0
        // disables every sensor/camera read — refuse before any URL
        // resolution or connection.
        let caps = EffectiveCaps::from_config(&cfg.ros2.caps);
        if clamp_subscribe_count(Some(1), &caps).is_none() {
            return Err(
                "reading sensors and cameras is not permitted: ros2.caps.max_subscribe_messages is 0 (sampling disabled by the operator)"
                    .to_string(),
            );
        }
        let window_secs = clamp_subscribe_window(Some(SENSOR_WINDOW_SECS), &caps);

        let Some(url) = self.source.bridge_url(cfg) else {
            return Err(format!(
                "no rosbridge URL configured — set {BRIDGE_URL_ENV} or ros2.bridge_url in config.yaml"
            ));
        };
        validate_bridge_url(&url)?;
        let client = self
            .session
            .client_for(&url)
            .await
            .map_err(|e| format!("cannot connect to rosbridge at {url} — {e}"))?;

        let req = SubscribeRequest {
            topic: topic.to_string(),
            msg_type: Some(msg_type.to_string()),
            max_messages: 1,
            window: Duration::from_secs_f64(window_secs),
            prompt: None,
        };
        let sample = read_ops::subscribe_sample(&client, &req)
            .await
            .map_err(|e| e.to_string())?;
        sample.messages.into_iter().next().ok_or_else(|| {
            format!(
                "no message on {topic} within {window_secs}s — check the name with the ros2 tool's list_topics"
            )
        })
    }

    async fn do_read_imu(&self) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let topic = match target_or_error("imu_topic", &cfg.ros2.rospider.imu_topic) {
            Ok(t) => t,
            Err(e) => return Ok(e),
        };
        let msg = match self.sample_one(&cfg, &topic, "sensor_msgs/msg/Imu").await {
            Ok(m) => m,
            Err(e) => return Ok(format!("Error: {e}")),
        };

        let ori = &msg["orientation"];
        let (roll, pitch, yaw) = quaternion_to_rpy(
            ori["x"].as_f64().unwrap_or(0.0),
            ori["y"].as_f64().unwrap_or(0.0),
            ori["z"].as_f64().unwrap_or(0.0),
            ori["w"].as_f64().unwrap_or(1.0),
        );
        let av = &msg["angular_velocity"];
        let la = &msg["linear_acceleration"];
        Ok(format!(
            "IMU: roll {:.1}° pitch {:.1}° yaw {:.1}°; angular velocity ({:.3},{:.3},{:.3}) rad/s; \
             linear acceleration ({:.3},{:.3},{:.3}) m/s²",
            roll.to_degrees(),
            pitch.to_degrees(),
            yaw.to_degrees(),
            av["x"].as_f64().unwrap_or(0.0),
            av["y"].as_f64().unwrap_or(0.0),
            av["z"].as_f64().unwrap_or(0.0),
            la["x"].as_f64().unwrap_or(0.0),
            la["y"].as_f64().unwrap_or(0.0),
            la["z"].as_f64().unwrap_or(0.0),
        ))
    }

    async fn do_read_battery(&self) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let topic = match target_or_error("battery_topic", &cfg.ros2.rospider.battery_topic) {
            Ok(t) => t,
            Err(e) => return Ok(e),
        };
        let msg = match self.sample_one(&cfg, &topic, "std_msgs/msg/UInt16").await {
            Ok(m) => m,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let millivolts = msg["data"].as_f64().unwrap_or(0.0);
        Ok(format!(
            "Battery: {:.2} V ({} mV, as reported by the ROSPider driver)",
            millivolts / 1000.0,
            millivolts as i64
        ))
    }

    async fn do_read_odom(&self) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let topic = match target_or_error("odom_topic", &cfg.ros2.rospider.odom_topic) {
            Ok(t) => t,
            Err(e) => return Ok(e),
        };
        let msg = match self.sample_one(&cfg, &topic, "nav_msgs/msg/Odometry").await {
            Ok(m) => m,
            Err(e) => return Ok(format!("Error: {e}")),
        };

        let pos = &msg["pose"]["pose"]["position"];
        let ori = &msg["pose"]["pose"]["orientation"];
        let (_, _, yaw) = quaternion_to_rpy(
            ori["x"].as_f64().unwrap_or(0.0),
            ori["y"].as_f64().unwrap_or(0.0),
            ori["z"].as_f64().unwrap_or(0.0),
            ori["w"].as_f64().unwrap_or(1.0),
        );
        let lin = &msg["twist"]["twist"]["linear"];
        let ang = &msg["twist"]["twist"]["angular"];
        let vx = lin["x"].as_f64().unwrap_or(0.0);
        let vy = lin["y"].as_f64().unwrap_or(0.0);
        let v = (vx * vx + vy * vy).sqrt();
        Ok(format!(
            "Odometry: x {:.2} m, y {:.2} m, yaw {:.1}°; v {:.2} m/s, ω {:.2} rad/s",
            pos["x"].as_f64().unwrap_or(0.0),
            pos["y"].as_f64().unwrap_or(0.0),
            yaw.to_degrees(),
            v,
            ang["z"].as_f64().unwrap_or(0.0),
        ))
    }

    async fn do_lidar_summary(&self) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let topic = match target_or_error("scan_topic", &cfg.ros2.rospider.scan_topic) {
            Ok(t) => t,
            Err(e) => return Ok(e),
        };
        let msg = match self
            .sample_one(&cfg, &topic, "sensor_msgs/msg/LaserScan")
            .await
        {
            Ok(m) => m,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        Ok(crate::ros2::summarize::summarize_laserscan(&msg))
    }

    /// D-13/D-15: samples the compressed camera topic first (preferring it —
    /// smaller over the wire); if no frame arrives within the window, falls
    /// back to the raw topic. Either frame is handed to
    /// [`read_ops::describe_frame`] — the frame itself never appears in the
    /// returned text.
    async fn do_camera_snapshot(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let prompt = args["prompt"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let compressed_topic = cfg.ros2.rospider.camera_compressed_topic.trim().to_string();
        let raw_topic = cfg.ros2.rospider.camera_raw_topic.trim().to_string();

        if !compressed_topic.is_empty()
            && let Ok(msg) = self
                .sample_one(&cfg, &compressed_topic, "sensor_msgs/msg/CompressedImage")
                .await
        {
            return Ok(read_ops::describe_frame(
                "sensor_msgs/msg/CompressedImage",
                &msg,
                &compressed_topic,
                prompt.as_deref(),
                self.vision.as_ref(),
            )
            .await);
        }

        if raw_topic.is_empty() {
            return Ok(
                "Error: no camera topic configured — set ros2.rospider.camera_compressed_topic \
                 or ros2.rospider.camera_raw_topic"
                    .to_string(),
            );
        }

        match self
            .sample_one(&cfg, &raw_topic, "sensor_msgs/msg/Image")
            .await
        {
            Ok(msg) => Ok(read_ops::describe_frame(
                "sensor_msgs/msg/Image",
                &msg,
                &raw_topic,
                prompt.as_deref(),
                self.vision.as_ref(),
            )
            .await),
            Err(e) => Ok(format!("Error: {e}")),
        }
    }
}

/// G-49.8-2b: the exact `kinematics_msgs/msg/Traveling` gait-0 rest-stop
/// message. A code constant — no model-supplied argument reaches any field
/// of it (D-08/D-11).
pub fn rospider_rest_stop_msg() -> serde_json::Value {
    json!({
        "gait": ROSPIDER_REST_STOP_GAIT,
        "stride": 0.0,
        "height": 0.0,
        "direction": 0.0,
        "rotation": 0.0,
        "time": ROSPIDER_REST_STOP_POSE_SECS,
        "steps": 0,
        "relative_height": false,
        "interrupt": false,
    })
}

/// G-49.8-2b: the gait-0 rest stop follow-up, targeting the operator's
/// configured `ros2.rospider.traveling_topic` (D-14 — rospider-owned, never
/// `ros2.publish`). `None` when the trimmed topic is empty — the operator's
/// explicit, documented way to disable the rest stop.
pub fn rospider_rest_stop(cfg: &ironhermes_core::Config) -> Option<StopFollowup> {
    let topic = cfg.ros2.rospider.traveling_topic.trim();
    if topic.is_empty() {
        return None;
    }
    Some(StopFollowup {
        topic: topic.to_string(),
        msg_type: ROSPIDER_TRAVELING_TYPE.to_string(),
        msg: rospider_rest_stop_msg(),
    })
}

/// G-49.8-2b: `Some` iff `motion_topic` is exactly the trimmed, non-empty
/// `ros2.rospider.cmd_vel_topic` AND [`rospider_rest_stop`] is `Some` — the
/// rest stop only attaches to motion on the ROSPider's own cmd_vel topic,
/// never to an arbitrary generic-tool motion.
pub fn rospider_stop_followup(
    cfg: &ironhermes_core::Config,
    motion_topic: &str,
) -> Option<StopFollowup> {
    let cmd_vel_topic = cfg.ros2.rospider.cmd_vel_topic.trim();
    if cmd_vel_topic.is_empty() || motion_topic != cmd_vel_topic {
        return None;
    }
    rospider_rest_stop(cfg)
}

/// D-15: reads a `ros2.rospider.{key}` target string — an empty value
/// disables the verb rather than sending to an accidental empty topic name.
fn target_or_error(key: &str, value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(format!(
            "Error: ros2.rospider.{key} is empty — this verb is disabled"
        ))
    } else {
        Ok(trimmed.to_string())
    }
}

/// Clamps `requested` into `[min, max]`, appending a [`ClampNote`] to `notes`
/// (D-08/D-11: every clamp is visible, never silent) when the clamp actually
/// changed the value.
fn clamp_field(notes: &mut Vec<ClampNote>, path: &str, requested: f64, min: f64, max: f64) -> f64 {
    let applied = requested.clamp(min, max);
    if (applied - requested).abs() > f64::EPSILON {
        notes.push(ClampNote {
            path: path.to_string(),
            requested,
            applied,
        });
    }
    applied
}

/// Standard atan2/asin quaternion -> roll/pitch/yaw (radians), REP-103 axis
/// convention. The pitch argument is clamped to `[-1, 1]` before `asin` so a
/// slightly-off-normalized quaternion near the gimbal-lock pole never panics
/// on a NaN.
pub fn quaternion_to_rpy(x: f64, y: f64, z: f64, w: f64) -> (f64, f64, f64) {
    let sinr_cosp = 2.0 * (w * x + y * z);
    let cosr_cosp = 1.0 - 2.0 * (x * x + y * y);
    let roll = sinr_cosp.atan2(cosr_cosp);

    let sinp = (2.0 * (w * y - z * x)).clamp(-1.0, 1.0);
    let pitch = sinp.asin();

    let siny_cosp = 2.0 * (w * z + x * y);
    let cosy_cosp = 1.0 - 2.0 * (y * y + z * z);
    let yaw = siny_cosp.atan2(cosy_cosp);

    (roll, pitch, yaw)
}

#[async_trait]
impl Tool for RospiderTool {
    fn name(&self) -> &str {
        "rospider"
    }

    fn toolset(&self) -> &str {
        ROS2_TOOLSET
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "rospider",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "walk", "turn", "stop", "set_pose", "body_height", "gait",
                            "builtin_pose", "camera_pan", "read_imu", "read_battery",
                            "read_odom", "lidar_summary", "camera_snapshot"
                        ],
                        "description": "Action to perform."
                    },
                    "direction": {
                        "type": "string",
                        "description": "walk: forward|backward|left|right (default forward). turn: left|right (default left)."
                    },
                    "speed": {
                        "type": "number",
                        "description": "walk only: m/s, default 0.05, further clamped by ros2.caps.max_linear_velocity."
                    },
                    "rate": {
                        "type": "number",
                        "description": "turn only: rad/s, default 0.3, further clamped by ros2.caps.max_angular_velocity."
                    },
                    "duration_secs": {
                        "type": "number",
                        "description": "walk/turn only: seconds, clamped by ros2.caps.max_motion_duration_secs (hard ceiling 5s)."
                    },
                    "roll": {"type": "number", "description": "set_pose only: radians, clamped ±0.2."},
                    "pitch": {"type": "number", "description": "set_pose only: radians, clamped ±0.2."},
                    "yaw": {"type": "number", "description": "set_pose only: radians, clamped ±0.4."},
                    "x": {"type": "number", "description": "set_pose only: mm offset, clamped ±20."},
                    "y": {"type": "number", "description": "set_pose only: mm offset, clamped ±20."},
                    "height": {
                        "type": "number",
                        "description": "set_pose/body_height: mm body height, clamped 100-160 (default 120)."
                    },
                    "gait": {"type": "string", "description": "gait only: ripple|tripod (default tripod)."},
                    "step_height": {"type": "number", "description": "gait only: mm, clamped 10-40 (default 20)."},
                    "period": {"type": "number", "description": "gait only: seconds, clamped 0.5-2.0 (default 1.0)."},
                    "pose": {
                        "type": "string",
                        "description": "gait/builtin_pose: one of DEFAULT_POSE, DEFAULT_POSE_M, NARROW_POSE, SLAM_POSE, HOLE_POSE, SHRINK_POSE, SIDE_SHIFT_POSE (default DEFAULT_POSE)."
                    },
                    "duration": {
                        "type": "number",
                        "description": "builtin_pose: seconds, clamped 0.5-3.0 (default 1.0). camera_pan: seconds, clamped 0.1-2.0 (default 0.5)."
                    },
                    "degrees": {"type": "number", "description": "camera_pan only: clamped ±72 (0 = centre)."},
                    "prompt": {
                        "type": "string",
                        "description": "camera_snapshot only: what to ask the vision model to look for."
                    }
                },
                "required": ["action"]
            }),
        )
    }

    fn prerequisites(&self) -> Vec<Prerequisite> {
        vec![
            Prerequisite::grouped_env_var(
                BRIDGE_URL_ENV,
                "rosbridge WebSocket URL, e.g. ws://127.0.0.1:9090. Required for rospider to reach the robot.",
                "ros2_bridge",
            ),
            Prerequisite {
                kind: "config_field".to_string(),
                name: "ros2.bridge_url".to_string(),
                description: "ros2.bridge_url in config.yaml — the rosbridge WebSocket URL."
                    .to_string(),
                required: false,
                group: Some("ros2_bridge".to_string()),
            },
        ]
    }

    fn credentials(&self) -> Option<&crate::credentials::ToolCredentials> {
        Some(&self.creds)
    }

    /// Same shape as `Ros2Tool::is_available` — hides the tool (rather than
    /// warning on every `get_definitions()` call) when no bridge URL
    /// resolves.
    fn is_available(&self) -> bool {
        match &self.source {
            Ros2ConfigSource::Fixed(cfg) => cfg
                .ros2
                .bridge_url
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false),
            Ros2ConfigSource::Live => {
                std::env::var(BRIDGE_URL_ENV)
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false)
                    || self.creds.config_field_present("ros2.bridge_url")
            }
        }
    }

    fn timeout_secs(&self) -> Option<u64> {
        Some(ROS2_TOOL_TIMEOUT_SECS)
    }

    /// D-10/G-49.8-2b: identical shape to `Ros2Tool::on_session_end` — both
    /// tools share the same [`Ros2Session`], so this and the generic tool's
    /// own hook converge on the one `session_end_hook_stop` (49.8-16), which
    /// also replays the gait-0 rest stop recorded for any ROSPider motion
    /// topic this process actually drove (the second call typically
    /// coalesces with the first, or reports "nothing to stop" on a session
    /// that never drove anything).
    fn on_session_end(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            tracing::warn!(
                "rospider: on_session_end called outside a tokio runtime — skipping stop"
            );
            return;
        }
        let session = self.session.clone();
        let source = self.source.clone();
        tokio::spawn(async move {
            let cfg = source.load();
            let url = source.bridge_url(&cfg);
            let report = session.session_end_hook_stop(url.as_deref()).await;
            tracing::info!(report = %report, "rospider: on_session_end stop");
        });
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let action = args["action"].as_str().unwrap_or("");
        let result = match action {
            "walk" => self.do_walk(&args).await,
            "turn" => self.do_turn(&args).await,
            "stop" => self.do_stop().await,
            "set_pose" => self.do_set_pose(&args).await,
            "body_height" => self.do_body_height(&args).await,
            "gait" => self.do_gait(&args).await,
            "builtin_pose" => self.do_builtin_pose(&args).await,
            "camera_pan" => self.do_camera_pan(&args).await,
            "read_imu" => self.do_read_imu().await,
            "read_battery" => self.do_read_battery().await,
            "read_odom" => self.do_read_odom().await,
            "lidar_summary" => self.do_lidar_summary().await,
            "camera_snapshot" => self.do_camera_snapshot(&args).await,
            other => Ok(format!(
                "Action '{other}' is blocked — not permitted via rospider. Never send this command."
            )),
        };
        match result {
            Ok(s) => Ok(crate::ros2::summarize::cap_result(s)),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ros2_rospider_quaternion_to_rpy() {
        let (roll, pitch, yaw) = quaternion_to_rpy(0.0, 0.0, 0.0, 1.0);
        assert!(roll.abs() < 1e-9, "identity roll: got {roll}");
        assert!(pitch.abs() < 1e-9, "identity pitch: got {pitch}");
        assert!(yaw.abs() < 1e-9, "identity yaw: got {yaw}");

        // 90 degree yaw quaternion: z = sin(45deg), w = cos(45deg).
        let half = std::f64::consts::FRAC_PI_4.sin();
        let w = std::f64::consts::FRAC_PI_4.cos();
        let (_, _, yaw90) = quaternion_to_rpy(0.0, 0.0, half, w);
        assert!(
            (yaw90 - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "expected yaw ~= pi/2; got {yaw90}"
        );
    }

    // -----------------------------------------------------------------
    // 49.8-13: gait-0 rest stop (G-49.8-2b)
    // -----------------------------------------------------------------

    #[test]
    fn ros2_rospider_rest_stop_msg_is_gait_zero() {
        let msg = rospider_rest_stop_msg();
        assert_eq!(
            msg,
            json!({
                "gait": 0,
                "stride": 0.0,
                "height": 0.0,
                "direction": 0.0,
                "rotation": 0.0,
                "time": 1.0,
                "steps": 0,
                "relative_height": false,
                "interrupt": false,
            })
        );
    }

    #[test]
    fn ros2_rospider_stop_followup_only_for_rospider_cmd_vel() {
        let cfg = ironhermes_core::Config::default();
        let followup = rospider_stop_followup(&cfg, "/controller/cmd_vel").expect(
            "default config must attach a rest-stop follow-up on the default cmd_vel topic",
        );
        assert_eq!(followup.topic, "/controller/traveling");
        assert_eq!(followup.msg_type, ROSPIDER_TRAVELING_TYPE);
        assert_eq!(followup.msg, rospider_rest_stop_msg());

        assert!(
            rospider_stop_followup(&cfg, "/cmd_vel").is_none(),
            "a topic other than the configured cmd_vel_topic must not get a follow-up"
        );

        let mut empty_traveling = ironhermes_core::Config::default();
        empty_traveling.ros2.rospider.traveling_topic = "  ".to_string();
        assert!(
            rospider_rest_stop(&empty_traveling).is_none(),
            "a blank traveling_topic must disable the rest stop"
        );
        assert!(rospider_stop_followup(&empty_traveling, "/controller/cmd_vel").is_none());

        let mut empty_cmd_vel = ironhermes_core::Config::default();
        empty_cmd_vel.ros2.rospider.cmd_vel_topic = "".to_string();
        assert!(
            rospider_stop_followup(&empty_cmd_vel, "").is_none(),
            "an empty cmd_vel_topic must never match, even against an empty motion_topic"
        );
        assert!(
            rospider_rest_stop(&empty_cmd_vel).is_some(),
            "rospider_rest_stop itself is independent of cmd_vel_topic"
        );

        let mut padded_cmd_vel = ironhermes_core::Config::default();
        padded_cmd_vel.ros2.rospider.cmd_vel_topic = " /controller/cmd_vel ".to_string();
        assert!(
            rospider_stop_followup(&padded_cmd_vel, "/controller/cmd_vel").is_some(),
            "cmd_vel_topic must be trimmed before comparison"
        );
    }
}
