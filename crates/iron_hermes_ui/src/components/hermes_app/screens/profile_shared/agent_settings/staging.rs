//! Phase 54.4 Plan 10 (D-17, D-18, D-19) — the slide-over's staged tools and
//! skills selection, as plain data.
//!
//! The model is OPT-OUT only: a chip means ENABLED, removing a chip disables
//! the item, and nothing here is an allow-list. Toolsets stage into
//! `tools.toolsets` (whole built-in toolsets) and individual tools into
//! `tools.disabled`; skills stage into `skills.disabled`. The module holds no
//! dioxus types, so every rule is unit-tested natively.
//!
//! Only catalog-derived names ever enter a stage (MCP groups never do, stale
//! skill names never do), which keeps one stale name from bricking Save — the
//! server re-validates every name regardless (plan 03).

use std::collections::{BTreeMap, BTreeSet};

use crate::components::hermes_app::screens::skills::search_matches;
use crate::protocol::{ProfileSkillRow, RootAgentDefaults};
use crate::server::tools_config_api::{ToolsPageState, ToolsetKind};

/// The staged tools selection: which writable built-in toolsets are enabled,
/// and which individual tools are disabled.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct ToolsStage {
    /// Writable built-in toolsets (BuiltIn AND in `bulk_targets`) -> enabled.
    pub toolsets: BTreeMap<String, bool>,
    /// Individually disabled built-in tools. Seeded from EVERY built-in group
    /// (not only the writable ones) so a Save that rewrites `tools.disabled`
    /// never drops an entry the panel has no chip for.
    pub tools_disabled: BTreeSet<String>,
}

/// One member tool chip.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct ToolChip {
    pub name: String,
    pub description: String,
}

/// One enabled toolset chip with its enabled member tools.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct ToolsetChip {
    pub name: String,
    pub tools: Vec<ToolChip>,
}

/// One row of the "Tool selection" list (things that can be re-added).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum SelectionRow {
    /// A disabled toolset.
    Toolset { name: String },
    /// A disabled tool inside an ENABLED toolset.
    Tool {
        toolset: String,
        name: String,
        description: String,
    },
}

/// The tools payload fields of `ProfileConfigWritePayload`:
/// `(tools_toolsets, tools_disable, tools_enable)` — the toolsets that changed
/// and the operator's delta of tools to disable / re-enable (WR-06).
pub(crate) type ToolsPayload = (
    Option<Vec<(String, bool)>>,
    Option<Vec<String>>,
    Option<Vec<String>>,
);

/// Seed a stage from the profile-scoped Tools page state. MCP groups and the
/// flat sentinel toolsets (anything outside `bulk_targets`) never become
/// toolset entries; their disabled tools are still carried in
/// `tools_disabled` (built-in groups only) so a Save cannot drop them.
pub(crate) fn tools_stage_from_page(page: &ToolsPageState) -> ToolsStage {
    let mut stage = ToolsStage::default();
    for group in &page.toolsets {
        if !matches!(group.kind, ToolsetKind::BuiltIn) {
            continue;
        }
        if page.bulk_targets.iter().any(|t| t == &group.name) {
            stage.toolsets.insert(group.name.clone(), group.enabled);
        }
        for tool in &group.tools {
            if tool.disabled_override {
                stage.tools_disabled.insert(tool.name.clone());
            }
        }
    }
    stage
}

/// Flip one staged toolset to `enabled`. Returns whether the stage changed;
/// a name that is not a staged (writable) toolset never changes anything.
fn set_toolset(stage: &mut ToolsStage, name: &str, enabled: bool) -> bool {
    match stage.toolsets.get_mut(name) {
        Some(current) if *current != enabled => {
            *current = enabled;
            true
        }
        _ => false,
    }
}

/// Disable a staged toolset. Returns whether the stage changed.
pub(crate) fn disable_toolset(stage: &mut ToolsStage, name: &str) -> bool {
    set_toolset(stage, name, false)
}

/// Re-enable a staged toolset. Returns whether the stage changed.
pub(crate) fn enable_toolset(stage: &mut ToolsStage, name: &str) -> bool {
    set_toolset(stage, name, true)
}

/// Disable one tool (adds it to `tools_disabled`). Returns whether it changed.
pub(crate) fn disable_tool(stage: &mut ToolsStage, name: &str) -> bool {
    stage.tools_disabled.insert(name.to_string())
}

/// Re-enable one tool (removes it from `tools_disabled`).
pub(crate) fn enable_tool(stage: &mut ToolsStage, name: &str) -> bool {
    stage.tools_disabled.remove(name)
}

/// The enabled toolset chips, each with its enabled member tools, in the
/// page's order.
pub(crate) fn toolset_chips(stage: &ToolsStage, page: &ToolsPageState) -> Vec<ToolsetChip> {
    page.toolsets
        .iter()
        .filter(|g| stage.toolsets.get(&g.name) == Some(&true))
        .map(|g| ToolsetChip {
            name: g.name.clone(),
            tools: g
                .tools
                .iter()
                .filter(|t| !stage.tools_disabled.contains(&t.name))
                .map(|t| ToolChip {
                    name: t.name.clone(),
                    description: t.description.clone(),
                })
                .collect(),
        })
        .collect()
}

/// The re-addable rows, in the page's order: a disabled toolset is one row;
/// an enabled toolset contributes a row per disabled member tool.
pub(crate) fn tool_selection(stage: &ToolsStage, page: &ToolsPageState) -> Vec<SelectionRow> {
    let mut rows = Vec::new();
    for group in &page.toolsets {
        match stage.toolsets.get(&group.name) {
            Some(false) => rows.push(SelectionRow::Toolset {
                name: group.name.clone(),
            }),
            Some(true) => {
                for tool in &group.tools {
                    if stage.tools_disabled.contains(&tool.name) {
                        rows.push(SelectionRow::Tool {
                            toolset: group.name.clone(),
                            name: tool.name.clone(),
                            description: tool.description.clone(),
                        });
                    }
                }
            }
            None => {}
        }
    }
    rows
}

/// The payload fields for `stage` against `seed`: only the toolsets whose
/// staged state differs from the seed, and the DELTA of the disabled set
/// (WR-06): tools newly disabled in `stage` and tools re-enabled relative to
/// `seed`, each sorted. The server applies the delta to the `tools.disabled`
/// on disk at save time, so a tool disabled elsewhere since the panel opened
/// is never re-enabled. `(None, None, None)` when nothing changed.
pub(crate) fn tools_payload(stage: &ToolsStage, seed: &ToolsStage) -> ToolsPayload {
    let changed: Vec<(String, bool)> = stage
        .toolsets
        .iter()
        .filter(|(name, enabled)| seed.toolsets.get(*name) != Some(*enabled))
        .map(|(name, enabled)| (name.clone(), *enabled))
        .collect();
    let toolsets = if changed.is_empty() {
        None
    } else {
        Some(changed)
    };
    let delta = |from: &BTreeSet<String>, not_in: &BTreeSet<String>| -> Option<Vec<String>> {
        let names: Vec<String> = from.difference(not_in).cloned().collect();
        (!names.is_empty()).then_some(names)
    };
    (
        toolsets,
        delta(&stage.tools_disabled, &seed.tools_disabled),
        delta(&seed.tools_disabled, &stage.tools_disabled),
    )
}

/// The tool selection list narrowed by the search box (D-20): case-insensitive
/// through the shared `search_matches` on name + description (a toolset row
/// matches on its name). An empty query keeps every row.
pub(crate) fn tool_selection_filtered(rows: &[SelectionRow], query: &str) -> Vec<SelectionRow> {
    rows.iter()
        .filter(|row| match row {
            SelectionRow::Toolset { name } => search_matches(name, "", query),
            SelectionRow::Tool {
                name, description, ..
            } => search_matches(name, description, query),
        })
        .cloned()
        .collect()
}

/// The staged skills opt-out: skills turned OFF for the agent
/// (`skills.disabled`). One root catalog, per-profile opt-out (54.1 D-01/D-12).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct SkillsStage {
    pub disabled: BTreeSet<String>,
}

/// Seed from the profile's catalog rows. The rows are already intersected with
/// the live catalog server-side, so a stale name in `skills.disabled` (one the
/// catalog no longer has) never enters the stage (Pitfall 6).
pub(crate) fn skills_stage_from_rows(rows: &[ProfileSkillRow]) -> SkillsStage {
    SkillsStage {
        disabled: rows
            .iter()
            .filter(|r| !r.enabled_for_profile)
            .map(|r| r.name.clone())
            .collect(),
    }
}

/// Turn a skill off. Returns whether the stage changed.
pub(crate) fn disable_skill(stage: &mut SkillsStage, name: &str) -> bool {
    stage.disabled.insert(name.to_string())
}

/// Turn a skill back on. Returns whether the stage changed.
pub(crate) fn enable_skill(stage: &mut SkillsStage, name: &str) -> bool {
    stage.disabled.remove(name)
}

/// The enabled skills (chips), in catalog order.
pub(crate) fn skill_chips(stage: &SkillsStage, rows: &[ProfileSkillRow]) -> Vec<String> {
    rows.iter()
        .filter(|r| !stage.disabled.contains(&r.name))
        .map(|r| r.name.clone())
        .collect()
}

/// The disabled skills whose name or category matches `query`, in catalog
/// order. Search never touches the chips.
pub(crate) fn skill_selection(
    stage: &SkillsStage,
    rows: &[ProfileSkillRow],
    query: &str,
) -> Vec<ProfileSkillRow> {
    rows.iter()
        .filter(|r| stage.disabled.contains(&r.name))
        .filter(|r| search_matches(&r.name, &r.category, query))
        .cloned()
        .collect()
}

/// `skills_disabled` for the save request: sorted names, only when the stage
/// differs from the seed.
pub(crate) fn skills_payload(stage: &SkillsStage, seed: &SkillsStage) -> Option<Vec<String>> {
    if stage == seed {
        None
    } else {
        Some(stage.disabled.iter().cloned().collect())
    }
}

/// What "Load default" stages: the root profile's provider and model plus the
/// tools and skills selections (D-19). Pure data; applying it writes nothing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct AppliedDefaults {
    pub provider: String,
    pub model: String,
    pub tools: ToolsStage,
    pub skills: SkillsStage,
}

/// Load default's provider/model rule (CR-01): a root value that is unset
/// (blank) has nothing to load, so the staged `current` value is kept rather
/// than replaced by an empty string the server would reject.
pub(crate) fn keep_when_unset(root: &str, current: &str) -> String {
    if root.trim().is_empty() {
        current.to_string()
    } else {
        root.to_string()
    }
}

/// Fold the root defaults into the working copy. A section whose catalog did
/// not load (`None`) keeps its current staged value, so Load default can never
/// invent state for data the panel never saw. Disabled toolsets, tools and
/// skills are intersected with what the target catalog actually has.
pub(crate) fn apply_root_defaults(
    defaults: &RootAgentDefaults,
    page: Option<&ToolsPageState>,
    current_tools: &ToolsStage,
    catalog: Option<&[ProfileSkillRow]>,
    current_skills: &SkillsStage,
) -> AppliedDefaults {
    let tools = match page {
        Some(page) => {
            let known_tools: BTreeSet<&str> = page
                .toolsets
                .iter()
                .filter(|g| matches!(g.kind, ToolsetKind::BuiltIn))
                .flat_map(|g| g.tools.iter().map(|t| t.name.as_str()))
                .collect();
            ToolsStage {
                toolsets: current_tools
                    .toolsets
                    .keys()
                    .map(|name| (name.clone(), !defaults.disabled_toolsets.contains(name)))
                    .collect(),
                tools_disabled: defaults
                    .tools_disabled
                    .iter()
                    .filter(|t| known_tools.contains(t.as_str()))
                    .cloned()
                    .collect(),
            }
        }
        None => current_tools.clone(),
    };
    let skills = match catalog {
        Some(rows) => SkillsStage {
            disabled: rows
                .iter()
                .filter(|r| defaults.skills_disabled.contains(&r.name))
                .map(|r| r.name.clone())
                .collect(),
        },
        None => current_skills.clone(),
    };
    AppliedDefaults {
        provider: defaults.provider.clone(),
        model: defaults.model.clone(),
        tools,
        skills,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tools_config_api::{
        ConfigScope, ToolAvailability, ToolCatalogEntry, ToolsetGroup,
    };

    fn entry(group: &str, name: &str, disabled: bool) -> ToolCatalogEntry {
        ToolCatalogEntry {
            name: name.to_string(),
            description: format!("{name} description"),
            toolset: group.to_string(),
            group: group.to_string(),
            disabled_override: disabled,
            availability: ToolAvailability::Available,
            origin: ToolsetKind::BuiltIn,
            runtime_dependency: None,
        }
    }

    fn group(name: &str, enabled: bool, tools: Vec<ToolCatalogEntry>) -> ToolsetGroup {
        ToolsetGroup {
            name: name.to_string(),
            kind: ToolsetKind::BuiltIn,
            enabled,
            tools,
        }
    }

    /// web (enabled; web_search + web_extract), memory (enabled), kanban (a
    /// non-writable sentinel group), mcp__github (an MCP group).
    fn page() -> ToolsPageState {
        ToolsPageState {
            gate_open: true,
            scope: ConfigScope::Profile("dev".to_string()),
            scope_label: "dev".to_string(),
            toolsets: vec![
                group(
                    "web",
                    true,
                    vec![
                        entry("web", "web_search", false),
                        entry("web", "web_extract", false),
                    ],
                ),
                group("memory", true, vec![entry("memory", "memory_add", false)]),
                group("kanban", true, vec![entry("kanban", "kanban_show", true)]),
                ToolsetGroup {
                    name: "mcp__github".to_string(),
                    kind: ToolsetKind::Mcp {
                        server: "github".to_string(),
                    },
                    enabled: true,
                    tools: vec![],
                },
            ],
            bulk_targets: vec!["memory".to_string(), "web".to_string()],
        }
    }

    #[test]
    fn chips_tools_stage_excludes_mcp_and_non_bulk_groups() {
        let stage = tools_stage_from_page(&page());
        let expected: BTreeMap<String, bool> =
            [("web".to_string(), true), ("memory".to_string(), true)].into();
        assert_eq!(stage.toolsets, expected);
    }

    #[test]
    fn chips_tools_stage_seeds_disabled_tools_from_overrides() {
        let mut p = page();
        p.toolsets[0].tools[1].disabled_override = true;
        let stage = tools_stage_from_page(&p);
        assert!(stage.tools_disabled.contains("web_extract"));
        assert!(!stage.tools_disabled.contains("web_search"));
        // A disabled tool in a non-writable group is kept, so a Save never
        // drops an entry the panel has no chip for.
        assert!(stage.tools_disabled.contains("kanban_show"));
    }

    #[test]
    fn chips_disable_toolset_flips_only_that_toolset() {
        let mut stage = tools_stage_from_page(&page());
        let before = stage.clone();
        assert!(disable_toolset(&mut stage, "web"));
        assert_eq!(stage.toolsets.get("web"), Some(&false));
        assert_eq!(stage.toolsets.get("memory"), before.toolsets.get("memory"));
        assert_eq!(stage.tools_disabled, before.tools_disabled);
        // Disabling twice, or an unknown name, changes nothing.
        assert!(!disable_toolset(&mut stage, "web"));
        assert!(!disable_toolset(&mut stage, "mcp__github"));
    }

    #[test]
    fn chips_enable_toolset_restores_it() {
        let seed = tools_stage_from_page(&page());
        let mut stage = seed.clone();
        assert!(disable_toolset(&mut stage, "web"));
        assert!(enable_toolset(&mut stage, "web"));
        assert_eq!(stage, seed);
        assert!(!enable_toolset(&mut stage, "web"), "already enabled");
    }

    #[test]
    fn chips_disable_tool_inside_enabled_toolset_adds_to_tools_disabled() {
        let mut stage = tools_stage_from_page(&page());
        assert!(disable_tool(&mut stage, "web_extract"));
        assert!(stage.tools_disabled.contains("web_extract"));
        assert!(!disable_tool(&mut stage, "web_extract"), "already disabled");
        assert_eq!(
            stage.toolsets.get("web"),
            Some(&true),
            "the toolset stays on"
        );
    }

    #[test]
    fn chips_enable_tool_removes_it() {
        let seed = tools_stage_from_page(&page());
        let mut stage = seed.clone();
        disable_tool(&mut stage, "web_extract");
        assert!(enable_tool(&mut stage, "web_extract"));
        assert_eq!(stage, seed);
        assert!(!enable_tool(&mut stage, "web_extract"));
    }

    #[test]
    fn chips_tools_payload_sends_only_changed_toolsets() {
        let seed = tools_stage_from_page(&page());
        let mut stage = seed.clone();
        assert_eq!(
            tools_payload(&stage, &seed),
            (None, None, None),
            "unchanged -> nothing"
        );
        disable_toolset(&mut stage, "web");
        assert_eq!(
            tools_payload(&stage, &seed),
            (Some(vec![("web".to_string(), false)]), None, None)
        );
    }

    #[test]
    fn chips_tools_payload_sends_only_the_disabled_delta() {
        let seed = tools_stage_from_page(&page());
        let mut stage = seed.clone();
        disable_tool(&mut stage, "web_extract");
        let (toolsets, disable, enable) = tools_payload(&stage, &seed);
        assert_eq!(toolsets, None);
        // WR-06: only the NEWLY disabled tool — the seed's `kanban_show` (a
        // non-writable group's entry) is not re-sent, so a tool disabled
        // elsewhere since the panel opened can never be reverted by a Save.
        assert_eq!(disable, Some(vec!["web_extract".to_string()]));
        assert_eq!(enable, None);
        enable_tool(&mut stage, "web_extract");
        assert_eq!(tools_payload(&stage, &seed), (None, None, None));
    }

    #[test]
    fn chips_tools_payload_sends_re_enabled_tools_in_the_enable_list() {
        let seed = tools_stage_from_page(&page());
        let mut stage = seed.clone();
        assert!(enable_tool(&mut stage, "kanban_show"));
        disable_tool(&mut stage, "web_extract");
        let (_, disable, enable) = tools_payload(&stage, &seed);
        assert_eq!(disable, Some(vec!["web_extract".to_string()]));
        assert_eq!(enable, Some(vec!["kanban_show".to_string()]));
    }

    #[test]
    fn chips_enabled_chips_and_selection_partition() {
        let mut stage = tools_stage_from_page(&page());
        let p = page();
        disable_toolset(&mut stage, "memory");
        disable_tool(&mut stage, "web_extract");

        let chips = toolset_chips(&stage, &p);
        assert_eq!(chips.len(), 1, "only web is still enabled");
        assert_eq!(chips[0].name, "web");
        let members: Vec<&str> = chips[0].tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            members,
            vec!["web_search"],
            "a disabled member is not a chip"
        );

        let rows = tool_selection(&stage, &p);
        assert_eq!(
            rows,
            vec![
                SelectionRow::Tool {
                    toolset: "web".to_string(),
                    name: "web_extract".to_string(),
                    description: "web_extract description".to_string(),
                },
                SelectionRow::Toolset {
                    name: "memory".to_string()
                },
            ]
        );
    }

    fn skill(name: &str, category: &str, enabled: bool) -> ProfileSkillRow {
        ProfileSkillRow {
            name: name.to_string(),
            category: category.to_string(),
            enabled_for_profile: enabled,
        }
    }

    fn catalog() -> Vec<ProfileSkillRow> {
        vec![
            skill("a", "bundled", true),
            skill("b", "bundled", false),
            skill("code-review", "coding", false),
        ]
    }

    #[test]
    fn stale_skill_names_are_dropped_from_the_stage() {
        let rows = vec![skill("a", "bundled", true), skill("b", "bundled", false)];
        let seed = skills_stage_from_rows(&rows);
        let expected: BTreeSet<String> = ["b".to_string()].into();
        assert_eq!(seed.disabled, expected);
        let mut stage = seed.clone();
        disable_skill(&mut stage, "a");
        let payload = skills_payload(&stage, &seed).expect("a changed stage is sent");
        let names: BTreeSet<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(
            payload.iter().all(|n| names.contains(n.as_str())),
            "the payload must never carry a name absent from the catalog rows: {payload:?}"
        );
    }

    #[test]
    fn stale_skill_root_defaults_intersected_with_catalog() {
        let rows = vec![skill("a", "bundled", true), skill("b", "bundled", true)];
        let defaults = RootAgentDefaults {
            skills_disabled: vec!["b".to_string(), "gone".to_string()],
            ..Default::default()
        };
        let applied = apply_root_defaults(
            &defaults,
            None,
            &ToolsStage::default(),
            Some(&rows),
            &SkillsStage::default(),
        );
        let expected: BTreeSet<String> = ["b".to_string()].into();
        assert_eq!(applied.skills.disabled, expected);
    }

    #[test]
    fn staging_load_default_overwrites_provider_model_tools_and_skills() {
        let p = page();
        let rows = catalog();
        let defaults = RootAgentDefaults {
            provider: "openrouter".to_string(),
            model: "some/model".to_string(),
            skills_disabled: vec!["a".to_string()],
            disabled_toolsets: vec!["web".to_string()],
            tools_disabled: vec!["web_extract".to_string(), "gone_tool".to_string()],
        };
        // The profile currently has the opposite of everything.
        let mut current_tools = tools_stage_from_page(&p);
        disable_toolset(&mut current_tools, "memory");
        let current_skills = skills_stage_from_rows(&rows);

        let applied = apply_root_defaults(
            &defaults,
            Some(&p),
            &current_tools,
            Some(&rows),
            &current_skills,
        );
        assert_eq!(applied.provider, "openrouter");
        assert_eq!(applied.model, "some/model");
        let toolsets: BTreeMap<String, bool> =
            [("web".to_string(), false), ("memory".to_string(), true)].into();
        assert_eq!(applied.tools.toolsets, toolsets);
        let tools: BTreeSet<String> = ["web_extract".to_string()].into();
        assert_eq!(
            applied.tools.tools_disabled, tools,
            "intersected with the profile's built-in tools"
        );
        let skills: BTreeSet<String> = ["a".to_string()].into();
        assert_eq!(applied.skills.disabled, skills);
    }

    #[test]
    fn staging_load_default_keeps_a_section_whose_catalog_did_not_load() {
        let defaults = RootAgentDefaults {
            provider: "p".to_string(),
            model: "m".to_string(),
            skills_disabled: vec!["a".to_string()],
            disabled_toolsets: vec!["web".to_string()],
            tools_disabled: vec!["web_extract".to_string()],
        };
        let mut current_tools = tools_stage_from_page(&page());
        disable_tool(&mut current_tools, "web_search");
        let mut current_skills = SkillsStage::default();
        disable_skill(&mut current_skills, "b");
        let applied = apply_root_defaults(&defaults, None, &current_tools, None, &current_skills);
        assert_eq!(applied.provider, "p");
        assert_eq!(applied.tools, current_tools);
        assert_eq!(applied.skills, current_skills);
    }

    #[test]
    fn staging_skills_payload_only_when_changed() {
        let rows = catalog();
        let seed = skills_stage_from_rows(&rows);
        assert_eq!(skills_payload(&seed, &seed), None, "unchanged -> nothing");
        let mut stage = seed.clone();
        disable_skill(&mut stage, "a");
        assert_eq!(
            skills_payload(&stage, &seed),
            Some(vec![
                "a".to_string(),
                "b".to_string(),
                "code-review".to_string()
            ])
        );
        enable_skill(&mut stage, "a");
        assert_eq!(skills_payload(&stage, &seed), None);
    }

    #[test]
    fn chips_skill_selection_filtered_by_search() {
        let rows = catalog();
        let stage = skills_stage_from_rows(&rows);
        assert_eq!(skill_chips(&stage, &rows), vec!["a".to_string()]);
        let all: Vec<String> = skill_selection(&stage, &rows, "")
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(all, vec!["b".to_string(), "code-review".to_string()]);
        // Name and category both match, case-insensitively.
        let by_name: Vec<String> = skill_selection(&stage, &rows, "COD")
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(by_name, vec!["code-review".to_string()]);
        let by_category: Vec<String> = skill_selection(&stage, &rows, "coding")
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(by_category, vec!["code-review".to_string()]);
        assert!(skill_selection(&stage, &rows, "zzz").is_empty());
    }

    #[test]
    fn chips_tool_selection_filtered_by_search() {
        let mut stage = tools_stage_from_page(&page());
        disable_toolset(&mut stage, "memory");
        disable_tool(&mut stage, "web_extract");
        let rows = tool_selection(&stage, &page());
        assert_eq!(tool_selection_filtered(&rows, "").len(), 2);
        // A tool matches on its description, a toolset on its name; the query
        // is case-insensitive.
        let by_desc = tool_selection_filtered(&rows, "WEB_EXTRACT DESC");
        assert_eq!(by_desc.len(), 1);
        assert!(matches!(&by_desc[0], SelectionRow::Tool { name, .. } if name == "web_extract"));
        let by_set = tool_selection_filtered(&rows, "memo");
        assert_eq!(by_set.len(), 1);
        assert!(matches!(&by_set[0], SelectionRow::Toolset { name } if name == "memory"));
        assert!(tool_selection_filtered(&rows, "zzz").is_empty());
    }
}
