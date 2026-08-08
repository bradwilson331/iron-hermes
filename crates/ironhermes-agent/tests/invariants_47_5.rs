//! Phase 47.5 source guards — compile-time include_str!, offline, no I/O (same
//! convention as invariants_34b.rs).
//!
//! These are static presence/absence checks over the phase's three fix loci —
//! they complement (never replace) the behavioral suites shipped in
//! 47.5-01/02/03 (`session_reset_persistence.rs`,
//! `memory_recall_filters_irrelevant_results`,
//! `compression_does_not_pin_first_conversation_pair`). Accepted as
//! defense-in-depth (T-47.5-04-T1): a refactor that silently drops the
//! phase's wiring — or reintroduces a platform-specific patch (D-06) — fails
//! at `cargo test` time, offline, rather than only in production.

const COMPRESSOR_SOURCE: &str = include_str!("../src/context_compressor.rs");
const SESSION_SOURCE: &str = include_str!("../../ironhermes-gateway/src/session.rs");
const MEMORY_SQLITE_SOURCE: &str = include_str!("../../../providers/memory-sqlite/src/lib.rs");

/// 47.5-03 (D-05): the compressor protects only the leading SYSTEM-message
/// run, not a raw front-of-list message count — `system_prefix_len` is the
/// wired fix locus.
#[test]
fn compressor_protects_system_prefix_only() {
    assert!(
        COMPRESSOR_SOURCE.contains("system_prefix_len"),
        "context_compressor.rs must contain system_prefix_len (47.5-03 D-05 fix must be wired)"
    );
}

/// 47.5-01 (D-04): `/new` must be a DURABLE reset — `reset_session` ending
/// the session and clearing the route, not the old memory-only `remove`
/// path the review's consensus-HIGH finding replaced.
#[test]
fn session_reset_is_durable() {
    assert!(
        SESSION_SOURCE.contains("reset_session"),
        "session.rs must contain reset_session (47.5-01 D-04 fix must be wired)"
    );
    assert!(
        SESSION_SOURCE.contains("end_session"),
        "session.rs must call end_session as part of the durable reset"
    );
    assert!(
        SESSION_SOURCE.contains("delete_route"),
        "session.rs must call delete_route as part of the durable reset"
    );
    assert!(
        SESSION_SOURCE.contains("get_route"),
        "session.rs must call get_route to resolve the durable-only fallback \
         (post-restart /new, review consensus-HIGH finding)"
    );
    assert!(
        !SESSION_SOURCE.contains("pub fn remove("),
        "session.rs must NOT reintroduce a memory-only pub fn remove — \
         /new must always go through the durable reset_session path"
    );
}

/// 47.5-02 (D-03): sqlite memory recall must enforce a relevance-score floor.
#[test]
fn sqlite_recall_has_score_floor() {
    assert!(
        MEMORY_SQLITE_SOURCE.contains("recall_min_score"),
        "memory-sqlite/src/lib.rs must contain recall_min_score (47.5-02 D-03 fix must be wired)"
    );
}

/// D-06: the phase's fixes live on shared machinery, never behind a
/// platform-specific special case. A future `Platform::Telegram`-only patch
/// in any of the three fix loci fails this test at `cargo test` time.
#[test]
fn fix_loci_have_no_platform_special_casing() {
    let sources: [(&str, &str); 3] = [
        ("context_compressor.rs", COMPRESSOR_SOURCE),
        ("session.rs", SESSION_SOURCE),
        ("memory-sqlite/src/lib.rs", MEMORY_SQLITE_SOURCE),
    ];
    for (name, source) in sources {
        assert!(
            !source.contains("Platform::"),
            "{name} must not contain a Platform:: special case (D-06: fixes must \
             live on shared machinery, not a platform-specific patch)"
        );
    }
}
