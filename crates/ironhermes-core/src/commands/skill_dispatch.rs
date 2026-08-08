//! Shared, pure skill-invocation resolver (Phase 41.1 Plan 01).
//!
//! This module centralizes the ONE piece of the per-surface SKILL-13 fallback
//! that is safely centralizable (RESEARCH Pattern 2): turning a slash token +
//! a `SkillRegistry` into a small, identity-free `SkillInvocation { name, body,
//! trigger_text }`. Every surface (TUI, Web, CLI REPL, Telegram gateway) calls
//! this same resolver, then drives its OWN turn-spawn primitive with the
//! resulting `trigger_text` — the spawn mechanics are deliberately NOT
//! centralized here (they are structurally different per surface).
//!
//! Security (T-41.1-01-02 / T-41.1-01-03):
//! - The skill body comes ONLY from `SkillRegistry::read_content`, the same
//!   SKILL-07-scanned load path used today — no second, unscanned injection path.
//! - `SkillInvocation` is pure data with NO chat_id/sender_id/session field;
//!   identity stays each surface's responsibility.

use crate::skills::SkillRegistry;

/// A resolved skill ready to be run one-shot at a surface.
///
/// Carries only the pure, identity-free data every surface needs:
/// - `name`: the normalized (registry) skill name.
/// - `body`: the SKILL.md body text, read via `SkillRegistry::read_content`.
/// - `trigger_text`: the message to submit as the run turn (D-02): either the
///   verbatim trailing args, or the bare-invoke run-now instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInvocation {
    pub name: String,
    pub body: String,
    pub trigger_text: String,
}

/// Build a `SkillInvocation`, computing `trigger_text` per D-02.
///
/// - `Some(non-empty-after-trim)` → the trimmed trailing text verbatim.
/// - `None` or whitespace-only → the bare-invoke run-now instruction
///   (UI-SPEC Copywriting Contract).
pub fn build_skill_invocation(name: String, body: String, args: Option<String>) -> SkillInvocation {
    let trigger_text = match args {
        Some(a) if !a.trim().is_empty() => a.trim().to_string(),
        _ => format!("Run the {name} skill now: carry out its instructions immediately."),
    };
    SkillInvocation {
        name,
        body,
        trigger_text,
    }
}

/// Resolve a raw slash `input` against the `SkillRegistry` into a
/// `SkillInvocation`, or `None` if the leading token is not a registered skill.
///
/// Mirrors the proven per-surface fallback shape: the command token is the
/// first whitespace token of the input (leading `/` stripped); trailing text
/// after `/{skill-name}` becomes the argued-invoke `trigger_text` (D-02).
/// The body is read ONLY via `registry.read_content` (no second body source).
pub fn resolve_skill_invocation(registry: &SkillRegistry, input: &str) -> Option<SkillInvocation> {
    let cmd_token = input
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("");
    if cmd_token.is_empty() {
        return None;
    }
    let record = registry.find(cmd_token)?;
    let body = registry.read_content(&record.name)?;
    let args_str = input
        .strip_prefix(&format!("/{}", record.name))
        .unwrap_or("")
        .trim();
    let args = if args_str.is_empty() {
        None
    } else {
        Some(args_str.to_string())
    };
    Some(build_skill_invocation(record.name.clone(), body, args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn make_skill_md(name: &str, description: &str, body: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}")
    }

    /// Build an isolated registry containing a single `gsd-config` skill.
    /// The returned TempDir MUST be kept alive: `read_content` reads from disk.
    fn test_registry() -> (tempfile::TempDir, SkillRegistry) {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("gsd-config");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            make_skill_md("gsd-config", "Configure GSD", "SKILL BODY CONTENT"),
        )
        .unwrap();
        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        (dir, registry)
    }

    #[test]
    fn resolve_skill_invocation_bare_uses_instruction() {
        let (_dir, registry) = test_registry();
        let inv = resolve_skill_invocation(&registry, "/gsd-config").expect("registered skill");
        assert_eq!(inv.name, "gsd-config");
        assert!(
            inv.body.contains("SKILL BODY CONTENT"),
            "body must come from read_content"
        );
        assert_eq!(
            inv.trigger_text,
            "Run the gsd-config skill now: carry out its instructions immediately."
        );
        // Whitespace-only args also fall back to the bare-invoke instruction.
        let inv_ws = resolve_skill_invocation(&registry, "/gsd-config   ").expect("registered skill");
        assert_eq!(inv_ws.trigger_text, inv.trigger_text);
    }

    #[test]
    fn resolve_skill_invocation_argued_uses_trailing_text() {
        let (_dir, registry) = test_registry();
        let inv = resolve_skill_invocation(&registry, "/gsd-config show me the config")
            .expect("registered skill");
        assert_eq!(inv.name, "gsd-config");
        assert_eq!(inv.trigger_text, "show me the config");
    }

    #[test]
    fn resolve_skill_invocation_unknown_token_is_none() {
        let (_dir, registry) = test_registry();
        assert!(resolve_skill_invocation(&registry, "/no-such-skill").is_none());
        assert!(resolve_skill_invocation(&registry, "/").is_none());
    }

    #[test]
    fn build_skill_invocation_trims_args_and_defaults_to_instruction() {
        let inv = build_skill_invocation(
            "gsd-config".to_string(),
            "body".to_string(),
            Some("  x  ".to_string()),
        );
        assert_eq!(inv.trigger_text, "x");

        let bare = build_skill_invocation("gsd-config".to_string(), "body".to_string(), None);
        assert_eq!(
            bare.trigger_text,
            "Run the gsd-config skill now: carry out its instructions immediately."
        );

        let ws = build_skill_invocation(
            "gsd-config".to_string(),
            "body".to_string(),
            Some("   ".to_string()),
        );
        assert_eq!(ws.trigger_text, bare.trigger_text);
    }
}
