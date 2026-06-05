//! Phase 36.17.7 Plan 04 — Wave 0 tests for ChatStreamEvent::AudioOut protocol.
//!
//! Locks:
//! - protocol.rs: `ChatStreamEvent::AudioOut { mime, uuid, bytes }` variant +
//!   the inline `test_audio_out_json_shape` test that pins the wire format.
//! - hermes_app/mod.rs: UNCONDITIONAL `Message::Binary` recv arm (REVISION HIGH 7)
//!   + `ChatStreamEvent::AudioOut` handler + `create_object_url_with_blob` Blob
//!   URL first-play (REVISION HIGH 4).
//!
//! Test 5 (`mod_rs_handles_audio_out_in_recv_loop`) stays RED until Task 5
//! ships all three markers into hermes_app/mod.rs.

use iron_hermes_ui::protocol::ChatStreamEvent;

const PROTOCOL_SOURCE: &str = include_str!("../src/protocol.rs");
const MOD_RS: &str = include_str!("../src/components/hermes_app/mod.rs");

// ─────────────────────────────────────────────────────────────────────────────
// D-02-a: protocol.rs AudioOut variant shape.
// ─────────────────────────────────────────────────────────────────────────────

/// Phase 36.17.7 D-02-a: protocol.rs must declare `ChatStreamEvent::AudioOut`.
#[test]
fn protocol_rs_declares_audio_out_variant() {
    assert!(
        PROTOCOL_SOURCE.contains("AudioOut"),
        "D-02-a: protocol.rs must declare the `AudioOut` variant on `ChatStreamEvent`"
    );
}

/// Phase 36.17.7 D-02-a: AudioOut must carry mime, uuid, bytes fields.
#[test]
fn protocol_rs_audio_out_has_mime_uuid_bytes_fields() {
    assert!(
        PROTOCOL_SOURCE.contains("mime: String"),
        "D-02-a: `AudioOut` must carry a `mime: String` field"
    );
    assert!(
        PROTOCOL_SOURCE.contains("uuid: String"),
        "D-02-a: `AudioOut` must carry a `uuid: String` field"
    );
    assert!(
        PROTOCOL_SOURCE.contains("bytes: Vec<u8>"),
        "D-02-a: `AudioOut` must carry a `bytes: Vec<u8>` field"
    );
}

/// Phase 36.17.7 D-02-a: protocol.rs inline shape test must exist.
#[test]
fn protocol_rs_audio_out_shape_test_exists() {
    assert!(
        PROTOCOL_SOURCE.contains("test_audio_out_json_shape"),
        "D-02-a: protocol.rs must contain inline `test_audio_out_json_shape` test"
    );
}

/// Phase 36.17.7 D-02-a/HIGH-4/HIGH-7: mod.rs must have UNCONDITIONAL
/// `Message::Binary` recv arm + `ChatStreamEvent::AudioOut` handler +
/// `create_object_url_with_blob` Blob URL first-play.
///
/// This test stays RED until Task 5 ships all three markers.
#[test]
fn mod_rs_handles_audio_out_in_recv_loop() {
    assert!(
        MOD_RS.contains("AudioOut"),
        "HIGH 7: hermes_app/mod.rs must handle `ChatStreamEvent::AudioOut` in recv loop"
    );
    assert!(
        MOD_RS.contains("Message::Binary"),
        "HIGH 7: hermes_app/mod.rs must have an UNCONDITIONAL `Message::Binary` recv arm"
    );
    assert!(
        MOD_RS.contains("create_object_url_with_blob"),
        "HIGH 4: hermes_app/mod.rs must construct Blob URL via `web_sys::Url::create_object_url_with_blob` for first-play"
    );
}

/// Phase 36.17.7 D-02-a: AudioOut serializes to external-tagged JSON and round-trips.
#[test]
fn audio_out_serializes_to_external_tagged_json() {
    let ev = ChatStreamEvent::AudioOut {
        mime: "audio/mpeg".into(),
        uuid: "x".into(),
        bytes: vec![0xFF, 0xFB],
    };
    let json = serde_json::to_string(&ev).expect("serialize AudioOut");
    assert!(
        json.starts_with(r#"{"AudioOut":"#),
        "D-02-a: AudioOut must serialize with external tagging (got {json})"
    );
    assert!(
        json.contains(r#""mime":"audio/mpeg""#),
        "D-02-a: AudioOut must serialize `mime` field (got {json})"
    );
    assert!(
        json.contains(r#""uuid":"x""#),
        "D-02-a: AudioOut must serialize `uuid` field (got {json})"
    );
    // Round-trip.
    let parsed: ChatStreamEvent = serde_json::from_str(&json).expect("deserialize AudioOut");
    assert!(
        matches!(parsed, ChatStreamEvent::AudioOut { .. }),
        "D-02-a: AudioOut must round-trip via serde_json"
    );
}
