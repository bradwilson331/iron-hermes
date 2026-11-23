//! Phase 54.9 Plan 01 (D-03/D-07/D-08): menu-layout read/write server fns.
//!
//! The layout lives under `ui.menu` in the SERVER PROCESS HOME `config.yaml`
//! (`Config::config_path()`, the same file the Settings General tab edits), not
//! in localStorage and not per header-switched profile. Every read is a fresh
//! `Config::load_from` and every write is load-modify-save, so no
//! `AppRuntimeBundle` boot snapshot is involved.
//!
//! `#[server]` (not `#[get]`): `set_menu_layout` takes an argument. The module
//! is unconditional like `display_tz_api`; helpers are gated
//! `#[cfg(feature = "server")]`.

use crate::menu_layout::MenuLayout;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Same copy as the other gated config writers (`update_agent_config`).
pub const MENU_WRITE_DISABLED: &str = "Config writes are disabled";

/// What `get_menu_layout` returns: the layout to render (saved or default), a
/// one-line notice when a saved layout was ignored, and whether the write gate
/// is open (so the editor can disable Save up front).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MenuLayoutSnapshot {
    pub layout: MenuLayout,
    pub notice: Option<String>,
    pub web_config_write_enabled: bool,
}

/// Read the layout from `config_path`. Never errors, never panics (D-08): any
/// unreadable file, wrong-typed value or invalid layout yields the shipped
/// default plus the one-line notice.
#[cfg(feature = "server")]
pub(crate) fn menu_snapshot_at(config_path: &std::path::Path) -> MenuLayoutSnapshot {
    use crate::menu_layout::{default_layout, invalid_notice, migrate, validate, MenuLayoutError};

    let fallback = |err: MenuLayoutError, gate: bool| MenuLayoutSnapshot {
        layout: default_layout(),
        notice: Some(invalid_notice(&err)),
        web_config_write_enabled: gate,
    };
    let cfg = match ironhermes_core::config::Config::load_from(config_path) {
        Ok(cfg) => cfg,
        Err(_) => return fallback(MenuLayoutError::Unreadable, false),
    };
    let gate = cfg.security.web_config_write_enabled;
    if cfg.ui.menu.is_null() {
        return MenuLayoutSnapshot {
            layout: default_layout(),
            notice: None,
            web_config_write_enabled: gate,
        };
    }
    // A layout saved before a catalog entry existed is upgraded, not rejected.
    match serde_yaml::from_value::<MenuLayout>(cfg.ui.menu.clone()).map(migrate) {
        Err(_) => fallback(MenuLayoutError::Unreadable, gate),
        Ok(layout) => match validate(&layout) {
            Ok(()) => MenuLayoutSnapshot {
                layout,
                notice: None,
                web_config_write_enabled: gate,
            },
            Err(e) => fallback(e, gate),
        },
    }
}

/// Validate, then load-modify-save `ui.menu` at `config_path`.
#[cfg(feature = "server")]
pub(crate) fn save_menu_layout_at(
    config_path: &std::path::Path,
    layout: &MenuLayout,
) -> Result<(), String> {
    crate::menu_layout::validate(layout).map_err(|e| e.to_string())?;
    // Shared with the activation-record, agent-config and provider-config writers: they
    // all load-modify-save the same root file, so unserialised they would lose each other's
    // update. Other root-config writers (tools config / credentials, ...) do not take it yet.
    let _guard = crate::server::profile_activation_api::lock_root_config_write();
    let mut cfg = ironhermes_core::config::Config::load_from(config_path)
        .map_err(|e| format!("Config load failed: {e}"))?;
    if !cfg.security.web_config_write_enabled {
        return Err(MENU_WRITE_DISABLED.to_string());
    }
    cfg.ui.menu = serde_yaml::to_value(layout).map_err(|e| format!("Config save failed: {e}"))?;
    cfg.save_to(config_path)
        .map_err(|e| format!("Config save failed: {e}"))
}

/// Read the menu layout (fresh from disk every call).
#[server]
pub async fn get_menu_layout() -> Result<MenuLayoutSnapshot, ServerFnError> {
    Ok(menu_snapshot_at(
        &ironhermes_core::config::Config::config_path(),
    ))
}

/// Save a menu layout. Gated by `security.web_config_write_enabled`.
#[server]
pub async fn set_menu_layout(layout: MenuLayout) -> Result<(), ServerFnError> {
    save_menu_layout_at(&ironhermes_core::config::Config::config_path(), &layout)
        .map_err(ServerFnError::new)
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::menu_layout::default_layout;
    use ironhermes_core::config::Config;

    fn write_cfg(dir: &tempfile::TempDir, text: &str) -> std::path::PathBuf {
        let p = dir.path().join("config.yaml");
        std::fs::write(&p, text).unwrap();
        p
    }

    const GATE_OPEN: &str = "security:\n  web_config_write_enabled: true\n";

    fn reversed_header() -> MenuLayout {
        let mut l = default_layout();
        l.header.reverse();
        l
    }

    #[test]
    fn menu_layout_round_trips_through_config_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_cfg(&dir, GATE_OPEN);
        let l = reversed_header();
        save_menu_layout_at(&p, &l).unwrap();
        let snap = menu_snapshot_at(&p);
        assert_eq!(snap.layout, l);
        assert_eq!(snap.notice, None);
        assert!(snap.web_config_write_enabled);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("ui:") && text.contains("menu:"), "{text}");
    }

    #[test]
    fn menu_save_preserves_other_config_keys() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_cfg(
            &dir,
            &format!(
                "{GATE_OPEN}model:\n  default: keep-me\nactive_profile:\n  name: alpha\n  scope: everywhere\n"
            ),
        );
        let before = Config::load_from(&p).unwrap();
        save_menu_layout_at(&p, &reversed_header()).unwrap();
        let after = Config::load_from(&p).unwrap();
        assert_eq!(after.model.default, "keep-me");
        assert_eq!(
            serde_yaml::to_string(&after.active_profile).unwrap(),
            serde_yaml::to_string(&before.active_profile).unwrap()
        );
        assert!(!after.ui.menu.is_null());
    }

    fn assert_default_with_notice(snap: &MenuLayoutSnapshot, needle: &str) {
        assert_eq!(snap.layout, default_layout());
        let n = snap.notice.as_deref().expect("notice");
        assert!(n.starts_with("Saved menu layout ignored: "), "{n}");
        assert!(n.contains(needle), "{n}");
    }

    #[test]
    fn snapshot_falls_back_on_invalid_layout_with_notice() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_cfg(
            &dir,
            "ui:\n  menu:\n    version: 1\n    wheel: []\n    header: []\n",
        );
        assert_default_with_notice(&menu_snapshot_at(&p), "");
    }

    /// Phase 54.9 Plan 09: a layout saved at version 1 (before `openui` existed) is upgraded in
    /// place, keeping its order and labels, not thrown away with a notice.
    #[test]
    fn snapshot_migrates_v1_layouts_instead_of_rejecting() {
        let dir = tempfile::tempdir().unwrap();
        let mut v1 = reversed_header();
        v1.version = 1;
        v1.header.retain(|e| e.screen != "openui");
        v1.header[0].label = "Mine".into();
        let yaml = serde_yaml::to_string(&serde_json::json!({ "ui": { "menu": v1 } })).unwrap();
        let p = write_cfg(&dir, &yaml);
        let snap = menu_snapshot_at(&p);
        assert_eq!(snap.notice, None);
        assert_eq!(snap.layout.version, crate::menu_layout::MENU_LAYOUT_VERSION);
        assert_eq!(snap.layout.header.last().unwrap().screen, "openui");
        assert_eq!(snap.layout.header[0].label, "Mine");
        assert_eq!(snap.layout.wheel, v1.wheel);
    }

    #[test]
    fn snapshot_falls_back_on_wrong_yaml_type() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_cfg(&dir, "ui:\n  menu: 42\n");
        assert_default_with_notice(&menu_snapshot_at(&p), "could not be read");
    }

    #[test]
    fn snapshot_falls_back_when_config_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_cfg(&dir, ":::not yaml");
        assert_default_with_notice(&menu_snapshot_at(&p), "could not be read");
    }

    #[test]
    fn snapshot_absent_menu_is_default_without_notice() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_cfg(&dir, "model:\n  default: x\n");
        let snap = menu_snapshot_at(&p);
        assert_eq!(snap.layout, default_layout());
        assert_eq!(snap.notice, None);
    }

    #[test]
    fn menu_save_refuses_when_config_writes_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_cfg(&dir, "model:\n  default: x\n");
        let before = std::fs::read(&p).unwrap();
        assert_eq!(
            save_menu_layout_at(&p, &default_layout()),
            Err(MENU_WRITE_DISABLED.to_string())
        );
        assert_eq!(std::fs::read(&p).unwrap(), before);
    }

    #[test]
    fn menu_save_rejects_invalid_layout_before_touching_disk() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_cfg(&dir, GATE_OPEN);
        let before = std::fs::read(&p).unwrap();
        let mut l = default_layout();
        let moved: Vec<_> = l.wheel.drain(1..6).collect();
        l.header.extend(moved);
        let err = save_menu_layout_at(&p, &l).unwrap_err();
        assert!(err.contains("wheel has 5 sections"), "{err}");
        assert_eq!(std::fs::read(&p).unwrap(), before);
    }

    #[test]
    fn menu_save_and_activation_writer_do_not_lose_updates() {
        use crate::server::profile_activation_api::lock_root_config_write;
        let dir = tempfile::tempdir().unwrap();
        let p = write_cfg(&dir, GATE_OPEN);
        let (a, b) = (default_layout(), reversed_header());
        std::thread::scope(|s| {
            for i in 0..4 {
                let (p, l) = (&p, if i % 2 == 0 { &a } else { &b });
                s.spawn(move || save_menu_layout_at(p, l).unwrap());
                s.spawn(move || {
                    let _g = lock_root_config_write();
                    let mut cfg = Config::load_from(p).unwrap();
                    cfg.active_profile.name = Some(format!("t{i}"));
                    cfg.save_to(p).unwrap();
                });
            }
        });
        let cfg = Config::load_from(&p).unwrap();
        let snap = menu_snapshot_at(&p);
        assert!(!cfg.ui.menu.is_null());
        assert_eq!(snap.notice, None);
        assert!(cfg.active_profile.name.is_some());
    }
}
