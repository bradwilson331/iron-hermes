# Phase 4: Data Layer & Interactions - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-05-03
**Phase:** 04-data-layer-interactions
**Areas discussed:** State ownership topology, Async timer primitive + deps, Markdown rendering for is-ai, Personality switching UX

---

## Gray-Area Selection

User selected ALL FOUR offered gray areas (multi-select) and added freeform context: **"This will be integrated into this project: /Users/twilson/code/ironhermes/"**.

| Offered area | Selected |
|--------------|----------|
| State ownership topology | ✓ |
| Async timer primitive + deps | ✓ |
| Markdown rendering for is-ai | ✓ |
| Personality switching UX | ✓ |

**Freeform context captured:** integration target `/Users/twilson/code/ironhermes/` exists as a Cargo workspace (Tokio + reqwest + rusqlite, 11 crates, edition 2024). Acknowledged and added to canonical refs as a v2 forward reference; Phase 4 stays mocks-only per PROJECT.md but mock signatures shaped to mirror v2 server-fn returns. CONTEXT D-36/D-37 capture the integration boundary.

---

## State Ownership Topology

### Question 1: How should Phase 4's reactive shell state be owned?

| Option | Description | Selected |
|--------|-------------|----------|
| Hybrid: local + context (Recommended) | Local Signal<T> in WarpHermes(); use_context_provider for ShellSettings { personality } now, Phase 5 extends with theme/density/block/agent. AgentPanel + StatusBar use_context for the cross-cutting bits. | ✓ |
| All local in WarpHermes (prototype-direct) | Mirror app.jsx 1:1 — every signal local, no context. Heavier prop-list churn across phases when Phase 5 adds TweaksPanel. | |
| All-context: single ShellState bundle | One ShellState struct with every signal provided at root. Zero prop drilling but obscures component data dependencies. | |
| Trait-abstracted data layer (anticipates v2) | Define ShellRunner/AgentRunner traits with mock impls; ships strictly more code; v2 swap-cost benefit is unproven. | |

**User's choice:** Hybrid: local + context (Recommended)
**Notes:** Captured as CONTEXT D-01. Trait abstraction (D-37) deliberately rejected — v2 server-function injection happens at function boundaries, not trait dispatch.

### Question 2: What lives in the use_context_provider'd ShellSettings bundle in Phase 4?

| Option | Description | Selected |
|--------|-------------|----------|
| Just `personality` (Recommended) | ShellSettings { personality: Signal<Personality> } only. Phase 5 extends. Most principled — context only for cross-cutting state with no single owner. | ✓ |
| Personality + mode | Add mode to bundle. Three consumers but Mode has a clear owner in WarpHermes (it routes submissions). | |
| Everything StatusBar reads | personality + mode + tokens + scanner_active. Maximum DRY but slippery slope toward all-context. | |

**User's choice:** Just `personality` (Recommended)
**Notes:** Captured as CONTEXT D-02. ShellSettings struct shape forward-compatible — Phase 5 adds theme/density/block/agent fields without refactoring.

---

## Async Timer Primitive + Deps

### Question 1: Which timer API drives the 600/1400/2000ms mock delays?

| Option | Description | Selected |
|--------|-------------|----------|
| gloo-timers (Recommended) | gloo-timers = "0.3" with futures feature. Idiomatic .await-friendly. WASM-only. | ✓ |
| Direct web_sys::setTimeout | No new crate but verbose, callback-based, doesn't compose with async/await without manual JsFuture wrapping. | |
| wasm-bindgen-futures + js-sys setTimeout (manual JsFuture) | Roll a small ~10-LOC sleep helper. No new dep. Slightly less obvious than gloo-timers. | |

**User's choice:** gloo-timers (Recommended)
**Notes:** Triggered the platform-parity follow-up (Q2) since gloo-timers is wasm-only.

### Question 2: How should the cross-platform timer story work?

| Option | Description | Selected |
|--------|-------------|----------|
| cfg-gated dual timer (Recommended) | src/platform/timer.rs: gloo-timers under cfg(target_arch="wasm32"), tokio::time::sleep elsewhere. tokio with `time` feature only. Three-platform compile gate stays green. | ✓ |
| futures-timer (cross-platform crate) | One crate, zero cfg gates. Less popular ecosystem. | |
| Web-only Phase 4, desktop/mobile non-functional | gloo-timers behind web feature; submit() no-op on desktop/mobile. Breaks runtime parity. | |

**User's choice:** cfg-gated dual timer (Recommended)
**Notes:** Captured as CONTEXT D-04 and D-05. tokio added with `time` feature only (NOT `full`) to keep native binary size minimal. New file src/platform/timer.rs naturally fills the empty src/platform/ directory created in Phase 1 D-04.

---

## Markdown Rendering for is-ai

Pre-question grep evidence: prototype's seed b5 uses inline `<code>concise</code>` and `<code>personality.rs</code>`; fakeAgentReply's technical personality returns ``` `crates/ironhermes-cli/src/tui/render.rs` ```. Otherwise plain strings. Narrow gap → narrow solution.

### Question 1: How should is-ai blocks render their content in Phase 4?

| Option | Description | Selected |
|--------|-------------|----------|
| Inline-code spans only — no crate (Recommended) | ~25 LOC renderer in src/components/shell/markdown.rs. Splits on backtick pairs, alternates plain/code spans, preserves newlines. Matches prototype seed b5. v2 swap-in is one-file. | ✓ |
| Add pulldown-cmark now | Full CommonMark parser. ~280KB WASM bundle delta. Future-proofs for v2 ironhermes-agent's markdown output. | |
| Plaintext + <pre> only | Lowest effort, highest visual drift. PROJECT.md says visual drift is the primary failure mode. | |

**User's choice:** Inline-code spans only — no crate (Recommended)
**Notes:** Captured as CONTEXT D-15 and D-16. v2 swap path: replace markdown.rs with a real parser when ironhermes-agent's response stream lands.

---

## Personality Switching UX

### Question 1: How does the user switch personality preset in Phase 4?

| Option | Description | Selected |
|--------|-------------|----------|
| /personality opens secondary palette (Recommended) | Selecting /personality swaps palette items to 6 personality choices; palette stays open. Slight state machine: pal_state Browse | PersonalityPick. Mirrors prototype intent. Zero new components. | ✓ |
| Status-bar pill click cycles | One-click discovery but adds a clickable affordance the prototype doesn't show. Drifts visually. | |
| ⌥+P hotkey cycles | Pure keyboard, fast. Worst discoverability. KBD-XX requirements don't include ⌥+P → flirts with scope creep. | |
| Both /personality palette AND status-bar visual indicator (no click) | Sub-palette plus non-interactive status-bar pill for current preset. | |

**User's choice:** /personality opens secondary palette (Recommended)
**Notes:** Captured as CONTEXT D-20 and D-22. Status-bar gets a NEW read-only personality pill (visible feedback after switching) but click-to-cycle is deferred to Phase 5's TweaksPanel.

### Question 2: How rich is each personality's mock-reply table in Phase 4?

| Option | Description | Selected |
|--------|-------------|----------|
| 1:1 with prototype — one fixed reply per preset (Recommended for fidelity) | Match fakeAgentReply exactly: one canned string per personality, prompt-agnostic. 6 strings total. Most prototype-faithful. | ✓ |
| Round-robin 3-5 replies per preset | Vec<String> per personality, ~24 strings total. Adds demo variety. Content drift, not visual drift. | |
| Keyword-matched per preset | Per-personality lookup, ~24 strings + 6 fallbacks. Most "alive" feel. | |

**User's choice:** 1:1 with prototype — one fixed reply per preset (Recommended for fidelity)
**Notes:** Captured as CONTEXT D-21. PROJECT.md core value (visually indistinguishable from prototype side-by-side) outranks demo richness. Demo variety becomes irrelevant when v2 wires real Hermes.

---

## Final-Check Question

### Question: Anything left to discuss before I write CONTEXT.md?

| Option | Description | Selected |
|--------|-------------|----------|
| Ready for context | Write CONTEXT.md and DISCUSSION-LOG.md with the decisions above and Claude's discretion items as locked. | ✓ |
| Object to a discretion item | Override one of the 10 numbered Claude's-discretion items. | |
| Explore more gray areas | Additional areas before context written. | |

**User's choice:** Ready for context
**Notes:** All 10 Claude's-discretion items presented inline in chat (block IDs, mock-content placement, hover handlers, token counter, palette pick handlers, keyboard handler placement, submit tokenization, auto-scroll, three-platform compile gate, ironhermes integration boundary) accepted without objection. Each is captured as numbered decisions in CONTEXT.md.

---

## Claude's Discretion

User accepted all 10 Claude's-discretion items presented inline at the summary stage. Each is now a CONTEXT decision:

| # | Discretion area | CONTEXT D-# |
|---|-----------------|-------------|
| 1 | Block IDs (BlockEntry wrapper, next_id Signal) | D-07, D-08, D-09 |
| 2 | Mock content placement (src/mocks/ module split) | D-10, D-11 |
| 3 | Hover handlers (copy=clipboard, rerun=Cmd-only, share=unwired) | D-23, D-24, D-25 |
| 4 | Token counter (+120 saturating at 128_000) | D-26 |
| 5 | Palette pick() handlers matching prototype exactly | D-27..D-32 |
| 6 | Keyboard handlers (global keydown, ⌥M only when focused) | D-17, D-18, D-19 |
| 7 | Submit tokenization (naive whitespace split, Token::Str deferred) | D-12, D-13 |
| 8 | Auto-scroll on new blocks (use_effect + scroll_into_view) | D-33 |
| 9 | Three-platform compile gate stays canonical | D-35 |
| 10 | ironhermes integration boundary (mocks shaped to mirror v2 server-fn returns) | D-36, D-37 |

Plus four sub-discretion items in CONTEXT "Claude's Discretion" sub-block: personality enum traits, spawn-vs-use_future for pulse_scanner, substring filter case-folding, /help rendered output style.

---

## Deferred Ideas

(Captured to CONTEXT.md `<deferred>` section.)

- Round-robin/keyword-matched personality replies (v2; mocks irrelevant when real Hermes lands)
- Full markdown rendering for is-ai (v2; replace markdown.rs)
- Share button handler (v2 share-link backend)
- Click-to-cycle status-bar personality pill (Phase 5 TweaksPanel)
- Theme/density/block/agent runtime switching + TweaksPanel (Phase 5)
- Mobile shell variant (Phase 6)
- Trait-abstracted data layer (rejected; v2 may revisit if server-fn refactor reveals dispatch needs)
- Persistence (PERS-01/02 are v2)
- Tests (TEST-01/02/03 are v2)
- Real shell exec / real LLM API / authentication (REAL-01..04 + AUTH-01 are v2)
- Token::Str (quoted-string args) tokenization (v2; real runShell will need it)
- Live token estimation from prompt size (v2 from API response metadata)
- Fuzzy palette filtering (10-item palette doesn't warrant the dep)
