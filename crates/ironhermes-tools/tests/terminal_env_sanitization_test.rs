//! EXEC-03 / D-06 Wave-0 security tests: TerminalTool subprocess env sanitization.
//!
//! Covers both spawn paths required by D-06:
//!   - Foreground: `terminal.rs::execute()` (sh -c <cmd>)
//!   - Background: `SpawnSpec → ProcessRegistry::spawn()` (background=true)
//!
//! A CLOUDFLARE_API_TOKEN-like secret planted in the parent process env must be
//! absent from the child env on BOTH paths (T-42-03, T-42-04).
//! PATH must be present (proves env_clear() ran BEFORE envs(), not after — T-42-06).
//!
//! Tests use unique per-test env var names to avoid parallel-test races.

#[cfg(unix)]
mod exec03_tests {
    use std::sync::Arc;

    use ironhermes_exec::process_registry::ProcessRegistry;
    use ironhermes_tools::Tool;
    use ironhermes_tools::terminal::TerminalTool;
    use tokio::sync::RwLock;

    // -------------------------------------------------------------------------
    // Foreground path (T-42-03)
    // -------------------------------------------------------------------------

    /// EXEC-03 foreground gate: CLOUDFLARE_API_TOKEN-like secret is absent from
    /// the child env produced by the foreground `sh -c <cmd>` path.
    /// PATH must also be present (T-42-06 — proves env_clear ran BEFORE envs).
    #[tokio::test]
    async fn terminal_foreground_env_sanitized() {
        let secret_name = "EXEC03_FG_SECRET_TOKEN";
        let secret_val = "exec03-fg-secret-placeholder-abc123";
        // SAFETY: test-only; unique var name avoids cross-test races.
        unsafe { std::env::set_var(secret_name, secret_val) };

        let tool = TerminalTool::new().with_env_allowlist(vec![]);

        // Run a command that would print the secret if it were in the child env.
        let output = tool
            .execute(serde_json::json!({
                "command": format!("printenv {secret_name} 2>/dev/null; true")
            }))
            .await
            .unwrap();

        // T-42-03: secret must be absent from foreground child env.
        assert!(
            !output.contains(secret_val),
            "foreground path leaked {secret_name}: output={output:?}"
        );

        // T-42-06: PATH must be present (env_clear must precede envs).
        let path_out = tool
            .execute(serde_json::json!({"command": "printenv PATH"}))
            .await
            .unwrap();
        assert!(
            !path_out.trim().is_empty() && path_out.trim() != "(exit code: 1)",
            "PATH absent from foreground child env — env_clear ordering bug: {path_out:?}"
        );

        // SAFETY: cleanup.
        unsafe { std::env::remove_var(secret_name) };
    }

    /// EXEC-03 allowlist opt-in: with_env_allowlist(["MY_ALLOWED_VAR"]) passes
    /// that var through to the foreground child (D-05 global allowlist).
    #[tokio::test]
    async fn terminal_foreground_allowlist_passes_through() {
        let allowed_name = "EXEC03_FG_ALLOWED_VAR";
        let allowed_val = "exec03-fg-allowed-value-xyz";
        // SAFETY: test-only.
        unsafe { std::env::set_var(allowed_name, allowed_val) };

        let tool = TerminalTool::new().with_env_allowlist(vec![allowed_name.to_string()]);
        let output = tool
            .execute(serde_json::json!({
                "command": format!("printenv {allowed_name} 2>/dev/null; true")
            }))
            .await
            .unwrap();

        assert!(
            output.contains(allowed_val),
            "allowlist var {allowed_name} must pass through to foreground child: output={output:?}"
        );

        // SAFETY: cleanup.
        unsafe { std::env::remove_var(allowed_name) };
    }

    // -------------------------------------------------------------------------
    // Background path (T-42-04 / D-06)
    // -------------------------------------------------------------------------

    /// EXEC-03 background gate: secret planted in parent env is absent from the
    /// background-spawn child env (SpawnSpec → ProcessRegistry::spawn()).
    ///
    /// ProcessRegistry::spawn() splits the command with shell_words (no shell),
    /// so this test wraps the probe in `sh -c '...'` to get proper shell semantics
    /// (file redirection, sentinel write). A sentinel line proves the command
    /// actually ran — without it an empty file would be a false positive.
    ///
    /// Both Task 1 (SpawnSpec.env = sanitized) AND Task 2 (env_clear in spawn())
    /// are required for this test to pass: Task 1 alone leaves the child inheriting
    /// the parent env additively; Task 2 clears it first.
    #[tokio::test]
    async fn terminal_background_env_sanitized() {
        let secret_name = "EXEC03_BG_SECRET_TOKEN";
        let secret_val = "exec03-bg-secret-placeholder-xyz789";
        // SAFETY: test-only; unique var name avoids cross-test races.
        unsafe { std::env::set_var(secret_name, secret_val) };

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let tmp_path = tmp.path().to_string_lossy().into_owned();

        let reg = Arc::new(RwLock::new(ProcessRegistry::new_for_session(
            "exec03-bg-test",
        )));
        let tool = TerminalTool::new()
            .with_process_registry(reg.clone())
            .with_env_allowlist(vec![]);

        // Use `sh -c '...'` so file redirection and sentinel write work correctly.
        // ProcessRegistry splits the command with shell_words into [sh, -c, script]
        // so the shell handles the redirect — no shell means no redirect.
        let script = format!(
            "printenv {secret_name} 2>/dev/null > {tmp_path}; echo EXEC03_DONE >> {tmp_path}"
        );
        let resp = tool
            .execute(serde_json::json!({
                "command": format!("sh -c '{script}'"),
                "background": true,
            }))
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&resp).unwrap();
        let proc_id = parsed["process_id"].as_str().unwrap().to_string();

        // Wait for the background command to complete before reading the file.
        {
            let mut r = reg.write().await;
            let _ = r.wait(&proc_id, std::time::Duration::from_secs(10)).await;
        }

        let file_contents = std::fs::read_to_string(&tmp_path).unwrap_or_default();

        // Sentinel must be present — proves the command actually ran and wrote output.
        // Without this check an empty file would be a false-positive pass.
        assert!(
            file_contents.contains("EXEC03_DONE"),
            "background command did not write sentinel — command may not have run: file={file_contents:?}"
        );

        // T-42-04: secret must be absent from background child env.
        assert!(
            !file_contents.contains(secret_val),
            "background path leaked {secret_name} via env: file={file_contents:?}"
        );

        // Best-effort cleanup of any lingering background process.
        reg.write().await.drain_and_kill().await.ok();

        // SAFETY: cleanup.
        unsafe { std::env::remove_var(secret_name) };
    }
}
