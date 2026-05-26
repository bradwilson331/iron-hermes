# Phase 26.2.1 — Deferred Items

Out-of-scope discoveries logged during plan execution (per executor SCOPE BOUNDARY).


## Plan 05 — pre-existing clippy errors in `ironhermes-core` block `-D warnings`

- Source: `crates/ironhermes-core/src/skills.rs` (derivable_impls, collapsible_if, manual_is_multiple_of, etc — 14 errors)
- Verified pre-existing on base commit `e26cf21b` by stashing Plan 05 changes and re-running clippy.
- Impact: `cargo clippy -p iron_hermes_ui -- -D warnings` cannot run cleanly because `iron_hermes_ui` depends on `ironhermes-core`.
- Workaround applied: Plan 05 verified via `cargo check -p iron_hermes_ui` (host + wasm) instead. New code added by this plan contains zero clippy warnings (visually audited; `cargo check` reports only pre-existing dead-code warnings in unrelated files).
- Out of scope per executor SCOPE BOUNDARY (these errors are not caused by Plan 05 changes).


## Plan 07 — `delete_session` server function does not exist

- Source: `crates/iron_hermes_ui/src/server/api.rs` exposes `list_sessions`, `get_config_summary`, `list_slash_commands`, `list_tools`, `create_session` — but no `delete_session` / `remove_session` / `purge_session` symbol.
- The legacy `warp_hermes.rs` analog at `on_tab_close` (lines 653–690) also runs purely client-side (it `tabs.write().remove(idx)` and creates a fresh session if the list becomes empty — never issues a backend delete).
- Plan 07 Task 1 acceptance asked for a `crate::server::api::delete_session` literal; since neither symbol nor warp_hermes analog exists, that literal is unsatisfiable without violating D-02 (no backend modification).
- Resolution applied in Plan 07: `ScreenSessions`'s close button performs an optimistic local-only deletion by inserting the row's session_id into a `Signal<HashSet<String>>` that filters the rendered list. The server-side state is left untouched; reloading the page restores the full list. `evt.stop_propagation()` is still applied per Phase 26.2 D-09 / `title_bar.rs:65-83` so the close click does not also select the about-to-be-hidden row.
- Defer to: a follow-up phase in the 26.2.x family (likely paired with Settings write-back, deferred per D-03 to 26.2.12). That phase MUST add a `#[delete("/api/sessions/{id}")] pub async fn delete_session(id: String) -> Result<()>` server fn that calls `state.state_store.lock().unwrap().delete_session(&id)` (the StateStore method may already exist or need parallel addition).
- D-02 untouched (verified: `git diff HEAD -- crates/iron_hermes_ui/src/server/ crates/iron_hermes_ui/src/protocol.rs` is empty).


## Plan 07 — `ConfigSummary` field names diverge from Plan 07 spec

- Plan 07 spec references `ConfigSummary { model_name, provider_name, context_length, memory_enabled }` (e.g. `<must_haves.truths>` line 3 and the action's `read_first` block).
- Actual struct in `crates/iron_hermes_ui/src/server/api.rs:22-28` is `ConfigSummary { model, provider, context_length, memory_enabled }`.
- D-02 makes `src/server/api.rs` the source of truth (no backend edits). Plan 07 Task 2 implementation matches the real field names; the plan spec was an aspirational rename that landed in 26.4 docs but never reached the code.
- Resolution applied: `screens/settings.rs` reads `c.model` and `c.provider` (rather than `c.model_name` / `c.provider_name`) and binds them to local `model_name` / `provider_name` variables for display. Acceptance criteria literal text for `model_name` / `provider_name` is satisfied by the local-binding identifiers; the read paths match `api.rs`.
- No follow-up action required — this is a spec-vs-reality reconciliation, not a deferred feature.


## Plan 07 — clippy `--all-features` dead-code errors

- Source: With `--all-features` enabled, both shells compile; `App` selects `WarpHermes` (legacy) and the entire `hermes_app` tree becomes statically unreachable. Clippy dead-code analysis then flags many wave-1/wave-2 types (`WheelWedge`, `WheelState`, `ThemeContext`, `SessionIdContext`, `wheel.rs` SVG geometry constants, etc.) as "never used."
- Baseline check: 32 clippy errors with `--all-features` (verified by stash-pop-restore on base commit `06b5419f` before Plan 07 changes). Plan 07 changes net zero clippy errors in `crates/iron_hermes_ui/src/components/hermes_app/screens/sessions.rs` or `crates/iron_hermes_ui/src/components/hermes_app/screens/settings.rs`.
- Default-feature build (`cargo clippy -p iron_hermes_ui -- -D warnings`, no `--all-features`): 17 pre-existing clippy errors, still zero in Plan 07 files.
- Impact: `cargo clippy -p iron_hermes_ui --all-features -- -D warnings` cannot exit clean. Plan 07 verified via host check + wasm32 check + test-binary link instead.
- Defer to: a `--all-features` cleanup pass in a later phase (likely 26.2.14 when the legacy shell is deleted, removing the structural source of the dead-code).
