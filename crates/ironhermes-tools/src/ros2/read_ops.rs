//! Bounded-sample `subscribe` and rosapi introspection (Phase 49.8 D-04/D-12).
//!
//! `subscribe_sample`/`subscribe_action` implement D-12's bounded-sample
//! contract: subscribe (always sending `throttle_rate`/`queue_length` — see
//! `bridge_client.rs`'s `subscribe`, RESEARCH.md Pitfall 4), collect up to
//! `max_messages` or until `window` elapses, then ALWAYS unsubscribe before
//! returning — the collection loop only ever `break`s, it never `return`s
//! past the point a `Subscription` exists, so no exit path (full sample,
//! timeout with zero messages, or a mid-collection bridge error) can skip
//! the unsubscribe.
//!
//! `introspect` answers every rosapi read (`list_topics`/`list_services`/
//! `list_nodes`/`list_actions`/`topic_type`/`service_type`/
//! `message_details`) — reads may target any name (D-04), so this fn takes
//! an already-connected client and never consults an allowlist.
//!
//! `render_frame_or_message` is the D-13 image seam: for a canonical
//! `sensor_msgs/Image`/`CompressedImage` topic it routes through
//! [`frame::describe_frame`] (convert -> internal vision-model call -> text
//! description — the frame itself never reaches the tool result); every
//! other type still delegates to [`crate::ros2::summarize::render_message`].
//! `subscribe_action` resolves an omitted topic type up front (one
//! `/rosapi/topic_type` call, same as before) so an image-family topic can be
//! capped to exactly one frame per call BEFORE the bounded-sample loop
//! starts — a 10s subscribe window stacked with a 25s vision call must never
//! run for more than one frame inside the tool's own timeout budget.

pub mod frame;

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::time::Duration;

use crate::ros2::bridge_client::{DEFAULT_SERVICE_TIMEOUT_SECS, Ros2BridgeClient};
use crate::ros2::safety::EffectiveCaps;

pub use frame::describe_frame;

/// Shared, per-call context every read action needs: the resolved caps (for
/// 49.8-06's image branch to size a snapshot request against) and the vision
/// handle (D-13) — `None` when no vision-capable model is configured, which
/// this plan never reads (49.8-06 does).
pub struct ReadContext {
    pub caps: EffectiveCaps,
    pub vision: Option<Arc<dyn crate::browser_vision::VisionClientHandle>>,
}

/// One `subscribe` action request — already clamped by the caller
/// (`Ros2Tool::do_subscribe`) against [`EffectiveCaps::max_subscribe_messages`]
/// / [`EffectiveCaps::max_subscribe_secs`] before this struct is built.
pub struct SubscribeRequest {
    pub topic: String,
    pub msg_type: Option<String>,
    pub max_messages: u32,
    pub window: Duration,
    /// D-13/49.8-06: only consulted for a camera topic — tells the vision
    /// model what to look for. Unused by every other message type.
    pub prompt: Option<String>,
}

/// The outcome of one bounded-sample subscribe.
pub struct SampleResult {
    pub topic: String,
    pub msg_type: String,
    pub messages: Vec<Value>,
    pub elapsed: Duration,
}

/// Resolves `topic`'s message type via `/rosapi/topic_type`. `Err` names the
/// topic when the service call fails OR reports an empty type (rosbridge's
/// documented "unknown topic" answer).
pub async fn resolve_topic_type(client: &Ros2BridgeClient, topic: &str) -> Result<String, String> {
    let resp = client
        .call_service(
            "/rosapi/topic_type",
            Some("rosapi_msgs/srv/TopicType"),
            json!({"topic": topic}),
            Duration::from_secs(DEFAULT_SERVICE_TIMEOUT_SECS),
        )
        .await
        .map_err(|e| format!("unknown type for {topic} — /rosapi/topic_type call failed: {e}"))?;

    if !resp.result {
        return Err(format!(
            "unknown type for {topic} — /rosapi/topic_type reported failure: {}",
            resp.values
        ));
    }
    let ty = resp
        .values
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("");
    if ty.is_empty() {
        return Err(format!("unknown type for {topic}"));
    }
    Ok(ty.to_string())
}

/// D-12: subscribe, collect up to `req.max_messages` messages or until
/// `req.window` elapses (whichever first), then unsubscribe and return.
/// Stateless per call — no background subscription survives this fn.
///
/// Once [`Ros2BridgeClient::subscribe`] has returned a live [`super::bridge_client::Subscription`],
/// every remaining step in this fn uses `break`, never `return` — so the
/// trailing `unsubscribe` below always runs, on every exit path (a full
/// sample, a timeout with zero messages, or the channel closing because the
/// connection dropped mid-collection).
pub async fn subscribe_sample(
    client: &Ros2BridgeClient,
    req: &SubscribeRequest,
) -> anyhow::Result<SampleResult> {
    let msg_type = match req.msg_type.as_deref() {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => resolve_topic_type(client, &req.topic)
            .await
            .map_err(|e| anyhow::anyhow!(e))?,
    };

    let window_ms = req.window.as_millis().max(1) as u64;
    let max_messages_u64 = u64::from(req.max_messages.max(1));
    // RESEARCH.md Pitfall 4: throttle to headroom under the client-side
    // count/window cap — rosbridge's own `throttle_rate` is advisory, this
    // client-side cap is the authoritative bound either way.
    let throttle_rate_ms = std::cmp::min(1000, window_ms / (2 * max_messages_u64)) as u32;
    let queue_length: u32 = 1;

    let mut sub = client
        .subscribe(&req.topic, Some(&msg_type), throttle_rate_ms, queue_length)
        .await?;

    let start = tokio::time::Instant::now();
    let deadline = start + req.window;
    let mut messages: Vec<Value> = Vec::with_capacity(req.max_messages as usize);

    while (messages.len() as u32) < req.max_messages {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, sub.rx.recv()).await {
            Ok(Some(msg)) => messages.push(msg),
            Ok(None) => break, // connection dropped — sender end gone
            Err(_) => break,   // window elapsed
        }
    }

    let elapsed = start.elapsed();

    // D-12: always attempt the unsubscribe, regardless of how the loop above
    // ended — a send failure here (e.g. connection already gone) is logged,
    // not propagated, since there is nothing further this fn can do about it.
    if let Err(e) = client.unsubscribe(&sub).await {
        tracing::warn!(topic = %req.topic, "ros2: unsubscribe failed: {e}");
    }

    Ok(SampleResult {
        topic: req.topic.clone(),
        msg_type,
        messages,
        elapsed,
    })
}

/// Renders one already-sampled message for the tool result. D-13: a
/// canonical `sensor_msgs/Image`/`CompressedImage` message routes through
/// [`frame::describe_frame`] — convert to a JPEG data URL, hand it to
/// `ctx.vision`'s internal vision-model call, and return the model's TEXT
/// description; the frame itself never becomes part of this function's
/// return value. Every other type delegates to
/// [`crate::ros2::summarize::render_message`] (`LaserScan`/`PointCloud2`
/// summarized, never raw; every other large array/string truncated). Kept as
/// a free `pub fn` (not a method) with this exact name and signature so the
/// rospider `camera_snapshot` verb (49.8-09) can reuse the same seam.
pub async fn render_frame_or_message(
    msg_type: &str,
    msg: &Value,
    req: &SubscribeRequest,
    ctx: &ReadContext,
) -> String {
    match crate::ros2::canonical_type(msg_type).as_str() {
        "sensor_msgs/Image" | "sensor_msgs/CompressedImage" => {
            frame::describe_frame(
                msg_type,
                msg,
                &req.topic,
                req.prompt.as_deref(),
                ctx.vision.as_ref(),
            )
            .await
        }
        _ => crate::ros2::summarize::render_message(msg_type, msg),
    }
}

/// True when `msg_type` (any `pkg/Type` or `pkg/msg/Type` form) is a
/// canonical camera-frame type D-13's image branch handles.
fn is_image_type(msg_type: &str) -> bool {
    matches!(
        crate::ros2::canonical_type(msg_type).as_str(),
        "sensor_msgs/Image" | "sensor_msgs/CompressedImage"
    )
}

/// The full `subscribe` action result string — never `Err`, matching the
/// hexapod `Ok("...")` convention. A type-resolution failure or any other
/// bridge error renders as `Error: ...` text.
///
/// The topic type is resolved up front (one `/rosapi/topic_type` call when
/// `req.msg_type` is omitted — identical to `subscribe_sample`'s own
/// resolution, which this fn short-circuits by always handing it a resolved
/// `Some` type) so an image-family topic can force `max_messages` to 1
/// BEFORE the bounded-sample loop starts (D-13: one frame per call keeps the
/// call inside the tool's own timeout budget).
pub async fn subscribe_action(
    client: &Ros2BridgeClient,
    mut req: SubscribeRequest,
    ctx: &ReadContext,
) -> String {
    let resolved_type = match req.msg_type.as_deref() {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => match resolve_topic_type(client, &req.topic).await {
            Ok(t) => t,
            Err(e) => return format!("Error: {e}"),
        },
    };
    if is_image_type(&resolved_type) {
        req.max_messages = 1;
    }
    req.msg_type = Some(resolved_type);

    let max = req.max_messages;
    match subscribe_sample(client, &req).await {
        Ok(sample) => {
            let mut out = format!(
                "Received {}/{} message(s) on {} [{}] in {:.1}s",
                sample.messages.len(),
                max,
                sample.topic,
                sample.msg_type,
                sample.elapsed.as_secs_f64()
            );
            for msg in &sample.messages {
                out.push('\n');
                out.push_str(&render_frame_or_message(&sample.msg_type, msg, &req, ctx).await);
            }
            out
        }
        Err(e) => format!("Error: {e}"),
    }
}

/// D-04: every introspection action may target any name — no allowlist
/// lookup. `client` is already connected; this fn never resolves a bridge
/// URL itself.
pub async fn introspect(client: &Ros2BridgeClient, action: &str, args: &Value) -> String {
    match action {
        "list_topics" => introspect_topics(client).await,
        "list_services" => {
            introspect_named_list(
                client,
                "/rosapi/services",
                "rosapi_msgs/srv/Services",
                "services",
                "services",
            )
            .await
        }
        "list_nodes" => {
            introspect_named_list(
                client,
                "/rosapi/nodes",
                "rosapi_msgs/srv/Nodes",
                "nodes",
                "nodes",
            )
            .await
        }
        "list_actions" => introspect_action_servers(client).await,
        "topic_type" => introspect_topic_type(client, args).await,
        "service_type" => introspect_service_type(client, args).await,
        "message_details" => introspect_message_details(client, args).await,
        other => format!("Action '{other}' is not a known introspection action."),
    }
}

async fn introspect_topics(client: &Ros2BridgeClient) -> String {
    let resp = match client
        .call_service(
            "/rosapi/topics",
            Some("rosapi_msgs/srv/Topics"),
            json!({}),
            Duration::from_secs(DEFAULT_SERVICE_TIMEOUT_SECS),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => return format!("Error: /rosapi/topics call failed — {e}"),
    };
    if !resp.result {
        return format!("Error: /rosapi/topics reported failure: {}", resp.values);
    }
    let topics = resp
        .values
        .get("topics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let types = resp
        .values
        .get("types")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if topics.is_empty() {
        return "0 topics reported.".to_string();
    }
    let mut lines: Vec<String> = Vec::with_capacity(topics.len());
    for (i, t) in topics.iter().enumerate() {
        let name = t.as_str().unwrap_or("?");
        let ty = types.get(i).and_then(Value::as_str).unwrap_or("?");
        lines.push(format!("{name} ({ty})"));
    }
    crate::ros2::truncate_result(&format!("{} topics:\n{}", topics.len(), lines.join("\n")))
}

/// `list_services`/`list_nodes` share this shape: call `service`, read the
/// `field` array out of `values`, render as a numbered list.
async fn introspect_named_list(
    client: &Ros2BridgeClient,
    service: &str,
    service_type: &str,
    field: &str,
    label: &str,
) -> String {
    let resp = match client
        .call_service(
            service,
            Some(service_type),
            json!({}),
            Duration::from_secs(DEFAULT_SERVICE_TIMEOUT_SECS),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => return format!("Error: {service} call failed — {e}"),
    };
    if !resp.result {
        return format!("Error: {service} reported failure: {}", resp.values);
    }
    let items = resp
        .values
        .get(field)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return format!("0 {label} reported.");
    }
    let lines: Vec<String> = items
        .iter()
        .map(|v| v.as_str().unwrap_or("?").to_string())
        .collect();
    format!("{} {label}:\n{}", items.len(), lines.join("\n"))
}

/// `list_actions` (`/rosapi/action_servers`) — a failed call (rosapi on this
/// robot not answering the op, e.g. a pre-2.0.0 rosbridge_suite that lacks
/// the action_servers introspection service) renders an `Error:` naming
/// `action_servers`, rather than a silent empty list.
async fn introspect_action_servers(client: &Ros2BridgeClient) -> String {
    let resp = match client
        .call_service(
            "/rosapi/action_servers",
            Some("rosapi_msgs/srv/GetActionServers"),
            json!({}),
            Duration::from_secs(DEFAULT_SERVICE_TIMEOUT_SECS),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return format!(
                "Error: rosapi on this robot did not answer /rosapi/action_servers — {e}"
            );
        }
    };
    if !resp.result {
        return format!(
            "Error: rosapi on this robot did not answer /rosapi/action_servers — {}",
            resp.values
        );
    }
    let servers = resp
        .values
        .get("action_servers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if servers.is_empty() {
        return "0 action servers reported.".to_string();
    }
    let lines: Vec<String> = servers
        .iter()
        .map(|v| v.as_str().unwrap_or("?").to_string())
        .collect();
    format!("{} action servers:\n{}", servers.len(), lines.join("\n"))
}

async fn introspect_topic_type(client: &Ros2BridgeClient, args: &Value) -> String {
    let Some(topic) = args["topic"].as_str().filter(|s| !s.is_empty()) else {
        return "Error: 'topic' parameter is required for the topic_type action".to_string();
    };
    match resolve_topic_type(client, topic).await {
        Ok(ty) => format!("{topic}: {ty}"),
        Err(e) => format!("Error: {e}"),
    }
}

async fn introspect_service_type(client: &Ros2BridgeClient, args: &Value) -> String {
    let Some(service) = args["service"].as_str().filter(|s| !s.is_empty()) else {
        return "Error: 'service' parameter is required for the service_type action".to_string();
    };
    let resp = match client
        .call_service(
            "/rosapi/service_type",
            Some("rosapi_msgs/srv/ServiceType"),
            json!({"service": service}),
            Duration::from_secs(DEFAULT_SERVICE_TIMEOUT_SECS),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => return format!("Error: /rosapi/service_type call failed — {e}"),
    };
    if !resp.result {
        return format!(
            "Error: /rosapi/service_type reported failure: {}",
            resp.values
        );
    }
    let ty = resp
        .values
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("");
    if ty.is_empty() {
        return format!("Error: unknown type for service {service}");
    }
    format!("{service}: {ty}")
}

async fn introspect_message_details(client: &Ros2BridgeClient, args: &Value) -> String {
    let Some(msg_type_name) = args["msg_type_name"].as_str().filter(|s| !s.is_empty()) else {
        return "Error: 'msg_type_name' parameter is required for the message_details action"
            .to_string();
    };
    let resp = match client
        .call_service(
            "/rosapi/message_details",
            Some("rosapi_msgs/srv/MessageDetails"),
            json!({"type": msg_type_name}),
            Duration::from_secs(DEFAULT_SERVICE_TIMEOUT_SECS),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => return format!("Error: /rosapi/message_details call failed — {e}"),
    };
    if !resp.result {
        return format!(
            "Error: /rosapi/message_details reported failure: {}",
            resp.values
        );
    }
    let typedefs = resp
        .values
        .get("typedefs")
        .cloned()
        .unwrap_or_else(|| json!([]));
    format!(
        "{msg_type_name}: {}",
        serde_json::to_string(&typedefs).unwrap_or_default()
    )
}
