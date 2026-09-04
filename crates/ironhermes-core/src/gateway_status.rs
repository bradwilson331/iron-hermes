//! Phase 49.3 Plan 06 (D-08): the versioned cross-process status schema
//! shared by the gateway process's periodic heartbeat writer
//! (`ironhermes-gateway::pid::write_gateway_status`, `runner.rs`'s "9c"
//! heartbeat task) and the web server's status reader
//! (`iron_hermes_ui::server::gateway_platform_status_api::read_platform_status`).
//!
//! # Versioned cross-process contract
//!
//! The gateway OS process and the web server process are separate binaries
//! that may be upgraded independently. [`GATEWAY_STATUS_SCHEMA_VERSION`] is
//! bumped whenever the shape of [`GatewayPlatformStatus`] changes in a way
//! that is not backward-compatible; the reader treats any version mismatch
//! as "no live heartbeat" (falls back to pidfile liveness) rather than
//! misinterpreting a differently-shaped payload (T-49.3-06-01).
//!
//! # No secrets (T-49.3-06-03)
//!
//! [`PlatformStatusEntry`] carries only `connected: bool` and
//! `session_count: usize` — never a token, key, or any config value. The
//! round-trip test below pins this shape.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Bump whenever [`GatewayPlatformStatus`]'s shape changes in a way that is
/// not backward-compatible. The reader treats a mismatched version as a
/// stale/absent heartbeat (never a hard error).
pub const GATEWAY_STATUS_SCHEMA_VERSION: u32 = 1;

/// Per-adapter status: whether the adapter is currently connected, and how
/// many active sessions it currently hosts (from `SessionStore`, grouped by
/// `SessionKey::platform`). No secret-bearing field — see module doc
/// (T-49.3-06-03).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlatformStatusEntry {
    pub connected: bool,
    pub session_count: usize,
}

/// The versioned status-file payload written atomically by the gateway
/// process's periodic heartbeat task and read by the web server's status
/// reader. `platforms` is keyed by the platform's canonical lowercase name
/// (`Platform`'s `Display` string — e.g. `"telegram"`, `"discord"`,
/// `"slack"`, `"buzz"`, `"webhook"`, `"api_server"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayPlatformStatus {
    pub schema_version: u32,
    /// RFC3339 UTC timestamp of the tick that produced this snapshot. Used
    /// by the reader's staleness check.
    pub written_at: String,
    pub platforms: BTreeMap<String, PlatformStatusEntry>,
}

impl GatewayPlatformStatus {
    /// Build a snapshot at the current schema version, stamped `now`.
    pub fn new(platforms: BTreeMap<String, PlatformStatusEntry>) -> Self {
        Self {
            schema_version: GATEWAY_STATUS_SCHEMA_VERSION,
            written_at: chrono::Utc::now().to_rfc3339(),
            platforms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_schema_version_and_platforms() {
        let mut platforms = BTreeMap::new();
        platforms.insert(
            "telegram".to_string(),
            PlatformStatusEntry {
                connected: true,
                session_count: 3,
            },
        );
        platforms.insert(
            "discord".to_string(),
            PlatformStatusEntry {
                connected: false,
                session_count: 0,
            },
        );
        let status = GatewayPlatformStatus::new(platforms);

        let json = serde_json::to_string(&status).expect("serialize");
        let parsed: GatewayPlatformStatus = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.schema_version, GATEWAY_STATUS_SCHEMA_VERSION);
        assert_eq!(parsed, status);
        assert_eq!(
            parsed.platforms.get("telegram"),
            Some(&PlatformStatusEntry {
                connected: true,
                session_count: 3,
            })
        );
    }

    /// T-49.3-06-03: the schema carries only `connected`/`session_count` —
    /// assert the serialized JSON contains no field name resembling a
    /// secret (token/key/secret/password), pinning the shape so a future
    /// edit can't silently widen it.
    #[test]
    fn serialized_shape_carries_no_secret_bearing_field() {
        let mut platforms = BTreeMap::new();
        platforms.insert(
            "buzz".to_string(),
            PlatformStatusEntry {
                connected: true,
                session_count: 1,
            },
        );
        let status = GatewayPlatformStatus::new(platforms);
        let json = serde_json::to_string(&status).expect("serialize");
        for forbidden in ["token", "secret", "key", "password", "nsec"] {
            assert!(
                !json.to_lowercase().contains(forbidden),
                "serialized GatewayPlatformStatus unexpectedly contains '{forbidden}': {json}"
            );
        }
    }

    #[test]
    fn schema_version_constant_is_one() {
        // Pins the starting version explicitly so a future bump is a
        // deliberate, reviewed diff on this line, not an accidental drift.
        assert_eq!(GATEWAY_STATUS_SCHEMA_VERSION, 1);
    }
}
