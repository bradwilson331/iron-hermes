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

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level kanban configuration block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanConfig {
    /// Run the dispatcher tokio task inside the gateway runtime (D-09).
    /// `HERMES_KANBAN_DISPATCH_IN_GATEWAY=0` env override at startup turns
    /// this off without a config edit.
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
            auto_decompose: false,
            auto_decompose_per_tick: default_auto_decompose_per_tick(),
            orchestrator_profile: String::new(),
            default_assignee: String::new(),
            decomposer_model: String::new(),
            auto_promote_children: default_auto_promote_children(),
        }
    }
}

// ---------------------------------------------------------------------------
// Default helpers (named so serde can reach them from `#[serde(default = …)]`)
// ---------------------------------------------------------------------------

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
        assert!(!cfg.auto_decompose);
        assert_eq!(cfg.auto_decompose_per_tick, 3);
        assert!(cfg.orchestrator_profile.is_empty());
        assert!(cfg.default_assignee.is_empty());
        assert!(cfg.decomposer_model.is_empty());
        assert!(cfg.auto_promote_children);
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
