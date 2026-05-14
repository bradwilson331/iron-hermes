---
status: diagnosed
trigger: "/clear slash command is intercepted by the server-side CommandRouter and clears the chat buffer (Plan 06 D-20 contract) — but user reports LLM is generating cordial reply instead. find_root_cause_only mode."
created: 2026-05-14
updated: 2026-05-14
---

## Current Focus

reasoning_checkpoint:
  hypothesis: "The chat UI's submit handler in warp_hermes.rs forwards ALL non-empty input verbatim over the WebSocket as a ChatRequest. The server-side ws_chat handler in server/ws.rs has zero slash-command branch and immediately calls app_state.run_web_turn(message), which feeds the message to the AgentLoop / inference. The CommandRouter is constructed in AppState::init (server/state.rs:53) and exposed via api::list_slash_commands() for palette UI population only — it is NEVER invoked on the message path. The client-side /clear branch (warp_hermes.rs:800) ONLY fires when the user picks /clear FROM THE PALETTE (the `pick` callback), not when they type '/clear' + Enter via the InputBox. Therefore typing '/clear' in the input pill bypasses the palette branch entirely, goes straight through `submit`, hits ws_chat, hits the LLM, and the LLM hallucinates the cordial 'Context cleared' reply. Plan 06 D-20's assertion that 'slash commands route server-side via existing CommandRouter' inherited an assumption that was never implemented for the iron_hermes_ui WebSocket path — the existing CommandRouter lives at a completely different layer (ironhermes-gateway/handler.rs handle_slash_command for Telegram) and was never wired into the new web ws_chat handler."
  confirming_evidence:
    - "warp_hermes.rs:697-782 `submit` handler — no slash check, builds ChatRequest from `text` (entire trimmed input) and ws.send_raw's it; identical path for shell and agent modes."
    - "warp_hermes.rs:799-871 `pick` handler (palette item picker) — DOES have `match item.cmd.as_str() { '/clear' => blocks.set(Vec::new()) }`. This is the ONLY client-side /clear interception, and it fires from palette picks (Cmd-K), not from typed input. So /clear-via-palette would work, but /clear-via-Enter does not."
    - "server/ws.rs:53-242 `ws_chat` — parses ChatRequest at line 143, then unconditionally constructs stream/tool callbacks and calls `app_state.run_web_turn(&session_id, &message, ...)` at line 212. No `message.starts_with('/')` check, no router invocation. The CommandRouter is not referenced anywhere in ws.rs."
    - "server/state.rs:53,135-156 — AppState builds a CommandRouter (`Arc::new(CommandRouter::new(build_command_registry()))`) at init but never uses it. `run_web_turn` builds messages, builds AgentLoop, runs inference. There is no slash-dispatch helper on AppState."
    - "Workspace grep for `CommandRouter` shows it is used by: ironhermes-cli (TUI), ironhermes-gateway (Telegram via `handler.rs::handle_slash_command` — handler.rs:686 calls it only when `event.content.starts_with('/')`). NEITHER of those paths is reachable from the new iron_hermes_ui ws_chat handler."
    - "api.rs:37 `list_slash_commands()` is a server function whose ONLY consumer is the palette population (warp_hermes.rs:391). It returns metadata. It is not a dispatch function."
    - "User-supplied symptom — 'takes a while to respond' + cordial LLM phrasing ('I have reset the conversation state... fresh start... How can I assist you now?') — is exactly what you'd see if '/clear' were sent as a user message to a chatty model. The model has no tool/effect to actually clear state; it just generates an answer."
  falsification_test: "If hypothesis is wrong, then either (a) ws.rs contains a slash-prefix branch (it does not — verified end-to-end read of all 343 lines), or (b) AppState::run_web_turn dispatches to CommandRouter before AgentLoop (it does not — verified state.rs lines 135-156: it goes straight to build_messages_for_turn → build_agent_loop → agent.run), or (c) the client's `submit` handler pre-checks for '/' prefix (it does not — verified warp_hermes.rs:697-782)."
  fix_rationale: "Fix MUST live client-side under D-02 (server unchanged in this phase). Confine the change to the `submit` handler in warp_hermes.rs: branch on `text.starts_with('/')` and either (1) execute the same palette-pick semantics inline for the matching command (route through the existing `pick` logic by mapping text → PaletteItem cmd), or (2) explicitly handle /clear as `blocks.set(Vec::new())` and short-circuit before the ChatRequest send. Option (1) is more correct because it covers /status, /help, /personality, /doctor, /quit too — all of which currently suffer the same bug (typing any of them as text forwards to inference). The architectural reason this fix CAN live client-side: every slash command currently implemented for the web UI is purely a UI-state operation (clear blocks, render help/status text, open personality picker, fill the input box). None require server-side state mutation, which means the server's CommandRouter doesn't actually need to be invoked from ws_chat at all in this phase. Doing it client-side honors D-02 (no server edits) and preserves the existing CommandRouter wiring (it remains available via list_slash_commands for the palette). A future phase that adds slash commands which DO need server-side effects (e.g., /compress, /agents) would need to extend ws.rs and add a router-dispatch branch — but that is out of scope for 26.2.1."
  blind_spots:
    - "Have not run the app to dynamically confirm — diagnosis is from static read of submit, pick, ws_chat, state.rs. But the source lines are unambiguous: submit does no slash check, ws.rs forwards verbatim, run_web_turn never touches command_router."
    - "Have not verified the exact UAT Test 8 text (couldn't find 26.2.1-UAT.md at the prompt-specified path) — diagnosis based on user-supplied symptom verbatim, which is sufficient to identify the bug regardless of UAT wording."
    - "Plan 06 SUMMARY also not located at the prompt-specified path — but the user-supplied quote ('Slash commands route server-side via existing CommandRouter (D-20) — no client-side parsing') is conclusive that D-20 contract was asserted-without-verification."
    - "Did NOT inspect every workflow item in PALETTE_ITEMS — there could be additional commands that need client-side interception, but the bug class is the same: anything typed as '/x' bypasses palette and gets sent to inference."

## Symptoms

expected: User types "/clear" + Enter → chat bubbles clear. CommandRouter intercepts the "/" prefix server-side BEFORE any LLM call.
actual: After delay, LLM-style cordial reply appears: "no - it takes a while for the system to respond and this does not appear to be a builtin command *Context cleared.* I have reset the conversation state. I am ready for your next request, taking a completely fresh start! How can I assist you now?"
errors: (none — silent semantic failure, response is generated by inference)
reproduction: Type "/clear" in the chat pill input and press Enter (do NOT use Cmd-K + palette). Wait ~few seconds. Observe LLM-generated reply instead of immediate buffer clear.
started: Phase 26.2.1 Plan 06 (chat WS wiring). The bug was present from Plan 06's merge — typing slash commands has never worked via Enter; only palette picks work.

## Eliminated

- hypothesis: "ws_chat handler intercepts '/' and forwards to a server-side CommandRouter dispatch path"
  evidence: "Full read of server/ws.rs (343 lines, lines 53-242 inclusive of message handling): zero references to 'starts_with', 'CommandRouter', slash, '/clear', router.resolve, or router.dispatch. ChatRequest is parsed at line 143 and passed straight to app_state.run_web_turn at line 212. The CommandRouter built in server/state.rs:53 is never invoked from the ws path."
  timestamp: 2026-05-14
- hypothesis: "AppState::run_web_turn dispatches to CommandRouter before invoking AgentLoop"
  evidence: "server/state.rs:135-156 run_web_turn body: build_messages_for_turn → build_agent_loop → agent.run(messages). No router call, no slash branch."
  timestamp: 2026-05-14
- hypothesis: "The client `submit` handler in warp_hermes.rs pre-parses '/' before sending"
  evidence: "warp_hermes.rs:697-782 `submit` closure: trims input, pushes blocks, sends ChatRequest unconditionally. No slash check. Verified by grep — only `trim_start_matches('/')` reference in the file is in `pick` (line 789), inside the PersonalityPick branch."
  timestamp: 2026-05-14

## Evidence

- timestamp: 2026-05-14
  checked: "grep workspace for CommandRouter / slash_command / handle_slash"
  found: "CommandRouter is used by: (1) ironhermes-cli/src/main.rs:1380 + tui/commands.rs (interactive TUI), (2) ironhermes-gateway/src/handler.rs:178 (Telegram gateway, calls handle_slash_command at lines 687 + 1082 only when event.content.starts_with('/') at handler.rs ~line 685), (3) iron_hermes_ui/src/server/state.rs:53 (BUILT but NEVER DISPATCHED). iron_hermes_ui has no analogue to handler.rs::handle_slash_command."
  implication: "CommandRouter is a real, working abstraction at two layers (CLI and gateway/Telegram) but was never wired into the new web UI's WebSocket handler. Plan 06 D-20 inherited the assumption that the routing existed; it did not."

- timestamp: 2026-05-14
  checked: "Re-read of server/ws.rs in full (343 lines)"
  found: "Single recv_raw arm at line 89-141 parses Text into ChatRequest (line 143). Busy-gate check at lines 162-178. Spawns inference task at line 185 calling app_state.run_web_turn with the message string verbatim. No slash interception anywhere."
  implication: "The 'D-02 forbids server-side edits' constraint is honored — but the constraint is moot for slash interception because slash interception is not on the server at all. The bug is in the client."

- timestamp: 2026-05-14
  checked: "Client submit path in warp_hermes.rs lines 697-782 vs pick path lines 786-872"
  found: "submit() (typed-Enter path): no slash check; calls ws.send_raw(ChatRequest). pick(item) (palette-pick path): line 800 has `'/clear' => blocks.set(Vec::new())`. The /clear logic exists but is unreachable from the typed-input path."
  implication: "Two parallel UX entry points — typed and palette-picked — and only one of them honors slash semantics. The fix is to lift the slash-handling from `pick` into `submit` (or share via a helper), keying off `text.starts_with('/')`."

- timestamp: 2026-05-14
  checked: "Existence of `crates/iron_hermes_ui/src/components/hermes_app/` (the prompt's stated location)"
  found: "Directory does NOT exist. The chat UI lives at `crates/iron_hermes_ui/src/components/warp_hermes.rs` (single file, ~900+ lines). use_websocket at line 135; submit at 697-782; pick (with /clear branch) at 786-872."
  implication: "Prompt's file references (hermes_app/mod.rs, screens/chat.rs) describe a directory layout that doesn't match the current code. This is informational only — the diagnosis is unchanged."

- timestamp: 2026-05-14
  checked: "Gateway analogue: ironhermes-gateway/src/handler.rs"
  found: "handler.rs:686 has `if event.content.starts_with('/') { return self.handle_slash_command(...) }` BEFORE run_agent. handle_slash_command at line 338 builds a CommandContext and dispatches via self.command_router."
  implication: "This is the canonical pattern for slash interception. The iron_hermes_ui ws_chat handler has no equivalent. A future phase that wants web-UI slash commands with server-side effects would replicate this branch in ws.rs at the post-parse point (after line 157)."

- timestamp: 2026-05-14
  checked: "api.rs list_slash_commands consumer"
  found: "Only consumer is warp_hermes.rs:391 — populates the palette. The fetched list is metadata, not a dispatch interface."
  implication: "list_slash_commands does not implement /clear; it only enumerates available commands for UI rendering. Calling it from the client gives you a list, not an action."

## Resolution

root_cause: |
  Slash commands typed into the chat InputBox (Enter-submitted) are forwarded verbatim to the inference WebSocket because the client's `submit` handler at warp_hermes.rs:697-782 has no slash-prefix branch, and the server-side ws_chat handler at server/ws.rs:53-343 has no slash-interception branch either. The CommandRouter built in server/state.rs:53 is never dispatched on the message path — it exists only to populate the palette via list_slash_commands. Plan 06 D-20's contract ("slash commands route server-side via existing CommandRouter") was an inherited assumption that was never implemented for the iron_hermes_ui WebSocket path. The /clear logic DOES exist client-side, but only inside the `pick` callback at warp_hermes.rs:800, which fires from palette item picks (Cmd-K + click) — not from typed-Enter input. Hence: palette-pick /clear works, typed /clear hits the LLM, and the LLM hallucinates "Context cleared. I have reset the conversation state…"
fix: |
  (Fix direction only — not applied per find_root_cause_only mode.)
  Confine the fix to the CLIENT in warp_hermes.rs to honor D-02 (server unchanged in 26.2.1).
  Modify the `submit` closure (warp_hermes.rs:697-782): immediately after trimming the input text and before any block-stream push or ws.send_raw, branch:

    if text.starts_with('/') {
        // Route to the same handler the palette uses.
        let item = palette_items.iter().find(|p| p.cmd == text).cloned()
            .unwrap_or_else(|| PaletteItem { cmd: text.clone(), section: "slash".into(), label: text.clone() });
        pick(item);   // (or inline /clear + /status + /help branches with shared helper)
        input.set(String::new());
        return;
    }

  Concretely: extract the body of `pick`'s `match item.cmd.as_str()` arms into a `dispatch_slash(cmd: &str)` helper closure that both `submit` and `pick` call. /clear → blocks.set(Vec::new()). /status, /help, /personality, /doctor, /quit follow the same pattern that already works in `pick`.

  RATIONALE: D-02 forbids server changes, which is fine — every slash command currently exposed by the web UI is a pure client-side UI-state operation (clear blocks, append help/status block, open personality submenu, fill input). None requires server-side effects. So the routing genuinely can stay client-side for 26.2.1.

  A future phase that adds server-effecting slash commands (e.g., /compress, /agents start) would extend ws.rs with an interception branch at the post-ChatRequest-parse point (after server/ws.rs:157), invoking app_state.command_router.dispatch(...) and returning a ChatStreamEvent::Delta (or new Finished variant) instead of running the agent loop. That belongs to a follow-up phase, not 26.2.1.
verification: (not applied — find_root_cause_only)
files_changed: []
