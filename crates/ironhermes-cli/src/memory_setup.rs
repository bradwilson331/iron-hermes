//! `hermes memory setup` — minimal interactive setup for the selected
//! memory provider (Plan 20-03, D-08).
//!
//! Workflow:
//! 1. Enumerate compiled-in providers via `available_providers()`.
//! 2. Ask user which to activate.
//! 3. Construct that provider via the factory (read-only — no writes yet).
//! 4. Call `provider.get_config_schema()`.
//! 5. Prompt only for fields where `required == true && default.is_none()`.
//!    (Optional-or-defaulted fields go to the JSON with their default value.)
//! 6. For secret fields, append `KEY='VALUE'` to `$HERMES_HOME/.env`
//!    with POSIX single-quote escaping and newline refusal (T-20-03).
//! 7. For non-secret fields, pass the collected values to
//!    `provider.save_config(&values, &hermes_home)`.
//! 8. Update `$HERMES_HOME/config.yaml` to set `memory.provider` to the
//!    selected name so the next launch picks it up
//!    (resolves research Open Question #1).

// ================================================================
// RED-phase stubs — replaced by the GREEN commit.
// ================================================================

/// Compiled-in providers. Feature-gated; kept in lockstep with factory.rs.
pub fn available_providers() -> Vec<&'static str> {
    unimplemented!("Task 20-03-01 GREEN — not yet implemented")
}

pub(crate) fn is_valid_env_var_name(_s: &str) -> bool {
    unimplemented!("Task 20-03-01 GREEN — not yet implemented")
}

pub(crate) fn posix_single_quote(_value: &str) -> anyhow::Result<String> {
    unimplemented!("Task 20-03-01 GREEN — not yet implemented")
}

/// Redacted wrapper so Debug-formatting never leaks secret content (T-20-03b).
pub struct RedactedValue(#[allow(dead_code)] String);

impl RedactedValue {
    pub fn new<S: Into<String>>(_s: S) -> Self {
        unimplemented!("Task 20-03-01 GREEN — not yet implemented")
    }

    pub fn reveal(&self) -> &str {
        unimplemented!("Task 20-03-01 GREEN — not yet implemented")
    }
}

impl std::fmt::Debug for RedactedValue {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unimplemented!("Task 20-03-01 GREEN — not yet implemented")
    }
}

pub async fn run_memory_setup(_cli: &crate::Cli) -> anyhow::Result<()> {
    unimplemented!("Task 20-03-01 GREEN — not yet implemented")
}

// =============================================================================
// Tests (RED phase — these intentionally fail against the stubs above)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_name_validation() {
        assert!(is_valid_env_var_name("API_KEY"));
        assert!(is_valid_env_var_name("_LEADING_UNDERSCORE"));
        assert!(is_valid_env_var_name("K1_2"));
        assert!(!is_valid_env_var_name("api_key")); // lowercase
        assert!(!is_valid_env_var_name("1KEY")); // digit start
        assert!(!is_valid_env_var_name("KEY-DASH")); // dash
        assert!(!is_valid_env_var_name("")); // empty
        assert!(!is_valid_env_var_name("PATH")); // deny-list
        assert!(!is_valid_env_var_name("HOME")); // deny-list
        assert!(!is_valid_env_var_name("HERMES_HOME")); // deny-list
        assert!(!is_valid_env_var_name("IRONHERMES_HOME")); // deny-list
    }

    #[test]
    fn posix_quote_escaping_ok() {
        assert_eq!(posix_single_quote("sk-abc").unwrap(), "'sk-abc'");
        assert_eq!(posix_single_quote("it's").unwrap(), "'it'\\''s'");
        assert_eq!(posix_single_quote("").unwrap(), "''");
    }

    #[test]
    fn posix_quote_rejects_newlines() {
        assert!(posix_single_quote("has\nnewline").is_err());
        assert!(posix_single_quote("has\rcarriage").is_err());
    }

    #[test]
    fn redacted_value_debug_is_masked() {
        let r = RedactedValue::new("secret-xyz");
        let s = format!("{:?}", r);
        assert!(!s.contains("secret-xyz"), "Debug impl leaked secret: {s}");
        assert!(s.contains("***"), "expected masking marker, got: {s}");
        // Round-trip: reveal() still returns the original.
        assert_eq!(r.reveal(), "secret-xyz");
    }

    #[test]
    fn appending_env_preserves_existing_keys() {
        use std::fs::OpenOptions;
        use std::io::Write;

        let tmp = tempfile::TempDir::new().unwrap();
        let env = tmp.path().join(".env");
        std::fs::write(&env, "EXISTING_KEY='value'\n").unwrap();

        let quoted = posix_single_quote("new-secret").unwrap();
        let mut f = OpenOptions::new().append(true).open(&env).unwrap();
        writeln!(f, "{}={}", "NEW_KEY", quoted).unwrap();

        let text = std::fs::read_to_string(&env).unwrap();
        assert!(text.contains("EXISTING_KEY='value'"));
        assert!(text.contains("NEW_KEY='new-secret'"));
    }

    #[test]
    fn available_providers_always_contains_file() {
        let v = available_providers();
        assert!(v.contains(&"file"), "file provider must always be present");
    }
}
