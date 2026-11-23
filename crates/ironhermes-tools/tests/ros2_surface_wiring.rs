//! 49.8-08 (D-10/D-13/D-16): region-scoped source invariants proving every
//! surface's real session end awaits `ironhermes_tools::ros2::end_session_stop`,
//! and that both agent composition roots wire the ros2 tools with the live
//! vision handle. These are static `include_str!` text-anchor tests (mirroring
//! `crates/ironhermes-cli/tests/invariants_21_7.rs`) rather than integration
//! tests, because the behavior under test IS the wiring itself — which surface
//! calls which hook, not runtime robot behavior (already covered by
//! `ros2_actions.rs`/`ros2_write_safety.rs`).

const GATEWAY_HANDLER: &str = include_str!("../../ironhermes-gateway/src/handler.rs");
const GATEWAY_RUNNER: &str = include_str!("../../ironhermes-gateway/src/runner.rs");
const CRON_RUNNER: &str = include_str!("../../ironhermes-cron-runner/src/runner.rs");
const WS_RS: &str = include_str!("../../iron_hermes_ui/src/server/ws.rs");
const API_RS: &str = include_str!("../../iron_hermes_ui/src/server/api.rs");
const SESSION_MANAGER: &str = include_str!("../../ironhermes-acp/src/session_manager.rs");
const CLI_MAIN: &str = include_str!("../../ironhermes-cli/src/main.rs");
const CLI_EVENT_LOOP: &str = include_str!("../../ironhermes-cli/src/tui_rata/event_loop.rs");
const KANBAN_WORKER_SPAWN: &str = include_str!("../../ironhermes-kanban/src/worker_spawn.rs");
const APP_RUNTIME_FACTORY: &str = include_str!("../../ironhermes-agent/src/app_runtime_factory.rs");

/// Returns the text strictly between the end of the first occurrence of
/// `start` and the start of the following occurrence of `end`, with
/// comment-only lines (trimmed line begins with `//`) filtered out. Panics
/// naming whichever anchor could not be found — a missing anchor means the
/// source moved and this invariant needs re-anchoring, not a silent false
/// pass.
fn between(source: &str, start: &str, end: &str) -> String {
    let start_idx = source
        .find(start)
        .unwrap_or_else(|| panic!("anchor not found: {start:?}"));
    let after_start = start_idx + start.len();
    let rest = &source[after_start..];
    let end_idx = rest
        .find(end)
        .unwrap_or_else(|| panic!("anchor not found (after {start:?}): {end:?}"));
    let slice = &rest[..end_idx];
    slice
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Counts occurrences of `needle` across non-comment lines of `source`.
fn count_non_comment(source: &str, needle: &str) -> usize {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .map(|line| line.matches(needle).count())
        .sum()
}

// ---------------------------------------------------------------------------
// Gap closure 49.8-11 (D-10/D-16): containment helpers for run_single /
// run_chat's turn-error paths. `between`/`count_non_comment` above are
// presence checks; these prove CONTAINMENT — which call sits inside which
// argument, and whether a `?` appears in a specific region — which a
// text-presence count cannot express (see 49.8-11-PLAN.md's "no-weaker-path"
// must-have).
// ---------------------------------------------------------------------------

/// Strips Rust source down to code-shape only: drops `//` line comments
/// (both full-line and trailing, outside string literals) and empties the
/// contents of `"`-delimited string literals (honoring `\"` escapes), so a
/// literal `?`, `//`, or call-name text sitting inside a comment or a string
/// can never produce a false match in a structural scan below. If a raw
/// string or a bare quote-character literal (`'"'`) ever appears in a
/// scanned body, this helper must be extended to handle it rather than
/// weakening a downstream assertion.
fn code_only(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            if c == '\\' {
                // Consume (but do not emit) the escaped character so an
                // escaped quote (`\"`) never prematurely closes the string.
                chars.next();
                continue;
            }
            if c == '"' {
                in_string = false;
                out.push('"');
            }
            // Swallow all other string content.
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push('"');
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            // Line comment — consume through (not including) the newline.
            while let Some(&next) = chars.peek() {
                if next == '\n' {
                    break;
                }
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Returns the text of `source` from the first occurrence of `start_anchor`
/// (a function signature prefix, e.g. `"async fn run_single("`) up to —
/// but not including — the next top-level `"\nasync fn "` after it. Panics
/// naming whichever anchor could not be found.
fn fn_body(source: &str, start_anchor: &str) -> String {
    let start_idx = source
        .find(start_anchor)
        .unwrap_or_else(|| panic!("anchor not found: {start_anchor:?}"));
    let rest = &source[start_idx..];
    let search_from = start_anchor.len();
    let next_fn_rel = rest[search_from..].find("\nasync fn ").unwrap_or_else(|| {
        panic!("no following \"\\nasync fn \" found after {start_anchor:?} (last fn in file?)")
    });
    rest[..search_from + next_fn_rel].to_string()
}

/// Balanced-paren scan of the argument text immediately following the
/// opening `(` of the first occurrence of `call_prefix` (which must itself
/// end in `(`) in `source`. Returns `(argument_text, end_idx)` where
/// `end_idx` is the byte index in `source` of the character immediately
/// AFTER the call's matching closing `)` — i.e. `&source[end_idx..]` is
/// everything following the whole call expression. Tracks nested `(`/`)`
/// pairs so a call whose argument itself contains other calls (e.g.
/// `with_session_end_stop(registry, async { ... run_turn(...) ... })`) is
/// scanned correctly. Panics naming the call if it is not found or the
/// parens never balance — a stale anchor must fail loudly, not silently.
fn call_args_with_end(source: &str, call_prefix: &str) -> (String, usize) {
    assert!(
        call_prefix.ends_with('('),
        "call_prefix must end with '(': {call_prefix:?}"
    );
    let start_idx = source
        .find(call_prefix)
        .unwrap_or_else(|| panic!("anchor not found: {call_prefix:?}"));
    let args_start = start_idx + call_prefix.len();
    let rest = &source[args_start..];
    let mut depth: i32 = 1;
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return (rest[..i].to_string(), args_start + i + 1);
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced parens scanning call {call_prefix:?} (source truncated or anchor stale)");
}

// ---------------------------------------------------------------------------
// Task 1: gateway /new, /start, idle-expiry sweep, shutdown, and cron job end
// ---------------------------------------------------------------------------

#[test]
fn gateway_new_and_start_resets_stop_ros2() {
    let start_region = between(
        GATEWAY_HANDLER,
        r#"store.reset_session(&session_key, "start")"#,
        "intro_event",
    );
    assert!(
        start_region.contains("end_session_stop("),
        "gateway /start must await end_session_stop between the session reset and \
         the intro_event greeting. Region:\n{start_region}"
    );

    let new_region = between(
        GATEWAY_HANDLER,
        r#"store.reset_session(&session_key, "new")"#,
        "Conversation cleared. Starting fresh.",
    );
    assert!(
        new_region.contains("end_session_stop("),
        "gateway /new must await end_session_stop between the session reset and \
         the \"Conversation cleared\" reply. Region:\n{new_region}"
    );
}

#[test]
fn gateway_expiry_sweep_stops_ros2() {
    let region = between(GATEWAY_RUNNER, "9b. Session cleanup task", "9c.");
    assert!(
        region.contains("expire_stale("),
        "gateway idle-expiry sweep (9b) must still call expire_stale. Region:\n{region}"
    );
    assert!(
        region.contains("end_session_stop("),
        "gateway idle-expiry sweep (9b) must await end_session_stop when it expires \
         at least one session. Region:\n{region}"
    );
}

#[test]
fn gateway_shutdown_stops_ros2() {
    let region = between(
        GATEWAY_RUNNER,
        "initiating graceful shutdown",
        "Shutting down MCP servers",
    );
    assert!(
        region.contains("end_session_stop("),
        "gateway graceful shutdown must await end_session_stop before MCP servers \
         (and everything else) tear down. Region:\n{region}"
    );
}

#[test]
fn cron_job_end_stops_ros2() {
    let region = between(
        CRON_RUNNER,
        "(6) Process outcome",
        "complete_and_dispatch(ctx, job, &output, success).await",
    );
    assert!(
        region.contains("job_touched_ros2("),
        "cron job end must compute job_touched_ros2 before dispatching. Region:\n{region}"
    );
    assert!(
        region.contains("end_session_stop("),
        "cron job end must await end_session_stop when the job touched ros2/rospider \
         (or errored). Region:\n{region}"
    );
}

// ---------------------------------------------------------------------------
// Task 2: web chat, voice2voice, ACP and CLI/TUI/kanban session ends
// ---------------------------------------------------------------------------

#[test]
fn web_chat_ws_teardown_stops_ros2() {
    let region = between(
        WS_RS,
        r#""keepalive failed""#,
        r#"#[cfg(not(feature = "server"))]"#,
    );
    assert!(
        region.contains("end_session_stop("),
        "web chat ws_chat teardown (every break path, including keepalive failure) \
         must await end_session_stop. Region:\n{region}"
    );
}

#[test]
fn voice_realtime_end_stops_ros2() {
    let session_end = between(
        API_RS,
        "pub async fn realtime_session_end(turn_id: String) -> Result<(), ServerFnError> {",
        "pub async fn ",
    );
    assert!(
        session_end.contains("end_session_stop("),
        "realtime_session_end must await end_session_stop. Region:\n{session_end}"
    );

    let heartbeat = between(
        API_RS,
        "pub async fn realtime_heartbeat(turn_id: String) -> Result<RealtimeTurnSignal, ServerFnError> {",
        "pub async fn ",
    );
    assert!(
        heartbeat.contains("end_session_stop("),
        "realtime_heartbeat's Cancelled arm must await end_session_stop. Region:\n{heartbeat}"
    );

    let prune = between(
        API_RS,
        "pub async fn realtime_prune(stale_secs: Option<u64>) -> Result<serde_json::Value, ServerFnError> {",
        "pub async fn ",
    );
    assert!(
        prune.contains("end_session_stop("),
        "realtime_prune must await end_session_stop when it prunes at least one \
         stale session. Region:\n{prune}"
    );
}

#[test]
fn acp_close_stops_ros2() {
    let region = between(
        SESSION_MANAGER,
        "pub async fn close(&mut self, session_id: &str) -> bool {",
        "pub async fn fork",
    );
    assert!(
        region.contains("end_session_stop("),
        "ACP SessionManager::close must await end_session_stop. Region:\n{region}"
    );
}

/// Gap closure 49.8-11 (D-16): rewritten from a whole-file presence count to
/// region-scoped containment. `run_chat` is multi-turn (capture-then-cleanup
/// via `end_session_stop`); `run_single` is a whole-session dispatch routed
/// through the seam (`with_session_end_stop`) — these are two DIFFERENT
/// call shapes, so the invariant checks each surface's own body rather than
/// counting either name across the whole file.
#[test]
fn cli_session_ends_stop_ros2() {
    let run_chat_body = fn_body(CLI_MAIN, "async fn run_chat(");
    assert!(
        run_chat_body.contains("end_session_stop("),
        "run_chat's body must await end_session_stop("
    );

    let run_single_body = fn_body(CLI_MAIN, "async fn run_single(");
    assert!(
        run_single_body.contains("with_session_end_stop("),
        "run_single's body must route its dispatch through with_session_end_stop("
    );

    let event_loop_count = count_non_comment(CLI_EVENT_LOOP, "end_session_stop(");
    assert!(
        event_loop_count >= 1,
        "tui_rata/event_loop.rs must await end_session_stop at TUI exit. Found {event_loop_count}."
    );
}

/// Gap closure 49.8-11 (D-10/D-16): structural containment invariant that
/// replaces the old presence-only `cli_session_ends_stop_ros2` count for
/// run_single specifically. Proves — not merely observes — that EVERY
/// dispatch call in run_single (both the worker-mode
/// `run_goal_loop_if_enabled(` path and the non-worker `run_turn(` path) is
/// lexically inside `with_session_end_stop('s argument, and that no `?`
/// sits between that call and the D-07 all-tools hook
/// (`call_session_end_hooks()`) — so a turn error, a worker-setup error, or
/// (via Task 1's seam) a panic can never skip the robot stop.
#[test]
fn cli_run_single_routes_every_dispatch_through_session_end_stop() {
    let body = fn_body(CLI_MAIN, "async fn run_single(");
    let code = code_only(&body);

    assert!(
        code.contains("with_session_end_stop("),
        "run_single's body must call with_session_end_stop("
    );

    let (args, after_call_idx) = call_args_with_end(&code, "with_session_end_stop(");
    assert!(
        args.contains("run_goal_loop_if_enabled(") && args.contains("run_turn("),
        "with_session_end_stop('s argument must contain both the worker-mode \
         run_goal_loop_if_enabled( dispatch and the non-worker run_turn( dispatch. \
         Argument text:\n{args}"
    );

    let without_args = code.replacen(&args, "", 1);
    assert!(
        !without_args.contains("run_goal_loop_if_enabled(") && !without_args.contains("run_turn("),
        "run_single must not dispatch run_goal_loop_if_enabled( or run_turn( anywhere \
         outside the with_session_end_stop( argument"
    );

    let after_call = &code[after_call_idx..];
    let hooks_idx = after_call
        .find("call_session_end_hooks()")
        .unwrap_or_else(|| {
            panic!("call_session_end_hooks() not found in run_single after with_session_end_stop(")
        });
    let between_region = &after_call[..hooks_idx];
    assert!(
        !between_region.contains('?'),
        "no `?` may appear between the with_session_end_stop( call and \
         call_session_end_hooks() in run_single. Region:\n{between_region}"
    );

    let main_count = count_non_comment(CLI_MAIN, "with_session_end_stop(");
    assert_eq!(
        main_count, 1,
        "main.rs must contain exactly one with_session_end_stop( call (inside \
         run_single). Found {main_count}."
    );
}

// ---------------------------------------------------------------------------
// Task 3: classic REPL (run_chat) turn errors reach the ros2 stop
// ---------------------------------------------------------------------------

/// Gap closure 49.8-11 (D-10/D-16): `run_chat` is multi-turn, so it cannot
/// route through `with_session_end_stop` (that would cancel every
/// in-flight Nav2 goal at every turn boundary — see Task 1's doc comment).
/// Instead it must capture-then-cleanup: both turn sites (the initial
/// message and every REPL turn) must feed into a shared `fatal_turn_error`
/// slot rather than `?`-returning past the D-07 all-tools hook and the
/// awaited ros2 stop. Proven here as a containment invariant: no `?`
/// anywhere in the whole region from the first `run_agent_turn(` call to
/// `end_session_stop(`, both turn sites present in that region, and
/// `call_session_end_hooks()` ordered before `end_session_stop(`.
#[test]
fn cli_run_chat_turn_errors_reach_ros2_stop() {
    let body = fn_body(CLI_MAIN, "async fn run_chat(");
    let code = code_only(&body);

    let first_turn_idx = code
        .find("run_agent_turn(")
        .unwrap_or_else(|| panic!("run_agent_turn( not found in run_chat"));
    let stop_idx = code
        .find("end_session_stop(")
        .unwrap_or_else(|| panic!("end_session_stop( not found in run_chat"));
    assert!(
        stop_idx > first_turn_idx,
        "end_session_stop( must come after the first run_agent_turn( call in run_chat"
    );

    let region = &code[first_turn_idx..stop_idx];
    let turn_call_count = region.matches("run_agent_turn(").count();
    assert!(
        turn_call_count >= 2,
        "expected at least 2 run_agent_turn( calls (initial message + REPL) between \
         the first turn and end_session_stop(. Found {turn_call_count}. Region:\n{region}"
    );
    assert!(
        !region.contains('?'),
        "no `?` may appear between the first run_agent_turn( call and \
         end_session_stop( in run_chat. Region:\n{region}"
    );

    let hooks_idx = code
        .find("call_session_end_hooks()")
        .unwrap_or_else(|| panic!("call_session_end_hooks() not found in run_chat"));
    assert!(
        hooks_idx < stop_idx,
        "call_session_end_hooks() must come before end_session_stop( in run_chat"
    );
}

#[test]
fn kanban_worker_runs_cli_single_path() {
    assert!(
        KANBAN_WORKER_SPAWN.contains(r#".arg("chat")"#)
            && KANBAN_WORKER_SPAWN.contains(r#".arg("-q")"#),
        "kanban worker must invoke `chat -q` (main.rs's run_single path), which is \
         what guarantees the awaited end_session_stop actually runs before the \
         worker process exits."
    );
}

#[test]
fn new_call_sites_leave_hexapod_hook_alone() {
    // D-07: the all-tools session-end hook is `call_session_end_hooks` — none of
    // this plan's new call sites may invoke it (that would toggle the Freenove
    // hexapod's stop-and-relax on every ros2-only session boundary).
    for (name, source) in [
        ("gateway handler.rs", GATEWAY_HANDLER),
        ("gateway runner.rs", GATEWAY_RUNNER),
        ("cron runner.rs", CRON_RUNNER),
        ("ws.rs", WS_RS),
        ("api.rs", API_RS),
        ("session_manager.rs", SESSION_MANAGER),
    ] {
        let count = count_non_comment(source, "call_session_end_hooks");
        assert_eq!(
            count, 0,
            "{name} must not call the all-tools session-end hook \
             (call_session_end_hooks) — D-07 hexapod behavior must stay unchanged \
             on this surface. Found {count}."
        );
    }
}

// ---------------------------------------------------------------------------
// Task 3: live vision handle wired into ros2 tools in both composition roots
// ---------------------------------------------------------------------------

/// Returns the argument text of the first `register_ros2_tools(` call in
/// `source` — from just after the opening `(` up to the following literal
/// `");"` (the call's own closing paren + statement semicolon; a nested
/// `Some(vision_handle)` argument ends in `),`, not `);`, so this simple
/// literal search finds the real outer close without needing a
/// balanced-paren scanner).
fn register_ros2_tools_call_args(source: &str) -> String {
    let anchor = "register_ros2_tools(";
    let start_idx = source
        .find(anchor)
        .unwrap_or_else(|| panic!("anchor not found: {anchor:?}"));
    let after_anchor = &source[start_idx + anchor.len()..];
    let end_idx = after_anchor
        .find(");")
        .unwrap_or_else(|| panic!("no closing \");\" found after {anchor:?}"));
    after_anchor[..end_idx].to_string()
}

#[test]
fn vision_wired_for_ros2_in_factory_and_tui() {
    let factory_args = register_ros2_tools_call_args(APP_RUNTIME_FACTORY);
    assert!(
        factory_args.contains("Some("),
        "app_runtime_factory.rs's register_ros2_tools call must pass Some(vision_handle), \
         not None. Args:\n{factory_args}"
    );

    let tui_args = register_ros2_tools_call_args(CLI_EVENT_LOOP);
    assert!(
        tui_args.contains("Some("),
        "tui_rata/event_loop.rs's register_ros2_tools call must pass Some(vision_handle), \
         not None. Args:\n{tui_args}"
    );
}

// ---------------------------------------------------------------------------
// 49.8-22 (D-10/D-16): drift guard pinning every surface's 49.8-08
// session-end comment block to value-free phrasing. VERIFICATION gap #1
// found seven surface comments restating a stale "bounded (4s)" figure the
// code has not used since 49.8-17 widened the bound to 6s. This guard scans
// ONLY the 49.8-08 session-end comment blocks of every surface source (so
// unrelated text elsewhere in these large files can never trip it) and
// fails on any duration literal inside them, in any common spelling, so the
// surface wording cannot drift from `ironhermes_tools::ros2::session`'s own
// bound again.
// ---------------------------------------------------------------------------

/// Scans `source` line-by-line for every contiguous block of `//`-prefixed
/// lines that carries the `49.8-08` tag: a line whose trimmed text starts
/// with `//` and contains `49.8-08`, plus every immediately following line
/// whose trimmed text also starts with `//`. Returns each block as a list
/// of `(1-based line number, line text)` pairs, in file order. A file may
/// carry more than one such block (e.g. the gateway handler's `/start` and
/// `/new` comments).
fn find_49_8_08_blocks(source: &str) -> Vec<Vec<(usize, &str)>> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("//") && trimmed.contains("49.8-08") {
            let mut block = vec![(i + 1, lines[i])];
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim_start().starts_with("//") {
                block.push((j + 1, lines[j]));
                j += 1;
            }
            blocks.push(block);
            i = j;
        } else {
            i += 1;
        }
    }
    blocks
}

/// Duration-literal grammar (49.8-22 Design decisions): an ASCII digit run,
/// optionally `.` plus digits, optionally one space, then one of the units
/// `ms`/`s`/`sec`/`secs`/`second`/`seconds`, followed by a character that is
/// not an ASCII letter or digit (or the line end). Matches `(4s)`, `4 s`,
/// `4 secs`, `bounded to 4 seconds`, and `4000ms`; does not match `49.8-08`
/// or `D-10/D-16` (no unit word immediately follows their digit runs).
fn line_has_duration_literal(line: &str) -> bool {
    const UNITS: [&str; 6] = ["seconds", "second", "secs", "sec", "ms", "s"];
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i].is_ascii_digit() {
            let mut j = i;
            while j < n && bytes[j].is_ascii_digit() {
                j += 1;
            }
            // Optional `.` + digits.
            if j < n && bytes[j] == b'.' && j + 1 < n && bytes[j + 1].is_ascii_digit() {
                j += 1;
                while j < n && bytes[j].is_ascii_digit() {
                    j += 1;
                }
            }
            // Optional single space before the unit.
            let mut k = j;
            if k < n && bytes[k] == b' ' {
                k += 1;
            }
            for unit in UNITS.iter() {
                let ulen = unit.len();
                // `line.get()` (not `&line[..]`) so a candidate span that lands
                // mid-multi-byte-character (e.g. the digit in "voice2voice"
                // followed shortly by a non-ASCII em dash) safely yields `None`
                // instead of panicking on an invalid UTF-8 slice boundary.
                if k + ulen <= n && line.get(k..k + ulen) == Some(*unit) {
                    let after = k + ulen;
                    let boundary_ok = after >= n || !bytes[after].is_ascii_alphanumeric();
                    if boundary_ok {
                        return true;
                    }
                }
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    false
}

#[test]
fn ros2_session_end_bound_comments_carry_no_hardcoded_seconds() {
    let sources: [(&str, &str); 8] = [
        ("gateway handler", GATEWAY_HANDLER),
        ("gateway runner", GATEWAY_RUNNER),
        ("acp session_manager", SESSION_MANAGER),
        ("cron runner", CRON_RUNNER),
        ("web ws", WS_RS),
        ("web api", API_RS),
        ("cli main", CLI_MAIN),
        ("tui event_loop", CLI_EVENT_LOOP),
    ];

    let mut hits: Vec<(&str, usize, String)> = Vec::new();

    for (name, source) in sources.iter() {
        assert!(
            source.contains("end_session_stop"),
            "{name} must contain end_session_stop"
        );

        let blocks = find_49_8_08_blocks(source);
        assert!(
            !blocks.is_empty(),
            "{name} must contain at least one 49.8-08 session-end comment block \
             (the drift guard cannot pass vacuously)"
        );

        for block in &blocks {
            for (line_no, line_text) in block {
                if line_has_duration_literal(line_text) {
                    hits.push((name, *line_no, (*line_text).to_string()));
                }
            }
        }
    }

    assert!(
        hits.is_empty(),
        "session-end comment blocks must not hardcode a duration literal (49.8-22 \
         drift guard) — found:\n{hits:#?}"
    );
}

// ---------------------------------------------------------------------------
// Task 2 (49.8-23, G-49.8-21): drift guard — the TUI teardown must target
// the registry turns drive, capture unwinds, run the D-15 hooks BEFORE the
// stop, and be the ONLY session-end path in run_with_deps. Region-scoped
// (each function's own signature prefix up to the NEXT top-level fn
// signature) over `code_only(CLI_EVENT_LOOP)` — deliberately NOT `fn_body`,
// which stops only at a following plain `async fn` and would make a region
// over-inclusive the moment a following function happens to be `pub async
// fn` (as every function here is).
// ---------------------------------------------------------------------------

#[test]
fn tui_exit_stops_the_session_turns_drive() {
    let code = code_only(CLI_EVENT_LOOP);

    const SESSION_END_REGISTRY_SIG: &str =
        "pub fn session_end_registry(app: &App) -> Arc<RwLock<ironhermes_tools::ToolRegistry>> {";
    const WITH_TUI_SESSION_END_STOP_SIG: &str = "pub async fn with_tui_session_end_stop<F>(";
    const RUN_TUI_SESSION_SIG: &str = "pub async fn run_tui_session<B, S>(";
    const BUILD_APP_DEPS_SIG: &str =
        "async fn build_app_deps(cli: &crate::cli_args::Cli, yolo: bool) -> Result<AppDeps> {";

    // (d) run_with_deps: single session-end path — routes through
    // run_tui_session and contains NEITHER end_session_stop( nor
    // call_session_end_hooks directly (both checked first since this
    // region also anchors (a)'s starting boundary).
    let run_with_deps_region = between(&code, "async fn run_with_deps(", SESSION_END_REGISTRY_SIG);
    assert!(
        run_with_deps_region.contains("run_tui_session("),
        "G-49.8-21: run_with_deps must call run_tui_session(. Region:\n{run_with_deps_region}"
    );
    assert!(
        !run_with_deps_region.contains("end_session_stop(")
            && !run_with_deps_region.contains("call_session_end_hooks"),
        "G-49.8-21: run_with_deps must have EXACTLY ONE session-end path (run_tui_session) — \
         found a direct end_session_stop(/call_session_end_hooks call outside it. Region:\n{run_with_deps_region}"
    );

    // (a) session_end_registry targets the runtime registry.
    let registry_fn_region = between(
        &code,
        SESSION_END_REGISTRY_SIG,
        WITH_TUI_SESSION_END_STOP_SIG,
    );
    assert!(
        registry_fn_region.contains("agent_runtime.registry()"),
        "G-49.8-21: session_end_registry must target agent_runtime.registry() (the registry \
         turns drive), not app.registry. Region:\n{registry_fn_region}"
    );

    // (b) with_tui_session_end_stop: catch_unwind, hooks BEFORE the stop, resume_unwind.
    let teardown_region = between(&code, WITH_TUI_SESSION_END_STOP_SIG, RUN_TUI_SESSION_SIG);
    for needle in [
        "catch_unwind",
        "call_session_end_hooks()",
        "end_session_stop(",
        "resume_unwind",
    ] {
        assert!(
            teardown_region.contains(needle),
            "G-49.8-21: with_tui_session_end_stop must contain {needle:?}. Region:\n{teardown_region}"
        );
    }
    let hooks_idx = teardown_region
        .find("call_session_end_hooks()")
        .expect("checked above");
    let stop_idx = teardown_region
        .find("end_session_stop(")
        .expect("checked above");
    assert!(
        hooks_idx < stop_idx,
        "G-49.8-21: call_session_end_hooks() must run BEFORE end_session_stop( in \
         with_tui_session_end_stop. Region:\n{teardown_region}"
    );

    // (c) run_tui_session composes session_end_registry + with_tui_session_end_stop.
    let run_tui_session_region = between(&code, RUN_TUI_SESSION_SIG, BUILD_APP_DEPS_SIG);
    for needle in ["session_end_registry(", "with_tui_session_end_stop("] {
        assert!(
            run_tui_session_region.contains(needle),
            "G-49.8-21: run_tui_session must call {needle:?}. Region:\n{run_tui_session_region}"
        );
    }
}
