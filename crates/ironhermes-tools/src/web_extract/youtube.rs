//! Phase 25.2 D-10: YouTube dispatch via the in-tree `youtube-content` skill helper script.
//!
//! IMPORTANT: The Phase 19 skills runtime exposes NO programmatic `execute` API. Skills are
//! content (Markdown bodies + helper scripts). This tool shells out to the skill's
//! `scripts/fetch_transcript.py` directly via `tokio::process::Command` (RESEARCH.md target #4
//! + Open Question §2). The CONTEXT.md D-10 wording "via the Phase 19 skills framework" is
//! satisfied by using the skill's on-disk layout as the canonical extension point — operators
//! can swap the script without rebuilding the binary.
//!
//! Skill name: `youtube-content` (HYPHENATED) — verified against
//! skills/media/youtube-content/SKILL.md `name:` frontmatter (RESEARCH.md target #4).

use anyhow::{anyhow, Result};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::web_extract::ExtractionResult;
use ironhermes_core::SkillRegistry;

/// Skill dispatch key — must match the `name:` field in the skill's SKILL.md frontmatter.
/// HYPHENATED, not underscored. Verified RESEARCH.md target #4.
const YOUTUBE_SKILL_NAME: &str = "youtube-content";

/// Helper script relative path inside the skill directory.
const HELPER_SCRIPT_RELPATH: &str = "scripts/fetch_transcript.py";

/// D-10 entry point. Looks up the youtube-content skill, then shells out to its helper script.
/// Returns `Err` (mapped by Plan 13 dispatcher into `ExtractionResult.error`) on any failure
/// of the skill / script / parser layers — partial-success per D-02.
pub async fn extract_youtube(
    url: &str,
    registry: &Arc<SkillRegistry>,
) -> Result<ExtractionResult> {
    // 1. Look up the skill record (case-insensitive per skills.rs:729 verified)
    let record = registry
        .find(YOUTUBE_SKILL_NAME)
        .ok_or_else(|| anyhow!("youtube_skill_not_installed: {}", YOUTUBE_SKILL_NAME))?;

    // 2. Resolve the helper script path
    let skill_dir = record.path.parent().ok_or_else(|| {
        anyhow!(
            "youtube_skill_invalid_path: {} has no parent",
            record.path.display()
        )
    })?;
    let script_path = skill_dir.join(HELPER_SCRIPT_RELPATH);

    if !script_path.exists() {
        return Err(anyhow!(
            "youtube_skill_helper_missing: {}",
            script_path.display()
        ));
    }

    debug!(
        "web_extract.youtube: invoking {} for {}",
        script_path.display(),
        url
    );

    // 3. Invoke python3 <script> <url> --text-only — capture stdout AND stderr.
    //    URL is passed as a separate `arg(url)` (no shell interpolation) per
    //    T-25.2-shell-injection mitigation (tokio::process::Command does not invoke a shell;
    //    args go directly to execve).
    let output = tokio::process::Command::new("python3")
        .arg(&script_path)
        .arg(url)
        .arg("--text-only")
        .output()
        .await
        .map_err(|e| {
            anyhow!(
                "youtube_skill_helper_spawn_failed: {} (is python3 on PATH?)",
                e
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last = stderr.lines().last().unwrap_or("(no stderr output)");
        warn!(
            "youtube-content helper exited {}: {}",
            output.status, stderr
        );
        return Err(anyhow!("youtube_skill_helper_failed: {}", last));
    }

    // 4. Parse stdout as Markdown; pull the first `# ` heading as title.
    let body = String::from_utf8(output.stdout)
        .map_err(|e| anyhow!("youtube_skill_output_not_utf8: {}", e))?;

    if body.trim().is_empty() {
        return Err(anyhow!("youtube_skill_returned_empty_output"));
    }

    let title = first_h1(&body);
    Ok(ExtractionResult {
        url: url.to_string(),
        title,
        content: body,
        error: None,
    })
}

/// Extract the first `# Heading` line as plain title text. Empty string if none found.
fn first_h1(markdown: &str) -> String {
    markdown
        .lines()
        .find(|l| l.starts_with("# "))
        .map(|l| l.trim_start_matches("# ").trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_skill_name_is_hyphenated() {
        // Guard against PRD copy-paste error: D-10 caveat from RESEARCH.md target #4.
        assert_eq!(YOUTUBE_SKILL_NAME, "youtube-content");
        assert_ne!(YOUTUBE_SKILL_NAME, "youtube_content");
    }

    #[test]
    fn helper_script_relpath_correct() {
        assert_eq!(HELPER_SCRIPT_RELPATH, "scripts/fetch_transcript.py");
    }

    #[test]
    fn first_h1_pulls_title() {
        let md = "# My Video Title\nDescription...\n## Transcript\n[00:00] hello";
        assert_eq!(first_h1(md), "My Video Title");
    }

    #[test]
    fn first_h1_returns_empty_when_no_heading() {
        assert_eq!(first_h1("Just plain text"), "");
    }

    #[test]
    fn first_h1_skips_subheadings() {
        let md = "## Subhead\n# Title\n";
        assert_eq!(first_h1(md), "Title");
    }

    // Real skill-dispatch behavior is exercised by Plan 14 wiremock test
    // (web_extract_youtube_url_dispatches_to_skill) which builds a mock SkillRegistry
    // with a stub script returning canned Markdown.
}
