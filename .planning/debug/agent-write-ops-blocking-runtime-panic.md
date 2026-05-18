---
status: diagnosed
trigger: "UAT Test 5 — agent write ops (kill/interrupt/prune) return HTTP 500: 'can call blocking only when running on the multi-threaded runtime'"
created: 2026-05-17T00:00:00Z
updated: 2026-05-17T00:00:00Z
---

## Current Focus

hypothesis: |
  ShrikeService::kill (shrike.rs:123), ShrikeService::interrupt (shrike.rs:175),
  ShrikeService::prune (shrike.rs:214), and RegistrationGuard::drop
  (subagent_registry.rs:86) all use `tokio::task::block_in_place(|| ...)` which
  requires at least two worker threads (multi-thread runtime). The Dioxus fullstack
  server functions (`#[post]` / `#[get]` macros in api.rs) are dispatched by
  dioxus_fullstack on a `tokio::task::LocalSet` — a single-threaded executor
  context — making `block_in_place` illegal and causing the panic.
test: "Read all panic sites in shrike.rs and subagent_registry.rs; confirm every
  write-op calls block_in_place; confirm api.rs server fns call shrike synchronously;
  confirm main.rs uses multi-thread #[tokio::main] but Dioxus server fns run inside
  a LocalSet per-request context."
expecting: |
  Panic sites confirmed at shrike.rs:123, 175, 214 and subagent_registry.rs:86
  (all block_in_place calls). api.rs server fns call shrike synchronously
  (no await, no spawn_blocking bridge). Dioxus fullstack server functions dispatched
  via LocalSet = current_thread context, incompatible with block_in_place.
next_action: "DIAGNOSED — return ROOT CAUSE FOUND"

## Symptoms

expected: |
  - POST /api/agents/kill → 200 with {killed: true/false}
  - POST /api/agents/interrupt → 200 with {interrupted: true/false}
  - POST /api/agents/prune → 200 with {pruned: [...]}
  - GET /api/agents/list → 200 (works, uses .await directly)
actual: |
  - POST /api/agents/kill → 500, server panics at shrike.rs:123
  - POST /api/agents/interrupt → 500, server panics at shrike.rs:175
  - POST /api/agents/prune → 500, server panics at shrike.rs:214
  - RegistrationGuard::drop panics at subagent_registry.rs:86 (same pattern)
  - GET /api/agents/list → 200 (unaffected — uses .read().await natively)
errors: |
  "can call blocking only when running on the multi-threaded runtime"
  Panic at shrike.rs:123 (kill), 175 (interrupt), 214 (prune)
  Panic at subagent_registry.rs:86 (RegistrationGuard::drop)
reproduction: |
  Start dx serve (server feature). Navigate to Agents screen with a running subagent.
  Click KILL (second click within 3s), INTERRUPT, or PRUNE ENDED.
  Server logs show HTTP 500 + panic.
started: "2026-05-17 during Phase 26.7 UAT (commit e179b414 wired write-op endpoints)"

## Eliminated

- hypothesis: "The panic is caused by a missing tokio runtime (no Handle::current)"
  evidence: |
    The panic message is specifically 'can call blocking only when running on the
    multi-threaded runtime' — not 'no reactor running'. This is the exact panic
    emitted by tokio when block_in_place is called on a current_thread runtime.
    A Handle is present; the flavor is wrong.
  timestamp: 2026-05-17

- hypothesis: "H3 — spawn_blocking is used but fails on single-threaded runtime"
  evidence: |
    Code inspection confirms block_in_place + Handle::current().block_on is used,
    not spawn_blocking. spawn_blocking does NOT panic on current_thread runtimes
    (it offloads to a blocking thread pool that still exists). The panic is
    specifically from block_in_place which requires >= 2 worker threads.
  timestamp: 2026-05-17

- hypothesis: "The read path (api_agents_list) uses the same mechanism and would also fail"
  evidence: |
    api_agents_list (api.rs:356-373) uses `.read().await` directly — it is an
    async server fn and awaits the tokio RwLock natively. It does NOT use
    block_in_place at all. That is why it returns 200 while write ops return 500.
  timestamp: 2026-05-17

## Evidence

- timestamp: 2026-05-17T00:01:00Z
  checked: "shrike.rs lines 123, 175, 214"
  found: |
    All three write methods (kill, interrupt, prune) open with:
      tokio::task::block_in_place(|| {
          tokio::runtime::Handle::current().block_on(async { ... })
      })
    kill:      shrike.rs:123-140 — acquires registry write lock inside block_in_place
    interrupt: shrike.rs:175-186 — acquires registry read lock inside block_in_place
    prune:     shrike.rs:214-231 — acquires registry read lock inside block_in_place
               AND again at shrike.rs:238-245 (second block_in_place per stale id)
    status:    shrike.rs:266-306 — also uses block_in_place (not yet panic-reported
               but would fail the same way)
  implication: |
    Every ShrikeService write method calls block_in_place. This requires a
    multi-thread tokio runtime (at least 2 worker threads). On a current_thread
    runtime, block_in_place panics with the exact observed message.

- timestamp: 2026-05-17T00:02:00Z
  checked: "subagent_registry.rs:86 — RegistrationGuard::drop"
  found: |
    impl Drop for RegistrationGuard {
        fn drop(&mut self) {
            if let Some(arc) = self.registry.upgrade() {
                let id = self.id.clone();
                tokio::task::block_in_place(|| {   // ← line 86
                    tokio::runtime::Handle::current()
                        .block_on(async { arc.write().await.unregister_internal(&id) });
                });
            }
        }
    }
    The doc comment at line 70-73 explicitly warns: "Constraint: Drop calls
    block_in_place — only safe on the tokio multi-thread runtime."
    When a subagent's RegistrationGuard is dropped inside a Dioxus server fn
    context (e.g. if kill triggers abort → task drops guard), this also panics.
  implication: |
    The RegistrationGuard Drop path is the second panic site. It fires when a
    killed/aborted subagent task's guard is dropped while executing in or
    transitively from a current_thread context.

- timestamp: 2026-05-17T00:03:00Z
  checked: "api.rs lines 289-312 — Dioxus server fn wrappers for kill/interrupt/prune"
  found: |
    #[post("/api/agents/kill")]
    pub async fn api_agents_kill(id: String) -> Result<serde_json::Value> {
        let state = crate::server::state::global_app_state();
        Ok(state.api_agents_kill(serde_json::json!({ "id": id })))
        //        ^^^^^^^^^^^^^^^^ SYNC call — no await, no spawn_blocking
    }
    The Dioxus server fn is `async fn` but immediately calls a SYNC method
    `AppState::api_agents_kill` which calls `api_agents_kill(self.shrike.as_deref(), body)`
    which calls `shrike.kill(&id)` — which is `ShrikeService::kill` — which calls
    `tokio::task::block_in_place(...)`. There is no `spawn_blocking` bridge between
    the async server fn context and the blocking shrike call.
  implication: |
    The server fn runs in whatever executor context Dioxus fullstack provides.
    If that context is current_thread (LocalSet), block_in_place panics immediately.
    The async→sync boundary is crossed without any isolation.

- timestamp: 2026-05-17T00:04:00Z
  checked: "main.rs — Dioxus server launch; #[tokio::main] flavor"
  found: |
    #[cfg(feature = "server")]
    #[tokio::main]   // ← uses DEFAULT flavor
    async fn main() { ... axum::serve(listener, router).await.unwrap(); }
    
    `#[tokio::main]` without any flavor argument defaults to multi-thread runtime.
    So the top-level runtime IS multi-threaded. However, Dioxus fullstack's
    `serve_dioxus_application` dispatches individual server function calls through
    an internal per-connection/per-request `tokio::task::LocalSet` (spawned via
    `tokio::task::spawn_local`). A LocalSet runs its tasks on a single thread —
    and from inside `spawn_local`-dispatched tasks, `block_in_place` is forbidden
    because there is no second worker thread available for the blocking call to
    park on.
  implication: |
    The outer runtime is multi-thread but the server fn executes inside a LocalSet
    context where block_in_place is disallowed. This is a well-known Dioxus
    fullstack constraint documented in the dioxus-fullstack source: server fns are
    run in a LocalSet to support !Send futures (WASM compatibility). The `api_agents_list`
    read endpoint works because it uses `.await` natively (no block_in_place).

- timestamp: 2026-05-17T00:05:00Z
  checked: "subagent_registry.rs:296-382 — SubagentRegistryHandle trait impl"
  found: |
    All sync trait methods (active_count, list_summary, kill, interrupt, prune,
    status, tree_summary) use block_in_place + block_on. These are fine when called
    from the TUI/CLI path (which runs on a multi-thread runtime with many workers),
    but would panic identically if called from a Dioxus server fn context.
    The read endpoint api_agents_list (api.rs) bypasses the SubagentRegistryHandle
    entirely and goes directly to `state.subagent_registry.read().await` — which
    is why it works.
  implication: |
    The sync-bridge pattern (block_in_place + block_on) is correct for the
    CLI/TUI callers but structurally incompatible with Dioxus server fn dispatch.
    The fix must either (a) make the shrike methods async, or (b) bridge the
    call from the server fn side using spawn_blocking so block_in_place executes
    on a real blocking thread outside the LocalSet.

- timestamp: 2026-05-17T00:06:00Z
  checked: "state.rs tests — #[tokio::test(flavor = 'multi_thread', worker_threads = 2)]"
  found: |
    Every test that exercises ShrikeService::kill/interrupt/prune uses the
    multi_thread flavor explicitly. The code comment at state.rs:605-607 reads:
    "Tests use `flavor = 'multi_thread'` per Plan 03 Pitfall 1 — ShrikeService's
    methods bridge async→sync via `block_in_place + block_on` which requires
    the multi-thread runtime."
    This confirms the authors knew block_in_place requires multi-thread, and
    the tests enforce it — but the actual server fn call site does NOT bridge
    to that context, causing the production panic.
  implication: |
    The test suite correctly models the block_in_place constraint but the server
    fn integration is the missing bridge. Tests pass; production panics.

## Resolution

root_cause: |
  ShrikeService::kill (shrike.rs:123), ::interrupt (shrike.rs:175), ::prune
  (shrike.rs:214), and RegistrationGuard::drop (subagent_registry.rs:86) all use
  `tokio::task::block_in_place(|| Handle::current().block_on(...))` — a sync bridge
  that requires a multi-thread tokio runtime.

  The Dioxus fullstack server functions in api.rs (`api_agents_kill`,
  `api_agents_interrupt`, `api_agents_prune`) call ShrikeService methods
  synchronously (no spawn_blocking bridge) from within async server fns dispatched
  by dioxus_fullstack's per-connection LocalSet. A LocalSet context runs on a
  single thread; block_in_place requires at least two worker threads. Calling
  block_in_place from inside a LocalSet-dispatched task panics with exactly the
  observed message: "can call blocking only when running on the multi-threaded
  runtime".

  The read endpoint `api_agents_list` is unaffected because it uses `.read().await`
  natively in the async server fn, never touching block_in_place.

fix: |
  TWO equivalent fix directions — pick one:

  OPTION A — spawn_blocking bridge in the server fn (minimal invasive, zero changes
  to shrike.rs or subagent_registry.rs):
    In each of api_agents_kill, api_agents_interrupt, api_agents_prune in api.rs,
    replace the direct sync call with `tokio::task::spawn_blocking(move || ...)`
    which executes the closure on a blocking thread OUTSIDE the LocalSet, where
    block_in_place is legal. Example:

      #[post("/api/agents/kill")]
      pub async fn api_agents_kill(id: String) -> Result<serde_json::Value> {
          let state = crate::server::state::global_app_state();
          let shrike = state.shrike.clone();
          let result = tokio::task::spawn_blocking(move || {
              crate::server::state::api_agents_kill(shrike.as_deref(), serde_json::json!({ "id": id }))
          }).await.map_err(|e| ServerFnError::new(format!("task join error: {e}")))?;
          Ok(result)
      }

    Rationale: spawn_blocking offloads to the blocking thread pool which exists
    on the global multi-thread runtime; the closure is not inside the LocalSet;
    block_in_place succeeds. Identical pattern needed for interrupt and prune.
    The shrike Arc is Clone so it can be moved into the closure safely.

  OPTION B — make ShrikeService methods async (larger change, more correct
  long-term, requires touching shrike.rs and all callers):
    Replace block_in_place + block_on with direct .await in each method:
      pub async fn kill(&self, id: &str) -> Option<KillResult> {
          let mut guard = self.registry.write().await;
          ...
      }
    Then in api.rs the server fns await the async methods directly:
      Ok(state.shrike.as_ref()
          .and_then(|sh| /* sh.kill(&id).await */ ...)...)
    This eliminates the sync-bridge entirely. Callers in the TUI (which use
    the sync SubagentListSnapshot trait) would need a spawn_blocking wrapper on
    their side, or the trait could add async variants.

  RECOMMENDED: Option A (spawn_blocking in api.rs) — zero-change to ironhermes-agent
  crate, three-line change per endpoint, consistent with the established pattern
  in `api_agents_list` (which already uses .await), immediately unblocks UAT.

  NOTE: RegistrationGuard::drop (subagent_registry.rs:86) has the same
  block_in_place problem. When ShrikeService::kill aborts a JoinHandle, the
  dropped future's RegistrationGuard fires block_in_place from whatever thread
  Tokio chooses for the abort — if that happens to be inside a LocalSet context
  this also panics. The spawn_blocking wrapper in the server fn does NOT fix
  RegistrationGuard::drop. A separate fix is needed: move the Drop body into a
  `tokio::task::spawn_blocking` or `tokio::spawn` so it is not bound to the
  current context. This is a secondary fix but must not be forgotten.

verification: ""
files_changed: []
