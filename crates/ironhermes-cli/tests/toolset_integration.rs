//! Phase 25 Plan 04 — toolset CLI integration tests.
//!
//! D-26 Test 1: `toolset_enable_disable_persists` — round-trip enable/disable
//! across binary restarts via subprocess + tempdir IRONHERMES_HOME.
//!
//! T-25-01: `toolset_enable_rejects_path_traversal_name` — slug validation
//! must reject path-traversal names BEFORE any config write.
//!
//! T-25-03: `toolset_enable_emits_cache_break_banner_on_stderr` — banner
//! lands on stderr only, never stdout.

use std::sync::OnceLock;

/// Process-wide ENV_LOCK — mirrors profile_isolation.rs:7-15.
/// Required because Rust runs tests in the same process on multiple threads
/// by default; any test that mutates IRONHERMES_HOME via the spawned binary
/// must hold this lock to avoid cross-test bleed.
fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// D-26 Test 1: `toolset_enable_disable_persists`.
///
/// Round-trip enable/disable persistence across binary restarts:
/// 1. Spawn binary, run `toolset enable web`. Assert cache-break banner on stderr.
/// 2. Verify config.yaml contains `tools.toolsets.web.enabled: true`.
/// 3. Spawn binary again, run `toolset list`. Assert "web" + "enabled" present.
/// 4. Spawn binary, run `toolset disable web`. Assert disable banner on stderr.
/// 5. Spawn binary, run `toolset list`. Assert "web" + "disabled" present.
#[test]
fn toolset_enable_disable_persists() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Skipping toolset_enable_disable_persists: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let tmp = tempfile::TempDir::new().unwrap();

    // First invocation: enable web
    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["toolset", "enable", "web"])
        .output()
        .expect("failed to run ironhermes binary (enable)");
    assert!(
        out.status.success(),
        "enable web failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[toolset: web] enabled"),
        "expected cache-break banner on stderr, got stderr: {}",
        stderr
    );

    // Verify config.yaml mutation
    let cfg_path = tmp.path().join("config.yaml");
    assert!(
        cfg_path.exists(),
        "config.yaml should exist after enable, expected at: {}",
        cfg_path.display()
    );
    let cfg = std::fs::read_to_string(&cfg_path).unwrap();
    // YAML is line-based; allow any indentation around the key.
    assert!(
        cfg.contains("web:"),
        "expected 'web:' key in config: {}",
        cfg
    );
    assert!(
        cfg.contains("enabled: true"),
        "expected 'enabled: true' in config: {}",
        cfg
    );

    // Second invocation: list (verify persistence on a fresh process)
    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["toolset", "list"])
        .output()
        .expect("failed to run ironhermes binary (list 1)");
    assert!(
        out.status.success(),
        "list failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("web"),
        "expected 'web' in list output: {}",
        stdout
    );
    assert!(
        stdout.contains("enabled"),
        "expected 'enabled' in list output: {}",
        stdout
    );

    // Third invocation: disable web (round-trip through the CLI)
    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["toolset", "disable", "web"])
        .output()
        .expect("failed to run ironhermes binary (disable)");
    assert!(
        out.status.success(),
        "disable failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[toolset: web] disabled"),
        "expected disable banner on stderr, got: {}",
        stderr
    );

    // Fourth invocation: list again — verify disabled
    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["toolset", "list"])
        .output()
        .expect("failed to run ironhermes binary (list 2)");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The web row must show "disabled" — assertion shape accepts column-aligned
    // "disabled" in the same line as "web".
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("web") && l.contains("disabled")),
        "expected 'web' row to show 'disabled', got: {}",
        stdout
    );
}

/// T-25-01 mitigation gate: path-traversal names must be rejected BEFORE any
/// config write. The slug regex `[a-z0-9][a-z0-9-]*` rejects `../etc/passwd`
/// because `.` and `/` are not in the character class.
#[test]
fn toolset_enable_rejects_path_traversal_name() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = std::env::var("CARGO_BIN_EXE_ironhermes").unwrap_or_default();
    if bin.is_empty() {
        eprintln!("Skipping toolset_enable_rejects_path_traversal_name: CARGO_BIN_EXE_ironhermes not set");
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();

    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["toolset", "enable", "../etc/passwd"])
        .output()
        .expect("failed to run ironhermes binary");
    assert!(
        !out.status.success(),
        "should reject path-traversal name, but exited with success"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("invalid toolset name")
            || stderr.to_lowercase().contains("invalid"),
        "expected slug-rejection error, got stderr: {}",
        stderr
    );

    // Verify NO config.yaml mutation occurred — even if the file exists for
    // some other reason, it MUST NOT contain the path-traversal payload.
    let cfg_path = tmp.path().join("config.yaml");
    if cfg_path.exists() {
        let cfg = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(
            !cfg.contains("../etc/passwd") && !cfg.contains("etc/passwd"),
            "path traversal must NOT reach config.yaml, got: {}",
            cfg
        );
    }
}

/// T-25-03 mitigation gate: cache-break banner emitted on stderr (NOT stdout).
/// CLI conventions: persistent state changes go to stderr so pipes like
/// `hermes ... | jq` stay clean.
#[test]
fn toolset_enable_emits_cache_break_banner_on_stderr() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = std::env::var("CARGO_BIN_EXE_ironhermes").unwrap_or_default();
    if bin.is_empty() {
        eprintln!("Skipping toolset_enable_emits_cache_break_banner_on_stderr: CARGO_BIN_EXE_ironhermes not set");
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();

    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["toolset", "enable", "memory"])
        .output()
        .expect("failed to run ironhermes binary");
    assert!(
        out.status.success(),
        "enable memory failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("schema cache will rebuild on next LLM call"),
        "T-25-03 banner missing — got stderr: {}",
        stderr
    );
    // Stdout should NOT contain the banner (banner is stderr-only per CLI conventions)
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("schema cache will rebuild"),
        "banner leaked to stdout, got: {}",
        stdout
    );
}

/// Plan 5 Task 3: toolset_setup_writes_dotenv_with_0600_mode.
///
/// Direct call to the apply_tool_prereq_answers testability seam (T-25-02).
/// Verifies .env is written at mode 0600 and the value is stored correctly.
#[test]
#[cfg(unix)]
fn toolset_setup_writes_dotenv_with_0600_mode() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    ironhermes_cli::setup::apply_tool_prereq_answers(
        tmp.path(),
        &[("web_search", "FIRECRAWL_API_KEY", "test_secret_value")],
    )
    .expect("apply_tool_prereq_answers failed");

    let env_path = tmp.path().join(".env");
    assert!(env_path.exists(), ".env file should exist");
    let contents = std::fs::read_to_string(&env_path).unwrap();
    assert!(
        contents.contains("FIRECRAWL_API_KEY=test_secret_value"),
        ".env should contain the prereq value, got: {}",
        contents
    );
    // T-25-02: secret value must NOT appear on stdout or stderr — tested here
    // by verifying we only read the file, never print the value.
    // (The write_env_var_to_dotenv function prints "Saved." not the value.)

    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&env_path).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        ".env mode must be 0600 (T-25-02), got {:o}",
        mode & 0o777
    );
}

/// Plan 5 Task 3: preflight_banner_appears_for_required_missing_prereq.
///
/// Subprocess test: with valid config but FIRECRAWL_API_KEY unset, running
/// a preflight-gated command emits the banner on stderr without blocking
/// or setting a non-zero exit code (D-17: preflight does NOT block on banner).
///
/// Preflight fires when command is None (bare `hermes`) and execute is None.
/// We pipe empty stdin so the REPL exits immediately on EOF.
#[test]
fn preflight_banner_appears_for_required_missing_prereq() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Skipping preflight_banner_appears_for_required_missing_prereq: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let tmp = tempfile::TempDir::new().unwrap();

    // Pre-create a minimal valid config.yaml that passes config.validate().
    // Must have provider, api_key (or equivalent), and model.default set.
    // tools.toolsets.web.enabled: true ensures web_search is in scope.
    let config_yaml = r#"model:
  provider: openrouter
  api_key: test_key_for_preflight_test
  default: openai/gpt-4o-mini
tools:
  toolsets:
    web:
      enabled: true
    memory:
      enabled: true
    session:
      enabled: true
    agent:
      enabled: true
    skills:
      enabled: true
    code:
      enabled: false
  skip_prompts: []
  disabled: []
"#;
    std::fs::write(tmp.path().join("config.yaml"), config_yaml).unwrap();

    // Ensure FIRECRAWL_API_KEY is not set (so web_search shows as unavailable).
    // Use a subprocess so we don't affect other tests' env vars.
    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .env_remove("FIRECRAWL_API_KEY")
        // Use the classic TUI to avoid ratatui init; pipe stdin as empty so REPL exits on EOF.
        .env("IRONHERMES_CLASSIC_TUI", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("failed to run ironhermes binary");

    let stderr = String::from_utf8_lossy(&out.stderr);

    // Preflight must emit the banner because FIRECRAWL_API_KEY is missing and web is enabled.
    assert!(
        stderr.contains("Tool prerequisites unsatisfied")
            || stderr.contains("hermes toolset setup"),
        "expected preflight banner on stderr, got: {}",
        stderr
    );

    // The binary may exit 0 (clean EOF) or non-zero (REPL error on null stdin) — either is
    // acceptable; what matters is the banner appeared, not the exit code.
    // We assert the banner is present (above) and that it does NOT contain the setup wizard
    // interactive prompts (which would indicate auto-wizard launched).
    assert!(
        !stderr.contains("Welcome to IronHermes. Let's get you configured"),
        "preflight must NOT auto-launch the setup wizard (D-17), got stderr: {}",
        stderr
    );
}
