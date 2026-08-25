//! Transcript text selection + OSC52 clipboard write (Phase 36.6.4 Plan 01).
//!
//! D-04/D-06/D-08: mouse-drag selection with mouse capture ON, copied to the
//! system clipboard over OSC52 (write-only, works over SSH — no native
//! clipboard dependency chain). See `App::handle_mouse`'s `Down`/`Drag`/
//! `Up(Left)` arms (`app.rs`) for the dispatch that drives this module.
//!
//! ## Additive macOS `pbcopy` write (gap-closure, operator evidence)
//! D-06's own text names this as reversible: "adding a native fallback
//! later is additive." OSC52 has since proven insufficient on macOS
//! Terminal.app (the operator's daily driver, which has never implemented
//! OSC52 and silently discards the escape sequence). `yank` now ALSO shells
//! out to `pbcopy` on macOS when the session is local (never over SSH — see
//! `is_remote_session`), strictly alongside the unconditional OSC52 write,
//! never instead of it. No new dependency: `pbcopy` is invoked via
//! `std::process::Command`, not a native clipboard crate — D-06's actual
//! rationale (avoiding the arboard/copypasta/x11/wayland/objc dependency
//! chain) still stands.
//!
//! ## tmux setup note (RESEARCH Assumption A4, D-06)
//! OSC52 writes are silently dropped by tmux unless `set -g set-clipboard on`
//! is configured, and some tmux configurations additionally require OSC52
//! passthrough enabled. This module does not attempt in-app detection or DCS
//! passthrough wrapping — it is documented here and in the Plan 06 Help
//! overlay entry only.
//!
//! ## Coordinate model
//! `ContentPos { row, col }` is a VIRTUAL CONTENT coordinate: `row` indexes
//! the full wrapped transcript (the same row space `ScrollViewState`'s
//! vertical offset lives in), `col` is a CHARACTER index (not a byte offset,
//! not a display-cell column) into that row's plain text. Using `char`
//! indices throughout (the same unicode granularity
//! `app::word_wrapped_line_count` already assumes elsewhere in this crate)
//! means a selection boundary can never land "inside" a multi-byte or wide
//! glyph — a `char` is Rust's atomic unit, so any valid `col` value names a
//! boundary BETWEEN characters, never inside one. This phase does not
//! introduce a `unicode-segmentation` dependency (D-17's dependency list is
//! deliberately narrow); true extended-grapheme-cluster splitting (e.g.
//! skin-tone-modified emoji ZWJ sequences, which are several `char`s) is the
//! one theoretical edge this does not fully cover.

use std::time::{Duration, Instant};

use ratatui::layout::{Position, Rect};

// ── Coordinate model ─────────────────────────────────────────────────────────

/// A position in virtual content coordinates. See module doc for the
/// coordinate model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContentPos {
    /// Row index within the FULL wrapped transcript (not the viewport).
    pub row: usize,
    /// Character index within that row's plain (unstyled) text.
    pub col: usize,
}

impl ContentPos {
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// An active or completed text selection. `anchor` is where the press
/// started; `cursor` is the endpoint the drag is currently extending toward
/// (or where it ended). Order is not normalized here — `selected_text`
/// normalizes at extraction time so a backward drag (cursor before anchor)
/// still yields the correct forward-reading text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: ContentPos,
    pub cursor: ContentPos,
}

impl Selection {
    /// A zero-length selection anchored (and cursored) at `pos` — the state
    /// `Down(Left)` seeds before any `Drag` event has arrived.
    pub const fn new_at(pos: ContentPos) -> Self {
        Self {
            anchor: pos,
            cursor: pos,
        }
    }

    /// True when anchor and cursor coincide — D-04/UI-SPEC's "empty
    /// selection" case: a yank must be a silent no-op.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Normalized (start, end) — start is topmost/leftmost regardless of
    /// which endpoint the drag physically began at.
    fn ordered(&self) -> (ContentPos, ContentPos) {
        let a = (self.anchor.row, self.anchor.col);
        let c = (self.cursor.row, self.cursor.col);
        if a <= c {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }
}

// ── Vim-style keyboard visual mode (Phase 36.6.4 Plan 02, D-05) ─────────────

/// Keyboard-driven selection mode — the SSH-safe fallback for D-04's
/// mouse-drag selection: mouse events do not reliably survive every
/// SSH/tmux configuration. `Idle` = no keyboard selection in progress (a
/// mouse-drag-established `Selection` on `App` lives independently of this
/// enum and stays yankable via `Ctrl+Y` from either mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    #[default]
    Idle,
    Visual,
}

/// A single-step cursor movement direction for vim-style `hjkl`/arrow
/// extension. `h`/`Left` and `j`/`Down` etc. both map to the SAME variant
/// for a given direction — `App::handle_key` collapses the vim letter and
/// the arrow key onto this one enum before calling `move_cursor`, so there
/// is exactly one movement behavior per direction regardless of which
/// physical key produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveDir {
    Up,
    Down,
    Left,
    Right,
}

/// Pure cursor movement for visual-mode extension — advances `cursor` one
/// step in `dir`, leaving the caller's `anchor` untouched (the caller never
/// passes `anchor` here; `App::handle_key` mutates only `selection.cursor`).
/// `max_row` is the transcript's last wrapped-row index (`App`-supplied,
/// since only `App` knows the live render width) — there is no upper clamp
/// on `col` here because `selected_text`'s `.min(chars.len())` already
/// clamps an out-of-range column at extraction time, the same
/// "clamp at the site that knows the true bound" discipline
/// `transcript_max_scroll` uses elsewhere in this crate. Saturating
/// arithmetic throughout so a press at row/col 0 never underflows.
pub fn move_cursor(cursor: ContentPos, dir: MoveDir, max_row: usize) -> ContentPos {
    match dir {
        MoveDir::Up => ContentPos {
            row: cursor.row.saturating_sub(1),
            col: cursor.col,
        },
        MoveDir::Down => ContentPos {
            row: cursor.row.saturating_add(1).min(max_row),
            col: cursor.col,
        },
        MoveDir::Left => ContentPos {
            row: cursor.row,
            col: cursor.col.saturating_sub(1),
        },
        MoveDir::Right => ContentPos {
            row: cursor.row,
            col: cursor.col.saturating_add(1),
        },
    }
}

/// The single VIEWPORT-mappable content position that should additionally
/// carry `Modifier::BOLD` on top of the range's `Modifier::REVERSED`
/// (UI-SPEC §2 "Anchor/cursor visual") — the highlighted cell nearest
/// `sel.cursor`. `ContentPos::col` is an EXCLUSIVE char boundary (module
/// doc), not a selected character itself, so a forward-extending cursor
/// (`cursor` is the range's END) bolds the LAST actually-selected cell one
/// step back from that boundary; a backward-extending cursor (`cursor` is
/// the range's START) bolds the cell AT the boundary, which is already
/// inclusive/selected. Returns `None` for an empty selection — nothing to
/// bold when there's no highlighted range at all.
pub fn cursor_highlight_cell(sel: &Selection) -> Option<ContentPos> {
    if sel.is_empty() {
        return None;
    }
    let (start, end) = sel.ordered();
    if sel.cursor == end {
        Some(if end.col == 0 {
            end
        } else {
            ContentPos::new(end.row, end.col - 1)
        })
    } else {
        Some(start)
    }
}

// ── Click-count granularity (Phase 36.6.4 Plan 02 Task 2, D-07) ─────────────

/// The granularity a press-count resolves to: 1 = character-range anchor
/// (Plan 01's original behavior), 2 = the word under the cursor, 3 = the
/// full displayed (wrapped) line. A 4th press wraps back to `Char` rather
/// than stranding the operator in an unexpected mode (see `classify_click`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickGranularity {
    Char,
    Word,
    Line,
}

/// Pure click-count classification. `previous` is the last recorded press
/// (content position, timestamp, and the click count 1..=3 it resolved to),
/// if any; `pos`/`now` are the new press's content position and timestamp.
/// `window` is the double/triple-click time budget — a named, documented
/// constant at the `App` call site (no platform double-click setting is
/// visible to a TUI, so the exact duration is Claude's discretion per the
/// plan's `planner_assumptions`; this fn only pins the BEHAVIOR).
///
/// A press escalates the count ONLY when it lands in the SAME content cell
/// as the previous press AND within `window` of it; any other press resets
/// to count 1. Count 4 wraps back to 1 (never strands the operator in an
/// unexpected mode). Returns `(granularity, count)` — the count is handed
/// back so the caller can remember it for the NEXT press without
/// re-deriving it.
pub fn classify_click(
    previous: Option<(ContentPos, Instant, u8)>,
    pos: ContentPos,
    now: Instant,
    window: Duration,
) -> (ClickGranularity, u8) {
    let count = match previous {
        Some((prev_pos, prev_time, prev_count))
            if prev_pos == pos && now.saturating_duration_since(prev_time) <= window =>
        {
            if prev_count >= 3 { 1 } else { prev_count + 1 }
        }
        _ => 1,
    };
    let granularity = match count {
        1 => ClickGranularity::Char,
        2 => ClickGranularity::Word,
        _ => ClickGranularity::Line,
    };
    (granularity, count)
}

/// Word boundary resolution reusing the crate's existing wrap word-splitting
/// convention (`char::is_whitespace()` — the same predicate
/// `app::word_wrapped_line_count` uses) rather than a second tokenizer, per
/// UI-SPEC §2. Returns the `[start, end)` CHAR-INDEX range of the run
/// containing `col` (clamped to the row's length); `col` landing on
/// whitespace selects the whitespace run itself (matches conventional
/// double-click-on-space behavior; still pure and testable). An empty
/// `row_text` returns `(0, 0)`.
pub fn word_range_at(row_text: &str, col: usize) -> (usize, usize) {
    let chars: Vec<char> = row_text.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let col = col.min(chars.len() - 1);
    let is_ws = chars[col].is_whitespace();
    let mut start = col;
    while start > 0 && chars[start - 1].is_whitespace() == is_ws {
        start -= 1;
    }
    let mut end = col + 1;
    while end < chars.len() && chars[end].is_whitespace() == is_ws {
        end += 1;
    }
    (start, end)
}

/// The full displayed (wrapped) row — triple-click's target (UI-SPEC §2:
/// "the wrapped DISPLAY row under the click, matching what triple-click
/// means in every terminal emulator, not the logical pre-wrap line").
/// Returns `[0, row_text.chars().count())`.
pub fn line_range_at(row_text: &str) -> (usize, usize) {
    (0, row_text.chars().count())
}

// ── Coordinate mapping ───────────────────────────────────────────────────────

/// Map a viewport mouse cell to virtual content coordinates.
///
/// `area` is the OUTER transcript block `Rect` (border included — the same
/// convention `App::handle_mouse`'s existing bounds check and
/// `rebuild_chip_hit_test` already use); `scroll_offset` is
/// `ScrollViewState::offset()` for the render the click landed on.
///
/// Saturating arithmetic throughout (TUI-SEL-01/precision) — a resize or
/// scroll mid-drag can never wrap a `u16` or invert a range. A click on the
/// border itself saturates to content row/col 0 rather than underflowing.
pub fn content_pos_at(area: Rect, scroll_offset: Position, mouse_col: u16, mouse_row: u16) -> ContentPos {
    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let viewport_col = mouse_col.saturating_sub(inner_x);
    let viewport_row = mouse_row.saturating_sub(inner_y);
    ContentPos {
        row: (scroll_offset.y as usize).saturating_add(viewport_row as usize),
        col: (scroll_offset.x as usize).saturating_add(viewport_col as usize),
    }
}

// ── Extraction (pure) ────────────────────────────────────────────────────────

/// Extract the selected text out of `rendered_plain_rows` — one entry per
/// wrapped transcript row, in the SAME order/wrap-width the live render
/// uses (plain, unstyled text — no ANSI). Pure and unit-testable without a
/// terminal; the caller (`App`, via `App::transcript_rendered_plain_rows`)
/// is responsible for producing rows that match what's actually drawn.
///
/// Empty selection (`sel.is_empty()`) returns an empty string — never a
/// panic, never a spurious single character.
pub fn selected_text(rendered_plain_rows: &[String], sel: &Selection) -> String {
    if sel.is_empty() {
        return String::new();
    }
    let (start, end) = sel.ordered();
    let mut out = String::new();
    for row_idx in start.row..=end.row {
        let Some(line) = rendered_plain_rows.get(row_idx) else {
            break;
        };
        let chars: Vec<char> = line.chars().collect();
        let col_start = if row_idx == start.row {
            start.col.min(chars.len())
        } else {
            0
        };
        let col_end = if row_idx == end.row {
            end.col.min(chars.len())
        } else {
            chars.len()
        };
        if col_start < col_end {
            out.extend(chars[col_start..col_end].iter());
        }
        if row_idx != end.row {
            out.push('\n');
        }
    }
    out
}

// ── Clipboard write ──────────────────────────────────────────────────────────

/// Build the OSC52 command for `text` — pure, no I/O. Factored out so the
/// module's own tests can capture `Command::write_ansi` output (mirroring
/// crossterm's own `clipboard.rs` test pattern) without a live terminal.
fn build_clipboard_command(text: &str) -> crossterm::clipboard::CopyToClipboard<String> {
    crossterm::clipboard::CopyToClipboard::to_clipboard_from(text.to_string())
}

/// Write `text` to the system clipboard over OSC52. Module-private — the
/// ONLY caller family is the operator-initiated yank paths in `app.rs`
/// (mouse drag-release, future `Ctrl+Y`). Never reachable from model-,
/// tool-, or transcript-supplied text directly (T-36.6.4-OSC52-01).
///
/// Never logs or echoes `text` (T-36.6.4-OSC52-04) — the operator copies
/// passwords and tokens through this path.
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    crossterm::execute!(std::io::stdout(), build_clipboard_command(text))
}

/// OSC52 payload cap, in `char`s (D-06, RESEARCH Assumption A1). The
/// ~74-100KB figure is CONTEXT-supplied and corroborated only to
/// order-of-magnitude — some terminals historically capped far lower. A
/// named, source-cited constant rather than a bare number so a wrong
/// magnitude is a one-line change. This is a CEILING, not a target:
/// truncate-and-tell fires correctly regardless of the exact value (D-06 —
/// a silent drop is never acceptable).
const OSC52_PAYLOAD_CAP_CHARS: usize = 74_000;

/// Observed outcome of the ADDITIVE macOS `pbcopy` write (gap-closure,
/// operator evidence — see module doc). OSC52 never acks a write, so this
/// is the one field on `CopyReport` with a real signal behind it: `pbcopy`
/// gives an exit status.
///
/// `NotAttempted` covers both "not macOS" and "SSH session" (D2) — the
/// caller does not need to distinguish those cases, only whether the
/// native write happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeClipboardOutcome {
    /// Non-macOS build, or a remote (SSH) session — `pbcopy` was never
    /// invoked.
    NotAttempted,
    /// `pbcopy` ran and exited successfully.
    Confirmed,
    /// `pbcopy` was attempted but failed (missing binary, spawn error,
    /// stdin write error, or non-zero exit). Degrades silently (D4) — never
    /// surfaced as an error, never turns a successful OSC52 write into an
    /// `Err` result.
    Failed,
}

/// Report carrying both the copied count and the original total, for the
/// truncate-and-tell status message (D-06, UI-SPEC §2 Copywriting Contract),
/// plus the observed [`NativeClipboardOutcome`] of the additive `pbcopy`
/// write. The two original fields are unchanged so every existing reader of
/// `report.copied`/`report.total` keeps compiling unmodified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyReport {
    pub copied: usize,
    pub total: usize,
    pub native_clipboard: NativeClipboardOutcome,
}

// ── OSC52 capability detection (Plan 08, gap-closure: honest toast wording) ──

/// The three possible verdicts on whether the current terminal implements
/// OSC52. OSC52 has no reply mechanism — a write can never be confirmed —
/// so `Unknown` is a load-bearing THIRD state, not a synonym for
/// `Unsupported`: an unrecognised terminal may well have accepted the
/// write, and collapsing it into `Unsupported` would assert a failure the
/// app never observed (the honesty defect this plan closes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc52Support {
    Supported,
    Unsupported,
    Unknown,
}

/// The detected OSC52 verdict plus a safe-to-render display name.
/// `display_name` is drawn from a fixed, compiled-in allowlist of `&'static str` literals — NEVER
/// the raw environment value — closing the ANSI/OSC status-line spoofing primitive
/// (T-36.6.4-G08-02): an operator- (or attacker-) influencable `TERM_PROGRAM` value can carry
/// arbitrary bytes, and this type guarantees none of them ever reach a rendered surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalClipboardCaps {
    pub support: Osc52Support,
    pub display_name: &'static str,
}

/// Classify the current terminal's OSC52 support from an injectable
/// environment lookup — tests drive this directly and never mutate real
/// process environment (mirrors `is_remote_session`'s pattern, for the same
/// reason: this crate already has env-mutating tests that poison a shared
/// lock on any panic).
///
/// The `Unsupported` arm is evaluated BEFORE the `Supported` arms so a
/// terminal that happens to set both cannot be talked into claiming
/// support — `Apple_Terminal` is checked first and wins outright.
pub fn detect_osc52_support(lookup: &dyn Fn(&str) -> Option<String>) -> TerminalClipboardCaps {
    let term_program = lookup("TERM_PROGRAM");

    if term_program.as_deref() == Some("Apple_Terminal") {
        return TerminalClipboardCaps {
            support: Osc52Support::Unsupported,
            display_name: "Terminal.app",
        };
    }

    let term = lookup("TERM").unwrap_or_default();
    let supported = match term_program.as_deref() {
        Some("iTerm.app") => Some("iTerm2"),
        Some("WezTerm") => Some("WezTerm"),
        Some("ghostty") => Some("Ghostty"),
        Some("Hyper") => Some("Hyper"),
        _ if term.starts_with("xterm-kitty") => Some("kitty"),
        _ if lookup("KITTY_WINDOW_ID").is_some() => Some("kitty"),
        _ if lookup("WT_SESSION").is_some() => Some("Windows Terminal"),
        _ => None,
    };
    if let Some(display_name) = supported {
        return TerminalClipboardCaps { support: Osc52Support::Supported, display_name };
    }

    if lookup("TMUX").is_some() {
        // tmux forwards OSC52 only when `set -g set-clipboard on` is
        // configured, which the app cannot read from here (D-06's
        // documented constraint) — genuinely indeterminate, not supported.
        return TerminalClipboardCaps { support: Osc52Support::Unknown, display_name: "tmux" };
    }

    TerminalClipboardCaps { support: Osc52Support::Unknown, display_name: "this terminal" }
}

/// Production entry point for [`detect_osc52_support`] — wraps `std::env::var`.
pub fn detect_osc52_support_from_env() -> TerminalClipboardCaps {
    detect_osc52_support(&|key| std::env::var(key).ok())
}

// ── ClipboardOutcome + copy_toast (Plan 08: the toast is a pure fn of evidence) ──

/// What `yank` observed, promoted to primary status over the old
/// `io::Result<Option<CopyReport>>` shape (Plan 08 assumption delta): the
/// toast becomes a pure function of this single type rather than a second
/// message path bolted alongside an unconditional one.
#[derive(Debug)]
pub enum ClipboardOutcome {
    /// D-04: an empty selection is a silent no-op — no write, no toast, no
    /// transcript line, no error.
    Empty,
    /// The OSC52 `execute!` call itself failed (rare — e.g. a broken stdout
    /// pipe). Never produced by a failing/missing `pbcopy` (D4) — only the
    /// OSC52 write itself can fail this way.
    WriteFailed(std::io::Error),
    /// A non-empty selection was written (OSC52 always fires; the native
    /// `pbcopy` write may or may not have). Carries both the counts/native
    /// outcome (`CopyReport`) and the detected OSC52 capability
    /// (`TerminalClipboardCaps`) `copy_toast` needs to word the toast.
    Written(CopyReport, TerminalClipboardCaps),
}

/// Compose the copy-confirmation status-line toast — pure, no I/O, table-testable.
///
/// **Branch order is load-bearing (2026-08-17 amendment).** The observed
/// native-clipboard outcome is checked FIRST and, when `Confirmed`, wins
/// OUTRIGHT regardless of the `Osc52Support` verdict: an observed result
/// outranks a guess. Getting this order backwards reintroduces the G-03 lie
/// in mirror image — telling the operator a copy failed while their
/// clipboard genuinely holds the text (the common case on Terminal.app,
/// where OSC52 is `Unsupported` but `pbcopy` confirms).
///
/// Only when nothing was observed via the native write (`Failed` — pbcopy
/// ran and did not exit 0 — or `NotAttempted` — off macOS, or any SSH
/// session) does the wording fall through to the three-state OSC52
/// heuristic below. A `Failed` native write must neither upgrade nor
/// further degrade the message versus `NotAttempted` with the same caps —
/// OSC52 was still attempted regardless and may have landed.
pub fn copy_toast(report: CopyReport, caps: TerminalClipboardCaps) -> String {
    if report.native_clipboard == NativeClipboardOutcome::Confirmed {
        return supported_wording(report);
    }
    match caps.support {
        Osc52Support::Supported => supported_wording(report),
        Osc52Support::Unknown => unknown_wording(report, caps.display_name),
        Osc52Support::Unsupported => unsupported_wording(caps.display_name),
    }
}

/// The unchanged shipped working-case wording (UI-SPEC Copywriting
/// Contract) — byte-identical whether it was reached via a `Confirmed`
/// native write or a `Supported` OSC52 verdict, so there is exactly one
/// working-case string, not two.
fn supported_wording(report: CopyReport) -> String {
    if report.copied < report.total {
        format!(
            "Copied {} of {} chars (terminal clipboard limit — truncated)",
            report.copied, report.total
        )
    } else {
        format!("Copied {} chars", report.copied)
    }
}

/// `Unknown`: OSC52 never acks, so the write may well have landed — the
/// wording must assert neither success nor failure, name what happened
/// (sent, not copied), and invite the operator to verify by pasting.
fn unknown_wording(report: CopyReport, display_name: &str) -> String {
    if report.copied < report.total {
        format!(
            "Sent {} of {} chars — {display_name} doesn't confirm clipboard receipt (truncated at the clipboard limit; paste to check)",
            report.copied, report.total
        )
    } else {
        format!("Sent {} chars — {display_name} doesn't confirm clipboard receipt; paste to check", report.copied)
    }
}

/// `Unsupported`: a terminal known NOT to implement OSC52 (and, since
/// `c87f882d0`, unreachable on macOS-local — `pbcopy` confirms there. This
/// governs a non-macOS terminal known not to implement OSC52, and SSH
/// sessions into one). Names the limitation via the allowlisted
/// `display_name`, never claims a copy occurred, and points at the
/// terminal's own selection as the working alternative — never at turning
/// mouse capture off (D-08, the operator's G-03 decision).
fn unsupported_wording(display_name: &str) -> String {
    format!(
        "{display_name} doesn't support terminal clipboard copy — select the text directly in {display_name} to copy it"
    )
}

/// Prepare `text` for an OSC52 write. Returns `None` for an empty selection
/// (D-04's silent no-op — no write at all). Otherwise truncates at the cap
/// on a `char` boundary (never mid-codepoint) and reports both counts —
/// truncate-and-tell is non-negotiable (D-06): a payload over the cap is
/// never silently dropped and never emitted unbounded.
///
/// `native_clipboard` starts as `NotAttempted` — `yank`/`yank_with` fill in
/// the real value once the native write (or the decision to skip it) has
/// actually happened, so this function stays pure/no-I/O.
fn prepare_payload(text: &str) -> Option<(String, CopyReport)> {
    if text.is_empty() {
        return None;
    }
    let total = text.chars().count();
    if total <= OSC52_PAYLOAD_CAP_CHARS {
        return Some((
            text.to_string(),
            CopyReport {
                copied: total,
                total,
                native_clipboard: NativeClipboardOutcome::NotAttempted,
            },
        ));
    }
    let truncated: String = text.chars().take(OSC52_PAYLOAD_CAP_CHARS).collect();
    Some((
        truncated,
        CopyReport {
            copied: OSC52_PAYLOAD_CAP_CHARS,
            total,
            native_clipboard: NativeClipboardOutcome::NotAttempted,
        },
    ))
}

/// True when the current session is remote (SSH) — the native `pbcopy`
/// write must never run in that case (D2): it would silently write to the
/// REMOTE host's clipboard, which is useless and more confusing than doing
/// nothing at all. `lookup` is an injectable environment reader so tests
/// exercise this without mutating real process environment (mirrors the
/// pattern Plan 08's `detect_osc52_support` uses for the same reason —
/// this crate already has env-mutating tests that poison a shared lock).
fn is_remote_session(lookup: &dyn Fn(&str) -> Option<String>) -> bool {
    ["SSH_CONNECTION", "SSH_TTY", "SSH_CLIENT"]
        .iter()
        .any(|key| lookup(key).is_some_and(|v| !v.is_empty()))
}

/// Run `binary`, writing `text` to its stdin and waiting for it to exit.
/// Returns the observed [`NativeClipboardOutcome`] plus whatever the child
/// wrote to stdout.
///
/// The stdout capture exists ONLY so tests can substitute a non-`pbcopy`
/// binary (`cat`, which echoes stdin back) and assert on the exact bytes
/// received — `pbcopy` itself writes no stdout, so the real call site
/// (`write_native_clipboard`) ignores it. This keeps the truncation and
/// failure-handling tests independent of a real `pbcopy` binary and of the
/// test host's OS.
fn run_clipboard_command(binary: &str, text: &str) -> (NativeClipboardOutcome, Vec<u8>) {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    let child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(_) => return (NativeClipboardOutcome::Failed, Vec::new()),
    };

    let Some(mut stdin) = child.stdin.take() else {
        return (NativeClipboardOutcome::Failed, Vec::new());
    };
    if stdin.write_all(text.as_bytes()).is_err() {
        let _ = child.wait();
        return (NativeClipboardOutcome::Failed, Vec::new());
    }
    // Explicitly drop `stdin` (closing the pipe) before reading stdout or
    // waiting, so a reader-blocked child (e.g. `cat` in tests) sees EOF and
    // exits rather than deadlocking.
    drop(stdin);

    let mut stdout_buf = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut stdout_buf);
    }

    match child.wait() {
        Ok(status) if status.success() => (NativeClipboardOutcome::Confirmed, stdout_buf),
        _ => (NativeClipboardOutcome::Failed, stdout_buf),
    }
}

/// Invoke the real `pbcopy` binary. `#[cfg(target_os = "macos")]`-gated so
/// a non-macOS BUILD compiles to exactly today's (OSC52-only) behaviour —
/// `pbcopy` is never referenced, let alone invoked, on other platforms
/// (D2). The `is_remote_session` gate is applied by the caller
/// (`yank`/`yank_with`), not here.
#[cfg(target_os = "macos")]
fn write_native_clipboard(text: &str) -> NativeClipboardOutcome {
    run_clipboard_command("pbcopy", text).0
}

/// Non-macOS stub — always `NotAttempted`, no process ever spawned.
#[cfg(not(target_os = "macos"))]
fn write_native_clipboard(_text: &str) -> NativeClipboardOutcome {
    NativeClipboardOutcome::NotAttempted
}

/// Testable core of `yank`. Split out so tests can inject the platform
/// gate, the environment lookup, the OSC52 writer, and the native writer —
/// exercising the macOS-local path, the SSH-skip path, the truncation
/// behaviour, and native-write failure, all without a real `pbcopy`, real
/// stdout I/O, or depending on the test host's OS (T1-T5).
///
/// D1: `osc52_write` fires unconditionally on every non-empty selection,
/// BEFORE the native-write decision is even evaluated — it is the only
/// mechanism that works over SSH, and the native write must never gate or
/// short-circuit it.
fn yank_with(
    text: &str,
    attempt_native: bool,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    osc52_write: &dyn Fn(&str) -> std::io::Result<()>,
    native_write: &dyn Fn(&str) -> NativeClipboardOutcome,
) -> std::io::Result<Option<CopyReport>> {
    let Some((payload, mut report)) = prepare_payload(text) else {
        return Ok(None);
    };
    osc52_write(&payload)?;
    report.native_clipboard = if attempt_native && !is_remote_session(env_lookup) {
        native_write(&payload)
    } else {
        NativeClipboardOutcome::NotAttempted
    };
    Ok(Some(report))
}

/// Public entry point for the yank paths in `app.rs`. Returns a
/// [`ClipboardOutcome`] — Plan 08 promotes this to the primary
/// representation so `copy_toast` (and, before it, `App::apply_clipboard_outcome`)
/// is a pure function of what was actually observed, rather than a second
/// message path bolted alongside an unconditional toast.
///
/// Empty selections are a silent no-op (D-04): `ClipboardOutcome::Empty`, no
/// write, no error. `ClipboardOutcome::WriteFailed` is returned ONLY when
/// the OSC52 write itself fails (D4) — a failing/missing `pbcopy` never
/// turns this into a failure, it is folded into `report.native_clipboard`
/// on the `Written` variant instead. `Written` carries both the report and
/// the OSC52 capability detected via [`detect_osc52_support_from_env`], the
/// two independent signals `copy_toast` reconciles into one honest string.
///
/// `yank_with`'s injectable seam (env lookup, OSC52 writer, native writer)
/// is unchanged and untouched by this promotion — only this thin wrapper's
/// return type changed.
pub fn yank(text: &str) -> ClipboardOutcome {
    let result = yank_with(
        text,
        cfg!(target_os = "macos"),
        &|key| std::env::var(key).ok(),
        &copy_to_clipboard,
        &write_native_clipboard,
    );
    match result {
        Ok(None) => ClipboardOutcome::Empty,
        Ok(Some(report)) => ClipboardOutcome::Written(report, detect_osc52_support_from_env()),
        Err(e) => ClipboardOutcome::WriteFailed(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use crossterm::Command;

    fn decode_osc52(buf: &str) -> Vec<u8> {
        let prefix = "\x1b]52;c;";
        let suffix = "\x1b\\";
        assert!(buf.starts_with(prefix), "unexpected OSC52 prefix: {buf:?}");
        assert!(buf.ends_with(suffix), "unexpected OSC52 suffix: {buf:?}");
        let b64 = &buf[prefix.len()..buf.len() - suffix.len()];
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .expect("valid base64 payload")
    }

    #[test]
    fn osc52_write_emits_expected_sequence() {
        let text = "hello, selection!";
        let cmd = build_clipboard_command(text);
        let mut buf = String::new();
        cmd.write_ansi(&mut buf).unwrap();

        let decoded = decode_osc52(&buf);
        assert_eq!(decoded, text.as_bytes());
        // No raw newline or raw control byte anywhere in the emitted
        // sequence outside the OSC52 framing escapes themselves.
        let body = &buf[7..buf.len() - 2]; // strip "\x1b]52;c;" and "\x1b\\"
        assert!(
            !body.chars().any(|c| c.is_control()),
            "base64 body must contain no raw control bytes: {body:?}"
        );
        assert!(!buf.contains('\n'), "emitted sequence must contain no raw newline");
    }

    #[test]
    fn selection_is_empty_when_anchor_equals_cursor() {
        let pos = ContentPos::new(2, 5);
        let sel = Selection::new_at(pos);
        assert!(sel.is_empty());
        assert_eq!(selected_text(&["irrelevant".to_string()], &sel), "");
    }

    #[test]
    fn selected_text_single_row_forward_drag() {
        let rows = vec!["hello world".to_string()];
        let sel = Selection {
            anchor: ContentPos::new(0, 0),
            cursor: ContentPos::new(0, 5),
        };
        assert_eq!(selected_text(&rows, &sel), "hello");
    }

    #[test]
    fn selected_text_single_row_backward_drag_normalizes() {
        let rows = vec!["hello world".to_string()];
        // Drag started at col 5, ended at col 0 — cursor before anchor.
        let sel = Selection {
            anchor: ContentPos::new(0, 5),
            cursor: ContentPos::new(0, 0),
        };
        assert_eq!(selected_text(&rows, &sel), "hello");
    }

    #[test]
    fn selected_text_spans_multiple_rows() {
        let rows = vec!["first row".to_string(), "second row".to_string()];
        let sel = Selection {
            anchor: ContentPos::new(0, 6),
            cursor: ContentPos::new(1, 6),
        };
        assert_eq!(selected_text(&rows, &sel), "row\nsecond");
    }

    #[test]
    fn content_pos_at_maps_border_inset_cell() {
        let area = Rect::new(0, 0, 80, 24);
        let pos = content_pos_at(area, Position::new(0, 0), 5, 3);
        // Border consumes (area.x+1, area.y+1); mouse (5,3) -> content (4,2).
        assert_eq!(pos, ContentPos::new(2, 4));
    }

    #[test]
    fn content_pos_at_adds_scroll_offset() {
        let area = Rect::new(0, 0, 80, 24);
        let pos = content_pos_at(area, Position::new(0, 10), 5, 3);
        assert_eq!(pos, ContentPos::new(12, 4));
    }

    #[test]
    fn content_pos_at_saturates_on_border_click() {
        let area = Rect::new(0, 0, 80, 24);
        // Click directly on the top-left border corner — must not underflow.
        let pos = content_pos_at(area, Position::new(0, 0), 0, 0);
        assert_eq!(pos, ContentPos::new(0, 0));
    }

    // — Phase 36.6.4 Plan 01 Task 3 `<behavior>` tests (TDD) ────────────────

    #[test]
    fn osc52_write_truncates_at_cap_and_reports_counts() {
        let long: String = "x".repeat(super::OSC52_PAYLOAD_CAP_CHARS + 500);
        let (payload, report) = prepare_payload(&long).expect("non-empty text must prepare");
        assert_eq!(
            payload.chars().count(),
            super::OSC52_PAYLOAD_CAP_CHARS,
            "payload must be truncated to exactly the cap"
        );
        assert_eq!(report.copied, super::OSC52_PAYLOAD_CAP_CHARS);
        assert_eq!(report.total, long.chars().count());
        assert!(report.copied < report.total);
    }

    #[test]
    fn osc52_write_is_noop_on_empty_selection() {
        assert!(
            prepare_payload("").is_none(),
            "an empty selection must produce no payload at all — no write, no toast"
        );
    }

    #[test]
    fn osc52_payload_contains_no_raw_control_bytes() {
        // Embedded escape byte + newline — base64 encoding is itself the
        // mitigation (T-36.6.4-OSC52-02): the DECODED content is the
        // literal text unchanged, but the EMITTED sequence's base64 body
        // contains only base64-alphabet characters, so a crafted transcript
        // can never smuggle a second escape sequence or terminator through
        // the clipboard channel.
        let text = "line1\x1bline2\nline3";
        let (payload, _) = prepare_payload(text).unwrap();
        assert_eq!(payload, text, "under the cap, the payload must be byte-identical");

        let cmd = build_clipboard_command(&payload);
        let mut buf = String::new();
        cmd.write_ansi(&mut buf).unwrap();
        let decoded = decode_osc52(&buf);
        assert_eq!(
            decoded,
            text.as_bytes(),
            "decoded content must be the literal text, unstripped"
        );
        let body = &buf[7..buf.len() - 2];
        assert!(
            body.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
            "emitted base64 body must contain ONLY base64-alphabet characters, \
             never a raw control byte from the source text: {body:?}"
        );
    }

    // — Phase 36.6.4 Plan 02 Task 1 `<behavior>` tests (TDD) — pure helpers ──

    #[test]
    fn move_cursor_down_and_up_change_row_only() {
        let cursor = ContentPos::new(5, 5);
        assert_eq!(move_cursor(cursor, MoveDir::Down, 100), ContentPos::new(6, 5));
        assert_eq!(move_cursor(cursor, MoveDir::Up, 100), ContentPos::new(4, 5));
    }

    #[test]
    fn move_cursor_left_and_right_change_col_only() {
        let cursor = ContentPos::new(5, 5);
        assert_eq!(move_cursor(cursor, MoveDir::Left, 100), ContentPos::new(5, 4));
        assert_eq!(move_cursor(cursor, MoveDir::Right, 100), ContentPos::new(5, 6));
    }

    #[test]
    fn move_cursor_saturates_at_zero() {
        let cursor = ContentPos::new(0, 0);
        assert_eq!(move_cursor(cursor, MoveDir::Up, 100), ContentPos::new(0, 0));
        assert_eq!(move_cursor(cursor, MoveDir::Left, 100), ContentPos::new(0, 0));
    }

    #[test]
    fn move_cursor_down_clamps_at_max_row() {
        let cursor = ContentPos::new(10, 0);
        assert_eq!(move_cursor(cursor, MoveDir::Down, 10), ContentPos::new(10, 0));
    }

    #[test]
    fn cursor_highlight_cell_none_on_empty_selection() {
        let sel = Selection::new_at(ContentPos::new(2, 2));
        assert_eq!(cursor_highlight_cell(&sel), None);
    }

    #[test]
    fn cursor_highlight_cell_forward_selection_bolds_last_selected_cell() {
        // anchor=(0,8) cursor=(0,12) — end (exclusive) is col 12, so the
        // last actually-selected cell is col 11.
        let sel = Selection {
            anchor: ContentPos::new(0, 8),
            cursor: ContentPos::new(0, 12),
        };
        assert_eq!(cursor_highlight_cell(&sel), Some(ContentPos::new(0, 11)));
    }

    #[test]
    fn cursor_highlight_cell_backward_selection_bolds_the_start_boundary() {
        // anchor=(0,12) cursor=(0,8) — cursor IS the start, already inclusive.
        let sel = Selection {
            anchor: ContentPos::new(0, 12),
            cursor: ContentPos::new(0, 8),
        };
        assert_eq!(cursor_highlight_cell(&sel), Some(ContentPos::new(0, 8)));
    }

    // — Phase 36.6.4 Plan 02 Task 2 `<behavior>` tests (TDD) — pure helpers ──

    #[test]
    fn classify_click_same_cell_within_window_escalates_char_word_line() {
        let window = Duration::from_millis(500);
        let pos = ContentPos::new(3, 4);
        let t0 = Instant::now();

        let (g1, c1) = classify_click(None, pos, t0, window);
        assert_eq!((g1, c1), (ClickGranularity::Char, 1));

        let t1 = t0 + Duration::from_millis(100);
        let (g2, c2) = classify_click(Some((pos, t0, c1)), pos, t1, window);
        assert_eq!((g2, c2), (ClickGranularity::Word, 2));

        let t2 = t1 + Duration::from_millis(100);
        let (g3, c3) = classify_click(Some((pos, t1, c2)), pos, t2, window);
        assert_eq!((g3, c3), (ClickGranularity::Line, 3));
    }

    #[test]
    fn classify_click_fourth_press_wraps_to_char() {
        let window = Duration::from_millis(500);
        let pos = ContentPos::new(1, 1);
        let t0 = Instant::now();
        let (g, c) = classify_click(Some((pos, t0, 3)), pos, t0, window);
        assert_eq!((g, c), (ClickGranularity::Char, 1));
    }

    #[test]
    fn classify_click_different_cell_resets_to_char() {
        let window = Duration::from_millis(500);
        let prev_pos = ContentPos::new(1, 1);
        let new_pos = ContentPos::new(1, 2);
        let t0 = Instant::now();
        let (g, c) = classify_click(Some((prev_pos, t0, 2)), new_pos, t0, window);
        assert_eq!((g, c), (ClickGranularity::Char, 1));
    }

    #[test]
    fn classify_click_outside_window_resets_to_char() {
        let window = Duration::from_millis(500);
        let pos = ContentPos::new(1, 1);
        let t0 = Instant::now();
        let t_late = t0 + Duration::from_millis(501);
        let (g, c) = classify_click(Some((pos, t0, 2)), pos, t_late, window);
        assert_eq!((g, c), (ClickGranularity::Char, 1));
    }

    #[test]
    fn word_range_at_finds_word_boundaries() {
        // "hello world" — col 2 lands inside "hello" (0..5); col 7 lands
        // inside "world" (6..11).
        assert_eq!(word_range_at("hello world", 2), (0, 5));
        assert_eq!(word_range_at("hello world", 7), (6, 11));
    }

    #[test]
    fn word_range_at_on_whitespace_selects_the_whitespace_run() {
        // "hello world" — col 5 is the single space between the two words.
        assert_eq!(word_range_at("hello world", 5), (5, 6));
    }

    #[test]
    fn word_range_at_empty_row_returns_zero_zero() {
        assert_eq!(word_range_at("", 3), (0, 0));
    }

    #[test]
    fn line_range_at_returns_the_full_row() {
        assert_eq!(line_range_at("hello world"), (0, 11));
        assert_eq!(line_range_at(""), (0, 0));
    }

    #[test]
    fn word_range_at_respects_grapheme_boundaries() {
        // "a😀 bc" — "😀" is a single char; the word run containing it must
        // select exactly the glyph, never split mid-codepoint.
        let row = "a😀 bc";
        // chars: 'a'(0) '😀'(1) ' '(2) 'b'(3) 'c'(4)
        assert_eq!(word_range_at(row, 1), (0, 2));
    }

    #[test]
    fn selected_text_never_splits_a_grapheme_cluster() {
        // "😀" is a single Rust `char` despite being 4 UTF-8 bytes and 2
        // display cells wide. Because ContentPos::col is a CHAR index, a
        // boundary can only ever fall BETWEEN characters — this test proves
        // that structurally, not just by accident.
        let rows = vec!["a😀b".to_string()];
        let sel = Selection {
            anchor: ContentPos::new(0, 1),
            cursor: ContentPos::new(0, 2),
        };
        assert_eq!(
            selected_text(&rows, &sel),
            "😀",
            "the wide glyph must be copied whole, never split mid-codepoint"
        );
    }

    // — Additive macOS `pbcopy` write (gap-closure, operator evidence) ──────
    // T1-T5: all driven through `yank_with`'s injected closures so none of
    // these depend on a real `pbcopy` binary or on the test host being
    // macOS, per the task's explicit requirement.

    #[test]
    fn is_remote_session_detects_ssh_connection() {
        assert!(is_remote_session(&|key| if key == "SSH_CONNECTION" {
            Some("1.2.3.4 22 5.6.7.8 22".to_string())
        } else {
            None
        }));
    }

    #[test]
    fn is_remote_session_detects_ssh_tty() {
        assert!(is_remote_session(
            &|key| if key == "SSH_TTY" { Some("/dev/ttys0".to_string()) } else { None }
        ));
    }

    #[test]
    fn is_remote_session_detects_ssh_client() {
        assert!(is_remote_session(
            &|key| if key == "SSH_CLIENT" { Some("x".to_string()) } else { None }
        ));
    }

    #[test]
    fn is_remote_session_false_when_no_ssh_vars_set() {
        assert!(!is_remote_session(&|_| None));
    }

    #[test]
    fn is_remote_session_ignores_present_but_empty_values() {
        assert!(!is_remote_session(&|_| Some(String::new())));
    }

    #[test]
    fn run_clipboard_command_pipes_stdin_and_reports_success() {
        // `cat` echoes stdin to stdout and exits 0 — a ubiquitous stand-in
        // for `pbcopy` (which has no stdout) that still proves the real
        // spawn/pipe/wait plumbing without requiring `pbcopy` or a macOS
        // host.
        let (outcome, stdout) = run_clipboard_command("cat", "round trip");
        assert_eq!(outcome, NativeClipboardOutcome::Confirmed);
        assert_eq!(stdout, b"round trip");
    }

    #[test]
    fn run_clipboard_command_missing_binary_returns_failed() {
        let (outcome, stdout) = run_clipboard_command(
            "definitely-not-a-real-binary-ironhermes-clipboard-test",
            "hello",
        );
        assert_eq!(outcome, NativeClipboardOutcome::Failed);
        assert!(stdout.is_empty());
    }

    // T5: OSC52 keeps firing unconditionally on the macOS-local path,
    // alongside (never instead of) the native write (D1).
    #[test]
    fn osc52_still_fires_on_macos_local_path_alongside_native_write() {
        let osc52_called = std::cell::Cell::new(false);
        let native_called = std::cell::Cell::new(false);

        let result = yank_with(
            "hello",
            true, // simulate: compiled for macOS
            &|_| None, // simulate: not a remote session
            &|_text| {
                osc52_called.set(true);
                Ok(())
            },
            &|_text| {
                native_called.set(true);
                NativeClipboardOutcome::Confirmed
            },
        );

        let report = result.expect("must not error").expect("non-empty selection must report");
        assert!(osc52_called.get(), "OSC52 must still fire on the macOS-local path (D1)");
        assert!(native_called.get(), "the native write must also fire on the macOS-local path");
        assert_eq!(report.native_clipboard, NativeClipboardOutcome::Confirmed);
    }

    // T2: an empty selection performs NO clipboard write of either kind.
    #[test]
    fn empty_selection_performs_no_clipboard_write_of_either_kind() {
        let osc52_called = std::cell::Cell::new(false);
        let native_called = std::cell::Cell::new(false);

        let result = yank_with(
            "",
            true,
            &|_| None,
            &|_text| {
                osc52_called.set(true);
                Ok(())
            },
            &|_text| {
                native_called.set(true);
                NativeClipboardOutcome::Confirmed
            },
        );

        assert!(result.expect("must not error").is_none(), "empty selection must be Ok(None)");
        assert!(!osc52_called.get(), "empty selection must never trigger an OSC52 write");
        assert!(!native_called.get(), "empty selection must never trigger a native write");
    }

    // T3: a failing/missing `pbcopy` still yields the OSC52-success result
    // — `yank` never returns `Err` because the ADDITIVE write failed (D4).
    #[test]
    fn failing_native_write_still_yields_osc52_success_result() {
        let result = yank_with(
            "hello",
            true,
            &|_| None,
            &|_text| Ok(()),
            &|_text| NativeClipboardOutcome::Failed,
        );

        let report = result
            .expect("a failing pbcopy must never turn yank into Err")
            .expect("the OSC52 write succeeded, so Some(report) must still be returned");
        assert_eq!(report.native_clipboard, NativeClipboardOutcome::Failed);
        assert_eq!(report.copied, 5);
        assert_eq!(report.total, 5);
    }

    // T1: `pbcopy` receives the TRUNCATED payload, not the original, when
    // input exceeds the OSC52 cap — it must get exactly what OSC52 got.
    #[test]
    fn native_write_receives_the_truncated_payload_not_the_original() {
        let long: String = "x".repeat(OSC52_PAYLOAD_CAP_CHARS + 500);
        let received = std::cell::RefCell::new(String::new());

        let result = yank_with(
            &long,
            true,
            &|_| None,
            &|_text| Ok(()),
            &|text| {
                *received.borrow_mut() = text.to_string();
                NativeClipboardOutcome::Confirmed
            },
        );

        let report = result.expect("must not error").expect("non-empty selection must report");
        assert_eq!(report.copied, OSC52_PAYLOAD_CAP_CHARS);
        assert_eq!(report.total, long.chars().count());
        assert_eq!(
            received.borrow().chars().count(),
            OSC52_PAYLOAD_CAP_CHARS,
            "pbcopy must receive exactly the truncated payload"
        );
        assert_ne!(
            *received.borrow(),
            long,
            "pbcopy must never receive the untruncated original"
        );
    }

    #[test]
    fn native_write_not_attempted_when_platform_gate_is_false() {
        // Simulates a non-macOS build: `attempt_native = false`.
        let native_called = std::cell::Cell::new(false);
        let result = yank_with(
            "hello",
            false,
            &|_| None,
            &|_text| Ok(()),
            &|_text| {
                native_called.set(true);
                NativeClipboardOutcome::Confirmed
            },
        );
        let report = result.expect("must not error").expect("non-empty selection must report");
        assert!(!native_called.get(), "non-macOS builds must never invoke the native writer");
        assert_eq!(report.native_clipboard, NativeClipboardOutcome::NotAttempted);
    }

    #[test]
    fn native_write_not_attempted_when_session_is_remote_even_on_macos() {
        let native_called = std::cell::Cell::new(false);
        let result = yank_with(
            "hello",
            true, // simulate: compiled for macOS
            &|key| if key == "SSH_CONNECTION" { Some("x".to_string()) } else { None },
            &|_text| Ok(()),
            &|_text| {
                native_called.set(true);
                NativeClipboardOutcome::Confirmed
            },
        );
        let report = result.expect("must not error").expect("non-empty selection must report");
        assert!(
            !native_called.get(),
            "an SSH session must never invoke pbcopy, even on a macOS build (D2)"
        );
        assert_eq!(report.native_clipboard, NativeClipboardOutcome::NotAttempted);
    }

    // — Plan 08 Task 1: three-state OSC52 capability detection (gap-closure) ──

    #[test]
    fn detects_apple_terminal_as_unsupported() {
        let caps = detect_osc52_support(&|key| {
            if key == "TERM_PROGRAM" { Some("Apple_Terminal".to_string()) } else { None }
        });
        assert_eq!(caps.support, Osc52Support::Unsupported);
        assert_eq!(caps.display_name, "Terminal.app");
    }

    #[test]
    fn detects_iterm2_and_kitty_as_supported() {
        let iterm = detect_osc52_support(&|key| {
            if key == "TERM_PROGRAM" { Some("iTerm.app".to_string()) } else { None }
        });
        assert_eq!(iterm.support, Osc52Support::Supported);
        assert_eq!(iterm.display_name, "iTerm2");

        let kitty = detect_osc52_support(&|key| {
            if key == "KITTY_WINDOW_ID" { Some("1".to_string()) } else { None }
        });
        assert_eq!(kitty.support, Osc52Support::Supported);
        assert_eq!(kitty.display_name, "kitty");

        let kitty_term = detect_osc52_support(&|key| {
            if key == "TERM" { Some("xterm-kitty".to_string()) } else { None }
        });
        assert_eq!(kitty_term.support, Osc52Support::Supported);
        assert_eq!(kitty_term.display_name, "kitty");
    }

    #[test]
    fn tmux_is_unknown_not_supported() {
        let caps = detect_osc52_support(&|key| if key == "TMUX" { Some("1".to_string()) } else { None });
        assert_eq!(
            caps.support,
            Osc52Support::Unknown,
            "tmux forwards OSC52 only when `set -g set-clipboard on` is configured, which this \
             app cannot read — it must be Unknown, never Supported nor Unsupported"
        );
        assert_eq!(caps.display_name, "tmux");
    }

    #[test]
    fn unrecognised_terminal_is_unknown_not_unsupported() {
        let caps = detect_osc52_support(&|_| None);
        assert_eq!(
            caps.support,
            Osc52Support::Unknown,
            "an unrecognised terminal may well have accepted the write — Unknown must never \
             collapse into Unsupported"
        );
        assert_eq!(caps.display_name, "this terminal");
    }

    #[test]
    fn terminal_display_name_is_allowlisted_never_raw_env() {
        // A TERM_PROGRAM value carrying an escape byte and a control
        // sequence — the ANSI/OSC status-line spoofing primitive
        // (T-36.6.4-G08-02) this type must close.
        let hostile = "\x1b]0;pwned\x07 evil";
        let caps = detect_osc52_support(&|key| {
            if key == "TERM_PROGRAM" { Some(hostile.to_string()) } else { None }
        });
        const ALLOWLIST: &[&str] = &[
            "Terminal.app",
            "iTerm2",
            "kitty",
            "WezTerm",
            "Ghostty",
            "Hyper",
            "Windows Terminal",
            "tmux",
            "this terminal",
        ];
        assert!(
            ALLOWLIST.contains(&caps.display_name),
            "display_name must be one of the compiled-in allowlisted literals: {:?}",
            caps.display_name
        );
        assert!(
            !caps.display_name.chars().any(|c| (c as u32) < 0x20),
            "display_name must contain no byte below 0x20: {:?}",
            caps.display_name
        );
    }

    // — Plan 08 Task 2: copy_toast is a pure fn of the observed evidence ──

    fn supported_caps() -> TerminalClipboardCaps {
        TerminalClipboardCaps { support: Osc52Support::Supported, display_name: "iTerm2" }
    }

    fn unsupported_caps() -> TerminalClipboardCaps {
        TerminalClipboardCaps { support: Osc52Support::Unsupported, display_name: "Terminal.app" }
    }

    fn unknown_caps() -> TerminalClipboardCaps {
        TerminalClipboardCaps { support: Osc52Support::Unknown, display_name: "tmux" }
    }

    fn report(copied: usize, total: usize, native: NativeClipboardOutcome) -> CopyReport {
        CopyReport { copied, total, native_clipboard: native }
    }

    #[test]
    fn unsupported_terminal_toast_does_not_claim_a_copy() {
        // The non-macOS / SSH case this branch now governs (NotAttempted).
        let toast = copy_toast(report(5, 5, NativeClipboardOutcome::NotAttempted), unsupported_caps());
        assert!(
            !toast.starts_with("Copied"),
            "must not begin with the word the working case begins with: {toast:?}"
        );
        assert!(toast.contains("Terminal.app"), "must name the allowlisted terminal: {toast:?}");
        assert!(
            !toast.to_lowercase().contains("mouse") && !toast.to_lowercase().contains("capture"),
            "must never suggest a capture-toggle escape hatch (D-08): {toast:?}"
        );
    }

    #[test]
    fn confirmed_native_write_reports_a_copy_even_when_osc52_is_unsupported() {
        // The exact operator-facing case (Terminal.app): Unsupported OSC52
        // verdict, but pbcopy Confirmed the write. The observed outcome
        // must win.
        let toast = copy_toast(report(5, 5, NativeClipboardOutcome::Confirmed), unsupported_caps());
        assert!(toast.starts_with("Copied"), "must state the copy plainly: {toast:?}");
        assert!(
            !toast.to_lowercase().contains("doesn't support") && !toast.to_lowercase().contains("confirm"),
            "must not name an OSC52 limitation: {toast:?}"
        );
    }

    #[test]
    fn confirmed_native_write_reports_a_copy_when_osc52_is_unknown() {
        let toast = copy_toast(report(5, 5, NativeClipboardOutcome::Confirmed), unknown_caps());
        assert!(
            toast.starts_with("Copied"),
            "the observed outcome must win over an Unknown OSC52 verdict too: {toast:?}"
        );
    }

    #[test]
    fn failed_native_write_falls_through_to_the_osc52_wording() {
        let failed = copy_toast(report(5, 5, NativeClipboardOutcome::Failed), unsupported_caps());
        let not_attempted =
            copy_toast(report(5, 5, NativeClipboardOutcome::NotAttempted), unsupported_caps());
        assert_eq!(
            failed, not_attempted,
            "a failed native write must neither upgrade nor further degrade the message — \
             OSC52 was still attempted and may have landed"
        );
    }

    #[test]
    fn not_attempted_native_write_preserves_the_three_state_contract() {
        let supported = copy_toast(report(5, 5, NativeClipboardOutcome::NotAttempted), supported_caps());
        let unknown = copy_toast(report(5, 5, NativeClipboardOutcome::NotAttempted), unknown_caps());
        let unsupported =
            copy_toast(report(5, 5, NativeClipboardOutcome::NotAttempted), unsupported_caps());
        assert_eq!(supported, "Copied 5 chars");
        assert_ne!(unknown, supported);
        assert_ne!(unsupported, supported);
        assert_ne!(unknown, unsupported);
    }

    #[test]
    fn unknown_terminal_toast_asserts_neither_success_nor_failure() {
        let unknown = copy_toast(report(5, 5, NativeClipboardOutcome::NotAttempted), unknown_caps());
        let supported = copy_toast(report(5, 5, NativeClipboardOutcome::NotAttempted), supported_caps());
        let unsupported =
            copy_toast(report(5, 5, NativeClipboardOutcome::NotAttempted), unsupported_caps());
        assert_ne!(unknown, supported, "Unknown must not read as a success claim");
        assert_ne!(unknown, unsupported, "Unknown must not read as a failure claim");
    }

    #[test]
    fn supported_terminal_toast_is_unchanged_copied_n_chars() {
        let toast = copy_toast(report(5, 5, NativeClipboardOutcome::NotAttempted), supported_caps());
        assert_eq!(toast, "Copied 5 chars", "byte-equality with the UI-SPEC Copywriting Contract");
    }

    #[test]
    fn empty_selection_is_a_silent_no_op() {
        let outcome = yank_with(
            "",
            true,
            &|_| None,
            &|_text| Ok(()),
            &|_text| NativeClipboardOutcome::Confirmed,
        );
        assert!(outcome.expect("must not error").is_none(), "empty text must produce no report");
    }

    #[test]
    fn truncation_reports_char_counts_and_never_splits_a_char() {
        // Multi-byte characters (2-byte UTF-8 each) around the cap boundary.
        let long: String = "é".repeat(OSC52_PAYLOAD_CAP_CHARS + 500);
        let (payload, report) = prepare_payload(&long).expect("non-empty text must prepare");
        assert_eq!(payload.chars().count(), OSC52_PAYLOAD_CAP_CHARS, "truncation is char-counted, not byte-counted");
        assert_eq!(report.copied, OSC52_PAYLOAD_CAP_CHARS);
        assert_eq!(report.total, long.chars().count());
        assert!(std::str::from_utf8(payload.as_bytes()).is_ok(), "truncated payload must remain valid UTF-8");
        // Grapheme clusters may be split at the cap — an accepted, documented
        // outcome (this plan does not introduce a grapheme-segmentation dep).
    }
}
