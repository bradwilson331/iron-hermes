//! Phase 22.4.2.2 Plan 02 — behavioral tests for `cronjob` tool TG default-routing.
//!
//! Mirrors Plan 01's CLI tests (`crates/ironhermes-cli/tests/cron_default_deliver.rs`)
//! but exercises the LLM-tool entry point: `CronjobTool::execute(json!({"action":"create",...}))`.
//! Verifies the JSON response's `job.deliver` and `job.origin` reflect the helper's decision.
//! `IRONHERMES_HOME` is redirected to a TempDir per test so `Config::load()` reads the fixture.
//!
//! Run: `cargo test -p ironhermes-tools --test cronjob_tool_default_deliver`

use std::sync::{Arc, Mutex};

use ironhermes_cron::JobStore;
use ironhermes_tools::cronjob_tool::CronjobTool;
use ironhermes_tools::registry::Tool;
use serde_json::{json, Value};
use tempfile::TempDir;

fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

fn make_test_config(tmp: &TempDir, tg_enabled: bool, whitelist: &[i64]) {
    let whitelist_yaml = if whitelist.is_empty() {
        "      whitelist: []".to_string()
    } else {
        let entries = whitelist
            .iter()
            .map(|id| format!("        - {}", id))
            .collect::<Vec<_>>()
            .join("\n");
        format!("      whitelist:\n{}", entries)
    };
    let yaml = format!(
        "gateway:\n  platforms:\n    telegram:\n      enabled: {}\n{}\n",
        tg_enabled, whitelist_yaml
    );
    std::fs::write(tmp.path().join("config.yaml"), yaml).unwrap();
}

fn make_tool(tmp: &TempDir) -> CronjobTool {
    let cron_dir = tmp.path().join("cron");
    let store = JobStore::open(cron_dir).expect("JobStore opens");
    CronjobTool::new(Arc::new(Mutex::new(store)))
}

fn parse_response(s: &str) -> Value {
    serde_json::from_str(s).expect("valid JSON response")
}

#[tokio::test]
async fn tg_enabled_single_chat_routes_to_origin_via_tool() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_test_config(&tmp, true, &[12345]);
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    let tool = make_tool(&tmp);
    let response = tool
        .execute(json!({
            "action": "create",
            "name": "tg-routed-job",
            "schedule": "every 1h",
            "prompt": "say hi"
        }))
        .await
        .expect("execute returns Ok");
    let v = parse_response(&response);

    assert_eq!(v["status"], "created", "expected status=created, got: {}", v);
    assert_eq!(v["job"]["deliver"], "origin", "expected deliver=origin");
    assert_eq!(v["job"]["origin"]["platform"], "telegram");
    assert_eq!(v["job"]["origin"]["chat_id"], "12345");
}

#[tokio::test]
async fn tg_enabled_multi_chat_falls_back_to_local_via_tool() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_test_config(&tmp, true, &[12345, 67890]);
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    let tool = make_tool(&tmp);
    let response = tool
        .execute(json!({
            "action": "create",
            "name": "multi-chat-job",
            "schedule": "every 1h",
            "prompt": "p"
        }))
        .await
        .expect("execute returns Ok");
    let v = parse_response(&response);

    assert_eq!(v["status"], "created");
    assert_eq!(v["job"]["deliver"], "local", "multi-chat must fall back to local");
    assert!(v["job"]["origin"].is_null(), "multi-chat must not auto-set origin");
}

#[tokio::test]
async fn tg_disabled_falls_back_to_local_via_tool() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_test_config(&tmp, false, &[12345]);
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    let tool = make_tool(&tmp);
    let response = tool
        .execute(json!({
            "action": "create",
            "name": "tg-disabled-job",
            "schedule": "every 1h",
            "prompt": "p"
        }))
        .await
        .expect("execute returns Ok");
    let v = parse_response(&response);

    assert_eq!(v["status"], "created");
    assert_eq!(v["job"]["deliver"], "local");
    assert!(v["job"]["origin"].is_null());
}

#[tokio::test]
async fn tg_section_missing_falls_back_to_local_via_tool() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("config.yaml"), "model:\n  default: test\n").unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    let tool = make_tool(&tmp);
    let response = tool
        .execute(json!({
            "action": "create",
            "name": "no-tg-job",
            "schedule": "every 1h",
            "prompt": "p"
        }))
        .await
        .expect("execute returns Ok");
    let v = parse_response(&response);

    assert_eq!(v["status"], "created");
    assert_eq!(v["job"]["deliver"], "local");
    assert!(v["job"]["origin"].is_null());
}

#[tokio::test]
async fn explicit_deliver_arg_skips_helper_via_tool() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_test_config(&tmp, true, &[12345]);
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    let tool = make_tool(&tmp);
    let response = tool
        .execute(json!({
            "action": "create",
            "name": "explicit-local-job",
            "schedule": "every 1h",
            "prompt": "p",
            "deliver": "local"
        }))
        .await
        .expect("execute returns Ok");
    let v = parse_response(&response);

    assert_eq!(v["status"], "created");
    assert_eq!(v["job"]["deliver"], "local", "explicit arg must win");
    assert!(
        v["job"]["origin"].is_null(),
        "explicit arg must not trigger origin auto-population"
    );
}
