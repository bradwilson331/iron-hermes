//! Phase 46.9 Plan 11 (GAP-4/GAP-5): footer display-timezone read fn.
//!
//! Closes UAT GAP-4 (footer clock hardcoded to UTC despite a configured
//! `agent.timezone`) and GAP-5 (no multi-timezone footer). This is a
//! read-only surface — no write gate, no secret/config value beyond
//! timezone name strings (T-46.9-29) — mirroring `get_config_summary`'s
//! ungated `Config::load()` read shape (`server/api.rs:366-378`) and the
//! NEW-server-module file layout of `provider_config_api.rs`.
//!
//! `#[server]` (not `#[get]`) is used like `provider_config_api.rs` /
//! `schedules_api.rs`: the macro handles the client/server split, and this
//! file reads FRESH from disk via `Config::load()` on every call (config
//! never hot-reloads — `kanban_api.rs:946-950`), not the startup
//! `app_state.config` snapshot. The resolution logic is factored into
//! `resolve_display_timezones` (native-only helper) so it is directly unit
//! testable against a `Config` fixture, mirroring `provider_config_api.rs`'s
//! `merge_provider_payload`/`build_provider_snapshot` pattern.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Maximum number of extra timezones rendered in the footer (T-46.9-27: a
/// server-side cap so an oversized `config.display.extra_timezones` list
/// cannot spawn unbounded footer clocks).
pub const MAX_EXTRA_TIMEZONES: usize = 4;

/// Read-only snapshot returned by [`get_display_timezones`]. `primary` is
/// the resolved IANA zone for the footer's primary wall-clock — `None`
/// means "format in the browser's host-local zone" (GAP-4's fix: this used
/// to be an implicit hardcoded UTC on the client). `extras` is the
/// `config.display.extra_timezones` list truncated to [`MAX_EXTRA_TIMEZONES`].
/// `hour12` is the resolved QDG-01 footer clock format — `false` (default)
/// renders 24-hour, `true` renders 12-hour AM/PM.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct DisplayTimezones {
    pub primary: Option<String>,
    pub extras: Vec<String>,
    pub hour12: bool,
}

/// Phase 49.4 Plan 04 (D-13): resolve `primary = display.timezone.or(agent.timezone)`
/// and `hour12 = display.hour12.unwrap_or(false)` — the single source of
/// truth for the display-timezone + hour-format resolution rule. Extracted
/// out of [`resolve_display_timezones`] so `schedules_api.rs` can share the
/// exact same rule instead of re-reading `config.agent.timezone` directly
/// (the D-13 bug this plan fixes). Pure/native-only so it is directly unit
/// testable without a filesystem-backed `Config::load()`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_display_tz_parts(
    config: &ironhermes_core::config::Config,
) -> (Option<String>, bool) {
    let primary = config
        .display
        .timezone
        .clone()
        .or_else(|| config.agent.timezone.clone());
    let hour12 = config.display.hour12.unwrap_or(false);
    (primary, hour12)
}

/// Resolve `primary = display.timezone.or(agent.timezone)` and truncate
/// `extra_timezones` to [`MAX_EXTRA_TIMEZONES`]. Pure/native-only so it is
/// directly unit testable without a filesystem-backed `Config::load()`.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_display_timezones(config: &ironhermes_core::config::Config) -> DisplayTimezones {
    let (primary, hour12) = resolve_display_tz_parts(config);
    let extras = config
        .display
        .extra_timezones
        .iter()
        .take(MAX_EXTRA_TIMEZONES)
        .cloned()
        .collect();

    DisplayTimezones {
        primary,
        extras,
        hour12,
    }
}

/// Phase 46.9 Plan 11: Resolve the footer's primary + extra display
/// timezones from a fresh `Config::load()`. No write gate — this is a
/// read-only surface returning only timezone name strings (T-46.9-29).
#[server]
pub async fn get_display_timezones() -> Result<DisplayTimezones, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

    Ok(resolve_display_timezones(&config))
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::{resolve_display_timezones, resolve_display_tz_parts};
    use ironhermes_core::config::{Config, DisplayConfig};

    fn config_with(display: DisplayConfig, agent_timezone: Option<&str>) -> Config {
        let mut config = Config {
            display,
            ..Config::default()
        };
        config.agent.timezone = agent_timezone.map(str::to_string);
        config
    }

    // =========================================================================
    // Phase 46.9 Plan 11 (GAP-4): primary resolution — display > agent > None
    // =========================================================================

    #[test]
    fn primary_prefers_display_timezone_over_agent_timezone() {
        let config = config_with(
            DisplayConfig {
                timezone: Some("America/New_York".to_string()),
                extra_timezones: vec![],
                ..Default::default()
            },
            Some("Europe/London"),
        );
        let resolved = resolve_display_timezones(&config);
        assert_eq!(resolved.primary.as_deref(), Some("America/New_York"));
    }

    #[test]
    fn primary_falls_back_to_agent_timezone_when_display_unset() {
        let config = config_with(DisplayConfig::default(), Some("Europe/London"));
        let resolved = resolve_display_timezones(&config);
        assert_eq!(resolved.primary.as_deref(), Some("Europe/London"));
    }

    #[test]
    fn primary_is_none_when_neither_display_nor_agent_timezone_set() {
        let config = config_with(DisplayConfig::default(), None);
        let resolved = resolve_display_timezones(&config);
        assert_eq!(resolved.primary, None);
    }

    // =========================================================================
    // Phase 46.9 Plan 11 (GAP-5 / T-46.9-27): extras truncation
    // =========================================================================

    /// T-46.9-27: more than MAX_EXTRA_TIMEZONES extras must truncate, not error.
    #[test]
    fn extras_truncate_to_max_four() {
        let config = config_with(
            DisplayConfig {
                timezone: None,
                extra_timezones: vec![
                    "America/New_York".to_string(),
                    "Europe/London".to_string(),
                    "Asia/Tokyo".to_string(),
                    "Australia/Sydney".to_string(),
                    "Europe/Paris".to_string(), // 5th — must be dropped
                ],
                ..Default::default()
            },
            None,
        );
        let resolved = resolve_display_timezones(&config);
        assert_eq!(resolved.extras.len(), 4);
        assert_eq!(
            resolved.extras,
            vec![
                "America/New_York".to_string(),
                "Europe/London".to_string(),
                "Asia/Tokyo".to_string(),
                "Australia/Sydney".to_string(),
            ]
        );
    }

    #[test]
    fn extras_under_cap_pass_through_unchanged() {
        let extras = vec!["America/New_York".to_string(), "Europe/London".to_string()];
        let config = config_with(
            DisplayConfig {
                timezone: None,
                extra_timezones: extras.clone(),
                ..Default::default()
            },
            None,
        );
        let resolved = resolve_display_timezones(&config);
        assert_eq!(resolved.extras, extras);
    }

    #[test]
    fn display_timezones_default_is_empty() {
        let dt = super::DisplayTimezones::default();
        assert_eq!(dt.primary, None);
        assert!(dt.extras.is_empty());
    }

    // =========================================================================
    // Quick task 260718-qdg: hour12 resolution — None -> false, Some(true) -> true
    // =========================================================================

    #[test]
    fn hour12_resolves_true_when_config_sets_it() {
        let config = config_with(
            DisplayConfig {
                hour12: Some(true),
                ..Default::default()
            },
            None,
        );
        let resolved = resolve_display_timezones(&config);
        assert!(resolved.hour12);
    }

    #[test]
    fn hour12_defaults_to_false_when_config_unset() {
        let config = config_with(DisplayConfig::default(), None);
        let resolved = resolve_display_timezones(&config);
        assert!(!resolved.hour12);
    }

    // =========================================================================
    // Phase 49.4 Plan 04 (D-13): resolve_display_tz_parts — the shared
    // primary/hour12 resolver `schedules_api.rs` reads instead of
    // `config.agent.timezone` directly.
    // =========================================================================

    #[test]
    fn parts_prefers_display_timezone_over_agent_timezone() {
        let config = config_with(
            DisplayConfig {
                timezone: Some("America/New_York".to_string()),
                extra_timezones: vec![],
                ..Default::default()
            },
            Some("Europe/London"),
        );
        let (primary, _hour12) = resolve_display_tz_parts(&config);
        assert_eq!(primary.as_deref(), Some("America/New_York"));
    }

    #[test]
    fn parts_falls_back_to_agent_timezone_when_display_unset() {
        let config = config_with(DisplayConfig::default(), Some("Europe/London"));
        let (primary, _hour12) = resolve_display_tz_parts(&config);
        assert_eq!(primary.as_deref(), Some("Europe/London"));
    }

    #[test]
    fn parts_primary_is_none_when_neither_set() {
        let config = config_with(DisplayConfig::default(), None);
        let (primary, _hour12) = resolve_display_tz_parts(&config);
        assert_eq!(primary, None);
    }

    #[test]
    fn parts_hour12_matches_resolve_display_timezones() {
        let config = config_with(
            DisplayConfig {
                hour12: Some(true),
                ..Default::default()
            },
            None,
        );
        let (_primary, hour12) = resolve_display_tz_parts(&config);
        assert!(hour12);
        // Cross-check: the full DisplayTimezones resolver must agree —
        // there is exactly one implementation of the rule.
        assert_eq!(resolve_display_timezones(&config).hour12, hour12);
    }
}
