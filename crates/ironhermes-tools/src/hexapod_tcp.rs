// RED phase: tests written first, implementation stubs to follow.
// This file intentionally does NOT compile — the helpers and types
// referenced by the tests do not exist yet. Running:
//   cargo test -p ironhermes-tools hexapod
// will produce compilation errors (RED gate).

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::time::Duration;

use crate::registry::{Prerequisite, Tool};

// Stubs — these will be replaced by the real implementation in the GREEN commit.
// The test module below references these, causing compile failure in RED state.
pub struct HexapodTcpTool;

// Intentionally incomplete — missing: build_walk_wire, parse_battery_response,
// parse_distance_response, map_read_outcome, STOP_CMD, RELAX_CMD, impl Tool for HexapodTcpTool

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn test_missing_env_var_returns_ok_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        let tool = HexapodTcpTool;
        let result = tool
            .execute(json!({"action": "walk", "direction": "forward", "speed": 5}))
            .await
            .unwrap();
        assert!(result.starts_with("Error: HEXAPOD_IP env var not set"), "got: {result}");
    }

    #[tokio::test]
    async fn test_unknown_action_returns_blocked() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("HEXAPOD_IP", "127.0.0.255") };
        let tool = HexapodTcpTool;
        let result = tool.execute(json!({"action": "chase_lights"})).await.unwrap();
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        assert_eq!(result, "Action 'chase_lights' is blocked — not permitted via hexapod_tcp. Never send this command.", "got: {result}");
    }

    #[tokio::test]
    async fn test_calibration_action_blocked() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("HEXAPOD_IP", "127.0.0.255") };
        let tool = HexapodTcpTool;
        let result = tool.execute(json!({"action": "calibration"})).await.unwrap();
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        assert_eq!(result, "Action 'calibration' is blocked — not permitted via hexapod_tcp. Never send this command.", "got: {result}");
    }

    #[tokio::test]
    async fn test_servo_power_action_blocked() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("HEXAPOD_IP", "127.0.0.255") };
        let tool = HexapodTcpTool;
        let result = tool.execute(json!({"action": "servo_power"})).await.unwrap();
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        assert_eq!(result, "Action 'servo_power' is blocked — not permitted via hexapod_tcp. Never send this command.", "got: {result}");
    }

    #[test]
    fn test_walk_speed_clamps_low() {
        let wire = build_walk_wire("forward", 1); // build_walk_wire not defined yet — RED
        assert!(wire.contains("#1#0#25#2#0\n"), "expected clamped speed=2: {wire}");
    }

    #[test]
    fn test_walk_speed_clamps_high() {
        let wire = build_walk_wire("forward", 99); // RED
        assert!(wire.contains("#1#0#25#10#0\n"), "expected clamped speed=10: {wire}");
    }

    #[test]
    fn test_walk_direction_strings() {
        assert_eq!(build_walk_wire("forward", 5),  "CMD_MOVE#1#0#25#5#0\n");
        assert_eq!(build_walk_wire("backward", 5), "CMD_MOVE#1#0#-25#5#0\n");
        assert_eq!(build_walk_wire("left", 5),     "CMD_MOVE#1#-25#0#5#0\n");
        assert_eq!(build_walk_wire("right", 5),    "CMD_MOVE#1#25#0#5#0\n");
    }

    #[test]
    fn test_stop_wire_string() {
        assert_eq!(STOP_CMD, "CMD_MOVE#0#0#0#0#0\n"); // STOP_CMD not defined yet — RED
    }

    #[test]
    fn test_relax_wire_string() {
        assert_eq!(RELAX_CMD, "CMD_RELAX\n"); // RELAX_CMD not defined yet — RED
    }

    #[test]
    fn test_battery_parse_ok() {
        let result = parse_battery_response("CMD_POWER#7.2#8.1\n"); // not defined yet — RED
        assert_eq!(result, "Battery: 7.2V / 8.1V (OK)", "got: {result}");
    }

    #[test]
    fn test_battery_parse_low() {
        let r1 = parse_battery_response("CMD_POWER#5.4#7.9\n");
        assert!(r1.ends_with("(LOW)"), "v1<5.5 case: {r1}");
        let r2 = parse_battery_response("CMD_POWER#7.0#5.9\n");
        assert!(r2.ends_with("(LOW)"), "v2<6.0 case: {r2}");
    }

    #[test]
    fn test_distance_parse() {
        let result = parse_distance_response("CMD_SONIC#42\n"); // not defined yet — RED
        assert_eq!(result, "Distance: 42 cm", "got: {result}");
    }

    #[tokio::test]
    async fn test_on_session_end_no_panic() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        let tool = HexapodTcpTool;
        tool.on_session_end(); // on_session_end not implemented yet — RED
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn test_read_timeout_branch() {
        let elapsed = tokio::time::timeout(
            Duration::from_nanos(1),
            std::future::pending::<()>(),
        ).await.unwrap_err();
        let timeout_result: Result<anyhow::Result<String>, tokio::time::error::Elapsed> = Err(elapsed);
        let out = map_read_outcome(timeout_result, "127.0.0.255:5002").unwrap(); // not defined yet — RED
        assert!(out.starts_with("Error: read timed out after 3s"), "D-18: {out}");

        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let conn_result: Result<anyhow::Result<String>, tokio::time::error::Elapsed> = Ok(Err(anyhow::anyhow!(io_err)));
        let out2 = map_read_outcome(conn_result, "127.0.0.255:5002").unwrap();
        assert!(out2.starts_with("Error: cannot connect to robot at 127.0.0.255:5002"), "D-17: {out2}");

        let ok_result: Result<anyhow::Result<String>, tokio::time::error::Elapsed> = Ok(Ok("CMD_SONIC#30".to_string()));
        let out3 = map_read_outcome(ok_result, "127.0.0.255:5002").unwrap();
        assert_eq!(out3, "CMD_SONIC#30");
    }
}
