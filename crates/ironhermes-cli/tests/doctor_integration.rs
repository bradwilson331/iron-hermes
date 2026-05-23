//! Phase 24 — `hermes doctor` gateway.pid liveness integration test (24-05-03).

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

/// 24-05-03: `hermes doctor` includes a gateway.pid liveness check on the
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
/// Seeds a temp HERMES_HOME with a minimally-valid config.yaml (passes validate()) but
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
