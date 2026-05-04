//! Phase 21.7 Plan 09 Task 9-02 — `hermes status` integration contract.
//!
//! Locks the default-text `format_styled` output under insta (E-10) and
//! exercises the JSON path through `StatusReport::fixture` so changes to
//! the default styling are flagged before they ship.
//!
//! The fixture provides deterministic bytes; real runs through
//! `StatusReport::collect` are covered by status_cmd_deep_probe.rs.

use ironhermes_cli::status_cmd::{StatusReport, format_styled};

#[test]
fn default_text_output_e10_snapshot() {
    // Force colored off for deterministic bytes in the snapshot. Without
    // this, the snapshot diffs depending on terminal color detection.
    colored::control::set_override(false);
    let snap = StatusReport::fixture();
    let text = format_styled(&snap);
    insta::assert_snapshot!("status_default_text", text);
}

#[test]
fn default_text_output_contains_every_section_header() {
    colored::control::set_override(false);
    let snap = StatusReport::fixture();
    let text = format_styled(&snap);
    assert!(text.contains("Provider"), "Provider section missing");
    assert!(text.contains("Memory"), "Memory section missing");
    assert!(text.contains("Gateway"), "Gateway section missing");
    // D-18 folds subagents/processes/mcp into one section header.
    assert!(
        text.contains("Subagents") || text.contains("subagents"),
        "Subagents section missing"
    );
}

#[test]
fn json_serialization_stays_v1_compatible_with_plan04_schema() {
    // Plan 04's status_cmd_schema test locks the v1 shape via insta. Here
    // we just round-trip to make sure serde_json::to_string_pretty still
    // produces a well-formed document with all 7 top-level keys.
    let snap = StatusReport::fixture();
    let json = serde_json::to_string_pretty(&snap).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let obj = v.as_object().unwrap();
    for key in &[
        "provider",
        "memory",
        "gateway",
        "subagents",
        "processes",
        "mcp",
        "yolo",
    ] {
        assert!(obj.contains_key(*key), "JSON missing top-level key: {key}");
    }
}

#[test]
fn yolo_enabled_banner_surfaces_in_provider_block() {
    // D-18 / D-12: when yolo is on, the provider block must carry a visible
    // banner so operators see it at a glance without scrolling.
    colored::control::set_override(false);
    let mut snap = StatusReport::fixture();
    snap.yolo.enabled = true;
    snap.yolo.source = "config".into();
    let text = format_styled(&snap);
    assert!(
        text.contains("--yolo"),
        "yolo banner must appear in default output when enabled, got:\n{text}"
    );
    assert!(
        text.contains("approvals bypassed")
            || text.contains("approvals are bypassed")
            || text.contains("bypassed"),
        "yolo banner must mention approval bypass, got:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// Phase 24 — Profile section integration test (24-05-01)
// ---------------------------------------------------------------------------

fn make_profile_dir(root: &std::path::Path, name: &str, learning_loop_on: bool) {
    let dir = root.join("profiles").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = format!(
        "memory:\n  enabled: true\nskills:\n  generation_enabled: {}\n",
        learning_loop_on
    );
    std::fs::write(dir.join("config.yaml"), cfg).unwrap();
}

/// 24-05-01: `hermes status` Profile section enumerates `profiles/*/` subdirs
/// that contain config.yaml. Both profile slugs must appear in the output.
#[test]
fn profile_section() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Skipping profile_section: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let tmp = tempfile::TempDir::new().unwrap();
    make_profile_dir(tmp.path(), "work", true);
    make_profile_dir(tmp.path(), "personal", false);
    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["status"])
        .output()
        .expect("ironhermes status");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("profiles"),
        "expected 'Profiles' header in status stdout, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("work"),
        "expected profile name 'work' in status stdout, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("personal"),
        "expected profile name 'personal' in status stdout, got:\n{}",
        stdout
    );
}
