//! Phase 42 D-04/D-05: Terminal subprocess env sanitization.
//!
//! Homes `build_terminal_safe_env()` in `ironhermes-core` — the base crate that
//! every downstream crate depends on — to avoid the `mcp → tools → mcp`
//! dependency cycle (see 42-01-PLAN.md planner deviation note).
//!
//! Semantics mirror `ironhermes_mcp::security::build_safe_env()` but add two
//! D-05 overlay layers: a global operator allowlist and a per-command `pass_env`.
//!
//! # Caller contract
//!
//! This function returns a map only. The caller must call `cmd.env_clear()` before
//! `cmd.envs(build_terminal_safe_env(...))`. Calling `.envs()` without `.env_clear()`
//! first is additive-only and does NOT scrub the inherited process env
//! (Anti-Pattern 2 in 42-RESEARCH.md; mirroring `worker_spawn.rs` canonical ordering).

use std::collections::HashMap;

/// Base allowlist of env var names that are always safe to pass to subprocesses.
///
/// Mirrors `SAFE_ENV_KEYS` in `ironhermes-mcp/src/security.rs` (D-04 — exactly 8 keys).
/// `IRONHERMES_HOME` is NOT in this list; it is added unconditionally at the end of
/// `build_terminal_safe_env()` when present, matching `SAFE_SYSTEM_VARS` in
/// `ironhermes-kanban/src/worker_spawn.rs`.
pub const SAFE_ENV_KEYS: &[&str] = &[
    "PATH", "HOME", "USER", "LANG", "LC_ALL", "TERM", "SHELL", "TMPDIR",
];

/// Build a sanitized environment map for terminal subprocesses.
///
/// Layers applied in order (later layers may override earlier):
///
/// 1. **Base** — `SAFE_ENV_KEYS` (PATH/HOME/USER/LANG/LC_ALL/TERM/SHELL/TMPDIR) and
///    every `XDG_*` var from the current process env (D-04).
/// 2. **Global operator allowlist** — names in `global_allowlist` that exist in the
///    process env are added (D-05; sourced from `TerminalConfig.terminal_env_allowlist`).
///    Absent names are silently skipped.
/// 3. **Per-command least-privilege opt-in** — names in `per_command_pass_env` that
///    exist in the process env are added (D-05; sourced from `QuickCommandDef.pass_env`).
///    Absent names are silently skipped.
/// 4. **`IRONHERMES_HOME`** — always added when present so that nested ironhermes CLI
///    invocations find the correct home directory (mirrors `SAFE_SYSTEM_VARS` in
///    `ironhermes-kanban/src/worker_spawn.rs`).
///
/// # Security note (T-42-01)
///
/// Any env var not in the above layers is excluded. This prevents secrets such as
/// `CLOUDFLARE_API_TOKEN` from being exfiltrated by a subprocess that calls `env`
/// or `curl -d "$(env)"`. The real security boundary is this allowlist, not the
/// dangerous-command denylist (Anti-Pattern 1 in 42-RESEARCH.md).
pub fn build_terminal_safe_env(
    global_allowlist: &[String],
    per_command_pass_env: &[String],
) -> HashMap<String, String> {
    // Layer 1: base SAFE_ENV_KEYS + XDG_* from process env
    let mut env: HashMap<String, String> = std::env::vars()
        .filter(|(k, _)| SAFE_ENV_KEYS.contains(&k.as_str()) || k.starts_with("XDG_"))
        .collect();

    // Layer 2: global operator allowlist (D-05; TerminalConfig.terminal_env_allowlist)
    for key in global_allowlist {
        if let Ok(val) = std::env::var(key) {
            env.insert(key.clone(), val);
        }
        // Absent names are silently skipped per D-05
    }

    // Layer 3: per-command least-privilege opt-in (D-05; QuickCommandDef.pass_env)
    for key in per_command_pass_env {
        if let Ok(val) = std::env::var(key) {
            env.insert(key.clone(), val);
        }
        // Absent names are silently skipped per D-05
    }

    // Layer 4: IRONHERMES_HOME always passes through (worker_spawn.rs SAFE_SYSTEM_VARS)
    if let Ok(val) = std::env::var("IRONHERMES_HOME") {
        env.insert("IRONHERMES_HOME".to_string(), val);
    }

    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_env_keys_has_eight_entries() {
        // Invariant: must match the 8-key list from ironhermes-mcp/src/security.rs (D-04).
        assert_eq!(SAFE_ENV_KEYS.len(), 8);
    }
}
