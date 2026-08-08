/// Phase 42 D-04 / D-05: build_terminal_safe_env() coverage.
///
/// Tests are named with unique env-var suffixes (_bte42_*) to avoid cross-test
/// races when tests run in parallel. Unsafe env mutations are wrapped in `unsafe`
/// blocks per Rust 2024 edition requirements.
use ironhermes_core::build_terminal_safe_env;

// ---------------------------------------------------------------------------
// D-04: Base allowlist (SAFE_ENV_KEYS + XDG_* + IRONHERMES_HOME)
// ---------------------------------------------------------------------------

#[test]
fn build_terminal_safe_env_base_excludes_planted_secret() {
    // T-42-01: a planted credential must never appear in the output.
    unsafe { std::env::set_var("CLOUDFLARE_API_TOKEN_BTE42", "secret-cf-token") };
    let env = build_terminal_safe_env(&[], &[]);
    assert!(
        !env.contains_key("CLOUDFLARE_API_TOKEN_BTE42"),
        "CLOUDFLARE_API_TOKEN must be excluded from safe env (T-42-01)"
    );
    unsafe { std::env::remove_var("CLOUDFLARE_API_TOKEN_BTE42") };
}

#[test]
fn build_terminal_safe_env_base_includes_path_and_home() {
    // D-04: standard SAFE_ENV_KEYS are present when they exist in the process env.
    let env = build_terminal_safe_env(&[], &[]);
    if std::env::var("PATH").is_ok() {
        assert!(env.contains_key("PATH"), "PATH must be in safe env");
    }
    if std::env::var("HOME").is_ok() {
        assert!(env.contains_key("HOME"), "HOME must be in safe env");
    }
}

#[test]
fn build_terminal_safe_env_xdg_vars_pass_through() {
    // D-04: every present XDG_* var passes through.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR_BTE42", "xdg_terminal_val") };
    let env = build_terminal_safe_env(&[], &[]);
    assert_eq!(
        env.get("XDG_RUNTIME_DIR_BTE42").map(|s| s.as_str()),
        Some("xdg_terminal_val"),
        "XDG_* vars must pass through"
    );
    unsafe { std::env::remove_var("XDG_RUNTIME_DIR_BTE42") };
}

#[test]
fn build_terminal_safe_env_ironhermes_home_always_passes_through() {
    // D-04 + worker_spawn.rs SAFE_SYSTEM_VARS: IRONHERMES_HOME always present when set.
    unsafe { std::env::set_var("IRONHERMES_HOME", "/tmp/test-ironhermes-home-bte42") };
    let env = build_terminal_safe_env(&[], &[]);
    assert_eq!(
        env.get("IRONHERMES_HOME").map(|s| s.as_str()),
        Some("/tmp/test-ironhermes-home-bte42"),
        "IRONHERMES_HOME must always pass through (worker_spawn.rs SAFE_SYSTEM_VARS)"
    );
    unsafe { std::env::remove_var("IRONHERMES_HOME") };
}

// ---------------------------------------------------------------------------
// D-05: global_allowlist layer
// ---------------------------------------------------------------------------

#[test]
fn build_terminal_safe_env_global_allowlist_present_var() {
    // A var named in the global allowlist is included when it exists.
    unsafe { std::env::set_var("KUBECONFIG_BTE42", "~/.kube/config") };
    let allowlist = vec!["KUBECONFIG_BTE42".to_string()];
    let env = build_terminal_safe_env(&allowlist, &[]);
    assert_eq!(
        env.get("KUBECONFIG_BTE42").map(|s| s.as_str()),
        Some("~/.kube/config"),
        "Global allowlist var must be present when it exists in process env"
    );
    unsafe { std::env::remove_var("KUBECONFIG_BTE42") };
}

#[test]
fn build_terminal_safe_env_global_allowlist_absent_name_silently_skipped() {
    // An allowlist name that is not set in the process env must be silently skipped.
    let allowlist = vec!["TOTALLY_NONEXISTENT_BTE42".to_string()];
    let env = build_terminal_safe_env(&allowlist, &[]);
    assert!(
        !env.contains_key("TOTALLY_NONEXISTENT_BTE42"),
        "Absent allowlist names must be silently skipped"
    );
}

// ---------------------------------------------------------------------------
// D-05: per_command_pass_env layer
// ---------------------------------------------------------------------------

#[test]
fn build_terminal_safe_env_pass_env() {
    // D-05: per_command_pass_env adds only the named var; a sibling secret stays absent.
    unsafe {
        std::env::set_var("REDIS_URL_BTE42", "redis://localhost:6379");
        std::env::set_var("AWS_SECRET_BTE42", "aws-secret-must-not-leak");
    }
    let env = build_terminal_safe_env(&[], &["REDIS_URL_BTE42".to_string()]);
    assert_eq!(
        env.get("REDIS_URL_BTE42").map(|s| s.as_str()),
        Some("redis://localhost:6379"),
        "pass_env var should be present"
    );
    assert!(
        !env.contains_key("AWS_SECRET_BTE42"),
        "Sibling secret must be absent — only REDIS_URL_BTE42 was in pass_env"
    );
    unsafe {
        std::env::remove_var("REDIS_URL_BTE42");
        std::env::remove_var("AWS_SECRET_BTE42");
    }
}
