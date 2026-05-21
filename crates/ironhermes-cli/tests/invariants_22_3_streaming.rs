//! Phase 22.3 streaming-discipline static-grep regression gates.
//!
//! Sibling to `invariants_22_3.rs` (which covers INV-22.3-01..06 from the
//! original phase scope). This file covers the streaming-discipline fix
//! from Plan 22.3-11 (GAP-22.3-01 closure):
//!
//!   - INV-22.3-07: `tui::render::write_into_scroll_region` exists, is
//!     re-exported from tui/mod.rs, and is imported by main.rs.
//!   - INV-22.3-08: run_agent_turn's streaming callback in run_chat uses
//!     the helper — the legacy `print!("{}", delta)` form is gone from
//!     inside that function (run_single's callback is intentionally
//!     untouched per CONTEXT D-15).
//!   - INV-22.3-09: DECSTBM (`\x1b[1;`), DECSC (`\x1b7`), and DECRC
//!     (`\x1b8`) byte sequences do NOT appear inline in main.rs — the
//!     encapsulation invariant that keeps raw terminal escape codes
//!     contained in tui/render.rs.
//!
//! Follows the established INV-21.7-* / INV-22.1-04 / INV-22.3-01..06
//! `include_str!` + per-test `#[test]` pattern. Static-grep only per
//! CONTEXT D-03 + Phase 21 D-18 — no terminal-emulator harness, no
//! pseudo-terminal spawner, no scripted-REPL crate.

const MAIN_RS: &str = include_str!("../src/main.rs");
const RENDER_RS: &str = include_str!("../src/tui/render.rs");
const MOD_RS: &str = include_str!("../src/tui/mod.rs");

#[test]
fn invariant_22_3_07_write_into_scroll_region_helper_exists() {
    // (a) The helper is `pub fn` in render.rs with the expected signature.
    assert!(
        RENDER_RS.contains("pub fn write_into_scroll_region(bytes: &[u8], reserved: u16)"),
        "INV-22.3-07: tui/render.rs must define `pub fn write_into_scroll_region(bytes: &[u8], reserved: u16)`. \
         If you renamed or removed this helper, the streaming-discipline closure of GAP-22.3-01 is broken."
    );

    // (b) The helper is re-exported through tui/mod.rs so main.rs can
    //     import it as `crate::tui::write_into_scroll_region`.
    assert!(
        MOD_RS.contains("write_into_scroll_region"),
        "INV-22.3-07: tui/mod.rs must re-export write_into_scroll_region in the `pub use render::{{...}}` line. \
         Without this re-export, main.rs cannot import it."
    );

    // (c) main.rs imports the helper via the brace-group import added by Plan 22.3-11.
    //     We assert the brace-group form (NOT the bare `use crate::tui::write_into_scroll_region;`)
    //     to lock in the post-22.3-11 import shape and catch accidental rewrites.
    assert!(
        MAIN_RS.contains("use crate::tui::{reset_terminal_visual, write_into_scroll_region};"),
        "INV-22.3-07: main.rs must import `use crate::tui::{{reset_terminal_visual, write_into_scroll_region}};` \
         (the brace-group form added by Plan 22.3-11). Splitting the import or removing write_into_scroll_region \
         from it would orphan the streaming-discipline call sites."
    );
}

#[test]
fn invariant_22_3_08_run_agent_turn_streaming_uses_helper() {
    // Scope-isolate the run_agent_turn function body. We split on the
    // function header and take everything after, then bound it loosely
    // by the next top-level `async fn` or `fn` declaration in main.rs.
    // For the purposes of this grep, taking everything after the function
    // header is sufficient because the streaming callback is the FIRST
    // place inside run_agent_turn that mentions delta and write_into_scroll_region.
    let after_header = MAIN_RS
        .split("async fn run_agent_turn")
        .nth(1)
        .expect("INV-22.3-08: `async fn run_agent_turn` must exist in main.rs");

    // (a) The streaming callback in run_agent_turn routes tokens through the
    //     scroll-region helper. Phase 34a (MEM-READ-05) inserted the streaming
    //     scrubber, so the callback now writes the SCRUBBED `visible` bytes
    //     rather than the raw `delta`; the streaming-discipline contract
    //     (write_into_scroll_region, never raw print!) is unchanged.
    assert!(
        after_header.contains("write_into_scroll_region(visible.as_bytes()"),
        "INV-22.3-08: run_agent_turn's `with_streaming` callback must call \
         `write_into_scroll_region(visible.as_bytes(), ...)` (scrubbed deltas, Phase 34a) — \
         the streaming-discipline fix from Plan 22.3-11. Reverting to a raw `print!` \
         re-opens GAP-22.3-01 (streaming clobbers prompt)."
    );

    // (b) No raw-print form survives in run_agent_turn's body — neither the
    //     pre-34a `delta` nor the post-34a scrubbed `visible` may be printed
    //     directly here (run_agent_turn must use write_into_scroll_region).
    assert!(
        !after_header.contains("print!(\"{}\", delta)")
            && !after_header.contains("print!(\"{}\", visible)"),
        "INV-22.3-08: run_agent_turn must NOT raw-print streamed tokens — Plan 22.3-11 \
         replaced this with `write_into_scroll_region(...)`. Use `tracing::trace!` to debug."
    );

    // (c) Sanity check: run_single (the one-shot path, CONTEXT D-15 scope) IS
    //     still allowed to print streamed tokens directly. Phase 34a routes them
    //     through the scrubber first, so the literal is now `print!("{}", visible)`.
    //     Asserting it remains present proves this invariant is scoped to
    //     run_agent_turn only, not to all streaming sites.
    assert!(
        MAIN_RS.contains("print!(\"{}\", visible)"),
        "INV-22.3-08 (scope sanity): main.rs MUST still contain `print!(\"{{}}\", visible)` — \
         in `run_single`'s one-shot streaming callback. If this fails, either run_single's callback \
         was accidentally rewritten too (out of scope per CONTEXT D-15) OR run_single was deleted. \
         Investigate before adjusting this invariant."
    );
}

#[test]
fn invariant_22_3_09_decstbm_decsc_decrc_bytes_not_inline_in_main() {
    // IMPORTANT: `include_str!` loads source files as TEXT, so Rust
    // escape sequences like `"\x1b[1;{}r"` appear verbatim in the
    // loaded string as the 7-character ASCII sequence `\`, `x`, `1`,
    // `b`, `[`, `1`, `;`. We therefore search for the SOURCE-FORM of
    // these escape literals (raw strings), not for the actual ESC byte
    // (0x1b). This is the same convention used by a developer eyeballing
    // the source file with `grep -F '\x1b[1;' main.rs`.

    // (a) DECSTBM (\x1b[1;) — the scroll-region setup byte sequence.
    //     This sequence has been in tui/render.rs since Plan 21-02 and
    //     must STAY in render.rs only. Inlining it in main.rs would
    //     scatter terminal-mode management across files.
    assert!(
        !MAIN_RS.contains(r"\x1b[1;"),
        "INV-22.3-09: main.rs must NOT contain the DECSTBM byte sequence `\\x1b[1;` \
         (in Rust escape-literal source form). All DECSTBM scroll-region escapes belong \
         inside tui/render.rs (currently lines 193, 428, 600, 627)."
    );

    // (b) DECSC (\x1b7) — cursor-save. Added by Plan 22.3-11 inside
    //     write_into_scroll_region. Must stay inside render.rs only.
    assert!(
        !MAIN_RS.contains(r"\x1b7"),
        "INV-22.3-09: main.rs must NOT contain the DECSC byte sequence `\\x1b7` \
         (in Rust escape-literal source form). Cursor-save belongs inside \
         tui/render.rs::write_into_scroll_region. If you need cursor save/restore at a new \
         call site, add it through the helper, not inline."
    );

    // (c) DECRC (\x1b8) — cursor-restore. Added by Plan 22.3-11 inside
    //     write_into_scroll_region. Must stay inside render.rs only.
    assert!(
        !MAIN_RS.contains(r"\x1b8"),
        "INV-22.3-09: main.rs must NOT contain the DECRC byte sequence `\\x1b8` \
         (in Rust escape-literal source form). Cursor-restore belongs inside \
         tui/render.rs::write_into_scroll_region."
    );

    // (d) Positive control: the DECSTBM bytes ARE present in render.rs
    //     (as the source-form escape `\x1b[1;`). This catches the
    //     accidental case where someone deletes ALL DECSTBM machinery
    //     from render.rs and the inline check above trivially passes
    //     because the bytes are nowhere to be found.
    assert!(
        RENDER_RS.contains(r"\x1b[1;"),
        "INV-22.3-09 (positive control): tui/render.rs MUST contain DECSTBM `\\x1b[1;` — \
         the scroll-region machinery from Plan 21-02 / 22.3-05. Its absence indicates the entire \
         scroll-region establishment was removed, which would also break GAP-22.3-01 closure."
    );

    // (e) Positive control: DECSC/DECRC ARE present in render.rs (added by Plan 22.3-11).
    assert!(
        RENDER_RS.contains(r"\x1b7"),
        "INV-22.3-09 (positive control): tui/render.rs MUST contain DECSC `\\x1b7` — \
         added by Plan 22.3-11 inside write_into_scroll_region for the GAP-22.3-01 fix."
    );
    assert!(
        RENDER_RS.contains(r"\x1b8"),
        "INV-22.3-09 (positive control): tui/render.rs MUST contain DECRC `\\x1b8` — \
         added by Plan 22.3-11 inside write_into_scroll_region for the GAP-22.3-01 fix."
    );
}
