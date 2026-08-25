//! Phase 48.2 Plan 06 Task 1 (D-15): the Tools page search/favorites
//! toolbar. `ToolsToolbar` owns the query and favorites-only signals and
//! renders the search input plus the favorites-only chip. `matches_query`
//! and `filter_groups` are pure, unit-testable helpers so the filter logic
//! is verified without rendering anything — `ScreenTools` (`tools.rs`)
//! applies `filter_groups` to the fetched catalog before handing groups to
//! `ToolsetSection`/`FavoritesRow`.
//!
//! Filtering is entirely client-side over the already-fetched catalog — no
//! server call per keystroke (D-15 must_haves).

use crate::server::tools_config_api::{ToolCatalogEntry, ToolsetGroup};
use dioxus::prelude::*;

/// Case-insensitive match on a tool's name OR description. An empty query
/// matches everything. Because MCP-imported tool names are the full
/// `<server>__<tool>` prefixed form (`ironhermes_mcp::make_prefixed_name`),
/// a substring match against the full name already matches both the full
/// prefixed name and the bare suffix after the prefix — no special-casing
/// needed.
#[allow(dead_code)] // used inside ScreenTools + filter_groups; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
pub fn matches_query(entry: &ToolCatalogEntry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let needle = query.to_lowercase();
    entry.name.to_lowercase().contains(&needle) || entry.description.to_lowercase().contains(&needle)
}

/// Filter `groups` by `query` and (when `favorites_only` is set) pinned
/// membership. A group whose tools all filter out is dropped entirely
/// rather than rendered with an empty header — the returned `Vec` therefore
/// drives both the main grid and (called with `favorites_only: true`) the
/// FAVORITES row.
#[allow(dead_code)] // used inside ScreenTools; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
pub fn filter_groups(
    groups: &[ToolsetGroup],
    query: &str,
    favorites_only: bool,
    pinned: &crate::ui_prefs::ToolFavorites,
) -> Vec<ToolsetGroup> {
    groups
        .iter()
        .filter_map(|group| {
            let tools: Vec<ToolCatalogEntry> = group
                .tools
                .iter()
                .filter(|tool| matches_query(tool, query))
                .filter(|tool| !favorites_only || pinned.is_pinned(&tool.name))
                .cloned()
                .collect();
            if tools.is_empty() {
                None
            } else {
                Some(ToolsetGroup {
                    name: group.name.clone(),
                    kind: group.kind.clone(),
                    enabled: group.enabled,
                    tools,
                })
            }
        })
        .collect()
}

/// The search bar + favorites-only filter chip mounted beneath the profile
/// bar in the Tools screen header. `query`/`favorites_only` are owned by
/// the caller (`ScreenTools`) so filtering can be applied to the fetched
/// catalog before it reaches the catalog components.
#[component]
pub fn ToolsToolbar(mut query: Signal<String>, mut favorites_only: Signal<bool>) -> Element {
    let favorites_active = favorites_only();
    rsx! {
        div { class: "tools-toolbar",
            input {
                class: "tools-search-input",
                r#type: "text",
                placeholder: "Search tools…",
                "aria-label": "Search tools",
                value: "{query}",
                oninput: move |evt| query.set(evt.value()),
            }
            button {
                class: if favorites_active { "tools-favorites-chip on" } else { "tools-favorites-chip" },
                "aria-label": "Show only favorited tools",
                "aria-pressed": if favorites_active { "true" } else { "false" },
                onclick: move |_| favorites_only.set(!favorites_active),
                "★ FAVORITES"
            }
        }
    }
}

// =============================================================================
// Pure-fn tests — Phase 48.2 Plan 06 Task 1 (D-15), RED first.
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tools_config_api::ToolAvailability;
    use crate::ui_prefs::ToolFavorites;

    fn entry(name: &str, description: &str) -> ToolCatalogEntry {
        ToolCatalogEntry {
            name: name.to_string(),
            description: description.to_string(),
            toolset: "web".to_string(),
            group: "web".to_string(),
            disabled_override: false,
            availability: ToolAvailability::Available,
            origin: crate::server::tools_config_api::ToolsetKind::BuiltIn,
            runtime_dependency: None,
        }
    }

    fn group(name: &str, tools: Vec<ToolCatalogEntry>) -> ToolsetGroup {
        ToolsetGroup {
            name: name.to_string(),
            kind: crate::server::tools_config_api::ToolsetKind::BuiltIn,
            enabled: true,
            tools,
        }
    }

    // -------------------------------------------------------------------
    // matches_query
    // -------------------------------------------------------------------

    #[test]
    fn matches_query_empty_query_matches_everything() {
        let e = entry("web_search", "Search the web");
        assert!(matches_query(&e, ""));
    }

    #[test]
    fn matches_query_is_case_insensitive_on_name() {
        let e = entry("web_search", "Search the web");
        assert!(matches_query(&e, "WEB_SEARCH"));
    }

    #[test]
    fn matches_query_is_case_insensitive_on_description() {
        let e = entry("web_search", "Search the web");
        assert!(matches_query(&e, "SEARCH THE WEB"));
    }

    #[test]
    fn matches_query_non_matching_query_returns_false() {
        let e = entry("web_search", "Search the web");
        assert!(!matches_query(&e, "totally_unrelated_zzz"));
    }

    /// An MCP-prefixed tool (`<server>__<tool>` per
    /// `ironhermes_mcp::make_prefixed_name`) matches both by its full
    /// prefixed name and by the bare part after the prefix.
    #[test]
    fn matches_query_mcp_prefixed_tool_matches_full_name_and_bare_suffix() {
        let e = entry("github__create_issue", "Create a GitHub issue");
        assert!(matches_query(&e, "github__create_issue"));
        assert!(matches_query(&e, "create_issue"));
    }

    // -------------------------------------------------------------------
    // filter_groups
    // -------------------------------------------------------------------

    #[test]
    fn filter_groups_query_matching_nothing_returns_zero_groups() {
        let groups = vec![group("web", vec![entry("web_search", "Search the web")])];
        let result = filter_groups(&groups, "zzz_no_match", false, &ToolFavorites::default());
        assert!(result.is_empty());
    }

    #[test]
    fn filter_groups_query_matching_one_tool_returns_one_group_with_only_that_tool() {
        let groups = vec![group(
            "web",
            vec![
                entry("web_search", "Search the web"),
                entry("web_extract", "Extract page content"),
            ],
        )];
        let result = filter_groups(&groups, "web_search", false, &ToolFavorites::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tools.len(), 1);
        assert_eq!(result[0].tools[0].name, "web_search");
    }

    #[test]
    fn filter_groups_drops_groups_with_no_match_rather_than_empty_headers() {
        let groups = vec![
            group("web", vec![entry("web_search", "Search the web")]),
            group("code", vec![entry("exec_python", "Run Python code")]),
        ];
        let result = filter_groups(&groups, "web_search", false, &ToolFavorites::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "web");
    }

    #[test]
    fn filter_groups_favorites_only_returns_only_pinned_tools() {
        let groups = vec![group(
            "web",
            vec![
                entry("web_search", "Search the web"),
                entry("web_extract", "Extract page content"),
            ],
        )];
        let mut pinned = ToolFavorites::default();
        pinned.pin("web_search");
        let result = filter_groups(&groups, "", true, &pinned);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tools.len(), 1);
        assert_eq!(result[0].tools[0].name, "web_search");
    }

    #[test]
    fn filter_groups_favorites_only_combined_with_query_applies_both_conditions() {
        let groups = vec![group(
            "web",
            vec![
                entry("web_search", "Search the web"),
                entry("web_extract", "Extract page content"),
            ],
        )];
        let mut pinned = ToolFavorites::default();
        pinned.pin("web_search");
        pinned.pin("web_extract");
        // Query narrows to only "extract", favorites_only requires pinned —
        // both are pinned, so only web_extract survives the query.
        let result = filter_groups(&groups, "extract", true, &pinned);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tools.len(), 1);
        assert_eq!(result[0].tools[0].name, "web_extract");
    }
}
