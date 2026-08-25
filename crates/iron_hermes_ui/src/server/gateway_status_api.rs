//! Phase 48.2 Plan 11 (D-03/D-14/D-16, G-48.2-6 slice a): read-only,
//! scope-aware gateway liveness for the Tools page RUNTIME section.
//!
//! Liveness is already observable through `$IRONHERMES_HOME/gateway.pid` +
//! `ironhermes_gateway::pid::read_gateway_pid` + the signal-0
//! `is_pid_alive` probe (pid.rs:88,113) — the CLI `status`/`doctor`
//! commands already read it (`ironhermes-cli/src/status_cmd.rs:107-112`).
//! Before this plan nothing on the web server side ever read it: the Tools
//! page had no way to tell an operator that the cron tick loop
//! (`runner.rs:2041-2061`), the kanban dispatcher (`:2064+`), and the
//! notifier (`:2132`) it hosts were not running — the exact silence the
//! UAT screenshot caught on `cronjob`.
//!
//! # Six honest states, never collapsed (D-16)
//!
//! [`GatewayRuntimeStatus`] carries exactly what the pidfile + probe can
//! prove: `Running`/`RunningOtherUser` (both carry pid/started_at/profile —
//! "other user" is a DISTINCT fact from plain `Running`, because it is what
//! will make a later stop attempt fail), `NotRunning` (no pidfile),
//! `StalePidfile` (file present, process gone — kept SEPARATE from
//! `NotRunning`; an operator with a leftover file needs to know it is
//! there), `UnsupportedPlatform`, and `Unknown { reason }` for a read
//! failure. [`classify_gateway_status`] is the pure core: every branch is
//! unit-testable from a hand-built record and an injected probe closure, no
//! real process or pidfile required.
//!
//! # The non-Unix panic never reaches a request handler (T-48.2-11-01)
//!
//! `ironhermes_gateway::pid::is_pid_alive` PANICS by construction on
//! non-Unix (pid.rs:127-133). [`read_gateway_runtime_status`] calls it ONLY
//! inside a `#[cfg(unix)]` arm; the `#[cfg(not(unix))]` arm returns
//! `UnsupportedPlatform` WITHOUT calling the probe or even reading the
//! pidfile — a panicking server fn would be a denial of service on every
//! request to this endpoint, not a compile-time concern.
//!
//! # This is a READ — no write gate (T-48.2-11-07)
//!
//! Unlike every write-side fn in `tools_config_api.rs`, this module never
//! calls `check_tools_write_gate` — reading whether a process is alive is
//! not a write-class action, and gating it would blank the status on a
//! read-only deployment, which is precisely the deployment most likely to
//! need it. This plan adds no start/stop/restart control anywhere; 48.2-13
//! owns process lifecycle with its own threat model.
//!
//! # Scope resolution mirrors `tools_config_api::resolve_scope_target`
//!
//! Root scope reads `ironhermes_core::get_hermes_home()`; profile scope
//! validates the name through `ironhermes_core::profile::validate_profile_name`
//! BEFORE calling `profile_api::profile_dir_for` — the same validate-first
//! discipline `resolve_scope_target` already applies to a profile's
//! `config.yaml` read, for the identical class of client-supplied name
//! (T-48.2-11-03). An invalid profile name reports `Unknown`, never a raw
//! error and never a 500.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::server::tools_config_api::ConfigScope;

/// D-16: every state the pidfile + signal-0 probe can honestly produce, and
/// no others. `Running`/`RunningOtherUser` intentionally carry the SAME
/// payload shape (pid, started_at, profile) — the distinction is the fact
/// itself, not the data available. `StalePidfile` carries only the dead
/// pid: `started_at`/`profile` describe a process that no longer exists,
/// not the leftover file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GatewayRuntimeStatus {
    Running {
        pid: u32,
        started_at: String,
        profile: String,
    },
    RunningOtherUser {
        pid: u32,
        started_at: String,
        profile: String,
    },
    NotRunning,
    StalePidfile {
        pid: u32,
    },
    UnsupportedPlatform,
    Unknown {
        reason: String,
    },
}

impl GatewayRuntimeStatus {
    /// True for a CONFIRMED-running gateway (either user) — the only cases
    /// where the cron/kanban loops it hosts are actually ticking. Every
    /// other variant, including `StalePidfile` and `Unknown`, must NOT be
    /// treated as running: this is the exact fact `ToolCard`'s annotation
    /// predicate (catalog.rs, Task 3) branches on.
    pub fn is_confirmed_running(&self) -> bool {
        matches!(
            self,
            GatewayRuntimeStatus::Running { .. } | GatewayRuntimeStatus::RunningOtherUser { .. }
        )
    }
}

/// D-16: the pure classifier — every branch reachable from a hand-built
/// record and an injected probe closure, so it is directly unit-testable
/// without a real process or a real pidfile. `probe` mirrors
/// `ironhermes_gateway::pid::is_pid_alive`'s signature so the production
/// call site (`read_gateway_runtime_status` below) can pass the real fn
/// directly.
#[cfg(not(target_arch = "wasm32"))]
fn classify_gateway_status<F>(
    record: Option<ironhermes_gateway::pid::GatewayPidRecord>,
    probe: F,
) -> GatewayRuntimeStatus
where
    F: FnOnce(u32) -> ironhermes_gateway::pid::PidLiveness,
{
    use ironhermes_gateway::pid::PidLiveness;

    let Some(record) = record else {
        return GatewayRuntimeStatus::NotRunning;
    };

    match probe(record.pid) {
        PidLiveness::Live => GatewayRuntimeStatus::Running {
            pid: record.pid,
            started_at: record.started_at,
            profile: record.profile,
        },
        PidLiveness::LiveOtherUser => GatewayRuntimeStatus::RunningOtherUser {
            pid: record.pid,
            started_at: record.started_at,
            profile: record.profile,
        },
        PidLiveness::Stale => GatewayRuntimeStatus::StalePidfile { pid: record.pid },
    }
}

/// D-16/T-48.2-11-01/T-48.2-11-03/T-48.2-11-05: resolve `scope`'s home,
/// read its `gateway.pid`, and classify it. The probe is called ONLY under
/// `#[cfg(unix)]` (see module doc). An `Err` from `read_gateway_pid` (I/O or
/// parse failure) never forwards its raw text — it maps to `Unknown` with a
/// short fixed reason. An invalid profile name maps to `Unknown` the same
/// way, before any path is built.
#[cfg(not(target_arch = "wasm32"))]
fn read_gateway_runtime_status(scope: &ConfigScope) -> GatewayRuntimeStatus {
    #[cfg(unix)]
    {
        let home = match scope {
            ConfigScope::Root => ironhermes_core::get_hermes_home(),
            ConfigScope::Profile(name) => {
                match ironhermes_core::profile::validate_profile_name(name) {
                    Ok(validated) => crate::server::profile_api::profile_dir_for(&validated),
                    Err(_) => {
                        return GatewayRuntimeStatus::Unknown {
                            reason: "invalid profile name".to_string(),
                        };
                    }
                }
            }
        };

        match ironhermes_gateway::pid::read_gateway_pid(&home) {
            Ok(record) => classify_gateway_status(record, ironhermes_gateway::pid::is_pid_alive),
            Err(_) => GatewayRuntimeStatus::Unknown {
                reason: "gateway.pid could not be read".to_string(),
            },
        }
    }
    #[cfg(not(unix))]
    {
        let _ = scope;
        GatewayRuntimeStatus::UnsupportedPlatform
    }
}

/// Read-only gateway liveness for `scope`'s Tools page RUNTIME section.
/// Deliberately does NOT call `check_tools_write_gate` — see module doc
/// (T-48.2-11-07).
#[server]
pub async fn get_gateway_runtime_status(
    scope: ConfigScope,
) -> Result<GatewayRuntimeStatus, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(read_gateway_runtime_status(&scope))
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
    use ironhermes_gateway::pid::{GatewayPidRecord, PidLiveness};

    fn record(pid: u32) -> GatewayPidRecord {
        GatewayPidRecord {
            pid,
            started_at: "2026-08-22T00:00:00Z".to_string(),
            profile: "default".to_string(),
        }
    }

    #[test]
    fn classify_absent_record_is_not_running() {
        assert_eq!(
            classify_gateway_status(None, |_| PidLiveness::Live),
            GatewayRuntimeStatus::NotRunning
        );
    }

    #[test]
    fn classify_live_pid_is_running() {
        let status = classify_gateway_status(Some(record(4242)), |_| PidLiveness::Live);
        assert_eq!(
            status,
            GatewayRuntimeStatus::Running {
                pid: 4242,
                started_at: "2026-08-22T00:00:00Z".to_string(),
                profile: "default".to_string(),
            }
        );
    }

    #[test]
    fn classify_live_other_user_pid_is_running_other_user() {
        let status = classify_gateway_status(Some(record(4242)), |_| PidLiveness::LiveOtherUser);
        assert_eq!(
            status,
            GatewayRuntimeStatus::RunningOtherUser {
                pid: 4242,
                started_at: "2026-08-22T00:00:00Z".to_string(),
                profile: "default".to_string(),
            }
        );
    }

    /// The stale case must report the pid it found AND must not equal the
    /// not-running case — a leftover file is a distinct, more informative
    /// fact than "nothing has ever started here" (must_haves).
    #[test]
    fn classify_stale_pid_reports_pid_and_is_distinct_from_not_running() {
        let status = classify_gateway_status(Some(record(9999)), |_| PidLiveness::Stale);
        assert_eq!(status, GatewayRuntimeStatus::StalePidfile { pid: 9999 });
        assert_ne!(status, GatewayRuntimeStatus::NotRunning);
    }

    #[test]
    fn classify_read_error_maps_to_unknown_with_fixed_reason_no_matter_the_probe() {
        // No record at all is the "absent" branch (NotRunning), covered
        // above. This test instead exercises `read_gateway_runtime_status`'s
        // own error mapping end to end against a REAL malformed pidfile on
        // disk — the read error path `classify_gateway_status` itself never
        // sees, since it only receives an already-`Ok` `Option<record>`.
        let _g = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
        std::fs::write(dir.path().join("gateway.pid"), "not: a-valid-record\n")
            .expect("write malformed pidfile");

        let status = read_gateway_runtime_status(&ConfigScope::Root);

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        match status {
            GatewayRuntimeStatus::Unknown { reason } => {
                assert_eq!(reason, "gateway.pid could not be read");
            }
            other => panic!("expected Unknown from a malformed pidfile, got {other:?}"),
        }
    }

    /// T-48.2-11-03: an invalid (traversal-shaped) profile name must never
    /// reach `profile_dir_for` — it maps to `Unknown` before any path is
    /// built, mirroring `resolve_scope_target`'s validate-first discipline.
    #[test]
    fn classify_invalid_profile_name_is_unknown_not_a_path_traversal() {
        let status =
            read_gateway_runtime_status(&ConfigScope::Profile("../../etc/passwd".to_string()));
        match status {
            GatewayRuntimeStatus::Unknown { reason } => {
                assert_eq!(reason, "invalid profile name");
            }
            other => panic!("expected Unknown for an invalid profile name, got {other:?}"),
        }
    }

    /// Plan 11 verification step 7 — a "live" check against a REAL tempdir
    /// (real disk I/O, real `read_gateway_pid`, real signal-0 syscall via
    /// `is_pid_alive`) rather than the injected-closure classifier tests
    /// above: no gateway.pid at all reports `NotRunning`.
    #[test]
    fn live_check_no_pidfile_reports_not_running() {
        let _g = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };

        let status = read_gateway_runtime_status(&ConfigScope::Root);

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert_eq!(status, GatewayRuntimeStatus::NotRunning);
    }

    /// Plan 11 verification step 7 — the stale-pidfile half of the same
    /// live check: a real `gateway.pid` naming a pid that does not exist on
    /// this machine reports `StalePidfile`, never `Running`. The pid stays
    /// well within `i32` positive range (a negative value would target a
    /// process GROUP in POSIX `kill()` semantics, not a single pid) and
    /// well above any real `pid_max` on macOS/Linux dev machines.
    #[test]
    fn live_check_pidfile_naming_a_dead_pid_reports_stale_not_running() {
        let _g = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
        let dead_pid: u32 = 999_999_999;
        ironhermes_gateway::pid::write_gateway_pid(
            dir.path(),
            &GatewayPidRecord {
                pid: dead_pid,
                started_at: "2026-08-22T00:00:00Z".to_string(),
                profile: "default".to_string(),
            },
        )
        .expect("write real gateway.pid");

        let status = read_gateway_runtime_status(&ConfigScope::Root);

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert_eq!(status, GatewayRuntimeStatus::StalePidfile { pid: dead_pid });
    }
}
