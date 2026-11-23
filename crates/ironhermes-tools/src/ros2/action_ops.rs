//! ROS 2 action goal lifecycle — send, poll, cancel (Phase 49.8 D-06/D-04/D-08).
//!
//! Mirrors `publish`'s D-04 allowlist-before-I/O gate exactly: [`send_goal`]
//! looks up `ros2.actions` with [`find_allow_entry`] BEFORE
//! [`Ros2Session::client_for`] ever opens a connection — an undeclared or
//! type-mismatched action returns [`not_permitted`] with zero connections
//! opened. A declared entry's `limits` clamp the goal's dotted-path numeric
//! fields the same way `publish`/`call_service` do (clamp-then-send, never
//! reject). Every sent goal is registered with [`Ros2Session::track_goal`]
//! and given a watchdog: after `caps.max_goal_secs` with no terminal status,
//! the tool cancels it itself (D-08) — a goal is never left running
//! unbounded. [`cancel_goal`] cancels on the goal's own owning connection
//! when it is still open, otherwise falls back to
//! [`crate::ros2::session::cancel_all_goals_fallback`] on whatever connection
//! the caller currently holds — the same fallback [`Ros2Session::stop_all`]
//! uses after a reconnect.

use std::sync::Arc;

use ironhermes_core::Config;
use serde_json::{Value, json};
use tokio::time::Duration;

use crate::ros2::bridge_client::{ACTION_OPS_UNSUPPORTED, Ros2BridgeClient, goal_status_name};
use crate::ros2::safety::{EffectiveCaps, HARD_MAX_GOAL_WAIT_SECS, apply_field_limits};
use crate::ros2::session::{
    GOAL_NOT_CANCELABLE_NOTE, GoalCancelOutcome, Ros2Session, cancel_all_goals_fallback,
    classify_cancel_all_response,
};
use crate::ros2::{canonical_type, find_allow_entry, not_permitted};

/// `action_msgs/srv/CancelGoal` — the ROS 2 cancel-all-goals-on-this-server
/// request type. Used by [`cancel_goal`]'s closed-connection fallback and
/// [`crate::ros2::session::cancel_all_goals_fallback`] (shared by
/// `Ros2Session::stop_all`).
pub const CANCEL_GOAL_SERVICE_TYPE: &str = "action_msgs/srv/CancelGoal";

/// How long [`send_goal`] waits, right after sending, for an immediate
/// rosbridge rejection ([`ACTION_OPS_UNSUPPORTED`]) before returning its
/// normal "goal sent" text — long enough to catch a `status` error frame,
/// short enough to never make a successful send feel slow.
pub const ACTION_REJECT_WAIT_MS: u64 = 500;

/// How long [`cancel_goal`] waits for a terminal status after sending the
/// cancel, before rendering whatever status it has.
pub const CANCEL_WAIT_SECS: u64 = 2;

/// D-06/D-04/D-08: send a declared ROS 2 action goal. `action_name` must be
/// declared under `ros2.actions` with a matching type — checked via
/// [`find_allow_entry`] BEFORE `session.client_for(url)` ever opens a
/// connection. `goal` (a JSON object, or a string that parses to one) is
/// clamped through the declared entry's `limits` (clamp-then-send, never
/// reject). On success the goal is tracked (for `stop_all`/`cancel_goal`) and
/// given a `caps.max_goal_secs` watchdog before this returns.
pub async fn send_goal(
    session: &Arc<Ros2Session>,
    url: &str,
    cfg: &Config,
    args: &Value,
    caps: &EffectiveCaps,
) -> String {
    let Some(action_name) = args["action_name"].as_str().filter(|s| !s.is_empty()) else {
        return "Error: 'action_name' parameter is required for the send_goal action".to_string();
    };

    // D-04: allowlist-before-I/O — this lookup happens before any connection
    // is opened (`session.client_for` below is the first thing that can open
    // a socket).
    let entry = match find_allow_entry(&cfg.ros2.actions, action_name) {
        Some(e) => e.clone(),
        None => return not_permitted("actions", action_name),
    };
    if let Some(supplied_type) = args["type"].as_str()
        && canonical_type(supplied_type) != canonical_type(&entry.msg_type)
    {
        return not_permitted("actions", action_name);
    }

    let mut goal = match &args["goal"] {
        Value::Object(_) => args["goal"].clone(),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v) => v,
            Err(_) => {
                return format!("Error: 'goal' string for '{action_name}' did not parse as JSON");
            }
        },
        Value::Null => json!({}),
        _ => {
            return "Error: 'goal' must be a JSON object (or a string that parses to one)"
                .to_string();
        }
    };

    let notes = apply_field_limits(&mut goal, &entry.limits);

    let client = match session.client_for(url).await {
        Ok(c) => c,
        Err(e) => return format!("Error: cannot connect to rosbridge at {url} — {e}"),
    };

    tracing::info!(
        action = action_name,
        action_type = %entry.msg_type,
        clamp_notes = notes.len(),
        "ros2: send_goal"
    );
    let goal_id = match client
        .send_action_goal(action_name, &entry.msg_type, goal)
        .await
    {
        Ok(id) => id,
        Err(e) => return format!("Error: send_goal to '{action_name}' failed — {e}"),
    };

    session
        .track_goal(
            &goal_id,
            action_name,
            &entry.msg_type,
            client.connection_id(),
        )
        .await;

    // An unsupported rosbridge (< 2.0.0) rejects the op immediately with a
    // `status` error frame naming it unknown/unsupported — wait a short
    // window to catch that before spawning the watchdog and returning the
    // normal "goal sent" text (D-06). Tracking happens first so a stop racing
    // this window still covers the goal; a rejected goal never existed on the
    // robot, so it is untracked here rather than left for stop_all forever.
    if let Some(snap) = client
        .wait_goal_result(&goal_id, Duration::from_millis(ACTION_REJECT_WAIT_MS))
        .await
        && let Some(err) = &snap.error
        && err == ACTION_OPS_UNSUPPORTED
    {
        session.untrack_goal(&goal_id).await;
        return err.clone();
    }

    // D-08: no goal is left running unbounded — after the effective
    // max_goal_secs, cancel it ourselves if it has no terminal status yet.
    let max_secs = caps.max_goal_secs;
    let watchdog_client = client.clone();
    let watchdog_action = action_name.to_string();
    let watchdog_goal_id = goal_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs_f64(max_secs)).await;
        let unfinished = watchdog_client
            .goal_snapshot(&watchdog_goal_id)
            .map(|s| s.result.is_none())
            .unwrap_or(false);
        if unfinished {
            tracing::warn!(
                action = %watchdog_action,
                goal_id = %watchdog_goal_id,
                max_secs,
                "ros2: goal watchdog cancelling unfinished goal"
            );
            let _ = watchdog_client
                .cancel_action_goal(&watchdog_action, &watchdog_goal_id)
                .await;
        }
    });

    let mut out = format!(
        "Goal {goal_id} sent to {action_name} [{}]. Poll with goal_status (wait_secs up to 20) \
         or stop it with cancel_goal. It is auto-cancelled after {max_secs}s.",
        entry.msg_type
    );
    for note in &notes {
        out.push_str(&format!("\nClamped: {note}"));
    }
    out
}

/// D-06: poll a goal's status/feedback/result. `goal_id` (required) must be
/// known to `client` — an unknown id renders `Error: unknown goal id {id}`.
/// `wait_secs` (clamped to `[0, HARD_MAX_GOAL_WAIT_SECS]`, default 0)
/// optionally waits for a terminal status before rendering; `0` renders an
/// immediate snapshot.
pub async fn goal_status(client: &Ros2BridgeClient, args: &Value) -> String {
    let Some(goal_id) = args["goal_id"].as_str().filter(|s| !s.is_empty()) else {
        return "Error: 'goal_id' parameter is required for the goal_status action".to_string();
    };
    if client.goal_snapshot(goal_id).is_none() {
        return format!("Error: unknown goal id {goal_id}");
    }

    let wait_secs = args["wait_secs"]
        .as_f64()
        .unwrap_or(0.0)
        .clamp(0.0, HARD_MAX_GOAL_WAIT_SECS);

    let snap = if wait_secs > 0.0 {
        client
            .wait_goal_result(goal_id, Duration::from_secs_f64(wait_secs))
            .await
    } else {
        client.goal_snapshot(goal_id)
    };
    let Some(snap) = snap else {
        return format!("Error: unknown goal id {goal_id}");
    };

    let status_name = snap.status.map(goal_status_name).unwrap_or_else(|| {
        if snap.error.is_some() {
            "UNKNOWN"
        } else {
            goal_status_name(0)
        }
    });

    let mut out = format!(
        "Goal {goal_id} on {}: status={status_name}, succeeded={}",
        snap.action,
        snap.succeeded
            .map(|b| b.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );
    if let Some(fb) = &snap.last_feedback {
        out.push_str(&format!(
            "\nLast feedback: {}",
            serde_json::to_string(&crate::ros2::summarize::truncate_value(fb)).unwrap_or_default()
        ));
    }
    if let Some(result) = &snap.result {
        out.push_str(&format!(
            "\nResult: {}",
            serde_json::to_string(&crate::ros2::summarize::truncate_value(result))
                .unwrap_or_default()
        ));
    }
    if let Some(err) = &snap.error {
        // Never interpolate `err`'s text (T-49.8-63) — it is set verbatim
        // from rosbridge's peer-controlled `status` error `msg`, matching
        // the hardening `cancel_goal` (below, same file) already applies.
        // `ACTION_OPS_UNSUPPORTED` is our own fixed constant, not peer
        // text, so it is safe to render as-is.
        if err == ACTION_OPS_UNSUPPORTED {
            out.push_str(&format!("\nError: {ACTION_OPS_UNSUPPORTED}"));
        } else {
            out.push_str(
                "\nError: the bridge reported an error for this goal (see the connection logs)",
            );
        }
    }
    out
}

/// D-06/D-10 (49.8-18, review 49.8-18, CR-01's `cancel_goal` half): cancel
/// `goal_id` — on its own owning connection (`cancel_action_goal`) when
/// `client` still is that connection, otherwise
/// [`cancel_all_goals_fallback`] on `client` (already connected to the right
/// bridge URL, just not the goal's original connection). Renders ONLY what
/// the robot actually confirmed: CANCELED is reported only on the goal's own
/// status 5; a refusal, an ignored/unanswered cancel, a present-but-non-
/// terminal/missing status, or a merely-accepted (CANCELING, not terminal)
/// cancel-all are all explicit, non-CANCELED outcomes that say the goal may
/// still be running. Waits up to [`CANCEL_WAIT_SECS`] for the goal's own
/// terminal status before rendering.
pub async fn cancel_goal(
    session: &Arc<Ros2Session>,
    client: &Ros2BridgeClient,
    args: &Value,
) -> String {
    let Some(goal_id) = args["goal_id"].as_str().filter(|s| !s.is_empty()) else {
        return "Error: 'goal_id' parameter is required for the cancel_goal action".to_string();
    };

    let Some(tracked) = session.tracked_goal(goal_id).await else {
        return format!("Error: unknown goal id {goal_id}");
    };

    if tracked.connection_id == client.connection_id() {
        if let Err(e) = client.cancel_action_goal(&tracked.action, goal_id).await {
            return format!("Error: cancel_goal for '{goal_id}' failed — {e}");
        }

        let Some(snap) = client
            .wait_goal_result(goal_id, Duration::from_secs(CANCEL_WAIT_SECS))
            .await
        else {
            return format!(
                "Error: cancel_goal for '{goal_id}' was not confirmed — the goal is unknown on this connection"
            );
        };

        // Check `result` before `error` — bridge_client.rs's `action_result`
        // arm stores `result = Some(values)` unconditionally, so "a result
        // is present" only means a frame arrived; the status inside it
        // decides confirmation.
        if snap.result.is_some() {
            return match snap.status {
                // action_msgs/msg/GoalStatus: 5 == CANCELED. Written as a
                // guard (not `Some(5) => ..`) so `goal_status_name` is
                // always called on the OBSERVED status, never a hardcoded
                // literal — CANCELED is rendered only because it was
                // actually observed.
                Some(status) if status == 5 => format!(
                    "Goal {goal_id} on {}: cancel requested — status={}",
                    tracked.action,
                    goal_status_name(status)
                ),
                Some(status @ (4 | 6)) => format!(
                    "Goal {goal_id} on {}: finished before the cancel landed — status={}",
                    tracked.action,
                    goal_status_name(status)
                ),
                other => {
                    let name = other.map(goal_status_name).unwrap_or("UNKNOWN");
                    format!(
                        "Error: cancel_goal for '{goal_id}' was not confirmed — the goal reported a missing or non-terminal status ({name}); the goal may still be running — poll goal_status or retry cancel_goal"
                    )
                }
            };
        }
        if let Some(err) = &snap.error {
            // Never interpolate `err`'s text (T-49.8-63) — it is set from
            // rosbridge's peer-controlled `status` error `msg`.
            return if err == ACTION_OPS_UNSUPPORTED {
                ACTION_OPS_UNSUPPORTED.to_string()
            } else {
                format!(
                    "Error: cancel_goal for '{goal_id}' was not confirmed — the bridge reported an error for this goal (see goal_status)"
                )
            };
        }
        let name = snap.status.map(goal_status_name).unwrap_or("UNKNOWN");
        return format!(
            "Goal {goal_id} on {}: cancel requested but not confirmed within {CANCEL_WAIT_SECS}s — status={name}; the goal may still be running — poll goal_status or retry cancel_goal",
            tracked.action
        );
    }

    // Fallback path: the goal's owning connection has closed. The SAME
    // classify_cancel_all_response the stop path uses decides the outcome —
    // the two paths can never disagree about what counts as a refusal.
    match cancel_all_goals_fallback(client, &tracked.action).await {
        Err(_) => format!(
            "Error: cancel_goal for '{goal_id}' was not confirmed — the cancel-all service call failed"
        ),
        Ok(resp) => match classify_cancel_all_response(&resp) {
            GoalCancelOutcome::Accepted => format!(
                "Goal {goal_id} on {}: cancel-all accepted by the action server — its goals are CANCELING; the final state is not observable because this goal's connection has closed. Retry cancel_goal to confirm.",
                tracked.action
            ),
            GoalCancelOutcome::NotCancelable => format!(
                "Goal {goal_id} on {}: {GOAL_NOT_CANCELABLE_NOTE} — it already finished, or the server declined the cancel",
                tracked.action
            ),
            GoalCancelOutcome::Refused(r) => {
                format!("Error: cancel_goal for '{goal_id}' was refused — {r}")
            }
            GoalCancelOutcome::Unconfirmed(r) => {
                format!("Error: cancel_goal for '{goal_id}' was not confirmed — {r}")
            }
            // classify_cancel_all_response never produces these three —
            // fail closed on an impossible outcome rather than silently
            // rendering CANCELED without evidence.
            GoalCancelOutcome::TerminalConfirmed
            | GoalCancelOutcome::FinishedFirst
            | GoalCancelOutcome::ActionOpsUnsupported => format!(
                "Error: cancel_goal for '{goal_id}' was not confirmed — unexpected terminal outcome from a cancel-all classification"
            ),
        },
    }
}
