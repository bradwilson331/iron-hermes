---
phase: 04-data-layer-interactions
plan: 04b
subsystem: shell

# Dependency graph
requires:
  - phase: 04-data-layer-interactions
    plan: 02
    provides: "Personality enum (label/ALL), BlockEntry, PaletteState, ShellSettings, now_time()"
  - phase: 04-data-layer-interactions
    plan: 03
    provides: "mocks/ module tree + Wave 3 warp_hermes.rs shim"
  - phase: 04-data-layer-interactions
    plan: 04a
    provides: "BlockStream ReadSignal<Vec<BlockEntry>> refactor + markdown.rs + block.rs entry-typed props; ReadSignal (not ReadOnlySignal) as the non-deprecated Dioxus 0.7.1 prop type"
provides:
  - "src/components/shell/input_box.rs: InputBox refactored — controlled textarea (Signal<String> value + oninput) + on_submit EventHandler<()> + Enter/Shift+Enter keydown + focus Signal<bool> (D-19, KBD-04)"
  - "src/components/shell/command_palette.rs: CommandPalette refactored — controlled query Signal<String>, ↑/↓/Enter local Signal<usize> nav, Browse|PersonalityPick substate, live filter on cmd+label (D-18, D-20, D-32, KBD-03)"
  - "src/components/shell/status_bar.rs: StatusBar refactored — ReadSignal<TokenBudget> + ReadSignal<bool> props + personality pill via use_context::<ShellSettings>() (D-22)"
  - "src/components/shell/agent_panel.rs: AgentPanel refactored — ReadSignal<Vec<Message>> + personality prop removed, read via use_context (D-02, KBD-06)"
  - "src/components/warp_hermes.rs: Wave 4 expanded shim with use_context_provider(ShellSettings) + all-signal no-op placeholder body; Plan 04-05 replaces entirely"
  - "Three-platform cargo build green AND cargo clippy --features web -- -D warnings green (Wave 4 exit witness)"
affects: [04-05]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Controlled textarea via Signal<String> + value interpolation + oninput — canonical Dioxus 0.7 reactive form pattern"
    - "EventHandler<()> props for delegated actions (on_submit, on_pick, on_copy, on_rerun) — component fires, parent resolves semantics"
    - "Local Signal<usize> for palette selected index with use_effect reset on query change — pattern for any local-keyboard-nav component"
    - "use_context::<ShellSettings>() for cross-cutting personality — replaces prop-drilling; Signal<Personality> inside the bundle provides reactive re-render on personality switch (KBD-06)"
    - "ReadSignal<T> (not ReadOnlySignal<T>) for read-only-but-reactive props — Dioxus 0.7.1 deprecated ReadOnlySignal; clippy -D warnings promotes deprecation to error (per 04-04a finding)"

key-files:
  created: []
  modified:
    - "src/components/shell/input_box.rs"
    - "src/components/shell/command_palette.rs"
    - "src/components/shell/status_bar.rs"
    - "src/components/shell/agent_panel.rs"
    - "src/components/warp_hermes.rs"
    - "src/state.rs"

key-decisions:
  - "ReadSignal<T> used instead of ReadOnlySignal<T> for read-only reactive props — Dioxus 0.7.1 deprecated ReadOnlySignal; clippy -- -D warnings promotes deprecation warnings to errors (per 04-04a decision D-1)"
  - "Signal<T> to ReadSignal<T> prop conversion uses direct pass (not .into()) — matches 04-04a finding that .into() is ambiguous across multiple SuperInto impls in dioxus_core vs dioxus_stores; the #[component] macro's generated props builder handles the conversion internally"
  - "PaletteState::PersonalityPick gets targeted #[allow(dead_code)] — used in command_palette.rs match arm but never constructed as a value in Wave 4 shim (Plan 04-05 constructs it on /personality pick); narrower than previous blanket suppression on the whole enum"
  - "Command palette selected index uses use_effect to reset on query change — reads query.read() to register dependency, then sets selected to 0; per PATTERNS risk note"

patterns-established:
  - "Pattern: controlled Dioxus textarea — Signal<String> value prop + value: \"{value}\" interpolation + oninput: move |e| value.set(e.value()) — reactivity preserved through Signal Display impl"
  - "Pattern: keyboard-gated submit — onkeydown checks e.key() == Key::Enter && !e.modifiers().shift() → e.prevent_default() + on_submit.call(()); Shift+Enter falls through to browser-default newline"
  - "Pattern: local keyboard nav with wrapping — Signal<usize> selected index, ArrowDown: (s+1)%len, ArrowUp: (s+len-1)%len, Enter: on_pick.call(items[selected])"
  - "Pattern: personality via use_context — ShellSettings { personality: Signal<Personality> } provided by WarpHermes via use_context_provider; consumed by AgentPanel and StatusBar via use_context::<ShellSettings>() for reactive cross-component personality updates without prop-drilling"

requirements-completed: [KBD-03, KBD-04]

# Metrics
duration: ~5min
completed: 2026-05-03
---

# Phase 04 Plan 04b: Prop-Shape Refactor (Non-BlockEntry Primitives) Summary

**Refactored four shell primitives with Signal/ReadSignal/EventHandler prop signatures: InputBox (controlled textarea + on_submit + focus signal), CommandPalette (controlled query + ↑/↓/Enter local nav + Browse/PersonalityPick substate + live filter), StatusBar (ReadSignal props + personality pill via use_context), AgentPanel (ReadSignal<Vec<Message>> + personality via use_context replacing removed prop). Three-platform compile gate green + clippy green via expanded WarpHermes shim.**

## Performance

- **Duration:** ~5 min
- **Started:** 2026-05-03T17:31:21Z
- **Completed:** 2026-05-03T17:36:00Z
- **Tasks:** 5 (all auto)
- **Files modified:** 6 (0 created, 6 modified)

## Accomplishments

- **InputBox** now accepts `Signal<String>` value + `ReadSignal<Mode>` mode + `Signal<bool>` focused + `EventHandler<()>` on_submit. Controlled textarea via `value: "{value}"` + `oninput: move |e| value.set(e.value())`. Enter (no shift) calls `on_submit.call(())` + prevents default; Shift+Enter falls through. Focus signal written by `onfocus`/`onblur` for ⌥M gating. Run button click also calls `on_submit`. (KBD-04 ✓, D-17 ✓, D-19 ✓)

- **CommandPalette** now accepts `Signal<String>` query + `ReadSignal<bool>` open + `Signal<PaletteState>` state + `EventHandler<PaletteItem>` on_pick. Local `Signal<usize>` selected walked by ↑/↓ with boundary wrapping. Enter dispatches `on_pick.call(items[selected])`. Live filter via `to_lowercase().contains` on cmd+label. PersonalityPick substate enumerates `Personality::ALL` as palette rows. Selected resets to 0 on query change via `use_effect`. `tabindex: "-1"` + `autofocus: true` for keyboard focus routing. (KBD-03 ✓, D-18 ✓, D-20 ✓, D-32 ✓)

- **StatusBar** now accepts `ReadSignal<TokenBudget>` tokens + `ReadSignal<bool>` scanner_active. Personality pill added between tokens and scanner via `use_context::<ShellSettings>()` reading `settings.personality.read().label()`. Pill displays `"/{label}"` with `--pill-4` color token. (D-22 ✓, D-02 ✓)

- **AgentPanel** now accepts only `ReadSignal<Vec<Message>>` messages. `personality: String` prop REMOVED. Personality read via `use_context::<ShellSettings>()` for reactive cross-component updates without prop-drilling. (D-02 ✓, KBD-06 ✓)

- **WarpHermes expanded shim** provides `use_signal`-backed values for all new prop types from both 04-04a and 04-04b. Installs `use_context_provider(|| ShellSettings { personality })`. All closures are no-ops. Plan 04-05 replaces the file entirely.

- **Three-platform compile gate green**: `cargo build --features {web|desktop|mobile} --no-default-features` all exit 0.
- **Clippy gate green**: `cargo clippy --features web --no-default-features -- -D warnings` exits 0 with zero warnings.

## Task Commits

Each task was committed atomically:

1. **Task 1: Refactor input_box.rs** — `556e343` (feat)
2. **Task 2: Refactor command_palette.rs** — `a810f8a` (feat)
3. **Task 3: Refactor status_bar.rs** — `c253093` (feat)
4. **Task 4: Refactor agent_panel.rs** — `f9d109c` (feat)
5. **Task 5: Expand warp_hermes.rs shim + three-platform compile gate** — `4bede9c` (feat)

**Plan metadata commit:** pending (final commit after STATE.md / ROADMAP.md updates).

## Files Created/Modified

- `src/components/shell/input_box.rs` (MODIFIED, +44/-19) — controlled textarea with Signal<String> value + oninput + onkeydown Enter/Shift+Enter + onfocus/onblur focus signal + on_submit EventHandler + run button click handler
- `src/components/shell/command_palette.rs` (MODIFIED, +127/-26) — controlled query Signal<String> + ReadSignal<bool> open + Signal<PaletteState> state + on_pick EventHandler + local Signal<usize> selected + ↑/↓/Enter local nav + live filter + PersonalityPick substate + tabindex + autofocus
- `src/components/shell/status_bar.rs` (MODIFIED, +25/-18) — ReadSignal<TokenBudget> + ReadSignal<bool> scanner_active + personality pill via use_context::<ShellSettings>() + --pill-4 color token
- `src/components/shell/agent_panel.rs` (MODIFIED, +18/-13) — ReadSignal<Vec<Message>> + personality prop removed + personality via use_context::<ShellSettings>()
- `src/components/warp_hermes.rs` (MODIFIED, +40/-18) — expanded Wave 4 shim with use_signal-backed values for all prop types + use_context_provider(ShellSettings) + all no-op closures
- `src/state.rs` (MODIFIED, +2/-1) — targeted #[allow(dead_code)] on PaletteState::PersonalityPick variant with named-consumer comment

## Decisions Made

- **ReadSignal over ReadOnlySignal** (per 04-04a finding): `ReadOnlySignal<T>` is deprecated in Dioxus 0.7.1 and `clippy -- -D warnings` promotes the deprecation to a hard error. `ReadSignal<T>` is the non-deprecated equivalent with the same semantics. All four refactored primitives use `ReadSignal<T>` for read-only-but-reactive props.

- **Direct Signal-to-ReadSignal prop pass** (not `.into()`): The `Signal<T>` to `ReadSignal<T>` conversion at call sites uses direct pass (e.g., `mode: mode,` not `mode: mode.into()`). This matches 04-04a's finding that `.into()` is ambiguous across multiple `SuperInto` impls; the `#[component]` macro's generated props builder handles the conversion internally.

- **Targeted dead_code allow on PersonalityPick variant only**: The previous Wave 3 suppression was a blanket `#[allow(dead_code)]` on the entire `PaletteState` enum. This plan wires `PaletteState::Browse` (default in shim + match arm in command_palette). `PersonalityPick` is used in a match arm pattern but never constructed as a value (the shim stays in `Browse`; Plan 04-05 wires the transition). The new suppression is narrower — only the `PersonalityPick` variant gets `#[allow(dead_code)]` with a named-consumer comment pointing to Plan 04-05.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Targeted #[allow(dead_code)] on PaletteState::PersonalityPick**
- **Found during:** Task 5 (clippy gate)
- **Issue:** `cargo clippy --features web --no-default-features -- -D warnings` failed with error: `variant PersonalityPick is never constructed`. The variant is used in `command_palette.rs` match arm but never constructed as a value — the shim always stays in `Browse` state, and Plan 04-05 wires the actual `/personality` transition.
- **Fix:** Added `#[allow(dead_code)]` with named-consumer comment on just the `PersonalityPick` variant (not the whole enum). This is narrower than the previous blanket suppression removed by 04-04a.
- **Files modified:** `src/state.rs`
- **Verification:** `cargo clippy --features web --no-default-features -- -D warnings` exits 0.
- **Committed in:** `4bede9c` (folded into Task 5 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** The `#[allow(dead_code)]` on `PersonalityPick` is narrower and more targeted than the previous blanket suppression on the entire `PaletteState` enum. No scope creep. Plan 04-05 will remove this allow when it wires the transition.

## Wave 4 Expanded WarpHermes Shim

The expanded `warp_hermes.rs` provides minimal reactive plumbing so that all refactored primitives type-check, but ZERO behavioral wiring:

| Signal | Type | Initial Value | Behavioral Wiring |
|--------|------|---------------|-------------------|
| `blocks` | `Signal<Vec<BlockEntry>>` | `demo_block_entries()` | Append via run_shell — Plan 04-05 |
| `messages` | `Signal<Vec<Message>>` | `demo_messages()` | Append via run_agent_steps — Plan 04-05 |
| `tokens` | `Signal<TokenBudget>` | `{ used: 12_300, max: 128_000 }` | pulse_token(120) — Plan 04-05 |
| `scanner_active` | `Signal<bool>` | `true` | pulse_scanner(2000) — Plan 04-05 |
| `input` | `Signal<String>` | `String::new()` | submit() reads+clears — Plan 04-05 |
| `mode` | `Signal<Mode>` | `Mode::Shell` | ⌥M toggle — Plan 04-05 |
| `focused` | `Signal<bool>` | `true` | onfocus/onblur — wired in InputBox |
| `pal_open` | `Signal<bool>` | `true` | ⌘K toggle + Esc close — Plan 04-05 |
| `pal_query` | `Signal<String>` | `String::new()` | Controlled in CommandPalette — wired |
| `pal_state` | `Signal<PaletteState>` | `Browse` | /personality transition — Plan 04-05 |
| `personality` | `Signal<Personality>` | `Default` | /personality pick — Plan 04-05 |

**Context provider:** `use_context_provider(|| ShellSettings { personality })` installed — consumed by `StatusBar` and `AgentPanel` via `use_context::<ShellSettings>()`.

**All closures are no-ops:** `on_submit: move |_| {}`, `on_rerun: move |_id: u64| {}`, `on_pick: move |_item: PaletteItem| {}`.

**Plan 04-05 replaces this file entirely** with the integration body (12 use_signals + use_context_provider + global keydown + auto-scroll + pulse_scanner/pulse_token + submit/on_rerun/pick + checkpoint:human-verify UAT).

## Wave 4 Coordination

Plan 04-04a (BlockEntry-id propagation: markdown.rs, block.rs, block_stream.rs) and this plan (04-04b: input_box, command_palette, status_bar, agent_panel) touch disjoint primitive files. Both edit `warp_hermes.rs` as a transient shim. 04-04a ran first and established the `ReadSignal` pattern + the initial shim. 04-04b extends the shim to cover the four additional refactored primitives' new prop signatures. The combined shim is the Wave 4 exit witness — three-platform build green + clippy green.

## Known Stubs

| File | Line | Description | Resolution |
|------|------|-------------|------------|
| `src/components/warp_hermes.rs` | ~49 | `on_submit: move \|_\| {}` no-op shim | Plan 04-05 — full WarpHermes rewire replaces the entire file |
| `src/components/warp_hermes.rs` | ~44 | `on_rerun: move \|_id: u64\| {}` no-op shim | Plan 04-05 — full WarpHermes rewire replaces the entire file |
| `src/components/warp_hermes.rs` | ~62 | `on_pick: move \|_item: PaletteItem\| {}` no-op shim | Plan 04-05 — full WarpHermes rewire replaces the entire file |
| `src/components/shell/input_box.rs` | ~73 | Attach button onclick no-op `/* file-picker is v2 */` | v2 — real file picker |
| `src/components/shell/input_box.rs` | ~79 | Voice button onclick no-op `/* audio capture is v2 */` | v2 — real audio capture |
| `src/components/shell/block.rs` | ~289 | Share button onclick no-op | v2 — real share-link backend |

## Threat Flags

None. The plan's `<threat_model>` has T-04-03 (command_palette substring filter — disposition `mitigate` via pure-string operation, no regex/eval) and T-04-NA-01 (input_box/status_bar/agent_panel — n/a, pure prop refactor). No new trust boundaries beyond those documented.

## Self-Check

- `src/components/shell/input_box.rs` — FOUND (InputBox with Signal<String> value, ReadSignal<Mode> mode, Signal<bool> focused, EventHandler<()> on_submit)
- `src/components/shell/command_palette.rs` — FOUND (CommandPalette with Signal<String> query, ReadSignal<bool> open, Signal<PaletteState> state, EventHandler<PaletteItem> on_pick)
- `src/components/shell/status_bar.rs` — FOUND (StatusBar with ReadSignal<TokenBudget> tokens, ReadSignal<bool> scanner_active, use_context::<ShellSettings>)
- `src/components/shell/agent_panel.rs` — FOUND (AgentPanel with ReadSignal<Vec<Message>> messages, use_context::<ShellSettings>, no personality: String prop)
- `src/components/warp_hermes.rs` — FOUND (expanded shim with all signals + use_context_provider)
- `src/state.rs` — FOUND (#[allow(dead_code)] on PersonalityPick variant)
- Commit `556e343` (Task 1) — FOUND
- Commit `a810f8a` (Task 2) — FOUND
- Commit `c253093` (Task 3) — FOUND
- Commit `f9d109c` (Task 4) — FOUND
- Commit `4bede9c` (Task 5) — FOUND
- Three-platform `cargo build` — exits 0, zero warnings
- `cargo clippy --features web -- -D warnings` — exits 0

## Self-Check: PASSED

---
*Phase: 04-data-layer-interactions*
*Plan: 04b — Wave 4 second half (prop-shape refactor for non-BlockEntry primitives)*
*Completed: 2026-05-03*
