//! `Ros2Tool` — the generic `ros2` action-enum tool (Phase 49.8 D-05).
//!
//! Mirrors `hexapod_tcp.rs`'s shape: a single `Tool` impl whose `execute()`
//! matches on an `"action"` string, an `Ok("… is not permitted …")` string
//! (never `Err`) for every blocked or undeclared write, and clamp-then-send
//! (never reject) for physical parameters once 49.8-04 lands the motion path.
//!
//! This plan implements `list_topics` and `publish`. `publish` is D-04's
//! allowlist-before-I/O gate: the config lookup happens BEFORE the bridge URL
//! is resolved or any socket opens, so an undeclared or type-mismatched write
//! never reaches the network (`test_ros2_undeclared_publish_blocked`,
//! `ros2_type_mismatch_blocked`). Twist-family topics are refused fail-closed
//! until 49.8-04 replaces that branch with the timed-motion engine.
//!
//! 49.8-05 adds `subscribe` (D-12 bounded sample) and the six rosapi
//! introspection actions (`list_services`/`list_nodes`/`list_actions`/
//! `topic_type`/`service_type`/`message_details`) — every read may target
//! any name (D-04), so none of these consult the write allowlists. The
//! shared implementation lives in [`crate::ros2::read_ops`].

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::time::Duration;

use crate::registry::{Prerequisite, Tool};
use crate::ros2::action_ops;
use crate::ros2::read_ops::{ReadContext, SubscribeRequest, introspect, subscribe_action};
use crate::ros2::safety::{
    EffectiveCaps, HARD_MAX_SERVICE_TIMEOUT_SECS, apply_field_limits, clamp_subscribe_count,
    clamp_subscribe_window,
};
use crate::ros2::{
    BRIDGE_URL_ENV, ROS2_TOOL_TIMEOUT_SECS, ROS2_TOOLSET, Ros2ConfigSource, Ros2Session,
    canonical_type, find_allow_entry, is_twist_family, not_permitted, validate_bridge_url,
};

const DESCRIPTION: &str = "Talk to a ROS 2 robot over a rosbridge WebSocket. \
    Actions: list_topics (no parameters — lists every topic and type the robot's \
    rosapi reports), subscribe (with topic; type optional (resolved via rosapi \
    when omitted), max_messages, timeout_secs, prompt — a BOUNDED sample: \
    subscribes, collects up to max_messages messages or until timeout_secs \
    elapses, then always unsubscribes and returns; never a background \
    subscription), list_services / list_nodes / list_actions (no parameters), \
    topic_type (with topic), service_type (with service), message_details \
    (with msg_type_name), publish (with topic, type, and msg — publish one \
    message to a topic; add duration_secs for a Twist-family motion topic), \
    call_service (with service, type, args, and timeout_secs — call a declared \
    ROS 2 service), send_goal (with action_name, type, and goal — send a \
    declared ROS 2 action goal, e.g. Nav2 NavigateToPose), goal_status (with \
    goal_id and wait_secs up to 20 — poll a goal's status/feedback/result), \
    cancel_goal (with goal_id — cancel a goal and wait for its terminal \
    status; a refused, not confirmed, or merely-accepted-but-not-yet-terminal \
    cancel is reported as such, not as CANCELED), stop (no parameters — \
    immediately zero every motion topic this \
    session has driven and cancel every unfinished action goal; also runs \
    automatically when the session ends). Reads (list_topics, subscribe, and \
    every introspection action) may target any name — no declaration \
    required. Writes (publish, call_service, send_goal) are permitted ONLY \
    for names the operator has declared under ros2.publish / ros2.services / \
    ros2.actions in config.yaml, together with their exact message/service/ \
    action type — an undeclared or type-mismatched write returns an 'is not \
    permitted' answer, which is final; do not retry it. A declared entry's \
    configured numeric field limits are clamped, never rejected — the result \
    names every clamp. Motion-producing message types (geometry_msgs \
    Twist/TwistStamped) always publish through a capped, timed engine: \
    velocity is clamped to a hard ceiling (config can only lower it, never \
    raise it), the message republishes for a capped duration (about 5s max), \
    and the robot always receives a zero-velocity stop when the call returns — \
    even if the call is cancelled or a new motion preempts it; on the topic \
    configured as ros2.rospider.cmd_vel_topic, that zero-velocity stop is \
    followed by the ROSPider's gait-0 rest stop (G-49.8-2b), since a zero \
    Twist alone leaves that robot's legs stepping in place. Every action \
    goal is watchdog-bounded (about 120s default, 300s hard ceiling) — a goal \
    with no terminal status after that long is cancelled automatically. \
    stop and every session end cancel every unfinished action goal and \
    report any cancel the robot refused or did not confirm. Every result is \
    capped at 50,000 characters and large sensor arrays \
    (LaserScan, PointCloud2) come back summarized, never raw. For the \
    Hiwonder ROSPider hexapod, prefer the higher-level rospider tool, which \
    wraps this one with robot-specific verbs.";

/// The generic `ros2` tool (D-05). One struct, stateless per call except for
/// the shared, persistent [`Ros2Session`] connection (Claude's Discretion,
/// recorded in `49.8-01-PLAN.md`).
pub struct Ros2Tool {
    session: Arc<Ros2Session>,
    source: Ros2ConfigSource,
    creds: Arc<crate::credentials::ToolCredentials>,
    /// D-13: the vision handle `subscribe`'s [`ReadContext`] carries. 49.8-06
    /// is the first plan to actually read it (camera-topic image branch);
    /// this plan (49.8-05) only threads it through so `ros2_tool.rs` never
    /// needs to change again for that later plan.
    vision: Option<Arc<dyn crate::browser_vision::VisionClientHandle>>,
}

impl Ros2Tool {
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

    /// `list_topics`/`list_services`/`list_nodes`/`list_actions`/
    /// `topic_type`/`service_type`/`message_details` — every rosapi
    /// introspection action shares this connect-then-dispatch shape. No
    /// allowlist check: reads may target any name (D-04).
    async fn do_introspect(
        &self,
        action: &str,
        args: &serde_json::Value,
    ) -> anyhow::Result<String> {
        let cfg = self.source.load();
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
        Ok(introspect(&client, action, args).await)
    }

    /// `subscribe` (D-12) — a bounded sample: subscribe, collect up to
    /// `max_messages` messages or until `timeout_secs` elapses, then
    /// unsubscribe and return. No allowlist check: reads may target any
    /// name (D-04). `max_messages`/`timeout_secs` are clamped against
    /// [`EffectiveCaps`] BEFORE the bridge URL is resolved or any socket
    /// opens.
    async fn do_subscribe(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let topic = match args["topic"].as_str() {
            Some(t) if !t.is_empty() => t,
            _ => {
                return Ok(
                    "Error: 'topic' parameter is required for the subscribe action".to_string(),
                );
            }
        };

        let cfg = self.source.load();
        let caps = EffectiveCaps::from_config(&cfg.ros2.caps);

        // D-12: max_messages/window are both hard-capped here — the operator
        // may request more, but the tool never samples more than
        // caps.max_subscribe_messages or for longer than caps.max_subscribe_secs.
        // D-08 gap closure: an explicit ros2.caps.max_subscribe_messages: 0
        // disables sampling entirely — refuse before the bridge URL is even
        // resolved, let alone a connection opened.
        let Some(max_messages) = clamp_subscribe_count(args["max_messages"].as_u64(), &caps) else {
            return Ok(
                "subscribe is not permitted: ros2.caps.max_subscribe_messages is 0 (sampling disabled by the operator)"
                    .to_string(),
            );
        };
        let timeout_secs = clamp_subscribe_window(args["timeout_secs"].as_f64(), &caps);

        let msg_type = args["type"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let prompt = args["prompt"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string);

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

        let req = SubscribeRequest {
            topic: topic.to_string(),
            msg_type,
            max_messages,
            window: Duration::from_secs_f64(timeout_secs),
            prompt,
        };
        let ctx = ReadContext {
            caps,
            vision: self.vision.clone(),
        };
        Ok(subscribe_action(&client, req, &ctx).await)
    }

    /// `publish` — sends one message on a declared topic (D-04). The
    /// `ros2.publish` allowlist is checked BEFORE the bridge URL is resolved
    /// or any socket opens: a miss, a declared-type mismatch, or a
    /// Twist-family declared type all return an `Ok` string and never reach
    /// the network (`test_ros2_undeclared_publish_blocked` asserts zero
    /// connections; `ros2_type_mismatch_blocked` and
    /// `ros2_motion_type_publish_fails_closed` assert zero publish frames).
    async fn do_publish(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let topic = match args["topic"].as_str() {
            Some(t) if !t.is_empty() => t,
            _ => {
                return Ok(
                    "Error: 'topic' parameter is required for the publish action".to_string(),
                );
            }
        };

        let cfg = self.source.load();
        // D-04: allowlist-before-I/O — the exact-match lookup happens before
        // the bridge URL is resolved or any connection opens.
        let entry = match find_allow_entry(&cfg.ros2.publish, topic) {
            Some(e) => e,
            None => return Ok(not_permitted("publish", topic)),
        };

        if let Some(supplied_type) = args["type"].as_str()
            && canonical_type(supplied_type) != canonical_type(&entry.msg_type)
        {
            return Ok(not_permitted("publish", topic));
        }

        let mut msg = match &args["msg"] {
            serde_json::Value::Object(_) => args["msg"].clone(),
            serde_json::Value::String(s) => match serde_json::from_str::<serde_json::Value>(s) {
                Ok(v) => v,
                Err(_) => {
                    return Ok(format!(
                        "Error: 'msg' string for '{topic}' did not parse as JSON"
                    ));
                }
            },
            serde_json::Value::Null => {
                return Ok("Error: 'msg' parameter is required for the publish action".to_string());
            }
            _ => {
                return Ok(
                    "Error: 'msg' must be a JSON object (or a string that parses to one)"
                        .to_string(),
                );
            }
        };

        // D-08/D-09: Twist-family motion topics go through the timed-motion
        // engine — every publish is capped, republished for a capped
        // duration, and always ends in a guaranteed zero-velocity stop, even
        // through cancellation, preemption, or a mid-motion bridge drop.
        // D-11: this branch never consults the yolo flag or the
        // approval/bypass helper — caps and auto-stop are physical safety.
        if is_twist_family(&entry.msg_type) {
            return self.do_motion_publish(topic, entry, msg, args, &cfg).await;
        }

        // D-08: a declared entry's own `limits` clamp the named dotted-path
        // numeric fields — clamp-then-send, never reject.
        let notes = apply_field_limits(&mut msg, &entry.limits);

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
        if let Err(e) = client.advertise(topic, &entry.msg_type).await {
            return Ok(format!(
                "Error: advertise failed for '{topic}' at {url} — {e}"
            ));
        }
        tracing::info!(
            topic,
            msg_type = %entry.msg_type,
            clamp_notes = notes.len(),
            "ros2: publish"
        );
        if let Err(e) = client.publish(topic, msg).await {
            return Ok(format!(
                "Error: publish failed for '{topic}' at {url} — {e}"
            ));
        }

        let mut out = format!("Published 1 message to {topic} [{}].", entry.msg_type);
        for note in &notes {
            out.push_str(&format!("\nClamped: {note}"));
        }
        Ok(out)
    }

    /// D-09/D-10: the timed-motion path for a declared Twist-family topic.
    /// Every physical parameter is clamped BEFORE the motion engine ever
    /// touches the network — `apply_field_limits` (operator-declared, per
    /// entry) then `clamp_twist` (the hard velocity ceiling, which can only
    /// ever lower an operator's own limit, never raise it) then
    /// `clamp_duration`. [`crate::ros2::motion::run_timed_motion`] republishes
    /// at the capped rate for the capped duration and ALWAYS finishes with a
    /// zero-velocity stop — through cancellation (the registry's own
    /// `run_bounded` timeout drops this future, see `registry.rs`), topic
    /// preemption, and a mid-motion bridge drop.
    async fn do_motion_publish(
        &self,
        topic: &str,
        entry: &ironhermes_core::config::Ros2AllowEntry,
        mut msg: serde_json::Value,
        args: &serde_json::Value,
        cfg: &ironhermes_core::Config,
    ) -> anyhow::Result<String> {
        use crate::ros2::motion::{MotionRequest, run_timed_motion};
        use crate::ros2::safety::{ClampNote, DEFAULT_MOTION_SECS, clamp_duration, clamp_twist};

        let caps = EffectiveCaps::from_config(&cfg.ros2.caps);
        let stamped = canonical_type(&entry.msg_type) == "geometry_msgs/TwistStamped";

        // D-08: operator-declared field limits first (they can only be
        // narrower than the hard velocity ceiling), then the hard cap itself
        // — clamp_twist normalizes the whole Twist and never leaves a
        // component above caps.max_linear/max_angular, regardless of what
        // apply_field_limits did.
        let mut notes = apply_field_limits(&mut msg, &entry.limits);
        notes.extend(clamp_twist(&mut msg, &caps, stamped));

        let requested_duration = args["duration_secs"].as_f64();
        let applied_duration = clamp_duration(requested_duration, &caps);
        let baseline_duration = requested_duration.unwrap_or(DEFAULT_MOTION_SECS);
        if (baseline_duration - applied_duration).abs() > f64::EPSILON {
            notes.push(ClampNote {
                path: "duration_secs".to_string(),
                requested: baseline_duration,
                applied: applied_duration,
            });
        }

        let Some(url) = self.source.bridge_url(cfg) else {
            return Ok(format!(
                "Error: no rosbridge URL configured — set {BRIDGE_URL_ENV} or ros2.bridge_url in config.yaml"
            ));
        };
        if let Err(e) = validate_bridge_url(&url) {
            return Ok(format!("Error: {e}"));
        }

        let (lx, ly, lz, ax, ay, az) = read_twist_components(&msg, stamped);

        tracing::info!(
            topic,
            msg_type = %entry.msg_type,
            clamp_notes = notes.len(),
            duration_secs = applied_duration,
            "ros2: motion publish"
        );

        // G-49.8-2b: on the topic configured as ros2.rospider.cmd_vel_topic,
        // a generic timed motion gets the same gait-0 rest stop a rospider
        // walk/turn does — the real stop on ROSPider firmware. Any other
        // topic is unaffected: rospider_stop_followup returns None.
        let stop_followup = crate::ros2::rospider_tool::rospider_stop_followup(cfg, topic);
        let followup_topic = stop_followup.as_ref().map(|f| f.topic.clone());

        let req = MotionRequest {
            topic: topic.to_string(),
            msg_type: entry.msg_type.clone(),
            twist: msg,
            duration: Duration::from_secs_f64(applied_duration),
            hz: caps.publish_hz,
            stamped,
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
            "Motion on {topic} [{}]: linear=({lx:.3},{ly:.3},{lz:.3}) angular=({ax:.3},{ay:.3},{az:.3}) \
             for {applied_duration:.2}s ({} published, {:.2}s elapsed)",
            entry.msg_type,
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
        if outcome.stop_followup_sent == Some(false) {
            out.push_str(&format!(
                "\nError: the gait-0 rest stop on {} was not confirmed sent — the ROSPider may keep \
                 stepping in place; run the rospider stop action or the manual emergency stop in \
                 docs/robots/rospider.md",
                followup_topic.as_deref().unwrap_or("(traveling topic)")
            ));
        }
        Ok(out)
    }

    /// `call_service` (D-04) — calls one declared ROS 2 service. The
    /// `ros2.services` allowlist is checked BEFORE the bridge URL is resolved
    /// or any socket opens: a miss or a declared-type mismatch returns an
    /// `Ok` string and never reaches the network
    /// (`test_ros2_undeclared_service_blocked` asserts zero connections).
    /// Declared `limits` clamp the call's numeric `args` fields the same way
    /// `publish` does.
    async fn do_call_service(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let service = match args["service"].as_str() {
            Some(s) if !s.is_empty() => s,
            _ => {
                return Ok(
                    "Error: 'service' parameter is required for the call_service action"
                        .to_string(),
                );
            }
        };

        let cfg = self.source.load();
        // D-04: allowlist-before-I/O — the exact-match lookup happens before
        // the bridge URL is resolved or any connection opens.
        let entry = match find_allow_entry(&cfg.ros2.services, service) {
            Some(e) => e,
            None => return Ok(not_permitted("services", service)),
        };

        if let Some(supplied_type) = args["type"].as_str()
            && canonical_type(supplied_type) != canonical_type(&entry.msg_type)
        {
            return Ok(not_permitted("services", service));
        }

        let mut call_args = match &args["args"] {
            serde_json::Value::Object(_) => args["args"].clone(),
            serde_json::Value::String(s) => match serde_json::from_str::<serde_json::Value>(s) {
                Ok(v) => v,
                Err(_) => {
                    return Ok(format!(
                        "Error: 'args' string for '{service}' did not parse as JSON"
                    ));
                }
            },
            serde_json::Value::Null => json!({}),
            _ => {
                return Ok(
                    "Error: 'args' must be a JSON object (or a string that parses to one)"
                        .to_string(),
                );
            }
        };

        let notes = apply_field_limits(&mut call_args, &entry.limits);

        let timeout_secs = args["timeout_secs"]
            .as_f64()
            .unwrap_or(5.0)
            .clamp(0.5, HARD_MAX_SERVICE_TIMEOUT_SECS);

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

        tracing::info!(
            service,
            msg_type = %entry.msg_type,
            clamp_notes = notes.len(),
            "ros2: call_service"
        );
        let resp = match client
            .call_service(
                service,
                Some(&entry.msg_type),
                call_args,
                Duration::from_secs_f64(timeout_secs),
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

        // D-13: call_service values are truncated the same way a subscribed
        // message is — a service that happens to echo back a huge array or
        // string never blows past the shared result cap on its own.
        let truncated_values = crate::ros2::summarize::truncate_value(&resp.values);
        let values_str = serde_json::to_string(&truncated_values).unwrap_or_default();
        let mut out = format!(
            "Service {service} returned result={}: {values_str}",
            resp.result
        );
        for note in &notes {
            out.push_str(&format!("\nClamped: {note}"));
        }
        Ok(out)
    }

    /// `send_goal` (D-06/D-04) — the `ros2.actions` allowlist check happens
    /// INSIDE [`action_ops::send_goal`], before any connection is opened;
    /// this fn only resolves/validates the bridge URL (pure — no I/O) and
    /// the effective caps before delegating.
    async fn do_send_goal(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let caps = EffectiveCaps::from_config(&cfg.ros2.caps);
        let Some(url) = self.source.bridge_url(&cfg) else {
            return Ok(format!(
                "Error: no rosbridge URL configured — set {BRIDGE_URL_ENV} or ros2.bridge_url in config.yaml"
            ));
        };
        if let Err(e) = validate_bridge_url(&url) {
            return Ok(format!("Error: {e}"));
        }
        Ok(action_ops::send_goal(&self.session, &url, &cfg, args, &caps).await)
    }

    /// `goal_status` (D-06) — an open read: no allowlist check, since it
    /// only polls a goal this process already sent.
    async fn do_goal_status(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let cfg = self.source.load();
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
        Ok(action_ops::goal_status(&client, args).await)
    }

    /// `cancel_goal` (D-06/D-10) — cancels on the goal's own connection when
    /// it is still open, otherwise the cancel-all fallback.
    async fn do_cancel_goal(&self, args: &serde_json::Value) -> anyhow::Result<String> {
        let cfg = self.source.load();
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
        Ok(action_ops::cancel_goal(&self.session, &client, args).await)
    }

    /// `stop` (D-10) — the on-demand form of the same stop `on_session_end`
    /// runs automatically: zeroes every motion topic and cancels every
    /// unfinished goal.
    async fn do_stop(&self) -> anyhow::Result<String> {
        let cfg = self.source.load();
        let url = self.source.bridge_url(&cfg);
        Ok(self.session.stop_all(url.as_deref()).await)
    }
}

#[async_trait]
impl Tool for Ros2Tool {
    fn name(&self) -> &str {
        "ros2"
    }

    fn toolset(&self) -> &str {
        ROS2_TOOLSET
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "ros2",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "list_topics", "subscribe", "list_services", "list_nodes",
                            "list_actions", "topic_type", "service_type", "message_details",
                            "publish", "call_service", "send_goal", "goal_status",
                            "cancel_goal", "stop"
                        ],
                        "description": "Action to perform."
                    },
                    "topic": {
                        "type": "string",
                        "description": "Topic name, e.g. /chatter. Used for publish, subscribe, and topic_type."
                    },
                    "type": {
                        "type": "string",
                        "description": "Message/service type, e.g. std_msgs/msg/String. Used for publish, call_service, and (optionally) subscribe — when omitted on subscribe, the type is resolved via rosapi."
                    },
                    "msg": {
                        "type": "object",
                        "description": "The message body as a JSON object. Used only for publish."
                    },
                    "service": {
                        "type": "string",
                        "description": "Service name, e.g. /set_mode. Used for call_service and service_type."
                    },
                    "args": {
                        "type": "object",
                        "description": "Service call arguments as a JSON object (or a JSON string that parses to one). Used only for call_service; defaults to {}."
                    },
                    "timeout_secs": {
                        "type": "number",
                        "description": "For call_service: timeout in seconds, clamped 0.5-10 (default 5). For subscribe: the bounded-sample window in seconds, clamped 0.2-10 (default 3)."
                    },
                    "duration_secs": {
                        "type": "number",
                        "description": "How long to move for a Twist-family publish (motion topic), in seconds — clamped 0.1-5.0 (default 1.0, hard ceiling 5.0). The tool republishes at rate for this duration then always sends a zero-velocity stop. Used only for publish on a motion topic."
                    },
                    "max_messages": {
                        "type": "integer",
                        "description": "subscribe only: how many messages to collect before returning, clamped 1-20 (default 1)."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "subscribe only, and only used for camera topics: what to ask the vision model to look for."
                    },
                    "msg_type_name": {
                        "type": "string",
                        "description": "message_details only: the message/service/action type to describe, e.g. sensor_msgs/msg/LaserScan."
                    },
                    "action_name": {
                        "type": "string",
                        "description": "send_goal only: the declared ROS 2 action name, e.g. /navigate_to_pose."
                    },
                    "goal": {
                        "type": "object",
                        "description": "send_goal only: the goal body as a JSON object (or a JSON string that parses to one)."
                    },
                    "goal_id": {
                        "type": "string",
                        "description": "goal_status and cancel_goal only: the goal id returned by send_goal."
                    },
                    "wait_secs": {
                        "type": "number",
                        "description": "goal_status only: how long to wait for a terminal status before returning, clamped 0-20 (default 0 — an immediate snapshot)."
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
                "rosbridge WebSocket URL, e.g. ws://127.0.0.1:9090. Required for ros2 to reach the robot.",
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

    /// Overrides the trait default so the `config_field` prerequisite arm
    /// never logs a warning on every `get_definitions()` call for an operator
    /// with no robot configured — `Fixed` reads the config value directly;
    /// `Live` checks the env var (bare, not via `credentials()`) OR the
    /// resolved config-field snapshot.
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

    /// D-10: fire-and-forget safety stop when the session ends — mirrors
    /// `hexapod_tcp.rs`'s `on_session_end` shape, plus a guard `hexapod_tcp`
    /// does not need: `tokio::spawn` panics with no runtime in scope, so this
    /// checks `Handle::try_current()` FIRST and just logs+returns on a miss
    /// (e.g. a plain `#[test]` with no `#[tokio::test]`) rather than crashing
    /// the caller. No `&self` capture in the spawned future — only cloned
    /// owned handles, so the future outlives this call.
    ///
    /// 49.8-16 (G-49.8-2b): uses the hook-coalescing
    /// `Ros2Session::session_end_hook_stop` form, not `stop_all` directly —
    /// this hook fires at the same moment as an awaited session-end stop on
    /// several surfaces, and coalescing is what keeps that from resending a
    /// stop the awaited call already sent.
    fn on_session_end(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            tracing::warn!("ros2: on_session_end called outside a tokio runtime — skipping stop");
            return;
        }
        let session = self.session.clone();
        let source = self.source.clone();
        tokio::spawn(async move {
            let cfg = source.load();
            let url = source.bridge_url(&cfg);
            let report = session.session_end_hook_stop(url.as_deref()).await;
            tracing::info!(report = %report, "ros2: on_session_end stop");
        });
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let action = args["action"].as_str().unwrap_or("");
        let result = match action {
            "list_topics" | "list_services" | "list_nodes" | "list_actions" | "topic_type"
            | "service_type" | "message_details" => self.do_introspect(action, &args).await,
            "subscribe" => self.do_subscribe(&args).await,
            "publish" => self.do_publish(&args).await,
            "call_service" => self.do_call_service(&args).await,
            "send_goal" => self.do_send_goal(&args).await,
            "goal_status" => self.do_goal_status(&args).await,
            "cancel_goal" => self.do_cancel_goal(&args).await,
            "stop" => self.do_stop().await,
            other => Ok(format!(
                "Action '{other}' is blocked — not permitted via ros2. Never send this command."
            )),
        };
        // D-13: the single result-size cap, applied once around every action
        // — no branch above needs its own truncation call for the overall
        // result (large per-value content is already truncated upstream by
        // `read_ops`/`summarize`; this is the last-resort backstop).
        match result {
            Ok(s) => Ok(crate::ros2::summarize::cap_result(s)),
            Err(e) => Err(e),
        }
    }
}

/// Reads the (already-clamped) `linear`/`angular` `x`/`y`/`z` components out
/// of `msg` (or, when `stamped`, its `twist` sub-object) for display in a
/// motion result — read-only, never mutates.
fn read_twist_components(msg: &serde_json::Value, stamped: bool) -> (f64, f64, f64, f64, f64, f64) {
    let target = if stamped { &msg["twist"] } else { msg };
    let get = |group: &str, axis: &str| target[group][axis].as_f64().unwrap_or(0.0);
    (
        get("linear", "x"),
        get("linear", "y"),
        get("linear", "z"),
        get("angular", "x"),
        get("angular", "y"),
        get("angular", "z"),
    )
}
