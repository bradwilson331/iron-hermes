//! `ros2` module root (Phase 49.8 D-01/D-05/D-07/D-14).
//!
//! The generic `ros2` tool talks to a robot over the rosbridge v2 JSON protocol
//! on a WebSocket (D-01) — no ROS client library, DDS, or Zenoh crate is linked.
//! Reads (list/introspect) may target any name; writes (publish/call_service/
//! send_goal) are permitted only for names declared under `ros2.publish` /
//! `ros2.services` / `ros2.actions` in config, together with their message type
//! (D-04, hybrid access model — the inverse of `BrowserConfig.allowed_domains`'
//! empty-means-allow-all default).
//!
//! This plan (49.8-01) lands the tracer slice: config resolution, a persistent
//! [`session::Ros2Session`], the minimal [`bridge_client::Ros2BridgeClient`] path
//! (connect/advertise/publish/call_service), and [`ros2_tool::Ros2Tool`] with
//! `list_topics` + `publish`. 49.8-02 completes the bridge client (subscribe,
//! actions) and registers the `ros2` toolset in the taxonomy lists.

pub mod action_ops;
pub mod bridge_client;
pub mod motion;
pub mod read_ops;
pub mod ros2_tool;
pub mod rospider_tool;
pub mod safety;
pub mod session;
pub mod summarize;

pub use bridge_client::Ros2BridgeClient;
pub use ros2_tool::Ros2Tool;
pub use rospider_tool::RospiderTool;
pub use session::{Ros2Session, end_session_stop, with_session_end_stop};

use std::sync::Arc;

use ironhermes_core::Config;

/// D-14/D-02: env var overriding `ros2.bridge_url`. Read fresh at call time
/// (hexapod D-12 precedent) — never cached.
pub const BRIDGE_URL_ENV: &str = "IRONHERMES_ROS2_BRIDGE_URL";

/// D-07: toolset name for the generic `ros2` tool, independent of `robotics`.
pub const ROS2_TOOLSET: &str = "ros2";

/// Wall-clock execution budget for `ros2`/`rospider` tool calls. 45s covers a
/// 5s motion, a 10s bounded-subscribe window, or a 20s goal wait, plus connect.
pub const ROS2_TOOL_TIMEOUT_SECS: u64 = 45;

/// Result truncation cap — parity with `terminal.rs`'s `MAX_OUTPUT_LEN` (used
/// starting 49.8-05's LaserScan/PointCloud2 summarization).
pub const ROS2_MAX_RESULT_CHARS: usize = 50_000;

/// Where [`ros2_tool::Ros2Tool`] (and 49.8-09's `RospiderTool`) read their
/// [`Config`] from.
#[derive(Clone)]
pub enum Ros2ConfigSource {
    /// Production: `Config::load()` fresh on every call (hexapod D-12
    /// precedent) — the config file may change between calls. The env
    /// override IS consulted.
    Live,
    /// Tests: a fixed, in-memory `Config` set up by the test. The env
    /// override is NEVER consulted here — a developer shell exporting a real
    /// robot URL can never drive `cargo test` (T-49.8-08).
    Fixed(Arc<Config>),
}

impl Ros2ConfigSource {
    /// The `Config` this source currently resolves to.
    pub fn load(&self) -> Arc<Config> {
        match self {
            Ros2ConfigSource::Live => Arc::new(Config::load().unwrap_or_default()),
            Ros2ConfigSource::Fixed(cfg) => cfg.clone(),
        }
    }

    /// The effective rosbridge bridge URL for `cfg` (already resolved via
    /// [`Ros2ConfigSource::load`]). `Live` checks [`BRIDGE_URL_ENV`] first;
    /// `Fixed` reads the config value only.
    pub fn bridge_url(&self, cfg: &Config) -> Option<String> {
        match self {
            Ros2ConfigSource::Live => resolve_bridge_url(
                std::env::var(BRIDGE_URL_ENV).ok().as_deref(),
                cfg.ros2.bridge_url.as_deref(),
            ),
            Ros2ConfigSource::Fixed(_) => {
                let trimmed = cfg.ros2.bridge_url.as_deref().unwrap_or("").trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
        }
    }
}

/// D-02/D-14: trimmed non-blank `env` wins over trimmed non-blank `config`;
/// blank/absent values on either side fall through.
pub fn resolve_bridge_url(env: Option<&str>, config: Option<&str>) -> Option<String> {
    if let Some(e) = env {
        let trimmed = e.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(c) = config {
        let trimmed = c.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// D-01/D-02: only `ws://` with a host is accepted. `wss://` gets a clear
/// message pointing at `ws://` on a trusted LAN or localhost — TLS is out of
/// scope for this phase (research Security Domain).
pub fn validate_bridge_url(url: &str) -> Result<(), String> {
    let parsed = match url::Url::parse(url) {
        Ok(p) => p,
        Err(e) => return Err(format!("'{url}' is not a valid URL: {e}")),
    };
    match parsed.scheme() {
        "ws" => {
            if parsed.host().is_none() {
                Err(format!("'{url}' has no host — expected ws://<host>:<port>"))
            } else {
                Ok(())
            }
        }
        "wss" => Err(format!(
            "'{url}' uses wss:// — TLS is not supported. Use ws:// on a trusted LAN or localhost."
        )),
        other => Err(format!(
            "'{url}' uses unsupported scheme '{other}' — only ws:// is supported."
        )),
    }
}

/// D-06: drop a middle `msg`/`srv`/`action` path segment so `pkg/msg/Type` and
/// `pkg/Type` compare equal.
pub fn canonical_type(msg_type: &str) -> String {
    let parts: Vec<&str> = msg_type.split('/').collect();
    if parts.len() == 3 && matches!(parts[1], "msg" | "srv" | "action") {
        format!("{}/{}", parts[0], parts[2])
    } else {
        msg_type.to_string()
    }
}

/// D-08/D-09: is `msg_type` a Twist-family message? Motion-producing writes on
/// these types fail closed in this plan — 49.8-04 replaces the refusal with the
/// capped, timed-motion engine.
pub fn is_twist_family(msg_type: &str) -> bool {
    matches!(
        canonical_type(msg_type).as_str(),
        "geometry_msgs/Twist" | "geometry_msgs/TwistStamped"
    )
}

/// D-04: exact-match lookup — no globbing.
pub fn find_allow_entry<'a>(
    entries: &'a [ironhermes_core::config::Ros2AllowEntry],
    name: &str,
) -> Option<&'a ironhermes_core::config::Ros2AllowEntry> {
    entries.iter().find(|e| e.name == name)
}

/// D-04: the fixed refusal string returned for an undeclared or type-mismatched
/// write — final, no retry (the hexapod D-16 `Ok("… blocked …")` pattern).
pub fn not_permitted(section: &str, name: &str) -> String {
    format!(
        "'{name}' is not permitted — it is not declared under ros2.{section} in config.yaml. \
         Do not retry; ask the operator to declare it."
    )
}

/// Truncate `s` to at most [`ROS2_MAX_RESULT_CHARS`] characters at a char
/// boundary, matching `terminal.rs`'s `MAX_OUTPUT_LEN` truncation shape.
pub(crate) fn truncate_result(s: &str) -> String {
    if s.len() <= ROS2_MAX_RESULT_CHARS {
        return s.to_string();
    }
    let mut end = ROS2_MAX_RESULT_CHARS;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n...[truncated]", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ros2_resolve_bridge_url_env_wins_over_config() {
        assert_eq!(
            resolve_bridge_url(Some("ws://robot:9090"), Some("ws://config:9090")),
            Some("ws://robot:9090".to_string())
        );
    }

    #[test]
    fn ros2_resolve_bridge_url_ignores_blank() {
        assert_eq!(
            resolve_bridge_url(Some("   "), Some("ws://config:9090")),
            Some("ws://config:9090".to_string())
        );
        assert_eq!(resolve_bridge_url(None, None), None);
        assert_eq!(resolve_bridge_url(Some(""), Some("")), None);
        assert_eq!(resolve_bridge_url(Some("  "), None), None);
    }

    #[test]
    fn ros2_validate_bridge_url_accepts_ws_only() {
        assert!(validate_bridge_url("ws://h:9090").is_ok());
        assert!(validate_bridge_url("wss://h:9090").is_err());
        assert!(validate_bridge_url("http://h:9090").is_err());
        assert!(validate_bridge_url("file:///x").is_err());
        assert!(validate_bridge_url("not a url").is_err());
    }

    #[test]
    fn ros2_canonical_type_equivalence() {
        assert_eq!(canonical_type("geometry_msgs/Twist"), "geometry_msgs/Twist");
        assert_eq!(
            canonical_type("geometry_msgs/msg/Twist"),
            "geometry_msgs/Twist"
        );
        assert_eq!(
            canonical_type("geometry_msgs/msg/Twist"),
            canonical_type("geometry_msgs/Twist")
        );
        assert!(is_twist_family("geometry_msgs/msg/Twist"));
        assert!(is_twist_family("geometry_msgs/TwistStamped"));
        assert!(!is_twist_family("std_msgs/msg/String"));
    }
}
