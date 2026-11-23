//! `GatePolicy`: the DEFCON level and the sidecar service a gated call is judged under
//! (Phase 57, D-08 / D-09 / D-21).
//!
//! `execute_gated_command` used to take a `yolo: bool` captured when the surface was
//! built. A captured bool cannot follow a live level change, so the chokepoint now takes
//! a [`GatePolicy`]: a shared [`DefconHandle`] it reads at the moment of the decision,
//! plus the process-global [`SecurityService`] (absent in legacy/test paths).
//!
//! Production callers build one with [`GatePolicy::for_session`]. A `bool` still
//! converts, through the D-08 alias, so unmigrated callers and tests keep compiling:
//! `true` is level 5 and `false` is level 3, both with no sidecar.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ironhermes_core::config::DefconLevel;
use ironhermes_core::defcon::{DefconHandle, session_defcon};
use ironhermes_security::service::{SecurityService, installed};

/// Logs the "sidecar was never booted" warning at most once per process.
static UNBOOTED_WARNED: AtomicBool = AtomicBool::new(false);

/// The level and the sidecar a gated call is decided under.
#[derive(Clone)]
pub struct GatePolicy {
    /// Live per-session DEFCON level (D-09). Read at the decision, never cached.
    pub level: DefconHandle,
    /// The process-global sidecar service. `None` skips the sidecar step entirely:
    /// the legacy bool path, tests, and a process where nothing called `boot()`.
    pub security: Option<Arc<SecurityService>>,
}

impl GatePolicy {
    /// The policy a production surface uses for `session_id`: the session's live
    /// level handle and whatever service `boot()` installed.
    ///
    /// When no service is installed the call proceeds without the sidecar step, and ONE
    /// process-wide warning says so: that is a host that never booted the sidecar, and
    /// the operator should hear about it once, not per call.
    pub fn for_session(session_id: &str) -> Self {
        let security = installed();
        if security.is_none() && !UNBOOTED_WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                target: "ironhermes::security::gate_policy",
                "the security sidecar service was never booted in this process; \
                 terminal/execute_code are screened by the built-in floor only"
            );
        }
        Self {
            level: session_defcon().handle_for(session_id),
            security,
        }
    }

    /// A policy pinned at `level`. For tests: nothing can change the level afterwards
    /// except through the returned handle.
    pub fn fixed(level: DefconLevel, security: Option<Arc<SecurityService>>) -> Self {
        Self {
            level: DefconHandle::fixed(level),
            security,
        }
    }
}

/// The D-08 alias for unmigrated callers and tests: `true` is level 5, `false` is level
/// 3, neither with a sidecar. Production callers use [`GatePolicy::for_session`]; the
/// workspace sweep (plan 16) fails if one still passes a bool.
impl From<bool> for GatePolicy {
    fn from(yolo: bool) -> Self {
        let level = if yolo {
            DefconLevel::FIVE
        } else {
            DefconLevel::THREE
        };
        Self::fixed(level, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_alias_maps_true_to_five_and_false_to_three_without_a_sidecar() {
        let on = GatePolicy::from(true);
        let off = GatePolicy::from(false);
        assert_eq!(on.level.get(), DefconLevel::FIVE);
        assert_eq!(off.level.get(), DefconLevel::THREE);
        assert!(on.security.is_none() && off.security.is_none());
        assert!(on.level.get().yolo_bypass_active());
        assert!(!off.level.get().yolo_bypass_active());
    }

    #[test]
    fn for_session_level_is_live() {
        let policy = GatePolicy::for_session("policy-gate-live-test");
        let before = policy.level.get();
        session_defcon().set_session_level("policy-gate-live-test", DefconLevel::ONE);
        assert_eq!(policy.level.get(), DefconLevel::ONE);
        session_defcon().set_session_level("policy-gate-live-test", before);
        session_defcon().forget("policy-gate-live-test");
    }
}
