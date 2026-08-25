//! Phase 50.2 Plan 15 (G-50.2-2a): the room-membership-update `#[server]`
//! surface — the missing server half of `50.2-UI-SPEC.md:316`'s `Edit
//! members` overflow item. Plan 06 shipped only `Delete room` (the
//! deferral is written into the code at
//! `group_chat_workspace.rs:611-613`) and no later plan picked the other
//! half up: no protocol DTO, no server fn, no store mutation existed for
//! changing an EXISTING room's roster.
//!
//! A small, single-concern sibling of `group_settings_api.rs`, chosen over
//! growing the 2000-line `group_chat_api.rs` (concurrently owned by this
//! gap round's in-flight-turns plan in the same wave). This module defines
//! NO second member validator and NO second persistence path — it calls
//! straight through to `group_chat_store::update_room_members_impl`, which
//! itself calls the single `group_chat_store::validate_room_members` this
//! plan extracted from `create_room_impl`.
//!
//! **No UI change lands in this plan.** `50.2-17` owns the room header's
//! `Edit members` control and depends on this module's server fn.

use dioxus::prelude::*;

/// Phase 50.2 Plan 15 (G-50.2-2a): change an existing room's membership.
/// Follows the crate's four-step write protocol byte for byte
/// (`create_group_room`, `group_chat_api.rs:790-817`): fresh
/// `Config::load()` → `profile_api::check_profile_write_gate` (fails
/// closed) → `spawn_blocking` around
/// `group_chat_store::update_room_members_impl` → map the join error and
/// then the domain error to `ServerFnError::new`. A membership change takes
/// effect on the NEXT dispatched round — `run_group_rounds_with_settings`
/// re-reads `transcript.room.members` at the start of every drive, so no
/// orchestration change is needed for this update to be observed.
#[server]
pub async fn update_group_room_members(
    req: crate::protocol::UpdateGroupRoomMembersRequest,
) -> Result<crate::protocol::GroupRoom, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        tokio::task::spawn_blocking(move || {
            crate::server::group_chat_store::update_room_members_impl(&req.room_id, &req.members)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = req;
        Err(ServerFnError::new(
            "update_group_room_members unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// RAII guard that sets/removes an env var and restores the previous
    /// value on drop. Duplicated from `group_chat_store.rs`'s own
    /// `ScopedEnv` — each `#[cfg(test)]` module is its own namespace, the
    /// crate's own sanctioned "duplicate the guard" precedent.
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

    fn scaffold_profile(name: &str) {
        std::fs::create_dir_all(crate::server::profile_api::profile_dir_for(name))
            .expect("mkdir profile dir");
    }

    fn write_write_gate_config(home: &std::path::Path, enabled: bool) {
        std::fs::write(
            home.join("config.yaml"),
            format!("security:\n  web_config_write_enabled: {enabled}\n"),
        )
        .expect("write config.yaml");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn update_group_room_members_changes_membership_on_both_read_paths() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        scaffold_profile("scout");
        scaffold_profile("zig");
        scaffold_profile("ada");
        write_write_gate_config(dir.path(), true);

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed (G-50.2-2a fixture setup)");

        let updated = update_group_room_members(crate::protocol::UpdateGroupRoomMembersRequest {
            room_id: room.id.clone(),
            members: vec!["scout".to_string(), "zig".to_string(), "ada".to_string()],
        })
        .await
        .expect("update_group_room_members should succeed (G-50.2-2a)");

        assert_eq!(
            updated.members,
            vec!["scout".to_string(), "zig".to_string(), "ada".to_string()],
            "G-50.2-2a: the returned room must carry the new membership"
        );

        let header_view = crate::server::group_chat_store::load_room_impl(&room.id)
            .expect("load_room_impl should succeed (G-50.2-2a header read path)");
        assert_eq!(
            header_view.room.members,
            vec!["scout".to_string(), "zig".to_string(), "ada".to_string()],
            "G-50.2-2a: the header's read path (transcript copy) must report the new membership"
        );

        let roster_view = crate::server::group_chat_store::list_rooms_impl()
            .expect("list_rooms_impl should succeed (G-50.2-2a roster read path)");
        let roster_row = roster_view
            .iter()
            .find(|r| r.id == room.id)
            .expect("updated room must still be present in the roster");
        assert_eq!(
            roster_row.members,
            vec!["scout".to_string(), "zig".to_string(), "ada".to_string()],
            "G-50.2-2a: the roster's read path (index copy) must agree with the header's"
        );
    }

    /// Phase 50.2 Plan 15 (G-50.2-2a): fail-closed must be observable in
    /// the STORE, not only in the return value — the same discipline
    /// `mention_handoff_api.rs`'s write-gate tests use.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn update_group_room_members_is_refused_when_the_write_gate_is_closed() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        scaffold_profile("scout");
        scaffold_profile("zig");
        scaffold_profile("ada");
        write_write_gate_config(dir.path(), false);

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed (G-50.2-2a fixture setup)");

        let result = update_group_room_members(crate::protocol::UpdateGroupRoomMembersRequest {
            room_id: room.id.clone(),
            members: vec!["scout".to_string(), "zig".to_string(), "ada".to_string()],
        })
        .await;
        assert!(
            result.is_err(),
            "G-50.2-2a: the write gate must refuse the call when closed"
        );

        let header_view = crate::server::group_chat_store::load_room_impl(&room.id)
            .expect("load_room_impl should succeed");
        assert_eq!(
            header_view.room.members,
            vec!["scout".to_string(), "zig".to_string()],
            "G-50.2-2a: fail-closed must be observable in the store, not only in the return value"
        );
    }
}
