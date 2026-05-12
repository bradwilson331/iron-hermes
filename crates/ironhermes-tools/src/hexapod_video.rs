//! Phase 27.1.4 D-01/D-02/D-03: hexapod_video tool.
//!
//! Captures a single JPEG frame from the Freenove hexapod robot's video port (8002)
//! and returns it as a `data:image/jpeg;base64,<...>` data URI for multimodal vision
//! analysis by Gemma 4.
//!
//! # Connection protocol
//!
//! The Freenove server (`server.py transmit_video()`) sends frames as:
//!   4 bytes (little-endian u32 length) + `length` bytes of JPEG data
//!
//! This matches the Python client's `receiving_video()`:
//!   `struct.unpack('<L', stream_bytes[:4])` → `connection.read(leng[0])`
//!
//! Port 8002 is single-client (`video_socket.listen(1)`) — another connected client
//! (e.g., the desktop PyQt5 UI) causes `ConnectionRefused`. Disconnect the desktop
//! client before calling `capture_frame`.

use std::env;

use async_trait::async_trait;
use base64::Engine as _; // CRITICAL: Engine trait must be in scope for .encode() — Pitfall 5
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::time::{Duration, timeout};
use tracing::debug;

use crate::registry::{Prerequisite, Tool};

// ---------------------------------------------------------------------------
// Wire-protocol constants
// ---------------------------------------------------------------------------

/// Video port — matches Freenove server `video_socket.bind(("", 8002))`. Per D-04.
const VIDEO_PORT: u16 = 8002;

/// Read timeout for the full frame read (both `read_exact` calls). Per D-05/RESEARCH OQ-1.
/// 5s is generous for a single JPEG at ~15fps; prevents indefinite hang when the camera
/// is not streaming (camera.py's `get_frame()` blocks on `Condition.wait()`).
const VIDEO_READ_TIMEOUT_SECS: u64 = 5;

/// Maximum frame size in bytes. Rejects oversized length prefixes before allocating
/// the `vec![0u8; frame_len]` buffer — defends against T-27.1.4-05 (malicious/buggy
/// length prefix that would trigger a multi-GB allocation). 2 MB is well above
/// Freenove's typical ~50–200 KB JPEG frame.
const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Tool description
// ---------------------------------------------------------------------------

const DESCRIPTION: &str = "\
Capture a single JPEG frame from the Freenove hexapod robot's camera (video port 8002). \
Returns a data:image/jpeg;base64,<...> data URI suitable for multimodal vision analysis. \
One action: capture_frame (no parameters). \
The robot's video port is single-client — disconnect the desktop UI client first. \
Point the camera with hexapod_tcp camera_pan / camera_tilt before capturing.";

// ---------------------------------------------------------------------------
// Struct
// ---------------------------------------------------------------------------

/// Hexapod video tool — stateless single-frame JPEG capture. Per D-01/D-05.
pub struct HexapodVideoTool;

// ---------------------------------------------------------------------------
// Tool trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for HexapodVideoTool {
    fn name(&self) -> &str {
        "hexapod_video"
    }

    fn toolset(&self) -> &str {
        "robotics"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "hexapod_video",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["capture_frame"],
                        "description": "Action to perform. capture_frame: connect to video port 8002, read one JPEG frame, return as a data:image/jpeg;base64 URI."
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
                          Required for hexapod_video to connect to the robot's video stream."
                .to_string(),
            required: true,
        }]
    }

    // NOTE: `is_available()` is NOT overridden. The default impl walks `prerequisites()`
    // and returns false when HEXAPOD_IP is unset — hiding the tool automatically.
    // Same pattern as HexapodTcpTool.

    /// Execute a hexapod_video action.
    ///
    /// ## Allowlist
    /// Only `capture_frame` is permitted. All other strings hit the catch-all arm and
    /// return the blocked-command string — no TCP connection is attempted and no env var
    /// is read (D-16 / T-27.1.4-09 mitigation).
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let action = args["action"].as_str().unwrap_or("");

        match action {
            "capture_frame" => {
                // Read HEXAPOD_IP only after the allowlist passes (D-04/D-12 pattern).
                let ip = match env::var("HEXAPOD_IP") {
                    Ok(v) => v,
                    Err(_) => {
                        return Ok(
                            "Error: HEXAPOD_IP env var not set — cannot connect to robot"
                                .to_string(),
                        )
                    }
                };
                let addr = format!("{ip}:{VIDEO_PORT}");
                debug!("hexapod_video: action={action} addr={addr}");

                // Connect — split ConnectionRefused from generic errors (D-06 / T-27.1.4-09).
                // Port 8002 is single-client (video_socket.listen(1)); another connected
                // client causes ConnectionRefused, not a timeout.
                let mut stream = match TcpStream::connect(&addr).await {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                        return Ok(
                            "Error: video port 8002 is busy — another client is connected. \
                             Disconnect the other client and retry."
                                .to_string(),
                        );
                    }
                    Err(_) => {
                        return Ok(format!(
                            "Error: cannot connect to robot at {addr} — \
                             is HEXAPOD_IP set and the robot powered on?"
                        ));
                    }
                };

                // Read the length-prefixed JPEG frame wrapped in a 5s timeout (D-05/OQ-1).
                // Both read_exact calls are inside the async block so the timeout covers
                // the full frame read — T-27.1.4-06 mitigation.
                let read_fut = async {
                    // 4-byte little-endian frame length (Freenove Client.py: struct.unpack('<L', ...))
                    let mut len_buf = [0u8; 4];
                    stream.read_exact(&mut len_buf).await?;
                    let frame_len = u32::from_le_bytes(len_buf) as usize;

                    // Bounds check before allocating — T-27.1.4-05 mitigation.
                    if frame_len == 0 || frame_len > MAX_FRAME_BYTES {
                        return Err(anyhow::anyhow!(
                            "frame length {frame_len} out of range (0..{})",
                            MAX_FRAME_BYTES
                        ));
                    }

                    let mut jpg_buf = vec![0u8; frame_len];
                    stream.read_exact(&mut jpg_buf).await?;

                    anyhow::Ok(jpg_buf)
                };

                let jpg_buf =
                    match timeout(Duration::from_secs(VIDEO_READ_TIMEOUT_SECS), read_fut).await {
                        Ok(Ok(b)) => b,
                        Ok(Err(_)) => {
                            return Ok(format!(
                                "Error: cannot read video frame from robot at {addr} \
                                 — camera may not be running"
                            ));
                        }
                        Err(_) => {
                            return Ok(
                                "Error: read timed out after 5s waiting for video frame \
                                 — camera may not be streaming"
                                    .to_string(),
                            );
                        }
                    };

                // base64 encode and return as data URI (D-03).
                // Pattern verified from browser_vision.rs lines 201–202.
                let b64 = base64::engine::general_purpose::STANDARD.encode(&jpg_buf);
                Ok(format!("data:image/jpeg;base64,{}", b64))
            }

            other => Ok(format!(
                "Action '{other}' is blocked — not permitted via hexapod_video. \
                 Never send this command."
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Inline unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    /// Serialize env-var-mutating tests within this module.
    /// NOTE: This mutex only protects against races within this module.
    /// Run the full test binary with RUST_TEST_THREADS=1 to avoid races with
    /// other modules that also read HEXAPOD_IP.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // -----------------------------------------------------------------------
    // Test: Missing HEXAPOD_IP returns Ok(error) without TCP — D-04, D-12
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_missing_env_var() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        let tool = HexapodVideoTool;
        let result = tool
            .execute(json!({"action": "capture_frame"}))
            .await
            .unwrap();
        assert!(
            result.starts_with("Error: HEXAPOD_IP env var not set"),
            "got: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // Test: Unknown action returns blocked string — D-16 / T-27.1.4-09
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_unknown_action_blocked() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // Env var does not matter — blocked arm fires before env read (D-16).
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        let tool = HexapodVideoTool;
        let result = tool
            .execute(json!({"action": "foo"}))
            .await
            .unwrap();
        assert!(
            result.starts_with("Action 'foo' is blocked"),
            "got: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // Test: capture_frame passes allowlist (reaches env-var guard, not blocked)
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_capture_frame_passes_allowlist() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("HEXAPOD_IP") };
        let tool = HexapodVideoTool;
        let result = tool
            .execute(json!({"action": "capture_frame"}))
            .await
            .unwrap();
        // Must NOT start with "Action '" — it passed the allowlist and hit the env-var guard.
        assert!(
            !result.starts_with("Action '"),
            "capture_frame was incorrectly blocked; got: {result}"
        );
        // Must start with the env-var error (HEXAPOD_IP was removed).
        assert!(
            result.starts_with("Error: HEXAPOD_IP env var not set"),
            "expected env-var error; got: {result}"
        );
    }
}
