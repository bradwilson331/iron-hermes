//! Phase 49.3 Plan 06: the D-08 per-platform status reader — a
//! gateway-written heartbeat status file (connected state, active session
//! count) next to `gateway.pid`, falling back to `gateway_status_api.rs`'s
//! existing 6-state pidfile liveness probe when the status file is
//! absent/stale.
//!
//! # Heartbeat-first, pidfile-fallback (D-08, T-49.3-06-01)
//!
//! [`read_platform_status`] tries the heartbeat file
//! (`ironhermes_gateway::pid::read_gateway_status`, written every 15s by
//! `runner.rs`'s "9c" task) first. Absent file, I/O error, unparseable
//! JSON, a `schema_version` mismatch, a `written_at` older than
//! [`STALENESS_WINDOW_SECS`], or a `written_at` further in the future than
//! [`FUTURE_SKEW_GRACE_SECS`] are ALL treated uniformly as "no live
//! heartbeat" — never a hard error surfaced to the UI — and the fn falls
//! back to the EXISTING [`crate::server::gateway_status_api::get_gateway_runtime_status`]
//! pidfile probe (the same one the Tools page RUNTIME section and Plan
//! 01-05's per-card status assembly already use), mapped into the same
//! per-platform shape with `session_count: None` (no count signal from a
//! bare process-liveness probe).
//!
//! # Reused validate-before-path-build discipline (T-49.3-06-04)
//!
//! Profile-scope resolution mirrors `gateway_status_api::read_gateway_runtime_status`:
//! `ironhermes_core::profile::validate_profile_name` runs BEFORE any path is
//! built from the client-supplied profile name. On an invalid name, the
//! heartbeat lookup is skipped and the pidfile fallback path is taken
//! (`get_gateway_runtime_status` performs the identical validation itself
//! and reports `Unknown` — never a raw error, never a path built from an
//! unvalidated name).
//!
//! # No secrets (T-49.3-06-03)
//!
//! [`PlatformStatusView`] carries only `connected`/`session_count` — the
//! same no-secret shape as `ironhermes_core::gateway_status::PlatformStatusEntry`
//! it is derived from.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::server::gateway_status_api::{get_gateway_runtime_status, GatewayRuntimeStatus};
use crate::server::tools_config_api::ConfigScope;

/// A small staleness window — the heartbeat writes every 15s
/// (`ironhermes-gateway/src/runner.rs`'s "9c" task); anything older than
/// this is treated as "no live heartbeat" rather than trusting a possibly
/// crashed/hung gateway's last-known snapshot (T-49.3-06-01).
#[cfg(not(target_arch = "wasm32"))]
const STALENESS_WINDOW_SECS: i64 = 60;

/// The gateway and web-UI are separate processes whose system clocks can
/// disagree by a small amount even when both are healthy (ordinary NTP
/// jitter, container clock drift). This lower bound is therefore a
/// tolerance, not a hard zero: a `written_at` up to this many seconds ahead
/// of the reading process's clock is still trusted. A snapshot further
/// ahead than this is evidence of real clock skew or a stuck writer, which
/// the pidfile fallback handles more honestly than trusting a
/// future-dated — and therefore unverifiable — snapshot would (WR-03,
/// T-49.3-08-01).
#[cfg(not(target_arch = "wasm32"))]
const FUTURE_SKEW_GRACE_SECS: i64 = 5;

/// Per-platform status the UI renders from — DTO-shape-independent of
/// whether the data came from the live heartbeat (which carries a real
/// count) or the pidfile fallback (which does not). `session_count: None`
/// means "no count signal available" — the caller (chat_platform_cards.rs)
/// omits the counts segment entirely rather than rendering a fake zero
/// (E7 partial).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlatformStatusView {
    pub connected: bool,
    pub session_count: Option<usize>,
}

/// Keyed by the platform's canonical lowercase name — see
/// `ironhermes_core::gateway_status::GatewayPlatformStatus::platforms`'s own
/// doc for the exact key vocabulary (`"telegram"`, `"discord"`, `"slack"`,
/// `"buzz"`, `"webhook"`, `"api_server"`).
pub type PlatformStatusMap = BTreeMap<String, PlatformStatusView>;

/// Every platform key the heartbeat/fallback shape is expected to cover —
/// shared between the heartbeat and fallback builders so both always
/// produce the same key set (never a partial map missing a platform the UI
/// expects a card for).
#[cfg(not(target_arch = "wasm32"))]
const PLATFORM_KEYS: [&str; 6] = ["telegram", "discord", "slack", "buzz", "webhook", "api_server"];

/// Maps a freshly-read [`ironhermes_core::gateway_status::GatewayPlatformStatus`]
/// into [`PlatformStatusMap`], or `None` when the snapshot must be treated
/// as stale (schema mismatch, `written_at` older than the staleness window,
/// or `written_at` further in the future than [`FUTURE_SKEW_GRACE_SECS`] —
/// T-49.3-06-01, WR-03/T-49.3-08-01). A snapshot with parseable-but-invalid
/// `written_at` is also treated as stale (fail safe, never trust an
/// unparseable timestamp).
#[cfg(not(target_arch = "wasm32"))]
fn platform_status_from_heartbeat(
    status: ironhermes_core::gateway_status::GatewayPlatformStatus,
) -> Option<PlatformStatusMap> {
    if status.schema_version != ironhermes_core::gateway_status::GATEWAY_STATUS_SCHEMA_VERSION {
        return None;
    }
    let written_at = chrono::DateTime::parse_from_rfc3339(&status.written_at).ok()?;
    let age_secs = chrono::Utc::now()
        .signed_duration_since(written_at.with_timezone(&chrono::Utc))
        .num_seconds();
    // Two-sided range: too old (age_secs > STALENESS_WINDOW_SECS) is a
    // stale/possibly-crashed writer; too far in the future
    // (age_secs < -FUTURE_SKEW_GRACE_SECS) is clock skew or a stuck writer.
    // Both fall back to the pidfile probe rather than being trusted.
    if !(-FUTURE_SKEW_GRACE_SECS..=STALENESS_WINDOW_SECS).contains(&age_secs) {
        return None;
    }

    Some(
        status
            .platforms
            .into_iter()
            .map(|(name, entry)| {
                (
                    name,
                    PlatformStatusView {
                        connected: entry.connected,
                        session_count: Some(entry.session_count),
                    },
                )
            })
            .collect(),
    )
}

/// Maps the existing pidfile-derived [`GatewayRuntimeStatus`] into the same
/// per-platform shape: every platform reports the SAME `connected` value
/// (a bare process-liveness probe cannot distinguish adapters), and
/// `session_count: None` (E7 partial — the caller omits the counts segment
/// entirely rather than rendering a fake zero).
#[cfg(not(target_arch = "wasm32"))]
fn platform_status_from_pidfile_fallback(runtime_status: &GatewayRuntimeStatus) -> PlatformStatusMap {
    let connected = runtime_status.is_confirmed_running();
    PLATFORM_KEYS
        .iter()
        .map(|name| {
            (
                name.to_string(),
                PlatformStatusView {
                    connected,
                    session_count: None,
                },
            )
        })
        .collect()
}

/// Heartbeat-first, pidfile-fallback read for `scope`. Never returns an
/// `Err` from the fallback path itself — `get_gateway_runtime_status`'s own
/// contract already guarantees a `GatewayRuntimeStatus` (never blank), and
/// this fn maps whatever it returns.
#[cfg(not(target_arch = "wasm32"))]
async fn read_platform_status_inner(scope: ConfigScope) -> PlatformStatusMap {
    let heartbeat_home = match &scope {
        ConfigScope::Root => Some(ironhermes_core::get_hermes_home()),
        ConfigScope::Profile(name) => ironhermes_core::profile::validate_profile_name(name)
            .ok()
            .map(|validated| crate::server::profile_api::profile_dir_for(&validated)),
    };

    let heartbeat_map = heartbeat_home
        .and_then(|home| ironhermes_gateway::pid::read_gateway_status(&home).ok().flatten())
        .and_then(platform_status_from_heartbeat);

    if let Some(map) = heartbeat_map {
        return map;
    }

    // Absent / stale / mismatched-version / parse-error / invalid-profile-
    // name heartbeat: fall back to the EXISTING gated pidfile probe
    // (T-49.3-06-04 — same validate-before-path-build discipline, never
    // re-derived here).
    let runtime_status = get_gateway_runtime_status(scope)
        .await
        .unwrap_or(GatewayRuntimeStatus::Unknown {
            reason: "gateway status could not be read".to_string(),
        });
    platform_status_from_pidfile_fallback(&runtime_status)
}

/// Read `scope`'s per-platform gateway status — heartbeat-first, pidfile
/// fallback (D-08). See module doc.
#[server]
pub async fn read_platform_status(scope: ConfigScope) -> Result<PlatformStatusMap, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(read_platform_status_inner(scope).await)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use ironhermes_core::gateway_status::{GatewayPlatformStatus, PlatformStatusEntry};
    use std::collections::BTreeMap as StdBTreeMap;

    fn sample_status(connected: bool, session_count: usize) -> GatewayPlatformStatus {
        let mut platforms = StdBTreeMap::new();
        platforms.insert(
            "telegram".to_string(),
            PlatformStatusEntry {
                connected,
                session_count,
            },
        );
        GatewayPlatformStatus::new(platforms)
    }

    #[test]
    fn fresh_matching_status_file_yields_per_platform_statuses() {
        let status = sample_status(true, 4);
        let map = platform_status_from_heartbeat(status).expect("fresh status must not be stale");
        let entry = map.get("telegram").expect("telegram key present");
        assert!(entry.connected);
        assert_eq!(entry.session_count, Some(4));
    }

    #[test]
    fn schema_version_mismatch_is_treated_as_stale() {
        let mut status = sample_status(true, 1);
        status.schema_version += 1;
        assert!(platform_status_from_heartbeat(status).is_none());
    }

    #[test]
    fn written_at_older_than_staleness_window_is_treated_as_stale() {
        let mut status = sample_status(true, 1);
        status.written_at = (chrono::Utc::now() - chrono::Duration::seconds(STALENESS_WINDOW_SECS + 30))
            .to_rfc3339();
        assert!(platform_status_from_heartbeat(status).is_none());
    }

    #[test]
    fn written_at_far_in_the_future_is_treated_as_stale() {
        let mut status = sample_status(true, 1);
        status.written_at = (chrono::Utc::now() + chrono::Duration::seconds(FUTURE_SKEW_GRACE_SECS + 3600))
            .to_rfc3339();
        assert!(platform_status_from_heartbeat(status).is_none());
    }

    #[test]
    fn written_at_slightly_ahead_within_the_skew_grace_is_still_fresh() {
        let mut status = sample_status(true, 1);
        status.written_at = (chrono::Utc::now() + chrono::Duration::seconds(1)).to_rfc3339();
        let map = platform_status_from_heartbeat(status)
            .expect("sub-second future skew must still be treated as fresh");
        let entry = map.get("telegram").expect("telegram key present");
        assert!(entry.connected);
        assert_eq!(entry.session_count, Some(1));
    }

    #[test]
    fn unparseable_written_at_is_treated_as_stale() {
        let mut status = sample_status(true, 1);
        status.written_at = "not-a-timestamp".to_string();
        assert!(platform_status_from_heartbeat(status).is_none());
    }

    /// An absent status file (no heartbeat has ever been written) must fall
    /// back to the pidfile-derived shape — never blank, never an error.
    #[test]
    fn absent_status_file_falls_back_to_pidfile_derived_shape() {
        let fallback = platform_status_from_pidfile_fallback(&GatewayRuntimeStatus::NotRunning);
        assert_eq!(fallback.len(), PLATFORM_KEYS.len());
        for key in PLATFORM_KEYS {
            let entry = fallback.get(key).unwrap_or_else(|| panic!("missing {key}"));
            assert!(!entry.connected);
            assert_eq!(entry.session_count, None);
        }
    }

    #[test]
    fn confirmed_running_pidfile_fallback_reports_connected_for_every_platform() {
        let fallback = platform_status_from_pidfile_fallback(&GatewayRuntimeStatus::Running {
            pid: 4242,
            started_at: "2026-08-27T00:00:00Z".to_string(),
            profile: "default".to_string(),
        });
        for key in PLATFORM_KEYS {
            assert!(fallback.get(key).unwrap().connected);
        }
    }

    /// End-to-end via a real tempdir: a live matching heartbeat file wins
    /// over what would otherwise be a pidfile-fallback read.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // env_lock must span the awaited read — same idiom as tools_config_api.rs tests
    async fn end_to_end_fresh_heartbeat_file_is_preferred_over_pidfile_fallback() {
        let _g = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };

        let status = sample_status(true, 9);
        ironhermes_gateway::pid::write_gateway_status(dir.path(), &status)
            .expect("write real gateway-status.json");

        let map = read_platform_status_inner(ConfigScope::Root).await;

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let entry = map.get("telegram").expect("telegram key present");
        assert!(entry.connected);
        assert_eq!(entry.session_count, Some(9));
    }

    /// End-to-end: no heartbeat file at all falls back through the real
    /// `get_gateway_runtime_status` pidfile probe, never blank.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // env_lock must span the awaited read — same idiom as tools_config_api.rs tests
    async fn end_to_end_no_heartbeat_file_falls_back_to_pidfile_probe() {
        let _g = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };

        let map = read_platform_status_inner(ConfigScope::Root).await;

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert_eq!(map.len(), PLATFORM_KEYS.len());
        for key in PLATFORM_KEYS {
            let entry = map.get(key).unwrap_or_else(|| panic!("missing {key}"));
            assert!(!entry.connected);
            assert_eq!(entry.session_count, None);
        }
    }

    /// End-to-end: a stale heartbeat file (old `written_at`) is ignored in
    /// favor of the pidfile fallback — never trusts an old snapshot.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // env_lock must span the awaited read — same idiom as tools_config_api.rs tests
    async fn end_to_end_stale_heartbeat_file_falls_back_to_pidfile_probe() {
        let _g = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };

        let mut status = sample_status(true, 9);
        status.written_at = (chrono::Utc::now() - chrono::Duration::seconds(STALENESS_WINDOW_SECS + 30))
            .to_rfc3339();
        ironhermes_gateway::pid::write_gateway_status(dir.path(), &status)
            .expect("write stale gateway-status.json");

        let map = read_platform_status_inner(ConfigScope::Root).await;

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        // Falls back to pidfile probe (NotRunning here — no gateway.pid
        // either), never the stale heartbeat's connected=true/count=9.
        let entry = map.get("telegram").expect("telegram key present");
        assert!(!entry.connected);
        assert_eq!(entry.session_count, None);
    }
}
