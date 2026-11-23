//! Phase 54.8 Plan 10 (D-21, D-13, D-10): `ironhermes doctor` "Site deploy
//! preflight" section, exercised against the real binary.
//!
//! The tests put a tempdir FIRST on PATH holding fake `gh` scripts, so no test
//! touches the network. Fixtures use the generic `demo-site` /
//! `example-org/demo-site` names; the marker below must never appear in output.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

const MARKER: &str = "test-value-should-not-print";

const CONFIG: &str = "terminal:\n  terminal_env_allowlist: [GITHUB_TOKEN]\nsites:\n  demo-site:\n    repo: example-org/demo-site\n    clone_path: workspace/demo-site\n    deploy_mode: wrangler\n    staging:\n      deploy_args: [\"--env\", \"staging\"]\n      url: \"https://demo-site-staging.example.workers.dev\"\n    production:\n      url: \"https://demo-site.example.workers.dev\"\n";

fn write_script(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Run `ironhermes doctor` in a tempdir home whose PATH starts with `bin`.
fn run_doctor(home: &Path, bin: &Path, extra_env: &[(&str, &str)]) -> Output {
    std::fs::create_dir_all(home.join("workspace")).unwrap();
    std::fs::write(home.join("config.yaml"), CONFIG).unwrap();
    let system_path = std::env::var("PATH").unwrap_or_default();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ironhermes"));
    cmd.env("IRONHERMES_HOME", home)
        .env("PATH", format!("{}:{system_path}", bin.display()))
        .env_remove("CLOUDFLARE_API_TOKEN")
        .env_remove("CLOUDFLARE_ACCOUNT_ID")
        .env("GITHUB_TOKEN", MARKER)
        .arg("doctor");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("run ironhermes doctor")
}

#[test]
fn doctor_reports_site_preflight_section() {
    let home = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    write_script(bin.path(), "gh", "#!/bin/sh\nexit 1\n");
    let out = run_doctor(home.path(), bin.path(), &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Site deploy preflight"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("CLOUDFLARE_API_TOKEN: not set"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("CLOUDFLARE_ACCOUNT_ID: not set"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("GITHUB_TOKEN: set"), "stdout:\n{stdout}");
    assert!(stdout.contains("demo-site"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("base branch main: unknown"),
        "stdout:\n{stdout}"
    );
}

#[test]
fn doctor_never_prints_token_values() {
    let home = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    write_script(bin.path(), "gh", "#!/bin/sh\nexit 1\n");
    let out = run_doctor(home.path(), bin.path(), &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("Site deploy preflight"),
        "the section must have run:\n{stdout}"
    );
    assert!(!stdout.contains(MARKER), "stdout leaked a token value");
    assert!(!stderr.contains(MARKER), "stderr leaked a token value");
}

#[test]
fn doctor_probes_use_the_terminal_allowlist_env() {
    let home = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    let record = bin.path().join("gh-env-names.txt");
    // Records the NAMES of the variables the probe child received, never values.
    let script = format!(
        "#!/bin/sh\nenv | cut -d= -f1 >> '{}'\nexit 1\n",
        record.display()
    );
    write_script(bin.path(), "gh", &script);
    let out = run_doctor(home.path(), bin.path(), &[("IH_TEST_NOT_ALLOWLISTED", "x")]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Site deploy preflight"),
        "the section must have run"
    );
    let names = std::fs::read_to_string(&record).expect("the fake gh must have been invoked");
    let names: Vec<&str> = names.lines().collect();
    assert!(
        names.contains(&"GITHUB_TOKEN"),
        "allowlisted GITHUB_TOKEN must reach the probe; saw {names:?}"
    );
    assert!(
        !names.contains(&"IH_TEST_NOT_ALLOWLISTED"),
        "a non-allowlisted variable must not reach the probe; saw {names:?}"
    );
}
