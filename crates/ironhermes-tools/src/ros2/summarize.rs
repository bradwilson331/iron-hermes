//! Sensor-message summaries, value truncation, and the one result-size cap
//! shared by every `ros2` action (Phase 49.8 D-13).
//!
//! `render_message` is [`super::read_ops::render_frame_or_message`]'s
//! delegate for every non-image message type: `sensor_msgs/LaserScan` and
//! `sensor_msgs/PointCloud2` get a purpose-built numeric summary (never their
//! raw `ranges`/`data` array); everything else is [`truncate_value`]'d and
//! rendered as compact JSON. [`cap_result`] is applied exactly once, in
//! `ros2_tool.rs`'s `execute()`, around every action's return value — parity
//! with `terminal.rs`'s `MAX_OUTPUT_LEN` truncation shape.
//!
//! Pure functions only — no I/O.

use base64::Engine as _;
use serde_json::{Map, Value, json};

/// D-13: arrays longer than this become a `{truncated_array, len, head}`
/// object instead of their full contents.
pub const MAX_ARRAY_ELEMENTS: usize = 32;
/// How many leading elements [`truncate_value`] keeps in `head` for a
/// truncated array.
pub const ARRAY_HEAD_ELEMENTS: usize = 8;
/// D-13: strings longer than this (by `char` count) are truncated.
pub const MAX_STRING_CHARS: usize = 256;
/// How many leading characters [`truncate_value`] keeps for a truncated
/// string.
pub const STRING_HEAD_CHARS: usize = 64;
/// D-13: recursion depth ceiling for [`truncate_value`] — a value nested
/// deeper than this is replaced with a depth-limit marker rather than
/// recursed into further.
pub const MAX_RENDER_DEPTH: usize = 16;

const SECTOR_LABELS: [&str; 8] = [
    "front",
    "front-left",
    "left",
    "back-left",
    "back",
    "back-right",
    "right",
    "front-right",
];

/// Dispatches on `msg_type` (via [`super::canonical_type`]): `LaserScan` and
/// `PointCloud2` get a numeric summary; everything else is
/// [`truncate_value`]'d and rendered as compact JSON. Never returns the raw
/// message for a large-array/large-string sensor type (D-13).
pub fn render_message(msg_type: &str, msg: &Value) -> String {
    match super::canonical_type(msg_type).as_str() {
        "sensor_msgs/LaserScan" => summarize_laserscan(msg),
        "sensor_msgs/PointCloud2" => summarize_pointcloud2(msg),
        _ => serde_json::to_string(&truncate_value(msg)).unwrap_or_default(),
    }
}

/// D-13: `sensor_msgs/LaserScan` -> beam count, valid count (excluding JSON
/// `null` — 49.8-02's non-finite sanitizer's no-return marker — and any
/// out-of-`[range_min, range_max]` value), nearest valid distance and its
/// bearing, and the nearest valid distance in each of 8 compass sectors —
/// never the raw `ranges` array. Bearings follow REP-103: `+x` forward,
/// counter-clockwise positive, so a positive bearing is to the left.
pub fn summarize_laserscan(msg: &Value) -> String {
    let angle_min = msg.get("angle_min").and_then(Value::as_f64).unwrap_or(0.0);
    let angle_increment = msg
        .get("angle_increment")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let range_min = msg.get("range_min").and_then(Value::as_f64).unwrap_or(0.0);
    let range_max = msg
        .get("range_max")
        .and_then(Value::as_f64)
        .unwrap_or(f64::INFINITY);
    let ranges = msg
        .get("ranges")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let beam_count = ranges.len();
    let mut valid_count = 0usize;
    // (distance_m, bearing_deg) — bearing is None when the beam's derived
    // angle overflowed to a non-finite value (CR-02).
    let mut nearest: Option<(f64, Option<f64>)> = None;
    let mut sector_nearest: [Option<f64>; 8] = [None; 8];
    let mut nonfinite_angles = false;

    for (i, r) in ranges.iter().enumerate() {
        let Some(dist) = r
            .as_f64()
            .filter(|d| d.is_finite() && *d >= range_min && *d <= range_max)
        else {
            continue;
        };
        valid_count += 1;

        let raw = angle_min + angle_increment * i as f64;
        match normalize_angle(raw) {
            Some(angle) => {
                let angle_deg = angle.to_degrees();

                if !nearest.is_some_and(|(d, _)| d <= dist) {
                    nearest = Some((dist, Some(angle_deg)));
                }

                let sector = sector_index(angle_deg);
                if !sector_nearest[sector].is_some_and(|d| d <= dist) {
                    sector_nearest[sector] = Some(dist);
                }
            }
            None => {
                // Non-finite derived angle: the beam still counts toward
                // valid/nearest (it has a real, in-range distance), but it
                // is never attributed to a sector and never rendered as a
                // real bearing (T-49.8-75 — never render a misleading
                // bearing/sector for a value that was never geometrically
                // real).
                nonfinite_angles = true;
                if !nearest.is_some_and(|(d, _)| d <= dist) {
                    nearest = Some((dist, None));
                }
            }
        }
    }

    let mut out = format!("LaserScan: {beam_count} beams, {valid_count} valid");
    match nearest {
        Some((dist, Some(bearing))) => {
            out.push_str(&format!(", nearest {dist:.2} m at {bearing:.0}°"))
        }
        Some((dist, None)) => out.push_str(&format!(", nearest {dist:.2} m (bearing unavailable)")),
        None => out.push_str(", no valid returns"),
    }

    out.push_str("\nSectors: ");
    let sector_strs: Vec<String> = SECTOR_LABELS
        .iter()
        .enumerate()
        .map(|(i, label)| match sector_nearest[i] {
            Some(d) => format!("{label}={d:.2}"),
            None => format!("{label}=—"),
        })
        .collect();
    out.push_str(&sector_strs.join(" "));

    if nonfinite_angles {
        out.push_str("\nWarning: non-finite scan angles — bearings and sectors are incomplete");
    }

    out
}

/// Maps a bearing in degrees (any real value) to one of the 8
/// [`SECTOR_LABELS`] indices, by nearest 45° center — `0` is `front`, a
/// positive bearing (left, REP-103) walks the array forward.
fn sector_index(angle_deg: f64) -> usize {
    let k = (angle_deg / 45.0).round() as i64;
    k.rem_euclid(8) as usize
}

/// Reduces `a` (radians) into `(-π, π]` in O(1), regardless of magnitude
/// (CR-02: the previous `while`-loop reduction never terminated once a
/// peer-supplied `angle_min`/`angle_increment` grew large enough that adding
/// or subtracting `TAU` became a no-op at that magnitude's ULP). Returns
/// `None` when `a` is not finite (NaN or +/-infinity) — such a value can only
/// arise from `angle_min + angle_increment * i` overflowing `f64::MAX` for a
/// hostile beam index, since JSON itself cannot carry NaN/Infinity directly
/// (49.8-02's sanitizer maps those tokens to `null` before this ever runs).
fn normalize_angle(a: f64) -> Option<f64> {
    if !a.is_finite() {
        return None;
    }
    let two_pi = std::f64::consts::TAU;
    let r = a.rem_euclid(two_pi);
    Some(if r > std::f64::consts::PI {
        r - two_pi
    } else {
        r
    })
}

/// `sensor_msgs/datatype` code (1-8) -> its `PointField` constant name.
fn point_field_datatype_name(code: u64) -> &'static str {
    match code {
        1 => "INT8",
        2 => "UINT8",
        3 => "INT16",
        4 => "UINT16",
        5 => "INT32",
        6 => "UINT32",
        7 => "FLOAT32",
        8 => "FLOAT64",
        _ => "UNKNOWN",
    }
}

/// D-13: `sensor_msgs/PointCloud2` -> point count (`width * height`),
/// dimensions, field `name:datatype` list, `point_step`/`row_step`/
/// `is_dense`, and the decoded byte length of `data` — never the raw
/// point buffer itself.
pub fn summarize_pointcloud2(msg: &Value) -> String {
    let width = msg.get("width").and_then(Value::as_u64).unwrap_or(0);
    let height = msg.get("height").and_then(Value::as_u64).unwrap_or(0);
    let point_count = width.saturating_mul(height);
    let point_step = msg.get("point_step").and_then(Value::as_u64).unwrap_or(0);
    let row_step = msg.get("row_step").and_then(Value::as_u64).unwrap_or(0);
    let is_dense = msg
        .get("is_dense")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let fields = msg
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let field_strs: Vec<String> = fields
        .iter()
        .map(|f| {
            let name = f.get("name").and_then(Value::as_str).unwrap_or("?");
            let datatype = f.get("datatype").and_then(Value::as_u64).unwrap_or(0);
            format!("{name}:{}", point_field_datatype_name(datatype))
        })
        .collect();

    let data_bytes = match msg.get("data") {
        Some(Value::String(s)) => base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map(|b| b.len())
            .unwrap_or(0),
        Some(Value::Array(a)) => a.len(),
        _ => 0,
    };

    format!(
        "PointCloud2: {point_count} points ({width}x{height}), fields: {}, point_step={point_step} \
         row_step={row_step} is_dense={is_dense} data={data_bytes} bytes",
        field_strs.join(", ")
    )
}

/// D-13: recursively truncates `v` — arrays longer than [`MAX_ARRAY_ELEMENTS`]
/// become `{"truncated_array": true, "len": N, "head": [..]}` (the first
/// [`ARRAY_HEAD_ELEMENTS`], each itself truncated); strings longer than
/// [`MAX_STRING_CHARS`] (by `char` count) become their first
/// [`STRING_HEAD_CHARS`] characters plus a `…({N} chars)` marker; recursion
/// past [`MAX_RENDER_DEPTH`] is replaced by a depth-limit marker string
/// rather than descending further. Never invents structure absent from `v`.
pub fn truncate_value(v: &Value) -> Value {
    truncate_value_at_depth(v, 0)
}

fn truncate_value_at_depth(v: &Value, depth: usize) -> Value {
    if depth > MAX_RENDER_DEPTH {
        return json!("…(depth limit)");
    }
    match v {
        Value::Array(arr) => {
            if arr.len() > MAX_ARRAY_ELEMENTS {
                let head: Vec<Value> = arr
                    .iter()
                    .take(ARRAY_HEAD_ELEMENTS)
                    .map(|item| truncate_value_at_depth(item, depth + 1))
                    .collect();
                json!({"truncated_array": true, "len": arr.len(), "head": head})
            } else {
                Value::Array(
                    arr.iter()
                        .map(|item| truncate_value_at_depth(item, depth + 1))
                        .collect(),
                )
            }
        }
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            for (k, val) in map {
                out.insert(k.clone(), truncate_value_at_depth(val, depth + 1));
            }
            Value::Object(out)
        }
        Value::String(s) => {
            let char_count = s.chars().count();
            if char_count > MAX_STRING_CHARS {
                let head: String = s.chars().take(STRING_HEAD_CHARS).collect();
                json!(format!("{head}…({char_count} chars)"))
            } else {
                Value::String(s.clone())
            }
        }
        other => other.clone(),
    }
}

/// D-13: the one result-size cap shared by every `ros2` action — parity with
/// `terminal.rs`'s `MAX_OUTPUT_LEN`. When `s` is longer than
/// [`super::ROS2_MAX_RESULT_CHARS`] bytes, keeps a char-boundary-safe prefix
/// and appends a marker naming how many characters were cut.
pub fn cap_result(s: String) -> String {
    if s.len() <= super::ROS2_MAX_RESULT_CHARS {
        return s;
    }
    let mut end = super::ROS2_MAX_RESULT_CHARS;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let remaining = s[end..].chars().count();
    format!("{}\n…[truncated {remaining} more characters]", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ros2_summarize_laserscan() {
        let beam_count = 360;
        let mut ranges: Vec<Value> = (0..beam_count).map(|_| json!(2.0)).collect();
        // Straight-ahead obstacle: angle_min=-pi, angle_increment=1deg, so
        // beam index 180 sits at angle -pi + 180deg = 0deg (front).
        ranges[180] = json!(0.5);
        // A handful of no-return beams (49.8-02's non-finite sanitizer emits
        // JSON null for inf/NaN).
        ranges[10] = Value::Null;
        ranges[20] = Value::Null;
        ranges[30] = Value::Null;
        // One out-of-range value beyond range_max=12.0.
        ranges[50] = json!(50.0);

        let msg = json!({
            "angle_min": -std::f64::consts::PI,
            "angle_increment": 1.0_f64.to_radians(),
            "range_min": 0.1,
            "range_max": 12.0,
            "ranges": ranges,
        });

        let summary = summarize_laserscan(&msg);
        assert!(summary.contains("360 beams"), "got: {summary}");
        assert!(
            summary.contains("356 valid"),
            "expected 360 - 3 nulls - 1 out-of-range = 356 valid; got: {summary}"
        );
        assert!(summary.contains("nearest 0.50 m"), "got: {summary}");
        assert!(summary.contains("front=0.50"), "got: {summary}");
    }

    #[test]
    fn ros2_summarize_laserscan_all_invalid() {
        let ranges: Vec<Value> = (0..10).map(|_| Value::Null).collect();
        let msg = json!({
            "angle_min": -std::f64::consts::PI,
            "angle_increment": 1.0_f64.to_radians(),
            "range_min": 0.1,
            "range_max": 12.0,
            "ranges": ranges,
        });
        let summary = summarize_laserscan(&msg);
        assert!(summary.contains("no valid returns"), "got: {summary}");
        assert!(summary.contains("10 beams"), "got: {summary}");
    }

    #[test]
    fn ros2_summarize_pointcloud2() {
        let point_step = 12u64; // 3 FLOAT32 fields * 4 bytes
        let width = 640u64;
        let height = 480u64;
        let raw = vec![0u8; (point_step * width) as usize];
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        let expected_bytes = raw.len();

        let msg = json!({
            "width": width,
            "height": height,
            "point_step": point_step,
            "row_step": point_step * width,
            "is_dense": true,
            "fields": [
                {"name": "x", "datatype": 7, "offset": 0, "count": 1},
                {"name": "y", "datatype": 7, "offset": 4, "count": 1},
                {"name": "z", "datatype": 7, "offset": 8, "count": 1},
            ],
            "data": data_b64,
        });

        let summary = summarize_pointcloud2(&msg);
        assert!(summary.contains("307200 points"), "got: {summary}");
        assert!(
            summary.contains("x:FLOAT32")
                && summary.contains("y:FLOAT32")
                && summary.contains("z:FLOAT32"),
            "got: {summary}"
        );
        assert!(
            summary.contains(&format!("data={expected_bytes} bytes")),
            "got: {summary}"
        );
        assert!(
            !summary.contains(&data_b64),
            "the raw base64 data must never appear in the summary"
        );
    }

    #[test]
    fn ros2_truncate_large_arrays() {
        let big_array: Vec<Value> = (0..1000).map(|i| json!(i)).collect();
        let truncated = truncate_value(&json!(big_array));
        assert_eq!(truncated["truncated_array"], json!(true));
        assert_eq!(truncated["len"], json!(1000));
        assert_eq!(
            truncated["head"].as_array().map(|a| a.len()),
            Some(ARRAY_HEAD_ELEMENTS)
        );

        let big_string = "x".repeat(10_000);
        let truncated_string = truncate_value(&json!(big_string));
        let s = truncated_string
            .as_str()
            .expect("string stays a JSON string");
        assert!(s.starts_with(&"x".repeat(STRING_HEAD_CHARS)));
        assert!(s.contains("(10000 chars)"), "got: {s}");

        // Nesting beyond MAX_RENDER_DEPTH is replaced by a depth marker
        // rather than recursed into further.
        let mut nested = json!("bottom");
        for _ in 0..(MAX_RENDER_DEPTH + 10) {
            nested = json!([nested]);
        }
        let truncated_nested = truncate_value(&nested);
        let rendered = serde_json::to_string(&truncated_nested).unwrap();
        assert!(rendered.contains("depth limit"), "got: {rendered}");
        assert!(!rendered.contains("bottom"), "got: {rendered}");
    }

    // -----------------------------------------------------------------
    // CR-02 regression tests (Phase 49.8-20). None of these may run the
    // reduction on the test thread itself — a reintroduced unbounded loop
    // would hang the harness, not just fail an assertion. Each spawns a
    // `std::thread` and waits with `recv_timeout`, so a regression fails at
    // the deadline and the abandoned thread ends only when the test process
    // exits.
    // -----------------------------------------------------------------

    #[test]
    fn ros2_normalize_angle_huge_finite_input_is_bounded() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let ranges: Vec<Value> = (0..10).map(|_| json!(1.0)).collect();
            let msg = json!({
                "angle_min": 1e308,
                "angle_increment": 0.01,
                "range_min": 0.1,
                "range_max": 12.0,
                "ranges": ranges,
            });
            let summary = summarize_laserscan(&msg);
            let _ = tx.send(summary);
        });

        let summary = rx.recv_timeout(std::time::Duration::from_secs(2)).expect(
            "summarize_laserscan must return within 2s for a huge finite angle_min (CR-02) \
             — a reintroduced unbounded loop hangs the reduction instead",
        );
        assert!(summary.contains("10 beams"), "got: {summary}");
        assert!(summary.contains("10 valid"), "got: {summary}");
    }

    #[test]
    fn ros2_summarize_laserscan_overflow_to_infinity_is_reported() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let ranges = vec![json!(1.0), json!(2.0), json!(3.0), json!(4.0)];
            let msg = json!({
                "angle_min": 1e308,
                "angle_increment": 1e308,
                "range_min": 0.1,
                "range_max": 12.0,
                "ranges": ranges,
            });
            let summary = summarize_laserscan(&msg);
            let _ = tx.send(summary);
        });

        let summary = rx.recv_timeout(std::time::Duration::from_secs(2)).expect(
            "summarize_laserscan must return within 2s when angle_min + angle_increment * i \
             overflows to infinity (CR-02)",
        );
        assert!(summary.contains("4 valid"), "got: {summary}");
        assert!(summary.contains("non-finite scan angles"), "got: {summary}");
    }

    #[test]
    fn ros2_summarize_laserscan_nearest_on_overflowed_beam_has_no_bearing() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let ranges = vec![json!(3.0), json!(1.0)];
            let msg = json!({
                "angle_min": 1e308,
                "angle_increment": 1e308,
                "range_min": 0.1,
                "range_max": 12.0,
                "ranges": ranges,
            });
            let summary = summarize_laserscan(&msg);
            let _ = tx.send(summary);
        });

        let summary = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("summarize_laserscan must return within 2s (CR-02)");
        assert!(
            summary.contains("nearest 1.00 m (bearing unavailable)"),
            "got: {summary}"
        );
        assert!(!summary.contains("nearest 1.00 m at"), "got: {summary}");
        assert!(summary.contains("non-finite scan angles"), "got: {summary}");
    }

    #[test]
    fn ros2_normalize_angle_reference_values() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::f64::consts::PI;

            assert_eq!(normalize_angle(-PI), Some(PI));
            assert_eq!(normalize_angle(PI), Some(PI));
            assert_eq!(normalize_angle(0.0), Some(0.0));

            let a = normalize_angle(3.0 * PI / 2.0).expect("finite input must return Some");
            assert!((a - (-PI / 2.0)).abs() < 1e-12, "got: {a}");
            let b = normalize_angle(-3.0 * PI / 2.0).expect("finite input must return Some");
            assert!((b - (PI / 2.0)).abs() < 1e-12, "got: {b}");

            assert_eq!(normalize_angle(f64::INFINITY), None);
            assert_eq!(normalize_angle(f64::NEG_INFINITY), None);
            assert_eq!(normalize_angle(f64::NAN), None);

            let huge = normalize_angle(1e308).expect("finite input must return Some");
            assert!(huge > -PI && huge <= PI, "got: {huge}");

            let _ = tx.send(());
        });

        rx.recv_timeout(std::time::Duration::from_secs(2)).expect(
            "normalize_angle reference-value assertions must complete within 2s (CR-02) — \
             recv_timeout errors on either a hang or a propagated thread panic",
        );
    }

    #[test]
    fn ros2_result_cap_50000() {
        let long = "a".repeat(60_000);
        let capped = cap_result(long);
        let kept_len = capped.split('\n').next().unwrap().len();
        assert!(
            kept_len <= super::super::ROS2_MAX_RESULT_CHARS,
            "kept {kept_len} bytes"
        );
        assert!(
            capped.contains("truncated"),
            "got a truncation marker: {capped}"
        );

        // Multi-byte characters must never be cut mid-character.
        let multi_byte = "é".repeat(30_000); // 2 bytes each -> 60,000 bytes
        let capped_multi = cap_result(multi_byte);
        assert!(capped_multi.is_char_boundary(capped_multi.len().min(capped_multi.len())));
        // Re-parsing as UTF-8 would panic on a mid-character split; the fact
        // this string exists at all proves the split was char-boundary-safe.
        assert!(capped_multi.chars().count() > 0);
    }
}
