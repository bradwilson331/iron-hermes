//! Shared toolset display helpers — Phase 25, D-06 / Critical Constraint 1.
//!
//! Lives in `ironhermes-core` so BOTH the CLI subcommand (`toolset_cmd.rs`)
//! and the slash command handler (`handlers.rs`) can call it without creating
//! a circular dependency (ironhermes-cli → ironhermes-core, NEVER the reverse).
//!
//! Pure functions: no I/O, no environment access — only rendering.

/// A single row in the `hermes toolset list` aligned-columns table.
pub struct ToolsetRow {
    pub name: String,
    pub enabled: bool,
    pub member_count: usize,
    pub available_count: usize,
    /// Human-readable member summary, e.g. "web_search ✓, web_read ✗ FIRECRAWL_API_KEY"
    pub member_summary: String,
    /// Phase 36.17.7 D-06 (REVISION BLOCKER 2 — Path B): registration state
    /// for the `voice` toolset's TTS tools. One of:
    ///   - `"Live"` — a real per-session SessionKey has registered TTS tools.
    ///   - `"Inspection"` — the CLI inspection sentinel
    ///     `(Platform::Local, "inspect")` registered TTS tools (no live session).
    ///   - `"—"` — not applicable (non-voice rows) or not registered.
    ///
    /// Builder helpers compute this from `CommandContext.tts_registration_status`
    /// (or default to `Inspection` on the CLI inspection path).
    pub registered: &'static str,
}

/// Render the aligned-columns table for `hermes toolset list` / `/toolset list`.
///
/// Output shape per CONTEXT.md §Specifics:
/// ```text
/// TOOLSET   STATUS    TOOLS  AVAILABLE
/// web       enabled   2      1/2 (web_search ✓, web_read ✗ FIRECRAWL_API_KEY)
/// memory    enabled   1      1/1
/// ```
///
/// Pure function — no I/O. Both `cmd_toolset_list` (toolset_cmd.rs) and
/// the slash `/toolset list` arm (handlers.rs) call this for a single
/// source-of-truth rendered table.
pub fn render_toolset_list(rows: Vec<ToolsetRow>) -> String {
    // Phase 36.17.7 D-06: Registered column appended after AVAILABLE.
    // Header widths kept stable for legacy renderers; new column has its own
    // fixed width.
    let header = format!(
        "{:<10} {:<10} {:<7} {:<11} REGISTERED\n",
        "TOOLSET", "STATUS", "TOOLS", "AVAILABLE"
    );

    let mut out = header;
    for row in &rows {
        let status = if row.enabled { "enabled" } else { "disabled" };
        let avail = format!("{}/{}", row.available_count, row.member_count);
        let detail = if row.member_summary.is_empty() {
            String::new()
        } else {
            format!(" ({})", row.member_summary)
        };
        // Render: NAME STATUS TOOLS  AVAILABLE-COL[detail trailing]  REGISTERED
        // The detail (member summary) trails AVAILABLE to preserve legacy
        // wrapping; REGISTERED is appended after the wrapping detail.
        out.push_str(&format!(
            "{:<10} {:<10} {:<7} {}{}  {}\n",
            row.name, status, row.member_count, avail, detail, row.registered
        ));
    }
    out
}

/// Render the detail view for a single toolset.
///
/// `members` is a list of `(tool_name, is_available, prereq_description)` triples.
pub fn render_toolset_show(row: &ToolsetRow, members: &[(String, bool, String)]) -> String {
    let status = if row.enabled { "enabled" } else { "disabled" };
    // Phase 36.17.7 D-06: surface the Registered state in the detail view too.
    let mut out = format!(
        "Toolset:    {}\nStatus:     {}\nRegistered: {}\nTools:      {}/{} available\n\nMembers:\n",
        row.name, status, row.registered, row.available_count, row.member_count
    );
    for (name, avail, prereq) in members {
        let mark = if *avail { "\u{2713}" } else { "\u{2717}" };
        if prereq.is_empty() {
            out.push_str(&format!("  {} {}\n", mark, name));
        } else {
            out.push_str(&format!("  {} {} (requires: {})\n", mark, name, prereq));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_row(
        name: &str,
        enabled: bool,
        total: usize,
        avail: usize,
        summary: &str,
    ) -> ToolsetRow {
        ToolsetRow {
            name: name.to_string(),
            enabled,
            member_count: total,
            available_count: avail,
            member_summary: summary.to_string(),
            // Phase 36.17.7 D-06: default test rows render the "—" sentinel —
            // the per-test setup overrides this for Voice rows.
            registered: "\u{2014}",
        }
    }

    #[test]
    fn render_toolset_list_includes_header() {
        let rows = vec![make_row(
            "web",
            true,
            2,
            1,
            "web_search \u{2713}, web_read \u{2717}",
        )];
        let out = render_toolset_list(rows);
        assert!(out.contains("TOOLSET"), "missing TOOLSET header");
        assert!(out.contains("STATUS"), "missing STATUS header");
        assert!(out.contains("TOOLS"), "missing TOOLS header");
        assert!(out.contains("AVAILABLE"), "missing AVAILABLE header");
    }

    #[test]
    fn render_toolset_list_shows_enabled_row() {
        let rows = vec![make_row("web", true, 2, 1, "")];
        let out = render_toolset_list(rows);
        assert!(out.contains("web"), "missing toolset name");
        assert!(out.contains("enabled"), "missing enabled status");
        assert!(out.contains("1/2"), "missing availability ratio");
    }

    #[test]
    fn render_toolset_list_shows_disabled_row() {
        let rows = vec![make_row("code", false, 6, 6, "")];
        let out = render_toolset_list(rows);
        assert!(out.contains("disabled"), "missing disabled status");
    }

    #[test]
    fn render_toolset_show_format() {
        let row = make_row("web", true, 2, 1, "");
        let members = vec![
            ("web_search".to_string(), true, String::new()),
            (
                "web_read".to_string(),
                false,
                "FIRECRAWL_API_KEY".to_string(),
            ),
        ];
        let out = render_toolset_show(&row, &members);
        assert!(out.contains("web"), "missing toolset name");
        assert!(out.contains("enabled"), "missing status");
        assert!(out.contains("web_search"), "missing web_search member");
        assert!(out.contains("web_read"), "missing web_read member");
        assert!(out.contains("FIRECRAWL_API_KEY"), "missing prereq");
    }
}
