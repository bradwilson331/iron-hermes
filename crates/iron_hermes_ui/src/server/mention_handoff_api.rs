//! Phase 50.2 Plan 07 (D-20/D-21, RESEARCH Pitfall 1 / Open Question 2):
//! the `@mention` handoff middleware.
//!
//! **This module implements deterministic host-orchestrated middleware —
//! NOT an agent-callable tool.** RESEARCH Pitfall 1 found that upstream has
//! TWO distinct bot-to-bot mechanisms: a *local* `@mention` handoff that is
//! agent-self-executed (a bot's own LLM is taught, via its SOUL.md, a
//! literal shell command to run itself in the background), and a
//! *host-orchestrated* group-chat/remote-mention delivery mechanism that
//! calls into a session and WAITS. The ROADMAP goal this plan closes says
//! the mention "waits for the reply and reports back", which is the
//! host-orchestrated shape — the one this module implements. Nothing here
//! injects a shell command into a SOUL.md or system prompt, and no new
//! LLM-invocable `message_bot` tool is added (RESEARCH Open Question 2):
//! `iron_hermes_ui` depends on `ironhermes-tools` and not the reverse, so an
//! agent-callable variant would require extracting `cli_handoff`'s whole
//! subprocess-isolation layer into a lower crate both crates could depend
//! on — a real refactor, foreclosed by this decision (recorded here as the
//! locked resolution RESEARCH asked the planner to make).
//!
//! **The one mention grammar.** [`resolve_roster_mentions`] calls
//! [`crate::server::group_chat_api::parse_group_mentions`] directly — the
//! SAME parser Phase 50.2 Plan 04 ported from `plugin.js` for the
//! group-chat responder-resolution path. No second regex is written here.
//!
//! **1:1 filtering differs from the room responder rule, deliberately.**
//! [`resolve_roster_mentions`] does NOT share
//! [`crate::server::group_chat_api::resolve_group_responders`]'s "unknown
//! handles fall back to everyone" rule. That fallback exists to keep a
//! ROOM from stalling on a typo (a message mentioning only outsiders is not
//! a silent no-op in a room where SOMEONE must respond). A 1:1 chat has no
//! room to keep alive — applying the same fallback here would turn an
//! operator's typo into an unintended broadcast to every roster bot. An
//! unknown handle, `@everyone`/`@all` (which addresses a ROOM, not a
//! chat), and `@user` (the escalation handle, never a target) are all
//! simply dropped.
//!
//! **Client-safe duplicate, not a client-reachable server fn (isomorphic
//! duplicate precedent).** [`resolve_roster_mentions`] calls
//! `parse_group_mentions`, which is `#[cfg(feature = "server")]` because it
//! depends on the `regex` crate — deliberately restricted to this crate's
//! native-only target dependencies (Phase 50.2 Plan 04) to keep it out of
//! the wasm client bundle. The three UI call sites this plan wires
//! (`bot_roster/chat_window.rs`, `bot_roster/group_chat_workspace.rs`,
//! `hermes_app/mod.rs`) all compile for the wasm client target too, so none
//! of them can call the server-gated fn directly. [`resolve_roster_mentions_client`]
//! is a regex-free, hand-rolled duplicate of the SAME decision logic
//! (mention scan + roster filter + speaker exclusion), compiled
//! unconditionally — the same "isomorphic duplicate" precedent
//! `bot_roster/chat_window.rs`'s `strip_think_blocks_for_display` and
//! `bot_roster/group_chat_workspace.rs`'s `round_band_is_settled` already
//! established in this crate for pure decision logic a wasm-compiled
//! component needs but cannot reach behind a `feature = "server"` gate.
//! [`mention_resolution_client_and_server_agree`] (test) proves the two
//! never drift.
//!
//! **`dispatch_mention_handoff` reuses the existing transport verbatim.**
//! Its blocking body is a single call to
//! [`crate::server::group_chat_api::dispatch_member_turn`] with session
//! title `Some("Bot Chat")` — the SAME seam a direct Bot Chat message uses
//! (`cli_handoff.rs`'s `dispatch_bot_message`), so a handoff and a direct
//! Bot Chat message land in the same persistent thread. This is a NEW
//! CALLER of that seam, never a re-derivation of it: no second env-scrub
//! allowlist, no second argv builder, no second reply sanitizer.

use dioxus::prelude::*;

/// Phase 50.2 Plan 07 (D-20): pure, I/O-free — resolves `@mention` handles
/// in `text` to actual roster members, filtered per the behavior this
/// module's doc comment describes. Delegates the mention GRAMMAR to
/// [`crate::server::group_chat_api::parse_group_mentions`] (the one parser
/// Phase 50.2 Plan 04 ported from `plugin.js`) — this fn writes no second
/// regex. `roster_names` should be every actual bot in the roster (e.g.
/// from `list_profiles`); `speaker` is the sender's own name, excluded from
/// its own mention (the crate's Bot Chat/live-Chat call sites pass
/// `"operator"`, which never collides with a roster name).
///
/// Order-preserving, de-duplicated, case-insensitive against `roster_names`.
/// `@everyone`/`@all` (a ROOM address, not a chat target), `@user` (the
/// escalation handle), an unknown handle, and the speaker's own name are
/// all dropped — never a fallback to "every roster bot" (see module doc for
/// why this differs from the group-chat responder-resolution rule).
// Phase 50.2 Plan 07 Task 1: this fn's first production caller
// (`dispatch_mention_handoff`, below) doesn't call it directly — Task 3
// wires the client-callable `resolve_roster_mentions_client` twin into the
// three UI surfaces, and THIS fn's own production role is being the tested
// source of truth `mention_resolution_client_and_server_agree` (below)
// checks the twin against. Same intra-plan staging `#[allow(dead_code)]`
// precedent as `group_chat_api.rs`'s Plan 04 pure fns.
#[allow(dead_code)]
#[cfg(feature = "server")]
pub(crate) fn resolve_roster_mentions(
    text: &str,
    roster_names: &[String],
    speaker: &str,
) -> Vec<String> {
    let mentions = crate::server::group_chat_api::parse_group_mentions(text);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for handle in &mentions.handles {
        if handle.eq_ignore_ascii_case(speaker) {
            continue;
        }
        if let Some(name) = roster_names
            .iter()
            .find(|n| n.eq_ignore_ascii_case(handle))
        {
            if seen.insert(name.clone()) {
                out.push(name.clone());
            }
        }
        // An unknown handle is dropped, never a fallback to everyone —
        // this fn's own module-doc rationale.
    }
    out
}

/// Phase 50.2 Plan 07: a regex-free scan for `@handle` tokens, mirroring
/// [`crate::server::group_chat_api::parse_group_mentions`]'s grammar
/// (`/@([a-z0-9][a-z0-9._-]*)/gi`) closely enough for THIS module's
/// purposes — case-folded to lowercase, a mention starts at `@` immediately
/// followed by an ASCII alphanumeric character, and continues through
/// further alphanumerics or `.`/`_`/`-`. Byte-indexed scanning is safe here
/// because every byte this fn inspects or slices on (`@`, ASCII
/// alphanumerics, `.`/`_`/`-`) is a single-byte ASCII character — a
/// multi-byte UTF-8 continuation byte can never equal any of them, so the
/// scan never lands mid-character.
#[allow(dead_code)] // see module doc: the client-only counterpart to the server-gated regex scan
fn scan_at_mention_handles(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut handles = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            let start = i + 1;
            if start < bytes.len() && (bytes[start] as char).is_ascii_alphanumeric() {
                let mut j = start + 1;
                while j < bytes.len() {
                    let c = bytes[j] as char;
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                        j += 1;
                    } else {
                        break;
                    }
                }
                handles.push(text[start..j].to_ascii_lowercase());
                i = j;
                continue;
            }
        }
        i += 1;
    }
    handles
}

/// Phase 50.2 Plan 07 (module doc: isomorphic duplicate precedent): the
/// client-compilable twin of [`resolve_roster_mentions`] — identical
/// decision logic (mention scan, `@everyone`/`@all`/`@user` skip, roster
/// filter, speaker exclusion, order-preserving de-duplication), but built
/// on [`scan_at_mention_handles`] instead of the server-gated regex parser,
/// so it compiles unconditionally (native AND wasm32). This is the fn the
/// three UI call sites (`bot_roster/chat_window.rs`,
/// `bot_roster/group_chat_workspace.rs`, `hermes_app/mod.rs`) actually
/// call — `resolve_roster_mentions` above stays the single source of truth
/// tested against `parse_group_mentions`'s own grammar, and
/// `mention_resolution_client_and_server_agree` (below) is the guard that
/// keeps this duplicate from silently drifting away from it.
// Phase 50.2 Plan 07 Task 1: staged ahead of Task 3's UI call sites — same
// intra-plan `#[allow(dead_code)]` precedent as `resolve_roster_mentions`
// above.
#[allow(dead_code)]
pub(crate) fn resolve_roster_mentions_client(
    text: &str,
    roster_names: &[String],
    speaker: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for handle in scan_at_mention_handles(text) {
        match handle.as_str() {
            "everyone" | "all" | "user" => continue,
            _ => {}
        }
        if handle.eq_ignore_ascii_case(speaker) {
            continue;
        }
        if let Some(name) = roster_names
            .iter()
            .find(|n| n.eq_ignore_ascii_case(&handle))
        {
            if seen.insert(name.clone()) {
                out.push(name.clone());
            }
        }
    }
    out
}

/// Phase 50.2 Plan 07 (D-20/D-04/D-05) / Phase 50.2 Plan 14 (G-50.2-2b):
/// dispatch ONE `@mention` handoff — one call per mentioned bot, following
/// [`crate::server::cli_handoff::dispatch_bot_message`]'s exact four-step
/// protocol (`Config::load` -> `check_profile_write_gate` -> the real work
/// -> error mapping). The real work awaits
/// [`crate::server::group_chat_api::dispatch_member_turn`] directly with
/// session title `Some("Bot Chat")` — the SAME seam a direct Bot Chat
/// message uses, so a handoff and a direct Bot Chat message land in the
/// SAME persistent thread, AND the same tracked registration Plan 14 wires
/// through that seam. `handoff_turn_registry()`'s non-panicking fallback is
/// exactly why the two end-to-end tokio tests below — which run with no
/// `AppState` initialised — keep passing unchanged. Never routes through
/// `dispatch_bot_message` itself (that fn is the roster composer's one-shot
/// shape) and never adds a second transport.
///
/// Phase 50.2 Plan 19 (CR-01): the `@mention` handoff's detached wrapper —
/// rides the SAME [`crate::server::cli_handoff::await_detached`] seam the
/// Bot Chat dispatch path uses, rather than a second hand-rolled spawn.
/// `target` is cloned before being moved into the `async move` block so the
/// join-failure arm can still name the mentioned bot in its returned pair.
/// The session title stays `Some("Bot Chat")` — unchanged from the inline
/// call this replaces — which is what makes a handoff and a direct Bot Chat
/// message land in the SAME persistent thread.
#[cfg(feature = "server")]
pub(crate) async fn dispatch_mention_handoff_detached(
    registry: ironhermes_core::TurnRegistry,
    target: String,
    message: String,
) -> (
    String,
    Result<crate::protocol::BotHandoffResult, crate::server::cli_handoff::BotHandoffError>,
) {
    let target_clone = target.clone();
    let fut = async move {
        crate::server::group_chat_api::dispatch_member_turn(
            registry,
            &target,
            &message,
            Some("Bot Chat"),
        )
        .await
    };
    match crate::server::cli_handoff::await_detached(fut).await {
        Ok(pair) => pair,
        Err(e) => (
            target_clone,
            Err(crate::server::cli_handoff::BotHandoffError::SpawnFailed {
                reason: format!("detached mention dispatch task: {e}"),
            }),
        ),
    }
}

/// A `BotHandoffError` maps to a `ServerFnError` carrying its `Display`
/// string — including `LiveProfileMustStream` when the operator mentions
/// the live bot, which is D-04's guard doing its job, not an edge case to
/// special-case away.
///
/// Phase 50.2 Plan 19 (CR-01): the real work no longer `.await`s
/// `dispatch_member_turn` inline inside this fn's own request future — it
/// hands off to [`dispatch_mention_handoff_detached`], which runs that call
/// on its own task via [`crate::server::cli_handoff::await_detached`] (the
/// SAME seam the Bot Chat dispatch path uses). A client disconnect can
/// still drop THIS fn's future, but the detached task keeps running, so the
/// mentioned bot's turn stays registered and its busy marker stays set
/// until the subprocess actually finishes or is explicitly cancelled.
#[server]
pub async fn dispatch_mention_handoff(
    req: crate::protocol::MentionHandoffRequest,
) -> Result<crate::protocol::MentionHandoffResult, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        let target = req.target;
        let message = req.message;
        let (name, result) = dispatch_mention_handoff_detached(
            crate::server::cli_handoff::handoff_turn_registry(),
            target,
            message,
        )
        .await;

        result
            .map(|handoff| crate::protocol::MentionHandoffResult {
                target: name,
                reply: handoff.reply,
                elapsed_ms: handoff.elapsed_ms,
            })
            .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = req;
        Err(ServerFnError::new(
            "dispatch_mention_handoff unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// RAII guard, duplicated from `cli_handoff.rs`'s / `group_chat_api.rs`'s
    /// own `ScopedEnv` — each `#[cfg(test)]` module is its own namespace.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; env_lock() below
            // serializes every test in this crate that mutates process env.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    /// Writes a `#!/bin/sh` stub at `dir/name`, `chmod +x` on unix.
    /// Duplicated from `cli_handoff.rs`'s own `write_stub_script`.
    fn write_stub_script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write stub script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("set_permissions 0755");
        }
        path
    }

    fn roster() -> Vec<String> {
        vec!["zig".to_string(), "ada".to_string()]
    }

    // -------------------------------------------------------------------
    // resolve_roster_mentions — behavior block
    // -------------------------------------------------------------------

    #[test]
    fn resolve_roster_mentions_single_target() {
        assert_eq!(
            resolve_roster_mentions("@zig ping", &roster(), "operator"),
            vec!["zig".to_string()]
        );
    }

    #[test]
    fn resolve_roster_mentions_two_targets_in_mention_order() {
        assert_eq!(
            resolve_roster_mentions("@zig and @ada", &roster(), "operator"),
            vec!["zig".to_string(), "ada".to_string()]
        );
    }

    #[test]
    fn resolve_roster_mentions_unknown_handle_drops_never_falls_back_to_everyone() {
        assert_eq!(
            resolve_roster_mentions("@nobody hi", &roster(), "operator"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn resolve_roster_mentions_everyone_addresses_a_room_not_a_chat() {
        assert_eq!(
            resolve_roster_mentions("@everyone hi", &roster(), "operator"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn resolve_roster_mentions_user_is_the_escalation_handle_never_a_target() {
        assert_eq!(
            resolve_roster_mentions("@user hi", &roster(), "operator"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn resolve_roster_mentions_no_mentions_returns_empty() {
        assert_eq!(
            resolve_roster_mentions("just a normal message", &roster(), "operator"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn resolve_roster_mentions_speaker_never_targets_itself() {
        assert_eq!(
            resolve_roster_mentions("@zig ping", &roster(), "zig"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn resolve_roster_mentions_duplicate_mention_deduplicates() {
        assert_eq!(
            resolve_roster_mentions("@zig @zig @zig", &roster(), "operator"),
            vec!["zig".to_string()]
        );
    }

    #[test]
    fn resolve_roster_mentions_mentioning_the_live_profile_is_not_filtered_here() {
        // The 1:1 resolver has no concept of "live" — that guard lives in
        // dispatch_member_turn -> run_bot_handoff (D-04). This asserts the
        // resolver itself passes a live-named handle straight through, so
        // the LiveProfileMustStream failure surfaces at dispatch time
        // rather than the mention silently vanishing at resolution time.
        let roster = vec!["scout".to_string()];
        assert_eq!(
            resolve_roster_mentions("@scout status", &roster, "operator"),
            vec!["scout".to_string()]
        );
    }

    // -------------------------------------------------------------------
    // parity with parse_group_mentions — the grammar cannot drift
    // -------------------------------------------------------------------

    #[test]
    fn resolve_roster_mentions_reuses_parse_group_mentions_grammar() {
        let text = "@zig and @ada, ignore @nobody and @everyone and @user";
        let direct = crate::server::group_chat_api::parse_group_mentions(text);
        let roster = roster();
        let resolved = resolve_roster_mentions(text, &roster, "operator");
        let expected: Vec<String> = direct
            .handles
            .iter()
            .filter(|h| roster.iter().any(|r| r.eq_ignore_ascii_case(h)))
            .cloned()
            .collect();
        assert_eq!(resolved, expected);
    }

    // -------------------------------------------------------------------
    // resolve_roster_mentions_client — never drifts from the server twin
    // -------------------------------------------------------------------

    #[test]
    fn mention_resolution_client_and_server_agree() {
        let roster = roster();
        for (text, speaker) in [
            ("@zig ping", "operator"),
            ("@zig and @ada", "operator"),
            ("@nobody hi", "operator"),
            ("@everyone hi", "operator"),
            ("@user hi", "operator"),
            ("just a normal message", "operator"),
            ("@zig ping", "zig"),
            ("@zig @zig @zig", "operator"),
            ("a@b.com mentions nobody real", "operator"),
        ] {
            assert_eq!(
                resolve_roster_mentions(text, &roster, speaker),
                resolve_roster_mentions_client(text, &roster, speaker),
                "client/server mention resolution diverged for {text:?}/{speaker:?}"
            );
        }
    }

    // -------------------------------------------------------------------
    // dispatch_mention_handoff (via the stub-worker path)
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn dispatch_mention_handoff_returns_sanitized_reply() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        std::fs::create_dir_all(
            crate::server::profile_api::profile_dir_for("zig").join("workspace"),
        )
        .expect("mkdir workspace");
        // Config::load() reads <IRONHERMES_HOME>/config.yaml fresh from disk
        // (never a test-injected Config value) — write the write-gate-open
        // record this #[server] fn's own check_profile_write_gate call
        // requires, following profile_api.rs's own precedent for exercising
        // a gated #[server] fn end to end rather than only its impl layer.
        std::fs::write(
            dir.path().join("config.yaml"),
            "security:\n  web_config_write_enabled: true\n",
        )
        .expect("write config.yaml");
        let stub = write_stub_script(dir.path(), "mention-stub.sh", "echo hello from zig");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let result = dispatch_mention_handoff(crate::protocol::MentionHandoffRequest {
            target: "zig".to_string(),
            message: "ping".to_string(),
        })
        .await
        .expect("dispatch_mention_handoff should succeed");

        assert_eq!(result.target, "zig");
        assert_eq!(result.reply, "hello from zig");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn dispatch_mention_handoff_failing_worker_yields_a_named_error_not_a_panic() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        std::fs::create_dir_all(
            crate::server::profile_api::profile_dir_for("zig").join("workspace"),
        )
        .expect("mkdir workspace");
        std::fs::write(
            dir.path().join("config.yaml"),
            "security:\n  web_config_write_enabled: true\n",
        )
        .expect("write config.yaml");
        let stub = write_stub_script(dir.path(), "mention-fail-stub.sh", "exit 3");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let result = dispatch_mention_handoff(crate::protocol::MentionHandoffRequest {
            target: "zig".to_string(),
            message: "ping".to_string(),
        })
        .await;

        assert!(result.is_err(), "a failing worker must yield Err, never panic");
    }

    // -------------------------------------------------------------------
    // dispatch_mention_handoff_detached (Phase 50.2 Plan 19, CR-01)
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dropping_the_mention_dispatch_future_keeps_the_turn_registered() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        std::fs::create_dir_all(
            crate::server::profile_api::profile_dir_for("zig").join("workspace"),
        )
        .expect("mkdir workspace");
        std::fs::write(
            dir.path().join("config.yaml"),
            "security:\n  web_config_write_enabled: true\n",
        )
        .expect("write config.yaml");
        let stub = write_stub_script(
            dir.path(),
            "mention-detached-drop-stub.sh",
            "sleep 5\necho 'should not be observed by the dropped future'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        // Resolve the SAME registry `dispatch_mention_handoff_detached` will
        // write into so this test can poll it independently.
        let registry_for_poll = crate::server::cli_handoff::handoff_turn_registry();

        // Simulated client disconnect: this caller's own future (standing
        // in for the HTTP request future axum/hyper would drop) is wrapped
        // in a short timeout and DROPPED when it elapses, well before the
        // stub's 5s sleep finishes.
        let dropped = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            dispatch_mention_handoff_detached(
                crate::server::cli_handoff::handoff_turn_registry(),
                "zig".to_string(),
                "ping".to_string(),
            ),
        )
        .await;
        assert!(
            dropped.is_err(),
            "the timeout must elapse — the stub's 5s sleep must still be running"
        );

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let observed = registry_for_poll.list_all().await;
        assert_eq!(
            observed.len(),
            1,
            "the mentioned bot's turn must still be registered after the caller's future was dropped"
        );
        assert_eq!(
            observed[0].session_id,
            crate::server::cli_handoff::handoff_turn_session_id("zig", Some("Bot Chat"))
        );

        match crate::server::handoff_steering::begin_turn_or_queue("zig", "second send") {
            Ok(crate::server::handoff_steering::BeginOrQueue::Queued { .. }) => {}
            other => panic!("the mentioned bot must still read as busy after the dropped future — got {other:?}"),
        }

        let turn_id = observed[0].turn_id;
        let cancelled = registry_for_poll.cancel_one(turn_id).await;
        assert!(cancelled, "cancel_one must find the surviving turn");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if registry_for_poll.list_all().await.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the registry must drain to empty within the bounded poll window after cancellation"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}
