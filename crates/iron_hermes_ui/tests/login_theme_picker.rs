//! Phase 47.3 Plan 06 (D-01/D-03) — Settings login-theme picker.
//!
//! Locks:
//! - D-01: the picker offers all five treatments (basic / fade /
//!   blast-doors / matrix-rain / orbit-veil).
//! - D-03: the disabled (read-only) state carries the true HTML `disabled`
//!   attribute on every input — never visual dimming alone — plus the exact
//!   helper copy naming `web_ui.auth.login_theme`.
//! - T-47.3-24/T-47.3-25: the save path reuses the existing
//!   `security.web_config_write_enabled` gate and validates the slug against
//!   the five known values.
//!
//! Pattern: source-read assertion (mirrors tests/kanban_server_fns.rs), plus
//! a data-layer round trip against `ironhermes_core` (an importable lib
//! crate) for every slug through `web_ui.auth.login_theme`.
//!
//! All tests live inside the `login_theme_picker` module below so the
//! plan's own filter (`cargo test -p iron_hermes_ui --all-features
//! login_theme_picker`) matches every test's fully-qualified name, not just
//! the file name.

mod login_theme_picker {
    use std::fs;
    use std::path::PathBuf;

    use ironhermes_core::config::Config;

    fn crate_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn read(path: &str) -> String {
        fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
    }

    const SLUGS: [&str; 5] = ["basic", "fade", "blast-doors", "matrix-rain", "orbit-veil"];

    #[test]
    fn settings_screen_lists_all_five_theme_slugs() {
        let src = read("src/components/hermes_app/screens/settings.rs");
        for slug in SLUGS {
            assert!(
                src.contains(slug),
                "settings.rs must reference the '{slug}' login-theme slug (D-01)"
            );
        }
    }

    #[test]
    fn settings_screen_has_readonly_helper_copy_exactly_once() {
        let src = read("src/components/hermes_app/screens/settings.rs");
        const HELPER: &str =
        "Read-only — config writes are disabled. Edit web_ui.auth.login_theme in config.yaml to change.";
        let count = src.matches(HELPER).count();
        assert_eq!(
            count, 1,
            "the D-03 read-only helper copy must appear exactly once verbatim; found {count}"
        );
    }

    #[test]
    fn settings_screen_picker_disabled_binding_is_write_enabled_driven() {
        let src = read("src/components/hermes_app/screens/settings.rs");
        assert!(
            src.contains("disabled: !write_enabled_val"),
            "D-03: the picker's radio inputs must carry a `disabled:` binding \
             derived from the write-enabled signal (visual dimming alone is a \
             rejected outcome)"
        );
    }

    #[test]
    fn settings_screen_login_theme_panel_uses_read_not_peek_for_write_enabled() {
        let src = read("src/components/hermes_app/screens/settings.rs");
        // Locate the LoginThemePanel component body specifically, so this
        // test doesn't get a false pass/fail from unrelated panels in the
        // same file.
        let panel_start = src
            .find("fn LoginThemePanel")
            .expect("LoginThemePanel component must exist");
        let panel_src = &src[panel_start..];
        assert!(
            panel_src.contains("write_enabled.read()"),
            "LoginThemePanel must read write_enabled with `.read()` (subscribes), \
             never `.peek()` in render (does not subscribe)"
        );
        assert!(
            !panel_src.contains("write_enabled.peek()"),
            "LoginThemePanel must not use `.peek()` on write_enabled in render body"
        );
    }

    #[test]
    fn api_rs_login_theme_appears_in_snapshot_construction_and_save_validation() {
        let src = read("src/server/api.rs");
        let count = src.matches("login_theme").count();
        assert!(
            count >= 3,
            "api.rs must reference login_theme at least 3 times \
             (snapshot field, snapshot construction, save-path validation); found {count}"
        );
    }

    #[test]
    fn api_rs_save_path_is_server_not_get() {
        let src = read("src/server/api.rs");
        // The argument-taking save path (`update_agent_config`) must be
        // `#[server]`, never `#[get]` — this task must not increase the
        // `#[get]` count.
        let update_fn_idx = src
            .find("pub async fn update_agent_config")
            .expect("update_agent_config must exist");
        let preceding = &src[..update_fn_idx];
        let last_attr_start = preceding
            .rfind("#[server]")
            .expect("update_agent_config must be preceded by #[server]");
        // No #[get] between the attribute and the fn signature.
        assert!(
            !preceding[last_attr_start..].contains("#[get]"),
            "update_agent_config must be #[server], not #[get] (it takes an argument)"
        );
    }

    #[test]
    fn config_login_theme_round_trips_through_yaml_for_every_known_slug() {
        for slug in SLUGS {
            let mut config = Config::default();
            config.web_ui.auth.login_theme = slug.to_string();

            let yaml = serde_yaml::to_string(&config).expect("Config must serialize");
            let reloaded: Config = serde_yaml::from_str(&yaml).expect("Config must deserialize");

            assert_eq!(
                reloaded.web_ui.auth.login_theme, slug,
                "login_theme '{slug}' must round-trip through YAML unchanged"
            );
        }
    }
}
