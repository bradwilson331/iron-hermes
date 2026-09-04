//! Pasted-content skills source adapter (D-05, Phase 49.4/49.5) — promoted
//! here in Phase 49.6 Plan 03 (D-08, T-49.6-03-05) so both the UI's
//! `/blueprint save` write path (`iron_hermes_ui::server::skills_import_api`)
//! and the CLI's `/blueprint save` write path (`ironhermes-cli`'s
//! `BlueprintSaverImpl`) derive an installed artifact's name IDENTICALLY.
//! Promoting rather than copying is the point: a second pasted-content
//! adapter would be a second thing to keep in sync, and a UI-saved and a
//! CLI-saved blueprint must land under the same slug or the two surfaces
//! would silently disagree about a shared skill's identity.
//!
//! Stateless-unit-struct shape and doc-comment conventions mirror
//! [`crate::local_dir::LocalDirSource`].

use async_trait::async_trait;
use ironhermes_core::SkillSource;

use crate::error::{HubError, HubErrorKind};
use crate::source::{BundleFile, HubSource, SkillBundle, SkillMeta};

fn typed(kind: HubErrorKind, msg: impl Into<String>) -> HubError {
    HubError::Typed {
        kind,
        message: msg.into(),
        suggestion: None,
        retry_after_s: None,
    }
}

/// Extract and sanitize the `name:` field from SKILL.md YAML frontmatter.
///
/// Deliberately a lenient line-scan, NOT the full strict
/// `ironhermes_core::skills::parse_skill_md` parser — mirrors
/// [`crate::local_dir::LocalDirSource`]'s own private
/// `parse_skill_name_from_frontmatter` helper (and `github.rs`'s, the third
/// hand-rolled copy of this exact shape) rather than reusing it directly:
/// each adapter needs its own copy since the upstream ones are private to
/// their modules. This preserves the EXACT pre-promotion behavior this
/// module was moved from (`iron_hermes_ui::server::skills_import_api`): a
/// name is extractable even from a `SKILL.md` whose YAML is otherwise
/// malformed (e.g. an unclosed bracket), because a later, SEPARATE
/// `parse_skill_md_verbose` call downstream (in `preview_skill_import_impl`)
/// is what surfaces the SPECIFIC, field-naming parse error to the operator —
/// switching this extraction to the strict parser would make `fetch` itself
/// fail first, collapsing that specific error back to a generic
/// "couldn't read a SKILL.md" (a real UX regression, not just a test
/// change; see this crate's `pasted_skill_source_tests` for the parity
/// proof).
fn parse_frontmatter_name(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()? != "---" {
        return None;
    }
    for line in lines {
        if line == "---" {
            return None;
        }
        if let Some(rest) = line.strip_prefix("name:") {
            let name = rest.trim().trim_matches('"').trim_matches('\'');
            if !name.is_empty() {
                return Some(crate::sanitize::sanitize_name(name));
            }
        }
    }
    None
}

/// Pasted-content adapter closing D-05's "pasted SKILL.md content" gap.
/// The `identifier` passed to `fetch` IS the pasted text itself — there is
/// no separate address to resolve, so this adapter never performs network
/// I/O.
pub struct PastedSkillSource;

#[async_trait]
impl HubSource for PastedSkillSource {
    fn source_id(&self) -> &str {
        "pasted-skill"
    }

    /// Pasted text has no verifiable provenance whatsoever — the hub's
    /// lowest trust tier (mirrors `UrlSkillSource`'s identical rationale in
    /// `iron_hermes_ui::server::skills_import_api`, the only other adapter
    /// that never touches a local filesystem path or a vetted-origin URL).
    fn trust_level_for(&self, _identifier: &str) -> SkillSource {
        SkillSource::Community
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SkillMeta>, HubError> {
        Ok(vec![])
    }

    async fn fetch(&self, identifier: &str) -> Result<SkillBundle, HubError> {
        let name = parse_frontmatter_name(identifier).ok_or_else(|| {
            typed(
                HubErrorKind::Parse,
                "could not find a skill name in the pasted content's frontmatter",
            )
        })?;

        Ok(SkillBundle {
            name,
            identifier: identifier.to_string(),
            source_id: "pasted-skill".to_string(),
            files: vec![BundleFile {
                path: "SKILL.md".to_string(),
                bytes: identifier.as_bytes().to_vec(),
            }],
            skill_md: identifier.to_string(),
            metadata: serde_json::json!({}),
            snapshot_hash: None,
        })
    }
}

#[cfg(test)]
mod pasted_skill_source_tests {
    use super::*;

    fn valid_skill_md(name: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: A test skill for pasted-source tests.\n---\nBody content.\n"
        )
    }

    #[tokio::test]
    async fn fetch_on_valid_content_returns_a_bundle_with_the_frontmatter_name() {
        let content = valid_skill_md("my-blueprint");
        let bundle = PastedSkillSource
            .fetch(&content)
            .await
            .expect("fetch must succeed");
        assert_eq!(bundle.name, "my-blueprint");
        assert_eq!(bundle.files.len(), 1);
        assert_eq!(bundle.files[0].path, "SKILL.md");
        assert_eq!(bundle.files[0].bytes, content.as_bytes());
        assert_eq!(bundle.skill_md, content);
        assert_eq!(bundle.source_id, "pasted-skill");
        assert!(bundle.snapshot_hash.is_none());
    }

    #[tokio::test]
    async fn fetch_on_content_with_no_frontmatter_name_returns_a_parse_kind_error() {
        let content = "No frontmatter here at all.";
        let err = PastedSkillSource
            .fetch(content)
            .await
            .expect_err("fetch must fail");
        match err {
            HubError::Typed { kind, .. } => assert_eq!(kind, HubErrorKind::Parse),
            other => panic!("expected Typed(Parse), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn trust_level_is_community_the_lowest_tier() {
        assert_eq!(
            PastedSkillSource.trust_level_for("anything"),
            SkillSource::Community
        );
    }

    #[tokio::test]
    async fn search_always_returns_empty() {
        let results = PastedSkillSource
            .search("anything", 10)
            .await
            .expect("search must succeed");
        assert!(results.is_empty());
    }

    /// Parity proof for the promotion (T-49.6-03-05): a `name:` line is
    /// still extractable even when the surrounding YAML is otherwise
    /// malformed (an unclosed bracket) — the pre-promotion behavior a
    /// downstream, SEPARATE strict-parse call relies on to surface a
    /// field-naming error instead of a generic read failure. If this
    /// extraction were switched to the strict parser, `fetch` itself would
    /// fail here instead, which is the regression this test guards against.
    #[tokio::test]
    async fn fetch_extracts_a_name_even_when_the_rest_of_the_yaml_is_malformed() {
        let content = "---\nname: [this is not valid yaml\ndescription: d\n---\nbody".to_string();
        let bundle = PastedSkillSource
            .fetch(&content)
            .await
            .expect("fetch must still succeed on a malformed-but-name-bearing block");
        assert!(!bundle.name.is_empty());
        assert_eq!(bundle.skill_md, content, "raw content passed through unchanged");
    }
}
