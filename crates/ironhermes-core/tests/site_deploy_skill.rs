//! Phase 54.8 plan 11 — the `site-deploy` skill is bundled, ungated, and carries
//! its safety steps (D-09..D-13, D-26).
//!
//! These are presence checks on a procedure document: they prove the skill is
//! discoverable in the default `skills/` tree and that the load-bearing steps
//! were not edited out. The behavioral proof of the refusal and resume branches
//! is the live drill in plan 14.

use ironhermes_core::SkillRegistry;
use std::path::PathBuf;

fn devops_skills_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/ironhermes-core -> crates/ -> workspace root
    let workspace_root = manifest_dir
        .parent()
        .expect("crates/ironhermes-core has a parent (crates/)")
        .parent()
        .expect("crates/ has a parent (workspace root)");
    workspace_root.join("skills").join("devops")
}

fn skill_body() -> String {
    let path = devops_skills_root().join("site-deploy").join("SKILL.md");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("site-deploy SKILL.md must exist at {path:?}: {e}"))
}

#[test]
fn site_deploy_skill_loads_ungated() {
    let registry = SkillRegistry::load_with_paths(&[devops_skills_root()]);
    let record = registry
        .find("site-deploy")
        .expect("site-deploy must be discovered under skills/devops");

    assert!(
        ironhermes_core::skills::validate_skill_name("site-deploy").is_ok(),
        "site-deploy must be a valid skill name"
    );

    let requires = record
        .hermes_metadata
        .as_ref()
        .map(|m| m.requires_mcp_servers.clone())
        .unwrap_or_default();
    assert!(
        requires.is_empty(),
        "site-deploy must carry no MCP gate (D-17 spirit: visible with no server); got {requires:?}"
    );
}

#[test]
fn site_deploy_skill_has_safety_steps() {
    let body = skill_body();
    let needles = [
        "ironhermes sites show",
        "--json",
        "deploy_record",
        "ship it",
        "--match-head-commit",
        "worktree add --detach",
        "staged",
        "merged",
        "deployed",
        "[Session:",
        "/approve",
        "git -C",
        "deploy_mode",
        "timeout",
        "--ignore-scripts",
        "env -u CLOUDFLARE_API_TOKEN -u GITHUB_TOKEN",
    ];
    for needle in needles {
        assert!(
            body.contains(needle),
            "site-deploy SKILL.md must contain {needle:?}"
        );
    }
}

#[test]
fn site_deploy_skill_installs_without_lifecycle_scripts() {
    let body = skill_body();
    // Built from fragments so this test source never matches its own needles.
    let plain_ci = ["npm", " ci"].concat();
    let prefixed_ci = ["npm --prefix", " <"].concat();
    let flag = ["--ignore", "-scripts"].concat();

    let mut ci_lines = 0;
    for line in body.lines() {
        let has_plain = line.contains(&plain_ci);
        // `npm --prefix <dir> ci` — the `ci` follows the directory placeholder.
        let has_prefixed = line.contains(&prefixed_ci) && line.contains("> ci");
        if has_plain || has_prefixed {
            ci_lines += 1;
            assert!(
                line.contains(&flag),
                "every npm ci invocation must also carry {flag} on the same line; offending line: {line}"
            );
        }
    }
    assert!(
        ci_lines > 0,
        "the skill must contain at least one npm ci line (otherwise this check is vacuous)"
    );
}

#[test]
fn site_deploy_skill_never_uses_the_package_runner() {
    let body = skill_body();
    // Built from fragments so this test source never matches its own needles.
    let runner = ["np", "x wrangler"].concat();
    let pull = ["git", " pull"].concat();
    assert!(
        !body.contains(&runner),
        "the skill must invoke the installed wrangler binary, never the package runner (T-54.8-64)"
    );
    assert!(
        !body.contains(&pull),
        "the skill must not pull the base branch to deploy (T-54.8-63)"
    );
}
