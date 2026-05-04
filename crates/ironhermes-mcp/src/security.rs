use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// D-19: Safe environment key allowlist for stdio subprocess.
/// Only these keys (plus XDG_* prefix) are passed to MCP server child processes.
/// User-specified `env` values from config are added on top.
const SAFE_ENV_KEYS: &[&str] = &[
    "PATH", "HOME", "USER", "LANG", "LC_ALL", "TERM", "SHELL", "TMPDIR",
];

/// Build a safe environment for a stdio MCP server subprocess.
///
/// Returns a HashMap containing:
/// 1. All allowlisted keys from the host environment (`SAFE_ENV_KEYS` + `XDG_*`).
/// 2. All user-specified `env` vars from the server config (may override allowlist).
///
/// This matches hermes-agent's `_build_safe_env()` (D-19).
pub fn build_safe_env(user_env: &HashMap<String, String>) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars()
        .filter(|(k, _)| SAFE_ENV_KEYS.contains(&k.as_str()) || k.starts_with("XDG_"))
        .collect();
    env.extend(user_env.iter().map(|(k, v)| (k.clone(), v.clone())));
    env
}

/// D-20: Credential pattern — matches sensitive tokens/keys that must be redacted.
///
/// Patterns ported from hermes-agent's `_CREDENTIAL_PATTERN`:
/// - GitHub tokens: `ghp_*`
/// - OpenAI/Anthropic API keys: `sk-*`
/// - Bearer tokens: `Bearer <value>`
/// - Generic credential assignments: `token=`, `key=`, `API_KEY=`, `password=`, `secret=`
pub static CREDENTIAL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Note: use regular string literals (not raw) for the character classes that
    // contain backslash escapes (\s, \S) and quote characters.
    Regex::new(concat!(
        r"(?i)(?:",
        r"ghp_[A-Za-z0-9_]{1,255}",
        r"|sk-[A-Za-z0-9_]{1,255}",
        "|Bearer\\s+\\S+",
        "|token=[^\\s&,;\"']{1,255}",
        "|key=[^\\s&,;\"']{1,255}",
        "|API_KEY=[^\\s&,;\"']{1,255}",
        "|password=[^\\s&,;\"']{1,255}",
        "|secret=[^\\s&,;\"']{1,255}",
        r")",
    ))
    .unwrap()
});

/// Strip credential patterns from error text before returning to the LLM (D-20).
///
/// Replaces all credential matches with `[REDACTED]`.
/// Matches hermes-agent's `_sanitize_error()`.
pub fn sanitize_error(text: &str) -> String {
    CREDENTIAL_PATTERN
        .replace_all(text, "[REDACTED]")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // build_safe_env tests (D-19)
    // -------------------------------------------------------------------------

    #[test]
    fn test_build_safe_env_includes_allowlisted_keys() {
        // These vars should be present on macOS/Linux test environments
        let user_env = HashMap::new();
        let env = build_safe_env(&user_env);

        // At minimum PATH and HOME should be present in most environments
        // We test the keys that are present rather than assuming all are set
        for key in &["PATH", "HOME"] {
            if std::env::var(key).is_ok() {
                assert!(
                    env.contains_key(*key),
                    "Expected safe env to include {key} since it exists in host env"
                );
            }
        }
    }

    #[test]
    fn test_build_safe_env_includes_xdg_vars() {
        // Inject a test XDG var to verify XDG_* prefix filtering
        // SAFETY: test-only env mutation; single-threaded via --test-threads=1 convention
        unsafe { std::env::set_var("XDG_TEST_MCP_VAR", "xdg_test_value") };
        let user_env = HashMap::new();
        let env = build_safe_env(&user_env);
        assert_eq!(
            env.get("XDG_TEST_MCP_VAR").map(|s| s.as_str()),
            Some("xdg_test_value"),
            "XDG_* vars from host env should be included"
        );
        unsafe { std::env::remove_var("XDG_TEST_MCP_VAR") };
    }

    #[test]
    fn test_build_safe_env_excludes_credential_vars() {
        // Inject credential vars and verify they are excluded.
        // SAFETY: test-only env mutation; these keys are not used by other tests.
        unsafe {
            std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test");
            std::env::set_var("AWS_SECRET_ACCESS_KEY", "aws-secret-test");
            std::env::set_var("OPENAI_API_KEY", "sk-openai-test");
        }

        let user_env = HashMap::new();
        let env = build_safe_env(&user_env);

        assert!(
            !env.contains_key("ANTHROPIC_API_KEY"),
            "ANTHROPIC_API_KEY must be excluded from safe env"
        );
        assert!(
            !env.contains_key("AWS_SECRET_ACCESS_KEY"),
            "AWS_SECRET_ACCESS_KEY must be excluded from safe env"
        );
        assert!(
            !env.contains_key("OPENAI_API_KEY"),
            "OPENAI_API_KEY must be excluded from safe env"
        );

        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("AWS_SECRET_ACCESS_KEY");
            std::env::remove_var("OPENAI_API_KEY");
        }
    }

    #[test]
    fn test_build_safe_env_includes_user_overrides() {
        let mut user_env = HashMap::new();
        user_env.insert("GITHUB_TOKEN".to_string(), "ghp_user_token".to_string());
        user_env.insert("MY_CUSTOM_VAR".to_string(), "custom_value".to_string());

        let env = build_safe_env(&user_env);

        assert_eq!(
            env.get("GITHUB_TOKEN").map(|s| s.as_str()),
            Some("ghp_user_token"),
            "User-specified env vars should be included"
        );
        assert_eq!(
            env.get("MY_CUSTOM_VAR").map(|s| s.as_str()),
            Some("custom_value"),
            "User-specified env vars should be included"
        );
    }

    #[test]
    fn test_build_safe_env_user_overrides_take_precedence() {
        // User env should override the host allowlisted value
        let mut user_env = HashMap::new();
        user_env.insert("PATH".to_string(), "/custom/path".to_string());

        let env = build_safe_env(&user_env);
        assert_eq!(
            env.get("PATH").map(|s| s.as_str()),
            Some("/custom/path"),
            "User-specified env should override host env for same key"
        );
    }

    // -------------------------------------------------------------------------
    // sanitize_error tests (D-20)
    // -------------------------------------------------------------------------

    #[test]
    fn test_sanitize_error_redacts_github_token() {
        let text = "Error connecting: ghp_abc123XYZ token rejected";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "ghp_ tokens should be redacted: {result}"
        );
        assert!(
            !result.contains("ghp_abc123XYZ"),
            "Original token must not appear in output"
        );
    }

    #[test]
    fn test_sanitize_error_redacts_sk_token() {
        let text = "API call failed with key sk-abc123DEF456 invalid";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "sk- tokens should be redacted: {result}"
        );
        assert!(!result.contains("sk-abc123DEF456"));
    }

    #[test]
    fn test_sanitize_error_redacts_bearer_token() {
        let text = "Unauthorized: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "Bearer tokens should be redacted: {result}"
        );
        assert!(!result.contains("eyJhbGciOiJIUzI1NiJ9"));
    }

    #[test]
    fn test_sanitize_error_redacts_token_assignment() {
        let text = "Request with token=supersecretvalue123 failed";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "token= assignments should be redacted: {result}"
        );
        assert!(!result.contains("supersecretvalue123"));
    }

    #[test]
    fn test_sanitize_error_redacts_key_assignment() {
        let text = "Request with key=mysecretkey456 failed";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "key= assignments should be redacted: {result}"
        );
        assert!(!result.contains("mysecretkey456"));
    }

    #[test]
    fn test_sanitize_error_redacts_api_key_assignment() {
        let text = "Auth header API_KEY=abcdefg12345 not accepted";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "API_KEY= should be redacted: {result}"
        );
        assert!(!result.contains("abcdefg12345"));
    }

    #[test]
    fn test_sanitize_error_redacts_password_assignment() {
        let text = "Connection refused: password=hunter2 wrong";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "password= should be redacted: {result}"
        );
        assert!(!result.contains("hunter2"));
    }

    #[test]
    fn test_sanitize_error_redacts_secret_assignment() {
        let text = "Validation failed: secret=topsecret123 mismatch";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "secret= should be redacted: {result}"
        );
        assert!(!result.contains("topsecret123"));
    }

    #[test]
    fn test_sanitize_error_preserves_non_credential_text() {
        let text = "Connection refused: server at localhost:8080 is not running";
        let result = sanitize_error(text);
        assert_eq!(
            result, text,
            "Non-credential error text should pass through unchanged"
        );
    }

    #[test]
    fn test_sanitize_error_empty_string() {
        assert_eq!(sanitize_error(""), "");
    }

    #[test]
    fn test_sanitize_error_multiple_credentials() {
        let text = "Token ghp_abc123 and key=mysecret both invalid";
        let result = sanitize_error(text);
        assert!(!result.contains("ghp_abc123"));
        assert!(!result.contains("mysecret"));
        // Should have two [REDACTED] replacements
        assert_eq!(result.matches("[REDACTED]").count(), 2);
    }
}
