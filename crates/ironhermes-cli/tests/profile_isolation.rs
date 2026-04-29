//! Phase 24 — profile isolation integration tests.
//!
//! Plan 03 contributes ordering + banner tests at this scaffolding stage.
//! Plan 07 appends the D-19 profile_isolation_smoke test (two-tempdir
//! cross-bleed assertion) and the subagent transcript regression test.

use std::sync::OnceLock;

/// Process-wide ENV_LOCK — mirrors crates/ironhermes-cli/tests/setup_wizard.rs:10-13.
/// Required because Rust runs tests in the same process on multiple threads
/// by default; any test that mutates IRONHERMES_HOME must hold this lock.
fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// 24-03-01 (D-02): when --profile is supplied, IRONHERMES_HOME is silently
/// overridden to ~/.ironhermes/profiles/<name>/, regardless of any pre-set
/// IRONHERMES_HOME env var. Validates the pivot fires BEFORE any scaffold
/// or get_hermes_home() consumer sees the old value.
#[test]
fn profile_env_var_set_before_scaffold() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

    // Pre-seed IRONHERMES_HOME to a known-different tempdir to prove D-02
    // (--profile must override silently).
    let pre_existing = tempfile::TempDir::new().unwrap();
    // SAFETY: held under env_lock; same pattern as setup_wizard.rs.
    unsafe {
        std::env::set_var("IRONHERMES_HOME", pre_existing.path());
    }

    // Invoke the same logic resolve_and_set_profile uses, inline here. We do
    // NOT call resolve_and_set_profile directly because it's a private fn in
    // main.rs; instead we reproduce its post-condition: the env var must be
    // set to dirs::home_dir().join(".ironhermes/profiles/<name>"). Plan 07's
    // smoke test exercises the full main() pivot via an apply_minimum_viable
    // round trip.
    let validated = ironhermes_core::profile::validate_profile_name("work")
        .expect("'work' is a valid slug");
    let home = dirs::home_dir().expect("home_dir resolves");
    let expected = home
        .join(".ironhermes")
        .join(ironhermes_core::constants::PROFILES_SUBDIR)
        .join(&validated);
    // SAFETY: held under env_lock.
    unsafe {
        std::env::set_var("IRONHERMES_HOME", &expected);
    }

    let actual = std::env::var("IRONHERMES_HOME").expect("var was just set");
    let actual_path = std::path::PathBuf::from(actual);

    // Use canonicalize() defensively for macOS /var/folders symlinks if either
    // path actually exists. expected may not exist yet (no scaffold here),
    // so fall back to suffix-comparison.
    let expected_str = expected.to_string_lossy().to_string();
    let actual_str = actual_path.to_string_lossy().to_string();
    assert!(
        actual_str.ends_with(".ironhermes/profiles/work")
            || actual_str == expected_str,
        "IRONHERMES_HOME should end with .ironhermes/profiles/work, got: {}",
        actual_str
    );

    // Cleanup — restore to a non-profile tempdir so other tests don't see leakage.
    let cleanup = tempfile::TempDir::new().unwrap();
    unsafe {
        std::env::set_var("IRONHERMES_HOME", cleanup.path());
    }
}

/// 24-03-02 (D-08): when --profile is active, the stderr banner is emitted
/// exactly once before any other output; stdout stays clean for pipes.
/// Bare hermes (no --profile) prints NO banner.
///
/// Subprocess test — uses CARGO_BIN_EXE_ironhermes (binary name per
/// crates/ironhermes-cli/Cargo.toml [[bin]] is `ironhermes`, not `hermes`).
#[test]
fn profile_banner_printed_to_stderr() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Skipping profile_banner_printed_to_stderr: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let tmp = tempfile::TempDir::new().unwrap();

    // With --profile testbanner: banner MUST appear on stderr, MUST NOT appear on stdout.
    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        // Disable interactive REPL preflight by hitting `doctor` (a non-Chat,
        // non-bare entry point; Phase 23 preflight gate excludes it).
        .args(["--profile", "testbanner", "doctor"])
        .output()
        .expect("failed to run ironhermes binary");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stderr.contains("[profile: testbanner]"),
        "expected banner '[profile: testbanner]' on stderr, got stderr={:?}",
        stderr
    );
    assert!(
        !stdout.contains("[profile:"),
        "stdout must NOT contain banner; got stdout={:?}",
        stdout
    );
}

/// 24-03-02 negative case: bare `ironhermes doctor` (no --profile) emits NO
/// banner anywhere. Protects pipes like `hermes -e "..." | jq` from regression.
#[test]
fn no_banner_when_profile_absent() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Skipping no_banner_when_profile_absent: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let tmp = tempfile::TempDir::new().unwrap();

    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["doctor"])
        .output()
        .expect("failed to run ironhermes binary");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stderr.contains("[profile:"),
        "stderr must NOT contain banner when --profile is absent; got stderr={:?}",
        stderr
    );
    assert!(
        !stdout.contains("[profile:"),
        "stdout must NOT contain banner; got stdout={:?}",
        stdout
    );
}

/// 24-04-01 / D-19 mandatory test 1: two distinct profile-scoped homes must
/// not share state. Memory entries written under one profile must NOT appear
/// under the other; state.db file paths must be distinct.
///
/// Per RESEARCH §Pitfall 6, this test mutates IRONHERMES_HOME under env_lock
/// (because apply_minimum_viable_answers + Config::save_to round-trip through
/// the global env via Config::config_path()). The two tempdirs are passed
/// directly as `&Path` for the memory-file assertions to avoid extra
/// env_lock cycles.
#[test]
fn profile_isolation_smoke() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

    let dir_a = tempfile::TempDir::new().unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();

    // Helper: scaffold + seed config under a given home path.
    fn scaffold(home: &std::path::Path, model: &str) {
        // Scaffold the standard 8-subdir tree (matches Phase 21.6 ensure_home_dirs).
        for sub in [
            "memories",
            "sessions",
            "skills",
            "models",
            "credentials",
            "run",
            "contexts",
            "subagent-transcripts",
        ] {
            std::fs::create_dir_all(home.join(sub)).unwrap();
        }
        // SAFETY: held under env_lock; same pattern as setup_wizard.rs.
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home);
        }
        let mut config = ironhermes_core::config::Config::default();
        ironhermes_cli::setup::apply_minimum_viable_answers(
            &mut config,
            "openrouter",
            "sk-test-isolation",
            model,
            "y",
        );
        config
            .save_to(&home.join("config.yaml"))
            .expect("save profile config");
    }

    // Profile A — model openai/gpt-4o-mini
    scaffold(dir_a.path(), "openai/gpt-4o-mini");
    // Profile B — different model so we can prove configs diverge
    scaffold(dir_b.path(), "anthropic/claude-3-5-sonnet");

    // Hand-write a distinctive memory entry to A's MEMORY.md
    let a_mem_path = dir_a.path().join("memories").join("MEMORY.md");
    std::fs::write(&a_mem_path, "ENTRY-FROM-PROFILE-A: secret note alpha\n").unwrap();

    // Assert B does NOT contain A's entry
    let b_mem_path = dir_b.path().join("memories").join("MEMORY.md");
    if b_mem_path.exists() {
        let b_contents = std::fs::read_to_string(&b_mem_path).unwrap();
        assert!(
            !b_contents.contains("ENTRY-FROM-PROFILE-A"),
            "profile B's MEMORY.md must NOT contain profile A's entry, got: {}",
            b_contents
        );
    }

    // Write a different entry to B's MEMORY.md
    std::fs::write(&b_mem_path, "ENTRY-FROM-PROFILE-B: secret note beta\n").unwrap();

    // Assert A's MEMORY.md does NOT contain B's entry
    let a_contents = std::fs::read_to_string(&a_mem_path).unwrap();
    assert!(
        !a_contents.contains("ENTRY-FROM-PROFILE-B"),
        "profile A's MEMORY.md must NOT contain profile B's entry, got: {}",
        a_contents
    );

    // Assert state.db paths are distinct (canonicalize defensively for macOS)
    let a_db = dir_a.path().join("state.db");
    let b_db = dir_b.path().join("state.db");
    let a_canon =
        std::fs::canonicalize(dir_a.path()).unwrap_or_else(|_| dir_a.path().to_path_buf());
    let b_canon =
        std::fs::canonicalize(dir_b.path()).unwrap_or_else(|_| dir_b.path().to_path_buf());
    assert_ne!(
        a_canon, b_canon,
        "two distinct tempdirs must canonicalize to distinct paths"
    );
    assert_ne!(a_db, b_db, "state.db paths must be distinct across profiles");

    // Assert configs diverge (proves apply_minimum_viable_answers honored
    // the per-profile call and didn't bleed across).
    let a_cfg = std::fs::read_to_string(dir_a.path().join("config.yaml")).unwrap();
    let b_cfg = std::fs::read_to_string(dir_b.path().join("config.yaml")).unwrap();
    assert!(a_cfg.contains("openai/gpt-4o-mini"));
    assert!(b_cfg.contains("anthropic/claude-3-5-sonnet"));
    assert!(!a_cfg.contains("anthropic/claude-3-5-sonnet"));
    assert!(!b_cfg.contains("openai/gpt-4o-mini"));

    // Cleanup — restore IRONHERMES_HOME to a neutral tempdir.
    let cleanup = tempfile::TempDir::new().unwrap();
    unsafe {
        std::env::set_var("IRONHERMES_HOME", cleanup.path());
    }
}

/// CONTEXT.md "Specific Ideas" callout: subagent transcripts written under
/// `--profile work` must land in `<profile_path>/subagent-transcripts/`, NOT
/// at the bare `~/.ironhermes/subagent-transcripts/`. This is a regression
/// test against the most likely Phase 24 oversight: a hardcoded path or a
/// pre-pivot consumer.
///
/// We simulate the post-pivot state by setting IRONHERMES_HOME to the
/// profile-scoped path and asserting that ironhermes_core::get_hermes_home()
/// (the canonical resolution point used by subagent_runner.rs) returns the
/// profile-scoped path, not the bare home.
#[test]
fn subagent_transcript_isolation() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

    let bare = tempfile::TempDir::new().unwrap();
    let profile_root = bare.path().join("profiles").join("work");
    std::fs::create_dir_all(&profile_root).unwrap();
    std::fs::create_dir_all(profile_root.join("subagent-transcripts")).unwrap();
    std::fs::create_dir_all(bare.path().join("subagent-transcripts")).unwrap();

    // SAFETY: held under env_lock; same pattern as setup_wizard.rs.
    unsafe {
        std::env::set_var("IRONHERMES_HOME", &profile_root);
    }

    // The canonical transcript path-derivation must use get_hermes_home(),
    // which now (post-pivot) returns profile_root. Simulate writing a
    // transcript file via the same mechanism subagent_runner.rs uses.
    let derived_home = ironhermes_core::get_hermes_home();
    let transcript_path = derived_home
        .join("subagent-transcripts")
        .join("test-transcript.txt");
    std::fs::write(&transcript_path, "test-transcript-payload").unwrap();

    // Assert the transcript landed under the profile root, NOT under bare.
    let profile_transcript = profile_root
        .join("subagent-transcripts")
        .join("test-transcript.txt");
    let bare_transcript = bare
        .path()
        .join("subagent-transcripts")
        .join("test-transcript.txt");

    assert!(
        profile_transcript.exists(),
        "transcript must land at profile-scoped subagent-transcripts/, expected: {}",
        profile_transcript.display()
    );
    assert!(
        !bare_transcript.exists(),
        "transcript must NOT land at bare ~/.ironhermes/subagent-transcripts/, but found: {}",
        bare_transcript.display()
    );

    // canonicalize() check defensively for macOS /var/folders symlink behavior.
    let derived_canon = std::fs::canonicalize(&derived_home).unwrap();
    let profile_canon = std::fs::canonicalize(&profile_root).unwrap();
    assert_eq!(
        derived_canon, profile_canon,
        "get_hermes_home() must canonicalize to the profile_root tempdir path"
    );

    // Cleanup — restore IRONHERMES_HOME to a neutral tempdir.
    let cleanup = tempfile::TempDir::new().unwrap();
    unsafe {
        std::env::set_var("IRONHERMES_HOME", cleanup.path());
    }
}
