//! Physical safety primitives for the `ros2` tool (Phase 49.8 D-08/D-09).
//!
//! Pure functions only — no I/O, no `Config::load()`. `EffectiveCaps::from_config`
//! turns an operator-supplied [`Ros2Caps`] into caps that can be lowered but never
//! raised above a hard code ceiling (D-08). The split is per-field:
//!
//! - `max_linear_velocity` / `max_angular_velocity` / `max_subscribe_messages`
//!   honor an explicit `0`/`0.0` as the operator's strictest choice — this
//!   codebase's "0 disables" idiom (`timeout_secs: 0`, `nudge_interval: 0`,
//!   `session_pool: 0`). An absent key still takes `Ros2Caps::default()`'s
//!   value (struct-level `#[serde(default)]`); for the two `f64` caps, a
//!   negative or non-finite value also falls back to the default.
//! - `max_motion_duration_secs` / `motion_publish_hz` / `max_subscribe_secs` /
//!   `max_goal_secs` are not usable at `0` (a zero-length motion, a
//!   divide-by-zero rate, an empty window or watchdog) — a non-positive value
//!   here falls back to the default, as before.
//!
//! No path ever raises an effective cap above its `HARD_*` ceiling.
//! [`clamp_twist`]/[`apply_field_limits`] clamp every physical parameter
//! before it leaves the process — clamp-then-send, never reject, mirroring
//! `hexapod_tcp.rs`'s clamping style; `clamp_twist` also normalizes each
//! clamped component to `+0.0` so a zero cap never publishes `-0.0` on the
//! wire. [`clamp_duration`], [`clamp_subscribe_count`],
//! [`clamp_subscribe_window`] and [`zero_twist`] are the remaining building
//! blocks 49.8-04's `motion.rs` and the subscribe paths compose into the
//! timed-motion and bounded-sample engines — none of the three panics on a
//! strict (but positive) configured cap below the code's own minimum
//! request.

use std::collections::BTreeMap;

use ironhermes_core::config::Ros2Caps;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Hard ceilings (Cap numbers table, `49.8-04-PLAN.md`) — a configured cap can
// lower but never raise an effective cap above these (D-08).
// ---------------------------------------------------------------------------

pub const HARD_MAX_LINEAR_VELOCITY: f64 = 0.5;
pub const HARD_MAX_ANGULAR_VELOCITY: f64 = 1.5;
pub const HARD_MAX_MOTION_SECS: f64 = 5.0;
pub const HARD_MIN_MOTION_SECS: f64 = 0.1;
/// Default requested motion duration when the caller omits `duration_secs`.
pub const DEFAULT_MOTION_SECS: f64 = 1.0;
pub const HARD_MIN_PUBLISH_HZ: f64 = 2.0;
pub const HARD_MAX_PUBLISH_HZ: f64 = 20.0;
pub const HARD_MAX_SUBSCRIBE_MESSAGES: u32 = 20;
pub const HARD_MAX_SUBSCRIBE_SECS: f64 = 10.0;
/// Floor for a `subscribe` window, formerly a literal in `do_subscribe`
/// (D-08 gap closure: the floor must never exceed a strict configured
/// `max_subscribe_secs`, or `clamp` would panic on `min > max`).
pub const HARD_MIN_SUBSCRIBE_SECS: f64 = 0.2;
pub const HARD_MAX_GOAL_SECS: f64 = 300.0;
/// Ceiling for a `goal_status` call's own `wait_secs` parameter (consumed by
/// 49.8-07 — declared here so every ros2 cap ceiling lives in one file).
pub const HARD_MAX_GOAL_WAIT_SECS: f64 = 20.0;
/// Ceiling for `call_service`'s `timeout_secs` parameter.
pub const HARD_MAX_SERVICE_TIMEOUT_SECS: f64 = 10.0;

/// Number of zero-Twist publishes sent to guarantee a motion stop, and the
/// interval between them (D-09) — survives a single dropped message.
pub const STOP_REPEATS: u32 = 3;
pub const STOP_INTERVAL_MS: u64 = 100;

/// A value that is non-finite or `<= 0` falls back to `default`; the result is
/// then never allowed to exceed `hard_ceiling` (D-08). For the four caps where
/// `0` is not a usable limit (`max_motion_duration_secs`, `motion_publish_hz`,
/// `max_subscribe_secs`, `max_goal_secs`) — "disable" is already expressed
/// elsewhere (a zero velocity cap, a zero subscribe cap, an empty `actions`
/// allowlist).
fn effective_capped(value: f64, default: f64, hard_ceiling: f64) -> f64 {
    let v = if value.is_finite() && value > 0.0 {
        value
    } else {
        default
    };
    v.min(hard_ceiling)
}

/// Same fallback rule as [`effective_capped`], except a finite value `>= 0.0`
/// — including an explicit `0.0` — is honored as is, rather than treated as
/// unset (D-08 gap closure). Only a negative or non-finite value falls back
/// to `default`. The result is `.min(hard_ceiling)`, then normalized so it is
/// never `-0.0` on the wire (adding `0.0` turns a `-0.0` result into `+0.0`).
fn effective_capped_allow_zero(value: f64, default: f64, hard_ceiling: f64) -> f64 {
    let v = if value.is_finite() && value >= 0.0 {
        value
    } else {
        default
    };
    v.min(hard_ceiling) + 0.0
}

/// Same fallback rule as [`effective_capped`], but clamped into
/// `[HARD_MIN_PUBLISH_HZ, HARD_MAX_PUBLISH_HZ]` on both ends — a rate has a
/// floor as well as a ceiling (RESEARCH.md Pitfall 4: too slow starves the
/// timed-motion loop's own duration budget).
fn effective_hz(value: f64, default: f64) -> f64 {
    let v = if value.is_finite() && value > 0.0 {
        value
    } else {
        default
    };
    v.clamp(HARD_MIN_PUBLISH_HZ, HARD_MAX_PUBLISH_HZ)
}

/// Operator caps resolved against the hard code ceilings above (D-08). A
/// configured value can only ever lower the effective cap, never raise it
/// past the corresponding `HARD_*` constant. `max_linear`/`max_angular`/
/// `max_subscribe_messages` honor an explicit `0` as the strictest setting;
/// the other four fields treat a non-positive value as invalid and fall back
/// to the default (D-08 gap closure — see the module doc).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectiveCaps {
    pub max_linear: f64,
    pub max_angular: f64,
    pub max_motion_secs: f64,
    pub publish_hz: f64,
    pub max_subscribe_messages: u32,
    pub max_subscribe_secs: f64,
    pub max_goal_secs: f64,
}

impl EffectiveCaps {
    pub fn from_config(caps: &Ros2Caps) -> Self {
        let defaults = Ros2Caps::default();
        Self {
            max_linear: effective_capped_allow_zero(
                caps.max_linear_velocity,
                defaults.max_linear_velocity,
                HARD_MAX_LINEAR_VELOCITY,
            ),
            max_angular: effective_capped_allow_zero(
                caps.max_angular_velocity,
                defaults.max_angular_velocity,
                HARD_MAX_ANGULAR_VELOCITY,
            ),
            max_motion_secs: effective_capped(
                caps.max_motion_duration_secs,
                defaults.max_motion_duration_secs,
                HARD_MAX_MOTION_SECS,
            ),
            publish_hz: effective_hz(caps.motion_publish_hz, defaults.motion_publish_hz),
            // A u32 has no invalid value (no sign, no NaN), so a present 0
            // stays 0 — no separate "allow zero" fallback function needed.
            max_subscribe_messages: caps.max_subscribe_messages.min(HARD_MAX_SUBSCRIBE_MESSAGES),
            max_subscribe_secs: effective_capped(
                caps.max_subscribe_secs,
                defaults.max_subscribe_secs,
                HARD_MAX_SUBSCRIBE_SECS,
            ),
            max_goal_secs: effective_capped(
                caps.max_goal_secs,
                defaults.max_goal_secs,
                HARD_MAX_GOAL_SECS,
            ),
        }
    }
}

/// One field this process clamped before sending, for the tool result to name
/// (D-08/D-11: every clamp is visible, never silent).
#[derive(Debug, Clone, PartialEq)]
pub struct ClampNote {
    pub path: String,
    pub requested: f64,
    pub applied: f64,
}

impl std::fmt::Display for ClampNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {} → {}", self.path, self.requested, self.applied)
    }
}

/// Normalizes `msg` (or, when `stamped`, its inner `twist` object) to a full
/// Twist — all six of `linear`/`angular` `x`/`y`/`z` present as `f64`, with a
/// missing, non-numeric, or non-finite component treated as `0.0` — and
/// clamps every component to `caps.max_linear`/`caps.max_angular`
/// (symmetric: `[-max, max]`). Returns one [`ClampNote`] per component that
/// was actually changed by clamping — an already-zero missing component never
/// generates a note.
pub fn clamp_twist(msg: &mut Value, caps: &EffectiveCaps, stamped: bool) -> Vec<ClampNote> {
    if !msg.is_object() {
        *msg = json!({});
    }
    if stamped {
        let twist_is_object = msg.get("twist").map(Value::is_object).unwrap_or(false);
        if !twist_is_object {
            msg["twist"] = json!({});
        }
    }
    let target: &mut Value = if stamped { &mut msg["twist"] } else { msg };

    let mut notes = Vec::with_capacity(2);
    for (group, max) in [("linear", caps.max_linear), ("angular", caps.max_angular)] {
        let group_is_object = target.get(group).map(Value::is_object).unwrap_or(false);
        if !group_is_object {
            target[group] = json!({});
        }
        for axis in ["x", "y", "z"] {
            let requested = target[group]
                .get(axis)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .unwrap_or(0.0);
            // + 0.0 normalizes a -0.0 clamp result (e.g. requested -0.3,
            // max 0.0 clamps to the lower bound -0.0) to +0.0 — a zero cap
            // must never publish -0.0 on the wire (D-08 gap closure).
            let applied = requested.clamp(-max, max) + 0.0;
            target[group][axis] = json!(applied);
            if (applied - requested).abs() > f64::EPSILON {
                notes.push(ClampNote {
                    path: format!("{group}.{axis}"),
                    requested,
                    applied,
                });
            }
        }
    }
    notes
}

/// Reads the value at dot-separated `path` inside `value` (e.g.
/// `"position.z"` -> `value["position"]["z"]`). `None` if any segment is
/// missing along the way.
fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// Writes `new_value` at dot-separated `path` inside `value`, creating any
/// missing intermediate object along the way. `value` (and every
/// intermediate) must already be a JSON object — callers only ever invoke
/// this after confirming the path exists via [`get_path`].
fn set_path(value: &mut Value, path: &str, new_value: Value) {
    let segments: Vec<&str> = path.split('.').collect();
    let mut current = value;
    for (i, seg) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            current[*seg] = new_value;
            return;
        }
        let is_object = current.get(*seg).map(Value::is_object).unwrap_or(false);
        if !is_object {
            current[*seg] = json!({});
        }
        current = &mut current[*seg];
    }
}

/// Clamps every dotted-path numeric field named in `limits` (e.g.
/// `"position.z": [100.0, 160.0]`) to its `[min, max]` bound. A `min > max`
/// entry is skipped with a `warn!` (operator typo, not a panic). A path
/// absent from `msg` is left absent — no field is invented. A present but
/// non-numeric value at a limited path is replaced with whichever bound is
/// nearest zero (the safest available fallback under an unreadable value).
/// Returns one [`ClampNote`] per field actually changed.
pub fn apply_field_limits(msg: &mut Value, limits: &BTreeMap<String, [f64; 2]>) -> Vec<ClampNote> {
    let mut notes = Vec::new();
    for (path, bounds) in limits {
        let [min, max] = *bounds;
        if min > max {
            tracing::warn!("ros2: field limit '{path}' has min > max ({min} > {max}) — skipping");
            continue;
        }

        let Some(current) = get_path(msg, path) else {
            continue;
        };

        let (requested, applied) = match current.as_f64().filter(|v| v.is_finite()) {
            Some(v) => (v, v.clamp(min, max)),
            None => {
                let nearest_zero_bound = if min.abs() <= max.abs() { min } else { max };
                (0.0, nearest_zero_bound)
            }
        };

        if (applied - requested).abs() <= f64::EPSILON {
            continue;
        }

        set_path(msg, path, json!(applied));
        notes.push(ClampNote {
            path: path.clone(),
            requested,
            applied,
        });
    }
    notes
}

/// Clamps a requested motion duration: `None` (or non-finite) becomes
/// [`DEFAULT_MOTION_SECS`]; the result is then clamped into
/// `[HARD_MIN_MOTION_SECS.min(caps.max_motion_secs), caps.max_motion_secs]`.
/// The lower bound is lowered to the cap itself (never raised above it) so a
/// strict but positive `max_motion_duration_secs` below `HARD_MIN_MOTION_SECS`
/// is honored instead of panicking in `clamp` (D-08 gap closure: `clamp`
/// requires `min <= max`).
pub fn clamp_duration(requested: Option<f64>, caps: &EffectiveCaps) -> f64 {
    let v = requested
        .filter(|v| v.is_finite())
        .unwrap_or(DEFAULT_MOTION_SECS);
    let floor = HARD_MIN_MOTION_SECS.min(caps.max_motion_secs);
    v.clamp(floor, caps.max_motion_secs)
}

/// Clamps a requested `subscribe`/sensor-read message count. Returns `None`
/// when `caps.max_subscribe_messages == 0` — sampling is disabled by the
/// operator, and the caller must refuse before any bridge I/O (D-08 gap
/// closure). Otherwise returns `Some` of the requested count (default `1`,
/// saturating on overflow rather than wrapping) clamped into
/// `[1, caps.max_subscribe_messages]`.
pub fn clamp_subscribe_count(requested: Option<u64>, caps: &EffectiveCaps) -> Option<u32> {
    if caps.max_subscribe_messages == 0 {
        return None;
    }
    let requested_u32 = u32::try_from(requested.unwrap_or(1)).unwrap_or(u32::MAX);
    Some(requested_u32.clamp(1, caps.max_subscribe_messages))
}

/// Clamps a requested `subscribe` timeout window: `None` (or non-finite)
/// becomes the 3.0s default `do_subscribe` uses today; the result is then
/// clamped into `[HARD_MIN_SUBSCRIBE_SECS.min(caps.max_subscribe_secs),
/// caps.max_subscribe_secs]`. The lower bound is lowered to the cap itself
/// (never raised above it) so a strict but positive `max_subscribe_secs`
/// below `HARD_MIN_SUBSCRIBE_SECS` is honored instead of panicking in
/// `clamp` (D-08 gap closure).
pub fn clamp_subscribe_window(requested: Option<f64>, caps: &EffectiveCaps) -> f64 {
    const DEFAULT_SUBSCRIBE_WINDOW_SECS: f64 = 3.0;
    let v = requested
        .filter(|v| v.is_finite())
        .unwrap_or(DEFAULT_SUBSCRIBE_WINDOW_SECS);
    let floor = HARD_MIN_SUBSCRIBE_SECS.min(caps.max_subscribe_secs);
    v.clamp(floor, caps.max_subscribe_secs)
}

/// A zero-velocity Twist (or, when `stamped`, a `TwistStamped` nesting one) —
/// the message [`crate::ros2::motion::run_timed_motion`] (49.8-04) always
/// sends [`STOP_REPEATS`] times before returning (D-09).
pub fn zero_twist(stamped: bool) -> Value {
    let twist = json!({
        "linear": {"x": 0.0, "y": 0.0, "z": 0.0},
        "angular": {"x": 0.0, "y": 0.0, "z": 0.0},
    });
    if stamped {
        json!({"twist": twist})
    } else {
        twist
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ros2_clamp_linear_velocity() {
        let caps = EffectiveCaps::from_config(&Ros2Caps::default());
        let mut msg = json!({"linear": {"x": 0.5}, "angular": {"z": -3.0}});
        let notes = clamp_twist(&mut msg, &caps, false);

        assert_eq!(msg["linear"]["x"], json!(0.10));
        assert_eq!(msg["angular"]["z"], json!(-0.5));
        assert_eq!(msg["linear"]["y"], json!(0.0));
        assert_eq!(msg["linear"]["z"], json!(0.0));
        assert_eq!(msg["angular"]["x"], json!(0.0));
        assert_eq!(msg["angular"]["y"], json!(0.0));

        assert_eq!(
            notes.len(),
            2,
            "expected exactly 2 clamp notes; got: {notes:?}"
        );
        assert!(notes.iter().any(|n| n.path == "linear.x"));
        assert!(notes.iter().any(|n| n.path == "angular.z"));
    }

    #[test]
    fn ros2_effective_caps_never_exceed_hard_ceilings() {
        let extreme = Ros2Caps {
            max_linear_velocity: 100.0,
            max_angular_velocity: 100.0,
            max_motion_duration_secs: 60.0,
            motion_publish_hz: 100.0,
            max_subscribe_messages: 999,
            max_subscribe_secs: 999.0,
            max_goal_secs: 9_999.0,
        };
        let caps = EffectiveCaps::from_config(&extreme);
        assert_eq!(caps.max_linear, HARD_MAX_LINEAR_VELOCITY);
        assert_eq!(caps.max_angular, HARD_MAX_ANGULAR_VELOCITY);
        assert_eq!(caps.max_motion_secs, HARD_MAX_MOTION_SECS);
        assert_eq!(caps.publish_hz, HARD_MAX_PUBLISH_HZ);
        assert_eq!(caps.max_subscribe_messages, HARD_MAX_SUBSCRIBE_MESSAGES);
        assert_eq!(caps.max_subscribe_secs, HARD_MAX_SUBSCRIBE_SECS);
        assert_eq!(caps.max_goal_secs, HARD_MAX_GOAL_SECS);

        let bad = Ros2Caps {
            max_linear_velocity: f64::NAN,
            max_angular_velocity: -1.0,
            max_motion_duration_secs: f64::NAN,
            motion_publish_hz: -5.0,
            ..Ros2Caps::default()
        };
        let caps2 = EffectiveCaps::from_config(&bad);
        let defaults = Ros2Caps::default();
        assert_eq!(caps2.max_linear, defaults.max_linear_velocity);
        assert_eq!(caps2.max_angular, defaults.max_angular_velocity);
        assert_eq!(caps2.max_motion_secs, defaults.max_motion_duration_secs);
        assert_eq!(caps2.publish_hz, defaults.motion_publish_hz);
    }

    #[test]
    fn ros2_field_limits_clamp_dotted_paths() {
        let mut limits: BTreeMap<String, [f64; 2]> = BTreeMap::new();
        limits.insert("position.z".to_string(), [100.0, 160.0]);
        limits.insert("orientation.roll".to_string(), [-0.2, 0.2]);
        limits.insert("battery.level".to_string(), [0.0, 100.0]); // absent from msg

        let mut msg = json!({
            "position": {"z": 300.0, "x": 5.0},
            "orientation": {"roll": -1.0},
        });
        let notes = apply_field_limits(&mut msg, &limits);

        assert_eq!(msg["position"]["z"], json!(160.0), "300 clamped to max 160");
        assert_eq!(
            msg["orientation"]["roll"],
            json!(-0.2),
            "-1.0 clamped to min -0.2"
        );
        assert_eq!(
            msg["position"]["x"],
            json!(5.0),
            "unlimited field must be untouched"
        );
        assert!(
            msg.get("battery").is_none(),
            "a path absent from msg must stay absent, not be invented"
        );
        assert_eq!(
            notes.len(),
            2,
            "expected exactly 2 clamp notes; got: {notes:?}"
        );

        // A non-numeric value at a limited path is replaced with the bound
        // nearest zero (100.0, not 160.0, for [100.0, 160.0]).
        let mut msg2 = json!({"position": {"z": "far"}});
        let notes2 = apply_field_limits(&mut msg2, &limits);
        assert_eq!(msg2["position"]["z"], json!(100.0));
        assert_eq!(notes2.len(), 1);
    }

    #[test]
    fn ros2_clamp_duration_bounds() {
        let caps = EffectiveCaps::from_config(&Ros2Caps::default());
        assert_eq!(clamp_duration(None, &caps), DEFAULT_MOTION_SECS);
        assert_eq!(clamp_duration(Some(0.0), &caps), HARD_MIN_MOTION_SECS);
        assert_eq!(clamp_duration(Some(60.0), &caps), caps.max_motion_secs);
    }

    #[test]
    fn ros2_zero_twist_shape() {
        let unstamped = zero_twist(false);
        for group in ["linear", "angular"] {
            for axis in ["x", "y", "z"] {
                assert_eq!(unstamped[group][axis], json!(0.0));
            }
        }
        assert!(unstamped.get("twist").is_none());

        let stamped = zero_twist(true);
        assert!(stamped.get("twist").is_some_and(Value::is_object));
        for group in ["linear", "angular"] {
            for axis in ["x", "y", "z"] {
                assert_eq!(stamped["twist"][group][axis], json!(0.0));
            }
        }
    }

    // -----------------------------------------------------------------
    // Gap closure 49.8-12 (D-08): an explicit 0 cap is honored, not
    // silently reverted to the (less restrictive) default.
    // -----------------------------------------------------------------

    #[test]
    fn ros2_effective_caps_explicit_zero_honored() {
        let caps = Ros2Caps {
            max_linear_velocity: 0.0,
            max_angular_velocity: -0.0,
            max_subscribe_messages: 0,
            ..Ros2Caps::default()
        };
        let effective = EffectiveCaps::from_config(&caps);
        assert_eq!(effective.max_linear, 0.0);
        assert!(effective.max_linear.is_sign_positive());
        assert_eq!(effective.max_angular, 0.0);
        assert!(effective.max_angular.is_sign_positive());
        assert_eq!(effective.max_subscribe_messages, 0);

        // Every other field equals the defaults' effective value.
        let defaults_effective = EffectiveCaps::from_config(&Ros2Caps::default());
        assert_eq!(
            effective.max_motion_secs,
            defaults_effective.max_motion_secs
        );
        assert_eq!(effective.publish_hz, defaults_effective.publish_hz);
        assert_eq!(
            effective.max_subscribe_secs,
            defaults_effective.max_subscribe_secs
        );
        assert_eq!(effective.max_goal_secs, defaults_effective.max_goal_secs);
    }

    #[test]
    fn ros2_effective_caps_absent_key_vs_explicit_zero_yaml() {
        // Absent `max_linear_velocity` key keeps the default; a present
        // `max_angular_velocity` is honored; other caps stay default.
        let yaml_absent = r#"
ros2:
  caps:
    max_angular_velocity: 0.2
"#;
        let cfg_absent: ironhermes_core::Config = serde_yaml::from_str(yaml_absent).unwrap();
        let effective_absent = EffectiveCaps::from_config(&cfg_absent.ros2.caps);
        assert_eq!(effective_absent.max_linear, 0.10);
        assert_eq!(effective_absent.max_angular, 0.2);
        assert_eq!(effective_absent.max_subscribe_messages, 20);

        // Explicit zeros (integer form for max_linear_velocity, float form
        // for max_angular_velocity, integer for max_subscribe_messages) are
        // all honored; an untouched cap (max_motion_duration_secs) keeps its
        // default.
        let yaml_explicit_zero = r#"
ros2:
  caps:
    max_linear_velocity: 0
    max_angular_velocity: 0.0
    max_subscribe_messages: 0
"#;
        let cfg_zero: ironhermes_core::Config = serde_yaml::from_str(yaml_explicit_zero).unwrap();
        let effective_zero = EffectiveCaps::from_config(&cfg_zero.ros2.caps);
        assert_eq!(effective_zero.max_linear, 0.0);
        assert_eq!(effective_zero.max_angular, 0.0);
        assert_eq!(effective_zero.max_subscribe_messages, 0);
        assert_eq!(effective_zero.max_motion_secs, 5.0);
    }

    #[test]
    fn ros2_clamp_twist_zero_cap_publishes_positive_zero() {
        let caps = EffectiveCaps::from_config(&Ros2Caps {
            max_linear_velocity: 0.0,
            max_angular_velocity: 0.0,
            ..Ros2Caps::default()
        });
        let mut msg = json!({"linear": {"x": -0.3}, "angular": {"z": 0.4}});
        let notes = clamp_twist(&mut msg, &caps, false);

        for group in ["linear", "angular"] {
            for axis in ["x", "y", "z"] {
                let v = msg[group][axis].as_f64().unwrap();
                assert_eq!(v, 0.0, "{group}.{axis} must be 0.0; got {v}");
                assert!(v.is_sign_positive(), "{group}.{axis} must not be -0.0");
            }
        }
        assert!(notes.iter().any(|n| n.path == "linear.x"));
        assert!(notes.iter().any(|n| n.path == "angular.z"));
    }

    #[test]
    fn ros2_subscribe_and_duration_clamps_honor_small_and_zero_caps() {
        let zero_caps = EffectiveCaps::from_config(&Ros2Caps {
            max_subscribe_messages: 0,
            ..Ros2Caps::default()
        });
        assert_eq!(clamp_subscribe_count(Some(5), &zero_caps), None);

        let default_caps = EffectiveCaps::from_config(&Ros2Caps::default());
        assert_eq!(clamp_subscribe_count(Some(50), &default_caps), Some(20));
        assert_eq!(clamp_subscribe_count(None, &default_caps), Some(1));
        assert_eq!(
            clamp_subscribe_count(Some(u64::MAX), &default_caps),
            Some(20)
        );

        let strict_window_caps = EffectiveCaps::from_config(&Ros2Caps {
            max_subscribe_secs: 0.1,
            ..Ros2Caps::default()
        });
        assert_eq!(clamp_subscribe_window(Some(9.0), &strict_window_caps), 0.1);
        assert_eq!(clamp_subscribe_window(None, &default_caps), 3.0);

        let strict_duration_caps = EffectiveCaps::from_config(&Ros2Caps {
            max_motion_duration_secs: 0.05,
            ..Ros2Caps::default()
        });
        assert_eq!(clamp_duration(Some(1.0), &strict_duration_caps), 0.05);

        // The existing bounds assertions still hold.
        assert_eq!(clamp_duration(None, &default_caps), DEFAULT_MOTION_SECS);
        assert_eq!(
            clamp_duration(Some(0.0), &default_caps),
            HARD_MIN_MOTION_SECS
        );
        assert_eq!(
            clamp_duration(Some(60.0), &default_caps),
            default_caps.max_motion_secs
        );
    }
}
