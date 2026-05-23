//! Mock data layer entry points + submodule re-exports.
//!
//! Per CONTEXT D-10:
//!   - `personalities` — REPLIES const + pick_reply.
//!   - `shell_outputs` — fake_shell_out + STATUS_TEXT.
//!   - `agent_steps`   — run_agent_steps async chain.
//!
//! This file additionally exposes `run_shell` — the runShell async entry
//! point per CONTEXT D-12. Function signatures match v2 server-fn shapes
//! (D-36) so the v2 swap is impl-only.
//!
//! Borrow-then-await discipline (CONTEXT D-06): every `.write()` drops at
//! the semicolon before any `.await`. See RESEARCH.md Pattern 2.
//!
//! Module-level `#![allow(dead_code, unused_imports)]`: every symbol is
//! consumed by Wave 4 (Plan 04-04a/04-04b WarpHermes rewire). Until then,
//! these are "ready but unwired" — clippy `-D warnings` would otherwise
//! reject the Wave 3 gate.
#![allow(dead_code, unused_imports)]

pub mod agent_steps;
pub mod personalities;
pub mod shell_outputs;
pub mod stub_data;

pub use agent_steps::run_agent_steps;

use crate::platform::timer::sleep;
use crate::state::{now_time, Block, BlockEntry, CommandLine, Token};
use dioxus::prelude::*;

/// Tokenize a shell input line per CONTEXT D-12 step 1.
///
/// Splits on whitespace; first token is `Token::Bin`; tokens starting
/// with `-` (any prefix length) are `Token::Flag`; else `Token::Arg`.
/// `Token::Str` (quoted-string args) is intentionally unhandled in
/// Phase 4 (Deferred Ideas — v2 real-runShell needs it).
fn tokenize(text: &str) -> Vec<Token> {
    let mut iter = text.split_whitespace();
    let mut out = Vec::new();
    if let Some(first) = iter.next() {
        out.push(Token::Bin(first.into()));
    }
    for tok in iter {
        if tok.starts_with('-') {
            out.push(Token::Flag(tok.into()));
        } else {
            out.push(Token::Arg(tok.into()));
        }
    }
    out
}

/// runShell mock per CONTEXT D-12 + MOCK-02.
///
/// Steps:
///   1. Tokenize `text` and append a `Block::Cmd` BlockEntry to `blocks`.
///      Each new entry gets a fresh id from `next_id`.
///   2. sleep(600).await — prototype timing (app.jsx line 169).
///   3. Append a `Block::*` output BlockEntry via `fake_shell_out`.
///
/// Note on D-12 step 3 (pulse_scanner): `pulse_scanner(2000)` is called
/// from `submit()` in `WarpHermes` (Wave 4) BEFORE this fn is awaited,
/// not inside it. The `scanner_active` Signal is included in this
/// signature for v2-swap shape compatibility per D-36 even though Phase 4
/// does not write to it from here.
///
/// Borrow-then-await discipline: `next_id()` (call-as-fn) clones the
/// Copy u64 value; `next_id.set(...)` is a method that takes a brief
/// internal lock and drops it. `blocks.write().push(...);` drops the
/// WriteLock at the `;`. No live signal borrow crosses any `.await`.
pub async fn run_shell(
    text: String,
    mut blocks: Signal<Vec<BlockEntry>>,
    mut next_id: Signal<u64>,
    _scanner_active: Signal<bool>,
) {
    // ── Stage 1: append Cmd block. ──
    let id1 = {
        let id = next_id();
        next_id.set(id + 1);
        id
    };
    let tokens = tokenize(&text);
    blocks.write().push(BlockEntry {
        id: id1,
        block: Block::Cmd {
            command: CommandLine {
                tokens,
                time: Some("…".into()),
                cwd: None,
                glyph: Some("❯".into()),
            },
        },
    });

    // No live borrows → safe to await.
    sleep(600).await;

    // ── Stage 2: append output block via keyword-routed factory. ──
    let id2 = {
        let id = next_id();
        next_id.set(id + 1);
        id
    };
    let time = now_time();
    let out_block = shell_outputs::fake_shell_out(&text, &time);
    blocks.write().push(BlockEntry {
        id: id2,
        block: out_block,
    });
}
