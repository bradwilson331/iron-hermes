//! Sandboxed script execution + wake-gate parsing.

use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use ironhermes_core::get_hermes_home;

/// Outcome of running a cron job script.
#[derive(Debug, Clone)]
pub struct ScriptOutcome {
    /// `true` if the script exited with status 0, `false` on non-zero exit or timeout.
    pub ok: bool,
    /// Captured stdout (after pass-through redaction).
    pub stdout: String,
    /// Captured stderr (after pass-through redaction), or timeout message.
    pub stderr: String,
    /// Wake-agent flag: `false` only when the last non-empty stdout line is
    /// `{"wakeAgent": false}`; `true` in all other cases (default-wake).
    pub wake_agent: bool,
}

/// Select the interpreter for a script based on its file extension.
///
/// `.sh` / `.bash` → `$BASH_PATH` env override, else `/bin/bash`.
/// Everything else → `$PYTHON_PATH` env override, else `python3`.
fn select_interpreter(script_path: &Path) -> String {
    match script_path.extension().and_then(|e| e.to_str()) {
        Some("sh") | Some("bash") => {
            std::env::var("BASH_PATH").unwrap_or_else(|_| "/bin/bash".to_string())
        }
        _ => std::env::var("PYTHON_PATH").unwrap_or_else(|_| "python3".to_string()),
    }
}

/// Parse an unsigned-integer environment variable, returning `default` on
/// absence or parse failure.
fn parse_env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Pass-through redaction placeholder.
///
/// TODO(Phase 32.x): wire to a workspace secret-redaction primitive once one
/// exists. For now, pass-through preserves the API shape so future callers
/// don't need to change.
fn redact_passthrough(s: String) -> String {
    s
}

/// Parse the wake-agent gate from a script's stdout.
///
/// Returns `false` only when the **last non-empty line** of `stdout` is valid
/// JSON containing `{"wakeAgent": false}`. Every other shape returns `true`
/// (default-wake).
pub fn parse_wake_gate(stdout: &str) -> bool {
    let last_non_empty = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(last_non_empty) {
        if matches!(v.get("wakeAgent").and_then(|v| v.as_bool()), Some(false)) {
            return false;
        }
    }
    true
}

/// Run a cron job script under a sandboxed directory.
///
/// # Sandbox
///
/// `script_name` is resolved relative to `${IRONHERMES_HOME}/scripts/`.
/// After `canonicalize()`, the resolved path MUST still be a child of that
/// directory — any path traversal (e.g. `../etc/passwd`) or absolute path
/// injection returns `Err` containing `"escapes scripts_dir"`.
///
/// # Interpreter selection
///
/// Extension `.sh`/`.bash` → bash (from `$BASH_PATH` or `/bin/bash`).
/// Anything else → python3 (from `$PYTHON_PATH` or `python3`).
///
/// # Timeout
///
/// Controlled by `IRONHERMES_CRON_SCRIPT_TIMEOUT` seconds (default 120).
/// On timeout the process is killed (`kill_on_drop`) and the function returns
/// `Ok(ScriptOutcome { ok: false, stderr: "script timed out after Xs" })`.
pub async fn run_job_script(script_name: &str) -> Result<ScriptOutcome> {
    let scripts_dir = get_hermes_home().join("scripts");

    // Create the scripts directory if it doesn't exist, hardened to 0700 on Unix.
    tokio::fs::create_dir_all(&scripts_dir)
        .await
        .with_context(|| format!("create scripts dir: {:?}", scripts_dir))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            &scripts_dir,
            std::fs::Permissions::from_mode(0o700),
        );
    }

    // Reject absolute paths and path components that escape the scripts dir
    // BEFORE canonicalize(), so we give a clear error even when the target
    // file does not exist on disk.
    let script_path = Path::new(script_name);
    if script_path.is_absolute() {
        bail!("script path escapes scripts_dir: {:?}", script_path);
    }
    // Reject any path that contains a `..` component — these can escape the
    // scripts dir regardless of whether the target file exists.
    for component in script_path.components() {
        if component == std::path::Component::ParentDir {
            bail!("script path escapes scripts_dir: {:?}", script_path);
        }
    }

    let candidate = scripts_dir.join(script_name);
    let scripts_dir_canonical = scripts_dir
        .canonicalize()
        .with_context(|| format!("canonicalize scripts dir: {:?}", scripts_dir))?;
    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("script not found or unreadable: {:?}", candidate))?;

    // Final check: even without `..`, a symlink could point outside.
    if !canonical.starts_with(&scripts_dir_canonical) {
        bail!("script path escapes scripts_dir: {:?}", canonical);
    }

    let interpreter = select_interpreter(&canonical);
    let timeout_secs = parse_env_u64("IRONHERMES_CRON_SCRIPT_TIMEOUT", 120);

    let child = Command::new(&interpreter)
        .arg(&canonical)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            format!(
                "spawn script {:?} with {}",
                canonical, interpreter
            )
        })?;

    let outcome = match timeout(
        Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(out)) => {
            let stdout =
                redact_passthrough(String::from_utf8_lossy(&out.stdout).into_owned());
            let stderr =
                redact_passthrough(String::from_utf8_lossy(&out.stderr).into_owned());
            let wake_agent = parse_wake_gate(&stdout);
            ScriptOutcome {
                ok: out.status.success(),
                stdout,
                stderr,
                wake_agent,
            }
        }
        Ok(Err(e)) => return Err(anyhow!("script subprocess failure: {}", e)),
        Err(_) => ScriptOutcome {
            ok: false,
            stdout: String::new(),
            stderr: format!("script timed out after {}s", timeout_secs),
            wake_agent: true,
        },
    };

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    // Serialize all env-mutating tests to avoid data races.
    // Returns the guard even if the lock was poisoned by a prior test panic.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let m = LOCK.get_or_init(|| Mutex::new(()));
        m.lock().unwrap_or_else(|e| e.into_inner())
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
        let _guard = env_lock();
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

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
        let _guard = env_lock();
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

        let outcome = super::run_job_script("test.py").await.unwrap();
        assert!(outcome.ok, "expected ok=true");
        assert_eq!(outcome.stdout.trim(), "world");
    }

    // Test 3: path traversal rejection
    #[tokio::test]
    async fn test_path_traversal_rejected() {
        let tmp = TempDir::new().unwrap();
        let _guard = env_lock();
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
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
        let _guard = env_lock();
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
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
        let _guard = env_lock();
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
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
        let _guard = env_lock();
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

        let outcome = super::run_job_script("fail.sh").await.unwrap();
        assert!(!outcome.ok, "expected ok=false for exit 1");
        assert!(outcome.wake_agent, "wake_agent should still be true");
    }

    // Test 7: script timeout
    #[tokio::test]
    async fn test_script_timeout() {
        let tmp = TempDir::new().unwrap();
        make_script(&tmp, "slow.sh", "#!/bin/sh\nsleep 5\n");
        let _guard = env_lock();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
            std::env::set_var("IRONHERMES_CRON_SCRIPT_TIMEOUT", "1");
        }

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

        unsafe { std::env::remove_var("IRONHERMES_CRON_SCRIPT_TIMEOUT"); }
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
        let _guard = env_lock();
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
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
        let _guard = env_lock();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
            std::env::set_var("BASH_PATH", "/bin/bash");
        }

        let outcome = super::run_job_script("hi.sh").await.unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.stdout.trim(), "hi");

        unsafe { std::env::remove_var("BASH_PATH"); }
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
        let _guard = env_lock();
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

        let outcome = super::run_job_script("gate.sh").await.unwrap();
        assert!(outcome.ok, "exit 0 should be ok=true");
        assert!(
            !outcome.wake_agent,
            "wake_agent should be false when last line is {{\"wakeAgent\": false}}"
        );
    }
}
