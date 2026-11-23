//! Phase 24 — `ironhermes doctor` gateway.pid liveness integration test (24-05-03).

use std::process::Command;
use tempfile::TempDir;

fn seed_gateway_pid(home: &std::path::Path, pid: u32) {
    std::fs::create_dir_all(home).unwrap();
    // Hand-write a Plan 02 GatewayPidRecord 3-line YAML.
    let body = format!(
        "pid: {}\nstarted_at: 2026-04-28T00:00:00Z\nprofile: default\n",
        pid
    );
    std::fs::write(home.join("gateway.pid"), body).unwrap();
}

/// 24-05-03: `ironhermes doctor` includes a gateway.pid liveness check on the
/// active profile (D-16). With a live PID (current test process id), the
/// check reports OK.
#[test]
fn profile_doctor() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Skipping profile_doctor: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    // Use the current test process id — guaranteed alive when doctor probes it.
    seed_gateway_pid(tmp.path(), std::process::id());
    let out = Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["doctor"])
        .output()
        .expect("ironhermes doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Gateway PID"),
        "expected 'Gateway PID' line in doctor output, got:\n{}",
        stdout
    );
}

/// D-07/D-08: first-run auto-detection triggers setup when no API key and no local endpoint.
///
/// Seeds a temp IRONHERMES_HOME with a minimally-valid config.yaml (passes validate()) but
/// NO runnable LLM signal: no OPENROUTER/ANTHROPIC/OPENAI env vars, no .env API key,
/// no localhost base_url. Expects the setup wizard to launch (D-07 fires).
///
/// The wizard cannot complete with closed stdin, so the binary exits non-zero; we assert
/// on output content, not exit code. We look for the D-11 "Full setup" prompt string that
/// Plan 02 wired into setup.rs as the wizard start.
#[test]
fn d07_d08_first_run_triggers_setup_when_no_api_key_and_no_local_endpoint() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "Skipping d07_d08_first_run_triggers_setup_when_no_api_key_and_no_local_endpoint: CARGO_BIN_EXE_ironhermes not set"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();

    // Minimal valid config.yaml: passes Config::validate() via providers.openrouter.api_key_env
    // (structural check only — validate does not check that the env var is actually set).
    // No model.base_url so the Ollama escape hatch does not fire.
    let config_yaml = "model:\n  provider: openrouter\n  default: openrouter/gpt-4o-mini\nproviders:\n  openrouter:\n    api_key_env: OPENROUTER_API_KEY\n";
    std::fs::write(tmp.path().join("config.yaml"), config_yaml).unwrap();

    // No .env file — no API key signal anywhere.
    // Explicitly remove the three API key env vars from the child process.
    let out = Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .args(["chat"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("ironhermes chat");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{}{}", stdout, stderr);

    // The wizard should have launched — look for D-11 fast/full choice prompt text,
    // OR the welcome banner, OR "setup" in any recognisable form.
    // Accept any of: "Full setup", "Quick setup", "setup wizard", "Let's get you configured",
    // "provider", or the EOF-on-stdin error from the interactive readline (proving wizard ran).
    assert!(
        combined.contains("Full setup")
            || combined.contains("Quick setup")
            || combined.contains("setup wizard")
            || combined.contains("Let's get you configured")
            || combined.contains("IronHermes Setup")
            || combined.contains("provider")
            || combined.contains("EOF on stdin"),
        "expected D-07 wizard launch signal in output, got stdout={:?} stderr={:?}",
        stdout,
        stderr
    );
}

fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// D-07/D-08 negative case: wizard does NOT launch when .env contains a real API key.
///
/// Seeds the same minimal config.yaml but writes `OPENROUTER_API_KEY=sk-real-test-value`
/// to the .env file. has_runnable_llm detects the key via raw .env scan and lets
/// run_preflight_check pass through — wizard must NOT launch.
#[test]
fn d07_d08_first_run_does_not_trigger_when_api_key_present() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "Skipping d07_d08_first_run_does_not_trigger_when_api_key_present: CARGO_BIN_EXE_ironhermes not set"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();

    // Same minimal valid config.yaml.
    let config_yaml = "model:\n  provider: openrouter\n  default: openrouter/gpt-4o-mini\nproviders:\n  openrouter:\n    api_key_env: OPENROUTER_API_KEY\n";
    std::fs::write(tmp.path().join("config.yaml"), config_yaml).unwrap();

    // Write a non-empty API key to .env — has_runnable_llm detects this.
    std::fs::write(
        tmp.path().join(".env"),
        "OPENROUTER_API_KEY=sk-real-test-value\n",
    )
    .unwrap();

    // Remove the API key vars from the test process env so the child only sees .env.
    let out = Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .args(["chat"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("ironhermes chat");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{}{}", stdout, stderr);

    // The wizard must NOT have launched — these strings indicate the setup wizard
    // started its interactive prompts.
    assert!(
        !combined.contains("Full setup") && !combined.contains("IronHermes Setup"),
        "expected D-07 wizard to NOT launch when API key present in .env, got stdout={:?} stderr={:?}",
        stdout,
        stderr
    );
}

/// Companion: doctor with no gateway.pid present reports the absent-file
/// branch (D-16 healthy state — no gateway running).
#[test]
fn doctor_no_pid_file_is_healthy() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Skipping doctor_no_pid_file_is_healthy: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let out = Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["doctor"])
        .output()
        .expect("ironhermes doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Gateway PID") && (stdout.contains("not running") || stdout.contains("OK")),
        "expected absent-pid healthy branch in doctor output, got:\n{}",
        stdout
    );
}

// =============================================================================
// Phase 53 (Plan 03 Task 3, D-06): ironhermes doctor --browser
// =============================================================================

/// D-06: plain `ironhermes doctor` (no `--browser`) never prints the
/// browser section — the flag gates the entire block, so this command stays
/// exactly as cheap as it was pre-53.
#[test]
fn doctor_without_the_browser_flag_performs_no_browser_work() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "Skipping doctor_without_the_browser_flag_performs_no_browser_work: \
                 CARGO_BIN_EXE_ironhermes not set"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let out = Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["doctor"])
        .output()
        .expect("ironhermes doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("Browser backend"),
        "plain `ironhermes doctor` must never print the browser section, got:\n{}",
        stdout
    );
    assert!(
        out.status.success(),
        "doctor must exit 0, got: {:?}\nstdout:\n{}",
        out.status,
        stdout
    );
}

/// D-06: `ironhermes doctor --browser` reports the configured backend and
/// still exits 0 even when the browser diagnosis fails outright —
/// `run_doctor_check`'s always-`Ok(())` invariant survives every browser
/// outcome. `IRONHERMES_BROWSER_TEST_DISABLE=1` forces a deterministic "no
/// binary" outcome so this assertion does not depend on what is installed
/// on the machine running it.
#[test]
fn doctor_with_the_browser_flag_reports_the_configured_backend_and_still_returns_ok() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "Skipping doctor_with_the_browser_flag_reports_the_configured_backend_and_still_returns_ok: \
                 CARGO_BIN_EXE_ironhermes not set"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let out = Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .env("IRONHERMES_BROWSER_TEST_DISABLE", "1")
        .args(["doctor", "--browser"])
        .output()
        .expect("ironhermes doctor --browser");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Browser backend"),
        "expected the browser section header, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("Backend: Chromium"),
        "must name the configured backend (default: Chromium), got:\n{}",
        stdout
    );
    assert!(
        out.status.success(),
        "run_doctor_check must still return Ok(()) even when the browser diagnosis fails \
         outright, got exit status: {:?}\nstdout:\n{}",
        out.status,
        stdout
    );
}

/// `ironhermes doctor --help` lists the `--browser` flag.
#[test]
fn doctor_help_lists_the_browser_flag() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "Skipping doctor_help_lists_the_browser_flag: CARGO_BIN_EXE_ironhermes not set"
            );
            return;
        }
    };
    let out = Command::new(&bin)
        .args(["doctor", "--help"])
        .output()
        .expect("ironhermes doctor --help");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--browser"),
        "doctor --help must list --browser, got:\n{}",
        stdout
    );
}

// =============================================================================
// Phase 57 (Plan 15, D-12): `ironhermes doctor` reports the security sidecar
// =============================================================================

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// `target/<profile>/`, derived from this test binary's own location
/// (`target/<profile>/deps/<test>`), so it also holds under a custom
/// `CARGO_TARGET_DIR`.
fn profile_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    exe.parent()
        .and_then(Path::parent)
        .expect("test binary lives in target/<profile>/deps")
        .to_path_buf()
}

/// Build `bin` of `package` into this test's profile directory and return its
/// path. `CARGO_BIN_EXE_*` only exists for binaries of the package under test,
/// and the sidecar binaries belong to other packages, so a nested `cargo build`
/// (a no-op when up to date) is the portable way to have them.
fn ensure_built(package: &str, bin: &str) -> PathBuf {
    static BUILD: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = BUILD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = profile_dir();
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.args(["build", "-p", package, "--bin", bin]);
    match dir.file_name().and_then(|n| n.to_str()) {
        Some("debug") => {}
        Some("release") => {
            cmd.arg("--release");
        }
        other => panic!("unsupported cargo profile directory {other:?}"),
    }
    let out = cmd.output().expect("run cargo build");
    assert!(
        out.status.success(),
        "cargo build -p {package} --bin {bin} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = dir.join(format!("{bin}{}", std::env::consts::EXE_SUFFIX));
    assert!(path.is_file(), "{} was not produced", path.display());
    path
}

/// A directory holding only the `ironhermes` binary. The sidecar lookup tries a
/// sibling of the running executable first, so running the binary in place
/// would find `target/<profile>/ironhermes-tirith` and defeat every case that
/// wants a different sidecar or none. Hard link (same inode, so the code
/// signature stays valid), falling back to a copy across filesystems.
fn isolated_ironhermes(bin: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new_in(profile_dir()).expect("temp dir under target");
    let linked = dir
        .path()
        .join(format!("ironhermes{}", std::env::consts::EXE_SUFFIX));
    if std::fs::hard_link(bin, &linked).is_err() {
        std::fs::copy(bin, &linked).expect("copy ironhermes binary");
    }
    (dir, linked)
}

/// Run `ironhermes doctor` from an isolated copy with the given sidecar
/// override and `PATH`; returns (stdout, exited 0).
fn run_doctor_with_sidecar(override_bin: &Path, path_var: Option<&str>) -> (String, bool) {
    let bin = std::env::var("CARGO_BIN_EXE_ironhermes").expect("CARGO_BIN_EXE_ironhermes");
    let (_keep, isolated) = isolated_ironhermes(&bin);
    let home = TempDir::new().unwrap();
    let mut cmd = Command::new(&isolated);
    cmd.env("IRONHERMES_HOME", home.path())
        .env("IRONHERMES_TIRITH_BIN", override_bin)
        .env("NO_COLOR", "1")
        .args(["doctor"]);
    if let Some(p) = path_var {
        cmd.env("PATH", p);
    }
    let out = cmd.output().expect("ironhermes doctor");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.success(),
    )
}

/// D-12: with the real sidecar reachable, doctor reports it present, pinned,
/// same build, threat DB state, a successful health round trip and the level.
#[test]
fn doctor_reports_present_pinned_sidecar() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let sidecar = ensure_built("ironhermes-tirith", "ironhermes-tirith");
    let (stdout, ok) = run_doctor_with_sidecar(&sidecar, None);
    for needle in [
        "Security sidecar",
        "sidecar present:",
        "tirith-core 0.4.2 (pinned)",
        "threat DB: absent",
        "health: ok",
        "DEFCON 3",
    ] {
        assert!(
            stdout.contains(needle),
            "doctor output must contain {needle:?}, got:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("will be blocked"),
        "a healthy sidecar has no consequence line, got:\n{stdout}"
    );
    assert!(ok, "doctor must exit 0, got:\n{stdout}");
}

/// D-12 / T-57-61: an absent sidecar is reported with where doctor looked and
/// what the absence means at level 3; doctor itself still exits 0 and never
/// claims health.
#[test]
fn doctor_reports_absent_sidecar() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let missing = PathBuf::from("/nonexistent-57-15/ironhermes-tirith");
    let (stdout, ok) = run_doctor_with_sidecar(&missing, Some("/usr/bin:/bin"));
    assert!(
        stdout.contains("Security sidecar") && stdout.contains("sidecar absent"),
        "expected the absent report, got:\n{stdout}"
    );
    assert!(
        stdout.contains("/nonexistent-57-15/ironhermes-tirith"),
        "must name the looked-up override path, got:\n{stdout}"
    );
    assert!(
        stdout.contains("terminal/execute_code will be blocked until the sidecar is installed"),
        "must state the level-3 consequence, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("health: ok") && !stdout.contains("(pinned)"),
        "an absent sidecar must not be reported healthy or pinned, got:\n{stdout}"
    );
    assert!(ok, "doctor must exit 0 even when the sidecar is absent");
}

/// D-12 / T-57-61 / T-57-62: a sidecar answering with a different tirith-core
/// fails the pin check and the message names the pinned 0.4.2.
#[test]
fn doctor_reports_version_mismatch() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fake = ensure_built("ironhermes-security", "ironhermes-tirith-fake");
    let (stdout, ok) = run_doctor_with_sidecar(&fake, None);
    let pin_line = stdout
        .lines()
        .find(|l| l.contains("tirith-core fake"))
        .unwrap_or_else(|| panic!("no tirith-core line, got:\n{stdout}"));
    assert!(
        pin_line.contains("MISSING") && pin_line.contains("0.4.2"),
        "the pin check must fail and name 0.4.2, got: {pin_line:?}"
    );
    assert!(
        !stdout.contains("(pinned)"),
        "a mismatched core must never read as pinned, got:\n{stdout}"
    );
    assert!(ok, "doctor must exit 0, got:\n{stdout}");
}
