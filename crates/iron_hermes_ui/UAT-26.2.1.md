# Phase 26.2.1 Manual UAT

**Phase:** 26.2.1 — new-web-ui-with-wheel-menu
**Audience:** developer running through end-to-end verification before sign-off
**Prerequisite:** `cargo check -p iron_hermes_ui` exits 0 AND `cargo test -p iron_hermes_ui --bin iron_hermes_ui` exits 0 AND `cargo test -p iron_hermes_ui --test wave0_smoke` exits 0 AND `cargo clippy -p iron_hermes_ui --all-features -- -D warnings` exits 0 AND `cargo check -p iron_hermes_ui --features legacy-shell` exits 0

**How to run:**
1. Start the server-side binary with `cargo run -p iron_hermes_ui --bin iron_hermes_ui --features server`.
2. In a second terminal, start the WASM dev server with `dx serve --package iron_hermes_ui`.
3. Open `http://localhost:8080/` in a Chromium-based browser (DevTools open for the localStorage + network checks).
4. Walk through every section below in order; tick each box as you confirm the behavior.
5. After every section is green, return to the planning workflow and run `/gsd-verify-work`.

---

## 1. Page Load & HUD Chrome

- [ ] 1.1 Page loads without console errors (DevTools → Console — no red errors).
- [ ] 1.2 HUD corners render in all four corners (TL/TR/BL/BR).
- [ ] 1.3 Scanlines overlay visible (animated horizontal lines).
- [ ] 1.4 Hud-grid background visible (faint grid texture).
- [ ] 1.5 Hud-vignette darkening at the edges.
- [ ] 1.6 Scan-bar animates across the screen (horizontal sweep).
- [ ] 1.7 Breadcrumb at top-left reads `NODE HERMES-7 › BRIDGE › CHAT` (rightmost crumb tracks the active screen).
- [ ] 1.8 SysMeta at top-right shows BUILD / UPTIME / OP rows.
- [ ] 1.9 App-footer at bottom-left shows NODE / SCREEN / AGENT fields; right shows MEM / SKILLS / P50 / a UTC clock.
- [ ] 1.10 Wait 2 seconds. Confirm the footer clock has ticked at least once.

## 2. Wheel — Rendering & Hover

- [ ] 2.1 Wheel SVG renders in the upper-left of the viewport.
- [ ] 2.2 Count the wedges visually — there are exactly 10 (CONTEXT D-10).
- [ ] 2.3 Wedge labels read (D-10 order from index 0): CHAT, AGENTS, MODELS, TOOLS, SKILLS, MEMORY, SESSIONS, PROVIDER, GATEWAY, SYSTEM.
- [ ] 2.4 Wedge glyphs render (▓ ◆ ◇ ◈ ✦ ⬢ ▣ ◉ ⌬ ⚙).
- [ ] 2.5 Hub center shows the currently-active wedge's label (CHAT by default), sub-label (INTELLIGENCE CONSOLE per wheel-v2.js DEFAULT_SECTIONS), and `▸ LAUNCH` cta.
- [ ] 2.6 Hover each wedge in turn — confirm the `is-active` class toggles (visual highlight) and the hub label updates to the hovered wedge.
- [ ] 2.7 Move cursor outside the wheel — the rim tooltip disappears.

## 3. Wheel — Click & Navigate

- [ ] 3.1 Click each of the 10 wedges in turn. After each click, confirm:
  - The matching `<section id="screen-XXX">` gets `class="screen is-active"` (DevTools → Elements).
  - The footer's rightmost SCREEN field updates.
  - The breadcrumb's rightmost crumb updates.
- [ ] 3.2 Click the hub (center circle). Confirm it launches the currently-hovered wedge.
- [ ] 3.3 Return to the Chat wedge (Screen::Chat).

## 4. Wheel — Drag & Resize

- [ ] 4.1 Pointerdown on the wheel's rim annulus (the invisible band between R_INNER and R_OUTER). Drag the wheel to each of the four viewport corners. Confirm the wheel follows the cursor smoothly without snap-back. (VALIDATION.md Manual-Only #1.)
- [ ] 4.2 Drag the wheel close to the right edge. Confirm DRAG_MARGIN (33px) keeps the wheel and its resize ring inside the viewport.
- [ ] 4.3 Pointerdown on the orange resize ring (the floating ring outside the rim at the south-east). Drag outward → confirm `--wheel-size` CSS variable increases up to 640. Drag inward → confirm it decreases down to 240. (VALIDATION.md Manual-Only #2.)
- [ ] 4.4 Move the wheel to bottom-right and resize to ~320px. Hard-reload the page (Cmd-Shift-R / Ctrl-Shift-R). Confirm position + size persist. (VALIDATION.md Manual-Only #5 partial.)

## 5. Themes & Tweaks Panel

- [ ] 5.1 Click the gear FAB at bottom-right. Confirm the tweaks panel slides up.
- [ ] 5.2 Cycle through all 5 themes in the THEME row: slate-dark, slate-light, iron-dark, terminal-dark, parchment-light. After each click, confirm:
  - The `<html data-theme="…">` attribute updates (DevTools → Elements).
  - The visual palette flips (background + foreground colors change).
  (VALIDATION.md Manual-Only #4.)
- [ ] 5.3 Pick each AccentColor in the ACCENT row: TEAL, ORANGE, GREEN, VIOLET, AMBER. Confirm the `--teal` / `--teal-bright` CSS variables on `<html>` change (DevTools → Elements → Styles).
- [ ] 5.4 Move the WHEEL slider in the tweaks panel. Confirm BOTH the wheel SVG size changes AND the slider's value persists across reload.
- [ ] 5.5 Toggle SCANLINES OFF → confirm the scanlines overlay disappears AND `body.no-scanlines` appears.
- [ ] 5.6 Toggle BREADCRUMB OFF → confirm the top-left breadcrumb chrome hides AND `body.no-breadcrumb` appears.
- [ ] 5.7 Toggle FOOTER OFF → confirm the bottom strip hides AND `body.no-footer` appears.
- [ ] 5.8 Toggle RAIL ON → confirm `body.has-rail` appears AND the wheel-rail panel becomes visible below the wheel.
- [ ] 5.9 Set density DENSE → confirm `body.density-dense` appears AND padding tokens shrink.
- [ ] 5.10 Close the tweaks panel.

## 6. Persistence (localStorage)

- [ ] 6.1 With the changes from §5 still applied, open DevTools → Application → localStorage → `http://localhost:8080`. Confirm three keys: `ih.ui.tweaks`, `ih.ui.theme`, `ih.ui.wheel`.
- [ ] 6.2 Hard-reload the page (Cmd-Shift-R / Ctrl-Shift-R). Confirm every tweak from §5 persists.
- [ ] 6.3 In DevTools → Application → localStorage, edit `ih.ui.theme` to `iron-dark` directly. Reload. Confirm the theme picks up the manual edit. (VALIDATION.md Manual-Only #5.)
- [ ] 6.4 In DevTools → Application → localStorage, set `ih.ui.tweaks` to invalid JSON like `{not-json}`. Reload. Confirm the app falls back to `UiPrefs::default()` without panic (T-26.2.1-05 mitigation).

## 7. Chat — Round Trip

- [ ] 7.1 Click the CHAT wedge (or reload — Chat is the default).
- [ ] 7.2 In the chat-mini header, confirm the session id short-form is visible (8-char prefix) and the status reads READY.
- [ ] 7.3 Type `hello, what model are you?` in the input pill. Press Enter. Confirm:
  - A user bubble appears immediately.
  - An assistant bubble appears below it, streaming deltas character by character.
  - The chat-mini status flips to STREAMING during deltas, then back to READY on Finished.
- [ ] 7.4 In the input, type `line one`, then press Shift+Enter (newline), then `line two`. Press Enter. Confirm the user bubble contains both lines (newline preserved). (Phase 22.3 D-15 + Plan 06 D-20 contract.)
- [ ] 7.5 Type `/clear`. Press Enter. Confirm chat bubbles clear (server-side CommandRouter handles it). (VALIDATION.md Manual-Only #6.)
- [ ] 7.6 Type `/research weather`. Press Enter. Confirm the assistant bubble streams AND tool-progress rows appear under the bubble during tool execution (D-19).
- [ ] 7.7 In DevTools → Network → WS, find the `/api/ws/chat` connection. Disconnect Wi-Fi (or use DevTools "Offline" toggle). Wait 5 seconds. Reconnect. Confirm the WS reconnects automatically (Phase 26.1 `with_automatic_reconnect`) and the next message goes through. (VALIDATION.md Manual-Only #3.)

## 8. Sessions — Switch & Delete

- [ ] 8.1 Click the SESSIONS wedge.
- [ ] 8.2 Confirm the sessions list renders (assuming the backend has sessions from prior testing — if empty, send a chat message first to create one).
- [ ] 8.3 Click a non-current session row. Confirm the screen switches to Chat AND the chat-mini header shows the new session id short-form.
- [ ] 8.4 Return to SESSIONS. Click a row's "×" close button (NOT the row itself). Confirm:
  - The DELETE request fires in DevTools → Network (look for `delete_session` or equivalent server fn).
  - The row's `evt.stop_propagation()` works — the session is NOT selected on the click.
- [ ] 8.5 Reload the page. Confirm the deleted session is no longer in the list.

## 9. Settings — Read & Write

- [ ] 9.1 Click the SYSTEM wedge (which routes to Screen::Settings).
- [ ] 9.2 Confirm the Runtime block renders four read-only rows: MODEL / PROVIDER / CONTEXT / MEMORY. Values come from `ConfigSummary` (Phase 26.4 widened).
- [ ] 9.3 Each row has the disabled affordance (visually muted, "(read-only — coming soon)" note in the block head).
- [ ] 9.4 Confirm the UI Preferences block renders all controls: 5 theme buttons, 5 accent buttons, wheel size slider, 4 boolean toggles, density two-state.
- [ ] 9.5 Repeat the §5 toggles via the Settings panel (instead of the tweaks panel). Confirm both surfaces show the same state.
- [ ] 9.6 Under "Other Screens", click each of SOUL / SCHEDULES / OFFICE. Confirm each switches `Signal<Screen>` and the corresponding screen activates (D-10 reachability for non-wheel screens).

## 10. Visual Stubs

- [ ] 10.1 Click each of the non-live wedges + the providers wedge: AGENTS, SKILLS, MODELS, MEMORY, TOOLS, GATEWAY, PROVIDERS. Confirm each renders content (NOT the Plan 03 placeholder).
- [ ] 10.2 Via Settings → Other Screens, click SOUL / SCHEDULES / OFFICE. Confirm each renders content too.
- [ ] 10.3 For each stub screen, confirm: data shape matches the prototype (cards / rows / grids), no console errors, navigation back to Chat works.

## 11. D-02 — Backend Untouched

- [ ] 11.1 In a terminal at the repo root, run: `git diff HEAD~9 HEAD -- crates/iron_hermes_ui/src/server/ crates/iron_hermes_ui/src/protocol.rs`. Confirm output is empty across the entire phase. (HEAD~9 is approximately Phase 26.2.1's first commit; adjust the rev range as needed.)

## 12. Build & Clippy

- [ ] 12.1 Run `cargo check -p iron_hermes_ui`. Confirm exit 0.
- [ ] 12.2 Run `cargo check -p iron_hermes_ui --features legacy-shell`. Confirm exit 0.
- [ ] 12.3 Run `cargo clippy -p iron_hermes_ui --all-features -- -D warnings`. Confirm exit 0.
- [ ] 12.4 Run `cargo test -p iron_hermes_ui --bin iron_hermes_ui`. Confirm exit 0.
- [ ] 12.5 Run `cargo test -p iron_hermes_ui --test wave0_smoke`. Confirm exit 0.

## Sign-Off

- [ ] Every section above is green.
- [ ] No untriaged console errors during UAT.
- [ ] All persistence behaviors confirmed across reload.
- [ ] Tester initials + date: ______________

After sign-off, return to the planning workflow and flip `26.2.1-VALIDATION.md`'s frontmatter to `nyquist_compliant: true` and `wave_0_complete: true`, tick every checkbox in the Validation Sign-Off block, and run `/gsd-verify-work`.
