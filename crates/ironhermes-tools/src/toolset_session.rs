//! Phase 25.2 Plan 15 (Gap Closure for UAT Issue 2 / Symptom 1):
//! Production `impl ToolsetSessionHandle` for the live REPL / Telegram /
//! single-shot binary. Closes the wireup that Phase 25 Plan 04 explicitly
//! deferred (see Phase 25-04 SUMMARY: "the actual wire-up of a
//! ToolRegistry-backed implementation onto CommandContext for the REPL's
//! live session belongs to a future plan"). That future plan is THIS plan.
//!
//! D-06 contract: enable/disable mutate ONLY the in-session ToolsConfig.
//! They MUST NOT call config_setter or write to config.yaml. Persistent
//! changes require the `hermes toolset` CLI subcommand.

use std::sync::Arc;

use ironhermes_core::commands::context::ToolsetSessionHandle;
use ironhermes_core::commands::toolset_display::{
    ToolsetRow, render_toolset_list, render_toolset_show,
};
use ironhermes_core::config::{ToolsConfig, ToolsetEntry};
use tokio::sync::RwLock as TokioRwLock;

use crate::registry::ToolRegistry;

/// Production-side `ToolsetSessionHandle` for the live REPL/Telegram/single-shot
/// binary. Holds the live `ToolRegistry` and the live `ToolsConfig` so that
/// enable/disable mutations are reflected immediately on the next LLM call
/// (the registry's `get_definitions()` reads `toolset_config` per call).
pub struct RegistryToolsetSession {
    registry: Arc<TokioRwLock<ToolRegistry>>,
    config: Arc<std::sync::Mutex<ToolsConfig>>,
}

impl RegistryToolsetSession {
    /// Construct a new session handle from the live `ToolRegistry` and the
    /// initial `ToolsConfig` (typically `config.tools` at session start).
    ///
    /// The config is wrapped in `Arc<Mutex<_>>` so enable/disable mutations
    /// are visible to subsequent `set_toolset_config` calls without requiring
    /// a registry rebuild.
    pub fn new(registry: Arc<TokioRwLock<ToolRegistry>>, initial_config: ToolsConfig) -> Self {
        Self {
            registry,
            config: Arc::new(std::sync::Mutex::new(initial_config)),
        }
    }

    /// D-01 member map (toolset -> member tool names). Local copy of the CLI
    /// subcommand's `toolset_members_map()` (see crates/ironhermes-cli/src/toolset_cmd.rs:239)
    /// so this crate stays leaf w.r.t. the CLI crate. Keep these two in sync —
    /// Plan 15 Task 3 simultaneously updates the CLI map to add web_extract.
    fn members_map() -> std::collections::HashMap<&'static str, &'static [&'static str]> {
        let mut m: std::collections::HashMap<&'static str, &'static [&'static str]> =
            std::collections::HashMap::new();
        m.insert("web", &["web_search", "web_read", "web_extract"]);
        m.insert(
            "code",
            &[
                "execute_code",
                "terminal",
                "read_file",
                "write_file",
                "list_dir",
                "grep_files",
            ],
        );
        m.insert("memory", &["memory"]);
        m.insert("agent", &["delegate_task", "cronjob"]);
        m.insert("skills", &["skills"]);
        m.insert("session", &["session_search"]);
        m.insert(
            "browser",
            &[
                "browser_back",
                "browser_click",
                "browser_close",
                "browser_console",
                "browser_get_images",
                "browser_navigate",
                "browser_press",
                "browser_scroll",
                "browser_snapshot",
                "browser_type",
                "browser_vision",
            ],
        );
        m
    }

    /// Validate a toolset name against `^[a-z][a-z0-9_]{0,31}$` (Phase 25 T-25-01).
    /// Mirrors `validate_toolset_name` in toolset_cmd.rs but kept local to this crate.
    fn validate_name(name: &str) -> Result<String, String> {
        if name.is_empty() {
            return Err("toolset name must not be empty".to_string());
        }
        let mut chars = name.chars();
        if let Some(first) = chars.next() {
            if !first.is_ascii_lowercase() {
                return Err(format!(
                    "invalid toolset name '{}': must start with a-z",
                    name
                ));
            }
        }
        for c in chars {
            if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                return Err(format!(
                    "invalid toolset name '{}': must match [a-z0-9_]",
                    name
                ));
            }
        }
        if name.len() > 32 {
            return Err(format!(
                "invalid toolset name '{}': must be <=32 chars",
                name
            ));
        }
        Ok(name.to_string())
    }

    /// Check that the validated name is one of the known D-01 toolsets.
    fn check_known(name: &str) -> Result<(), String> {
        let known = Self::members_map();
        if !known.contains_key(name) {
            let names: Vec<&&str> = known.keys().collect();
            return Err(format!("unknown toolset '{}'. Known: {:?}", name, names));
        }
        Ok(())
    }

    /// Push the current config into the registry so the next `get_definitions()`
    /// call reflects the mutation. Async work is bridged via `block_in_place +
    /// block_on` because the slash handler is sync.
    fn push_config_to_registry(&self) {
        let cfg_snapshot = self.config.lock().unwrap().clone();
        let registry = self.registry.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let mut guard = registry.write().await;
                guard.set_toolset_config(Some(cfg_snapshot));
            });
        });
    }

    /// Build the per-toolset display rows for `render_list`. Mirrors
    /// `build_toolset_rows` in toolset_cmd.rs but uses the local registry
    /// snapshot directly.
    fn build_rows(&self) -> Vec<ToolsetRow> {
        let cfg = self.config.lock().unwrap().clone();
        let registry = self.registry.clone();
        let unavailable_list = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let guard = registry.read().await;
                guard.list_unavailable()
            })
        });
        let unavailable_names: std::collections::HashSet<String> = unavailable_list
            .iter()
            .map(|(name, _)| name.clone())
            .collect();

        let members_map = Self::members_map();
        // Stable sort order: alphabetical toolset name (matches CLI behavior).
        let mut names: Vec<&'static str> = members_map.keys().copied().collect();
        names.sort_unstable();
        names
            .into_iter()
            .map(|ts_name| {
                let member_names = members_map.get(ts_name).copied().unwrap_or(&[]);
                let enabled = cfg.is_toolset_enabled(ts_name);
                let available_count = member_names
                    .iter()
                    .filter(|n| !unavailable_names.contains(**n))
                    .count();
                let member_summary = member_names
                    .iter()
                    .map(|n| {
                        if unavailable_names.contains(*n) {
                            format!("{} \u{2717}", n)
                        } else {
                            format!("{} \u{2713}", n)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                ToolsetRow {
                    name: ts_name.to_string(),
                    enabled,
                    member_count: member_names.len(),
                    available_count,
                    member_summary,
                }
            })
            .collect()
    }
}

impl ToolsetSessionHandle for RegistryToolsetSession {
    fn enable_toolset(&self, name: &str) -> Result<(), String> {
        let validated = Self::validate_name(name)?;
        Self::check_known(&validated)?;
        {
            let mut cfg = self.config.lock().unwrap();
            cfg.toolsets
                .entry(validated.clone())
                .and_modify(|e| e.enabled = true)
                .or_insert(ToolsetEntry { enabled: true });
        }
        self.push_config_to_registry();
        Ok(())
    }

    fn disable_toolset(&self, name: &str) -> Result<(), String> {
        let validated = Self::validate_name(name)?;
        Self::check_known(&validated)?;
        {
            let mut cfg = self.config.lock().unwrap();
            cfg.toolsets
                .entry(validated.clone())
                .and_modify(|e| e.enabled = false)
                .or_insert(ToolsetEntry { enabled: false });
        }
        self.push_config_to_registry();
        Ok(())
    }

    fn render_list(&self) -> String {
        render_toolset_list(self.build_rows())
    }

    fn render_show(&self, name: &str) -> Result<String, String> {
        let validated = Self::validate_name(name)?;
        Self::check_known(&validated)?;

        let cfg = self.config.lock().unwrap().clone();
        let registry = self.registry.clone();
        let unavailable_list = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let guard = registry.read().await;
                guard.list_unavailable()
            })
        });
        let unavailable_names: std::collections::HashSet<String> = unavailable_list
            .iter()
            .map(|(name, _)| name.clone())
            .collect();

        let members_map = Self::members_map();
        let member_names = members_map.get(validated.as_str()).copied().unwrap_or(&[]);

        let members: Vec<(String, bool, String)> = member_names
            .iter()
            .map(|&tool_name| {
                let avail = !unavailable_names.contains(tool_name);
                let prereq_str = if avail {
                    String::new()
                } else {
                    unavailable_list
                        .iter()
                        .find(|(n, _)| n == tool_name)
                        .map(|(_, prereqs)| {
                            prereqs
                                .iter()
                                .filter(|p| p.required)
                                .map(|p| p.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default()
                };
                (tool_name.to_string(), avail, prereq_str)
            })
            .collect();

        let row = ToolsetRow {
            name: validated.clone(),
            enabled: cfg.is_toolset_enabled(&validated),
            member_count: member_names.len(),
            available_count: members.iter().filter(|(_, a, _)| *a).count(),
            member_summary: String::new(),
        };

        Ok(render_toolset_show(&row, &members))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_handle() -> RegistryToolsetSession {
        let registry = Arc::new(TokioRwLock::new(ToolRegistry::new()));
        let cfg = ToolsConfig::default();
        RegistryToolsetSession::new(registry, cfg)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn enable_then_disable_round_trip() {
        let h = fresh_handle();
        // web is disabled by default (Phase 25.1 D-04); enable it.
        assert!(h.enable_toolset("web").is_ok());
        // Now render_show should report enabled=true for web.
        let s = h.render_show("web").unwrap();
        assert!(s.to_lowercase().contains("web"));
        // Disable round-trip.
        assert!(h.disable_toolset("web").is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn render_list_returns_non_empty() {
        let h = fresh_handle();
        let s = h.render_list();
        // Should NOT be the deferred fallback string.
        assert!(!s.contains("toolset session handle not configured"));
        // Should mention multiple toolsets.
        assert!(s.contains("web") && s.contains("code") && s.contains("memory"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn render_show_reports_web_extract_in_web_toolset() {
        let h = fresh_handle();
        let s = h.render_show("web").expect("web is a known toolset");
        // GAP-15 invariant: web_extract MUST appear in the web toolset's
        // member list now that Phase 25.2 ships it (Plan 15 Task 3 keeps
        // the CLI map and this map in sync).
        assert!(
            s.contains("web_extract"),
            "render_show('web') must list web_extract; got:\n{}",
            s
        );
        assert!(s.contains("web_search"));
        assert!(s.contains("web_read"));
    }

    #[test]
    fn validate_name_rejects_uppercase_and_dots() {
        assert!(RegistryToolsetSession::validate_name("Web").is_err());
        assert!(RegistryToolsetSession::validate_name("web.tools").is_err());
        assert!(RegistryToolsetSession::validate_name("").is_err());
        assert!(RegistryToolsetSession::validate_name("web").is_ok());
    }

    #[test]
    fn check_known_rejects_unknown_toolset() {
        assert!(RegistryToolsetSession::check_known("not_a_real_toolset").is_err());
        assert!(RegistryToolsetSession::check_known("web").is_ok());
    }
}
