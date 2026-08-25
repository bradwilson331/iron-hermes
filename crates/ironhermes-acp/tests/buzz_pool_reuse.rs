//! Multi-session-per-process isolation tests (Phase 47.7 plan 04).
//!
//! RESEARCH.md's D-15 finding: `buzz-acp`'s `AgentPool` keeps N long-lived agent
//! subprocesses, claiming one per prompt task and returning it to the pool afterwards, with
//! only best-effort `channel_id` affinity — so a SINGLE `ironhermes acp` process serves
//! prompts from DIFFERENT Buzz community channels over its lifetime. Zed's one-session-
//! per-editor-window usage pattern never exercised that; `AcpSessionManager` was BUILT for
//! it (36.8 D-11), but "designed for it" and "proven under it" are different claims. This
//! file closes that gap: every test here drives multiple `session/new` calls over ONE
//! long-lived connection (mirroring one pooled `ironhermes acp` OS process serving many
//! channels), and asserts nothing shared — cwd, context files, conversation history,
//! approval grants, or cancellation — leaks between the sessions sharing that process.
//!
//! Six tests total: four session-identity/isolation tests (task 1, T-47.7-12/T-47.7-15)
//! plus two containment tests (task 2, T-47.7-13/T-47.7-14). All four task-1 tests drive
//! the real wire-level harness (`spawn_agent_over_ndjson`, byte-oriented NDJSON framing —
//! the same framing a spawned `ironhermes acp` subprocess talking over real stdin/stdout
//! would exercise) to prove multiple `session/new` calls succeed live over one connection.
//! Two of those four (context-file isolation, interleaving) ALSO drive a
//! directly-constructed `AcpSessionManager` for their actual assertion, because the field
//! they assert on (`AgentRuntime::context_file_paths()`, `AcpSession::cwd`/`session_id`) is
//! not observable through the wire protocol at all. The two task-2 containment tests are
//! wire-free entirely — `ApprovalsStore` state and live `CancellationToken` state are
//! likewise not wire-observable, and (for cancellation specifically) turns across every
//! session on one process are strictly serialized by `AcpSessionManager`'s own lock, making
//! a true "two turns simultaneously in flight" wire-level scenario structurally
//! impossible. Every test states its own rationale inline where it departs from the wire
//! harness.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::{NdjsonClient, spawn_agent_over_ndjson};
use ironhermes_acp::session_manager::AcpSessionManager;
use ironhermes_core::{Config, ProviderConfig, ProviderResolver};
use serde_json::json;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Same pattern as `session_cancel.rs`/`execute_code_gate.rs`: a `Config`/`ProviderResolver`
/// whose main provider resolves to the mocked `server_uri`, so a real turn's HTTP call lands
/// on our wiremock server instead of a real provider.
fn build_config_and_resolver_pointed_at(server_uri: &str) -> (Arc<Config>, Arc<ProviderResolver>) {
    let mut config = Config::default();
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            base_url: Some(server_uri.to_string()),
            api_key: Some("test-key".to_string()),
            ..Default::default()
        },
    );
    let resolver =
        ProviderResolver::build(&config).expect("ProviderResolver::build with mocked provider");
    (Arc::new(config), Arc::new(resolver))
}

/// SSE body for a plain, successful (non-tool-call) turn: one text delta, then a
/// `finish_reason: stop` chunk carrying usage. Mirrors `session_cancel.rs`'s
/// `sse_normal_turn` / `execute_code_gate.rs`'s `sse_final_text_with_usage`.
fn sse_final_text_with_usage() -> String {
    let chunks = [
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{"content":"ok"}}]}"#,
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}"#,
    ];
    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// `initialize`, discarding the response beyond a basic success check — every test needs
/// this handshake before `session/new` will be accepted.
async fn initialize(client: &mut NdjsonClient) {
    let resp = client.request("initialize", json!({"protocolVersion": 1})).await;
    assert!(
        resp.get("result").is_some(),
        "initialize should succeed: {resp:?}"
    );
}

/// `session/new` rooted at `cwd`, returning the allocated session id.
async fn session_new(client: &mut NdjsonClient, cwd: &Path) -> String {
    let resp = client
        .request("session/new", json!({"cwd": cwd, "mcpServers": []}))
        .await;
    resp["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("session/new should return a sessionId: {resp:?}"))
        .to_string()
}

/// `session/prompt` against `session_id`, awaiting the matching `PromptResponse` (discarding
/// any `session/update` notifications in between — `NdjsonClient::request` already does
/// this).
async fn prompt_and_wait(client: &mut NdjsonClient, session_id: &str, text: &str) -> serde_json::Value {
    client
        .request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": text}],
            }),
        )
        .await
}

// ── Task 1: session identity and isolation across one pooled connection ───────────────

/// Test 1: four `session/new` calls over ONE connection return four distinct session ids
/// and none errors — the process serves the pool, not one editor window.
#[tokio::test]
async fn four_sequential_sessions_on_one_connection_each_get_a_distinct_id() {
    let (config, resolver) = common::build_config_and_resolver();
    let (state_store, _state_tmp) = common::build_state_store();
    let (agent_task, mut client) = spawn_agent_over_ndjson(config, resolver, state_store);

    initialize(&mut client).await;

    let mut dirs = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..4 {
        let dir = tempfile::tempdir().expect("tempdir");
        let id = session_new(&mut client, dir.path()).await;
        assert!(!id.is_empty(), "session/new must return a non-empty session id");
        ids.push(id);
        dirs.push(dir);
    }

    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        4,
        "all four sessions created over the one pooled connection must have distinct ids: {ids:?}"
    );

    agent_task.abort();
}

/// Test 2: two sessions created over one connection, rooted at two tempdirs each holding a
/// distinct `AGENTS.md`, resolve context file paths containing only their own directory's
/// file (D-15, T-47.7-12).
///
/// Two parts, deliberately: first, the wire-level connection proves TWO independently
/// addressable sessions can be created live, over ONE pooled connection, rooted at two
/// distinct cwds — buzz-acp's actual usage shape. Second, the actual context-file-path
/// assertion is done via DIRECT `AcpSessionManager` introspection, NOT by inspecting what a
/// real turn sends a mocked provider: tracing `handlers.rs::run_prompt_turn` ->
/// `AgentRuntime::run_turn` -> `agent.run(req.messages)` shows `req.messages` reaches the
/// LLM client UNMODIFIED — no `PromptBuilder` call exists anywhere in the `ironhermes-acp`
/// crate, and `AgentLoop::context_file_paths` (set via `with_context_file_paths`) is
/// consumed ONLY by the D-CACHE-03 mtime cache-break-warning mechanism, never to embed
/// content into the prompt. So `AgentRuntime::context_file_paths()` — the exact accessor
/// `session_cwd_isolation.rs` asserts on — is genuinely not wire-observable through a real
/// turn's provider request; the manager-level call below drives the SAME production
/// construction path (`AcpSessionManager::create` -> `build_and_insert` ->
/// `AgentRuntime::from_config`) the wire-level `session/new` handler above just exercised,
/// which is "reusing the accessor" in the way the plan means, not a hand-rolled
/// recomputation.
#[tokio::test]
async fn sessions_rooted_at_different_cwds_do_not_share_context_files() {
    let (config, resolver) = common::build_config_and_resolver();
    let (state_store, _state_tmp) = common::build_state_store();
    let (agent_task, mut client) = spawn_agent_over_ndjson(config, resolver, state_store);

    initialize(&mut client).await;

    let dir_a = tempfile::tempdir().expect("tempdir a");
    let dir_b = tempfile::tempdir().expect("tempdir b");
    std::fs::write(dir_a.path().join("AGENTS.md"), "project A instructions")
        .expect("write dir_a AGENTS.md");
    std::fs::write(dir_b.path().join("AGENTS.md"), "project B instructions")
        .expect("write dir_b AGENTS.md");

    let session_a_wire = session_new(&mut client, dir_a.path()).await;
    let session_b_wire = session_new(&mut client, dir_b.path()).await;
    assert_ne!(
        session_a_wire, session_b_wire,
        "session/new for two distinct cwds over one connection must return distinct ids"
    );
    agent_task.abort();

    // Manager-level introspection (see doc comment above for why).
    let (config2, resolver2) = common::build_config_and_resolver();
    let (state_store2, _state_tmp2) = common::build_state_store();
    let mut manager = AcpSessionManager::new(state_store2, config2, resolver2);

    let id_a = manager
        .create(dir_a.path().to_path_buf(), None)
        .await
        .expect("create session a");
    let id_b = manager
        .create(dir_b.path().to_path_buf(), None)
        .await
        .expect("create session b");

    let agents_md_a = dir_a.path().join("AGENTS.md");
    let agents_md_b = dir_b.path().join("AGENTS.md");

    let session_a = manager.get(&id_a).expect("session a must exist");
    let paths_a = session_a.runtime.context_file_paths();
    assert!(
        paths_a.contains(&agents_md_a),
        "session A's resolved context-file paths must include its OWN directory's AGENTS.md: {paths_a:?}"
    );
    assert!(
        !paths_a.contains(&agents_md_b),
        "session A's resolved context-file paths must NOT include session B's directory \
         (this is the exact regression a shared, process-wide AgentRuntime would \
         reintroduce): {paths_a:?}"
    );

    let session_b = manager.get(&id_b).expect("session b must exist");
    let paths_b = session_b.runtime.context_file_paths();
    assert!(
        paths_b.contains(&agents_md_b),
        "session B's resolved context-file paths must include its OWN directory's AGENTS.md: {paths_b:?}"
    );
    assert!(
        !paths_b.contains(&agents_md_a),
        "session B's resolved context-file paths must NOT include session A's directory: {paths_b:?}"
    );
}

/// Test 3: a message persisted against session A is not returned when reading session B's
/// history from the shared `StateStore` — the store is shared by design (one `state.db` per
/// home, same as CLI and gateway sessions); isolation must come from the session key, and
/// this is what proves it holds (D-15, T-47.7-15).
#[tokio::test]
async fn conversation_history_does_not_bleed_between_sessions() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_final_text_with_usage(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let (config, resolver) = build_config_and_resolver_pointed_at(&server.uri());
    let (state_store, _state_tmp) = common::build_state_store();
    // Kept alongside the handle handed to `spawn_agent_over_ndjson` — the SAME `StateStore`
    // the server persists turns into (D-10 write-through), so reading through this handle
    // after the turns complete reads the real production write path, not a second store.
    let state_store_for_assertions = state_store.clone();
    let (agent_task, mut client) = spawn_agent_over_ndjson(config, resolver, state_store);

    initialize(&mut client).await;

    let dir_a = tempfile::tempdir().expect("tempdir a");
    let dir_b = tempfile::tempdir().expect("tempdir b");

    let session_a = session_new(&mut client, dir_a.path()).await;
    let session_b = session_new(&mut client, dir_b.path()).await;

    prompt_and_wait(&mut client, &session_a, "session A secret marker CCC333").await;
    prompt_and_wait(&mut client, &session_b, "session B secret marker DDD444").await;

    let messages_a = {
        let store = state_store_for_assertions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        store
            .get_chat_messages(&session_a)
            .expect("read session A history")
    };
    let messages_b = {
        let store = state_store_for_assertions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        store
            .get_chat_messages(&session_b)
            .expect("read session B history")
    };

    assert!(
        messages_a
            .iter()
            .any(|m| m.content_text().is_some_and(|t| t.contains("CCC333"))),
        "session A's history must contain ITS OWN message: {messages_a:?}"
    );
    assert!(
        !messages_a
            .iter()
            .any(|m| m.content_text().is_some_and(|t| t.contains("DDD444"))),
        "session A's history must NOT contain session B's message — the shared StateStore is \
         intentional (one state.db per home); per-session keying is the actual isolation \
         boundary being asserted here: {messages_a:?}"
    );

    assert!(
        messages_b
            .iter()
            .any(|m| m.content_text().is_some_and(|t| t.contains("DDD444"))),
        "session B's history must contain ITS OWN message: {messages_b:?}"
    );
    assert!(
        !messages_b
            .iter()
            .any(|m| m.content_text().is_some_and(|t| t.contains("CCC333"))),
        "session B's history must NOT contain session A's message: {messages_b:?}"
    );

    agent_task.abort();
}

/// Test 4: create A, create B, then operate on A again — A's identity and cwd are unchanged
/// by B's creation.
///
/// Same two-part structure as test 2 (wire connection proves live multi-session creation;
/// the actual identity/cwd assertion is manager-level, for the same reason documented on
/// test 2 — `session.cwd`/`session.session_id` are ordinary struct fields, not something a
/// real turn's provider request would ever surface either).
#[tokio::test]
async fn sessions_interleave_without_cross_contamination() {
    let (config, resolver) = common::build_config_and_resolver();
    let (state_store, _state_tmp) = common::build_state_store();
    let (agent_task, mut client) = spawn_agent_over_ndjson(config, resolver, state_store);

    initialize(&mut client).await;

    let dir_a = tempfile::tempdir().expect("tempdir a");
    let dir_b = tempfile::tempdir().expect("tempdir b");

    let session_a_wire = session_new(&mut client, dir_a.path()).await;
    let session_b_wire = session_new(&mut client, dir_b.path()).await;
    assert_ne!(session_a_wire, session_b_wire, "session A and session B must have distinct ids");
    agent_task.abort();

    // Manager-level introspection (see test 2's doc comment for why).
    let (config2, resolver2) = common::build_config_and_resolver();
    let (state_store2, _state_tmp2) = common::build_state_store();
    let mut manager = AcpSessionManager::new(state_store2, config2, resolver2);

    let id_a = manager
        .create(dir_a.path().to_path_buf(), None)
        .await
        .expect("create session a");
    let id_b = manager
        .create(dir_b.path().to_path_buf(), None)
        .await
        .expect("create session b");
    assert_ne!(id_a, id_b, "session A and session B must have distinct ids on the manager too");

    // Operate on A AGAIN, after B has already been created on the same manager — A's own
    // identity/cwd must be untouched by B's insertion.
    let session_a_again = manager
        .get(&id_a)
        .expect("session A must still exist after session B's creation");
    assert_eq!(
        session_a_again.session_id, id_a,
        "session A's own id must be unchanged by session B's creation"
    );
    assert_eq!(
        session_a_again.cwd,
        dir_a.path(),
        "session A's own cwd must be unchanged by session B's creation"
    );

    let list = manager.list();
    assert_eq!(
        list,
        vec![id_a.clone(), id_b.clone()],
        "manager.list() must reflect BOTH sessions in creation order after interleaving"
    );
}

// ── Task 2: cross-session approval and cancellation containment ───────────────────────

/// Test 5 (approval containment, T-47.7-13, D-14): an `allow-always` grant recorded for
/// session A must not cause the same tool/arguments to be auto-approved in session B on the
/// same process.
///
/// Deviation from this file's otherwise-universal `spawn_agent_over_ndjson` wire harness,
/// noted per the plan's own instruction: `ApprovalsStore` is not observable through the ACP
/// wire protocol without driving a full LLM turn through a real tool-call + permission round
/// trip (the approach `execute_code_gate.rs`'s `allow_always_does_not_leak_into_a_new_session`
/// already takes, at the full turn level). `AcpSessionManager` OWNS `ApprovalsStore`
/// construction — `build_and_insert` builds a fresh `Arc::new(ApprovalsStore::new())` per
/// session — so this test drives that SAME production method (`AcpSessionManager::create`,
/// the exact function the wire-level `session/new` handler itself calls into) directly,
/// which is "the lowest level that genuinely proves it": a hand-assembled pair of
/// `AcpApprovalGate`s built off two independently-constructed `ApprovalsStore`s would NOT
/// prove the real wiring — it would pass even if `build_and_insert` were mutated to share one
/// store between sessions, which is exactly the defect this test's mutation check targets.
#[tokio::test]
async fn allow_always_grant_in_one_session_does_not_authorize_another_on_the_same_process() {
    let (config, resolver) = common::build_config_and_resolver();
    let (state_store, _state_tmp) = common::build_state_store();
    let mut manager = AcpSessionManager::new(state_store, config, resolver);

    let dir_a = tempfile::tempdir().expect("tempdir a");
    let dir_b = tempfile::tempdir().expect("tempdir b");

    let id_a = manager
        .create(dir_a.path().to_path_buf(), None)
        .await
        .expect("create session a");
    let id_b = manager
        .create(dir_b.path().to_path_buf(), None)
        .await
        .expect("create session b");

    let cache_key = format!(
        "terminal:{}",
        ironhermes_core::ApprovalsStore::normalize_command("curl https://x")
    );

    let approvals_a = manager
        .get(&id_a)
        .expect("session a must exist")
        .approvals
        .clone();
    approvals_a.approve_session(&cache_key).await;
    assert!(
        approvals_a.is_session_approved(&cache_key).await,
        "sanity: session A's own store must reflect its own grant"
    );

    let approvals_b = manager
        .get(&id_b)
        .expect("session b must exist")
        .approvals
        .clone();
    assert!(
        !approvals_b.is_session_approved(&cache_key).await,
        "an allow-always grant recorded in session A's ApprovalsStore must NOT be visible \
         from session B's store on the same process (T-47.7-13, D-14) — each session gets \
         its own fresh ApprovalsStore from AcpSessionManager::build_and_insert"
    );
}

/// Test 6 (cancellation containment, T-47.7-14, D-06): a `session/cancel` naming session A
/// fires ONLY session A's cancellation token — session B's token, live on the SAME shared
/// map (one process serving both sessions), is untouched. The multi-session form of D-06's
/// cancellation floor: on Buzz, another channel's conversation is sharing the process, so a
/// stray cancel must not take it down.
///
/// Manager-level — calls `handle_session_cancel` directly, mirroring `session_cancel.rs`'s
/// own `unknown_session_returns_error`/`no_in_flight_turn_is_a_noop` tests, extended to two
/// sessions — rather than driving a real in-flight turn over the wire, for a STRUCTURAL
/// reason specific to this exact property: `handlers::run_prompt_turn` holds
/// `AcpSessionManager`'s own `Arc<TokioMutex<..>>` for the FULL DURATION of an in-flight
/// `run_turn` (documented on `AcpSessionManager::cancel_tokens`'s own field doc), so turns
/// across every session on one process are strictly SERIALIZED — two sessions can never
/// both be genuinely "in-flight, actively selecting on their own live token" at the same
/// wall-clock instant. Every session's cancel token is unconditionally REPLACED with a
/// fresh one at the START of its own turn (T-36.8-16), so a token cancelled while a
/// session's turn was merely QUEUED (not yet started) is silently superseded once that turn
/// actually begins — an end-to-end "does B's LATER turn complete normally" assertion cannot
/// distinguish correct behavior from a `handle_session_cancel` that (incorrectly) trips
/// every token in the map, because by the time B's turn runs, B holds a token minted AFTER
/// the bad trip already happened. (Verified empirically while writing this test: an
/// end-to-end version of this test, built exactly that way, passed even under the "trip
/// every token" mutation below.) Direct token-state introspection is the only assertion
/// that genuinely proves this property.
#[tokio::test]
async fn a_cancel_naming_one_session_does_not_disturb_another_live_session() {
    let (config, resolver) = common::build_config_and_resolver();
    let (state_store, _state_tmp) = common::build_state_store();
    let mut manager = AcpSessionManager::new(state_store, config, resolver);

    let dir_a = tempfile::tempdir().expect("tempdir a");
    let dir_b = tempfile::tempdir().expect("tempdir b");

    let id_a = manager
        .create(dir_a.path().to_path_buf(), None)
        .await
        .expect("create session a");
    let id_b = manager
        .create(dir_b.path().to_path_buf(), None)
        .await
        .expect("create session b");

    let cancel_tokens = manager.cancel_tokens();

    {
        let tokens = cancel_tokens.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            !tokens.get(&id_a).expect("A token must exist").is_cancelled(),
            "sanity: session A's token must start uncancelled"
        );
        assert!(
            !tokens.get(&id_b).expect("B token must exist").is_cancelled(),
            "sanity: session B's token must start uncancelled"
        );
    }

    let result = ironhermes_acp::session_cancel::handle_session_cancel(
        agent_client_protocol::schema::v1::CancelNotification::new(id_a.clone()),
        cancel_tokens.clone(),
    )
    .await;
    assert!(result.is_ok(), "cancelling a live session must succeed: {result:?}");

    let tokens = cancel_tokens.lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        tokens.get(&id_a).expect("A token must exist").is_cancelled(),
        "session A's own cancel token must be tripped by a session/cancel naming A"
    );
    assert!(
        !tokens.get(&id_b).expect("B token must exist").is_cancelled(),
        "session B's cancel token must NOT be tripped by a session/cancel naming session A \
         only (T-47.7-14, D-06)"
    );
}
