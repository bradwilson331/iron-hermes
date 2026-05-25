---
phase: 34b-context-system-parity
reviewed: 2026-05-22T00:00:00Z
depth: standard
files_reviewed: 7
files_reviewed_list:
  - crates/ironhermes-agent/src/context_refs.rs
  - crates/ironhermes-agent/src/agent_loop.rs
  - crates/ironhermes-agent/src/agent_runtime.rs
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-gateway/src/handler.rs
  - crates/iron_hermes_ui/src/server/state.rs
  - crates/ironhermes-agent/tests/invariants_34b.rs
findings:
  critical: 0
  warning: 4
  info: 3
  total: 7
status: issues_found
---

# Phase 34b: Code Review Report

**Reviewed:** 2026-05-22
**Depth:** standard
**Files Reviewed:** 7
**Status:** issues_found

## Summary

This review covers plan 34b-03 (WR-01 gap closure): removal of the in-message
`--- Context Warnings ---` embedding in `context_refs.rs::preprocess_context_references_async`,
and the new out-of-band rendering of `result.context_warnings` across four
production call sites — CLI `run_single` and `run_chat_turn` (`main.rs`), gateway
`run_agent` (`handler.rs`), and web `run_web_turn` (`state.rs`, including the
`Arc<StreamCallback>` wrapping).

The core mechanic is sound. `context_refs.rs` correctly stops embedding warnings
into `final_msg` while preserving the `--- Attached Context ---` block (verified
against the invariant guard in `invariants_34b.rs` and the new
`test_warnings_not_in_message_text_but_on_warnings_vec` test). The warnings flow
through `ContextReferenceResult.warnings → AgentResult.context_warnings` via the
unconditional overwrite at `agent_runtime.rs:381`, and all three surfaces read the
field. No critical correctness or security defects were found in the changed code.

The findings below are real robustness and maintainability defects: a warning-loss
window on the error path that affects every surface, an unbounded warning payload
that is now user-visible, four-way duplication of the rendering block, and a
couple of minor quality issues.

## Narrative Findings (AI reviewer)

## Warnings

### WR-01: Context warnings are silently lost when the turn errors

**File:** `crates/ironhermes-agent/src/agent_runtime.rs:373-382`
**Issue:** `context_warnings` is computed BEFORE `agent.run()` (lines 221-300) but
is only attached to the result on the success path:

```rust
let mut out = agent.run(req.messages).await?;   // line 373 — `?` propagates Err
// ...
out.context_warnings = context_warnings;        // line 381 — only runs on Ok
Ok(out)
```

If `agent.run()` returns `Err` (model connection failure, provider 5xx, cancellation,
etc.), the `?` short-circuits and the already-computed `context_warnings` are dropped.
A user who typed a malformed `@file:does-not-exist` or a blocklisted `@file:~/.ssh/id_rsa`
gets zero feedback about the rejected reference whenever the turn then fails for an
unrelated reason. This affects all three surfaces, because none of them can see
`context_warnings` once `run_turn` returns `Err` (e.g. gateway `handler.rs:1062` only
renders warnings inside the `Ok` arm). The warnings are still logged via
`tracing::warn!` (line 290), but the user-facing channel loses them — defeating the
stated WR-01 goal of surfacing warnings to the user.

**Fix:** Attach the warnings before the `?` can fire, or restructure so the warnings
are surfaced regardless of run outcome:

```rust
let run_result = agent.run(req.messages).await;
match run_result {
    Ok(mut out) => {
        if let Some(ref engine) = engine_handle {
            engine.update_from_response(&out.total_usage);
        }
        out.context_warnings = context_warnings;
        Ok(out)
    }
    Err(e) => {
        for w in &context_warnings {
            tracing::warn!(target: "ironhermes_agent::context_refs", warning = %w,
                "context warning lost on errored turn");
        }
        Err(e)
    }
}
```

If the product decision is "warnings are best-effort and only matter on success,"
document that explicitly — the doc comment at lines 369-372 currently implies surfaces
always render them, which is not true on the error path.

### WR-02: Unbounded warning payload is now rendered directly to users

**File:** `crates/ironhermes-cli/src/main.rs:863-872`, `crates/ironhermes-cli/src/main.rs:2345-2353`, `crates/ironhermes-gateway/src/handler.rs:1133-1144`, `crates/iron_hermes_ui/src/server/state.rs:264-278`
**Issue:** Each surface renders `result.context_warnings` verbatim with no cap on
count or per-warning length. `context_warnings` is populated one-entry-per-reference
in `preprocess_context_references_async` (`context_refs.rs:840-842`), and individual
warnings interpolate untrusted strings: the raw reference text `r.raw`, filesystem
error strings, git `stderr` (`context_refs.rs:687-689`), and URL-fetcher error text
(`context_refs.rs:730`). A message containing dozens of `@file:` references, or a git
command that emits a large `stderr`, produces a `--- Context Warnings ---` block of
arbitrary size flushed straight to the terminal (CLI), sent as a Telegram message
(gateway — note `send_message` has platform length limits and the call result is
discarded with `let _ =` at `handler.rs:1141`), or streamed to the web client.

Embedding was removed precisely to avoid spending model tokens on this metadata, but
the user-facing rendering inherited the same unbounded growth with no truncation.

**Fix:** Cap the rendered block (both warning count and per-line length) before
formatting:

```rust
const MAX_WARNINGS: usize = 20;
const MAX_WARNING_LEN: usize = 500;
let warning_lines: Vec<String> = result.context_warnings.iter().take(MAX_WARNINGS)
    .map(|w| {
        let mut s = w.clone();
        if s.len() > MAX_WARNING_LEN { s.truncate(MAX_WARNING_LEN); s.push('…'); }
        format!("- {}", s)
    })
    .collect();
// append "…and N more" when truncated; on gateway, respect platform message limits.
```

### WR-03: Warning-rendering block duplicated across four call sites

**File:** `crates/ironhermes-cli/src/main.rs:863-872`, `crates/ironhermes-cli/src/main.rs:2345-2353`, `crates/ironhermes-gateway/src/handler.rs:1133-1144`, `crates/iron_hermes_ui/src/server/state.rs:264-278`
**Issue:** The `warning_lines` map/format + `--- Context Warnings ---` join logic is
copy-pasted in four places with formatting drift: CLI and web wrap the block in
leading/trailing `\n` (`"\n--- Context Warnings ---\n{}\n"`), while the gateway omits
them (`"--- Context Warnings ---\n{}"`). This is exactly the divergence WR-02's fix
would have to be applied four times to. Drift here means a future change (e.g. the
truncation in WR-02) is easy to apply inconsistently or miss one surface — and the
inconsistency directly affects user-visible output.

**Fix:** Extract a single formatter, e.g.
`fn format_context_warnings(warnings: &[String]) -> Option<String>` in the agent crate
(next to `AgentResult`), and have all four surfaces call it. Surface-specific transport
(print vs `write_into_scroll_region` vs `send_message` vs stream callback) stays at the
call site; only the block formatting is shared.

### WR-04: Web surface sends the warning block through a turn-scoped callback (fragile coupling)

**File:** `crates/iron_hermes_ui/src/server/state.rs:231-238`, `crates/iron_hermes_ui/src/server/state.rs:264-278`
**Issue:** `run_web_turn` wraps the incoming `stream_callback` in an `Arc`
(`stream_cb_arc`), passes a forwarding `Box` into the turn, then re-invokes
`(stream_cb_arc)(warnings_block)` at line 277 AFTER `run_turn` returns. Per the field
docs in this file (lines 47-52, 110, 168), the underlying per-turn stream sender is
installed by `ws.rs` immediately before `run_web_turn` and cleared by an RAII guard
after the call. The warnings invocation happens inside `run_web_turn` so the sender is
still installed — currently correct — but delivery is coupled to the precise ordering
of the RAII guard relative to the warning send. If a future refactor moves the warning
rendering outside `run_web_turn` (mirroring the gateway's "separate send" pattern), the
`stream_cb_arc` would fire after the sender is torn down and the warnings would be
silently dropped (the callback typically does a `try_send` that fails quietly). Note
also that the warning block streamed at line 277 is not added to `result.appended`
(lines 253-256 persist only `appended`), so on web reload the warnings disappear.

**Fix:** Add a comment/assertion that the warning send MUST occur while the per-turn
sender is installed (i.e. before returning from `run_web_turn`), and document the
ephemerality. If the gateway's "send as a distinct message" model is preferred for
consistency, capture the warnings into an owned value and render through a transport
that does not depend on the turn-scoped callback.

## Info

### IN-01: Duplicate `"toml"` entry in binary-file text-extension allowlist

**File:** `crates/ironhermes-agent/src/context_refs.rs:471`
**Issue:** `text_exts` lists `"toml"` twice:
`["py","md","txt","json","yaml","yml","toml","js","ts","rs","toml","sh","html","css"]`.
Harmless (membership check is unaffected) but indicates a copy-paste slip and is dead
duplication.
**Fix:** Remove the second `"toml"`.

### IN-02: CLI `run_chat_turn` computes an unused, mislabeled `context_length` local

**File:** `crates/ironhermes-cli/src/main.rs:2365-2366`
**Issue:** `let context_length = runtime.config().agent.max_iterations;` is immediately
followed by `let _ = context_length;` with a comment that the real value is resolved
inside `run_turn`. This is dead code that also mislabels `max_iterations` as
`context_length`, which is misleading to a future reader.
**Fix:** Delete both lines; the status line already reads the resolved limit from
`tui.status_snapshot().tokens_limit` (line 2372).

### IN-03: Gateway discards `send_message` result for the warnings block

**File:** `crates/ironhermes-gateway/src/handler.rs:1141-1143`
**Issue:** The warnings `send_message` call result is dropped with `let _ = ...`. This
matches the surrounding best-effort pattern (e.g. the `error_suffix` send at
`handler.rs:1167`), so it is consistent, but combined with WR-02 (unbounded payload) a
warnings block that exceeds the platform message-size limit would fail silently with no
log line.
**Fix:** Log the send failure with `tracing::debug!`, or rely on the WR-02 truncation
fix to keep the block within platform limits.

---

_Reviewed: 2026-05-22_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
