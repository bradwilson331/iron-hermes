//! Centralized dual-read resolver for kanban + tenant env vars (D-COMPAT).
//!
//! Every kanban/tenant consumer MUST route through these helpers — NOT call
//! `std::env::var("HERMES_KANBAN_*")` directly — so that the
//! `IRONHERMES_*` → `HERMES_*` fallback is handled in exactly one place.
//!
//! ## Deprecation window
//!
//! During the transition period the legacy `HERMES_KANBAN_*` / `HERMES_TENANT`
//! vars are still accepted as a fallback. Once all writers (producers) have
//! migrated to the canonical `IRONHERMES_*` names and sufficient time has
//! passed, remove the legacy fallback by deleting every block marked
//! `// TODO(deprecation)` below.
//!
//! The canonical name is modeled on `get_hermes_home()` in
//! `crates/ironhermes-core/src/constants.rs`, which performs a *hard cut* to
//! `IRONHERMES_HOME`. This helper uses **dual-read** semantics per the user's
//! D-COMPAT decision: `IRONHERMES_*` wins when present and non-empty;
//! otherwise fall back to `HERMES_*`.

/// Resolve a kanban env var by suffix with dual-read backward compatibility (D-COMPAT).
///
/// Reads `IRONHERMES_KANBAN_{suffix}` first. If that var is present and
/// non-empty, return it. Otherwise read `HERMES_KANBAN_{suffix}`. Return
/// `None` when neither is set or both are empty.
///
/// An empty-string value is treated as "unset", matching the semantics of
/// `get_hermes_home()` (`Ok(p) if !p.is_empty()`). This makes the tool-gate
/// behavior (`kanban_env("TASK").is_some()`) more correct than the old
/// `env::var(...).is_ok()` because an empty string no longer falsely enables
/// the gate — `worker_spawn` always sets a non-empty task id.
///
/// # Examples
///
/// ```no_run
/// use ironhermes_kanban::kanban_env;
///
/// // Gates the kanban tools on (worker mode):
/// if kanban_env("TASK").is_some() { /* in worker */ }
///
/// // Resolves a task id with arg override:
/// let task_id = args_task_id.or_else(|| kanban_env("TASK"));
/// ```
pub fn kanban_env(suffix: &str) -> Option<String> {
    let new_key = format!("IRONHERMES_KANBAN_{suffix}");
    match std::env::var(&new_key) {
        Ok(v) if !v.is_empty() => return Some(v),
        _ => {}
    }
    // TODO(deprecation): drop HERMES_* legacy fallback after <window>
    let legacy_key = format!("HERMES_KANBAN_{suffix}");
    match std::env::var(&legacy_key) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// Resolve the tenant env var with dual-read backward compatibility (D-COMPAT).
///
/// Reads `IRONHERMES_TENANT` first; if present and non-empty, returns it.
/// Otherwise reads `HERMES_TENANT`. Returns `None` when neither is set or
/// both are empty.
pub fn tenant_env() -> Option<String> {
    match std::env::var("IRONHERMES_TENANT") {
        Ok(v) if !v.is_empty() => return Some(v),
        _ => {}
    }
    // TODO(deprecation): drop HERMES_* legacy fallback after <window>
    match std::env::var("HERMES_TENANT") {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests use the unique suffix "VR4_PROBE" so they never collide with real
    // kanban var names. All tests acquire the process-global ENV_LOCK before
    // mutating env to prevent cross-test races.

    #[test]
    fn kanban_env_ironhermes_wins_when_both_set() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("IRONHERMES_KANBAN_VR4_PROBE", "canonical");
            std::env::set_var("HERMES_KANBAN_VR4_PROBE", "legacy");
        }
        let result = kanban_env("VR4_PROBE");
        unsafe {
            std::env::remove_var("IRONHERMES_KANBAN_VR4_PROBE");
            std::env::remove_var("HERMES_KANBAN_VR4_PROBE");
        }
        assert_eq!(result, Some("canonical".to_string()));
    }

    #[test]
    fn kanban_env_legacy_fallback_when_only_legacy_set() {
        // (b) The dual-read guard: ONLY the legacy var is set.
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("IRONHERMES_KANBAN_VR4_PROBE");
            std::env::set_var("HERMES_KANBAN_VR4_PROBE", "legacy-only");
        }
        let result = kanban_env("VR4_PROBE");
        unsafe {
            std::env::remove_var("HERMES_KANBAN_VR4_PROBE");
        }
        assert_eq!(result, Some("legacy-only".to_string()));
    }

    #[test]
    fn kanban_env_empty_ironhermes_falls_through_to_legacy() {
        // (c) Empty IRONHERMES value falls through to non-empty legacy value.
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("IRONHERMES_KANBAN_VR4_PROBE", "");
            std::env::set_var("HERMES_KANBAN_VR4_PROBE", "legacy-value");
        }
        let result = kanban_env("VR4_PROBE");
        unsafe {
            std::env::remove_var("IRONHERMES_KANBAN_VR4_PROBE");
            std::env::remove_var("HERMES_KANBAN_VR4_PROBE");
        }
        assert_eq!(result, Some("legacy-value".to_string()));
    }

    #[test]
    fn kanban_env_none_when_neither_set() {
        // (d) None when neither is set.
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("IRONHERMES_KANBAN_VR4_PROBE");
            std::env::remove_var("HERMES_KANBAN_VR4_PROBE");
        }
        assert_eq!(kanban_env("VR4_PROBE"), None);
    }

    #[test]
    fn tenant_env_ironhermes_wins_when_both_set() {
        // (e-i) IRONHERMES_TENANT wins when both set.
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("IRONHERMES_TENANT", "canonical-tenant");
            std::env::set_var("HERMES_TENANT", "legacy-tenant");
        }
        let result = tenant_env();
        unsafe {
            std::env::remove_var("IRONHERMES_TENANT");
            std::env::remove_var("HERMES_TENANT");
        }
        assert_eq!(result, Some("canonical-tenant".to_string()));
    }

    #[test]
    fn tenant_env_legacy_fallback_when_only_legacy_set() {
        // (e-ii) Legacy fallback: only HERMES_TENANT set.
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("IRONHERMES_TENANT");
            std::env::set_var("HERMES_TENANT", "legacy-tenant-only");
        }
        let result = tenant_env();
        unsafe {
            std::env::remove_var("HERMES_TENANT");
        }
        assert_eq!(result, Some("legacy-tenant-only".to_string()));
    }

    #[test]
    fn tenant_env_empty_ironhermes_falls_through_to_legacy() {
        // (e-iii) Empty IRONHERMES_TENANT falls through to legacy.
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("IRONHERMES_TENANT", "");
            std::env::set_var("HERMES_TENANT", "legacy-tenant-value");
        }
        let result = tenant_env();
        unsafe {
            std::env::remove_var("IRONHERMES_TENANT");
            std::env::remove_var("HERMES_TENANT");
        }
        assert_eq!(result, Some("legacy-tenant-value".to_string()));
    }

    #[test]
    fn tenant_env_none_when_neither_set() {
        // (e-iv) None when neither is set.
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("IRONHERMES_TENANT");
            std::env::remove_var("HERMES_TENANT");
        }
        assert_eq!(tenant_env(), None);
    }
}
