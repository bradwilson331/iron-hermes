//! Phase 37.2 Plan 01 — turn-end delivery RED tests.
//!
//! Covers:
//!   REQ-37.2-01: empty stream + non-empty `final_response` → edit placeholder with escaped content
//!   REQ-37.2-02: empty stream + no `final_response` → delete placeholder (no bare `█`)
//!   REQ-37.2-06: structured turn-end tracing record (content verified via escaped delivery)
//!
//! ## RED Status
//!
//! These tests are INTENTIONALLY RED at the end of Plan 01.
//!
//! They reference `ironhermes_gateway::deliver_turn_end_fallback`, a function that Plan 02
//! must extract from `handler.rs`. Until Plan 02 adds that symbol, this file produces a
//! **compile-error RED** state. The exact symbol Plan 02 must provide:
//!
//! ```rust
//! // crates/ironhermes-gateway/src/lib.rs (or stream_consumer_fallback.rs)
//! pub async fn deliver_turn_end_fallback(
//!     adapter: &dyn ironhermes_gateway::adapter::PlatformAdapter,
//!     chat_id: &str,
//!     placeholder_id: &str,
//!     final_body: &str,
//!     final_response: Option<&str>,
//! ) -> anyhow::Result<()>
//! ```
//!
//! Plan 02 Task 2 extracts this helper and makes both tests GREEN.

#![allow(clippy::expect_used)]
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_core::{MessageResponse, Platform};
use ironhermes_gateway::adapter::PlatformAdapter;
use ironhermes_gateway::markdown_v2::escape_outside_code_blocks;

// ===========================================================================
// RecordingAdapter — records PlatformAdapter calls for assertions
// ===========================================================================
//
// Extends the MockAdapter shape from stream_consumer.rs (in-module) with a
// `DeleteMessage` variant so REQ-37.2-02 (delete placeholder) can be asserted.

#[derive(Debug)]
#[allow(dead_code)]
enum AdapterCall {
    EditMessage {
        chat_id: String,
        message_id: String,
        content: String,
    },
    EditMessageMarkdownV2 {
        chat_id: String,
        message_id: String,
        content: String,
    },
    SendMessage {
        chat_id: String,
        content: String,
    },
    DeleteMessage {
        chat_id: String,
        message_id: String,
    },
}

struct RecordingAdapter {
    calls: Arc<Mutex<Vec<AdapterCall>>>,
    next_message_id: Arc<Mutex<String>>,
}

impl RecordingAdapter {
    fn new() -> (Arc<Self>, Arc<Mutex<Vec<AdapterCall>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let adapter = Arc::new(RecordingAdapter {
            calls: calls.clone(),
            next_message_id: Arc::new(Mutex::new("msg-2".to_string())),
        });
        (adapter, calls)
    }
}

#[async_trait]
impl PlatformAdapter for RecordingAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        let id = self.next_message_id.lock().unwrap().clone();
        self.calls.lock().unwrap().push(AdapterCall::SendMessage {
            chat_id: chat_id.to_string(),
            content: content.to_string(),
        });
        Ok(MessageResponse {
            message_id: id,
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn edit_message(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(AdapterCall::EditMessage {
            chat_id: chat_id.to_string(),
            message_id: message_id.to_string(),
            content: content.to_string(),
        });
        Ok(())
    }

    async fn edit_message_markdown_v2(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(AdapterCall::EditMessageMarkdownV2 {
                chat_id: chat_id.to_string(),
                message_id: message_id.to_string(),
                content: content.to_string(),
            });
        Ok(())
    }

    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        _content: &str,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        let id = self.next_message_id.lock().unwrap().clone();
        Ok(MessageResponse {
            message_id: id,
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn delete_message(&self, chat_id: &str, message_id: &str) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(AdapterCall::DeleteMessage {
            chat_id: chat_id.to_string(),
            message_id: message_id.to_string(),
        });
        Ok(())
    }

    fn is_running(&self) -> bool {
        true
    }
}

// ===========================================================================
// RED tests — REQ-37.2-01 / REQ-37.2-02 / REQ-37.2-06
//
// These tests call `ironhermes_gateway::deliver_turn_end_fallback`, which does
// not exist until Plan 02. This file therefore produces a **compile-error RED**
// state (symbol not found). Plan 02 Task 2 must export this function from the
// ironhermes-gateway crate to make both tests GREEN.
//
// Expected Plan 02 export path:
//   ironhermes_gateway::deliver_turn_end_fallback(adapter, chat_id, placeholder_id,
//                                                  final_body, final_response)
// ===========================================================================

/// REQ-37.2-01: empty stream + non-empty `final_response` edits the placeholder.
///
/// Asserts:
/// 1. Exactly one `EditMessageMarkdownV2` is recorded.
/// 2. Its `content` equals `escape_outside_code_blocks("answer")` (REQ-37.2-06 — delivery
///    uses the escaped final_response content, not raw text).
/// 3. No `DeleteMessage` is recorded.
///
/// RED until Plan 02 extracts `deliver_turn_end_fallback`.
#[tokio::test]
async fn empty_stream_with_final_response_edits_placeholder() {
    let (adapter, calls) = RecordingAdapter::new();

    // Simulate: stream produced no text (final_body = ""), but result.final_response = Some("answer")
    ironhermes_gateway::deliver_turn_end_fallback(
        adapter.as_ref(),
        "chat1",
        "msg-placeholder",
        "",             // final_body: empty stream
        Some("answer"), // final_response: Some
    )
    .await
    .expect("deliver_turn_end_fallback must not error");

    let calls = calls.lock().unwrap();

    // Assert exactly one EditMessageMarkdownV2 — no DeleteMessage
    let edit_calls: Vec<_> = calls
        .iter()
        .filter(|c| matches!(c, AdapterCall::EditMessageMarkdownV2 { .. }))
        .collect();
    let delete_calls: Vec<_> = calls
        .iter()
        .filter(|c| matches!(c, AdapterCall::DeleteMessage { .. }))
        .collect();

    assert_eq!(
        edit_calls.len(),
        1,
        "REQ-37.2-01: exactly one EditMessageMarkdownV2 expected (got {})",
        edit_calls.len()
    );
    assert_eq!(
        delete_calls.len(),
        0,
        "REQ-37.2-01: no DeleteMessage expected when final_response is present"
    );

    // REQ-37.2-06: content must be the escaped final_response
    if let AdapterCall::EditMessageMarkdownV2 {
        content,
        message_id,
        ..
    } = &edit_calls[0]
    {
        let expected = escape_outside_code_blocks("answer");
        assert_eq!(
            content, &expected,
            "REQ-37.2-06: fallback must escape final_response before editing placeholder"
        );
        assert_eq!(
            message_id, "msg-placeholder",
            "REQ-37.2-01: must edit the placeholder message, not a new message"
        );
    }
}

/// REQ-37.2-02: empty stream AND no `final_response` → delete the placeholder.
///
/// Asserts:
/// 1. Exactly one `DeleteMessage` is recorded for the placeholder id.
/// 2. No `EditMessageMarkdownV2` is recorded (no bare `█` left behind).
///
/// RED until Plan 02 extracts `deliver_turn_end_fallback`.
#[tokio::test]
async fn empty_stream_no_final_response_deletes_placeholder() {
    let (adapter, calls) = RecordingAdapter::new();

    // Simulate: stream empty, final_response = None (tool-only turn)
    ironhermes_gateway::deliver_turn_end_fallback(
        adapter.as_ref(),
        "chat1",
        "msg-placeholder",
        "",   // final_body: empty stream
        None, // final_response: None
    )
    .await
    .expect("deliver_turn_end_fallback must not error");

    let calls = calls.lock().unwrap();

    let delete_calls: Vec<_> = calls
        .iter()
        .filter(|c| matches!(c, AdapterCall::DeleteMessage { .. }))
        .collect();
    let edit_calls: Vec<_> = calls
        .iter()
        .filter(|c| matches!(c, AdapterCall::EditMessageMarkdownV2 { .. }))
        .collect();

    assert_eq!(
        delete_calls.len(),
        1,
        "REQ-37.2-02: exactly one DeleteMessage expected for empty turn (got {})",
        delete_calls.len()
    );
    assert_eq!(
        edit_calls.len(),
        0,
        "REQ-37.2-02: no EditMessageMarkdownV2 expected when both stream and final_response are empty"
    );

    if let AdapterCall::DeleteMessage { message_id, .. } = &delete_calls[0] {
        assert_eq!(
            message_id, "msg-placeholder",
            "REQ-37.2-02: must delete the placeholder message id"
        );
    }
}
