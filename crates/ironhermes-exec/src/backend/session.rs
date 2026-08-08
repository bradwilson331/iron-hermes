//! `Session` — snapshot/cwd persistence across fresh `bash -c` calls.
//!
//! Ports `docs/EXEC-BACKENDS-ARCHITECTURE.md` §2 byte-for-byte in spirit
//! (D-04): because every command is a fresh `bash -c` with no persistent
//! shell, env/alias/function/cwd state is reconstructed via a snapshot file
//! re-sourced before, and re-dumped after, each command; cwd travels via an
//! in-band `printf` marker in stdout. See also `36.3.12-RESEARCH.md`
//! "Pattern 1: Session core" for the concrete Rust sketch this mirrors.

use uuid::Uuid;

/// Backend-agnostic session state: snapshot path, cwd marker, and whether the
/// snapshot has been successfully bootstrapped yet.
///
/// 1. `id` uniquely names this session's snapshot/marker inside the target
///    environment's `/tmp` so concurrent sessions never collide.
/// 2. `snapshot_path` is a shell script (inside the target env) capturing
///    `export -p` / functions / aliases, re-sourced before and re-dumped
///    after every command (§2.1/§2.2).
/// 3. `cwd_file` mirrors the Python reference's local-backend cwd channel
///    (a temp file, read directly instead of parsed from stdout) — kept here
///    for backend parity even though this plan's `wrap_command`/
///    `extract_cwd` pair only exercises the marker channel (§2.3).
/// 4. `cwd_marker` is the in-band stdout marker pair
///    (`__HERMES_CWD_<id>__<path>__HERMES_CWD_<id>__`) remote backends parse.
/// 5. `ready` flips true once `bootstrap_script` has run successfully;
///    `Executor::execute` uses `!ready` to decide whether this call needs a
///    login shell fallback (§2.1).
/// 6. `cwd` is the session's best-known current working directory inside the
///    target environment, updated by `Executor::execute` after every call via
///    `extract_cwd` so the next caller-supplied `cwd` naturally continues
///    from wherever the last `cd` landed.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub snapshot_path: String,
    pub cwd_file: String,
    pub cwd_marker: String,
    pub ready: bool,
    pub cwd: String,
}

impl Session {
    /// Construct a new session rooted at `initial_cwd`. `id` is derived from
    /// a fresh UUIDv4 (first 12 hex chars, matching the Python reference's
    /// `uuid.uuid4().hex[:12]`).
    pub fn new(initial_cwd: impl Into<String>) -> Self {
        let id = Uuid::new_v4().simple().to_string()[..12].to_string();
        Self {
            snapshot_path: format!("/tmp/hermes-snap-{id}.sh"),
            cwd_file: format!("/tmp/hermes-cwd-{id}.txt"),
            cwd_marker: format!("__HERMES_CWD_{id}__"),
            id,
            ready: false,
            cwd: initial_cwd.into(),
        }
    }

    /// The one-time bootstrap script (§2.1), run as a login shell to capture
    /// the target environment's exported vars/functions/aliases into
    /// `snapshot_path`. Callers set `ready = true` after this succeeds.
    pub fn bootstrap_script(&self, cwd: &str) -> String {
        let quoted_cwd = shell_words::quote(cwd);
        let snap = shell_words::quote(&self.snapshot_path);
        let cwd_file = shell_words::quote(&self.cwd_file);
        format!(
            "export -p > {snap}\n\
             declare -f | grep -vE '^_[^_]' >> {snap}\n\
             alias -p >> {snap}\n\
             echo 'shopt -s expand_aliases' >> {snap}\n\
             echo 'set +e' >> {snap}\n\
             echo 'set +u' >> {snap}\n\
             builtin cd {quoted_cwd} 2>/dev/null || true\n\
             pwd -P > {cwd_file} 2>/dev/null || true\n\
             printf '\\n{marker}%s{marker}\\n' \"$(pwd -P)\"\n",
            marker = self.cwd_marker,
        )
    }

    /// Wrap `cmd` into a script that sources the snapshot (when `ready`),
    /// `cd`s into `cwd` (or `exit 126`), runs the command via `eval`,
    /// captures its exit code, re-dumps env (when `ready`), prints the cwd
    /// marker line, and exits with the captured code (§2.2). Every
    /// interpolated cwd/command segment is passed through `shell_words::
    /// quote` — no raw string interpolation into the shell (T-36.3.12-03).
    pub fn wrap_command(&self, cmd: &str, cwd: &str) -> String {
        let mut parts = Vec::new();

        if self.ready {
            parts.push(format!(
                "source {} >/dev/null 2>&1 || true",
                shell_words::quote(&self.snapshot_path)
            ));
        }

        parts.push(format!(
            "builtin cd -- {} || exit 126",
            shell_words::quote(cwd)
        ));
        parts.push(format!("eval {}", shell_words::quote(cmd)));
        parts.push("__hermes_ec=$?".to_string());

        if self.ready {
            parts.push(format!(
                "export -p > {} 2>/dev/null || true",
                shell_words::quote(&self.snapshot_path)
            ));
        }

        parts.push(format!(
            "printf '\\n{marker}%s{marker}\\n' \"$(pwd -P)\"",
            marker = self.cwd_marker
        ));
        parts.push("exit $__hermes_ec".to_string());

        parts.join("\n")
    }

    /// Locate the `__HERMES_CWD_<id>__<path>__HERMES_CWD_<id>__` marker pair
    /// in `output`, return the extracted path, and strip the marker line
    /// (including its injected leading newline, and the trailing newline
    /// `printf` emits) so callers never see it (§2.3).
    pub fn extract_cwd(&mut self, output: &mut String) -> Option<String> {
        let marker = self.cwd_marker.as_str();

        let first_start = output.find(marker)?;
        let after_first = first_start + marker.len();
        let second_start = output[after_first..].find(marker)? + after_first;
        let path = output[after_first..second_start].to_string();
        let after_second = second_start + marker.len();

        // The printf format string is `\n{marker}%s{marker}\n` — consume the
        // leading newline it emitted before the marker (if present) and the
        // trailing newline after it, so the marker line disappears entirely
        // rather than leaving a blank line behind.
        let removal_start = if first_start > 0 && output.as_bytes()[first_start - 1] == b'\n' {
            first_start - 1
        } else {
            first_start
        };
        let removal_end = if output[after_second..].starts_with('\n') {
            after_second + 1
        } else {
            after_second
        };

        output.replace_range(removal_start..removal_end, "");

        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// Runs `script` through a real `bash -c`, returning (stdout, exit
    /// code). Used by the round-trip tests below to prove `wrap_command` /
    /// `extract_cwd` actually reproduce the Python reference's cwd/env
    /// persistence semantics against a real shell — not a mock.
    async fn run_bash(script: &str) -> (String, i32) {
        let output = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .expect("bash spawn");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let code = output.status.code().unwrap_or(-1);
        (stdout, code)
    }

    fn ready_session() -> Session {
        let mut sess = Session::new("/tmp");
        sess.ready = true;
        sess
    }

    #[test]
    fn wrap_command_and_extract_cwd_contain_expected_pieces() {
        let sess = ready_session();
        let wrapped = sess.wrap_command("echo hi", "/tmp");
        assert!(wrapped.contains("builtin cd -- /tmp || exit 126"));
        assert!(wrapped.contains("eval 'echo hi'"));
        assert!(wrapped.contains(&sess.cwd_marker));
        assert!(wrapped.contains("exit $__hermes_ec"));
    }

    #[tokio::test]
    async fn round_trip_cd_persists_across_two_calls() {
        // Call A: `cd /tmp` — the snapshot doesn't need to exist on disk yet
        // for cwd persistence, since call B receives its cwd argument from
        // whatever call A's extract_cwd returned (this is the Executor's
        // job in Task 2; here we drive it manually to lock the primitive).
        //
        // Expected cwd is computed via `pwd -P`'s own physical-path
        // resolution (not a literal "/tmp" string) because on macOS /tmp is
        // a symlink to /private/tmp, and `pwd -P` — like our wrapped script
        // — reports the resolved physical path.
        let expected_tmp = physical_tmp().await;
        let mut sess = ready_session();

        let wrapped_a = sess.wrap_command("cd /tmp", "/");
        let (mut out_a, code_a) = run_bash(&wrapped_a).await;
        assert_eq!(code_a, 0, "call A should succeed, got output: {out_a:?}");
        let cwd_a = sess.extract_cwd(&mut out_a).expect("marker present");
        assert_eq!(cwd_a, expected_tmp);
        assert!(
            !out_a.contains(&sess.cwd_marker),
            "extract_cwd must strip the marker line"
        );

        // Call B: use the cwd extracted from call A as the next cwd — this
        // is what the Executor will do in Task 2.
        let wrapped_b = sess.wrap_command("pwd -P", &cwd_a);
        let (mut out_b, code_b) = run_bash(&wrapped_b).await;
        assert_eq!(code_b, 0, "call B should succeed, got output: {out_b:?}");
        let cwd_b = sess.extract_cwd(&mut out_b).expect("marker present");
        assert_eq!(cwd_b, expected_tmp);
        assert!(
            out_b.trim() == expected_tmp,
            "pwd -P output should report {expected_tmp}, got: {out_b:?}"
        );
    }

    /// Resolves `/tmp` the same way `pwd -P` does inside the wrapped script,
    /// so round-trip assertions are correct on platforms (macOS) where
    /// `/tmp` is itself a symlink.
    async fn physical_tmp() -> String {
        let (out, code) = run_bash("cd /tmp && pwd -P").await;
        assert_eq!(code, 0, "resolving physical /tmp should succeed");
        out.trim().to_string()
    }

    #[tokio::test]
    async fn round_trip_export_persists_across_two_calls_via_snapshot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let snap_path = dir.path().join("hermes-snap-test.sh");

        let mut sess = ready_session();
        sess.snapshot_path = snap_path.to_str().expect("utf8 path").to_string();

        // Snapshot file must exist before `source ... || true` is a no-op
        // success rather than a missing-file no-op — create it empty first,
        // mirroring what `bootstrap_script`/an initial run would produce.
        std::fs::write(&sess.snapshot_path, "").expect("seed snapshot");

        // Call A: export FOO=bar — `ready=true` means wrap_command re-dumps
        // `export -p` into the snapshot after the command runs.
        let wrapped_a = sess.wrap_command("export FOO=bar", "/tmp");
        let (mut out_a, code_a) = run_bash(&wrapped_a).await;
        assert_eq!(code_a, 0, "call A should succeed, got output: {out_a:?}");
        sess.extract_cwd(&mut out_a);

        let snapshot_contents = std::fs::read_to_string(&sess.snapshot_path).expect("read snap");
        assert!(
            snapshot_contents.contains("FOO=\"bar\"") || snapshot_contents.contains("FOO=bar"),
            "snapshot should have persisted FOO=bar, got: {snapshot_contents:?}"
        );

        // Call B: a fresh `bash -c` that sources the snapshot should see FOO.
        let wrapped_b = sess.wrap_command("echo \"FOO=$FOO\"", "/tmp");
        let (mut out_b, code_b) = run_bash(&wrapped_b).await;
        assert_eq!(code_b, 0, "call B should succeed, got output: {out_b:?}");
        sess.extract_cwd(&mut out_b);
        assert!(
            out_b.contains("FOO=bar"),
            "call B should see FOO=bar re-sourced from the snapshot, got: {out_b:?}"
        );
    }

    #[tokio::test]
    async fn wrap_command_cd_nonexistent_exits_126() {
        let sess = ready_session();
        let wrapped = sess.wrap_command("echo unreachable", "/nonexistent-hermes-exec-test-dir");
        let (_out, code) = run_bash(&wrapped).await;
        assert_eq!(code, 126, "cd into a nonexistent dir must exit 126");
    }

    #[test]
    fn extract_cwd_strips_marker_line_and_leading_newline() {
        let mut sess = ready_session();
        let mut output = format!("hello\n\n{}/tmp{}\n", sess.cwd_marker, sess.cwd_marker);
        let cwd = sess.extract_cwd(&mut output).expect("marker present");
        assert_eq!(cwd, "/tmp");
        assert_eq!(output, "hello\n");
        assert!(!output.contains(&sess.cwd_marker));
    }

    #[test]
    fn extract_cwd_returns_none_when_marker_absent() {
        let mut sess = ready_session();
        let mut output = "no marker here\n".to_string();
        assert_eq!(sess.extract_cwd(&mut output), None);
        assert_eq!(output, "no marker here\n");
    }

    #[test]
    fn new_session_derives_distinct_ids() {
        let a = Session::new("/tmp");
        let b = Session::new("/tmp");
        assert_ne!(a.id, b.id);
        assert_eq!(a.id.len(), 12);
        assert!(a.snapshot_path.contains(&a.id));
        assert!(a.cwd_marker.contains(&a.id));
        assert!(!a.ready);
    }
}
