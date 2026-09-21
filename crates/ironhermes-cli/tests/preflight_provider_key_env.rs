//! Quick task 260820-5fu — binary-level regression coverage for the reported
//! groq preflight bug: a config with `model.provider: groq` and
//! `providers.groq.api_key_env: GROQ_API_KEY`, with `GROQ_API_KEY` actually
//! exported, is a valid AND runnable configuration. Before this fix,
//! `has_runnable_llm` only recognised `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`,
//! and `OPENAI_API_KEY`, so this config relaunched the FirstRun setup wizard on
//! every interactive start even though the wizard could not fix anything.
//!
//! A test harness can never present a TTY, so `interactive` is always `false`
//! in-process and neither test here can observe `LaunchWizard` directly. Both
//! observe the same `runnable` boolean through its other consumer: the
//! non-interactive stderr notice ("No runnable LLM provider detected") fires
//! if and only if `runnable == false` on the present+valid path.

use std::process::{Command, Stdio};
use tempfile::TempDir;

const NOTICE: &str = "No runnable LLM provider detected";

/// groq config.yaml: `model.provider: groq` + `providers.groq.api_key_env`,
/// no `base_url` and no `model.api_key` so checks 3 and 4 cannot fire — only
/// the new check 0 (or its absence) can explain the outcome.
const GROQ_CONFIG_YAML: &str = "model:\n  provider: groq\n  default: groq/llama-3.3-70b\nproviders:\n  groq:\n    api_key_env: GROQ_API_KEY\n";

fn write_groq_config(home: &std::path::Path) {
    std::fs::write(home.join("config.yaml"), GROQ_CONFIG_YAML).unwrap();
}

/// Task 1 positive case: `GROQ_API_KEY` set in the child's environment ⇒ the
/// non-runnable notice must be absent.
#[test]
fn groq_provider_with_api_key_env_set_does_not_emit_non_runnable_notice() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "Skipping groq_provider_with_api_key_env_set_does_not_emit_non_runnable_notice: CARGO_BIN_EXE_ironhermes not set"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    write_groq_config(tmp.path());

    let out = Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .env("GROQ_API_KEY", "gsk-dummy-test-value")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .args(["chat"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("ironhermes chat");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");

    // Exit status is intentionally not asserted: with null stdin the chat
    // REPL fails on EOF after preflight passes, which is expected and
    // irrelevant to this test's claim.
    assert!(
        !combined.contains(NOTICE),
        "expected NO '{NOTICE}' notice when GROQ_API_KEY is exported for a \
         groq main provider configured via providers.groq.api_key_env, got \
         stdout={stdout:?} stderr={stderr:?}"
    );
}

/// Task 2 negative control: same groq config.yaml, but `GROQ_API_KEY` is
/// removed from the child's environment (and no `.env` file exists) ⇒ the
/// notice must be present. This is what proves the positive assertion above
/// is not vacuous — without it, a test that always passed regardless of the
/// fix would be worthless.
#[test]
fn groq_provider_with_api_key_env_unset_emits_non_runnable_notice() {
    let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "Skipping groq_provider_with_api_key_env_unset_emits_non_runnable_notice: CARGO_BIN_EXE_ironhermes not set"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    write_groq_config(tmp.path());

    let out = Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .env_remove("GROQ_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .args(["chat"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("ironhermes chat");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");

    assert!(
        combined.contains(NOTICE),
        "expected the '{NOTICE}' notice when GROQ_API_KEY is absent everywhere \
         for a groq main provider configured via providers.groq.api_key_env, \
         got stdout={stdout:?} stderr={stderr:?}"
    );
}

// ---------------------------------------------------------------------------
// CR-02 (Phase 51 Plan 12): artifact-level proof that the SHA-256 env-var
// observation oracle (main.rs's IRONHERMES_TEST_PRINT_ENV_VAR_SHA256 hook,
// gated behind the `test-oracles` Cargo feature) is absent from a
// DEFAULT-feature build. A source-text grep alone would pass even if the
// block still shipped unconditionally in the binary — the compiled binary's
// own bytes are the only thing that proves it is really gone. This module
// only runs in the configuration it is meaningful for: a build WITHOUT
// `test-oracles`, where the hook must not exist at all.
// ---------------------------------------------------------------------------
#[cfg(not(feature = "test-oracles"))]
mod cr02_oracle_absent_from_default_build {
    /// The variable name a caller sets to trigger the hook. If this string
    /// occurs anywhere in the default-feature `ironhermes` binary's bytes,
    /// the oracle (and its silent `exit(0)` after hashing an arbitrary named
    /// env var) shipped in a build that did not opt into `test-oracles`.
    const TRIGGER_VAR_NAME: &str = "IRONHERMES_TEST_PRINT_ENV_VAR_SHA256";

    #[test]
    fn default_build_does_not_contain_the_oracle_trigger_var_name() {
        let bin_path = match std::env::var("CARGO_BIN_EXE_ironhermes") {
            Ok(p) => p,
            Err(_) => {
                eprintln!(
                    "Skipping default_build_does_not_contain_the_oracle_trigger_var_name: \
                     CARGO_BIN_EXE_ironhermes not set"
                );
                return;
            }
        };
        let bytes = std::fs::read(&bin_path)
            .unwrap_or_else(|e| panic!("read compiled binary at {bin_path}: {e}"));
        let found = bytes
            .windows(TRIGGER_VAR_NAME.len())
            .any(|w| w == TRIGGER_VAR_NAME.as_bytes());
        assert!(
            !found,
            "the default-feature ironhermes binary at {bin_path} contains the oracle \
             trigger variable name {TRIGGER_VAR_NAME:?} — the SHA-256 env-var observation \
             hook (and its exit(0)) shipped in a build that did not enable the \
             `test-oracles` feature"
        );
    }
}
