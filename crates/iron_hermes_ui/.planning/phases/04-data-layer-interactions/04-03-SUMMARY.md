---
phase: 04-data-layer-interactions
plan: 03
subsystem: mocks
tags: [dioxus, mocks, async, signals, clippy, await-discipline, borrow-then-await]

# Dependency graph
requires:
  - phase: 04-data-layer-interactions
    plan: 01
    provides: "crate::platform::timer::sleep cfg-gated primitive (consumed by run_shell sleep(600) and run_agent_steps sleep(400)/sleep(1000)); three-platform compile gate cadence"
  - phase: 04-data-layer-interactions
    plan: 02
    provides: "Personality enum (with Eq + Hash for pick_reply const-table .find lookup); BlockEntry { id, block } wrapper consumed by run_shell blocks Signal<Vec<BlockEntry>>; now_time() cfg-gated helper consumed by all mock outputs; ShellSettings type (forward-referenced for v2 swap shape D-36)"
provides:
  - "src/mocks/personalities.rs: REPLIES const ([(Personality, &str); 6] verbatim from app.jsx 339-349) + pick_reply(p: Personality) -> &'static str helper (MOCK-01)"
  - "src/mocks/shell_outputs.rs: fake_shell_out(text, time) -> Block keyword-routed factory (git status / cargo / ls / fallback) + STATUS_TEXT const (verbatim from app.jsx 25-36) (MOCK-02 supporting + /status palette pick supporting)"
  - "src/mocks/agent_steps.rs: pub async fn run_agent_steps(prompt, personality, messages: Signal<Vec<Message>>) — 3-stage chain user→sleep(400)→tool-call→sleep(1000)→reply (MOCK-03)"
  - "src/mocks/mod.rs: tokenize(text) -> Vec<Token> + pub async fn run_shell(text, blocks: Signal<Vec<BlockEntry>>, next_id: Signal<u64>, _scanner_active: Signal<bool>) — 2-stage Cmd→sleep(600)→Out chain; signature matches D-36 v2-swap shape (MOCK-02)"
  - "src/main.rs: mod mocks; declaration registering the new module tree alongside state/components/platform"
  - "src/components/warp_hermes.rs TEMPORARY shim: imports demo_block_entries instead of the removed demo_blocks; collects Vec<BlockEntry> into Vec<Block> so Phase 3 BlockStream prop still type-checks. Wave 4 (Plan 04-04a/04-04b) replaces this entirely with the prop-shape refactor"
  - "borrow-then-await discipline (D-06) verified at compile time by cargo clippy --features web -- -D warnings — every Signal<T> .write() in src/mocks/ drops at the semicolon BEFORE any .await; this is the FIRST wave with .await and clippy await-holding-invalid-types first becomes binding here"
affects: [04-04a, 04-04b, 04-05]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Borrow-then-await (D-06 + clippy.toml + RESEARCH Pattern 2): Signal<T>::write() borrows are inline-and-dropped at the statement semicolon before any .await; reads are cloned into owned locals; next_id() (call-as-fn) clones the Copy u64 — no live signal borrow ever crosses an await point"
    - "v2 server-fn shape parity (D-36): mock async fn signatures (run_shell, run_agent_steps) mirror the future dioxus_fullstack server-fn signatures byte-for-byte so the v2 swap is impl-only — replace function bodies, not call sites"
    - "Module-level dead-code suppression for ready-but-unwired surfaces: each mocks/ file uses #![allow(dead_code)] (mod.rs additionally allows unused_imports for the run_agent_steps re-export); same approach applied to Wave-1 PaletteState / ShellSettings / Personality::label / Personality::ALL via scoped #[allow(dead_code)] until Wave 4 wires them"
    - "Verbatim-port pattern from prototype JS: REPLIES strings + STATUS_TEXT + git-status / cargo / ls bodies are byte-for-byte from app.jsx 25-36 / 309-337 / 339-349 — design fidelity is the constraint per CLAUDE.md"
    - "Inline-id-block for next_id allocation: { let id = next_id(); next_id.set(id + 1); id } — isolates the read+write so the WriteLock from .set drops within the block scope before the surrounding statement; mirrors RESEARCH Pattern 2 GOOD example"

key-files:
  created:
    - "src/mocks/personalities.rs"
    - "src/mocks/shell_outputs.rs"
    - "src/mocks/agent_steps.rs"
    - "src/mocks/mod.rs"
    - ".planning/phases/04-data-layer-interactions/04-03-SUMMARY.md"
  modified:
    - "src/main.rs"
    - "src/components/warp_hermes.rs"
    - "src/state.rs"

key-decisions:
  - "Module-level #![allow(dead_code)] in each mocks/ file (rather than per-item attributes) — the entire module is unwired-until-Wave-4; one inner attribute per file is cleaner than scattered per-item attributes and the doc comment makes the intent explicit"
  - "Wave-1 carry-forward symbols (Personality::label, Personality::ALL, PaletteState, ShellSettings) get scoped #[allow(dead_code)] in src/state.rs — these were introduced in Plan 04-02 specifically for Wave 4 consumption; without these allows the Wave 3 clippy gate fails because -D warnings promotes Wave-1 dead-code to errors. The allow comments name the consuming wave (Plan 04-04a/04-04b)"
  - "warp_hermes.rs shim retained as planned (Task 5 plan rationale): import demo_block_entries instead of the removed demo_blocks; collect into Vec<Block> at the call site. Wave 4 (Plan 04-04a/04-04b) replaces this with the proper Signal<Vec<BlockEntry>> prop-shape refactor — the shim is the documented bridge that keeps Wave 2/3 builds green without forcing the Wave 4 work to land here"
  - "tokenize lives in src/mocks/mod.rs (not a separate file) — it's a 12-line helper with one caller (run_shell) and no other consumer. A separate src/mocks/tokenize.rs would be code-bloat; the helper is private to mod.rs (no pub) and the doc comment names D-12 step 1 as its specification"
  - "_scanner_active param in run_shell signature uses leading underscore to silence unused-arg lint without changing the v2-swap shape (D-36). Phase 4 Wave 5 (Plan 04-05) WarpHermes.submit() calls pulse_scanner BEFORE awaiting run_shell, not from inside it — the param is forward-compat ballast for the v2 server-fn that may need it"

patterns-established:
  - "Pattern: borrow-then-await with inline-id-block — { let id = sig(); sig.set(id+1); id } evaluates to the new id while keeping all signal locks scoped within the block; followed by .await safely. RESEARCH Pattern 2 GOOD example codified in production code"
  - "Pattern: keyword-routed Block factory — text.trim_start() → starts_with(<keyword>) → Block::Ok / Block::Out variant with verbatim prototype output bodies stored as module-private const &str. Future fake_xxx_out helpers in v2 (e.g., fake_grep_out, fake_test_out) follow the same shape"
  - "Pattern: dead-code suppression with named-consumer comment — // consumed by Wave N (Plan XX-YY) — symbol-name. Makes the temporary nature of the allow attribute legible and self-documenting; reviewer can grep // consumed by to find all such markers"

requirements-completed: [MOCK-01, MOCK-02, MOCK-03]

# Metrics
duration: ~6min
completed: 2026-05-03
---

# Phase 04 Plan 03: mocks/ Module Tree Summary

**Phase 4 Wave 3: created the `src/mocks/` module tree (4 files: personalities.rs, shell_outputs.rs, agent_steps.rs, mod.rs) implementing the project's first async code. `run_shell` and `run_agent_steps` mirror the prototype's `runShell` (app.jsx 163-174) and `runAgent` (app.jsx 176-185) byte-for-byte in their stage timing (600ms / 400ms+1000ms) and Block / Message shapes. Three-platform `cargo build` GREEN AND `cargo clippy --features web -- -D warnings` GREEN — the borrow-then-await discipline (D-06 + clippy.toml `await-holding-invalid-types`) is verified at compile time for every `.await` in the mocks/ tree. The `warp_hermes.rs` shim (Task 5) bridges the Wave-1 destructive-rename carry-forward; Wave 4 (Plan 04-04a/04-04b) replaces it.**

## Performance

- **Duration:** ~6 min (combined initial run + resumption)
- **Started:** initial-run timestamp captured by previous executor (4 task commits landed before interruption)
- **Resumed/Completed:** 2026-05-03
- **Tasks:** 5 (4 commit + 1 verify-with-shim)
- **Files created:** 4 source files (src/mocks/*.rs) + this SUMMARY.md
- **Files modified:** 3 (src/main.rs, src/components/warp_hermes.rs, src/state.rs)

## Accomplishments

- **`src/mocks/personalities.rs`** (commit `c9a9cba`): added `pub const REPLIES: [(Personality, &str); 6]` with byte-for-byte verbatim strings from app.jsx 339-349 (Concise / Technical / Noir / Hype / Catgirl / Default — including the literal `⚡` Unicode in the Hype reply and the `(=^.^=)` ASCII emoticon in Catgirl); plus `pub fn pick_reply(p: Personality) -> &'static str` using `.iter().find(|(k, _)| *k == p)` (relies on Personality's Eq derive from Wave 1) with `.unwrap_or("…")` defensive fallback. Module-level `#![allow(dead_code)]` because all symbols are consumed in Wave 4. (MOCK-01 ✓)

- **`src/mocks/shell_outputs.rs`** (commit `811e147`): added `pub const STATUS_TEXT: &str` verbatim from app.jsx 25-36 (IronHermes Status / paths / API Keys block); three module-private const &str bodies (`GIT_STATUS_TEXT`, `CARGO_BUILD_TEXT`, `LS_OUTPUT`) verbatim from app.jsx 312-318 / 322-326 / 330-332; and `pub fn fake_shell_out(text: &str, time: &str) -> Block` with the prototype's keyword routing (`text.trim_start().starts_with("git status")` → `Block::Ok` author "git"; `"cargo"` → `Block::Ok` author "cargo"; `"ls"` → `Block::Out` author "ls"; else → `Block::Ok` author "sh" with `format!("(simulated) ran: {text}")` using the un-trimmed text per prototype interpolation). Author strings byte-exact ("git" / "cargo" / "ls" / "sh"). Pure-fn factory — no async, no Signal usage; clippy passes trivially. (MOCK-02 ✓ supporting)

- **`src/mocks/agent_steps.rs`** (commit `ffaec5e`): added `pub async fn run_agent_steps(prompt: String, personality: Personality, mut messages: Signal<Vec<Message>>)` implementing the 3-stage chain — append user `Message` → `sleep(400).await` → append hermes tool-call `Message` (with `ToolCall { name: "search", args_summary: format!("{{\"q\":\"{summary}\"}}"), status: ToolStatus::Done }` where summary = `prompt.chars().take(40).collect()`) → `sleep(1000).await` → append hermes reply via `pick_reply(personality).to_string()`. Every `.write().push(...);` is inline-and-dropped at the semicolon (RESEARCH Pattern 2 GOOD); no `let binding = messages.write()` anywhere. The `summary` local is owned String, computed before the second `.write()` and `.await`. ToolStatus::Done (not Running) per RESEARCH Pattern 2 + the prototype's expectation that the visible tool-call has resolved by the time the reply arrives. (MOCK-03 ✓)

- **`src/mocks/mod.rs`** (commit `983e8db`): added the module entry with `pub mod {agent_steps, personalities, shell_outputs};` declarations + `pub use agent_steps::run_agent_steps;` re-export; module-private `fn tokenize(text: &str) -> Vec<Token>` (first whitespace token → `Token::Bin`, tokens starting with `-` → `Token::Flag`, else → `Token::Arg` per D-12 step 1; `Token::Str` deferred to v2); and `pub async fn run_shell(text: String, mut blocks: Signal<Vec<BlockEntry>>, mut next_id: Signal<u64>, _scanner_active: Signal<bool>)` implementing the 2-stage chain — allocate id1 via inline `{ let id = next_id(); next_id.set(id+1); id }` block → `blocks.write().push(BlockEntry { id: id1, block: Block::Cmd { command: CommandLine { tokens, time: Some("…".into()), cwd: None, glyph: Some("❯".into()) } } });` → `sleep(600).await` → allocate id2 the same way → `let time = now_time(); let out_block = shell_outputs::fake_shell_out(&text, &time);` → `blocks.write().push(BlockEntry { id: id2, block: out_block });`. The `_scanner_active` param keeps the v2-swap signature shape (D-36) without forcing pulse_scanner integration in this wave; it lives in WarpHermes::submit() per D-12 step 3 (Wave 5 / Plan 04-05). Module-level `#![allow(dead_code, unused_imports)]` for the unwired surfaces. (MOCK-02 ✓ + tokenizer per D-12)

- **`src/main.rs`** (commit `983e8db`): added `mod mocks;` line registering the module alongside `mod state; mod platform; mod components; mod fonts; mod app;`. (Module declaration ✓)

- **`src/components/warp_hermes.rs`** (commit `6ef651d` — Task 5 shim): changed `use crate::state::demo_blocks` to `use crate::state::demo_block_entries` and rewrote the `let blocks` line as `let blocks: Vec<crate::state::Block> = demo_block_entries().into_iter().map(|e| e.block).collect();` — collecting BlockEntry into Vec<Block> so the Phase 3 BlockStream prop (which still takes `Vec<Block>`, not `Signal<Vec<BlockEntry>>`) type-checks. **This is a TEMPORARY shim documented in the plan and replaced by Wave 4 (Plan 04-04a/04-04b prop-shape refactor).**

- **`src/state.rs`** (commit `6ef651d` — Wave-1 dead-code allows): added `#[allow(dead_code)]` with named-consumer comments on (a) the `impl Personality {}` block (label + ALL — consumed by Wave 4 palette substate + status-bar pill), (b) `pub enum PaletteState` (consumed by Wave 4 palette substate transitions), (c) `pub struct ShellSettings` (consumed by Wave 4 use_context_provider in WarpHermes). These were introduced in Plan 04-02 specifically for Wave 4 consumption; without these allows the Wave 3 clippy gate (-D warnings) fails because dead-code warnings get promoted to errors.

- **Three-platform compile gate GREEN:** `cargo build --no-default-features --features {web|desktop|mobile}` all exit 0 with zero warnings.

- **Clippy gate GREEN:** `cargo clippy --no-default-features --features web -- -D warnings` exits 0 with zero warnings/errors. This is the FIRST wave with `.await` and the first time the clippy `await-holding-invalid-types` rule (configured in `clippy.toml` for `dioxus_signals::WriteLock`, `generational_box::GenerationalRef`, `generational_box::GenerationalRefMut`) becomes binding. Borrow-then-await discipline (D-06) verified at compile time across `run_shell` and `run_agent_steps`.

## Task Commits

Each task was committed atomically:

1. **Task 1: Create src/mocks/personalities.rs (REPLIES + pick_reply)** — `c9a9cba` (feat)
2. **Task 2: Create src/mocks/shell_outputs.rs (fake_shell_out + STATUS_TEXT)** — `811e147` (feat)
3. **Task 3: Create src/mocks/agent_steps.rs (run_agent_steps 3-stage chain)** — `ffaec5e` (feat)
4. **Task 4: Create src/mocks/mod.rs + register mod mocks; in main.rs** — `983e8db` (feat)
5. **Task 5: warp_hermes.rs shim + Wave-1 dead-code allows for clippy gate** — `6ef651d` (feat)

**Plan metadata commit:** pending (final commit after STATE.md / ROADMAP.md updates).

## Files Created/Modified

- `src/mocks/personalities.rs` (NEW, 35 lines)
- `src/mocks/shell_outputs.rs` (NEW, ~70 lines)
- `src/mocks/agent_steps.rs` (NEW, ~55 lines)
- `src/mocks/mod.rs` (NEW, ~115 lines including tokenize + run_shell)
- `src/main.rs` (MODIFIED, +1 line — `mod mocks;` declaration)
- `src/components/warp_hermes.rs` (MODIFIED, +1 / -1 — temporary BlockEntry-to-Block-collect shim)
- `src/state.rs` (MODIFIED, +3 lines — three scoped `#[allow(dead_code)]` attributes on Wave-1-but-Wave-4-consumed symbols)
- `.planning/phases/04-data-layer-interactions/04-03-SUMMARY.md` (NEW) — this file.

## Decisions Made

- **Module-level dead-code suppression style** (per file in src/mocks/): used `#![allow(dead_code)]` as an inner attribute at the top of each mocks/ file rather than scattered per-item `#[allow(dead_code)]` attributes. The doc comment above each allow names the consuming wave (Wave 4 / Plan 04-04a/04-04b) so the intent is explicit. Wave 4 will retain or remove these allows based on whether all symbols end up wired (most will).
- **Wave-1 carry-forward dead-code: scoped #[allow] not module-level** (in src/state.rs): the four affected symbols (Personality impl block, PaletteState enum, ShellSettings struct) are scattered through state.rs alongside actively-used types (Block, BlockEntry, demo_*, etc.). A module-level `#![allow(dead_code)]` would be too broad — scoped per-item allows with named-consumer comments preserve dead-code warnings for genuinely-unused state.rs additions. Auto-fixed under deviation Rule 3 (blocking issue) — without these the Wave 3 clippy gate fails.
- **warp_hermes.rs shim collects rather than threads BlockEntry**: the alternative (refactoring BlockStream to take `Vec<BlockEntry>` here in Wave 3) would do Wave 4's job out of phase. Collect-into-Vec<Block> is the minimum-fidelity bridge — id is discarded, demo_block_entries() ids 1..=10 are not visible in the Phase 3 stream rendering, but BlockStream's Phase 3 `Vec<Block>` prop shape is preserved unchanged. Wave 4 (Plan 04-04a) replaces this entirely with the prop-shape refactor.
- **tokenize stays in src/mocks/mod.rs** (not a separate src/mocks/tokenize.rs file): it's 12 lines, one caller, no public API. A separate file would be code-bloat.
- **_scanner_active in run_shell signature**: kept as `Signal<bool>` (not removed, not `&mut`) per D-36 v2-swap shape compat. Underscore prefix silences unused-arg lint without bending the signature.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Wave-1 dead-code symbols block clippy gate**

- **Found during:** Task 5 (clippy gate run)
- **Issue:** `cargo clippy --no-default-features --features web -- -D warnings` failed with three errors and one warning-promoted-to-error covering `Personality::label`, `Personality::ALL`, `PaletteState`, and `ShellSettings`. These symbols were introduced in Wave 1 (Plan 04-02) specifically for Wave 4 consumption; the plain `cargo build` shows them as warnings, but `clippy -D warnings` promotes the four dead-code warnings to errors.
- **Why blocking:** the plan's Task 5 acceptance criterion is `cargo clippy --features web -- -D warnings` exits 0. Without silencing these Wave-1-introduced symbols the gate cannot pass.
- **Fix:** added scoped `#[allow(dead_code)]` to the `impl Personality {}` block (covers both `label` and `ALL`), the `pub enum PaletteState`, and the `pub struct ShellSettings`, each with a `// consumed by Wave 4 (Plan 04-04X) — <consumer-name>` named-consumer comment.
- **Files modified:** `src/state.rs` (+3 attribute lines, no behavior change).
- **Commit:** `6ef651d` (folded into Task 5's commit since both are clippy-gate prerequisites).

**2. [Style consolidation] Mocks/ dead-code allow attributes**

- **Found during:** Task 5 finalization (working-tree had a mix of per-item `#[allow(dead_code)]` from in-progress iteration plus a module-level `#![allow(dead_code)]` in shell_outputs.rs).
- **Issue:** mid-iteration the prior executor had begun adding per-item `#[allow(dead_code)]` to mocks/ symbols, then switched to module-level `#![allow(dead_code)]` in shell_outputs.rs without removing the per-item attributes elsewhere.
- **Fix:** consolidated to a single module-level `#![allow(dead_code)]` per file (mod.rs additionally `unused_imports` for the run_agent_steps re-export), removed the per-item attributes. One style across all four mocks/ files.
- **Files modified:** `src/mocks/{personalities,shell_outputs,agent_steps,mod}.rs`.
- **Commit:** `6ef651d` (folded into Task 5's commit alongside the warp_hermes shim and state.rs allows).

## Temporary warp_hermes.rs Shim

Per the plan's Task 5 explicit instruction, this wave installs a TEMPORARY shim in `src/components/warp_hermes.rs` to keep the Wave 3 build green while leaving the proper rewire to Wave 4:

| Line | Before (Phase 3 / Wave 0) | After (Wave 3 shim) | Replaced by |
|------|---------------------------|----------------------|-------------|
| 6 | `demo_blocks,` (in the `use crate::state::{...}` block) | `demo_block_entries,` | Plan 04-04a — adds `BlockEntry` import + threads `Signal<Vec<BlockEntry>>` |
| 23 | `let blocks = demo_blocks();` | `let blocks: Vec<crate::state::Block> = demo_block_entries().into_iter().map(|e| e.block).collect();` | Plan 04-04a — `let blocks = use_signal(\|\| demo_block_entries());` |

The shim:
1. Imports the renamed `demo_block_entries` (from Wave 1's destructive rename per D-09).
2. Calls it, then strips the BlockEntry wrapper (`e.block`) to recover the Phase 3 `Vec<Block>` shape that `BlockStream` still requires.
3. Discards the per-block ids 1..=10 — they're invisible to BlockStream's current rendering.

This is documented in the plan as the bridge between Wave 1's destructive rename (which left `warp_hermes.rs:6` as the single carry-forward error) and Wave 4's prop-shape refactor (which converts BlockStream from `Vec<Block>` to `ReadOnlySignal<Vec<BlockEntry>>` and uses the ids for stable RSX keys per D-07/D-08).

**Wave 4 (Plan 04-04a) is responsible for replacing this shim entirely.**

## Issues Encountered

- **clippy promotes Wave-1 dead-code to errors** (handled inline as deviation Rule 3 — see above): the project's first `cargo clippy -- -D warnings` run surfaced four Wave-1-introduced-but-Wave-4-consumed symbols as errors. The plan anticipated clippy await-holding-invalid-types violations (the primary risk for this wave per CONTEXT D-06) but did not anticipate the Wave-1 dead-code promotion. Fixed by scoped `#[allow(dead_code)]` with named-consumer comments. No await-holding violations were triggered — the borrow-then-await discipline held first time across `run_shell` and `run_agent_steps`.
- **mid-iteration lint allow style mix** (handled inline as style consolidation): the prior executor had partially migrated from per-item to module-level allow attributes; finalized to module-level for consistency.

## Next Phase Readiness

- **Wave 4 (Plan 04-04a — BlockEntry-id propagation half) is unblocked.** All Wave 4 inputs are present: `BlockEntry { id, block }` (Wave 1), `run_shell` returns `Vec<BlockEntry>`-appended state (Wave 3), `crate::platform::timer::sleep` (Wave 0). Plan 04-04a will refactor `block.rs` and `block_stream.rs` to consume `ReadOnlySignal<Vec<BlockEntry>>` and remove the warp_hermes.rs shim's `.collect()`.
- **Wave 4 (Plan 04-04b — prop-shape refactor half) is unblocked.** Inputs ready: `ShellSettings` (Wave 1), `PaletteState` (Wave 1), `Personality::label` / `Personality::ALL` (Wave 1), `pick_reply` (Wave 3 — for KBD-06). Plan 04-04b will refactor `input_box`, `command_palette`, `status_bar`, `agent_panel` to take `Signal<T>` props.
- **Wave 5 (Plan 04-05 — WarpHermes integration) is type-unblocked.** `run_shell(text, blocks, next_id, scanner_active)` and `run_agent_steps(prompt, personality, messages)` have their final v2-swap-compatible signatures (D-36) — Plan 04-05's `submit()` closure spawns these directly without further refactoring.
- **clippy await-discipline verified at compile time** going forward. Future waves adding `.await` (Plan 04-05 spawn blocks, pulse_scanner, pulse_token) inherit the now-binding clippy gate; any violation will fail CI before reaching review.
- **Three-platform compile gate cadence preserved.** All three platforms exit 0 with zero warnings post-Wave-3.

## Threat Flags

None. The plan's `<threat_model>` has only T-04-04 (DoS via overlapping run_shell spawns — disposition `accept` per D-14) and T-04-INT (clippy await-discipline — `mitigate` verified by Task 5 gate). No new surface introduced beyond the planned mocks tree. No network, no clipboard, no untrusted input, no auth.

## Self-Check

- `src/mocks/personalities.rs` — FOUND
- `src/mocks/shell_outputs.rs` — FOUND
- `src/mocks/agent_steps.rs` — FOUND
- `src/mocks/mod.rs` — FOUND
- `src/main.rs` — `mod mocks;` line — FOUND
- `src/components/warp_hermes.rs` — shim with `demo_block_entries` import and `.collect()` line — FOUND
- `src/state.rs` — three scoped `#[allow(dead_code)]` attributes — FOUND
- `.planning/phases/04-data-layer-interactions/04-03-SUMMARY.md` — FOUND (this file)
- Commit `c9a9cba` (Task 1: feat — personalities.rs) — FOUND
- Commit `811e147` (Task 2: feat — shell_outputs.rs) — FOUND
- Commit `ffaec5e` (Task 3: feat — agent_steps.rs) — FOUND
- Commit `983e8db` (Task 4: feat — mocks/mod.rs + main.rs registration) — FOUND
- Commit `6ef651d` (Task 5: feat — warp_hermes shim + Wave-1 dead-code allows) — FOUND
- Three-platform `cargo build --no-default-features --features {web|desktop|mobile}` — exit 0, zero warnings
- `cargo clippy --no-default-features --features web -- -D warnings` — exit 0

## Self-Check: PASSED

---
*Phase: 04-data-layer-interactions*
*Completed: 2026-05-03*
