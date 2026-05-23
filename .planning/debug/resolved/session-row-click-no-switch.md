---
status: diagnosed
trigger: "ScreenSessions: clicking a non-current session row shows different sessions but all reference the current session"
created: 2026-05-14T00:00:00Z
updated: 2026-05-14T00:00:00Z
---

## Current Focus

hypothesis: ROOT CAUSE FOUND — the chat-mini header in `screens/chat.rs` displays `sid_full.chars().take(8)`, and every session id created/listed by the server is formatted `"agent:main:web:dm:<uuid>"`. The first 8 chars are always literally `"agent:ma"`, so the header looks identical for every session even though `SessionIdContext` is being written correctly.
test: Trace what ScreenChat actually displays in the SID code element vs what `on_select` writes
expecting: SET path writes per-row id correctly; GET path truncates to a non-distinguishing prefix
next_action: Return ROOT CAUSE FOUND with evidence

## Symptoms

expected: SESSIONS wedge -> list renders >1 row -> click a non-current row -> Chat opens with the clicked session's id in the chat-mini header
actual: List shows multiple session rows, but clicking any non-current row results in chat-mini header still showing the previously-active (current) session id
errors: none (no panic; logic-level bug)
reproduction: Run UI, open SESSIONS wedge, confirm list has >1 rows, click any non-current row, observe chat-mini header still shows current id
started: introduced in Plan 26.2.1-07 (ScreenSessions wiring)

## Eliminated

- hypothesis: ROW DATA IS WRONG — captured-variable bug where outer `current_session_id` is sent to `on_select` instead of `s.id`
  evidence: screens/sessions.rs:110-117 iterates `for s in visible.iter().cloned()` and passes `session: s.clone()` to SessionRow; SessionRow lines 135-136 capture `sid_for_select = session.id.clone()` (the per-row id); onclick line 164 calls `on_select.call(sid_for_select.clone())`. No outer-scope capture. Per-row data is correct.
  timestamp: 2026-05-14T00:00:00Z

- hypothesis: SessionIdContext newtype mismatch — writer and reader looking at different signals
  evidence: hermes_app/mod.rs:290 provides `SessionIdContext(session_id)`; both screens/sessions.rs:38 and screens/chat.rs:128 consume it via `use_context::<crate::state::SessionIdContext>().0`. Same provider, same newtype. No mismatch.
  timestamp: 2026-05-14T00:00:00Z

- hypothesis: visible per-row metadata (title, ACTIVE pill) collapses every row to the current
  evidence: title falls back to `session.id.clone()` (the row's own id), pill uses `is_current` prop computed at the call site as `s.id == current` (snapshot taken once via `let current = session_id.read().clone()`). The user's symptom is also explicitly "shows different sessions" — meaning the rows DO render distinguishable. This hypothesis matches the WRONG symptom.
  timestamp: 2026-05-14T00:00:00Z

## Evidence

- timestamp: 2026-05-14T00:00:00Z
  checked: worktree base
  found: had drifted to e6db61dc; reset to 7d4e1a5a48d003f84c46f7ca0790a3d04e158739 per worktree_branch_check
  implication: subsequent file reads were performed after the reset, against the correct codebase state

- timestamp: 2026-05-14T00:00:00Z
  checked: screens/sessions.rs — the for-loop, on_select closure, and SessionRow component
  found: For-loop captures `s` per iteration; SessionRow receives `session: s.clone()` and stores `sid_for_select = session.id.clone()`. onclick fires `on_select.call(sid_for_select.clone())`. The `on_select` closure at lines 63-66 calls `session_id.set(sid); active_screen.set(Screen::Chat);`. SET path is structurally correct — different rows produce different `sid` writes.
  implication: The bug is NOT a captured-variable mistake on the writer side.

- timestamp: 2026-05-14T00:00:00Z
  checked: hermes_app/mod.rs — context provider for SessionIdContext
  found: Line 123: `let mut session_id = use_signal(|| "pending".to_string())`. Line 290: `use_context_provider(|| SessionIdContext(session_id))`. Single provider, single signal. Both writer (sessions.rs) and reader (chat.rs) bind to it via the same newtype lookup.
  implication: Writer and reader share the same signal. No newtype mismatch.

- timestamp: 2026-05-14T00:00:00Z
  checked: screens/chat.rs — what the chat-mini header actually displays
  found: Lines 163-164: `let sid_full = session_id.read().clone();` and `let sid_short: String = sid_full.chars().take(8).collect();`. Line 186: `code { "{sid_short}" }`. The header displays only the first 8 chars of the session id.
  implication: The user-visible "session id in the header" is a TRUNCATION, not the full id.

- timestamp: 2026-05-14T00:00:00Z
  checked: server/api.rs — session key format produced by create_session() and returned by list_sessions()
  found: Line 114: `let session_key = format!("agent:main:web:dm:{session_uuid}");`. This is the key stored via `ensure_web_session` and the key returned by `list_sessions` (via `state_store.list_sessions(Platform::Web, ...)`).
  implication: EVERY session id starts with the literal 17-char prefix `"agent:main:web:dm:"`. The first 8 chars (what the header displays) are ALWAYS `"agent:ma"`.

- timestamp: 2026-05-14T00:00:00Z
  checked: prefix collision math
  found: chars().take(8) on "agent:main:web:dm:<uuid>" → "agent:ma" for every web session. The actual session_id signal IS being updated to a different value on row click; only the user-visible projection collapses.
  implication: ROOT CAUSE — the chat-mini header's "SID code" rendering uses too-short a prefix to distinguish web sessions. The data layer is fine; the visualization is a constant.

## Resolution

root_cause: |
  screens/chat.rs:164 computes the SID badge as `sid_full.chars().take(8).collect()`,
  but every session id produced by server/api.rs:114 begins with the constant 17-character prefix
  "agent:main:web:dm:". The first 8 characters are therefore ALWAYS "agent:ma" for every web
  session. When the user clicks a different row, the SessionIdContext signal IS updated to the
  new id (sessions.rs writer + hermes_app context provider are correct), but the chat-mini
  header truncates to a non-distinguishing prefix, so it visually appears that "all rows
  reference the current session". The data layer is correct; the display projection collapses.
fix: |
  screens/chat.rs:163-164 — change the truncation strategy so it shows distinguishing
  characters from the id. Recommended (smallest change): show the LAST 8 characters of
  the uuid suffix instead of the first 8 of the full key. Mirrors sessions.rs:147-152
  which already does this for the row-sub line:
    let sid_short: String = if sid_full.len() <= 8 {
        sid_full.clone()
    } else {
        sid_full[sid_full.len() - 8..].to_string()
    };
  Optional polish: strip the "agent:main:web:dm:" prefix entirely and show the first
  8 chars of the uuid (more readable than a trailing tail). Either way, the header
  must read from chars BEYOND the constant prefix.
verification: |
  Manual UAT against test 9:
    1. SESSIONS wedge → list renders >1 row.
    2. Click any non-current row.
    3. Chat opens; SID code element shows a value distinct from the previously-active row.
  Optional unit assertion in screens/chat.rs tests:
    assert_ne!(short_sid("agent:main:web:dm:AAAA"), short_sid("agent:main:web:dm:BBBB"));
  Should also walk the SessionIdContext signal value directly (e.g. via tracing::debug
  in on_select) to confirm distinct full ids are written — already proven by code-read,
  but worth a single console.log_1 to lock it.
files_changed:
  - crates/iron_hermes_ui/src/components/hermes_app/screens/chat.rs
