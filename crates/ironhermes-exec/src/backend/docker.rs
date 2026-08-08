//! `DockerEnvironment` — the full persistent-container lifecycle (Plan 05).
//!
//! Ports `docs/EXEC-BACKENDS-ARCHITECTURE.md` §4 into `ironhermes-exec::backend`:
//! `{runtime} run -d --init … sleep infinity` with label-based cross-process
//! reuse (§4.2, D-02), `{runtime} exec` per command driven through the shared
//! `Session`/`Executor` primitives (§4.1), cap-drop security args (§4.5, D-09),
//! bind-mount vs tmpfs (§4.4), allow-listed `-e` env injection at init only
//! (§4.1's note, D-09), "No such container" same-backend recovery (§4.3, D-05
//! recoverable-vs-unavailable distinction), persist-mode no-op cleanup (§4.6),
//! and a startup orphan-reaper (§4.6, D-02). Runtime is the explicit
//! `docker`|`podman` knob (D-07) — never auto-detected.
//!
//! Plan 04 landed only the D-05 availability preflight as a compiling stub
//! (`connect()` always errored or bailed, `run_bash` always bailed). This
//! plan replaces those bails with the real lifecycle while preserving Plan
//! 04's preflight/hard-error contract and its two existing tests verbatim.

use std::process::Stdio;

use super::executor::run_bash_with_limits;
use super::{BackendUnavailableError, Environment, ExecResult, RecoverableEnvError, RunOpts};

/// Persistent-container handle. `container_id` is interior-mutable
/// (`tokio::sync::RwLock`) because `Environment::run_bash` only receives
/// `&self` — the D-05 "No such container" same-backend recovery path
/// (§4.3) must be able to swap in a freshly recreated container id from
/// inside a `&self` method.
pub struct DockerEnvironment {
    /// `docker` | `podman` — explicit config value, never auto-detected (D-07).
    runtime: String,
    task_id: String,
    profile: String,
    cwd: String,
    image: String,
    /// D-09: operator-declared credential allowlist
    /// (`TerminalConfig.forward_env`), forwarded via `-e` only on init calls.
    forward_env: Vec<String>,
    /// The security/mount/resource args used to create the container —
    /// stashed so `recreate_container` can rebuild an identical container
    /// (same image + same `all_run_args`) after an out-of-band removal.
    all_run_args: Vec<String>,
    /// `container.persistent` (D-02/§4.4/§4.6): controls BOTH bind-mount vs
    /// tmpfs at creation time AND whether `cleanup()` is a no-op (persist
    /// mode — background processes survive `/quit`) or a real
    /// `stop`+`rm -f` (ephemeral mode). Single source of truth per
    /// `TerminalConfig`'s one exposed knob.
    persistent: bool,
    container_id: tokio::sync::RwLock<String>,
}

impl DockerEnvironment {
    /// D-05 availability preflight (Researcher Decision: D-05) — unchanged
    /// from the Plan 04 stub. Runs `{runtime} version`; a missing binary or
    /// non-zero exit hard-errors with a `BackendUnavailableError` naming the
    /// fix. D-07: the runtime binary name is an explicit config value, never
    /// auto-detected.
    async fn preflight(runtime: &str) -> Result<(), BackendUnavailableError> {
        let status = tokio::process::Command::new(runtime)
            .arg("version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(BackendUnavailableError(format!(
                "`{runtime} version` exited with {status}; install/start the `{runtime}` \
                 runtime, switch terminal.container_runtime, or set terminal.backend: local"
            ))),
            Err(e) => Err(BackendUnavailableError(format!(
                "`{runtime}` binary not found or failed to run ({e}); install the `{runtime}` \
                 runtime, set terminal.container_runtime: podman, or set terminal.backend: local"
            ))),
        }
    }

    /// Full §4 lifecycle: (1) D-05 preflight, (2) label-based reuse or
    /// fresh create (D-02/§4.1/§4.2).
    pub async fn connect(
        cfg: &ironhermes_core::config::TerminalConfig,
        task_id: &str,
        profile: &str,
    ) -> anyhow::Result<Self> {
        let runtime = cfg.container_runtime.clone();
        Self::preflight(&runtime).await?;

        let container_cfg = cfg.container.clone();
        let mut all_run_args = security_args(&container_cfg);
        all_run_args.extend(mount_args(&container_cfg, task_id));
        all_run_args.extend(resource_args(&container_cfg));

        let container_id = match find_reusable_container(&runtime, task_id, profile).await? {
            Some((id, state)) => {
                if state != "running" {
                    run_checked(&runtime, &["start", &id]).await?;
                }
                id
            }
            None => {
                let container_name = new_container_name();
                create_container(
                    &runtime,
                    &container_name,
                    task_id,
                    profile,
                    &cfg.cwd,
                    &cfg.image,
                    &all_run_args,
                )
                .await?
            }
        };

        Ok(Self {
            runtime,
            task_id: task_id.to_string(),
            profile: profile.to_string(),
            cwd: cfg.cwd.clone(),
            image: cfg.image.clone(),
            forward_env: cfg.forward_env.clone(),
            all_run_args,
            persistent: container_cfg.persistent,
            container_id: tokio::sync::RwLock::new(container_id),
        })
    }

    /// D-09: allow-listed `-e KEY=VAL` args for `docker exec`. Reuses the
    /// existing `ironhermes_core::build_terminal_safe_env` helper (Don't
    /// Hand-Roll — same function `LocalEnvironment` and `terminal.rs`
    /// already call) so only the base `SAFE_ENV_KEYS`/`XDG_*`/
    /// `IRONHERMES_HOME` set plus names explicitly listed in
    /// `cfg.forward_env` ever cross the host→container boundary. A
    /// `KEY`/`TOKEN`-pattern var not named in `forward_env` never appears
    /// here because it is not in `SAFE_ENV_KEYS` and was never opted in.
    fn init_env_args(&self) -> Vec<String> {
        ironhermes_core::build_terminal_safe_env(&self.forward_env, &[])
            .into_iter()
            .flat_map(|(k, v)| ["-e".to_string(), format!("{k}={v}")])
            .collect()
    }

    /// Single `{runtime} exec` attempt (§4.1). Host env is injected via
    /// `-e` ONLY when `opts.login` is true (i.e. the session's first call,
    /// before the snapshot exists) so `export -p` captures it into the
    /// snapshot — subsequent calls source the snapshot instead and carry no
    /// `-e` (matches `docker.py:943`'s `if login: cmd.extend(self._init_env_args)`).
    async fn exec_once(&self, cmd: &str, opts: &RunOpts) -> anyhow::Result<ExecResult> {
        let container_id = self.container_id.read().await.clone();
        let env_args = if opts.login {
            self.init_env_args()
        } else {
            Vec::new()
        };
        let argv = build_exec_argv(
            &self.runtime,
            &container_id,
            cmd,
            opts.login,
            &env_args,
            opts.stdin.is_some(),
        );

        let mut command = tokio::process::Command::new(&argv[0]);
        command
            .args(&argv[1..])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if opts.stdin.is_some() {
            command.stdin(Stdio::piped());
        }

        let mut child = command.spawn()?;

        if let Some(stdin_data) = &opts.stdin
            && let Some(mut stdin_pipe) = child.stdin.take()
        {
            use tokio::io::AsyncWriteExt;
            let _ = stdin_pipe.write_all(stdin_data.as_bytes()).await;
            drop(stdin_pipe);
        }

        run_bash_with_limits(child, opts.timeout, opts.cancel.clone()).await
    }

    /// D-05 recoverable path (§4.3): a container removed out-of-band is
    /// re-attached by label (in case another process already recreated it)
    /// or recreated fresh from the SAME `image` + `all_run_args` this
    /// environment was constructed with. This is same-backend self-heal,
    /// never the forbidden cross-backend fallback to local.
    async fn recreate_container(&self) -> anyhow::Result<()> {
        if let Some((id, state)) =
            find_reusable_container(&self.runtime, &self.task_id, &self.profile).await?
        {
            if state != "running" {
                run_checked(&self.runtime, &["start", &id]).await?;
            }
            *self.container_id.write().await = id;
            return Ok(());
        }

        let container_name = new_container_name();
        let id = create_container(
            &self.runtime,
            &container_name,
            &self.task_id,
            &self.profile,
            &self.cwd,
            &self.image,
            &self.all_run_args,
        )
        .await?;
        *self.container_id.write().await = id;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Environment for DockerEnvironment {
    /// `{runtime} exec` (+ `-i` if stdin) `<id> bash -c <wrapped>`, driven
    /// through the shared [`run_bash_with_limits`] helper so the 124/130/
    /// drain contract is identical to every other backend. If the exec
    /// attempt hits a "No such container"/"is not running" signature
    /// (container removed out-of-band, §4.3), recreate-by-label and retry
    /// exactly once — a same-backend `RecoverableEnvError` self-heal, not a
    /// `BackendUnavailableError` cross-backend fallback (D-05).
    async fn run_bash(&self, cmd: &str, opts: RunOpts) -> anyhow::Result<ExecResult> {
        let first = self.exec_once(cmd, &opts).await?;

        if first.returncode != 0 && is_no_such_container_signature(&first.output) {
            let stale_id = self.container_id.read().await.clone();
            tracing::warn!(
                container_id = %stale_id,
                runtime = %self.runtime,
                "docker exec hit a No-such-container/not-running signature — \
                 recreating by label and retrying once (D-05 same-backend recovery)"
            );

            self.recreate_container().await.map_err(|e| {
                RecoverableEnvError(format!("docker container recreate-by-label failed: {e}"))
            })?;

            return self.exec_once(cmd, &opts).await;
        }

        Ok(first)
    }

    /// D-02/§4.6: no-op in persist mode (the container keeps running so
    /// background processes like `npm run dev` survive `/quit`); otherwise
    /// `{runtime} stop -t 10` then `{runtime} rm -f`.
    async fn cleanup(&mut self) -> anyhow::Result<()> {
        if self.persistent {
            return Ok(());
        }
        let id = self.container_id.read().await.clone();
        let _ = run_checked(&self.runtime, &["stop", "-t", "10", &id]).await;
        let _ = run_checked(&self.runtime, &["rm", "-f", &id]).await;
        Ok(())
    }
}

/// D-09 §4.5: drop-all-then-add-back. The container is the isolation
/// boundary — `--cap-drop ALL` then selective add-back of only
/// `DAC_OVERRIDE`/`CHOWN`/`FOWNER`, `--security-opt no-new-privileges`, a
/// `--pids-limit`, and size-capped `/tmp`+`/var/tmp` tmpfs mounts. Never
/// passes `--userns` — Researcher Decision: D-07 explicitly forbids it
/// (podman is already namespaced by default; passing `--userns=host` would
/// opt OUT of that, and the daemon-side flag is meaningless for docker
/// unless the daemon itself remaps).
fn security_args(container_cfg: &ironhermes_core::config::ContainerResourceConfig) -> Vec<String> {
    let mut args = vec![
        "--cap-drop".to_string(),
        "ALL".to_string(),
        "--cap-add".to_string(),
        "DAC_OVERRIDE".to_string(),
        "--cap-add".to_string(),
        "CHOWN".to_string(),
        "--cap-add".to_string(),
        "FOWNER".to_string(),
        "--security-opt".to_string(),
        "no-new-privileges".to_string(),
        "--pids-limit".to_string(),
        container_cfg.pids_limit.to_string(),
        "--tmpfs".to_string(),
        "/tmp:rw,nosuid,size=512m".to_string(),
        "--tmpfs".to_string(),
        "/var/tmp:rw,noexec,nosuid,size=256m".to_string(),
    ];
    if !container_cfg.network {
        args.push("--network=none".to_string());
    }
    args
}

/// §4.4: bind-mount `/workspace` under `$IRONHERMES_HOME/sandboxes/<task_id>/
/// workspace` when `container.persistent` (survives container recreation),
/// else a size-capped ephemeral tmpfs.
fn mount_args(
    container_cfg: &ironhermes_core::config::ContainerResourceConfig,
    task_id: &str,
) -> Vec<String> {
    if container_cfg.persistent {
        let host_dir = ironhermes_core::get_hermes_home()
            .join("sandboxes")
            .join(sanitize_task_id_for_path(task_id))
            .join("workspace");
        // Best-effort: if this fails, `{runtime} run` will fail loudly
        // (no silent fallback) rather than silently mounting nothing.
        let _ = std::fs::create_dir_all(&host_dir);
        vec!["-v".to_string(), format!("{}:/workspace", host_dir.display())]
    } else {
        vec![
            "--tmpfs".to_string(),
            format!("/workspace:rw,exec,size={}m", container_cfg.disk_mib),
        ]
    }
}

/// `--cpus`/`--memory` resource caps (§4.5).
fn resource_args(container_cfg: &ironhermes_core::config::ContainerResourceConfig) -> Vec<String> {
    vec![
        "--cpus".to_string(),
        container_cfg.cpu.to_string(),
        "--memory".to_string(),
        format!("{}m", container_cfg.memory_mib),
    ]
}

/// Keeps `task_id` safe as a path component for the bind-mount host dir —
/// non-alphanumeric/`-`/`_` characters become `_`.
fn sanitize_task_id_for_path(task_id: &str) -> String {
    task_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `hermes-<8 hex chars>` container name (mirrors `docker.py:221`'s
/// `f"hermes-{uuid.uuid4().hex[:8]}"`).
fn new_container_name() -> String {
    format!("hermes-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

/// D-02/§4.2: search for an existing container by `(task_id, profile)`
/// labels ONLY — not image/mounts/resources — so cross-process reuse
/// tolerates config drift between the process that created the container
/// and the process reattaching to it. Returns `(container_id, state)` for
/// the first match, if any.
async fn find_reusable_container(
    runtime: &str,
    task_id: &str,
    profile: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let output = tokio::process::Command::new(runtime)
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("label=hermes-task-id={task_id}"),
            "--filter",
            &format!("label=hermes-profile={profile}"),
            "--format",
            "{{.ID}}\t{{.State}}",
        ])
        .output()
        .await?;

    if !output.status.success() {
        anyhow::bail!(
            "`{runtime} ps` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().find_map(|line| {
        let mut parts = line.splitn(2, '\t');
        let id = parts.next()?.trim();
        let state = parts.next()?.trim();
        if id.is_empty() {
            None
        } else {
            Some((id.to_string(), state.to_string()))
        }
    }))
}

/// D-02/§4.1: `{runtime} run -d --init --name … --label … -w <cwd>
/// <all_run_args> <image> sleep infinity`. Returns the new container id.
async fn create_container(
    runtime: &str,
    container_name: &str,
    task_id: &str,
    profile: &str,
    cwd: &str,
    image: &str,
    all_run_args: &[String],
) -> anyhow::Result<String> {
    let mut args: Vec<String> = vec![
        "run".to_string(),
        "-d".to_string(),
        "--init".to_string(),
        "--name".to_string(),
        container_name.to_string(),
        "--label".to_string(),
        "hermes-agent=1".to_string(),
        "--label".to_string(),
        format!("hermes-task-id={task_id}"),
        "--label".to_string(),
        format!("hermes-profile={profile}"),
        "-w".to_string(),
        cwd.to_string(),
    ];
    args.extend(all_run_args.iter().cloned());
    args.push(image.to_string());
    args.push("sleep".to_string());
    args.push("infinity".to_string());

    let output = tokio::process::Command::new(runtime)
        .args(&args)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "`{runtime} run` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Runs `{runtime} <args>`, mapping a non-zero exit into an `anyhow` error
/// carrying stderr. Used for `start`/`stop`/`rm -f` where the caller only
/// needs success/failure plus trimmed stdout (a container id, for `start`
/// this is unused).
async fn run_checked(runtime: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = tokio::process::Command::new(runtime)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "`{runtime} {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Builds the `{runtime} exec …` argv (§4.1, `docker.py:943`'s `_run_bash`).
/// A pure function (no spawning) so D-07's "explicit runtime, never
/// auto-detected" contract is unit-testable without a live daemon: argv[0]
/// must equal `runtime` verbatim for both `"docker"` and `"podman"`.
fn build_exec_argv(
    runtime: &str,
    container_id: &str,
    cmd: &str,
    login_env_injection: bool,
    env_args: &[String],
    has_stdin: bool,
) -> Vec<String> {
    let mut argv = vec![runtime.to_string(), "exec".to_string()];
    if has_stdin {
        argv.push("-i".to_string());
    }
    if login_env_injection {
        argv.extend(env_args.iter().cloned());
    }
    argv.push(container_id.to_string());
    argv.push("bash".to_string());
    argv.push("-c".to_string());
    argv.push(cmd.to_string());
    argv
}

/// §4.3: detect the specific `docker`/`podman exec` failure signature that
/// means "the container is gone/stopped out of band" — distinct from the
/// wrapped command's own non-zero exit, which never reaches this codepath
/// because in that case `docker exec` itself succeeds (the wrapped script
/// ran; `ExecResult.returncode` is the *script's* exit code, not the CLI's).
fn is_no_such_container_signature(output: &str) -> bool {
    let lower = output.to_lowercase();
    lower.contains("no such container")
        || lower.contains("is not running")
        || lower.contains("no container with name or id")
        || lower.contains("no container with id")
}

/// §4.6: startup orphan-reaper (D-02). Runs ONCE at process boot (no
/// background timer thread) — scans every `hermes-agent=1`-labeled
/// container and `{runtime} rm -f`'s any whose `State.StartedAt` is older
/// than `2 × reap_after_secs`. Best-effort: a `ps`/`inspect` failure (e.g.
/// daemon unreachable) is swallowed rather than propagated, since this is a
/// GC pass, not a request the caller is blocked on.
pub async fn reap_orphan_containers(runtime: &str, reap_after_secs: u64) -> anyhow::Result<()> {
    let threshold = chrono::Duration::seconds(reap_after_secs.saturating_mul(2) as i64);
    let now = chrono::Utc::now();

    let output = tokio::process::Command::new(runtime)
        .args([
            "ps",
            "-a",
            "--filter",
            "label=hermes-agent=1",
            "--format",
            "{{.ID}}",
        ])
        .output()
        .await;
    let Ok(output) = output else {
        return Ok(());
    };
    if !output.status.success() {
        return Ok(());
    }

    let ids: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    for id in ids {
        let Ok(inspect) = tokio::process::Command::new(runtime)
            .args(["inspect", "--format", "{{.State.StartedAt}}", &id])
            .output()
            .await
        else {
            continue;
        };
        if !inspect.status.success() {
            continue;
        }
        let started_at_str = String::from_utf8_lossy(&inspect.stdout).trim().to_string();
        let Ok(started_at) = chrono::DateTime::parse_from_rfc3339(&started_at_str) else {
            continue;
        };
        let idle = now.signed_duration_since(started_at.with_timezone(&chrono::Utc));
        if idle > threshold {
            let _ = tokio::process::Command::new(runtime)
                .args(["rm", "-f", &id])
                .status()
                .await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::backend::create_environment;

    use super::*;

    /// D-05 hard-error contract, locked against this dev machine's actual
    /// missing `docker` binary (per RESEARCH.md "Environment Availability" —
    /// confirmed absent, `podman` present instead): selecting `backend:
    /// docker` with a definitely-absent runtime binary must hard-error with
    /// a message naming the fix, and must NEVER silently execute the
    /// command locally. Unchanged from Plan 04 — the D-05 preflight this
    /// locks was not touched by this plan's lifecycle additions.
    #[tokio::test]
    async fn unavailable_hard_errors() {
        let cfg = ironhermes_core::config::TerminalConfig {
            backend: "docker".to_string(),
            container_runtime: "definitely-absent-hermes-exec-test-binary-xyz".to_string(),
            ..Default::default()
        };

        let result = create_environment(&cfg, "test-task", "test-profile").await;
        let err = result
            .err()
            .expect("docker backend with absent runtime must hard-error");
        let msg = err.to_string();

        assert!(
            msg.contains("docker backend unavailable"),
            "error should name the failing backend: {msg}"
        );
        assert!(
            msg.contains("terminal.backend: local"),
            "error should mention the local fallback option (operator choice, not silent): {msg}"
        );
    }

    /// The `docker` (`container_runtime`) preflight failure is surfaced as a
    /// hard error — no local execution path exists for it to fall through
    /// to. This is asserted implicitly by `unavailable_hard_errors` above
    /// returning `Err` rather than a captured command output; this test
    /// additionally locks that the error surfaces even when a `container`
    /// resource sub-config is present (i.e. a fully-populated config still
    /// hard-errors on a missing binary).
    #[tokio::test]
    async fn unavailable_hard_errors_with_full_config() {
        let cfg = ironhermes_core::config::TerminalConfig {
            backend: "docker".to_string(),
            container_runtime: "definitely-absent-hermes-exec-test-binary-xyz".to_string(),
            forward_env: vec!["SOME_VAR".to_string()],
            ..Default::default()
        };

        let result = create_environment(&cfg, "test-task", "test-profile").await;
        assert!(
            result.is_err(),
            "docker backend must hard-error regardless of other config fields being populated"
        );
    }

    /// D-07: the container runtime binary name is the explicit config
    /// value, verbatim — never auto-detected by probing PATH. Pure argv
    /// construction, no live spawn, so this runs identically whether
    /// `docker`/`podman` are installed on the test machine or not.
    #[test]
    fn runtime_knob_docker_and_podman_argv_program_matches_config_verbatim() {
        let docker_argv = build_exec_argv("docker", "abc123", "echo hi", true, &[], false);
        assert_eq!(
            docker_argv[0], "docker",
            "docker container_runtime must build a Command whose program is `docker`"
        );

        let podman_argv = build_exec_argv("podman", "abc123", "echo hi", true, &[], false);
        assert_eq!(
            podman_argv[0], "podman",
            "podman container_runtime must build a Command whose program is `podman`"
        );
    }

    /// D-09 §4.1 note: `-e` env args are appended to the exec argv ONLY when
    /// `login_env_injection` (i.e. `opts.login`, the session's first call)
    /// is true — subsequent calls source the snapshot instead and must
    /// carry no `-e` flags.
    #[test]
    fn exec_argv_injects_env_only_on_login_call() {
        let env_args = vec!["-e".to_string(), "FOO=bar".to_string()];

        let login_argv = build_exec_argv("docker", "cid", "echo hi", true, &env_args, false);
        assert!(login_argv.contains(&"-e".to_string()));
        assert!(login_argv.contains(&"FOO=bar".to_string()));

        let non_login_argv = build_exec_argv("docker", "cid", "echo hi", false, &env_args, false);
        assert!(
            !non_login_argv.contains(&"-e".to_string()),
            "non-login (snapshot-sourcing) calls must carry no -e flags: {non_login_argv:?}"
        );
    }

    /// D-09 §4.5: `--cap-drop ALL` + selective add-back + `no-new-privileges`
    /// + `--pids-limit`, and — per Researcher Decision: D-07 — never `--userns`.
    #[test]
    fn security_args_drops_all_caps_adds_back_minimal_set_and_excludes_userns() {
        let cfg = ironhermes_core::config::ContainerResourceConfig::default();
        let args = security_args(&cfg);

        assert!(
            args.windows(2).any(|w| w == ["--cap-drop", "ALL"]),
            "expected --cap-drop ALL, got: {args:?}"
        );
        assert!(
            args.windows(2).any(|w| w == ["--cap-add", "DAC_OVERRIDE"]),
            "expected --cap-add DAC_OVERRIDE, got: {args:?}"
        );
        assert!(
            args.windows(2).any(|w| w == ["--cap-add", "CHOWN"]),
            "expected --cap-add CHOWN, got: {args:?}"
        );
        assert!(
            args.windows(2).any(|w| w == ["--cap-add", "FOWNER"]),
            "expected --cap-add FOWNER, got: {args:?}"
        );
        assert!(
            args.contains(&"no-new-privileges".to_string()),
            "expected no-new-privileges, got: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--pids-limit" && w[1] == cfg.pids_limit.to_string()),
            "expected --pids-limit {}, got: {args:?}",
            cfg.pids_limit
        );
        assert!(
            !args.iter().any(|a| a.contains("--userns")),
            "must never pass --userns (D-07 podman note), got: {args:?}"
        );
    }

    /// `container.network: false` (the hardened default) must add
    /// `--network=none`; `network: true` must not.
    #[test]
    fn security_args_network_none_only_when_network_disabled() {
        let mut cfg = ironhermes_core::config::ContainerResourceConfig {
            network: false,
            ..Default::default()
        };
        assert!(security_args(&cfg).contains(&"--network=none".to_string()));

        cfg.network = true;
        assert!(!security_args(&cfg).contains(&"--network=none".to_string()));
    }

    /// §4.3: the exec-failure signature detector recognizes the two
    /// documented docker/podman phrasings and does not false-positive on
    /// ordinary command output.
    #[test]
    fn no_such_container_signature_detection() {
        assert!(is_no_such_container_signature(
            "Error response from daemon: No such container: abc123"
        ));
        assert!(is_no_such_container_signature(
            "Error response from daemon: Container abc123 is not running"
        ));
        assert!(is_no_such_container_signature(
            "Error: no container with name or ID \"abc123\" found"
        ));
        assert!(!is_no_such_container_signature("hello world\nexit 1\n"));
    }

    /// `sanitize_task_id_for_path` must not let a task id escape the
    /// sandboxes directory or inject unexpected path separators.
    #[test]
    fn sanitize_task_id_for_path_strips_unsafe_chars() {
        assert_eq!(sanitize_task_id_for_path("task-123_ABC"), "task-123_ABC");
        assert_eq!(sanitize_task_id_for_path("../../etc"), "______etc");
        assert_eq!(sanitize_task_id_for_path("a/b c"), "a_b_c");
    }
}

/// Integration tests exercising the real `{runtime}` binary end-to-end.
/// NOT part of the default `cargo test -p ironhermes-exec` run (they spawn
/// real containers and require a running daemon) — gated behind the
/// `docker-integration` cargo feature.
///
/// Runtime-agnostic: this dev machine lacks `docker` but has `podman`
/// (RESEARCH.md "Environment Availability"), so these tests read the
/// runtime from `HERMES_TEST_CONTAINER_RUNTIME`, defaulting to `"podman"`,
/// rather than hardcoding `docker`. Run with:
///
/// ```text
/// cargo test -p ironhermes-exec --features docker-integration -- --ignored-not-needed
/// # or, to target docker explicitly if installed:
/// HERMES_TEST_CONTAINER_RUNTIME=docker cargo test -p ironhermes-exec --features docker-integration
/// ```
///
/// These tests require a reachable container daemon (e.g. `podman machine
/// start` on macOS) and are expected to be un-runnable in this dev
/// environment (podman installed but no VM running) — they MUST compile
/// under `cargo clippy --all-features -- -D warnings` regardless.
#[cfg(all(test, feature = "docker-integration"))]
mod integration_tests {
    use std::time::Duration;

    use super::*;
    use crate::backend::{Session, create_environment, execute};

    fn test_runtime() -> String {
        std::env::var("HERMES_TEST_CONTAINER_RUNTIME").unwrap_or_else(|_| "podman".to_string())
    }

    fn base_cfg() -> ironhermes_core::config::TerminalConfig {
        ironhermes_core::config::TerminalConfig {
            backend: "docker".to_string(),
            container_runtime: test_runtime(),
            cwd: "/workspace".to_string(),
            ..Default::default()
        }
    }

    /// D-02: two `create_environment` calls for the same `(task_id,
    /// profile)` must reuse the SAME container id, not create a second one.
    #[tokio::test]
    async fn reuse_same_task_and_profile_reuses_container() {
        let cfg = base_cfg();
        let task_id = "docker-integration-reuse-task";
        let profile = "docker-integration-reuse-profile";

        let env_a = create_environment(&cfg, task_id, profile)
            .await
            .expect("first create_environment should succeed against a live daemon");
        let (id_after_a, _state) = find_reusable_container(&cfg.container_runtime, task_id, profile)
            .await
            .expect("find_reusable_container should succeed")
            .expect("a container must exist after the first create_environment call");

        let env_b = create_environment(&cfg, task_id, profile)
            .await
            .expect("second create_environment should reuse the same container");
        let (id_after_b, _state) = find_reusable_container(&cfg.container_runtime, task_id, profile)
            .await
            .expect("find_reusable_container should succeed")
            .expect("the reused container must still exist");

        assert_eq!(
            id_after_a, id_after_b,
            "D-02: two create_environment calls for the same (task_id, profile) must reuse the same container id"
        );

        drop(env_a);
        drop(env_b);
    }

    /// D-09: a `FOO_TOKEN` host var must NOT appear inside the container
    /// unless added to `forward_env`.
    #[tokio::test]
    async fn env_scrub_excludes_unlisted_secret_pattern_var() {
        // SAFETY: test-only; unique var name avoids cross-test races.
        unsafe { std::env::set_var("HERMES_DOCKER_IT_FOO_TOKEN", "should-not-leak") };

        let cfg = base_cfg();
        let mut env = create_environment(&cfg, "docker-integration-env-scrub-task", "default")
            .await
            .expect("create_environment should succeed against a live daemon");
        let mut sess = Session::new("/workspace");

        let result = execute(
            env.as_ref(),
            &mut sess,
            "printenv HERMES_DOCKER_IT_FOO_TOKEN 2>/dev/null; true",
            "/workspace",
            Duration::from_secs(10),
            None,
        )
        .await
        .expect("execute should succeed");

        assert!(
            !result.output.contains("should-not-leak"),
            "unlisted secret-pattern var leaked into container: {:?}",
            result.output
        );

        env.cleanup().await.ok();
        // SAFETY: cleanup.
        unsafe { std::env::remove_var("HERMES_DOCKER_IT_FOO_TOKEN") };
    }

    /// cwd persists across two calls through the same container (mirrors
    /// `local.rs`'s round-trip test but against a live docker/podman exec).
    #[tokio::test]
    async fn cwd_persists_across_two_calls() {
        let cfg = base_cfg();
        let mut env = create_environment(&cfg, "docker-integration-cwd-task", "default")
            .await
            .expect("create_environment should succeed against a live daemon");
        let mut sess = Session::new("/workspace");

        execute(
            env.as_ref(),
            &mut sess,
            "cd /tmp",
            "/workspace",
            Duration::from_secs(10),
            None,
        )
        .await
        .expect("cd call should succeed");

        let cwd_after = sess.cwd.clone();
        let result = execute(
            env.as_ref(),
            &mut sess,
            "pwd -P",
            &cwd_after,
            Duration::from_secs(10),
            None,
        )
        .await
        .expect("pwd call should succeed");

        assert_eq!(result.output.trim(), cwd_after);

        env.cleanup().await.ok();
    }
}
