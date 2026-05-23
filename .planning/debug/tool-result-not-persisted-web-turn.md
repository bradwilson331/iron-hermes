---
status: resolved
trigger: "LLM call failures after initial success: orphan tool_call_id at messages[6] — a new assistant message arrived before tool messages answered the prior assistant block"
created: 2026-05-05
updated: 2026-05-05
---

## Symptoms

- Expected: conversation continues across turns with tool results properly paired
- Actual: `validate_tool_call_pairing` fails on every retry (3x) with orphan tool_call_id at messages[6]
- Error: `orphan tool_call_id 'call_7a62b843ad434c55b02f5719' (and 0 other(s)) at messages[6]: a new assistant message arrived before tool messages answered the prior assistant block`
- Timing: first call works; failure happens on subsequent turns after any turn that contained tool calls

## Current Focus

hypothesis: "run_web_turn in server/state.rs persists only Role::Assistant messages (not Role::Tool), AND iterates result.messages (full history) instead of result.appended, producing duplicate assistant messages and missing tool results in the stored history"
test: "Fix persistence loop to use result.appended and persist all messages (no role filter); verify validate_tool_call_pairing passes on turn 2+"
expecting: "No orphan tool_call_id errors; message history reconstructed correctly across turns"
next_action: "Apply fix to crates/iron_hermes_ui/src/server/state.rs lines 150-155"
reasoning_checkpoint: "Root cause confirmed by reading all relevant code"

## Evidence

- timestamp: 2026-05-05T03:54:51Z
  file: crates/iron_hermes_ui/src/server/state.rs:150-155
  finding: |
    ```rust
    for msg in &result.messages {
        if msg.role == Role::Assistant {
            let _ = store.add_message(session_id, msg);
        }
    }
    ```
    Two bugs: (1) Role::Tool messages never persisted. (2) result.messages is full history — re-persists ALL prior assistant messages as duplicates on every turn.

- timestamp: 2026-05-05T03:54:51Z
  file: crates/ironhermes-agent/src/agent_loop.rs:718-724
  finding: "Comment explicitly calls this out: 'track messages appended by THIS run so the gateway/REPL persistence path can include matching tool results without a Role-based filter (which would drop Role::Tool)'. result.appended is the intended API."

- timestamp: 2026-05-05T03:54:51Z
  file: crates/ironhermes-core/src/types.rs:419-460
  finding: "validate_tool_call_pairing strict semantics: new Assistant message while pending tool_call_ids → error at that index. Reconstructed messages[6] = asst_final_turn2 while tool calls from messages[5] = asst_with_tools_turn2 are still pending."

- timestamp: 2026-05-05T03:54:51Z
  file: crates/ironhermes-agent/src/agent_loop.rs:898-940
  finding: "appended vec is populated at line 899 (assistant) and line 940 (tool results). System pressure advisories are NOT in appended. This is exactly what should be persisted."

## Eliminated

- hypothesis: "context compression stripping tool results"
  reason: "compression runs pre-chat on the in-memory messages vec, not on the persistence layer. The error is caught before the LLM call, not after."

- hypothesis: "execute_tool_call failing to push results"
  reason: "execute_tool_call is infallible (returns String). Tool results ARE in result.messages and result.appended — they're just never persisted."

## Resolution

root_cause: "run_web_turn persists only Role::Assistant from result.messages, dropping Role::Tool messages and duplicating prior-turn assistant messages. Reconstructed history on subsequent turns has assistant+tools → assistant_final (no tool result in between) which triggers the orphan invariant."
fix: "Replace result.messages + role filter with result.appended (no filter). result.appended contains exactly this-run assistant and tool messages, excludes prior history and system advisories."
files_changed: ["crates/iron_hermes_ui/src/server/state.rs"]
