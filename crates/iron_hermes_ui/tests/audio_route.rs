//! Phase 36.17.7 Plan 05 Wave 0 — `GET /audio/:uuid` source-grep guards.
//!
//! These guards lock the T-path-traversal mitigations on the audio replay
//! HTTP route shipped by Task 2. All checks are source-grep — they fail
//! cleanly if Task 2's implementation drops a mitigation literal.

#![cfg(feature = "server")]

const SOURCE: &str = include_str!("../src/server/audio_route.rs");

#[test]
fn audio_route_declared_as_get_handler() {
    assert!(
        SOURCE.contains("/audio/:uuid"),
        "D-02-c: audio_route.rs must declare the `/audio/:uuid` route literal"
    );
}

#[test]
fn audio_route_validates_uuid_format() {
    assert!(
        SOURCE.contains("Uuid::parse_str") || SOURCE.contains("Uuid::from_str"),
        "T-path-traversal mitigation (a): audio_route.rs must validate the uuid \
         path segment with `Uuid::parse_str` or `Uuid::from_str` before any \
         filesystem access"
    );
}

#[test]
fn audio_route_canonicalizes_and_checks_containment() {
    assert!(
        SOURCE.contains("canonicalize"),
        "T-path-traversal mitigation (b): audio_route.rs must call \
         `.canonicalize()` on the resolved path"
    );
    assert!(
        SOURCE.contains("starts_with"),
        "T-path-traversal mitigation (c): audio_route.rs must guard the \
         canonical path with `.starts_with(audio_cache_dir)`"
    );
}

#[test]
fn audio_route_returns_audio_mpeg_content_type() {
    assert!(
        SOURCE.contains("audio/mpeg"),
        "D-02-e: audio_route.rs must serve `Content-Type: audio/mpeg`"
    );
}

#[test]
fn audio_route_no_unwrap_or_panic() {
    assert!(
        !SOURCE.contains(".unwrap()"),
        "audio_route.rs must not use `.unwrap()` — return BAD_REQUEST/NOT_FOUND/FORBIDDEN \
         responses instead"
    );
    assert!(
        !SOURCE.contains("panic!"),
        "audio_route.rs must not use `panic!` — return BAD_REQUEST/NOT_FOUND/FORBIDDEN \
         responses instead"
    );
}
