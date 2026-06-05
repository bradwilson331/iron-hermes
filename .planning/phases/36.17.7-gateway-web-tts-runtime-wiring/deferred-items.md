# Phase 36.17.7 — Deferred Items

Items discovered during phase execution that are out of scope per the Scope Boundary rule.

## Plan 01 (foundations) discoveries

### Pre-existing clippy errors in `crates/ironhermes-core/src/tts.rs` — **RESOLVED 2026-06-04**

- **Discovered:** 2026-06-04 during Plan 01 verify_plan check 5 (`cargo clippy -p ironhermes-agent -p ironhermes-tools --tests -- -D warnings`).
- **File:** `crates/ironhermes-core/src/tts.rs` (last modified in `b9d5b018`, Phase 36.17.5-01).
- **Issue:** `clippy::ptr_arg` — `TtsProvider::synthesize` trait used `output_path: &PathBuf` instead of `&Path`.
- **Resolution (2026-06-04):** Migrated trait signature + 4 impl/test sites to `&Path`:
  - `crates/ironhermes-core/src/tts.rs:52` — trait `synthesize` parameter `&PathBuf` → `&Path` (added `Path` to import).
  - `crates/ironhermes-tools/src/tts/edge.rs:81` — impl signature + `output_path.clone()` → `output_path.to_path_buf()` at line 116.
  - `crates/ironhermes-tools/src/tts/elevenlabs.rs:82` — same shape.
  - `crates/ironhermes-tools/tests/tts_tools.rs:115, 167` — 2 FakeProvider test impls + `clone()` → `to_path_buf()`.
- **Verification:** `cargo build -p ironhermes-core -p ironhermes-tools` clean. `cargo test -p ironhermes-tools --test tts_tools --no-run` compiles. `cargo clippy -p ironhermes-core --no-deps` no longer reports `ptr_arg` on tts.rs (ironhermes-core warning count: 17 → 16).
- **Callers unaffected:** `&PathBuf` auto-derefs to `&Path`, so caller sites passing `&PathBuf` keep working without edits.

### Remaining `ironhermes-core` clippy warnings — separate cleanup item

After the `tts.rs` fix, `cargo clippy -p ironhermes-core --no-deps` still reports **16 warnings** unrelated to `tts.rs`:

- Multiple `clippy::collapsible_if` (idiomatic let-chain rewrites)
- `clippy::manual_div_ceil`
- `clippy::needless_range_loop` (loop variable used to index slice)
- `clippy::contains_for_iter`
- `clippy::derivable_impls` (manual `impl X` that could be `#[derive(...)]`)
- `clippy::manual_is_multiple_of`
- `clippy::field_reassign_with_default`

**Scope:** These are pre-existing across `ironhermes-core` (not introduced by Phase 36.17.7). They block `verify_plan` check 5 on any plan that runs `cargo clippy -p ironhermes-agent -p ironhermes-tools --tests -- -D warnings`.

**Status:** Out of scope for Phase 36.17.7 — the user-requested deferred item was specifically the `tts.rs` `ptr_arg` issue, which is now resolved. Approximately 13 of 16 remaining are auto-fixable via `cargo clippy --fix --lib -p ironhermes-core`. Recommend a separate cleanup PR or a follow-up phase before any future plan-execution that needs `-D warnings` to pass cleanly across the workspace.

**Workaround for Phase 36.17.7 verify gates:** `verify_plan` blocks that include clippy-with-deny-warnings should either (a) scope to specific files (`--lib -- -A clippy::all -W clippy::correctness`), or (b) accept the documented pre-existing warnings as out-of-scope until the separate cleanup lands.

## Plan 05 (audio cache + Registered column) discoveries

### `/toolset list` voice row shows `—` / not "Live" despite working voice — **DEFERRED to follow-up**

- **Discovered:** 2026-06-05 during Plan 05 Task 7 operator UAT-Reg, and re-confirmed by the user after phase work landed.
- **Symptom:** Voice (TTS `text_to_speech` + `send_audio`) works end-to-end in **all three** runtime surfaces (Telegram, TUI, Web) — audio is synthesized and delivered. However the in-session `/toolset list` slash command does not surface the `voice` toolset members as registered/`Live`; the `Registered` column reads `—` (or `Inspection`) rather than `Live`.
- **Root cause (hypothesis):** D-06 "Live" display depends on `ToolRegistry::tts_registration_status()` read through the toolset-session handle that the slash dispatcher holds. TTS tools are registered **per-turn** via `register_tts_tools` against the live `AgentRuntime` registry (the `TtsPerTurnWiring` path). The `/toolset list` slash handle in each surface reads a *different* registry instance (the inspection/config-scoped registry from `RegistryToolsetSession`/`ironagent-tools-api`), which never observes the per-turn registration. So the call-time tool availability and the display-time status are sourced from two different registries. Plan 05 Task 5 (REVISION BLOCKER 2 / Path B) wired the accessor + `CommandContext.tts_registration_status` field + dispatch-site threading, but the live per-turn registry handle is not the one the slash command queries in the running surfaces.
- **Why deferred (not blocking):** The functional contract — voice output — is satisfied on all platforms. This is a *display/observability* gap in one diagnostic slash command, not a runtime-capability gap. User explicitly accepted closing the phase with this deferred (equivalent to the executor's `approved-partial-blocker2-live-deferred` verdict).
- **Files implicated:** `crates/ironhermes-tools/src/toolset_session.rs:157` (`RegistryToolsetSession::build_rows`), `crates/ironagent-tools-api/src/toolset_session.rs`, `crates/ironhermes-core/src/commands/toolset_display.rs`, and the per-surface slash-dispatch wiring that constructs the `ToolsetSessionHandle` (must be given the *live* runtime registry Arc, not a fresh inspection registry).
- **Suggested fix (follow-up phase):** Thread the same `Arc<RwLock<ToolRegistry>>` that `register_tts_tools` mutates into the `ToolsetSessionHandle` used by the slash dispatcher in each surface, so `build_rows`/`render_show` read the live registration state. Add a cross-surface behavioral test asserting `/toolset list` shows `voice → Live` after a TTS turn has fired in-session.
- **Status:** OUT OF SCOPE for 36.17.7 close — tracked here for a future `/toolset` observability follow-up.
