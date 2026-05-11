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

// ---------------------------------------------------------------------------
// Tool description (for LLM — includes blocked-command guidance, D-16)
// ---------------------------------------------------------------------------

const DESCRIPTION: &str = "Control the Freenove hexapod robot over TCP. \
    Phase 1 actions: walk (with direction and speed), stop, read_battery, \
    read_distance, relax_servos. \
    BLOCKED (do not attempt): calibration, servo_power, led_mode 2-5 — \
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

/// Parse a `CMD_POWER#<v1>#<v2>\n` response into a human-readable battery string (D-09).
///
/// Low thresholds per adc.py: v1 (load/channel 0) < 5.5 V OR v2 (Pi/channel 4) < 6.0 V.
pub(crate) fn parse_battery_response(raw: &str) -> String {
    let parts: Vec<&str> = raw.trim().split('#').collect();
    let v1: f32 = parts.get(1).unwrap_or(&"0").parse().unwrap_or(0.0);
    let v2: f32 = parts.get(2).unwrap_or(&"0").parse().unwrap_or(0.0);
    let status = if v1 < BATTERY_LOW_V1 || v2 < BATTERY_LOW_V2 { "LOW" } else { "OK" };
    format!("Battery: {v1}V / {v2}V ({status})")
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
                        "enum": ["walk", "stop", "read_battery", "read_distance", "relax_servos"],
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
            "walk" | "stop" | "read_battery" | "read_distance" | "relax_servos" => {
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
                        let direction = args["direction"].as_str().unwrap_or("forward");
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

                    // Unreachable: outer arm already enumerates exactly these 5 values
                    _ => unreachable!("outer match guarantees only the 5 allowed actions reach here"),
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

    /// Serialise all env-var-mutating tests in this module so that parallel
    /// test threads cannot race on HEXAPOD_IP. Tests that DO NOT call
    /// set_var/remove_var are exempt and can run concurrently as normal.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // -----------------------------------------------------------------------
    // Test 1: Missing env var returns Ok(error) — D-12
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_missing_env_var_returns_ok_error() {
        let _guard = ENV_LOCK.lock().unwrap();
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
        let _guard = ENV_LOCK.lock().unwrap();
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
        let _guard = ENV_LOCK.lock().unwrap();
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
        let _guard = ENV_LOCK.lock().unwrap();
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
        let _guard = ENV_LOCK.lock().unwrap();
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
}
