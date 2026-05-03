//! Keyword-routed shell-output Block factory + STATUS_TEXT const.
//!
//! `fake_shell_out(text, time)` matches the prototype's `fakeShellOut` in
//! `warp2ironhermes/project/app/app.jsx` lines 309-337 — starts_with
//! routing on `git status` / `cargo` / `ls`, fallback to a
//! `(simulated) ran: <text>` Ok block. STATUS_TEXT is the verbatim
//! port of app.jsx lines 25-36 used by the `/status` palette pick (D-28).
//!
//! Author strings ("git" / "cargo" / "ls" / "sh") MUST match the
//! prototype byte-for-byte for visual fidelity — they appear in the
//! `wh-block-head .wh-author` span and the design references them in
//! pixel-snapshot comparisons.
//!
//! Module-level `#![allow(dead_code)]`: every symbol here is consumed by
//! Wave 4 (Plan 04-04a/04-04b WarpHermes rewire). Until then, these are
//! "ready but unwired" — clippy `-D warnings` would otherwise reject the
//! Wave 2 gate. Wave 4 will retain the allow only if some symbols stay
//! unused after wiring.
#![allow(dead_code)]

use crate::state::Block;

/// `/status` palette pick body per CONTEXT D-28; verbatim from
/// `warp2ironhermes/project/app/app.jsx` lines 25-36.
pub const STATUS_TEXT: &str = "IronHermes Status\n\
    ────────────────────────────────────────\n  \
    Home:     ~/.ironhermes/\n  \
    Model:    anthropic/claude-sonnet-4-20250514\n  \
    Provider: anthropic\n  \
    Terminal: bash\n  \
    Web:      firecrawl\n\n\
    API Keys\n  \
    OpenRouter:  configured\n  \
    Anthropic:   configured\n  \
    OpenAI:      not set";

/// Verbatim `git status` body from app.jsx 312-318.
const GIT_STATUS_TEXT: &str = "On branch main\n\
    Your branch is up to date with 'origin/main'.\n\n\
    Changes not staged for commit:\n  \
    modified:   crates/ironhermes-cli/src/tui/render.rs\n  \
    modified:   crates/ironhermes-agent/src/personality.rs\n\n\
    no changes added to commit (use \"git add\" and/or \"git commit -a\")";

/// Verbatim `cargo` build body from app.jsx 322-326.
const CARGO_BUILD_TEXT: &str = "   Compiling ironhermes-cli v0.4.1\n   \
    Compiling ironhermes-agent v0.4.1\n    \
    Finished `dev` profile [unoptimized + debuginfo] in 4.82s";

/// Verbatim `ls` body from app.jsx 330-332.
const LS_OUTPUT: &str = "Cargo.toml   README.md   crates/   target/   .ironhermes/";

/// Keyword-routed shell-output factory per CONTEXT D-12 step 5 + MOCK-02.
///
/// Routes on the leading whitespace-trimmed prefix of `text`:
///   - `git status` → `Block::Ok` author "git" with verbatim git-status output.
///   - `cargo`      → `Block::Ok` author "cargo" with verbatim cargo build output.
///   - `ls`         → `Block::Out` author "ls" with directory listing.
///   - else         → `Block::Ok` author "sh" with `(simulated) ran: <text>`.
///
/// `text` may include leading whitespace from a textarea submit; we
/// `trim_start()` before matching (per PATTERNS.md risk note).
pub fn fake_shell_out(text: &str, time: &str) -> Block {
    let trimmed = text.trim_start();
    if trimmed.starts_with("git status") {
        Block::Ok {
            author: Some("git".into()),
            time: Some(time.into()),
            message: GIT_STATUS_TEXT.into(),
        }
    } else if trimmed.starts_with("cargo") {
        Block::Ok {
            author: Some("cargo".into()),
            time: Some(time.into()),
            message: CARGO_BUILD_TEXT.into(),
        }
    } else if trimmed.starts_with("ls") {
        Block::Out {
            author: Some("ls".into()),
            time: Some(time.into()),
            text: LS_OUTPUT.into(),
        }
    } else {
        Block::Ok {
            author: Some("sh".into()),
            time: Some(time.into()),
            message: format!("(simulated) ran: {text}"),
        }
    }
}
