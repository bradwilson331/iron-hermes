//! Phase 54.8 Plan 08 (D-20, D-12): `ironhermes sites list|show` subprocess tests.
//!
//! Uses the compile-time `CARGO_BIN_EXE_ironhermes` path and a tempdir
//! `IRONHERMES_HOME`. Fixtures use generic names only.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

const VALID_SITE: &str = "  demo-site:\n    repo: example-org/demo-site\n    clone_path: workspace/demo-site\n    deploy_mode: wrangler\n    staging:\n      deploy_args: [\"--env\", \"staging\"]\n      url: \"https://demo-site-staging.example.workers.dev\"\n    production:\n      url: \"https://demo-site.example.workers.dev\"\n";

fn seed(home: &Path, extra_sites: &str) {
    std::fs::create_dir_all(home.join("workspace")).unwrap();
    let cfg = format!("sites:\n{VALID_SITE}{extra_sites}");
    std::fs::write(home.join("config.yaml"), cfg).unwrap();
}

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ironhermes"))
        .env("IRONHERMES_HOME", home)
        .args(args)
        .output()
        .expect("run ironhermes")
}

#[test]
fn cli_show_unknown_site_exits_1() {
    let tmp = TempDir::new().unwrap();
    seed(tmp.path(), "");
    let out = run(tmp.path(), &["sites", "show", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("site 'nope' not found in config"));
}

#[test]
fn cli_show_invalid_site_exits_2() {
    let tmp = TempDir::new().unwrap();
    seed(
        tmp.path(),
        "  broken:\n    repo: x\n    deploy_mode: bogus\n",
    );
    let out = run(tmp.path(), &["sites", "show", "broken"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("broken"), "stderr: {stderr}");
    assert!(stderr.contains("entry"), "stderr: {stderr}");
}

#[cfg(unix)]
#[test]
fn cli_show_symlinked_clone_exits_2() {
    let tmp = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    seed(tmp.path(), "");
    std::os::unix::fs::symlink(outside.path(), tmp.path().join("workspace/demo-site")).unwrap();
    let out = run(tmp.path(), &["sites", "show", "demo-site"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("clone_path"), "stderr: {stderr}");
    assert!(stderr.contains("symlink"), "stderr: {stderr}");
    // The same check guards `--json`, which the skill reads.
    let out = run(tmp.path(), &["sites", "show", "demo-site", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn cli_list_reports_invalid_by_field() {
    let tmp = TempDir::new().unwrap();
    seed(
        tmp.path(),
        "  broken:\n    repo: x\n    deploy_mode: bogus\n",
    );
    let out = run(tmp.path(), &["sites", "list"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("demo-site"), "stdout: {stdout}");
    let broken = stdout
        .lines()
        .find(|l| l.starts_with("broken"))
        .unwrap_or_else(|| panic!("no broken line in: {stdout}"));
    assert!(broken.contains("INVALID"), "line: {broken}");
    assert!(broken.contains("entry"), "line: {broken}");
}
