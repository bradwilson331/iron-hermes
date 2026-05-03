//! Phase 25.3 LOAD-BEARING parity-guard tests.
//!
//! Locks the 4-site wireup contract for the new CommandContext fields
//! (`workspace`, `trajectory_writer`) BEFORE Plan 8 implements the wireup.
//!
//! ## Why this file exists (RESEARCH.md Pitfall 1)
//!
//! Phase 25.2 Plan 15's parity-guard test only grep'd `main.rs`. It MISSED:
//!   - `crates/ironhermes-cli/src/tui_rata/commands.rs::build_command_context`
//!     (the DEFAULT `hermes chat` REPL since Phase 22.4)
//!   - `crates/ironhermes-gateway/src/handler.rs::handle_slash_command`
//!     (Telegram per-message slash dispatch)
//!
//! Result: Phase 25.2 UAT FAILED for `/toolset` in REPL + Telegram. Two follow-up
//! commits (`61ba493`, `3f25ac5`) were required after the verifier returned PASS.
//! See `.planning/phases/25.2-web-extract-tools/25.2-VERIFICATION.md` Addendum (2026-05-03)
//! for the full post-mortem.
//!
//! ## Wave 0 -> Plan 8 RED-then-GREEN protocol
//!
//! These tests are RED at Wave 0 — the wireup does not exist until Plan 8 (4-site
//! wireup of Workspace + TrajectoryWriter into all CommandContext construction sites
//! AND GatewayRunner setters). They are gated with `#[ignore]` so other Wave 0 / 1
//! plans' `cargo test --workspace` runs are not blocked by the planned-RED state.
//!
//! Plan 8's FIRST task MUST remove the `#[ignore]` attributes after wiring the 4 sites.
//! Plan 8's success criterion is that all 6 tests turn GREEN with the ignores removed.
//!
//! ## Coverage matrix
//!
//! | Field            | Site                                       | Test                                                          |
//! |------------------|--------------------------------------------|---------------------------------------------------------------|
//! | workspace        | main.rs::build_cmd_ctx                     | workspace_wired_in_all_commandcontext_construction_sites      |
//! | workspace        | tui_rata/commands.rs::build_command_context| workspace_wired_in_all_commandcontext_construction_sites      |
//! | workspace        | gateway/handler.rs::handle_slash_command   | workspace_wired_in_gateway_handler                            |
//! | workspace        | gateway/runner.rs::set_workspace setter    | workspace_wired_in_gateway_runner_setter                      |
//! | trajectory_writer| main.rs::build_cmd_ctx                     | trajectory_writer_wired_in_all_commandcontext_construction_sites|
//! | trajectory_writer| tui_rata/commands.rs::build_command_context| trajectory_writer_wired_in_all_commandcontext_construction_sites|
//! | trajectory_writer| gateway/handler.rs::handle_slash_command   | trajectory_writer_wired_in_gateway_handler                    |
//! | trajectory_writer| gateway/runner.rs::set_trajectory_writer   | trajectory_writer_wired_in_gateway_runner_setter              |

const MAIN_RS: &str = include_str!("../src/main.rs");
const TUI_COMMANDS: &str = include_str!("../src/tui_rata/commands.rs");
const GW_HANDLER: &str = include_str!("../../ironhermes-gateway/src/handler.rs");
const GW_RUNNER: &str = include_str!("../../ironhermes-gateway/src/runner.rs");

// Phase 25.3 GAP-CLOSURE include_str! anchors (Wave 7 — see INV-25.3-07..11 below).
const TUI_EVENT_LOOP: &str = include_str!("../src/tui_rata/event_loop.rs");
const GW_SESSION: &str = include_str!("../../ironhermes-gateway/src/session.rs");

// ============================================================================
// Workspace parity (Phase 25.3 D-W-2)
// ============================================================================

#[test]
fn workspace_wired_in_all_commandcontext_construction_sites() {
    // Strip line comments first (per planner antipattern: bare grep can match
    // a doc comment and produce a self-invalidating gate).
    let main_no_comments: String = MAIN_RS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let tui_no_comments: String = TUI_COMMANDS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        main_no_comments.contains(".with_workspace("),
        "INV-25.3-01a: main.rs::build_cmd_ctx must call `.with_workspace(handle)` on \
         the CommandContext builder. Phase 25.2 lesson: missing this attach makes \
         cmd_sessions --workspace fall back to None for the CLI subcommand entry points."
    );
    assert!(
        tui_no_comments.contains(".with_workspace("),
        "INV-25.3-01b: tui_rata/commands.rs::build_command_context must call \
         `.with_workspace(handle)`. The ratatui REPL is the DEFAULT hermes chat surface \
         since Phase 22.4 — missing this attach is the EXACT regression that bit Phase 25.2."
    );
}

#[test]
fn workspace_wired_in_gateway_handler() {
    let no_comments: String = GW_HANDLER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        no_comments.contains(".with_workspace("),
        "INV-25.3-02: gateway/handler.rs::handle_slash_command must call \
         `.with_workspace(handle)` on the per-message CommandContext. Without this, \
         /sessions --workspace returns None for Telegram users."
    );
}

#[test]
fn workspace_wired_in_gateway_runner_setter() {
    let no_comments: String = GW_RUNNER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        no_comments.contains("set_workspace("),
        "INV-25.3-03: gateway/runner.rs must define `pub fn set_workspace(...)` setter \
         so run_gateway can install the resolved Workspace before runner.start(). \
         Mirrors set_toolset_session at line 111. build_gateway_handler clones it into the handler."
    );
}

// ============================================================================
// TrajectoryWriter parity (Phase 25.3 D-T-3)
// ============================================================================

#[test]
fn trajectory_writer_wired_in_all_commandcontext_construction_sites() {
    let main_no_comments: String = MAIN_RS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let tui_no_comments: String = TUI_COMMANDS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        main_no_comments.contains(".with_trajectory_writer("),
        "INV-25.3-04a: main.rs::build_cmd_ctx must call `.with_trajectory_writer(handle)`. \
         Without this, slash commands dispatched in the CLI cannot record trajectory entries."
    );
    assert!(
        tui_no_comments.contains(".with_trajectory_writer("),
        "INV-25.3-04b: tui_rata/commands.rs::build_command_context must call \
         `.with_trajectory_writer(handle)`. The ratatui REPL is the default chat surface — \
         missing this attach starves Phase 25.4 Curator's training-data source."
    );
}

#[test]
fn trajectory_writer_wired_in_gateway_handler() {
    let no_comments: String = GW_HANDLER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        no_comments.contains(".with_trajectory_writer("),
        "INV-25.3-05: gateway/handler.rs::handle_slash_command must call \
         `.with_trajectory_writer(handle)` on the per-message CommandContext. \
         Telegram is the primary user-facing surface — excluding it would starve 25.4 Curator."
    );
}

#[test]
fn trajectory_writer_wired_in_gateway_runner_setter() {
    let no_comments: String = GW_RUNNER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    // Phase 25.3-15 CR-02 close-out widened the contract: the runner exposes
    // EITHER `set_trajectory_writer` (legacy process-wide handle) OR
    // `set_trajectory_root` (canonical per-session lazy-open via SessionStore).
    // The latter is the correct shape post-CR-02; the former is grandfathered
    // for compatibility with the original Plan 8 wireup.
    assert!(
        no_comments.contains("set_trajectory_writer(")
            || no_comments.contains("set_trajectory_root("),
        "INV-25.3-06: gateway/runner.rs must expose `set_trajectory_writer(...)` OR \
         `set_trajectory_root(...)` so run_gateway can configure trajectory destination \
         before runner.start(). Phase 25.3-15 CR-02 close-out switched the canonical \
         form to set_trajectory_root (per-session lazy-open keyed by canonical SQLite \
         session UUID); the original setter shape is grandfathered for compatibility."
    );
}

// ============================================================================
// Sanity check — these pass at Wave 0 (locks the existing toolset_session parity
// so Plan 8 cannot accidentally remove it while adding workspace + trajectory).
// ============================================================================

#[test]
fn existing_toolset_session_parity_still_holds() {
    // This is GREEN at Wave 0 because Phase 25.2 Plan 15 already wired toolset_session.
    // Plan 8 must NOT regress these calls when adding workspace + trajectory.
    let main_no_comments: String = MAIN_RS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let tui_no_comments: String = TUI_COMMANDS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let gw_handler_no_comments: String = GW_HANDLER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let gw_runner_no_comments: String = GW_RUNNER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(main_no_comments.contains(".with_toolset_session("));
    assert!(tui_no_comments.contains(".with_toolset_session("));
    assert!(gw_handler_no_comments.contains(".with_toolset_session("));
    assert!(gw_runner_no_comments.contains("set_toolset_session("));
}

// ============================================================================
// Phase 25.3 GAP-CLOSURE parity-guards (Wave 7 RED -> Wave 8/9 GREEN)
//
// These five tests lock the contract for the four CR-XX critical code-review
// findings + the verifier-flagged gateway SessionStore gap. They are RED at
// the start of Wave 8 and turn GREEN as Plans 25.3-13..16 land their fixes.
//
// They are gated with `#[ignore]` so the Wave 7 `cargo test --workspace` run
// is not blocked by the planned-RED state (mirrors the Wave 0 -> Plan 8
// RED-then-GREEN protocol used by the parity-guards above). Plans 25.3-13..16
// each remove the `#[ignore]` for their corresponding test as the fix lands.
//
// Coverage matrix:
//
// | Test                                                     | Locks                                | Fixed by Plan |
// |----------------------------------------------------------|--------------------------------------|---------------|
// | invariant_25_3_07_repl_calls_create_session              | CR-01 (REPL session row missing)     | 25.3-13       |
// | invariant_25_3_08_with_workspace_root_called_in_all_...  | CR-04 (REPL [Workspace:] line miss)  | 25.3-15       |
// | invariant_25_3_09_gateway_session_store_passes_...       | WR-02 + verifier blocker             | 25.3-14       |
// | invariant_25_3_10_gateway_trajectory_uses_canonical_...  | CR-02 (gateway-{} trajectory token)  | 25.3-16       |
// | invariant_25_3_11_session_store_get_or_create_accepts_...| Verifier blocker signature change    | 25.3-14       |
// ============================================================================

/// Comment-stripping helper used by every gap-closure parity-guard below to
/// avoid the planner antipattern of self-invalidating gates (a doc comment
/// that mentions the asserted token would otherwise satisfy the grep).
///
/// Strips BOTH leading line-comments (`// foo`) AND inline trailing
/// line-comments (`code, // foo`). Inline-trailing stripping is required
/// because the gateway/session.rs Plan 0 placeholder TODAY is written as
/// `None, // workspace_root: Plan 0 placeholder` — without this, the
/// `workspace_root` token survives in the kept line and self-invalidates
/// INV-25.3-11.
///
/// Block comments (`/* ... */`) are NOT stripped (none of the asserted-on
/// files use them around the asserted tokens — verified at authoring time).
fn strip_line_comments(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .map(|l| match l.find("//") {
            // Inline trailing comment — keep only the code portion.
            // Note: this is a literal substring match; it is safe here because
            // none of the asserted-on tokens contain `//` and none of the kept
            // lines have `//` inside a string literal that we depend on.
            Some(idx) => l[..idx].to_string(),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn invariant_25_3_07_repl_calls_create_session() {
    // CR-01: ratatui REPL build_app_deps must persist a sessions row via
    // state.create_session so /sessions, /resume, /history, /export-session,
    // and the workspace_root filter (D-W-2) work on the default chat surface.
    // Without this row, every REPL session is invisible to /sessions and
    // its workspace_root is never persisted.
    let src = strip_line_comments(TUI_EVENT_LOOP);
    // Match either `state_store.create_session(` or `s.create_session(` on a
    // bound StateStore variable. The token `.create_session(` is sufficient
    // — no other type in the REPL exposes a method with that exact name.
    assert!(
        src.contains(".create_session("),
        "INV-25.3-07: tui_rata/event_loop.rs::build_app_deps must call \
         `state_store.create_session(&session_id, \"cli-repl\", Some(model), None, None, \
         workspace.as_ref().and_then(|w| w.root.to_str()))` so the REPL session row \
         is persisted. Without it, /sessions, /resume, /history, /export-session all \
         fail on the default chat surface (CR-01 from 25.3-REVIEW.md)."
    );
}

#[test]
fn invariant_25_3_08_with_workspace_root_called_in_all_chat_entry_points() {
    // CR-04: the durable Identity-slot [Workspace: <root>] line must be
    // injected on EVERY chat entry surface — not just main.rs run_chat
    // and run_single. The ratatui REPL (default chat since Phase 22.4) and
    // the gateway handler are the two missing surfaces.
    let main_src = strip_line_comments(MAIN_RS);
    let event_loop_src = strip_line_comments(TUI_EVENT_LOOP);
    let gw_handler_src = strip_line_comments(GW_HANDLER);

    // main.rs already has 2 sites today (run_chat + run_single); locking that
    // count prevents accidental regression while we add the third site.
    let main_count = main_src.matches(".with_workspace_root(").count();
    assert!(
        main_count >= 2,
        "INV-25.3-08a: main.rs must have >= 2 .with_workspace_root( calls \
         (run_chat at ~line 853 + run_single at ~line 1452); found {main_count}. \
         Plan 25.3-15 must NOT regress these."
    );

    // gateway handler already has 1 site today (handler.rs:543); regression-lock.
    assert!(
        gw_handler_src.contains(".with_workspace_root("),
        "INV-25.3-08b: gateway handler.rs must call prompt_builder.with_workspace_root \
         (already at line 543 today). Plan 25.3-15 must NOT regress."
    );

    // The NEW assertion — RED until Plan 25.3-15 adds the call to event_loop.rs.
    assert!(
        event_loop_src.contains(".with_workspace_root("),
        "INV-25.3-08c: tui_rata/event_loop.rs::build_app_deps must construct a \
         PromptBuilder, call .with_workspace_root(&ws.root) on it when a workspace \
         is resolved, and seed the resulting system message into app.history (or \
         into the per-turn AgentLoop). Without this, the durable [Workspace: <root>] \
         Identity-slot line is missing on the DEFAULT hermes chat surface — same \
         Pitfall-1 class as the original 4-site CommandContext gap (CR-04 from 25.3-REVIEW.md)."
    );
}

#[test]
// Phase 25.3 Plan 14: un-ignored after threading workspace into SessionStore.
// SessionStore now has an Option<Arc<Workspace>> field + set_workspace setter,
// and GatewayRunner::set_workspace propagates the resolved workspace down so
// state.create_session(..., workspace_root) is no longer hardcoded None.
fn invariant_25_3_09_gateway_session_store_passes_workspace_root() {
    // VERIFIER BLOCKER + WR-02 + CR-03 (gateway half): SessionStore::get_or_create
    // must NOT pass a literal `None` to state.create_session for the workspace_root
    // argument. It must thread the resolved workspace from GatewayRunner via a new
    // field/setter and pass `workspace.as_ref().and_then(|ws| ws.root.to_str())`
    // (or to_string_lossy after Plan 25.3-16 lands).
    let src = strip_line_comments(GW_SESSION);

    // RED guard: the Plan 0 placeholder comment is a code reference, not a comment.
    // After Plan 25.3-14 lands the fix, the literal "Plan 0 placeholder" must be removed.
    assert!(
        !src.contains("Plan 0 placeholder"),
        "INV-25.3-09a: gateway/session.rs must NOT contain the 'Plan 0 placeholder' \
         marker — it indicates the workspace_root is still hardcoded to None. \
         Plan 25.3-14 must remove the marker as part of fixing the verifier blocker."
    );

    // The SessionStore must reference a workspace field for the create_session arg.
    // After the fix, get_or_create reads `self.workspace.as_ref().and_then(...)` (or
    // similar). The token `workspace` (lowercased) appears in the source body 0
    // times today (only in the comment we just banned).
    let workspace_refs = src.matches("workspace").count();
    assert!(
        workspace_refs >= 2,
        "INV-25.3-09b: gateway/session.rs must reference `workspace` at least twice \
         (struct field + read in get_or_create); found {workspace_refs}. \
         Plan 25.3-14 must add an Option<Arc<Workspace>> field to SessionStore (or \
         equivalent) and thread it into the create_session call."
    );
}

#[test]
// Phase 25.3-15 CR-02 close-out: un-ignored after main.rs stopped opening a
// process-wide TrajectoryWriter keyed by `gateway-<random-uuid>`. The gateway
// now hands `SessionStore` a trajectory ROOT (`set_trajectory_root`), and
// `SessionStore::get_or_create_trajectory_writer` lazily opens per-session
// writers at `<root>/<canonical_session_id>/trajectories.jsonl` — the same
// canonical UUID `state.create_session` received, so `hermes session export`
// can find the file.
fn invariant_25_3_10_gateway_trajectory_uses_canonical_session_uuid() {
    // CR-02: gateway trajectory file path must be derived from the canonical
    // SQLite session UUID, NOT a process-wide `gateway-<uuid>` token.
    // Today main.rs:2309 has `format!("gateway-{}", uuid::Uuid::new_v4())`
    // which decouples trajectories from per-message session IDs.
    let src = strip_line_comments(MAIN_RS);

    // The literal banned token. Plan 25.3-16 must replace this with a per-message
    // session_id derivation (preferably moving the open into handler.rs::run_agent
    // keyed by GatewaySession.session_id).
    assert!(
        !src.contains("gateway-{}"),
        "INV-25.3-10a: main.rs must NOT contain the literal `gateway-{{}}` format \
         string for a trajectory directory name — that decouples trajectories from \
         per-message session UUIDs. Plan 25.3-16 must remove this and key the \
         trajectory path off the canonical SQLite session UUID (CR-02 from 25.3-REVIEW.md)."
    );

    // Symmetric guard on the variable name.
    assert!(
        !src.contains("gateway_trajectory_id"),
        "INV-25.3-10b: main.rs must NOT define `gateway_trajectory_id` — that \
         variable name is the marker for the process-wide trajectory token CR-02 \
         flagged. Plan 25.3-16 must remove it (CR-02 from 25.3-REVIEW.md)."
    );
}

#[test]
// Phase 25.3 Plan 14: un-ignored after SessionStore wires workspace_root through
// to state.create_session via the SessionStore.workspace field + set_workspace
// setter. The signature widening was implemented as the field+setter variant
// (option (b) in the test docstring below) rather than a parameter widening.
fn invariant_25_3_11_session_store_get_or_create_accepts_workspace_root() {
    // VERIFIER BLOCKER signature change: SessionStore::get_or_create must take
    // a workspace_root parameter (Option<&str> or similar) so per-message dispatch
    // in the gateway handler can pass the resolved workspace through. The
    // GatewayRunner-held workspace can also be threaded via a SessionStore field;
    // either approach is acceptable but the NEW interface MUST exist.
    let src = strip_line_comments(GW_SESSION);

    // After the fix, the SessionStore impl block contains either:
    //   (a) `pub fn get_or_create(&mut self, key: SessionKey, model: &str, source: &str,
    //         workspace_root: Option<&str>) -> &mut GatewaySession`
    //   (b) a SessionStore field + setter (e.g. `set_workspace`) plus an internal
    //       read in get_or_create.
    // We accept either by checking that the file mentions `workspace_root` outside
    // the existing comment. (The body had ZERO references before the fix; after
    // the fix, the `workspace_root` token MUST appear in the impl block.)
    let workspace_root_refs = src.matches("workspace_root").count();
    assert!(
        workspace_root_refs >= 1,
        "INV-25.3-11: gateway/session.rs must reference `workspace_root` at least \
         once in non-comment source (signature parameter, field, or pass-through). \
         Plan 25.3-14 must wire the resolved workspace into SessionStore so \
         get_or_create can pass it to state.create_session."
    );
}
