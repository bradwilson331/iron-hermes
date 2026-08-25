//! Phase 48.2 Plan 11 (D-03/D-14/D-16, G-48.2-6 slice a) + Plan 13 (G-48.2-6
//! slice b): the RUNTIME section — the live pidfile+probe status for the
//! `ironhermes gateway` process, plus the start/stop/restart controls
//! introduced by Plan 13.
//!
//! Extracted out of `tools.rs` verbatim for the status half (behavior
//! unchanged from Plan 11 — module doc, `RuntimeSection`, and
//! `runtime_pill` are the same code that lived inline there), so the
//! control surface lives in a module with its own security doc rather than
//! growing inside the page shell.
//!
//! # The control surface's security posture (Plan 13)
//!
//! Every control here is disabled-by-declaration, never live-but-silent,
//! when either `security.web_config_write_enabled` or the NEW
//! `security.web_process_control_enabled` is closed — the exact wording is
//! fetched from [`crate::server::gateway_control_api::get_process_control_gate_status`]
//! (the client-visible twin of the server's own gate helper) rather than a
//! second, hand-written copy of that text. The argon2id session boundary
//! is inherited from `auth::require_auth`; this file does nothing of its
//! own to re-check or weaken it.
//!
//! All three lifecycle controls are wired: STOP (Task 1), START (Task 2),
//! and RESTART (Task 3, which composes the other two rather than a third
//! implementation — see `gateway_control_api::perform_restart`).
//!
//! # No background poll (T-48.2-11-08, unchanged by Plan 13)
//!
//! A pidfile is not an event source. REFRESH bumps the page's own
//! `refresh_tick` — the same signal every lifecycle action bumps on
//! completion — which covers the real cases without inventing a second
//! freshness clock. Do not "fix" this into a `setInterval`-style poll.

use crate::server::gateway_control_api::{
    get_process_control_gate_status, restart_gateway, start_gateway, stop_gateway,
    GatewayLifecycleOutcome, ProcessControlGateStatus,
};
use crate::server::gateway_status_api::GatewayRuntimeStatus;
use crate::server::tools_config_api::ConfigScope;
use dioxus::prelude::*;

/// Phase 48.2 Plan 11: RUNTIME pill class/label/optional-detail for
/// `status`. Mirrors `mcp_section.rs`'s `mcp_status_pill_class` per-state
/// mapping (UI-SPEC "Which CSS layer this phase lives in" / Task 1's `d`):
/// a live gateway reuses CONNECTED's `pill teal`, a down gateway reuses
/// AUTH REQUIRED's `pill amber`, and a stale pidfile reuses the failure
/// `pill red` — never a fourth color vocabulary. `None` (the resource has
/// not resolved yet) renders a neutral checking state, distinct from every
/// resolved state.
///
/// `pub(super)` (not module-private): `buzz_section.rs` — a sibling module
/// under `tools/` — reuses this exact classifier so the BUZZ section's own
/// gateway-status note (48.2-12) renders the identical pill treatment
/// rather than a second, hand-written copy (Task 4's regression check).
pub(super) fn runtime_pill(
    status: Option<&GatewayRuntimeStatus>,
) -> (&'static str, &'static str, Option<String>) {
    match status {
        None => ("pill", "CHECKING…", None),
        Some(GatewayRuntimeStatus::Running { pid, .. }) => {
            ("pill teal", "RUNNING", Some(format!("pid {pid}")))
        }
        Some(GatewayRuntimeStatus::RunningOtherUser { pid, .. }) => (
            "pill teal",
            "RUNNING",
            Some(format!(
                "pid {pid} — owned by another user; a stop attempt from this operator will fail"
            )),
        ),
        Some(GatewayRuntimeStatus::NotRunning) => ("pill amber", "NOT RUNNING", None),
        Some(GatewayRuntimeStatus::StalePidfile { pid }) => (
            "pill red",
            "STALE PIDFILE",
            Some(format!(
                "pid {pid} is no longer running; the file was not cleaned up"
            )),
        ),
        Some(GatewayRuntimeStatus::UnsupportedPlatform) => ("pill", "UNSUPPORTED PLATFORM", None),
        Some(GatewayRuntimeStatus::Unknown { reason }) => ("pill", "UNKNOWN", Some(reason.clone())),
    }
}

/// Which lifecycle action a confirm dialog names.
#[derive(Clone, Copy, PartialEq, Debug)]
enum RuntimeAction {
    Stop,
    Start,
    Restart,
}

#[allow(dead_code)] // constructed inside RuntimeSection rsx!; dead_code fires on test target
fn runtime_confirm_title(action: RuntimeAction) -> &'static str {
    match action {
        RuntimeAction::Stop => "STOP GATEWAY?",
        RuntimeAction::Start => "START GATEWAY?",
        RuntimeAction::Restart => "RESTART GATEWAY?",
    }
}

/// D-17-precedent copywriting (`bulk_confirm.rs`): name exactly what is
/// about to happen before it happens. STOP's and RESTART's blast radius is
/// the whole point of their confirm — scheduled jobs and board automation
/// stop firing while the gateway is down, and any in-flight work it is
/// doing is interrupted; RESTART's body additionally states its own abort
/// rule so the operator knows what "restart" can leave them with. START has
/// no blast radius and is not dressed up as if it did — its only variable
/// is whether a stale pidfile is present, which `status` tells us: when it
/// is, the copy says a leftover pidfile will be cleaned up by the starting
/// process rather than staying silent about the only non-obvious fact of
/// that action. Every one of the three bodies is distinct text — never the
/// same wording reused across actions.
#[allow(dead_code)] // constructed inside RuntimeSection rsx!; dead_code fires on test target
fn runtime_confirm_body(action: RuntimeAction, status: Option<&GatewayRuntimeStatus>) -> String {
    match action {
        RuntimeAction::Stop => "STOP — scheduled jobs and board automation stop firing until \
            the gateway is started again, and any in-flight work it is doing is interrupted."
            .to_string(),
        RuntimeAction::Start => {
            if matches!(status, Some(GatewayRuntimeStatus::StalePidfile { .. })) {
                "START — a leftover pidfile is present from a previous run; it will be \
                    cleaned up automatically by the starting process."
                    .to_string()
            } else {
                "START the gateway process — this brings the cron scheduler and the kanban \
                    dispatch/notify loops back online."
                    .to_string()
            }
        }
        RuntimeAction::Restart => "RESTART — the gateway is stopped, its death confirmed, then \
            started again. Scheduled jobs and board automation stop firing while it is down, and \
            any in-flight work it is doing is interrupted. If the stop does not confirm within the \
            deadline, the restart aborts and the gateway is left stopped rather than started on top \
            of a process that may still be shutting down."
            .to_string(),
    }
}

/// The result line's text for `outcome` — rendered after a lifecycle action
/// completes. Never a raw OS error (the outcome DTO itself carries none —
/// `gateway_control_api`'s own T-48.2-13-11 contract).
#[allow(dead_code)] // used inside RuntimeSection rsx!; dead_code fires on test target
fn runtime_outcome_line(outcome: &GatewayLifecycleOutcome) -> String {
    use GatewayLifecycleOutcome::*;
    match outcome {
        Stopped { pid } => format!("STOPPED — pid {pid} confirmed gone."),
        StopNotConfirmed { pid } => {
            format!("SIGTERM sent to pid {pid}, but death was not confirmed in time.")
        }
        NotRunning => "Nothing was running.".to_string(),
        RefusedOtherUser { pid } => {
            format!("REFUSED — pid {pid} is owned by another user.")
        }
        RefusedInvalidTarget => "REFUSED — the recorded pid failed validation.".to_string(),
        RefusedAlreadyRunning { pid } => {
            format!("REFUSED — a gateway is already running (pid {pid}).")
        }
        Started { pid, log_path } => format!("STARTED — pid {pid} confirmed live. Log: {log_path}"),
        StartNotConfirmed { log_path } => {
            format!("Spawned, but not confirmed live in time. Log: {log_path}")
        }
        SpawnFailed => "The gateway process could not be spawned.".to_string(),
        StoppedButNotRestarted { pid } => format!(
            "STOPPED but NOT restarted — pid {pid}'s death was not confirmed, so start was not attempted."
        ),
        GateClosed { message } => format!("REFUSED — {message}."),
        CooldownActive => "REFUSED — please wait a moment before trying again.".to_string(),
        InvalidScope { message } => format!("REFUSED — {message}."),
        UnsupportedPlatform => {
            "REFUSED — this platform cannot control the gateway process.".to_string()
        }
        InternalError { reason } => format!("REFUSED — {reason}."),
    }
}

/// Why a shown control is disabled (or `None` — enabled). A `String`
/// reason is resolved separately from the fetched gate message / a fixed
/// literal, so this predicate itself stays pure and comparable in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisabledReasonKind {
    None,
    GateClosed,
    OtherUserOwned,
    Unsupported,
}

/// Whether one control renders at all, and if so, why it might be disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ControlState {
    shown: bool,
    reason: DisabledReasonKind,
}

/// Phase 48.2 Plan 13 Task 3: the pure control-state predicate — `(start,
/// stop, restart)` for every `status` value, independent of `submitting`
/// (an in-flight submission additionally disables a SHOWN control; that is
/// applied at the call site, not here, since it's not a property of the
/// gateway's state).
///
/// UnsupportedPlatform is the one case where EVERY control is shown
/// (never hidden) and disabled with the unsupported reason — Task 3's own
/// requirement: "render the controls disabled... rather than hiding them,
/// so the operator learns why rather than wondering where they went."
/// Every other status follows Task 1/2's shown-when-actionable rule: STOP
/// and RESTART are shown in both running states (RESTART groups with STOP,
/// not START — Task 3(b)'s own grouping of their shared blast-radius
/// copy); START is shown for not-running and stale-pidfile. `None`
/// (still loading) and `Unknown` show nothing — an unproven fact offers no
/// action.
#[allow(dead_code)] // used inside RuntimeSection; dead_code fires on test target
fn control_states(
    status: Option<&GatewayRuntimeStatus>,
    gate_open: bool,
) -> (ControlState, ControlState, ControlState) {
    use GatewayRuntimeStatus::*;

    if matches!(status, Some(UnsupportedPlatform)) {
        let unsupported = ControlState {
            shown: true,
            reason: DisabledReasonKind::Unsupported,
        };
        return (unsupported, unsupported, unsupported);
    }

    let is_running = status
        .map(GatewayRuntimeStatus::is_confirmed_running)
        .unwrap_or(false);
    let is_other_user = matches!(status, Some(RunningOtherUser { .. }));
    let is_startable = matches!(status, Some(NotRunning) | Some(StalePidfile { .. }));
    let gate_reason = if gate_open {
        DisabledReasonKind::None
    } else {
        DisabledReasonKind::GateClosed
    };
    let running_reason = if is_other_user {
        DisabledReasonKind::OtherUserOwned
    } else {
        gate_reason
    };

    let start = ControlState {
        shown: is_startable,
        reason: gate_reason,
    };
    let stop = ControlState {
        shown: is_running,
        reason: running_reason,
    };
    let restart = ControlState {
        shown: is_running,
        reason: running_reason,
    };
    (start, stop, restart)
}

/// Resolves a [`DisabledReasonKind`] into the actual text shown to the
/// operator — the ONLY place `gate_message` (the fetched gate helper's own
/// text) and the fixed literals for the other two reasons live, so every
/// control renders identical wording for the identical underlying fact.
#[allow(dead_code)] // used inside RuntimeSection; dead_code fires on test target
fn disabled_reason_text(kind: DisabledReasonKind, gate_message: &Option<String>) -> Option<String> {
    match kind {
        DisabledReasonKind::None => None,
        DisabledReasonKind::GateClosed => gate_message.clone(),
        DisabledReasonKind::OtherUserOwned => Some(
            "the running gateway is owned by another user; the underlying signal would fail"
                .to_string(),
        ),
        DisabledReasonKind::Unsupported => {
            Some("this platform cannot control the gateway process".to_string())
        }
    }
}

/// Phase 48.2 Plan 11 (G-48.2-6 slice a) + Plan 13 (G-48.2-6 slice b): the
/// RUNTIME section, mounted above `McpSection`. States the live pidfile+
/// probe fact for `scope`'s gateway, what that fact means for THIS page's
/// tools (the cron scheduler and kanban dispatch/notify loops the gateway
/// hosts — `runner.rs:2041-2061`, kanban dispatcher at `:2064+`, notifier
/// at `:2132`), the two facts that deliberately get NO status pill
/// (`ironhermes acp` and the kanban worker toolset), and — Plan 13 — the
/// operator's start/stop/restart controls.
#[component]
pub fn RuntimeSection(
    status: Option<GatewayRuntimeStatus>,
    scope: ConfigScope,
    mut refresh_tick: Signal<u32>,
) -> Element {
    let (pill_class, pill_label, detail) = runtime_pill(status.as_ref());

    // Plan 13: the gate helper's own text, fetched fresh on every
    // `refresh_tick` bump — never a second, hand-written copy of the
    // message `gateway_control_api::process_control_gate_message` would
    // have produced server-side.
    let gate_resource = use_resource(move || {
        let _tick = refresh_tick();
        async move { get_process_control_gate_status().await }
    });
    let gate_status: Option<ProcessControlGateStatus> = match gate_resource() {
        Some(Ok(s)) => Some(s),
        _ => None,
    };
    // Fail closed while the gate status has not resolved yet, or errored —
    // never render a control live before its gate is proven open.
    let gate_open = gate_status.as_ref().map(|s| s.open).unwrap_or(false);
    let gate_message = gate_status.and_then(|s| s.message);

    let mut confirm_action: Signal<Option<RuntimeAction>> = use_signal(|| None);
    let mut submitting: Signal<bool> = use_signal(|| false);
    let mut last_outcome: Signal<Option<GatewayLifecycleOutcome>> = use_signal(|| None);

    let (start_state, stop_state, restart_state) = control_states(status.as_ref(), gate_open);
    let is_submitting = *submitting.read();
    let start_disabled = start_state.reason != DisabledReasonKind::None || is_submitting;
    let stop_disabled = stop_state.reason != DisabledReasonKind::None || is_submitting;
    let restart_disabled = restart_state.reason != DisabledReasonKind::None || is_submitting;
    let start_disabled_reason = disabled_reason_text(start_state.reason, &gate_message);
    let stop_disabled_reason = disabled_reason_text(stop_state.reason, &gate_message);
    let restart_disabled_reason = disabled_reason_text(restart_state.reason, &gate_message);

    let scope_for_confirm = scope.clone();
    let status_for_confirm = status.clone();

    rsx! {
        div { class: "runtime-section",
            div { class: "runtime-section-header",
                div { class: "section-label", "RUNTIME" }
                button {
                    class: "btn btn--ghost btn--sm",
                    "aria-label": "Refresh gateway status",
                    onclick: move |_| {
                        let cur = *refresh_tick.read();
                        refresh_tick.set(cur + 1);
                    },
                    "REFRESH"
                }
            }
            div { class: "runtime-status-row",
                span { class: "{pill_class}", "{pill_label}" }
                if let Some(detail) = detail {
                    span { class: "runtime-status-detail", "{detail}" }
                }
            }
            div { class: "runtime-controls",
                if start_state.shown {
                    button {
                        class: "btn btn--sm",
                        "aria-label": "Start the gateway process",
                        title: start_disabled_reason.clone().unwrap_or_default(),
                        disabled: start_disabled,
                        onclick: move |_| {
                            if !start_disabled {
                                confirm_action.set(Some(RuntimeAction::Start));
                            }
                        },
                        "START"
                    }
                }
                if stop_state.shown {
                    button {
                        class: "btn btn--danger btn--sm",
                        "aria-label": "Stop the gateway process",
                        title: stop_disabled_reason.clone().unwrap_or_default(),
                        disabled: stop_disabled,
                        onclick: move |_| {
                            if !stop_disabled {
                                confirm_action.set(Some(RuntimeAction::Stop));
                            }
                        },
                        "STOP"
                    }
                }
                if restart_state.shown {
                    button {
                        class: "btn btn--danger btn--sm",
                        "aria-label": "Restart the gateway process",
                        title: restart_disabled_reason.clone().unwrap_or_default(),
                        disabled: restart_disabled,
                        onclick: move |_| {
                            if !restart_disabled {
                                confirm_action.set(Some(RuntimeAction::Restart));
                            }
                        },
                        "RESTART"
                    }
                }
            }
            if let Some(action) = confirm_action() {
                div { class: "bulk-confirm-overlay", role: "presentation",
                    div {
                        class: "bulk-confirm-dialog",
                        role: "dialog",
                        aria_modal: "true",
                        "aria-labelledby": "runtime-confirm-title",
                        div { class: "bulk-confirm-title", id: "runtime-confirm-title", "{runtime_confirm_title(action)}" }
                        div { class: "bulk-confirm-body", "{runtime_confirm_body(action, status_for_confirm.as_ref())}" }
                        div { class: "bulk-confirm-actions",
                            button {
                                class: "btn btn--ghost btn--sm",
                                "aria-label": "Cancel gateway action",
                                disabled: *submitting.read(),
                                onclick: move |_| confirm_action.set(None),
                                "CANCEL"
                            }
                            button {
                                class: if action == RuntimeAction::Start { "btn btn--sm" } else { "btn btn--danger btn--sm" },
                                "aria-label": "Confirm {runtime_confirm_title(action)}",
                                disabled: *submitting.read(),
                                onclick: move |_| {
                                    if *submitting.read() {
                                        return;
                                    }
                                    submitting.set(true);
                                    let scope_value = scope_for_confirm.clone();
                                    spawn(async move {
                                        let outcome = match action {
                                            RuntimeAction::Stop => stop_gateway(scope_value).await,
                                            RuntimeAction::Start => start_gateway(scope_value).await,
                                            RuntimeAction::Restart => restart_gateway(scope_value).await,
                                        };
                                        submitting.set(false);
                                        confirm_action.set(None);
                                        let cur = *refresh_tick.read();
                                        refresh_tick.set(cur + 1);
                                        last_outcome
                                            .set(Some(outcome.unwrap_or(GatewayLifecycleOutcome::InternalError {
                                                reason: "the request failed".to_string(),
                                            })));
                                    });
                                },
                                if *submitting.read() { "WORKING…" } else { "CONFIRM" }
                            }
                        }
                    }
                }
            }
            if let Some(outcome) = last_outcome() {
                div { class: "runtime-result-line", "{runtime_outcome_line(&outcome)}" }
            }
            p { class: "runtime-status-explain",
                "The gateway process hosts the cron scheduler and the kanban dispatch/notify loops. With it down, creating a schedule or a board action still works — nothing will fire it until the gateway runs again."
            }
            p { class: "runtime-status-explain",
                "\"ironhermes acp\" is a stdio JSON-RPC server whose lifecycle belongs to the client that spawns it — it has no pidfile, port, or health endpoint, so this page does not claim to know its state."
            }
            p { class: "runtime-status-explain",
                "Kanban worker tools are registered only inside a kanban worker process and do not appear in this catalog — the gateway dependency you will feel on the board page is its automated dispatch and notification."
            }
        }
    }
}

#[cfg(test)]
mod runtime_pill_tests {
    use super::*;

    #[test]
    fn running_gets_the_connected_treatment() {
        let (class, label, _) = runtime_pill(Some(&GatewayRuntimeStatus::Running {
            pid: 1,
            started_at: "t".to_string(),
            profile: "default".to_string(),
        }));
        assert_eq!(class, "pill teal");
        assert_eq!(label, "RUNNING");
    }

    #[test]
    fn not_running_gets_the_auth_required_treatment() {
        let (class, _, _) = runtime_pill(Some(&GatewayRuntimeStatus::NotRunning));
        assert_eq!(class, "pill amber");
    }

    #[test]
    fn stale_pidfile_gets_the_failure_treatment_and_is_distinct_from_not_running() {
        let (class, label, _) = runtime_pill(Some(&GatewayRuntimeStatus::StalePidfile { pid: 42 }));
        assert_eq!(class, "pill red");
        assert_ne!(label, "NOT RUNNING");
    }

    // -------------------------------------------------------------------
    // Phase 48.2 Plan 13 (Task 1) — outcome line + status predicate tests.
    // -------------------------------------------------------------------

    #[test]
    fn outcome_line_never_echoes_a_raw_os_error() {
        // Every variant's text is fixed/pre-written — this is a smoke test
        // that the match itself is exhaustive and produces non-empty text
        // for every outcome, not a claim about specific wording.
        let outcomes = [
            GatewayLifecycleOutcome::Stopped { pid: 1 },
            GatewayLifecycleOutcome::StopNotConfirmed { pid: 1 },
            GatewayLifecycleOutcome::NotRunning,
            GatewayLifecycleOutcome::RefusedOtherUser { pid: 1 },
            GatewayLifecycleOutcome::RefusedInvalidTarget,
            GatewayLifecycleOutcome::RefusedAlreadyRunning { pid: 1 },
            GatewayLifecycleOutcome::Started {
                pid: 1,
                log_path: "x".to_string(),
            },
            GatewayLifecycleOutcome::StartNotConfirmed {
                log_path: "x".to_string(),
            },
            GatewayLifecycleOutcome::SpawnFailed,
            GatewayLifecycleOutcome::StoppedButNotRestarted { pid: 1 },
            GatewayLifecycleOutcome::GateClosed {
                message: "m".to_string(),
            },
            GatewayLifecycleOutcome::CooldownActive,
            GatewayLifecycleOutcome::InvalidScope {
                message: "m".to_string(),
            },
            GatewayLifecycleOutcome::UnsupportedPlatform,
            GatewayLifecycleOutcome::InternalError {
                reason: "r".to_string(),
            },
        ];
        for outcome in &outcomes {
            assert!(!runtime_outcome_line(outcome).is_empty());
        }
    }

    /// The status-to-control predicate (`is_confirmed_running`, inlined in
    /// `RuntimeSection`) enables STOP only in the running states —
    /// `GatewayRuntimeStatus::is_confirmed_running` itself already carries
    /// the unit-test coverage for the full state matrix (48.2-11), so this
    /// pins the ADDITIONAL Plan 13 fact: `RunningOtherUser` counts as
    /// "shown" (STOP renders) but not "enabled" (the disabled-reason path
    /// takes over) — asserted directly against the enum here since that
    /// combination is specific to this file's own logic, not
    /// `is_confirmed_running`'s.
    #[test]
    fn running_other_user_is_confirmed_running_but_flagged_separately_for_disable() {
        let status = GatewayRuntimeStatus::RunningOtherUser {
            pid: 1,
            started_at: "t".to_string(),
            profile: "default".to_string(),
        };
        assert!(
            status.is_confirmed_running(),
            "STOP must be SHOWN for this state"
        );
        assert!(
            matches!(status, GatewayRuntimeStatus::RunningOtherUser { .. }),
            "STOP must be DISABLED for this state via the other-user reason"
        );
    }

    // -------------------------------------------------------------------
    // Phase 48.2 Plan 13 Task 2 — START's confirm copy.
    // -------------------------------------------------------------------

    #[test]
    fn start_confirm_body_names_the_leftover_pidfile_only_when_stale() {
        let stale = GatewayRuntimeStatus::StalePidfile { pid: 9 };
        let not_running = GatewayRuntimeStatus::NotRunning;

        let stale_body = runtime_confirm_body(RuntimeAction::Start, Some(&stale));
        let not_running_body = runtime_confirm_body(RuntimeAction::Start, Some(&not_running));

        assert!(
            stale_body.contains("leftover pidfile"),
            "stale-pidfile start must mention the leftover file: {stale_body}"
        );
        assert!(
            !not_running_body.contains("leftover pidfile"),
            "an ordinary start must not mention a pidfile that is not there: {not_running_body}"
        );
        assert_ne!(stale_body, not_running_body);
    }

    /// Task 3's own wording contract, already true from Task 2: START has
    /// no blast radius and must not be dressed up as if it did — its body
    /// must not share STOP's wording.
    #[test]
    fn start_and_stop_confirm_bodies_never_share_wording() {
        let start = runtime_confirm_body(
            RuntimeAction::Start,
            Some(&GatewayRuntimeStatus::NotRunning),
        );
        let stop = runtime_confirm_body(RuntimeAction::Stop, None);
        assert_ne!(start, stop);
        assert_ne!(
            runtime_confirm_title(RuntimeAction::Start),
            runtime_confirm_title(RuntimeAction::Stop)
        );
    }

    #[test]
    fn restart_confirm_body_is_distinct_from_stop_and_start() {
        let restart = runtime_confirm_body(
            RuntimeAction::Restart,
            Some(&GatewayRuntimeStatus::Running {
                pid: 1,
                started_at: "t".to_string(),
                profile: "default".to_string(),
            }),
        );
        let stop = runtime_confirm_body(RuntimeAction::Stop, None);
        let start = runtime_confirm_body(
            RuntimeAction::Start,
            Some(&GatewayRuntimeStatus::NotRunning),
        );
        assert_ne!(restart, stop);
        assert_ne!(restart, start);
        assert_ne!(
            runtime_confirm_title(RuntimeAction::Restart),
            runtime_confirm_title(RuntimeAction::Stop)
        );
    }

    // -------------------------------------------------------------------
    // Phase 48.2 Plan 13 Task 3 — the control-state predicate, for every
    // status value including the unsupported one.
    // -------------------------------------------------------------------

    fn running() -> GatewayRuntimeStatus {
        GatewayRuntimeStatus::Running {
            pid: 1,
            started_at: "t".to_string(),
            profile: "default".to_string(),
        }
    }
    fn running_other_user() -> GatewayRuntimeStatus {
        GatewayRuntimeStatus::RunningOtherUser {
            pid: 1,
            started_at: "t".to_string(),
            profile: "default".to_string(),
        }
    }

    #[test]
    fn control_states_for_running_shows_stop_and_restart_only() {
        let (start, stop, restart) = control_states(Some(&running()), true);
        assert!(!start.shown, "START must not show while already running");
        assert!(stop.shown && stop.reason == DisabledReasonKind::None);
        assert!(restart.shown && restart.reason == DisabledReasonKind::None);
    }

    #[test]
    fn control_states_for_running_other_user_shows_stop_and_restart_disabled() {
        let (start, stop, restart) = control_states(Some(&running_other_user()), true);
        assert!(!start.shown);
        assert!(stop.shown && stop.reason == DisabledReasonKind::OtherUserOwned);
        assert!(restart.shown && restart.reason == DisabledReasonKind::OtherUserOwned);
    }

    #[test]
    fn control_states_for_not_running_shows_start_only() {
        let (start, stop, restart) = control_states(Some(&GatewayRuntimeStatus::NotRunning), true);
        assert!(start.shown && start.reason == DisabledReasonKind::None);
        assert!(!stop.shown);
        assert!(!restart.shown);
    }

    #[test]
    fn control_states_for_stale_pidfile_shows_start_only() {
        let (start, stop, restart) =
            control_states(Some(&GatewayRuntimeStatus::StalePidfile { pid: 9 }), true);
        assert!(start.shown && start.reason == DisabledReasonKind::None);
        assert!(!stop.shown);
        assert!(!restart.shown);
    }

    #[test]
    fn control_states_for_unsupported_platform_shows_all_three_disabled() {
        let (start, stop, restart) =
            control_states(Some(&GatewayRuntimeStatus::UnsupportedPlatform), true);
        for state in [start, stop, restart] {
            assert!(
                state.shown,
                "unsupported platform must SHOW every control, never hide it"
            );
            assert_eq!(state.reason, DisabledReasonKind::Unsupported);
        }
    }

    #[test]
    fn control_states_for_unknown_and_loading_show_nothing() {
        let (start, stop, restart) = control_states(
            Some(&GatewayRuntimeStatus::Unknown {
                reason: "x".to_string(),
            }),
            true,
        );
        assert!(!start.shown && !stop.shown && !restart.shown);

        let (start, stop, restart) = control_states(None, true);
        assert!(!start.shown && !stop.shown && !restart.shown);
    }

    #[test]
    fn control_states_close_gate_disables_every_shown_control() {
        let (_start, stop, restart) = control_states(Some(&running()), false);
        assert_eq!(stop.reason, DisabledReasonKind::GateClosed);
        assert_eq!(restart.reason, DisabledReasonKind::GateClosed);

        let (start, _stop, _restart) =
            control_states(Some(&GatewayRuntimeStatus::NotRunning), false);
        assert_eq!(start.reason, DisabledReasonKind::GateClosed);
    }

    #[test]
    fn disabled_reason_text_uses_the_fetched_gate_message_for_gate_closed() {
        let msg = Some("security.web_process_control_enabled is false".to_string());
        assert_eq!(
            disabled_reason_text(DisabledReasonKind::GateClosed, &msg),
            msg
        );
        assert_eq!(disabled_reason_text(DisabledReasonKind::None, &msg), None);
    }
}
