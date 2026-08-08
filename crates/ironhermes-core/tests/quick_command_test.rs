/// Phase 42 EXEC-01 / D-02 / D-11: QuickCommand dispatch coverage.
///
/// Tests are LLM-free — no agent loop is invoked; prepare_quick_command() must
/// return a QuickCommandPlan using only config data (EXEC-01).
use ironhermes_core::{ApprovalNeed, QuickCommandDef, prepare_quick_command};

fn make_def(name: &str, cmd: &str, dangerous: Option<bool>) -> QuickCommandDef {
    QuickCommandDef {
        name: name.to_string(),
        command: cmd.to_string(),
        description: None,
        dangerous,
        pass_env: None,
    }
}

fn make_def_with_pass_env(
    name: &str,
    cmd: &str,
    dangerous: Option<bool>,
    pass_env: Vec<String>,
) -> QuickCommandDef {
    QuickCommandDef {
        name: name.to_string(),
        command: cmd.to_string(),
        description: None,
        dangerous,
        pass_env: Some(pass_env),
    }
}

// ---------------------------------------------------------------------------
// EXEC-01: LLM-free dispatch + D-02 cache_key == name
// ---------------------------------------------------------------------------

#[test]
fn quick_command_exec_dispatch() {
    // EXEC-01: prepare_quick_command() returns a fully-formed QuickCommandPlan with
    // no LLM call. The cache_key must equal the command name (D-02).
    let def = make_def("wipe-cache", "redis-cli flushall", None);
    let plan = prepare_quick_command(&def, &[]);
    assert_eq!(
        plan.command, "redis-cli flushall",
        "command must match def.command"
    );
    assert_eq!(
        plan.cache_key, "wipe-cache",
        "cache_key must be the command NAME, not the command string (D-02)"
    );
    // env is present — at minimum PATH or HOME will be set in CI
    // We assert the field exists and the dangerous-secrets gate holds
    assert!(!plan.env.is_empty(), "env must be populated (D-11)");
    // dangerous:None → Auto
    assert!(
        matches!(plan.approval_need, ApprovalNeed::Auto),
        "dangerous:None should yield Auto approval need"
    );
}

// ---------------------------------------------------------------------------
// D-11: env scrub is unconditional regardless of dangerous flag
// ---------------------------------------------------------------------------

#[test]
fn quick_command_dangerous_false_env_scrubbed() {
    // D-11: dangerous:Some(false) skips approval prompt but env is still sanitized.
    unsafe { std::env::set_var("CLOUDFLARE_API_TOKEN_QC42", "must-not-leak") };
    let def = make_def("safe-cmd", "echo hello", Some(false));
    let plan = prepare_quick_command(&def, &[]);
    // env is non-empty (sanitized base vars present)
    assert!(
        !plan.env.is_empty(),
        "env must be populated even when dangerous:false (D-11)"
    );
    // The secret must not be in the env
    assert!(
        !plan.env.contains_key("CLOUDFLARE_API_TOKEN_QC42"),
        "CLOUDFLARE_API_TOKEN must be absent even when dangerous:false (D-11)"
    );
    // approval_need is Skip because dangerous:false
    assert!(
        matches!(plan.approval_need, ApprovalNeed::Skip),
        "dangerous:false should yield Skip approval need"
    );
    unsafe { std::env::remove_var("CLOUDFLARE_API_TOKEN_QC42") };
}

#[test]
fn quick_command_dangerous_true_forces_approval() {
    // D-11: dangerous:Some(true) forces approval prompt (Force); env still sanitized.
    let def = make_def("network-cmd", "curl https://example.com", Some(true));
    let plan = prepare_quick_command(&def, &[]);
    assert!(
        matches!(plan.approval_need, ApprovalNeed::Force),
        "dangerous:true should yield Force approval need"
    );
    // env is still sanitized regardless of dangerous:true
    assert!(
        !plan.env.is_empty(),
        "env must be populated even when dangerous:true (D-11)"
    );
}

#[test]
fn quick_command_dangerous_none_is_auto() {
    // D-11: dangerous:None → guard checks at dispatch time (represented as Auto here).
    let def = make_def("maybe-cmd", "grep -r pattern /tmp", None);
    let plan = prepare_quick_command(&def, &[]);
    assert!(
        matches!(plan.approval_need, ApprovalNeed::Auto),
        "dangerous:None should yield Auto (deferred to guard at dispatch)"
    );
}

// ---------------------------------------------------------------------------
// D-05 + D-11: pass_env is applied even when dangerous:false
// ---------------------------------------------------------------------------

#[test]
fn quick_command_pass_env_applied_with_dangerous_false() {
    // Even when dangerous:false, pass_env vars are included in the sanitized env.
    unsafe { std::env::set_var("REDIS_URL_QC42", "redis://localhost:6379") };
    let def = make_def_with_pass_env(
        "flush-redis",
        "redis-cli flushall",
        Some(false),
        vec!["REDIS_URL_QC42".to_string()],
    );
    let plan = prepare_quick_command(&def, &[]);
    assert_eq!(
        plan.env.get("REDIS_URL_QC42").map(|s| s.as_str()),
        Some("redis://localhost:6379"),
        "pass_env var must be in env even when dangerous:false"
    );
    unsafe { std::env::remove_var("REDIS_URL_QC42") };
}
