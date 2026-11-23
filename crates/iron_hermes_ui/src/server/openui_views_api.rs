//! Phase 54.9 Plan 10 (D-14, D-16): saved OpenUI views, one JSON file per view under the
//! ACTIVE profile's home (`<profile home>/openui/views/<uuid>.json`).
//!
//! The home is resolved per call from the persisted activation record
//! (`resolve_active_profile_for(ActivationSurface::Chat, None)`), never from
//! `ironhermes_core::current_profile()`, which always reads `default` in this never-pivoted
//! UI server (the inertness trap documented above `api.rs::resolve_chat_optout_target`).
//! ADR-0007 "Saved views" correction: a slug equal to `current_profile()` (or the `default`
//! sentinel) maps to `get_hermes_home()` itself, because `profile_dir_for(current_profile())`
//! would nest a profile home under itself; any other slug is validated and joined by
//! `profile_dir_for`. An invalid slug is refused, never redirected to the root home.
//!
//! A view is a snapshot (D-16): the text the agent produced plus the prompt that made it.
//! Nothing here regenerates a view on a schedule. The struct is plain data so a later board
//! (SEED-003) can add geometry fields beside it; unknown fields in a file are ignored.
//!
//! `#[server]` fns resolve the directory once per call inside `spawn_blocking` and pass it to
//! the path-addressed impls, so an id from another profile resolves to not-found.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// One saved view. Times are unix seconds.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OpenUiView {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub snapshot: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[cfg(feature = "server")]
mod store {
    use super::*;
    use std::path::{Path, PathBuf};

    /// At most this many saved views per profile (T-54.9-26).
    pub(crate) const OPENUI_VIEWS_MAX: usize = 200;
    /// Largest snapshot (and prompt) accepted, in bytes (T-54.9-26).
    pub(crate) const OPENUI_SNAPSHOT_MAX_BYTES: usize = 256 * 1024;
    /// Names are truncated to this many characters.
    pub(crate) const OPENUI_VIEW_NAME_MAX: usize = 80;

    fn now_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// The profile home for one already-resolved profile slug (ADR-0007; see the module doc for
    /// the home rule). Pure apart from the `current_profile()` / home env reads. Shared with the
    /// Phase 54.10 dashboard board store.
    pub(crate) fn profile_home_for_slug(slug: &str) -> Result<PathBuf, String> {
        if slug == "default" || slug == ironhermes_core::current_profile() {
            Ok(ironhermes_core::get_hermes_home())
        } else {
            let valid = ironhermes_core::profile::validate_profile_name(slug)
                .map_err(|_| "invalid active profile name".to_string())?;
            Ok(crate::server::profile_api::profile_dir_for(&valid))
        }
    }

    /// The ACTIVE profile's home. Loads config.yaml, so call from `spawn_blocking`.
    pub(crate) fn active_profile_home() -> Result<PathBuf, String> {
        let slug = crate::server::profile_activation_api::resolve_active_profile_for(
            crate::server::profile_activation_api::ActivationSurface::Chat,
            None,
        );
        profile_home_for_slug(&slug)
    }

    /// The views directory for one already-resolved profile slug (test-only: runtime callers
    /// resolve the active profile through `views_dir`).
    #[cfg(test)]
    pub(crate) fn views_dir_for_slug(slug: &str) -> Result<PathBuf, String> {
        Ok(profile_home_for_slug(slug)?.join("openui").join("views"))
    }

    /// The ACTIVE profile's views directory. Loads config.yaml, so call from `spawn_blocking`.
    pub(crate) fn views_dir() -> Result<PathBuf, String> {
        Ok(active_profile_home()?.join("openui").join("views"))
    }

    fn valid_id(id: &str) -> Result<(), String> {
        uuid::Uuid::parse_str(id)
            .map(|_| ())
            .map_err(|_| "Invalid view id".to_string())
    }

    fn file_for(dir: &Path, id: &str) -> Result<PathBuf, String> {
        valid_id(id)?;
        Ok(dir.join(format!("{id}.json")))
    }

    /// Temp file + rename, like `Config::save_to`.
    pub(crate) fn write_view(dir: &Path, view: &OpenUiView) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("Could not save view: {e}"))?;
        let path = file_for(dir, &view.id)?;
        // Unique temp name: concurrent writes of one view never share (and truncate) a file.
        let tmp = dir.join(format!("{}.{}.tmp", view.id, uuid::Uuid::new_v4()));
        let json =
            serde_json::to_vec_pretty(view).map_err(|e| format!("Could not save view: {e}"))?;
        let result = std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, &path));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result.map_err(|e| format!("Could not save view: {e}"))
    }

    /// Newest first. Unreadable, non-JSON and non-UUID-named files are skipped, never fatal.
    pub(crate) fn list_views_at(dir: &Path) -> Vec<OpenUiView> {
        let mut views: Vec<OpenUiView> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let stem = path.file_stem()?.to_str()?.to_string();
                if path.extension()? != "json" || valid_id(&stem).is_err() {
                    return None;
                }
                let view: OpenUiView = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
                (view.id == stem).then_some(view)
            })
            .collect();
        views.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        views
    }

    pub(crate) fn load_view_at(dir: &Path, id: &str) -> Result<OpenUiView, String> {
        let bytes = std::fs::read(file_for(dir, id)?).map_err(|_| "View not found".to_string())?;
        serde_json::from_slice(&bytes).map_err(|_| "View not found".to_string())
    }

    fn check_snapshot(snapshot: &str) -> Result<(), String> {
        if snapshot.trim().is_empty() {
            return Err("Nothing to save".to_string());
        }
        if snapshot.len() > OPENUI_SNAPSHOT_MAX_BYTES {
            return Err("View is too large to save".to_string());
        }
        Ok(())
    }

    pub(crate) fn save_view_at(
        dir: &Path,
        name: &str,
        prompt: &str,
        snapshot: &str,
    ) -> Result<OpenUiView, String> {
        insert_view(dir, name, prompt, snapshot, true)
    }

    /// Phase 54.10 D-02: a view whose prompt is saved but whose snapshot is not generated yet,
    /// so a dashboard widget always has a valid `view_id`. Same caps as [`save_view_at`]; only
    /// the empty snapshot is allowed (normal saves still refuse it).
    pub(crate) fn save_pending_view_at(
        dir: &Path,
        name: &str,
        prompt: &str,
    ) -> Result<OpenUiView, String> {
        insert_view(dir, name, prompt, "", false)
    }

    fn insert_view(
        dir: &Path,
        name: &str,
        prompt: &str,
        snapshot: &str,
        require_snapshot: bool,
    ) -> Result<OpenUiView, String> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err("A saved view needs its prompt".to_string());
        }
        if prompt.len() > OPENUI_SNAPSHOT_MAX_BYTES {
            return Err("Prompt is too long to save".to_string());
        }
        if require_snapshot {
            check_snapshot(snapshot)?;
        }
        if list_views_at(dir).len() >= OPENUI_VIEWS_MAX {
            return Err("Saved view limit reached".to_string());
        }
        let name = match name.trim() {
            "" => prompt.chars().take(40).collect::<String>(),
            n => n.chars().take(OPENUI_VIEW_NAME_MAX).collect(),
        };
        let now = now_secs();
        let view = OpenUiView {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            prompt: prompt.to_string(),
            snapshot: snapshot.to_string(),
            created_at: now,
            updated_at: now,
        };
        write_view(dir, &view)?;
        Ok(view)
    }

    /// D-16: a successful Re-run replaces only the snapshot.
    pub(crate) fn replace_snapshot_at(
        dir: &Path,
        id: &str,
        snapshot: &str,
    ) -> Result<OpenUiView, String> {
        check_snapshot(snapshot)?;
        let mut view = load_view_at(dir, id)?;
        view.snapshot = snapshot.to_string();
        view.updated_at = now_secs();
        write_view(dir, &view)?;
        Ok(view)
    }

    pub(crate) fn delete_view_at(dir: &Path, id: &str) -> Result<(), String> {
        std::fs::remove_file(file_for(dir, id)?).map_err(|_| "View not found".to_string())
    }

    /// Run a store op against the active profile's directory off the async runtime.
    pub(crate) async fn with_active_dir<T: Send + 'static>(
        f: impl FnOnce(PathBuf) -> Result<T, String> + Send + 'static,
    ) -> Result<T, ServerFnError> {
        tokio::task::spawn_blocking(move || views_dir().and_then(f))
            .await
            .map_err(|e| ServerFnError::new(format!("join error: {e}")))?
            .map_err(ServerFnError::new)
    }
}

#[cfg(feature = "server")]
pub(crate) use store::*;

/// The active profile's saved views, newest first.
#[server]
pub async fn list_openui_views() -> Result<Vec<OpenUiView>, ServerFnError> {
    with_active_dir(|dir| Ok(list_views_at(&dir))).await
}

/// Save a generated view. An empty `name` defaults to the first 40 characters of the prompt.
#[server]
pub async fn save_openui_view(
    name: String,
    prompt: String,
    snapshot: String,
) -> Result<OpenUiView, ServerFnError> {
    with_active_dir(move |dir| save_view_at(&dir, &name, &prompt, &snapshot)).await
}

/// Phase 54.10 D-02: save a view with its prompt and no snapshot yet (a dashboard AI widget
/// before its first generation). An empty `name` defaults to the first 40 prompt characters.
#[server]
pub async fn create_pending_openui_view(
    name: String,
    prompt: String,
) -> Result<OpenUiView, ServerFnError> {
    with_active_dir(move |dir| save_pending_view_at(&dir, &name, &prompt)).await
}

/// Replace a view's snapshot after a successful Re-run (D-16).
#[server]
pub async fn replace_openui_snapshot(
    id: String,
    snapshot: String,
) -> Result<OpenUiView, ServerFnError> {
    with_active_dir(move |dir| replace_snapshot_at(&dir, &id, &snapshot)).await
}

/// Delete a view of the active profile.
#[server]
pub async fn delete_openui_view(id: String) -> Result<(), ServerFnError> {
    with_active_dir(move |dir| delete_view_at(&dir, &id)).await
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::protocol::{ActivationScope, ActiveProfileRecord};
    use crate::server::profile_activation_api::{
        clear_active_profile_record, write_active_profile_record,
    };

    /// Module-local RAII env guard (each test module is its own namespace in this crate).
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }
    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: tests run with --test-threads=1.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }
    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: tests run with --test-threads=1.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    #[test]
    fn views_round_trip_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let v = save_view_at(dir.path(), "Failures", "a table", "root = Card([])").unwrap();
        assert!(uuid::Uuid::parse_str(&v.id).is_ok());
        assert!(dir.path().join(format!("{}.json", v.id)).is_file());
        assert_eq!(list_views_at(dir.path()), vec![v.clone()]);
        assert_eq!(load_view_at(dir.path(), &v.id).unwrap(), v);
        let r = replace_snapshot_at(dir.path(), &v.id, "root = Card([x])").unwrap();
        assert_eq!(r.snapshot, "root = Card([x])");
        assert_eq!(
            (r.name.as_str(), r.prompt.as_str()),
            ("Failures", "a table")
        );
        delete_view_at(dir.path(), &v.id).unwrap();
        assert!(list_views_at(dir.path()).is_empty());
    }

    #[test]
    fn views_list_newest_first_and_skips_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let mut ids = Vec::new();
        for (i, n) in ["a", "b", "c"].iter().enumerate() {
            let mut v = save_view_at(dir.path(), n, "p", "s").unwrap();
            v.created_at = 100 + i as i64; // distinct, deterministic order
            write_view(dir.path(), &v).unwrap();
            ids.push(v.id);
        }
        std::fs::write(dir.path().join("x.json"), "{}").unwrap();
        std::fs::write(dir.path().join("not-a-uuid.txt"), "junk").unwrap();
        std::fs::write(
            dir.path().join(format!("{}.json", uuid::Uuid::new_v4())),
            "{ not json",
        )
        .unwrap();
        let names: Vec<String> = list_views_at(dir.path())
            .into_iter()
            .map(|v| v.name)
            .collect();
        assert_eq!(names, ["c", "b", "a"]);
    }

    #[test]
    fn views_enforce_caps() {
        let dir = tempfile::tempdir().unwrap();
        let long = "n".repeat(200);
        let v = save_view_at(dir.path(), &long, "p", "s").unwrap();
        assert_eq!(v.name.chars().count(), OPENUI_VIEW_NAME_MAX);
        let blank = save_view_at(dir.path(), " ", &"q".repeat(90), "s").unwrap();
        assert_eq!(blank.name.chars().count(), 40);
        assert!(save_view_at(dir.path(), "n", "  ", "s").is_err());
        let big = "x".repeat(OPENUI_SNAPSHOT_MAX_BYTES + 1);
        assert!(save_view_at(dir.path(), "n", "p", &big).is_err());
        assert!(save_view_at(dir.path(), "n", "p", "s").is_ok());
        // Fill to the cap with hand-written files, then the next save is refused.
        let full = tempfile::tempdir().unwrap();
        for _ in 0..OPENUI_VIEWS_MAX {
            let id = uuid::Uuid::new_v4().to_string();
            let view = OpenUiView {
                id: id.clone(),
                name: "n".into(),
                prompt: "p".into(),
                snapshot: "s".into(),
                created_at: 1,
                updated_at: 1,
            };
            std::fs::write(
                full.path().join(format!("{id}.json")),
                serde_json::to_vec(&view).unwrap(),
            )
            .unwrap();
        }
        assert_eq!(
            save_view_at(full.path(), "n", "p", "s").unwrap_err(),
            "Saved view limit reached"
        );
    }

    #[test]
    fn pending_view_saves_with_empty_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let v = save_pending_view_at(dir.path(), "", "a weekly spend table").unwrap();
        assert_eq!(v.snapshot, "");
        assert_eq!(v.name, "a weekly spend table");
        assert_eq!(v.prompt, "a weekly spend table");
        assert_eq!(list_views_at(dir.path()), vec![v.clone()]);
        let long = save_pending_view_at(dir.path(), " ", &"q".repeat(90)).unwrap();
        assert_eq!(long.name.chars().count(), 40);
        assert!(save_pending_view_at(dir.path(), "n", "  ").is_err());
        let big = "x".repeat(OPENUI_SNAPSHOT_MAX_BYTES + 1);
        assert!(save_pending_view_at(dir.path(), "n", &big).is_err());
        // Pending views count against the cap (T-54.10-32).
        let full = tempfile::tempdir().unwrap();
        for _ in 0..OPENUI_VIEWS_MAX {
            save_pending_view_at(full.path(), "n", "p").unwrap();
        }
        assert_eq!(
            save_pending_view_at(full.path(), "n", "p").unwrap_err(),
            "Saved view limit reached"
        );
        // The first generation fills the same view.
        let filled = replace_snapshot_at(dir.path(), &v.id, "root = Card([])").unwrap();
        assert_eq!(filled.id, v.id);
        assert_eq!(filled.snapshot, "root = Card([])");
        assert_eq!(filled.prompt, "a weekly spend table");
    }

    #[test]
    fn normal_save_still_rejects_empty_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            save_view_at(dir.path(), "n", "p", "").unwrap_err(),
            "Nothing to save"
        );
        assert_eq!(
            save_view_at(dir.path(), "n", "p", "  \n").unwrap_err(),
            "Nothing to save"
        );
        assert!(list_views_at(dir.path()).is_empty());
    }

    #[test]
    fn views_reject_non_uuid_ids() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("config.json");
        std::fs::write(&outside, "{}").unwrap();
        let views = dir.path().join("views");
        std::fs::create_dir_all(&views).unwrap();
        assert!(delete_view_at(&views, "../config").is_err());
        assert!(load_view_at(&views, "a/b").is_err());
        assert!(replace_snapshot_at(&views, "../config", "s").is_err());
        assert!(
            outside.is_file(),
            "traversal id must not touch the filesystem"
        );
    }

    fn record(name: &str) -> ActiveProfileRecord {
        ActiveProfileRecord {
            name: name.into(),
            scope: ActivationScope::ChatOnly,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn views_saved_under_profile_a_not_listed_under_profile_b() {
        let home = tempfile::tempdir().unwrap();
        let _g = ScopedEnv::set("IRONHERMES_HOME", home.path().to_str().unwrap());
        for p in ["alpha", "beta"] {
            std::fs::create_dir_all(home.path().join("profiles").join(p)).unwrap();
        }
        // The load-bearing guard: a current_profile()-based predicate could not tell them apart.
        assert_ne!(ironhermes_core::current_profile(), "alpha");

        write_active_profile_record(&record("alpha")).unwrap();
        let a = views_dir().unwrap();
        assert_eq!(a, home.path().join("profiles/alpha/openui/views"));
        let saved = save_view_at(&a, "A", "p", "s").unwrap();

        write_active_profile_record(&record("beta")).unwrap();
        let b = views_dir().unwrap();
        assert_eq!(b, home.path().join("profiles/beta/openui/views"));
        assert!(list_views_at(&b).is_empty());
        assert!(
            load_view_at(&b, &saved.id).is_err(),
            "another profile's id is not found"
        );
        assert!(delete_view_at(&b, &saved.id).is_err());

        write_active_profile_record(&record("alpha")).unwrap();
        assert_eq!(list_views_at(&views_dir().unwrap()), vec![saved]);

        clear_active_profile_record().unwrap();
        let root = views_dir().unwrap();
        assert_eq!(root, home.path().join("openui/views"));
        assert!(list_views_at(&root).is_empty());
    }

    /// ADR-0007 "Saved views" correction: when the resolved slug IS the server's own profile
    /// (`current_profile()`), the home is `get_hermes_home()` itself, never a nested
    /// `<home>/profiles/<slug>`.
    #[test]
    fn views_dir_uses_the_home_itself_when_the_slug_is_the_server_profile() {
        let root = tempfile::tempdir().unwrap();
        let prof_home = root.path().join("profiles").join("pivoted");
        std::fs::create_dir_all(&prof_home).unwrap();
        let _g = ScopedEnv::set("IRONHERMES_HOME", prof_home.to_str().unwrap());
        assert_eq!(ironhermes_core::current_profile(), "pivoted");
        assert_eq!(
            views_dir_for_slug("pivoted").unwrap(),
            prof_home.join("openui/views")
        );
        assert_eq!(
            views_dir_for_slug("default").unwrap(),
            prof_home.join("openui/views")
        );
    }

    #[test]
    fn views_dir_refuses_an_invalid_active_profile_name() {
        let home = tempfile::tempdir().unwrap();
        let _g = ScopedEnv::set("IRONHERMES_HOME", home.path().to_str().unwrap());
        write_active_profile_record(&record("../evil")).unwrap();
        assert!(views_dir().is_err());
        assert!(
            !home.path().join("openui").exists() && !home.path().join("profiles").exists(),
            "no directory is created for a refused slug"
        );
        clear_active_profile_record().unwrap();
    }
}
