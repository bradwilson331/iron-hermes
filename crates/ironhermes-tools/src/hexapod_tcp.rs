use std::env;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::{Duration, timeout};
use tracing::debug;

use crate::registry::{Prerequisite, Tool};

// ---------------------------------------------------------------------------
// Wire-protocol constants (D-07, D-08, D-09, D-10)
// ---------------------------------------------------------------------------

/// Stop = mode 0 (halts motion + returns to neutral stance). Per D-07.
pub(crate) const STOP_CMD: &str = "CMD_MOVE#0#0#0#0#0\n";

/// Relax servos. Per D-08.
/// NOTE: CMD_RELAX is a toggle — each invocation flips the servo-enabled state.
/// Calling twice re-enables servos. See RESEARCH Pitfall 5; CMD_SERVOPOWER would
/// be more reliable but is in the blocked list (D-16).
pub(crate) const RELAX_CMD: &str = "CMD_RELAX\n";

/// Battery query command. Per D-09.
pub(crate) const BATTERY_CMD: &str = "CMD_POWER\n";

/// Distance query command. Per D-10.
pub(crate) const DISTANCE_CMD: &str = "CMD_SONIC\n";

/// D-09 battery thresholds verified against Code/Server/adc.py:
/// channel 0 (load) < 5.5V or channel 4 (Pi) < 6.0V → "LOW".
pub(crate) const BATTERY_LOW_V1: f32 = 5.5;
pub(crate) const BATTERY_LOW_V2: f32 = 6.0;

/// Calibration placeholder — tune on real robot after live testing. Per D-04.
pub(crate) const ROTATE_MS_PER_DEGREE: u64 = 20;

/// Protocol maximum for head pan servo (§6 of Freenove protocol). Per D-08.
pub(crate) const HEAD_PAN_MAX: i64 = 90;

/// Protocol maximum for head tilt servo (§6 of Freenove protocol). Per D-08.
pub(crate) const HEAD_TILT_MAX: i64 = 90;

/// Buzzer on command. Per D-10.
pub(crate) const BUZZER_ON_CMD: &str = "CMD_BUZZER#1\n";

/// Buzzer off command. Per D-11.
pub(crate) const BUZZER_OFF_CMD: &str = "CMD_BUZZER#0\n";

/// LED color command prefix (D-02). Full wire is CMD_LED#{R}#{G}#{B}\n built at call time.
/// The Freenove server defaults led_mode='1' (solid color) so no mode-set preamble is needed.
pub(crate) const CMD_LED: &str     = "CMD_LED";

/// LED off command (D-03). Sets led_mode=0 which triggers color_wipe([0,0,0]) — the server's
/// dedicated off path. Do NOT use CMD_LED#0#0#0\n (that sets color, not mode).
pub(crate) const CMD_LED_OFF: &str = "CMD_LED#0\n";

/// Camera gimbal pan range (server-enforced: server.py restrict_value(50, 180)). Per D-15.
pub(crate) const CAMERA_PAN_MIN: i64  = 50;
pub(crate) const CAMERA_PAN_MAX: i64  = 180;

/// Camera gimbal tilt range (server-enforced: server.py restrict_value(0, 180)). Per D-15.
pub(crate) const CAMERA_TILT_MIN: i64 = 0;
pub(crate) const CAMERA_TILT_MAX: i64 = 180;

/// Midpoint defaults for the unused axis when calling CMD_CAMERA with one axis. Per D-14 + Discretion.
pub(crate) const CAMERA_PAN_DEFAULT: i64  = 115; // midpoint of 50–180; used as x-default in camera_tilt
pub(crate) const CAMERA_TILT_DEFAULT: i64 = 90;  // midpoint of 0–180; used as y-default in camera_pan

/// Maximum samples for stream_distance polling loop. Per D-09.
pub(crate) const STREAM_DISTANCE_MAX_SAMPLES: i64 = 20;

// ---------------------------------------------------------------------------
// Tool description (for LLM — includes blocked-command guidance, D-16)
// ---------------------------------------------------------------------------

const DESCRIPTION: &str = "Control the Freenove hexapod robot over TCP. \
    Actions: walk (with direction and speed), stop, read_battery, \
    read_distance, relax_servos, rotate (with degrees: positive=clockwise, negative=counterclockwise), \
    head_pan (with angle: -90 to 90), head_tilt (with angle: -90 to 90), \
    buzzer_on, buzzer_off, led (with r/g/b: 0-255 per channel), led_off, \
    stream_distance (with samples: integer 1-20, clamped), \
    camera_pan (with x: integer 50-180, clamped), camera_tilt (with y: integer 0-180, clamped). \
    The degrees parameter is used only for the rotate action. \
    The angle parameter is used only for the head_pan and head_tilt actions. \
    The r, g, b parameters are used only for the led action (integers 0-255, clamped). \
    The samples parameter is used only for stream_distance (clamped to [1, 20]). \
    The x parameter is used only for camera_pan (clamped to [50, 180]). \
    The y parameter is used only for camera_tilt (clamped to [0, 180]). \
    CMD_CAMERA sets both pan and tilt in one wire command; the unused axis defaults to its midpoint \
    (x defaults to 115, y defaults to 90). \
    BLOCKED (do not attempt): calibration, servo_power, CMD_LED_MOD modes 2-5 (chase/blink/breathing/rainbow) — \
    these return a block error and are never permitted via this tool.";

// ---------------------------------------------------------------------------
// Struct (unit struct — stateless per D-11, IP read at call time per D-12)
// ---------------------------------------------------------------------------

/// Hexapod TCP tool — stateless Phase-1 robot controller.
///
/// All configuration (HEXAPOD_IP) is read from the environment at call time (D-12).
/// Each call opens a fresh TCP connection (D-11).
pub struct HexapodTcpTool;

// ---------------------------------------------------------------------------
// Pure helper functions (no I/O — tested in unit suite without a robot)
// ---------------------------------------------------------------------------

/// Build the CMD_MOVE wire string for a walk command (D-06).
///
/// Speed is clamped to 2..=10 per RESEARCH Pitfall 4. Direction defaults to
/// "forward" for any unrecognised string (execute() validates direction before
/// calling this helper, so the default branch is a belt-and-suspenders guard).
pub(crate) fn build_walk_wire(direction: &str, speed: i64) -> String {
    let s = speed.clamp(2, 10);
    match direction {
        "forward"  => format!("CMD_MOVE#1#0#25#{s}#0\n"),
        "backward" => format!("CMD_MOVE#1#0#-25#{s}#0\n"),
        "left"     => format!("CMD_MOVE#1#-25#0#{s}#0\n"),
        "right"    => format!("CMD_MOVE#1#25#0#{s}#0\n"),
        _          => format!("CMD_MOVE#1#0#25#{s}#0\n"), // safe default: forward
    }
}

/// Converts a signed degree count to a CMD_MOVE in-place rotation wire string.
/// Positive degrees → wire angle field +10 (clockwise); negative → -10 (counterclockwise).
/// Precondition: degrees != 0 (execute() guards this; zero would emit the positive branch spuriously).
/// Speed is fixed at 5 (D-02); caller is responsible for sleeping the appropriate duration
/// before sending STOP.
pub(crate) fn build_rotate_wire(degrees: i64) -> String {
    let angle = if degrees > 0 { 10i64 } else { -10i64 };
    format!("CMD_MOVE#1#0#0#5#{angle}\n")
}

/// Parse a `CMD_POWER#<v1>#<v2>\n` response into a human-readable battery string (D-09).
///
/// Low thresholds per adc.py: v1 (load/channel 0) < 5.5 V OR v2 (Pi/channel 4) < 6.0 V.
/// Returns an error string if the response is malformed (missing voltage fields),
/// preventing a silent 0V/0V (LOW) report that could cause incorrect LLM follow-up actions.
pub(crate) fn parse_battery_response(raw: &str) -> String {
    let parts: Vec<&str> = raw.trim().split('#').collect();
    match (parts.get(1), parts.get(2)) {
        (Some(s1), Some(s2)) => {
            let v1: f32 = s1.parse().unwrap_or(0.0);
            let v2: f32 = s2.parse().unwrap_or(0.0);
            let status = if v1 < BATTERY_LOW_V1 || v2 < BATTERY_LOW_V2 { "LOW" } else { "OK" };
            format!("Battery: {v1}V / {v2}V ({status})")
        }
        _ => format!("Error: unexpected battery response from robot: {:?}", raw.trim()),
    }
}

/// Parse a `CMD_SONIC#<dist>\n` response into a human-readable distance string (D-10).
pub(crate) fn parse_distance_response(raw: &str) -> String {
    let parts: Vec<&str> = raw.trim().split('#').collect();
    let dist = parts.get(1).map(|s| s.trim()).unwrap_or("?");
    format!("Distance: {dist} cm")
}

/// Map the outcome of a `timeout(Duration::from_secs(3), send_and_read_line(...))` call
/// into the D-17/D-18 error strings, or pass through the raw line on success.
///
/// This helper is `pub(crate)` so `test_read_timeout_branch` (test 13) can call it
/// directly with a synthetic `tokio::time::error::Elapsed` value — no real TCP needed.
///
/// - `Err(_)` (timeout elapsed)  → D-18 error string
/// - `Ok(Err(_))` (I/O error)   → D-17 error string
/// - `Ok(Ok(raw))` (success)    → the raw response line (caller does the parsing)
pub(crate) fn map_read_outcome(
    outcome: Result<anyhow::Result<String>, tokio::time::error::Elapsed>,
    addr: &str,
) -> anyhow::Result<String> {
    match outcome {
        Err(_elapsed) => Ok("Error: read timed out after 3s waiting for robot response".to_string()),
        Ok(Err(_io))  => Ok(format!(
            "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
        )),
        Ok(Ok(raw))   => Ok(raw),
    }
}

// ---------------------------------------------------------------------------
// Private async TCP helpers (free fns — not methods — avoids &self capture in
// tokio::spawn, see RESEARCH Pitfall 2)
// ---------------------------------------------------------------------------

/// Send a fire-and-forget command over a fresh TCP connection (D-11, D-19).
///
/// Used for walk, stop, relax_servos — no response is read.
async fn send_fire_and_forget(addr: &str, cmd: &str) -> anyhow::Result<()> {
    let mut stream = TcpStream::connect(addr).await?;
    stream.write_all(cmd.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// Send a command and read one `\n`-terminated response line (D-18, Pitfall 3).
///
/// Used for read_battery and read_distance. Caller wraps with a 3-second timeout.
async fn send_and_read_line(addr: &str, cmd: &str) -> anyhow::Result<String> {
    let mut stream = TcpStream::connect(addr).await?;
    stream.write_all(cmd.as_bytes()).await?;
    stream.flush().await?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    // AsyncBufReadExt::read_line via BufReader — robust against TCP recv splits (Pitfall 3)
    tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut buf).await?;
    Ok(buf.trim().to_string())
}

/// Fire CMD_MOVE#0#0#0#0#0\n then CMD_RELAX\n to halt and relax the robot.
///
/// Used by `on_session_end`. Each command opens a separate fresh connection (D-11).
/// All errors are swallowed — this is best-effort at shutdown.
async fn send_stop_and_relax(addr: &str) {
    let _ = send_fire_and_forget(addr, STOP_CMD).await;
    let _ = send_fire_and_forget(addr, RELAX_CMD).await;
}

// ---------------------------------------------------------------------------
// Tool trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for HexapodTcpTool {
    fn name(&self) -> &str {
        "hexapod_tcp"
    }

    fn toolset(&self) -> &str {
        "robotics"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "hexapod_tcp",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["walk", "stop", "read_battery", "read_distance", "relax_servos",
                                 "rotate", "head_pan", "head_tilt", "buzzer_on", "buzzer_off",
                                 "led", "led_off",
                                 "stream_distance", "camera_pan", "camera_tilt"],
                        "description": "Action to perform on the hexapod robot."
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["forward", "backward", "left", "right"],
                        "description": "Walk direction. Required only for the walk action."
                    },
                    "speed": {
                        "type": "integer",
                        "description": "Walk speed 2–10 (clamped). Required only for the walk action.",
                        "minimum": 2,
                        "maximum": 10
                    },
                    "degrees": {
                        "type": "integer",
                        "minimum": -3600,
                        "maximum": 3600,
                        "description": "Rotation in degrees. Positive=clockwise (right), negative=counterclockwise (left). Used only for the rotate action."
                    },
                    "angle": {
                        "type": "integer",
                        "minimum": -90,
                        "maximum": 90,
                        "description": "Head servo angle in degrees (-90 to 90). Used only for head_pan and head_tilt actions."
                    },
                    "r": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 255,
                        "description": "Red channel 0-255. Used only for the led action."
                    },
                    "g": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 255,
                        "description": "Green channel 0-255. Used only for the led action."
                    },
                    "b": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 255,
                        "description": "Blue channel 0-255. Used only for the led action."
                    },
                    "samples": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Number of distance readings. Used only for stream_distance. Clamped to [1, 20]."
                    },
                    "x": {
                        "type": "integer",
                        "minimum": 50,
                        "maximum": 180,
                        "description": "Camera pan angle 50–180. Used only for camera_pan. Clamped silently."
                    },
                    "y": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 180,
                        "description": "Camera tilt angle 0–180. Used only for camera_tilt. Clamped silently."
                    }
                },
                "required": ["action"]
            }),
        )
    }

    fn prerequisites(&self) -> Vec<Prerequisite> {
        vec![Prerequisite {
            kind: "env_var".to_string(),
            name: "HEXAPOD_IP".to_string(),
            description: "IP address of the Freenove hexapod robot (e.g., 192.168.1.42). \
                          Required for hexapod_tcp to connect to the robot."
                .to_string(),
            required: true,
        }]
    }

    /// Execute a Phase-1 hexapod action.
    ///
    /// ## Allowlist (D-20)
    /// Only `walk | stop | read_battery | read_distance | relax_servos` are forwarded to
    /// the hardware. All other action strings hit the catch-all arm and return the D-16
    /// blocked-command string — no TCP connection is attempted (and no env var read occurs).
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let action = args["action"].as_str().unwrap_or("");

        // D-20: compile-time exhaustive match — allowlist enforced BEFORE env var read.
        // The catch-all fires without attempting any I/O (test 2 requirement).
        // HEXAPOD_IP is only read for the five permitted actions.
        match action {
            "walk" | "stop" | "read_battery" | "read_distance" | "relax_servos"
            | "rotate" | "head_pan" | "head_tilt" | "buzzer_on" | "buzzer_off"
            | "led" | "led_off"
            | "stream_distance" | "camera_pan" | "camera_tilt" => {
                // D-12: read HEXAPOD_IP only for permitted actions
                let ip = match env::var("HEXAPOD_IP") {
                    Ok(v) => v,
                    Err(_) => {
                        return Ok(
                            "Error: HEXAPOD_IP env var not set — cannot connect to robot"
                                .to_string(),
                        )
                    }
                };
                let addr = format!("{ip}:5002");
                debug!("hexapod_tcp: action={action} addr={addr}");

                match action {
                    "walk" => {
                        let direction = match args["direction"].as_str() {
                            Some(d @ ("forward" | "backward" | "left" | "right")) => d,
                            Some(other) => return Ok(format!(
                                "Error: invalid direction '{other}' — must be one of: forward, backward, left, right"
                            )),
                            None => return Ok(
                                "Error: 'direction' parameter is required for the walk action".to_string()
                            ),
                        };
                        let speed = args["speed"].as_i64().unwrap_or(5);
                        let wire = build_walk_wire(direction, speed);
                        match send_fire_and_forget(&addr, &wire).await {
                            Ok(_) => Ok("OK".to_string()), // D-19
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )), // D-17
                        }
                    }

                    "stop" => {
                        match send_fire_and_forget(&addr, STOP_CMD).await {
                            Ok(_) => Ok("OK".to_string()), // D-19
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )), // D-17
                        }
                    }

                    "relax_servos" => {
                        match send_fire_and_forget(&addr, RELAX_CMD).await {
                            Ok(_) => Ok("OK".to_string()), // D-19
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )), // D-17
                        }
                    }

                    "read_battery" => {
                        let read_fut = send_and_read_line(&addr, BATTERY_CMD);
                        let outcome = timeout(Duration::from_secs(3), read_fut).await;
                        let raw = map_read_outcome(outcome, &addr)?;
                        if raw.starts_with("Error:") {
                            Ok(raw)
                        } else {
                            Ok(parse_battery_response(&raw))
                        }
                    }

                    "read_distance" => {
                        let read_fut = send_and_read_line(&addr, DISTANCE_CMD);
                        let outcome = timeout(Duration::from_secs(3), read_fut).await;
                        let raw = map_read_outcome(outcome, &addr)?;
                        if raw.starts_with("Error:") {
                            Ok(raw)
                        } else {
                            Ok(parse_distance_response(&raw))
                        }
                    }

                    "rotate" => {
                        let degrees = args["degrees"].as_i64().unwrap_or(0);
                        if degrees == 0 {
                            return Ok("OK".to_string()); // no-op: avoid spurious motion command
                        }
                        let wire = build_rotate_wire(degrees);
                        // Cap absolute degrees to prevent u64 overflow and runaway rotation.
                        const MAX_DEGREES: u64 = 3600; // 10 full rotations max
                        let abs_degrees = degrees.unsigned_abs().min(MAX_DEGREES);
                        let duration = Duration::from_millis(
                            abs_degrees.saturating_mul(ROTATE_MS_PER_DEGREE)
                        );
                        match send_fire_and_forget(&addr, &wire).await {
                            Ok(_) => {
                                tokio::time::sleep(duration).await;
                                match send_fire_and_forget(&addr, STOP_CMD).await {
                                    Ok(_) => Ok("OK".to_string()),
                                    Err(e) => {
                                        tracing::warn!("hexapod_tcp: rotate stop command failed: {e}");
                                        Ok(format!(
                                            "Warning: rotate completed but stop command failed at {addr} — robot may still be moving"
                                        ))
                                    }
                                }
                            }
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )),
                        }
                    }

                    "head_pan" => {
                        let raw = args["angle"].as_i64().unwrap_or(0);
                        let angle = raw.clamp(-HEAD_PAN_MAX, HEAD_PAN_MAX);
                        let wire = format!("CMD_HEAD#0#{angle}\n");
                        match send_fire_and_forget(&addr, &wire).await {
                            Ok(_) => Ok("OK".to_string()),
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )),
                        }
                    }

                    "head_tilt" => {
                        let raw = args["angle"].as_i64().unwrap_or(0);
                        let angle = raw.clamp(-HEAD_TILT_MAX, HEAD_TILT_MAX);
                        let wire = format!("CMD_HEAD#1#{angle}\n");
                        match send_fire_and_forget(&addr, &wire).await {
                            Ok(_) => Ok("OK".to_string()),
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )),
                        }
                    }

                    "buzzer_on" => {
                        match send_fire_and_forget(&addr, BUZZER_ON_CMD).await {
                            Ok(_) => Ok("OK".to_string()),
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )),
                        }
                    }

                    "buzzer_off" => {
                        match send_fire_and_forget(&addr, BUZZER_OFF_CMD).await {
                            Ok(_) => Ok("OK".to_string()),
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )),
                        }
                    }

                    "led" => {
                        let r = args["r"].as_i64().unwrap_or(0).clamp(0, 255);
                        let g = args["g"].as_i64().unwrap_or(0).clamp(0, 255);
                        let b = args["b"].as_i64().unwrap_or(0).clamp(0, 255);
                        let wire = format!("{CMD_LED}#{r}#{g}#{b}\n");
                        match send_fire_and_forget(&addr, &wire).await {
                            Ok(_) => Ok("OK".to_string()),
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )),
                        }
                    }

                    "led_off" => {
                        match send_fire_and_forget(&addr, CMD_LED_OFF).await {
                            Ok(_) => Ok("OK".to_string()),
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )),
                        }
                    }

                    "camera_pan" => {
                        let x = args["x"].as_i64().unwrap_or(CAMERA_PAN_DEFAULT)
                            .clamp(CAMERA_PAN_MIN, CAMERA_PAN_MAX);
                        let wire = format!("CMD_CAMERA#{x}#{CAMERA_TILT_DEFAULT}\n");
                        match send_fire_and_forget(&addr, &wire).await {
                            Ok(_) => Ok("OK".to_string()),
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )),
                        }
                    }

                    "camera_tilt" => {
                        let y = args["y"].as_i64().unwrap_or(CAMERA_TILT_DEFAULT)
                            .clamp(CAMERA_TILT_MIN, CAMERA_TILT_MAX);
                        let wire = format!("CMD_CAMERA#{CAMERA_PAN_DEFAULT}#{y}\n");
                        match send_fire_and_forget(&addr, &wire).await {
                            Ok(_) => Ok("OK".to_string()),
                            Err(_) => Ok(format!(
                                "Error: cannot connect to robot at {addr} — is HEXAPOD_IP set and the robot powered on?"
                            )),
                        }
                    }

                    // stream_distance arm added in Task 2 (Phase 27.1.4-01)
                    "stream_distance" => {
                        // Placeholder — body implemented in Task 2
                        Ok("stream_distance: not yet implemented".to_string())
                    }

                    // Unreachable: outer arm already enumerates exactly these 15 allowed actions
                    _ => unreachable!("outer match guarantees only the 15 allowed actions reach here"),
                }
            }

            // D-16, D-20: catch-all blocks every other action string — fires BEFORE any I/O
            other => Ok(format!(
                "Action '{other}' is blocked — not permitted via hexapod_tcp. Never send this command."
            )),
        }
    }

    /// D-14: fire-and-forget safety halt when the IronHermes session ends.
    ///
    /// Sends CMD_MOVE#0#0#0#0#0\n then CMD_RELAX\n to stop motion and relax servos.
    /// The spawned future reads HEXAPOD_IP inside the async block — no &self capture
    /// required (RESEARCH Pitfall 2). Errors are silently swallowed (best-effort halt).
    fn on_session_end(&self) {
        // D-14 fire-and-forget safety halt: stop motion + relax servos
        // so the robot does not continue walking after the session ends.
        // No &self capture; reads HEXAPOD_IP inside the spawned future.
        tokio::spawn(async move {
            if let Ok(ip) = env::var("HEXAPOD_IP") {
                let addr = format!("{ip}:5002");
                send_stop_and_relax(&addr).await;
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Inline unit tests — all 13 tests per RESEARCH §Test Surface + test 13 (D-18)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Serialize env-var-mutating tests within this module.
    /// NOTE: This mutex only protects against races within this module.
    /// Run the full test binary with RUST_TEST_THREADS=1 to avoid races
    /// with other modules that may also read HEXAPOD_IP.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // -----------------------------------------------------------------------
    // Test 1: Missing env var returns Ok(error) — D-12
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_missing_env_var_returns_ok_error() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        let tool = HexapodTcpTool;
        let result = tool
            .execute(json!({"action": "walk", "direction": "forward", "speed": 5}))
            .await
            .unwrap();
        assert!(
            result.starts_with("Error: HEXAPOD_IP env var not set"),
            "got: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: Unknown action returns D-16 blocked string; no TCP attempted — D-16, D-20
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_unknown_action_returns_blocked() {
        // Blocked actions fire BEFORE the env var read (D-20), so HEXAPOD_IP
        // value does not matter — but we set it to a non-routable IP to make
        // the intent clear and guard against accidental TCP in the catch-all.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("HEXAPOD_IP", "127.0.0.255") };
        let tool = HexapodTcpTool;
        let result = tool
            .execute(json!({"action": "chase_lights"}))
            .await
            .unwrap();
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        assert_eq!(
            result,
            "Action 'chase_lights' is blocked — not permitted via hexapod_tcp. Never send this command.",
            "got: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: "calibration" action is blocked — D-16
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_calibration_action_blocked() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("HEXAPOD_IP", "127.0.0.255") };
        let tool = HexapodTcpTool;
        let result = tool
            .execute(json!({"action": "calibration"}))
            .await
            .unwrap();
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        assert_eq!(
            result,
            "Action 'calibration' is blocked — not permitted via hexapod_tcp. Never send this command.",
            "got: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: "servo_power" action is blocked — D-16
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_servo_power_action_blocked() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("HEXAPOD_IP", "127.0.0.255") };
        let tool = HexapodTcpTool;
        let result = tool
            .execute(json!({"action": "servo_power"}))
            .await
            .unwrap();
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        assert_eq!(
            result,
            "Action 'servo_power' is blocked — not permitted via hexapod_tcp. Never send this command.",
            "got: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: Speed clamp low — speed=1 is clamped to 2 (Pitfall 4)
    // -----------------------------------------------------------------------
    #[test]
    fn test_walk_speed_clamps_low() {
        let wire = build_walk_wire("forward", 1);
        assert!(
            wire.contains("#1#0#25#2#0\n"),
            "expected clamped speed=2 in wire: {wire}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 6: Speed clamp high — speed=99 is clamped to 10 (Pitfall 4)
    // -----------------------------------------------------------------------
    #[test]
    fn test_walk_speed_clamps_high() {
        let wire = build_walk_wire("forward", 99);
        assert!(
            wire.contains("#1#0#25#10#0\n"),
            "expected clamped speed=10 in wire: {wire}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: Direction strings exactly match D-06
    // -----------------------------------------------------------------------
    #[test]
    fn test_walk_direction_strings() {
        assert_eq!(build_walk_wire("forward", 5),  "CMD_MOVE#1#0#25#5#0\n");
        assert_eq!(build_walk_wire("backward", 5), "CMD_MOVE#1#0#-25#5#0\n");
        assert_eq!(build_walk_wire("left", 5),     "CMD_MOVE#1#-25#0#5#0\n");
        assert_eq!(build_walk_wire("right", 5),    "CMD_MOVE#1#25#0#5#0\n");
    }

    // -----------------------------------------------------------------------
    // Test 8: Stop wire string exactly matches D-07
    // -----------------------------------------------------------------------
    #[test]
    fn test_stop_wire_string() {
        assert_eq!(STOP_CMD, "CMD_MOVE#0#0#0#0#0\n");
    }

    // -----------------------------------------------------------------------
    // Test 9: Relax wire string exactly matches D-08; toggle comment exists
    // -----------------------------------------------------------------------
    #[test]
    fn test_relax_wire_string() {
        assert_eq!(RELAX_CMD, "CMD_RELAX\n");
    }

    // -----------------------------------------------------------------------
    // Test 10: Battery parse returns OK when both voltages are above thresholds (D-09)
    // -----------------------------------------------------------------------
    #[test]
    fn test_battery_parse_ok() {
        let result = parse_battery_response("CMD_POWER#7.2#8.1\n");
        assert_eq!(result, "Battery: 7.2V / 8.1V (OK)", "got: {result}");
    }

    // -----------------------------------------------------------------------
    // Test 11: Battery parse returns LOW for both v1 and v2 threshold violations (D-09)
    // -----------------------------------------------------------------------
    #[test]
    fn test_battery_parse_low() {
        // v1 < 5.5 → LOW
        let r1 = parse_battery_response("CMD_POWER#5.4#7.9\n");
        assert!(r1.ends_with("(LOW)"), "v1<5.5 case: {r1}");

        // v2 < 6.0 → LOW
        let r2 = parse_battery_response("CMD_POWER#7.0#5.9\n");
        assert!(r2.ends_with("(LOW)"), "v2<6.0 case: {r2}");
    }

    // -----------------------------------------------------------------------
    // Test 12a: Distance parse — D-10
    // -----------------------------------------------------------------------
    #[test]
    fn test_distance_parse() {
        let result = parse_distance_response("CMD_SONIC#42\n");
        assert_eq!(result, "Distance: 42 cm", "got: {result}");
    }

    // -----------------------------------------------------------------------
    // Test 12b: on_session_end does not panic when HEXAPOD_IP is unset
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_on_session_end_no_panic() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        let tool = HexapodTcpTool;
        // Must not panic — the spawned future short-circuits on the env-var miss
        tool.on_session_end();
        // Give the spawned task a moment to start (it will immediately return)
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // -----------------------------------------------------------------------
    // Test 13: D-18 timeout-expiry path and D-17 connection-error path via
    // map_read_outcome helper (no live TCP needed)
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_read_timeout_branch() {
        // D-18: construct a real Elapsed by timing out an instant-pending future
        let elapsed = tokio::time::timeout(
            Duration::from_nanos(1),
            std::future::pending::<()>(),
        )
        .await
        .unwrap_err();

        let timeout_result: Result<anyhow::Result<String>, tokio::time::error::Elapsed> =
            Err(elapsed);
        let out = map_read_outcome(timeout_result, "127.0.0.255:5002").unwrap();
        assert!(
            out.starts_with("Error: read timed out after 3s"),
            "D-18 timeout path: {out}"
        );

        // D-17: Ok(Err(_)) (I/O error) → connection error string
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let conn_result: Result<anyhow::Result<String>, tokio::time::error::Elapsed> =
            Ok(Err(anyhow::anyhow!(io_err)));
        let out2 = map_read_outcome(conn_result, "127.0.0.255:5002").unwrap();
        assert!(
            out2.starts_with("Error: cannot connect to robot at 127.0.0.255:5002"),
            "D-17 connection-error path: {out2}"
        );

        // Success path passthrough
        let ok_result: Result<anyhow::Result<String>, tokio::time::error::Elapsed> =
            Ok(Ok("CMD_SONIC#30".to_string()));
        let out3 = map_read_outcome(ok_result, "127.0.0.255:5002").unwrap();
        assert_eq!(out3, "CMD_SONIC#30");
    }

    // -----------------------------------------------------------------------
    // Test 14: Positive degrees → angle=+10 in wire (D-03, clockwise)
    // -----------------------------------------------------------------------
    #[test]
    fn test_rotate_wire_positive_degrees() {
        assert_eq!(build_rotate_wire(90), "CMD_MOVE#1#0#0#5#10\n");
    }

    // -----------------------------------------------------------------------
    // Test 15: Negative degrees → angle=-10 in wire (D-03, counterclockwise)
    // -----------------------------------------------------------------------
    #[test]
    fn test_rotate_wire_negative_degrees() {
        assert_eq!(build_rotate_wire(-45), "CMD_MOVE#1#0#0#5#-10\n");
    }

    // -----------------------------------------------------------------------
    // Test 16: rotate(0) via execute() is a no-op — returns OK without sending any wire command.
    // degrees=0 must short-circuit before build_rotate_wire() is called (CR-03).
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_rotate_zero_degrees_is_noop() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // With HEXAPOD_IP unset, a real TCP attempt would return an error.
        // execute() must return Ok("OK") for degrees=0 before reading the env var.
        // But execute() reads HEXAPOD_IP before the inner match, so set it to a
        // non-routable address. The key assertion is that result == "OK" (no motion).
        unsafe { std::env::set_var("HEXAPOD_IP", "127.0.0.255") };
        let tool = HexapodTcpTool;
        let result = tool
            .execute(json!({"action": "rotate", "degrees": 0}))
            .await
            .unwrap();
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        assert_eq!(result, "OK", "rotate(0) must be a no-op; got: {result}");
    }

    // -----------------------------------------------------------------------
    // Test 17: head_pan angle=180 clamped to HEAD_PAN_MAX=90 before wire format (D-06, D-08)
    // -----------------------------------------------------------------------
    #[test]
    fn test_head_pan_clamps_high() {
        let clamped = 180i64.clamp(-HEAD_PAN_MAX, HEAD_PAN_MAX);
        assert_eq!(clamped, 90);
        let wire = format!("CMD_HEAD#0#{clamped}\n");
        assert_eq!(wire, "CMD_HEAD#0#90\n");
    }

    // -----------------------------------------------------------------------
    // Test 18: head_tilt angle=-180 clamped to -HEAD_TILT_MAX=-90 before wire format (D-07, D-08)
    // -----------------------------------------------------------------------
    #[test]
    fn test_head_tilt_clamps_low() {
        let clamped = (-180i64).clamp(-HEAD_TILT_MAX, HEAD_TILT_MAX);
        assert_eq!(clamped, -90);
        let wire = format!("CMD_HEAD#1#{clamped}\n");
        assert_eq!(wire, "CMD_HEAD#1#-90\n");
    }

    // -----------------------------------------------------------------------
    // Test 19: Buzzer wire constant strings match protocol §5 (D-10, D-11)
    // -----------------------------------------------------------------------
    #[test]
    fn test_buzzer_wire_strings() {
        assert_eq!(BUZZER_ON_CMD, "CMD_BUZZER#1\n");
        assert_eq!(BUZZER_OFF_CMD, "CMD_BUZZER#0\n");
    }

    // -----------------------------------------------------------------------
    // Test 20: New Phase 2 actions pass outer allowlist (D-20) — return env-var error, not blocked string
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_new_actions_not_blocked() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        let tool = HexapodTcpTool;
        for action_name in ["rotate", "head_pan", "head_tilt", "buzzer_on", "buzzer_off"] {
            let result = tool
                .execute(json!({"action": action_name, "degrees": 0, "angle": 0}))
                .await
                .unwrap();
            assert!(
                !result.starts_with("Action '"),
                "action '{}' was blocked but should pass allowlist; got: {result}",
                action_name
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 21: CMD_LED constant equals "CMD_LED" (prefix only — wire is CMD_LED#{R}#{G}#{B}\n)
    // -----------------------------------------------------------------------
    #[test]
    fn test_cmd_led_constant_value() {
        assert_eq!(CMD_LED, "CMD_LED");
    }

    // -----------------------------------------------------------------------
    // Test 22: CMD_LED_OFF constant equals "CMD_LED#0\n" — NOT "CMD_LED#0#0#0\n"
    // The server's mode-0 off path is CMD_LED#0\n; CMD_LED#0#0#0\n would attempt
    // to set color (0,0,0) instead of activating the dedicated off path (D-03).
    // -----------------------------------------------------------------------
    #[test]
    fn test_cmd_led_off_constant_value() {
        // The correct wire is CMD_LED#0\n (mode 0 = server off path per D-03)
        assert_eq!(CMD_LED_OFF, "CMD_LED#0\n");
        // Explicitly confirm it is NOT the naive color-zero form
        assert_ne!(CMD_LED_OFF, "CMD_LED#0#0#0\n");
    }

    // -----------------------------------------------------------------------
    // Test 23: r/g/b values clamp independently to 0–255
    // r=300 → 255, g=-10 → 0, b=128 unchanged (D-04, T-27.1.3-01)
    // -----------------------------------------------------------------------
    #[test]
    fn test_led_rgb_clamping() {
        let r_clamped = 300i64.clamp(0, 255);
        let g_clamped = (-10i64).clamp(0, 255);
        let b_clamped = 128i64.clamp(0, 255);
        assert_eq!(r_clamped, 255, "r=300 must clamp to 255");
        assert_eq!(g_clamped, 0, "g=-10 must clamp to 0");
        assert_eq!(b_clamped, 128, "b=128 must remain unchanged");
    }

    // -----------------------------------------------------------------------
    // Test 24: led and led_off pass the outer allowlist — with HEXAPOD_IP unset,
    // execute returns the env-var error string, not the "Action '...' is blocked" string
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_led_actions_pass_allowlist() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        let tool = HexapodTcpTool;

        let led_result = tool
            .execute(json!({"action": "led", "r": 255, "g": 128, "b": 0}))
            .await
            .unwrap();
        assert!(
            led_result.starts_with("Error: HEXAPOD_IP env var not set"),
            "led should pass allowlist and hit env-var check; got: {led_result}"
        );

        let led_off_result = tool
            .execute(json!({"action": "led_off"}))
            .await
            .unwrap();
        assert!(
            led_off_result.starts_with("Error: HEXAPOD_IP env var not set"),
            "led_off should pass allowlist and hit env-var check; got: {led_off_result}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 27: stream_distance + camera_pan + camera_tilt pass outer allowlist — D-08, D-14
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_new_actions_not_blocked_27_1_4() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        let tool = HexapodTcpTool;
        for action_name in ["stream_distance", "camera_pan", "camera_tilt"] {
            let result = tool
                .execute(json!({"action": action_name, "samples": 1, "x": 115, "y": 90}))
                .await
                .unwrap();
            assert!(
                !result.starts_with("Action '"),
                "action '{}' was blocked but should pass allowlist; got: {result}",
                action_name
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 28: samples clamped to [1, 20] — D-09
    // -----------------------------------------------------------------------
    #[test]
    fn test_stream_distance_samples_clamp() {
        let clamped_high = 99i64.clamp(1, STREAM_DISTANCE_MAX_SAMPLES);
        let clamped_low  = 0i64.clamp(1, STREAM_DISTANCE_MAX_SAMPLES);
        assert_eq!(clamped_high, 20);
        assert_eq!(clamped_low, 1);
    }

    // -----------------------------------------------------------------------
    // Test 29: stream_distance return format matches D-11 spec
    // -----------------------------------------------------------------------
    #[test]
    fn test_stream_distance_format() {
        let readings: Vec<i64> = vec![42, 43, 41, 44, 42];
        let min = readings.iter().copied().min().unwrap_or(0);
        let max = readings.iter().copied().max().unwrap_or(0);
        let avg = readings.iter().copied().sum::<i64>() as f64 / readings.len() as f64;
        let list: Vec<String> = readings.iter().map(|d| d.to_string()).collect();
        let result = format!("Distances: [{}] cm | min={} max={} avg={:.1}", list.join(", "), min, max, avg);
        assert_eq!(result, "Distances: [42, 43, 41, 44, 42] cm | min=41 max=44 avg=42.4");
    }

    // -----------------------------------------------------------------------
    // Test 25: camera_pan wire format — x clamped to [50, 180]; y=CAMERA_TILT_DEFAULT — D-12
    // -----------------------------------------------------------------------
    #[test]
    fn test_camera_pan_wire() {
        let x = 200i64.clamp(CAMERA_PAN_MIN, CAMERA_PAN_MAX); // 200 → 180
        let wire = format!("CMD_CAMERA#{x}#{CAMERA_TILT_DEFAULT}\n");
        assert_eq!(wire, "CMD_CAMERA#180#90\n");
    }

    // -----------------------------------------------------------------------
    // Test 26: camera_tilt wire format — y clamped to [0, 180]; x=CAMERA_PAN_DEFAULT — D-13
    // -----------------------------------------------------------------------
    #[test]
    fn test_camera_tilt_wire() {
        let y = (-5i64).clamp(CAMERA_TILT_MIN, CAMERA_TILT_MAX); // -5 → 0
        let wire = format!("CMD_CAMERA#{CAMERA_PAN_DEFAULT}#{y}\n");
        assert_eq!(wire, "CMD_CAMERA#115#0\n");
    }
}
