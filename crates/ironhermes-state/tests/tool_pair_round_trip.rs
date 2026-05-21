//! Phase 25.1 GAP-7 round-trip regression tests.
//!
//! Locks three properties:
//!
//! 1. `state_store_round_trip_preserves_tool_pair_order_id_ordering` — a
//!    7-message browser-style conversation persisted via `add_message` and
//!    reloaded via `get_messages` reconstructs into a `Vec<ChatMessage>`
//!    that satisfies `validate_tool_call_pairing` across 5 iterations
//!    (proves the `ORDER BY id ASC` fix at lib.rs:468 keeps insert order
//!    deterministic regardless of same-millisecond timestamp ties).
//!
//! 2. `state_store_round_trip_preserves_parallel_tool_calls` — same
//!    coverage with the assistant emitting parallel tool_calls in one
//!    message followed by two tool results.
//!
//! 3. `state_store_get_messages_orders_by_id_not_timestamp` — directly
//!    forces a timestamp inversion via raw rusqlite UPDATE on the same DB
//!    file and asserts that `get_messages` returns insert (id) order, NOT
//!    timestamp order. This catches any regression that re-introduces
//!    `ORDER BY timestamp ASC`.

use ironhermes_core::{
    ChatMessage, FunctionCall, MessageContent, Role, ToolCall, validate_tool_call_pairing,
};
use ironhermes_state::{StateStore, StoredMessage};
use rusqlite::{Connection, params};
use tempfile::TempDir;

fn tc(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: name.to_string(),
            arguments: "{}".to_string(),
        },
    }
}

fn role_from_str(s: &str) -> Role {
    match s {
        "system" => Role::System,
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        other => panic!("unknown role string: {other}"),
    }
}

/// Reconstruct a `ChatMessage` from a `StoredMessage` row, including parsing
/// the JSON-encoded `tool_calls` field. This mirrors what the runtime
/// session-restore path must do (the StoredMessage layer keeps tool_calls
/// as a raw String; the agent re-deserializes when feeding the next turn).
fn stored_to_chat(stored: &StoredMessage) -> ChatMessage {
    let role = role_from_str(&stored.role);
    let content = stored
        .content
        .as_ref()
        .map(|s| MessageContent::Text(s.clone()));
    let tool_calls = stored
        .tool_calls
        .as_ref()
        .map(|s| serde_json::from_str::<Vec<ToolCall>>(s).expect("tool_calls JSON parse"));
    ChatMessage {
        role,
        content,
        tool_calls,
        tool_call_id: stored.tool_call_id.clone(),
        name: stored.tool_name.clone(),
        is_recall_context: false,
    }
}

fn open_store_in_tempdir() -> (TempDir, StateStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("state.db");
    let store = StateStore::new(&path).expect("StateStore::new");
    (dir, store)
}

#[test]
fn state_store_round_trip_preserves_tool_pair_order_id_ordering() {
    // 5-iteration outer loop: id-ordering should be deterministic per insert,
    // independent of same-tick timestamp ties.
    for iteration in 0..5 {
        let (_dir, mut store) = open_store_in_tempdir();
        let session_id = format!("sess-rt-{iteration}");
        store
            .create_session(&session_id, "test", None, None, None, None)
            .expect("create_session");

        // Build the production-shape browser conversation: two sequential
        // tool round-trips followed by a final assistant text message.
        let messages = vec![
            ChatMessage::system("you are a helpful agent"),
            ChatMessage::user("nav to example.com"),
            ChatMessage::assistant_tool_calls(vec![tc("call_A", "browser_navigate")]),
            ChatMessage::tool_result("call_A", r#"{"status":200}"#),
            ChatMessage::assistant_tool_calls(vec![tc("call_B", "browser_snapshot")]),
            ChatMessage::tool_result(
                "call_B",
                r#"{"snapshot":"<html><body>Example Domain ...~2KB...</body></html>"}"#,
            ),
            ChatMessage::assistant("Here's the page"),
        ];

        // Tight loop: NO sleep. Reproduces the same-millisecond-tick collision.
        for msg in &messages {
            store.add_message(&session_id, msg).expect("add_message");
        }

        // Round-trip: load and reconstruct.
        let stored = store.get_messages(&session_id).expect("get_messages");
        assert_eq!(
            stored.len(),
            messages.len(),
            "all messages persisted (iter={iteration})"
        );

        let reconstructed: Vec<ChatMessage> = stored.iter().map(stored_to_chat).collect();

        // Strict invariant must pass.
        validate_tool_call_pairing(&reconstructed)
            .unwrap_or_else(|e| panic!("invariant failed iter={iteration}: {e}"));

        // Order must match input EXACTLY (role + tool_call_id zip).
        for (i, (input, out)) in messages.iter().zip(reconstructed.iter()).enumerate() {
            assert_eq!(input.role, out.role, "role at idx {i} (iter={iteration})");
            assert_eq!(
                input.tool_call_id, out.tool_call_id,
                "tool_call_id at idx {i} (iter={iteration})"
            );
            let in_ids: Vec<&str> = input
                .tool_calls
                .as_deref()
                .map(|v| v.iter().map(|c| c.id.as_str()).collect())
                .unwrap_or_default();
            let out_ids: Vec<&str> = out
                .tool_calls
                .as_deref()
                .map(|v| v.iter().map(|c| c.id.as_str()).collect())
                .unwrap_or_default();
            assert_eq!(
                in_ids, out_ids,
                "tool_calls ids at idx {i} (iter={iteration})"
            );
        }
    }
}

#[test]
fn state_store_round_trip_preserves_parallel_tool_calls() {
    for iteration in 0..5 {
        let (_dir, mut store) = open_store_in_tempdir();
        let session_id = format!("sess-par-{iteration}");
        store
            .create_session(&session_id, "test", None, None, None, None)
            .expect("create_session");

        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("do two things"),
            ChatMessage::assistant_tool_calls(vec![tc("a", "tool_a"), tc("b", "tool_b")]),
            ChatMessage::tool_result("a", "ra"),
            ChatMessage::tool_result("b", "rb"),
            ChatMessage::assistant("done"),
        ];

        for msg in &messages {
            store.add_message(&session_id, msg).expect("add_message");
        }

        let stored = store.get_messages(&session_id).expect("get_messages");
        let reconstructed: Vec<ChatMessage> = stored.iter().map(stored_to_chat).collect();

        validate_tool_call_pairing(&reconstructed)
            .unwrap_or_else(|e| panic!("invariant failed iter={iteration}: {e}"));

        for (i, (input, out)) in messages.iter().zip(reconstructed.iter()).enumerate() {
            assert_eq!(input.role, out.role, "role at idx {i} (iter={iteration})");
            assert_eq!(
                input.tool_call_id, out.tool_call_id,
                "tool_call_id at idx {i} (iter={iteration})"
            );
        }
    }
}

#[test]
fn state_store_get_messages_orders_by_id_not_timestamp() {
    // Forces a divergence between id-order and timestamp-order, then
    // asserts the load path picks id-order. Catches any regression that
    // re-introduces `ORDER BY timestamp ASC`.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("state.db");
    let mut store = StateStore::new(&path).expect("StateStore::new");
    let session_id = "sess-inversion";
    store
        .create_session(session_id, "test", None, None, None, None)
        .expect("create_session");

    // Add three messages back-to-back. Capture insert order via the order
    // we write content_text — first/second/third.
    let m1 = ChatMessage::user("first");
    let m2 = ChatMessage::user("second");
    let m3 = ChatMessage::user("third");
    store.add_message(session_id, &m1).expect("add m1");
    store.add_message(session_id, &m2).expect("add m2");
    store.add_message(session_id, &m3).expect("add m3");

    // Open a sibling rusqlite connection on the same db file to force a
    // timestamp inversion: set the LAST-inserted row's timestamp to 0.0
    // (well before any other row). Under `ORDER BY timestamp ASC` this
    // would pull the third row to the FRONT of the result; under
    // `ORDER BY id ASC` (the GAP-7 fix) the result still respects insert
    // order.
    let raw = Connection::open(&path).expect("open sibling conn");
    raw.busy_timeout(std::time::Duration::from_millis(5000))
        .expect("busy_timeout");
    raw.execute(
        "UPDATE messages SET timestamp = 0.0 \
         WHERE id = (SELECT MAX(id) FROM messages WHERE session_id = ?1)",
        params![session_id],
    )
    .expect("force timestamp inversion");

    // Reload via the production read path.
    let stored = store.get_messages(session_id).expect("get_messages");
    assert_eq!(stored.len(), 3);

    // Confirm content reads in INSERT order, NOT timestamp order. Under
    // a timestamp-ordering regression the third row (timestamp=0.0)
    // would land at index 0.
    assert_eq!(
        stored[0].content.as_deref(),
        Some("first"),
        "id-ordering must place insert#1 at idx 0 (timestamps={}..{})",
        stored[0].timestamp,
        stored[2].timestamp,
    );
    assert_eq!(stored[1].content.as_deref(), Some("second"));
    assert_eq!(
        stored[2].content.as_deref(),
        Some("third"),
        "third (with forced timestamp=0.0) MUST stay at idx 2 under id-ordering"
    );

    // Sanity: id ascending.
    assert!(stored[0].id < stored[1].id);
    assert!(stored[1].id < stored[2].id);

    // Sanity: timestamp DOES diverge from id-order (the inversion is real).
    assert!(
        stored[2].timestamp < stored[0].timestamp,
        "test setup must produce a real timestamp inversion; got {} vs {}",
        stored[2].timestamp,
        stored[0].timestamp,
    );
}
