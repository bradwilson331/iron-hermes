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
