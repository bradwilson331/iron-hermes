# Roadmap: IronHermes

**Milestone:** v1 — Dioxus 0.7 port of the Warp × IronHermes prototype
**Granularity:** Standard (6 phases)
**Coverage:** 38/38 requirements mapped

---

## Phases

- [ ] **Phase 1: Hygiene** - Pin deps, commit Cargo.lock, fix Tailwind config, split src/ into modules, clean .gitignore
- [ ] **Phase 2: Design System** - Port Ioskeley Mono fonts, design tokens, Warp shell CSS, and brand assets into the Dioxus asset pipeline
- [x] **Phase 3: Desktop Shell** - Implement the full WarpHermes desktop/web shell with all block types, input, side panel, status bar, scanner, and command palette (completed 2026-05-03)
- [ ] **Phase 4: Data Layer & Interactions** - Wire the mocked data layer (personality presets, runShell, runAgent, token counter) and all keyboard/click interactions
- [ ] **Phase 5: Theming & Tweaks** - Implement all four data-attribute switches and the TweaksPanel component
- [ ] **Phase 6: Mobile Shell** - Implement WarpHermesMobile with compact title bar, bottom tab bar, and mobile-tuned layout

---

## Phase Details

### Phase 1: Hygiene
**Goal**: The project builds reproducibly, with correct dependency declarations, Tailwind config wired, and src/ organized into a module hierarchy that can hold the full implementation
**Depends on**: Nothing (first phase)
**Requirements**: HYG-01, HYG-02, HYG-03, HYG-04, HYG-05
**Success Criteria** (what must be TRUE):
  1. `cargo build --features web` succeeds from a clean clone without any extra steps
  2. `Cargo.lock` is committed and `dioxus = "=0.7.1"` appears in `Cargo.toml` with explicit `web`, `desktop`, and `mobile` features
  3. `Dioxus.toml` has `tailwind_input` and `tailwind_output` set; `dx serve` generates `assets/tailwind.css` without manual intervention
  4. `src/` contains at least `main.rs`, `app.rs`, `components/mod.rs`, `state.rs`, and `platform/mod.rs` — no single-file monolith
  5. `.gitignore` blocks `**/.DS_Store` recursively and excludes `warp2ironhermes-handoff.zip`
**Plans**: 3 plans
Plans:
**Wave 1**
- [x] 01-01-PLAN.md — Fix Cargo.toml version pin, wire Dioxus.toml Tailwind keys, tighten .gitignore (HYG-01, HYG-03, HYG-05)

**Wave 2** *(blocked on Wave 1 completion)*
- [x] 01-02-PLAN.md — Split src/main.rs into module hierarchy: app.rs, components/, state.rs, platform/ (HYG-04)

**Wave 3** *(blocked on Wave 2 completion)*
- [x] 01-03-PLAN.md — Generate and stage Cargo.lock; run three-platform phase gate (HYG-02)

### Phase 2: Design System
**Goal**: The Dioxus app loads and applies the full IronHermes visual identity — Ioskeley Mono font, ANSI-derived color tokens, Warp shell layout CSS, and brand assets — so all downstream components render against the correct design foundation
**Depends on**: Phase 1
**Requirements**: DS-01, DS-02, DS-03, DS-04
**Success Criteria** (what must be TRUE):
  1. Opening the running app in a browser shows body text rendered in Ioskeley Mono (verified in DevTools computed styles)
  2. CSS custom properties `--accent-primary`, `--brand`, `--font-mono`, and `--w-radius-block` resolve to their correct values when inspected in DevTools
  3. The Warp shell layout classes (`wh-app`, `wh-block`, `wh-status`, etc.) are present in the loaded stylesheet and carry the correct rules from the prototype
  4. The IronHermes wordmark SVG and shield PNG appear in the app instead of the Dioxus scaffold SVG
**Plans**: 4 plans
Plans:
- [x] 02-01-PLAN.md — Copy 16 Ioskeley Mono fonts + verbatim-port colors_and_type.css → assets/design-tokens.css (DS-01, DS-02)
- [x] 02-02-PLAN.md — Verbatim-port warp-ih.css → assets/warp-ih.css (DS-03)
- [x] 02-03-PLAN.md — Copy wordmark.svg + ih-shield.png; delete header.svg (DS-04 file half)
- [⊙] 02-04-PLAN.md — Wire CSS into src/app.rs (cascade-corrected); rewrite hero.rs to brand stub; rewrite main.css; three-platform gate + manual UAT (DS-01..04 wiring half) — auto tasks 1-4 complete; manual UAT (Task 5) blocking
**UI hint**: yes

### Phase 3: Desktop Shell
**Goal**: The WarpHermes desktop/web shell is fully rendered — title bar, scrollable block stream with all five block types, input box with mode glyph, agent side panel, status bar with scanner animation, and command palette overlay — matching the prototype layout pixel-for-pixel
**Depends on**: Phase 2
**Requirements**: SHELL-01, SHELL-02, SHELL-03, SHELL-04, SHELL-05, SHELL-06, SHELL-07, SHELL-08, SHELL-09, SHELL-10
**Success Criteria** (what must be TRUE):
  1. The title bar shows macOS traffic lights, a tab strip, and the `⌘K` shortcut label
  2. The block stream displays all five block types (`is-cmd`, `is-out`, `is-ai`, `is-ok`, `is-err`), each with a distinct color-coded 2px left stripe; hovering a block reveals copy, rerun, and share buttons
  3. A `CommandLine` block renders `bin`, `arg`, and `flag` token spans in distinct colors; a tool-call block renders name, args summary, and status
  4. The input box shows the `❯` (Shell) or `✦` (Agent) mode glyph, expands as text is typed, and displays an accent-color focus ring with glow on focus
  5. The agent side panel is visible on the right at 360px; the status bar shows rotating dot-pills including the knight-rider scanner cells animated via CSS `@keyframes` (not signal-driven)
  6. Pressing `⌘K` opens the command palette overlay showing slash commands and workflow items in a keyboard-navigable list
**Plans**: 5 plans
Plans:
**Wave 1**
- [x] 03-01-PLAN.md — Foundation: state.rs types + 4 demo fixtures; assets/scanner-anim.css; components/mod.rs + shell/mod.rs (SHELL-02, SHELL-04, SHELL-05, SHELL-08, SHELL-09, SHELL-10)

**Wave 2** *(blocked on Wave 1 completion)*
- [x] 03-02-PLAN.md — 5 leaf primitives: sigil.rs, scanner.rs, command_line.rs, tool_call.rs, status_bar.rs (SHELL-04, SHELL-05, SHELL-08, SHELL-09)

**Wave 3** *(blocked on Wave 2 completion)*
- [x] 03-03-PLAN.md — 6 composite primitives: title_bar.rs, block.rs, block_stream.rs, input_box.rs, agent_panel.rs, command_palette.rs (SHELL-01, SHELL-02, SHELL-03, SHELL-04, SHELL-05, SHELL-06, SHELL-07, SHELL-10)

**Wave 4** *(blocked on Wave 3 completion)*
- [x] 03-04-PLAN.md — Composer warp_hermes.rs + app.rs swap + scanner-anim.css link + hero.rs deletion + three-platform compile gate (SHELL-01..SHELL-10)

**Wave 5** *(blocked on Wave 4 completion)*
- [x] 03-05-PLAN.md — Manual UAT side-by-side against prototype HTML (checkpoint:human-verify, blocking) (SHELL-01..SHELL-10)
**UI hint**: yes

### Phase 4: Data Layer & Interactions
**Goal**: The shell is fully interactive — keyboard shortcuts work, the input box submits commands, mock shell and agent responses arrive with prototype-matching timing, personality presets swap the reply set, and the token counter increments per submission
**Depends on**: Phase 3
**Requirements**: MOCK-01, MOCK-02, MOCK-03, MOCK-04, KBD-01, KBD-02, KBD-03, KBD-04, KBD-05, KBD-06
**Success Criteria** (what must be TRUE):
  1. Typing a command and pressing `Enter` appends an `is-cmd` block immediately, followed by a single output block (`is-out` / `is-ok` / `is-err` per command-keyword routing) after ~600 ms — matching the prototype `runShell` (app.jsx 163–174). In Agent mode (⌥M), Enter appends a user message immediately, a hermes tool-call message after ~400 ms, and a hermes reply after ~1400 ms total — matching the prototype `runAgent` (app.jsx 176–185)
  2. Pressing `⌥M` toggles the input mode glyph between `❯` and `✦`; subsequent `Enter` submission routes to `runShell` or `runAgent` accordingly
  3. `⌘K` opens the palette; `Esc` closes it; `↑` / `↓` move the selection highlight; `Enter` selects the active item
  4. `Shift+Enter` inserts a newline into the input box without submitting
  5. Switching the personality preset (concise / technical / noir / hype / catgirl / default) changes which scripted reply appears on the next `runAgent` call
  6. The token count displayed in the status bar increases by a fixed amount after each submission
**Plans**: 6 plans
Plans:
**Wave 1** (foundation: deps + cfg-gated timer + desktop runtime smoke test)
- [x] 04-01-PLAN.md — Cargo.toml deltas (gloo-timers, tokio time-only, web-sys, wasm-bindgen, js-sys); src/platform/timer.rs cfg-gated sleep + register module; three-platform compile gate; desktop runtime smoke test for tokio::time::sleep (D-04, D-05, D-35; resolves RESEARCH Open Question Q1)

**Wave 2** *(blocked on Wave 1)* (state.rs type vocabulary)
- [x] 04-02-PLAN.md — state.rs extensions: Personality enum + label/ALL, BlockEntry wrapper, PaletteState enum, ShellSettings context struct, cfg-gated now_time helper; destructive rename demo_blocks → demo_block_entries (D-02, D-03, D-07, D-09, D-20, D-34)

**Wave 3** *(blocked on Wave 2)* (mocks/ module tree)
- [x] 04-03-PLAN.md — src/mocks/ tree: personalities.rs (REPLIES + pick_reply), shell_outputs.rs (fake_shell_out + STATUS_TEXT), agent_steps.rs (run_agent_steps 3-stage async chain), mod.rs (run_shell + tokenize); main.rs adds mod mocks; transient warp_hermes.rs shim; clippy await-discipline first applies (MOCK-01, MOCK-02, MOCK-03; D-06, D-10, D-11, D-12, D-21, D-28, D-36, D-37)

**Wave 4** *(blocked on Wave 3 — 04-04a and 04-04b run in parallel; touch disjoint files)* (shell primitive prop refactor + markdown helper)
- [x] 04-04a-PLAN.md — BlockEntry-id propagation half: markdown.rs (render_inline_code) + register; refactor block.rs (entry+on_copy/on_rerun+render_inline_code routing for is-ai), block_stream.rs (ReadSignal<Vec<BlockEntry>>+stable id keys+clipboard wasm32); transient warp_hermes.rs shim (KBD-05; D-07, D-15, D-16, D-23, D-24, D-25) — completed 2026-05-03
- [x] 04-04b-PLAN.md — Prop-shape refactor half: input_box.rs (controlled textarea+on_submit+focus signal), command_palette.rs (controlled query+↑/↓/Enter+Browse|PersonalityPick substate+live filter), status_bar.rs (read-only signal props+personality pill via use_context), agent_panel.rs (ReadOnlySignal<Vec<Message>>+drop personality prop); expanded warp_hermes.rs shim + three-platform compile gate (KBD-03, KBD-04; D-02, D-18, D-19, D-20, D-22, D-32, D-35)

**Wave 5** *(blocked on Wave 4 — both 04-04a and 04-04b)* (WarpHermes integration + manual UAT)
- [x] 04-05-PLAN.md — WarpHermes() rewire: 12 use_signals + use_context_provider(ShellSettings) + global keydown use_effect/use_drop with Signal<Option<Closure<_>>> storage + auto-scroll use_effect with triple-guard + pulse_scanner/pulse_token + submit/on_rerun/pick closures + composition; checkpoint:human-verify side-by-side against Warp × IronHermes.html (MOCK-01..04 + KBD-01..06; D-01, D-02, D-13, D-14, D-17, D-26, D-27..D-31, D-33, D-35)
**UI hint**: yes

### Phase 5: Theming & Tweaks
**Goal**: All four data-attribute theme dimensions are switchable at runtime, and the TweaksPanel exposes every switch in one place so the full design system can be exercised end-to-end
**Depends on**: Phase 3 (shell renders), Phase 2 (tokens in place)
**Requirements**: THEME-01, THEME-02, THEME-03, THEME-04, THEME-05
**Success Criteria** (what must be TRUE):
  1. Setting `data-theme` to `cyan`, `magenta`, `green`, or `amber` visibly changes the accent color of the focus ring, block stripes, and active elements throughout the shell
  2. Setting `data-density` to `compact` reduces block padding and spacing noticeably compared to `comfy`
  3. Setting `data-block` to `framed`, `flat`, or `minimal` visibly changes the block border/background treatment
  4. Setting `data-agent` to `right`, `bottom`, or `hidden` repositions or hides the agent side panel accordingly
  5. The TweaksPanel UI renders as an overlay or sidebar with controls for all four attributes; clicking a control immediately updates the shell without a page reload
**Plans**: TBD
**UI hint**: yes

### Phase 6: Mobile Shell
**Goal**: The WarpHermesMobile variant renders correctly on a narrow viewport — compact title bar, bottom tab bar, hidden side panel, and compact density — reusing the block stream and input components from the desktop shell
**Depends on**: Phase 3 (shared block and input components), Phase 4 (interactions apply equally to mobile)
**Requirements**: MOB-01, MOB-02, MOB-03, MOB-04
**Success Criteria** (what must be TRUE):
  1. On a mobile-width viewport (or under the `mobile` feature flag), the title bar renders without traffic lights or a tab strip
  2. A bottom tab bar is visible with three tabs: shell (`❯`), hermes (`✦`), and files (`▤`); tapping each tab switches the active view
  3. The mobile shell starts with `data-agent="hidden"` (no side panel) and `data-density="compact"` applied by default
  4. The block stream and input box render identically to the desktop variants (same components, same block types, same interactions) with only spacing adjusted for mobile density
**Plans**: TBD
**UI hint**: yes

---

## Progress Table

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Hygiene | 3/3 | Ready for verification | - |
| 2. Design System | 4/4 (auto-task work) | Implementation complete; manual UAT pending (Plan 02-04 checkpoint:human-verify) | - |
| 3. Desktop Shell | 5/5 | Complete   | 2026-05-03 |
| 4. Data Layer & Interactions | 3/6 | In Progress|  |
| 5. Theming & Tweaks | 0/0 | Not started | - |
| 6. Mobile Shell | 0/0 | Not started | - |

---

*Roadmap created: 2026-05-02*
*Last updated: 2026-05-03 — Phase 4 plans revised per checker feedback: 6 plans across 5 waves (Wave 4 split into 04-04a [BlockEntry-id propagation half] and 04-04b [prop-shape refactor half] running in parallel because they touch disjoint files; Plan 04-01 gains a desktop runtime smoke test for tokio::time::sleep resolving RESEARCH Open Question Q1; D-11/D-37 explicitly cited in 04-03 must_haves; warp_hermes.rs added to files_modified in plans that mutate it; focused-gate grep added to 04-05 verify; RESEARCH Open Questions marked RESOLVED). Each plan's frontmatter declares its requirements; all 10 KBD-XX/MOCK-XX IDs covered. Plan 04-05 contains the blocking checkpoint:human-verify UAT against Warp × IronHermes.html.*
