//! Process-wide, per-session DEFCON level registry (Phase 57 Plan 02, D-09).
//!
//! Every surface (TUI, gateway chat platforms, web UI, subagents) reads the
//! session's strictness level through a live [`DefconHandle`] — a shared atomic
//! read at call time at every gate decision (RESEARCH Pattern 3). Never capture
//! a level (or a derived bool) at construction: a `/yolo <N>` change would then
//! be invisible to the holder.
//!
//! The registry is in-memory only. A session level is lost on restart by design
//! and is never written back to `config.yaml` (D-09); this module performs no
//! file I/O and has no `Config` write path. (The one write it makes is the
//! audit row for a level change, which goes through
//! [`crate::audit::AuditLog`], never to `config.yaml`.)

use crate::config::DefconLevel;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, RwLock};

/// A live, shared view of one session's DEFCON level.
///
/// Cloning shares the same underlying atomic, so a [`DefconHandle::set`]
/// through any clone is observed by every other holder at its next `get`.
#[derive(Clone, Debug)]
pub struct DefconHandle(Arc<AtomicU8>);

impl DefconHandle {
    /// An unregistered handle pinned to `level` (used by tests and the legacy
    /// bool conversion). It is still settable, but belongs to no session.
    pub fn fixed(level: DefconLevel) -> Self {
        Self(Arc::new(AtomicU8::new(level.level())))
    }

    /// The current level. Falls back to level 3 if the stored byte is somehow
    /// out of range (it cannot be, through this API).
    pub fn get(&self) -> DefconLevel {
        DefconLevel::try_from(self.0.load(Ordering::SeqCst)).unwrap_or(DefconLevel::THREE)
    }

    /// Set the level, returning the previous one.
    pub fn set(&self, level: DefconLevel) -> DefconLevel {
        let old = self.0.swap(level.level(), Ordering::SeqCst);
        DefconLevel::try_from(old).unwrap_or(DefconLevel::THREE)
    }

    /// True when both handles share one underlying atomic.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Session id -> live level handle, plus the process default that seeds each
/// NEW session.
pub struct SessionDefconRegistry {
    default: AtomicU8,
    sessions: RwLock<HashMap<String, DefconHandle>>,
}

impl SessionDefconRegistry {
    pub fn new(default: DefconLevel) -> Self {
        Self {
            default: AtomicU8::new(default.level()),
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// The live handle for `session_id`, created on first lookup and seeded
    /// from the CURRENT process default. A poisoned lock is recovered, never a
    /// panic.
    pub fn handle_for(&self, session_id: &str) -> DefconHandle {
        {
            let map = self.sessions.read().unwrap_or_else(|e| e.into_inner());
            if let Some(h) = map.get(session_id) {
                return h.clone();
            }
        }
        let mut map = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        // Re-check under the write lock: another thread may have created it.
        map.entry(session_id.to_string())
            .or_insert_with(|| DefconHandle::fixed(self.process_default()))
            .clone()
    }

    /// Set one session's level, returning its previous level. Other sessions
    /// are unaffected.
    pub fn set_session_level(&self, session_id: &str, level: DefconLevel) -> DefconLevel {
        self.handle_for(session_id).set(level)
    }

    /// Change the level that seeds sessions created from now on. Existing
    /// sessions keep their level.
    pub fn set_process_default(&self, level: DefconLevel) {
        self.default.store(level.level(), Ordering::SeqCst);
    }

    pub fn process_default(&self) -> DefconLevel {
        DefconLevel::try_from(self.default.load(Ordering::SeqCst)).unwrap_or(DefconLevel::THREE)
    }

    /// Drop a session's handle (session ended). Holders of existing clones keep
    /// a working, now-orphaned handle.
    pub fn forget(&self, session_id: &str) {
        let mut map = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        map.remove(session_id);
    }
}

/// `AuditEntry::kind` of every DEFCON level change or refusal (D-10).
pub const DEFCON_CHANGE_KIND: &str = "defcon_change";

/// Append one `defcon_change` audit row (D-10), synchronously.
///
/// `old` / `new` are `None` when the level is unknown (not used by `/yolo`,
/// which always knows both); they serialize as JSON `null`. `decision` is
/// `"changed"` for an applied change and `"refused"` for a rejected attempt;
/// a change records resolution `"operator"`, a refusal `"policy"`.
///
/// The caller must treat an `Err` as a failed audit write and surface it; it
/// must never skip the row.
#[allow(clippy::too_many_arguments)]
pub fn record_defcon_change(
    audit: &crate::audit::AuditLog,
    session_id: &str,
    surface: &str,
    chat_id: &str,
    actor: &str,
    old: Option<DefconLevel>,
    new: Option<DefconLevel>,
    decision: &str,
    reason: &str,
) -> std::io::Result<()> {
    let resolution = if decision == "changed" {
        "operator"
    } else {
        "policy"
    };
    let args = serde_json::json!({
        "old": old.map(DefconLevel::level),
        "new": new.map(DefconLevel::level),
        "actor": actor,
    });
    let entry = audit.make_entry(
        DEFCON_CHANGE_KIND,
        "yolo",
        None,
        session_id,
        surface,
        chat_id,
        reason,
        decision,
        resolution,
        &args,
    );
    audit.append_sync(&entry)
}

static SESSION_DEFCON: LazyLock<SessionDefconRegistry> =
    LazyLock::new(|| SessionDefconRegistry::new(DefconLevel::THREE));

/// The process-wide registry. Seeded with level 3 (D-21): a host that never
/// calls [`SessionDefconRegistry::set_process_default`] gets today's "needs
/// approval" behaviour, never a bypass level.
pub fn session_defcon() -> &'static SessionDefconRegistry {
    &SESSION_DEFCON
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lvl(n: u8) -> DefconLevel {
        DefconLevel::try_from(n).unwrap()
    }

    #[test]
    fn session_handle_is_live() {
        let reg = SessionDefconRegistry::new(lvl(3));
        let h1 = reg.handle_for("s1");
        let h1b = reg.handle_for("s1");
        assert!(h1.ptr_eq(&h1b), "same session id must share one handle");
        let old = reg.set_session_level("s1", lvl(1));
        assert_eq!(old.level(), 3);
        assert_eq!(h1.get().level(), 1);
        assert_eq!(h1b.get().level(), 1);
        assert_eq!(reg.handle_for("s2").get().level(), 3);
    }

    #[test]
    fn new_sessions_take_current_default() {
        let reg = SessionDefconRegistry::new(lvl(3));
        let s2 = reg.handle_for("s2");
        reg.set_process_default(lvl(5));
        assert_eq!(reg.handle_for("s3").get().level(), 5);
        assert_eq!(s2.get().level(), 3);
    }

    #[test]
    fn forget_orphans_the_handle_and_next_lookup_is_fresh() {
        let reg = SessionDefconRegistry::new(lvl(3));
        let h = reg.handle_for("s1");
        reg.set_session_level("s1", lvl(1));
        reg.forget("s1");
        assert_eq!(h.get().level(), 1, "existing holders keep their handle");
        assert_eq!(reg.handle_for("s1").get().level(), 3);
    }

    #[test]
    fn global_registry_defaults_to_three() {
        // Never mutate the global in tests: assert only the seed contract on a
        // session id no other test uses.
        assert!(
            !session_defcon()
                .handle_for("defcon-global-probe")
                .get()
                .yolo_bypass_active()
        );
        session_defcon().forget("defcon-global-probe");
    }
}
