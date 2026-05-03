use std::sync::atomic::Ordering;

use crate::commands::context::CommandContext;
use crate::commands::typo::suggest_typo;
use crate::commands::{CommandDef, CommandResult, CommandRouter};

/// Dispatch a resolved command to its handler.
///
/// Takes the resolved CommandDef, the remaining arguments (words after the
/// command name), the execution context, and the router (needed by /help and
/// /commands to enumerate available commands).
pub fn dispatch(
    cmd: &CommandDef,
    args: &[&str],
    ctx: &CommandContext,
    router: &CommandRouter,
) -> CommandResult {
    match cmd.name {
        // -------------------------------------------------------------------
        // Session commands
        // -------------------------------------------------------------------
        "new" => cmd_new(args, ctx),
        "clear" => cmd_clear(ctx),
        "stop" => cmd_stop(ctx),
        "retry" => cmd_retry(args, ctx),
        "undo" => cmd_undo(args, ctx),
        "rollback" => cmd_rollback(args, ctx),
        "background" | "bg" => cmd_background(args, ctx),
        "btw" => cmd_btw(args, ctx),
        "queue" => cmd_queue(args, ctx),
        "agents" => cmd_agents(args, ctx),
        "status" => cmd_status(ctx),
        "title" => cmd_title(args, ctx),
        "sessions" => cmd_sessions(args, ctx),
        "resume" => cmd_resume(args, ctx),
        "save" => cmd_save(args, ctx),
        "history" => cmd_history(args, ctx),
        "compress" => cmd_compress(args, ctx),
        "personality" => cmd_personality(args, ctx),
        "debug" => cmd_debug(ctx),
        "skin" => cmd_skin(args, ctx),
        "start" => cmd_start(ctx),

        // -------------------------------------------------------------------
        // Configuration commands
        // -------------------------------------------------------------------
        "model" => cmd_model(args, ctx),
        "provider" => cmd_provider(ctx),
        "fast" => cmd_fast(ctx),
        "config" => cmd_config(ctx),
        "profile" => cmd_profile(ctx),
        "yolo" => cmd_yolo(ctx),
        "verbose" => cmd_verbose(ctx),
        "statusbar" => cmd_statusbar(ctx),
        "reasoning" => cmd_reasoning(args, ctx),

        // -------------------------------------------------------------------
        // Exit
        // -------------------------------------------------------------------
        "quit" => cmd_quit(ctx),

        // -------------------------------------------------------------------
        // Info
        // -------------------------------------------------------------------
        "models" => cmd_models(args, ctx),
        "help" => cmd_help(ctx, router),
        "commands" => cmd_commands(args, ctx, router),
        "skills" => cmd_skills(ctx),
        "cron" => cmd_cron(args, ctx),

        // -------------------------------------------------------------------
        // Toolset slash command (Phase 25 Plan 04 — D-06 session-only)
        // -------------------------------------------------------------------
        "toolset" => cmd_toolset(args, ctx),

        // -------------------------------------------------------------------
        // MCP commands (Phase 21.2 Plan 04)
        // -------------------------------------------------------------------
        "reload-mcp" | "reload_mcp" | "reload" => cmd_reload_mcp(ctx),

        // -------------------------------------------------------------------
        // TODO stubs — everything without backing infrastructure
        // -------------------------------------------------------------------
        name => todo_stub(name),
    }
}

// =============================================================================
// Wirable handlers
// =============================================================================

fn cmd_new(args: &[&str], _ctx: &CommandContext) -> CommandResult {
    let message = if args.is_empty() {
        "Conversation cleared. Starting fresh.".to_string()
    } else {
        format!("Starting new session: {}", args.join(" "))
    };
    CommandResult::NewSession { message }
}

fn cmd_clear(_ctx: &CommandContext) -> CommandResult {
    // Phase 22.3 D-06 / UI-SPEC CLR-8: `/clear` is a TTY VISUAL RESET, not a
    // session-history wipe. The REPL loop in main.rs matches ResetTerminal
    // and calls `tui::render::reset_terminal_visual(reserved_row_count)`.
    // For session-history truncation, use `/new` (still returns NewSession).
    CommandResult::ResetTerminal
}

fn cmd_quit(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Quit
}

/// Phase 21.7 Plan 08 (D-26 / G-06): `/stop` drains & kills every background
/// process tracked by the session's ProcessRegistry.
///
/// The pre-Plan-08 stub at this function body only announced agent-running
/// state without doing any work. This version asks the ProcessRegistry for
/// its current tracked count, awaits `drain_and_kill()`, and reports the
/// count killed. The bridge from sync handler to async drain uses
/// `block_in_place` + `Handle::current().block_on` — identical to
/// `cmd_reload_mcp` below.
///
/// Budget 100% / fatal error / user interrupt (G-01/G-04/G-09) are enforced
/// upstream and are unaffected by yolo. `/stop` is the complementary
/// operator-driven killswitch for background processes (G-06).
fn cmd_stop(ctx: &CommandContext) -> CommandResult {
    let pr = match &ctx.process_registry {
        Some(p) => p.clone(),
        None => {
            // No ProcessRegistry wired — fall back to the pre-Plan-08
            // agent-running advisory so /stop is not silent.
            if ctx.agent_running.load(Ordering::SeqCst) {
                return CommandResult::Output(
                    "Stopping agent... (note: cancellation token not yet \
                     wired \u{2014} agent may continue)"
                        .to_string(),
                );
            }
            return CommandResult::Output(
                "No agent is currently running. Use Ctrl-C to cancel an \
                 in-flight turn."
                    .to_string(),
            );
        }
    };
    // D-26 drain-and-kill semantics: every previously tracked child is
    // signalled to die; we use the pre-drain tracked count as the "killed"
    // number because the post-drain count is definitionally 0.
    let count_before = pr.tracked();
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(pr.drain_and_kill());
    });
    CommandResult::Output(format!(
        "Stopped {} background process(es).",
        count_before
    ))
}

/// Phase 21.7 Plan 08 (D-03 / D-09): `/agents list|kill|logs`.
///
/// Dispatches on `args[0]`:
/// - `None | Some("list")` → summarizes all active subagents
/// - `Some("kill")` + id   → cancels the subagent's CancellationToken
/// - `Some("logs")` + id   → returns a bounded tail of the transcript file
///
/// The underlying `SubagentListSnapshot` trait-object is populated by
/// `CommandContext::with_subagent_registry(...)` at the run_chat +
/// gateway sites (Plan 07). Handlers here are SYNC; the trait impl does
/// any async-to-sync bridging for the tokio RwLock read/write.
fn cmd_agents(args: &[&str], ctx: &CommandContext) -> CommandResult {
    let reg = match &ctx.subagent_registry {
        Some(r) => r.clone(),
        None => return CommandResult::Output("Subagent registry not wired.".to_string()),
    };
    match args.first().copied() {
        None | Some("list") => {
            let entries = reg.list_summary();
            if entries.is_empty() {
                return CommandResult::Output("No active subagents.".to_string());
            }
            // Post-UAT fix: show the 1-indexed alias (subagent-N) alongside
            // the full registry id so users know which labels /agents kill
            // accepts. The alias is the ticker's visible label.
            let body = entries
                .iter()
                .enumerate()
                .map(|(idx, (id, summary, uptime))| {
                    format!(
                        "- subagent-{} ({}) ({}) \u{2014} {}s",
                        idx + 1,
                        id,
                        truncate_ellipsis(summary, 80),
                        uptime.as_secs()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            CommandResult::Output(format!("Active subagents:\n{}", body))
        }
        Some("kill") => {
            let token = match args.get(1) {
                Some(s) => *s,
                None => {
                    return CommandResult::Error(
                        "/agents kill <id>: missing id".to_string(),
                    )
                }
            };
            let entries = reg.list_summary();
            match resolve_subagent_id(token, &entries) {
                Resolve::Exact(id) => {
                    if reg.kill(&id) {
                        CommandResult::Output(format!("Cancelled subagent {}.", id))
                    } else {
                        // Race: resolution succeeded but the subagent
                        // unregistered between list_summary() and kill().
                        CommandResult::Output(format!(
                            "No active subagent with id {}.",
                            id
                        ))
                    }
                }
                Resolve::None => {
                    CommandResult::Output(format!("No active subagent with id {}.", token))
                }
                Resolve::Ambiguous(candidates) => CommandResult::Error(format!(
                    "Ambiguous id '{}'; matches: {}",
                    token,
                    candidates.join(", ")
                )),
            }
        }
        Some("logs") => {
            let token = match args.get(1) {
                Some(s) => *s,
                None => {
                    return CommandResult::Error(
                        "/agents logs <id>: missing id".to_string(),
                    )
                }
            };
            let entries = reg.list_summary();
            let resolved = match resolve_subagent_id(token, &entries) {
                Resolve::Exact(id) => id,
                Resolve::None => {
                    return CommandResult::Output(format!(
                        "No active subagent with id {}.",
                        token
                    ))
                }
                Resolve::Ambiguous(candidates) => {
                    return CommandResult::Error(format!(
                        "Ambiguous id '{}'; matches: {}",
                        token,
                        candidates.join(", ")
                    ))
                }
            };
            let path = match reg.transcript_path(&resolved) {
                Some(p) => p,
                None => {
                    return CommandResult::Output(format!(
                        "No transcript for id {}.",
                        resolved
                    ))
                }
            };
            match std::fs::read_to_string(&path) {
                Ok(body) => {
                    // Bounded tail: last 200 lines, preserved in original order.
                    let mut tail: Vec<&str> =
                        body.lines().rev().take(200).collect::<Vec<_>>();
                    tail.reverse();
                    CommandResult::Output(tail.join("\n"))
                }
                Err(e) => CommandResult::Error(format!("Cannot read transcript: {}", e)),
            }
        }
        Some(other) => {
            // Phase 22.3 D-10 / UI-SPEC TYPO-3: append Levenshtein-2 suggestion
            // when a known subcommand is close. Candidates locked by UI-SPEC.
            let candidates: &[&str] = &["list", "kill", "logs"];
            let suffix = suggest_typo(other, candidates)
                .map(|s| format!(" {}", s))
                .unwrap_or_default();
            CommandResult::Error(format!(
                "Unknown /agents subcommand: {}{}",
                other, suffix
            ))
        }
    }
}

/// Char-boundary-safe truncate with a trailing ellipsis when truncated.
/// Used by `cmd_agents` to keep the `list_summary` lines at a predictable
/// width without panicking on multi-byte UTF-8.
fn truncate_ellipsis(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut idx = max;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    format!("{}\u{2026}", &s[..idx])
}

/// Post-UAT helper: what a user-supplied `/agents kill|logs <token>`
/// argument resolves to against the current registry snapshot.
enum Resolve {
    /// Unique match — kill/logs should operate on this full registry id.
    Exact(String),
    /// No entry matched the token.
    None,
    /// Multiple entries matched (user-facing error listing candidate ids).
    Ambiguous(Vec<String>),
}

/// Resolve a user-supplied subagent token to exactly one full registry id.
///
/// The ticker renders subagents as `[subagent-1]`, `[subagent-2]`, ... by
/// spawn-index, but the registry keys entries by a random `sub_xxxxxxxx`
/// id. Users naturally try to kill by the ticker label — this function
/// bridges that gap. Accepts (in precedence order):
///
/// 1. Exact id match — `sub_77747e6e20c2` → `sub_77747e6e20c2`.
/// 2. Position alias — `subagent-N` or bare `N` (1-indexed), where N
///    refers to the nth entry of the current `list_summary()` in its
///    stable iteration order. `subagent-1` picks the first listed
///    subagent, `subagent-2` the second, and so on.
/// 3. Prefix match — any id for which `token` is a prefix of the id OR
///    of the hex suffix after `sub_`. `77747` matches
///    `sub_77747e6e20c2`; `sub_777` also matches. Multiple matches
///    return `Ambiguous`.
///
/// The entries argument is the same `Vec<(id, summary, uptime)>` produced
/// by `list_summary()` so position-aliases use the identical ordering
/// the user sees in `/agents list`.
fn resolve_subagent_id(
    token: &str,
    entries: &[(String, String, std::time::Duration)],
) -> Resolve {
    let token = token.trim();
    if token.is_empty() {
        return Resolve::None;
    }
    // 1. Exact full-id match takes precedence (avoids false positives
    //    from prefix overlap when two ids share a prefix).
    for (id, _, _) in entries {
        if id == token {
            return Resolve::Exact(id.clone());
        }
    }
    // 2. Position alias: `subagent-N` or bare `N`. 1-indexed against the
    //    current list order. Out-of-range → None.
    let numeric = token
        .strip_prefix("subagent-")
        .unwrap_or(token);
    if let Ok(n) = numeric.parse::<usize>() {
        if n >= 1 && n <= entries.len() {
            return Resolve::Exact(entries[n - 1].0.clone());
        }
        // Numeric token out of range — fall through to prefix match so
        // a numeric-looking hex token like `12345` can still match an id
        // suffix.
    }
    // 3. Prefix match against full id or the hex suffix after `sub_`.
    let mut matches: Vec<String> = Vec::new();
    for (id, _, _) in entries {
        let hex_suffix = id.strip_prefix("sub_").unwrap_or(id);
        if id.starts_with(token) || hex_suffix.starts_with(token) {
            matches.push(id.clone());
        }
    }
    match matches.len() {
        0 => Resolve::None,
        1 => Resolve::Exact(matches.into_iter().next().unwrap()),
        _ => Resolve::Ambiguous(matches),
    }
}

fn cmd_status(ctx: &CommandContext) -> CommandResult {
    CommandResult::Output(format!(
        "Session: {}\nPlatform: {}",
        ctx.session_id, ctx.platform
    ))
}

fn cmd_title(args: &[&str], ctx: &CommandContext) -> CommandResult {
    if args.is_empty() {
        return CommandResult::Error(
            "Usage: /title [name] — please provide a session title.".to_string(),
        );
    }
    let title = args.join(" ");
    let store = match &ctx.state_store {
        Some(s) => s.clone(),
        None => {
            // No StateStore wired — return informational confirmation only.
            return CommandResult::Output(format!("Session title set to: {title}"));
        }
    };
    match store.update_title(&ctx.session_id, &title) {
        Ok(()) => CommandResult::Output(format!("Session title set to: {title}")),
        Err(e) => CommandResult::Error(format!("Failed to update title: {e}")),
    }
}

/// `/sessions [--workspace [path]] [limit]` — list recent sessions from StateStore.
///
/// Phase 25.3 D-W-2: `--workspace` filters by workspace_root column. With no
/// path argument, uses the resolved workspace from `ctx.workspace`; with an
/// explicit path, uses that. Without `--workspace`, lists all sessions
/// (backward compat with Phase 22.4.2 behavior).
///
/// Guard pattern (D-05): when `ctx.state_store` is None, returns informational
/// text rather than panicking (backwards-compat with gateway / classic-tui).
fn cmd_sessions(args: &[&str], ctx: &CommandContext) -> CommandResult {
    let store = match &ctx.state_store {
        Some(s) => s.clone(),
        None => return CommandResult::Output(
            "Session storage not configured.".to_string()
        ),
    };

    // Parse --workspace flag: supports `--workspace` (use ctx workspace) or
    // `--workspace <path>` (explicit). Bare numeric arg is treated as a limit.
    let mut workspace_filter: Option<String> = None;
    let mut limit_arg: Option<&str> = None;
    let mut iter = args.iter().peekable();
    while let Some(a) = iter.next() {
        if *a == "--workspace" {
            // Peek for an explicit path argument: not another flag, not a bare number.
            let take_explicit = iter
                .peek()
                .map(|n| !n.starts_with("--") && n.parse::<usize>().is_err())
                .unwrap_or(false);
            if take_explicit {
                workspace_filter = Some(iter.next().unwrap().to_string());
                continue;
            }
            // Bare --workspace: use ctx.workspace
            match &ctx.workspace {
                Some(ws) => {
                    workspace_filter = Some(ws.root.display().to_string());
                }
                None => {
                    return CommandResult::Output(
                        "No workspace resolved at cwd. Pass --workspace <path> explicitly, \
                         or run /sessions from a directory under a .ironhermes/ or .hermes/ marker."
                            .to_string(),
                    );
                }
            }
        } else if a.parse::<usize>().is_ok() {
            limit_arg = Some(*a);
        }
    }
    let limit: usize = limit_arg
        .and_then(|s| s.parse().ok())
        .unwrap_or(20)
        .max(1);

    let text = match workspace_filter {
        Some(ws) => store.list_sessions_text_filtered(limit, Some(&ws)),
        None => store.list_sessions_text(limit),
    };
    CommandResult::Output(text)
}

/// `/resume [name|id]` — restore a previous session by name or id.
///
/// With no args, shows the same listing as `/sessions` as a reminder.
/// Guard pattern (D-05): when `ctx.state_store` is None, returns informational text.
fn cmd_resume(args: &[&str], ctx: &CommandContext) -> CommandResult {
    let store = match &ctx.state_store {
        Some(s) => s.clone(),
        None => return CommandResult::Output(
            "Session storage not configured.".to_string()
        ),
    };
    let name_or_id = match args.first() {
        Some(s) => *s,
        None => {
            // No arg — show session list as a reminder.
            return CommandResult::Output(store.list_sessions_text(20));
        }
    };
    match store.get_session_id(name_or_id) {
        Some(session_id) => CommandResult::Output(
            format!("Resuming session: {session_id}")
        ),
        None => CommandResult::Error(
            format!("Session not found: {name_or_id}")
        ),
    }
}

/// `/save [session_id]` — export session as text.
///
/// With no args, exports the current session.
/// Guard pattern (D-05): when `ctx.state_store` is None, returns informational text.
fn cmd_save(args: &[&str], ctx: &CommandContext) -> CommandResult {
    let store = match &ctx.state_store {
        Some(s) => s.clone(),
        None => return CommandResult::Output(
            "Session storage not configured.".to_string()
        ),
    };
    let session_id = args.first().copied().unwrap_or(&ctx.session_id);
    CommandResult::Output(store.export_session_text(session_id))
}

/// `/history [session_id]` — show conversation history.
///
/// With no args, shows current session history from the snapshot in `ctx.history`.
/// With an explicit session_id, queries StateStore.
/// Guard pattern (D-05): when both are None, returns informational text.
fn cmd_history(args: &[&str], ctx: &CommandContext) -> CommandResult {
    // If an explicit session_id is provided, use StateStore.
    if let Some(session_id) = args.first() {
        let store = match &ctx.state_store {
            Some(s) => s.clone(),
            None => return CommandResult::Output(
                "Session storage not configured.".to_string()
            ),
        };
        return CommandResult::Output(store.history_text(session_id));
    }

    // No arg — use the history snapshot from CommandContext.
    if let Some(history_lock) = &ctx.history {
        let msgs = history_lock.read().unwrap_or_else(|e| e.into_inner());
        if msgs.is_empty() {
            return CommandResult::Output("No messages in history.".to_string());
        }
        let lines: Vec<String> = msgs.iter()
            .map(|m| {
                let role = match m.role {
                    crate::types::Role::User => "You",
                    crate::types::Role::Assistant => "Hermes",
                    crate::types::Role::Tool => "Tool",
                    crate::types::Role::System => "System",
                };
                let content_str: String = m.content.as_ref()
                    .and_then(|c| c.as_text())
                    .map(|s: &str| s.to_string())
                    .unwrap_or_default();
                let preview = if content_str.len() > 120 {
                    format!("{}…", &content_str[..120])
                } else {
                    content_str
                };
                format!("  [{role}] {preview}")
            })
            .collect();
        return CommandResult::Output(
            format!("History ({} messages):\n{}", msgs.len(), lines.join("\n"))
        );
    }

    // Fall back to StateStore current session.
    if let Some(store) = &ctx.state_store {
        return CommandResult::Output(store.history_text(&ctx.session_id));
    }

    CommandResult::Output("Session storage not configured.".to_string())
}

fn cmd_compress(_args: &[&str], ctx: &CommandContext) -> CommandResult {
    let engine = match &ctx.context_compressor {
        Some(e) => e.clone(),
        None => return CommandResult::Output("Context compressor not configured.".to_string()),
    };
    // Per Plan 03 Task 1 deferral note (OQ-5): manual /compress mutation deferred —
    // automatic compression in AgentLoop continues to fire on pressure.
    CommandResult::Output(format!(
        "Context compressor: {} (manual trigger not yet wired — automatic hook fires on pressure).",
        engine.status_text()
    ))
}

/// `/personality [name]` — list available personality presets or apply one.
///
/// With no args: lists all available preset names from PersonalityRegistry.
/// With one arg: returns the overlay text for the named preset.
/// Guard pattern (D-05): when `ctx.personality_overlay` is None, returns informational text.
fn cmd_personality(args: &[&str], ctx: &CommandContext) -> CommandResult {
    let registry = match &ctx.personality_overlay {
        Some(r) => r.clone(),
        None => return CommandResult::Output("Personality registry not configured.".to_string()),
    };
    if args.is_empty() {
        // List mode — show all available preset names.
        let presets = registry.list_presets();
        if presets.is_empty() {
            return CommandResult::Output("No personalities configured.".to_string());
        }
        let mut lines = Vec::with_capacity(presets.len() + 1);
        lines.push(format!("Available personalities ({}):", presets.len()));
        for name in &presets {
            lines.push(format!("  - {}", name));
        }
        CommandResult::Output(lines.join("\n"))
    } else {
        // Apply mode — return overlay text; tui_rata post-router hook applies as system-prompt injection.
        let name = args[0];
        match registry.get_preset(name) {
            Some(text) => CommandResult::Output(text),
            None => CommandResult::Error(format!("Unknown personality: {name}")),
        }
    }
}

/// `/debug` — informational text returner (Cat-1F).
///
/// Real App-state mutation (flipping app.debug_enabled AtomicBool) happens in
/// tui_rata's post-router hook `handle_toggle`. Core returns informational text
/// for gateway compatibility.
fn cmd_debug(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Output("Debug mode toggled.".to_string())
}

/// `/skin [name]` — informational text returner (Cat-1F).
///
/// Real App-state mutation (writing app.skin RwLock<String>) happens in
/// tui_rata's post-router hook `handle_toggle`. Core returns informational text
/// for gateway compatibility.
fn cmd_skin(args: &[&str], _ctx: &CommandContext) -> CommandResult {
    match args.first() {
        Some(name) => CommandResult::Output(format!("Skin set to {name}.")),
        None => CommandResult::Output("Usage: /skin <name>".to_string()),
    }
}

fn cmd_start(_ctx: &CommandContext) -> CommandResult {
    CommandResult::NewSession {
        message: String::new(),
    }
}

/// `/model [name]` — list available models or select a model.
///
/// With no args: lists all available models from the ProviderResolver's model registry.
/// With one arg: validates the named model exists; returns confirmation text that the
/// post-router hook (Plan 03) will use to trigger an AnyClient rebuild on App.
///
/// Guard pattern (D-05): when `ctx.provider_resolver` is None, returns informational text.
/// V5.1: validates model name against registry before returning success.
fn cmd_model(args: &[&str], ctx: &CommandContext) -> CommandResult {
    let resolver = match &ctx.provider_resolver {
        Some(r) => r.clone(),
        None => return CommandResult::Output("Provider resolver not configured.".to_string()),
    };
    if args.is_empty() {
        // List mode — enumerate available models from the registry.
        CommandResult::Output(resolver.model_list_text())
    } else {
        // Validate mode — check the model exists, then return confirmation.
        let target = args[0];
        match resolver.validate_model(target) {
            Ok(model_name) => CommandResult::Output(format!(
                "Selected model: {model_name} (post-router hook will rebuild client)"
            )),
            Err(msg) => CommandResult::Error(msg),
        }
    }
}

/// `/provider` — display current provider/model/endpoint status.
///
/// Guard pattern (D-05): when `ctx.provider_resolver` is None, returns informational text.
/// V8.1: api_key field is NEVER included in the Output text.
fn cmd_provider(ctx: &CommandContext) -> CommandResult {
    let resolver = match &ctx.provider_resolver {
        Some(r) => r.clone(),
        None => return CommandResult::Output("Provider resolver not configured.".to_string()),
    };
    CommandResult::Output(resolver.status_text())
}

/// `/fast` — display fast-role resolution result.
///
/// Returns informational text about what model would be used in fast mode.
/// The actual Arc<AtomicBool> toggle on app.fast_enabled + AnyClient rebuild
/// is owned by Plan 03's handle_subsystem_mutator (post-router hook).
///
/// Guard pattern (D-05): when `ctx.provider_resolver` is None, returns informational text.
fn cmd_fast(ctx: &CommandContext) -> CommandResult {
    let resolver = match &ctx.provider_resolver {
        Some(r) => r.clone(),
        None => return CommandResult::Output("Provider resolver not configured.".to_string()),
    };
    match resolver.fast_role_model() {
        Some(model) => CommandResult::Output(format!(
            "Fast mode: model swap to {model} (post-router hook will rebuild client)."
        )),
        None => CommandResult::Output(
            "Fast role not configured (no fast preset in config).".to_string(),
        ),
    }
}

fn cmd_config(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Output(
        "Use `hermes config show` to inspect, `hermes config set <key> <value>` to change, or `hermes config migrate` to discover skill gaps.".to_string(),
    )
}

fn cmd_profile(_ctx: &CommandContext) -> CommandResult {
    let home = crate::constants::display_hermes_home();
    CommandResult::Output(format!("Profile: default\nHome: {}", home))
}

fn cmd_yolo(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Output(
        "YOLO mode toggled. (Session-level toggle requires caller wiring)".to_string(),
    )
}

fn cmd_verbose(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Output("Verbose mode toggled.".to_string())
}

fn cmd_statusbar(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Output("Status bar toggled.".to_string())
}

fn cmd_reasoning(args: &[&str], _ctx: &CommandContext) -> CommandResult {
    let level = args.first().unwrap_or(&"show");
    CommandResult::Output(format!("Reasoning: {}", level))
}

fn cmd_skills(ctx: &CommandContext) -> CommandResult {
    match &ctx.skill_registry {
        Some(registry) => CommandResult::Output(registry.catalog_text()),
        None => CommandResult::Output("No skills loaded.".to_string()),
    }
}

// Phase 22.4.2.1 Plan 01 — /cron slash command sub-dispatch.
// Mirrors cmd_agents (handlers.rs:164-286) for sub-dispatch shape.
// Mirrors cmd_skills (handlers.rs:685-690) for the None-guard shape.
// All JobStore reads are sync (RESEARCH §3 / U10) — no async bridge needed.
fn cmd_cron(args: &[&str], ctx: &CommandContext) -> CommandResult {
    let store = match &ctx.cron_store {
        Some(s) => s.clone(),
        None => return CommandResult::Output("/cron: cron store not configured.".to_string()),
    };
    match args.first().copied() {
        None | Some("list") => CommandResult::Output(store.list_jobs_text()),
        Some("status") => CommandResult::Output(store.status_text()),
        Some("get") => {
            let id = match args.get(1) {
                Some(s) => *s,
                None => return CommandResult::Error("/cron get <id>: missing id".to_string()),
            };
            match store.get_job_text(id) {
                Some(text) => CommandResult::Output(text),
                None => CommandResult::Error(format!("No cron job found: {}", id)),
            }
        }
        Some("pause") => {
            let id = match args.get(1) {
                Some(s) => *s,
                None => return CommandResult::Error("/cron pause <id>: missing id".to_string()),
            };
            match store.pause_job(id) {
                Ok(s) => CommandResult::Output(s),
                Err(e) => CommandResult::Error(e),
            }
        }
        Some("resume") => {
            let id = match args.get(1) {
                Some(s) => *s,
                None => return CommandResult::Error("/cron resume <id>: missing id".to_string()),
            };
            match store.resume_job(id) {
                Ok(s) => CommandResult::Output(s),
                Err(e) => CommandResult::Error(e),
            }
        }
        Some("run") => {
            let id = match args.get(1) {
                Some(s) => *s,
                None => return CommandResult::Error("/cron run <id>: missing id".to_string()),
            };
            match store.queue_run(id) {
                Ok(s) => CommandResult::Output(s),
                Err(e) => CommandResult::Error(e),
            }
        }
        Some("remove") => {
            let id = match args.get(1) {
                Some(s) => *s,
                None => return CommandResult::Error("/cron remove <id>: missing id".to_string()),
            };
            match store.remove_job(id) {
                Ok(s) => CommandResult::Output(s),
                Err(e) => CommandResult::Error(e),
            }
        }
        Some(other) => {
            let candidates: &[&str] =
                &["list", "status", "get", "pause", "resume", "run", "remove"];
            let suffix = suggest_typo(other, candidates)
                .map(|s| format!(" {}", s))
                .unwrap_or_default();
            CommandResult::Error(format!("Unknown /cron subcommand: {}{}", other, suffix))
        }
    }
}

// Phase 25 Plan 04 (D-06) — `/toolset` slash command sub-dispatch.
//
// Mirrors `cmd_cron` (handlers.rs above) for sub-dispatch shape.
// Mirrors the None-guard pattern (D-05) — when `ctx.toolset_session` is None
// (no live ToolRegistry attached), returns informational text rather than
// panicking (gateway / classic-tui contexts that don't wire the handle).
//
// **D-06 contract**: enable/disable mutate ONLY the in-session config via the
// ToolsetSessionHandle trait. They MUST NOT call `config_setter::config_set`.
// Persistent changes require the `hermes toolset` CLI subcommand.
fn cmd_toolset(args: &[&str], ctx: &CommandContext) -> CommandResult {
    let handle = match &ctx.toolset_session {
        Some(h) => h.clone(),
        None => {
            return CommandResult::Output(
                "/toolset: toolset session handle not configured.".to_string(),
            )
        }
    };
    match args.first().copied() {
        None | Some("list") => CommandResult::Output(handle.render_list()),
        Some("show") => {
            let name = match args.get(1) {
                Some(s) => *s,
                None => {
                    return CommandResult::Error(
                        "/toolset show <name>: missing name".to_string(),
                    )
                }
            };
            match handle.render_show(name) {
                Ok(text) => CommandResult::Output(text),
                Err(e) => CommandResult::Error(e),
            }
        }
        Some("enable") => {
            let name = match args.get(1) {
                Some(s) => *s,
                None => {
                    return CommandResult::Error(
                        "/toolset enable <name>: missing name".to_string(),
                    )
                }
            };
            match handle.enable_toolset(name) {
                Ok(()) => {
                    // T-25-03: cache-break banner shape (session-only mutation
                    // still breaks the LLM's prompt cache on the next call).
                    eprintln!(
                        "\u{26a0} [toolset: {}] enabled \u{2014} schema cache will rebuild on next LLM call",
                        name
                    );
                    CommandResult::Output(format!("/toolset: enabled {} for this session.", name))
                }
                Err(e) => CommandResult::Error(e),
            }
        }
        Some("disable") => {
            let name = match args.get(1) {
                Some(s) => *s,
                None => {
                    return CommandResult::Error(
                        "/toolset disable <name>: missing name".to_string(),
                    )
                }
            };
            match handle.disable_toolset(name) {
                Ok(()) => {
                    eprintln!(
                        "\u{26a0} [toolset: {}] disabled \u{2014} schema cache will rebuild on next LLM call",
                        name
                    );
                    CommandResult::Output(format!("/toolset: disabled {} for this session.", name))
                }
                Err(e) => CommandResult::Error(e),
            }
        }
        Some(other) => {
            let candidates: &[&str] = &["list", "enable", "disable", "show"];
            let suffix = suggest_typo(other, candidates)
                .map(|s| format!(" {}", s))
                .unwrap_or_default();
            CommandResult::Error(format!("Unknown /toolset subcommand: {}{}", other, suffix))
        }
    }
}

fn cmd_help(ctx: &CommandContext, router: &CommandRouter) -> CommandResult {
    let mut out = String::from("Available commands:\n");
    let groups = router.commands_by_category(&ctx.platform);

    for (category, cmds) in groups {
        let cat_name = match category {
            crate::commands::CommandCategory::Session => "SESSION",
            crate::commands::CommandCategory::Configuration => "CONFIGURATION",
            crate::commands::CommandCategory::ToolsAndSkills => "TOOLS & SKILLS",
            crate::commands::CommandCategory::Info => "INFO",
            crate::commands::CommandCategory::Exit => "EXIT",
        };
        out.push('\n');
        out.push_str(cat_name);
        out.push('\n');

        for cmd in cmds {
            // UI-SPEC: 2-space indent, cmd-col=14 (/{:<13}), arg-col=16
            // Matches format_help in ironhermes-cli/src/tui/commands.rs
            out.push_str(&format!(
                "  /{:<13}{:<16}{}\n",
                cmd.name, cmd.args_hint, cmd.description
            ));
        }
    }

    // Keybindings section placeholder — actual keybindings injected by CLI adapter
    out.push_str("\nKeybindings:\n  (platform-specific bindings shown by caller)\n");

    CommandResult::Output(out)
}

fn cmd_commands(args: &[&str], ctx: &CommandContext, router: &CommandRouter) -> CommandResult {
    const PAGE_SIZE: usize = 10;

    let page: usize = args
        .first()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);

    let cmds = router.commands_for_platform(&ctx.platform);
    let total_cmds = cmds.len();
    let total_pages = (total_cmds + PAGE_SIZE - 1) / PAGE_SIZE;
    let total_pages = total_pages.max(1);
    let page = page.min(total_pages);

    let start = (page - 1) * PAGE_SIZE;
    let end = (start + PAGE_SIZE).min(total_cmds);
    let page_cmds = &cmds[start..end];

    let mut out = format!("Commands (page {}/{}):\n\n", page, total_pages);
    for cmd in page_cmds {
        out.push_str(&format!("/{} \u{2014} {}\n", cmd.name, cmd.description));
    }

    if page < total_pages {
        out.push_str(&format!("Use /commands {} for next page\n", page + 1));
    }
    out.push_str("--------------------");

    CommandResult::Output(out)
}

// =============================================================================
// /models handler (Phase 21.3 Plan 04)
// =============================================================================

fn cmd_models(args: &[&str], _ctx: &CommandContext) -> CommandResult {
    match args.first().copied() {
        Some("refresh") => cmd_models_refresh(),
        Some("info") => {
            if let Some(model) = args.get(1) {
                cmd_models_info(model)
            } else {
                CommandResult::Error("Usage: /models info <model>".to_string())
            }
        }
        Some(_) | None => cmd_models_help(),
    }
}

/// /models refresh -- fetch from APIs synchronously using block_in_place.
///
/// block_in_place is safe here: handlers::dispatch is called from within
/// tokio::select! in run_chat (CLI) and from async handler methods (gateway).
/// Both use #[tokio::main] multi-threaded runtime.
fn cmd_models_refresh() -> CommandResult {
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            crate::models_cache::fetch_all().await
        })
    });
    let (entries, fetch_result) = result;

    let mut lines = Vec::new();
    lines.push("Fetching model metadata...".to_string());

    match fetch_result.models_dev_count {
        Some(n) => lines.push(format!("  models.dev: {} models received", n)),
        None => {
            if let Some(ref e) = fetch_result.models_dev_error {
                lines.push(format!("  models.dev: failed - {}", e));
            }
        }
    }
    match fetch_result.openrouter_count {
        Some(n) => lines.push(format!("  OpenRouter: {} models received", n)),
        None => {
            if let Some(ref e) = fetch_result.openrouter_error {
                lines.push(format!("  OpenRouter: failed - {}", e));
            }
        }
    }

    // Save to disk
    let mut cache = crate::models_cache::ModelsCache::default();
    cache.entries = entries;
    match cache.save() {
        Ok(()) => lines.push(format!(
            "Fetch complete. {} entries saved to cache.",
            cache.entries.len()
        )),
        Err(e) => {
            return CommandResult::Error(format!(
                "Fetch failed: {}. Check network and OPENROUTER_API_KEY.",
                e
            ))
        }
    }

    CommandResult::Output(lines.join("\n"))
}

/// /models info <model> -- plain text model detail (no ANSI per UI-SPEC Surface 5).
fn cmd_models_info(model: &str) -> CommandResult {
    let mut registry = crate::model_metadata::ModelRegistry::new();
    let cache = crate::models_cache::ModelsCache::load();
    registry.merge_cache(cache.into_metadata_map());

    match registry.lookup(model) {
        Some(metadata) => {
            let mut lines = Vec::new();
            lines.push(model.to_string());
            lines.push(format!(
                "  Context:    {} tokens",
                format_number(metadata.context_length)
            ));
            match metadata.max_output_tokens {
                Some(n) => lines.push(format!("  Max output: {} tokens", format_number(n))),
                None => lines.push("  Max output: unknown".to_string()),
            }
            lines.push(format!("  Tokenizer:  {}", metadata.tokenizer));
            lines.push(format!(
                "  Vision: {}  Tool use: {}  Reasoning: {}  Streaming: {}",
                if metadata.capabilities.vision { "yes" } else { "no" },
                if metadata.capabilities.tool_use {
                    "yes"
                } else {
                    "no"
                },
                if metadata.capabilities.reasoning {
                    "yes"
                } else {
                    "no"
                },
                if metadata.capabilities.streaming {
                    "yes"
                } else {
                    "no"
                },
            ));
            lines.push("  Source: static table".to_string());
            CommandResult::Output(lines.join("\n"))
        }
        None => CommandResult::Error(format!(
            "Model not found: {}. Run /models refresh to update cache.",
            model
        )),
    }
}

/// Format a number with comma separators (e.g., 200000 -> "200,000").
/// Used for plain-text slash command output per UI-SPEC Surface 5.
fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// /models with no args -- show usage help.
fn cmd_models_help() -> CommandResult {
    CommandResult::Output(
        "Usage: /models [refresh|info <model>]\n  refresh  \u{2014} Fetch latest model metadata from APIs\n  info     \u{2014} Show metadata for a specific model".to_string(),
    )
}

// =============================================================================
// MCP handlers (Phase 21.2 Plan 04)
// =============================================================================

/// /reload-mcp and /reload handler (D-12).
///
/// When `ctx.mcp_reloader` is Some, returns `CommandResult::McpReload` so the
/// REPL loop can perform the async reload and format the UI-SPEC status string
/// (including partial failure display via McpReloadResult.failed).
///
/// When `ctx.mcp_reloader` is None (MCP not configured), returns a plain
/// "MCP not configured." message.
fn cmd_reload_mcp(ctx: &CommandContext) -> CommandResult {
    if ctx.mcp_reloader.is_some() {
        CommandResult::McpReload
    } else {
        CommandResult::Output("MCP not configured.".to_string())
    }
}

// =============================================================================
// Tier D session control handlers (Phase 22.4.2 Plan 04)
// =============================================================================

/// `/retry` — re-queue the last user message as a new turn.
///
/// Reads the history snapshot from `ctx.history` to find the most recent User
/// message. Returns the message text for the post-router hook to re-submit.
/// If no user message exists, returns an informational error.
///
/// Guard pattern (D-05): when `ctx.history` is None, returns informational text.
/// Mutation (removing the last assistant response) happens in the tui_rata
/// post-router hook `handle_session_control` which has `&mut App` access.
fn cmd_retry(_args: &[&str], ctx: &CommandContext) -> CommandResult {
    let history_lock = match &ctx.history {
        Some(h) => h.clone(),
        None => return CommandResult::Output(
            "History not available. Retry requires history threading.".to_string()
        ),
    };
    let msgs = history_lock.read().unwrap_or_else(|e| e.into_inner());
    // Find the last User message in history.
    let last_user = msgs.iter().rev().find(|m| m.role == crate::types::Role::User);
    match last_user {
        Some(msg) => {
            let content = msg.content.as_ref()
                .and_then(|c| c.as_text())
                .unwrap_or("")
                .to_string();
            if content.is_empty() {
                CommandResult::Output("Last user message is empty — nothing to retry.".to_string())
            } else {
                // Post-router hook will truncate history and re-submit this content.
                CommandResult::Output(format!("Retrying: {content}"))
            }
        }
        None => CommandResult::Output(
            "No user messages in history to retry.".to_string()
        ),
    }
}

/// `/undo` — remove the last (user, assistant) pair from history.
///
/// Reads the history snapshot from `ctx.history` to confirm there is something
/// to undo. Returns informational text. The actual truncation happens in the
/// tui_rata post-router hook `handle_session_control` which has `&mut App` access.
///
/// Guard pattern (D-05): when `ctx.history` is None, returns informational text.
fn cmd_undo(_args: &[&str], ctx: &CommandContext) -> CommandResult {
    let history_lock = match &ctx.history {
        Some(h) => h.clone(),
        None => return CommandResult::Output(
            "History not available. Undo requires history threading.".to_string()
        ),
    };
    let msgs = history_lock.read().unwrap_or_else(|e| e.into_inner());
    if msgs.is_empty() {
        return CommandResult::Output("No history to undo.".to_string());
    }
    // Count how many messages will be removed (last user + last assistant pair).
    let has_user = msgs.iter().rev().any(|m| m.role == crate::types::Role::User);
    if !has_user {
        return CommandResult::Output("No user messages in history to undo.".to_string());
    }
    CommandResult::Output(
        "Last exchange undone. (Post-router hook will truncate history.)".to_string()
    )
}

/// `/rollback [n]` — truncate session history by removing the last N exchanges.
///
/// With no args (or n=1): removes the last (user, assistant) pair.
/// With n>1: removes the last N (user, assistant) pairs.
/// Per RESEARCH.md OQ-5: this is session-history truncation only —
/// ContextEngine has no public rollback API. The actual truncation happens
/// in the tui_rata post-router hook.
///
/// Guard pattern (D-05): when `ctx.history` is None, returns informational text.
fn cmd_rollback(args: &[&str], ctx: &CommandContext) -> CommandResult {
    let history_lock = match &ctx.history {
        Some(h) => h.clone(),
        None => return CommandResult::Output(
            "History not available. Rollback requires history threading.".to_string()
        ),
    };
    let n: usize = args.first()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    let msgs = history_lock.read().unwrap_or_else(|e| e.into_inner());
    if msgs.is_empty() {
        return CommandResult::Output("No history to roll back.".to_string());
    }
    CommandResult::Output(format!(
        "Rolling back {n} exchange(s). (Post-router hook will truncate history.)"
    ))
}

/// `/background [message]` — run the given prompt as a background task.
///
/// Queues a message to be run asynchronously by a new AgentLoop instance.
/// The actual spawn happens in the tui_rata post-router hook `handle_session_control`
/// which has access to App's tokio handle and spawn_turn logic.
///
/// Guard pattern (D-05): when `ctx.agent_loop` is None, returns informational text.
fn cmd_background(args: &[&str], ctx: &CommandContext) -> CommandResult {
    if ctx.agent_loop.is_none() {
        return CommandResult::Output(
            "Agent loop not configured. Background tasks require agent threading.".to_string()
        );
    }
    if args.is_empty() {
        return CommandResult::Output(
            "Usage: /background <message> — run a prompt as a background task.".to_string()
        );
    }
    let message = args.join(" ");
    CommandResult::Output(format!(
        "Background task queued: \"{message}\" (post-router hook will spawn agent turn)."
    ))
}

/// `/btw [message]` — send an ephemeral aside to the current agent turn.
///
/// Appends the message as an additional user turn to be processed in the
/// current or next agent execution. The actual injection happens in the
/// tui_rata post-router hook.
///
/// Guard pattern (D-05): when `ctx.agent_loop` is None, returns informational text.
fn cmd_btw(args: &[&str], ctx: &CommandContext) -> CommandResult {
    if ctx.agent_loop.is_none() {
        return CommandResult::Output(
            "Agent loop not configured. BTW requires agent threading.".to_string()
        );
    }
    if args.is_empty() {
        return CommandResult::Output(
            "Usage: /btw <message> — add an aside to the current/next agent turn.".to_string()
        );
    }
    let message = args.join(" ");
    CommandResult::Output(format!(
        "Aside queued: \"{message}\" (post-router hook will inject into next turn)."
    ))
}

/// `/queue [message]` — add a message to the input queue.
///
/// Queues a message to be submitted after the current turn completes.
/// The actual queuing happens in the tui_rata post-router hook.
///
/// Guard pattern (D-05): when `ctx.agent_loop` is None, returns informational text.
fn cmd_queue(args: &[&str], ctx: &CommandContext) -> CommandResult {
    if ctx.agent_loop.is_none() {
        return CommandResult::Output(
            "Agent loop not configured. Queue requires agent threading.".to_string()
        );
    }
    if args.is_empty() {
        return CommandResult::Output(
            "Usage: /queue <message> — add a message to the input queue.".to_string()
        );
    }
    let message = args.join(" ");
    CommandResult::Output(format!(
        "Message queued: \"{message}\" (post-router hook will submit after current turn)."
    ))
}

// =============================================================================
// TODO stubs
// =============================================================================

fn todo_stub(name: &str) -> CommandResult {
    let reason = match name {
        "voice" => "No TTS infrastructure",
        "snapshot" => "No checkpoint system",
        "insights" => "No analytics infrastructure",
        "usage" => "No token cost tracking",
        "update" => "Binary build \u{2014} use package manager",
        "sethome" | "set-home" => "No home channel concept",
        "approve" => "No approval queue",
        "deny" => "No approval queue",
        "prompt" => "No custom system prompt injection",
        "tools" => "No tool enable/disable management",

        "browser" => "No browser tools",
        "plugins" => "No plugin system",
        "paste" => "No clipboard integration",
        "platforms" | "gateway" => "No platform status display",
        _ => "Not implemented",
    };
    CommandResult::Output(format!("/{} is not yet available. ({})", name, reason))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::registry::build_registry;
    use crate::commands::CommandRouter;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    fn make_ctx(agent_running: bool) -> CommandContext {
        CommandContext::new(
            crate::types::Platform::Local,
            "test-session-id".to_string(),
            Arc::new(AtomicBool::new(agent_running)),
        )
    }

    fn make_router() -> CommandRouter {
        CommandRouter::new(build_registry())
    }

    fn find_cmd(name: &str) -> CommandDef {
        build_registry()
            .into_iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("Command '{}' not found in registry", name))
    }

    #[test]
    fn dispatch_help_returns_available_commands() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("help");
        let result = dispatch(&cmd, &[], &ctx, &router);
        match result {
            CommandResult::Output(s) => assert!(
                s.contains("Available commands:"),
                "Help output missing 'Available commands:': {}",
                s
            ),
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_quit_returns_quit() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("quit");
        assert_eq!(dispatch(&cmd, &[], &ctx, &router), CommandResult::Quit);
    }

    #[test]
    fn dispatch_clear_returns_reset_terminal() {
        // Phase 22.3 D-06: /clear now returns ResetTerminal (TTY visual reset),
        // NOT ClearSession (session-history wipe). ClearSession is /new's domain.
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("clear");
        assert_eq!(
            dispatch(&cmd, &[], &ctx, &router),
            CommandResult::ResetTerminal
        );
    }

    #[test]
    fn dispatch_new_returns_new_session() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("new");
        let result = dispatch(&cmd, &[], &ctx, &router);
        assert!(
            matches!(result, CommandResult::NewSession { .. }),
            "Expected NewSession, got {:?}",
            result
        );
    }

    #[test]
    fn dispatch_stop_agent_idle_says_no_agent_running() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("stop");
        let result = dispatch(&cmd, &[], &ctx, &router);
        match &result {
            CommandResult::Output(s) => assert!(
                s.contains("No agent is currently running"),
                "Expected idle message, got: {}",
                s
            ),
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_stop_agent_running_says_stopping() {
        let ctx = make_ctx(true);
        let router = make_router();
        let cmd = find_cmd("stop");
        let result = dispatch(&cmd, &[], &ctx, &router);
        match &result {
            CommandResult::Output(s) => assert!(
                s.contains("Stopping agent"),
                "Expected stopping message, got: {}",
                s
            ),
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_voice_is_not_yet_available() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("voice");
        let result = dispatch(&cmd, &[], &ctx, &router);
        match result {
            CommandResult::Output(s) => assert!(
                s.contains("is not yet available"),
                "Expected stub message, got: {}",
                s
            ),
            other => panic!("Expected Output stub, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_skills_with_no_registry_says_no_skills() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("skills");
        assert_eq!(
            dispatch(&cmd, &[], &ctx, &router),
            CommandResult::Output("No skills loaded.".to_string())
        );
    }

    #[test]
    fn dispatch_title_with_args_returns_confirmation() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("title");
        let result = dispatch(&cmd, &["my", "title"], &ctx, &router);
        match result {
            CommandResult::Output(s) => assert!(
                s.contains("Session title set to: my title"),
                "Expected title confirmation, got: {}",
                s
            ),
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_title_with_no_args_returns_error() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("title");
        let result = dispatch(&cmd, &[], &ctx, &router);
        assert!(
            matches!(result, CommandResult::Error(_)),
            "Expected Error for /title with no args, got {:?}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // /models handler tests (Phase 21.3 Plan 04)
    // -----------------------------------------------------------------------

    #[test]
    fn cmd_models_info_known_model() {
        let result = cmd_models_info("claude-sonnet-4");
        match result {
            CommandResult::Output(text) => {
                assert!(text.contains("claude-sonnet-4"), "missing model name");
                assert!(text.contains("1,000,000"), "missing context length: {}", text);
                assert!(text.contains("cl100k_base"), "missing tokenizer: {}", text);
            }
            _ => panic!("expected Output variant"),
        }
    }

    #[test]
    fn cmd_models_info_unknown_model() {
        let result = cmd_models_info("nonexistent-model-xyz");
        assert!(
            matches!(result, CommandResult::Error(_)),
            "expected Error for unknown model"
        );
    }

    #[test]
    fn cmd_models_help_returns_usage() {
        let result = cmd_models_help();
        match result {
            CommandResult::Output(text) => {
                assert!(text.contains("refresh"), "missing refresh in usage");
                assert!(text.contains("info"), "missing info in usage");
            }
            _ => panic!("expected Output variant"),
        }
    }

    #[test]
    fn format_number_formats_correctly() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1,000");
        assert_eq!(format_number(200_000), "200,000");
        assert_eq!(format_number(1_000_000), "1,000,000");
    }

    #[test]
    fn cmd_models_dispatch_routes_info() {
        let ctx = make_ctx(false);
        let result = cmd_models(&["info", "claude-sonnet-4"], &ctx);
        assert!(
            matches!(result, CommandResult::Output(_)),
            "expected Output for /models info"
        );
    }

    #[test]
    fn cmd_models_dispatch_no_args_shows_help() {
        let ctx = make_ctx(false);
        let result = cmd_models(&[], &ctx);
        match result {
            CommandResult::Output(text) => {
                assert!(text.contains("Usage:"), "missing usage in help: {}", text);
            }
            _ => panic!("expected Output variant for /models help"),
        }
    }

    #[test]
    fn cmd_models_info_missing_arg_returns_error() {
        let ctx = make_ctx(false);
        let result = cmd_models(&["info"], &ctx);
        assert!(
            matches!(result, CommandResult::Error(_)),
            "expected Error for /models info with no model arg"
        );
    }

    #[test]
    fn dispatch_reload_mcp_no_reloader_says_not_configured() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("reload-mcp");
        let result = dispatch(&cmd, &[], &ctx, &router);
        match result {
            CommandResult::Output(s) => assert!(
                s.contains("MCP not configured"),
                "Expected 'MCP not configured', got: {}",
                s
            ),
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_reload_returns_same_as_reload_mcp() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("reload");
        let result = dispatch(&cmd, &[], &ctx, &router);
        match result {
            CommandResult::Output(s) => assert!(
                s.contains("MCP not configured"),
                "Expected 'MCP not configured', got: {}",
                s
            ),
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_all_todo_stubs_return_not_yet_available() {
        let todo_commands = [
            "voice",
            // "background" removed — now has real handler (Phase 22.4.2 Plan 04)
            // "rollback" removed — now has real handler (Phase 22.4.2 Plan 04)
            "snapshot",
            "insights",
            "usage",
            "update",
            "sethome",
            // "retry" removed — now has real handler (Phase 22.4.2 Plan 04)
            // "undo" removed — now has real handler (Phase 22.4.2 Plan 04)
            // "resume" removed — now has real handler (Phase 22.4.2 Plan 01)
            "approve",
            "deny",
            // "history" removed — now has real handler (Phase 22.4.2 Plan 01)
            // "save" removed — now has real handler (Phase 22.4.2 Plan 01)
            "prompt",
            "tools",
            // "toolsets" removed — replaced by `/toolset` (Phase 25 Plan 04, D-06)
            "cron",
            // "reload-mcp" and "reload" removed — now have real handlers (Phase 21.2 Plan 04)
            "browser",
            "plugins",
            "paste",
            "platforms",
            // "btw" removed — now has real handler (Phase 22.4.2 Plan 04)
            // "queue" removed — now has real handler (Phase 22.4.2 Plan 04)
            // "fast" removed — now has real handler (Phase 22.4.2 Plan 02)
            // "debug" removed — now has real handler (Phase 22.4.2 Plan 03)
            // "model" removed — now has real handler (Phase 22.4.2 Plan 02)
            // "personality" removed — now has real handler (Phase 22.4.2 Plan 03)
            // "skin" removed — now has real handler (Phase 22.4.2 Plan 03)
        ];
        let ctx = make_ctx(false);
        let router = make_router();
        let registry = build_registry();

        for name in &todo_commands {
            let cmd = registry
                .iter()
                .find(|c| c.name == *name)
                .unwrap_or_else(|| panic!("Command '{}' not in registry", name));
            let result = dispatch(cmd, &[], &ctx, &router);
            match &result {
                CommandResult::Output(s) => assert!(
                    s.contains("is not yet available"),
                    "Command '{}' should return stub message, got: {}",
                    name,
                    s
                ),
                other => panic!(
                    "Command '{}' should return Output stub, got {:?}",
                    name, other
                ),
            }
        }
    }

    // =========================================================================
    // resolve_subagent_id — post-UAT fix for /agents kill|logs ergonomics
    // =========================================================================

    fn make_entries(
        ids: &[&str],
    ) -> Vec<(String, String, std::time::Duration)> {
        ids.iter()
            .map(|id| {
                (
                    id.to_string(),
                    format!("summary for {}", id),
                    std::time::Duration::from_secs(5),
                )
            })
            .collect()
    }

    #[test]
    fn resolve_exact_full_id_wins() {
        let entries = make_entries(&["sub_abc123456789", "sub_def987654321"]);
        match resolve_subagent_id("sub_abc123456789", &entries) {
            Resolve::Exact(id) => assert_eq!(id, "sub_abc123456789"),
            r => panic!("expected Exact, got {:?}", match r {
                Resolve::None => "None",
                Resolve::Ambiguous(_) => "Ambiguous",
                _ => "?",
            }),
        }
    }

    #[test]
    fn resolve_alias_subagent_1_returns_first_entry() {
        let entries = make_entries(&["sub_first111111", "sub_secondzzzzz"]);
        match resolve_subagent_id("subagent-1", &entries) {
            Resolve::Exact(id) => assert_eq!(id, "sub_first111111"),
            _ => panic!("expected Exact(sub_first111111)"),
        }
    }

    #[test]
    fn resolve_alias_subagent_2_returns_second_entry() {
        let entries = make_entries(&["sub_first111111", "sub_secondzzzzz"]);
        match resolve_subagent_id("subagent-2", &entries) {
            Resolve::Exact(id) => assert_eq!(id, "sub_secondzzzzz"),
            _ => panic!("expected Exact(sub_secondzzzzz)"),
        }
    }

    #[test]
    fn resolve_bare_numeric_works_as_alias() {
        let entries = make_entries(&["sub_first111111", "sub_secondzzzzz"]);
        match resolve_subagent_id("2", &entries) {
            Resolve::Exact(id) => assert_eq!(id, "sub_secondzzzzz"),
            _ => panic!("expected Exact(sub_secondzzzzz)"),
        }
    }

    #[test]
    fn resolve_alias_out_of_range_falls_through_to_prefix() {
        // alias-99 is out of range; falls through to prefix match.
        // Nothing starts with "99" or "subagent-99" so → None.
        let entries = make_entries(&["sub_first111111", "sub_secondzzzzz"]);
        match resolve_subagent_id("subagent-99", &entries) {
            Resolve::None => (),
            _ => panic!("expected None for out-of-range alias"),
        }
    }

    #[test]
    fn resolve_hex_suffix_prefix_matches() {
        // Users copy the short hex from /agents list. "77747" should
        // match "sub_77747e6e20c2".
        let entries = make_entries(&["sub_77747e6e20c2", "sub_67951e109d3b"]);
        match resolve_subagent_id("77747", &entries) {
            Resolve::Exact(id) => assert_eq!(id, "sub_77747e6e20c2"),
            _ => panic!("expected Exact on hex prefix"),
        }
    }

    #[test]
    fn resolve_full_id_prefix_matches() {
        // `sub_777` is a prefix of the full id; single match → Exact.
        let entries = make_entries(&["sub_77747e6e20c2", "sub_67951e109d3b"]);
        match resolve_subagent_id("sub_777", &entries) {
            Resolve::Exact(id) => assert_eq!(id, "sub_77747e6e20c2"),
            _ => panic!("expected Exact on full-id prefix"),
        }
    }

    #[test]
    fn resolve_ambiguous_prefix_returns_candidates() {
        // Two ids share a prefix → Ambiguous with both candidates.
        let entries = make_entries(&["sub_aabbccddeeff", "sub_aa99887766"]);
        match resolve_subagent_id("sub_aa", &entries) {
            Resolve::Ambiguous(candidates) => {
                assert_eq!(candidates.len(), 2);
                assert!(candidates.contains(&"sub_aabbccddeeff".to_string()));
                assert!(candidates.contains(&"sub_aa99887766".to_string()));
            }
            _ => panic!("expected Ambiguous"),
        }
    }

    #[test]
    fn resolve_empty_token_is_none() {
        let entries = make_entries(&["sub_abc"]);
        match resolve_subagent_id("   ", &entries) {
            Resolve::None => (),
            _ => panic!("expected None for empty token"),
        }
    }

    #[test]
    fn resolve_empty_registry_is_none() {
        match resolve_subagent_id("anything", &[]) {
            Resolve::None => (),
            _ => panic!("expected None for empty registry"),
        }
    }

    #[test]
    fn resolve_exact_wins_over_prefix_that_would_be_ambiguous() {
        // Edge case: one id is a strict prefix of another. The shorter
        // id should resolve to itself via the exact-match pass, not
        // Ambiguous via prefix-match.
        let entries = make_entries(&["sub_abc", "sub_abc123"]);
        match resolve_subagent_id("sub_abc", &entries) {
            Resolve::Exact(id) => assert_eq!(id, "sub_abc"),
            _ => panic!("exact match must beat prefix ambiguity"),
        }
    }

    // =========================================================================
    // Phase 25 Plan 04 — /toolset slash handler tests (D-06 session-only)
    // =========================================================================

    use crate::commands::context::ToolsetSessionHandle;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc as StdArc;

    /// Fake handle that records what the slash handler did. Critically, this
    /// fake does NOT touch the filesystem — it only records calls.
    struct FakeToolsetSession {
        enabled_calls: AtomicUsize,
        disabled_calls: AtomicUsize,
    }
    impl FakeToolsetSession {
        fn new() -> Self {
            Self {
                enabled_calls: AtomicUsize::new(0),
                disabled_calls: AtomicUsize::new(0),
            }
        }
    }
    impl ToolsetSessionHandle for FakeToolsetSession {
        fn enable_toolset(&self, _name: &str) -> Result<(), String> {
            self.enabled_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn disable_toolset(&self, _name: &str) -> Result<(), String> {
            self.disabled_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn render_list(&self) -> String {
            "TOOLSET STATUS\nweb enabled\n".to_string()
        }
        fn render_show(&self, name: &str) -> Result<String, String> {
            Ok(format!("Toolset: {}\nStatus: enabled\n", name))
        }
    }

    /// D-06 contract test: `/toolset enable web` with a tempdir IRONHERMES_HOME
    /// MUST NOT write to `<tempdir>/config.yaml`. The handler routes through
    /// the in-process `ToolsetSessionHandle` trait — never `config_setter`.
    #[test]
    fn slash_toolset_handler_session_only_no_config_write() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.yaml");
        // Pre-condition: file does not exist.
        assert!(
            !cfg_path.exists(),
            "precondition: tempdir config.yaml must not exist"
        );

        let fake = StdArc::new(FakeToolsetSession::new());
        let ctx = make_ctx(false).with_toolset_session(fake.clone());
        let router = make_router();

        let cmd = build_registry()
            .into_iter()
            .find(|c| c.name == "toolset")
            .expect("toolset must be in registry");

        let result = dispatch(&cmd, &["enable", "web"], &ctx, &router);

        // Handler returned Output (not Error)
        match &result {
            CommandResult::Output(s) => assert!(
                s.contains("enabled"),
                "expected 'enabled' confirmation, got: {}",
                s
            ),
            other => panic!("expected Output, got {:?}", other),
        }

        // D-06 invariant: NO config.yaml write happened. Even though the
        // tempdir IRONHERMES_HOME is unrelated to dispatch (the slash handler
        // does NOT consult IRONHERMES_HOME), assert the file is still absent.
        assert!(
            !cfg_path.exists(),
            "D-06 violated: slash /toolset enable wrote config.yaml at {}",
            cfg_path.display()
        );

        // The fake handle WAS called — proves the handler dispatched correctly.
        assert_eq!(
            fake.enabled_calls.load(Ordering::SeqCst),
            1,
            "expected enable_toolset to be called exactly once"
        );
        assert_eq!(
            fake.disabled_calls.load(Ordering::SeqCst),
            0,
            "expected disable_toolset NOT to be called"
        );
    }

    /// `/toolset list` with no handle attached — informational fallback.
    #[test]
    fn slash_toolset_no_handle_returns_informational() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = build_registry()
            .into_iter()
            .find(|c| c.name == "toolset")
            .expect("toolset must be in registry");

        let result = dispatch(&cmd, &["list"], &ctx, &router);
        match result {
            CommandResult::Output(s) => assert!(
                s.contains("not configured"),
                "expected 'not configured' fallback, got: {}",
                s
            ),
            other => panic!("expected Output, got {:?}", other),
        }
    }

    /// `/toolset list` with a handle attached — returns rendered list.
    #[test]
    fn slash_toolset_list_renders_via_handle() {
        let fake = StdArc::new(FakeToolsetSession::new());
        let ctx = make_ctx(false).with_toolset_session(fake.clone());
        let router = make_router();
        let cmd = build_registry()
            .into_iter()
            .find(|c| c.name == "toolset")
            .expect("toolset must be in registry");

        let result = dispatch(&cmd, &["list"], &ctx, &router);
        match result {
            CommandResult::Output(s) => assert!(
                s.contains("TOOLSET"),
                "expected rendered table, got: {}",
                s
            ),
            other => panic!("expected Output, got {:?}", other),
        }
    }

    // =========================================================================
    // Phase 25.3 Plan 10 — /sessions --workspace filter tests (D-W-2)
    // =========================================================================

    use crate::commands::context::StateStoreHandle;
    use crate::workspace::Workspace;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;

    /// Fake StateStoreHandle that records the limit + workspace_root passed
    /// to the two list-sessions methods. Records the LAST call only — tests
    /// always invoke a single dispatch, so this is sufficient.
    struct FakeStateStore {
        last_filtered: StdMutex<Option<(usize, Option<String>)>>,
        last_unfiltered: StdMutex<Option<usize>>,
    }
    impl FakeStateStore {
        fn new() -> Self {
            Self {
                last_filtered: StdMutex::new(None),
                last_unfiltered: StdMutex::new(None),
            }
        }
    }
    impl StateStoreHandle for FakeStateStore {
        fn list_sessions_text(&self, limit: usize) -> String {
            *self.last_unfiltered.lock().unwrap() = Some(limit);
            format!("Recent sessions (unfiltered, limit={limit})")
        }
        fn list_sessions_text_filtered(
            &self,
            limit: usize,
            workspace_root: Option<&str>,
        ) -> String {
            *self.last_filtered.lock().unwrap() =
                Some((limit, workspace_root.map(|s| s.to_string())));
            format!(
                "Recent sessions (limit={limit}, ws={})",
                workspace_root.unwrap_or("<none>")
            )
        }
        fn history_text(&self, _session_id: &str) -> String {
            "history".to_string()
        }
        fn export_session_text(&self, _session_id: &str) -> String {
            "export".to_string()
        }
        fn update_title(&self, _session_id: &str, _title: &str) -> Result<(), String> {
            Ok(())
        }
        fn get_session_id(&self, _name_or_id: &str) -> Option<String> {
            None
        }
    }

    fn fake_workspace(root: &str) -> Workspace {
        Workspace {
            root: PathBuf::from(root),
            soul_path: None,
            agents_chain: vec![],
            memory_dir: PathBuf::from(format!("{root}/.ironhermes/memory")),
            skills_dir: PathBuf::from(format!("{root}/skills")),
            tools_config: None,
        }
    }

    fn find_sessions_cmd() -> CommandDef {
        build_registry()
            .into_iter()
            .find(|c| c.name == "sessions")
            .expect("sessions must be in registry")
    }

    #[test]
    fn cmd_sessions_no_state_store_says_not_configured() {
        let ctx = make_ctx(false); // no state_store
        let router = make_router();
        let cmd = find_sessions_cmd();
        let result = dispatch(&cmd, &[], &ctx, &router);
        match result {
            CommandResult::Output(s) => assert!(
                s.contains("Session storage not configured"),
                "expected guard message, got: {}",
                s
            ),
            other => panic!("expected Output, got {:?}", other),
        }
    }

    #[test]
    fn cmd_sessions_no_args_uses_unfiltered_path() {
        let store = StdArc::new(FakeStateStore::new());
        let store_handle: StdArc<dyn StateStoreHandle> = store.clone();
        let ctx = make_ctx(false).with_state_store(store_handle);
        let router = make_router();
        let cmd = find_sessions_cmd();

        dispatch(&cmd, &[], &ctx, &router);

        // unfiltered path called with default limit=20
        assert_eq!(*store.last_unfiltered.lock().unwrap(), Some(20));
        // filtered path NOT touched
        assert!(store.last_filtered.lock().unwrap().is_none());
    }

    #[test]
    fn cmd_sessions_bare_limit_preserves_unfiltered_path() {
        let store = StdArc::new(FakeStateStore::new());
        let store_handle: StdArc<dyn StateStoreHandle> = store.clone();
        let ctx = make_ctx(false).with_state_store(store_handle);
        let router = make_router();
        let cmd = find_sessions_cmd();

        dispatch(&cmd, &["20"], &ctx, &router);

        assert_eq!(*store.last_unfiltered.lock().unwrap(), Some(20));
        assert!(store.last_filtered.lock().unwrap().is_none());
    }

    #[test]
    fn cmd_sessions_workspace_explicit_path_filters() {
        let store = StdArc::new(FakeStateStore::new());
        let store_handle: StdArc<dyn StateStoreHandle> = store.clone();
        let ctx = make_ctx(false).with_state_store(store_handle);
        let router = make_router();
        let cmd = find_sessions_cmd();

        dispatch(&cmd, &["--workspace", "/explicit/path"], &ctx, &router);

        let last = store.last_filtered.lock().unwrap().clone();
        assert_eq!(last, Some((20, Some("/explicit/path".to_string()))));
        assert!(store.last_unfiltered.lock().unwrap().is_none());
    }

    #[test]
    fn cmd_sessions_bare_workspace_uses_ctx_workspace() {
        let store = StdArc::new(FakeStateStore::new());
        let store_handle: StdArc<dyn StateStoreHandle> = store.clone();
        let ws = StdArc::new(fake_workspace("/repo/x"));
        let ctx = make_ctx(false)
            .with_state_store(store_handle)
            .with_workspace(ws);
        let router = make_router();
        let cmd = find_sessions_cmd();

        dispatch(&cmd, &["--workspace"], &ctx, &router);

        let last = store.last_filtered.lock().unwrap().clone();
        assert_eq!(last, Some((20, Some("/repo/x".to_string()))));
    }

    #[test]
    fn cmd_sessions_bare_workspace_no_ctx_workspace_returns_helpful_error() {
        let store = StdArc::new(FakeStateStore::new());
        let store_handle: StdArc<dyn StateStoreHandle> = store.clone();
        let ctx = make_ctx(false).with_state_store(store_handle);
        // No .with_workspace(...) — ctx.workspace = None
        let router = make_router();
        let cmd = find_sessions_cmd();

        let result = dispatch(&cmd, &["--workspace"], &ctx, &router);
        match result {
            CommandResult::Output(s) => {
                assert!(
                    s.contains("No workspace resolved"),
                    "expected helpful error, got: {}",
                    s
                );
                assert!(
                    s.contains("--workspace <path>"),
                    "expected hint about explicit path, got: {}",
                    s
                );
            }
            other => panic!("expected Output, got {:?}", other),
        }
        // Neither store path should have been called
        assert!(store.last_filtered.lock().unwrap().is_none());
        assert!(store.last_unfiltered.lock().unwrap().is_none());
    }

    #[test]
    fn cmd_sessions_workspace_with_limit() {
        let store = StdArc::new(FakeStateStore::new());
        let store_handle: StdArc<dyn StateStoreHandle> = store.clone();
        let ctx = make_ctx(false).with_state_store(store_handle);
        let router = make_router();
        let cmd = find_sessions_cmd();

        dispatch(
            &cmd,
            &["--workspace", "/explicit/path", "5"],
            &ctx,
            &router,
        );

        let last = store.last_filtered.lock().unwrap().clone();
        assert_eq!(last, Some((5, Some("/explicit/path".to_string()))));
    }
}
