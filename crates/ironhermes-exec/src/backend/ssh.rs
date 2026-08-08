//! `SshEnvironment` — OpenSSH ControlMaster CLI shell-out backend (§5 of
//! `docs/EXEC-BACKENDS-ARCHITECTURE.md`). Smallest backend: one persistent
//! master connection multiplexes every command/file-transfer over a single
//! Unix socket instead of re-handshaking per call.
//!
//! Design notes (see individual doc comments for detail):
//! - `control_socket_path` (Pitfall 6 / T-36.3.12-18): a fixed-length hashed
//!   socket filename so a long hostname can never overflow the OS
//!   `sun_path` limit (104 bytes macOS, 108 Linux).
//! - `connect()` is the D-05 hard-availability check: an unreachable host or
//!   missing/invalid `terminal.ssh` config hard-errors — this backend NEVER
//!   falls back to running the command locally.
//! - `run_bash` forwards ONLY `terminal.forward_env`-named vars into the
//!   remote session (D-09 scrub-by-default) — `ssh` itself does not forward
//!   host env by default, so the safe state is the starting state; this
//!   backend adds an explicit, allow-listed opt-in on top.
//! - `SshTransport` (bottom of this file) implements
//!   [`super::file_sync::FileTransport`] via tar-over-ssh (§5.2), the
//!   transport [`super::file_sync::FileSyncManager`] uses for `before_execute`.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use sha2::{Digest, Sha256};

use super::executor::run_bash_with_limits;
use super::file_sync::{FileSyncManager, FileTransport};
use super::{BackendUnavailableError, Environment, ExecResult, RunOpts};

/// Fixed-length, deterministic socket-path builder for the OpenSSH
/// ControlMaster multiplexed connection (Pitfall 6 / T-36.3.12-18): even an
/// arbitrarily long hostname always collapses to a short, constant-length
/// hex digest, so `<tmp_dir>/hermes-ssh-<digest>` stays comfortably under the
/// OS `sun_path` limit — unlike a naive `ControlPath=~/.ssh/cm-{user}@{host}
/// :{port}`, which silently degrades to a non-multiplexed connection (or
/// fails outright with "ControlPath too long") for a long hostname.
///
/// Mirrors the porting spec (`docs/EXEC-BACKENDS-ARCHITECTURE.md` §5.1) and the
/// Python reference: `sha256(user@host:port)` truncated to 16 hex chars.
pub fn control_socket_path(user: &str, host: &str, port: u16) -> PathBuf {
    let key = format!("{user}@{host}:{port}");
    let digest = hex::encode(Sha256::digest(key.as_bytes()));
    std::env::temp_dir().join(format!("hermes-ssh-{}", &digest[..16]))
}

/// D-09: compute the env vars this backend explicitly `export`s into the
/// remote bash session before the wrapped command runs. `ssh` itself does
/// NOT forward host env into a remote session by default (no `SendEnv`
/// config is used here) — so absent this function, nothing at all would
/// cross. This reuses the same sanitized base+allowlist layering as
/// `LocalEnvironment`/`terminal.rs` (`ironhermes_core::build_terminal_safe_env`,
/// per `36.3.12-PATTERNS.md`'s explicit "don't reimplement env filtering"
/// guidance) so only `forward_env`-named vars (plus the always-safe base
/// layer) are ever exported into the remote session — no host creds by
/// default (D-09).
fn forwarded_env(forward_env: &[String]) -> std::collections::HashMap<String, String> {
    ironhermes_core::build_terminal_safe_env(forward_env, &[])
}

/// `true` if `key` is a valid POSIX shell identifier: first char ASCII
/// alphabetic or `_`, every subsequent char ASCII alphanumeric or `_`
/// (`^[A-Za-z_][A-Za-z0-9_]*$`, hand-rolled rather than pulling in a regex
/// crate for one trivial predicate — WR-04, Phase 36.3.12 Plan 10).
fn is_valid_shell_identifier(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Build the `export KEY=value\n` prefix prepended to every remote `bash -c`
/// script (WR-04, Phase 36.3.12 Plan 10).
///
/// Restores the module's documented "every interpolated segment is quoted or
/// validated" invariant: `value` was already `shell_words::quote`d, but `key`
/// was spliced in raw. A malformed key (containing whitespace, `;`, `$`, or
/// any other shell metacharacter) cannot be fixed by quoting — `export "ODD
/// KEY"=v` is not a valid bash assignment regardless of quoting, so quoting a
/// bad key would only defer the failure to an opaque remote error. Instead,
/// each key is validated as a shell identifier BEFORE splicing; a pair whose
/// key fails validation is skipped (never emitted) and a `tracing::warn!`
/// names the rejected key so an operator who mistyped a `forward_env` entry
/// gets a local, actionable signal instead of a silent no-op or a remote
/// syntax error.
///
/// Exploitability context (not a live vulnerability — defense-in-depth only):
/// every key here originates from `TerminalConfig.forward_env`, operator-only
/// config (D-06 — never LLM/tool-call input), filtered through
/// `build_terminal_safe_env`, which only inserts a key when
/// `std::env::var(key)` already succeeds in the host process's own env.
fn build_export_prefix(forwarded: &[(String, String)]) -> String {
    let mut export_prefix = String::new();
    for (key, value) in forwarded {
        if !is_valid_shell_identifier(key) {
            tracing::warn!(
                target: "ironhermes::exec::ssh",
                rejected_key = %key,
                "forward_env key is not a valid shell identifier — skipping export \
                 (WR-04): fix the `terminal.forward_env` entry in config.yaml"
            );
            continue;
        }
        let quoted_value = shell_words::quote(value);
        export_prefix.push_str(&format!("export {key}={quoted_value}\n"));
    }
    export_prefix
}

/// Connection parameters + the derived ControlMaster socket path, shared
/// between [`SshEnvironment`]'s own `ssh`/`bash -c` invocations and
/// [`SshTransport`]'s tar-over-ssh/scp file-transfer invocations so both
/// reuse the identical multiplexed master connection.
#[derive(Clone)]
struct SshConnParams {
    user: String,
    host: String,
    port: u16,
    key_path: Option<String>,
    control_socket: PathBuf,
}

impl SshConnParams {
    fn user_host(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    /// The six ControlMaster/security options every `ssh`/`scp` invocation
    /// against this host shares (§5.1): `ControlMaster=auto` multiplexes
    /// (auto-establishing the master on first use, reusing it thereafter);
    /// `BatchMode=yes` fails fast instead of prompting interactively;
    /// `StrictHostKeyChecking=accept-new` is TOFU (T-36.3.12-17, accepted
    /// risk); `ConnectTimeout=10` bounds an unreachable-host hang.
    fn push_common_opts(&self, cmd: &mut tokio::process::Command) {
        cmd.arg("-o");
        cmd.arg(format!("ControlPath={}", self.control_socket.display()));
        cmd.arg("-o");
        cmd.arg("ControlMaster=auto");
        cmd.arg("-o");
        cmd.arg("ControlPersist=300");
        cmd.arg("-o");
        cmd.arg("BatchMode=yes");
        cmd.arg("-o");
        cmd.arg("StrictHostKeyChecking=accept-new");
        cmd.arg("-o");
        cmd.arg("ConnectTimeout=10");
    }

    fn push_identity_opts(&self, cmd: &mut tokio::process::Command, port_flag: &str) {
        if self.port != 22 {
            cmd.arg(port_flag);
            cmd.arg(self.port.to_string());
        }
        if let Some(key) = &self.key_path {
            cmd.arg("-i");
            cmd.arg(key);
        }
    }

    /// `ssh [opts] user@host` — callers append trailing remote command args.
    fn ssh_command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("ssh");
        self.push_common_opts(&mut cmd);
        self.push_identity_opts(&mut cmd, "-p");
        cmd.arg(self.user_host());
        cmd
    }

    /// `scp [opts]` — callers append source/destination args. `scp` uses
    /// `-P` (uppercase) for the port flag, unlike `ssh`'s `-p`.
    fn scp_command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("scp");
        self.push_common_opts(&mut cmd);
        self.push_identity_opts(&mut cmd, "-P");
        cmd
    }

    /// `ssh [opts] -O exit [identity] user@host` — drop the ControlMaster
    /// and let `ssh` unlink its own socket (§5.2's `cleanup()`).
    fn control_exit_command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("ssh");
        self.push_common_opts(&mut cmd);
        cmd.arg("-O");
        cmd.arg("exit");
        self.push_identity_opts(&mut cmd, "-p");
        cmd.arg(self.user_host());
        cmd
    }
}

/// SSH backend: `bash -c` over an OpenSSH ControlMaster-multiplexed CLI
/// shell-out (D-04 — no `russh`/`openssh` crate; every command is a real
/// `ssh` binary spawn via `tokio::process`).
pub struct SshEnvironment {
    params: SshConnParams,
    forward_env: Vec<String>,
    /// Retained for parity with the Python reference's constructor shape and
    /// possible future use (e.g. per-task remote workspace naming); not yet
    /// read by any method here.
    #[allow(dead_code)]
    task_id: String,
    sync: tokio::sync::Mutex<FileSyncManager<SshTransport>>,
    /// The current `(local_path, remote_path)` set `before_execute` pushes
    /// via `FileSyncManager::sync` (§7). Empty by default: this plan wires
    /// the change-map/rate-limit/transport machinery end-to-end and
    /// unit-tests it (`file_sync.rs`'s fake-transport tests), but IronHermes
    /// does not yet define a concrete analog of the Python reference's
    /// `~/.hermes` `iter_sync_files()` enumeration — that policy is left for
    /// a future phase to populate via [`SshEnvironment::with_tracked_files`]
    /// rather than inventing an unspecified convention in this plan.
    tracked_files: Vec<(PathBuf, String)>,
}

impl SshEnvironment {
    /// D-05 hard availability check (Researcher Decision: D-05): validates
    /// `terminal.ssh` is populated, derives the hashed ControlMaster socket
    /// path, then opens/verifies the master connection via a trivial remote
    /// no-op (`ssh ... true`). `ControlMaster=auto` means this call itself
    /// establishes the persistent master if none exists yet; `BatchMode=yes`
    /// together with `ConnectTimeout=10` means an unreachable host, wrong
    /// key, or auth failure fails fast and loud here — NEVER falling back
    /// to `LocalEnvironment` (D-05).
    pub async fn connect(
        cfg: &ironhermes_core::config::TerminalConfig,
        task_id: &str,
    ) -> anyhow::Result<Self> {
        let ssh_cfg = cfg.ssh.clone().ok_or_else(|| {
            BackendUnavailableError(
                "terminal.ssh is not configured (host/user required for backend: ssh)"
                    .to_string(),
            )
        })?;

        if ssh_cfg.host.trim().is_empty() || ssh_cfg.user.trim().is_empty() {
            return Err(BackendUnavailableError(
                "terminal.ssh.host and terminal.ssh.user must both be set (non-empty)"
                    .to_string(),
            )
            .into());
        }

        let control_socket = control_socket_path(&ssh_cfg.user, &ssh_cfg.host, ssh_cfg.port);
        let params = SshConnParams {
            user: ssh_cfg.user,
            host: ssh_cfg.host,
            port: ssh_cfg.port,
            key_path: ssh_cfg.key_path,
            control_socket,
        };

        let mut probe = params.ssh_command();
        probe.arg("true");
        probe.stdout(Stdio::null());
        probe.stderr(Stdio::piped());
        probe.kill_on_drop(true);

        let output = probe.output().await.map_err(|e| {
            BackendUnavailableError(format!(
                "failed to spawn ssh to reach {}: {e}",
                params.user_host()
            ))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BackendUnavailableError(format!(
                "cannot reach ssh host {} ({}): {}",
                params.user_host(),
                output.status,
                stderr.trim()
            ))
            .into());
        }

        let transport = SshTransport {
            params: params.clone(),
        };

        Ok(Self {
            params,
            forward_env: cfg.forward_env.clone(),
            task_id: task_id.to_string(),
            sync: tokio::sync::Mutex::new(FileSyncManager::new(transport)),
            tracked_files: Vec::new(),
        })
    }

    /// Register the local files to push to the remote host before each
    /// command (§7's tracked-file set). See the `tracked_files` field doc
    /// comment for why this defaults to empty.
    pub fn with_tracked_files(mut self, files: Vec<(PathBuf, String)>) -> Self {
        self.tracked_files = files;
        self
    }
}

#[async_trait::async_trait]
impl Environment for SshEnvironment {
    /// `cmd` is already the fully wrapped script from `Session::wrap_command`
    /// (source-snapshot / cd / eval / re-dump / cwd-marker). This prepends
    /// `export KEY=value` lines for every `forward_env`-allow-listed var
    /// (D-09), then runs the whole thing as ONE shell-quoted argument to a
    /// remote `bash -c` (`bash -l -c` when `opts.login`, mirroring §5.1's
    /// `["bash","-l","-c",shlex.quote(cmd_string)] if login else [...]`) —
    /// driven through the shared [`run_bash_with_limits`] helper so
    /// timeout→124 / cancel→130 / drain apply identically to every other
    /// backend.
    ///
    /// WR-04 (Phase 36.3.12 Plan 10): each key is validated as a shell
    /// identifier before being spliced into an `export` line — see
    /// [`build_export_prefix`]'s doc for the reject-vs-quote rationale. A
    /// non-conforming key is skipped (never emitted) with a `tracing::warn!`.
    async fn run_bash(&self, cmd: &str, opts: RunOpts) -> anyhow::Result<ExecResult> {
        let forwarded: Vec<(String, String)> =
            forwarded_env(&self.forward_env).into_iter().collect();
        let export_prefix = build_export_prefix(&forwarded);
        let full_script = format!("{export_prefix}{cmd}");

        let mut ssh_cmd = self.params.ssh_command();
        ssh_cmd.arg("bash");
        if opts.login {
            ssh_cmd.arg("-l");
        }
        ssh_cmd.arg("-c");
        ssh_cmd.arg(shell_words::quote(&full_script).into_owned());
        ssh_cmd.stdout(Stdio::piped());
        ssh_cmd.stderr(Stdio::piped());
        ssh_cmd.kill_on_drop(true);

        let child = ssh_cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn ssh for remote exec: {e}"))?;

        run_bash_with_limits(child, opts.timeout, opts.cancel).await
    }

    /// `ssh -O exit` drops the ControlMaster (§5.2); best-effort unlink of
    /// the socket file afterward in case `ssh` itself didn't clean it up
    /// (e.g. the master was never fully established). Neither step's
    /// failure surfaces as a hard error — there is nothing left to tear
    /// down if the master never existed.
    async fn cleanup(&mut self) -> anyhow::Result<()> {
        let mut exit_cmd = self.params.control_exit_command();
        exit_cmd.stdout(Stdio::null());
        exit_cmd.stderr(Stdio::null());
        let _ = exit_cmd.status().await;
        let _ = std::fs::remove_file(&self.params.control_socket);
        Ok(())
    }

    /// §7: push any changed tracked files to the remote host before the
    /// next command runs, rate-limited to once per 5s window by
    /// `FileSyncManager` itself.
    async fn before_execute(&self) -> anyhow::Result<()> {
        let mut sync = self.sync.lock().await;
        sync.sync(&self.tracked_files).await
    }
}

/// Tar-over-ssh [`FileTransport`] impl (§5.2): a single `tar c | ssh … tar x`
/// stream instead of N `scp` calls for a batch; a lone file falls back to a
/// direct `scp` over the same ControlMaster socket.
struct SshTransport {
    params: SshConnParams,
}

#[async_trait::async_trait]
impl FileTransport for SshTransport {
    async fn bulk_upload(&self, files: &[(PathBuf, String)]) -> anyhow::Result<()> {
        if files.is_empty() {
            return Ok(());
        }
        if files.len() == 1 {
            let (local, remote) = &files[0];
            return self.scp_upload(local, remote).await;
        }
        self.tar_upload(files).await
    }

    /// Downloads the remote tracked-file base (`$HOME`) into `dest` as a
    /// single tar-over-ssh stream (§5.2's download mirror of the upload
    /// path).
    async fn bulk_download(&self, dest: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(dest)
            .map_err(|e| anyhow::anyhow!("failed to create download dest {dest:?}: {e}"))?;

        let mut ssh_cmd = self.params.ssh_command();
        ssh_cmd.arg("tar cf - -C \"$HOME\" .");
        ssh_cmd.stdout(Stdio::piped());
        ssh_cmd.kill_on_drop(true);
        let mut ssh_proc = ssh_cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn ssh for tar-over-ssh download: {e}"))?;
        let ssh_stdout = ssh_proc
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("ssh stdout pipe missing"))?;
        let ssh_stdio: Stdio = ssh_stdout
            .try_into()
            .map_err(|e| anyhow::anyhow!("failed to convert ssh stdout to a Stdio: {e}"))?;

        let tar_status = tokio::process::Command::new("tar")
            .arg("-xf")
            .arg("-")
            .arg("-C")
            .arg(dest)
            .stdin(ssh_stdio)
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("local tar extraction failed to spawn: {e}"))?;

        let ssh_status = ssh_proc
            .wait()
            .await
            .map_err(|e| anyhow::anyhow!("ssh download process failed: {e}"))?;

        if !tar_status.success() || !ssh_status.success() {
            anyhow::bail!(
                "tar-over-ssh bulk download failed (local tar: {tar_status}, remote ssh: {ssh_status})"
            );
        }
        Ok(())
    }

    /// Every remote path is `shell_words::quote`d before being embedded into
    /// the single remote command string (T-36.3.12-15 — no hand-rolled
    /// quoting).
    async fn delete(&self, paths: &[String]) -> anyhow::Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let quoted: Vec<String> = paths
            .iter()
            .map(|p| shell_words::quote(p).into_owned())
            .collect();
        let remote_cmd = format!("cd \"$HOME\" && rm -f -- {}", quoted.join(" "));

        let mut ssh_cmd = self.params.ssh_command();
        ssh_cmd.arg(remote_cmd);
        let status = ssh_cmd
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("failed to spawn ssh for remote delete: {e}"))?;

        if !status.success() {
            anyhow::bail!("remote delete failed: {status}");
        }
        Ok(())
    }
}

impl SshTransport {
    /// Single-file fallback (§5.2): a direct `scp` over the same
    /// ControlMaster socket instead of staging a tar for one file.
    async fn scp_upload(&self, local_path: &Path, remote_path: &str) -> anyhow::Result<()> {
        let mut cmd = self.params.scp_command();
        cmd.arg(local_path);
        cmd.arg(format!("{}:{}", self.params.user_host(), remote_path));
        let status = cmd
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("failed to spawn scp: {e}"))?;

        if !status.success() {
            anyhow::bail!("scp upload of {local_path:?} to {remote_path} failed: {status}");
        }
        Ok(())
    }

    /// Stages every `(local_path, remote_relative_path)` pair into a
    /// symlink-mirrored temp dir (avoids `tar --transform` fragility, per
    /// §5.2), then streams `tar -chf - -C <staging> .` into
    /// `ssh … tar xf - --no-overwrite-dir -C "$HOME"` as a single TCP
    /// stream instead of N individual transfers.
    async fn tar_upload(&self, files: &[(PathBuf, String)]) -> anyhow::Result<()> {
        let staging = tempfile::tempdir()
            .map_err(|e| anyhow::anyhow!("failed to create tar staging dir: {e}"))?;

        for (local_path, remote_rel) in files {
            let dest = staging.path().join(remote_rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    anyhow::anyhow!("failed to create staging parent for {remote_rel}: {e}")
                })?;
            }
            symlink_or_copy(local_path, &dest)?;
        }

        let mut tar_proc = tokio::process::Command::new("tar")
            .arg("-chf")
            .arg("-")
            .arg("-C")
            .arg(staging.path())
            .arg(".")
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn local tar for upload: {e}"))?;
        let tar_stdout = tar_proc
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("tar stdout pipe missing"))?;
        let tar_stdio: Stdio = tar_stdout
            .try_into()
            .map_err(|e| anyhow::anyhow!("failed to convert tar stdout to a Stdio: {e}"))?;

        let mut ssh_cmd = self.params.ssh_command();
        ssh_cmd.arg("tar xf - --no-overwrite-dir -C \"$HOME\"");
        ssh_cmd.stdin(tar_stdio);
        let ssh_status = ssh_cmd
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("failed to spawn ssh for tar-over-ssh upload: {e}"))?;

        let tar_status = tar_proc
            .wait()
            .await
            .map_err(|e| anyhow::anyhow!("local tar process failed: {e}"))?;

        if !tar_status.success() || !ssh_status.success() {
            anyhow::bail!(
                "tar-over-ssh bulk upload failed (local tar: {tar_status}, remote ssh: {ssh_status})"
            );
        }
        Ok(())
    }
}

/// Symlinks `src` at `dest` on Unix (avoids copying file contents into the
/// staging dir, per §5.2); falls back to a real copy on non-Unix targets
/// where symlinks aren't universally available/permitted.
fn symlink_or_copy(src: &Path, dest: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dest)
            .map_err(|e| anyhow::anyhow!("failed to symlink {src:?} -> {dest:?}: {e}"))
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(src, dest)
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("failed to copy {src:?} -> {dest:?}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::create_environment;

    // -------------------------------------------------------------------
    // Pitfall 6: control_socket_path stays under the OS sun_path limit
    // -------------------------------------------------------------------

    #[test]
    fn control_socket_path_stays_under_sun_path_limit_for_long_hostname() {
        let long_host = "h".repeat(200);
        let path = control_socket_path("user", &long_host, 22);
        let path_str = path.to_string_lossy();
        assert!(
            path_str.len() < 104,
            "socket path is {} bytes, expected < 104 (macOS sun_path limit): {path_str}",
            path_str.len()
        );
    }

    #[test]
    fn control_socket_path_is_deterministic_for_same_tuple() {
        let a = control_socket_path("alice", "example.com", 22);
        let b = control_socket_path("alice", "example.com", 22);
        assert_eq!(a, b, "the same (user,host,port) tuple must yield the same socket path");
    }

    #[test]
    fn control_socket_path_differs_for_different_ports() {
        let a = control_socket_path("alice", "example.com", 22);
        let b = control_socket_path("alice", "example.com", 2222);
        assert_ne!(a, b, "different ports must yield different socket paths");
    }

    #[test]
    fn control_socket_path_differs_for_different_hosts() {
        let a = control_socket_path("alice", "example.com", 22);
        let b = control_socket_path("alice", "example.org", 22);
        assert_ne!(a, b, "different hosts must yield different socket paths");
    }

    // -------------------------------------------------------------------
    // D-09: scrub-by-default env, forward_env allowlist
    // -------------------------------------------------------------------

    #[test]
    fn forwarded_env_excludes_unlisted_var_by_default() {
        let secret_name = "SSH_BACKEND_TEST_FOO_TOKEN";
        // SAFETY: test-only; unique var name avoids cross-test races.
        unsafe { std::env::set_var(secret_name, "leaked-secret-value") };

        let env = forwarded_env(&[]);
        assert!(
            !env.contains_key(secret_name),
            "a FOO_TOKEN-style var must not be forwarded without an explicit forward_env entry (D-09): {env:?}"
        );

        // SAFETY: cleanup.
        unsafe { std::env::remove_var(secret_name) };
    }

    #[test]
    fn forwarded_env_includes_var_named_in_forward_env() {
        let secret_name = "SSH_BACKEND_TEST_FOO_TOKEN_ALLOWED";
        // SAFETY: test-only; unique var name avoids cross-test races.
        unsafe { std::env::set_var(secret_name, "allowed-value") };

        let env = forwarded_env(&[secret_name.to_string()]);
        assert_eq!(
            env.get(secret_name).map(String::as_str),
            Some("allowed-value"),
            "a var named in forward_env must cross (D-09 explicit opt-in): {env:?}"
        );

        // SAFETY: cleanup.
        unsafe { std::env::remove_var(secret_name) };
    }

    // -------------------------------------------------------------------
    // WR-04 (Phase 36.3.12 Plan 10): export-prefix KEY validation
    // -------------------------------------------------------------------

    /// A forwarded pair whose key is not a valid shell identifier (contains a
    /// space, a semicolon, or a `$`) is excluded from the generated export
    /// prefix — the prefix contains no line for it.
    #[test]
    fn export_prefix_rejects_key_that_is_not_a_shell_identifier() {
        let bad_keys = [
            "ODD KEY",
            "KEY;rm -rf /",
            "KEY$(whoami)",
            "1LEADS_WITH_DIGIT",
        ];
        for bad_key in bad_keys {
            let forwarded = vec![(bad_key.to_string(), "value".to_string())];
            let prefix = build_export_prefix(&forwarded);
            assert!(
                prefix.is_empty(),
                "a non-shell-identifier key {bad_key:?} must be skipped entirely, got: {prefix:?}"
            );
        }
    }

    /// A forwarded pair with an ordinary key (matching `^[A-Za-z_][A-Za-z0-9_]*$`)
    /// still produces exactly one `export` line, with the value quoted as
    /// before — proving the fix does not regress D-09's allowlist forwarding.
    #[test]
    fn export_prefix_emits_valid_identifier_keys_unchanged() {
        let forwarded = vec![
            ("MY_TOKEN".to_string(), "plain-value".to_string()),
            ("_leading_underscore".to_string(), "another".to_string()),
        ];
        let prefix = build_export_prefix(&forwarded);
        assert!(
            prefix.contains("export MY_TOKEN=plain-value\n"),
            "a valid identifier key must produce an unmodified export line: {prefix:?}"
        );
        assert!(
            prefix.contains("export _leading_underscore=another\n"),
            "a leading-underscore identifier key must be accepted: {prefix:?}"
        );
        assert_eq!(
            prefix.lines().count(),
            2,
            "exactly one export line per valid key, no extras: {prefix:?}"
        );
    }

    /// A value containing shell metacharacters is quoted such that it cannot
    /// terminate the assignment — the existing quoting behavior is locked,
    /// not changed, by this plan's key-validation addition.
    #[test]
    fn export_prefix_quotes_the_value() {
        let forwarded = vec![("MY_TOKEN".to_string(), "value; rm -rf / #".to_string())];
        let prefix = build_export_prefix(&forwarded);
        // shell_words::quote wraps a value containing metacharacters in single
        // quotes — the exact same treatment run_bash always applied to values.
        let expected_quoted = shell_words::quote("value; rm -rf / #");
        assert_eq!(
            prefix,
            format!("export MY_TOKEN={expected_quoted}\n"),
            "the value must still be shell_words::quote'd exactly as before: {prefix:?}"
        );
    }

    // -------------------------------------------------------------------
    // D-05: hard-error, never a local fallback
    // -------------------------------------------------------------------

    /// D-05: selecting `backend: ssh` with no `terminal.ssh` config at all
    /// must hard-error — it must never silently execute the command
    /// locally. No live host is involved: `connect()` rejects a missing/
    /// empty `terminal.ssh` before ever spawning the `ssh` binary, so this
    /// test is fast and network-free (project constraint: no live SSH host
    /// in this plan).
    #[tokio::test]
    async fn connect_hard_errors_when_ssh_config_absent_no_local_fallback() {
        let cfg = ironhermes_core::config::TerminalConfig {
            backend: "ssh".to_string(),
            ..Default::default()
        };

        let result = create_environment(&cfg, "test-task", "test-profile").await;
        let err = result
            .err()
            .expect("ssh backend must hard-error when terminal.ssh is unset");
        let msg = err.to_string();
        assert!(
            msg.contains("ssh backend unavailable"),
            "error should name the failing backend: {msg}"
        );
        assert!(
            msg.contains("terminal.backend: local"),
            "error should mention the local fallback option (operator choice, not silent): {msg}"
        );
    }

    /// D-05, extra coverage: a `terminal.ssh` config with an empty host must
    /// also hard-error (and, like the test above, never spawn the real `ssh`
    /// binary — host is empty before any command is built).
    #[tokio::test]
    async fn connect_hard_errors_when_host_empty_no_local_fallback() {
        let cfg = ironhermes_core::config::TerminalConfig {
            backend: "ssh".to_string(),
            ssh: Some(ironhermes_core::config::SshBackendConfig {
                host: String::new(),
                user: "someone".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = create_environment(&cfg, "test-task", "test-profile").await;
        assert!(
            result.is_err(),
            "ssh backend must hard-error when host is empty, never falling back to local"
        );
    }
}
