//! Phase 49.8 Plan 10, Task 2: a docs/config/schema drift guard (D-15/D-05).
//! Fails the moment `docs/CONFIGURATION.md`, `docs/ros2-integration.md`,
//! `docs/robots/rospider.md`, or `skills/rospider/SKILL.md` diverges from
//! the actual `Ros2Config`/`RospiderConfig` defaults or the `Ros2Tool`/
//! `RospiderTool` action schemas — no hand-maintained doc can silently
//! drift out of sync with the source of truth.

use std::collections::HashSet;
use std::sync::Arc;

use ironhermes_core::Config;
use ironhermes_core::config::{Ros2Caps, Ros2Config, RospiderConfig};
use ironhermes_tools::credentials::ToolCredentials;
use ironhermes_tools::registry::Tool;
use ironhermes_tools::ros2::safety::EffectiveCaps;
use ironhermes_tools::ros2::session::{
    GOAL_CANCEL_NOT_CONFIRMED_NOTE, MAX_MOTION_TOPICS, PREEMPT_WAIT_MS,
    ROS2_SESSION_END_TIMEOUT_SECS, SESSION_END_TIMEOUT_REPORT,
};
use ironhermes_tools::ros2::{Ros2ConfigSource, Ros2Session, Ros2Tool, RospiderTool};

const CONFIGURATION_MD: &str = include_str!("../../../docs/CONFIGURATION.md");
const ROS2_INTEGRATION_MD: &str = include_str!("../../../docs/ros2-integration.md");
const ROSPIDER_SKILL_MD: &str = include_str!("../../../skills/rospider/SKILL.md");
const ROSPIDER_DOC_MD: &str = include_str!("../../../docs/robots/rospider.md");
const ROSPIDER_UAT_MD: &str = include_str!("../../../docs/robots/rospider-uat.md");
const ROSPIDER_TOOL_RS: &str = include_str!("../src/ros2/rospider_tool.rs");
const CLI_EVENT_LOOP_RS: &str = include_str!("../../ironhermes-cli/src/tui_rata/event_loop.rs");

/// Flattens a `serde_yaml::Value` into dotted-path leaf keys. A `Mapping`
/// recurses; anything else (scalar, `Null`, or `Sequence`) is a leaf —
/// matching this task's own `<behavior>` spec ("sequences and nulls count
/// as leaves").
fn flatten_leaves(prefix: &str, value: &serde_yaml::Value, out: &mut Vec<String>) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                let key = k.as_str().unwrap_or_default();
                let next = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_leaves(&next, v, out);
            }
        }
        _ => out.push(prefix.to_string()),
    }
}

/// Extracts every `<key>` from a `` `ros2.<key>` `` backticked span in
/// `text` — used for the reverse direction of `ros2_config_keys_documented`
/// (every documented key must resolve to a real leaf).
fn extract_backticked_ros2_keys(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut idx = 0;
    while let Some(rel) = text[idx..].find("`ros2.") {
        let start = idx + rel + 1; // skip the opening backtick
        let rest = &text[start..];
        let Some(end) = rest.find('`') else { break };
        let full = &rest[..end];
        if let Some(key) = full.strip_prefix("ros2.") {
            // Only a plain dotted-path identifier is a config-key
            // reference — skip anything with whitespace or punctuation
            // that isn't part of a key (e.g. a prose sentence that
            // happens to start with "ros2." would not end up here since
            // the scan stops at the next backtick, but guard anyway).
            if !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
            {
                keys.push(key.to_string());
            }
        }
        idx = start + end + 1;
    }
    keys
}

fn contains_backticked(haystack: &str, needle: &str) -> bool {
    haystack.contains(&format!("`{needle}`"))
}

fn ros2_actions() -> Vec<String> {
    let session = Ros2Session::new();
    let source = Ros2ConfigSource::Fixed(Arc::new(Config::default()));
    let creds = Arc::new(ToolCredentials::env_only());
    let tool = Ros2Tool::new(session, source, creds, None);
    let schema = tool.schema();
    schema.function.parameters["properties"]["action"]["enum"]
        .as_array()
        .expect("ros2 schema must declare an action enum")
        .iter()
        .map(|v| {
            v.as_str()
                .expect("action enum values must be strings")
                .to_string()
        })
        .collect()
}

fn rospider_actions() -> Vec<String> {
    let session = Ros2Session::new();
    let source = Ros2ConfigSource::Fixed(Arc::new(Config::default()));
    let creds = Arc::new(ToolCredentials::env_only());
    let tool = RospiderTool::new(session, source, creds, None);
    let schema = tool.schema();
    schema.function.parameters["properties"]["action"]["enum"]
        .as_array()
        .expect("rospider schema must declare an action enum")
        .iter()
        .map(|v| {
            v.as_str()
                .expect("action enum values must be strings")
                .to_string()
        })
        .collect()
}

/// `ros2_config_keys_documented` — every leaf key of
/// `serde_yaml::to_value(Ros2Config::default())` appears in
/// `docs/CONFIGURATION.md` as `` `ros2.<key>` ``, and every such backticked
/// `ros2.` key in that file resolves to a real leaf (bidirectional — a
/// stale doc entry for a removed key fails just as loudly as a missing
/// one).
#[test]
fn ros2_config_keys_documented() {
    let value = serde_yaml::to_value(Ros2Config::default()).expect("Ros2Config must serialize");
    let mut leaves = Vec::new();
    flatten_leaves("", &value, &mut leaves);
    assert!(
        !leaves.is_empty(),
        "Ros2Config::default() produced zero leaves — flattening is broken"
    );

    let mut missing: Vec<String> = leaves
        .iter()
        .filter(|leaf| !contains_backticked(CONFIGURATION_MD, &format!("ros2.{leaf}")))
        .cloned()
        .collect();
    missing.sort();
    assert!(
        missing.is_empty(),
        "docs/CONFIGURATION.md is missing these ros2.* leaf keys: {missing:?}"
    );

    let leaf_set: HashSet<&str> = leaves.iter().map(|s| s.as_str()).collect();
    let mut stale: Vec<String> = extract_backticked_ros2_keys(CONFIGURATION_MD)
        .into_iter()
        .filter(|k| !leaf_set.contains(k.as_str()))
        .collect();
    stale.sort();
    stale.dedup();
    assert!(
        stale.is_empty(),
        "docs/CONFIGURATION.md references ros2.* keys that are not real Ros2Config leaves: {stale:?}"
    );
}

/// `ros2_tool_actions_documented` — every value in the `action` enum of
/// `Ros2Tool::schema()` and `RospiderTool::schema()` appears backticked in
/// `docs/ros2-integration.md`; every `RospiderTool` action also appears in
/// `skills/rospider/SKILL.md`.
#[test]
fn ros2_tool_actions_documented() {
    let ros2 = ros2_actions();
    assert!(!ros2.is_empty(), "Ros2Tool::schema() produced zero actions");
    let mut missing_ros2: Vec<&String> = ros2
        .iter()
        .filter(|a| !contains_backticked(ROS2_INTEGRATION_MD, a))
        .collect();
    missing_ros2.sort();
    assert!(
        missing_ros2.is_empty(),
        "docs/ros2-integration.md is missing ros2 actions: {missing_ros2:?}"
    );

    let rospider = rospider_actions();
    assert!(
        !rospider.is_empty(),
        "RospiderTool::schema() produced zero actions"
    );
    let mut missing_rospider_doc: Vec<&String> = rospider
        .iter()
        .filter(|a| !contains_backticked(ROS2_INTEGRATION_MD, a))
        .collect();
    missing_rospider_doc.sort();
    assert!(
        missing_rospider_doc.is_empty(),
        "docs/ros2-integration.md is missing rospider actions: {missing_rospider_doc:?}"
    );

    let mut missing_skill: Vec<&String> = rospider
        .iter()
        .filter(|a| !ROSPIDER_SKILL_MD.contains(a.as_str()))
        .collect();
    missing_skill.sort();
    assert!(
        missing_skill.is_empty(),
        "skills/rospider/SKILL.md is missing rospider actions: {missing_skill:?}"
    );
}

/// `rospider_skill_requires_ros2_toolset` — `parse_skill_md` +
/// `extract_hermes_metadata` on `skills/rospider/SKILL.md` yields
/// `requires_toolsets` containing `ros2`.
#[test]
fn rospider_skill_requires_ros2_toolset() {
    let (frontmatter, _body) = ironhermes_core::skills::parse_skill_md(ROSPIDER_SKILL_MD)
        .expect("skills/rospider/SKILL.md must parse as a valid skill");
    let metadata = ironhermes_core::skills::extract_hermes_metadata(&frontmatter.metadata)
        .expect("skills/rospider/SKILL.md must carry a metadata.hermes block");
    assert!(
        metadata.requires_toolsets.iter().any(|t| t == "ros2"),
        "skills/rospider/SKILL.md's requires_toolsets must contain 'ros2'; got: {:?}",
        metadata.requires_toolsets
    );
}

/// `rospider_doc_names_match_config_defaults` — every non-numeric
/// `RospiderConfig::default()` value (the twelve topic/service names)
/// appears in `docs/robots/rospider.md`.
#[test]
fn rospider_doc_names_match_config_defaults() {
    let cfg = RospiderConfig::default();
    let value = serde_yaml::to_value(&cfg).expect("RospiderConfig must serialize");
    let mapping = value
        .as_mapping()
        .expect("RospiderConfig must serialize to a mapping");

    assert!(
        mapping.len() > 1,
        "sanity check: RospiderConfig::default() must have more than one field"
    );

    let mut checked = 0;
    let mut missing = Vec::new();
    for (key, v) in mapping {
        let key_name = key.as_str().unwrap_or_default();
        // "Non-numeric" — skip camera_pan_servo_id (an integer), keep every
        // string-valued default (the twelve topic/service names).
        let Some(name) = v.as_str() else { continue };
        checked += 1;
        if !ROSPIDER_DOC_MD.contains(name) {
            missing.push(format!("{key_name} = {name}"));
        }
    }
    assert!(
        checked > 0,
        "sanity check: RospiderConfig::default() must have at least one string-valued field"
    );
    assert!(
        missing.is_empty(),
        "docs/robots/rospider.md is missing these RospiderConfig::default() names: {missing:?}"
    );
}

/// Returns the first markdown table row (a line starting with `|`) in `doc`
/// that contains the backticked `` `ros2.caps.<key>` `` span. Panics naming
/// `doc_name` and `key` when no such row exists — a doc drift guard is
/// useless if its own lookup can silently match nothing.
fn find_caps_row<'a>(doc: &'a str, doc_name: &str, key: &str) -> &'a str {
    let needle = format!("`ros2.caps.{key}`");
    doc.lines()
        .find(|line| line.starts_with('|') && line.contains(&needle))
        .unwrap_or_else(|| panic!("{doc_name} has no table row for ros2.caps.{key}"))
}

/// `ros2_zero_cap_semantics_documented` — binds the D-08 gap-closure (49.8-12)
/// zero-cap semantics to both docs and to `EffectiveCaps::from_config`'s
/// actual behavior, so the docs and the code can never silently disagree
/// about which caps honor an explicit `0` and which fall back to the
/// default on a non-positive value.
#[test]
fn ros2_zero_cap_semantics_documented() {
    // The three caps that honor an explicit 0 as the strictest setting.
    for key in [
        "max_linear_velocity",
        "max_angular_velocity",
        "max_subscribe_messages",
    ] {
        for (doc, doc_name) in [
            (CONFIGURATION_MD, "docs/CONFIGURATION.md"),
            (ROS2_INTEGRATION_MD, "docs/ros2-integration.md"),
        ] {
            let row = find_caps_row(doc, doc_name, key);
            assert!(
                row.contains("`0` disables"),
                "{doc_name}'s ros2.caps.{key} row must contain '`0` disables'; got: {row}"
            );
        }
    }
    assert_eq!(
        EffectiveCaps::from_config(&Ros2Caps {
            max_linear_velocity: 0.0,
            ..Ros2Caps::default()
        })
        .max_linear,
        0.0,
        "an explicit max_linear_velocity: 0 must resolve to an effective 0.0"
    );
    assert_eq!(
        EffectiveCaps::from_config(&Ros2Caps {
            max_angular_velocity: 0.0,
            ..Ros2Caps::default()
        })
        .max_angular,
        0.0,
        "an explicit max_angular_velocity: 0 must resolve to an effective 0.0"
    );
    assert_eq!(
        EffectiveCaps::from_config(&Ros2Caps {
            max_subscribe_messages: 0,
            ..Ros2Caps::default()
        })
        .max_subscribe_messages,
        0,
        "an explicit max_subscribe_messages: 0 must resolve to an effective 0"
    );

    // The four caps where 0 is not a usable limit — a non-positive value
    // falls back to the default, and the docs must say so (never "`0`
    // disables", which would contradict the fallback behavior).
    let defaults_effective = EffectiveCaps::from_config(&Ros2Caps::default());
    for key in [
        "max_motion_duration_secs",
        "motion_publish_hz",
        "max_subscribe_secs",
        "max_goal_secs",
    ] {
        for (doc, doc_name) in [
            (CONFIGURATION_MD, "docs/CONFIGURATION.md"),
            (ROS2_INTEGRATION_MD, "docs/ros2-integration.md"),
        ] {
            let row = find_caps_row(doc, doc_name, key);
            assert!(
                row.contains("falls back to the default"),
                "{doc_name}'s ros2.caps.{key} row must contain 'falls back to the default'; got: {row}"
            );
            assert!(
                !row.contains("`0` disables"),
                "{doc_name}'s ros2.caps.{key} row must NOT contain '`0` disables' (0 is not a usable limit here); got: {row}"
            );
        }
    }

    assert_eq!(
        EffectiveCaps::from_config(&Ros2Caps {
            max_motion_duration_secs: 0.0,
            ..Ros2Caps::default()
        })
        .max_motion_secs,
        defaults_effective.max_motion_secs,
        "max_motion_duration_secs: 0.0 must fall back to the default, not resolve to 0"
    );
    assert_eq!(
        EffectiveCaps::from_config(&Ros2Caps {
            motion_publish_hz: 0.0,
            ..Ros2Caps::default()
        })
        .publish_hz,
        defaults_effective.publish_hz,
        "motion_publish_hz: 0.0 must fall back to the default, not resolve to 0"
    );
    assert_eq!(
        EffectiveCaps::from_config(&Ros2Caps {
            max_subscribe_secs: 0.0,
            ..Ros2Caps::default()
        })
        .max_subscribe_secs,
        defaults_effective.max_subscribe_secs,
        "max_subscribe_secs: 0.0 must fall back to the default, not resolve to 0"
    );
    assert_eq!(
        EffectiveCaps::from_config(&Ros2Caps {
            max_goal_secs: 0.0,
            ..Ros2Caps::default()
        })
        .max_goal_secs,
        defaults_effective.max_goal_secs,
        "max_goal_secs: 0.0 must fall back to the default, not resolve to 0"
    );
}

/// `rospider_rest_stop_documented` (49.8-14, G-49.8-2b) — the robot guide,
/// the live UAT checklist, the integration doc, the skill and the
/// `RospiderTool` schema description all describe the real ROSPider stop
/// (the gait-0 rest stop), not a zero-Twist-only stop.
#[test]
fn rospider_rest_stop_documented() {
    let topic = RospiderConfig::default().traveling_topic;
    assert!(
        !topic.is_empty(),
        "sanity check: the default traveling_topic must be non-empty"
    );

    for needle in ["kinematics_msgs/msg/Traveling", topic.as_str(), "gait: 0"] {
        assert!(
            ROSPIDER_DOC_MD.contains(needle),
            "docs/robots/rospider.md is missing {needle:?}"
        );
    }

    for needle in [topic.as_str(), "gait: 0", "stepping in place"] {
        assert!(
            ROSPIDER_UAT_MD.contains(needle),
            "docs/robots/rospider-uat.md is missing {needle:?}"
        );
    }

    assert!(
        ROS2_INTEGRATION_MD.contains("gait-0 rest stop"),
        "docs/ros2-integration.md is missing \"gait-0 rest stop\""
    );

    assert!(
        ROSPIDER_SKILL_MD.contains("gait-0 rest stop"),
        "skills/rospider/SKILL.md is missing \"gait-0 rest stop\""
    );

    let session = Ros2Session::new();
    let source = Ros2ConfigSource::Fixed(Arc::new(Config::default()));
    let creds = Arc::new(ToolCredentials::env_only());
    let tool = RospiderTool::new(session, source, creds, None);
    assert!(
        tool.description().contains("gait-0 rest stop"),
        "RospiderTool::description() is missing \"gait-0 rest stop\""
    );
}

/// `rospider_stop_ordering_documented` (49.8-16, G-49.8-2b; 49.8-17 WR-01) —
/// the robot guide, the integration doc and the live UAT checklist describe
/// the stop-ordering guarantee (serialization, coalescing) and its degraded
/// (timeout) case, pinned to the real `PREEMPT_WAIT_MS` bound — and, since
/// 49.8-17, to `ROS2_SESSION_END_TIMEOUT_SECS` — so the docs can never
/// silently drift from the code.
#[test]
fn rospider_stop_ordering_documented() {
    let secs = format!("{} s", PREEMPT_WAIT_MS as f64 / 1000.0);
    // 49.8-17 (WR-01) / 49.8-19: the integration doc's session-end bound is
    // rendered as "(Ns)" — pin it to the constant so a docs edit can never
    // desync the bound the surfaces actually enforce. 49.8-19 widened it to
    // 12s and added the motion-topic ceiling.
    let end_secs = format!("({}s)", ROS2_SESSION_END_TIMEOUT_SECS);
    let max_topics = format!("at most {MAX_MOTION_TOPICS} distinct motion topics");

    for needle in [secs.as_str(), "never skipped", "one at a time"] {
        assert!(
            ROSPIDER_DOC_MD.contains(needle),
            "docs/robots/rospider.md is missing {needle:?}"
        );
    }

    for needle in [
        "one at a time",
        "PREEMPT_WAIT_MS",
        end_secs.as_str(),
        "GOAL_CANCEL_PHASE_MS",
        "STOP_SEQUENCE_MS",
        max_topics.as_str(),
        "TimeoutStopSec",
        SESSION_END_TIMEOUT_REPORT,
    ] {
        assert!(
            ROS2_INTEGRATION_MD.contains(needle),
            "docs/ros2-integration.md is missing {needle:?}"
        );
    }

    for needle in [
        "did not finish its own stop",
        "one brief step",
        GOAL_CANCEL_NOT_CONFIRMED_NOTE,
        "ros2 session-end stop timed out",
    ] {
        assert!(
            ROSPIDER_UAT_MD.contains(needle),
            "docs/robots/rospider-uat.md is missing {needle:?}"
        );
    }
}

/// `rospider_odometry_source_documented` (49.8-25, G-49.8-12) — the operator
/// guide's "Odometry needs a robot-side source" section names the actual
/// EKF plumbing (`ekf_filter_node`, `odom_rf2o`, `rf2o_laser_odometry`,
/// `odom_raw`, `ekf.yaml`), and the documented symptom string is built from
/// `RospiderConfig::default().odom_topic` and checked against the tool's
/// real error-format string in `rospider_tool.rs`'s `sample_one`, so a
/// wording change on either side fails this test.
#[test]
fn rospider_odometry_source_documented() {
    for needle in [
        "## Odometry needs a robot-side source",
        "ekf_filter_node",
        "odom_rf2o",
        "rf2o_laser_odometry",
        "odom_raw",
        "ekf.yaml",
    ] {
        assert!(
            ROSPIDER_DOC_MD.contains(needle),
            "docs/robots/rospider.md is missing {needle:?} (G-49.8-12)"
        );
    }

    let odom_topic = RospiderConfig::default().odom_topic;
    let symptom = format!("no message on {odom_topic} within");
    assert!(
        ROSPIDER_DOC_MD.contains(&symptom),
        "docs/robots/rospider.md is missing the documented odom symptom {symptom:?} (G-49.8-12)"
    );
    assert!(
        ROSPIDER_TOOL_RS.contains("no message on {topic} within"),
        "crates/ironhermes-tools/src/ros2/rospider_tool.rs no longer uses \"no message on {{topic}} within\" — \
         update docs/robots/rospider.md's symptom text to match (G-49.8-12)"
    );
}

/// `rospider_camera_pan_direction_documented` (49.8-25, G-49.8-12) — the
/// 2026-10-05 UAT finding that a positive `degrees` value pans the camera
/// left is recorded in both the operator guide and the skill, and the
/// agent-facing odometry-gap guidance ("robot-side odometry source") is
/// recorded in both the skill and the live UAT checklist.
#[test]
fn rospider_camera_pan_direction_documented() {
    for (doc, name) in [
        (ROSPIDER_DOC_MD, "docs/robots/rospider.md"),
        (ROSPIDER_SKILL_MD, "skills/rospider/SKILL.md"),
    ] {
        assert!(
            doc.contains("positive `degrees` pans the camera left"),
            "{name} is missing \"positive `degrees` pans the camera left\" (G-49.8-12)"
        );
    }

    for (doc, name) in [
        (ROSPIDER_SKILL_MD, "skills/rospider/SKILL.md"),
        (ROSPIDER_UAT_MD, "docs/robots/rospider-uat.md"),
    ] {
        assert!(
            doc.contains("robot-side odometry source"),
            "{name} is missing \"robot-side odometry source\" (G-49.8-12)"
        );
    }
}

/// `tui_exit_triggers_documented` (49.8-24, G-49.8-21) — the operator guide
/// names every TUI exit trigger that runs the ros2 session-end stop
/// (`/quit`, SIGTERM, SIGHUP) and states the single-ctrl+c limitation; the
/// live UAT checklist tells the tester exactly how to exit mid-walk; and
/// the CLI event loop source actually implements the SIGTERM/SIGHUP
/// listeners and the ctrl_c re-arm the docs describe — so a doc edit or a
/// code change on either side fails this test.
#[test]
fn tui_exit_triggers_documented() {
    for needle in [
        "`/quit`",
        "SIGTERM",
        "SIGHUP",
        "cancels the turn, not a running motion",
    ] {
        assert!(
            ROS2_INTEGRATION_MD.contains(needle),
            "docs/ros2-integration.md is missing {needle:?} (G-49.8-21)"
        );
    }

    assert!(
        ROSPIDER_UAT_MD.contains("ctrl+c twice"),
        "docs/robots/rospider-uat.md is missing \"ctrl+c twice\" (G-49.8-21)"
    );

    for needle in [
        "SignalKind::terminate()",
        "SignalKind::hangup()",
        "ctrl_c.set(",
    ] {
        assert!(
            CLI_EVENT_LOOP_RS.contains(needle),
            "crates/ironhermes-cli/src/tui_rata/event_loop.rs is missing {needle:?} (G-49.8-21)"
        );
    }
}
