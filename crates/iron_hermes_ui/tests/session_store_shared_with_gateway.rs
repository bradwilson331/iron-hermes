const STATE_SOURCE: &str = include_str!("../src/server/state.rs");

/// Locks the invariant that web UI sessions are keyed by `Platform::Web` in
/// the StateStore, per Phase 34 D-07/D-08. A future refactor that replaces
/// `Platform::Web.to_string()` with a bare `"web"` literal or a different
/// platform enum variant would silently break session isolation between
/// platforms — this test catches that at compile time / test time.
#[test]
fn web_session_keyed_by_platform_web() {
    let count = STATE_SOURCE.matches("Platform::Web").count();
    assert!(
        count >= 1,
        "D-07/D-08: iron_hermes_ui/src/server/state.rs must use Platform::Web \
         as the SessionStore key for web chat sessions. Found {count}. \
         See Phase 34 Plan 02."
    );
}
