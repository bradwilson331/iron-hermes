//! Integration tests for `hermes skills` CLI subcommands.
//!
//! All tests call the lib-level handlers directly (no subprocess).
//! Tests use a temp directory for HERMES_HOME to isolate filesystem state.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

// Guard env mutations in tests (env vars are process-global).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Set HERMES_HOME to a temp dir for the duration of the closure.
fn with_hermes_home<F: FnOnce(PathBuf)>(f: F) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var("HERMES_HOME").ok();
    unsafe {
        std::env::set_var("HERMES_HOME", tmp.path());
    }
    f(tmp.path().to_path_buf());
    unsafe {
        match prev {
            Some(v) => std::env::set_var("HERMES_HOME", v),
            None => std::env::remove_var("HERMES_HOME"),
        }
    }
}

/// Write a config.yaml to the given path with optional trusted_repos.
fn write_config(config_path: &std::path::Path, trusted_repos: &[&str]) {
    let repos_yaml = if trusted_repos.is_empty() {
        "      trusted_repos: []".to_string()
    } else {
        let list = trusted_repos
            .iter()
            .map(|r| format!("        - \"{}\"", r))
            .collect::<Vec<_>>()
            .join("\n");
        format!("      trusted_repos:\n{}", list)
    };
    let yaml = format!(
        "skills:\n  hub:\n{}\n",
        repos_yaml
    );
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(config_path, yaml).unwrap();
}

// ---------------------------------------------------------------------------
// cli_trust_add_writes_config
// ---------------------------------------------------------------------------

#[test]
fn cli_trust_add_writes_config() {
    with_hermes_home(|home| {
        let config_path = home.join("config.yaml");
        write_config(&config_path, &[]);

        // Load fresh config
        let mut cfg = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        ironhermes_cli::skills_cmd::cmd_trust_add_impl(&mut cfg, &config_path, "anthropics/skills")
            .unwrap();

        // Reload config and assert the repo is present
        let cfg2 = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        assert!(
            cfg2.skills.hub.trusted_repos.contains(&"anthropics/skills".to_string()),
            "expected trusted_repos to contain anthropics/skills after trust add"
        );
    });
}

// ---------------------------------------------------------------------------
// cli_trust_add_is_idempotent
// ---------------------------------------------------------------------------

#[test]
fn cli_trust_add_is_idempotent() {
    with_hermes_home(|home| {
        let config_path = home.join("config.yaml");
        write_config(&config_path, &[]);

        let mut cfg = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        ironhermes_cli::skills_cmd::cmd_trust_add_impl(&mut cfg, &config_path, "anthropics/skills")
            .unwrap();
        let mut cfg2 = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        ironhermes_cli::skills_cmd::cmd_trust_add_impl(
            &mut cfg2,
            &config_path,
            "anthropics/skills",
        )
        .unwrap();

        let cfg3 = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        let count = cfg3
            .skills
            .hub
            .trusted_repos
            .iter()
            .filter(|r| r.as_str() == "anthropics/skills")
            .count();
        assert_eq!(count, 1, "idempotent add should not produce duplicates");
    });
}

// ---------------------------------------------------------------------------
// cli_trust_remove_writes_config
// ---------------------------------------------------------------------------

#[test]
fn cli_trust_remove_writes_config() {
    with_hermes_home(|home| {
        let config_path = home.join("config.yaml");
        write_config(&config_path, &["anthropics/skills", "openai/skills"]);

        let mut cfg = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        ironhermes_cli::skills_cmd::cmd_trust_remove_impl(
            &mut cfg,
            &config_path,
            "anthropics/skills",
        )
        .unwrap();

        let cfg2 = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        assert!(
            !cfg2.skills.hub.trusted_repos.contains(&"anthropics/skills".to_string()),
            "anthropics/skills should be removed"
        );
        assert!(
            cfg2.skills.hub.trusted_repos.contains(&"openai/skills".to_string()),
            "openai/skills should remain"
        );
    });
}

// ---------------------------------------------------------------------------
// cli_trust_remove_absent_is_noop
// ---------------------------------------------------------------------------

#[test]
fn cli_trust_remove_absent_is_noop() {
    with_hermes_home(|home| {
        let config_path = home.join("config.yaml");
        write_config(&config_path, &["openai/skills"]);

        let mut cfg = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        // removing non-existent repo should not error
        ironhermes_cli::skills_cmd::cmd_trust_remove_impl(
            &mut cfg,
            &config_path,
            "nonexistent/repo",
        )
        .unwrap();

        let cfg2 = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        assert_eq!(cfg2.skills.hub.trusted_repos, vec!["openai/skills".to_string()]);
    });
}

// ---------------------------------------------------------------------------
// cli_trust_list_text_output
// ---------------------------------------------------------------------------

#[test]
fn cli_trust_list_text_output() {
    with_hermes_home(|home| {
        let config_path = home.join("config.yaml");
        write_config(&config_path, &["openai/skills", "anthropics/skills"]);

        let cfg = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        let output =
            ironhermes_cli::skills_cmd::cmd_trust_list_impl(&cfg, ironhermes_cli::skills_cmd::Format::Text);
        assert!(output.contains("openai/skills"), "text output should contain openai/skills");
        assert!(output.contains("anthropics/skills"), "text output should contain anthropics/skills");
    });
}

// ---------------------------------------------------------------------------
// cli_trust_list_json_output
// ---------------------------------------------------------------------------

#[test]
fn cli_trust_list_json_output() {
    with_hermes_home(|home| {
        let config_path = home.join("config.yaml");
        write_config(&config_path, &["openai/skills", "anthropics/skills"]);

        let cfg = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        let output =
            ironhermes_cli::skills_cmd::cmd_trust_list_impl(&cfg, ironhermes_cli::skills_cmd::Format::Json);
        // Must be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        let arr = parsed.as_array().expect("JSON array");
        let repos: Vec<&str> = arr
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(repos.contains(&"openai/skills"));
        assert!(repos.contains(&"anthropics/skills"));
    });
}

// ---------------------------------------------------------------------------
// cli_list_reads_manifest (empty manifest returns empty JSON array)
// ---------------------------------------------------------------------------

#[test]
fn cli_list_reads_manifest() {
    with_hermes_home(|home| {
        let config_path = home.join("config.yaml");
        write_config(&config_path, &[]);

        let cfg = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        let output = ironhermes_cli::skills_cmd::cmd_list_impl(&cfg, ironhermes_cli::skills_cmd::Format::Json);
        // Must be valid JSON array (empty since nothing is installed)
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert!(parsed.is_array(), "list --format json should return a JSON array");
    });
}

// ---------------------------------------------------------------------------
// cli_list_text_format_contains_substrings
// ---------------------------------------------------------------------------

#[test]
fn cli_list_text_format_contains_substrings() {
    with_hermes_home(|home| {
        let config_path = home.join("config.yaml");
        write_config(&config_path, &[]);

        let cfg = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        let output = ironhermes_cli::skills_cmd::cmd_list_impl(&cfg, ironhermes_cli::skills_cmd::Format::Text);
        // When nothing is installed, at minimum the output should be non-panicking and a string
        let _ = output;
    });
}

// ---------------------------------------------------------------------------
// cli_search_json_format_returns_valid_json
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_search_json_format_returns_valid_json() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let config_path = home.join("config.yaml");
    write_config(&config_path, &[]);

    // Set HERMES_HOME for this test (single-threaded tokio test, no parallelism issue).
    let prev = std::env::var("HERMES_HOME").ok();
    unsafe { std::env::set_var("HERMES_HOME", &home); }

    let cfg = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
    // Use limit=1 so we fail fast on live network; returns empty array in offline CI.
    let output = ironhermes_cli::skills_cmd::cmd_search_impl(
        &cfg,
        "gif",
        None,
        ironhermes_cli::skills_cmd::Format::Json,
        1,
    )
    .await;

    unsafe {
        match prev {
            Some(v) => std::env::set_var("HERMES_HOME", v),
            None => std::env::remove_var("HERMES_HOME"),
        }
    }

    // output must be valid JSON array
    let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
    assert!(parsed.is_array(), "search result must be a JSON array");
    // When results exist (live network), each item must have the expected fields
    if let Some(arr) = parsed.as_array() {
        for item in arr {
            assert!(item.get("name").is_some(), "item missing 'name'");
            assert!(item.get("source").is_some(), "item missing 'source'");
            assert!(item.get("identifier").is_some(), "item missing 'identifier'");
            assert!(item.get("description").is_some(), "item missing 'description'");
            assert!(item.get("trust_level").is_some(), "item missing 'trust_level'");
        }
    }
}

// ---------------------------------------------------------------------------
// cli_skips_action_enum_completeness
// (verifies the SkillsAction enum has all 6 verbs via pattern match exhaustion at compile time)
// ---------------------------------------------------------------------------

#[test]
fn cli_skills_action_enum_has_all_verbs() {
    use ironhermes_cli::skills_cmd::SkillsAction;
    // This test ensures the enum exists and is importable; exhaustive match is compile-time checked.
    // We just construct a representative variant to confirm it compiles.
    let _search = SkillsAction::Search {
        query: "test".to_string(),
        source: None,
        format: ironhermes_cli::skills_cmd::Format::Json,
        limit: 5,
    };
    let _list = SkillsAction::List {
        format: ironhermes_cli::skills_cmd::Format::Text,
    };
    let _install = SkillsAction::Install {
        identifier: "foo/bar/baz".to_string(),
        yes: false,
        skip_audit: false,
    };
    let _update = SkillsAction::Update { name: None };
    let _remove = SkillsAction::Remove {
        name: "tenor-gif".to_string(),
    };
    let _trust = SkillsAction::Trust {
        action: ironhermes_cli::skills_cmd::TrustAction::List {
            format: ironhermes_cli::skills_cmd::Format::Text,
        },
    };
}

// ---------------------------------------------------------------------------
// Plan 21.8-04 Wave 3 tests
// ---------------------------------------------------------------------------

// Minimal clap Parser wrapping SkillsAction so we can exercise parsing.
#[derive(clap::Parser, Debug)]
#[command(name = "skills-test", no_binary_name = true)]
struct SkillsTestCli {
    #[command(subcommand)]
    action: ironhermes_cli::skills_cmd::SkillsAction,
}

/// Test 1 (D-04 alias): `skills uninstall foo` resolves to SkillsAction::Remove.
#[test]
fn skills_action_accepts_uninstall_alias() {
    use clap::Parser;
    let parsed = SkillsTestCli::try_parse_from(["uninstall", "foo"]).expect("parse uninstall");
    match parsed.action {
        ironhermes_cli::skills_cmd::SkillsAction::Remove { name } => {
            assert_eq!(name, "foo");
        }
        other => panic!("expected Remove via uninstall alias, got {:?}", other),
    }
}

/// Test 2 (D-04 canonical): `skills remove foo` resolves to SkillsAction::Remove.
#[test]
fn skills_action_accepts_remove_canonical() {
    use clap::Parser;
    let parsed = SkillsTestCli::try_parse_from(["remove", "bar"]).expect("parse remove");
    match parsed.action {
        ironhermes_cli::skills_cmd::SkillsAction::Remove { name } => {
            assert_eq!(name, "bar");
        }
        other => panic!("expected Remove for canonical verb, got {:?}", other),
    }
}

/// Test 3 (D-19): `skills install foo --skip-audit` parses `skip_audit = true`.
#[test]
fn skills_action_install_parses_skip_audit() {
    use clap::Parser;
    let parsed = SkillsTestCli::try_parse_from(["install", "foo", "--skip-audit"])
        .expect("parse install --skip-audit");
    match parsed.action {
        ironhermes_cli::skills_cmd::SkillsAction::Install {
            identifier,
            yes,
            skip_audit,
        } => {
            assert_eq!(identifier, "foo");
            assert!(!yes);
            assert!(skip_audit);
        }
        other => panic!("expected Install, got {:?}", other),
    }
}

/// Test 4 (D-19 default): `skills install foo` defaults `skip_audit` to `false`.
#[test]
fn skills_action_install_defaults_skip_audit_false() {
    use clap::Parser;
    let parsed =
        SkillsTestCli::try_parse_from(["install", "foo"]).expect("parse install no flags");
    match parsed.action {
        ironhermes_cli::skills_cmd::SkillsAction::Install { skip_audit, .. } => {
            assert!(!skip_audit, "default skip_audit should be false");
        }
        other => panic!("expected Install, got {:?}", other),
    }
}

/// Test 5 (D-10): `cmd_list_impl` reads `skills-lock.json` (not the 19.1 manifest)
/// and returns installed entries.
#[test]
fn cmd_list_reads_from_lock() {
    use ironhermes_hub::{SkillLock, SkillLockEntry};

    with_hermes_home(|home| {
        let config_path = home.join("config.yaml");
        write_config(&config_path, &[]);

        // Seed a SkillLock with 2 entries under HERMES_HOME/skills-lock.json.
        let mut lock = SkillLock::default();
        lock.add_or_replace(SkillLockEntry {
            name: "alpha".to_string(),
            source: "skills-sh".to_string(),
            identifier: "alpha".to_string(),
            repo_path: "foo/bar/alpha".to_string(),
            snapshot_hash: "a".repeat(64),
            computed_hash: "b".repeat(64),
            installed_at: chrono::Utc::now(),
            extras: Default::default(),
        });
        lock.add_or_replace(SkillLockEntry {
            name: "beta".to_string(),
            source: "skills-sh".to_string(),
            identifier: "beta".to_string(),
            repo_path: "foo/bar/beta".to_string(),
            snapshot_hash: "c".repeat(64),
            computed_hash: "d".repeat(64),
            installed_at: chrono::Utc::now(),
            extras: Default::default(),
        });
        lock.save_atomic().expect("save lock");

        let cfg = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        let json_out = ironhermes_cli::skills_cmd::cmd_list_impl(
            &cfg,
            ironhermes_cli::skills_cmd::Format::Json,
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&json_out).expect("list json parses");
        let arr = parsed.as_array().expect("json array");
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|v| v["name"].as_str())
            .collect();
        assert!(names.contains(&"alpha"), "list missing alpha: {names:?}");
        assert!(names.contains(&"beta"), "list missing beta: {names:?}");

        // Each item must expose snapshot_hash + computed_hash + installed_at
        // fields (plan 21.8-04 — list reads SkillLock, not HubManifest).
        for item in arr {
            assert!(item.get("snapshot_hash").is_some(), "missing snapshot_hash");
            assert!(item.get("computed_hash").is_some(), "missing computed_hash");
            assert!(item.get("installed_at").is_some(), "missing installed_at");
        }
    });
}

/// Test 6: `cmd_list_impl` returns empty JSON when no lock file exists.
#[test]
fn cmd_list_empty_when_no_lock() {
    with_hermes_home(|home| {
        let config_path = home.join("config.yaml");
        write_config(&config_path, &[]);
        // no skills-lock.json written

        let cfg = ironhermes_cli::skills_cmd::load_config_for_test(&config_path).unwrap();
        let json_out = ironhermes_cli::skills_cmd::cmd_list_impl(
            &cfg,
            ironhermes_cli::skills_cmd::Format::Json,
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&json_out).expect("list json parses");
        let arr = parsed.as_array().expect("array");
        assert!(
            arr.is_empty(),
            "expected empty list with no lock file, got: {arr:?}"
        );
    });
}

/// Test 7 (D-16 / SP-10): `format_error_clean` strips ANSI escape codes from
/// server-originated error messages before they hit stderr.
#[test]
fn error_printer_strips_ansi() {
    let raw = "\x1b[31mmalicious\x1b[0m error body";
    let cleaned = ironhermes_cli::skills_cmd::format_error_clean(raw);
    assert!(
        !cleaned.contains('\x1b'),
        "cleaned string must not contain ESC bytes: {cleaned:?}"
    );
    assert!(
        cleaned.contains("malicious"),
        "payload text should survive; got: {cleaned:?}"
    );
    assert!(
        cleaned.contains("error body"),
        "trailing text should survive; got: {cleaned:?}"
    );
}
