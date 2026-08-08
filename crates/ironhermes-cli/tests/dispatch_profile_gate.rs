//! Phase 47.4 Plan 10 (GAP-1) — hard pre-spawn dispatch gate regression tests.
//!
//! Covers the `bdev01`-shaped 401 that UAT reproduced (a profile whose
//! `.env` carries keys for OTHER providers but not the one its
//! `model.provider` actually names), the keyless-provider carve-out, and
//! every fail-closed branch in `evaluate_profile_dispatch_at`.
//!
//! All tests build a tempdir `profiles/` root and call
//! `evaluate_profile_dispatch_at` directly — none of them touch
//! `$IRONHERMES_HOME` or the operator's real profiles.

use ironhermes_cli::kanban::dispatch_gate::{
    DispatchDecision, evaluate_profile_dispatch_at, refuse_undispatchable_ready_tasks,
};
use ironhermes_core::config::{Config, ProviderConfig};
use ironhermes_kanban::store::CreateTaskOptions;
use ironhermes_kanban::KanbanStore;
use std::fs;
use std::path::{Path, PathBuf};

/// Write a profile directory at `root/<name>/` with the given `Config` and
/// `.env` contents (empty string = no `.env` file at all).
fn write_profile(root: &Path, name: &str, config: &Config, env_contents: Option<&str>) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).expect("mkdir profile dir");
    config
        .save_to(&dir.join("config.yaml"))
        .expect("save_to config.yaml");
    if let Some(contents) = env_contents {
        fs::write(dir.join(".env"), contents).expect("write .env");
    }
    dir
}

// ---------------------------------------------------------------------------
// The bdev01 shape — the exact UAT regression.
// ---------------------------------------------------------------------------

#[test]
fn bdev01_shape_is_refused_keys_for_other_providers_present() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config = Config::default();
    config.model.provider = "moonshot".to_string();
    config.model.default = "k3-256k".to_string();
    config.providers.insert(
        "moonshot".to_string(),
        ProviderConfig {
            api_key_env: Some("MOONSHOT_API_KEY".to_string()),
            ..Default::default()
        },
    );
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            ..Default::default()
        },
    );

    // Four non-empty, high-entropy fixture keys for OTHER providers — no
    // MOONSHOT_API_KEY. A single-key or empty-.env fixture would pass
    // against the buggy code and is the exact self-verifying-test failure
    // mode this project has shipped before.
    let env = "OPENROUTER_API_KEY=sk-fixture-openrouter-8f3a2c\n\
               OPENAI_API_KEY=sk-fixture-openai-1d9e77\n\
               GROQ_API_KEY=sk-fixture-groq-4b6a90\n\
               OLLAMA_API_KEY=sk-fixture-ollama-c2e015\n";
    write_profile(tmp.path(), "bdev01", &config, Some(env));

    let decision = evaluate_profile_dispatch_at(tmp.path(), "bdev01");
    match &decision {
        DispatchDecision::Refuse { reason } => {
            assert!(
                reason.contains("moonshot"),
                "reason must name the provider; got: {reason}"
            );
            assert!(
                reason.contains("bdev01"),
                "reason must name the profile; got: {reason}"
            );
            for leaked in [
                "sk-fixture-openrouter-8f3a2c",
                "sk-fixture-openai-1d9e77",
                "sk-fixture-groq-4b6a90",
                "sk-fixture-ollama-c2e015",
            ] {
                assert!(
                    !reason.contains(leaked),
                    "reason must never leak a key value; got: {reason}"
                );
            }
        }
        DispatchDecision::Allow => panic!("bdev01-shaped profile must be Refused, got Allow"),
    }
}

// ---------------------------------------------------------------------------
// Keyless provider carve-out — a local/self-hosted endpoint is dispatchable.
// ---------------------------------------------------------------------------

#[test]
fn keyless_provider_is_allowed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config = Config::default();
    config.model.provider = "llama".to_string();
    config.model.default = "llama3.1:70b".to_string();
    config.providers.insert(
        "llama".to_string(),
        ProviderConfig {
            api_key_env: None,
            api_key: None,
            base_url: Some("http://127.0.0.1:8080/v1".to_string()),
            ..Default::default()
        },
    );
    write_profile(tmp.path(), "llama-user", &config, Some(""));

    let decision = evaluate_profile_dispatch_at(tmp.path(), "llama-user");
    assert_eq!(
        decision,
        DispatchDecision::Allow,
        "a provider that declares no key source at all must be Allowed, got {decision:?}"
    );
}

// ---------------------------------------------------------------------------
// Fail-closed edge cases (Task 2 behaviors).
// ---------------------------------------------------------------------------

#[test]
fn missing_profile_dir_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Deliberately never created.
    let decision = evaluate_profile_dispatch_at(tmp.path(), "never-created");
    assert!(
        matches!(decision, DispatchDecision::Refuse { .. }),
        "missing profile dir must be Refused, got {decision:?}"
    );
}

#[test]
fn unparseable_config_yaml_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("badyaml");
    fs::create_dir_all(&dir).expect("mkdir");
    fs::write(dir.join("config.yaml"), "not: [valid: yaml: at: all\n").expect("write bad yaml");

    let decision = evaluate_profile_dispatch_at(tmp.path(), "badyaml");
    assert!(
        matches!(decision, DispatchDecision::Refuse { .. }),
        "unparseable config.yaml must be Refused (not a panic), got {decision:?}"
    );
}

#[test]
fn malformed_env_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config = Config::default();
    config.model.provider = "openrouter".to_string();
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            ..Default::default()
        },
    );
    // A bare line with no `=`, not a comment, not blank — dotenvy rejects
    // this as a parse error.
    write_profile(
        tmp.path(),
        "badenv",
        &config,
        Some("this line has no equals sign at all\n"),
    );

    let decision = evaluate_profile_dispatch_at(tmp.path(), "badenv");
    assert!(
        matches!(decision, DispatchDecision::Refuse { .. }),
        "malformed .env must be Refused, got {decision:?}"
    );
}

/// CR-06 (D-13): the Refuse reason must not carry the failing line.
///
/// Sibling of CR-05 (`read_env_keys`) and of plan 47.4-20's
/// `self_check_parse_error_never_leaks_the_value`. Same root cause each time:
/// `dotenvy::Error::LineParse`'s `Display` (`dotenvy-0.15.7/src/errors.rs:40-44`)
/// embeds the ENTIRE raw failing line, so interpolating the error ships the secret.
///
/// This site is the most damaging of the three: `run_dispatch_loop`
/// (`ironhermes-kanban/src/dispatcher.rs:1098-1113`) puts this reason through
/// `tracing::warn!` AND `store.block_task(...)`, which PERSISTS it into the kanban
/// DB as the task's block reason — where the board then renders it. A transient
/// error response is bad; a secret written to a database and displayed is worse.
#[test]
fn refuse_reason_never_leaks_the_env_line_content() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config = Config::default();
    config.model.provider = "openrouter".to_string();
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            ..Default::default()
        },
    );

    const SENTINEL: &str = "sk-CR06-CANARY-must-never-surface";
    // An unterminated single quote — the shape a profile can already hold on
    // disk from the documented CR-02 -> CR-04 window.
    write_profile(
        tmp.path(),
        "leakyenv",
        &config,
        Some(&format!("GOOD_KEY=fine\nBAD_KEY='{SENTINEL}\n")),
    );

    let decision = evaluate_profile_dispatch_at(tmp.path(), "leakyenv");
    let DispatchDecision::Refuse { reason } = decision else {
        panic!("malformed .env must be Refused, got {decision:?}");
    };

    assert!(
        !reason.contains(SENTINEL),
        "the Refuse reason must not contain the failing line's value — it is logged \
         AND persisted to the kanban DB as the block reason (CR-06); got: {reason}"
    );
    assert!(
        !reason.contains("BAD_KEY"),
        "the Refuse reason must not name the failing line at all (CR-06); got: {reason}"
    );
    // Still actionable: the operator must learn WHICH profile to repair.
    assert!(
        reason.contains("leakyenv"),
        "the Refuse reason must still name the profile; got: {reason}"
    );
}

#[test]
fn empty_model_provider_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config = Config::default();
    config.model.provider = String::new();
    write_profile(tmp.path(), "noprovider", &config, None);

    let decision = evaluate_profile_dispatch_at(tmp.path(), "noprovider");
    assert!(
        matches!(decision, DispatchDecision::Refuse { .. }),
        "an empty model.provider must be Refused, got {decision:?}"
    );
}

#[test]
fn unknown_provider_is_refused_without_panic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config = Config::default();
    config.model.provider = "totally-made-up-provider".to_string();
    write_profile(tmp.path(), "unknownprov", &config, None);

    // The point of this test is that calling evaluate_profile_dispatch_at
    // does not panic (resolve_for_main() would panic on an unknown main
    // provider) — reaching this assertion at all is part of the proof.
    let decision = evaluate_profile_dispatch_at(tmp.path(), "unknownprov");
    assert!(
        matches!(decision, DispatchDecision::Refuse { .. }),
        "an unknown provider must be Refused without panicking, got {decision:?}"
    );
}

#[test]
fn traversal_assignee_is_refused_before_join() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // No profile directory is ever created for this — if the traversal
    // string reached a path join and somehow found a real directory outside
    // the tempdir, that would be the vulnerability this test guards against.
    let decision = evaluate_profile_dispatch_at(tmp.path(), "../../etc");
    match decision {
        DispatchDecision::Refuse { reason } => {
            assert!(
                reason.contains("not a valid profile name"),
                "traversal assignee must be refused at name validation, got: {reason}"
            );
        }
        DispatchDecision::Allow => panic!("traversal assignee must never be Allowed"),
    }
}

#[test]
fn uppercase_assignee_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let decision = evaluate_profile_dispatch_at(tmp.path(), "BadProfile");
    match decision {
        DispatchDecision::Refuse { reason } => {
            assert!(
                reason.contains("not a valid profile name"),
                "uppercase assignee must be refused at name validation, got: {reason}"
            );
        }
        DispatchDecision::Allow => panic!("uppercase assignee must never be Allowed"),
    }
}

// ---------------------------------------------------------------------------
// Idempotent reporting + gate hygiene (Task 3).
// ---------------------------------------------------------------------------

/// RAII guard that sets an env var and restores the previous value on drop.
/// Mirrors `ironhermes-kanban/src/paths.rs`'s own `ScopedEnv` test helper.
/// Safe under `cargo nextest` (one process per test) without a
/// `--test-threads=1` constraint — this is the only test in this file that
/// touches process-global `IRONHERMES_HOME`.
struct ScopedEnv {
    key: &'static str,
    prev: Option<String>,
}

impl ScopedEnv {
    fn set(key: &'static str, value: &Path) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: single-threaded within this nextest-isolated process.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        match &self.prev {
            // SAFETY: single-threaded within this nextest-isolated process.
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// Running the gate twice against the same unusable task blocks it once:
/// `refuse_undispatchable_ready_tasks` only ever selects `status='ready'`
/// tasks, so once `block_task` flips a row to `blocked` a second pass finds
/// nothing to do.
#[test]
fn gate_is_idempotent_second_pass_blocks_nothing() {
    let home = tempfile::tempdir().expect("tempdir home");
    let _guard = ScopedEnv::set("IRONHERMES_HOME", home.path());

    let profiles_root = home.path().join("profiles");
    let mut config = Config::default();
    config.model.provider = "moonshot".to_string();
    config.providers.insert(
        "moonshot".to_string(),
        ProviderConfig {
            api_key_env: Some("MOONSHOT_API_KEY".to_string()),
            ..Default::default()
        },
    );
    write_profile(&profiles_root, "idempotent-bdev", &config, Some(""));

    let db_dir = tempfile::tempdir().expect("tempdir db");
    let mut store =
        KanbanStore::open(db_dir.path().join("test.db")).expect("open kanban store");
    let task = store
        .create_task(
            "unusable task",
            "idempotent-bdev",
            CreateTaskOptions::default(),
        )
        .expect("create_task");
    assert_eq!(task.status, "ready", "a freshly created task must start ready");

    let first_pass = refuse_undispatchable_ready_tasks(&mut store);
    assert_eq!(
        first_pass.len(),
        1,
        "first pass must block exactly the one unusable task"
    );
    assert_eq!(first_pass[0].0, task.id);
    assert!(
        !first_pass[0].1.starts_with("dispatch gate: "),
        "the returned reason must be the raw underlying reason, not the \
         gate-prefixed one written to block_task — the caller adds its own \
         presentation prefix"
    );

    let blocked_task = store.get_task(&task.id).expect("get_task after first pass");
    assert_eq!(blocked_task.status, "blocked");

    let second_pass = refuse_undispatchable_ready_tasks(&mut store);
    assert!(
        second_pass.is_empty(),
        "second pass over an already-blocked task must block nothing, got {second_pass:?}"
    );
}
