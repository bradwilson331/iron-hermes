// Phase 36.17.8 Plan 06 (D-13) — AudioInFrame inbound protocol tests.
//
// NOTE: `iron_hermes_ui` is a binary crate (no `[lib]` target), so integration
// tests cannot `use iron_hermes_ui::...`. These are all source-string
// assertions (project convention — see audio_out_protocol.rs and
// running_agent_guard_web_tests.rs for precedent).
//
// The AudioInFrame serde round-trip is unit-tested inside `protocol.rs` itself
// (`test_audio_in_frame_round_trip`), which `protocol_rs_audio_in_shape_test_exists`
// below asserts is present.
//
// Run: `cargo test -p iron_hermes_ui --test audio_in_protocol`

const PROTOCOL_SOURCE: &str = include_str!("../src/protocol.rs");

// ─────────────────────────────────────────────────────────────────────────────
// D-13: protocol.rs AudioInFrame struct shape.
// ─────────────────────────────────────────────────────────────────────────────

/// D-13: protocol.rs must declare `pub struct AudioInFrame` as a standalone struct.
#[test]
fn protocol_rs_declares_audio_in_frame_struct() {
    assert!(
        PROTOCOL_SOURCE.contains("pub struct AudioInFrame"),
        "D-13: protocol.rs must declare `pub struct AudioInFrame`"
    );
}

/// D-13: AudioInFrame must carry session_id, mime, and bytes fields.
#[test]
fn protocol_rs_audio_in_frame_has_session_id_mime_bytes() {
    assert!(
        PROTOCOL_SOURCE.contains("pub session_id: String"),
        "D-13: AudioInFrame must carry `pub session_id: String`"
    );
    assert!(
        PROTOCOL_SOURCE.contains("pub mime: String"),
        "D-13: AudioInFrame must carry `pub mime: String`"
    );
    // bytes is plain Vec<u8> — no serde_bytes, matching AudioOut
    assert!(
        PROTOCOL_SOURCE.contains("pub bytes: Vec<u8>"),
        "D-13: AudioInFrame must carry `pub bytes: Vec<u8>` (no serde_bytes)"
    );
}

/// D-13 acceptance gate: AudioInFrame is a SEPARATE struct, NOT a
/// `ChatStreamEvent::AudioIn` variant (RESEARCH Pitfall 6 / Assumption A2).
#[test]
fn protocol_rs_audio_in_frame_is_not_a_chat_stream_event_variant() {
    assert!(
        !PROTOCOL_SOURCE.contains("AudioIn {"),
        "D-13: AudioIn must NOT be a ChatStreamEvent variant — found 'AudioIn {{' in protocol.rs"
    );
    assert!(
        !PROTOCOL_SOURCE.contains("ChatStreamEvent::AudioIn"),
        "D-13: ChatStreamEvent::AudioIn must not exist — found in protocol.rs"
    );
}

/// D-13: protocol.rs must contain an inline `test_audio_in_frame_round_trip` test.
#[test]
fn protocol_rs_audio_in_frame_round_trip_test_exists() {
    assert!(
        PROTOCOL_SOURCE.contains("test_audio_in_frame_round_trip"),
        "D-13: protocol.rs must contain inline `test_audio_in_frame_round_trip` test"
    );
}
