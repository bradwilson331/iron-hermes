//! Sandboxed script execution + wake-gate parsing.

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    // Serialize all env-mutating tests to avoid data races.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    // Helper: create a script file in <tmpdir>/scripts/ and set IRONHERMES_HOME.
    fn make_script(tmp: &TempDir, name: &str, content: &str) {
        let scripts = tmp.path().join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        let path = scripts.join(name);
        std::fs::write(&path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    // Test 1: happy path bash
    #[tokio::test]
    async fn test_happy_path_bash() {
        let tmp = TempDir::new().unwrap();
        make_script(&tmp, "test.sh", "#!/bin/sh\necho hello\n");
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("IRONHERMES_HOME", tmp.path());

        let outcome = super::run_job_script("test.sh").await.unwrap();
        assert!(outcome.ok, "expected ok=true");
        assert_eq!(outcome.stdout.trim(), "hello");
        assert!(outcome.wake_agent, "default wake_agent should be true");
    }

    // Test 2: happy path python
    #[tokio::test]
    async fn test_happy_path_python() {
        let tmp = TempDir::new().unwrap();
        make_script(&tmp, "test.py", "print('world')\n");
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("IRONHERMES_HOME", tmp.path());

        let outcome = super::run_job_script("test.py").await.unwrap();
        assert!(outcome.ok, "expected ok=true");
        assert_eq!(outcome.stdout.trim(), "world");
    }

    // Test 3: path traversal rejection
    #[tokio::test]
    async fn test_path_traversal_rejected() {
        let tmp = TempDir::new().unwrap();
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("IRONHERMES_HOME", tmp.path());
        // create scripts dir so the traversal reaches canonicalize
        std::fs::create_dir_all(tmp.path().join("scripts")).unwrap();

        let err = super::run_job_script("../etc/passwd").await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("escapes scripts_dir"),
            "expected 'escapes scripts_dir' in: {msg}"
        );
    }

    // Test 4: absolute path rejection
    #[tokio::test]
    async fn test_absolute_path_rejected() {
        let tmp = TempDir::new().unwrap();
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("IRONHERMES_HOME", tmp.path());
        std::fs::create_dir_all(tmp.path().join("scripts")).unwrap();

        let err = super::run_job_script("/etc/passwd").await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("escapes scripts_dir"),
            "expected 'escapes scripts_dir' in: {msg}"
        );
    }

    // Test 5: non-existent script
    #[tokio::test]
    async fn test_nonexistent_script() {
        let tmp = TempDir::new().unwrap();
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("IRONHERMES_HOME", tmp.path());
        std::fs::create_dir_all(tmp.path().join("scripts")).unwrap();

        let err = super::run_job_script("nope.sh").await.unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("not found") || msg.contains("unreadable") || msg.contains("no such"),
            "expected not-found error in: {msg}"
        );
    }

    // Test 6: non-zero exit
    #[tokio::test]
    async fn test_nonzero_exit() {
        let tmp = TempDir::new().unwrap();
        make_script(&tmp, "fail.sh", "#!/bin/sh\nexit 1\n");
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("IRONHERMES_HOME", tmp.path());

        let outcome = super::run_job_script("fail.sh").await.unwrap();
        assert!(!outcome.ok, "expected ok=false for exit 1");
        assert!(outcome.wake_agent, "wake_agent should still be true");
    }

    // Test 7: script timeout
    #[tokio::test]
    async fn test_script_timeout() {
        let tmp = TempDir::new().unwrap();
        make_script(&tmp, "slow.sh", "#!/bin/sh\nsleep 5\n");
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("IRONHERMES_HOME", tmp.path());
        std::env::set_var("IRONHERMES_CRON_SCRIPT_TIMEOUT", "1");

        let start = std::time::Instant::now();
        let outcome = super::run_job_script("slow.sh").await.unwrap();
        let elapsed = start.elapsed().as_secs();

        assert!(!outcome.ok, "expected ok=false on timeout");
        assert!(
            outcome.stderr.contains("script timed out"),
            "expected 'script timed out' in stderr: {}",
            outcome.stderr
        );
        assert!(elapsed < 3, "expected timeout under 3s, got {}s", elapsed);

        std::env::remove_var("IRONHERMES_CRON_SCRIPT_TIMEOUT");
    }

    // Test 8: wake gate true (default — non-JSON)
    #[test]
    fn test_wake_gate_non_json() {
        assert!(super::parse_wake_gate("any non-json line\nanother line\n"));
    }

    // Test 9: wake gate true (valid JSON without wakeAgent)
    #[test]
    fn test_wake_gate_json_without_wake_agent() {
        assert!(super::parse_wake_gate(r#"{"foo":"bar"}"#));
    }

    // Test 10: wake gate false
    #[test]
    fn test_wake_gate_false() {
        assert!(!super::parse_wake_gate(
            "some preamble\n{\"wakeAgent\": false}\n"
        ));
    }

    // Test 11: wake gate true when wakeAgent=true
    #[test]
    fn test_wake_gate_true_explicit() {
        assert!(super::parse_wake_gate(r#"{"wakeAgent": true}"#));
    }

    // Test 12: trailing blank lines ignored
    #[test]
    fn test_wake_gate_trailing_blanks() {
        assert!(!super::parse_wake_gate("{\"wakeAgent\":false}\n\n\n"));
    }

    // Test 13: scripts dir auto-create + chmod 0700 on Unix
    #[tokio::test]
    #[cfg(unix)]
    async fn test_scripts_dir_created_with_permissions() {
        use std::os::unix::fs::MetadataExt;
        let tmp = TempDir::new().unwrap();
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("IRONHERMES_HOME", tmp.path());
        // scripts dir does NOT exist yet — run_job_script should create it
        // (the call will fail with not-found, but the dir should be created)
        let _ = super::run_job_script("nonexistent.sh").await;

        let scripts_dir = tmp.path().join("scripts");
        assert!(scripts_dir.exists(), "scripts dir should be created");
        let meta = std::fs::metadata(&scripts_dir).unwrap();
        let mode = meta.mode() & 0o777;
        assert_eq!(mode, 0o700, "scripts dir should have mode 0700, got {mode:o}");
    }

    // Test 14: BASH_PATH override
    #[tokio::test]
    async fn test_bash_path_override() {
        let tmp = TempDir::new().unwrap();
        make_script(&tmp, "hi.sh", "#!/bin/sh\necho hi\n");
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("IRONHERMES_HOME", tmp.path());
        std::env::set_var("BASH_PATH", "/bin/bash");

        let outcome = super::run_job_script("hi.sh").await.unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.stdout.trim(), "hi");

        std::env::remove_var("BASH_PATH");
    }

    // Test 15: ScriptOutcome.wake_agent populated from parse_wake_gate
    #[tokio::test]
    async fn test_outcome_wake_agent_false() {
        let tmp = TempDir::new().unwrap();
        make_script(
            &tmp,
            "gate.sh",
            "#!/bin/sh\necho '{\"wakeAgent\": false}'\n",
        );
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("IRONHERMES_HOME", tmp.path());

        let outcome = super::run_job_script("gate.sh").await.unwrap();
        assert!(outcome.ok, "exit 0 should be ok=true");
        assert!(
            !outcome.wake_agent,
            "wake_agent should be false when last line is {{\"wakeAgent\": false}}"
        );
    }
}
