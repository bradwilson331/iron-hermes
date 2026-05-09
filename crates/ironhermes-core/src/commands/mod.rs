use std::collections::HashMap;

use crate::types::Platform;

pub mod context;
pub mod handlers;
pub mod provider_display;
pub mod registry;
pub mod toolset_display;
pub mod typo;

// =============================================================================
// CommandCategory
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandCategory {
    Session,
    Configuration,
    ToolsAndSkills,
    Info,
    Exit,
}

// =============================================================================
// PlatformFilter
// =============================================================================

/// Which platforms a command is available on.
/// Maps to hermes-agent's cli_only / gateway_only booleans but extends to ACP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformFilter {
    /// Available on all platforms (CLI, gateway, ACP)
    All,
    /// CLI (Platform::Local) only
    CliOnly,
    /// CLI (Local) + ACP (ApiServer)
    CliAndAcp,
    /// Gateway/messaging only — any platform that is NOT Local and NOT ApiServer
    GatewayOnly,
    /// Universal: CLI + gateway (NOT ACP/ApiServer)
    Universal,
}

impl PlatformFilter {
    pub fn is_available_on(&self, platform: &Platform) -> bool {
        match self {
            PlatformFilter::All => true,
            PlatformFilter::CliOnly => *platform == Platform::Local,
            PlatformFilter::CliAndAcp => {
                matches!(platform, Platform::Local | Platform::ApiServer)
            }
            PlatformFilter::GatewayOnly => {
                !matches!(platform, Platform::Local | Platform::ApiServer)
            }
            PlatformFilter::Universal => *platform != Platform::ApiServer,
        }
    }
}

// =============================================================================
// CommandDef
// =============================================================================

#[derive(Debug, Clone)]
pub struct CommandDef {
    pub name: &'static str,
    pub description: &'static str,
    pub category: CommandCategory,
    pub aliases: &'static [&'static str],
    pub args_hint: &'static str,
    pub platform_filter: PlatformFilter,
}

impl CommandDef {
    pub fn new(name: &'static str, description: &'static str, category: CommandCategory) -> Self {
        Self {
            name,
            description,
            category,
            aliases: &[],
            args_hint: "",
            platform_filter: PlatformFilter::All,
        }
    }

    pub fn aliases(mut self, a: &'static [&'static str]) -> Self {
        self.aliases = a;
        self
    }

    pub fn args_hint(mut self, h: &'static str) -> Self {
        self.args_hint = h;
        self
    }

    pub fn platform(mut self, p: PlatformFilter) -> Self {
        self.platform_filter = p;
        self
    }

    pub fn is_available_on(&self, platform: &Platform) -> bool {
        self.platform_filter.is_available_on(platform)
    }
}

// =============================================================================
// ResolveResult
// =============================================================================

#[derive(Debug)]
pub enum ResolveResult<'a> {
    /// Exact name or alias match found and available on platform
    Exact(&'a CommandDef),
    /// Unique prefix match found
    PrefixMatch(&'a CommandDef),
    /// Multiple candidates match the prefix — caller should show these to user
    Ambiguous(Vec<&'static str>),
    /// No built-in match (caller checks skills)
    NotFound,
}

// =============================================================================
// CommandResult
// =============================================================================

/// Core router result type. Distinct from TUI CommandResult.
/// CLI and gateway adapters map this to their own result types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResult {
    /// Display this text to the user
    Output(String),
    /// Command handled silently (no output needed)
    Handled,
    /// Error message to display
    Error(String),
    /// Exit the application (CLI only)
    Quit,
    /// Clear session history
    ClearSession,
    /// Phase 22.3 D-06 / UI-SPEC CLR-8: TTY visual reset (scrollback wipe +
    /// DECSTBM re-anchor + prompt re-anchor). Does NOT mutate `messages` —
    /// that is `ClearSession`/`NewSession` semantics for `/new`. The REPL
    /// loop in `main.rs` matches this variant and calls
    /// `tui::render::reset_terminal_visual(reserved_row_count)`.
    ResetTerminal,
    /// Start a new session, display message
    NewSession { message: String },
    /// Not a built-in command; pass input to agent as normal message
    PassThrough,
    /// Request the caller to perform an MCP reload (async operation).
    /// The caller (REPL loop) dispatches to McpReloader and formats the output
    /// per UI-SPEC. Returned when `ctx.mcp_reloader` is Some; the handler
    /// returns `Output("MCP not configured.")` when it is None.
    McpReload,

    /// Phase 21.8.2: request the caller to reload the skill registry (synchronous operation).
    /// The caller (REPL loop / gateway handler) calls `SkillRegistry::load_with_config`,
    /// computes the diff of added/removed skill names, swaps the local Arc, and formats
    /// the D-04 diff output. Returned by `cmd_skills` when args[0] == "reload" and
    /// `ctx.skill_registry` is Some; otherwise the handler returns
    /// `Output("Skills not configured.")`.
    SkillsReload,

    /// Phase 21.8.2 SKILL-13: a dynamic skill was activated via the CommandRouter
    /// fallback at the dispatch call site. The caller prepends `body` to the system
    /// prompt for the next LLM turn (D-07). `name` is the normalized skill name
    /// (kebab-case); `body` is the full SKILL.md body text returned by
    /// `SkillRegistry::read_content`.
    SkillActivated { name: String, body: String },
}

// =============================================================================
// CommandRouter
// =============================================================================

pub struct CommandRouter {
    pub commands: Vec<CommandDef>,
    /// command name -> index into commands
    by_name: HashMap<&'static str, usize>,
    /// alias -> index into commands
    by_alias: HashMap<&'static str, usize>,
}

impl CommandRouter {
    /// Build a new router from a list of command definitions.
    /// Panics on duplicate name or alias (programming error in registry).
    pub fn new(commands: Vec<CommandDef>) -> Self {
        let mut by_name = HashMap::new();
        let mut by_alias = HashMap::new();

        for (idx, cmd) in commands.iter().enumerate() {
            if by_name.insert(cmd.name, idx).is_some() {
                panic!("Duplicate command name in registry: '{}'", cmd.name);
            }
            for alias in cmd.aliases {
                if by_name.contains_key(*alias) {
                    panic!(
                        "Alias '{}' for command '{}' conflicts with an existing command name",
                        alias, cmd.name
                    );
                }
                if by_alias.insert(*alias, idx).is_some() {
                    panic!("Duplicate alias '{}' for command '{}'", alias, cmd.name);
                }
            }
        }

        Self {
            commands,
            by_name,
            by_alias,
        }
    }

    /// Three-stage command resolution: exact -> alias -> prefix.
    /// Filters by platform availability at every stage.
    pub fn resolve<'a>(&'a self, input: &str, platform: &Platform) -> ResolveResult<'a> {
        let name = input
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();

        if name.is_empty() {
            return ResolveResult::NotFound;
        }

        // Stage 1: exact name match
        if let Some(&idx) = self.by_name.get(name.as_str()) {
            let cmd = &self.commands[idx];
            if cmd.is_available_on(platform) {
                return ResolveResult::Exact(cmd);
            }
        }

        // Stage 2: alias match
        if let Some(&idx) = self.by_alias.get(name.as_str()) {
            let cmd = &self.commands[idx];
            if cmd.is_available_on(platform) {
                return ResolveResult::Exact(cmd);
            }
        }

        // Stage 3: prefix match (platform-filtered)
        // A command matches if its name OR any of its aliases starts with the input prefix
        let candidates: Vec<&CommandDef> = self
            .commands
            .iter()
            .filter(|c| c.is_available_on(platform))
            .filter(|c| {
                c.name.starts_with(name.as_str())
                    || c.aliases.iter().any(|a| a.starts_with(name.as_str()))
            })
            .collect();

        match candidates.len() {
            0 => ResolveResult::NotFound,
            1 => ResolveResult::PrefixMatch(candidates[0]),
            _ => {
                // Shortest-match preference (hermes-agent behavior):
                // If one candidate has the shortest name, prefer it unambiguously.
                let min_len = candidates.iter().map(|c| c.name.len()).min().unwrap();
                let shortest: Vec<_> = candidates
                    .iter()
                    .filter(|c| c.name.len() == min_len)
                    .collect();
                if shortest.len() == 1 {
                    return ResolveResult::PrefixMatch(shortest[0]);
                }
                // Still ambiguous — return sorted candidate names
                let mut names: Vec<&'static str> = candidates.iter().map(|c| c.name).collect();
                names.sort_unstable();
                ResolveResult::Ambiguous(names)
            }
        }
    }

    /// All commands available on a given platform, ordered by category then name.
    pub fn commands_for_platform<'a>(&'a self, platform: &Platform) -> Vec<&'a CommandDef> {
        let category_order = |cat: &CommandCategory| match cat {
            CommandCategory::Session => 0,
            CommandCategory::Configuration => 1,
            CommandCategory::ToolsAndSkills => 2,
            CommandCategory::Info => 3,
            CommandCategory::Exit => 4,
        };

        let mut cmds: Vec<&CommandDef> = self
            .commands
            .iter()
            .filter(|c| c.is_available_on(platform))
            .collect();

        cmds.sort_by(|a, b| {
            category_order(&a.category)
                .cmp(&category_order(&b.category))
                .then(a.name.cmp(b.name))
        });

        cmds
    }

    /// Commands grouped by category, for /help output.
    /// Returns (category, commands) pairs in category order.
    pub fn commands_by_category<'a>(
        &'a self,
        platform: &Platform,
    ) -> Vec<(CommandCategory, Vec<&'a CommandDef>)> {
        let categories = [
            CommandCategory::Session,
            CommandCategory::Configuration,
            CommandCategory::ToolsAndSkills,
            CommandCategory::Info,
            CommandCategory::Exit,
        ];

        let mut result = Vec::new();
        for cat in &categories {
            let mut cmds: Vec<&CommandDef> = self
                .commands
                .iter()
                .filter(|c| c.is_available_on(platform) && &c.category == cat)
                .collect();
            cmds.sort_by_key(|c| c.name);
            if !cmds.is_empty() {
                result.push((cat.clone(), cmds));
            }
        }
        result
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::registry::build_registry;
    use crate::types::Platform;

    // ---------------------------------------------------------------------------
    // CommandDef builder tests
    // ---------------------------------------------------------------------------

    #[test]
    fn commanddef_new_creates_correct_struct() {
        let def = CommandDef::new("help", "Show help", CommandCategory::Info)
            .platform(PlatformFilter::All);
        assert_eq!(def.name, "help");
        assert_eq!(def.description, "Show help");
        assert!(matches!(def.category, CommandCategory::Info));
        assert!(def.aliases.is_empty());
        assert_eq!(def.args_hint, "");
    }

    #[test]
    fn commanddef_builder_populates_all_fields() {
        let def = CommandDef::new("new", "Start new session", CommandCategory::Session)
            .aliases(&["reset"])
            .args_hint("[name]")
            .platform(PlatformFilter::Universal);
        assert_eq!(def.name, "new");
        assert_eq!(def.aliases, &["reset"]);
        assert_eq!(def.args_hint, "[name]");
        assert!(matches!(def.platform_filter, PlatformFilter::Universal));
    }

    // ---------------------------------------------------------------------------
    // PlatformFilter tests
    // ---------------------------------------------------------------------------

    #[test]
    fn platform_filter_all_is_always_true() {
        assert!(PlatformFilter::All.is_available_on(&Platform::Local));
        assert!(PlatformFilter::All.is_available_on(&Platform::Telegram));
        assert!(PlatformFilter::All.is_available_on(&Platform::ApiServer));
    }

    #[test]
    fn platform_filter_cli_only_local_true() {
        assert!(PlatformFilter::CliOnly.is_available_on(&Platform::Local));
    }

    #[test]
    fn platform_filter_cli_only_telegram_false() {
        assert!(!PlatformFilter::CliOnly.is_available_on(&Platform::Telegram));
    }

    #[test]
    fn platform_filter_gateway_only_telegram_true() {
        assert!(PlatformFilter::GatewayOnly.is_available_on(&Platform::Telegram));
    }

    #[test]
    fn platform_filter_gateway_only_local_false() {
        assert!(!PlatformFilter::GatewayOnly.is_available_on(&Platform::Local));
    }

    #[test]
    fn platform_filter_cli_and_acp_local_true() {
        assert!(PlatformFilter::CliAndAcp.is_available_on(&Platform::Local));
    }

    #[test]
    fn platform_filter_cli_and_acp_api_server_true() {
        assert!(PlatformFilter::CliAndAcp.is_available_on(&Platform::ApiServer));
    }

    #[test]
    fn platform_filter_cli_and_acp_telegram_false() {
        assert!(!PlatformFilter::CliAndAcp.is_available_on(&Platform::Telegram));
    }

    #[test]
    fn platform_filter_universal_local_true() {
        assert!(PlatformFilter::Universal.is_available_on(&Platform::Local));
    }

    #[test]
    fn platform_filter_universal_telegram_true() {
        assert!(PlatformFilter::Universal.is_available_on(&Platform::Telegram));
    }

    #[test]
    fn platform_filter_universal_api_server_false() {
        assert!(!PlatformFilter::Universal.is_available_on(&Platform::ApiServer));
    }

    // ---------------------------------------------------------------------------
    // CommandCategory variants
    // ---------------------------------------------------------------------------

    #[test]
    fn command_category_has_five_variants() {
        let _session = CommandCategory::Session;
        let _config = CommandCategory::Configuration;
        let _tools = CommandCategory::ToolsAndSkills;
        let _info = CommandCategory::Info;
        let _exit = CommandCategory::Exit;
    }

    // ---------------------------------------------------------------------------
    // CommandResult variants
    // ---------------------------------------------------------------------------

    #[test]
    fn command_result_has_all_variants() {
        let _output = CommandResult::Output("test".to_string());
        let _handled = CommandResult::Handled;
        let _error = CommandResult::Error("err".to_string());
        let _quit = CommandResult::Quit;
        let _clear = CommandResult::ClearSession;
        let _reset = CommandResult::ResetTerminal;
        let _new = CommandResult::NewSession {
            message: "msg".to_string(),
        };
        let _pass = CommandResult::PassThrough;
    }

    // ---------------------------------------------------------------------------
    // resolve() tests using a small fixture registry
    // ---------------------------------------------------------------------------

    fn make_test_router() -> CommandRouter {
        use CommandCategory::*;
        use PlatformFilter::*;
        CommandRouter::new(vec![
            CommandDef::new("help", "Show help", Info).platform(All),
            CommandDef::new("new", "New session", Session)
                .aliases(&["reset"])
                .platform(Universal),
            CommandDef::new("clear", "Clear screen", Session).platform(CliOnly),
            CommandDef::new("stop", "Stop agent", Session).platform(Universal),
            CommandDef::new("status", "Show status", Session).platform(Universal),
            CommandDef::new("statusbar", "Toggle status bar", Configuration)
                .aliases(&["sb"])
                .platform(CliOnly),
            CommandDef::new("approve", "Approve command", Session).platform(GatewayOnly),
            CommandDef::new("quit", "Exit", Exit)
                .aliases(&["exit", "q"])
                .platform(CliOnly),
        ])
    }

    #[test]
    fn resolve_exact_help_on_local() {
        let router = make_test_router();
        let result = router.resolve("help", &Platform::Local);
        assert!(matches!(result, ResolveResult::Exact(cmd) if cmd.name == "help"));
    }

    #[test]
    fn resolve_exact_with_slash_prefix() {
        let router = make_test_router();
        let result = router.resolve("/help", &Platform::Local);
        assert!(matches!(result, ResolveResult::Exact(cmd) if cmd.name == "help"));
    }

    #[test]
    fn resolve_alias_reset_to_new() {
        let router = make_test_router();
        let result = router.resolve("reset", &Platform::Local);
        assert!(matches!(result, ResolveResult::Exact(cmd) if cmd.name == "new"));
    }

    #[test]
    fn resolve_prefix_hel_to_help() {
        let router = make_test_router();
        let result = router.resolve("hel", &Platform::Local);
        assert!(matches!(result, ResolveResult::PrefixMatch(cmd) if cmd.name == "help"));
    }

    #[test]
    fn resolve_prefix_st_is_ambiguous() {
        let router = make_test_router();
        let result = router.resolve("st", &Platform::Local);
        // "stop", "status", "statusbar" all start with "st" on Local
        match result {
            ResolveResult::Ambiguous(names) => {
                assert!(names.contains(&"stop") || names.contains(&"status"));
            }
            // If shortest-match preference fires and a single shortest wins, that's also valid.
            ResolveResult::PrefixMatch(_) => {}
            other => panic!("Expected Ambiguous or PrefixMatch, got {:?}", other),
        }
    }

    #[test]
    fn resolve_not_found_for_unknown() {
        let router = make_test_router();
        let result = router.resolve("zzz", &Platform::Local);
        assert!(matches!(result, ResolveResult::NotFound));
    }

    #[test]
    fn resolve_cli_only_command_not_found_on_telegram() {
        // "clear" is CliOnly — should return NotFound on Telegram
        let router = make_test_router();
        let result = router.resolve("clear", &Platform::Telegram);
        assert!(matches!(result, ResolveResult::NotFound));
    }

    #[test]
    fn resolve_gateway_only_command_not_found_on_local() {
        // "approve" is GatewayOnly — should return NotFound on Local
        let router = make_test_router();
        let result = router.resolve("approve", &Platform::Local);
        assert!(matches!(result, ResolveResult::NotFound));
    }

    // ---------------------------------------------------------------------------
    // Registry completeness tests (using real build_registry())
    // ---------------------------------------------------------------------------

    #[test]
    fn registry_has_at_least_44_commands() {
        assert!(build_registry().len() >= 44);
    }

    #[test]
    fn registry_no_duplicate_names() {
        let cmds = build_registry();
        let names: std::collections::HashSet<_> = cmds.iter().map(|c| c.name).collect();
        assert_eq!(names.len(), cmds.len(), "Duplicate command names found");
    }

    #[test]
    fn registry_no_duplicate_aliases() {
        let cmds = build_registry();
        let all_names: std::collections::HashSet<_> = cmds.iter().map(|c| c.name).collect();
        let mut seen_aliases = std::collections::HashSet::new();
        for cmd in &cmds {
            for alias in cmd.aliases {
                assert!(
                    !all_names.contains(*alias),
                    "Alias '{}' of '{}' conflicts with a command name",
                    alias,
                    cmd.name
                );
                assert!(
                    seen_aliases.insert(*alias),
                    "Duplicate alias '{}' in registry",
                    alias
                );
            }
        }
    }

    #[test]
    fn registry_all_have_descriptions() {
        for cmd in build_registry() {
            assert!(
                !cmd.description.is_empty(),
                "Command '{}' has empty description",
                cmd.name
            );
        }
    }

    #[test]
    fn registry_router_construction_succeeds() {
        // Should not panic
        let _router = CommandRouter::new(build_registry());
    }

    // ---------------------------------------------------------------------------
    // Resolution edge cases with real registry
    // ---------------------------------------------------------------------------

    fn real_router() -> CommandRouter {
        CommandRouter::new(build_registry())
    }

    #[test]
    fn resolve_s_on_cli_is_ambiguous_or_shortest_match() {
        let router = real_router();
        // Many commands start with "s" on Local; at minimum stop, status, statusbar, skills, skin, save, snapshot, sethome
        let result = router.resolve("s", &Platform::Local);
        match result {
            ResolveResult::Ambiguous(names) => {
                assert!(names.len() >= 2, "Expected at least 2 ambiguous candidates");
            }
            ResolveResult::PrefixMatch(_) => {} // shortest match won
            other => panic!("Expected Ambiguous or PrefixMatch for 's', got {:?}", other),
        }
    }

    #[test]
    fn resolve_he_on_cli_is_help() {
        let router = real_router();
        let result = router.resolve("he", &Platform::Local);
        assert!(
            matches!(result, ResolveResult::Exact(cmd) | ResolveResult::PrefixMatch(cmd) if cmd.name == "help"),
            "Expected 'help' for prefix 'he'"
        );
    }

    #[test]
    fn resolve_mod_resolves_model() {
        let router = real_router();
        // On any platform where model is available
        let result = router.resolve("mod", &Platform::Local);
        assert!(
            matches!(result, ResolveResult::Exact(cmd) | ResolveResult::PrefixMatch(cmd) if cmd.name == "model"),
            "Expected 'model' for prefix 'mod'"
        );
    }

    #[test]
    fn resolve_sb_alias_statusbar() {
        let router = real_router();
        let result = router.resolve("sb", &Platform::Local);
        assert!(
            matches!(result, ResolveResult::Exact(cmd) if cmd.name == "statusbar"),
            "Expected 'statusbar' via alias 'sb'"
        );
    }

    #[test]
    fn resolve_set_home_alias() {
        let router = real_router();
        let result = router.resolve("set-home", &Platform::Telegram);
        assert!(
            matches!(result, ResolveResult::Exact(cmd) if cmd.name == "sethome"),
            "Expected 'sethome' via alias 'set-home' on Telegram"
        );
    }

    #[test]
    fn resolve_bg_alias() {
        let router = real_router();
        let result = router.resolve("bg", &Platform::Local);
        assert!(
            matches!(result, ResolveResult::Exact(cmd) if cmd.name == "background"),
            "Expected 'background' via alias 'bg'"
        );
    }

    #[test]
    fn resolve_exact_over_prefix() {
        let router = real_router();
        // "/new" exact, not a prefix of another command
        let result = router.resolve("new", &Platform::Local);
        assert!(
            matches!(result, ResolveResult::Exact(cmd) if cmd.name == "new"),
            "Expected exact match for 'new'"
        );
    }

    #[test]
    fn resolve_alias_over_prefix() {
        let router = real_router();
        // "/reset" alias of "new" — should not resolve as prefix of "resume"/"reasoning"/"reload"
        let result = router.resolve("reset", &Platform::Local);
        assert!(
            matches!(result, ResolveResult::Exact(cmd) if cmd.name == "new"),
            "Expected alias 'reset' -> 'new', not a prefix match"
        );
    }

    #[test]
    fn commands_for_platform_cli_includes_quit_excludes_approve() {
        let router = real_router();
        let cmds = router.commands_for_platform(&Platform::Local);
        let names: Vec<_> = cmds.iter().map(|c| c.name).collect();
        assert!(
            names.contains(&"quit"),
            "Expected 'quit' for Local platform"
        );
        assert!(
            !names.contains(&"approve"),
            "'approve' should not be on Local platform"
        );
    }

    #[test]
    fn commands_for_platform_gateway_includes_approve_excludes_quit() {
        let router = real_router();
        let cmds = router.commands_for_platform(&Platform::Telegram);
        let names: Vec<_> = cmds.iter().map(|c| c.name).collect();
        assert!(
            names.contains(&"approve"),
            "Expected 'approve' for Telegram platform"
        );
        assert!(
            !names.contains(&"quit"),
            "'quit' should not be on Telegram platform"
        );
    }

    #[test]
    fn commands_by_category_returns_grouped_results() {
        let router = real_router();
        let groups = router.commands_by_category(&Platform::Local);
        assert!(!groups.is_empty());
        // Session should be first
        assert!(matches!(groups[0].0, CommandCategory::Session));
    }

    // ---------------------------------------------------------------------------
    // Task 3 additional edge case tests
    // ---------------------------------------------------------------------------

    #[test]
    fn resolve_com_on_cli_is_ambiguous_or_prefix() {
        // On CLI: both "compress" (Universal) and "config" (CliOnly) start with "com"
        let router = real_router();
        let result = router.resolve("com", &Platform::Local);
        // Both match — should be ambiguous (or shortest if one wins)
        match result {
            ResolveResult::Ambiguous(names) => {
                assert!(
                    names.contains(&"compress") || names.contains(&"config"),
                    "Expected compress or config in ambiguous list"
                );
            }
            ResolveResult::PrefixMatch(cmd) => {
                // shortest wins — both "config" and "compress" are 7/8 chars,
                // "config" (6) < "compress" (8) so config may win
                assert!(
                    cmd.name == "config" || cmd.name == "compress" || cmd.name == "commands",
                    "Unexpected prefix match: {}",
                    cmd.name
                );
            }
            other => panic!("Unexpected result for 'com' on CLI: {:?}", other),
        }
    }

    #[test]
    fn resolve_com_on_gateway_includes_commands() {
        // On gateway: "commands" (GatewayOnly) is available, "config" (CliOnly) is not
        let router = real_router();
        let result = router.resolve("com", &Platform::Telegram);
        match result {
            ResolveResult::Exact(cmd) | ResolveResult::PrefixMatch(cmd) => {
                // On Telegram, "config" and "tools"-type CLI commands are gone.
                // "commands" and "compress" are available.
                assert!(
                    cmd.name == "commands" || cmd.name == "compress",
                    "Unexpected match: {}",
                    cmd.name
                );
            }
            ResolveResult::Ambiguous(names) => {
                // Both commands and compress available
                assert!(names.iter().any(|n| *n == "commands" || *n == "compress"));
            }
            other => panic!("Unexpected result for 'com' on gateway: {:?}", other),
        }
    }

    // ---------------------------------------------------------------------------
    // Phase 21.8.2: SkillsReload + SkillActivated variant tests
    // ---------------------------------------------------------------------------

    #[test]
    fn command_result_skillsreload_variant_exists() {
        let r = CommandResult::SkillsReload;
        assert_eq!(r, CommandResult::SkillsReload);
    }

    #[test]
    fn command_result_skillactivated_variant_exists() {
        let r = CommandResult::SkillActivated {
            name: "ascii-art".to_string(),
            body: "skill body".to_string(),
        };
        assert_eq!(
            r,
            CommandResult::SkillActivated {
                name: "ascii-art".to_string(),
                body: "skill body".to_string(),
            }
        );
    }
}
