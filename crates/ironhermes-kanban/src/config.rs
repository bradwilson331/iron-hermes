//! `kanban:` subtree of `~/.ironhermes/config.yaml`.
//!
//! Defaults from CONTEXT.md D-09/D-11/D-12/D-14:
//!
//! | Field | Default | Decision |
//! |-------|---------|----------|
//! | `dispatch_in_gateway` | `true` | D-09 (gateway-embedded by default) |
//! | `dispatch_interval_seconds` | `60` | D-09 |
//! | `max_in_progress` | `Some(8)` | D-11 (CONTEXT.md value wins over reference's "unset") |
//! | `failure_limit` | `2` | D-12 (circuit breaker) |
//! | `stranded_threshold_seconds` | `1800` | D-14 (30 min) |
//! | `dispatch_stale_timeout_seconds` | `14400` | reference.md (4 h) |
//! | `default_workdir` | `None` | D-32 (no board-level default) |
//! | `notification_sources` | `None` | D-37 (RESERVED for deferred notifier phase) |
//! | `notifier_poll_seconds` | `3` | Phase 36.3.7.5 BUG-36.3.7.5-03 |
//! | `default_notify` | `None` | Phase 46.5-04 D-06 (config-driven notify target for non-chat-origin task creation) |

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::Result;
use crate::store::KanbanStore;

/// Top-level kanban configuration block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanConfig {
    /// Run the dispatcher tokio task inside the gateway runtime (D-09).
    /// `IRONHERMES_KANBAN_DISPATCH_IN_GATEWAY=0` env override at startup turns
    /// this off without a config edit (legacy `HERMES_KANBAN_DISPATCH_IN_GATEWAY=0`
    /// also accepted during the deprecation window).
    #[serde(default = "default_dispatch_in_gateway")]
    pub dispatch_in_gateway: bool,

    /// Tick period for the dispatcher loop (D-09).
    #[serde(default = "default_dispatch_interval_seconds")]
    pub dispatch_interval_seconds: u64,

    /// Concurrency cap for `running` tasks (D-11). `None` (or `Some(0)`) =
    /// unlimited. CONTEXT.md locks the default to `8`.
    #[serde(default = "default_max_in_progress")]
    pub max_in_progress: Option<usize>,

    /// Consecutive non-successful attempts before the circuit breaker
    /// auto-blocks the task with a `gave_up` event (D-12).
    #[serde(default = "default_failure_limit")]
    pub failure_limit: u32,

    /// Stranded-task diagnostic threshold (D-14, default 30 min).
    #[serde(default = "default_stranded_threshold_seconds")]
    pub stranded_threshold_seconds: u64,

    /// Dispatcher absence-detection timeout (reference.md, default 4 h).
    /// A `running` task with no heartbeat for this long is reset to `ready`
    /// and a `stale` event is appended.
    #[serde(default = "default_dispatch_stale_timeout_seconds")]
    pub dispatch_stale_timeout_seconds: u64,

    /// Board-level default workdir applied when neither `--workspace` nor
    /// the task overrides (D-32). Per-task `workspace:` still wins.
    #[serde(default)]
    pub default_workdir: Option<PathBuf>,

    /// **RESERVED — not consumed in v1** (D-37).
    ///
    /// Controls cross-profile gateway subscription allowance when the
    /// deferred notifier phase ships. Present so the deferred phase needs
    /// no config migration; the v1 dispatcher does not read this field.
    #[serde(default)]
    pub notification_sources: Option<Vec<String>>,

    /// Tick period for the gateway notifier loop (Phase 36.3.7.5 BUG-36.3.7.5-03).
    /// Default 3 seconds — small enough that operators don't feel the lag, large enough
    /// to keep the polling cost trivial vs. dispatcher tick (60s).
    #[serde(default = "default_notifier_poll_seconds")]
    pub notifier_poll_seconds: u64,

    /// Config-driven default notify target for block/give-up notifications
    /// on tasks created via a non-chat-origin path (CLI `hermes kanban
    /// create` / the `kanban_create` LLM tool) — Phase 46.5-04 D-06.
    ///
    /// `None` (the default) means only the existing chat-origin auto-
    /// subscribe hook (`/kanban create` from a chat platform) and explicit
    /// `notify-subscribe` calls apply — CLI/tool-created tasks get no
    /// subscribers and the notifier silently skips them, exactly as today.
    ///
    /// SECURITY (T-46.5-20): this target is read EXCLUSIVELY from operator
    /// config. It MUST NEVER be derived from task-controlled data (task
    /// fields, attachment filename/metadata) — otherwise a task could
    /// redirect its own block notification (which may carry a stderr
    /// diagnostic, see D-04) to an attacker-chosen chat. See
    /// `subscribe_default_notify` below, the sole writer of this
    /// relationship.
    #[serde(default)]
    pub default_notify: Option<DefaultNotifyTarget>,

    /// Auto-run decomposer on triage tasks every dispatcher tick (Phase 36.3.7.10).
    /// Default `false` for v1 (opt-in) — reference.md §444.
    #[serde(default)]
    pub auto_decompose: bool,

    /// Cap on LLM decompositions per dispatcher tick (Phase 36.3.7.10).
    /// Default 3 — reference.md §444.
    #[serde(default = "default_auto_decompose_per_tick")]
    pub auto_decompose_per_tick: u32,

    /// Profile that owns decomposition decisions (Phase 36.3.7.10).
    /// Empty = fall back to active default profile — reference.md §444.
    #[serde(default)]
    pub orchestrator_profile: String,

    /// Where child tasks land when the LLM picks an unknown profile (Phase 36.3.7.10).
    /// Empty = active default — reference.md §444.
    #[serde(default)]
    pub default_assignee: String,

    /// Model identifier for the decomposer LLM call (Phase 36.3.7.10).
    /// Empty = use auxiliary.kanban_decomposer then fall back to main provider — reference.md §449-451.
    #[serde(default)]
    pub decomposer_model: String,

    /// Auto-promote decomposed children to `ready` when they have no parent blockers (Phase 36.3.7.10).
    /// Default `true` per reference.md §625.
    #[serde(default = "default_auto_promote_children")]
    pub auto_promote_children: bool,

    /// Aux model id for the goal-mode judge LLM (Phase 36.3.7.12 D-05).
    /// Empty = resolve through the `auxiliary.kanban_judge` role then fall
    /// back to the main provider. Mirrors `decomposer_model` resolution.
    #[serde(default)]
    pub judge_model: String,

    /// How long the `blocker_auth` respawn guard holds a task after its last
    /// closed run failed with an auth/quota/rate-limit error (Phase 47.4 UAT
    /// follow-up).
    ///
    /// Before this was configurable it was **unbounded**: the guard returned
    /// on any auth-ish error text in the newest closed run with no time check,
    /// so a task whose profile was later repaired could never dispatch again —
    /// the operator's only recourse was to recreate the task. Bounding it lets
    /// a fixed profile recover on its own after a cool-off.
    ///
    /// `0` restores the old unbounded behaviour (guard forever).
    #[serde(default = "default_respawn_auth_backoff_seconds")]
    pub respawn_auth_backoff_seconds: u64,

    /// How long the `recent_success` respawn guard suppresses a re-run after a
    /// successful completion. Previously hardcoded to 3600.
    #[serde(default = "default_respawn_recent_success_seconds")]
    pub respawn_recent_success_seconds: u64,

    /// Look-back window for the `active_pr` respawn guard. Previously
    /// hardcoded to 7 days.
    #[serde(default = "default_respawn_active_pr_seconds")]
    pub respawn_active_pr_seconds: u64,

    /// Inner-loop iteration cap for goal-mode workers (Phase 36.3.7.13 F-03).
    ///
    /// The worker's `config.agent.max_iterations` is clamped to
    /// `min(original, goal_inner_max_iterations)` before the in-session
    /// loop starts. Prevents run-away inner loops when the per-turn LLM
    /// calls are themselves expensive.
    ///
    /// Default 10 — conservative cap that is well below the typical
    /// `max_iterations` default (50) while still allowing meaningful
    /// per-turn work. Operators can raise it per-board in config.yaml.
    #[serde(default = "default_goal_inner_max_iterations")]
    pub goal_inner_max_iterations: u32,
}

impl Default for KanbanConfig {
    fn default() -> Self {
        Self {
            dispatch_in_gateway: default_dispatch_in_gateway(),
            dispatch_interval_seconds: default_dispatch_interval_seconds(),
            max_in_progress: default_max_in_progress(),
            failure_limit: default_failure_limit(),
            stranded_threshold_seconds: default_stranded_threshold_seconds(),
            dispatch_stale_timeout_seconds: default_dispatch_stale_timeout_seconds(),
            default_workdir: None,
            notification_sources: None,
            notifier_poll_seconds: default_notifier_poll_seconds(),
            default_notify: None,
            auto_decompose: false,
            auto_decompose_per_tick: default_auto_decompose_per_tick(),
            orchestrator_profile: String::new(),
            default_assignee: String::new(),
            decomposer_model: String::new(),
            auto_promote_children: default_auto_promote_children(),
            judge_model: String::new(),
            goal_inner_max_iterations: default_goal_inner_max_iterations(),
            respawn_auth_backoff_seconds: default_respawn_auth_backoff_seconds(),
            respawn_recent_success_seconds: default_respawn_recent_success_seconds(),
            respawn_active_pr_seconds: default_respawn_active_pr_seconds(),
        }
    }
}

// ---------------------------------------------------------------------------
// default_notify (Phase 46.5-04 D-06)
// ---------------------------------------------------------------------------

/// Operator-configured notify target for block/give-up events on
/// non-chat-origin tasks (D-06). Deserialized from `kanban.default_notify`
/// in `config.yaml`:
///
/// ```yaml
/// kanban:
///   default_notify:
///     platform: telegram
///     chat_id: "123456789"
///     thread_id: "7"   # optional
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultNotifyTarget {
    /// Platform name (e.g. `"telegram"`) — passed verbatim to
    /// `append_subscription` / the notifier's `send_fn` platform arg.
    pub platform: String,
    /// Platform-specific chat identifier.
    pub chat_id: String,
    /// Optional platform-specific thread identifier (e.g. Telegram
    /// super-group topics). `None` is stored as `""` by `append_subscription`.
    #[serde(default)]
    pub thread_id: Option<String>,
}

/// Auto-subscribe `task_id` to the operator's configured `default_notify`
/// target with `source="auto"` (D-06), mirroring the existing chat-origin
/// auto-subscribe hook (`source="auto"`, written by `/kanban create`) for
/// tasks created via a non-chat-origin path (CLI `hermes kanban create` /
/// the `kanban_create` LLM tool).
///
/// Strictly additive: does not remove or alter any existing subscription
/// row. A task may end up with multiple subscription rows (chat-origin +
/// default-notify + explicit); the notifier fans out to each independently
/// (`list_subscriptions_for_task` returns all of them) — no conflict.
///
/// # Security (T-46.5-20)
///
/// `target` MUST be sourced exclusively from `KanbanConfig.default_notify`
/// (operator config) by the caller. This function does not — and must
/// never — read `platform`/`chat_id`/`thread_id` from any task field,
/// attachment filename, or other task-controlled data: doing so would let
/// a task redirect its own block notification (which may carry a stderr
/// diagnostic per D-04) to an attacker-chosen chat.
pub fn subscribe_default_notify(
    store: &mut KanbanStore,
    target: &DefaultNotifyTarget,
    task_id: &str,
) -> Result<()> {
    store.append_subscription(
        task_id,
        &target.platform,
        &target.chat_id,
        target.thread_id.as_deref(),
        "auto",
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Default helpers (named so serde can reach them from `#[serde(default = …)]`)
// ---------------------------------------------------------------------------

/// 1 hour. Long enough that a genuine 429/quota block is not hammered, short
/// enough that a repaired profile recovers without operator surgery.
fn default_respawn_auth_backoff_seconds() -> u64 {
    3600
}

/// 1 hour — preserves the previously hardcoded `recent_success` window.
fn default_respawn_recent_success_seconds() -> u64 {
    3600
}

/// 7 days — preserves the previously hardcoded `active_pr` look-back.
fn default_respawn_active_pr_seconds() -> u64 {
    7 * 24 * 3600
}

fn default_dispatch_in_gateway() -> bool {
    true
}

fn default_dispatch_interval_seconds() -> u64 {
    60
}

fn default_max_in_progress() -> Option<usize> {
    Some(8)
}

fn default_failure_limit() -> u32 {
    2
}

fn default_stranded_threshold_seconds() -> u64 {
    1800
}

fn default_dispatch_stale_timeout_seconds() -> u64 {
    14400
}

fn default_notifier_poll_seconds() -> u64 {
    3
}

fn default_auto_decompose_per_tick() -> u32 {
    3
}

fn default_auto_promote_children() -> bool {
    true
}

fn default_goal_inner_max_iterations() -> u32 {
    10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_context_md() {
        let cfg = KanbanConfig::default();
        assert!(cfg.dispatch_in_gateway);
        assert_eq!(cfg.dispatch_interval_seconds, 60);
        assert_eq!(cfg.max_in_progress, Some(8));
        assert_eq!(cfg.failure_limit, 2);
        assert_eq!(cfg.stranded_threshold_seconds, 1800);
        assert_eq!(cfg.dispatch_stale_timeout_seconds, 14400);
        assert!(cfg.default_workdir.is_none());
        assert!(cfg.notification_sources.is_none());
        assert_eq!(cfg.notifier_poll_seconds, 3);
        // Phase 46.5-04 D-06: default_notify defaults to None (no auto-
        // subscribe beyond the existing chat-origin hook).
        assert!(cfg.default_notify.is_none());
        assert!(!cfg.auto_decompose);
        assert_eq!(cfg.auto_decompose_per_tick, 3);
        assert!(cfg.orchestrator_profile.is_empty());
        assert!(cfg.default_assignee.is_empty());
        assert!(cfg.decomposer_model.is_empty());
        assert!(cfg.auto_promote_children);
        // Phase 36.3.7.12 D-05: judge_model default is empty (resolves via
        // auxiliary.kanban_judge then main provider).
        assert!(cfg.judge_model.is_empty());
        // Phase 36.3.7.13 F-03: inner-loop cap defaults to 10.
        assert_eq!(cfg.goal_inner_max_iterations, 10);
    }

    #[test]
    fn deserializes_partial_yaml() {
        // Missing keys fall back to defaults.
        let yaml = "dispatch_in_gateway: false\nfailure_limit: 5\n";
        let cfg: KanbanConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.dispatch_in_gateway);
        assert_eq!(cfg.failure_limit, 5);
        // Untouched fields still hold their default.
        assert_eq!(cfg.max_in_progress, Some(8));
        assert_eq!(cfg.stranded_threshold_seconds, 1800);
    }
}
