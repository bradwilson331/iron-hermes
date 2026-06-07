---
phase: 04-data-layer-interactions
plan: 04b
type: execute
wave: 4
depends_on: [01, 02, 03]
files_modified:
  - src/components/shell/input_box.rs
  - src/components/shell/command_palette.rs
  - src/components/shell/status_bar.rs
  - src/components/shell/agent_panel.rs
  - src/components/warp_hermes.rs
autonomous: true
requirements: [KBD-03, KBD-04]
tags: [dioxus, components, signals, refactor, props, event-handlers, palette, controlled-textarea, use_context]
must_haves:
  truths:
    - "src/components/shell/input_box.rs InputBox prop signature is (value: Signal<String>, mode: ReadOnlySignal<Mode>, focused: Signal<bool>, on_submit: EventHandler<()>); textarea is controlled (value=\"{value}\", oninput updates value); onkeydown checks Enter && !shift → e.prevent_default() + on_submit.call(()) (per D-19 + KBD-04); onfocus/onblur write the focused signal (per D-17 ⌥M gating)"
    - "src/components/shell/command_palette.rs CommandPalette prop signature is (items: Vec<PaletteItem>, query: Signal<String>, open: ReadOnlySignal<bool>, state: Signal<PaletteState>, on_pick: EventHandler<PaletteItem>); early-returns if !open(); local Signal<usize> selected; live filter on query() lowercase substring match against cmd+label (per D-32); search input controlled (oninput updates query, also resets selected to 0); PaletteState::Browse renders filtered PALETTE_ITEMS, PaletteState::PersonalityPick renders Personality::ALL as palette rows (per D-20 + KBD-03); palette overlay onkeydown handles ArrowUp/ArrowDown wrapping (per D-18) and Enter dispatching on_pick (per KBD-03)"
    - "src/components/shell/status_bar.rs StatusBar prop signature is (mode: String, model: String, provider: String, tokens: ReadOnlySignal<TokenBudget>, scanner_active: ReadOnlySignal<bool>, hint: String); reads tokens() owned (Copy); passes scanner_active() to Scanner; adds personality pill via use_context::<ShellSettings>() reading settings.personality.read().label() (per D-22)"
    - "src/components/shell/agent_panel.rs AgentPanel prop signature is (messages: ReadOnlySignal<Vec<Message>>); REMOVES the personality: String prop (now read via use_context::<ShellSettings>()); iterates messages.read() (per D-02 + KBD-06)"
    - "src/components/warp_hermes.rs receives a transient shim edit so the four refactored prop signatures (InputBox/CommandPalette/StatusBar/AgentPanel) type-check; combines with Plan 04-04a's BlockStream shim into one expanded shim WarpHermes body; Plan 04-05 will REPLACE the file entirely"
    - "Three-platform compile gate green AND cargo clippy --features web -- -D warnings green (per D-35); the expanded warp_hermes.rs shim is the wave-4 exit witness; Plan 04-05 (Wave 5) is the integration wave that replaces the shim"
  artifacts:
    - path: "src/components/shell/input_box.rs"
      provides: "InputBox refactored: controlled textarea + on_submit closure prop (D-19, KBD-04)"
    - path: "src/components/shell/command_palette.rs"
      provides: "CommandPalette refactored: controlled query, ↑/↓/Enter local nav, Browse|PersonalityPick substate (D-18, D-20, D-32, KBD-03)"
    - path: "src/components/shell/status_bar.rs"
      provides: "StatusBar refactored to read-only signal props + personality pill via use_context (D-22)"
    - path: "src/components/shell/agent_panel.rs"
      provides: "AgentPanel refactored: ReadOnlySignal<Vec<Message>> + personality via use_context (D-02, KBD-06)"
    - path: "src/components/warp_hermes.rs"
      provides: "Transient expanded shim that satisfies the new prop signatures; full replacement deferred to Plan 04-05"
  key_links:
    - from: "command_palette.rs query filter"
      to: "PaletteItem.cmd + PaletteItem.label"
      via: "case-insensitive substring match per D-32"
      pattern: "to_lowercase\\(\\)\\.contains"
    - from: "status_bar.rs personality pill"
      to: "ShellSettings.personality"
      via: "use_context::<ShellSettings>()"
      pattern: "use_context::<ShellSettings>"
    - from: "agent_panel.rs personality"
      to: "ShellSettings.personality"
      via: "use_context (replaces personality: String prop)"
      pattern: "use_context::<ShellSettings>"
---

<objective>
Wave 4 second half — Prop-shape refactor for the four primitives that do
NOT participate in BlockEntry-id propagation. Refactor input_box.rs
(controlled textarea + on_submit closure), command_palette.rs (controlled
query + ↑/↓/Enter local nav + Browse|PersonalityPick substate + live
filter), status_bar.rs (read-only signal props + personality pill via
use_context), agent_panel.rs (ReadOnlySignal<Vec<Message>> + drop personality
prop). Then run the three-platform compile gate (the wave-4 exit witness
shared with Plan 04-04a).

Plan 04-04a runs IN PARALLEL with this plan and refactors block.rs,
block_stream.rs, and creates markdown.rs. Plans 04-04a and 04-04b touch
disjoint primitive files but BOTH edit warp_hermes.rs as a transient shim;
the executor coordinates so the final shim covers both halves' new prop
signatures. Plan 04-05 then replaces warp_hermes.rs entirely.

Output: 4 MODIFIED component files (input_box, command_palette, status_bar,
agent_panel), 1 MODIFIED warp_hermes.rs (expanded shim), three-platform
compile gate green + clippy green.

Requirements:
- KBD-03: ↑/↓ palette navigation + Enter selection (local Signal<usize>).
- KBD-04: Enter submits via on_submit closure; Shift+Enter inserts newline.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/PROJECT.md
@.planning/ROADMAP.md
@.planning/phases/04-data-layer-interactions/04-CONTEXT.md
@.planning/phases/04-data-layer-interactions/04-RESEARCH.md
@.planning/phases/04-data-layer-interactions/04-PATTERNS.md
@.planning/phases/04-data-layer-interactions/04-VALIDATION.md
@CLAUDE.md
@AGENTS.md
@src/state.rs
@src/components/shell/mod.rs
@src/components/shell/input_box.rs
@src/components/shell/command_palette.rs
@src/components/shell/status_bar.rs
@src/components/shell/agent_panel.rs
@src/components/shell/scanner.rs
@src/components/warp_hermes.rs

<interfaces>
After Wave 1 (Plan 04-02), `src/state.rs` exposes:

```rust
pub enum Mode { Shell, Agent }
pub struct PaletteItem { section, cmd, label, kbd }
pub struct Message { who, time, body, tool }
pub struct TokenBudget { used: u32, max: u32 }     // derives Copy
pub enum Personality { Concise, Technical, Noir, Hype, Catgirl, Default }
impl Personality { pub fn label(&self) -> &'static str; pub const ALL: [Personality; 6]; }
pub enum PaletteState { Browse, PersonalityPick }
pub struct ShellSettings { pub personality: Signal<Personality> }     // Copy
```

Dioxus 0.7 prop conversions verified:
- `Signal<T>` → `ReadOnlySignal<T>` via `.into()` at the call site (Plan 04-05 wires).
- `EventHandler<T>` accepts `move |v| { ... }` from caller; component invokes `on_x.call(value)`.
- Keyboard: `e.key() == Key::Enter` (Key from `dioxus::events::Key`, also via prelude).
- Modifiers: `e.modifiers().shift() -> bool`.
- preventDefault: `e.prevent_default()`.

CURRENT prop signatures Plan 04-04b REPLACES:
```rust
pub fn InputBox(mode: Mode, focused: bool) -> Element                             // input_box.rs:22
pub fn CommandPalette(items: Vec<PaletteItem>, query: String, open: bool) -> Element  // command_palette.rs:19
pub fn StatusBar(mode, model, provider, tokens: TokenBudget, scanner_active: bool, hint) -> Element   // status_bar.rs:19
pub fn AgentPanel(messages: Vec<Message>, personality: String) -> Element         // agent_panel.rs:19
```
</interfaces>
</context>

<tasks>

<task type="auto">
  <name>Task 1: Refactor input_box.rs (controlled textarea + on_submit closure prop + onkeydown + focus signal)</name>
  <files>src/components/shell/input_box.rs</files>
  <read_first>
    - src/components/shell/input_box.rs (current — preserve all RSX class strings, mode pill, prompt glyph, action buttons)
    - .planning/phases/04-data-layer-interactions/04-CONTEXT.md §"Submit Routing & Tokenization" (D-13) + §"Keyboard Handler Placement" (D-17 ⌥M gating; D-19 textarea Enter/Shift+Enter)
    - .planning/phases/04-data-layer-interactions/04-PATTERNS.md §"MOD src/components/shell/input_box.rs"
    - AGENTS.md (Key::Enter pattern + e.modifiers().shift() + e.prevent_default())
  </read_first>
  <action>
    REPLACE the entirety of `src/components/shell/input_box.rs` with EXACTLY:

    ```rust
    use dioxus::prelude::*;
    use crate::state::Mode;

    /// InputBox — bottom-row form chrome with mode pill, prompt glyph,
    /// auto-grow textarea, and right-side action buttons.
    ///
    /// Phase 4 (per CONTEXT D-19 + KBD-04): textarea is controlled
    /// (`value: "{value}"` + `oninput` → `value.set(...)`). Plain Enter calls
    /// `on_submit.call(())` and prevents default; Shift+Enter inserts a
    /// newline (browser-default textarea behavior — no special handling).
    ///
    /// Per CONTEXT D-17: `focused` is a `Signal<bool>` written by `onfocus`
    /// (true) and `onblur` (false). The global keydown listener in
    /// `WarpHermes` reads this same signal to gate ⌥M mode toggle (so ⌥M
    /// only fires when input is focused — avoids intercepting macOS
    /// Option-letter combos that produce special characters).
    ///
    /// Mode-driven pill/glyph/placeholder copy unchanged from Phase 3 UI-SPEC
    /// lines 220-227. The run-button accent color preserved per UI-SPEC line 224.
    #[component]
    pub fn InputBox(
        value: Signal<String>,
        mode: ReadOnlySignal<Mode>,
        focused: Signal<bool>,
        on_submit: EventHandler<()>,
    ) -> Element {
        let is_agent = matches!(mode(), Mode::Agent);
        let pill_label   = if is_agent { "Agent" } else { "Shell" };
        let prompt_glyph = if is_agent { "✦" }     else { "❯" };
        let placeholder  = if is_agent { "Ask IronHermes anything…" } else { "Type a command, or `/` for commands" };
        rsx! {
            div {
                class: "wh-input-wrap",
                class: if focused() { "is-focus" },
                div { class: "wh-input-mode",
                    span {
                        class: "wh-mode-pill",
                        class: if is_agent { "is-agent" },
                        "{pill_label}"
                    }
                    span { "⌥+M to switch" }
                    span { style: "margin-left: auto;", "↵ run · ⇧↵ newline · ⌃C cancel" }
                }
                div { class: "wh-input-row",
                    span { class: "wh-prompt-glyph", "{prompt_glyph}" }
                    textarea {
                        class: "wh-textarea",
                        rows: "1",
                        placeholder: "{placeholder}",
                        value: "{value}",
                        oninput: move |e| value.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter && !e.modifiers().shift() {
                                e.prevent_default();
                                on_submit.call(());
                            }
                        },
                        onfocus: move |_| focused.set(true),
                        onblur:  move |_| focused.set(false),
                    }
                    div { class: "wh-input-actions",
                        button {
                            class: "wh-icon-btn",
                            title: "attach",
                            onclick: move |_| { /* attach unwired in Phase 4 (file-picker is v2). */ },
                            "@"
                        }
                        button {
                            class: "wh-icon-btn",
                            title: "voice",
                            onclick: move |_| { /* voice unwired in Phase 4 (audio capture is v2). */ },
                            "●"
                        }
                        button {
                            class: "wh-icon-btn",
                            title: "run",
                            style: "color: var(--accent-primary);",
                            onclick: move |_| on_submit.call(()),
                            "↵"
                        }
                    }
                }
            }
        }
    }
    ```

    Critical (per PATTERNS risks + D-19):
    - `value: Signal<String>` enables the controlled-component pattern. The interpolation `value: "{value}"` reads via Display impl on Signal — reactivity preserved.
    - `Key::Enter` is in `dioxus::prelude::*` re-export per AGENTS.md line 112; no extra import needed.
    - `e.modifiers().shift()` returns bool. Shift+Enter falls through to browser-default (newline insert into textarea).
    - `e.prevent_default()` only fires on plain Enter (no shift). On Shift+Enter we let the browser default through.
    - The global keydown handler in WarpHermes (Plan 04-05) reads `focused` for ⌥M gating — this Signal MUST be the same one the parent passes; passing by Signal<bool> ensures both directions stay in sync.
    - The run button onclick (`↵`) ALSO calls `on_submit.call(())` per UI-SPEC parity — clicking run is equivalent to pressing Enter.
    - Attach (@) and voice (●) buttons get explicit onclick no-ops with comments — keeps the Phase 3 button rendering visible per visual-fidelity rule.
  </action>
  <verify>
    <automated>grep -q 'pub fn InputBox(' src/components/shell/input_box.rs && grep -q 'value: Signal<String>,' src/components/shell/input_box.rs && grep -q 'mode: ReadOnlySignal<Mode>,' src/components/shell/input_box.rs && grep -q 'focused: Signal<bool>,' src/components/shell/input_box.rs && grep -q 'on_submit: EventHandler<()>,' src/components/shell/input_box.rs && grep -q 'oninput: move |e| value.set(e.value())' src/components/shell/input_box.rs && grep -q 'e.key() == Key::Enter' src/components/shell/input_box.rs && grep -q '!e.modifiers().shift()' src/components/shell/input_box.rs && grep -q 'e.prevent_default()' src/components/shell/input_box.rs && grep -q 'on_submit.call(())' src/components/shell/input_box.rs && grep -q 'onfocus: move |_| focused.set(true)' src/components/shell/input_box.rs && grep -q 'onblur:  move |_| focused.set(false)' src/components/shell/input_box.rs</automated>
  </verify>
  <done>
    input_box.rs accepts controlled value Signal + ReadOnlySignal Mode + focused Signal + on_submit EventHandler. Enter (no shift) submits + prevents default; Shift+Enter falls through; focus signal wired; run button click also submits.
  </done>
</task>

<task type="auto">
  <name>Task 2: Refactor command_palette.rs (controlled query + ↑/↓/Enter local nav + Browse/PersonalityPick substate + live filter)</name>
  <files>src/components/shell/command_palette.rs</files>
  <read_first>
    - src/components/shell/command_palette.rs (current — preserve wh-pal-overlay/wh-pal-search/wh-pal-list/wh-pal-section/wh-pal-row class strings)
    - src/state.rs (PaletteItem, PaletteState, Personality)
    - .planning/phases/04-data-layer-interactions/04-CONTEXT.md §"Personality Switching UX" (D-20 PaletteState transitions) + §"Keyboard Handler Placement" (D-18 palette ↑/↓/Enter local) + §"Palette Pick Handlers" (D-32 substring filter)
    - .planning/phases/04-data-layer-interactions/04-PATTERNS.md §"MOD src/components/shell/command_palette.rs"
  </read_first>
  <action>
    REPLACE the entirety of `src/components/shell/command_palette.rs` with EXACTLY:

    ```rust
    use dioxus::prelude::*;
    use crate::state::{PaletteItem, PaletteState, Personality};

    /// CommandPalette — overlay with two substates per CONTEXT D-20:
    ///
    ///   - `Browse`         — filtered PALETTE_ITEMS (slash + workflow rows).
    ///   - `PersonalityPick` — six rows for `Personality::ALL`; selecting one
    ///                        writes ShellSettings.personality (Plan 04-05 wires).
    ///
    /// Phase 4 adds (per CONTEXT D-18 + D-32 + KBD-03):
    ///   - Live filter: `query()` lowercase substring match against
    ///     `cmd` + `label` fields.
    ///   - Local `Signal<usize> selected` index walked by ↑/↓ keys (wraps
    ///     at boundaries).
    ///   - Enter dispatches `on_pick.call(items[selected])`.
    ///
    /// Esc behavior is handled by the global keydown listener in WarpHermes
    /// (D-17), not this component.
    ///
    /// Per PATTERNS file risk note: `selected` resets to 0 whenever `query`
    /// changes via a `use_effect` watching `query()`.
    #[component]
    pub fn CommandPalette(
        items: Vec<PaletteItem>,
        query: Signal<String>,
        open: ReadOnlySignal<bool>,
        state: Signal<PaletteState>,
        on_pick: EventHandler<PaletteItem>,
    ) -> Element {
        if !open() {
            return rsx! {};
        }

        let mut selected = use_signal(|| 0_usize);

        // Reset selected when query changes (per PATTERNS risk note).
        use_effect(move || {
            let _ = query.read();
            selected.set(0);
        });

        // Build the filtered/substate-derived items list.
        let cur_state = state();
        let items_for_render: Vec<PaletteItem> = match cur_state {
            PaletteState::Browse => {
                let q = query().to_lowercase();
                items
                    .iter()
                    .filter(|p| {
                        p.cmd.to_lowercase().contains(&q)
                            || p.label.to_lowercase().contains(&q)
                    })
                    .cloned()
                    .collect()
            }
            PaletteState::PersonalityPick => Personality::ALL
                .iter()
                .map(|p| PaletteItem {
                    section: "personality".into(),
                    cmd: format!("/{}", p.label()),
                    label: format!("Personality: {}", p.label()),
                    kbd: vec![],
                })
                .collect(),
        };

        let len = items_for_render.len();
        let items_for_keys = items_for_render.clone();

        let on_keydown = move |e: KeyboardEvent| {
            if len == 0 {
                return;
            }
            match e.key() {
                Key::ArrowDown => {
                    let s = selected();
                    selected.set((s + 1) % len);
                    e.prevent_default();
                }
                Key::ArrowUp => {
                    let s = selected();
                    selected.set((s + len - 1) % len);
                    e.prevent_default();
                }
                Key::Enter => {
                    if let Some(item) = items_for_keys.get(selected()) {
                        on_pick.call(item.clone());
                    }
                    e.prevent_default();
                }
                _ => {}
            }
        };

        let slash_items: Vec<PaletteItem> = items_for_render
            .iter()
            .filter(|p| p.section == "slash" || p.section == "personality")
            .cloned()
            .collect();
        let workflow_items: Vec<PaletteItem> = items_for_render
            .iter()
            .filter(|p| p.section == "workflow")
            .cloned()
            .collect();

        let section_label = match cur_state {
            PaletteState::Browse => "Slash commands",
            PaletteState::PersonalityPick => "Personalities",
        };

        rsx! {
            div { class: "wh-pal-overlay",
                tabindex: "-1",
                onkeydown: on_keydown,
                div { class: "wh-pal",
                    div { class: "wh-pal-search",
                        span {
                            style: "color: var(--accent-primary); font-weight: 700;",
                            "⌘K"
                        }
                        input {
                            placeholder: "Search commands, files, recent…",
                            value: "{query}",
                            oninput: move |e| query.set(e.value()),
                            autofocus: true,
                        }
                        span { class: "wh-kbd", "esc" }
                    }
                    div { class: "wh-pal-list",
                        div { class: "wh-pal-section", "{section_label}" }
                        for (i, it) in slash_items.iter().enumerate() {
                            div {
                                key: "{it.cmd}",
                                class: "wh-pal-row",
                                class: if i == selected() { "is-active" },
                                onclick: {
                                    let item = it.clone();
                                    move |_| on_pick.call(item.clone())
                                },
                                span { class: "wh-pal-glyph", "/" }
                                span { style: "color: var(--fg-strong);", "{it.cmd}" }
                                span { style: "color: var(--fg-dim);", "— {it.label}" }
                                span { class: "wh-pal-hint",
                                    span { class: "wh-pal-kbd",
                                        for (j, k) in it.kbd.iter().enumerate() {
                                            span { key: "{j}", class: "wh-kbd", "{k}" }
                                        }
                                    }
                                }
                            }
                        }
                        if !workflow_items.is_empty() {
                            div { class: "wh-pal-section", "Workflows" }
                            for (i, it) in workflow_items.iter().enumerate() {
                                div {
                                    key: "{it.cmd}",
                                    class: "wh-pal-row",
                                    class: if (slash_items.len() + i) == selected() { "is-active" },
                                    onclick: {
                                        let item = it.clone();
                                        move |_| on_pick.call(item.clone())
                                    },
                                    span { class: "wh-pal-glyph", "▸" }
                                    span { style: "color: var(--fg-strong);", "{it.label}" }
                                    span { style: "color: var(--fg-dim);", "{it.cmd}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    ```

    Critical (per PATTERNS risks):
    - `tabindex: "-1"` on the overlay div is required so it can receive `onkeydown` focus events. Without it, ↑/↓/Enter wouldn't fire.
    - `autofocus: true` on the search input ensures keyboard events route to the palette when it opens (per UI-SPEC focus expectation).
    - `selected` index wraps via `(s + 1) % len` and `(s + len - 1) % len` per D-18.
    - PersonalityPick substate maps `Personality::ALL` (Wave 1 const) to PaletteItem rows for visual consistency — the row's `cmd` is `/concise`, `/technical`, etc., so the WarpHermes pick handler (Plan 04-05) can match on `cmd.starts_with('/')` and `label()` lookup.
    - The `is_active` class follows `selected()` index against the rendered order (slash items first, then workflow items). The index arithmetic `(slash_items.len() + i)` keeps the highlight aligned across both sections.
    - `KeyboardEvent` here is `dioxus::events::KeyboardEvent` (re-exported via prelude), NOT `web_sys::KeyboardEvent` — these are different types per PATTERNS risk note.
    - The `use_effect` resetting `selected` to 0 on query change reads `query` to register the dependency; `let _ = query.read();` makes that explicit.
    - Esc handling is NOT here — global keydown in WarpHermes (Plan 04-05) handles Esc per D-17.
  </action>
  <verify>
    <automated>grep -q 'pub fn CommandPalette(' src/components/shell/command_palette.rs && grep -q 'query: Signal<String>,' src/components/shell/command_palette.rs && grep -q 'open: ReadOnlySignal<bool>,' src/components/shell/command_palette.rs && grep -q 'state: Signal<PaletteState>,' src/components/shell/command_palette.rs && grep -q 'on_pick: EventHandler<PaletteItem>,' src/components/shell/command_palette.rs && grep -q 'let mut selected = use_signal(|| 0_usize)' src/components/shell/command_palette.rs && grep -q 'PaletteState::PersonalityPick' src/components/shell/command_palette.rs && grep -q 'Personality::ALL' src/components/shell/command_palette.rs && grep -q 'to_lowercase().contains' src/components/shell/command_palette.rs && grep -q 'Key::ArrowDown' src/components/shell/command_palette.rs && grep -q 'Key::ArrowUp' src/components/shell/command_palette.rs && grep -q 'Key::Enter' src/components/shell/command_palette.rs && grep -q 'tabindex: "-1"' src/components/shell/command_palette.rs && grep -q 'oninput: move |e| query.set(e.value())' src/components/shell/command_palette.rs</automated>
  </verify>
  <done>
    command_palette.rs accepts controlled query + open ReadOnlySignal + state Signal + on_pick EventHandler; ↑/↓ wraps; Enter dispatches; PersonalityPick substate enumerates Personality::ALL; live filter on cmd+label substring; selected resets on query change.
  </done>
</task>

<task type="auto">
  <name>Task 3: Refactor status_bar.rs (read-only signal props + personality pill via use_context)</name>
  <files>src/components/shell/status_bar.rs</files>
  <read_first>
    - src/components/shell/status_bar.rs (current — preserve five-pill order, `wh-pill` styling, scanner placement)
    - src/state.rs (TokenBudget Copy; ShellSettings + Personality::label)
    - .planning/phases/04-data-layer-interactions/04-CONTEXT.md §"Personality Switching UX" (D-22 NEW personality pill, read-only in Phase 4) + §"State Ownership Topology" (D-02 use_context_provider)
    - .planning/phases/04-data-layer-interactions/04-RESEARCH.md §"Pattern 5: ShellSettings via use_context_provider"
    - .planning/phases/04-data-layer-interactions/04-PATTERNS.md §"MOD src/components/shell/status_bar.rs"
  </read_first>
  <action>
    REPLACE the entirety of `src/components/shell/status_bar.rs` with EXACTLY:

    ```rust
    use dioxus::prelude::*;
    use crate::state::{ShellSettings, TokenBudget};
    use super::scanner::Scanner;

    /// Status bar — bottom of terminal column. Five `.wh-pill` spans
    /// (mode/model/provider/tokens/personality) separated by `.wh-sep`
    /// middots, plus the Scanner cells and right-aligned `.wh-hint`.
    ///
    /// Phase 4 (per CONTEXT D-22): adds a NEW personality pill displaying
    /// `settings.personality.label()` between the tokens pill and the
    /// scanner. Read-only in Phase 4 (no click handler — TweaksPanel in
    /// Phase 5 wires the click).
    ///
    /// Phase 4 (per CONTEXT D-01): `tokens` and `scanner_active` are now
    /// `ReadOnlySignal<T>` so writes in WarpHermes (token pulse + scanner
    /// pulse) trigger re-render of just this component. Other props stay
    /// `String` (statics from WarpHermes).
    #[component]
    pub fn StatusBar(
        mode: String,
        model: String,
        provider: String,
        tokens: ReadOnlySignal<TokenBudget>,
        scanner_active: ReadOnlySignal<bool>,
        hint: String,
    ) -> Element {
        let t = tokens();
        let used_k = t.used as f32 / 1000.0;
        let max_k = t.max / 1000;
        let pct = ((t.used as f32 / t.max as f32) * 100.0).round() as u32;

        let settings = use_context::<ShellSettings>();
        let pers_label = settings.personality.read().label();

        rsx! {
            div { class: "wh-status",
                span { class: "wh-pill", style: "color: var(--pill-0);", "{mode}" }
                span { class: "wh-sep", "·" }
                span { class: "wh-pill", style: "color: var(--pill-1);", "{model}" }
                span { class: "wh-sep", "·" }
                span { class: "wh-pill", style: "color: var(--pill-2);", "{provider}" }
                span { class: "wh-sep", "·" }
                span { class: "wh-pill", style: "color: var(--pill-3);", "{used_k:.1}K/{max_k}K ({pct}%)" }
                span { class: "wh-sep", "·" }
                span { class: "wh-pill", style: "color: var(--pill-4);", "/{pers_label}" }
                span { class: "wh-sep", "·" }
                Scanner { active: scanner_active() }
                span { class: "wh-hint", "{hint}" }
            }
        }
    }
    ```

    Critical (per PATTERNS risks):
    - `tokens()` (call-as-fn) on `ReadOnlySignal<TokenBudget>` returns owned TokenBudget because TokenBudget derives `Copy` (state.rs line 159). Safe — no read borrow held.
    - `use_context::<ShellSettings>()` PANICS at runtime if WarpHermes (Plan 04-05) forgot to install the provider. This is acceptable as a development-time error per Pattern 5 — the panic message points immediately at the missing provider.
    - The personality pill uses `style: "color: var(--pill-4);"` (the 5th rotation slot) so it picks up the existing pill rotation defined in design-tokens.css. If the prototype HTML uses a different pill index for personality during UAT, swap `--pill-4` for the matching token; don't add new CSS.
    - The pill order MUST be mode → model → provider → tokens → personality → scanner → hint. Personality goes BEFORE scanner so the right-aligned hint stays at the trailing position.
    - The new pill string is `"/{pers_label}"` — the leading slash mirrors how the agent panel prefixes the personality (`/default`).
  </action>
  <verify>
    <automated>grep -q 'pub fn StatusBar(' src/components/shell/status_bar.rs && grep -q 'tokens: ReadOnlySignal<TokenBudget>,' src/components/shell/status_bar.rs && grep -q 'scanner_active: ReadOnlySignal<bool>,' src/components/shell/status_bar.rs && grep -q 'use_context::<ShellSettings>()' src/components/shell/status_bar.rs && grep -q 'settings.personality.read().label()' src/components/shell/status_bar.rs && grep -q '"/{pers_label}"' src/components/shell/status_bar.rs && grep -q 'Scanner { active: scanner_active() }' src/components/shell/status_bar.rs && grep -q 'use crate::state::{ShellSettings, TokenBudget}' src/components/shell/status_bar.rs</automated>
  </verify>
  <done>
    status_bar.rs accepts ReadOnlySignal<TokenBudget> + ReadOnlySignal<bool> + use_context::<ShellSettings>(); personality pill rendered as "/{label}"; pill order preserved; Scanner gets scanner_active().
  </done>
</task>

<task type="auto">
  <name>Task 4: Refactor agent_panel.rs (ReadOnlySignal<Vec<Message>> + drop personality prop, read via use_context)</name>
  <files>src/components/shell/agent_panel.rs</files>
  <read_first>
    - src/components/shell/agent_panel.rs (current — preserve aside.wh-side structure, sigil placement, message rendering)
    - src/state.rs (Message, ShellSettings, Personality::label)
    - .planning/phases/04-data-layer-interactions/04-CONTEXT.md §"State Ownership Topology" (D-02 ShellSettings via use_context) + §"Personality Switching UX" (KBD-06 reactive consumer re-render on personality write)
    - .planning/phases/04-data-layer-interactions/04-PATTERNS.md §"MOD src/components/shell/agent_panel.rs"
  </read_first>
  <action>
    REPLACE the entirety of `src/components/shell/agent_panel.rs` with EXACTLY:

    ```rust
    use dioxus::prelude::*;
    use crate::state::{Message, ShellSettings};
    use super::sigil::Sigil;
    use super::tool_call::ToolCall;

    /// AgentPanel — right-side `.wh-side` agent panel: Sigil + HERMES title +
    /// personality pill + scrollable message list.
    ///
    /// Phase 4 (per CONTEXT D-02): the `personality: String` prop is REMOVED.
    /// Personality is now read via `use_context::<ShellSettings>()` so any
    /// `/personality` palette pick (Plan 04-05) reactively re-renders this panel
    /// without parent prop-drilling (KBD-06 reactivity).
    ///
    /// Phase 4 (per CONTEXT D-01): `messages` is now `ReadOnlySignal<Vec<Message>>`
    /// so writes in `mocks::run_agent_steps` (Plan 04-03) trigger re-render here.
    ///
    /// `aside` semantics + 360px width preserved from Phase 3. `cursor: default`
    /// on the personality pill kept (Phase 4 doesn't add a click handler;
    /// TweaksPanel in Phase 5 will).
    #[component]
    pub fn AgentPanel(messages: ReadOnlySignal<Vec<Message>>) -> Element {
        let settings = use_context::<ShellSettings>();
        let personality = settings.personality.read().label();

        rsx! {
            aside { class: "wh-side",
                div { class: "wh-side-head",
                    Sigil { size: 20_u16 }
                    span { class: "wh-side-title", "HERMES" }
                    span {
                        class: "wh-personality",
                        style: "cursor: default;",
                        "/{personality}"
                    }
                }
                div { class: "wh-side-scroll",
                    for (i, m) in messages.read().iter().enumerate() {
                        div {
                            key: "{i}",
                            class: "wh-msg",
                            class: if m.who == "user" { "is-user" } else { "is-hermes" },
                            div { class: "wh-msg-meta",
                                b { if m.who == "user" { "You" } else { "Hermes" } }
                                span { "{m.time}" }
                            }
                            if let Some(tool) = &m.tool {
                                ToolCall {
                                    name: tool.name.clone(),
                                    args_summary: tool.args_summary.clone(),
                                    status: tool.status.clone(),
                                }
                            } else {
                                div { class: "wh-msg-body", "{m.body}" }
                            }
                        }
                    }
                }
            }
        }
    }
    ```

    Critical (per PATTERNS risks):
    - The `personality: String` prop is GONE. WarpHermes (Plan 04-05) must NOT pass it; the Plan 04-05 plan accounts for this.
    - `messages.read().iter().enumerate()` — the read borrow lives only inside the for-loop (no `.await` in this component) — safe.
    - `key: "{i}"` is acceptable here because Message Vec is append-only within a session and the index is stable across re-renders. (Future enhancement: add Message.id like BlockEntry.id; not required for Phase 4.)
    - The Sigil size `20_u16` matches Phase 3 — do NOT change.
    - `use_context::<ShellSettings>()` panics at runtime if WarpHermes forgot the provider — same acceptance as status_bar Task 3.
  </action>
  <verify>
    <automated>grep -q 'pub fn AgentPanel(messages: ReadOnlySignal<Vec<Message>>)' src/components/shell/agent_panel.rs && grep -q 'use_context::<ShellSettings>()' src/components/shell/agent_panel.rs && grep -q 'settings.personality.read().label()' src/components/shell/agent_panel.rs && grep -q '"/{personality}"' src/components/shell/agent_panel.rs && grep -q 'messages.read().iter().enumerate()' src/components/shell/agent_panel.rs && ! grep -q 'personality: String' src/components/shell/agent_panel.rs && grep -q 'use crate::state::{Message, ShellSettings};' src/components/shell/agent_panel.rs</automated>
  </verify>
  <done>
    agent_panel.rs accepts ONLY messages: ReadOnlySignal<Vec<Message>>; personality prop removed; personality read via use_context::<ShellSettings>(); rendering preserved.
  </done>
</task>

<task type="auto">
  <name>Task 5: Three-platform compile gate (Wave 4 exit) + expanded warp_hermes.rs shim</name>
  <files>src/components/warp_hermes.rs</files>
  <read_first>
    - src/components/warp_hermes.rs (current — Wave 2 + Plan 04-04a transient shim state)
    - .planning/phases/04-data-layer-interactions/04-CONTEXT.md §"Three-Platform Compile Gate" (D-35)
  </read_first>
  <action>
    # This task makes a transient shim edit to warp_hermes.rs; Plan 04-05 will REPLACE the file entirely.

    Wave 2's warp_hermes.rs shim from Plan 04-03 Task 5 plus Plan 04-04a Task 3's BlockStream-shim do not satisfy the Plan 04-04b Tasks 1-4 prop signatures (InputBox now requires Signal<String> + on_submit; CommandPalette now requires Signal<PaletteState>; etc.). EXPAND the warp_hermes.rs shim into a slightly-larger temporary patch that satisfies ALL the new prop signatures from BOTH 04-04a and 04-04b without doing the full integration.

    Add hardcoded use_signal-backed values for the new prop types so the call sites type-check. Example shape (do NOT make this the final WarpHermes — Plan 04-05 replaces it):

    ```rust
    use dioxus::prelude::*;
    use crate::components::shell::{
        AgentPanel, BlockStream, CommandPalette, InputBox, StatusBar, TitleBar,
    };
    use crate::state::{
        demo_block_entries, demo_messages, demo_palette_items, demo_tabs,
        Mode, PaletteItem, PaletteState, Personality, ShellSettings, TokenBudget,
    };

    #[component]
    pub fn WarpHermes() -> Element {
        let blocks = use_signal(demo_block_entries);
        let messages = use_signal(demo_messages);
        let palette_items = demo_palette_items();
        let tabs = demo_tabs();
        let tokens = use_signal(|| TokenBudget { used: 12_300, max: 128_000 });
        let scanner_active = use_signal(|| true);
        let input = use_signal(String::new);
        let mode = use_signal(|| Mode::Shell);
        let focused = use_signal(|| true);
        let pal_open = use_signal(|| true);
        let pal_query = use_signal(String::new);
        let pal_state = use_signal(|| PaletteState::Browse);

        let personality = use_signal(|| Personality::Default);
        use_context_provider(|| ShellSettings { personality });

        rsx! {
            div {
                class: "wh-app",
                "data-theme": "cyan",
                "data-density": "comfy",
                "data-block": "framed",
                "data-agent": "right",
                TitleBar { tabs: tabs, active_tab: 0_usize, show_traffic_lights: true }
                div { class: "wh-main",
                    div { class: "wh-col",
                        BlockStream { blocks: blocks.into(), on_rerun: move |_id: u64| {} }
                        InputBox {
                            value: input,
                            mode: mode.into(),
                            focused: focused,
                            on_submit: move |_| {},
                        }
                        StatusBar {
                            mode: "Chat".to_string(),
                            model: "claude-sonnet-4".to_string(),
                            provider: "anthropic".to_string(),
                            tokens: tokens.into(),
                            scanner_active: scanner_active.into(),
                            hint: "/help · ⌃C cancel · ⌘K palette".to_string(),
                        }
                    }
                    AgentPanel { messages: messages.into() }
                }
                CommandPalette {
                    items: palette_items,
                    query: pal_query,
                    open: pal_open.into(),
                    state: pal_state,
                    on_pick: move |_item: PaletteItem| {},
                }
            }
        }
    }
    ```

    Document this expanded shim in SUMMARY.md as the "Wave 4 expanded WarpHermes shim" — it provides minimal reactive plumbing for the gate to pass but ZERO behavioral wiring (every closure is a no-op, no submit logic, no global keydown, no auto-scroll, no pulse_scanner). Plan 04-05 (Wave 5) replaces this entirely with the integration body per CONTEXT Patterns 1-5 + Common Ops 1-4.

    Then run, in sequence:

    ```bash
    cargo build --features web --no-default-features
    cargo build --features desktop --no-default-features
    cargo build --features mobile --no-default-features
    cargo clippy --features web --no-default-features -- -D warnings
    ```

    Each MUST exit 0. The expanded shim contains no `.await`, no signal-borrow-across-await landmines, so clippy stays clean.

    If a primitive (e.g., command_palette.rs) fails to compile because of a typo in the prop signature or a Key::* import error, fix the offending Wave 4 task (in 04-04a or 04-04b) and re-run the gate. Do NOT silence with `#[allow]` attributes.
  </action>
  <verify>
    <automated>cargo build --features web --no-default-features 2>&1 | tail -5 && cargo build --features desktop --no-default-features 2>&1 | tail -5 && cargo build --features mobile --no-default-features 2>&1 | tail -5 && cargo clippy --features web --no-default-features -- -D warnings 2>&1 | tail -10</automated>
  </verify>
  <done>
    Three-platform `cargo build` and `cargo clippy --features web -- -D warnings` all exit 0 against the expanded warp_hermes.rs shim. Plan 04-05 (Wave 5) replaces the shim with the integration body.
  </done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| Palette query input → filter logic | command_palette.rs Task 2 lower-cases query and substring-matches against constant `PaletteItem.cmd`/`label` strings. No regex eval, no untrusted-data injection. |
| Personality pill render → user display | status_bar / agent_panel render `personality.label()` (a `&'static str` from a const enum-arm match) — no user-controlled string interpolation. |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-04-03 | Tampering | command_palette.rs Task 2 (substring filter) | mitigate | `to_lowercase().contains(&q)` is a pure-string operation — no regex engine, no eval, no path injection. Palette items are const developer-authored data; no untrusted source. |
| T-04-NA-01 | n/a | input_box.rs / status_bar.rs / agent_panel.rs | n/a | Pure prop refactor + use_context lookup; no untrusted input, no async, no async-borrow concerns. Clippy gate Task 5 catches any incidental signal-borrow regressions. |

</threat_model>

<verification>
- input_box.rs accepts (value: Signal<String>, mode: ReadOnlySignal<Mode>, focused: Signal<bool>, on_submit: EventHandler<()>); controlled textarea; Enter (no shift) submits + prevents default.
- command_palette.rs accepts (items, query: Signal<String>, open: ReadOnlySignal<bool>, state: Signal<PaletteState>, on_pick: EventHandler<PaletteItem>); ↑/↓/Enter local nav; live filter; PersonalityPick substate.
- status_bar.rs accepts read-only signal props + use_context::<ShellSettings>() personality pill.
- agent_panel.rs accepts messages: ReadOnlySignal<Vec<Message>>; personality removed from props, read via use_context.
- warp_hermes.rs expanded shim covers ALL Wave 4 prop signatures (both 04-04a and 04-04b).
- Three-platform `cargo build` exits 0 against the expanded warp_hermes.rs shim.
- `cargo clippy --features web -- -D warnings` exits 0 (no `.await` in this wave; no clippy regressions from prop-shape changes).
- The expanded WarpHermes shim is documented in SUMMARY.md and Plan 04-05 (Wave 5) is responsible for replacing it.
</verification>

<success_criteria>
Plan 04-04b complete when:
- [ ] input_box.rs accepts (value: Signal<String>, mode: ReadOnlySignal<Mode>, focused: Signal<bool>, on_submit: EventHandler<()>); controlled textarea; Enter (no shift) submits + prevents default; focus signal wired (KBD-04).
- [ ] command_palette.rs accepts (items, query: Signal<String>, open: ReadOnlySignal<bool>, state: Signal<PaletteState>, on_pick: EventHandler<PaletteItem>); ↑/↓/Enter local nav; live filter; PersonalityPick substate enumerates Personality::ALL (KBD-03, D-18, D-20, D-32).
- [ ] status_bar.rs accepts (mode, model, provider, tokens: ReadOnlySignal<TokenBudget>, scanner_active: ReadOnlySignal<bool>, hint); personality pill via use_context (D-22).
- [ ] agent_panel.rs accepts (messages: ReadOnlySignal<Vec<Message>>); personality removed from props, read via use_context (D-02, KBD-06).
- [ ] warp_hermes.rs expanded shim updated for all four refactored primitives + Plan 04-04a's BlockStream shape.
- [ ] Three-platform `cargo build` green AND `cargo clippy --features web -- -D warnings` green against expanded WarpHermes shim.
- [ ] Expanded shim documented in SUMMARY.md as Plan 04-05 replacement target.
</success_criteria>

<output>
After completion, create `.planning/phases/04-data-layer-interactions/04-04b-SUMMARY.md`
including a "Wave 4 Expanded WarpHermes Shim" section noting the no-op closures
+ all-Signal placeholder body (Plan 04-05 replaces with integration). Also note
the parallel coordination with Plan 04-04a (BlockStream / block / markdown half).
</output>
