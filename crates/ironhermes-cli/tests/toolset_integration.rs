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
