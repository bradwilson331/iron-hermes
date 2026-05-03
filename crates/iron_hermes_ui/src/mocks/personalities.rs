//! Personality preset reply table per CONTEXT D-21 + MOCK-01.
//!
//! Six personalities × one canned reply each, prompt-agnostic. Verbatim
//! port of `warp2ironhermes/project/app/app.jsx` lines 339-349
//! (`fakeAgentReply`). Phase 4 ships ONE reply per variant — D-21 explicitly
//! rejects round-robin or keyword-matching variation; visual fidelity to
//! the prototype outranks demo richness. Deferred to v2 when real Hermes
//! returns and mocks become irrelevant.
//!
//! Module-level `#![allow(dead_code)]`: every symbol here is consumed by
//! Wave 4 (Plan 04-04a/04-04b WarpHermes rewire). Until then, these are
//! "ready but unwired" — clippy `-D warnings` would otherwise reject the
//! Wave 3 gate.
#![allow(dead_code)]

use crate::state::Personality;

/// Six personality → canned-reply tuples. Order mirrors `Personality::ALL`.
/// Strings are byte-for-byte from `app.jsx` 339-349.
pub const REPLIES: [(Personality, &str); 6] = [
    (Personality::Concise,   "Will do. Reading now."),
    (Personality::Technical, "Acknowledged. Inspecting `crates/ironhermes-cli/src/tui/render.rs` and adjacent modules; diff incoming."),
    (Personality::Noir,      "Another case. The file's hiding something — they always are. I'll crack it open."),
    (Personality::Hype,      "OH HECK YES, ON IT! READING THE CODE NOW! THIS IS GOING TO BE INCREDIBLE! ⚡"),
    (Personality::Catgirl,   "nya~ ok! reading the file rn (=^.^=) gimme a sec~"),
    (Personality::Default,   "On it. I'll inspect the relevant files and propose a patch — give me a moment to read through what you have."),
];

/// Look up the canned reply for a personality. Falls back to the empty-
/// ish "…" if the table is somehow incomplete (defensive — never expected
/// to fire because REPLIES covers all six variants).
pub fn pick_reply(p: Personality) -> &'static str {
    REPLIES
        .iter()
        .find(|(k, _)| *k == p)
        .map(|(_, s)| *s)
        .unwrap_or("…")
}
