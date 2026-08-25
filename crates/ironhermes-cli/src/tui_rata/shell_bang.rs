//! `!` shell execution for the tui_rata REPL (Phase 36.6.4 Plan 03, D-09..D-11,
//! TUI-BANG-01).
//!
//! This module owns everything about running an operator-typed `!` command:
//! spawning, timeout, interactive-command refusal, cap-with-marker
//! truncation (Task 1), and the plain/styled render pair the transcript uses
//! to draw the resulting block (Task 2). `App::dispatch_bang_blocking`
//! (`app.rs`) is the sole caller — the transcript/`App.history`/`ReplHistory`
//! wiring lives there, not here.
//!
//! ## D-09: the environment divergence, stated explicitly
//!
//! `crates/ironagent-tools-api/src/terminal.rs`'s `TerminalTool` (Phase 42
//! EXEC-03 / D-04/D-06) calls `cmd.env_clear()` then `cmd.envs(
//! ironhermes_core::build_terminal_safe_env(...))` before every spawn, so an
//! AGENT-initiated subprocess only ever sees a sanitized allowlist — that
//! exists to stop a subprocess started via `env`/`printenv`/`curl -d
//! "$(env)"` from exfiltrating secrets in the parent process env (e.g.
//! `CLOUDFLARE_API_TOKEN`).
//!
//! This module deliberately does NOT call `env_clear()` or `envs(
//! build_terminal_safe_env(...))` anywhere below. `tokio::process::Command`
//! inherits the parent process environment by default when neither call is
//! made, so `!` runs in the OPERATOR's real shell environment by
//! construction (D-09) — `!echo $MY_VAR` behaves exactly as it would in
//! their own terminal. The allowlist's threat model constrains
//! AGENT-initiated subprocesses; `!` is OPERATOR-initiated (reachable only
//! from `App::dispatch_bang_blocking`, itself reachable only from text typed
//! into the input line — never from model or tool output), so that threat
//! model does not apply here. This module is a structurally SEPARATE
//! function from `TerminalTool::execute`, never a mode flag on it, so a
//! future refactor cannot silently reunify the two spawn paths and either
//! reintroduce scrubbing for operator commands or drop it for agent calls.
//! The mirrored one-line pointer lives in `terminal.rs`'s own doc comment
//! (see its D-09 reference).

use std::process::Stdio;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

/// D-11: cap on the COMBINED (stdout + stderr) captured output, ~8KB per
/// CONTEXT's suggestion — a stray `!cat bigfile` must not consume the
/// context window. Named so the figure carries a rationale, not a magic
/// literal.
pub const OUTPUT_CAP_BYTES: usize = 8 * 1024;

/// Default per-command timeout (D-10). `!` has no per-invocation timeout
/// argument (there is no JSON tool schema here, unlike the agent-side
/// `terminal` tool) — this mirrors `terminal.rs`'s own 30s default.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// D-10: first-token-only denylist for commands presumed interactive (would
/// capture the TTY / hang forever under a non-interactive capture, or
/// garble the alternate screen if left to run). RESEARCH's own targeted
/// grep across `ironagent-tools-api`/`ironhermes-core` found ZERO
/// TTY-detection infrastructure anywhere in this codebase (Assumption A2) —
/// this list is INVENTED, not inherited, seeded with the editors/pagers
/// CONTEXT names by name (`vim`, `less`, `top`) plus their common siblings.
/// List-completeness sign-off is deferred to this phase's Plan 06 manual
/// UAT checkpoint (see `<planner_assumptions>` item 2 in the PLAN). An
/// incomplete list degrades to "the command hangs until
/// `DEFAULT_TIMEOUT_SECS` fires", never to a garbled alternate screen,
/// because the timeout below bounds every spawn regardless of whether the
/// denylist caught it.
const INTERACTIVE_DENYLIST: &[&str] = &[
    "vim", "vi", "nvim", "view", "emacs", "emacsclient", "nano", "pico", "ed", "less", "more",
    "most", "top", "htop", "btop", "man", "watch", "tmux", "screen", "ssh", "telnet", "ftp",
    "sftp", "mysql", "psql", "sqlite3", "python", "python3", "irb", "node", "ipython", "vifm",
    "ranger", "mc",
];

/// The terminal result an executed (or refused) `!` command produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellResult {
    /// The subprocess ran to completion with this exit code.
    Exited(i32),
    /// The subprocess exceeded the timeout and was killed. `stdout`/`stderr`
    /// on the surrounding `ShellOutcome` hold whatever was captured before
    /// the deadline — partial output is never discarded (TUI-BANG-01).
    TimedOut { after_secs: u64 },
    /// D-10: the first token matched `INTERACTIVE_DENYLIST` — refused
    /// before any subprocess was spawned.
    Refused,
    /// The subprocess could not even be spawned (e.g. `sh` missing from
    /// `PATH`) — kept distinct from `Exited`/`TimedOut` so the renderer
    /// never mislabels a spawn failure as "the command ran and exited 0".
    SpawnError(String),
}

/// Byte counts for a truncated capture (D-11). Both fields are BYTE counts,
/// not character counts — the Copywriting Contract's marker text labels
/// them `bytes` explicitly (TUI-BANG-01/precision).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TruncationInfo {
    pub shown: usize,
    pub total: usize,
}

/// Everything the renderer (Task 2) needs to draw a `!` block without
/// re-deriving anything: the echoed command, the two captured streams
/// (kept separate so stderr can be styled `Color::Red` independently of
/// stdout), the terminal result, and truncation counts when truncation
/// occurred.
#[derive(Debug, Clone)]
pub struct ShellOutcome {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub result: ShellResult,
    pub truncation: Option<TruncationInfo>,
}

/// App-level record of a `!` invocation across its lifecycle — `Running`
/// while the spawn is in flight, `Done` once a `ShellOutcome` is known.
/// Rendered by `shell_block_plain`/`shell_block_lines` below; owned by
/// `App::shell_runs` (Task 2, `app.rs`).
#[derive(Debug, Clone)]
pub struct ShellRun {
    pub command: String,
    pub state: ShellRunState,
    /// Phase 36.6.4 Plan 12 (G-09 closure): the chronological insertion
    /// point for this block — `App.history` rows `0..history_anchor` are
    /// emitted before it, everything else after. This is an ORDERING KEY
    /// ONLY, never an index into `history` (see `App::apply_shell_outcome`
    /// and `App::transcript_render_units`, `app.rs`, for the stamp site and
    /// the comparison that reads it).
    pub history_anchor: usize,
}

#[derive(Debug, Clone)]
pub enum ShellRunState {
    Running,
    Done(ShellOutcome),
}

/// D-10: classify a command's FIRST TOKEN (before any shell expansion)
/// against `INTERACTIVE_DENYLIST`, compared by basename so `/usr/bin/vim`
/// refuses the same as `vim`. Pure and testable without spawning anything —
/// `interactive_command_denylist_matches_first_token_only` depends on this
/// NOT inspecting the rest of the command line, so `echo vim` is never
/// refused.
pub fn classify_interactive(first_token: &str) -> bool {
    let basename = first_token.rsplit('/').next().unwrap_or(first_token);
    INTERACTIVE_DENYLIST.contains(&basename)
}

/// Walk `idx` backward until it lands on a valid UTF-8 char boundary of
/// `s` (never mid-codepoint — TUI-BANG-01/precision).
fn char_boundary_floor(s: &str, idx: usize) -> usize {
    let mut end = idx.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Truncate `text` to at most `cap` bytes on a UTF-8 char boundary. Returns
/// `(possibly-truncated text, Some(TruncationInfo) iff truncation
/// occurred)`. Exposed standalone (in addition to `truncate_combined`
/// below) so the char-boundary-safety property is unit-testable without
/// needing two separate streams.
pub fn truncate_with_marker(text: &str, cap: usize) -> (String, Option<TruncationInfo>) {
    if text.len() <= cap {
        return (text.to_string(), None);
    }
    let end = char_boundary_floor(text, cap);
    let total = text.len();
    (text[..end].to_string(), Some(TruncationInfo { shown: end, total }))
}

/// D-11: apply the SAME cap to the combined (stdout + stderr) capture, but
/// keep the two streams separate (never concatenated into one string) so
/// Task 2's renderer can style stderr rows `Color::Red` independently of
/// stdout. Truncation eats into stdout first, in capture order (mirrors
/// `terminal.rs`'s stdout-then-stderr `combined` convention); once stdout
/// alone reaches the cap, stderr is dropped entirely rather than
/// interleaved.
fn truncate_combined(
    stdout: String,
    stderr: String,
    cap: usize,
) -> (String, String, Option<TruncationInfo>) {
    let total = stdout.len() + stderr.len();
    if total <= cap {
        return (stdout, stderr, None);
    }
    if stdout.len() >= cap {
        let end = char_boundary_floor(&stdout, cap);
        return (
            stdout[..end].to_string(),
            String::new(),
            Some(TruncationInfo { shown: end, total }),
        );
    }
    let remaining = cap - stdout.len();
    let end = char_boundary_floor(&stderr, remaining);
    let shown = stdout.len() + end;
    (
        stdout,
        stderr[..end].to_string(),
        Some(TruncationInfo { shown, total }),
    )
}

/// D-10 refusal copy (Copywriting Contract, verbatim). `command` is the
/// command text WITHOUT the leading `!`.
pub fn refusal_message(command: &str) -> String {
    format!(
        "'! {command}' refused — interactive commands aren't supported here. \
         Run it in your own terminal instead."
    )
}

/// Run `command` under `sh -c` with `DEFAULT_TIMEOUT_SECS`. Public entry
/// point `App::dispatch_bang_blocking` (Task 2, `app.rs`) calls.
pub async fn run(command: &str) -> ShellOutcome {
    run_with_timeout(command, DEFAULT_TIMEOUT_SECS).await
}

/// `run`'s timeout parameterized for testability —
/// `shell_bang_timeout_preserves_partial_output` uses a short deadline so
/// the test doesn't take `DEFAULT_TIMEOUT_SECS` (30s) to run.
pub async fn run_with_timeout(command: &str, timeout_secs: u64) -> ShellOutcome {
    let command_owned = command.to_string();
    let first_token = command.split_whitespace().next().unwrap_or("");
    if classify_interactive(first_token) {
        return ShellOutcome {
            command: command_owned,
            stdout: String::new(),
            stderr: String::new(),
            result: ShellResult::Refused,
            truncation: None,
        };
    }

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // D-09 (see module doc comment above): deliberately NO `cmd.env_clear()`
    // / `cmd.envs(ironhermes_core::build_terminal_safe_env(...))` here — `!`
    // inherits the operator's real process environment by construction.

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ShellOutcome {
                command: command_owned,
                stdout: String::new(),
                stderr: String::new(),
                result: ShellResult::SpawnError(e.to_string()),
                truncation: None,
            };
        }
    };
    let mut stdout_pipe = child.stdout.take().expect("stdout piped above");
    let mut stderr_pipe = child.stderr.take().expect("stderr piped above");

    let mut stdout_buf: Vec<u8> = Vec::new();
    let mut stderr_buf: Vec<u8> = Vec::new();

    // Read both pipes to EOF concurrently, then reap the child. The SAME
    // `Vec<u8>` buffers are mutated across every poll of the inner futures,
    // so if the outer `timeout` below cancels this future mid-read,
    // whatever bytes already arrived stay in `stdout_buf`/`stderr_buf` —
    // this is what makes `shell_bang_timeout_preserves_partial_output` true.
    let read_and_wait = async {
        let (_stdout_result, _stderr_result) = tokio::join!(
            stdout_pipe.read_to_end(&mut stdout_buf),
            stderr_pipe.read_to_end(&mut stderr_buf),
        );
        child.wait().await
    };

    match timeout(Duration::from_secs(timeout_secs), read_and_wait).await {
        Ok(Ok(status)) => {
            let stdout = String::from_utf8_lossy(&stdout_buf).to_string();
            let stderr = String::from_utf8_lossy(&stderr_buf).to_string();
            let (stdout, stderr, truncation) = truncate_combined(stdout, stderr, OUTPUT_CAP_BYTES);
            ShellOutcome {
                command: command_owned,
                stdout,
                stderr,
                result: ShellResult::Exited(status.code().unwrap_or(-1)),
                truncation,
            }
        }
        Ok(Err(e)) => ShellOutcome {
            command: command_owned,
            stdout: String::from_utf8_lossy(&stdout_buf).to_string(),
            stderr: String::from_utf8_lossy(&stderr_buf).to_string(),
            result: ShellResult::SpawnError(e.to_string()),
            truncation: None,
        },
        Err(_elapsed) => {
            // D-10/D-11: timed out — best-effort kill (a lingering zombie
            // here is not worse than the pre-existing foreground path in
            // `terminal.rs`, which has the same property) and keep
            // whatever partial output the two `read_to_end` calls already
            // buffered — never discarded.
            let _ = child.start_kill();
            let stdout = String::from_utf8_lossy(&stdout_buf).to_string();
            let stderr = String::from_utf8_lossy(&stderr_buf).to_string();
            let (stdout, stderr, truncation) = truncate_combined(stdout, stderr, OUTPUT_CAP_BYTES);
            ShellOutcome {
                command: command_owned,
                stdout,
                stderr,
                result: ShellResult::TimedOut {
                    after_secs: timeout_secs,
                },
                truncation,
            }
        }
    }
}

// ── Task 2: transcript rendering (plain/styled pair) ────────────────────────

/// Plain (unstyled) logical lines for `run` — echo, each stdout/stderr row,
/// the truncation marker (if any), and the footer — in RENDER ORDER. Single
/// source of truth shared by `shell_block_lines` (styling) and
/// `App::apply_shell_outcome` (the D-11 text injected into `App.history`),
/// so wrap-math, the styled render, and the LLM-facing text can never drift
/// from each other (PATTERNS.md "Plain-text/styled-text split" discipline).
pub fn shell_block_plain(run: &ShellRun) -> Vec<String> {
    let mut lines = vec![format!("$ {}", run.command)];
    match &run.state {
        ShellRunState::Running => {
            lines.push("Running…".to_string());
        }
        ShellRunState::Done(outcome) => {
            for l in outcome.stdout.lines() {
                lines.push(l.to_string());
            }
            for l in outcome.stderr.lines() {
                lines.push(l.to_string());
            }
            if let Some(t) = &outcome.truncation {
                lines.push(format!(
                    "[output truncated — {} of {} bytes shown]",
                    t.shown, t.total
                ));
            }
            lines.push(footer_text(outcome));
        }
    }
    lines
}

/// The footer line's TEXT (state-dependent — EMPTY/SUCCEEDED/FAILED/TIMED
/// OUT/spawn error, per UI-SPEC §3). Shared by `shell_block_plain` and
/// `shell_block_lines` so the two can never disagree on the wording.
fn footer_text(outcome: &ShellOutcome) -> String {
    match &outcome.result {
        ShellResult::Exited(0) => {
            if outcome.stdout.is_empty() && outcome.stderr.is_empty() {
                "[exit 0, no output]".to_string()
            } else {
                "[exit 0]".to_string()
            }
        }
        ShellResult::Exited(code) => format!("[exit {code}]"),
        ShellResult::TimedOut { after_secs } => format!("[timed out after {after_secs}s]"),
        ShellResult::Refused => {
            unreachable!(
                "ShellRunState::Done never wraps a Refused outcome — refusal short-circuits \
                 before a ShellRun is ever created (App::dispatch_bang_blocking)"
            )
        }
        ShellResult::SpawnError(e) => format!("[failed to run: {e}]"),
    }
}

/// Styled `Line`s for `run`, in the EXACT same order/count as
/// `shell_block_plain` — never merges or splits rows across the two
/// (PATTERNS.md "Plain-text/styled-text split" discipline). Echo:
/// `Modifier::BOLD` + `Color::Magenta`. Running placeholder: static
/// `Modifier::DIM` (no spinner — the existing knight-rider row is the sole
/// motion primitive, Motion Contract). stdout: default style. stderr:
/// `Color::Red`. Truncation marker: `Modifier::DIM`. Footer: `Modifier::DIM`
/// on a clean `[exit 0]`/no-output success, `Color::Red` on failure/timeout/
/// spawn-error (UI-SPEC §3).
pub fn shell_block_lines(run: &ShellRun) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    out.push(Line::from(Span::styled(
        format!("$ {}", run.command),
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    )));
    match &run.state {
        ShellRunState::Running => {
            out.push(Line::from(Span::styled(
                "Running…".to_string(),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        ShellRunState::Done(outcome) => {
            for l in outcome.stdout.lines() {
                out.push(Line::from(Span::raw(l.to_string())));
            }
            for l in outcome.stderr.lines() {
                out.push(Line::from(Span::styled(
                    l.to_string(),
                    Style::default().fg(Color::Red),
                )));
            }
            if let Some(t) = &outcome.truncation {
                out.push(Line::from(Span::styled(
                    format!("[output truncated — {} of {} bytes shown]", t.shown, t.total),
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            let footer = footer_text(outcome);
            let footer_style = match &outcome.result {
                ShellResult::Exited(0) => Style::default().add_modifier(Modifier::DIM),
                ShellResult::Exited(_) | ShellResult::TimedOut { .. } => {
                    Style::default().fg(Color::Red)
                }
                ShellResult::Refused => unreachable!("see footer_text"),
                ShellResult::SpawnError(_) => Style::default().fg(Color::Red),
            };
            out.push(Line::from(Span::styled(footer, footer_style)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Task 1 ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn shell_bang_captures_stdout_and_exit_code() {
        let outcome = run("echo hello").await;
        assert_eq!(outcome.result, ShellResult::Exited(0));
        assert!(
            outcome.stdout.contains("hello"),
            "stdout should contain 'hello': {:?}",
            outcome.stdout
        );
    }

    #[tokio::test]
    async fn shell_bang_inherits_operator_env() {
        // SAFETY: test-only, single-threaded env mutation scoped to this
        // process; no other test in this crate reads/writes this var name.
        unsafe {
            std::env::set_var("SHELL_BANG_TEST_VAR", "canary-value-36-6-4");
        }
        let outcome = run("echo $SHELL_BANG_TEST_VAR").await;
        unsafe {
            std::env::remove_var("SHELL_BANG_TEST_VAR");
        }
        assert_eq!(outcome.result, ShellResult::Exited(0));
        assert!(
            outcome.stdout.contains("canary-value-36-6-4"),
            "D-09: `!` must see the operator's real env var, proving the \
             environment-scrubbing step is genuinely absent — got stdout: {:?}",
            outcome.stdout
        );
    }

    #[tokio::test]
    async fn interactive_command_refused_before_spawn() {
        let outcome = run("vim file.txt").await;
        assert_eq!(outcome.result, ShellResult::Refused);
        assert!(outcome.stdout.is_empty());
        assert!(outcome.stderr.is_empty());
    }

    #[tokio::test]
    async fn interactive_command_denylist_matches_first_token_only() {
        // "vim" appears only in the ARGUMENT, not the first token ("echo") —
        // must run normally, not be refused.
        let outcome = run("echo vim").await;
        assert_eq!(outcome.result, ShellResult::Exited(0));
        assert!(outcome.stdout.contains("vim"));
    }

    #[test]
    fn shell_bang_truncates_at_cap_on_char_boundary_with_marker_at_cut_point() {
        // Build a string where the naive byte-cap would land mid-codepoint:
        // 9 ASCII 'a's then a 2-byte 'é' (U+00E9). Cap at 10 bytes would
        // split 'é' in half if we didn't walk back to a boundary.
        let text = format!("{}é", "a".repeat(9));
        assert_eq!(text.len(), 11); // 9 + 2 bytes
        let (shown, info) = truncate_with_marker(&text, 10);
        assert!(text.is_char_boundary(shown.len()) || shown.len() <= 9);
        assert!(
            shown.len() <= 9,
            "must walk back BEFORE the 2-byte char rather than split it: {shown:?}"
        );
        let info = info.expect("11 bytes > cap of 10 must report truncation");
        assert_eq!(info.total, 11);
        assert_eq!(info.shown, shown.len());
        assert!(String::from_utf8(shown.clone().into_bytes()).is_ok());

        // Marker placement: `truncate_combined` puts the marker line
        // immediately after the truncated content (in shell_block_plain),
        // never appended onto the same string as content — proven by the
        // fact that `shown` above contains ONLY content, no marker text.
        assert!(!shown.contains("truncated"));
    }

    #[tokio::test]
    async fn shell_bang_timeout_preserves_partial_output() {
        let outcome = run_with_timeout("echo partial; sleep 5", 1).await;
        assert_eq!(
            outcome.result,
            ShellResult::TimedOut { after_secs: 1 },
            "got: {:?}",
            outcome.result
        );
        assert!(
            outcome.stdout.contains("partial"),
            "partial output captured before the deadline must survive: {:?}",
            outcome.stdout
        );
    }

    // ── Task 2: shell_block_plain / shell_block_lines (pure render pair) ──

    fn done_run(command: &str, outcome: ShellOutcome) -> ShellRun {
        ShellRun {
            command: command.to_string(),
            state: ShellRunState::Done(outcome),
            history_anchor: 0,
        }
    }

    #[test]
    fn shell_bang_running_state_shows_static_dim_placeholder_no_spinner() {
        let run = ShellRun {
            command: "sleep 30".to_string(),
            state: ShellRunState::Running,
            history_anchor: 0,
        };
        let lines = shell_block_lines(&run);
        assert_eq!(
            lines.len(),
            2,
            "RUNNING must be exactly echo + one static placeholder line — no \
             second animated primitive"
        );
        let placeholder_text: String = lines[1]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(placeholder_text, "Running…");
        assert!(
            lines[1].spans[0].style.add_modifier.contains(Modifier::DIM),
            "placeholder must be DIM"
        );

        let plain = shell_block_plain(&run);
        assert_eq!(plain, vec!["$ sleep 30".to_string(), "Running…".to_string()]);
    }

    #[test]
    fn shell_bang_stderr_renders_red_stdout_default() {
        let outcome = ShellOutcome {
            command: "mycmd".to_string(),
            stdout: "out-line".to_string(),
            stderr: "err-line".to_string(),
            result: ShellResult::Exited(1),
            truncation: None,
        };
        let run = done_run("mycmd", outcome);
        let lines = shell_block_lines(&run);
        // [echo, stdout row, stderr row, footer]
        assert_eq!(lines.len(), 4);
        let stdout_style = lines[1].spans[0].style;
        assert!(
            stdout_style.fg.is_none() || stdout_style.fg == Some(Color::Reset),
            "stdout must use default style: {stdout_style:?}"
        );
        let stderr_style = lines[2].spans[0].style;
        assert_eq!(stderr_style.fg, Some(Color::Red), "stderr must be Color::Red");
    }

    #[test]
    fn shell_bang_exit_footer_never_wraps_into_output_line() {
        let outcome = ShellOutcome {
            command: "mycmd".to_string(),
            stdout: "a very long line of stdout output that could wrap at a narrow width"
                .to_string(),
            stderr: String::new(),
            result: ShellResult::Exited(0),
            truncation: None,
        };
        let run = done_run("mycmd", outcome);
        let plain = shell_block_plain(&run);
        // [echo, stdout, footer] — footer is its OWN entry, never appended
        // onto the stdout string.
        assert_eq!(plain.len(), 3);
        assert_eq!(plain[2], "[exit 0]");
        assert!(
            !plain[1].contains("[exit"),
            "footer text must never be concatenated onto the output line"
        );

        let lines = shell_block_lines(&run);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn shell_bang_truncation_marker_appears_at_cut_point_not_only_at_eof() {
        let outcome = ShellOutcome {
            command: "mycmd".to_string(),
            stdout: "kept-content".to_string(),
            stderr: String::new(),
            result: ShellResult::Exited(0),
            truncation: Some(TruncationInfo {
                shown: 12,
                total: 9000,
            }),
        };
        let run = done_run("mycmd", outcome);
        let plain = shell_block_plain(&run);
        // [echo, stdout, marker, footer] — the marker sits BETWEEN the
        // truncated content and the exit footer (at the cut point), not
        // merged into either.
        assert_eq!(plain.len(), 4);
        assert_eq!(plain[1], "kept-content");
        assert_eq!(plain[2], "[output truncated — 12 of 9000 bytes shown]");
        assert_eq!(plain[3], "[exit 0]");
    }

    #[test]
    fn empty_output_renders_no_output_footer_without_blank_gap() {
        let outcome = ShellOutcome {
            command: "true".to_string(),
            stdout: String::new(),
            stderr: String::new(),
            result: ShellResult::Exited(0),
            truncation: None,
        };
        let run = done_run("true", outcome);
        let plain = shell_block_plain(&run);
        assert_eq!(plain, vec!["$ true".to_string(), "[exit 0, no output]".to_string()]);
    }

    #[test]
    fn timeout_footer_is_red_and_replaces_exit_footer() {
        let outcome = ShellOutcome {
            command: "sleep 60".to_string(),
            stdout: "some partial output".to_string(),
            stderr: String::new(),
            result: ShellResult::TimedOut { after_secs: 30 },
            truncation: None,
        };
        let run = done_run("sleep 60", outcome);
        let lines = shell_block_lines(&run);
        let footer_style = lines.last().unwrap().spans[0].style;
        assert_eq!(footer_style.fg, Some(Color::Red));
        let plain = shell_block_plain(&run);
        assert_eq!(plain.last().unwrap(), "[timed out after 30s]");
    }
}
