//! CFL-01 load test — 5 Cloudflare skills load as Official tier + names valid (D-05).
//!
//! Proves success-criterion #3: the 5 vendored skill trees under
//! `optional-skills/cloudflare/` are discoverable by the existing skills loader as
//! `SkillSource::Official` with NO loader code change (D-03).
//!
//! Resolution path: `optional-skills/cloudflare/` contains one subdirectory per skill;
//! the loader scans one level deep and resolves each SKILL.md. Because every resolved
//! path contains the `optional-skills` path component, `resolve_source` returns
//! `SkillSource::Official` unconditionally (skills.rs lines 434-440).

use ironhermes_core::{HubConfig, SkillRegistry, SkillSource, SkillsConfig};
use std::path::PathBuf;

// =============================================================================
// Constants
// =============================================================================

/// The 5 Cloudflare skills vendored in `optional-skills/cloudflare/` for CFL-01.
/// Names are lowercase-kebab and must pass `validate_skill_name` (D-05).
const CLOUDFLARE_SKILLS: &[&str] = &[
    "cloudflare",
    "wrangler",
    "workers-best-practices",
    "durable-objects",
    "agents-sdk",
];

// =============================================================================
// Helpers
// =============================================================================

/// Resolve `optional-skills/cloudflare/` from `CARGO_MANIFEST_DIR`.
///
/// `CARGO_MANIFEST_DIR` = `<workspace>/crates/ironhermes-core`
/// Walk up 2 levels → workspace root → join `optional-skills/cloudflare`.
fn cloudflare_skills_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/ironhermes-core  →  parent = crates/  →  parent = workspace root
    let workspace_root = manifest_dir
        .parent()
        .expect("crates/ironhermes-core has a parent (crates/)")
        .parent()
        .expect("crates/ has a parent (workspace root)");
    workspace_root.join("optional-skills").join("cloudflare")
}

// =============================================================================
// Test: cloudflare_skills_load_as_official_and_names_valid
// =============================================================================

/// CFL-01 / D-05: all 5 Cloudflare skills load as Official tier; none silently dropped;
/// all 5 names pass validate_skill_name.
#[test]
fn cloudflare_skills_load_as_official_and_names_valid() {
    let cloudflare_root = cloudflare_skills_root();
    assert!(
        cloudflare_root.exists(),
        "optional-skills/cloudflare/ not found at {:?} — was the vendoring step skipped?",
        cloudflare_root
    );

    // Use a fresh tempdir as cwd so none of the 3 hardcoded default skill paths
    // (cwd/.ironhermes/skills, IRONHERMES_HOME/skills, ~/.agents/skills) exist,
    // ensuring only our explicit extra_path is scanned.
    let cwd_tmp = tempfile::tempdir().expect("tempdir for cwd isolation");

    // Point extra_paths at optional-skills/cloudflare/ — the directory containing
    // the 5 skill subdirectories (one level deep scan finds each SKILL.md).
    let cfg = SkillsConfig {
        enabled: true,
        extra_paths: vec![cloudflare_root.clone()],
        hub: HubConfig::default(),
        ..SkillsConfig::default()
    };
    let registry = SkillRegistry::load_with_config(cwd_tmp.path(), &cfg);

    for name in CLOUDFLARE_SKILLS {
        // D-05 (name validation): all 5 lowercase-kebab names pass the agentskills.io rules.
        assert!(
            ironhermes_core::skills::validate_skill_name(name).is_ok(),
            "validate_skill_name({name:?}) must pass for all CFL-01 skills (D-05); \
             got: {:?}",
            ironhermes_core::skills::validate_skill_name(name)
        );

        // D-05 (no silent drop): skill must be found in the registry.
        let record = registry.find(name);
        assert!(
            record.is_some(),
            "Skill {name:?} not found in registry after load — silently dropped? (D-05). \
             Checked cloudflare_root: {cloudflare_root:?}"
        );

        // D-03 (Official tier via optional-skills/ placement, no loader change):
        // resolve_source sees the `optional-skills` path component and returns Official.
        if let Some(r) = record {
            assert_eq!(
                r.source,
                SkillSource::Official,
                "Skill {name:?} must be SkillSource::Official because its path contains \
                 `optional-skills` (D-03); got {:?}",
                r.source
            );
        }
    }
}
